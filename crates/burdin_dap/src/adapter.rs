//! The adapter loop and its four augmentations (`spec/debug.md`, DBG11).
//!
//! Every message is forwarded unchanged except at four points:
//!
//! * `launch` and `attach` — a `target` (and an optional `test`) is built with `-g` first, and the forwarded
//!   request names the executable and the `preRunCommands` that import the lldb formatters (`launch.rs`);
//! * the panic filter — the `initialize` response advertises it, and `setExceptionBreakpoints` carrying it sets a
//!   function breakpoint on `burdin_panic_begin` (DBG1), merged with the client's own function breakpoints;
//! * stepping — a step that stops in generated code is stepped out of and never reported;
//! * ids — the adapter's own requests use a `seq` above a million and their responses are consumed, so a client's
//!   request and the adapter's never collide.
//!
//! DAP correlates a response with its request through `request_seq`, which names the request's `seq`, so the
//! adapter numbers what it forwards to the backend and remembers the client's id for each.

use crate::backend::Backend;
use crate::launch;
use burdin_json::json::Json;
use burdin_json::rpc;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};

/// The exception filter the adapter adds to `initialize`: the client's "Burdin panic" breakpoint.
pub(crate) const PANIC_FILTER: &str = "panic";
/// The function the panic filter breaks on (`spec/debug.md`, DBG1).
pub(crate) const PANIC_SYMBOL: &str = "burdin_panic_begin";
/// Where the adapter's own `seq` numbers start; their responses are consumed, never forwarded.
const OWN_SEQ_BASE: i64 = 1_000_000;

/// What the adapter talks to behind it: a debugger process (`backend.rs`), or a scripted peer that
/// `tests/protocol.rs` drives, so the augmentations are checked without a debugger installed.
pub trait Peer {
    fn send(&mut self, message: &Json) -> Result<(), String>;
}

impl Peer for Backend {
    fn send(&mut self, message: &Json) -> Result<(), String> {
        Backend::send(self, message).map_err(|e| format!("cannot write to the debugger backend: {e}"))
    }
}

/// A request waiting for the backend's response.
struct Pending {
    /// The client's request id (`request_seq` echoes it).
    id: Json,
    command: String,
}

/// The adapter's session.
struct Session {
    /// The directory this session writes into: the lldb formatters, and a `target`'s build.
    dir: PathBuf,
    /// The `seq` the next message to the backend carries.
    next_seq: i64,
    /// The `seq` the next message to the client carries.
    next_client_seq: i64,
    /// Where each forwarded request waits, by the `seq` the backend saw.
    pending: HashMap<i64, Pending>,
    /// The next id for a request the adapter makes on its own; also its `seq`.
    next_own: i64,
    /// The `stackTrace` the adapter asked for while a stop is held, when there is one.
    pending_stack: Option<i64>,
    /// A `stopped` event held back while the adapter decides whether the stop is Burdin code.
    held_stop: Option<Json>,
    /// The thread the held stop belongs to.
    held_thread: i64,
    /// The client's `setFunctionBreakpoints` list, which the panic filter joins.
    client_functions: Vec<Json>,
    /// Whether the panic filter is on.
    panic_filter: bool,
    /// Whether the adapter stepped out of a stop and is waiting for the next one.
    stepping_out: bool,
}

impl Session {
    fn new(dir: PathBuf) -> Session {
        Session {
            dir,
            next_seq: 1,
            next_client_seq: 1,
            pending: HashMap::new(),
            next_own: OWN_SEQ_BASE,
            pending_stack: None,
            held_stop: None,
            held_thread: 0,
            client_functions: Vec::new(),
            panic_filter: false,
            stepping_out: false,
        }
    }

    fn write_client(&mut self, mut message: Json, out: &mut impl Write) -> Result<(), String> {
        message.set("seq", self.next_client_seq);
        self.next_client_seq += 1;
        rpc::write_message(out, &message).map_err(|e| format!("cannot write to the client: {e}"))
    }

