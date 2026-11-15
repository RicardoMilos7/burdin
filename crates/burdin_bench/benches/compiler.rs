//! Compiler benchmarks: `cargo bench -p burdin_bench`.
//!
//! Each generated input is measured through the four stages of the compiler: the lexer, the parser, name
//! resolution and type checking, and code generation. A case reports the median time per iteration and the
//! number of iterations the harness chose to fill its time target, so a slow run is not confused with a fast
//! machine. The parser case consumes a clone of the pre-lexed token stream, because the parser takes its tokens
//! by value; that copy is small next to parsing and does not hide a parser regression.
//!
//! `cargo bench -p burdin_bench -- --smoke` runs one small generated input through every stage, checks that it
//! compiles, and fails when a stage takes longer than a deliberately generous budget. That is the CI guard
//! against catastrophic regressions (a hang or an accidental quadratic pass).
//!
//! A run can also be recorded and compared with a stored baseline: `--save FILE` writes the medians of this
//! run, and `--baseline FILE` compares this run with them and exits 1 when a case is more than `--threshold`
//! percent slower (default 25). CI runs both through `tests/bench_regression.py`, with the baseline in a cache
//! keyed by the runner, the pinned toolchain, and `Cargo.lock` (issue #446).
//!
//! `cargo bench -p burdin_bench -- <filter>` measures only the cases whose name contains `filter`
//! (`lex`, `sema`, `large`, ...).

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use burdin_bench::{
    Comparison, Generated, Measurement, check, check_memory, compare, emit, parse_baseline, parse_generated,
    render_baseline,
};
use burdin_diag::Diagnostics;
use burdin_syntax::lexer;
use burdin_syntax::lexer::LexResult;
use burdin_syntax::parser;

/// `(name, generated chunks)`. The chunk is the unit of the generator, not a line or a token, so these sizes
/// describe growth rather than an exact program size (`small` is about 600 lines, `large` about 9,600).
const SIZES: &[(&str, usize)] = &[("small", 8), ("medium", 32), ("large", 128)];

/// How long the harness keeps iterating a case before it reports the median.
const TARGET: Duration = Duration::from_millis(250);

/// How much slower a case may get before `--baseline` reports a regression, in percent. Hosted runners vary
/// enough that a tighter threshold is noise; it is a flag so a stable machine can tighten it.
const DEFAULT_THRESHOLD: u32 = 25;

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&raw) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("bench: {message}");
            print_usage();
            std::process::exit(2);
        }
    };
    if args.smoke {
        std::process::exit(smoke());
    }
    std::process::exit(run(&args));
}

fn print_usage() {
    println!(
        "\
usage: cargo bench -p burdin_bench [-- FILTER]
       cargo bench -p burdin_bench -- --smoke
       cargo bench -p burdin_bench -- [--save FILE] [--baseline FILE [--threshold PERCENT]] [FILTER]

  FILTER      measure only cases whose name contains FILTER
  --smoke     run one small generated input through every stage with generous time budgets (the CI guard)
  --save      write this run's medians to FILE in the baseline format (case<TAB>nanoseconds)
  --baseline  compare this run with FILE and exit 1 when a case is slower by more than the threshold
  --threshold how much slower a case may be before --baseline reports it, in percent (default 25)"
    );
}

