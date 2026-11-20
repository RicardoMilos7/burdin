//! Code actions, code lenses, and the commands they run (`spec/lsp.md`, LS18-LS19).

use crate::index::{Kind, Tk};
use crate::json::Json;
use crate::nav::Resolver;
use crate::project::{self, Loc};
use crate::server::{Reply, ReqError, Server, TestResult};
use crate::text::LineIndex;
use burdin_diag::Diagnostic;
use std::path::{Path, PathBuf};

/// The `quickfix` and refactoring actions for a range, and the `source.organizeImports` action for the file.
pub fn code_actions(server: &mut Server, params: &Json) -> Reply {
    let (i, path) = server.document(params, true)?;
    let text = server.packages[i].text(&path).unwrap_or("").to_string();
    let index = LineIndex::new(&text, server.encoding);
    let (lo, hi) = index.offsets(params.get("range")).unwrap_or((0, 0));
    let only: Vec<String> =
        params.at(&["context", "only"]).as_array().iter().filter_map(|k| k.as_str().map(str::to_string)).collect();
    let wanted = |kind: &str| only.is_empty() || only.iter().any(|o| kind == o || kind.starts_with(&format!("{o}.")));
    let paths: Vec<String> = server.packages[i].db.file_paths().iter().map(|p| p.to_string()).collect();
    let file_id = server.packages[i].db.file_id(&path);
    let mut diags: Vec<Diagnostic> = server.packages[i]
        .analysis
        .as_ref()
        .map(|a| a.diagnostics_of(&path).into_iter().cloned().collect())
        .unwrap_or_default();
    if server.config.lints {
        diags.extend(
            crate::diagnostics::lint(&mut server.packages[i]).into_iter().filter(|d| Some(d.span.file) == file_id),
        );
    }
    let uri = server.uri_of(&path);
    let mut actions: Vec<Json> = Vec::new();
    let edit_of = |edits: Vec<(u32, u32, String)>| {
        let items: Vec<Json> = edits
            .iter()
            .map(|(a, b, t)| Json::obj().with("range", index.range(*a, *b)).with("newText", t.as_str()))
            .collect();
        Json::obj().with("changes", Json::obj().with(&uri, Json::Arr(items)))
    };
    let overlaps = |d: &Diagnostic| d.span.lo <= hi && lo <= d.span.hi;
    if wanted("quickfix") {
        for d in diags.iter().filter(|d| overlaps(d)) {
            let diagnostic = crate::diagnostics::to_json(server, d, &index, &paths);
            for s in &d.suggestions {
                actions.push(
                    Json::obj()
                        .with("title", s.message.as_str())
                        .with("kind", "quickfix")
                        .with("diagnostics", Json::Arr(vec![diagnostic.clone()]))
                        .with("isPreferred", s.is_machine_applicable())
                        .with("edit", edit_of(burdin_diag::edit::text_edits(&s.edits, s.span().file))),
                );
            }
            for (title, edits) in fixes_for(server, i, &path, &text, d) {
                actions.push(
                    Json::obj()
                        .with("title", title)
                        .with("kind", "quickfix")
                        .with("diagnostics", Json::Arr(vec![diagnostic.clone()]))
                        .with("edit", edit_of(edits)),
                );
            }
        }
    }
    if wanted("refactor.rewrite") {
        // Write the inferred type of a `let` binding.
        let p = &mut server.packages[i];
        if let (Some(ix), Some(analysis)) = (p.index(&path), p.current()) {
            let locals = analysis.locals(&path);
            for (ti, t) in ix.tokens.iter().enumerate() {
                if t.kind != Tk::Keyword("let") {
                    continue;
                }
                let mut n = ti + 1;
                if ix.tokens.get(n).is_some_and(|x| x.kind == Tk::Keyword("mut")) {
                    n += 1;
                }
                let Some(name) = ix.tokens.get(n).filter(|x| x.ident().is_some()) else { continue };
                if name.hi < lo || name.lo > hi || ix.tokens.get(n + 1).is_some_and(|x| x.kind == Tk::Colon) {
                    continue;
                }
                if let Some(local) = locals.iter().find(|l| l.at.lo == name.lo && !l.ty.contains("{error}")) {
                    actions.push(
                        Json::obj()
                            .with("title", format!("write the type of `{}`", local.name))
                            .with("kind", "refactor.rewrite")
                            .with("edit", edit_of(vec![(name.hi, name.hi, format!(": {}", local.ty))])),
                    );
                }
            }
        }
    }
    if wanted("refactor.extract") && lo < hi {
        for (kind, name, title) in [
            ("extract-variable", "value", "extract the selection into a local"),
            ("extract-function", "extracted", "extract the selection into a function"),
        ] {
            if let Some(edits) = refactor_edits(server, i, &path, lo, hi, kind, Some(name)) {
                actions.push(
                    Json::obj().with("title", title).with("kind", "refactor.extract").with("edit", edit_of(edits)),
                );
            }
        }
    }
    if wanted("refactor.inline") && lo == hi {
        for (kind, title) in [
            ("inline-variable", "inline the local at the cursor"),
            ("inline-function", "inline the function at the cursor"),
        ] {
            if let Some(edits) = refactor_edits(server, i, &path, lo, hi, kind, None) {
                actions.push(
                    Json::obj().with("title", title).with("kind", "refactor.inline").with("edit", edit_of(edits)),
                );
            }
        }
    }
    if wanted("source.organizeImports") {
        let removals: Vec<(u32, u32, String)> = diags
            .iter()
            .filter(|d| d.code.0 == "L0006")
            .flat_map(|d| d.suggestions.iter().flat_map(|s| burdin_diag::edit::text_edits(&s.edits, s.span().file)))
            .collect();
        if !removals.is_empty() {
            actions.push(
                Json::obj()
                    .with("title", "remove the unused imports")
                    .with("kind", "source.organizeImports")
                    .with("edit", edit_of(removals)),
            );
        }
    }
    Ok(Json::Arr(actions))
}

