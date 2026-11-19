//! The adapter's four augmentations, driven against a scripted peer (`spec/debug.md`, DBG11).
//!
//! The peer stands in for lldb-dap: it records what the adapter sends it and replies with responses and events a
//! real backend would send. That is enough to check the id bookkeeping, the panic filter, and the glue-skipping
//! step without a debugger installed; `tests/check_dap.py` runs the same behavior through `burdin dap`.

use burdin_dap::{Peer, serve_with};
use burdin_json::json::Json;
use burdin_json::rpc;
use std::collections::VecDeque;
use std::io::{Cursor, Write};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// The messages the adapter sent the peer, in order.
type Sent = Arc<Mutex<Vec<Json>>>;

/// A peer that answers every request the way lldb-dap would, with the stack frames a scenario names.
struct Scripted {
    replies: Sender<Json>,
    sent: Sent,
    /// The frames the next `stackTrace` answers with; popped from the front.
    frames: VecDeque<Vec<Json>>,
}

impl Scripted {
    fn reply(&self, message: &Json, body: Json) {
        let response = Json::obj()
            .with("type", "response")
            .with("seq", 1)
            .with("request_seq", message.get("seq").clone())
            .with("success", true)
            .with("command", message.get("command").clone())
            .with("body", body);
        let _ = self.replies.send(response);
    }

    fn event(&self, event: &str, body: Json) {
        let _ =
            self.replies.send(Json::obj().with("type", "event").with("seq", 1).with("event", event).with("body", body));
    }
}

impl Peer for Scripted {
    fn send(&mut self, message: &Json) -> Result<(), String> {
        self.sent.lock().unwrap().push(message.clone());
        match message.get("command").as_str().unwrap_or_default() {
            // The client's initialize, as lldb-dap answers it: one filter, which the adapter augments.
            "initialize" => {
                let body = Json::obj().with(
                    "exceptionBreakpointFilters",
                    Json::Arr(vec![Json::obj().with("filter", "stackFrame").with("label", "Stack frame")]),
                );
                self.reply(message, body);
            }
            "setFunctionBreakpoints" => {
                let body = Json::obj().with(
                    "breakpoints",
                    Json::Arr(
                        message
                            .at(&["arguments", "breakpoints"])
                            .as_array()
                            .iter()
                            .map(|_| Json::obj().with("verified", true))
                            .collect(),
                    ),
                );
                self.reply(message, body);
            }
            // The adapter's own stack-trace follow-up: the scenario says what the top frame is.
            "stackTrace" => {
                let frames = self.frames.pop_front().unwrap_or_default();
                self.reply(message, Json::obj().with("stackFrames", Json::Arr(frames)).with("totalFrames", 1));
            }
            "stepOut" => {
                self.reply(message, Json::obj());
                // Stepping out lands on the caller, which is Burdin code.
                self.event("stopped", Json::obj().with("reason", "step").with("threadId", 1));
            }
            "configurationDone" => {
                self.reply(message, Json::obj());
                // The program starts and the first step lands in generated code.
                self.event("stopped", Json::obj().with("reason", "step").with("threadId", 1));
            }
            _ => self.reply(message, Json::obj()),
        }
        Ok(())
    }
}

/// Output written by the adapter, to be read back as messages.
#[derive(Clone)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn frame(name: &str, path: &str) -> Json {
    Json::obj()
        .with("id", 1)
        .with("name", name)
        .with("line", 1)
        .with("column", 1)
        .with("source", Json::obj().with("path", path))
}

/// The messages the client sends, framed as DAP writes them.
fn client_input() -> Vec<u8> {
    let messages = [
        Json::obj()
            .with("seq", 1)
            .with("type", "request")
            .with("id", 1)
            .with("command", "initialize")
            .with("arguments", Json::obj()),
        Json::obj()
            .with("seq", 2)
            .with("type", "request")
            .with("id", 2)
            .with("command", "setExceptionBreakpoints")
            .with("arguments", Json::obj().with("filters", Json::Arr(vec![Json::Str("panic".into())]))),
        Json::obj()
            .with("seq", 3)
            .with("type", "request")
            .with("id", 3)
            .with("command", "setFunctionBreakpoints")
            .with("arguments", Json::obj().with("breakpoints", Json::Arr(vec![Json::obj().with("name", "main")]))),
        Json::obj()
            .with("seq", 4)
            .with("type", "request")
            .with("id", 4)
            .with("command", "configurationDone")
            .with("arguments", Json::obj()),
    ];
    let mut bytes = Vec::new();
    for message in &messages {
        rpc::write_message(&mut bytes, message).unwrap();
    }
    bytes
}