struct Args {
    smoke: bool,
    save: Option<PathBuf>,
    baseline: Option<PathBuf>,
    threshold: u32,
    filter: Option<String>,
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut args = Args { smoke: false, save: None, baseline: None, threshold: DEFAULT_THRESHOLD, filter: None };
    let mut threshold_given = false;
    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            // cargo appends `--bench` when it runs a benchmark target; it is not an option of this harness.
            "--bench" => {}
            "--smoke" => args.smoke = true,
            "--save" => args.save = Some(PathBuf::from(value(&mut iter, "--save")?)),
            "--baseline" => args.baseline = Some(PathBuf::from(value(&mut iter, "--baseline")?)),
            "--threshold" => {
                threshold_given = true;
                let text = value(&mut iter, "--threshold")?;
                args.threshold =
                    text.parse().map_err(|_| format!("--threshold: '{text}' is not a whole percentage"))?;
            }
            other if other.starts_with('-') => return Err(format!("unknown option: {other}")),
            other => {
                if let Some(filter) = &args.filter {
                    return Err(format!("two filters: '{filter}' and '{other}'"));
                }
                args.filter = Some(other.to_string());
            }
        }
    }
    if args.smoke && (args.save.is_some() || args.baseline.is_some() || args.filter.is_some() || threshold_given) {
        return Err("--smoke does not take --save, --baseline, --threshold, or a filter".to_string());
    }
    Ok(args)
}

fn value<'a>(iter: &mut impl Iterator<Item = &'a String>, option: &str) -> Result<&'a str, String> {
    iter.next().map(String::as_str).ok_or_else(|| format!("{option} needs a value"))
}

fn run(args: &Args) -> i32 {
    let measurements = measure(args.filter.as_deref());
    if let Some(path) = &args.save
        && let Err(error) = std::fs::write(path, render_baseline(&measurements))
    {
        eprintln!("bench: cannot write {}: {error}", path.display());
        return 2;
    }
    if let Some(path) = &args.baseline {
        return compare_with_baseline(path, &measurements, args.threshold);
    }
    0
}

fn measure(filter: Option<&str>) -> Vec<Measurement> {
    println!("burdin compiler benchmarks (median per iteration)");
    println!("parse includes a clone of the pre-lexed token stream; code generation is the debug profile");
    println!();
    let mut measurements = Vec::new();
    for &(size, units) in SIZES {
        let generated = parse_generated(units);
        assert_no_errors(&generated);

        if enabled(filter, "lex", size) {
            let stats = run_iterations(
                || {
                    let mut diags = Diagnostics::default();
                    let lexed = lexer::lex(generated.file, &generated.text, &mut diags);
                    black_box(lexed.tokens.len());
                },
                3,
                500,
            );
            report("lex", size, stats);
            measurements.push(measurement("lex", size, stats.median));
        }

        if enabled(filter, "parse", size) {
            let tokens = &generated.tokens;
            let truncated = generated.truncated;
            let file = generated.file;
            let stats = run_iterations(
                || {
                    let mut diags = Diagnostics::default();
                    let ast = parser::parse_file(file, LexResult { tokens: tokens.clone(), truncated }, &mut diags);
                    black_box(ast.items.len());
                },
                3,
                300,
            );
            report("parse", size, stats);
            measurements.push(measurement("parse", size, stats.median));
        }

        if enabled(filter, "sema", size) {
            let stats = run_iterations(
                || {
                    let (program, diags) = check(&generated);
                    assert!(
                        !diags.has_errors(),
                        "sema reported errors in the benchmark input:\n{}",
                        diags.render(&generated.sources)
                    );
                    black_box(program.functions.len());
                },
                2,
                100,
            );
            report("sema", size, stats);
            measurements.push(measurement("sema", size, stats.median));
        }

        if enabled(filter, "codegen", size) {
            let (mut program, diags) = check(&generated);
            assert!(
                !diags.has_errors(),
                "sema reported errors in the benchmark input:\n{}",
                diags.render(&generated.sources)
            );
            let memory = check_memory(&mut program);
            assert!(!memory.has_errors(), "the memory checks reported errors in the benchmark input");
            let object = temp_object_path();
            let stats = run_iterations(
                || {
                    emit(&program, &generated.sources, &object).expect("code generation failed");
                },
                3,
                8,
            );
            let _ = std::fs::remove_file(&object);
            report("codegen", size, stats);
            measurements.push(measurement("codegen", size, stats.median));
        }
    }
    measurements
}

