//! `burdin bench`: compile the selected packages in bench mode and measure each `bench` block in its own process.
//!
//! The observable contract is `spec/bench.md`. A bench build compiles in the release profile (the one that
//! measures what ships), warms each bench up, picks an iteration count so one sample takes about
//! `--sample-time` milliseconds, takes `--sample-count` samples, and reports the median, the mean, a 95%
//! confidence interval on the mean, the median absolute deviation, and the samples beyond it. The measured
//! process prints one machine-readable line (`runtime::define_bench_main`); every number the report shows is
//! computed here, from that line. Benches run one at a time — `-j` bounds the compilation threads only —
//! because running two benches at once would put each other's load in the measurements. Baselines are stored
//! under the package's target directory; `--profile-time` runs one bench in a loop for an external profiler.

use crate::test_runner::select::name_selected;
use crate::test_runner::{isolation_env, package_target, select_packages};
use crate::{Args, CompileOptions, EntryMode, Site, TempDir, TestFormat, compile};
use burdin_diag::json_escape;
use burdin_tir::BenchEntry;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

/// The warmup of each bench when `--warmup` says nothing (milliseconds).
const DEFAULT_WARMUP_MS: u64 = 100;
/// How many samples each bench takes when `--sample-count` says nothing.
const DEFAULT_SAMPLE_COUNT: u32 = 20;
/// The target duration of one sample when `--sample-time` says nothing (milliseconds).
const DEFAULT_SAMPLE_TIME_MS: u64 = 5;
/// The per-bench timeout when `--timeout` says nothing. A bench that hangs must not block a run forever.
const DEFAULT_TIMEOUT_MS: u64 = 600_000;
/// How often a running bench is checked against its deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(5);
/// The one line a measured bench prints its numbers on (`spec/bench.md`, BN4).
const RESULT_PREFIX: &str = "burdin-bench-result:";
/// The 95% multiplier of a normal confidence interval on the mean.
const Z_95: f64 = 1.96;
/// A sample counts as an outlier when it is this many median absolute deviations from the median, and a change
/// is beyond noise when the medians differ by more than this many deviations.
const OUTLIER_MADS: f64 = 3.0;

/// One bench of a compiled package: everything needed to select, list, measure, and report it.
struct BenchCase {
    package: String,
    /// The dotted module path inside the package; empty for the package's entry file.
    module: String,
    /// The name written in `bench "name"`.
    name: String,
    /// The bench's identity inside its package: `<module>.<name>`, or `<name>` for an empty module. A baseline
    /// is keyed by this, so which packages a run selected does not change a comparison.
    local: String,
    /// The name the report shows: `<name>`, or `<package>::<name>` when more than one package runs.
    display: String,
    /// The full path filters match and `--exact` compares.
    path: String,
    file: String,
    line: u32,
    exe: PathBuf,
    /// The dispatcher index: the argument that selects this bench in the package's binary.
    index: usize,
}

/// One measured bench: its samples, or why it has none.
struct Measurement {
    /// The iterations per sample; every sample of one bench shares the count.
    iters: Option<u64>,
    /// Each sample's total nanoseconds.
    samples: Vec<u64>,
    /// Why the bench has no measurements: a timeout, an abort, or no result line.
    failure: Option<String>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    duration: Duration,
}

/// The statistics of one measured bench (`spec/bench.md`, BN5), all in nanoseconds per iteration.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Stats {
    median: f64,
    mean: f64,
    ci_low: f64,
    ci_high: f64,
    /// The median absolute deviation: the spread the outlier rule and the noise rule use.
    mad: f64,
    /// How many samples are more than [`OUTLIER_MADS`] deviations from the median.
    outliers: usize,
}

/// The comparison of one measurement with its baseline entry (`spec/bench.md`, BN11).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Compared {
    /// The bench is not in the baseline file.
    New,
    /// The medians differ by at most three times the larger spread.
    WithinNoise,
    /// `percent` is the change of the median, positive for slower.
    BeyondNoise { percent: f64 },
}

impl Compared {
    /// The human wording of the comparison.
    fn text(&self) -> String {
        match self {
            Compared::New => "new (not in the baseline)".to_string(),
            Compared::WithinNoise => "unchanged (within noise)".to_string(),
            Compared::BeyondNoise { percent } => {
                let direction = if *percent > 0.0 { "slower" } else { "faster" };
                format!("{percent:.1}% {direction} (beyond noise)")
            }
        }
    }

    /// Whether the bench regressed enough to fail a `--fail-on-regression` run: slower by more than `limit`
    /// percent, and beyond noise so a noisy machine does not fail a clean build.
    fn regressed(&self, limit: f64) -> bool {
        match self {
            Compared::BeyondNoise { percent } => *percent > 0.0 && *percent > limit,
            _ => false,
        }
    }

    /// The fields of the `baseline` object of a JSON event.
    fn json(&self) -> String {
        match self {
            Compared::New => "\"new\":true".to_string(),
            Compared::WithinNoise => "\"new\":false,\"changed\":false".to_string(),
            Compared::BeyondNoise { percent } => {
                format!("\"new\":false,\"changed\":true,\"change_percent\":{percent:.3}")
            }
        }
    }
}

/// One compiled package: its bench binary and the benches inside it.
struct CompiledPackage {
    name: String,
    exe: PathBuf,
    benches: Vec<BenchEntry>,
    sites: Vec<Site>,
}

/// One saved bench of a baseline file: all the comparison needs.
#[derive(Debug)]
struct BaselineBench {
    median: f64,
    mad: f64,
}

/// A loaded baseline: version 1, one entry per package-local bench path.
#[derive(Debug)]
struct Baseline {
    benches: HashMap<String, BaselineBench>,
}

