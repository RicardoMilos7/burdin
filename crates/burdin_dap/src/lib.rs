//! The Debug Adapter Protocol server (`spec/debug.md`, DBG11).
//!
//! `burdin dap` speaks DAP on standard input and output and forwards to lldb-dap (or the program
//! `BURDIN_DAP_BACKEND` names), adding Burdin behavior at four points and forwarding everything else unchanged:
//!
//! ```text
//! backend   the child process: its command line, its reader thread, and the message channel
//! launch    compiling a `target` the launch request names, and the lldb formatters it loads
//! adapter   the loop and the four augmentations: launch, the panic filter, glue-skipping steps, and ids
//! ```
//!
//! The adapter never interprets the program: lldb-dap is the debugger engine, and a Burdin-specific feature is a
//! rewrite of a message, not a second implementation of one.

mod adapter;
mod backend;
mod launch;

pub use adapter::{Peer, serve, serve_with};

/// The driver a `target` is built with, when the process that started the adapter is the driver itself.
static DRIVER: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Runs the adapter on the process's standard input and output. Returns the exit code.
pub fn serve_stdio() -> i32 {
    serve(Box::new(std::io::stdin()), Box::new(std::io::stdout()))
}

/// Runs the adapter with `driver` as the compiler a launch request's `target` is built with: `burdin dap` passes
/// its own executable, so the adapter always builds with the compiler that shipped with it.
pub fn serve_stdio_with_driver(driver: impl Into<std::path::PathBuf>) -> i32 {
    let _ = DRIVER.set(driver.into());
    serve_stdio()
}

/// The compiler the adapter builds a `target` with (`launch.rs`).
pub(crate) fn driver() -> String {
    DRIVER
        .get()
        .map(|path| path.display().to_string())
        .or_else(|| std::env::var("BURDIN_DAP_DRIVER").ok().filter(|path| !path.is_empty()))
        .unwrap_or_else(|| "burdin".to_string())
}