/// The edits one refactoring makes to the package, in the text the editor has (RF7). `None` when the refactoring
/// would change what the program means; the editor then offers no action, and the command line explains why.
fn refactor_edits(
    server: &mut Server,
    i: usize,
    path: &str,
    lo: u32,
    hi: u32,
    kind: &str,
    name: Option<&str>,
) -> Option<Vec<Edit>> {
    let encoding = server.encoding;
    let package = &mut server.packages[i];
    let mut engine = burdin_refactor::Engine::new(&mut package.db).ok()?;
    let outcome = match (kind, name) {
        ("extract-variable", Some(name)) => burdin_refactor::extract::variable(&mut engine, path, lo, hi, name),
        ("extract-function", Some(name)) => burdin_refactor::extract::function(&mut engine, path, lo, hi, name),
        ("inline-variable", _) => burdin_refactor::inline::variable(&mut engine, path, lo, hi),
        ("inline-function", _) => burdin_refactor::inline::function(&mut engine, path, lo, hi),
        _ => return None,
    };
    let changes = outcome.ok()?;
    let mut edits = Vec::new();
    for change in changes {
        if let Some(edit) = minimal_edit(&change.before, &change.after) {
            edits.push(edit);
        }
    }
    let _ = encoding;
    (!edits.is_empty()).then_some(edits)
}

/// The one edit that turns `before` into `after`: the text between their common prefix and their common suffix.
fn minimal_edit(before: &str, after: &str) -> Option<(u32, u32, String)> {
    let (b, a) = (before.as_bytes(), after.as_bytes());
    let mut prefix = 0;
    while prefix < b.len() && prefix < a.len() && b[prefix] == a[prefix] {
        prefix += 1;
    }
    while prefix > 0 && (!before.is_char_boundary(prefix) || !after.is_char_boundary(prefix)) {
        prefix -= 1;
    }
    let mut suffix = 0;
    while suffix < b.len() - prefix && suffix < a.len() - prefix && b[b.len() - 1 - suffix] == a[a.len() - 1 - suffix] {
        suffix += 1;
    }
    while suffix > 0 && (!before.is_char_boundary(b.len() - suffix) || !after.is_char_boundary(a.len() - suffix)) {
        suffix -= 1;
    }
    let text = after[prefix..a.len() - suffix].to_string();
    (before[prefix..b.len() - suffix] != text).then(|| (prefix as u32, (b.len() - suffix) as u32, text))
}

/// A text edit: the byte range it replaces and the new text.
type Edit = (u32, u32, String);

