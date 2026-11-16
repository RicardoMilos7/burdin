//! `burdin test`: compile the selected packages in test mode and run each `test` block in its own process.
//!
//! The observable contract is `spec/test-runner.md` and the `burdin test` section of the root `README.md`:
//! tests run concurrently (`-j`, default: the number of CPUs), each in a fresh working directory with a clean
//! environment, its output is captured and shown only when it fails, and the report is deterministic: failures and
//! the summary are printed in declaration order after every test has finished.

use crate::package::{self, Workspace};
use crate::{Args, CompileOptions, TempDir, USAGE, compile};
use burdin_tir::{TestAttrs, TestOutcome};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// The per-test timeout when neither `--timeout` nor `timeout(ms)` says otherwise. A test that hangs would
/// otherwise block the runner forever.
const DEFAULT_TIMEOUT_MS: u64 = 60_000;
/// How often a running test is checked against its deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// One planned test: an ignored test is reported but not run; a runnable test names the binary, the dispatcher
/// index, and the attributes that decide what counts as a pass.
enum PlannedTest {
    Ignored {
        display: String,
    },
    Run {
        display: String,
        exe: PathBuf,
        index: usize,
        timeout: Option<Duration>,
        attrs: TestAttrs,
        /// The compile-time outcome of a `[differential]` test; `None` for an ordinary test.
        comptime: Option<TestOutcome>,
    },
}

/// What one test process did. `note` explains a failure the output alone does not (a timeout, or an unmet
/// `should_panic`/`should_fail`).
struct Outcome {
    passed: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    note: Option<String>,
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
    let workspace = match package::load_for_tests(&args.target) {
        Ok(workspace) => workspace,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    };
    let selected = match select_packages(args, &workspace) {
        Ok(selected) => selected,
        Err(message) => {
            eprintln!("error: {message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    // One temporary directory holds every test binary. Each package is compiled as its own entry package, so its
    // tests see their own package's private items and are checked with their own `main` rules.
    let binaries = match TempDir::new() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("error: cannot create a temporary directory: {e}");
            return ExitCode::from(2);
        }
    };
    let options = CompileOptions {
        release: args.release,
        debug: args.debug,
        strip: args.strip,
        lto: args.lto,
        pgo_generate: args.pgo_generate,
        pgo_use: args.pgo_use.as_deref(),
        emit_ir: args.emit_ir.as_deref(),
        link_libs: &args.link_libs,
        test_mode: true,
        diagnostics: &args.diagnostics,
        comptime_steps: args.comptime_steps,
        emit_deps: None,
        run_build_scripts: true,
    };
    let mut compiled = Vec::new();
    for &index in &selected {
        let package = &workspace.packages[index];
        let target = package_target(&workspace, args, index);
        let exe = binaries.path.join(&package.name).with_extension(std::env::consts::EXE_EXTENSION);
        match compile(&target, &binaries.path, Some(&exe), &options) {
            Ok(built) => compiled.push((package.name.clone(), exe, built.tests)),
            Err(code) => return code,
        }
    }

    let (plan, ignored) = plan_tests(args, &compiled);
    let runnable: Vec<usize> =
        plan.iter().enumerate().filter(|(_, test)| matches!(test, PlannedTest::Run { .. })).map(|(i, _)| i).collect();
    let outcomes = match execute(&plan, &runnable, args.jobs, &binaries.path) {
        Ok(outcomes) => outcomes,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };

    let mut passed = 0usize;
    let mut failed = 0usize;
    for (index, test) in plan.iter().enumerate() {
        match test {
            PlannedTest::Ignored { display } => {
                if args.verbose {
                    println!("IGNORED {display}");
                }
            }
            PlannedTest::Run { display, .. } => {
                let outcome = outcomes[index].as_ref().expect("every runnable test has an outcome");
                if outcome.passed {
                    passed += 1;
                    if args.verbose {
                        print_output(display, outcome);
                        println!("PASS {display}");
                    }
                } else {
                    failed += 1;
                    print_failure(display, outcome);
                    println!("FAIL {display}");
                }
            }
        }
    }

    println!("test result: {passed} passed, {failed} failed{}", ignored_suffix(ignored));
    if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) }
}

fn ignored_suffix(ignored: usize) -> String {
    if ignored == 0 { String::new() } else { format!(", {ignored} ignored") }
}

