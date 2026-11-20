//! `burdin test`: compile the selected packages in test mode and run each `test` block in its own process, and
//! build and run the doc tests of their doc comments.
//!
//! The observable contract is `spec/test-runner.md` and the `burdin test` section of the root `README.md`:
//! tests run concurrently (`-j`, default: the number of CPUs), each in a fresh working directory with a clean
//! environment, its output is captured and shown only when it fails, and the report is deterministic: failures and
//! the summary are printed in declaration order after every test has finished. Which tests run is decided in
//! `select`; what is printed or written is in `report`; the doc tests are in `doc_tests`.

mod doc_tests;
mod report;
pub(crate) mod select;
pub(crate) mod snapshots;

use crate::package::{self, Workspace};
use crate::workspace;
use crate::{Args, CompileOptions, EntryMode, Site, TempDir, TestFormat, compile};
use burdin_tir::{TestAttrs, TestEntry, TestOutcome};
use doc_tests::{DocAttrs, DocExample};
use select::FailedRecord;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// The per-test timeout when neither `--timeout` nor `timeout(ms)` says otherwise. A test that hangs would
/// otherwise block the runner forever.
const DEFAULT_TIMEOUT_MS: u64 = 60_000;
/// A test that runs at least this long is listed as slow unless `--slow-threshold` says otherwise.
const DEFAULT_SLOW_MS: u64 = 5_000;
/// How often a running test is checked against its deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// One test of a compiled package: everything needed to select, list, run, and report it.
struct TestCase {
    pub(super) package: String,
    /// The dotted module path inside the package; empty for the package's entry file.
    pub(super) module: String,
    /// The name written in `test "name"`, or `<owner> (line N)` for a doc test.
    pub(super) name: String,
    /// The test's identity inside its package: `<module>.<name>`, or `<name>` for an empty module.
    pub(super) local: String,
    /// The name the report shows: `<name>`, or `<package>::<name>` when more than one package runs.
    pub(super) display: String,
    /// The full path filters match and `--exact` compares: `local`, prefixed with `<package>::` when more than one
    /// package runs.
    pub(super) path: String,
    pub(super) file: String,
    pub(super) line: u32,
    /// How the test runs.
    pub(super) run: Run,
    pub(super) attrs: Attrs,
}

/// How one test runs: a `test` block of a compiled package, or a doc test with its own program. The doc tests of
/// the plan hold a `DocExample` until the build turns them into executables or decided outcomes.
#[derive(Clone, Debug)]
enum Run {
    /// A `test` block: the package's test binary and the dispatcher index that selects it. The compile-time
    /// outcome of a `[differential]` test rides along; `None` for an ordinary test.
    Block { exe: PathBuf, index: usize, comptime: Option<TestOutcome> },
    /// A doc test that still needs its program built (the plan holds these until the build decides them).
    Doc(DocExample),
    /// A doc test whose program is built: ready for its process, which takes no index argument.
    DocExe(PathBuf),
    /// `--fuzz-replay`: a fuzz target run on one saved input. The dispatcher index selects the target and the
    /// input file is its second argument (`spec/fuzz.md`, FZ6).
    Replay { exe: PathBuf, index: usize, input: PathBuf },
}

/// The attributes a test runs by, one shape for `test` blocks and one for doc blocks (`Attrs::doc` selects the
/// doc shape, whose attributes are the doc block's). A `test` block's come from `tir::TestAttrs`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Attrs {
    pub(super) ignore: bool,
    pub(super) timeout_ms: Option<u64>,
    pub(super) should_panic: bool,
    pub(super) should_panic_message: Option<String>,
    pub(super) should_fail: bool,
    pub(super) differential: bool,
    /// A doc test: the attributes below are its, and `should_fail`, `differential`, and `timeout_ms` never apply.
    pub(super) doc: bool,
    pub(super) no_run: bool,
    pub(super) compile_fail: bool,
    pub(super) compile_fail_code: Option<String>,
}

impl Attrs {
    /// The attributes of a `test` block.
    fn of_block(attrs: &TestAttrs) -> Attrs {
        Attrs {
            ignore: attrs.ignore,
            timeout_ms: attrs.timeout_ms,
            should_panic: attrs.should_panic,
            should_panic_message: attrs.should_panic_message.clone(),
            should_fail: attrs.should_fail,
            differential: attrs.differential,
            doc: false,
            no_run: false,
            compile_fail: false,
            compile_fail_code: None,
        }
    }

    /// The attributes of a doc block.
    fn of_doc(attrs: &DocAttrs) -> Attrs {
        Attrs {
            ignore: attrs.ignore,
            should_panic: attrs.should_panic,
            doc: true,
            no_run: attrs.no_run,
            compile_fail: attrs.compile_fail,
            compile_fail_code: attrs.compile_fail_code.clone(),
            ..Attrs::default()
        }
    }
}

/// A selected test. An ignored test is reported but not run; a runnable one has the timeout that applies to it.
struct Planned {
    pub(super) case: TestCase,
    pub(super) ignored: bool,
    pub(super) timeout: Option<Duration>,
}

/// The selected tests in declaration order (packages in build order), with what selection left out.
struct Plan {
    pub(super) tests: Vec<Planned>,
    /// How many selected tests are skipped because of `[ignore]`.
    pub(super) ignored: usize,
    /// How many tests the name filters, `--skip`, `--failed`, or `--shard` left out.
    pub(super) filtered_out: usize,
}

/// What one test process did. `note` explains a failure the output alone does not (a timeout, or an unmet
/// `should_panic`/`should_fail`).
struct Outcome {
    pub(super) passed: bool,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
    pub(super) note: Option<String>,
    pub(super) duration: Duration,
}

/// One compiled package: its test binary and the tests inside it.
struct CompiledPackage {
    name: String,
    exe: PathBuf,
    tests: Vec<TestEntry>,
    sites: Vec<Site>,
    /// With `--fuzz-replay`, each fuzz target, where it is declared, and its saved inputs in name order; empty
    /// otherwise.
    replays: Vec<(burdin_tir::FuzzEntry, Site, Vec<PathBuf>)>,
}

