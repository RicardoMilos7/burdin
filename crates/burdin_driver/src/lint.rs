//! `burdin lint`: the built-in linter (`spec/lint.md`). The command compiles the package the way `burdin test` does (so
//! `test` blocks are checked too), runs the lints of `burdin_lint` on the typed program, and prints what they find with
//! the same diagnostics machinery as the compiler. `--fix` applies the edits the lints and the compiler mark as safe,
//! checks that the edited package still compiles, and writes every file or none.

use crate::{
    CompileOptions, DiagnosticOptions, EntryMode, FrontEnd, SourceOverrides, flag_without_value, front_end,
    jobs_or_default, option_value, parse_color, parse_format, parse_jobs, parse_number, render_options,
};
use burdin_diag::{DiagnosticFormat, Diagnostics, FileId, Severity};
use burdin_lint::{Config, Input, Level, Lint, Source};

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The lint reference, embedded so `burdin lint --explain` cannot disagree with the page `tests/check_lint.py` checks.
const LINT_DOCS: &str = include_str!("../../../docs/lints.md");

struct LintArgs {
    target: PathBuf,
    /// The level flags in command-line order: later flags win.
    levels: Vec<(String, Level)>,
    fix: bool,
    dry_run: bool,
    list: bool,
    explain: Option<String>,
    diagnostics: DiagnosticOptions,
    comptime_steps: Option<u64>,
    /// `-j`: the most parser threads.
    jobs: Option<usize>,
}