pub(crate) fn run(args: &Args) -> ExitCode {
    if let Err(code) = crate::sync_dependencies(&args.target, args.locked) {
        return code;
    }
    let workspace = match crate::package::load_for_tests(&args.target) {
        Ok(workspace) => workspace,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    };
    let selected = match select_packages(args, &workspace) {
        Ok(selected) => selected,
        Err(message) => {
            return crate::cli::usage_error("bench", &message);
        }
    };
    // Baselines live under the entry package's target directory, so they need a package on disk.
    let package_dir = workspace.packages[workspace.entry_package]
        .dir
        .as_ref()
        .map(|dir| dir.join("target").join("burdin").join("bench"));
    if package_dir.is_none() && (args.baseline.is_some() || args.save_baseline.is_some()) {
        return crate::cli::usage_error(
            "bench",
            "`--baseline` and `--save-baseline` need a package directory to keep their file in, not a single file",
        );
    }
    let baseline = match (args.baseline.as_deref(), package_dir.as_deref()) {
        (Some(name), Some(dir)) => match load_baseline(dir, name) {
            Ok(baseline) => Some(baseline),
            Err(message) => {
                eprintln!("error: {message}");
                return ExitCode::from(2);
            }
        },
        // A single-file target was rejected above, so a name without a directory cannot happen.
        (Some(_), None) => unreachable!("a baseline name on a single-file target"),
        (None, _) => None,
    };

    // One temporary directory holds every bench binary. Each package is compiled as its own entry package, so
    // its benches see its private items and are checked with its own `main` rules.
    let binaries = match TempDir::new() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("error: cannot create a temporary directory: {e}");
            return ExitCode::from(2);
        }
    };
    // Benchmarks measure the release profile: that is the default, and `--release` only spells it out
    // (`spec/bench.md`, BN3).
    let options = CompileOptions {
        release: true,
        debug: args.debug,
        strip: args.strip,
        lto: args.lto,
        pgo_generate: args.pgo_generate,
        pgo_use: args.pgo_use.as_deref(),
        emit_ir: args.emit_ir.as_deref(),
        emit: None,
        link_libs: &args.link_libs,
        mode: EntryMode::Bench,
        diagnostics: &args.diagnostics,
        comptime_steps: args.comptime_steps,
        emit_deps: None,
        report_clean: false,
        run_build_scripts: true,
        locked: args.locked,
        jobs: crate::jobs_or_default(args.jobs),
        sanitize: Default::default(),
        fuzz_target: None,
    };
    let mut compiled = Vec::new();
    for &index in &selected {
        let package = &workspace.packages[index];
        let target = package_target(&workspace, args, index);
        let exe = binaries.path.join(&package.name).with_extension(std::env::consts::EXE_EXTENSION);
        match compile(&target, &binaries.path, Some(&exe), &options) {
            Ok(built) => compiled.push(CompiledPackage {
                name: package.name.clone(),
                exe,
                benches: built.benches,
                sites: built.bench_sites,
            }),
            Err(code) => return code,
        }
    }

    let cases = discover(&compiled);
    let (chosen, filtered_out) = select(args, cases);
    if chosen.is_empty() && (!args.filters.is_empty() || !args.skip.is_empty()) {
        eprintln!("note: no bench matches the filters");
    }

    if args.list {
        match args.format {
            TestFormat::Human => print_list_human(&chosen, filtered_out),
            TestFormat::Json => print_list_json(&chosen),
        }
        return ExitCode::SUCCESS;
    }

    if let Some(seconds) = args.profile_time {
        if chosen.len() != 1 {
            let how_many = if chosen.is_empty() { "no bench" } else { "several benches" };
            return crate::cli::usage_error(
                "bench",
                &format!("`--profile-time` runs one bench, but {how_many} match the filters"),
            );
        }
        return profile(&chosen[0], seconds);
    }

    let json = args.format == TestFormat::Json;
    let package_names: Vec<String> = compiled.iter().map(|package| package.name.clone()).collect();
    if json {
        suite_started(&package_names, chosen.len(), filtered_out);
    }
    let timeout = default_timeout(args);
    let warmup = args.warmup_ms.unwrap_or(DEFAULT_WARMUP_MS);
    let samples = args.sample_count.unwrap_or(DEFAULT_SAMPLE_COUNT);
    let sample_ms = args.sample_time_ms.unwrap_or(DEFAULT_SAMPLE_TIME_MS);

    // Benches run one at a time: two benches at once would measure each other's load (`spec/bench.md`, BN14).
    let started = Instant::now();
    let mut measured = 0usize;
    let mut failed = 0usize;
    // What happened to each bench, kept so `--save-baseline` can store every measurement after the run.
    let mut results: Vec<(BenchCase, Measurement, Option<Stats>)> = Vec::new();
    for (slot, case) in chosen.into_iter().enumerate() {
        if json {
            bench_started(&case);
        }
        let measurement = match measure(&case, timeout, warmup, samples, sample_ms, &binaries.path, slot) {
            Ok(measurement) => measurement,
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::from(2);
            }
        };
        let stats = measurement.iters.and_then(|iters| statistics(&measurement.samples, iters));
        let compared = match (&baseline, stats) {
            (Some(baseline), Some(stats)) => Some(compare(baseline, &case, stats)),
            // A bench that failed next to a baseline is new to the report, not to the file.
            (Some(_), None) => Some(Compared::New),
            (None, _) => None,
        };
        let failed_bench = measurement.failure.is_some()
            || compared
                .as_ref()
                .is_some_and(|compared| args.fail_on_regression.is_some_and(|limit| compared.regressed(limit)));
        if failed_bench {
            failed += 1
        } else {
            measured += 1
        }
        match json {
            true => bench_finished(&case, &measurement, stats, compared.as_ref(), failed_bench, args.verbose),
            false => print_human(
                &case,
                &measurement,
                stats,
                compared.as_ref(),
                args.baseline.as_deref(),
                failed_bench,
                args.verbose,
            ),
        }
        results.push((case, measurement, stats));
    }

    // What a `--save-baseline` run stores: every bench that produced statistics, regressed or not, so saving
    // over the baseline a run just compared with is a rebase, not a no-op.
    if let (Some(name), Some(dir)) = (args.save_baseline.as_deref(), package_dir.as_deref()) {
        let to_save: Vec<(&BenchCase, u64, &[u64], Stats)> = results
            .iter()
            .filter_map(|(case, measurement, stats)| {
                stats.map(|stats| (case, measurement.iters.unwrap_or(0), measurement.samples.as_slice(), stats))
            })
            .collect();
        if let Err(e) = save_baseline(dir, name, &to_save) {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    }

    if json {
        suite_finished(measured, failed, filtered_out, started.elapsed());
    } else {
        let filtered = if filtered_out > 0 { format!(", {filtered_out} filtered out") } else { String::new() };
        println!("bench result: {measured} measured, {failed} failed{filtered}");
    }
    if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) }
}

