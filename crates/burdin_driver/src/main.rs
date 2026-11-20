//! The `burdin` command: `build`, `run`, `test`, `bench`, `check`, `expand`, `asm`, `bloat`, `fmt`, `lint`, `doc`,
//! `debug`, `explain`, `bindgen`, `new`, `add`, `repl`, `lsp`, and `dap`. The command tree and its help are declared with
//! clap in `cli.rs`.
//! The contract is documented in `README.md` at the repository root (`The driver contract`) and summarized at the
//! top of tests/run_tests.py.

mod add;
mod asm;
mod bench_runner;
mod bindgen;
mod bloat;
mod build;
mod check;
mod cli;
mod debug;
mod doc;
mod emit;
mod expand;
mod export;
mod fmt;
mod fuzz_runner;
mod lint;
mod msvc;
mod new;
mod repl;
mod rust_build;
mod rust_interop;
mod sanitize;
mod scripts;
mod test_runner;

use burdin_diag::{DiagnosticFormat, Diagnostics, RenderOptions, Rule, SourceMap, Span, Tool};
pub(crate) use burdin_query::EntryMode;
pub(crate) use burdin_query::package;
pub(crate) use burdin_query::package::source_path_text;
use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[derive(Clone, Debug, Default)]
pub(crate) struct Args {
    pub(crate) command: String,
    pub(crate) target: PathBuf,
    pub(crate) release: bool,
    /// `--locked`: refuse to update `burdin.lock` (`spec/packages.md`, PKG8).
    pub(crate) locked: bool,
    /// Emit source-level debug information (DWARF in the object and the executable).
    pub(crate) debug: bool,
    /// Pass the linker's strip option (Unix `-s`; with MSVC, optimize for size).
    pub(crate) strip: bool,
    /// Emit LLVM bitcode and link with `-flto`; needs an LLVM compiler driver.
    pub(crate) lto: bool,
    /// Instrument the program for profile-guided optimization.
    pub(crate) pgo_generate: bool,
    /// `burdin build --tests`: compile the package's `test` blocks and the test dispatcher instead of `main`, the
    /// way `burdin test` builds a test binary (`spec/test-runner.md`). The debug adapter uses it to launch a single
    /// test (`spec/debug.md`, DBG11).
    pub(crate) tests: bool,
    /// Use a profile (`*.profraw` or `*.profdata`) for profile-guided optimization.
    pub(crate) pgo_use: Option<PathBuf>,
    pub(crate) output: Option<PathBuf>,
    pub(crate) emit_ir: Option<PathBuf>,
    /// `--emit <kinds>`: the artifacts to write next to the output (`burdin build` only).
    pub(crate) emit: Vec<emit::EmitKind>,
    /// `--intel` or `--att`: the dialect of `--emit asm`.
    pub(crate) asm_syntax: burdin_codegen::AsmSyntax,
    pub(crate) link_libs: Vec<String>,
    pub(crate) program_args: Vec<String>,
    pub(crate) diagnostics: DiagnosticOptions,
    /// The step budget of each compile-time evaluation (`spec/comptime.md`, CT11). `None` keeps the default.
    pub(crate) comptime_steps: Option<u64>,
    /// Where to list the files `embed` read (`--emit-deps`).
    pub(crate) emit_deps: Option<PathBuf>,
    /// `-j`: the most threads (and, for `burdin test`, test processes) at once. `None` is the default.
    pub(crate) jobs: Option<usize>,
    // `burdin test` only.
    pub(crate) verbose: bool,
    pub(crate) packages: Vec<String>,
    pub(crate) workspace: bool,
    pub(crate) ignored_only: bool,
    pub(crate) include_ignored: bool,
    /// The positional filters and every `--filter`: a test runs when its full path matches any of them.
    pub(crate) filters: Vec<String>,
    /// `--exact`: filters and skip patterns must equal the full path.
    pub(crate) exact: bool,
    /// `--skip`: a test whose full path matches any of these is left out.
    pub(crate) skip: Vec<String>,
    /// `--failed`: run only the tests that failed in the previous run.
    pub(crate) failed: bool,
    /// `--shard i/n`, with `i` counted from 1.
    pub(crate) shard: Option<(usize, usize)>,
    /// Milliseconds; `Some(0)` disables the timeout. `None` keeps the default.
    pub(crate) timeout_ms: Option<u64>,
    /// Milliseconds a test may run before the summary lists it as slow; `Some(0)` disables the list.
    pub(crate) slow_ms: Option<u64>,
    /// `--list`: print the selected tests instead of running them.
    pub(crate) list: bool,
    /// `--doc`: run only the doc tests, the Burdin code blocks of doc comments (`spec/test-runner.md`).
    pub(crate) doc_only: bool,
    /// `--format`: the report format of `burdin test`.
    pub(crate) format: TestFormat,
    /// `--junit`: where to write the JUnit XML report.
    pub(crate) junit: Option<PathBuf>,
    /// `--sanitize`: the sanitizers to build with (`spec/sanitizers.md`).
    pub(crate) sanitize: burdin_codegen::Sanitizers,
    /// `--bless` or `--review`: what happens to a snapshot that is missing or differs.
    pub(crate) snapshots: test_runner::snapshots::SnapshotMode,
    /// `burdin test --fuzz-replay`: run each fuzz target on its saved crash inputs (`spec/fuzz.md`, FZ6).
    pub(crate) fuzz_replay: bool,
    /// `burdin test --interpret`: run the tests in the interpreter instead of compiling them
    /// (`spec/test-runner.md`, IN1).
    pub(crate) interpret: bool,
    // `burdin fuzz` only.
    /// The fuzz target to run: the positional argument after the target.
    pub(crate) fuzz_name: Option<String>,
    /// `--max-time <seconds>`: stop fuzzing after this long.
    pub(crate) max_time: Option<u64>,
    /// `--minimize <file>`: shrink this crashing input instead of fuzzing.
    pub(crate) minimize: Option<PathBuf>,
    // `burdin bench` only.
    /// `--save-baseline <name>`: store the measurements under the package's target directory.
    pub(crate) save_baseline: Option<String>,
    /// `--baseline <name>`: compare each measurement with the named saved baseline.
    pub(crate) baseline: Option<String>,
    /// `--fail-on-regression <percent>`: with `--baseline`, a bench this much slower than its baseline, beyond
    /// noise, fails the run.
    pub(crate) fail_on_regression: Option<f64>,
    /// `--profile-time <seconds>`: run one selected bench in a loop for this long without measuring.
    pub(crate) profile_time: Option<f64>,
    /// `--warmup <ms>`: how long the warmup of each bench runs.
    pub(crate) warmup_ms: Option<u64>,
    /// `--sample-count <n>`: how many samples each bench takes.
    pub(crate) sample_count: Option<u32>,
    /// `--sample-time <ms>`: the target duration of one sample.
    pub(crate) sample_time_ms: Option<u64>,
}