fn parse(argv: &[String]) -> Result<LintArgs, String> {
    let mut args = LintArgs {
        target: PathBuf::new(),
        levels: Vec::new(),
        fix: false,
        dry_run: false,
        list: false,
        explain: None,
        diagnostics: DiagnosticOptions::default(),
        comptime_steps: None,
        jobs: None,
    };
    let mut target = None;
    let mut it = argv.iter();
    while let Some(raw) = it.next() {
        let (a, inline) = match raw.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            // `-j4` is `-j 4`, as in cargo and make.
            _ if raw.len() > 2 && raw.starts_with("-j") => ("-j", Some(&raw[2..])),
            _ => (raw.as_str(), None),
        };
        match a {
            "-A" | "--allow" => args.levels.push((option_value(&mut it, inline, a, "a lint name")?, Level::Allow)),
            "-W" | "--warn" => args.levels.push((option_value(&mut it, inline, a, "a lint name")?, Level::Warn)),
            "-D" | "--deny" => args.levels.push((option_value(&mut it, inline, a, "a lint name")?, Level::Deny)),
            "--deny-warnings" => {
                flag_without_value(a, inline)?;
                args.levels.push(("warnings".to_string(), Level::Deny));
            }
            "-j" | "--jobs" => args.jobs = Some(parse_jobs(&mut it, inline, a)?),
            "--fix" => args.fix = flag_without_value(a, inline)?,
            "--dry-run" => args.dry_run = flag_without_value(a, inline)?,
            "--list" => args.list = flag_without_value(a, inline)?,
            "--explain" => args.explain = Some(option_value(&mut it, inline, a, "a lint name or code")?),
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
    if args.list || args.explain.is_some() {
        return Ok(args);
    }
    args.target = target.ok_or("missing target file")?;
    if args.dry_run && !args.fix {
        return Err("`--dry-run` is only valid with `--fix`".to_string());
    }
    Ok(args)
}

pub(crate) fn run(argv: &[String]) -> ExitCode {
    let args = match parse(argv) {
        Ok(a) => a,
        Err(message) => return crate::cli::usage_error("lint", &message),
    };
    if args.list {
        print_list();
        return ExitCode::SUCCESS;
    }
    if let Some(name) = &args.explain {
        return explain(name);
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
        // Test blocks are linted too, and a library package has no `main`.
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
    if args.fix { fix(&args, &options) } else { check(&args, &options) }
}

fn print_list() {
    for lint in Lint::ALL {
        println!("{}  {:<22} {:<5}  {}", lint.code(), lint.name(), lint.default_level().as_str(), lint.summary());
    }
    println!(
        "{UNKNOWN_LINT_CODE}  {UNKNOWN_LINT_NAME:<22} warn   a suppression comment names a lint that does not exist (always reported)"
    );
}

pub(crate) const UNKNOWN_LINT_NAME: &str = "unknown_lint";
pub(crate) const UNKNOWN_LINT_CODE: &str = "L0000";

fn explain(name: &str) -> ExitCode {
    let lint = Lint::from_name(name).or_else(|| Lint::from_code(&name.to_ascii_uppercase()));
    // `L0000` is not a lint that can be configured, so it is not in the registry, but it has a section.
    let heading = match lint {
        Some(l) => Some(format!("## {}: {}", l.code(), l.name())),
        None if name == UNKNOWN_LINT_NAME || name.eq_ignore_ascii_case(UNKNOWN_LINT_CODE) => {
            Some(format!("## {UNKNOWN_LINT_CODE}: {UNKNOWN_LINT_NAME}"))
        }
        None => None,
    };
    let section = heading.and_then(|heading| {
        let start = LINT_DOCS.find(&heading)?;
        let rest = &LINT_DOCS[start..];
        let end = rest[3..].find("\n## ").map_or(rest.len(), |i| 3 + i);
        Some(rest[..end].trim_end().to_string())
    });
    match section {
        Some(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("error: unknown lint `{name}` (`burdin lint --list` shows every lint)");
            ExitCode::from(2)
        }
    }
}

/// One run of the front end and the lints.
pub(crate) struct Outcome {
    pub(crate) front: FrontEnd,
    /// The modules of the entry package that the lints read: `(file, module path)`.
    pub(crate) entry: Vec<(FileId, Vec<String>)>,
    /// What the lints found. Empty when the front end reported errors.
    pub(crate) lints: Diagnostics,
}

impl Outcome {
    /// The compiler's diagnostics and the lints' together, for printing and for fixes.
    pub(crate) fn all(&self) -> Diagnostics {
        let mut all = Diagnostics::default();
        for d in self.front.diags.iter().chain(self.lints.iter()) {
            all.push(d.clone());
        }
        all
    }
}

pub(crate) fn lint_once(
    target: &Path,
    options: &CompileOptions,
    overrides: &SourceOverrides,
    levels: &[(String, Level)],
) -> Result<Outcome, ExitCode> {
    let front = front_end(target, options, overrides)?;
    let config = match build_config(&front, levels) {
        Ok(c) => c,
        Err(message) => {
            eprintln!("error: {message}");
            return Err(ExitCode::from(2));
        }
    };
    // The modules of the entry package, without the ones a build script generated under its `target` directory.
    let mut entry = Vec::new();
    let mut first = 0usize;
    for (index, package) in front.loaded.packages.iter().enumerate() {
        if index == front.loaded.entry_package {
            let generated = package.dir.as_ref().map(|d| d.join("target"));
            for (m, module) in package.modules.iter().enumerate() {
                let outside = generated.as_ref().is_some_and(|g| module.file.starts_with(g));
                if module.text.is_none() && !outside {
                    entry.push((FileId((first + m) as u32), module.path.clone()));
                }
            }
        }
        first += package.modules.len();
    }
    let mut lints = Diagnostics::default();
    if let Some(program) = front.program.as_ref().filter(|_| !front.diags.has_errors()) {
        let files: Vec<Source> = entry
            .iter()
            .map(|(file, path)| Source { file: *file, module: path, ast: &front.parsed[file.0 as usize].1 })
            .collect();
        lints = burdin_lint::run(&Input { sources: &front.sources, files, program, config: &config });
    }
    Ok(Outcome { front, entry, lints })
}

/// The levels for this run: the defaults, the manifest's `[lint]` table, then the command line.
fn build_config(front: &FrontEnd, levels: &[(String, Level)]) -> Result<Config, String> {
    let mut config = Config::default();
    let package = &front.loaded.packages[front.loaded.entry_package];
    for (name, level, line) in &package.lint {
        config.set_from_manifest(name, level).map_err(|e| {
            let manifest = package
                .dir
                .as_ref()
                .map_or_else(|| "burdin.toml".to_string(), |d| d.join("burdin.toml").display().to_string());
            format!("{manifest}:{line}: {e}")
        })?;
    }
    for (name, level) in levels {
        config.set(name, *level)?;
    }
    Ok(config)
}

/// Prints the diagnostics and the summary line. Returns whether any of them is an error.
fn report(outcome: &Outcome, args: &LintArgs) -> bool {
    let all = outcome.all();
    let render = render_options(&args.diagnostics, std::io::stderr().is_terminal());
    let rendered = all.render_with(&outcome.front.sources, &render);
    eprint!("{}", rendered.text);
    let errors = all.iter().filter(|d| d.severity == Severity::Error).count();
    let warnings = all.iter().filter(|d| d.severity == Severity::Warning).count();
    match render.format {
        DiagnosticFormat::Json => {
            let version = burdin_diag::JSON_VERSION;
            eprintln!(
                "{{\"type\":\"lint-summary\",\"version\":{version},\"warnings\":{warnings},\"errors\":{errors}}}"
            );
        }
        // The log is the whole report, even when the lints found nothing, and nothing may follow it.
        DiagnosticFormat::Sarif => {}
        _ => {
            if rendered.shown < rendered.total {
                let hidden = rendered.total - rendered.shown;
                let limit = render.error_limit.unwrap_or(0);
                eprintln!("note: {hidden} more not shown (--error-limit {limit})");
            }
            if outcome.front.diags.has_errors() {
                let s = if errors == 1 { "" } else { "s" };
                eprintln!(
                    "error: could not check {} ({errors} error{s}); fix them before linting",
                    args.target.display()
                );
            } else if errors + warnings == 0 {
                eprintln!("lint: no warnings in {}", args.target.display());
            } else {
                let (w, e) = (plural(warnings, "warning"), plural(errors, "error"));
                eprintln!("lint: {w}, {e}");
            }
        }
    }
    errors > 0
}

pub(crate) fn plural(n: usize, word: &str) -> String {
    let suffix = if n == 1 {
        ""
    } else if word.ends_with('x') {
        "es"
    } else {
        "s"
    };
    format!("{n} {word}{suffix}")
}

fn check(args: &LintArgs, options: &CompileOptions) -> ExitCode {
    match lint_once(&args.target, options, &SourceOverrides::new(), &args.levels) {
        Ok(outcome) => {
            if report(&outcome, args) {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(code) => code,
    }
}

// ----- --fix ----------------------------------------------------------------------------------------------------

fn fix(args: &LintArgs, options: &CompileOptions) -> ExitCode {
    let result = match crate::fix::fix_loop(&args.target, options, &args.levels, SourceOverrides::new()) {
        Ok(r) => r,
        Err(code) => return code,
    };
    let outcome = &result.last;
    // The package the fixes produce must compile: a fix that would break it is not applied at all.
    if outcome.front.diags.has_errors() {
        report(outcome, args);
        if !result.overrides.is_empty() {
            eprintln!("error: the fixes would leave the package with errors; no files were changed");
        }
        return ExitCode::from(1);
    }
    let changes = crate::fix::changes(&result);
    for (path, old, new) in &changes {
        print!("{}", burdin_lint::unified_diff(&diff_path(path), old, new));
    }
    if !changes.is_empty()
        && !args.dry_run
        && let Err(message) = write_all_or_nothing(&changes)
    {
        eprintln!("error: {message}; no files were changed");
        return ExitCode::from(2);
    }
    let failed = report(outcome, args);
    if changes.is_empty() {
        eprintln!("fix: nothing to fix");
    } else {
        let (edits, files) = (plural(result.applied.len(), "edit"), plural(changes.len(), "file"));
        let verb = if args.dry_run { "would apply" } else { "applied" };
        eprintln!("fix: {verb} {edits} to {files}");
    }
    if failed { ExitCode::from(1) } else { ExitCode::SUCCESS }
}

/// The path in a diff header: relative to the current directory when the file is under it, with `/` separators, and
/// never absolute, so `a/<path>` and `b/<path>` read like a `git diff`.
pub(crate) fn diff_path(path: &Path) -> String {
    // Both sides canonical: on Windows the working directory can be spelled with a short name (`RUNNER~1`) while the
    // source path is the canonical long one.
    let cwd = std::env::current_dir().ok().and_then(|cwd| cwd.canonicalize().ok());
    let relative = cwd.and_then(|cwd| path.canonicalize().ok()?.strip_prefix(cwd).ok().map(Path::to_path_buf));
    let text = crate::source_path_text(relative.as_deref().unwrap_or(path)).replace('\\', "/");
    text.trim_start_matches("./").trim_start_matches('/').to_string()
}

/// Writes every file or none: each is written beside its target and renamed over it, and when one step fails the
/// files already replaced get their old text back.
pub(crate) fn write_all_or_nothing(changes: &[(PathBuf, String, String)]) -> Result<(), String> {
    let mut done: Vec<usize> = Vec::new();
    for (i, (path, _, new)) in changes.iter().enumerate() {
        let mut temporary = path.clone().into_os_string();
        temporary.push(".burdin-fix");
        let temporary = PathBuf::from(temporary);
        let step = std::fs::write(&temporary, new).and_then(|()| std::fs::rename(&temporary, path));
        if let Err(e) = step {
            let _ = std::fs::remove_file(&temporary);
            let mut message = format!("cannot write {}: {e}", path.display());
            for &j in &done {
                if let Err(undo) = std::fs::write(&changes[j].0, &changes[j].1) {
                    message.push_str(&format!("; and cannot restore {}: {undo}", changes[j].0.display()));
                }
            }
            return Err(message);
        }
        done.push(i);
    }
    Ok(())
}
