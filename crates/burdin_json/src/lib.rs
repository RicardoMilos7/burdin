//! The JSON value and the `Content-Length` framing of the two protocol servers Burdin ships: the language server
//! (`spec/lsp.md`, LS1) and the Debug Adapter Protocol server (`spec/debug.md`, DBG11). `burdin_lsp` re-exports both
//! modules, so the server's own paths are unchanged.
//!
//! ```text
//! json    a value with a parser and a printer; objects keep their insertion order
//! rpc     JSON-RPC 2.0 messages over a byte stream, framed by `Content-Length`
//! ```

pub mod json;
pub mod rpc;