#[test]
fn the_adapter_augments_and_forwards() {
    let (replies, incoming): (Sender<Json>, Receiver<Json>) = channel();
    let sent: Sent = Arc::default();
    let output = Shared(Arc::default());
    let mut peer = Scripted {
        replies,
        sent: sent.clone(),
        frames: VecDeque::from(vec![
            // The first step stops in drop glue.
            vec![frame("burdin.drop.app.Item", "/src/app.bd")],
            // Stepping out lands in the user's function.
            vec![frame("app.load", "/src/app.bd")],
        ]),
    };
    let sink = output.clone();
    let exit = serve_with(Box::new(Cursor::new(client_input())), Box::new(sink), &mut peer, &incoming);
    assert_eq!(exit, 0);

    let messages: Vec<Json> = {
        let bytes = output.0.lock().unwrap().clone();
        let mut input = Cursor::new(bytes);
        let mut messages = Vec::new();
        while let Ok(Some(message)) = rpc::read_message(&mut input) {
            messages.push(message.unwrap());
        }
        messages
    };
    let sent = sent.lock().unwrap().clone();

    // The panic filter is advertised on top of the backend's own filters.
    let initialize = messages
        .iter()
        .find(|m| m.get("type").as_str() == Some("response") && m.get("command").as_str() == Some("initialize"))
        .expect("the initialize response reaches the client");
    let filters = initialize.at(&["body", "exceptionBreakpointFilters"]).as_array();
    assert!(filters.iter().any(|f| f.get("filter").as_str() == Some("stackFrame")), "{filters:?}");
    assert!(filters.iter().any(|f| f.get("filter").as_str() == Some("panic")), "{filters:?}");

    // `setExceptionBreakpoints` and `setFunctionBreakpoints` answer the client and set one merged list on the
    // backend: the client's breakpoints and the panic hook.
    let responses: Vec<&str> = messages
        .iter()
        .filter(|m| m.get("type").as_str() == Some("response"))
        .filter_map(|m| m.get("command").as_str())
        .collect();
    assert!(responses.contains(&"setExceptionBreakpoints"), "{responses:?}");
    assert!(responses.contains(&"setFunctionBreakpoints"), "{responses:?}");
    let merged = sent
        .iter()
        .rfind(|m| m.get("command").as_str() == Some("setFunctionBreakpoints"))
        .expect("the adapter sets function breakpoints");
    let names: Vec<&str> =
        merged.at(&["arguments", "breakpoints"]).as_array().iter().filter_map(|b| b.get("name").as_str()).collect();
    assert_eq!(names, ["main", "burdin_panic_begin"]);

    // The step into generated code is stepped out of and never reported; the next stop is.
    assert!(sent.iter().any(|m| m.get("command").as_str() == Some("stepOut")), "the adapter did not step out");
    let stops: Vec<&Json> = messages.iter().filter(|m| m.get("event").as_str() == Some("stopped")).collect();
    assert_eq!(stops.len(), 1, "exactly the stop in Burdin code is reported: {stops:?}");

    // Every message the client sees is numbered by the adapter itself.
    let seqs: Vec<u8> = messages.iter().filter_map(|m| m.get("seq").as_u32().map(|n| n as u8)).collect();
    assert_eq!(seqs, (1..=seqs.len() as u8).collect::<Vec<_>>(), "{seqs:?}");
}

#[test]
fn a_response_reaches_the_client_with_its_own_id() {
    let (replies, incoming) = channel();
    let sent: Sent = Arc::default();
    let output = Shared(Arc::default());
    let mut peer = Scripted { replies, sent, frames: VecDeque::new() };
    let sink = output.clone();
    let input = {
        let mut bytes = Vec::new();
        rpc::write_message(
            &mut bytes,
            &Json::obj()
                .with("seq", 7)
                .with("type", "request")
                .with("id", 42)
                .with("command", "threads")
                .with("arguments", Json::obj()),
        )
        .unwrap();
        bytes
    };
    serve_with(Box::new(Cursor::new(input)), Box::new(sink), &mut peer, &incoming);
    let bytes = output.0.lock().unwrap().clone();
    let mut reader = Cursor::new(bytes);
    let response = rpc::read_message(&mut reader).unwrap().unwrap().unwrap();
    assert_eq!(response.get("type").as_str(), Some("response"));
    assert_eq!(response.get("command").as_str(), Some("threads"));
    assert_eq!(response.get("request_seq").as_i64(), Some(42));
}