/// The per-bench timeout that applies when `--timeout` says nothing; `Some(0)` disables it.
fn default_timeout(args: &Args) -> Option<Duration> {
    match args.timeout_ms {
        Some(0) => None,
        Some(ms) => Some(Duration::from_millis(ms)),
        None => Some(Duration::from_millis(DEFAULT_TIMEOUT_MS)),
    }
}

/// Every bench of the compiled packages, in build order and then declaration order. When more than one package
/// runs, names are qualified with the package, like the test runner's discovery.
fn discover(compiled: &[CompiledPackage]) -> Vec<BenchCase> {
    let qualify = compiled.len() > 1;
    let mut cases = Vec::new();
    for package in compiled {
        for (index, bench) in package.benches.iter().enumerate() {
            let local =
                if bench.module.is_empty() { bench.name.clone() } else { format!("{}.{}", bench.module, bench.name) };
            let (display, path) = if qualify {
                (format!("{}::{}", package.name, bench.name), format!("{}::{local}", package.name))
            } else {
                (bench.name.clone(), local.clone())
            };
            let (file, line) = package.sites.get(index).map(|site| (site.file.clone(), site.line)).unwrap_or_default();
            cases.push(BenchCase {
                package: package.name.clone(),
                module: bench.module.clone(),
                name: bench.name.clone(),
                local,
                display,
                path,
                file,
                line,
                exe: package.exe.clone(),
                index,
            });
        }
    }
    cases
}

/// The benches the name rules keep (`spec/bench.md`, BN8): a bench is measured when some filter selects its full
/// path and no `--skip` pattern does; a bench left out counts as filtered out.
fn select(args: &Args, cases: Vec<BenchCase>) -> (Vec<BenchCase>, usize) {
    let mut filtered_out = 0usize;
    let mut chosen = Vec::new();
    for case in cases {
        if name_selected(args, &case.path) {
            chosen.push(case);
        } else {
            filtered_out += 1;
        }
    }
    (chosen, filtered_out)
}

/// Runs one measured bench process and captures what it printed. The output goes to files outside the bench's
/// working directory, the way the test runner captures, so a bench that writes a lot cannot deadlock.
fn measure(
    case: &BenchCase,
    timeout: Option<Duration>,
    warmup_ms: u64,
    samples: u32,
    sample_ms: u64,
    capture_dir: &Path,
    slot: usize,
) -> Result<Measurement, String> {
    let work = TempDir::new().map_err(|e| format!("cannot create a temporary directory: {e}"))?;
    let stdout_path = capture_dir.join(format!("capture-{slot}.out"));
    let stderr_path = capture_dir.join(format!("capture-{slot}.err"));
    let stdout_file =
        std::fs::File::create(&stdout_path).map_err(|e| format!("cannot create {}: {e}", stdout_path.display()))?;
    let stderr_file =
        std::fs::File::create(&stderr_path).map_err(|e| format!("cannot create {}: {e}", stderr_path.display()))?;
    let mut command = Command::new(&case.exe);
    command
        .args([
            case.index.to_string(),
            "measure".to_string(),
            warmup_ms.to_string(),
            samples.to_string(),
            sample_ms.to_string(),
        ])
        .current_dir(&work.path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    command.env_clear();
    for (name, value) in isolation_env(&work.path, "BURDIN_BENCH", &case.display) {
        command.env(name, value);
    }
    let start = Instant::now();
    let mut child = command.spawn().map_err(|e| format!("cannot run {}: {e}", case.exe.display()))?;
    let mut timed_out_after = None;
    let status = match timeout {
        None => child.wait().map_err(|e| format!("cannot wait for {}: {e}", case.exe.display()))?,
        Some(limit) => loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => return Err(format!("cannot wait for {}: {e}", case.exe.display())),
            }
            if start.elapsed() >= limit {
                timed_out_after = Some(limit.as_millis());
                let _ = child.kill();
                break child.wait().map_err(|e| format!("cannot wait for {}: {e}", case.exe.display()))?;
            }
            std::thread::sleep(POLL_INTERVAL);
        },
    };
    let duration = start.elapsed();
    let stdout = std::fs::read(&stdout_path).unwrap_or_default();
    let stderr = std::fs::read(&stderr_path).unwrap_or_default();

    let mut failure = if let Some(ms) = timed_out_after {
        Some(format!("timed out after {ms} ms"))
    } else if !status.success() {
        Some(match status.code() {
            Some(code) => format!("the bench aborted (exit code {code})"),
            None => "the bench was terminated by a signal".to_string(),
        })
    } else {
        None
    };
    // The harness prints one line; a bench that prints is tolerated, so the last result line wins.
    let text = String::from_utf8_lossy(&stdout);
    let line = text.lines().rev().find_map(|line| line.strip_prefix(RESULT_PREFIX)).map(str::trim);
    if failure.is_none() {
        match line {
            None => failure = Some("the bench printed no result line".to_string()),
            Some(line) if parse_result(line, samples).is_none() => {
                failure = Some("the bench printed a malformed result line".to_string());
            }
            Some(_) => {}
        }
    }
    let (iters, samples_read) = match (&failure, line.filter(|_| failure.is_none())) {
        (&None, Some(line)) => {
            let (iters, samples) = parse_result(line, samples).expect("the line was just validated");
            (Some(iters), samples)
        }
        _ => (None, Vec::new()),
    };
    Ok(Measurement { iters, samples: samples_read, failure, stdout, stderr, duration })
}

/// The numbers of one result line: the iteration count and each sample's total nanoseconds. `None` when the
/// line is not the `<iters> <sample>...` shape the harness prints, with exactly the sample count asked for
/// (`spec/bench.md`, BN4).
fn parse_result(line: &str, samples: u32) -> Option<(u64, Vec<u64>)> {
    let mut numbers = line.split_whitespace();
    let iters = numbers.next()?.parse::<u64>().ok()?;
    let read: Option<Vec<u64>> = numbers.map(|text| text.parse::<u64>().ok()).collect();
    let read = read?;
    (read.len() == samples as usize).then_some((iters, read))
}

