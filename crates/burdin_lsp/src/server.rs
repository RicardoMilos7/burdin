//! The server: the message loop, the lifecycle, documents, workspace folders, configuration, cancellation, and
//! progress (`spec/lsp.md`, LS2 and LS4-LS6). Feature requests are answered in the sibling modules.

use crate::json::Json;
use crate::project::{self, Loc, Package};
use crate::rpc::{self, code};
use crate::text::{self, Encoding, LineIndex};
use burdin_query::CancelHandle;
use std::collections::{HashMap, HashSet};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The commands the server runs itself (`workspace/executeCommand`).
pub const SERVER_COMMANDS: &[&str] = &["burdin.runTest", "burdin.runTests"];

/// Work a request hands to a worker thread; the response is sent when it ends.
pub type Job = Box<dyn FnOnce() -> Result<crate::actions::RunOutcome, String> + Send>;

/// What a request failed with, or the work it left for a worker thread.
pub struct ReqError {
    pub code: i64,
    pub message: String,
    pub deferred: Option<Job>,
}

impl std::fmt::Debug for ReqError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ReqError({}, {:?})", self.code, self.message)
    }
}

impl ReqError {
    pub fn new(code: i64, message: impl Into<String>) -> ReqError {
        ReqError { code, message: message.into(), deferred: None }
    }

    /// The request is answered when `job` finishes on a worker thread.
    pub fn deferred(job: Job) -> ReqError {
        ReqError { code: 0, message: String::new(), deferred: Some(job) }
    }

    pub fn cancelled() -> ReqError {
        ReqError::new(code::REQUEST_CANCELLED, "the request was cancelled")
    }

    pub fn params(message: impl Into<String>) -> ReqError {
        ReqError::new(code::INVALID_PARAMS, message)
    }

    pub fn failed(message: impl Into<String>) -> ReqError {
        ReqError::new(code::REQUEST_FAILED, message)
    }
}

pub type Reply = Result<Json, ReqError>;

/// The settings under `burdin` (`initializationOptions` or `workspace/didChangeConfiguration`), LS5.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub inlay_types: bool,
    pub inlay_parameters: bool,
    pub lints: bool,
    pub debounce_ms: u64,
    pub formatting: bool,
    pub code_lens: bool,
    /// The `burdin` command the test lenses run; empty means this executable when it is `burdin`, else `burdin` on
    /// `PATH`.
    pub path: String,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            inlay_types: true,
            inlay_parameters: true,
            lints: true,
            debounce_ms: 200,
            formatting: true,
            code_lens: true,
            path: String::new(),
        }
    }
}

impl Config {
    /// The configuration with the settings in `value` applied; keys that are absent keep their value.
    pub fn apply(&self, value: &Json) -> Config {
        let mut c = self.clone();
        let flag = |v: &Json, old: bool| v.as_bool().unwrap_or(old);
        c.inlay_types = flag(value.at(&["inlayHints", "types"]), c.inlay_types);
        c.inlay_parameters = flag(value.at(&["inlayHints", "parameterNames"]), c.inlay_parameters);
        c.lints = flag(value.at(&["diagnostics", "lints"]), c.lints);
        if let Some(ms) = value.at(&["diagnostics", "debounceMs"]).as_u32() {
            c.debounce_ms = ms.min(10_000) as u64;
        }
        c.formatting = flag(value.at(&["formatting", "enabled"]), c.formatting);
        c.code_lens = flag(value.at(&["codeLens", "enabled"]), c.code_lens);
        if let Some(path) = value.get("path").as_str() {
            c.path = path.to_string();
        }
        c
    }
}

/// What the client said it can do, as far as the server cares.
#[derive(Default)]
pub struct ClientCaps {
    pub progress: bool,
    pub configuration: bool,
    pub watched_files: bool,
    pub hierarchical_symbols: bool,
    pub code_lens_refresh: bool,
    pub inlay_hint_refresh: bool,
}

/// An open document.
pub struct Doc {
    pub uri: String,
    pub text: String,
    pub version: i64,
}

/// The state shared with the reader thread: which requests were cancelled, and how to stop the analysis running now.
#[derive(Default)]
pub struct Shared {
    cancelled: Mutex<HashSet<String>>,
    current: Mutex<Current>,
}

#[derive(Default)]
struct Current {
    /// The request being answered, or `None` for background work.
    id: Option<String>,
    /// Background work (diagnostics), which an edit cancels.
    background: bool,
    handle: Option<CancelHandle>,
}

enum Msg {
    Json(Json),
    Bad(String),
    Eof,
    /// A request's deferred work finished.
    Done {
        id: Json,
        token: Option<String>,
        outcome: Result<crate::actions::RunOutcome, String>,
    },
}

/// The result of a test the lenses show.
#[derive(Clone, Debug)]
pub struct TestResult {
    pub passed: bool,
    pub duration_ms: u64,
    pub reason: Option<String>,
}

/// A request the server sent to the client, waiting for its response.
enum Pending {
    Configuration,
    Ignore,
}