/// The report format of `burdin test` (`--format`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TestFormat {
    #[default]
    Human,
    Json,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DiagnosticOptions {
    pub(crate) format: DiagnosticFormat,
    pub(crate) color: ColorChoice,
    /// `None` uses `COLUMNS`, then 80; `Some(0)` disables wrapping.
    pub(crate) width: Option<usize>,
    /// `None` prints every diagnostic; `Some(0)` also means every diagnostic.
    pub(crate) error_limit: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

/// The build metadata `burdin --version` prints, in a fixed order: the version and release channel, the host target,
/// the LLVM library the compiler is linked against, the git commit, and the profile.
pub(crate) fn version_text() -> String {
    format!(
        "burdin {}
channel: {}
target: {}
llvm: {}
commit: {}
profile: {}",
        env!("BURDIN_VERSION"),
        env!("BURDIN_CHANNEL"),
        env!("BURDIN_TARGET"),
        burdin_codegen::llvm_version(),
        env!("BURDIN_GIT_COMMIT"),
        env!("BURDIN_PROFILE"),
    )
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut it = argv.iter();
    let command = it.next().ok_or("missing command")?.clone();
    if !matches!(command.as_str(), "build" | "run" | "test" | "bench" | "fuzz") {
        return Err(format!("unknown command `{command}`"));
    }
    let mut args = Args { command, ..Args::default() };
    let mut target = None;
    while let Some(raw) = it.next() {
        // Long options accept `--option value` and `--option=value`.
        let (a, inline) = match raw.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            // `-j4` is `-j 4`, as in cargo and make.
            _ if raw.len() > 2 && raw.starts_with("-j") => ("-j", Some(&raw[2..])),
            _ => (raw.as_str(), None),
        };
        match a {
            "--release" => args.release = flag_without_value(a, inline)?,
            "-g" | "--debug-info" => args.debug = flag_without_value(a, inline)?,
            "--strip" => args.strip = flag_without_value(a, inline)?,
            "--lto" => args.lto = flag_without_value(a, inline)?,
            "--tests" => args.tests = flag_without_value(a, inline)?,
            "--locked" => args.locked = flag_without_value(a, inline)?,
            "--pgo-generate" => args.pgo_generate = flag_without_value(a, inline)?,
            "--pgo-use" => args.pgo_use = Some(option_value(&mut it, inline, a, "a profile path")?.into()),
            "-o" => args.output = Some(option_value(&mut it, inline, a, "a path")?.into()),
            "--emit-ir" => args.emit_ir = Some(option_value(&mut it, inline, a, "a path")?.into()),
            "--emit" => args.emit = emit::parse_kinds(&option_value(&mut it, inline, a, "a list of kinds")?)?,
            "--intel" => {
                flag_without_value(a, inline)?;
                args.asm_syntax = burdin_codegen::AsmSyntax::Intel;
            }
            "--att" => {
                flag_without_value(a, inline)?;
                args.asm_syntax = burdin_codegen::AsmSyntax::Att;
            }
            "--link-lib" => args.link_libs.push(option_value(&mut it, inline, a, "a name")?),
            "--sanitize" => {
                args.sanitize = sanitize::parse(&option_value(&mut it, inline, a, "a list of sanitizers")?)?
            }
            "--diagnostic-format" => {
                args.diagnostics.format = parse_format(&option_value(&mut it, inline, a, "a value")?)?;
            }
            "--color" => {
                args.diagnostics.color = parse_color(&option_value(&mut it, inline, a, "a value")?)?;
            }
            "--no-color" => {
                flag_without_value(a, inline)?;
                args.diagnostics.color = ColorChoice::Never;
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
            "-j" | "--jobs" => args.jobs = Some(parse_jobs(&mut it, inline, a)?),
            "-v" | "--verbose" => args.verbose = flag_without_value(a, inline)?,
            "-p" => args.packages.push(option_value(&mut it, inline, a, "a package name")?),
            "--workspace" => args.workspace = flag_without_value(a, inline)?,
            "--ignored" => args.ignored_only = flag_without_value(a, inline)?,
            "--include-ignored" => args.include_ignored = flag_without_value(a, inline)?,
            "--filter" => args.filters.push(option_value(&mut it, inline, a, "a text")?),
            "--exact" => args.exact = flag_without_value(a, inline)?,
            "--skip" => args.skip.push(option_value(&mut it, inline, a, "a pattern")?),
            "--failed" => args.failed = flag_without_value(a, inline)?,
            "--shard" => args.shard = Some(parse_shard(&option_value(&mut it, inline, a, "`<i>/<n>`")?)?),
            "--list" => args.list = flag_without_value(a, inline)?,
            "--doc" => args.doc_only = flag_without_value(a, inline)?,
            "--format" => args.format = parse_test_format(&option_value(&mut it, inline, a, "a value")?)?,
            "--junit" => args.junit = Some(option_value(&mut it, inline, a, "a path")?.into()),
            "--bless" => {
                flag_without_value(a, inline)?;
                args.snapshots = test_runner::snapshots::SnapshotMode::Bless;
            }
            "--review" => {
                flag_without_value(a, inline)?;
                args.snapshots = test_runner::snapshots::SnapshotMode::Review;
            }
            "--fuzz-replay" => args.fuzz_replay = flag_without_value(a, inline)?,
            "--interpret" => args.interpret = flag_without_value(a, inline)?,
            "--max-time" => {
                let value = option_value(&mut it, inline, a, "a number of seconds")?;
                let seconds: u64 = value
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("`--max-time` needs a positive number of seconds, got `{value}`"))?;
                args.max_time = Some(seconds);
            }
            "--minimize" => args.minimize = Some(option_value(&mut it, inline, a, "a path")?.into()),
            "--slow-threshold" => {
                let value = option_value(&mut it, inline, a, "a number of milliseconds")?;
                let ms: u64 = value
                    .parse()
                    .map_err(|_| format!("`--slow-threshold` needs a number of milliseconds, got `{value}`"))?;
                args.slow_ms = Some(ms);
            }
            "--emit-deps" => args.emit_deps = Some(option_value(&mut it, inline, a, "a path")?.into()),
            "--comptime-steps" => {
                let value = option_value(&mut it, inline, a, "a number of steps")?;
                let steps: u64 = value
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("`--comptime-steps` needs a positive number of steps, got `{value}`"))?;
                args.comptime_steps = Some(steps);
            }
            "--timeout" => {
                let value = option_value(&mut it, inline, a, "a number of milliseconds")?;
                let ms: u64 =
                    value.parse().map_err(|_| format!("`--timeout` needs a number of milliseconds, got `{value}`"))?;
                args.timeout_ms = Some(ms);
            }
            "--" => {
                args.program_args.extend(it.by_ref().cloned());
            }
            "--save-baseline" => {
                args.save_baseline = Some(option_value(&mut it, inline, a, "a name")?);
            }
            "--baseline" => args.baseline = Some(option_value(&mut it, inline, a, "a name")?),
            "--fail-on-regression" => {
                let value = option_value(&mut it, inline, a, "a percentage")?;
                let percent: f64 =
                    value.parse().map_err(|_| format!("`--fail-on-regression` needs a percentage, got `{value}`"))?;
                if !(percent.is_finite() && percent >= 0.0) {
                    return Err(format!("`--fail-on-regression` needs a non-negative percentage, got `{value}`"));
                }
                args.fail_on_regression = Some(percent);
            }
            "--profile-time" => {
                let value = option_value(&mut it, inline, a, "a number of seconds")?;
                let seconds: f64 =
                    value.parse().map_err(|_| format!("`--profile-time` needs a number of seconds, got `{value}`"))?;
                if !(seconds.is_finite() && seconds > 0.0) {
                    return Err(format!("`--profile-time` needs a positive number of seconds, got `{value}`"));
                }
                args.profile_time = Some(seconds);
            }
            "--warmup" => {
                let value = option_value(&mut it, inline, a, "a number of milliseconds")?;
                let ms: u64 =
                    value.parse().map_err(|_| format!("`--warmup` needs a number of milliseconds, got `{value}`"))?;
                args.warmup_ms = Some(ms);
            }
            "--sample-count" => {
                let value = option_value(&mut it, inline, a, "a number of samples")?;
                let n: u32 =
                    value.parse().map_err(|_| format!("`--sample-count` needs a number of samples, got `{value}`"))?;
                if n == 0 {
                    return Err("`--sample-count` must be at least 1".to_string());
                }
                args.sample_count = Some(n);
            }
            "--sample-time" => {
                let value = option_value(&mut it, inline, a, "a number of milliseconds")?;
                let ms: u64 = value
                    .parse()
                    .map_err(|_| format!("`--sample-time` needs a number of milliseconds, got `{value}`"))?;
                args.sample_time_ms = Some(ms);
            }
            _ if a.starts_with('-') => return Err(format!("unknown option `{a}`")),
            _ if target.is_none() => target = Some(PathBuf::from(a)),
            // `burdin test <target> <filter>...`: the arguments after the target are name filters. `burdin bench`
            // takes the same positional filters.
            _ if args.command == "test" || args.command == "bench" => args.filters.push(a.to_string()),
            // `burdin fuzz <target> <name>`.
            _ if args.command == "fuzz" && args.fuzz_name.is_none() => args.fuzz_name = Some(a.to_string()),
            _ => return Err(format!("unexpected argument `{a}`")),
        }
    }
    if args.pgo_generate && args.pgo_use.is_some() {
        return Err("`--pgo-generate` and `--pgo-use` are different builds; choose one".to_string());
    }
    if args.lto && (args.pgo_generate || args.pgo_use.is_some()) {
        return Err("`--lto` cannot be combined with profile-guided optimization".to_string());
    }
    if args.sanitize != burdin_codegen::Sanitizers::default() {
        if args.command == "bench" {
            return Err("`--sanitize` is only valid for `burdin build`, `burdin run`, and `burdin test`".to_string());
        }
        if args.lto || args.pgo_generate || args.pgo_use.is_some() {
            return Err("`--sanitize` cannot be combined with `--lto` or profile-guided optimization".to_string());
        }
        sanitize::check_platform(&args.sanitize, std::env::consts::OS, std::env::consts::ARCH)?;
    }
    if !args.emit.is_empty() && args.command != "build" {
        return Err("`--emit` is only valid for `burdin build`".to_string());
    }
    if args.asm_syntax != burdin_codegen::AsmSyntax::Default && !args.emit.contains(&emit::EmitKind::Asm) {
        return Err("`--intel` and `--att` only apply to `--emit asm`".to_string());
    }
    args.target = target.ok_or("missing target file")?;
    if args.command == "fuzz" {
        if args.fuzz_name.is_none() {
            return Err("missing the fuzz target's name: `burdin fuzz <target> <name>`".to_string());
        }
        if args.sanitize != burdin_codegen::Sanitizers::default()
            || args.lto
            || args.pgo_generate
            || args.pgo_use.is_some()
        {
            return Err(
                "`burdin fuzz` builds with libFuzzer and AddressSanitizer; `--sanitize`, `--lto`, and PGO do not apply"
                    .to_string(),
            );
        }
        if args.tests || args.output.is_some() || !args.emit.is_empty() || args.emit_ir.is_some() {
            return Err(
                "`burdin fuzz` builds and runs its own binary; `--tests`, `-o`, and `--emit` do not apply".to_string()
            );
        }
    } else if args.max_time.is_some() || args.minimize.is_some() {
        return Err("`--max-time` and `--minimize` are only valid for `burdin fuzz`".to_string());
    }
    if args.fuzz_replay && args.command != "test" {
        return Err("`--fuzz-replay` is only valid for `burdin test`".to_string());
    }
    if args.interpret && args.command != "test" {
        return Err("`--interpret` is only valid for `burdin test`".to_string());
    }
    if args.interpret && args.fuzz_replay {
        return Err("`--interpret` runs the tests, and a fuzz replay needs a compiled program; choose one".to_string());
    }
    if args.interpret && args.snapshots != test_runner::snapshots::SnapshotMode::Check {
        return Err("`--bless` and `--review` need a compiled run; `--interpret` cannot write snapshots".to_string());
    }
    if args.interpret && args.sanitize != burdin_codegen::Sanitizers::default() {
        return Err("`--interpret` runs no compiled code, so `--sanitize` does not apply".to_string());
    }
    if args.interpret
        && (args.junit.is_some()
            || args.shard.is_some()
            || args.failed
            || args.slow_ms.is_some()
            || args.doc_only
            || args.format != TestFormat::Human)
    {
        return Err("`--interpret` does not take `--doc`, `--junit`, `--shard`, `--failed`, `--slow-threshold`, or \
             `--format`; run the tests compiled for those"
            .to_string());
    }
    let tests = args.command == "test";
    let benches = args.command == "bench";
    if !tests && !benches {
        if args.verbose {
            return Err("`--verbose` is only valid for `burdin test` and `burdin bench`".to_string());
        }
        if !args.packages.is_empty() || args.workspace {
            return Err("`-p` and `--workspace` are only valid for `burdin test` and `burdin bench`".to_string());
        }
        if !args.filters.is_empty() {
            return Err("`--filter` is only valid for `burdin test` and `burdin bench`".to_string());
        }
        if args.exact || !args.skip.is_empty() {
            return Err("`--exact` and `--skip` are only valid for `burdin test` and `burdin bench`".to_string());
        }
        if args.list {
            return Err("`--list` is only valid for `burdin test` and `burdin bench`".to_string());
        }
        if args.format != TestFormat::Human {
            return Err("`--format` is only valid for `burdin test` and `burdin bench`".to_string());
        }
        if args.timeout_ms.is_some() {
            return Err("`--timeout` is only valid for `burdin test` and `burdin bench`".to_string());
        }
    }
    if !tests {
        if args.ignored_only || args.include_ignored {
            return Err("`--ignored` and `--include-ignored` are only valid for `burdin test`".to_string());
        }
        if args.failed || args.shard.is_some() || args.junit.is_some() {
            return Err("`--failed`, `--shard`, and `--junit` are only valid for `burdin test`".to_string());
        }
        if args.slow_ms.is_some() {
            return Err("`--slow-threshold` is only valid for `burdin test`".to_string());
        }
        if args.doc_only {
            return Err("`--doc` is only valid for `burdin test`".to_string());
        }
        if args.snapshots != test_runner::snapshots::SnapshotMode::Check {
            return Err("`--bless` and `--review` are only valid for `burdin test`".to_string());
        }
    }
    if !benches {
        if args.save_baseline.is_some()
            || args.baseline.is_some()
            || args.fail_on_regression.is_some()
            || args.profile_time.is_some()
            || args.warmup_ms.is_some()
            || args.sample_count.is_some()
            || args.sample_time_ms.is_some()
        {
            return Err("`--save-baseline`, `--baseline`, `--fail-on-regression`, `--profile-time`, `--warmup`, \
                 `--sample-count`, and `--sample-time` are only valid for `burdin bench`"
                .to_string());
        }
    } else if !args.program_args.is_empty() {
        return Err("`burdin bench` takes no program arguments: the runner starts the measured program".to_string());
    }
    if argv.iter().any(|a| a == "--bless") && argv.iter().any(|a| a == "--review") {
        return Err(
            "`--bless` and `--review` cannot be used together: one accepts every snapshot, the other asks".to_string()
        );
    }
    if args.list && args.junit.is_some() {
        return Err("`--list` and `--junit` cannot be used together: a listing has no results".to_string());
    }
    if args.fail_on_regression.is_some() && args.baseline.is_none() {
        return Err("`--fail-on-regression` needs `--baseline <name>` to compare with".to_string());
    }
    if args.profile_time.is_some() && (args.list || args.save_baseline.is_some() || args.baseline.is_some()) {
        return Err(
            "`--profile-time` runs one bench for a profiler and produces no measurements; it cannot be combined with `--list`, `--save-baseline`, or `--baseline`"
                .to_string(),
        );
    }
    if args.ignored_only && args.include_ignored {
        return Err("`--ignored` and `--include-ignored` cannot be used together".to_string());
    }
    if args.workspace && !args.packages.is_empty() {
        return Err("`-p` and `--workspace` cannot be used together".to_string());
    }
    Ok(args)
}

