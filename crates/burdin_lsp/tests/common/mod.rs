//! The test client (issue #375): it starts `burdin-lsp` as a child process, speaks JSON-RPC to it over the child's
//! standard input and output, answers the requests the server sends (progress tokens, configuration, capability
//! registration), and keeps the notifications it receives so a test can wait for one.
#![allow(dead_code)]

use burdin_lsp::json::Json;
use burdin_lsp::rpc;
use burdin_lsp::text::{Encoding, LineIndex, path_to_uri};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(60);

pub struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Json>,
    next_id: i64,
    /// Notifications and server requests received and not yet consumed by a wait, in arrival order.
    pub inbox: Vec<Json>,
    /// Every request the server sent, in order.
    pub server_requests: Vec<Json>,
    pub encoding: Encoding,
    /// What the client answers to `workspace/configuration`.
    pub configuration: Json,
}

/// A copy of a fixture in a fresh temporary directory, so a test may change files.
pub fn fixture(name: &str) -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    let target = fresh_dir(&name.replace('/', "-"));
    copy_dir(&source, &target);
    target.canonicalize().unwrap()
}

/// A new, empty temporary directory. The tests of one binary run in parallel in one process, and the clock is too
/// coarse on some platforms to tell two of them apart, so a counter makes the name unique.
pub fn fresh_dir(label: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let target = std::env::temp_dir().join(format!("burdin-lsp-{label}-{}-{n}-{nanos}", std::process::id()));
    assert!(!target.exists(), "{} already exists", target.display());
    std::fs::create_dir_all(&target).unwrap();
    target.canonicalize().unwrap()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            copy_dir(&path, &to.join(entry.file_name()));
        } else {
            std::fs::copy(&path, to.join(entry.file_name())).unwrap();
        }
    }
}

pub fn uri(path: &Path) -> String {
    path_to_uri(&path.canonicalize().unwrap_or_else(|_| path.to_path_buf()))
}

/// The client capabilities the tests use unless they say otherwise.
pub fn default_caps() -> Json {
    Json::parse(
        r#"{
        "window": {"workDoneProgress": true},
        "workspace": {"configuration": true, "didChangeWatchedFiles": {"dynamicRegistration": true},
                      "codeLens": {"refreshSupport": true}, "inlayHint": {"refreshSupport": true}},
        "textDocument": {"documentSymbol": {"hierarchicalDocumentSymbolSupport": true},
                         "completion": {"completionItem": {"additionalTextEditsSupport": true}}}
    }"#,
    )
    .unwrap()
}

