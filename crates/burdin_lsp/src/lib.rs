//! The Burdin language server (`spec/lsp.md`): LSP over JSON-RPC on standard input and output, answered from the
//! query-based compiler library (`burdin_query`). It is synchronous and single-threaded around the databases, which
//! are not thread-safe; a reader thread takes messages off the input so a `$/cancelRequest` or an edit can stop the
//! analysis in progress, and test runs happen on worker threads.
//!
//! ```text
//! json, rpc      the protocol: a small JSON value and Content-Length framing
//! text           positions (UTF-8 and UTF-16) and file URIs
//! project        one query database per package root; which package owns a file
//! index          what a file declares, from its syntax tree and tokens
//! server         the loop, lifecycle, documents, folders, configuration, cancellation, progress
//! diagnostics    the compiler's and the linter's diagnostics, published as files change
//! nav            definition, references, rename, implementation, call and type hierarchies
//! edit           hover, completion, signature help, inlay hints, semantic tokens, formatting
//! outline        document and workspace symbols, folding, selection ranges, document links
//! actions        code actions, code lenses, and the test commands
//! ```

mod actions;
mod diagnostics;
mod edit;
mod index;
pub mod json;
mod nav;
mod outline;
mod project;
pub mod rpc;
mod server;
pub mod text;

pub use edit::{TOKEN_MODIFIERS, TOKEN_TYPES};
pub use server::serve;

/// Runs the server on the process's standard input and output. Returns the exit code.
pub fn serve_stdio() -> i32 {
    serve(Box::new(std::io::stdin()), Box::new(std::io::stdout()))
}