    fn write_backend(&mut self, backend: &mut dyn Peer, mut message: Json) -> Result<i64, String> {
        let seq = self.next_seq;
        self.next_seq += 1;
        message.set("seq", seq);
        backend.send(&message)?;
        Ok(seq)
    }

    /// A request the adapter sends on its own, whose response it consumes: a `seq` above a million.
    fn own_request(&mut self, backend: &mut dyn Peer, command: &str, arguments: Json) -> Result<i64, String> {
        let id = self.next_own;
        self.next_own += 1;
        let message = Json::obj()
            .with("seq", id)
            .with("type", "request")
            .with("command", command)
            .with("arguments", arguments)
            .with("id", id);
        backend.send(&message)?;
        Ok(id)
    }

    /// A response the adapter composes itself, for a request it does not forward.
    fn answer(&mut self, id: &Json, command: &str, body: Json, out: &mut impl Write) -> Result<(), String> {
        let message = Json::obj()
            .with("type", "response")
            .with("request_seq", id.clone())
            .with("success", true)
            .with("command", command)
            .with("body", body);
        self.write_client(message, out)
    }
}

/// Runs the adapter: DAP on `input` and `output`, lldb-dap (or `BURDIN_DAP_BACKEND`) behind it. Returns the
/// backend's exit code.
pub fn serve(input: Box<dyn Read + Send>, output: Box<dyn Write>) -> i32 {
    let (program, args) = match crate::backend::backend_program() {
        Ok(found) => found,
        Err(message) => {
            eprintln!("error: {message}");
            return 1;
        }
    };
    let (mut backend, incoming) = match Backend::start(&program, &args) {
        Ok(started) => started,
        Err(message) => {
            eprintln!("error: {message}");
            return 1;
        }
    };
    serve_with(input, output, &mut backend, &incoming);
    backend.finish().unwrap_or(0)
}

/// Runs the adapter against an already-started peer whose messages arrive on `incoming`: the process backend in
/// `serve`, and a scripted peer in `tests/protocol.rs`.
pub fn serve_with(
    input: Box<dyn Read + Send>,
    output: Box<dyn Write>,
    backend: &mut dyn Peer,
    incoming: &Receiver<Json>,
) -> i32 {
    let mut output = output;
    let dir = std::env::temp_dir().join(format!("burdin-dap-{}", std::process::id()));
    let mut session = Session::new(dir.clone());
    if let Err(message) = pump(&mut session, backend, incoming, input, &mut output) {
        eprintln!("error: {message}");
    }
    let _ = std::fs::remove_dir_all(&dir);
    0
}

/// The loop: the client's messages and the backend's are interleaved as they arrive, so a follow-up the adapter
/// makes (`stackTrace`) can be answered while the client is quiet.
fn pump(
    session: &mut Session,
    backend: &mut dyn Peer,
    incoming: &Receiver<Json>,
    input: Box<dyn Read + Send>,
    output: &mut impl Write,
) -> Result<(), String> {
    let client = read_client(input);
    loop {
        // Both sides are drained before the loop waits, so a burst goes through in order.
        let mut idle = true;
        while let Ok(message) = incoming.try_recv() {
            idle = false;
            deliver(session, backend, message, output)?;
        }
        while let Ok(message) = client.try_recv() {
            idle = false;
            forward(session, backend, message, output)?;
            while let Ok(message) = incoming.try_recv() {
                deliver(session, backend, message, output)?;
            }
        }
        if idle {
            match incoming.recv_timeout(std::time::Duration::from_millis(10)) {
                Ok(message) => deliver(session, backend, message, output)?,
                Err(RecvTimeoutError::Timeout) => match client.try_recv() {
                    Ok(message) => forward(session, backend, message, output)?,
                    // The client closing its input ends the session, as it would for lldb-dap.
                    Err(TryRecvError::Disconnected) => return Ok(()),
                    Err(TryRecvError::Empty) => {}
                },
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
    }
}

/// Reads the client's messages on a thread of its own.
fn read_client(input: Box<dyn Read + Send>) -> Receiver<Json> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut input = std::io::BufReader::new(input);
        while let Ok(Some(message)) = rpc::read_message(&mut input) {
            let Ok(message) = message else { break };
            if sender.send(message).is_err() {
                break;
            }
        }
    });
    receiver
}