/// How a test process ended. The dispatcher returns 0 for `Ok` and 1 for `Err`; anything else is an abort or a
/// signal. A test that calls `process.exit(n)` with another value is treated as an abort too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    Success,
    ReturnedError,
    Abnormal,
}

fn ending_of(code: Option<i32>) -> Ending {
    match code {
        Some(0) => Ending::Success,
        Some(1) => Ending::ReturnedError,
        _ => Ending::Abnormal,
    }
}

fn ending(status: &ExitStatus) -> Ending {
    ending_of(status.code())
}

pub(crate) fn run(args: &Args) -> ExitCode {
    if args.interpret {
        return run_interpreted(args);
    }
    if let Err(code) = crate::sync_dependencies(&args.target, args.locked, args.offline) {
        return code;
    }
    let workspace = match load_selection(args) {
        Ok(workspace) => workspace,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    };
    let selected = match select_packages(args, &workspace) {
        Ok(selected) => selected,
        Err(message) => {
            return crate::cli::usage_error("test", &message);
        }
    };
    // `--failed` reads what the previous run left under the package's `target` directory, so it needs a package.
    let record_path = failed_record_path(&workspace, args);
    if args.failed && record_path.is_none() {
        return crate::cli::usage_error(
            "test",
            "`--failed` needs a package directory to keep its record in, not a single file",
        );
    }
    let previous = record_path.as_deref().and_then(FailedRecord::load);

    // One temporary directory holds every test binary and every doc test's package. Each package is compiled as
    // its own entry package, so its tests see their own package's private items and are checked with their own
    // `main` rules.
    let binaries = match TempDir::new() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("error: cannot create a temporary directory: {e}");
            return ExitCode::from(2);
        }
    };
    let options = CompileOptions {
        profile: args.profile.clone(),
        debug: args.debug,
        strip: args.strip,
        lto: args.lto,
        pgo_generate: args.pgo_generate,
        pgo_use: args.pgo_use.as_deref(),
        emit_ir: args.emit_ir.as_deref(),
        emit: None,
        link_libs: &args.link_libs,
        mode: EntryMode::Test,
        diagnostics: &args.diagnostics,
        comptime_steps: args.comptime_steps,
        emit_deps: None,
        report_clean: false,
        run_build_scripts: true,
        locked: args.locked,
        offline: args.offline,
        jobs: crate::jobs_or_default(args.jobs),
        sanitize: args.sanitize,
        fuzz_target: None,
    };
    // `--doc` runs only the doc tests, so no `test` block is compiled.
    let mut compiled = Vec::new();
    if !args.doc_only {
        for &index in &selected {
            let package = &workspace.packages[index];
            let target = package_target(&workspace, args, index);
            let exe = binaries.path.join(&package.name).with_extension(std::env::consts::EXE_EXTENSION);
            match compile(&target, &binaries.path, Some(&exe), &options) {
                Ok(built) => {
                    let replays = if args.fuzz_replay {
                        let root = crate::fuzz_runner::fuzz_root(&target, package.dir.as_deref());
                        built
                            .fuzzes
                            .into_iter()
                            .zip(built.fuzz_sites)
                            .map(|(fuzz, site)| {
                                let inputs = saved_inputs(&crate::fuzz_runner::dirs(&root, &fuzz.name).artifacts);
                                (fuzz, site, inputs)
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    compiled.push(CompiledPackage {
                        name: package.name.clone(),
                        exe,
                        tests: built.tests,
                        sites: built.test_sites,
                        replays,
                    })
                }
                Err(code) => return code,
            }
        }
    }

    // The doc tests of the selected packages, read from their doc comments: they run together with the `test`
    // blocks, or alone with `--doc`.
    let examples = doc_tests::collect(&workspace, &selected);
    let package_names: Vec<String> = selected.iter().map(|&index| workspace.packages[index].name.clone()).collect();
    let cases = merge_cases(
        discover(&compiled, selected.len() > 1),
        doc_tests::cases(&examples, selected.len() > 1),
        &package_names,
    );
    let discovered: Vec<(String, String)> =
        cases.iter().map(|case| (case.package.clone(), case.local.clone())).collect();
    // `--failed` with no record at all has nothing to narrow by, so every test is selected.
    let narrowing = if args.failed { previous.as_ref() } else { None };
    if args.failed {
        match &previous {
            None => eprintln!("note: no record of a previous run was found; running every selected test"),
            Some(record) if record.is_empty() => eprintln!("note: the previous run had no failed tests"),
            Some(_) => {}
        }
    }
    let mut plan = select::plan_tests(args, cases, narrowing, default_timeout(args));
    if plan.tests.is_empty() && (!args.filters.is_empty() || !args.skip.is_empty()) {
        eprintln!("note: no test matches the filters");
    }

    if args.list {
        match args.format {
            TestFormat::Human => report::print_list_human(&plan),
            TestFormat::Json => report::print_list_json(&plan),
        }
        return ExitCode::SUCCESS;
    }

    let stream = (args.format == TestFormat::Json).then_some(report::Stream { verbose: args.verbose });
    if let Some(stream) = &stream {
        stream.suite_started(&plan, &package_names);
        for planned in plan.tests.iter().filter(|planned| planned.ignored) {
            stream.test_ignored(planned);
        }
    }
    // The outcomes a test has before any process runs: a doc test that failed to build, or one whose attributes
    // decide it without a run.
    let mut prefilled: Vec<Option<Outcome>> = (0..plan.tests.len()).map(|_| None).collect();
    let doc_indices: Vec<usize> = plan
        .tests
        .iter()
        .enumerate()
        .filter(|(_, planned)| !planned.ignored && matches!(planned.case.run, Run::Doc(_)))
        .map(|(index, _)| index)
        .collect();
    if !doc_indices.is_empty() {
        let built =
            match doc_tests::build(&binaries.path, &doc_indices, &plan.tests, &options, args.jobs, stream.as_ref()) {
                Ok(built) => built,
                Err(code) => return code,
            };
        for (index, built) in built {
            match built {
                doc_tests::Built::Exe(exe) => plan.tests[index].case.run = Run::DocExe(exe),
                doc_tests::Built::Done(outcome) => prefilled[index] = Some(outcome),
            }
        }
    }
    let runnable: Vec<usize> = plan
        .tests
        .iter()
        .enumerate()
        .filter(|(_, planned)| {
            !planned.ignored && matches!(planned.case.run, Run::Block { .. } | Run::DocExe(_) | Run::Replay { .. })
        })
        .map(|(index, _)| index)
        .collect();
    let started = Instant::now();
    let outcomes = match execute(&plan, &runnable, args.jobs, &binaries.path, stream.as_ref(), prefilled) {
        Ok(outcomes) => outcomes,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    // Snapshots are compared after every test has finished, so `--review` asks its questions in declaration order
    // and an accepted snapshot counts before the report is printed.
    let mut outcomes = outcomes;
    let settled = match snapshots::settle(&plan.tests, &mut outcomes, &binaries.path, args.snapshots) {
        Ok(settled) => settled,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    for updated in &settled.updated {
        eprintln!("note: updated {updated}");
    }
    let slow_ms = args.slow_ms.unwrap_or(DEFAULT_SLOW_MS);
    let summary = report::summarize(&plan, &outcomes, slow_ms, started.elapsed());
    // A snapshot file no test checked is stale, which only a run of every test of the selected packages can tell.
    let complete = args.filters.is_empty()
        && args.skip.is_empty()
        && !args.failed
        && args.shard.is_none()
        && !args.ignored_only
        && !args.doc_only
        && summary.ignored == 0
        && summary.failed == 0;
    if complete {
        let sources: Vec<PathBuf> = selected
            .iter()
            .flat_map(|&index| workspace.packages[index].modules.iter())
            .filter(|module| module.text.is_none())
            .map(|module| module.file.clone())
            .collect();
        for path in snapshots::stale(&sources, &settled.touched) {
            eprintln!("warning: stale snapshot {}: no test of this run checked it", path.display());
        }
    }
    match &stream {
        Some(stream) => stream.suite_finished(&plan, &summary),
        None => report::print_human(&plan, &outcomes, &summary, args.verbose),
    }

    let mut exit = if summary.failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) };
    if let Some(path) = &args.junit
        && let Err(error) = report::write_junit(path, &plan, &outcomes, &summary, args.verbose)
    {
        eprintln!("error: {error}");
        exit = ExitCode::from(2);
    }
    if let Some(path) = &record_path {
        let ids = |wanted: bool| -> Vec<(String, String)> {
            plan.tests
                .iter()
                .zip(&outcomes)
                .filter(|(_, outcome)| outcome.as_ref().is_some_and(|outcome| outcome.passed == wanted))
                .map(|(planned, _)| (planned.case.package.clone(), planned.case.local.clone()))
                .collect()
        };
        let record = FailedRecord::updated(previous.as_ref(), &package_names, &discovered, &ids(true), &ids(false));
        if let Err(e) = record.save(path) {
            eprintln!("warning: cannot write {}: {e}", path.display());
        }
    }
    exit
}

/// The per-test timeout that applies when a test has no `timeout(ms)` of its own.
fn default_timeout(args: &Args) -> Option<Duration> {
    match args.timeout_ms {
        Some(0) => None,
        Some(ms) => Some(Duration::from_millis(ms)),
        None => Some(Duration::from_millis(DEFAULT_TIMEOUT_MS)),
    }
}

/// Where `--failed` keeps its record: `<package>/target/burdin/<profile>/test-failed.txt`, next to the build-script
/// output. A single-file target has no package directory, so it keeps none.
fn failed_record_path(workspace: &Workspace, args: &Args) -> Option<PathBuf> {
    let dir = shared_dir(workspace, args)?;
    let profile = &args.profile.dir;
    Some(dir.join("target").join("burdin").join(profile).join("test-failed.txt"))
}

/// The packages a run loads: the build of the target, or, for a selection of workspace members (WS11), the members
/// together as the build's roots, so one report covers them and a package two members reach is loaded once.
pub(crate) fn load_selection(args: &Args) -> Result<Workspace, String> {
    match &args.scope {
        workspace::Scope::Standalone => package::load_for_tests(&args.target),
        workspace::Scope::Members { members, .. } => {
            let dirs: Vec<PathBuf> = members.iter().map(|member| member.dir.clone()).collect();
            package::load_members(&dirs)
        }
    }
}

/// The directory a run keeps its shared outputs in (the failed-test record, the benchmark baselines): the workspace
/// root for a selection of members (WS11), otherwise the entry package's own directory, when it has one.
pub(crate) fn shared_dir(workspace: &Workspace, args: &Args) -> Option<PathBuf> {
    workspace::shared_root(&args.scope).or_else(|| workspace.packages[workspace.entry_package].dir.clone())
}

/// `burdin test --interpret` (`spec/test-runner.md`, IN1-IN6): checks each package and runs its `test` blocks in
/// the interpreter instead of compiling them. A test that needs C or the operating system is skipped, with the
/// reason; a test that trips one of the strict checks fails at the source line that tripped it.
fn run_interpreted(args: &Args) -> ExitCode {
    let workspace = match load_selection(args) {
        Ok(workspace) => workspace,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    };
    let selected = match select_packages(args, &workspace) {
        Ok(selected) => selected,
        Err(message) => return crate::cli::usage_error("test", &message),
    };
    let options = CompileOptions {
        profile: args.profile.clone(),
        debug: false,
        strip: false,
        lto: false,
        pgo_generate: false,
        pgo_use: None,
        emit_ir: None,
        emit: None,
        link_libs: &[],
        mode: EntryMode::Test,
        diagnostics: &args.diagnostics,
        comptime_steps: args.comptime_steps,
        emit_deps: None,
        report_clean: false,
        run_build_scripts: true,
        locked: args.locked,
        offline: args.offline,
        jobs: 1,
        sanitize: Default::default(),
        fuzz_target: None,
    };
    let compile_options = CompileOptions { link_libs: args.link_libs.as_slice(), ..options };
    let interp_options = burdin_comptime::Options {
        overflow_checks: args.profile.overflow_checks,
        debug_assertions: args.profile.debug_assertions,
        steps: args.comptime_steps.unwrap_or(burdin_comptime::DEFAULT_STEPS),
    };
    let qualify = selected.len() > 1;
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    for &index in &selected {
        let package_name = workspace.packages[index].name.clone();
        let target = package_target(&workspace, args, index);
        let front = match crate::front_end(&target, &compile_options, &crate::SourceOverrides::new()) {
            Ok(front) => front,
            Err(code) => return code,
        };
        if front.diags.has_errors() {
            crate::report_errors("test", &target, &front.diags, &front.sources, &args.diagnostics);
            return ExitCode::from(1);
        }
        let Some(program) = front.program else {
            eprintln!("error: {} produced no program", target.display());
            return ExitCode::from(1);
        };
        let site = |span: burdin_diag::Span| {
            let file = front.sources.get(span.file);
            (file.path.clone(), file.line_col(span.lo))
        };
        for test in &program.tests {
            let local =
                if test.module.is_empty() { test.name.clone() } else { format!("{}.{}", test.module, test.name) };
            let path = if qualify { format!("{package_name}::{local}") } else { local };
            let display = if qualify { format!("{package_name}::{}", test.name) } else { test.name.clone() };
            if !select::name_selected(args, &path) {
                continue;
            }
            if args.list {
                let suffix = if test.attrs.ignore { " [ignore]" } else { "" };
                println!("{path}{suffix}");
                continue;
            }
            if test.attrs.ignore && !args.include_ignored {
                skipped += 1;
                println!("SKIP {display} ([ignore])");
                continue;
            }
            match burdin_comptime::interpret_test(&program, test.func, interp_options) {
                burdin_comptime::Interpreted::Passed => {
                    passed += 1;
                    if args.verbose {
                        println!("PASS {display} (interpreted)");
                    }
                }
                burdin_comptime::Interpreted::Skipped(reason) => {
                    skipped += 1;
                    println!("SKIP {display} ({reason})");
                }
                burdin_comptime::Interpreted::Failed(message, span) => {
                    failed += 1;
                    let (file, (line, column)) = site(span);
                    println!("---- test \"{display}\" failed ----");
                    println!("{file}:{line}:{column}: {message}");
                    println!("FAIL {display}");
                }
            }
        }
    }
    if args.list {
        return ExitCode::SUCCESS;
    }
    let mut line = format!("test result: {passed} passed, {failed} failed");
    if skipped > 0 {
        line.push_str(&format!(", {skipped} skipped"));
    }
    println!("{line}");
    if failed > 0 { ExitCode::from(1) } else { ExitCode::SUCCESS }
}

/// Every test of the compiled packages, in build order and then declaration order. When more than one package runs,
/// names are qualified with the package.
fn discover(compiled: &[CompiledPackage], qualify: bool) -> Vec<TestCase> {
    let mut cases = Vec::new();
    for package in compiled {
        for (index, test) in package.tests.iter().enumerate() {
            let local =
                if test.module.is_empty() { test.name.clone() } else { format!("{}.{}", test.module, test.name) };
            let (display, path) = if qualify {
                (format!("{}::{}", package.name, test.name), format!("{}::{local}", package.name))
            } else {
                (test.name.clone(), local.clone())
            };
            let (file, line) = package.sites.get(index).map(|site| (site.file.clone(), site.line)).unwrap_or_default();
            cases.push(TestCase {
                package: package.name.clone(),
                module: test.module.clone(),
                name: test.name.clone(),
                local,
                display,
                path,
                file,
                line,
                run: Run::Block { exe: package.exe.clone(), index, comptime: test.comptime },
                attrs: Attrs::of_block(&test.attrs),
            });
        }
        // `--fuzz-replay`: one case per saved input of each fuzz target, named `fuzz <target>: <file>`. The
        // dispatcher numbers the fuzz targets after the tests.
        for (j, (fuzz, site, inputs)) in package.replays.iter().enumerate() {
            for input in inputs {
                let file_name = input.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                let name = format!("fuzz {}: {file_name}", fuzz.name);
                let local = if fuzz.module.is_empty() { name.clone() } else { format!("{}.{name}", fuzz.module) };
                let (display, path) = if qualify {
                    (format!("{}::{name}", package.name), format!("{}::{local}", package.name))
                } else {
                    (name.clone(), local.clone())
                };
                cases.push(TestCase {
                    package: package.name.clone(),
                    module: fuzz.module.clone(),
                    name,
                    local,
                    display,
                    path,
                    file: site.file.clone(),
                    line: site.line,
                    run: Run::Replay { exe: package.exe.clone(), index: package.tests.len() + j, input: input.clone() },
                    attrs: Attrs::default(),
                });
            }
        }
    }
    cases
}

/// The saved inputs of one fuzz target, `fuzz/artifacts/<name>/`, in name order. Hidden files (an editor's or a
/// file manager's) are not inputs; a missing directory has none.
fn saved_inputs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut inputs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| !path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.')))
        .collect();
    inputs.sort();
    // The test process runs in its own directory, so the input is named by an absolute path.
    inputs.into_iter().map(|path| std::path::absolute(&path).unwrap_or(path)).collect()
}

/// The tests of a run in report order: each selected package's `test` blocks and then its doc tests, package by
/// package. `blocks` and `docs` are each in package order already; this only interleaves the two lists.
fn merge_cases(blocks: Vec<TestCase>, docs: Vec<TestCase>, packages: &[String]) -> Vec<TestCase> {
    let mut blocks = blocks.into_iter().peekable();
    let mut docs = docs.into_iter().peekable();
    let mut cases = Vec::new();
    for package in packages {
        while blocks.peek().is_some_and(|case| &case.package == package) {
            cases.push(blocks.next().expect("peeked"));
        }
        while docs.peek().is_some_and(|case| &case.package == package) {
            cases.push(docs.next().expect("peeked"));
        }
    }
    cases.extend(blocks);
    cases.extend(docs);
    cases
}

/// The packages `-p`/`--workspace` select, in build order (the entry package first, then its dependencies). The
/// embedded standard library has no directory and is never selected.
pub(crate) fn select_packages(args: &Args, workspace: &Workspace) -> Result<Vec<usize>, String> {
    if let workspace::Scope::Members { members, .. } = &args.scope {
        // The members are the build's roots, loaded from their directories, in workspace order (WS6).
        return members
            .iter()
            .map(|member| {
                workspace
                    .packages
                    .iter()
                    .position(|package| package.dir.as_deref() == Some(member.dir.as_path()))
                    .ok_or_else(|| format!("member `{}` is not in this build", member.name))
            })
            .collect();
    }
    if !args.packages.is_empty() {
        let mut selected = Vec::new();
        for name in &args.packages {
            let index = workspace
                .packages
                .iter()
                .position(|package| &package.name == name)
                .ok_or_else(|| format!("no package named `{name}` in this build"))?;
            if index != workspace.entry_package && workspace.packages[index].dir.is_none() {
                return Err(format!("package `{name}` is embedded and cannot be tested"));
            }
            if !selected.contains(&index) {
                selected.push(index);
            }
        }
        selected.sort_unstable();
        Ok(selected)
    } else if args.workspace {
        Ok((0..workspace.packages.len())
            .filter(|&index| index == workspace.entry_package || workspace.packages[index].dir.is_some())
            .collect())
    } else {
        Ok(vec![workspace.entry_package])
    }
}

pub(crate) fn package_target(workspace: &Workspace, args: &Args, index: usize) -> PathBuf {
    // A member is built from its own directory: the target names the workspace, which may be a virtual root.
    if index == workspace.entry_package && !matches!(args.scope, workspace::Scope::Members { .. }) {
        args.target.clone()
    } else {
        workspace.packages[index].dir.clone().expect("a selected dependency package has a directory")
    }
}

/// Runs the runnable tests with `jobs` workers. The returned vector is indexed like `plan.tests`; an entry that
/// already has an outcome (a doc test decided without a process) keeps it, and ignored entries stay `None`. With a
/// JSON `stream`, each test's start and end are reported as it happens. An error means the runner itself failed (a
/// test binary could not be started).
fn execute(
    plan: &Plan,
    runnable: &[usize],
    jobs: Option<usize>,
    capture_dir: &Path,
    stream: Option<&report::Stream>,
    prefilled: Vec<Option<Outcome>>,
) -> Result<Vec<Option<Outcome>>, String> {
    let outcomes: Vec<Option<Outcome>> = prefilled;
    if runnable.is_empty() {
        return Ok(outcomes);
    }
    let jobs = jobs.unwrap_or_else(default_jobs).min(runnable.len()).max(1);
    let next = AtomicUsize::new(0);
    let outcomes = Mutex::new(outcomes);
    let failure = Mutex::new(None::<String>);
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                loop {
                    let slot = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&plan_index) = runnable.get(slot) else { break };
                    let planned = &plan.tests[plan_index];
                    if let Some(stream) = stream {
                        stream.test_started(planned);
                    }
                    match run_one(planned, capture_dir, plan_index) {
                        Ok(outcome) => {
                            if let Some(stream) = stream {
                                stream.test_finished(planned, &outcome);
                            }
                            outcomes.lock().unwrap()[plan_index] = Some(outcome);
                        }
                        Err(error) => {
                            *failure.lock().unwrap() = Some(error);
                            break;
                        }
                    }
                }
            });
        }
    });
    if let Some(error) = failure.into_inner().unwrap() {
        return Err(error);
    }
    Ok(outcomes.into_inner().unwrap())
}

