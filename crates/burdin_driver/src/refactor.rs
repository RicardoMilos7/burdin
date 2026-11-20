//! `burdin ssr` and `burdin refactor` (`spec/refactor.md`): the command line over [`burdin_refactor`]. The engine
//! computes and checks the new texts; this module resolves the target, turns a `file:line:col` or a
//! `file:line:col-line:col` into byte offsets, prints what changed (a diff with `--dry-run`), and writes every file
//! or none.

use crate::fix::dirty_refusal;
use crate::lint::diff_path;
use crate::{EntryMode, flag_without_value, jobs_or_default, option_value, parse_jobs};
use burdin_query::{Db, Options};
use burdin_refactor::{Engine, FileChange};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Where in the target a refactoring works: a line and a column, both 1-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Place {
    pub(crate) line: u32,
    pub(crate) col: u32,
}

impl Place {
    /// Parses `line:col`.
    fn parse(text: &str) -> Option<Place> {
        let (line, col) = text.split_once(':')?;
        Some(Place { line: line.parse().ok()?, col: col.parse().ok()? })
    }
}

/// A target with an optional range: `<path>[:<line>:<col>[-<line>:<col>]]`.
#[derive(Clone, Debug)]
pub(crate) struct Target {
    pub(crate) path: PathBuf,
    pub(crate) start: Option<Place>,
    pub(crate) end: Option<Place>,
}

impl Target {
    /// Splits `<path>:<start>[-<end>]` from the right, so a Windows path's drive letter stays part of the path.
    pub(crate) fn parse(spec: &str) -> Target {
        if let Some((head, tail)) = spec.rsplit_once('-')
            && let Some(end) = Place::parse(tail)
        {
            return match path_and_place(head) {
                Some((path, start)) => Target { path, start: Some(start), end: Some(end) },
                None => Target { path: PathBuf::from(spec), start: None, end: None },
            };
        }
        match path_and_place(spec) {
            Some((path, start)) => Target { path, start: Some(start), end: None },
            None => Target { path: PathBuf::from(spec), start: None, end: None },
        }
    }
}

/// `<path>:<line>:<col>`, split at its last two colons.
fn path_and_place(spec: &str) -> Option<(PathBuf, Place)> {
    let (rest, col) = spec.rsplit_once(':')?;
    let (path, line) = rest.rsplit_once(':')?;
    Some((PathBuf::from(path), Place { line: line.parse().ok()?, col: col.parse().ok()? }))
}

/// The options `ssr` and `refactor` share.
struct Common {
    target: PathBuf,
    dry_run: bool,
    allow_dirty: bool,
    jobs: Option<usize>,
}

impl Common {
    fn new() -> Common {
        Common { target: PathBuf::new(), dry_run: false, allow_dirty: false, jobs: None }
    }

    /// Reads `--dry-run` and `--allow-dirty`; `false` when `raw` is not one of them.
    fn take(&mut self, raw: &str, value: Option<&str>) -> Option<Result<(), String>> {
        match raw {
            "--dry-run" => Some(flag_without_value(raw, value).map(|v| self.dry_run = v)),
            "--allow-dirty" => Some(flag_without_value(raw, value).map(|v| self.allow_dirty = v)),
            _ => None,
        }
    }
}

/// The arguments of `burdin ssr`.
struct SsrArgs {
    pattern: String,
    replacement: Option<String>,
    common: Common,
}

fn parse_ssr(argv: &[String]) -> Result<SsrArgs, String> {
    let mut common = Common::new();
    let mut spec: Option<String> = None;
    let mut target: Option<PathBuf> = None;
    let mut it = argv.iter();
    while let Some(raw) = it.next() {
        let (a, inline) = split_flag(raw);
        if let Some(result) = common.take(a, inline) {
            result?;
            continue;
        }
        match a {
            "-j" | "--jobs" => common.jobs = Some(parse_jobs(&mut it, inline, a)?),
            _ if a.starts_with('-') && a != "-" => return Err(format!("unknown option `{a}`")),
            _ if spec.is_none() => spec = Some(raw.clone()),
            _ if target.is_none() => target = Some(PathBuf::from(raw)),
            _ => return Err(format!("unexpected argument `{raw}`")),
        }
    }
    let Some(spec) = spec else { return Err("missing pattern".to_string()) };
    let Some(target) = target else { return Err("missing target".to_string()) };
    common.target = target;
    let (pattern, replacement) = match spec.split_once("==>>") {
        Some((p, r)) => (p.trim().to_string(), Some(r.trim().to_string())),
        None => (spec.trim().to_string(), None),
    };
    Ok(SsrArgs { pattern, replacement, common })
}