impl Client {
    pub fn start() -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_burdin-lsp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("burdin-lsp starts");
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut input = BufReader::new(stdout);
            while let Ok(Some(Ok(message))) = rpc::read_message(&mut input) {
                if tx.send(message).is_err() {
                    return;
                }
            }
        });
        let stdin = child.stdin.take();
        Client {
            child,
            stdin,
            rx,
            next_id: 0,
            inbox: Vec::new(),
            server_requests: Vec::new(),
            encoding: Encoding::Utf16,
            configuration: Json::Null,
        }
    }

    /// Starts a server and initializes it with these workspace folders, capabilities, and options. The fast
    /// debounce keeps the tests quick.
    pub fn with(folders: &[&Path], caps: Json, options: Json) -> Client {
        let mut client = Client::start();
        let folders: Vec<Json> =
            folders.iter().map(|f| Json::obj().with("uri", uri(f)).with("name", "fixture")).collect();
        let mut options = options;
        if options.is_null() {
            options = Json::obj();
        }
        if options.at(&["diagnostics", "debounceMs"]).is_null() {
            let mut d = options.get("diagnostics").clone();
            if d.is_null() {
                d = Json::obj();
            }
            d.set("debounceMs", 10);
            options.set("diagnostics", d);
        }
        let params = Json::obj()
            .with("processId", Json::Null)
            .with("capabilities", caps)
            .with("workspaceFolders", Json::Arr(folders))
            .with("initializationOptions", options);
        let response = client.request("initialize", params);
        assert!(response.get("error").is_null(), "initialize failed: {}", response.to_text());
        if response.at(&["result", "capabilities", "positionEncoding"]).as_str() == Some("utf-8") {
            client.encoding = Encoding::Utf8;
        }
        client.notify("initialized", Json::obj());
        client
    }

    pub fn new(folders: &[&Path]) -> Client {
        Client::with(folders, default_caps(), Json::Null)
    }

    pub fn send(&mut self, message: &Json) {
        self.send_all(std::slice::from_ref(message));
    }

    /// Writes several messages with one write, so the server reads them back to back.
    pub fn send_all(&mut self, messages: &[Json]) {
        let mut buffer = Vec::new();
        for m in messages {
            rpc::write_message(&mut buffer, m).unwrap();
        }
        let stdin = self.stdin.as_mut().expect("the input is open");
        stdin.write_all(&buffer).unwrap();
        stdin.flush().unwrap();
    }

    /// Writes raw bytes (for malformed input).
    pub fn send_raw(&mut self, bytes: &[u8]) {
        let stdin = self.stdin.as_mut().expect("the input is open");
        stdin.write_all(bytes).unwrap();
        stdin.flush().unwrap();
    }

    pub fn next_id(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn notify(&mut self, method: &str, params: Json) {
        self.send(&rpc::notification(method, params));
    }

    /// Sends a request and waits for its response (the whole response message).
    pub fn request(&mut self, method: &str, params: Json) -> Json {
        let id = self.next_id();
        self.send(&rpc::request(Json::from(id), method, params));
        self.response(id)
    }

    /// The result of a request; panics on an error response.
    pub fn call(&mut self, method: &str, params: Json) -> Json {
        let response = self.request(method, params);
        assert!(response.get("error").is_null(), "{method} failed: {}", response.to_text());
        response.get("result").clone()
    }

    /// Waits for the response to request `id`, keeping everything else.
    pub fn response(&mut self, id: i64) -> Json {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let message = self.receive(deadline);
            if message.get("method").is_null() && message.get("id").as_i64() == Some(id) {
                return message;
            }
            self.keep(message);
        }
    }

    /// Waits for a response whose `id` is exactly `id` (such as `null`, for a message the server could not read).
    pub fn response_to(&mut self, id: Json) -> Json {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let message = self.receive(deadline);
            if message.get("method").is_null() && *message.get("id") == id {
                return message;
            }
            self.keep(message);
        }
    }

    fn receive(&mut self, deadline: Instant) -> Json {
        let left = deadline.saturating_duration_since(Instant::now());
        match self.rx.recv_timeout(left) {
            Ok(message) => message,
            Err(_) => panic!(
                "no message from the server in time; inbox: {:?}",
                self.inbox.iter().map(Json::to_text).collect::<Vec<_>>()
            ),
        }
    }

    /// Answers a server request, or keeps a notification.
    fn keep(&mut self, message: Json) {
        if !message.get("method").is_null() && !message.get("id").is_null() {
            let result = match message.get("method").as_str() {
                Some("workspace/configuration") => Json::Arr(vec![self.configuration.clone()]),
                _ => Json::Null,
            };
            self.server_requests.push(message.clone());
            self.send(&rpc::response(message.get("id").clone(), result));
            return;
        }
        self.inbox.push(message);
    }

    /// Waits for a notification of `method` that `pred` accepts, and removes it from the inbox.
    pub fn wait_for(&mut self, method: &str, pred: impl Fn(&Json) -> bool) -> Json {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(i) =
                self.inbox.iter().position(|m| m.get("method").as_str() == Some(method) && pred(m.get("params")))
            {
                return self.inbox.remove(i).get("params").clone();
            }
            let message = self.receive(deadline);
            self.keep(message);
        }
    }

    /// The diagnostics published for a document, waiting for a publication that `pred` accepts.
    pub fn diagnostics(&mut self, uri: &str, pred: impl Fn(&Json) -> bool) -> Vec<Json> {
        let params =
            self.wait_for("textDocument/publishDiagnostics", |p| p.get("uri").as_str() == Some(uri) && pred(p));
        params.get("diagnostics").as_array().to_vec()
    }

    /// Opens a file with its text on disk. Returns its URI and text.
    pub fn open(&mut self, path: &Path) -> (String, String) {
        let text = std::fs::read_to_string(path).unwrap();
        let uri = uri(path);
        let doc = Json::obj()
            .with("uri", uri.as_str())
            .with("languageId", "burdin")
            .with("version", 1)
            .with("text", text.as_str());
        self.notify("textDocument/didOpen", Json::obj().with("textDocument", doc));
        (uri, text)
    }

    /// Replaces a document's whole text.
    pub fn change(&mut self, uri: &str, version: i64, text: &str) {
        let params = Json::obj()
            .with("textDocument", Json::obj().with("uri", uri).with("version", version))
            .with("contentChanges", Json::Arr(vec![Json::obj().with("text", text)]));
        self.notify("textDocument/didChange", params);
    }

    /// The position of the `nth` occurrence of `needle` in `text`, plus `delta` bytes, in the negotiated encoding.
    pub fn pos(&self, text: &str, needle: &str, nth: usize, delta: usize) -> Json {
        let at =
            text.match_indices(needle).nth(nth).unwrap_or_else(|| panic!("`{needle}` #{nth} is not in the text")).0
                + delta;
        let (line, character) = LineIndex::new(text, self.encoding).position(at as u32);
        Json::obj().with("line", line).with("character", character)
    }

    /// `{textDocument, position}` params.
    pub fn at(&self, uri: &str, text: &str, needle: &str, nth: usize, delta: usize) -> Json {
        Json::obj()
            .with("textDocument", Json::obj().with("uri", uri))
            .with("position", self.pos(text, needle, nth, delta))
    }

    /// Shuts the server down and returns its exit code.
    pub fn shutdown(mut self) -> i32 {
        let response = self.request("shutdown", Json::Null);
        assert!(response.get("error").is_null());
        self.notify("exit", Json::Null);
        self.stdin = None;
        self.wait_exit()
    }

    pub fn close_input(&mut self) {
        self.stdin = None;
    }

    pub fn wait_exit(&mut self) -> i32 {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code().unwrap_or(-1);
            }
            assert!(Instant::now() < deadline, "the server did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Replaces the fixture's root (as a URI and as a path) with `ROOT` and the materialized standard library with `STD`
/// everywhere in a value, and writes paths under them with `/`, so golden files do not depend on where the fixture was
/// copied or on the platform.
pub fn normalize(value: &Json, root: &Path) -> Json {
    let prefix = uri(root);
    let path = burdin_query::package::source_path_text(root);
    let fix = |s: &str| -> String {
        let mut s = s.replace(&prefix, "ROOT").replace(&path, "ROOT");
        if let Some(i) = s.find("burdin-lsp-std-") {
            let start = s[..i].rfind(['"', ' ']).map_or(0, |j| j + 1);
            let end = s[i..].find(['/', '\\']).map_or(s.len(), |j| i + j);
            s.replace_range(start..end, "STD");
        }
        if s.contains("ROOT") || s.contains("STD") {
            s = s.replace('\\', "/");
        }
        s
    };
    match value {
        Json::Str(s) => Json::Str(fix(s)),
        Json::Arr(items) => Json::Arr(items.iter().map(|v| normalize(v, root)).collect()),
        Json::Obj(fields) => Json::Obj(fields.iter().map(|(k, v)| (fix(k), normalize(v, root))).collect()),
        other => other.clone(),
    }
}

/// Compares a value with `tests/golden/<name>.json`; `BURDIN_LSP_UPDATE=1` rewrites the file.
pub fn golden(name: &str, value: &Json) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(format!("{name}.json"));
    let text = format!("{}\n", value.to_pretty());
    if std::env::var("BURDIN_LSP_UPDATE").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden file {}; run with BURDIN_LSP_UPDATE=1", path.display()));
    assert_eq!(expected.replace("\r\n", "\n"), text, "golden file {} differs", path.display());
}