/// Handles one of the diagnostics options shared by every command (`--diagnostic-format`, `--color`, `--no-color`,
/// `--width`, `--error-limit`). Returns whether `option` was one of them.
pub(crate) fn parse_diagnostic_option(
    it: &mut std::slice::Iter<'_, String>,
    inline: Option<&str>,
    option: &str,
    diagnostics: &mut DiagnosticOptions,
) -> Result<bool, String> {
    match option {
        "--diagnostic-format" => diagnostics.format = parse_format(&option_value(it, inline, option, "a value")?)?,
        "--color" => diagnostics.color = parse_color(&option_value(it, inline, option, "a value")?)?,
        "--no-color" => {
            flag_without_value(option, inline)?;
            diagnostics.color = ColorChoice::Never;
        }
        "--width" => {
            let value = option_value(it, inline, option, "a number")?;
            diagnostics.width = Some(parse_number(option, &value)?);
        }
        "--error-limit" => {
            let value = option_value(it, inline, option, "a number")?;
            let limit = parse_number(option, &value)?;
            diagnostics.error_limit = (limit > 0).then_some(limit);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// `-j <n>`/`--jobs <n>`, shared by every command that compiles or formats: a positive number of threads.
pub(crate) fn parse_jobs(
    it: &mut std::slice::Iter<'_, String>,
    inline: Option<&str>,
    option: &str,
) -> Result<usize, String> {
    let value = option_value(it, inline, option, "a number of jobs")?;
    let n: usize = value.parse().map_err(|_| format!("`-j` needs a number of jobs, got `{value}`"))?;
    if n == 0 {
        return Err("`-j` must be at least 1".to_string());
    }
    Ok(n)
}

/// The thread count a command runs with: `-j`, then `BURDIN_JOBS`, then 0 for the machine's parallelism.
pub(crate) fn jobs_or_default(jobs: Option<usize>) -> usize {
    jobs.or_else(burdin_query::env_jobs).unwrap_or(0)
}

/// `BURDIN_CODEGEN_UNITS`: split code generation into exactly this many units instead of choosing by size. The
/// conformance suite runs with it set to check that a split program behaves like an unsplit one.
fn env_codegen_units() -> Option<usize> {
    std::env::var("BURDIN_CODEGEN_UNITS").ok().and_then(|v| v.trim().parse::<usize>().ok()).filter(|n| *n > 0)
}

/// The value of an option: the part after `=` when written inline, otherwise the next argument.
fn option_value(
    it: &mut std::slice::Iter<'_, String>,
    inline: Option<&str>,
    option: &str,
    what: &str,
) -> Result<String, String> {
    match inline {
        Some(value) => Ok(value.to_string()),
        None => it.next().cloned().ok_or_else(|| format!("`{option}` needs {what}")),
    }
}

/// A flag that takes no value; `--release=yes` is a usage error rather than being ignored.
fn flag_without_value(option: &str, inline: Option<&str>) -> Result<bool, String> {
    match inline {
        Some(_) => Err(format!("`{option}` takes no value")),
        None => Ok(true),
    }
}

pub(crate) fn parse_format(value: &str) -> Result<DiagnosticFormat, String> {
    match value {
        "human" => Ok(DiagnosticFormat::Human),
        "short" => Ok(DiagnosticFormat::Short),
        "json" => Ok(DiagnosticFormat::Json),
        "sarif" => Ok(DiagnosticFormat::Sarif),
        _ => Err(format!("unknown diagnostic format `{value}` (expected human, short, json, or sarif)")),
    }
}

fn parse_test_format(value: &str) -> Result<TestFormat, String> {
    match value {
        "human" => Ok(TestFormat::Human),
        "json" => Ok(TestFormat::Json),
        _ => Err(format!("unknown test format `{value}` (expected human or json)")),
    }
}

/// `--shard <i>/<n>`: the i-th of n slices, counted from 1.
fn parse_shard(value: &str) -> Result<(usize, usize), String> {
    let bad = || format!("`--shard` needs `<i>/<n>` with 1 <= i <= n, got `{value}`");
    let (index, count) = value.split_once('/').ok_or_else(bad)?;
    let index: usize = index.parse().map_err(|_| bad())?;
    let count: usize = count.parse().map_err(|_| bad())?;
    if index == 0 || count == 0 || index > count {
        return Err(bad());
    }
    Ok((index, count))
}

pub(crate) fn parse_color(value: &str) -> Result<ColorChoice, String> {
    match value {
        "auto" => Ok(ColorChoice::Auto),
        "always" => Ok(ColorChoice::Always),
        "never" => Ok(ColorChoice::Never),
        _ => Err(format!("unknown color choice `{value}` (expected auto, always, or never)")),
    }
}

pub(crate) fn parse_number(option: &str, value: &str) -> Result<usize, String> {
    value.parse::<usize>().map_err(|_| format!("`{option}` needs a non-negative number, got `{value}`"))
}

/// The compiler runs on a thread with a large stack: the comptime interpreter recurses once per nested expression
/// and call it evaluates, up to the call depth `spec/comptime.md` (CT11) allows.
const COMPILER_STACK: usize = 1 << 30;

fn main() -> ExitCode {
    match std::thread::Builder::new().name("burdin".into()).stack_size(COMPILER_STACK).spawn(driver_main) {
        Ok(handle) => handle.join().unwrap_or(ExitCode::from(101)),
        Err(e) => {
            eprintln!("error: cannot start the compiler thread: {e}");
            ExitCode::from(2)
        }
    }
}

/// `burdin lsp`: the language server on standard input and output (`spec/lsp.md`). The exit code is the protocol's:
/// 0 after `shutdown` and `exit`, 1 when the client exits without shutting down or the input closes first.
fn lsp(args: &[String]) -> ExitCode {
    if let Some(other) = args.iter().find(|a| *a != "--stdio") {
        return cli::usage_error(
            "lsp",
            &format!("unknown argument `{other}` for `burdin lsp` (it speaks LSP on standard input and output)"),
        );
    }
    ExitCode::from(burdin_lsp::serve_stdio() as u8)
}

/// `burdin dap`: the Debug Adapter Protocol server on standard input and output (`spec/debug.md`, DBG11). It
/// forwards to lldb-dap and builds a launch request's `target` with this very executable.
fn dap(args: &[String]) -> ExitCode {
    if let Some(other) = args.iter().find(|a| *a != "--stdio") {
        return cli::usage_error(
            "dap",
            &format!("unknown argument `{other}` for `burdin dap` (it speaks DAP on standard input and output)"),
        );
    }
    let driver = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("burdin"));
    ExitCode::from(burdin_dap::serve_stdio_with_driver(driver) as u8)
}

fn driver_main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let command = match cli::route(&argv) {
        cli::Route::Exit(code) => return code,
        cli::Route::Command(command) => command,
    };
    let rest = &argv[1..];
    match command.as_str() {
        "fmt" => return fmt::run(rest),
        "check" => return check::run(rest),
        "asm" => return asm::run(rest),
        "bloat" => return bloat::run(rest),
        "expand" => return expand::run(rest),
        "explain" => return explain(rest),
        "bindgen" => return bindgen::run(rest),
        "lint" => return lint::run(rest),
        "doc" => return doc::run(rest),
        "debug" => return debug::run(rest),
        "new" => return new::run(rest),
        "add" => return add::run(rest),
        "lsp" => return lsp(rest),
        "dap" => return dap(rest),
        "repl" => return repl::run(rest),
        _ => {}
    }
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(msg) => return cli::usage_error(&command, &msg),
    };
    let emit = (!args.emit.is_empty()).then(|| emit::Emit {
        kinds: args.emit.clone(),
        asm_syntax: args.asm_syntax,
        skip_link: false,
    });
    let options = CompileOptions {
        release: args.release,
        debug: args.debug,
        strip: args.strip,
        lto: args.lto,
        pgo_generate: args.pgo_generate,
        pgo_use: args.pgo_use.as_deref(),
        emit_ir: args.emit_ir.as_deref(),
        emit: emit.as_ref(),
        link_libs: &args.link_libs,
        mode: if args.tests { EntryMode::Test } else { EntryMode::Build },
        diagnostics: &args.diagnostics,
        comptime_steps: args.comptime_steps,
        emit_deps: args.emit_deps.as_deref(),
        report_clean: args.command == "build",
        run_build_scripts: true,
        locked: args.locked,
        jobs: jobs_or_default(args.jobs),
        sanitize: args.sanitize,
        fuzz_target: None,
    };
    match args.command.as_str() {
        "build" => match compile(&args.target, Path::new("."), args.output.as_deref(), &options) {
            Ok(_) => ExitCode::SUCCESS,
            Err(code) => code,
        },
        "run" => run(&args, &options),
        "bench" => bench_runner::run(&args),
        "fuzz" => fuzz_runner::run(&args),
        _ => test_runner::run(&args),
    }
}

/// `burdin explain`: prints the reference entry for one code, or lists the codes. The text comes from the same
/// page `tests/check_error_docs.py` compiles examples from (issue #437).
fn explain(args: &[String]) -> ExitCode {
    match args {
        [] => {
            print!("{}", code_list());
            ExitCode::SUCCESS
        }
        [code] => match explain_section(code) {
            Some(section) => {
                println!("{section}");
                if let Some(url) = burdin_diag::code_url(&code.to_ascii_uppercase()) {
                    println!("\nOnline: {url}");
                }
                ExitCode::SUCCESS
            }
            None => cli::usage_error("explain", &format!("unknown error code `{code}`")),
        },
        _ => cli::usage_error("explain", "`explain` takes one error code"),
    }
}

/// The section of the error reference for `code`, as `burdin explain` prints it: the page
/// `tests/check_error_docs.py` checks, read through `burdin_diag` so the REPL's `:explain` prints the same text.
fn explain_section(code: &str) -> Option<String> {
    burdin_diag::explain_section(code)
}

/// Every heading in the reference, one per line, as `E0001: Syntax error`.
fn code_list() -> String {
    burdin_diag::code_list()
}

fn run(args: &Args, options: &CompileOptions) -> ExitCode {
    // A one-file target that makes itself a script runs from a content-addressed cache (`spec/scripts.md`, SC5).
    if let Some(code) = scripts::run_script(args, options) {
        return code;
    }
    let dir = match TempDir::new() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot create a temporary directory: {e}");
            return ExitCode::from(2);
        }
    };
    let exe = match compile(&args.target, &dir.path, None, options) {
        Ok(compiled) => compiled.exe,
        Err(code) => return code,
    };
    let status = match Command::new(&exe).args(&args.program_args).status() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot run {}: {e}", exe.display());
            return ExitCode::from(2);
        }
    };
    ExitCode::from(exit_code_of(status))
}

pub(crate) fn exit_code_of(status: std::process::ExitStatus) -> u8 {
    if let Some(code) = status.code() {
        // Windows exit codes are 32-bit; keep a failure a failure when truncating.
        let low = (code & 0xff) as u8;
        return if code != 0 && low == 0 { 1 } else { low };
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128u8.wrapping_add(signal as u8);
        }
    }
    1
}

/// The options of one compilation.
pub(crate) struct CompileOptions<'a> {
    pub(crate) release: bool,
    /// Emit source-level debug information.
    pub(crate) debug: bool,
    /// Strip the executable.
    pub(crate) strip: bool,
    /// Emit bitcode and link with `-flto`.
    pub(crate) lto: bool,
    /// Instrument for PGO.
    pub(crate) pgo_generate: bool,
    /// Use this profile for PGO.
    pub(crate) pgo_use: Option<&'a Path>,
    pub(crate) emit_ir: Option<&'a Path>,
    /// The artifacts `--emit` asked for (`spec/inspect.md`, INS7); `None` writes none.
    pub(crate) emit: Option<&'a emit::Emit>,
    pub(crate) link_libs: &'a [String],
    /// `burdin test`: declare and check the package's `test` blocks, and emit the test dispatcher instead of
    /// `main`. `burdin bench` does the same for `bench` blocks (`EntryMode::Bench`).
    pub(crate) mode: EntryMode,
    /// How diagnostics are rendered (format, color, width, and limit).
    pub(crate) diagnostics: &'a DiagnosticOptions,
    /// The step budget of each compile-time evaluation; `None` is the default (`spec/comptime.md`, CT11).
    pub(crate) comptime_steps: Option<u64>,
    /// Where to list the files `embed` read; `None` writes nothing.
    pub(crate) emit_deps: Option<&'a Path>,
    /// Write a SARIF log even when the build found nothing (`burdin build` only).
    pub(crate) report_clean: bool,
    /// Run the package's `build.bd` (and its granted dependencies' scripts) before reading sources. A build
    /// script's own compilation passes `false`, so a script cannot run another script (BS3).
    pub(crate) run_build_scripts: bool,
    /// `--locked`: a `burdin.lock` that does not match the manifests is an error instead of being rewritten
    /// (`spec/packages.md`, PKG8).
    pub(crate) locked: bool,
    /// The most threads parsing and code generation use (`-j`); 0 is the machine's parallelism.
    pub(crate) jobs: usize,
    /// The sanitizers to instrument for and link (`--sanitize`, `spec/sanitizers.md`).
    pub(crate) sanitize: burdin_codegen::Sanitizers,
    /// With [`EntryMode::Fuzz`], the fuzz target the binary is built for (`spec/fuzz.md`, FZ3).
    pub(crate) fuzz_target: Option<&'a str>,
}

/// What a successful compilation produced: the executable, its `test` blocks and `bench` blocks (empty for a
/// normal build), and the files a build script declared as its inputs (empty when there was no script).
pub(crate) struct Compiled {
    pub(crate) exe: PathBuf,
    pub(crate) tests: Vec<burdin_tir::TestEntry>,
    /// Where each test is declared, in the order of `tests`.
    pub(crate) test_sites: Vec<Site>,
    /// The `bench` blocks of a bench build, empty for every other build.
    pub(crate) benches: Vec<burdin_tir::BenchEntry>,
    /// Where each bench is declared, in the order of `benches`.
    pub(crate) bench_sites: Vec<Site>,
    /// The `fuzz` targets of a test or fuzz build, empty for every other build (`spec/fuzz.md`).
    pub(crate) fuzzes: Vec<burdin_tir::FuzzEntry>,
    /// Where each fuzz target is declared, in the order of `fuzzes`.
    pub(crate) fuzz_sites: Vec<Site>,
    pub(crate) build_inputs: Vec<String>,
}