/// The arguments of `burdin refactor`.
struct RefactorArgs {
    kind: String,
    target: Target,
    name: Option<String>,
    to: Option<String>,
    change: burdin_refactor::signature::Change,
    common: Common,
}

fn parse_refactor(argv: &[String]) -> Result<RefactorArgs, String> {
    let kinds =
        ["extract-variable", "extract-function", "inline-variable", "inline-function", "change-signature", "move-item"];
    let Some(kind) = argv.first().cloned() else {
        return Err(format!("missing the refactoring; one of: {}", kinds.join(", ")));
    };
    if !kinds.contains(&kind.as_str()) {
        return Err(format!("unknown refactoring `{kind}`; one of: {}", kinds.join(", ")));
    }
    let mut common = Common::new();
    let mut spec: Option<String> = None;
    let mut name: Option<String> = None;
    let mut to: Option<String> = None;
    let mut change = burdin_refactor::signature::Change::default();
    let mut it = argv[1..].iter();
    while let Some(raw) = it.next() {
        let (a, inline) = split_flag(raw);
        if let Some(result) = common.take(a, inline) {
            result?;
            continue;
        }
        match a {
            "-j" | "--jobs" => common.jobs = Some(parse_jobs(&mut it, inline, a)?),
            "--name" => name = Some(option_value(&mut it, inline, a, "a name")?),
            "--to" => to = Some(option_value(&mut it, inline, a, "a module")?),
            "--reorder" => {
                let list = option_value(&mut it, inline, a, "a list of parameters")?;
                change.reorder =
                    Some(list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect());
            }
            "--add" => {
                let text = option_value(&mut it, inline, a, "a parameter")?;
                match parse_param(&text) {
                    Some(param) => change.add.push(param),
                    None => return Err(format!("`--add` takes `name: type = default`, got `{text}`")),
                }
            }
            "--remove" => change.remove.push(option_value(&mut it, inline, a, "a parameter")?),
            "--rename" => {
                let pair = option_value(&mut it, inline, a, "old=new")?;
                match pair.split_once('=') {
                    Some((from, to)) => change.rename.push((from.trim().to_string(), to.trim().to_string())),
                    None => return Err(format!("`--rename` takes `old=new`, got `{pair}`")),
                }
            }
            _ if a.starts_with('-') && a != "-" => return Err(format!("unknown option `{a}`")),
            _ if spec.is_none() => spec = Some(raw.clone()),
            _ => return Err(format!("unexpected argument `{raw}`")),
        }
    }
    let Some(spec) = spec else { return Err("missing target".to_string()) };
    let target = Target::parse(&spec);
    common.target = target.path.clone();
    if target.path.as_os_str().is_empty() {
        return Err(format!("`{spec}` is not `<file>:<line>:<col>`"));
    }
    let needs_range = kind.starts_with("extract-");
    if target.start.is_none() {
        return Err(format!("`{kind}` needs a position: `<file>:<line>:<col>`"));
    }
    if needs_range && target.end.is_none() {
        return Err(format!("`{kind}` needs a range: `<file>:<line>:<col>-<line>:<col>`"));
    }
    if kind == "move-item" && to.is_none() {
        return Err("`move-item` needs --to <module>".to_string());
    }
    if needs_range && name.is_none() {
        return Err(format!("`{kind}` needs --name <the name of the new item>"));
    }
    Ok(RefactorArgs { kind, target, name, to, change, common })
}

/// A flag as `(name, inline value)`: `--name=x` and `-j4` are read like `--name x` and `-j 4`.
fn split_flag(raw: &str) -> (&str, Option<&str>) {
    match raw.split_once('=') {
        Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
        _ if raw.len() > 2 && raw.starts_with("-j") => ("-j", Some(&raw[2..])),
        _ => (raw, None),
    }
}

/// `name: type = default`, as `--add` writes a parameter.
fn parse_param(text: &str) -> Option<(String, String, String)> {
    let (name, rest) = text.split_once(':')?;
    let (ty, default) = rest.split_once('=')?;
    let (name, ty, default) = (name.trim().to_string(), ty.trim().to_string(), default.trim().to_string());
    (!name.is_empty() && !ty.is_empty() && !default.is_empty()).then_some((name, ty, default))
}

