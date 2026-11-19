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
                        .with("isPreferred", s.machine_applicable)
                        .with("edit", edit_of(vec![(s.span.lo, s.span.hi, s.replacement.clone())])),
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
    if wanted("source.organizeImports") {
        let removals: Vec<(u32, u32, String)> = diags
            .iter()
            .filter(|d| d.code.0 == "L0006")
            .flat_map(|d| d.suggestions.iter().map(|s| (s.span.lo, s.span.hi, s.replacement.clone())))
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

/// A text edit: the byte range it replaces and the new text.
type Edit = (u32, u32, String);

/// Fixes the server writes itself for a diagnostic without a suggestion: an import for an unknown name, `mut` on an
/// immutable binding, and stubs for missing interface methods.
fn fixes_for(server: &mut Server, i: usize, path: &str, text: &str, d: &Diagnostic) -> Vec<(String, Vec<Edit>)> {
    let mut out = Vec::new();
    let slice = text.get(d.span.lo as usize..d.span.hi as usize).unwrap_or("");
    match d.code.0 {
        "E0002" => {
            let name = slice.split(['.', '(', '<', ' ']).next().unwrap_or(slice).to_string();
            if !crate::index::is_identifier(&name) {
                return out;
            }
            let Some(package) = server.packages[i].package_of(path) else { return out };
            let modules = server.packages[i].visible_modules(package);
            let insert = import_position(server, i, path, text);
            let mut r = Resolver::new(&mut server.packages[i]);
            let all = r.all_items();
            let mut seen = Vec::new();
            for (file, item) in all.iter() {
                if item.name != name
                    || item.parent.is_some()
                    || item.kind == Kind::Impl
                    || item.kind == Kind::Test
                    || file == path
                {
                    continue;
                }
                let Some((module, _)) = modules.iter().filter(|(_, f)| f == file).min_by_key(|(m, _)| m.len()) else {
                    continue;
                };
                let line = format!("use {}.{name}\n", module.join("."));
                if seen.contains(&line) {
                    continue;
                }
                seen.push(line.clone());
                out.push((format!("import `{name}` from `{}`", module.join(".")), vec![(insert, insert, line)]));
            }
        }
        "E0005" | "E0113" => {
            let analysis = server.packages[i].current();
            let resolved = analysis.and_then(|a| {
                // The diagnostic covers the use; its first name is the binding.
                let at = text[d.span.lo as usize..]
                    .find(|c: char| c.is_ascii_alphabetic() || c == '_')
                    .map(|n| d.span.lo + n as u32)?;
                a.resolve_at(path, at)
            });
            if let Some(r) = resolved
                && let Some(def) = r.definition
                && matches!(r.kind, burdin_query::DefinitionKind::Local)
            {
                let def_text = server.text_of(&def.path).unwrap_or_default();
                let before = def_text[..def.lo as usize].trim_end();
                if !before.ends_with("mut") {
                    let loc = Loc::from(&def);
                    if loc.path == path {
                        out.push((format!("make `{}` mutable", r.name), vec![(def.lo, def.lo, "mut ".to_string())]));
                    }
                }
            }
        }
        "E0140" => {
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
        _ => {}
    }
    out
}

/// Where a new `use` goes: after the last one, or at the top of the file after its `//!` comments.
fn import_position(server: &mut Server, i: usize, path: &str, text: &str) -> u32 {
    if let Some(ix) = server.packages[i].index(path)
        && let Some(last) = ix.uses.iter().map(|u| u.span.1).max()
    {
        return text[last as usize..].find('\n').map_or(text.len(), |n| last as usize + n + 1) as u32;
    }
    let mut at = 0usize;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("//!") {
            at += line.len();
        } else {
            break;
        }
    }
    at as u32
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