/// A client message: forwarded, rewritten, or answered by the adapter.
fn forward(
    session: &mut Session,
    backend: &mut dyn Peer,
    mut message: Json,
    out: &mut impl Write,
) -> Result<(), String> {
    let kind = message.get("type").as_str().unwrap_or_default().to_string();
    if kind != "request" {
        session.write_backend(backend, message)?;
        return Ok(());
    }
    let command = message.get("command").as_str().unwrap_or_default().to_string();
    let id = message.get("id").clone();
    match command.as_str() {
        "launch" | "attach" => {
            let Some(prepared) = launch::prepare(message.get("arguments"), &session.dir)? else {
                let seq = session.write_backend(backend, message)?;
                session.pending.insert(seq, Pending { id, command });
                return Ok(());
            };
            // `target` and `test` are the adapter's own fields: the backend only understands the executable.
            {
                let mut arguments = message.get("arguments").clone();
                arguments.remove("target");
                arguments.remove("test");
                message.set("arguments", arguments);
            }
            if let Some(program) = prepared.program {
                message.set_at(&["arguments", "program"], program.display().to_string());
            }
            if !prepared.args.is_empty() {
                message.set_at(&["arguments", "args"], Json::Arr(prepared.args.into_iter().map(Json::Str).collect()));
            }
            let mut commands: Vec<Json> = message.at(&["arguments", "preRunCommands"]).as_array().to_vec();
            commands.extend(prepared.pre_run.into_iter().map(Json::Str));
            if !commands.is_empty() {
                message.set_at(&["arguments", "preRunCommands"], Json::Arr(commands));
            }
            let seq = session.write_backend(backend, message)?;
            session.pending.insert(seq, Pending { id, command });
            Ok(())
        }
        "setExceptionBreakpoints" => {
            let filters = message.at(&["arguments", "filters"]).as_array().to_vec();
            session.panic_filter = filters.iter().any(|filter| filter.as_str() == Some(PANIC_FILTER));
            session.send_function_breakpoints(backend)?;
            let body = Json::obj().with(
                "breakpoints",
                Json::Arr(
                    filters
                        .into_iter()
                        .map(|filter| Json::obj().with("verified", true).with("filter", filter))
                        .collect(),
                ),
            );
            session.answer(&id, &command, body, out)
        }
        "setFunctionBreakpoints" => {
            session.client_functions = message.at(&["arguments", "breakpoints"]).as_array().to_vec();
            session.send_function_breakpoints(backend)?;
            let count = session.client_functions.len();
            let body = Json::obj()
                .with("breakpoints", Json::Arr((0..count).map(|_| Json::obj().with("verified", true)).collect()));
            session.answer(&id, &command, body, out)
        }
        _ => {
            let seq = session.write_backend(backend, message)?;
            session.pending.insert(seq, Pending { id, command });
            Ok(())
        }
    }
}

impl Session {
    /// Sends the backend the merged function breakpoints: the client's own, plus the panic hook when the filter is
    /// on.
    fn send_function_breakpoints(&mut self, backend: &mut dyn Peer) -> Result<(), String> {
        let mut breakpoints = self.client_functions.clone();
        if self.panic_filter {
            breakpoints.push(Json::obj().with("name", PANIC_SYMBOL));
        }
        self.own_request(backend, "setFunctionBreakpoints", Json::obj().with("breakpoints", Json::Arr(breakpoints)))?;
        Ok(())
    }
}