/// Opens the package a target belongs to: the directory itself, or the nearest directory above a file that has a
/// burdin.toml. A file outside any package is a one-file target, as `burdin check` reads it.
fn open(target: &Path, jobs: Option<usize>) -> Result<Db, String> {
    let options = Options { mode: EntryMode::Test, jobs: jobs_or_default(jobs), ..Options::default() };
    Db::open(&package_root(target), options)
}

/// The directory of the package `target` is part of, or `target` itself when it is not in one.
fn package_root(target: &Path) -> PathBuf {
    let start = if target.is_dir() { Some(target) } else { target.parent() };
    let mut at = start.map(Path::to_path_buf);
    while let Some(dir) = at {
        if dir.join("burdin.toml").is_file() {
            return dir;
        }
        at = dir.parent().map(Path::to_path_buf);
    }
    target.to_path_buf()
}

/// The file of the package the target's path names, as the database lists it.
fn file_of(engine: &Engine<'_>, path: &Path) -> Result<String, String> {
    let wanted = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    for candidate in engine.entry_files() {
        let full = PathBuf::from(&candidate);
        if full == wanted || full.canonicalize().ok().as_deref() == Some(wanted.as_path()) {
            return Ok(candidate);
        }
    }
    Err(format!("`{}` is not a file of the package", path.display()))
}

fn place(engine: &Engine<'_>, path: &str, at: Place) -> Result<u32, String> {
    engine.offset(path, at.line, at.col).map_err(|r| r.0)
}

fn range(engine: &Engine<'_>, path: &str, target: &Target) -> Result<(u32, u32), String> {
    match (target.start, target.end) {
        (Some(start), Some(end)) => Ok((place(engine, path, start)?, place(engine, path, end)?)),
        _ => Err("the target needs a range, as `<file>:<line>:<col>-<line>:<col>`".to_string()),
    }
}

/// Writes the changes, or prints their diff with `--dry-run`, and reports what happened.
fn finish(mut changes: Vec<FileChange>, common: &Common, what: &str, verb: &str) -> ExitCode {
    // The touched files are formatted, as `burdin fix` formats the files it fixes.
    for change in &mut changes {
        if let Ok(formatted) = burdin_fmt::format_source(&change.after, &burdin_fmt::Options::default()) {
            change.after = formatted;
        }
    }
    if changes.is_empty() {
        eprintln!("{what}: nothing changed");
        return ExitCode::SUCCESS;
    }
    if common.dry_run {
        for change in &changes {
            print!("{}", burdin_lint::unified_diff(&diff_path(Path::new(&change.path)), &change.before, &change.after));
        }
        eprintln!("{what}: would {verb} {}", files(changes.len()));
        return ExitCode::SUCCESS;
    }
    if let Some(message) = dirty_refusal(&common.target, common.allow_dirty, what) {
        eprintln!("error: {message}");
        return ExitCode::from(2);
    }
    let writes: Vec<(PathBuf, String, String)> =
        changes.iter().map(|c| (PathBuf::from(&c.path), c.before.clone(), c.after.clone())).collect();
    if let Err(message) = crate::lint::write_all_or_nothing(&writes) {
        eprintln!("error: {message}; no files were changed");
        return ExitCode::from(2);
    }
    for change in &changes {
        println!("changed {}", diff_path(Path::new(&change.path)));
    }
    eprintln!("{what}: {verb} {}", files(changes.len()));
    ExitCode::SUCCESS
}

fn files(n: usize) -> String {
    if n == 1 { "1 file".to_string() } else { format!("{n} files") }
}

fn matches(n: usize) -> String {
    if n == 1 { "1 match".to_string() } else { format!("{n} matches") }
}

