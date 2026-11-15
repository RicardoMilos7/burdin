//! `burdin-fuzz-syntax`: a longer lexer/parser fuzzing run (issues #288 and #447).
//!
//! The fast regression check is `cargo test -p burdin_fuzz`, which replays the committed corpus and a
//! short mutation pass in every CI job. This binary runs a longer session locally and in the scheduled
//! `fuzz` job. Every panic is written to `--save` as `crash-<hash>.bd`; commit that file under
//! `crates/burdin_fuzz/corpus/syntax/` together with its fix, and the replay test keeps the panic from
//! coming back.
//!
//! A watchdog fails the run when the parser stops making progress (an input that never returns), and
//! saves the input that stalled. The exit status is 1 on a finding and 2 on a usage or I/O error.

use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use burdin_fuzz::{Failure, Seed, Session, check, load_seeds, save_failure};

const DEFAULT_ITERATIONS: u64 = 100_000;
const DEFAULT_WATCHDOG: u64 = 10;

fn main() {
    process::exit(run());
}

fn run() -> i32 {
    let args = match parse_args(std::env::args().skip(1).collect()) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("fuzz-syntax: {message}");
            usage();
            return 2;
        }
    };
    if args.help {
        usage();
        return 0;
    }
    let seeds = match load_seeds(&args.seeds) {
        Ok(seeds) if seeds.is_empty() => {
            eprintln!("fuzz-syntax: no .bd files under {}", display_paths(&args.seeds));
            return 2;
        }
        Ok(seeds) => seeds,
        Err(message) => {
            eprintln!("fuzz-syntax: {message}");
            return 2;
        }
    };
    match args.seconds {
        Some(seconds) => {
            println!("fuzz-syntax: {} seeds, fuzzing for {} seconds, seed value {}", seeds.len(), seconds, args.seed)
        }
        None => println!("fuzz-syntax: {} seeds, {} mutations, seed value {}", seeds.len(), args.iterations, args.seed),
    }

    // The session runs on its own thread so the main thread can notice a parser that no longer returns.
    // A caught panic is normal and reported; a panic in the fuzzer itself is reported too.
    let progress = Arc::new(Progress::default());
    let deadline = args.seconds.map(|seconds| Instant::now() + Duration::from_secs(seconds));
    // With a deadline the mutation count is unbounded; the deadline decides when the session stops.
    let iterations = if deadline.is_some() { u64::MAX } else { args.iterations };
    let worker = {
        let progress = Arc::clone(&progress);
        let save = args.save.clone();
        let seed = args.seed;
        thread::spawn(move || run_session(seeds, seed, iterations, deadline, &save, &progress))
    };

    let start = Instant::now();
    let mut last_done = 0;
    let mut idle = Instant::now();
    let mut next_report = Instant::now() + Duration::from_secs(10);
    loop {
        if worker.is_finished() {
            let done = progress.done.load(Ordering::Relaxed);
            let elapsed = start.elapsed().as_secs_f64();
            return match worker.join() {
                Ok(Ok(0)) => {
                    println!("fuzz-syntax: {done} inputs in {elapsed:.1}s, no failures");
                    0
                }
                Ok(Ok(failures)) => {
                    println!("fuzz-syntax: {done} inputs in {elapsed:.1}s, {failures} failure(s)");
                    1
                }
                Ok(Err(message)) => {
                    eprintln!("fuzz-syntax: the fuzzer itself panicked: {message}");
                    1
                }
                Err(_) => {
                    eprintln!("fuzz-syntax: the fuzzer thread panicked");
                    1
                }
            };
        }
        thread::sleep(Duration::from_millis(100));
        let done = progress.done.load(Ordering::Relaxed);
        if done != last_done {
            last_done = done;
            idle = Instant::now();
        } else if args.watchdog > 0 && idle.elapsed() >= Duration::from_secs(args.watchdog) {
            // The worker is still parsing one input after the budget: save it and stop. A thread cannot
            // be interrupted, so the process exits and leaves the worker behind.
            let text = progress.current.lock().map(|text| text.clone()).unwrap_or_default();
            let label = progress.label.lock().map(|label| label.clone()).unwrap_or_default();
            let failure =
                Failure { input: text, message: format!("no progress for {} seconds ({label})", args.watchdog) };
            eprintln!("fuzz-syntax: HANG: {label}");
            eprintln!("  input: {}", failure.escaped());
            match save_failure(&args.save, &failure) {
                Ok(path) => eprintln!("  saved: {}", path.display()),
                Err(message) => eprintln!("  cannot save the input: {message}"),
            }
            println!("fuzz-syntax: {done} inputs in {:.1}s, stopped on a hang", start.elapsed().as_secs_f64());
            return 1;
        }
        if Instant::now() >= next_report && done > 0 {
            println!("fuzz-syntax: {done} inputs in {:.1}s", start.elapsed().as_secs_f64());
            next_report = Instant::now() + Duration::from_secs(10);
        }
    }
}