/// The statistics of a measured bench (`spec/bench.md`, BN5): the median and the mean of the per-iteration
/// times, a 95% confidence interval on the mean, the median absolute deviation, and how many samples sit more
/// than three deviations from the median.
fn statistics(samples: &[u64], iters: u64) -> Option<Stats> {
    if samples.is_empty() || iters == 0 {
        return None;
    }
    let per: Vec<f64> = samples.iter().map(|&ns| ns as f64 / iters as f64).collect();
    let middle = median(&per);
    let mean = per.iter().sum::<f64>() / per.len() as f64;
    let deviations: Vec<f64> = per.iter().map(|x| (x - middle).abs()).collect();
    let mad = median(&deviations);
    // The interval is the normal approximation; with one sample there is nothing to spread over.
    let sd = if per.len() > 1 {
        let sum: f64 = per.iter().map(|x| (x - mean) * (x - mean)).sum();
        (sum / (per.len() - 1) as f64).sqrt()
    } else {
        0.0
    };
    let half = Z_95 * sd / (per.len() as f64).sqrt();
    let outliers = per.iter().filter(|x| (**x - middle).abs() > OUTLIER_MADS * mad).count();
    Some(Stats { median: middle, mean, ci_low: mean - half, ci_high: mean + half, mad, outliers })
}

/// The middle of `values` sorted, the average of the two middles when the count is even.
fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("the times are finite"));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 { sorted[mid] } else { (sorted[mid - 1] + sorted[mid]) / 2.0 }
}

/// The comparison of a measurement with its baseline entry (`spec/bench.md`, BN11): a change is beyond noise
/// when the medians differ by more than three times the larger of the two spreads. A run whose every sample was
/// identical (a zero deviation) counts any difference as beyond noise, which is exactly what a stable machine
/// should report.
fn compare(baseline: &Baseline, case: &BenchCase, stats: Stats) -> Compared {
    let Some(old) = baseline.benches.get(&case.local) else { return Compared::New };
    let change = stats.median - old.median;
    if change.abs() <= OUTLIER_MADS * old.mad.max(stats.mad) {
        return Compared::WithinNoise;
    }
    Compared::BeyondNoise { percent: change / old.median * 100.0 }
}