fn measurement(stage: &str, size: &str, median: Duration) -> Measurement {
    Measurement { case: format!("{stage}/{size}"), median }
}

/// Reads a stored baseline, compares this run with it, and returns 1 when a case is over the threshold so a
/// caller (a developer, or `tests/bench_regression.py`) can decide whether that fails the run.
fn compare_with_baseline(path: &Path, current: &[Measurement], threshold: u32) -> i32 {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("bench: cannot read baseline {}: {error}", path.display());
            return 2;
        }
    };
    let baseline = match parse_baseline(&text) {
        Ok(baseline) => baseline,
        Err(message) => {
            eprintln!("bench: {} is not a baseline: {message}", path.display());
            return 2;
        }
    };
    let comparison = compare(&baseline, current, threshold);
    print_comparison(path, &comparison, threshold);
    if comparison.regression_count() > 0 { 1 } else { 0 }
}

fn print_comparison(path: &Path, comparison: &Comparison, threshold: u32) {
    println!();
    println!(
        "baseline: {} ({} cases, threshold {threshold}%)",
        path.display(),
        comparison.changes.len() + comparison.missing.len()
    );
    println!("{:<18} {:>12} {:>12} {:>9}  verdict", "case", "baseline", "current", "change");
    for change in &comparison.changes {
        let percent = change.ratio_percent - 100.0;
        let verdict = if change.regressed {
            "REGRESSION"
        } else if percent <= -(f64::from(threshold)) {
            "faster"
        } else {
            "ok"
        };
        println!(
            "{:<18} {:>12} {:>12} {:>+8.1}%  {verdict}",
            change.case,
            format_duration(change.baseline),
            format_duration(change.current),
            percent
        );
    }
    if !comparison.missing.is_empty() {
        println!("not measured: {}", comparison.missing.join(", "));
    }
    if !comparison.new.is_empty() {
        println!("new (no baseline entry): {}", comparison.new.join(", "));
    }
    let regressions = comparison.regression_count();
    if regressions == 0 {
        println!(
            "bench check: every compared case is within {threshold}% of the baseline ({} compared)",
            comparison.changes.len()
        );
    } else {
        println!("bench check: {regressions} case(s) over the {threshold}% threshold:");
        for change in comparison.changes.iter().filter(|change| change.regressed) {
            println!(
                "  {} {} -> {} ({:+.1}%)",
                change.case,
                format_duration(change.baseline),
                format_duration(change.current),
                change.ratio_percent - 100.0
            );
        }
    }
}

/// Lexes, parses, checks, and emits one small program and enforces a time budget per stage. The budgets are
/// wide on purpose: they catch a hang or a change from linear to quadratic, not a ten-percent drift.
fn smoke() -> i32 {
    const SMOKE_UNITS: usize = 16;
    let generated = parse_generated(SMOKE_UNITS);
    let mut failures = 0;

    let lex_time = time(|| {
        let mut diags = Diagnostics::default();
        let lexed = lexer::lex(generated.file, &generated.text, &mut diags);
        assert!(!diags.has_errors(), "lexing the benchmark input reported errors");
        black_box(lexed.tokens.len());
    });

    let parse_time = time(|| {
        let mut diags = Diagnostics::default();
        let ast = parser::parse_file(
            generated.file,
            LexResult { tokens: generated.tokens.clone(), truncated: generated.truncated },
            &mut diags,
        );
        assert!(!diags.has_errors(), "parsing the benchmark input reported errors");
        black_box(ast.items.len());
    });

    let (mut program, sema_diags) = check(&generated);
    assert!(
        !sema_diags.has_errors(),
        "sema reported errors in the benchmark input:\n{}",
        sema_diags.render(&generated.sources)
    );
    let sema_time = time(|| {
        let (program, diags) = check(&generated);
        assert!(
            !diags.has_errors(),
            "sema reported errors in the benchmark input:\n{}",
            diags.render(&generated.sources)
        );
        black_box(program.functions.len());
    });
    let memory = check_memory(&mut program);
    assert!(!memory.has_errors(), "the memory checks reported errors in the benchmark input");

    let object = temp_object_path();
    let codegen_time = time(|| {
        emit(&program, &generated.sources, &object).expect("code generation failed");
    });
    let _ = std::fs::remove_file(&object);

    println!("bench smoke: {SMOKE_UNITS} generated chunks");
    // The measured times on the reference machine are ~0.3 ms, 2 ms, 1 ms, and 45 ms. These budgets are
    // hundreds of times those, so a slow or busy runner passes and only a hang or an accidental quadratic
    // pass fails (the `--baseline` comparison is the tighter check; issue #446).
    for (stage, elapsed, budget) in [
        ("lex", lex_time, Duration::from_secs(1)),
        ("parse", parse_time, Duration::from_secs(1)),
        ("sema", sema_time, Duration::from_secs(2)),
        ("codegen", codegen_time, Duration::from_secs(10)),
    ] {
        let verdict = if elapsed <= budget { "ok" } else { "OVER BUDGET" };
        println!("  {stage:<8} {:>10} (budget {:>6}) {verdict}", format_duration(elapsed), format_duration(budget));
        if elapsed > budget {
            failures += 1;
        }
    }
    if failures == 0 {
        println!("bench smoke: every stage finished within its budget");
        0
    } else {
        eprintln!("bench smoke: {failures} stage(s) exceeded their budget");
        1
    }
}