fn default_jobs() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

/// Runs one test process in a fresh directory with a clean environment and captures its output. The output goes to
/// files outside the test's working directory: a pipe would deadlock when a test writes more than the pipe buffer
/// holds (unless a reader thread drains it), and a grandchild that inherits a pipe can keep the reader blocked after
/// a timeout kills the test.
fn run_one(planned: &Planned, capture_dir: &Path, slot: usize) -> Result<Outcome, String> {
    let Planned { case, timeout, .. } = planned;
    let TestCase { display, attrs, run, .. } = case;
    // A doc test's process takes no dispatcher index: the program is the test.
    let (exe, argument, comptime) = match run {
        Run::Block { exe, index, comptime } => (exe, Some(index.to_string()), *comptime),
        Run::Replay { exe, index, .. } => (exe, Some(index.to_string()), None),
        Run::DocExe(exe) => (exe, None, None),
        Run::Doc(_) => unreachable!("the doc build decided or compiled every planned doc test"),
    };
    let work = TempDir::new().map_err(|e| format!("cannot create a temporary directory: {e}"))?;
    let stdout_path = capture_dir.join(format!("capture-{slot}.out"));
    let stderr_path = capture_dir.join(format!("capture-{slot}.err"));
    let stdout_file =
        std::fs::File::create(&stdout_path).map_err(|e| format!("cannot create {}: {e}", stdout_path.display()))?;
    let stderr_file =
        std::fs::File::create(&stderr_path).map_err(|e| format!("cannot create {}: {e}", stderr_path.display()))?;
    let mut command = Command::new(exe);
    command
        .current_dir(&work.path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    if let Some(argument) = argument {
        command.arg(argument);
    }
    if let Run::Replay { input, .. } = run {
        command.arg(input);
    }
    isolate(&mut command, &work.path, display);
    if matches!(run, Run::Block { .. }) {
        command.env("BURDIN_SNAPSHOTS", snapshots::record_path(capture_dir, slot));
    }
    let start = Instant::now();
    let mut child = command.spawn().map_err(|e| format!("cannot run {}: {e}", exe.display()))?;
    let mut timed_out_after = None;
    let status = match *timeout {
        None => child.wait().map_err(|e| format!("cannot wait for {}: {e}", exe.display()))?,
        Some(limit) => loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => return Err(format!("cannot wait for {}: {e}", exe.display())),
            }
            if start.elapsed() >= limit {
                timed_out_after = Some(limit.as_millis());
                let _ = child.kill();
                break child.wait().map_err(|e| format!("cannot wait for {}: {e}", exe.display()))?;
            }
            std::thread::sleep(POLL_INTERVAL);
        },
    };
    let duration = start.elapsed();
    let stdout = std::fs::read(&stdout_path).unwrap_or_default();
    let stderr = std::fs::read(&stderr_path).unwrap_or_default();
    let note = match timed_out_after {
        Some(ms) => Some(format!("timed out after {ms} ms")),
        None => match run {
            Run::Block { .. } => verdict(ending(&status), attrs, comptime, &String::from_utf8_lossy(&stderr)).err(),
            Run::DocExe(_) => doc_verdict(ending(&status), attrs).err(),
            Run::Replay { input, .. } => replay_verdict(&status, input).err(),
            Run::Doc(_) => unreachable!("the doc build decided every planned doc test"),
        },
    };
    Ok(Outcome { passed: note.is_none(), stdout, stderr, note, duration })
}