/// The source file and line a `test` or `bench` block is declared at.
pub(crate) struct Site {
    pub(crate) file: String,
    pub(crate) line: u32,
}

/// Everything the front end produced: the loaded packages, the parsed files, the type-checked program, and the
/// diagnostics (errors and warnings) of parsing, checking, and compile-time evaluation.
pub(crate) struct FrontEnd {
    pub(crate) loaded: package::Workspace,
    /// One entry per source file, indexed by `FileId`: the file's path and whether it is embedded in the driver.
    pub(crate) files: Vec<(PathBuf, bool)>,
    /// The parsed files, in `FileId` order.
    pub(crate) parsed: Vec<(burdin_diag::FileId, burdin_syntax::ast::File)>,
    pub(crate) sources: SourceMap,
    /// `None` when parsing failed: checking a tree with syntax errors would mostly report follow-on errors.
    pub(crate) program: Option<burdin_tir::Program>,
    pub(crate) diags: Diagnostics,
    pub(crate) script_inputs: Vec<String>,
    /// The shim libraries of the bound Rust crates, which the program links after `--link-lib`'s.
    pub(crate) rust_libs: Vec<String>,
}

/// Resolves the dependencies of a package directory and brings its `burdin.lock` up to date before the package is
/// loaded (`spec/packages.md`, PKG8): git dependencies are fetched into the cache, and with `locked` a lockfile that
/// would change is an error. A one-file target has no lockfile. Errors are printed; the exit status is 1.
pub(crate) fn sync_dependencies(target: &Path, locked: bool) -> Result<(), ExitCode> {
    if !target.join("burdin.toml").is_file() {
        return Ok(());
    }
    match burdin_pkg::resolve::sync(target, locked) {
        Ok(_) => Ok(()),
        Err(message) => {
            eprintln!("error: {message}");
            Err(ExitCode::from(1))
        }
    }
}

/// Replacement texts for source files, by path: `burdin lint --fix` checks the program it is about to write.
pub(crate) type SourceOverrides = std::collections::HashMap<PathBuf, String>;

/// Loads, parses, and checks `target` and returns what that produced, errors included. `Err` means the package could
/// not be loaded or read at all (already reported). Nothing is printed for diagnostics: the caller decides.
///
/// The parsing and checking is the query library's (`burdin_query::Db`), the same code a language server runs, so
/// the command line and a tool cannot disagree about a program (`spec/query.md`, QL4).
pub(crate) fn front_end(
    target: &Path,
    options: &CompileOptions,
    overrides: &SourceOverrides,
) -> Result<FrontEnd, ExitCode> {
    let mode = options.mode;
    sync_dependencies(target, options.locked)?;
    let mut loaded = match if mode.wants_main() { package::load(target) } else { package::load_for_tests(target) } {
        Ok(loaded) => loaded,
        Err(message) => {
            eprintln!("error: {message}");
            return Err(ExitCode::from(1));
        }
    };
    // Build scripts run first: a generated module must exist before the modules are read (`spec/build-scripts.md`,
    // BS3-BS4). A script's own compilation sets this flag to false, so there is no recursion.
    let mut script_inputs: Vec<String> = Vec::new();
    let mut rust_libs: Vec<String> = Vec::new();
    if options.run_build_scripts {
        match build::run(&mut loaded, options.release, options) {
            Ok(outcome) => {
                script_inputs = outcome.inputs;
                rust_libs = outcome.link_libs;
            }
            Err(failure) => {
                report_build_failure(&loaded, failure, options);
                return Err(ExitCode::from(1));
            }
        }
    }

    // One file per module, in module order: the ids the source map hands out are the ids `files` indexes.
    let mut file_info: Vec<(PathBuf, bool)> = Vec::new();
    for package in &loaded.packages {
        for module in &package.modules {
            file_info.push((module.file.clone(), module.text.is_some()));
        }
    }
    // A batch build asks once and exits, so it does not keep the checked bodies a language server would.
    let query_options = burdin_query::Options {
        mode,
        release: options.release,
        comptime_steps: options.comptime_steps,
        no_cache: true,
        jobs: options.jobs,
    };
    let mut db = match burdin_query::Db::from_workspace(&loaded, overrides, query_options) {
        Ok(db) => db,
        Err(message) => {
            eprintln!("error: {message}");
            return Err(ExitCode::from(1));
        }
    };
    // Nothing cancels a batch run, so the only failure here is a bug in the database itself.
    let analysis = match db.analyze() {
        Ok(analysis) => analysis,
        Err(cancelled) => {
            eprintln!("error: {cancelled}");
            return Err(ExitCode::from(1));
        }
    };
    let (sources, parsed) = db.into_parts();
    Ok(FrontEnd {
        loaded,
        files: file_info,
        parsed,
        sources,
        program: analysis.program,
        diags: analysis.diagnostics,
        script_inputs,
        rust_libs,
    })
}

/// Compiles `target` into an executable: `output` when given, otherwise `<output_dir>/<package name>`. On failure
/// the diagnostics are already printed. With [`EntryMode::Test`] or [`EntryMode::Bench`], the package's `test` or
/// `bench` blocks are compiled instead of its `main`.
pub(crate) fn compile(
    target: &Path,
    output_dir: &Path,
    output: Option<&Path>,
    options: &CompileOptions,
) -> Result<Compiled, ExitCode> {
    let FrontEnd { loaded, files, sources, program, diags, script_inputs, rust_libs, .. } =
        front_end(target, options, &SourceOverrides::new())?;
    if diags.has_errors() {
        report_errors("compile", target, &diags, &sources, options.diagnostics);
        return Err(ExitCode::from(1));
    }
    let mut program = program.expect("no errors means checking produced a program");
    if options.mode == EntryMode::Fuzz {
        let name = options.fuzz_target.unwrap_or_default();
        match program.fuzzes.iter().position(|fuzz| fuzz.name == name) {
            Some(index) => program.fuzz_target = Some(index),
            None => {
                let names: Vec<String> = program.fuzzes.iter().map(|fuzz| format!("`{}`", fuzz.name)).collect();
                let known = if names.is_empty() { "it has none".to_string() } else { names.join(", ") };
                eprintln!("error: {} has no fuzz target `{name}`; its targets: {known}", target.display());
                return Err(ExitCode::from(2));
            }
        }
    }
    // A build system watching `--emit-deps` must rebuild when a header the script declared changes too.
    program.build_inputs.extend(script_inputs);
    let mut deps = String::new();
    for input in &program.build_inputs {
        deps.push_str(input);
        deps.push('\n');
    }
    if let Some(path) = options.emit_deps {
        write_artifact(path, &deps)?;
    }
    // Warnings do not stop compilation, but they are printed before the build goes on (`spec/extensions.md`,
    // EX4: a shadowed extension method). They render like errors, with the `warning` severity.
    print_warnings(&diags, &sources, options.diagnostics, options.report_clean);

    let output = match output {
        Some(path) => path.to_path_buf(),
        None => output_dir.join(&loaded.stem).with_extension(std::env::consts::EXE_EXTENSION),
    };
    // `burdin build app` from the package's parent directory would otherwise ask the linker to replace the package
    // directory with a file.
    if output.is_dir() {
        eprintln!("error: the output path `{}` is a directory; pass `-o <file>`", output.display());
        return Err(ExitCode::from(2));
    }
    if let Some(emit) = options.emit {
        if emit.wants(emit::EmitKind::Deps) {
            write_artifact(&emit.path(&output, emit::EmitKind::Deps), &deps)?;
        }
        if emit.wants(emit::EmitKind::Tir) {
            let is_std = |file: burdin_diag::FileId| files.get(file.0 as usize).is_some_and(|(_, embedded)| *embedded);
            let text = burdin_inspect::tir::dump(&program, &sources, &is_std);
            write_artifact(&emit.path(&output, emit::EmitKind::Tir), &text)?;
        }
    }
    emit_program(&program, &sources, &loaded.stem, &output, options, &rust_libs)?;
    let site = |span: burdin_diag::Span| {
        let file = sources.get(span.file);
        Site { file: file.path.clone(), line: file.line_col(span.lo).0 }
    };
    let test_sites = program.tests.iter().map(|test| site(test.span)).collect();
    let bench_sites = program.benches.iter().map(|bench| site(bench.span)).collect();
    let fuzz_sites = program.fuzzes.iter().map(|fuzz| site(fuzz.span)).collect();
    Ok(Compiled {
        exe: output,
        tests: program.tests,
        test_sites,
        benches: program.benches,
        bench_sites,
        fuzzes: program.fuzzes,
        fuzz_sites,
        build_inputs: program.build_inputs,
    })
}

/// Writes one artifact file, reporting a failure the way the driver reports every I/O error.
pub(crate) fn write_artifact(path: &Path, text: &str) -> Result<(), ExitCode> {
    std::fs::write(path, text).map_err(|e| {
        eprintln!("error: cannot write {}: {e}", path.display());
        ExitCode::from(2)
    })
}

/// Emits the object files of the checked `program` and links them into `output`. This is the half of `compile` after
/// the front end; the doc-test builds (`test_runner/doc_tests.rs`) share it, rendering a failed build themselves
/// instead of printing it here.
pub(crate) fn emit_program(
    program: &burdin_tir::Program,
    sources: &SourceMap,
    stem: &str,
    output: &Path,
    options: &CompileOptions,
    rust_libs: &[String],
) -> Result<(), ExitCode> {
    let fail = |msg: String| {
        eprintln!("error: {msg}");
        ExitCode::from(2)
    };
    let dir = TempDir::new().map_err(|e| fail(format!("cannot create a temporary directory: {e}")))?;
    let extension = if options.lto {
        "bc"
    } else if cfg!(windows) {
        "obj"
    } else {
        "o"
    };
    let object = dir.path.join(format!("program.{extension}"));
    let pgo_ir = options.pgo_use.map(|_| dir.path.join("program.pgo.ll"));
    let emit = options.emit;
    let artifact = |kind: emit::EmitKind| emit.filter(|e| e.wants(kind)).map(|e| e.path(output, kind));
    let opts = burdin_codegen::Options {
        release: options.release,
        emit_ir: options.emit_ir.map(Path::to_path_buf).or_else(|| artifact(emit::EmitKind::LlvmIr)),
        pgo_ir,
        debug: options.debug,
        bitcode: options.lto,
        instrument: options.pgo_generate,
        jobs: options.jobs,
        // One object file is what `--emit obj` promises.
        units: if emit.is_some_and(|e| e.wants(emit::EmitKind::Obj)) { Some(1) } else { env_codegen_units() },
        emit_opt_ir: artifact(emit::EmitKind::LlvmIrOpt),
        emit_bitcode: artifact(emit::EmitKind::LlvmBc),
        emit_asm: artifact(emit::EmitKind::Asm),
        asm_syntax: emit.map_or(Default::default(), |e| e.asm_syntax),
        sanitize: options.sanitize,
    };
    // A large program is split into codegen units compiled on several threads, one object each (issue #334).
    let objects = burdin_codegen::emit_object(program, sources, stem, &object, &opts).map_err(fail)?;
    if emit.is_some_and(|e| e.wants(emit::EmitKind::Obj)) {
        for (index, object) in objects.iter().enumerate() {
            let target = emit::object_path(output, index);
            std::fs::copy(object, &target).map_err(|e| fail(format!("cannot write {}: {e}", target.display())))?;
        }
    }
    if emit.is_some_and(|e| e.skip_link) {
        return Ok(());
    }
    let objects = match options.pgo_use {
        Some(profile) => {
            let ir = dir.path.join("program.pgo.ll");
            pgo_object(&dir.path, &ir, profile, options.release).map_err(fail)?;
            vec![dir.path.join(if cfg!(windows) { "program.pgo.obj" } else { "program.pgo.o" })]
        }
        None => objects,
    };
    let link_opts = LinkOptions {
        export_dynamic: cfg!(target_os = "linux"),
        debug: options.debug,
        strip: options.strip,
        lto: options.lto,
        pgo_generate: options.pgo_generate,
        sanitize: options.sanitize,
    };
    let libs: Vec<String> = options.link_libs.iter().chain(rust_libs).cloned().collect();
    link(&objects, output, &libs, &link_opts).map_err(fail)?;
    Ok(())
}

