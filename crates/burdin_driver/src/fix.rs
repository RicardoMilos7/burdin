//! `burdin fix` (`spec/fix.md`, FX3-FX9): applies every machine-applicable suggestion of the compiler and the linter
//! across a package, round after round until nothing changes, formats the touched files, and prints what it changed.
//! `burdin lint --fix` runs the same loop ([`fix_loop`]) with its own level flags and its own output.

use crate::lint::{Outcome, diff_path, lint_once, plural, write_all_or_nothing};
use crate::{
    CompileOptions, DiagnosticOptions, EntryMode, SourceOverrides, flag_without_value, jobs_or_default, option_value,
    parse_color, parse_format, parse_jobs, parse_number, render_options,
};
use burdin_diag::edit::{TextEdit, apply_groups};
use burdin_diag::{Diagnostic, FileId, Severity, Suggestion};
use burdin_lint::Level;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// How many rounds the loop runs at most: a fix can reveal another, so it repeats until nothing is left to fix
/// (FX4).
pub(crate) const MAX_FIX_ROUNDS: usize = 8;

/// One fix the loop applied or skipped, for the report.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FixRecord {
    pub(crate) path: PathBuf,
    /// The 1-based line of the fix's first edit, in the text the round edited.
    pub(crate) line: u32,
    pub(crate) code: String,
    pub(crate) message: String,
}

/// What the loop came to.
pub(crate) struct LoopResult {
    /// The last run of the front end and the lints, over the fixed text.
    pub(crate) last: Outcome,
    /// The new text of every changed file.
    pub(crate) overrides: SourceOverrides,
    /// The text every changed file had on disk.
    pub(crate) originals: HashMap<PathBuf, String>,
    /// The fixes applied, in the order they were applied.
    pub(crate) applied: Vec<FixRecord>,
    /// The fixes skipped because their edits clash with a fix applied before them, and that are still offered at the
    /// end (FX5).
    pub(crate) conflicts: Vec<FixRecord>,
    /// A round's fixes would have raised the number of errors, so that round was undone and the loop stopped (FX6).
    pub(crate) reverted: bool,
}

fn error_count(outcome: &Outcome) -> usize {
    outcome.all().iter().filter(|d| d.severity == Severity::Error).count()
}

/// The machine-applicable suggestions of a diagnostic and its children, with the code they belong to.
fn suggestions<'a>(d: &'a Diagnostic, out: &mut Vec<(&'a str, &'a Suggestion)>) {
    for s in d.suggestions.iter().filter(|s| s.is_machine_applicable()) {
        out.push((d.code.0, s));
    }
    for child in &d.children {
        suggestions(child, out);
    }
}

/// The fixes one outcome offers, by file of the entry package: each suggestion is one group of edits. A suggestion
/// with an edit outside the entry package (the standard library, a dependency, a generated module) is not offered.
fn offered(outcome: &Outcome) -> BTreeMap<FileId, Vec<(FixRecord, Vec<TextEdit>)>> {
    let in_package: HashSet<FileId> = outcome.entry.iter().map(|(f, _)| *f).collect();
    let all = outcome.all();
    let mut found: Vec<(&str, &Suggestion)> = Vec::new();
    let mut sorted: Vec<&Diagnostic> = all.iter().collect();
    sorted.sort_by_key(|d| (d.span.file.0, d.span.lo, d.span.hi));
    for d in sorted {
        suggestions(d, &mut found);
    }
    let mut by_file: BTreeMap<FileId, Vec<(FixRecord, Vec<TextEdit>)>> = BTreeMap::new();
    for (code, s) in found {
        let file = s.span().file;
        if s.edits.is_empty() || s.edits.iter().any(|e| e.span.file != file) || !in_package.contains(&file) {
            continue;
        }
        let path = outcome.front.files[file.0 as usize].0.clone();
        let line = outcome.front.sources.get(file).line_col(s.span().lo).0;
        let record = FixRecord { path, line, code: code.to_string(), message: s.message.clone() };
        let edits = s.edits.iter().map(|e| (e.span.lo, e.span.hi, e.replacement.clone())).collect();
        by_file.entry(file).or_default().push((record, edits));
    }
    by_file
}