/// Whether a fuzz replay passed: the target must finish on the input (FZ6). A panic or a sanitizer report is the
/// failure the input was saved for; status 4 is the dispatcher's "cannot read the input".
fn replay_verdict(status: &std::process::ExitStatus, input: &Path) -> Result<(), String> {
    match status.code() {
        Some(0) => Ok(()),
        Some(4) => Err(format!("cannot read the saved input {}", input.display())),
        _ => Err(format!("the fuzz target still fails on {}", input.display())),
    }
}

/// Whether a doc test's ending satisfies its attributes: the example must exit 0, or abort when it is
/// `should_panic`. A doc test is an ordinary program, so there is no returned error to expect.
fn doc_verdict(ending: Ending, attrs: &Attrs) -> Result<(), String> {
    if attrs.should_panic {
        return match ending {
            Ending::Abnormal => Ok(()),
            Ending::Success => Err("expected the example to panic, but it passed".to_string()),
            Ending::ReturnedError => Err("expected a panic abort, but the example exited with status 1".to_string()),
        };
    }
    match ending {
        Ending::Success => Ok(()),
        // The output (a panic message) is the explanation.
        Ending::ReturnedError | Ending::Abnormal => Err(String::new()),
    }
}

/// A test process starts with an empty environment. `PATH` stays so `process.run` can find commands; the temporary
/// directory variables point at the test's own directory, so a program that creates temporary files cleans them up
/// with it. On Windows the variables the C runtime needs to start are kept. `BURDIN_TEST` names the running test.
fn isolate(command: &mut Command, workdir: &Path, display: &str) {
    command.env_clear();
    for (name, value) in isolation_env(workdir, "BURDIN_TEST", display) {
        command.env(name, value);
    }
}