/// Prints the errors of a failed front end and the closing line, `could not <verb> <target> (N errors)`: `compile`
/// for `build`, `run`, and `test`, `check` for `burdin check`. Everything goes to standard error.
pub(crate) fn report_errors(
    verb: &str,
    target: &Path,
    diags: &Diagnostics,
    sources: &SourceMap,
    diagnostics: &DiagnosticOptions,
) {
    let options = render_options(diagnostics, std::io::stderr().is_terminal());
    let rendered = diags.render_with(sources, &options);
    eprint!("{}", rendered.text);
    let total = rendered.total;
    let suffix = if total == 1 { "" } else { "s" };
    match options.format {
        DiagnosticFormat::Json => {
            eprint!("{}", summary_json(total, rendered.shown, options.error_limit));
        }
        // A SARIF log is one document, so nothing may follow it.
        DiagnosticFormat::Sarif => {}
        _ => {
            if rendered.shown < total {
                let hidden = total - rendered.shown;
                let plural = if hidden == 1 { "" } else { "s" };
                let limit = options.error_limit.unwrap_or(0);
                eprintln!("note: {hidden} more error{plural} not shown (--error-limit {limit})");
            }
            eprintln!("error: could not {verb} {} ({total} error{suffix})", target.display());
            if options.format == DiagnosticFormat::Human {
                eprintln!("note: for an explanation of an error code, run `burdin explain <code>`");
            }
        }
    }
}

/// Prints the warnings of a front end that found no errors; nothing when there are none. With `report_clean`, a
/// SARIF log is written even when there is nothing to report, so a CI job always has a file to upload
/// (`spec/diagnostics.md`, DG11).
pub(crate) fn print_warnings(
    diags: &Diagnostics,
    sources: &SourceMap,
    diagnostics: &DiagnosticOptions,
    report_clean: bool,
) {
    let render = render_options(diagnostics, std::io::stderr().is_terminal());
    let clean_log = report_clean && render.format == DiagnosticFormat::Sarif;
    if diags.has_warnings() || clean_log {
        eprint!("{}", diags.render_with(sources, &render).text);
    }
}

/// Reports a build-script failure at the entry package's first source line. The failure is about the package, not
/// a particular expression, so the span is the start of the file the test marks; the message names the script and
/// any note says how to fix it (`spec/build-scripts.md`, BS6).
fn report_build_failure(loaded: &package::Workspace, failure: build::Failure, options: &CompileOptions) {
    let entry = &loaded.packages[loaded.entry_package].modules[loaded.entry_module];
    let text = std::fs::read_to_string(&entry.file).unwrap_or_default();
    let mut sources = SourceMap::default();
    let file = sources.add(source_path_text(&entry.file), text);
    let mut diags = Diagnostics::default();
    let diagnostic = diags.error(failure.code, Span::new(file, 0, 0), failure.message);
    if let Some(note) = failure.note {
        diagnostic.note_without_span(note);
    }
    let render = render_options(options.diagnostics, std::io::stderr().is_terminal());
    eprint!("{}", diags.render_with(&sources, &render).text);
}

/// The rendering choices for this run: the format from the command line, color from the choice, the environment,
/// and whether stderr is a terminal, and the width from `--width`, then `COLUMNS`, then 80.
pub(crate) fn render_options(diagnostics: &DiagnosticOptions, stderr_is_terminal: bool) -> RenderOptions {
    let columns = std::env::var("COLUMNS").ok();
    let color = color_enabled(diagnostics.color, no_color_requested(), stderr_is_terminal);
    RenderOptions {
        format: diagnostics.format,
        color,
        width: terminal_width(diagnostics.width, columns.as_deref()),
        error_limit: diagnostics.error_limit,
        hyperlinks: color && hyperlinks_supported(&|name| std::env::var(name).ok()),
        tool: if diagnostics.format == DiagnosticFormat::Sarif { sarif_tool() } else { Tool::default() },
    }
}

/// Whether the terminal understands OSC 8 hyperlinks. `FORCE_HYPERLINK` decides when it is set (`0` is off, anything
/// else is on); otherwise a `dumb` terminal and the multiplexers that swallow the sequence say no, and the terminals
/// known to support it say yes. A terminal this list does not know gets plain text, which is always correct.
fn hyperlinks_supported(env: &dyn Fn(&str) -> Option<String>) -> bool {
    if let Some(force) = env("FORCE_HYPERLINK").filter(|value| !value.is_empty()) {
        return force != "0";
    }
    let term = env("TERM").unwrap_or_default();
    if term == "dumb" || term.starts_with("screen") || term.starts_with("tmux") || env("TMUX").is_some() {
        return false;
    }
    if env("WT_SESSION").is_some() || env("KITTY_WINDOW_ID").is_some() || env("KONSOLE_VERSION").is_some() {
        return true;
    }
    if env("VTE_VERSION").and_then(|version| version.parse::<u32>().ok()).is_some_and(|version| version >= 5000) {
        return true;
    }
    if matches!(
        env("TERM_PROGRAM").as_deref(),
        Some("iTerm.app" | "WezTerm" | "vscode" | "ghostty" | "Hyper" | "Tabby" | "rio")
    ) {
        return true;
    }
    term.contains("kitty") || term.contains("alacritty") || term.contains("foot") || term.contains("ghostty")
}

/// The tool a SARIF log names: the compiler's version, and one rule for every error code of the reference and every
/// lint, each with the link to its entry (`spec/diagnostics.md`, DG9).
fn sarif_tool() -> Tool {
    let mut rules = Vec::new();
    // `code_list` prints one `CODE: Name` line per registry code.
    for line in burdin_diag::code_list().lines() {
        let Some((code, name)) = line.split_once(':') else { continue };
        let bytes = code.as_bytes();
        if bytes.len() == 5 && bytes[0] == b'E' && bytes[1..].iter().all(u8::is_ascii_digit) {
            rules.push(Rule {
                id: code.to_string(),
                name: name.trim().to_string(),
                help_uri: burdin_diag::code_url(code),
            });
        }
    }
    let lints = std::iter::once((lint::UNKNOWN_LINT_CODE, lint::UNKNOWN_LINT_NAME))
        .chain(burdin_lint::Lint::ALL.iter().map(|lint| (lint.code(), lint.name())));
    for (code, name) in lints {
        rules.push(Rule { id: code.to_string(), name: name.to_string(), help_uri: burdin_diag::code_url(code) });
    }
    Tool {
        name: "burdin".to_string(),
        version: env!("BURDIN_VERSION").to_string(),
        information_uri: burdin_diag::URL_BASE.to_string(),
        rules,
    }
}

/// `--color=always` and `--color=never` are explicit and win over `NO_COLOR`; `auto` colors only a terminal
/// whose environment does not ask for no color.
fn color_enabled(choice: ColorChoice, no_color_env: bool, stderr_is_terminal: bool) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => stderr_is_terminal && !no_color_env,
    }
}

/// The no-color.org convention: `NO_COLOR` disables color when it is present and not empty.
fn no_color_requested() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty())
}

/// The width in characters: `--width` when given, then a positive `COLUMNS`, then 80. Zero disables wrapping.
fn terminal_width(explicit: Option<usize>, columns: Option<&str>) -> usize {
    if let Some(width) = explicit {
        return width;
    }
    columns.and_then(|value| value.trim().parse::<usize>().ok()).filter(|width| *width > 0).unwrap_or(80)
}

/// The final line of `--diagnostic-format=json`: one object with the counts, so a JSON consumer never has to
/// parse the human summary.
fn summary_json(errors: usize, shown: usize, limit: Option<usize>) -> String {
    let limit = limit.map_or_else(|| "null".to_string(), |limit| limit.to_string());
    format!(
        "{{\"type\":\"summary\",\"version\":{},\"errors\":{errors},\"shown\":{shown},\"limit\":{limit}}}\n",
        burdin_diag::JSON_VERSION
    )
}

/// Runs `--pgo-use`: merges the profile, applies `pgo-instr-use` with `opt`, and compiles the result with `llc`.
/// `opt` is used because the LLVM C API does not expose the pass builder's profile options.
fn pgo_object(dir: &Path, ir: &Path, profile: &Path, release: bool) -> Result<(), String> {
    if !profile.is_file() {
        return Err(format!("the profile `{}` does not exist", profile.display()));
    }
    let profdata_tool = llvm_tool("llvm-profdata")
        .ok_or("`--pgo-use` needs `llvm-profdata`; install the LLVM 21 tools or put them on PATH")?;
    let profdata = dir.join("merged.profdata");
    run_tool(&profdata_tool, &["merge".into(), "-o".into(), profdata.clone().into(), profile.into()])?;
    let opt = llvm_tool("opt").ok_or("`--pgo-use` needs `opt`; install the LLVM 21 tools or put them on PATH")?;
    let llc = llvm_tool("llc").ok_or("`--pgo-use` needs `llc`; install the LLVM 21 tools or put them on PATH")?;
    let pipeline = if release { "pgo-instr-use,default<O2>" } else { "pgo-instr-use" };
    let bitcode = dir.join("program.pgo.bc");
    run_tool(
        &opt,
        &[
            format!("-passes={pipeline}").into(),
            format!("-pgo-test-profile-file={}", profdata.display()).into(),
            ir.into(),
            "-o".into(),
            bitcode.clone().into(),
        ],
    )?;
    let object = dir.join(if cfg!(windows) { "program.pgo.obj" } else { "program.pgo.o" });
    run_tool(
        &llc,
        &["-relocation-model=pic".into(), "-filetype=obj".into(), bitcode.into(), "-o".into(), object.into()],
    )
}