/// The fixes the server writes itself, for a diagnostic the compiler gives no suggestion for: a stub for a missing
/// interface method. Everything else (`mut` on a binding, an import, the `mut` marker) is the compiler's suggestion
/// (`spec/fix.md`, FX13), which the server offers like any other.
fn fixes_for(server: &mut Server, i: usize, path: &str, text: &str, d: &Diagnostic) -> Vec<(String, Vec<Edit>)> {
    let mut out = Vec::new();
    if d.code.0 == "E0140" {
        let Some(method) = d.message.split('`').nth(3).map(str::to_string) else { return out };
        let mut r = Resolver::new(&mut server.packages[i]);
        let Some(ix) = r.index(path) else { return out };
        let Some(block) = ix
            .items
            .iter()
            .filter(|it| it.kind == Kind::Impl && it.span.0 <= d.span.lo && d.span.lo <= it.span.1)
            .min_by_key(|it| it.span.1 - it.span.0)
            .cloned()
        else {
            return out;
        };
        let Some(iface) = block.interface.clone() else { return out };
        let all = r.all_items();
        let Some((_, decl)) = all.iter().find(|(_, it)| {
            it.kind == Kind::InterfaceMethod && it.name == method && it.owner.as_deref() == Some(iface.as_str())
        }) else {
            return out;
        };
        let close = block.span.1.saturating_sub(1);
        let line_start = text[..close as usize].rfind('\n').map_or(0, |n| n + 1) as u32;
        let only_space = text[line_start as usize..close as usize].trim().is_empty();
        let stub = format!("    {} {{\n        todo()\n    }}\n", decl.detail);
        let edit = if only_space { (line_start, line_start, stub) } else { (close, close, format!("\n{stub}")) };
        out.push((format!("add the missing method `{method}`"), vec![edit]));
    }
    out
}

// ---- code lenses ------------------------------------------------------------------------------------------------

/// The `burdin` command lenses run: the configured one, this executable when it is `burdin`, a `burdin` next to
/// it, or `burdin` on `PATH`.
pub fn burdin_command(server: &Server) -> String {
    if !server.config.path.is_empty() {
        return server.config.path.clone();
    }
    if let Ok(exe) = std::env::current_exe() {
        if exe.file_stem().is_some_and(|s| s == "burdin") {
            return exe.display().to_string();
        }
        let sibling = exe.with_file_name(format!("burdin{}", std::env::consts::EXE_SUFFIX));
        if sibling.is_file() {
            return sibling.display().to_string();
        }
    }
    "burdin".to_string()
}

fn lens(server: &Server, path: &str, lo: u32, hi: u32, title: &str, command: &str, argument: Option<Json>) -> Json {
    let mut cmd = Json::obj().with("title", title).with("command", command);
    if let Some(argument) = argument {
        cmd.set("arguments", Json::Arr(vec![argument]));
    }
    Json::obj().with("range", server.range_of(&Loc::new(path, lo, hi))).with("command", cmd)
}

