//! The `launch` and `attach` augmentation (`spec/debug.md`, DBG11): a request may name a `target` — a `.bd` file or
//! a package directory — instead of an already-built program, and the adapter builds it with `-g` in the debug
//! profile. The forwarded request then carries the executable and the `preRunCommands` that import the lldb
//! formatters, so scopes, variables, and `evaluate` print through them (DBG7).

use burdin_json::json::Json;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The lldb formatters, embedded at compile time like the driver's copy (`spec/debug.md`, DBG7).
const LLDB_FORMATTERS: &str = include_str!("../../../debuggers/lldb/burdin_formatters.py");

/// What a request needs: the executable to debug, the program arguments it starts with, and the commands to run
/// before it starts.
pub(crate) struct Prepared {
    /// The executable, when the request named a `target` to build.
    pub(crate) program: Option<PathBuf>,
    pub(crate) args: Vec<String>,
    pub(crate) pre_run: Vec<String>,
}

/// Builds `target` for a request, in test mode when `test` names a `test` block, and returns the executable and the
/// formatter import. `None` when the request names no `target`, in which case the request is forwarded as it is.
pub(crate) fn prepare(arguments: &Json, session_dir: &Path) -> Result<Option<Prepared>, String> {
    let target = arguments.get("target").as_str().map(str::to_string);
    let test = arguments.get("test").as_str().map(str::to_string);
    let formatters = write_formatters(session_dir)?;
    let pre_run = vec![format!("command script import {}", formatters.display())];
    let Some(target) = target else {
        if test.is_some() {
            return Err("`test` needs a `target`: name the `.bd` file or package the test lives in".to_string());
        }
        return Ok(Some(Prepared { program: None, args: Vec::new(), pre_run }));
    };
    let stem = Path::new(&target)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "program".to_string());
    let build_dir = session_dir.join("build");
    std::fs::create_dir_all(&build_dir).map_err(|e| format!("cannot create {}: {e}", build_dir.display()))?;
    let program = build_dir.join(stem).with_extension(std::env::consts::EXE_EXTENSION);
    let mut command = Command::new(crate::driver());
    command.arg("build").arg(&target).arg("-g");
    let mut args = Vec::new();
    if let Some(test) = &test {
        // A test build compiles the tests and their dispatcher instead of `main`; the index the dispatcher takes
        // becomes the program's first argument (`spec/debug.md`, DBG10).
        command.arg("--tests");
        args.push(resolve_test(&target, test)?);
    }
    command.arg("-o").arg(&program);
    let output = command.output().map_err(|e| format!("cannot run `{}`: {e}", crate::driver()))?;
    if !output.status.success() {
        let mut message = String::from_utf8_lossy(&output.stderr).trim_end().to_string();
        if message.is_empty() {
            message = String::from_utf8_lossy(&output.stdout).trim_end().to_string();
        }
        return Err(format!("could not build `{target}`:\n{message}"));
    }
    Ok(Some(Prepared { program: Some(program), args, pre_run }))
}

/// The test's index in the dispatcher, resolved like `burdin debug --test` and the runner's filters: an index is
/// taken as it is and a name is the first full path (`module.name`) that contains it.
fn resolve_test(target: &str, query: &str) -> Result<String, String> {
    if query.parse::<usize>().is_ok() {
        return Ok(query.to_string());
    }
    let output = Command::new(crate::driver())
        .arg("test")
        .arg(target)
        .arg("--list")
        .arg("--format")
        .arg("json")
        .output()
        .map_err(|e| format!("cannot run `{}`: {e}", crate::driver()))?;
    if !output.status.success() {
        return Err(format!(
            "cannot list the tests of `{target}`:\n{}",
            String::from_utf8_lossy(&output.stderr).trim_end()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let tests = text.lines().filter(|line| !line.trim().is_empty());
    for (index, line) in tests.enumerate() {
        let path = Json::parse(line)?;
        if path.get("path").as_str().unwrap_or_default().contains(query) {
            return Ok(index.to_string());
        }
    }
    Err(format!("no test path contains `{query}` (run `burdin test --list` to see them)"))
}

/// Writes the embedded lldb formatters into the session's directory and returns the path.
fn write_formatters(session_dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(session_dir).map_err(|e| format!("cannot create {}: {e}", session_dir.display()))?;
    let path = session_dir.join("burdin_formatters.py");
    std::fs::write(&path, LLDB_FORMATTERS).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}