/// The text of an LSP range in a text.
pub fn slice(text: &str, range: &Json, encoding: Encoding) -> String {
    let index = LineIndex::new(text, encoding);
    let (lo, hi) = index.offsets(range).unwrap();
    text[lo as usize..hi as usize].to_string()
}

/// `{textDocument: {uri}}` params.
pub fn doc(uri: &str) -> Json {
    Json::obj().with("textDocument", Json::obj().with("uri", uri))
}

/// Applies LSP text edits to a text (edits must not overlap).
pub fn apply(text: &str, edits: &[Json], encoding: Encoding) -> String {
    let index = LineIndex::new(text, encoding);
    let mut spans: Vec<(u32, u32, String)> = edits
        .iter()
        .map(|e| {
            let (lo, hi) = index.offsets(e.get("range")).unwrap();
            (lo, hi, e.get("newText").as_str().unwrap().to_string())
        })
        .collect();
    spans.sort_by_key(|s| std::cmp::Reverse(s.0));
    let mut out = text.to_string();
    for (lo, hi, new) in spans {
        out.replace_range(lo as usize..hi as usize, &new);
    }
    out
}

/// The `(line, character)` of an LSP position.
pub fn line_char(position: &Json) -> (u32, u32) {
    (position.get("line").as_u32().unwrap(), position.get("character").as_u32().unwrap())
}

/// Reads a file of a fixture copy as text.
pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}