/// Lenses above each test (run, debug, and the last result), above `main` (run, debug), and at the top of a file
/// with tests (run them all). A lens's argument carries the exact command line, so the editor and the terminal
/// agree.
pub fn code_lenses(server: &mut Server, params: &Json) -> Reply {
    if !server.config.code_lens {
        return Ok(Json::Arr(Vec::new()));
    }
    let (i, path) = server.document(params, true)?;
    let burdin = burdin_command(server);
    let root = server.packages[i].root.clone();
    let root_text = project::path_key(&root);
    let cwd = if root.is_dir() { root_text.clone() } else { root.parent().map(project::path_key).unwrap_or_default() };
    let tests: Vec<burdin_query::TestInfo> = server.packages[i]
        .current()
        .map(|a| a.tests().into_iter().filter(|t| t.location.path == path).collect())
        .unwrap_or_default();
    let mut lenses = Vec::new();
    if !tests.is_empty() {
        let mut argv = vec![burdin.clone(), "test".to_string(), root_text.clone()];
        argv.extend(tests.iter().map(|t| t.path.clone()));
        argv.push("--exact".to_string());
        let argument = Json::obj()
            .with("kind", "tests")
            .with("root", root_text.as_str())
            .with("tests", Json::Arr(tests.iter().map(|t| Json::from(t.path.as_str())).collect()))
            .with("argv", Json::Arr(argv.iter().map(Json::from).collect()))
            .with("cwd", cwd.as_str());
        lenses.push(lens(
            server,
            &path,
            0,
            0,
            &format!("▶ Run {} tests in file", tests.len()),
            "burdin.runTests",
            Some(argument),
        ));
    }
    for t in &tests {
        let argv = [burdin.clone(), "test".to_string(), root_text.clone(), "--exact".to_string(), t.path.clone()];
        let argument = Json::obj()
            .with("kind", "test")
            .with("root", root_text.as_str())
            .with("tests", Json::Arr(vec![Json::from(t.path.as_str())]))
            .with("argv", Json::Arr(argv.iter().map(Json::from).collect()))
            .with("cwd", cwd.as_str());
        let (lo, hi) = (t.location.lo, t.location.hi);
        lenses.push(lens(server, &path, lo, hi, "▶ Run test", "burdin.runTest", Some(argument)));
        let debug = Json::obj()
            .with("type", "burdin")
            .with("request", "launch")
            .with("name", format!("Debug test {}", t.path))
            .with("target", root_text.as_str())
            .with("test", t.path.as_str())
            .with("cwd", cwd.as_str());
        lenses.push(lens(server, &path, lo, hi, "Debug", "burdin.debug", Some(debug)));
        if let Some(result) = server.tests.get(&(root.clone(), t.path.clone())) {
            let title = if result.passed {
                format!("✓ passed ({} ms)", result.duration_ms)
            } else {
                match &result.reason {
                    Some(reason) => format!("✗ failed: {reason}"),
                    None => "✗ failed".to_string(),
                }
            };
            lenses.push(lens(server, &path, lo, hi, &title, "", None));
        }
    }
    // `main` of the package's entry file.
    let entry = server.packages[i].packages.iter().position(|p| p.is_entry);
    let is_entry_main = server.packages[i]
        .module(&path)
        .is_some_and(|m| Some(m.package) == entry && (m.module.is_empty() || m.module == ["main"]));
    if is_entry_main
        && let Some(ix) = server.packages[i].index(&path)
        && let Some(main) =
            ix.items.iter().find(|it| it.kind == Kind::Function && it.name == "main" && it.parent.is_none())
    {
        let argv = [burdin.clone(), "run".to_string(), root_text.clone()];
        let run = Json::obj()
            .with("kind", "main")
            .with("root", root_text.as_str())
            .with("argv", Json::Arr(argv.iter().map(Json::from).collect()))
            .with("cwd", cwd.as_str());
        let (lo, hi) = main.name_span;
        lenses.push(lens(server, &path, lo, hi, "▶ Run", "burdin.run", Some(run)));
        let debug = Json::obj()
            .with("type", "burdin")
            .with("request", "launch")
            .with("name", format!("Debug {}", server.packages[i].name))
            .with("target", root_text.as_str())
            .with("cwd", cwd.as_str());
        lenses.push(lens(server, &path, lo, hi, "Debug", "burdin.debug", Some(debug)));
    }
    Ok(Json::Arr(lenses))
}

// ---- commands ---------------------------------------------------------------------------------------------------

