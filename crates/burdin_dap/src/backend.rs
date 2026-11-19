//! The backend: lldb-dap (or `BURDIN_DAP_BACKEND`) as a child process (`spec/debug.md`, DBG11).
//!
//! The child's standard input is written from the adapter's loop and its standard output is read on a thread that
//! hands whole messages to the loop, so a response the adapter is waiting for can never block the client's input.

use burdin_json::json::Json;
use burdin_json::rpc;
use std::io::{BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Receiver;

/// The backend child and its two ends.
pub(crate) struct Backend {
    child: Child,
    stdin: ChildStdin,
}

impl Backend {
    /// Starts the backend on the command line `program` and the arguments in `args`. Messages come back on the
    /// returned receiver; a closed receiver means the backend exited.
    pub(crate) fn start(program: &str, args: &[String]) -> Result<(Backend, Receiver<Json>), String> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("cannot start the debugger backend `{program}`: {e}"))?;
        let stdin = child.stdin.take().expect("the backend's input is piped");
        let mut stdout = BufReader::new(child.stdout.take().expect("the backend's output is piped"));
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // The loop ends when the backend's output closes; a framing error is a broken backend, which the
            // loop reports by dropping the receiver.
            while let Ok(Some(message)) = rpc::read_message(&mut stdout) {
                let Ok(message) = message else { break };
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        Ok((Backend { child, stdin }, receiver))
    }

    /// Writes one message to the backend.
    pub(crate) fn send(&mut self, message: &Json) -> std::io::Result<()> {
        rpc::write_message(&mut self.stdin, message)
    }

    /// Ends the backend and its own exit status.
    pub(crate) fn finish(mut self) -> Option<i32> {
        let _ = self.stdin.flush();
        drop(self.stdin);
        self.child.wait().ok().and_then(|status| status.code())
    }
}

/// The backend's command line: `BURDIN_DAP_BACKEND` when set (the tests use it to run a stub), otherwise the
/// `lldb-dap` of the LLVM installation, found like the driver finds LLVM's other tools.
pub(crate) fn backend_program() -> Result<(String, Vec<String>), String> {
    if let Ok(named) = std::env::var("BURDIN_DAP_BACKEND")
        && !named.is_empty()
    {
        return Ok((named, Vec::new()));
    }
    for candidate in ["lldb-dap", "lldb-dap-21", "lldb-vscode"] {
        if on_path(candidate) {
            return Ok((candidate.to_string(), Vec::new()));
        }
    }
    Err("cannot find `lldb-dap`: install LLVM 21 (its `bin` must be on PATH) or set BURDIN_DAP_BACKEND".to_string())
}

fn on_path(program: &str) -> bool {
    let paths = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(program);
        candidate.is_file() || candidate.with_extension("exe").is_file()
    })
}