#[derive(Default)]
struct Progress {
    /// Inputs finished; the main thread watches this for progress.
    done: AtomicU64,
    /// The input being parsed, saved when the run stalls.
    current: Mutex<String>,
    label: Mutex<String>,
}

/// Runs the session, reporting every failure and writing it under `save`. Returns the number of failures,
/// or a message when the fuzzer itself panicked.
fn run_session(
    seeds: Vec<Seed>,
    seed: u64,
    iterations: u64,
    deadline: Option<Instant>,
    save: &Path,
    progress: &Progress,
) -> Result<u64, String> {
    let mut session = Session::new(seeds, seed, iterations).with_deadline(deadline);
    let mut failures = 0u64;
    while let Some((text, label)) = session.next_input() {
        if let Ok(mut current) = progress.current.lock() {
            *current = text.clone();
        }
        if let Ok(mut current) = progress.label.lock() {
            *current = label.clone();
        }
        if let Err(failure) = check(&text) {
            failures += 1;
            eprintln!("fuzz-syntax: FAIL {label}: {}", failure.message);
            eprintln!("  input: {}", failure.escaped());
            match save_failure(save, &failure) {
                Ok(path) => eprintln!("  saved: {} (commit it as a corpus seed with the fix)", path.display()),
                Err(message) => eprintln!("  cannot save the input: {message}"),
            }
        }
        progress.done.fetch_add(1, Ordering::Relaxed);
    }
    Ok(failures)
}

struct Args {
    seeds: Vec<PathBuf>,
    iterations: u64,
    seconds: Option<u64>,
    seed: u64,
    save: PathBuf,
    watchdog: u64,
    help: bool,
}

fn parse_args(raw: Vec<String>) -> Result<Args, String> {
    let mut args = Args {
        seeds: vec![PathBuf::from(burdin_fuzz::CORPUS)],
        iterations: DEFAULT_ITERATIONS,
        seconds: None,
        seed: 1,
        save: PathBuf::from("fuzz-out/crashes"),
        watchdog: DEFAULT_WATCHDOG,
        help: false,
    };
    let mut index = 0;
    while index < raw.len() {
        let flag = raw[index].as_str();
        let value = |index: &mut usize| -> Result<&str, String> {
            *index += 1;
            raw.get(*index).map(String::as_str).ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag {
            "-h" | "--help" => args.help = true,
            "--corpus" | "--seed-dir" => {
                let path = value(&mut index)?;
                args.seeds.push(PathBuf::from(path));
            }
            "--iterations" => args.iterations = number(value(&mut index)?, flag)?,
            "--seconds" => args.seconds = Some(number(value(&mut index)?, flag)?),
            "--seed" => args.seed = number(value(&mut index)?, flag)?,
            "--save" => args.save = PathBuf::from(value(&mut index)?),
            "--watchdog" => args.watchdog = number(value(&mut index)?, flag)?,
            other => return Err(format!("unknown option {other}")),
        }
        index += 1;
    }
    Ok(args)
}

fn number(value: &str, flag: &str) -> Result<u64, String> {
    value.parse().map_err(|_| format!("{flag} needs a whole number, got `{value}`"))
}

fn display_paths(paths: &[PathBuf]) -> String {
    paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")
}

fn usage() {
    println!(
        "\
usage: cargo run -p burdin_fuzz --bin burdin-fuzz-syntax -- [options]

  --corpus PATH       a corpus directory or file; repeatable. The committed corpus is always included.
  --seed-dir PATH     another seed directory or file; repeatable, same as --corpus
  --iterations N      mutations after the corpus replay (default: 100000)
  --seconds N         run for about N seconds instead of a fixed number of mutations
  --seed N            seed for the deterministic generator (default: 1; print it to reproduce a finding)
  --save DIR          where a failing input is written (default: fuzz-out/crashes)
  --watchdog N        fail when no input finishes for N seconds; 0 disables (default: 10)
  -h, --help          show this help

Exit status: 0 when nothing panicked, 1 on a finding or a stall, 2 on a usage or I/O error."
    );
}
