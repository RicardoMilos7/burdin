//! JSON-RPC 2.0 framing over a byte stream (`spec/lsp.md`, LS1; the DAP server uses the same framing, `spec/debug.md`,
//! DBG11): each message is a `Content-Length` header, an empty line, and that many bytes of UTF-8 JSON.

use crate::json::Json;
use std::io::{BufRead, Write};

/// The JSON-RPC error codes the server sends.
pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    pub const SERVER_NOT_INITIALIZED: i64 = -32002;
    pub const REQUEST_FAILED: i64 = -32803;
    pub const REQUEST_CANCELLED: i64 = -32800;
    pub const CONTENT_MODIFIED: i64 = -32801;
}

/// Reads one message. `Ok(None)` is the end of the stream; an error is a framing problem the server reports and
/// survives when it can (a body that is not JSON) or stops on (a stream that is not LSP at all).
pub fn read_message(input: &mut impl BufRead) -> Result<Option<Result<Json, String>>, String> {
    let mut length: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        let n = input.read_line(&mut line).map_err(|e| format!("cannot read a message header: {e}"))?;
        if n == 0 {
            return Ok(None);
        }
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            if length.is_some() {
                break;
            }
            // Blank lines between messages are tolerated.
            continue;
        }
        let Some((name, value)) = header.split_once(':') else {
            return Err(format!("malformed message header `{header}`"));
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            length = Some(value.trim().parse().map_err(|_| format!("invalid Content-Length `{}`", value.trim()))?);
        }
    }
    let length = length.unwrap_or(0);
    let mut body = vec![0u8; length];
    input.read_exact(&mut body).map_err(|e| format!("cannot read a message body: {e}"))?;
    Ok(Some(match String::from_utf8(body) {
        Ok(text) => Json::parse(&text),
        Err(_) => Err("the message body is not UTF-8".to_string()),
    }))
}

/// Writes one message and flushes it.
pub fn write_message(out: &mut impl Write, message: &Json) -> std::io::Result<()> {
    let body = message.to_text();
    write!(out, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    out.flush()
}

/// A request or notification id: a number or a string.
pub fn id_key(id: &Json) -> String {
    id.to_text()
}

pub fn response(id: Json, result: Json) -> Json {
    Json::obj().with("jsonrpc", "2.0").with("id", id).with("result", result)
}

pub fn error_response(id: Json, code: i64, message: impl Into<String>) -> Json {
    let error = Json::obj().with("code", code).with("message", message.into());
    Json::obj().with("jsonrpc", "2.0").with("id", id).with("error", error)
}

pub fn notification(method: &str, params: Json) -> Json {
    Json::obj().with("jsonrpc", "2.0").with("method", method).with("params", params)
}

pub fn request(id: Json, method: &str, params: Json) -> Json {
    Json::obj().with("jsonrpc", "2.0").with("id", id).with("method", method).with("params", params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_round_trips_through_the_framing() {
        let mut buf = Vec::new();
        write_message(&mut buf, &notification("x", Json::obj().with("é", "😀"))).unwrap();
        let mut input = std::io::Cursor::new(buf);
        let message = read_message(&mut input).unwrap().unwrap().unwrap();
        assert_eq!(message.get("method").as_str(), Some("x"));
        assert_eq!(message.at(&["params", "é"]).as_str(), Some("😀"));
        assert!(read_message(&mut input).unwrap().is_none());
    }

    #[test]
    fn a_body_that_is_not_json_is_reported_and_the_stream_continues() {
        let text = "Content-Length: 3\r\n\r\n{x}Content-Length: 2\r\n\r\n[]";
        let mut input = std::io::Cursor::new(text.as_bytes().to_vec());
        assert!(read_message(&mut input).unwrap().unwrap().is_err());
        assert_eq!(read_message(&mut input).unwrap().unwrap().unwrap(), Json::Arr(Vec::new()));
    }
}