/// The packages `-p`/`--workspace` select, in build order (the entry package first, then its dependencies). The
/// embedded standard library has no directory and is never selected.
fn select_packages(args: &Args, workspace: &Workspace) -> Result<Vec<usize>, String> {
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

fn package_target(workspace: &Workspace, args: &Args, index: usize) -> PathBuf {
    if index == workspace.entry_package {
        args.target.clone()
    } else {
        workspace.packages[index].dir.clone().expect("a selected dependency package has a directory")
    }
}

/// Turns the compiled tests into the run plan and counts the ignored tests. The filter applies to the displayed
/// name, which includes the package name when more than one package runs.
fn plan_tests(args: &Args, compiled: &[(String, PathBuf, Vec<burdin_tir::TestEntry>)]) -> (Vec<PlannedTest>, usize) {
    let qualify = compiled.len() > 1;
    let global_timeout = match args.timeout_ms {
        Some(0) => None,
        Some(ms) => Some(Duration::from_millis(ms)),
        None => Some(Duration::from_millis(DEFAULT_TIMEOUT_MS)),
    };
    let mut plan = Vec::new();
    let mut ignored = 0usize;
    for (package, exe, tests) in compiled {
        for (index, test) in tests.iter().enumerate() {
            let display = if qualify { format!("{}::{}", package, test.name) } else { test.name.clone() };
            if args.filter.as_deref().is_some_and(|filter| !display.contains(filter)) {
                continue;
            }
            if test.attrs.ignore && !args.ignored_only && !args.include_ignored {
                ignored += 1;
                plan.push(PlannedTest::Ignored { display });
                continue;
            }
            if args.ignored_only && !test.attrs.ignore {
                continue;
            }
            let timeout = match test.attrs.timeout_ms {
                Some(0) => None,
                Some(ms) => Some(Duration::from_millis(ms)),
                None => global_timeout,
            };
            plan.push(PlannedTest::Run {
                display,
                exe: exe.clone(),
                index,
                timeout,
                attrs: test.attrs.clone(),
                comptime: test.comptime,
            });
        }
    }
    (plan, ignored)
}

/// Runs the runnable tests with `jobs` workers. The returned vector is indexed like `plan`; ignored and filtered
/// entries stay `None`. An error means the runner itself failed (a test binary could not be started).
fn execute(
    plan: &[PlannedTest],
    runnable: &[usize],
    jobs: Option<usize>,
    capture_dir: &Path,
) -> Result<Vec<Option<Outcome>>, String> {
    let outcomes: Vec<Option<Outcome>> = (0..plan.len()).map(|_| None).collect();
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
                    if !matches!(&plan[plan_index], PlannedTest::Run { .. }) {
                        continue;
                    }
                    match run_one(&plan[plan_index], capture_dir, plan_index) {
                        Ok(outcome) => outcomes.lock().unwrap()[plan_index] = Some(outcome),
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
fn run_one(test: &PlannedTest, capture_dir: &Path, slot: usize) -> Result<Outcome, String> {
    let PlannedTest::Run { display, exe, index, timeout, attrs, comptime } = test else {
        unreachable!("only runnable tests are executed");
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
        .arg(index.to_string())
        .current_dir(&work.path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    isolate(&mut command, &work.path, display);
    let mut child = command.spawn().map_err(|e| format!("cannot run {}: {e}", exe.display()))?;
    let mut timed_out_after = None;
    let status = match *timeout {
        None => child.wait().map_err(|e| format!("cannot wait for {}: {e}", exe.display()))?,
        Some(limit) => {
            let start = Instant::now();
            loop {
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
            }
        }
    };
    let stdout = std::fs::read(&stdout_path).unwrap_or_default();
    let stderr = std::fs::read(&stderr_path).unwrap_or_default();
    let note = match timed_out_after {
        Some(ms) => Some(format!("timed out after {ms} ms")),
        None => verdict(ending(&status), attrs, *comptime, &String::from_utf8_lossy(&stderr)).err(),
    };
    Ok(Outcome { passed: note.is_none(), stdout, stderr, note })
}

/// A test process starts with an empty environment. `PATH` stays so `process.run` can find commands; the temporary
/// directory variables point at the test's own directory, so a program that creates temporary files cleans them up
/// with it. On Windows the variables the C runtime needs to start are kept. `BURDIN_TEST` names the running test.
fn isolate(command: &mut Command, workdir: &Path, display: &str) {
    command.env_clear();
    for (name, value) in isolation_env(workdir, display) {
        command.env(name, value);
    }
}

fn isolation_env(workdir: &Path, display: &str) -> Vec<(&'static str, std::ffi::OsString)> {
    let mut vars = vec![
        ("TMPDIR", workdir.as_os_str().to_owned()),
        ("TMP", workdir.as_os_str().to_owned()),
        ("TEMP", workdir.as_os_str().to_owned()),
        ("BURDIN_TEST", std::ffi::OsString::from(display)),
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
/// (`spec/test-runner.md`).
fn verdict(ending: Ending, attrs: &TestAttrs, comptime: Option<TestOutcome>, stderr: &str) -> Result<(), String> {
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

fn print_failure(display: &str, outcome: &Outcome) {
    if let Some(note) = &outcome.note
        && !note.is_empty()
    {
        println!("---- test \"{display}\": {note} ----");
    }
    print_output(display, outcome);
}

fn print_output(display: &str, outcome: &Outcome) {
    print_stream(display, "stdout", &outcome.stdout);
    print_stream(display, "stderr", &outcome.stderr);
}

fn print_stream(display: &str, stream: &str, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    println!("---- test \"{display}\" {stream} ----");
    print!("{}", String::from_utf8_lossy(bytes));
    if !bytes.ends_with(b"\n") {
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::PackageFiles;

    fn args(target: &str) -> Args {
        Args {
            command: "test".to_string(),
            target: PathBuf::from(target),
            release: false,
            debug: false,
            strip: false,
            lto: false,
            pgo_generate: false,
            pgo_use: None,
            output: None,
            emit_ir: None,
            link_libs: Vec::new(),
            program_args: Vec::new(),
            diagnostics: crate::DiagnosticOptions::default(),
            comptime_steps: None,
            emit_deps: None,
            jobs: None,
            verbose: false,
            packages: Vec::new(),
            workspace: false,
            ignored_only: false,
            include_ignored: false,
            filter: None,
            timeout_ms: None,
        }
    }

    fn package(name: &str, dir: Option<&str>) -> PackageFiles {
        PackageFiles {
            name: name.to_string(),
            dir: dir.map(PathBuf::from),
            modules: Vec::new(),
            deps: Vec::new(),
            build_allow: Vec::new(),
            root_module: None,
        }
    }

    fn workspace(entry: usize, packages: Vec<PackageFiles>) -> Workspace {
        Workspace { packages, entry_package: entry, entry_module: 0, stem: "entry".to_string() }
    }

    fn attrs() -> TestAttrs {
        TestAttrs::default()
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

        let mut missing = args.clone_for_test();
        missing.packages = vec!["nope".to_string()];
        assert!(select_packages(&missing, &ws).unwrap_err().contains("no package named `nope`"));

        let mut embedded = args.clone_for_test();
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
        let vars = isolation_env(workdir, "one");
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

    #[test]
    fn the_filter_and_ignore_modes_select_tests() {
        fn test(name: &str, ignore: bool) -> burdin_tir::TestEntry {
            burdin_tir::TestEntry {
                name: name.to_string(),
                func: burdin_tir::FuncId(0),
                body: burdin_tir::FuncId(0),
                attrs: TestAttrs { ignore, ..TestAttrs::default() },
                comptime: None,
            }
        }
        let compiled = vec![(
            "app".to_string(),
            PathBuf::from("app-bin"),
            vec![test("one", false), test("skipped", true), test("two", false)],
        )];

        let (plan, ignored) = plan_tests(&args("app"), &compiled);
        assert_eq!(ignored, 1);
        assert_eq!(plan.len(), 3);

        let mut only = args("app");
        only.ignored_only = true;
        let (plan, ignored) = plan_tests(&only, &compiled);
        assert_eq!(ignored, 0);
        assert_eq!(plan.len(), 1);

        let mut all = args("app");
        all.include_ignored = true;
        let (plan, ignored) = plan_tests(&all, &compiled);
        assert_eq!(ignored, 0);
        assert_eq!(plan.len(), 3);

        let mut filtered = args("app");
        filtered.filter = Some("two".to_string());
        let (plan, ignored) = plan_tests(&filtered, &compiled);
        assert_eq!(ignored, 0);
        assert_eq!(plan.len(), 1);
        assert!(matches!(&plan[0], PlannedTest::Run { display, .. } if display == "two"));
    }

    impl Args {
        /// A copy for tests that need two variants; `Args` has no `Clone` because the command line is parsed once.
        fn clone_for_test(&self) -> Args {
            Args {
                command: self.command.clone(),
                target: self.target.clone(),
                release: self.release,
                debug: self.debug,
                strip: self.strip,
                lto: self.lto,
                pgo_generate: self.pgo_generate,
                pgo_use: self.pgo_use.clone(),
                output: self.output.clone(),
                emit_ir: self.emit_ir.clone(),
                link_libs: self.link_libs.clone(),
                program_args: self.program_args.clone(),
                diagnostics: crate::DiagnosticOptions::default(),
                comptime_steps: self.comptime_steps,
                emit_deps: self.emit_deps.clone(),
                jobs: self.jobs,
                verbose: self.verbose,
                packages: self.packages.clone(),
                workspace: self.workspace,
                ignored_only: self.ignored_only,
                include_ignored: self.include_ignored,
                filter: self.filter.clone(),
                timeout_ms: self.timeout_ms,
            }
        }
    }
}