/// The benchmark input is generated, so a diagnostic means the generator is broken (the conformance suite is
/// what tests the compiler's diagnostics).
fn assert_no_errors(generated: &Generated) {
    let (_, diags) = check(generated);
    assert!(!diags.has_errors(), "sema reported errors in the benchmark input:\n{}", diags.render(&generated.sources));
}

fn enabled(filter: Option<&str>, stage: &str, size: &str) -> bool {
    match filter {
        Some(filter) => format!("{stage}/{size}").contains(filter),
        None => true,
    }
}

/// Runs a warm-up, then chooses an iteration count that fills [`TARGET`], and reports the median sample.
fn run_iterations(mut run_once: impl FnMut(), min_iters: u32, max_iters: u32) -> Stats {
    // A warm-up keeps allocator and LLVM cold-start costs out of the iteration count.
    run_once();
    let start = Instant::now();
    run_once();
    let first = start.elapsed();
    let estimate = (TARGET.as_nanos() / first.as_nanos().max(1)).min(u32::MAX as u128) as u32;
    let iters = estimate.clamp(min_iters, max_iters);
    let mut samples = Vec::with_capacity(iters as usize);
    for _ in 0..iters {
        let start = Instant::now();
        run_once();
        samples.push(start.elapsed());
    }
    samples.sort_unstable();
    Stats { median: samples[samples.len() / 2], iters }
}

#[derive(Clone, Copy)]
struct Stats {
    median: Duration,
    iters: u32,
}

fn time(run_once: impl FnOnce()) -> Duration {
    let start = Instant::now();
    run_once();
    start.elapsed()
}

fn report(stage: &str, size: &str, stats: Stats) {
    let name = format!("{stage}/{size}");
    println!("{name:<18} {:>12}/iter  (n={})", format_duration(stats.median), stats.iters);
}

fn format_duration(elapsed: Duration) -> String {
    let ns = elapsed.as_nanos() as f64;
    if ns < 1_000.0 {
        format!("{ns:.0} ns")
    } else if ns < 1_000_000.0 {
        format!("{:.3} us", ns / 1_000.0)
    } else if ns < 1_000_000_000.0 {
        format!("{:.3} ms", ns / 1_000_000.0)
    } else {
        format!("{:.3} s", ns / 1_000_000_000.0)
    }
}

fn temp_object_path() -> PathBuf {
    std::env::temp_dir().join(format!("burdin-bench-{}.o", std::process::id()))
}