/// A bench or test process starts with an empty environment plus these variables (`name` is `BURDIN_TEST` for a
/// test and `BURDIN_BENCH` for a bench).
pub(crate) fn isolation_env(
    workdir: &Path,
    name: &'static str,
    display: &str,
) -> Vec<(&'static str, std::ffi::OsString)> {
    let mut vars = vec![
        ("TMPDIR", workdir.as_os_str().to_owned()),
        ("TMP", workdir.as_os_str().to_owned()),
        ("TEMP", workdir.as_os_str().to_owned()),
        (name, std::ffi::OsString::from(display)),
    ];
    if let Some(path) = std::env::var_os("PATH") {
        vars.push(("PATH", path));
    }
    #[cfg(windows)]
    for name in [
        "SystemRoot",
        "SystemDrive",
        "windir",
        "ComSpec",
        "PATHEXT",
        "NUMBER_OF_PROCESSORS",
        "PROCESSOR_ARCHITECTURE",
        "OS",
    ] {
        if let Some(value) = std::env::var_os(name) {
            vars.push((name, value));
        }
    }
    vars
}

/// Whether the test's ending satisfies its attributes. For a `[differential]` test, `comptime` is the outcome of
/// the same body run by the interpreter while the package was compiled; the compiled test must produce it too
/// (`spec/test-runner.md`). A doc test has its own verdict (`doc_verdict`): it is an ordinary program, so no
/// dispatcher exit status and no differential comparison apply.
fn verdict(ending: Ending, attrs: &Attrs, comptime: Option<TestOutcome>, stderr: &str) -> Result<(), String> {
    if let Some(expected) = comptime {
        return match (ending, expected) {
            (Ending::Success, TestOutcome::Ok) | (Ending::ReturnedError, TestOutcome::Err) => Ok(()),
            (Ending::Success, TestOutcome::Err) => {
                Err("the compiled test returned `Ok`, but the comptime interpreter returned `Err`".to_string())
            }
            (Ending::ReturnedError, TestOutcome::Ok) => {
                Err("the compiled test returned `Err`, but the comptime interpreter returned `Ok`".to_string())
            }
            (Ending::Abnormal, TestOutcome::Ok) => {
                Err("the compiled test aborted, but the comptime interpreter returned `Ok`".to_string())
            }
            (Ending::Abnormal, TestOutcome::Err) => {
                Err("the compiled test aborted, but the comptime interpreter returned `Err`".to_string())
            }
        };
    }
    if attrs.should_panic {
        return match ending {
            Ending::Abnormal => match &attrs.should_panic_message {
                Some(message) if !stderr.contains(message) => {
                    Err(format!("the expected panic message `{message}` was not printed to stderr"))
                }
                _ => Ok(()),
            },
            Ending::Success => Err("expected the test to panic, but it passed".to_string()),
            Ending::ReturnedError => Err("expected a panic abort, but the test returned an error".to_string()),
        };
    }
    if attrs.should_fail {
        return match ending {
            Ending::ReturnedError => Ok(()),
            Ending::Success => Err("expected the test to fail, but it passed".to_string()),
            Ending::Abnormal => Err("expected a returned error, but the test aborted".to_string()),
        };
    }
    match ending {
        Ending::Success => Ok(()),
        // The output (a panic message, or nothing for a returned `Err`) is the explanation.
        Ending::ReturnedError | Ending::Abnormal => Err(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::PackageFiles;

    fn args(target: &str) -> Args {
        Args { command: "test".to_string(), target: PathBuf::from(target), ..Args::default() }
    }

    fn package(name: &str, dir: Option<&str>) -> PackageFiles {
        PackageFiles {
            name: name.to_string(),
            dir: dir.map(PathBuf::from),
            modules: Vec::new(),
            deps: Vec::new(),
            build_allow: Vec::new(),
            lint: Vec::new(),
            rust: Default::default(),
            edition: Default::default(),
            root_module: None,
        }
    }

    fn workspace(entry: usize, packages: Vec<PackageFiles>) -> Workspace {
        Workspace { packages, entry_package: entry, entry_module: 0, stem: "entry".to_string() }
    }

    fn attrs() -> Attrs {
        Attrs::default()
    }

    #[test]
    fn default_selection_is_the_entry_package() {
        let ws = workspace(0, vec![package("app", Some("app")), package("lib", Some("lib")), package("std", None)]);
        assert_eq!(select_packages(&args("app"), &ws).unwrap(), vec![0]);
    }

    #[test]
    fn workspace_selection_keeps_packages_on_disk() {
        let ws = workspace(0, vec![package("app", Some("app")), package("lib", Some("lib")), package("std", None)]);
        let mut args = args("app");
        args.workspace = true;
        assert_eq!(select_packages(&args, &ws).unwrap(), vec![0, 1]);
    }

    #[test]
    fn named_packages_are_checked() {
        let ws = workspace(0, vec![package("app", Some("app")), package("lib", Some("lib")), package("std", None)]);
        let mut args = args("app");
        args.packages = vec!["lib".to_string(), "lib".to_string()];
        assert_eq!(select_packages(&args, &ws).unwrap(), vec![1]);

        let mut missing = args.clone();
        missing.packages = vec!["nope".to_string()];
        assert!(select_packages(&missing, &ws).unwrap_err().contains("no package named `nope`"));

        let mut embedded = args.clone();
        embedded.packages = vec!["std".to_string()];
        assert!(select_packages(&embedded, &ws).unwrap_err().contains("embedded"));
    }

    #[test]
    fn endings_follow_the_dispatcher_exit_codes() {
        assert_eq!(ending_of(Some(0)), Ending::Success);
        assert_eq!(ending_of(Some(1)), Ending::ReturnedError);
        assert_eq!(ending_of(Some(2)), Ending::Abnormal);
        assert_eq!(ending_of(None), Ending::Abnormal);
    }

    #[test]
    fn should_panic_requires_an_abort_and_the_message() {
        let mut with_message = attrs();
        with_message.should_panic = true;
        with_message.should_panic_message = Some("boom".to_string());
        assert!(verdict(Ending::Abnormal, &with_message, None, "panic: boom\n").is_ok());
        assert!(verdict(Ending::Abnormal, &with_message, None, "panic: other\n").is_err());
        assert!(verdict(Ending::Success, &with_message, None, "").is_err());
        assert!(verdict(Ending::ReturnedError, &with_message, None, "").is_err());

        let mut plain = attrs();
        plain.should_panic = true;
        assert!(verdict(Ending::Abnormal, &plain, None, "").is_ok());
    }

    #[test]
    fn should_fail_requires_a_returned_error() {
        let mut fail = attrs();
        fail.should_fail = true;
        assert!(verdict(Ending::ReturnedError, &fail, None, "").is_ok());
        assert!(verdict(Ending::Success, &fail, None, "").is_err());
        assert!(verdict(Ending::Abnormal, &fail, None, "").is_err());
    }

    #[test]
    fn a_normal_test_passes_only_on_success() {
        assert!(verdict(Ending::Success, &attrs(), None, "").is_ok());
        assert_eq!(verdict(Ending::ReturnedError, &attrs(), None, ""), Err(String::new()));
        assert_eq!(verdict(Ending::Abnormal, &attrs(), None, ""), Err(String::new()));
    }

    #[test]
    fn a_doc_test_passes_on_a_clean_exit_and_panics_only_when_asked() {
        // A doc test is an ordinary program: exit 0 is the only success, and a returned error is a failure.
        assert!(doc_verdict(Ending::Success, &attrs()).is_ok());
        assert_eq!(doc_verdict(Ending::ReturnedError, &attrs()), Err(String::new()));
        assert_eq!(doc_verdict(Ending::Abnormal, &attrs()), Err(String::new()));
        // `should_panic` needs the abort; the exit status a dispatcher would give means nothing here.
        let mut panics = attrs();
        panics.should_panic = true;
        assert!(doc_verdict(Ending::Abnormal, &panics).is_ok());
        assert_eq!(
            doc_verdict(Ending::Success, &panics),
            Err("expected the example to panic, but it passed".to_string())
        );
        assert_eq!(
            doc_verdict(Ending::ReturnedError, &panics),
            Err("expected a panic abort, but the example exited with status 1".to_string())
        );
    }

    #[test]
    fn a_differential_test_must_match_the_compile_time_outcome() {
        // Agreement passes whether the body returns `Ok` or `Err`; agreement is the expectation, not success.
        assert!(verdict(Ending::Success, &attrs(), Some(TestOutcome::Ok), "").is_ok());
        assert!(verdict(Ending::ReturnedError, &attrs(), Some(TestOutcome::Err), "").is_ok());
        // A different outcome, or a compiled run that aborts, is a divergence and fails with an explanation.
        assert_eq!(
            verdict(Ending::Success, &attrs(), Some(TestOutcome::Err), ""),
            Err("the compiled test returned `Ok`, but the comptime interpreter returned `Err`".to_string())
        );
        assert_eq!(
            verdict(Ending::ReturnedError, &attrs(), Some(TestOutcome::Ok), ""),
            Err("the compiled test returned `Err`, but the comptime interpreter returned `Ok`".to_string())
        );
        assert_eq!(
            verdict(Ending::Abnormal, &attrs(), Some(TestOutcome::Ok), ""),
            Err("the compiled test aborted, but the comptime interpreter returned `Ok`".to_string())
        );
        assert_eq!(
            verdict(Ending::Abnormal, &attrs(), Some(TestOutcome::Err), ""),
            Err("the compiled test aborted, but the comptime interpreter returned `Err`".to_string())
        );
    }

    #[test]
    fn isolation_keeps_only_the_defined_variables() {
        let workdir = Path::new("/tmp/burdin-test-dir");
        let vars = isolation_env(workdir, "BURDIN_TEST", "one");
        let find = |name: &str| vars.iter().find(|(key, _)| *key == name).map(|(_, value)| value.clone());
        assert_eq!(find("TMPDIR"), Some(workdir.as_os_str().to_owned()));
        assert_eq!(find("TMP"), Some(workdir.as_os_str().to_owned()));
        assert_eq!(find("TEMP"), Some(workdir.as_os_str().to_owned()));
        assert_eq!(find("BURDIN_TEST"), Some(std::ffi::OsString::from("one")));
        assert_eq!(find("PATH"), std::env::var_os("PATH"));
        // A variable the parent has is not passed through: the command clears the environment first.
        let parent = std::env::vars_os().find(|(key, _)| {
            key.to_str().is_some_and(|key| {
                !matches!(
                    key,
                    "PATH"
                        | "TMPDIR"
                        | "TMP"
                        | "TEMP"
                        | "BURDIN_TEST"
                        | "SystemRoot"
                        | "SystemDrive"
                        | "windir"
                        | "ComSpec"
                        | "PATHEXT"
                        | "NUMBER_OF_PROCESSORS"
                        | "PROCESSOR_ARCHITECTURE"
                        | "OS"
                )
            })
        });
        if let Some((key, _)) = parent {
            assert!(
                find(&key.to_string_lossy()).is_none(),
                "{} leaked into the test environment",
                key.to_string_lossy()
            );
        }
    }

    fn entry(name: &str, module: &str) -> TestEntry {
        TestEntry {
            name: name.to_string(),
            module: module.to_string(),
            span: burdin_diag::Span::new(burdin_diag::FileId(0), 0, 0),
            func: burdin_tir::FuncId(0),
            body: burdin_tir::FuncId(0),
            attrs: TestAttrs::default(),
            comptime: None,
        }
    }

    fn compiled(name: &str, tests: Vec<TestEntry>) -> CompiledPackage {
        let sites = tests.iter().map(|_| Site { file: format!("{name}/src/main.bd"), line: 3 }).collect();
        CompiledPackage {
            name: name.to_string(),
            exe: PathBuf::from(format!("{name}-bin")),
            tests,
            sites,
            replays: Vec::new(),
        }
    }

    #[test]
    fn discovery_builds_the_full_path_of_each_test() {
        let one = discover(&[compiled("app", vec![entry("plain", ""), entry("deep", "geometry.shapes")])], false);
        let paths: Vec<_> = one.iter().map(|case| (case.display.as_str(), case.path.as_str())).collect();
        assert_eq!(paths, [("plain", "plain"), ("deep", "geometry.shapes.deep")]);
        let index = match &one[1].run {
            Run::Block { index, .. } => *index,
            _ => panic!("a test block runs from the package's binary"),
        };
        assert_eq!((one[1].file.as_str(), one[1].line, index), ("app/src/main.bd", 3, 1));

        // With more than one package both names carry the package, and the module stays in the path.
        let two = discover(
            &[compiled("app", vec![entry("plain", "")]), compiled("lib", vec![entry("deep", "geometry")])],
            true,
        );
        let paths: Vec<_> = two.iter().map(|case| (case.display.as_str(), case.path.as_str())).collect();
        assert_eq!(paths, [("app::plain", "app::plain"), ("lib::deep", "lib::geometry.deep")]);
        assert_eq!(two[1].local, "geometry.deep");
    }

    /// A doc test case built by hand, standing in for what `doc_tests::cases` builds from a doc comment.
    fn doc_case(package: &str, module: &str, name: &str) -> TestCase {
        let local = if module.is_empty() { name.to_string() } else { format!("{module}.{name}") };
        TestCase {
            package: package.to_string(),
            module: module.to_string(),
            name: name.to_string(),
            local: local.clone(),
            display: name.to_string(),
            path: local,
            file: "src/lib.bd".to_string(),
            line: 12,
            run: Run::Doc(doc_tests::DocExample {
                package: package.to_string(),
                module: module.to_string(),
                owner: name.to_string(),
                file: "src/lib.bd".to_string(),
                line: 12,
                attrs: doc_tests::DocAttrs::default(),
                dep: doc_tests::Dep::None,
                lines: Vec::new(),
                fence: (0, 0),
                text: std::sync::Arc::new(String::new()),
            }),
            attrs: Attrs { doc: true, ..Attrs::default() },
        }
    }

    #[test]
    fn blocks_and_doc_tests_interleave_by_package() {
        let blocks = discover(
            &[compiled("app", vec![entry("plain", "")]), compiled("lib", vec![entry("deep", "geometry")])],
            false,
        );
        let docs = vec![
            doc_case("app", "", "Point (line 3)"),
            doc_case("lib", "geometry", "Shape (line 9)"),
            doc_case("lib", "", "later (line 20)"),
        ];
        let cases = merge_cases(blocks, docs, &["app".to_string(), "lib".to_string()]);
        let names: Vec<&str> = cases.iter().map(|case| case.display.as_str()).collect();
        assert_eq!(names, ["plain", "Point (line 3)", "deep", "Shape (line 9)", "later (line 20)"]);
        // A package with no doc tests, and doc tests of a package with no test blocks, keep their order.
        let docs2 = vec![doc_case("app", "", "Point (line 3)")];
        let blocks2 = discover(&[compiled("lib", vec![entry("deep", "geometry")])], false);
        let cases2 = merge_cases(blocks2, docs2, &["app".to_string(), "lib".to_string()]);
        let names2: Vec<&str> = cases2.iter().map(|case| case.display.as_str()).collect();
        assert_eq!(names2, ["Point (line 3)", "deep"]);
    }

    #[test]
    fn the_failed_record_lives_under_the_package_target_directory() {
        let ws = workspace(0, vec![package("app", Some("app")), package("std", None)]);
        let path = failed_record_path(&ws, &args("app")).unwrap();
        assert_eq!(path, Path::new("app").join("target").join("burdin").join("debug").join("test-failed.txt"));
        let release = Args { release: true, profile: crate::profile::Profile::release(), ..args("app") };
        assert!(failed_record_path(&ws, &release).unwrap().ends_with("release/test-failed.txt"));
        // A single-file target has no directory to keep a record in.
        let single = workspace(0, vec![package("one", None)]);
        assert_eq!(failed_record_path(&single, &args("one.bd")), None);
    }
}