/// `--profile-time <seconds>`: run the bench in a loop without measurement until the time is up, then stop it
/// (`spec/bench.md`, BN10). The process is killed, which is the intended ending: the profiler attached to it is
/// what collected the data.
fn profile(case: &BenchCase, seconds: f64) -> ExitCode {
    let work = match TempDir::new() {
        Ok(work) => work,
        Err(e) => {
            eprintln!("error: cannot create a temporary directory: {e}");
            return ExitCode::from(2);
        }
    };
    let mut command = Command::new(&case.exe);
    command.args([case.index.to_string(), "profile".to_string()]).current_dir(&work.path).stdin(Stdio::null());
    command.env_clear();
    for (name, value) in isolation_env(&work.path, "BURDIN_BENCH", &case.display) {
        command.env(name, value);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("error: cannot run {}: {e}", case.exe.display());
            return ExitCode::from(2);
        }
    };
    let limit = Duration::from_secs_f64(seconds);
    let start = Instant::now();
    while start.elapsed() < limit {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(e) => {
                eprintln!("error: cannot wait for {}: {e}", case.exe.display());
                return ExitCode::from(2);
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    let _ = child.kill();
    let _ = child.wait();
    println!("profiled {} for {seconds} s", case.display);
    ExitCode::SUCCESS
}

// ----- the human report ------------------------------------------------------------------------------------------

/// One bench's lines of the human report: the statistics, the baseline comparison when there is one, and the
/// failure line when the bench has none or regressed (`spec/bench.md`, BN6).
fn print_human(
    case: &BenchCase,
    measurement: &Measurement,
    stats: Option<Stats>,
    compared: Option<&Compared>,
    baseline_name: Option<&str>,
    failed: bool,
    verbose: bool,
) {
    if let Some(reason) = &measurement.failure {
        print_capture(&case.display, "stdout", &measurement.stdout);
        print_capture(&case.display, "stderr", &measurement.stderr);
        println!("FAIL {}: {reason}", case.display);
        return;
    }
    let Some(stats) = stats else { return };
    println!(
        "{}: {} (mean {}, ci 95% {} to {}, mad {}, {}/{} outliers)",
        case.display,
        format_ns(stats.median),
        format_ns(stats.mean),
        format_ns(stats.ci_low),
        format_ns(stats.ci_high),
        format_ns(stats.mad),
        stats.outliers,
        measurement.samples.len()
    );
    if let (Some(name), Some(compared)) = (baseline_name, compared) {
        println!("  baseline {name}: {}", compared.text());
    }
    if failed {
        println!("FAIL {}: regressed over the baseline {}", case.display, baseline_name.unwrap_or_default());
    }
    if verbose {
        print_capture(&case.display, "stdout", &measurement.stdout);
        print_capture(&case.display, "stderr", &measurement.stderr);
    }
}

/// The captured output of a bench under its heading, the way the test report prints a failing test's output.
fn print_capture(display: &str, stream: &str, captured: &[u8]) {
    if captured.is_empty() {
        return;
    }
    println!("---- bench \"{display}\" {stream} ----");
    println!("{}", String::from_utf8_lossy(captured));
}

/// A time in the largest unit that keeps it at least one: `41.20 ns`, `1.24 us`, `3.50 ms`, `2.10 s`.
fn format_ns(ns: f64) -> String {
    let (value, unit) = if ns < 1_000.0 {
        (ns, "ns")
    } else if ns < 1_000_000.0 {
        (ns / 1_000.0, "us")
    } else if ns < 1_000_000_000.0 {
        (ns / 1_000_000.0, "ms")
    } else {
        (ns / 1_000_000_000.0, "s")
    };
    format!("{value:.2} {unit}")
}

// ----- --list ----------------------------------------------------------------------------------------------------

/// The human listing: one line per selected bench and a count line (`spec/bench.md`, BN7).
fn print_list_human(chosen: &[BenchCase], filtered_out: usize) {
    for case in chosen {
        println!("{}", case.path);
    }
    let benches = if chosen.len() == 1 { "bench" } else { "benches" };
    let filtered = if filtered_out > 0 { format!(", {filtered_out} filtered out") } else { String::new() };
    println!("{} {benches}{filtered}", chosen.len());
}

/// The JSON listing: one object per selected bench, one per line (`spec/bench.md`, BN7).
fn print_list_json(chosen: &[BenchCase]) {
    for case in chosen {
        println!("{{{}}}", identity(case));
    }
}

// ----- the JSON event stream -------------------------------------------------------------------------------------

/// The fields every event about one bench carries, in the fixed order of the spec.
fn identity(case: &BenchCase) -> String {
    format!(
        "\"package\":{},\"module\":{},\"name\":{},\"path\":{},\"file\":{},\"line\":{}",
        json_str(&case.package),
        json_str(&case.module),
        json_str(&case.name),
        json_str(&case.path),
        json_str(&case.file),
        case.line
    )
}

fn suite_started(packages: &[String], benches: usize, filtered_out: usize) {
    let names: Vec<String> = packages.iter().map(|name| json_str(name)).collect();
    println!(
        "{{\"event\":\"suite_started\",\"version\":1,\"packages\":[{}],\"bench_count\":{benches},\"filtered_out\":{filtered_out}}}",
        names.join(",")
    );
}

fn bench_started(case: &BenchCase) {
    println!("{{\"event\":\"bench_started\",{}}}", identity(case));
}

/// The `bench_finished` event of one bench: its statistics when it has them, the comparison with the baseline
/// when there is one, the reason it failed, and the captured output (`spec/bench.md`, BN7).
fn bench_finished(
    case: &BenchCase,
    measurement: &Measurement,
    stats: Option<Stats>,
    compared: Option<&Compared>,
    failed: bool,
    verbose: bool,
) {
    let mut fields = identity(case);
    push(&mut fields, &format!("\"duration_ms\":{}", measurement.duration.as_millis()));
    match stats {
        Some(stats) => {
            let iters = measurement.iters.unwrap_or(0);
            let samples: Vec<String> = measurement.samples.iter().map(u64::to_string).collect();
            push(&mut fields, &format!("\"iters\":{iters}"));
            push(&mut fields, &format!("\"samples\":[{}]", samples.join(",")));
            push(&mut fields, &format!("\"median_ns\":{:.3}", stats.median));
            push(&mut fields, &format!("\"mean_ns\":{:.3}", stats.mean));
            push(&mut fields, &format!("\"ci_low_ns\":{:.3}", stats.ci_low));
            push(&mut fields, &format!("\"ci_high_ns\":{:.3}", stats.ci_high));
            push(&mut fields, &format!("\"mad_ns\":{:.3}", stats.mad));
            push(&mut fields, &format!("\"outliers\":{}", stats.outliers));
        }
        None => push(&mut fields, "\"iters\":null,\"samples\":[]"),
    }
    if let Some(compared) = compared {
        push(&mut fields, &format!("\"baseline\":{{{}}}", compared.json()));
    }
    match &measurement.failure {
        Some(reason) => push(&mut fields, &format!("\"reason\":{}", json_str(reason))),
        None if failed => push(&mut fields, "\"reason\":\"regressed over the baseline\""),
        None => push(&mut fields, "\"reason\":null"),
    }
    if measurement.failure.is_some() || failed || verbose {
        push(&mut fields, &format!("\"stdout\":{}", json_bytes(&measurement.stdout)));
        push(&mut fields, &format!("\"stderr\":{}", json_bytes(&measurement.stderr)));
    }
    println!("{{\"event\":\"bench_finished\",{fields}}}");
}

fn suite_finished(measured: usize, failed: usize, filtered_out: usize, duration: Duration) {
    println!(
        "{{\"event\":\"suite_finished\",\"measured\":{measured},\"failed\":{failed},\"filtered_out\":{filtered_out},\"duration_ms\":{}}}",
        duration.as_millis()
    );
}

fn push(fields: &mut String, field: &str) {
    fields.push(',');
    fields.push_str(field);
}

/// A JSON string, escaped in one place (`burdin_diag::json_escape`).
fn json_str(text: &str) -> String {
    format!("\"{}\"", json_escape(text))
}

/// Captured process output as a JSON string: bytes that are not UTF-8 are replaced.
fn json_bytes(bytes: &[u8]) -> String {
    json_str(&String::from_utf8_lossy(bytes))
}

// ----- baselines -------------------------------------------------------------------------------------------------

/// Where a named baseline lives: `<package>/target/burdin/bench/<name>.json`.
fn baseline_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.json"))
}

/// Writes the measured statistics of a run as one baseline file (`spec/bench.md`, BN11): version 1, one entry
/// per package-local bench path. The directory is created on first use.
fn save_baseline(dir: &Path, name: &str, measured: &[(&BenchCase, u64, &[u64], Stats)]) -> Result<(), String> {
    if let Err(e) = std::fs::create_dir_all(dir) {
        return Err(format!("cannot create {}: {e}", dir.display()));
    }
    let path = baseline_path(dir, name);
    let mut text = String::from("{\"version\":1,\"benches\":{");
    for (index, (case, iters, samples, stats)) in measured.iter().enumerate() {
        if index > 0 {
            text.push(',');
        }
        let sample_list: Vec<String> = samples.iter().map(u64::to_string).collect();
        write!(
            text,
            "{}:{{\"iters\":{iters},\"samples\":[{}],\"median_ns\":{:.3},\"mean_ns\":{:.3},\"ci_low_ns\":{:.3},\"ci_high_ns\":{:.3},\"mad_ns\":{:.3}}}",
            json_str(&case.local),
            sample_list.join(","),
            stats.median,
            stats.mean,
            stats.ci_low,
            stats.ci_high,
            stats.mad,
        )
        .map_err(|e| format!("cannot format the baseline: {e}"))?;
    }
    text.push_str("}}\n");
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Reads one baseline file: version 1, one entry per bench path with the median and the spread a comparison
/// needs. A file of another version is refused rather than guessed at.
fn load_baseline(dir: &Path, name: &str) -> Result<Baseline, String> {
    let path = baseline_path(dir, name);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| format!("no baseline named `{name}`; save one with `--save-baseline {name}`"))?;
    let value = parse_json(&text).map_err(|message| format!("cannot read {}: {message}", path.display()))?;
    let Json::Obj(entries) = &value else {
        return Err(format!("cannot read {}: not a baseline file", path.display()));
    };
    let version = entries
        .iter()
        .find(|(key, _)| key == "version")
        .and_then(|(_, value)| match value {
            Json::Num(version) => Some(*version),
            _ => None,
        })
        .unwrap_or(0);
    if version != 1 {
        return Err(format!(
            "the baseline `{name}` is version {version}; this driver reads version 1; save the baseline again"
        ));
    }
    let Some((_, Json::Obj(benches))) = entries.iter().find(|(key, _)| key == "benches") else {
        return Err(format!("cannot read {}: no `benches` object", path.display()));
    };
    let mut parsed = HashMap::new();
    for (local, entry) in benches {
        let Json::Obj(fields) = entry else {
            return Err(format!("cannot read {}: the entry for `{local}` is not an object", path.display()));
        };
        let number = |name: &str| -> Option<f64> {
            fields.iter().find(|(key, _)| key == name).and_then(|(_, value)| match value {
                Json::NumF(value) => Some(*value),
                Json::Num(value) => Some(*value as f64),
                _ => None,
            })
        };
        let Some(median) = number("median_ns") else {
            return Err(format!("cannot read {}: the entry for `{local}` has no median", path.display()));
        };
        parsed.insert(local.clone(), BaselineBench { median, mad: number("mad_ns").unwrap_or(0.0) });
    }
    Ok(Baseline { benches: parsed })
}