pub struct Server {
    out: Box<dyn Write>,
    tx: Sender<Msg>,
    pub encoding: Encoding,
    pub packages: Vec<Package>,
    pub docs: HashMap<String, Doc>,
    pub config: Config,
    pub caps: ClientCaps,
    folders: Vec<PathBuf>,
    initialized: bool,
    shutdown: bool,
    next_id: u64,
    pending: HashMap<String, Pending>,
    shared: Arc<Shared>,
    /// Test results by `(package root, test path)`.
    pub tests: HashMap<(PathBuf, String), TestResult>,
    /// The revision of each package's analysis whose diagnostics were published, by root.
    published: HashMap<PathBuf, u64>,
    /// Package roots that failed to load, with the message; the error is shown on the open files under them.
    broken: HashMap<PathBuf, String>,
}

/// Runs the server over a byte stream until `exit` or the end of the input. Returns the process exit code: 0 after
/// `shutdown` then `exit`, 1 otherwise (as the protocol asks).
pub fn serve(input: Box<dyn Read + Send>, output: Box<dyn Write>) -> i32 {
    let (tx, rx) = mpsc::channel();
    let shared = Arc::new(Shared::default());
    let reader_shared = shared.clone();
    let reader_tx = tx.clone();
    std::thread::spawn(move || reader(input, reader_tx, reader_shared));
    let mut server = Server::new(output, tx, shared);
    server.run(rx)
}

/// Reads messages and hands them to the main thread. A `$/cancelRequest` is acted on here, while the main thread may
/// be busy with the request it cancels; so is an edit, which cancels the background analysis it makes stale.
fn reader(input: Box<dyn Read + Send>, tx: Sender<Msg>, shared: Arc<Shared>) {
    let mut input = BufReader::new(input);
    loop {
        match rpc::read_message(&mut input) {
            Ok(Some(Ok(message))) => {
                match message.get("method").as_str() {
                    Some("$/cancelRequest") => {
                        let key = rpc::id_key(message.at(&["params", "id"]));
                        shared.cancelled.lock().unwrap().insert(key.clone());
                        let current = shared.current.lock().unwrap();
                        if current.id.as_deref() == Some(key.as_str())
                            && let Some(handle) = &current.handle
                        {
                            handle.cancel();
                        }
                    }
                    Some(
                        "textDocument/didChange"
                        | "textDocument/didOpen"
                        | "textDocument/didClose"
                        | "workspace/didChangeWatchedFiles",
                    ) => {
                        let current = shared.current.lock().unwrap();
                        if current.background
                            && let Some(handle) = &current.handle
                        {
                            handle.cancel();
                        }
                    }
                    _ => {}
                }
                if tx.send(Msg::Json(message)).is_err() {
                    return;
                }
            }
            Ok(Some(Err(error))) => {
                if tx.send(Msg::Bad(error)).is_err() {
                    return;
                }
            }
            Ok(None) | Err(_) => {
                let _ = tx.send(Msg::Eof);
                return;
            }
        }
    }
}

impl Server {
    fn new(out: Box<dyn Write>, tx: Sender<Msg>, shared: Arc<Shared>) -> Server {
        Server {
            out,
            tx,
            encoding: Encoding::Utf16,
            packages: Vec::new(),
            docs: HashMap::new(),
            config: Config::default(),
            caps: ClientCaps::default(),
            folders: Vec::new(),
            initialized: false,
            shutdown: false,
            next_id: 0,
            pending: HashMap::new(),
            shared,
            tests: HashMap::new(),
            published: HashMap::new(),
            broken: HashMap::new(),
        }
    }