/// A backend message: a response goes back to the request that caused it (or is consumed), an event is forwarded,
/// except a step that stops in generated code.
fn deliver(
    session: &mut Session,
    backend: &mut dyn Peer,
    mut message: Json,
    out: &mut impl Write,
) -> Result<(), String> {
    match message.get("type").as_str().unwrap_or_default() {
        "response" => {
            let request_seq = message.get("request_seq").as_i64().unwrap_or(0);
            if request_seq >= OWN_SEQ_BASE {
                if Some(request_seq) == session.pending_stack {
                    session.pending_stack = None;
                    let frames = message.at(&["body", "stackFrames"]).as_array().to_vec();
                    return resolve_held_stop(session, backend, &frames, out);
                }
                return Ok(());
            }
            let Some(pending) = session.pending.remove(&request_seq) else { return Ok(()) };
            message.set("request_seq", pending.id);
            if pending.command == "initialize" {
                // The panic filter is the adapter's, so it is advertised here rather than by the backend.
                let mut filters = message.at(&["body", "exceptionBreakpointFilters"]).as_array().to_vec();
                if filters.iter().all(|filter| filter.get("filter").as_str() != Some(PANIC_FILTER)) {
                    filters.push(
                        Json::obj()
                            .with("filter", PANIC_FILTER)
                            .with("label", "Burdin panic")
                            .with("default", false)
                            .with("description", "Stop in `burdin_panic_begin`, before a panic prints"),
                    );
                }
                message.set_at(&["body", "exceptionBreakpointFilters"], Json::Arr(filters));
            }
            session.write_client(message, out)
        }
        "event" => {
            let name = message.get("event").as_str().unwrap_or_default().to_string();
            if name != "stopped" {
                return session.write_client(message, out);
            }
            let reason = message.at(&["body", "reason"]).as_str().unwrap_or_default().to_string();
            if reason != "step" {
                // A breakpoint, a pause, a panic: the client sees the stop as it is.
                return session.write_client(message, out);
            }
            // A step may land in generated code. Hold the event and ask for the top frame; the response decides.
            session.held_thread = message.at(&["body", "threadId"]).as_i64().unwrap_or(1);
            session.held_stop = Some(message);
            let id = session.own_request(backend, "stackTrace", Json::obj().with("threadId", session.held_thread))?;
            session.pending_stack = Some(id);
            Ok(())
        }
        _ => session.write_client(message, out),
    }
}

/// What the held stop is: Burdin code, which the client sees, or generated code, which the adapter steps out of
/// without reporting it (`spec/debug.md`, DBG11).
fn resolve_held_stop(
    session: &mut Session,
    backend: &mut dyn Peer,
    frames: &[Json],
    out: &mut impl Write,
) -> Result<(), String> {
    let Some(stop) = session.held_stop.take() else { return Ok(()) };
    if frames.first().is_some_and(is_burdin_frame) {
        session.stepping_out = false;
        return session.write_client(stop, out);
    }
    session.stepping_out = true;
    session.own_request(backend, "stepOut", Json::obj().with("threadId", session.held_thread))?;
    Ok(())
}

/// Whether a stack frame is code the user wrote: a Burdin source file and a name that is not generated drop, clone,
/// closure, runtime, codegen-unit, or vtable code (`spec/debug.md`, DBG11).
fn is_burdin_frame(frame: &Json) -> bool {
    let name = frame.get("name").as_str().unwrap_or_default();
    let path = frame.at(&["source", "path"]).as_str().unwrap_or_default();
    let generated = ["burdin.drop.", "burdin.clone.", "burdin.closure.", "burdin_rt_", "burdin.cgu.", "burdin.vtable."];
    path.ends_with(".bd") && !generated.iter().any(|prefix| name.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(name: &str, path: &str) -> Json {
        Json::obj().with("name", name).with("source", Json::obj().with("path", path))
    }

    #[test]
    fn burdin_frames_are_the_users_code() {
        assert!(is_burdin_frame(&frame("app.load", "/src/app.bd")));
        assert!(!is_burdin_frame(&frame("burdin.drop.app.Item", "/src/app.bd")));
        assert!(!is_burdin_frame(&frame("burdin_rt_panic", "/src/app.bd")));
        assert!(!is_burdin_frame(&frame("app.load", "")));
    }
}