/// Runs `tool` with `args`, reporting its output when it fails.
fn run_tool(tool: &Path, args: &[OsString]) -> Result<(), String> {
    let out = Command::new(tool).args(args).output().map_err(|e| format!("cannot run {}: {e}", tool.display()))?;
    if !out.status.success() {
        return Err(format!(
            "{} failed:\n{}{}",
            tool.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// Finds an LLVM command-line tool: `$LLVM_SYS_211_PREFIX/bin/<name>[-21]` first, then the versioned and plain
/// names on `PATH`. A candidate counts only when it runs.
fn llvm_tool(name: &str) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(prefix) = std::env::var_os("LLVM_SYS_211_PREFIX") {
        let bin = PathBuf::from(prefix).join("bin");
        for candidate in [format!("{name}-21"), name.to_string(), format!("{name}-21.1")] {
            let path = bin.join(&candidate);
            candidates.push(path.with_extension(std::env::consts::EXE_EXTENSION));
            candidates.push(path);
        }
    }
    for candidate in [format!("{name}-21"), name.to_string()] {
        candidates.push(PathBuf::from(&candidate));
        if !std::env::consts::EXE_EXTENSION.is_empty() {
            candidates.push(PathBuf::from(format!("{candidate}.{}", std::env::consts::EXE_EXTENSION)));
        }
    }
    candidates.into_iter().find(|candidate| {
        Command::new(candidate).arg("--version").output().map(|out| out.status.success()).unwrap_or(false)
    })
}

/// The link-time choices that change the linker command line.
#[derive(Default)]
struct LinkOptions {
    /// Put the program's symbols in the dynamic symbol table (ELF), for panic backtraces.
    export_dynamic: bool,
    /// Keep source-level debug information (`-g`). On macOS, `link` also writes the `.dSYM` bundle.
    debug: bool,
    /// Strip the executable (`--strip`).
    strip: bool,
    /// The input is LLVM bitcode and the linker must run LTO (`--lto`).
    lto: bool,
    /// The program carries profile instrumentation, so the profile runtime must be linked (`--pgo-generate`).
    pgo_generate: bool,
    /// The sanitizer runtimes to link (`--sanitize`, `burdin fuzz`); they need clang as the linker.
    sanitize: burdin_codegen::Sanitizers,
}

/// Links the object files (which define `main` and the runtime) with libc into the executable `output`.
fn link(objects: &[PathBuf], output: &Path, libs: &[String], opts: &LinkOptions) -> Result<(), String> {
    if opts.sanitize.needs_runtime() {
        // The runtimes are compiler-rt's, which clang links with `-fsanitize=` (`spec/sanitizers.md`, SAN3).
        let cc = sanitize::linker()?;
        let out = Command::new(&cc)
            .args(unix_link_args(objects, output, libs, opts))
            .output()
            .map_err(|e| linker_error(&cc, e))?;
        if !out.status.success() {
            return Err(format!(
                "linking failed ({cc}):\n{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        return Ok(());
    }
    let named = std::env::var("BURDIN_CC").ok();
    let mut cc = named.clone().unwrap_or_else(|| if cfg!(windows) { "cl".into() } else { "cc".into() });
    // Outside a Developer Command Prompt `LIB` is not set and link.exe cannot find the C runtime, so find the
    // installation and name its library directories (issue #826). clang-cl finds them itself.
    let toolchain = if cfg!(windows) && is_cl(&cc) && !msvc::lib_env_is_set() {
        Some(msvc::discover(&msvc::Probe::from_env())?)
    } else {
        None
    };
    if let Some(toolchain) = &toolchain
        && named.is_none()
    {
        // The default `cl` comes from the same installation as the libraries, so it need not be on PATH.
        cc = toolchain.cl.display().to_string();
    }
    if is_msvc_style(&cc) && (opts.lto || opts.pgo_generate) {
        return Err(format!(
            "`--lto` and `--pgo-generate` need an LLVM compiler driver, and `{cc}` is not one; \
             set BURDIN_CC to clang or clang-cl"
        ));
    }
    if opts.debug && is_msvc_style(&cc) {
        // `link.exe` embeds the file in the PDB with `/NATVIS:`; it has to exist before the link runs
        // (`spec/debug.md`, DBG8).
        write_natvis(output)?;
    }
    let mut cmd = Command::new(&cc);
    cmd.args(link_args(&cc, objects, output, libs, opts));
    if let Some(toolchain) = &toolchain {
        cmd.args(toolchain.libpath_args());
        if named.is_none() {
            // `cl.exe` starts `link.exe` from PATH; put the installation's own first.
            cmd.env("PATH", path_with(toolchain.cl.parent().unwrap_or(Path::new("."))));
        }
    }
    let out = cmd.output().map_err(|e| linker_error(&cc, e))?;
    if !out.status.success() {
        let hint = if opts.lto || opts.pgo_generate {
            "\nnote: linking LLVM bitcode or the profile runtime needs an LLVM compiler driver; set BURDIN_CC=clang"
        } else {
            ""
        };
        return Err(format!(
            "linking failed ({cc}):\n{}{}{hint}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    if opts.debug && cfg!(target_os = "macos") {
        write_dsym(output)?;
    }
    Ok(())
}

/// The Windows visualizers, embedded at compile time (`spec/debug.md`, DBG8). `-g` writes them beside the
/// executable and hands them to the linker, which embeds them in the PDB.
const NATVIS: &str = include_str!("../../../debuggers/windows/burdin.natvis");

/// Where `link` writes the Natvis file: `<output>.natvis`, next to the executable.
fn natvis_path(output: &Path) -> PathBuf {
    let mut path = output.as_os_str().to_os_string();
    path.push(".natvis");
    PathBuf::from(path)
}

fn write_natvis(output: &Path) -> Result<(), String> {
    let path = natvis_path(output);
    std::fs::write(&path, NATVIS).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// ld64 keeps DWARF out of the executable and writes a debug map instead; `dsymutil` combines the object's DWARF
/// into `<output>.dSYM`, which is where lldb and other Darwin debuggers read it from.
fn write_dsym(output: &Path) -> Result<(), String> {
    let mut dsym = output.as_os_str().to_os_string();
    dsym.push(".dSYM");
    let dsym = PathBuf::from(dsym);
    let out = Command::new("dsymutil").arg(output).arg("-o").arg(&dsym).output().map_err(|e| {
        format!(
            "cannot run `dsymutil`: {e}; source-level debug information on macOS needs the Xcode command-line tools"
        )
    })?;
    if !out.status.success() {
        return Err(format!("`dsymutil` failed for `{}`:\n{}", output.display(), String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

/// The error for a linker that did not start. A missing `cl` is by far the most common case on Windows: the driver
/// falls back to `cl`, but it is only on `PATH` inside a Developer Command Prompt.
fn linker_error(cc: &str, error: std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        format!(
            "cannot find the linker `{cc}` on PATH; run from a Visual Studio Developer Command Prompt \
             (so `cl` is on PATH) or set BURDIN_CC"
        )
    } else {
        format!("cannot run the linker `{cc}`: {error} (set BURDIN_CC)")
    }
}

/// The linker command line. Split out so the MSVC branch, which cannot run on Linux CI, has unit tests.
fn link_args(cc: &str, objects: &[PathBuf], output: &Path, libs: &[String], opts: &LinkOptions) -> Vec<OsString> {
    if is_msvc_style(cc) {
        // `LLVM-C.lib` lives in the LLVM installation, which is not in the linker's `LIB` path. `LLVM_SYS_211_PREFIX`
        // and `llvm-config` are how the rest of the build finds that installation.
        let llvm_libdir = libs.iter().any(|l| l == "LLVM-C").then(resolve_llvm_c_libdir).flatten();
        let mut args = msvc_link_args(objects, output, libs, llvm_libdir.as_deref());
        args.extend(msvc_debug_args(output, opts));
        if opts.strip {
            // Without `/DEBUG` link.exe writes no debug directory; `/OPT:REF` drops unreferenced sections.
            args.push("/OPT:REF".into());
            args.push("/OPT:ICF".into());
        }
        args
    } else {
        unix_link_args(objects, output, libs, opts)
    }
}

/// `PATH` with `dir` in front.
fn path_with(dir: &Path) -> OsString {
    let rest = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&rest))).unwrap_or(rest)
}

/// MSVC's own `cl` or `cl.exe`, bare or as a path (not clang-cl, which finds the libraries itself).
fn is_cl(cc: &str) -> bool {
    let name = cc.rsplit(['/', '\\']).next().unwrap_or(cc);
    name.eq_ignore_ascii_case("cl") || name.eq_ignore_ascii_case("cl.exe")
}

/// `cl` and `cl.exe`, bare or as a path; `clang-cl` accepts the same flags. Both separators are recognized so the
/// check does not depend on the host `Path` semantics.
fn is_msvc_style(cc: &str) -> bool {
    let name = cc.rsplit(['/', '\\']).next().unwrap_or(cc);
    name.eq_ignore_ascii_case("cl")
        || name.eq_ignore_ascii_case("cl.exe")
        || name.eq_ignore_ascii_case("clang-cl")
        || name.eq_ignore_ascii_case("clang-cl.exe")
}

fn msvc_link_args(objects: &[PathBuf], output: &Path, libs: &[String], llvm_libdir: Option<&Path>) -> Vec<OsString> {
    // MSVC: cl only calls link.exe. The object files name no C runtime, so name the static one.
    let mut args: Vec<OsString> = vec!["/nologo".into()];
    args.extend(objects.iter().map(OsString::from));
    args.push(format!("/Fe:{}", output.display()).into());
    args.extend(
        [
            "/link",
            "libcmt.lib",
            "libucrt.lib",
            "libvcruntime.lib",
            // The runtime calls `CaptureStackBackTrace` for panic backtraces and Winsock for sockets. An object
            // emitted by LLVM carries no default-library directives, so name these explicitly; `cl` does not add
            // them for a bare object.
            "kernel32.lib",
            "ws2_32.lib",
        ]
        .map(OsString::from),
    );
    if let Some(dir) = llvm_libdir {
        args.push(format!("/LIBPATH:{}", dir.display()).into());
    }
    for lib in libs {
        if is_library_path(lib) {
            args.push(lib.into());
        } else {
            args.push(format!("{lib}.lib").into());
        }
    }
    args
}

/// The MSVC link arguments a `-g` build adds: `/DEBUG` writes the PDB, and `/NATVIS:` embeds the Burdin
/// visualizers in it so Visual Studio and WinDbg show a `List` as its elements (`spec/debug.md`, DBG8).
/// `/INCREMENTAL:NO` is what `/DEBUG` otherwise turns off: an incremental link makes a function's address a jump
/// thunk, which is not where its return addresses are, so the panic backtrace's frame table would name the wrong
/// functions.
fn msvc_debug_args(output: &Path, opts: &LinkOptions) -> Vec<OsString> {
    if !opts.debug {
        return Vec::new();
    }
    let natvis = natvis_path(output);
    vec!["/DEBUG".into(), "/INCREMENTAL:NO".into(), format!("/NATVIS:{}", natvis.display()).into()]
}

/// Whether a `--link-lib` value names a file (a path, or a library or object file name) rather than a library to look
/// up, so `--link-lib build/wrappers.a` links that file as it is. This is how a C file that `burdin bindgen --wrappers`
/// generated joins the link (`spec/bindgen.md`, BG8).
fn is_library_path(lib: &str) -> bool {
    let lower = lib.to_ascii_lowercase();
    lib.contains(['/', '\\'])
        || [".a", ".o", ".obj", ".lib", ".so", ".dylib", ".dll"].iter().any(|extension| lower.ends_with(extension))
}

fn unix_link_args(objects: &[PathBuf], output: &Path, libs: &[String], opts: &LinkOptions) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["-o".into(), output.into()];
    args.extend(objects.iter().map(OsString::from));
    if opts.debug {
        // ld64 keeps debug sections only when the link names -g; GNU ld ignores the flag.
        args.push("-g".into());
    }
    if opts.export_dynamic {
        args.push("-rdynamic".into());
    }
    if opts.lto {
        args.push("-flto".into());
    }
    if opts.pgo_generate {
        args.push("-fprofile-instr-generate".into());
    }
    if opts.sanitize.needs_runtime() {
        args.push(format!("-fsanitize={}", opts.sanitize.link_list()).into());
    }
    if opts.strip {
        args.push("-s".into());
        if cfg!(target_os = "macos") {
            // `-s` removes the symbol table but Mach-O still exports every global function through the export
            // trie, and `nm` reports those names. A stripped executable needs to export only its entry point.
            args.push("-Wl,-exported_symbol,_main".into());
        }
    }
    for lib in libs {
        if is_library_path(lib) {
            args.push(lib.into());
            continue;
        }
        match resolve_llvm_c(lib) {
            // `LLVM-C` is the C API of the LLVM shared library. Ubuntu installs it as `libLLVM-21.so`, not as
            // `libLLVM-C.so`, so `llvm-config` says which directory and flag to use.
            Some((dir, flag)) => {
                args.push(format!("-L{}", dir.display()).into());
                args.push(format!("-Wl,-rpath,{}", dir.display()).into());
                args.push(flag.into());
            }
            None => args.push(format!("-l{lib}").into()),
        }
    }
    // The float functions (`spec/stdlib.md`, "Float functions") call the C math library. It is a separate
    // library on Linux; macOS folds it into libSystem, where `-lm` still links.
    if !libs.iter().any(|l| l == "m") {
        args.push("-lm".into());
    }
    args
}

/// The directory holding `LLVM-C.lib` on Windows/MSVC, from `LLVM_SYS_211_PREFIX` or `llvm-config`.
fn resolve_llvm_c_libdir() -> Option<PathBuf> {
    if let Some(prefix) = std::env::var_os("LLVM_SYS_211_PREFIX") {
        let dir = PathBuf::from(prefix).join("lib");
        if dir.join("LLVM-C.lib").is_file() {
            return Some(dir);
        }
    }
    llvm_config().map(|(dir, _)| dir).filter(|dir| dir.join("LLVM-C.lib").is_file())
}

/// `--link-lib LLVM-C` on a Unix host: the directory to search and the flag that names the LLVM shared library
/// which exports the C API. Other names are left to the linker's own search path.
fn resolve_llvm_c(name: &str) -> Option<(PathBuf, String)> {
    if name != "LLVM-C" {
        return None;
    }
    let (dir, libs) = llvm_config()?;
    llvm_c_link(&dir, libs)
}

/// Picks the linker flag for the LLVM shared library in `dir`. Linux installs `libLLVM-C.so` or a versioned
/// `libLLVM-21.so`; Homebrew on macOS installs `libLLVM.dylib`. `libs` is what `llvm-config --link-shared --libs`
/// reported, used when the directory listing does not name a single library.
fn llvm_c_link(dir: &Path, libs: Option<String>) -> Option<(PathBuf, String)> {
    for stem in ["LLVM-C", "LLVM"] {
        for extension in ["so", "dylib"] {
            if dir.join(format!("lib{stem}.{extension}")).is_file() {
                return Some((dir.to_path_buf(), format!("-l{stem}")));
            }
        }
    }
    if let Some(flag) = libs {
        return Some((dir.to_path_buf(), flag));
    }
    // The versioned development symlink: `libLLVM-21.so` or `libLLVM-21.dylib` is `-lLLVM-21`.
    let mut found = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let file = entry.file_name().to_string_lossy().to_string();
        if let Some(stem) =
            file.strip_prefix("lib").and_then(|f| f.strip_suffix(".so").or_else(|| f.strip_suffix(".dylib")))
            && stem.starts_with("LLVM-")
        {
            found = Some(format!("-l{stem}"));
            break;
        }
    }
    found.map(|flag| (dir.to_path_buf(), flag))
}

/// The LLVM installation's library directory and the shared-library flag `llvm-config` reports for it. The tool is
/// looked up in `LLVM_SYS_211_PREFIX` (the variable that configures the `inkwell` build) and then on `PATH`, because
/// Homebrew's LLVM is keg-only and its `bin` directory is usually not on `PATH`.
fn llvm_config() -> Option<(PathBuf, Option<String>)> {
    let mut tools = Vec::new();
    if let Some(prefix) = std::env::var_os("LLVM_SYS_211_PREFIX") {
        tools.push(PathBuf::from(prefix).join("bin").join("llvm-config"));
    }
    tools.extend(
        ["llvm-config", "llvm-config-21", "llvm-config-20", "llvm-config-19", "llvm-config-18", "llvm-config-17"]
            .map(PathBuf::from),
    );
    for tool in tools {
        let Ok(out) = Command::new(&tool).arg("--libdir").output() else { continue };
        if !out.status.success() {
            continue;
        }
        let dir = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        if !dir.is_dir() {
            continue;
        }
        let libs = Command::new(&tool)
            .args(["--link-shared", "--libs"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| {
                let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
                (text.starts_with("-l") && !text.contains(' ')).then_some(text)
            });
        return Some((dir, libs));
    }
    None
}

/// A uniquely named temporary directory, removed when dropped. The driver and the parallel test runner create
/// these from several threads, so the name includes an atomic counter as well as the process id and the time.
pub(crate) struct TempDir {
    pub(crate) path: PathBuf,
}

impl TempDir {
    pub(crate) fn new() -> std::io::Result<TempDir> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        for attempt in 0..100u32 {
            let count = COUNTER.fetch_add(1, Ordering::Relaxed);
            let name = format!("burdin-{}-{nanos}-{count}-{attempt}", std::process::id());
            let path = std::env::temp_dir().join(name);
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(TempDir { path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::other("no free temporary directory name"))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The page `burdin_diag` embeds, read here directly so the checks below see the same text a release ships.
    const ERROR_DOCS: &str = include_str!("../../../docs/error-codes.md");

    fn as_strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn verbatim_windows_paths_are_spelled_without_the_prefix() {
        // `fs::canonicalize` returns `\\?\C:\...` on Windows; diagnostics and leak sites show the ordinary path.
        assert_eq!(package::strip_verbatim_prefix(r"\\?\C:\a\b.bd"), r"C:\a\b.bd");
        assert_eq!(package::strip_verbatim_prefix(r"\\?\UNC\server\share\a.bd"), r"\\server\share\a.bd");
        assert_eq!(package::strip_verbatim_prefix(r"\\server\share\a.bd"), r"\\server\share\a.bd");
        assert_eq!(package::strip_verbatim_prefix("/a/b.bd"), "/a/b.bd");
    }

    #[test]
    fn version_metadata_has_the_documented_fields() {
        let text = version_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 6, "{text}");
        assert!(lines[0].starts_with("burdin "), "{text}");
        for prefix in ["channel: ", "target: ", "llvm: ", "commit: ", "profile: "] {
            assert!(lines.iter().any(|line| line.starts_with(prefix)), "missing {prefix}: {text}");
        }
        let channel = lines.iter().find(|line| line.starts_with("channel: ")).unwrap();
        assert!(matches!(&channel["channel: ".len()..], "dev" | "nightly" | "stable"), "unexpected channel: {channel}");
        let llvm = lines.iter().find(|line| line.starts_with("llvm: ")).unwrap();
        assert!(llvm["llvm: ".len()..].split('.').count() == 3, "{llvm}");
    }

    #[test]
    fn msvc_link_args_name_the_static_crt_and_map_libraries_to_lib_files() {
        let args = as_strings(msvc_link_args(
            &[PathBuf::from("program.obj")],
            Path::new("out.exe"),
            &["LLVM-C".to_string()],
            None,
        ));
        assert_eq!(args[0], "/nologo");
        assert!(args.contains(&"/link".to_string()));
        assert!(args.contains(&"libcmt.lib".to_string()));
        assert!(args.contains(&"libucrt.lib".to_string()));
        assert!(args.contains(&"libvcruntime.lib".to_string()));
        assert!(args.contains(&"kernel32.lib".to_string()));
        assert!(args.contains(&"ws2_32.lib".to_string()));
        assert!(args.contains(&"LLVM-C.lib".to_string()));
        assert!(!args.iter().any(|arg| arg.starts_with("/LIBPATH:")));
    }

    #[test]
    fn msvc_link_args_add_the_llvm_library_directory() {
        let args = as_strings(msvc_link_args(
            &[PathBuf::from("program.obj")],
            Path::new("out.exe"),
            &["LLVM-C".to_string()],
            Some(Path::new(r"C:\llvm\lib")),
        ));
        assert!(args.contains(&r"/LIBPATH:C:\llvm\lib".to_string()));
    }

    #[test]
    fn msvc_debug_builds_write_the_pdb_and_embed_the_natvis() {
        let opts = LinkOptions { debug: true, ..Default::default() };
        let args = as_strings(link_args("cl", &[PathBuf::from("program.obj")], Path::new("out.exe"), &[], &opts));
        assert!(args.contains(&"/DEBUG".to_string()), "{args:?}");
        assert!(args.contains(&"/INCREMENTAL:NO".to_string()), "{args:?}");
        assert!(args.contains(&"/NATVIS:out.exe.natvis".to_string()), "{args:?}");
        // The visualizers are the repository file, embedded in the driver.
        assert!(NATVIS.contains("<AutoVisualizer"), "the embedded natvis is empty");
        let plain = as_strings(link_args(
            "cl",
            &[PathBuf::from("program.obj")],
            Path::new("out.exe"),
            &[],
            &LinkOptions::default(),
        ));
        assert!(!plain.iter().any(|arg| arg == "/DEBUG" || arg.starts_with("/NATVIS:")), "{plain:?}");
    }

    #[test]
    fn msvc_linker_is_recognized_by_name_or_path() {
        for cc in ["cl", "cl.exe", "CL.EXE", r"C:\VS\bin\cl.exe", "clang-cl", "clang-cl.exe"] {
            assert!(is_msvc_style(cc), "{cc} should use the MSVC command line");
        }
        for cc in ["cc", "cc.exe", "gcc", "clang"] {
            assert!(!is_msvc_style(cc), "{cc} should not use the MSVC command line");
        }
    }

    #[test]
    fn only_msvc_cl_needs_the_library_discovery() {
        for cc in ["cl", "CL.EXE", r"C:\VS\bin\Hostx64\x64\cl.exe"] {
            assert!(is_cl(cc), "{cc} is MSVC's cl");
        }
        for cc in ["clang-cl", "clang-cl.exe", "cc", "clang"] {
            assert!(!is_cl(cc), "{cc} finds its libraries itself");
        }
    }

    #[test]
    fn the_installation_directory_goes_first_on_path() {
        let dir = Path::new("toolchain-bin");
        let path = path_with(dir);
        assert_eq!(std::env::split_paths(&path).next().as_deref(), Some(dir));
    }

    /// An empty directory removed when dropped; each test gets its own.
    struct LibDir(PathBuf);

    impl LibDir {
        fn new(name: &str) -> LibDir {
            let path = std::env::temp_dir().join(format!("burdin-link-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).unwrap();
            LibDir(path)
        }

        fn touch(&self, name: &str) {
            fs::write(self.0.join(name), b"").unwrap();
        }
    }

    impl Drop for LibDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn unix_link_args_use_the_cc_command_line() {
        let args = as_strings(unix_link_args(
            &[PathBuf::from("program.o")],
            Path::new("out"),
            &["m".to_string()],
            &LinkOptions::default(),
        ));
        assert_eq!(args, ["-o", "out", "program.o", "-lm"]);
    }

    #[test]
    fn a_link_lib_that_names_a_file_is_linked_as_it_is() {
        let libs = ["build/wrappers.a".to_string(), "extra.o".to_string(), "z".to_string()];
        let unix =
            as_strings(unix_link_args(&[PathBuf::from("program.o")], Path::new("out"), &libs, &LinkOptions::default()));
        assert_eq!(unix, ["-o", "out", "program.o", "build/wrappers.a", "extra.o", "-lz", "-lm"]);
        let msvc = as_strings(msvc_link_args(&[PathBuf::from("program.obj")], Path::new("out.exe"), &libs, None));
        assert!(msvc.contains(&"build/wrappers.a".to_string()), "{msvc:?}");
        assert!(msvc.contains(&"extra.o".to_string()), "{msvc:?}");
        assert!(msvc.contains(&"z.lib".to_string()), "{msvc:?}");
    }

    #[test]
    fn unix_link_args_link_the_math_library_once() {
        // The float functions call libm, so every Unix program links it; naming it again with `--link-lib m`
        // does not repeat the flag.
        let args =
            as_strings(unix_link_args(&[PathBuf::from("program.o")], Path::new("out"), &[], &LinkOptions::default()));
        assert_eq!(args, ["-o", "out", "program.o", "-lm"]);
    }

    #[test]
    fn unix_link_args_keep_debug_information() {
        let opts = LinkOptions { debug: true, ..Default::default() };
        let args = as_strings(unix_link_args(&[PathBuf::from("program.o")], Path::new("out"), &[], &opts));
        assert!(args.contains(&"-g".to_string()), "{args:?}");
    }

    #[test]
    fn unix_link_args_export_symbols_for_backtraces() {
        let opts = LinkOptions { export_dynamic: true, ..Default::default() };
        let args = as_strings(unix_link_args(&[PathBuf::from("program.o")], Path::new("out"), &[], &opts));
        assert!(args.contains(&"-rdynamic".to_string()), "{args:?}");
    }

    #[test]
    fn a_missing_linker_is_reported_with_a_fix() {
        let error = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
        let message = linker_error("cl", error);
        assert!(message.contains("cannot find the linker `cl` on PATH"), "{message}");
        assert!(message.contains("Developer Command Prompt"), "{message}");
        assert!(message.contains("BURDIN_CC"), "{message}");
    }

    #[test]
    fn llvm_c_prefers_the_c_api_library() {
        let dir = LibDir::new("c-api");
        dir.touch("libLLVM.so");
        dir.touch("libLLVM-C.so");
        let (found, flag) = llvm_c_link(&dir.0, None).unwrap();
        assert_eq!(found, dir.0);
        assert_eq!(flag, "-lLLVM-C");
    }

    #[test]
    fn llvm_c_finds_the_homebrew_dylib() {
        let dir = LibDir::new("homebrew");
        dir.touch("libLLVM.dylib");
        let (_, flag) = llvm_c_link(&dir.0, None).unwrap();
        assert_eq!(flag, "-lLLVM");
    }

    #[test]
    fn llvm_c_finds_a_versioned_dylib() {
        let dir = LibDir::new("versioned-dylib");
        dir.touch("libLLVM-21.dylib");
        let (_, flag) = llvm_c_link(&dir.0, None).unwrap();
        assert_eq!(flag, "-lLLVM-21");
    }

    #[test]
    fn llvm_c_falls_back_to_the_llvm_config_flag() {
        let dir = LibDir::new("llvm-config");
        let (_, flag) = llvm_c_link(&dir.0, Some("-lLLVM-21".to_string())).unwrap();
        assert_eq!(flag, "-lLLVM-21");
    }

    #[test]
    fn llvm_c_ignores_other_library_names() {
        let dir = LibDir::new("empty");
        assert!(llvm_c_link(&dir.0, None).is_none());
        assert!(resolve_llvm_c("m").is_none());
    }

    fn parse(words: &[&str]) -> Result<Args, String> {
        let argv: Vec<String> = words.iter().map(|word| word.to_string()).collect();
        parse_args(&argv)
    }

    #[test]
    fn test_options_are_parsed() {
        let args = parse(&[
            "test",
            "app",
            "-j",
            "3",
            "--verbose",
            "-p",
            "lib",
            "--filter",
            "add",
            "--timeout",
            "250",
            "--ignored",
        ])
        .unwrap();
        assert_eq!(args.command, "test");
        assert_eq!(args.target, PathBuf::from("app"));
        assert_eq!(args.jobs, Some(3));
        assert!(args.verbose);
        assert_eq!(args.packages, ["lib"]);
        assert_eq!(args.filters, ["add"]);
        assert_eq!(args.timeout_ms, Some(250));
        assert!(args.ignored_only);
    }

    #[test]
    fn positional_filters_follow_the_target() {
        let args =
            parse(&["test", "app", "geometry", "--filter", "area", "circle", "--exact", "--skip", "slow"]).unwrap();
        assert_eq!(args.target, PathBuf::from("app"));
        assert_eq!(args.filters, ["geometry", "area", "circle"]);
        assert!(args.exact);
        assert_eq!(args.skip, ["slow"]);
        assert!(parse(&["build", "app", "extra"]).unwrap_err().contains("unexpected argument `extra`"));
    }

    #[test]
    fn listing_report_and_selection_options_are_parsed() {
        let args = parse(&[
            "test",
            "app",
            "--list",
            "--format",
            "json",
            "--failed",
            "--shard",
            "2/3",
            "--slow-threshold",
            "100",
        ])
        .unwrap();
        assert!(args.list && args.failed);
        assert_eq!(args.format, TestFormat::Json);
        assert_eq!(args.shard, Some((2, 3)));
        assert_eq!(args.slow_ms, Some(100));
        assert_eq!(parse(&["test", "app", "--junit", "report.xml"]).unwrap().junit, Some(PathBuf::from("report.xml")));
        assert_eq!(parse(&["test", "app", "--format=human"]).unwrap().format, TestFormat::Human);
    }

    #[test]
    fn bad_shards_and_formats_are_rejected() {
        for shard in ["0/3", "4/3", "1/0", "1", "a/b", "1/2/3", "-1/2"] {
            assert!(parse(&["test", "app", "--shard", shard]).unwrap_err().contains("`--shard` needs"), "{shard}");
        }
        assert!(parse(&["test", "app", "--format", "xml"]).unwrap_err().contains("unknown test format"));
        assert!(parse(&["test", "app", "--slow-threshold", "soon"]).unwrap_err().contains("milliseconds"));
        assert!(parse(&["test", "app", "--list", "--junit", "r.xml"]).unwrap_err().contains("cannot be used together"));
    }

    #[test]
    fn the_new_test_options_are_rejected_for_other_commands() {
        for option in [
            ["build", "app", "--list"],
            ["build", "app", "--exact"],
            ["run", "app", "--failed"],
            ["build", "app", "--junit"],
        ] {
            let mut argv = option.to_vec();
            if argv[2] == "--junit" {
                argv.push("r.xml");
            }
            assert!(parse(&argv).unwrap_err().contains("only valid for `burdin test`"), "{argv:?}");
        }
        assert!(parse(&["build", "app", "--skip", "x"]).unwrap_err().contains("only valid for `burdin test`"));
        assert!(parse(&["build", "app", "--shard", "1/2"]).unwrap_err().contains("only valid for `burdin test`"));
        assert!(parse(&["build", "app", "--format", "json"]).unwrap_err().contains("only valid for `burdin test`"));
        assert!(
            parse(&["build", "app", "--slow-threshold", "5"]).unwrap_err().contains("only valid for `burdin test`")
        );
    }

    #[test]
    fn test_options_are_rejected_for_other_commands() {
        assert!(parse(&["run", "app", "--verbose"]).unwrap_err().contains("only valid for `burdin test`"));
        assert!(parse(&["build", "app", "--workspace"]).unwrap_err().contains("only valid for `burdin test`"));
    }

    #[test]
    fn jobs_apply_to_every_compiling_command() {
        // `-j` limits the compiler's threads for `build` and `run` too, not only the tests `burdin test` runs.
        assert_eq!(parse(&["build", "app", "-j", "2"]).unwrap().jobs, Some(2));
        assert_eq!(parse(&["run", "app", "--jobs=3"]).unwrap().jobs, Some(3));
        assert_eq!(parse(&["build", "app", "-j4"]).unwrap().jobs, Some(4));
        assert!(parse(&["build", "app", "-j", "0"]).unwrap_err().contains("at least 1"));
        assert!(parse(&["build", "app", "-j"]).unwrap_err().contains("needs a number of jobs"));
        assert_eq!(jobs_or_default(Some(5)), 5);
    }

    #[test]
    fn every_codegen_unit_is_linked_in_order() {
        let objects = [PathBuf::from("program.o"), PathBuf::from("program.1.o"), PathBuf::from("program.2.o")];
        let args = as_strings(unix_link_args(&objects, Path::new("out"), &[], &LinkOptions::default()));
        assert_eq!(&args[2..5], ["program.o", "program.1.o", "program.2.o"]);
        let objects = [PathBuf::from("program.obj"), PathBuf::from("program.1.obj")];
        let args = as_strings(msvc_link_args(&objects, Path::new("out.exe"), &[], None));
        assert_eq!(&args[..3], ["/nologo", "program.obj", "program.1.obj"]);
    }

    #[test]
    fn conflicting_test_options_are_rejected() {
        assert!(parse(&["test", "app", "-j", "0"]).unwrap_err().contains("at least 1"));
        assert!(parse(&["test", "app", "-j", "many"]).unwrap_err().contains("needs a number"));
        assert!(
            parse(&["test", "app", "--ignored", "--include-ignored"]).unwrap_err().contains("cannot be used together")
        );
        assert!(parse(&["test", "app", "-p", "lib", "--workspace"]).unwrap_err().contains("cannot be used together"));
        assert!(parse(&["test", "app", "--timeout", "soon"]).unwrap_err().contains("milliseconds"));
    }

    #[test]
    fn color_choice_interacts_with_no_color_and_the_terminal() {
        assert!(color_enabled(ColorChoice::Always, true, false));
        assert!(!color_enabled(ColorChoice::Never, false, true));
        assert!(color_enabled(ColorChoice::Auto, false, true));
        assert!(!color_enabled(ColorChoice::Auto, true, true));
        assert!(!color_enabled(ColorChoice::Auto, false, false));
    }

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string())
    }

    #[test]
    fn hyperlinks_follow_force_hyperlink_then_the_terminal_that_is_running() {
        assert!(!hyperlinks_supported(&env_of(&[])));
        assert!(hyperlinks_supported(&env_of(&[("FORCE_HYPERLINK", "1")])));
        assert!(!hyperlinks_supported(&env_of(&[("FORCE_HYPERLINK", "0"), ("WT_SESSION", "x")])));
        assert!(hyperlinks_supported(&env_of(&[("FORCE_HYPERLINK", "1"), ("TERM", "dumb")])));
        assert!(hyperlinks_supported(&env_of(&[("WT_SESSION", "abc")])));
        assert!(hyperlinks_supported(&env_of(&[("TERM_PROGRAM", "iTerm.app")])));
        assert!(hyperlinks_supported(&env_of(&[("VTE_VERSION", "6800")])));
        assert!(!hyperlinks_supported(&env_of(&[("VTE_VERSION", "4000")])));
        assert!(hyperlinks_supported(&env_of(&[("TERM", "xterm-kitty")])));
        assert!(!hyperlinks_supported(&env_of(&[("TERM", "dumb"), ("WT_SESSION", "abc")])));
        assert!(!hyperlinks_supported(&env_of(&[("TERM", "screen-256color"), ("TERM_PROGRAM", "vscode")])));
        assert!(!hyperlinks_supported(&env_of(&[("TMUX", "/tmp/tmux-0/default,1,0"), ("TERM_PROGRAM", "vscode")])));
    }

    #[test]
    fn the_sarif_tool_lists_every_error_code_and_lint_with_its_link() {
        let tool = sarif_tool();
        let first = &tool.rules[0];
        assert_eq!(first.id, "E0001");
        assert_eq!(first.name, "Syntax error");
        assert_eq!(first.help_uri.as_deref(), Some("https://ricardomilos7.github.io/burdin/errors/E0001"));
        assert!(tool.rules.iter().any(|rule| rule.id == "E0215"));
        let lint = tool.rules.iter().find(|rule| rule.id == "L0007").expect("a rule for the lint");
        assert_eq!(lint.name, "unused_variables");
        assert_eq!(lint.help_uri.as_deref(), Some("https://ricardomilos7.github.io/burdin/lints/L0007"));
        assert!(tool.rules.iter().any(|rule| rule.id == "L0000"));
        assert!(tool.rules.iter().all(|rule| rule.id != "E9000"), "E9000 is not in the reference");
    }

    #[test]
    fn sarif_is_a_diagnostic_format() {
        assert_eq!(parse_format("sarif").unwrap(), DiagnosticFormat::Sarif);
        assert!(parse_format("xml").unwrap_err().contains("human, short, json, or sarif"));
    }

    #[test]
    fn width_comes_from_the_flag_then_columns_then_the_default() {
        assert_eq!(terminal_width(Some(100), Some("40")), 100);
        assert_eq!(terminal_width(None, Some("40")), 40);
        assert_eq!(terminal_width(None, Some(" 100 ")), 100);
        assert_eq!(terminal_width(None, Some("0")), 80);
        assert_eq!(terminal_width(None, Some("wide")), 80);
        assert_eq!(terminal_width(None, None), 80);
        assert_eq!(terminal_width(Some(0), Some("40")), 0);
    }

    #[test]
    fn parse_args_reads_the_diagnostic_options() {
        let argv: Vec<String> = [
            "build",
            "app.bd",
            "--diagnostic-format",
            "json",
            "--color",
            "always",
            "--width",
            "100",
            "--error-limit",
            "3",
        ]
        .map(String::from)
        .to_vec();
        let args = parse_args(&argv).unwrap();
        assert_eq!(args.diagnostics.format, DiagnosticFormat::Json);
        assert_eq!(args.diagnostics.color, ColorChoice::Always);
        assert_eq!(args.diagnostics.width, Some(100));
        assert_eq!(args.diagnostics.error_limit, Some(3));
        let argv: Vec<String> = ["run", "app.bd", "--no-color", "--error-limit", "0"].map(String::from).to_vec();
        let args = parse_args(&argv).unwrap();
        assert_eq!(args.diagnostics.color, ColorChoice::Never);
        assert_eq!(args.diagnostics.error_limit, None);
    }

    #[test]
    fn parse_args_rejects_bad_diagnostic_values() {
        for argv in [
            ["build", "a.bd", "--diagnostic-format", "xml"],
            ["build", "a.bd", "--color", "sparkles"],
            ["build", "a.bd", "--width", "wide"],
            ["build", "a.bd", "--error-limit", "-1"],
        ] {
            let argv: Vec<String> = argv.map(String::from).to_vec();
            assert!(parse_args(&argv).is_err(), "{argv:?} should be rejected");
        }
    }

    #[test]
    fn the_last_color_flag_wins() {
        let argv: Vec<String> = ["build", "a.bd", "--color", "always", "--no-color"].map(String::from).to_vec();
        assert_eq!(parse_args(&argv).unwrap().diagnostics.color, ColorChoice::Never);
        let argv: Vec<String> = ["build", "a.bd", "--no-color", "--color", "always"].map(String::from).to_vec();
        assert_eq!(parse_args(&argv).unwrap().diagnostics.color, ColorChoice::Always);
    }

    fn is_code(text: &str) -> bool {
        let bytes = text.as_bytes();
        bytes.len() == 5 && bytes[0] == b'E' && bytes[1..].iter().all(u8::is_ascii_digit)
    }

    #[test]
    fn every_registry_code_has_a_checked_reference_section() {
        let registry = include_str!("../../../tests/README.md");
        let mut codes: Vec<&str> = registry
            .lines()
            .filter_map(|line| {
                let rest = line.strip_prefix("| ")?;
                let code = &rest[..rest.find(' ')?];
                is_code(code).then_some(code)
            })
            .collect();
        assert!(codes.len() > 20, "the registry has only {} codes", codes.len());
        codes.sort_unstable();
        codes.dedup();
        for code in &codes {
            let section = explain_section(code).unwrap_or_else(|| panic!("no reference section for {code}"));
            assert!(section.contains(&format!("## {code}:")), "{code} has no heading: {section}");
            assert!(section.contains("**How to fix:**"), "{code} has no fix");
            assert!(section.contains(&format!("//~ {code}")), "{code} prints no checked example");
            assert!(
                ERROR_DOCS.contains(&format!("error-example: {code}"))
                    || ERROR_DOCS.contains(&format!("warning-example: {code}")),
                "{code} has no checked example"
            );
        }
        let listing = code_list();
        let listed: Vec<&str> = listing.lines().filter_map(|line| line.split(':').next()).collect();
        assert_eq!(listed, codes, "the reference must list exactly the registry codes");
        assert!(explain_section("E9999").is_none());
        assert!(explain_section("e0003").is_some(), "codes are accepted in lower case");
    }
}