/// Runs `burdin ssr '<pattern> [==>> <replacement>]' <target> [OPTIONS]` (RF1, RF6).
pub(crate) fn run_ssr(argv: &[String]) -> ExitCode {
    let args = match parse_ssr(argv) {
        Ok(args) => args,
        Err(message) => return crate::cli::usage_error("ssr", &message),
    };
    let pattern = match burdin_refactor::ssr::parse(&args.pattern) {
        Ok(pattern) => pattern,
        Err(r) => return crate::cli::usage_error("ssr", &r.0),
    };
    let mut db = match open(&args.common.target, args.common.jobs) {
        Ok(db) => db,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };
    let mut engine = match Engine::new(&mut db) {
        Ok(engine) => engine,
        Err(r) => {
            eprintln!("error: {}", r.0);
            return ExitCode::from(1);
        }
    };
    let Some(replacement) = args.replacement else {
        let hits = match burdin_refactor::ssr::search(&mut engine, &pattern, None) {
            Ok(hits) => hits,
            Err(r) => {
                eprintln!("error: {}", r.0);
                return ExitCode::from(1);
            }
        };
        for hit in &hits {
            let shown = diff_path(Path::new(&hit.path));
            let (line, col) = engine.db.line_col(&hit.path, hit.at.lo).unwrap_or((0, 0));
            println!("{shown}:{line}:{col}: {}", one_line(&hit.source));
        }
        eprintln!("ssr: {}", matches(hits.len()));
        return if hits.is_empty() { ExitCode::from(1) } else { ExitCode::SUCCESS };
    };
    let report = match burdin_refactor::ssr::replace(&mut engine, &pattern, &replacement) {
        Ok(report) => report,
        Err(r) => {
            eprintln!("error: {}", r.0);
            return ExitCode::from(1);
        }
    };
    for (hit, reason) in &report.skipped {
        eprintln!("warning: skipped the match at {}: {reason}", engine.place(&hit.path, hit.at.lo));
    }
    let count = report.hits.len();
    let verb = if args.common.dry_run { "replace" } else { "replaced" };
    let code = finish(report.changes, &args.common, "ssr", verb);
    if code == ExitCode::SUCCESS {
        eprintln!("ssr: {}", matches(count));
    }
    code
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Runs `burdin refactor <kind> <target> [OPTIONS]` (RF7-RF14).
pub(crate) fn run_refactor(argv: &[String]) -> ExitCode {
    let args = match parse_refactor(argv) {
        Ok(args) => args,
        Err(message) => return crate::cli::usage_error("refactor", &message),
    };
    let mut db = match open(&args.target.path, args.common.jobs) {
        Ok(db) => db,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };
    let mut engine = match Engine::new(&mut db) {
        Ok(engine) => engine,
        Err(r) => {
            eprintln!("error: {}", r.0);
            return ExitCode::from(1);
        }
    };
    let path = match file_of(&engine, &args.target.path) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };
    let result = (|| -> Result<Vec<FileChange>, String> {
        match args.kind.as_str() {
            "extract-variable" | "extract-function" => {
                let (lo, hi) = range(&engine, &path, &args.target)?;
                let name = args.name.clone().unwrap_or_default();
                if args.kind == "extract-variable" {
                    burdin_refactor::extract::variable(&mut engine, &path, lo, hi, &name)
                } else {
                    burdin_refactor::extract::function(&mut engine, &path, lo, hi, &name)
                }
                .map_err(|r| r.0)
            }
            "inline-variable" | "inline-function" => {
                let at = place(&engine, &path, args.target.start.expect("checked"))?;
                if args.kind == "inline-variable" {
                    burdin_refactor::inline::variable(&mut engine, &path, at, at + 1)
                } else {
                    burdin_refactor::inline::function(&mut engine, &path, at, at + 1)
                }
                .map_err(|r| r.0)
            }
            "change-signature" => {
                let at = place(&engine, &path, args.target.start.expect("checked"))?;
                burdin_refactor::signature::change(&mut engine, &path, at, at + 1, &args.change).map_err(|r| r.0)
            }
            "move-item" => {
                let at = place(&engine, &path, args.target.start.expect("checked"))?;
                let to = args.to.clone().unwrap_or_default();
                burdin_refactor::move_item::item(&mut engine, &path, at, at + 1, &to).map_err(|r| r.0)
            }
            _ => unreachable!("the kind was checked"),
        }
    })();
    match result {
        Ok(changes) => {
            let verb = if args.common.dry_run { "change" } else { "changed" };
            finish(changes, &args.common, "refactor", verb)
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_reads_its_path_and_its_positions() {
        let t = Target::parse("src/main.bd:3:5");
        assert_eq!(t.path, PathBuf::from("src/main.bd"));
        assert_eq!(t.start, Some(Place { line: 3, col: 5 }));
        assert_eq!(t.end, None);
        let t = Target::parse("src/main.bd:3:5-4:2");
        assert_eq!(t.start, Some(Place { line: 3, col: 5 }));
        assert_eq!(t.end, Some(Place { line: 4, col: 2 }));
        let t = Target::parse("pkg");
        assert_eq!(t.path, PathBuf::from("pkg"));
        assert_eq!(t.start, None);
    }

    #[test]
    fn an_added_parameter_needs_a_default() {
        assert_eq!(parse_param("c: i32 = 0"), Some(("c".into(), "i32".into(), "0".into())));
        assert_eq!(parse_param("c: i32"), None);
        assert_eq!(parse_param("c = 0"), None);
    }
}