    fn run(&mut self, rx: Receiver<Msg>) -> i32 {
        loop {
            let msg = if self.has_background_work() {
                match rx.recv_timeout(Duration::from_millis(self.config.debounce_ms)) {
                    Ok(m) => m,
                    Err(RecvTimeoutError::Timeout) => {
                        self.background();
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => Msg::Eof,
                }
            } else {
                rx.recv().unwrap_or(Msg::Eof)
            };
            match msg {
                Msg::Json(message) => {
                    if let Some(code) = self.handle(message) {
                        return code;
                    }
                }
                Msg::Bad(error) => self.send(&rpc::error_response(Json::Null, code::PARSE_ERROR, error)),
                Msg::Done { id, token, outcome } => {
                    self.progress_end(token, "done");
                    match crate::actions::finish_run(self, outcome) {
                        Ok(result) => self.send(&rpc::response(id, result)),
                        Err(e) => self.send(&rpc::error_response(id, e.code, e.message)),
                    }
                }
                Msg::Eof => return if self.shutdown { 0 } else { 1 },
            }
        }
    }

    // ---- output -------------------------------------------------------------------------------------------------

    pub fn send(&mut self, message: &Json) {
        // A client that went away is noticed when the input ends.
        let _ = rpc::write_message(&mut self.out, message);
    }

    pub fn notify(&mut self, method: &str, params: Json) {
        self.send(&rpc::notification(method, params));
    }

    fn request_client(&mut self, method: &str, params: Json, pending: Pending) {
        self.next_id += 1;
        let id = format!("burdin-{}", self.next_id);
        self.pending.insert(rpc::id_key(&Json::Str(id.clone())), pending);
        self.send(&rpc::request(Json::Str(id), method, params));
    }

    /// Asks the client to request the code lenses again (after a test run).
    pub fn refresh_lenses(&mut self) {
        if self.caps.code_lens_refresh {
            self.request_client("workspace/codeLens/refresh", Json::Null, Pending::Ignore);
        }
    }

    pub fn log(&mut self, message: impl Into<String>) {
        self.notify("window/logMessage", Json::obj().with("type", 3).with("message", message.into()));
    }

    pub fn show(&mut self, kind: u32, message: impl Into<String>) {
        self.notify("window/showMessage", Json::obj().with("type", kind).with("message", message.into()));
    }

    /// Starts a `$/progress` report and returns its token, when the client supports progress.
    pub fn progress_begin(&mut self, title: &str, message: &str) -> Option<String> {
        if !self.caps.progress {
            return None;
        }
        self.next_id += 1;
        let token = format!("burdin-progress-{}", self.next_id);
        self.request_client(
            "window/workDoneProgress/create",
            Json::obj().with("token", token.as_str()),
            Pending::Ignore,
        );
        let value = Json::obj().with("kind", "begin").with("title", title).with("message", message);
        self.notify("$/progress", Json::obj().with("token", token.as_str()).with("value", value));
        Some(token)
    }

    pub fn progress_end(&mut self, token: Option<String>, message: &str) {
        if let Some(token) = token {
            let value = Json::obj().with("kind", "end").with("message", message);
            self.notify("$/progress", Json::obj().with("token", token).with("value", value));
        }
    }

    // ---- dispatch -----------------------------------------------------------------------------------------------

    /// Handles one message. Returns the exit code when the server must stop.
    fn handle(&mut self, message: Json) -> Option<i32> {
        if !matches!(message, Json::Obj(_)) {
            self.send(&rpc::error_response(Json::Null, code::INVALID_REQUEST, "a message must be a JSON object"));
            return None;
        }
        let method = message.get("method").as_str().map(str::to_string);
        let id = message.get("id").clone();
        let params = message.get("params").clone();
        match method {
            None => {
                // A response to a request the server sent.
                if let Some(pending) = self.pending.remove(&rpc::id_key(&id))
                    && let Pending::Configuration = pending
                {
                    let settings = message.get("result").as_array().first().cloned().unwrap_or(Json::Null);
                    self.apply_config(&settings);
                }
                None
            }
            Some(method) if !id.is_null() => {
                self.request(id, &method, &params);
                None
            }
            Some(method) => self.notification(&method, &params),
        }
    }

    fn request(&mut self, id: Json, method: &str, params: &Json) {
        let key = rpc::id_key(&id);
        if !self.initialized && method != "initialize" {
            self.send(&rpc::error_response(id, code::SERVER_NOT_INITIALIZED, "the server is not initialized"));
            return;
        }
        if self.shutdown {
            self.send(&rpc::error_response(id, code::INVALID_REQUEST, "the server is shutting down"));
            return;
        }
        if self.shared.cancelled.lock().unwrap().remove(&key) {
            self.send(&rpc::error_response(id, code::REQUEST_CANCELLED, "the request was cancelled"));
            return;
        }
        *self.shared.current.lock().unwrap() = Current { id: Some(key.clone()), background: false, handle: None };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.dispatch(method, params)));
        let handle = std::mem::take(&mut *self.shared.current.lock().unwrap()).handle;
        if let Some(handle) = handle {
            handle.reset();
        }
        self.shared.cancelled.lock().unwrap().remove(&key);
        let reply = match outcome {
            Ok(reply) => reply,
            Err(panic) => {
                let what = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                self.log(format!("internal error in `{method}`: {what}"));
                // The databases may be half-updated; start them again from the documents.
                self.reload_all();
                Err(ReqError::new(code::INTERNAL_ERROR, format!("internal error: {what}")))
            }
        };
        match reply {
            Ok(result) => self.send(&rpc::response(id, result)),
            Err(ReqError { deferred: Some(job), .. }) => {
                let token = self.progress_begin("burdin test", "running tests");
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let outcome = job();
                    let _ = tx.send(Msg::Done { id, token, outcome });
                });
            }
            Err(e) => self.send(&rpc::error_response(id, e.code, e.message)),
        }
    }

    fn dispatch(&mut self, method: &str, params: &Json) -> Reply {
        use crate::{actions, edit, nav, outline};
        match method {
            "initialize" => self.initialize(params),
            "shutdown" => {
                self.shutdown = true;
                Ok(Json::Null)
            }
            "textDocument/hover" => edit::hover(self, params),
            "textDocument/completion" => edit::completion(self, params),
            "textDocument/signatureHelp" => edit::signature_help(self, params),
            "textDocument/inlayHint" => edit::inlay_hints(self, params),
            "textDocument/semanticTokens/full" => edit::semantic_tokens(self, params, false),
            "textDocument/semanticTokens/range" => edit::semantic_tokens(self, params, true),
            "textDocument/formatting" => edit::formatting(self, params, false),
            "textDocument/rangeFormatting" => edit::formatting(self, params, true),
            "textDocument/definition" | "textDocument/declaration" => nav::definition(self, params),
            "textDocument/implementation" => nav::implementation(self, params),
            "textDocument/references" => nav::references(self, params),
            "textDocument/documentHighlight" => nav::highlight(self, params),
            "textDocument/prepareRename" => nav::prepare_rename(self, params),
            "textDocument/rename" => nav::rename(self, params),
            "textDocument/prepareCallHierarchy" => nav::prepare_call_hierarchy(self, params),
            "callHierarchy/incomingCalls" => nav::incoming_calls(self, params),
            "callHierarchy/outgoingCalls" => nav::outgoing_calls(self, params),
            "textDocument/prepareTypeHierarchy" => nav::prepare_type_hierarchy(self, params),
            "typeHierarchy/supertypes" => nav::type_hierarchy(self, params, true),
            "typeHierarchy/subtypes" => nav::type_hierarchy(self, params, false),
            "textDocument/documentSymbol" => outline::document_symbols(self, params),
            "workspace/symbol" => outline::workspace_symbols(self, params),
            "textDocument/foldingRange" => outline::folding_ranges(self, params),
            "textDocument/selectionRange" => outline::selection_ranges(self, params),
            "textDocument/documentLink" => outline::document_links(self, params),
            "textDocument/codeAction" => actions::code_actions(self, params),
            "textDocument/codeLens" => actions::code_lenses(self, params),
            "workspace/executeCommand" => actions::execute_command(self, params),
            "burdin/status" => Ok(self.status()),
            _ => Err(ReqError::new(code::METHOD_NOT_FOUND, format!("unknown method `{method}`"))),
        }
    }

    fn notification(&mut self, method: &str, params: &Json) -> Option<i32> {
        if method == "exit" {
            return Some(if self.shutdown { 0 } else { 1 });
        }
        if !self.initialized {
            return None;
        }
        match method {
            "initialized" => self.on_initialized(),
            "textDocument/didOpen" => self.did_open(params),
            "textDocument/didChange" => self.did_change(params),
            "textDocument/didClose" => self.did_close(params),
            "textDocument/didSave" => {}
            "workspace/didChangeConfiguration" => {
                let settings = params.at(&["settings", "burdin"]);
                if !settings.is_null() {
                    let settings = settings.clone();
                    self.apply_config(&settings);
                } else if self.caps.configuration {
                    let items = Json::Arr(vec![Json::obj().with("section", "burdin")]);
                    self.request_client(
                        "workspace/configuration",
                        Json::obj().with("items", items),
                        Pending::Configuration,
                    );
                }
            }
            "workspace/didChangeWorkspaceFolders" => self.did_change_folders(params),
            "workspace/didChangeWatchedFiles" => self.did_change_watched(params),
            _ => {}
        }
        None
    }

    // ---- lifecycle ----------------------------------------------------------------------------------------------

    fn initialize(&mut self, params: &Json) -> Reply {
        if self.initialized {
            return Err(ReqError::new(code::INVALID_REQUEST, "the server is already initialized"));
        }
        let caps = params.get("capabilities");
        let encodings: Vec<&str> =
            caps.at(&["general", "positionEncodings"]).as_array().iter().filter_map(Json::as_str).collect();
        self.encoding = if encodings.contains(&"utf-8") { Encoding::Utf8 } else { Encoding::Utf16 };
        let flag = |path: &[&str]| caps.at(path).as_bool().unwrap_or(false);
        self.caps = ClientCaps {
            progress: flag(&["window", "workDoneProgress"]),
            configuration: flag(&["workspace", "configuration"]),
            watched_files: flag(&["workspace", "didChangeWatchedFiles", "dynamicRegistration"]),
            hierarchical_symbols: flag(&["textDocument", "documentSymbol", "hierarchicalDocumentSymbolSupport"]),
            code_lens_refresh: flag(&["workspace", "codeLens", "refreshSupport"]),
            inlay_hint_refresh: flag(&["workspace", "inlayHint", "refreshSupport"]),
        };
        self.config = self.config.apply(params.get("initializationOptions"));
        let mut folders: Vec<PathBuf> = params
            .get("workspaceFolders")
            .as_array()
            .iter()
            .filter_map(|f| f.get("uri").as_str().and_then(text::uri_to_path))
            .collect();
        if folders.is_empty()
            && let Some(root) = params.get("rootUri").as_str().and_then(text::uri_to_path)
        {
            folders.push(root);
        }
        self.folders = folders.into_iter().map(|f| f.canonicalize().unwrap_or(f)).collect();
        self.initialized = true;
        Ok(Json::obj()
            .with("capabilities", self.capabilities())
            .with("serverInfo", Json::obj().with("name", "burdin").with("version", env!("CARGO_PKG_VERSION"))))
    }

    fn capabilities(&self) -> Json {
        let strings = |items: &[&str]| Json::Arr(items.iter().map(|s| Json::from(*s)).collect());
        Json::obj()
            .with("positionEncoding", self.encoding.name())
            .with(
                "textDocumentSync",
                Json::obj()
                    .with("openClose", true)
                    .with("change", 2)
                    .with("save", Json::obj().with("includeText", false)),
            )
            .with("hoverProvider", true)
            .with("definitionProvider", true)
            .with("declarationProvider", true)
            .with("implementationProvider", true)
            .with("referencesProvider", true)
            .with("documentHighlightProvider", true)
            .with("renameProvider", Json::obj().with("prepareProvider", true))
            .with(
                "completionProvider",
                Json::obj().with("triggerCharacters", strings(&["."])).with("resolveProvider", false),
            )
            .with(
                "signatureHelpProvider",
                Json::obj()
                    .with("triggerCharacters", strings(&["(", ","]))
                    .with("retriggerCharacters", strings(&[","])),
            )
            .with("documentFormattingProvider", true)
            .with("documentRangeFormattingProvider", true)
            .with("documentSymbolProvider", true)
            .with("workspaceSymbolProvider", true)
            .with(
                "codeActionProvider",
                Json::obj()
                    .with("codeActionKinds", strings(&["quickfix", "refactor.rewrite", "source.organizeImports"])),
            )
            .with("codeLensProvider", Json::obj().with("resolveProvider", false))
            .with("executeCommandProvider", Json::obj().with("commands", strings(SERVER_COMMANDS)))
            .with(
                "semanticTokensProvider",
                Json::obj()
                    .with(
                        "legend",
                        Json::obj()
                            .with("tokenTypes", strings(crate::edit::TOKEN_TYPES))
                            .with("tokenModifiers", strings(crate::edit::TOKEN_MODIFIERS)),
                    )
                    .with("full", true)
                    .with("range", true),
            )
            .with("inlayHintProvider", true)
            .with("foldingRangeProvider", true)
            .with("selectionRangeProvider", true)
            .with("documentLinkProvider", Json::obj().with("resolveProvider", false))
            .with("callHierarchyProvider", true)
            .with("typeHierarchyProvider", true)
            .with(
                "workspace",
                Json::obj()
                    .with("workspaceFolders", Json::obj().with("supported", true).with("changeNotifications", true)),
            )
    }

    fn on_initialized(&mut self) {
        if self.caps.watched_files {
            let watchers = Json::Arr(vec![
                Json::obj().with("globPattern", "**/*.bd"),
                Json::obj().with("globPattern", "**/burdin.toml"),
            ]);
            let registration = Json::obj()
                .with("id", "burdin-watched-files")
                .with("method", "workspace/didChangeWatchedFiles")
                .with("registerOptions", Json::obj().with("watchers", watchers));
            self.request_client(
                "client/registerCapability",
                Json::obj().with("registrations", Json::Arr(vec![registration])),
                Pending::Ignore,
            );
        }
        let folders = self.folders.clone();
        for folder in folders {
            self.load_folder(&folder);
        }
    }

    fn load_folder(&mut self, folder: &Path) {
        for root in project::discover(folder) {
            if !self.packages.iter().any(|p| p.root == root) {
                self.load_package(&root);
            }
        }
    }

    /// Loads (or reloads) the package at `root`. Returns its index.
    fn load_package(&mut self, root: &Path) -> Option<usize> {
        let texts: HashMap<String, String> = self.docs.iter().map(|(k, d)| (k.clone(), d.text.clone())).collect();
        match Package::load(root, &texts) {
            Ok(mut package) => {
                self.broken.remove(root);
                // A new database counts its revisions from the start again: publish its first analysis.
                self.published.remove(&package.root);
                let existing = self.packages.iter().position(|p| p.root == root);
                match existing {
                    Some(i) => {
                        package.published = std::mem::take(&mut self.packages[i].published);
                        self.packages[i] = package;
                        Some(i)
                    }
                    None => {
                        self.packages.push(package);
                        Some(self.packages.len() - 1)
                    }
                }
            }
            Err(message) => {
                self.log(format!("cannot load {}: {message}", root.display()));
                self.broken.insert(root.to_path_buf(), message);
                if let Some(i) = self.packages.iter().position(|p| p.root == root) {
                    let old = self.packages.remove(i);
                    for path in old.published {
                        self.publish_empty(&path);
                    }
                }
                self.publish_broken();
                None
            }
        }
    }

    /// Starts every database again from the files and the open documents (after an internal error).
    fn reload_all(&mut self) {
        let roots: Vec<PathBuf> = self.packages.iter().map(|p| p.root.clone()).collect();
        for root in roots {
            self.load_package(&root);
        }
    }

    // ---- documents ----------------------------------------------------------------------------------------------

    /// The key of a document: the path the databases use (`<std>/x.bd` for a standard-library file the server
    /// wrote out).
    pub fn key_of_uri(&self, uri: &str) -> Option<String> {
        let path = text::uri_to_path(uri)?;
        if let Some(std) = project::std_path_of(&path) {
            return Some(std);
        }
        Some(project::path_key(&path))
    }

    /// The package that owns a file, loading it when needed (`spec/lsp.md`, LS4).
    pub fn owner(&mut self, path: &str) -> Option<usize> {
        if path.starts_with("<std>/") {
            return self.packages.iter().position(|p| p.has_file(path));
        }
        let file = Path::new(path);
        let root = project::root_of(file);
        if let Some(i) = self.packages.iter().position(|p| p.root == root && p.has_file(path)) {
            return Some(i);
        }
        if self.broken.contains_key(&root) && root != file {
            return None;
        }
        // A new file of a known package (or the first file of an unknown one).
        if let Some(i) = self.load_package(&root)
            && self.packages[i].has_file(path)
        {
            return Some(i);
        }
        // A file under a package that is not one of its modules (`build.bd`, a file outside `src`) is a program of
        // its own.
        let own = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
        if let Some(i) = self.packages.iter().position(|p| p.root == own) {
            return Some(i);
        }
        if self.broken.contains_key(&own) {
            return None;
        }
        if own.is_file() || self.docs.contains_key(path) {
            return self.load_package(&own).filter(|&i| self.packages[i].has_file(path));
        }
        None
    }

    fn did_open(&mut self, params: &Json) {
        let doc = params.get("textDocument");
        let (Some(uri), Some(text)) = (doc.get("uri").as_str(), doc.get("text").as_str()) else { return };
        let Some(key) = self.key_of_uri(uri) else { return };
        let version = doc.get("version").as_i64().unwrap_or(0);
        self.docs.insert(key.clone(), Doc { uri: uri.to_string(), text: text.to_string(), version });
        if key.starts_with("<std>/") {
            return;
        }
        self.set_text_everywhere(&key, text);
        // A newly opened file gets its diagnostics even when its package was analyzed before and nothing changed.
        for i in self.packages_with(&key) {
            let root = self.packages[i].root.clone();
            self.published.remove(&root);
        }
        if self.owner(&key).is_none() {
            self.publish_broken();
        }
    }

    fn did_change(&mut self, params: &Json) {
        let Some(uri) = params.at(&["textDocument", "uri"]).as_str() else { return };
        let Some(key) = self.key_of_uri(uri) else { return };
        let encoding = self.encoding;
        let Some(doc) = self.docs.get_mut(&key) else { return };
        for change in params.get("contentChanges").as_array() {
            let Some(new) = change.get("text").as_str() else { continue };
            let range = change.get("range");
            if range.is_null() {
                doc.text = new.to_string();
            } else if let Some((lo, hi)) = LineIndex::new(&doc.text, encoding).offsets(range) {
                doc.text.replace_range(lo as usize..hi as usize, new);
            }
        }
        if let Some(version) = params.at(&["textDocument", "version"]).as_i64() {
            doc.version = version;
        }
        let text = doc.text.clone();
        if !key.starts_with("<std>/") {
            self.set_text_everywhere(&key, &text);
        }
    }

    fn did_close(&mut self, params: &Json) {
        let Some(uri) = params.at(&["textDocument", "uri"]).as_str() else { return };
        let Some(key) = self.key_of_uri(uri) else { return };
        self.docs.remove(&key);
        if key.starts_with("<std>/") {
            return;
        }
        match std::fs::read_to_string(&key) {
            Ok(text) => self.set_text_everywhere(&key, &text),
            Err(_) => self.reload_containing(&key),
        }
        // A package nobody opened a file of and no workspace folder holds goes away with its diagnostics.
        let unused: Vec<usize> = (0..self.packages.len())
            .filter(|&i| {
                let p = &self.packages[i];
                !self.folders.iter().any(|f| p.root.starts_with(f))
                    && !p.own_files().iter().any(|f| self.docs.contains_key(f))
            })
            .collect();
        for i in unused.into_iter().rev() {
            let old = self.packages.remove(i);
            self.published.remove(&old.root);
            for path in old.published {
                self.publish_empty(&path);
            }
        }
    }

    /// Gives the new text of a file to every database that reads it.
    fn set_text_everywhere(&mut self, path: &str, text: &str) {
        for p in &mut self.packages {
            p.set_text(path, text);
        }
    }

    /// Reloads every package that has the file, or whose directory holds it.
    fn reload_containing(&mut self, path: &str) {
        let roots: Vec<PathBuf> = self
            .packages
            .iter()
            .filter(|p| {
                p.has_file(path) || (p.root.is_dir() && Path::new(path).starts_with(project::path_key(&p.root)))
            })
            .map(|p| p.root.clone())
            .collect();
        for root in roots {
            self.load_package(&root);
        }
    }

    fn did_change_watched(&mut self, params: &Json) {
        let mut rediscover = false;
        for change in params.get("changes").as_array() {
            let Some(path) = change.get("uri").as_str().and_then(text::uri_to_path) else { continue };
            let kind = change.get("type").as_u32().unwrap_or(2);
            let key = project::path_key(&path);
            let is_manifest = path.file_name().is_some_and(|n| n == "burdin.toml");
            if is_manifest {
                rediscover = true;
                if let Some(dir) = path.parent() {
                    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
                    let dependents: Vec<PathBuf> = self
                        .packages
                        .iter()
                        .filter(|p| p.root == dir || p.packages.iter().any(|q| q.dir.as_deref() == Some(&dir)))
                        .map(|p| p.root.clone())
                        .collect();
                    for root in dependents {
                        self.load_package(&root);
                    }
                }
            } else if kind == 2 {
                if !self.docs.contains_key(&key)
                    && let Ok(text) = std::fs::read_to_string(&path)
                {
                    self.set_text_everywhere(&key, &text);
                }
            } else {
                self.reload_containing(&key);
            }
        }
        if rediscover {
            let folders = self.folders.clone();
            for folder in folders {
                self.load_folder(&folder);
            }
        }
    }

    fn did_change_folders(&mut self, params: &Json) {
        let event = params.get("event");
        for removed in event.get("removed").as_array() {
            let Some(path) = removed.get("uri").as_str().and_then(text::uri_to_path) else { continue };
            let path = path.canonicalize().unwrap_or(path);
            self.folders.retain(|f| *f != path);
            let gone: Vec<usize> = (0..self.packages.len())
                .filter(|&i| {
                    let p = &self.packages[i];
                    p.root.starts_with(&path)
                        && !self.folders.iter().any(|f| p.root.starts_with(f))
                        && !p.own_files().iter().any(|f| self.docs.contains_key(f))
                })
                .collect();
            for i in gone.into_iter().rev() {
                let old = self.packages.remove(i);
                self.published.remove(&old.root);
                for path in old.published {
                    self.publish_empty(&path);
                }
            }
        }
        for added in event.get("added").as_array() {
            let Some(path) = added.get("uri").as_str().and_then(text::uri_to_path) else { continue };
            let path = path.canonicalize().unwrap_or(path);
            if !self.folders.contains(&path) {
                self.folders.push(path.clone());
            }
            self.load_folder(&path);
        }
    }

    // ---- configuration ------------------------------------------------------------------------------------------

    fn apply_config(&mut self, settings: &Json) {
        let new = self.config.apply(settings);
        if new == self.config {
            return;
        }
        let lints_changed = new.lints != self.config.lints;
        self.config = new;
        if lints_changed {
            // Publish again with or without the lints.
            self.published.clear();
        }
        if self.caps.inlay_hint_refresh {
            self.request_client("workspace/inlayHint/refresh", Json::Null, Pending::Ignore);
        }
        if self.caps.code_lens_refresh {
            self.request_client("workspace/codeLens/refresh", Json::Null, Pending::Ignore);
        }
    }

    // ---- analysis and diagnostics -------------------------------------------------------------------------------

    fn has_background_work(&self) -> bool {
        self.packages.iter().any(|p| {
            p.dirty
                || p.analysis.as_ref().is_some_and(|a| self.published.get(&p.root) != Some(&a.revision))
                || p.analysis.is_none()
        })
    }

    /// Analyzes the packages whose inputs changed and publishes their diagnostics. An edit that arrives meanwhile
    /// cancels the analysis; the loop comes back once the edits stop.
    fn background(&mut self) {
        for i in 0..self.packages.len() {
            let needs_analysis = self.packages[i].dirty || self.packages[i].analysis.is_none();
            if needs_analysis {
                let first = self.packages[i].analyses == 0;
                let token = if first {
                    let title = format!("Analyzing {}", self.packages[i].name);
                    self.progress_begin("burdin", &title)
                } else {
                    None
                };
                let handle = self.packages[i].db.cancel_handle();
                *self.shared.current.lock().unwrap() =
                    Current { id: None, background: true, handle: Some(handle.clone()) };
                let result = self.packages[i].analyze().map(|_| ());
                *self.shared.current.lock().unwrap() = Current::default();
                handle.reset();
                self.progress_end(token, if result.is_ok() { "done" } else { "cancelled" });
                if result.is_err() {
                    // An edit arrived; it is handled first, and the loop comes back here.
                    return;
                }
            }
            let revision = self.packages[i].analysis.as_ref().map(|a| a.revision);
            if revision.is_some() && self.published.get(&self.packages[i].root).copied() != revision {
                self.publish(i);
                self.published.insert(self.packages[i].root.clone(), revision.unwrap_or(0));
            }
        }
    }

    /// Analyzes a package for a request. The request's cancellation stops it.
    pub fn analyzed(&mut self, i: usize) -> Result<(), ReqError> {
        if !self.packages[i].dirty && self.packages[i].analysis.is_some() {
            return Ok(());
        }
        let handle = self.packages[i].db.cancel_handle();
        {
            let mut current = self.shared.current.lock().unwrap();
            if let Some(id) = &current.id
                && self.shared.cancelled.lock().unwrap().contains(id)
            {
                return Err(ReqError::cancelled());
            }
            current.handle = Some(handle);
        }
        self.packages[i].analyze().map(|_| ()).map_err(|_| ReqError::cancelled())
    }

    /// Publishes the diagnostics of the files a package owns.
    fn publish(&mut self, i: usize) {
        let lints = self.config.lints;
        let encoding = self.encoding;
        let files = self.packages[i].own_files();
        let by_file = crate::diagnostics::collect(&mut self.packages[i], lints);
        let paths: Vec<String> = self.packages[i].db.file_paths().iter().map(|p| p.to_string()).collect();
        let mut now_published = Vec::new();
        for path in &files {
            let diags = by_file.get(path).cloned().unwrap_or_default();
            let was = self.packages[i].published.contains(path);
            if diags.is_empty() && !was && !self.docs.contains_key(path) {
                continue;
            }
            let Some(text) = self.packages[i].text(path).map(str::to_string) else { continue };
            let index = LineIndex::new(&text, encoding);
            let items: Vec<Json> = diags.iter().map(|d| crate::diagnostics::to_json(self, d, &index, &paths)).collect();
            if !items.is_empty() {
                now_published.push(path.clone());
            }
            let uri = self.uri_of(path);
            let mut params = Json::obj().with("uri", uri).with("diagnostics", Json::Arr(items));
            if let Some(doc) = self.docs.get(path) {
                params.set("version", doc.version);
            }
            self.notify("textDocument/publishDiagnostics", params);
        }
        self.packages[i].published = now_published;
    }

    fn publish_empty(&mut self, path: &str) {
        let uri = self.uri_of(path);
        self.notify(
            "textDocument/publishDiagnostics",
            Json::obj().with("uri", uri).with("diagnostics", Json::Arr(vec![])),
        );
    }

    /// Shows a package that failed to load on its open files: one error at the start of each.
    fn publish_broken(&mut self) {
        let mut out = Vec::new();
        for (path, doc) in &self.docs {
            let root = project::root_of(Path::new(path));
            if let Some(message) = self.broken.get(&root) {
                let diagnostic = Json::obj()
                    .with("range", text::range_json(0, 0, 0, 0))
                    .with("severity", 1)
                    .with("source", "burdin")
                    .with("message", format!("cannot load the package: {message}"));
                out.push(
                    Json::obj()
                        .with("uri", doc.uri.as_str())
                        .with("version", doc.version)
                        .with("diagnostics", Json::Arr(vec![diagnostic])),
                );
            }
        }
        for params in out {
            self.notify("textDocument/publishDiagnostics", params);
        }
    }

    fn status(&mut self) -> Json {
        let packages = self
            .packages
            .iter()
            .map(|p| {
                let stats = p.db.stats();
                let checked: Vec<Json> =
                    stats.bodies_checked.iter().filter(|n| !n.starts_with("std.")).map(Json::from).collect();
                Json::obj()
                    .with("root", project::path_key(&p.root))
                    .with("name", p.name.as_str())
                    .with("files", p.own_files().len())
                    .with("analyses", p.analyses)
                    .with("dirty", p.dirty)
                    .with(
                        "last",
                        Json::obj()
                            .with("filesParsed", stats.files_parsed.len())
                            .with("bodiesChecked", Json::Arr(checked))
                            .with("bodiesReused", stats.bodies_reused),
                    )
            })
            .collect();
        Json::obj()
            .with("positionEncoding", self.encoding.name())
            .with("folders", Json::Arr(self.folders.iter().map(|f| Json::from(project::path_key(f))).collect()))
            .with("packages", Json::Arr(packages))
    }

    // ---- shared helpers for the features ------------------------------------------------------------------------

    /// The document a request is about: its path and its owning package, analyzed when `analyze` is set.
    pub fn document(&mut self, params: &Json, analyze: bool) -> Result<(usize, String), ReqError> {
        let uri =
            params.at(&["textDocument", "uri"]).as_str().ok_or_else(|| ReqError::params("missing textDocument.uri"))?;
        let path = self.key_of_uri(uri).ok_or_else(|| ReqError::params(format!("`{uri}` is not a file URI")))?;
        let i =
            self.owner(&path).ok_or_else(|| ReqError::failed(format!("`{uri}` is not a file of a Burdin package")))?;
        if analyze {
            self.analyzed(i)?;
        }
        Ok((i, path))
    }

    /// The byte offset of a request's `position`.
    pub fn offset(&self, i: usize, path: &str, position: &Json) -> Result<u32, ReqError> {
        let text = self.packages[i].text(path).unwrap_or("");
        LineIndex::new(text, self.encoding).offset_of(position).ok_or_else(|| ReqError::params("missing position"))
    }

    /// The URI of a path: the client's spelling for an open document, the written-out file for the standard
    /// library.
    pub fn uri_of(&self, path: &str) -> String {
        if let Some(doc) = self.docs.get(path) {
            return doc.uri.clone();
        }
        if path.starts_with("<std>/") {
            let text = self.packages.iter().find_map(|p| p.text(path)).unwrap_or("");
            if let Some(file) = project::std_file(path, text) {
                return text::path_to_uri(&file);
            }
        }
        text::path_to_uri(Path::new(path))
    }

    /// The text of a file from any package that has it.
    pub fn text_of(&self, path: &str) -> Option<String> {
        if let Some(doc) = self.docs.get(path) {
            return Some(doc.text.clone());
        }
        self.packages.iter().find_map(|p| p.text(path)).map(str::to_string)
    }

    /// The LSP range of a location.
    pub fn range_of(&self, loc: &Loc) -> Json {
        let text = self.text_of(&loc.path).unwrap_or_default();
        LineIndex::new(&text, self.encoding).range(loc.lo, loc.hi)
    }

    /// An LSP `Location`.
    pub fn location(&self, loc: &Loc) -> Json {
        Json::obj().with("uri", self.uri_of(&loc.path)).with("range", self.range_of(loc))
    }

    /// The indices of the packages that read a file.
    pub fn packages_with(&self, path: &str) -> Vec<usize> {
        (0..self.packages.len()).filter(|&i| self.packages[i].has_file(path)).collect()
    }
}