/// Runs the front end and the lints, applies the fixes they offer, and repeats until a round changes nothing or
/// [`MAX_FIX_ROUNDS`] rounds ran. A round whose fixes would raise the number of errors is undone and ends the loop.
pub(crate) fn fix_loop(
    target: &Path,
    options: &CompileOptions,
    levels: &[(String, Level)],
    start: SourceOverrides,
) -> Result<LoopResult, ExitCode> {
    let mut overrides = start;
    let mut originals: HashMap<PathBuf, String> = HashMap::new();
    let mut applied: Vec<FixRecord> = Vec::new();
    let mut skipped: Vec<FixRecord> = Vec::new();
    let mut reverted = false;
    let mut outcome = lint_once(target, options, &overrides, levels)?;
    for _ in 0..MAX_FIX_ROUNDS {
        let mut next = overrides.clone();
        let mut round_applied = Vec::new();
        let mut round_originals = Vec::new();
        for (file, fixes) in offered(&outcome) {
            let path = outcome.front.files[file.0 as usize].0.clone();
            let text = outcome.front.sources.get(file).text.clone();
            let groups: Vec<Vec<TextEdit>> = fixes.iter().map(|(_, edits)| edits.clone()).collect();
            let result = apply_groups(&text, &groups);
            skipped.extend(result.conflicts.iter().map(|&i| fixes[i].0.clone()));
            if result.text != text {
                round_applied.extend(result.applied.iter().map(|&i| fixes[i].0.clone()));
                round_originals.push((path.clone(), text));
                next.insert(path, result.text);
            }
        }
        if round_applied.is_empty() {
            break;
        }
        let candidate = lint_once(target, options, &next, levels)?;
        if error_count(&candidate) > error_count(&outcome) {
            reverted = true;
            break;
        }
        for (path, text) in round_originals {
            originals.entry(path).or_insert(text);
        }
        applied.extend(round_applied);
        overrides = next;
        outcome = candidate;
    }
    // A skipped fix is reported only when the final text still offers it: most clashes resolve in the next round.
    let still: HashSet<(PathBuf, String, String)> =
        offered(&outcome).into_values().flatten().map(|(r, _)| (r.path, r.code, r.message)).collect();
    let mut conflicts: Vec<FixRecord> =
        skipped.into_iter().filter(|r| still.contains(&(r.path.clone(), r.code.clone(), r.message.clone()))).collect();
    conflicts.sort();
    conflicts.dedup_by(|a, b| a.path == b.path && a.code == b.code && a.message == b.message);
    Ok(LoopResult { last: outcome, overrides, originals, applied, conflicts, reverted })
}

/// The changed files as `(path, old text, new text)`, sorted by path.
pub(crate) fn changes(result: &LoopResult) -> Vec<(PathBuf, String, String)> {
    let mut out: Vec<(PathBuf, String, String)> = result
        .overrides
        .iter()
        .filter_map(|(path, new)| {
            let old = result.originals.get(path)?;
            (old != new).then(|| (path.clone(), old.clone(), new.clone()))
        })
        .collect();
    out.sort();
    out
}

// ---- burdin fix ---------------------------------------------------------------------------------------------------

struct FixArgs {
    target: PathBuf,
    dry_run: bool,
    allow_dirty: bool,
    migrate: bool,
    diagnostics: DiagnosticOptions,
    comptime_steps: Option<u64>,
    jobs: Option<usize>,
}

