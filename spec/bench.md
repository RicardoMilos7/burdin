# The Burdin benchmark runner

This file is the behavior of `burdin bench`, of the `bench` items it measures, and of the `black_box` hint
(`spec/grammar.md`, `BenchItem`; `spec/stdlib.md`, "The optimizer barrier"). The driver contract is summarized in
[`README.md`](../README.md#burdin-bench); the black-box checks are in
[`tests/check_bench.py`](../tests/check_bench.py) with the conformance tests in [`tests/s3_bench`](../tests/s3_bench)
and the Rust unit tests in `crates/burdin_driver/src/bench_runner.rs`.

## Bench items

**BN1. The item.** `bench "name" { ... }` is a module-scope item, exactly where a `test` item is. The keyword is
contextual — `bench` is an ordinary identifier everywhere except at an item start followed by a string — and the
item takes no attributes in this stage. A bench item is kept out of the ordinary items: `burdin build`, `run`,
`test`, `check`, `lint`, and `doc` strip it from the program the way they strip `test` items, so a file with a
bench compiles as though the bench were not there. Only `burdin bench` declares and checks the entry package's
benches; a dependency's benches are never measured, and each package's bench bodies are compiled when that package
is the selected entry package.

A bench body is compiled as the private function `__burdin_bench_N`, which takes no parameters and returns
nothing; the parser appends a unit tail expression, so the body's last statement may be any value — the natural
place for `black_box(x)` to end a measurement — and the block still returns `()`. One call of that function is one
*iteration*: the harness times whole body calls, so anything the body does (allocating a `String`, building a
`List`) is inside the measurement, and setup that should not be measured belongs in a `const` or a `let` outside
the bench. A bench body may use the ordinary `assert` and panic like any code: a panic aborts the process and the
bench is reported as failed.

## The build

**BN2. Bench mode.** `burdin bench` compiles the selected packages the way `burdin test` compiles its packages
(`spec/test-runner.md`, "Execution"): each selected package is compiled as its own entry package, `main` is not
required, only that package's benches are declared, and the entry point is the bench dispatcher instead of the
user's `main`. `-p <name>` and `--workspace` select packages with the test runner's rules, including that the
embedded standard library cannot be selected.

**BN3. The release profile.** `burdin bench` compiles in the `bench` profile, which inherits `release`, because that
is the profile whose speed is worth measuring. `--profile <name>` measures another profile (`spec/profiles.md`),
`--release` is `--profile release`, and there is deliberately no way to bench a
debug build in this stage. A bench build uses the plain C heap, not the checking allocator: allocation bookkeeping
is not part of what a benchmark should measure (`spec/allocators.md`, AL9).

**BN4. The harness.** One process is started per bench, in a fresh temporary working directory with the test
runner's isolation (`spec/test-runner.md`, "Isolation"), with `BURDIN_BENCH` naming the running bench. The runner
starts it as `<bench binary> <index> measure <warmup-ms> <sample-count> <sample-ms>` and the process does the
measurement itself, against the monotonic clock:

1. **Warmup.** The body is called until `--warmup` milliseconds have passed, at least once, so the code is primed
   and the CPU has settled before anything is recorded.
2. **Calibration.** Growing batches of calls (`1, 4, 16, ...`, capped at 2^30) are timed until one batch takes at
   least a millisecond; the per-iteration time is that batch's total over its call count. A single call is not
   timed, because the clock's granularity (~25 ns) would dwarf a fast bench: one call of a bench measured alone
   always looks like one clock tick.
3. **Samples.** `--sample-count` samples are taken; each runs `--sample-time / per-iteration` iterations (at
   least 1) and records one total in nanoseconds. Every sample of one bench shares its iteration count.
4. **Report.** The process prints one line, `burdin-bench-result: <iters> <sample-1> <sample-2> ...`, through
   the buffered standard output, flushed when the process exits. The runner takes the last such line, so a bench
   that prints is tolerated, even though it should not.

A second invocation `<bench binary> <index> profile` calls the body in a loop with no clock at all (BN10). The
dispatcher's other exit statuses are for the runner's own mistakes: 2 for a missing argument, 3 for an unknown
index, 4 for an unknown mode.

**BN5. Statistics.** All statistics are over the per-iteration times (`sample total / iters`):

| Statistic | Definition |
| --- | --- |
| median | the middle per-iteration time, the average of the two middles when the count is even |
| mean | the arithmetic mean |
| 95% CI | `mean ± 1.96 · s / √n`, with `s` the sample standard deviation (`n - 1`); a single sample has no spread |
| MAD | the median absolute deviation, the median of the distances from the median |
| outlier | a sample more than `3 · MAD` from the median; when the MAD is zero, any difference counts |

The interval is the normal approximation of a confidence interval on the mean, which is honest for the tens of
samples a bench takes and cheap to verify by hand.

**BN6. The human report.** Benches are reported in declaration order as they are measured, one line per bench,
then one summary line. A measured bench prints

```
<display>: <median> (mean <mean>, ci 95% <lo> to <hi>, mad <mad>, <outliers>/<samples> outliers)
```

with every number in the largest unit that keeps it at least one (`41.20 ns`, `1.24 us`, `3.50 ms`, `2.10 s`) and
two decimals. With `--baseline`, a second line follows:

```
  baseline <name>: unchanged (within noise)
```

or `12.3% slower (beyond noise)`, `-4.0% faster (beyond noise)`, or `new (not in the baseline)`. A bench that
produced no measurements prints its captured standard output and standard error under
`---- bench "<display>" stdout ----` headings and then `FAIL <display>: <reason>`; a bench that regressed beyond
`--fail-on-regression` prints its statistics, its baseline line, and `FAIL <display>: regressed over the baseline
<name>`. `--verbose` prints the captured output of passing benches too; by default a passing bench prints only
its statistics. The summary is `bench result: <n> measured, <k> failed`, with `, <f> filtered out` appended when
the name rules left benches out. `measured` and `failed` are disjoint: a bench that regressed counts as failed.

**BN7. Machine-readable reports.** `--format json` replaces the human report with a stream of JSON objects, one
per line on standard output, written as the run progresses. Compilation diagnostics still go to standard error,
and a run that fails to compile prints no events. **Schema version 1**: a later version only adds fields; a
reader ignores fields it does not know. Every event about one bench carries its identity fields (`package`,
`module`, `name`, `path`, `file`, `line`), spelled like the test runner's:

| `event` | When | Fields besides the identity |
| --- | --- | --- |
| `suite_started` | once, first | `version`, `packages` (the selected package names), `bench_count`, `filtered_out` |
| `bench_started` | when a bench's process starts | |
| `bench_finished` | when it ends | `duration_ms`; with statistics `iters`, `samples` (the totals), `median_ns`, `mean_ns`, `ci_low_ns`, `ci_high_ns`, `mad_ns`, `outliers`, else `iters: null, samples: []`; `baseline` when comparing (`{"new", "changed"?, "change_percent"?}`); `reason` (`null`, the failure, or `"regressed over the baseline"`); `stdout` and `stderr` when the bench failed or `--verbose` was given |
| `suite_finished` | once, last | `measured`, `failed`, `filtered_out`, `duration_ms` |

`--list` prints the selected benches without measuring anything, in the human format (one full path per line and
`<n> benches, <k> filtered out`) or, with `--format json`, one identity object per line. The exit statuses of a
run are BN13's, and a listing exits 0.

**BN8. Filters and selection.** The name rules are the test runner's (`spec/test-runner.md`, "Name filters"):
positional filters and `--filter` match a bench's full path (`<package>::<module>.<bench name>`, qualified and
prefixed exactly as a test's is), `--exact` compares the whole path, `--skip` removes after the filters, and a
filter that matches nothing is not an error: the run measures nothing, exits 0, prints `bench result: 0 measured,
0 failed, <n> filtered out`, and `note: no bench matches the filters` goes to standard error. `-j` bounds the
compilation threads only.

**BN9. `black_box(value)`.** A compiler-known function of one argument: the value moves in and the same value
moves back out, through a barrier the optimizer cannot see through (a spill to a stack slot whose address crosses
an opaque inline-assembly call), so the computation that produced the value cannot be deleted, forwarded, or
lifted out of the measurement loop. It is available in every build, not only bench builds; at compile time it is
the identity; a view (`str`, `[T]`, `&T`, `&mut T`) is rejected with E0120, because the barrier hands back a fresh
value with no storage a view could point into. A bare `black_box` is not a name: like `format`, it is a call. It
takes no named arguments, and a wrong argument count is E0004.

**BN10. Profiler runs.** `--profile-time <seconds>` runs exactly one selected bench — a run that selects none or
several is a usage error — in a loop, with no clock anywhere in the loop, for the given seconds; the runner stops
the process when the time is up and prints `profiled <display> <seconds> s`. The point is that an external
profiler (perf, Instruments, VTune) can attach to a process that runs only the bench, unmeasured. It produces no
statistics, so it cannot be combined with `--list`, `--save-baseline`, or `--baseline`, and it always prints the
one human line, whatever `--format` says.

**BN11. Baselines.** `--save-baseline <name>` writes the statistics of every measured bench to
`<package>/target/burdin/bench/<name>.json`, a version 1 JSON object keyed by each bench's package-local path
(`<module>.<name>`), so a comparison does not depend on which packages a run selected. The file records the
iteration count, the sample totals, and the computed statistics. `--baseline <name>` loads that file and compares
each measured bench with its entry:

- A change is **beyond noise** when the medians differ by more than `3 · max(mad_baseline, mad_new)`. When both
  deviations are zero — every sample identical — any difference counts, which is what a stable machine should
  report.
- `--fail-on-regression <percent>` fails the run for each bench that is **slower** than its baseline by more than
  `percent` percent *and* beyond noise, so a noisy machine cannot fail a clean build and a real regression cannot
  hide inside huge noise. A faster bench, a within-noise change, and a bench missing from the file never fail a
  run; a missing bench is reported as `new`.
- A missing baseline file is an error (exit 2), as is a file of another version; saving again is the migration.
- Baselines need the target directory, so `--baseline` and `--save-baseline` on a single `.bd` file are usage
  errors.
- A run may compare with one baseline and save another; saving over the baseline a run just compared with stores
  every measurement, regressed or not.

**BN12. Timeouts.** `--timeout <ms>` (default 600000, `0` disables it) bounds one bench's whole process: warmup,
calibration, and samples. A bench past its deadline is killed and reported as failed with `timed out after <ms>
ms`. A bench that panics is failed with its captured output, like a test that aborts.

**BN13. Exit statuses.** 0 when every measured bench succeeded and none regressed past `--fail-on-regression`; 1
when a bench failed or compilation failed; 2 for a usage error (an unknown package, a bad number, conflicting
flags, `--profile-time` without exactly one bench, a baseline on a single-file target) or a driver failure after
loading.

**BN14. One bench at a time.** Benches are measured sequentially, never in parallel, however large `-j` is: two
benches running at once would put each other's load into the numbers. `-j` bounds the threads that compile the
bench binaries and nothing else.

## Tests

| Behavior | Test |
| --- | --- |
| `bench` items parse, stay contextual, and survive recovery | unit tests in `crates/burdin_syntax/src/parser/mod.rs` |
| A normal build strips bench items | `tests/s3_bench/stripped_from_build.bd` |
| A body error is reported by `burdin bench` only | `tests/s3_bench/body_checks.bd` (`//@ bench-compile-fail`) |
| Benches are measured, and a summary is printed | `tests/s3_bench/measures_two.bd`, `tests/s3_bench/fails.bd` (`//@ burdin-bench`), `tests/check_bench.py` |
| `black_box` is the identity, on any first-class type | `tests/s3_bench/black_box_is_identity.bd` |
| `black_box` rejects views (E0120) | `tests/s3_bench/black_box_view_rejected.bd` |
| A bench body's syntax mistake is a syntax error (E0001) | `tests/s3_bench/bench_attrs.bd` |
| Statistics, the result line, and comparisons | unit tests in `crates/burdin_driver/src/bench_runner.rs` |
| The harness protocol: warmup, calibration, samples | `tests/check_bench.py` (a bench's samples take about `--sample-time`) |
| The human report, `--list`, `--format json` | `tests/check_bench.py` |
| Filters, `--exact`, `--skip`, the filtered-out count | `tests/check_bench.py` |
| `-p`, `--workspace`, dependency benches | `tests/check_bench.py` |
| Failure: a panicking bench, a timeout, a compile error | `tests/check_bench.py` |
| Baselines: save, compare, regression, missing, versions | `tests/check_bench.py`, unit tests in `bench_runner.rs` |
| `--profile-time` | `tests/check_bench.py` |
| `burdin fmt` lays out bench items | `tests/fmt/exprs` (golden), `tests/check_fmt.py` |