// ----- the JSON the baseline files hold ---------------------------------------------------------------------------

/// The JSON values a baseline file holds. The driver writes objects, strings, integers, and decimals; a reader
/// accepts what the writer emits, plus the whitespace a hand-edited file may add.
#[derive(Clone, Debug, PartialEq)]
enum Json {
    Num(u64),
    NumF(f64),
    Str(String),
    Obj(Vec<(String, Json)>),
    Arr(Vec<Json>),
}

/// Parses one JSON document, the subset the driver's own baseline writer emits.
fn parse_json(text: &str) -> Result<Json, String> {
    let mut parser = Parser { bytes: text.as_bytes(), at: 0 };
    let value = parser.value()?;
    parser.space();
    if parser.at != parser.bytes.len() {
        return Err(format!("trailing text after the JSON value at byte {}", parser.at));
    }
    Ok(value)
}

/// A cursor over the bytes of a JSON document.
struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    /// Skips the whitespace the writer never writes but a hand-edited file may.
    fn space(&mut self) {
        while matches!(self.bytes.get(self.at), Some(b' ' | b'\n' | b'\t' | b'\r')) {
            self.at += 1;
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.space();
        match self.bytes.get(self.at) {
            Some(&b'{') => self.object(),
            Some(&b'[') => self.array(),
            Some(&b'"') => Ok(Json::Str(self.string()?)),
            Some(&byte) if byte == b'-' || byte.is_ascii_digit() => self.number(),
            _ => Err(format!("expected a JSON value at byte {}", self.at)),
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.at += 1;
        let mut fields = Vec::new();
        self.space();
        if self.bytes.get(self.at) == Some(&b'}') {
            self.at += 1;
            return Ok(Json::Obj(fields));
        }
        loop {
            self.space();
            let key = self.string()?;
            self.space();
            if self.bytes.get(self.at) != Some(&b':') {
                return Err(format!("expected `:` at byte {}", self.at));
            }
            self.at += 1;
            fields.push((key, self.value()?));
            self.space();
            match self.bytes.get(self.at) {
                Some(&b',') => self.at += 1,
                Some(&b'}') => {
                    self.at += 1;
                    return Ok(Json::Obj(fields));
                }
                _ => return Err(format!("expected `,` or `}}` at byte {}", self.at)),
            }
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.at += 1;
        let mut items = Vec::new();
        self.space();
        if self.bytes.get(self.at) == Some(&b']') {
            self.at += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            items.push(self.value()?);
            self.space();
            match self.bytes.get(self.at) {
                Some(&b',') => self.at += 1,
                Some(&b']') => {
                    self.at += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(format!("expected `,` or `]` at byte {}", self.at)),
            }
        }
    }

    /// One string, with the escapes the driver's own JSON writer can emit. Bytes are UTF-8 as written; a
    /// sequence that is not valid UTF-8 is replaced rather than rejected, the way the reports treat output.
    fn string(&mut self) -> Result<String, String> {
        if self.bytes.get(self.at) != Some(&b'"') {
            return Err(format!("expected a string at byte {}", self.at));
        }
        self.at += 1;
        let mut text = String::new();
        loop {
            match self.bytes.get(self.at) {
                None => return Err("the string is not closed".to_string()),
                Some(&b'"') => {
                    self.at += 1;
                    return Ok(text);
                }
                Some(&b'\\') => {
                    self.at += 1;
                    match self.bytes.get(self.at) {
                        Some(&b'"') => text.push('"'),
                        Some(&b'\\') => text.push('\\'),
                        Some(&b'n') => text.push('\n'),
                        Some(&b't') => text.push('\t'),
                        Some(&b'r') => text.push('\r'),
                        _ => return Err(format!("unknown escape at byte {}", self.at)),
                    }
                    self.at += 1;
                }
                Some(_) => {
                    let start = self.at;
                    self.at += 1;
                    while self.bytes.get(self.at).is_some_and(|byte| !matches!(byte, b'"' | b'\\')) {
                        self.at += 1;
                    }
                    text.push_str(&String::from_utf8_lossy(&self.bytes[start..self.at]));
                }
            }
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.at;
        if self.bytes.get(self.at) == Some(&b'-') {
            self.at += 1;
        }
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        let mut float = false;
        if self.bytes.get(self.at) == Some(&b'.') {
            float = true;
            self.at += 1;
            while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
                self.at += 1;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at]).map_err(|_| "not UTF-8".to_string())?;
        if text.is_empty() || text == "-" {
            return Err(format!("not a number at byte {start}"));
        }
        if float {
            text.parse::<f64>().map(Json::NumF).map_err(|e| e.to_string())
        } else {
            match text.parse::<u64>() {
                Ok(value) => Ok(Json::Num(value)),
                // A number a `u64` cannot hold: keep it as a float rather than reject the whole file.
                Err(_) => text.parse::<f64>().map(Json::NumF).map_err(|e| e.to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_median_averages_the_two_middles() {
        assert_eq!(median(&[3.0]), 3.0);
        assert_eq!(median(&[1.0, 9.0]), 5.0);
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0, 5.0]), 3.0);
        assert_eq!(median(&[5.0, 1.0, 4.0, 2.0, 3.0]), 3.0);
    }

    #[test]
    fn a_result_line_needs_the_iteration_count_and_every_sample() {
        assert_eq!(parse_result("100 5000 5010 4990", 3), Some((100, vec![5000, 5010, 4990])));
        // Too few or too many samples, a missing count, or a word instead of a number is malformed.
        assert_eq!(parse_result("100 5000 5010", 3), None);
        assert_eq!(parse_result("100 5000 5010 4990 1", 3), None);
        assert_eq!(parse_result("", 3), None);
        assert_eq!(parse_result("100 soon", 1), None);
        assert_eq!(parse_result("-1 100", 1), None);
    }

    #[test]
    fn the_statistics_are_computed_from_the_per_iteration_times() {
        // Four samples of 10 iterations each: 1000, 1100, 900, and 1000 nanoseconds per iteration.
        let stats = statistics(&[10_000, 11_000, 9_000, 10_000], 10).expect("the samples are usable");
        assert!((stats.median - 1000.0).abs() < 1e-9, "{}", stats.median);
        assert!((stats.mean - 1000.0).abs() < 1e-9, "{}", stats.mean);
        // The deviations from the median are 0, 100, 100, 0, so the MAD is 50; no sample is beyond 3*50 = 150.
        assert!((stats.mad - 50.0).abs() < 1e-9, "{}", stats.mad);
        assert_eq!(stats.outliers, 0);
        // The interval is the mean plus or minus 1.96 sample standard deviations over the square root of n: the
        // squared deviations are 0, 10000, 10000, 0, over n-1 = 3.
        let sd = (20_000.0_f64 / 3.0).sqrt();
        let half = Z_95 * sd / 2.0;
        assert!((stats.ci_low - (1000.0 - half)).abs() < 1e-9, "{}", stats.ci_low);
        assert!((stats.ci_high - (1000.0 + half)).abs() < 1e-9, "{}", stats.ci_high);
        // Nothing is measured without samples or without iterations.
        assert!(statistics(&[], 10).is_none());
        assert!(statistics(&[10], 0).is_none());
        // One sample has no spread: the interval is the mean itself and nothing is an outlier.
        let one = statistics(&[10_000], 10).expect("one sample is usable");
        assert_eq!(one.outliers, 0);
        assert!((one.ci_low - one.median).abs() < 1e-9);
        assert!((one.ci_high - one.median).abs() < 1e-9);
    }

    /// Arbitrary statistics for comparisons that only look at the median and the deviation.
    fn one_sample() -> Stats {
        Stats { median: 1000.0, mean: 1000.0, ci_low: 1000.0, ci_high: 1000.0, mad: 0.0, outliers: 0 }
    }

    fn case_of(local: &str) -> BenchCase {
        BenchCase {
            package: "app".to_string(),
            module: String::new(),
            name: local.to_string(),
            local: local.to_string(),
            display: local.to_string(),
            path: local.to_string(),
            file: String::new(),
            line: 1,
            exe: PathBuf::new(),
            index: 0,
        }
    }

    #[test]
    fn a_tight_run_counts_any_change_as_beyond_noise() {
        // Every sample is the same: the MAD is zero, so no sample deviates at all.
        let stats = statistics(&[10_000, 10_000, 10_000], 10).expect("the samples are usable");
        assert_eq!(stats.outliers, 0);
        assert!((stats.mad - 0.0).abs() < 1e-9);
        // Against a baseline of the same median, any change is beyond noise.
        let baseline =
            Baseline { benches: [("sum list".to_string(), BaselineBench { median: 1000.0, mad: 0.0 })].into() };
        let case = case_of("sum list");
        let same = compare(&baseline, &case, Stats { median: 1000.0, ..one_sample() });
        assert_eq!(same, Compared::WithinNoise);
        let faster = compare(&baseline, &case, Stats { median: 900.0, ..one_sample() });
        assert_eq!(faster, Compared::BeyondNoise { percent: -10.0 });
        assert!(!faster.regressed(5.0), "a faster bench never regressed");
        let slower = compare(&baseline, &case, Stats { median: 1200.0, ..one_sample() });
        assert_eq!(slower, Compared::BeyondNoise { percent: 20.0 });
        assert!(slower.regressed(5.0), "20% slower is beyond a 5% limit");
        assert!(!slower.regressed(50.0), "20% slower is within a 50% limit");
        // A change inside three deviations of the wider run is noise: 100 is within 3*50.
        let noisy = compare(&baseline, &case, Stats { median: 1100.0, mad: 50.0, ..one_sample() });
        assert_eq!(noisy, Compared::WithinNoise);
        // A bench the file does not know is new, and never a regression.
        assert_eq!(compare(&baseline, &case_of("other"), one_sample()), Compared::New);
        assert!(!Compared::New.regressed(0.0));
    }

    #[test]
    fn times_are_printed_in_the_largest_unit_that_keeps_them_at_least_one() {
        assert_eq!(format_ns(41.2), "41.20 ns");
        assert_eq!(format_ns(999.9), "999.90 ns");
        assert_eq!(format_ns(1240.0), "1.24 us");
        assert_eq!(format_ns(3_500_000.0), "3.50 ms");
        assert_eq!(format_ns(2_100_000_000.0), "2.10 s");
    }

    #[test]
    fn a_baseline_round_trips_through_its_file() {
        let dir = std::env::temp_dir().join(format!("burdin-baseline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let stats = Stats { median: 41.5, mean: 41.8, ci_low: 41.1, ci_high: 42.5, mad: 0.4, outliers: 1 };
        let case = BenchCase {
            module: "geometry".to_string(),
            local: "geometry.sum list".to_string(),
            file: "app/src/geometry.bd".to_string(),
            line: 3,
            ..case_of("sum list")
        };
        let samples: [u64; 2] = [41_500, 41_800];
        save_baseline(&dir, "nightly", [(&case, 1000u64, &samples[..], stats)].as_slice()).expect("the baseline saves");
        let loaded = load_baseline(&dir, "nightly").expect("the baseline loads back");
        let bench = loaded.benches.get("geometry.sum list").expect("the bench is keyed by its local path");
        assert!((bench.median - 41.5).abs() < 1e-9);
        assert!((bench.mad - 0.4).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&dir);

        // A name that was never saved says so, and a hand-broken file is refused.
        let missing = load_baseline(&dir, "nope").unwrap_err();
        assert!(missing.contains("no baseline named `nope`"), "{missing}");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("broken.json"), "{\"version\":2,\"benches\":{}}").unwrap();
        let wrong_version = load_baseline(&dir, "broken").unwrap_err();
        assert!(wrong_version.contains("version 2"), "{wrong_version}");
        std::fs::write(dir.join("broken.json"), "not json at all").unwrap();
        assert!(load_baseline(&dir, "broken").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_baseline_parser_reads_what_the_writer_emits_and_refuses_broken_files() {
        let value = parse_json("{\"a\":[1,2.5,\"x\"]}").expect("the document parses");
        assert_eq!(
            value,
            Json::Obj(
                [("a".to_string(), Json::Arr(vec![Json::Num(1), Json::NumF(2.5), Json::Str("x".to_string())]))].into()
            )
        );
        // Strings with the escapes the writer emits, and the whitespace a hand-edited file may have.
        let spaced = parse_json(" { \"k\": \"a\\\"b\\\\c\" } ").expect("the escapes parse");
        assert_eq!(spaced, Json::Obj([("k".to_string(), Json::Str("a\"b\\c".to_string()))].into()));
        for broken in ["", "{", "{\"a\"", "{\"a\":}", "{\"a\":1,}", "[1,", "\"open", "1 2"] {
            assert!(parse_json(broken).is_err(), "{broken:?} should not parse");
        }
    }

    #[test]
    fn the_comparison_wordings_are_stable() {
        assert_eq!(Compared::New.text(), "new (not in the baseline)");
        assert_eq!(Compared::WithinNoise.text(), "unchanged (within noise)");
        assert_eq!(Compared::BeyondNoise { percent: 12.34 }.text(), "12.3% slower (beyond noise)");
        assert_eq!(Compared::BeyondNoise { percent: -4.0 }.text(), "-4.0% faster (beyond noise)");
        assert_eq!(Compared::New.json(), "\"new\":true");
        assert_eq!(Compared::WithinNoise.json(), "\"new\":false,\"changed\":false");
        assert_eq!(
            Compared::BeyondNoise { percent: 12.34 }.json(),
            "\"new\":false,\"changed\":true,\"change_percent\":12.340"
        );
    }

    #[test]
    fn the_timeout_flag_reads_like_the_tests_one() {
        let plain = Args { command: "bench".to_string(), ..Args::default() };
        assert_eq!(default_timeout(&plain), Some(Duration::from_millis(DEFAULT_TIMEOUT_MS)));
        let off = Args { timeout_ms: Some(0), ..plain.clone() };
        assert_eq!(default_timeout(&off), None);
        let on = Args { timeout_ms: Some(250), ..plain };
        assert_eq!(default_timeout(&on), Some(Duration::from_millis(250)));
    }

    #[test]
    fn selection_keeps_what_some_filter_matches_and_no_skip_does() {
        let mut args = Args { command: "bench".to_string(), ..Args::default() };
        args.filters = vec!["geo".to_string()];
        args.skip = vec!["slow".to_string()];
        let cases = vec![case_of("geometry.area"), case_of("alpha"), case_of("slow geo")];
        let (chosen, filtered_out) = select(&args, cases);
        let kept: Vec<String> = chosen.into_iter().map(|case| case.path).collect();
        assert_eq!(kept, ["geometry.area"]);
        assert_eq!(filtered_out, 2);

        // `--exact` compares the whole path.
        args.exact = true;
        args.filters = vec!["geometry.area".to_string()];
        args.skip.clear();
        let cases = vec![case_of("geometry.area"), case_of("geometry.areas")];
        let (chosen, filtered_out) = select(&args, cases);
        let kept: Vec<String> = chosen.into_iter().map(|case| case.path).collect();
        assert_eq!(kept, ["geometry.area"]);
        assert_eq!(filtered_out, 1);
    }

    #[test]
    fn discovery_builds_the_full_path_of_each_bench() {
        let bench = |name: &str, module: &str| BenchEntry {
            name: name.to_string(),
            module: module.to_string(),
            span: burdin_diag::Span::new(burdin_diag::FileId(0), 0, 0),
            func: burdin_tir::FuncId(0),
        };
        let one = CompiledPackage {
            name: "app".to_string(),
            exe: PathBuf::from("app-bin"),
            benches: vec![bench("plain", ""), bench("deep", "geometry.shapes")],
            sites: vec![
                Site { file: "app/src/main.bd".to_string(), line: 3 },
                Site { file: "app/src/geometry/shapes.bd".to_string(), line: 7 },
            ],
        };
        let cases = discover(&[one]);
        let paths: Vec<&str> = cases.iter().map(|case| case.path.as_str()).collect();
        assert_eq!(paths, ["plain", "geometry.shapes.deep"]);
        assert_eq!((cases[1].file.as_str(), cases[1].line, cases[1].index), ("app/src/geometry/shapes.bd", 7, 1));

        // With more than one package both names carry the package, and the module stays in the path.
        let two = CompiledPackage {
            name: "lib".to_string(),
            exe: PathBuf::from("lib-bin"),
            benches: vec![bench("deep", "geometry")],
            sites: vec![Site { file: "lib/src/geometry.bd".to_string(), line: 2 }],
        };
        let plain = CompiledPackage { benches: vec![bench("plain", "")], ..names("app") };
        let cases = discover(&[plain, two]);
        let paths: Vec<String> = cases.iter().map(|case| format!("{}|{}", case.display, case.path)).collect();
        assert_eq!(paths, ["app::plain|app::plain", "lib::deep|lib::geometry.deep"]);
    }

    fn names(name: &str) -> CompiledPackage {
        CompiledPackage {
            name: name.to_string(),
            exe: PathBuf::from(format!("{name}-bin")),
            benches: Vec::new(),
            sites: Vec::new(),
        }
    }
}