fn parse(argv: &[String]) -> Result<FixArgs, String> {
    let mut args = FixArgs {
        target: PathBuf::new(),
        dry_run: false,
        allow_dirty: false,
        migrate: false,
        diagnostics: DiagnosticOptions::default(),
        comptime_steps: None,
        jobs: None,
    };
    let mut target = None;
    let mut it = argv.iter();
    while let Some(raw) = it.next() {
        let (a, inline) = match raw.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            _ if raw.len() > 2 && raw.starts_with("-j") => ("-j", Some(&raw[2..])),
            _ => (raw.as_str(), None),
        };
        match a {
            "--dry-run" => args.dry_run = flag_without_value(a, inline)?,
            "--allow-dirty" => args.allow_dirty = flag_without_value(a, inline)?,
            "--migrate" => args.migrate = flag_without_value(a, inline)?,
            "-j" | "--jobs" => args.jobs = Some(parse_jobs(&mut it, inline, a)?),
            "--diagnostic-format" => {
                args.diagnostics.format = parse_format(&option_value(&mut it, inline, a, "a value")?)?;
            }
            "--color" => args.diagnostics.color = parse_color(&option_value(&mut it, inline, a, "a value")?)?,
            "--no-color" => {
                flag_without_value(a, inline)?;
                args.diagnostics.color = crate::ColorChoice::Never;
            }
            "--width" => {
                let value = option_value(&mut it, inline, a, "a number")?;
                args.diagnostics.width = Some(parse_number(a, &value)?);
            }
            "--error-limit" => {
                let value = option_value(&mut it, inline, a, "a number")?;
                let limit = parse_number(a, &value)?;
                args.diagnostics.error_limit = (limit > 0).then_some(limit);
            }
            "--comptime-steps" => {
                let value = option_value(&mut it, inline, a, "a number of steps")?;
                let steps: u64 = value
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("`--comptime-steps` needs a positive number of steps, got `{value}`"))?;
                args.comptime_steps = Some(steps);
            }
            _ if a.starts_with('-') => return Err(format!("unknown option `{a}`")),
            _ if target.is_none() => target = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument `{a}`")),
        }
    }
    args.target = target.ok_or("missing target file")?;
    Ok(args)
}

/// The directory whose version-control state decides whether a fix may write: the package directory, or the
/// directory of a one-file target.
pub(crate) fn work_dir(target: &Path) -> PathBuf {
    if target.is_dir() {
        target.to_path_buf()
    } else {
        target.parent().filter(|p| !p.as_os_str().is_empty()).map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    }
}

/// The uncommitted changes under `dir`, one `git status --porcelain` line each (FX7). Empty when `dir` is not in a
/// git working tree or git is not installed: there is nothing to protect a review with then.
pub(crate) fn dirty_files(dir: &Path) -> Vec<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["status", "--porcelain", "--untracked-files=all", "--", "."])
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect()
        }
        _ => Vec::new(),
    }
}

/// Refuses to write over uncommitted changes (FX7): returns the error to print, or `None` when writing is allowed.
pub(crate) fn dirty_refusal(target: &Path, allow_dirty: bool, command: &str) -> Option<String> {
    if allow_dirty {
        return None;
    }
    let dirty = dirty_files(&work_dir(target));
    if dirty.is_empty() {
        return None;
    }
    let mut message = format!(
        "the working tree has uncommitted changes, so `burdin {command}` would mix its edits with them; commit or \
         stash them, or pass --allow-dirty:"
    );
    for line in dirty.iter().take(10) {
        message.push_str(&format!("\n  {line}"));
    }
    if dirty.len() > 10 {
        message.push_str(&format!("\n  ... and {} more", dirty.len() - 10));
    }
    Some(message)
}

fn record_path(record: &FixRecord) -> String {
    diff_path(&record.path)
}

