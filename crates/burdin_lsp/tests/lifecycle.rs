//! The transport and the lifecycle (issues #309, #365, #373, #375; `spec/lsp.md` LS1-LS3, LS6): initialize and its
//! capabilities, shutdown and exit codes, errors for requests out of order or unknown, malformed input, the position
//! encoding, and cancellation.

mod common;

use burdin_lsp::json::Json;
use burdin_lsp::rpc;
use burdin_lsp::text::Encoding;
use common::*;

fn initialize_params(caps: Json) -> Json {
    Json::obj().with("processId", Json::Null).with("capabilities", caps).with("workspaceFolders", Json::Null)
}

#[test]
fn initialize_announces_the_capabilities() {
    let mut client = Client::start();
    let mut result = client.call("initialize", initialize_params(default_caps()));
    assert_eq!(result.at(&["serverInfo", "name"]).as_str(), Some("burdin"));
    // The version changes with every release; the golden file keeps the rest.
    let mut info = result.get("serverInfo").clone();
    info.set("version", "VERSION");
    result.set("serverInfo", info);
    golden("capabilities", &result);
    client.notify("initialized", Json::obj());
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn exit_after_shutdown_is_0_and_without_it_is_1() {
    let client = Client::new(&[]);
    assert_eq!(client.shutdown(), 0);

    let mut client = Client::new(&[]);
    client.notify("exit", Json::Null);
    assert_eq!(client.wait_exit(), 1);

    // The input closing without `exit` ends the server too, as a failure.
    let mut client = Client::new(&[]);
    client.close_input();
    assert_eq!(client.wait_exit(), 1);
}

#[test]
fn requests_out_of_order_and_unknown_methods_are_errors() {
    let mut client = Client::start();
    let early = client.request("textDocument/hover", Json::obj());
    assert_eq!(early.at(&["error", "code"]).as_i64(), Some(rpc::code::SERVER_NOT_INITIALIZED));

    client.call("initialize", initialize_params(default_caps()));
    let again = client.request("initialize", initialize_params(default_caps()));
    assert_eq!(again.at(&["error", "code"]).as_i64(), Some(rpc::code::INVALID_REQUEST));
    client.notify("initialized", Json::obj());

    let unknown = client.request("burdin/nothing", Json::obj());
    assert_eq!(unknown.at(&["error", "code"]).as_i64(), Some(rpc::code::METHOD_NOT_FOUND));
    // An unknown notification is ignored, and a `$/` request is answered with an error, never left hanging.
    client.notify("$/somethingElse", Json::obj());
    let dollar = client.request("$/somethingElse", Json::obj());
    assert_eq!(dollar.at(&["error", "code"]).as_i64(), Some(rpc::code::METHOD_NOT_FOUND));

    client.call("shutdown", Json::Null);
    let late = client.request("textDocument/hover", Json::obj());
    assert_eq!(late.at(&["error", "code"]).as_i64(), Some(rpc::code::INVALID_REQUEST));
    client.notify("exit", Json::Null);
    assert_eq!(client.wait_exit(), 0);
}

#[test]
fn malformed_messages_get_a_parse_error_and_the_server_goes_on() {
    let mut client = Client::new(&[]);
    let body = b"{\"jsonrpc\": \"2.0\", \"id\": 1, \"method\": ";
    let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    bytes.extend_from_slice(body);
    client.send_raw(&bytes);
    // A message that is JSON but not a message.
    let body = b"[1, 2]";
    let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    bytes.extend_from_slice(body);
    client.send_raw(&bytes);
    let first = client.response_to(Json::Null);
    assert_eq!(first.at(&["error", "code"]).as_i64(), Some(rpc::code::PARSE_ERROR), "{}", first.to_text());
    let second = client.response_to(Json::Null);
    assert_eq!(second.at(&["error", "code"]).as_i64(), Some(rpc::code::INVALID_REQUEST), "{}", second.to_text());
    // Still alive.
    let status = client.call("burdin/status", Json::Null);
    assert_eq!(status.get("positionEncoding").as_str(), Some("utf-16"));
    assert_eq!(client.shutdown(), 0);
}

/// The definition of `sum` from its use after `😀` (two UTF-16 units, four bytes) and `á` (one unit, two bytes), in
/// both encodings: the client and the server must count the same way to land on the name.
#[test]
fn positions_count_utf16_units_or_utf8_bytes_as_negotiated() {
    for (offer, encoding) in
        [(None, Encoding::Utf16), (Some("utf-8"), Encoding::Utf8), (Some("utf-16"), Encoding::Utf16)]
    {
        let root = fixture("geometry");
        let mut caps = default_caps();
        if let Some(offer) = offer {
            caps.set("general", Json::obj().with("positionEncodings", Json::Arr(vec![Json::from(offer)])));
        }
        let mut client = Client::with(&[&root], caps, Json::Null);
        assert_eq!(client.encoding, encoding);
        let main = root.join("src/main.bd");
        let (uri, text) = client.open(&main);
        let position = client.pos(&text, "{sum}", 0, 1);
        let line = text.lines().position(|l| l.contains("{sum}")).unwrap() as u32;
        let column = text.lines().nth(line as usize).unwrap().find("{sum}").unwrap() + 1;
        assert_eq!(line_char(&position).0, line);
        let utf16_column = text.lines().nth(line as usize).unwrap()[..column].encode_utf16().count();
        assert_eq!(
            line_char(&position).1 as usize,
            if encoding == Encoding::Utf8 { column } else { utf16_column },
            "the harness counts in the negotiated encoding"
        );
        let definition = client.call("textDocument/definition", doc(&uri).with("position", position));
        assert_eq!(slice(&text, definition.get("range"), encoding), "sum");
        assert_eq!(
            line_char(definition.at(&["range", "start"])).0,
            text.lines().position(|l| l.contains("let mut sum")).unwrap() as u32
        );
        // The hover's range goes back the same way.
        let hover = client.call("textDocument/hover", client.at(&uri, &text, "{sum}", 0, 1));
        assert_eq!(slice(&text, hover.get("range"), encoding), "sum");
        assert_eq!(client.shutdown(), 0);
    }
}

#[test]
fn a_cancelled_request_answers_request_cancelled_and_the_next_one_succeeds() {
    let root = fixture("geometry");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    // An edit makes the next request analyze the package again; the cancellation arrives before or during that work.
    let edited = text.replace("sum = add(sum, 1)", "sum = add(sum, 2)");
    let change = rpc::notification(
        "textDocument/didChange",
        Json::obj()
            .with("textDocument", Json::obj().with("uri", uri.as_str()).with("version", 2))
            .with("contentChanges", Json::Arr(vec![Json::obj().with("text", edited.as_str())])),
    );
    let id = client.next_id();
    let hover = rpc::request(Json::from(id), "textDocument/hover", client.at(&uri, &edited, "total(sq", 0, 0));
    let cancel = rpc::notification("$/cancelRequest", Json::obj().with("id", id));
    client.send_all(&[change, hover, cancel]);
    let response = client.response(id);
    assert_eq!(response.at(&["error", "code"]).as_i64(), Some(rpc::code::REQUEST_CANCELLED), "{}", response.to_text());

    let again = client.call("textDocument/hover", client.at(&uri, &edited, "total(sq", 0, 0));
    assert!(again.at(&["contents", "value"]).as_str().unwrap().contains("fn total(a: i32, b: i32): i32"));
    // Cancelling a request that has already been answered changes nothing.
    client.notify("$/cancelRequest", Json::obj().with("id", id));
    let status = client.call("burdin/status", Json::Null);
    assert!(status.get("packages").as_array().len() == 1);
    assert_eq!(client.shutdown(), 0);
}