/// What a test run came to.
pub struct RunOutcome {
    pub root: PathBuf,
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// `burdin.runTest` and `burdin.runTests`: run the lens's command with `--format json` on a worker thread. The
/// response comes when the run ends (`crate::server` answers it), so the server keeps serving meanwhile.
pub fn execute_command(server: &mut Server, params: &Json) -> Reply {
    let command = params.get("command").as_str().unwrap_or("");
    if !crate::server::SERVER_COMMANDS.contains(&command) {
        return Err(ReqError::params(format!("unknown command `{command}`")));
    }
    let argument = params.get("arguments").as_array().first().cloned().unwrap_or(Json::Null);
    if command == "burdin.structuralSearchReplace" {
        return structural_search_replace(server, &argument);
    }
    let argv: Vec<String> =
        argument.get("argv").as_array().iter().filter_map(|a| a.as_str().map(str::to_string)).collect();
    let root = argument.get("root").as_str().map(PathBuf::from).ok_or_else(|| ReqError::params("missing root"))?;
    if argv.len() < 2 || argv[1] != "test" {
        return Err(ReqError::params("the command line must be a `burdin test` invocation"));
    }
    let mut command_line = argv.clone();
    command_line[0] = burdin_command(server);
    command_line.push("--format".to_string());
    command_line.push("json".to_string());
    let root = root.canonicalize().unwrap_or(root);
    Err(ReqError::deferred(Box::new(move || run_tests(&command_line, &root))))
}

/// `burdin.structuralSearchReplace`: replace every match of a pattern in a package (spec/refactor.md, RF6). The
/// argument is `{ "pattern": "...", "replacement": "...", "root": "<package directory>" }`; the answer is a workspace
/// edit for the editor to apply.
fn structural_search_replace(server: &mut Server, argument: &Json) -> Reply {
    let pattern_text = argument.get("pattern").as_str().ok_or_else(|| ReqError::params("missing pattern"))?;
    let replacement = argument.get("replacement").as_str().ok_or_else(|| ReqError::params("missing replacement"))?;
    let root = argument.get("root").as_str().map(PathBuf::from).ok_or_else(|| ReqError::params("missing root"))?;
    let root = root.canonicalize().unwrap_or(root);
    let index = server
        .packages
        .iter()
        .position(|p| p.root.canonicalize().map(|r| r == root).unwrap_or(p.root == root))
        .ok_or_else(|| ReqError::params("the root is not a package this server has open"))?;
    let pattern = burdin_refactor::ssr::parse(pattern_text).map_err(|r| ReqError::params(r.0))?;
    let encoding = server.encoding;
    let changes = {
        let package = &mut server.packages[index];
        let mut engine = burdin_refactor::Engine::new(&mut package.db).map_err(|r| ReqError::failed(r.0))?;
        burdin_refactor::ssr::replace(&mut engine, &pattern, replacement).map_err(|r| ReqError::failed(r.0))?.changes
    };
    let mut all = Json::obj();
    for change in &changes {
        if let Some((lo, hi, text)) = minimal_edit(&change.before, &change.after) {
            let index = LineIndex::new(&change.before, encoding);
            let uri = server.uri_of(&change.path);
            let edit = Json::obj().with("range", index.range(lo, hi)).with("newText", text);
            all = all.with(&uri, Json::Arr(vec![edit]));
        }
    }
    Ok(Json::obj().with("changes", all))
}

fn run_tests(argv: &[String], root: &Path) -> Result<RunOutcome, String> {
    let cwd = if root.is_dir() { root.to_path_buf() } else { root.parent().map(Path::to_path_buf).unwrap_or_default() };
    let output = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&cwd)
        .output()
        .map_err(|e| format!("cannot run `{}`: {e}", argv[0]))?;
    Ok(RunOutcome {
        root: root.to_path_buf(),
        status: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Records a finished run and builds the response: the counts and one entry per test.
pub fn finish_run(server: &mut Server, outcome: Result<RunOutcome, String>) -> Reply {
    let outcome = outcome.map_err(ReqError::failed)?;
    let mut results = Vec::new();
    let (mut passed, mut failed) = (0u32, 0u32);
    for line in outcome.stdout.lines() {
        let Ok(event) = Json::parse(line) else { continue };
        let Some(path) = event.get("path").as_str() else { continue };
        let duration = event.get("duration_ms").as_f64().unwrap_or(0.0) as u64;
        let (ok, status) = match event.get("event").as_str() {
            Some("test_passed") => (true, "passed"),
            Some("test_failed") => (false, "failed"),
            _ => continue,
        };
        let reason = event.get("reason").as_str().map(str::to_string);
        if ok {
            passed += 1;
        } else {
            failed += 1;
        }
        server.tests.insert(
            (outcome.root.clone(), path.to_string()),
            TestResult { passed: ok, duration_ms: duration, reason: reason.clone() },
        );
        results.push(
            Json::obj().with("path", path).with("status", status).with("durationMs", duration).with("reason", reason),
        );
    }
    if results.is_empty() && outcome.status != Some(0) {
        let first =
            outcome.stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("the tests did not run").to_string();
        server.show(1, format!("burdin test: {first}"));
        return Err(ReqError::failed(format!("the tests did not run: {first}")));
    }
    let summary = format!("test result: {passed} passed, {failed} failed");
    server.show(if failed == 0 { 3 } else { 1 }, summary.as_str());
    server.refresh_lenses();
    Ok(Json::obj()
        .with("passed", passed)
        .with("failed", failed)
        .with("summary", summary)
        .with("results", Json::Arr(results)))
}