pub(crate) fn run(argv: &[String]) -> ExitCode {
    let args = match parse(argv) {
        Ok(a) => a,
        Err(message) => return crate::cli::usage_error("fix", &message),
    };
    if !args.dry_run
        && let Some(message) = dirty_refusal(&args.target, args.allow_dirty, "fix")
    {
        eprintln!("error: {message}");
        return ExitCode::from(2);
    }
    let options = CompileOptions {
        release: false,
        debug: false,
        strip: false,
        lto: false,
        pgo_generate: false,
        pgo_use: None,
        emit_ir: None,
        emit: None,
        link_libs: &[],
        // Test blocks are fixed too, and a library package has no `main`.
        mode: EntryMode::Test,
        diagnostics: &args.diagnostics,
        comptime_steps: args.comptime_steps,
        emit_deps: None,
        report_clean: false,
        run_build_scripts: true,
        locked: false,
        jobs: jobs_or_default(args.jobs),
        sanitize: Default::default(),
        fuzz_target: None,
    };
    let migration = if args.migrate {
        match crate::migrate::plan(&args.target) {
            Ok(plan) => Some(plan),
            Err(message) => {
                eprintln!("error: {message}");
                return ExitCode::from(2);
            }
        }
    } else {
        None
    };
    let result = match fix_loop(&args.target, &options, &[], SourceOverrides::new()) {
        Ok(r) => r,
        Err(code) => return code,
    };
    // Format what the fixes touched (FX8). A file the formatter refuses keeps the fixed text as it is.
    let mut changes = changes(&result);
    for (_, _, new) in changes.iter_mut() {
        if let Ok(formatted) = burdin_fmt::format_source(new, &burdin_fmt::Options::default()) {
            *new = formatted;
        }
    }
    // `--migrate` raises the package's edition after its migration fixes, and the result must check under the new
    // edition before anything is written (FX10).
    if let Some(plan) = &migration {
        match crate::migrate::finish(plan, &args.target, &options, &changes) {
            Ok(Some(manifest)) => changes.push(manifest),
            Ok(None) => {}
            Err(code) => return code,
        }
        changes.sort();
    }
    for (path, old, new) in &changes {
        if args.dry_run {
            print!("{}", burdin_lint::unified_diff(&diff_path(path), old, new));
        }
    }
    if !args.dry_run {
        for record in &result.applied {
            println!("fixed {}:{}: [{}] {}", record_path(record), record.line, record.code, record.message);
        }
        if let Some(plan) = &migration
            && changes.iter().any(|(p, ..)| p.file_name().is_some_and(|n| n == "burdin.toml"))
        {
            println!("migrated the package to edition {}", plan.to.as_str());
        }
    }
    for record in &result.conflicts {
        eprintln!(
            "warning: skipped a fix that conflicts with another at {}:{}: [{}] {}",
            record_path(record),
            record.line,
            record.code,
            record.message
        );
    }
    if result.reverted {
        eprintln!("warning: the fixes of the last round would add errors, so they were not applied");
    }
    if !changes.is_empty()
        && !args.dry_run
        && let Err(message) = write_all_or_nothing(&changes)
    {
        eprintln!("error: {message}; no files were changed");
        return ExitCode::from(2);
    }
    // What is left: the diagnostics of the fixed text that no fix removed.
    let all = result.last.all();
    let remaining: Vec<&Diagnostic> = all.iter().collect();
    let errors = remaining.iter().filter(|d| d.severity == Severity::Error).count();
    let warnings = remaining.len() - errors;
    if !remaining.is_empty() {
        let render = render_options(&args.diagnostics, std::io::stderr().is_terminal());
        eprint!("{}", all.render_with(&result.last.front.sources, &render).text);
    }
    let files = changes.len();
    if changes.is_empty() {
        eprintln!("fix: nothing to fix");
    } else {
        let verb = if args.dry_run { "would apply" } else { "applied" };
        eprintln!("fix: {verb} {} to {}", plural(result.applied.len(), "fix"), plural(files, "file"));
    }
    if errors + warnings > 0 {
        eprintln!("fix: {} and {} remain", plural(errors, "error"), plural(warnings, "warning"));
    }
    if errors > 0 { ExitCode::from(1) } else { ExitCode::SUCCESS }
}
