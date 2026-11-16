# The Burdin test runner

This file is the behavior of `burdin test` and of the attributes on a `test` item. The syntax is in
[`grammar.md`](grammar.md) (`TestItem`); the driver contract is summarized in
[`README.md`](../README.md#burdin-test), and the black-box checks are in
[`tests/check_test_runner.py`](../tests/check_test_runner.py) with the conformance tests in
[`tests/m10_test_runner`](../tests/m10_test_runner).

The runner builds the selected packages in test mode. A test body is compiled as a function that returns
`Result<()>`, the dynamic error type, so `?` accepts both an assertion failure and any other error type; `assert`
and `assert_eq` return a typed error in test mode, and a helper can propagate it with `?`. A failing assertion's
message (the condition text, the values, and the location) is printed to standard
error before the exit status, so it appears in the captured stderr section of the failure report. A test that
returns `Err` is reported as failed; the generated dispatcher exits 1 for `Err` and 0 for `Ok`.

## Test items and attributes

```
test "name" { ... }
test "name" [ignore] { ... }
test "name" [timeout(500)] { ... }
test "name" [should_panic] { ... }
test "name" [should_panic("message")] { ... }
test "name" [should_fail] { ... }
test "name" [differential] { ... }
test "name" [ignore, timeout(500), should_panic("message")] { ... }
```

Attributes come after the name in one `[...]` list, separated by commas; a trailing comma is allowed. They are
compiled with the test, so an attribute mistake is reported by `burdin test`, not by `burdin build` (tests are
stripped from a normal build). The accepted attributes are:

| Attribute | Meaning |
| --- | --- |
| `ignore` | The test is compiled and checked but not run by default. It counts as *ignored* in the summary. `--ignored` runs only ignored tests; `--include-ignored` runs every test, ignored or not. |
| `timeout(ms)` | The test process must finish within `ms` milliseconds. `0` means no limit. The attribute overrides the runner's `--timeout` default for this test. A test past its deadline is killed and reported as failed. |
| `should_panic` | The test must end in a panic abort. |
| `should_panic("text")` | The same, and `text` must appear in the test's stderr. |
| `should_fail` | The test must return an error (`Err`), which the dispatcher reports with exit status 1. A panic does not satisfy it. The failure is a typed `AssertError` (`spec/errors.md`, ERR9), and the test binary prints its message (the condition text, the values, and the location) to standard error before exiting, so it appears under the captured stderr heading. `should_fail` itself checks only the exit status, not the error value. |
| `differential` | The body is also executed by the comptime interpreter while the package is compiled, and the compiled test must produce the same `Ok`/`Err` outcome. See [Differential tests](#differential-tests). |

A test with none of these passes exactly when its process exits 0. A panic is any ending other than exit 0 or 1: a
signal, an abort, or another exit status. `should_panic` and `should_fail` are opposite expectations and cannot be
combined.

An unknown attribute name, a repeated attribute, a value an attribute does not take (`ignore("x")`,
`should_panic(1)`), a missing value (`timeout`), `should_panic` together with `should_fail`, or `differential`
together with `should_panic` or `should_fail` is **E0170**. The attribute is reported at its name; the test is still
declared, so one bad attribute does not cascade into follow-on diagnostics.

## Execution

- Each test runs in its own process, so a panic or abort fails only that test.
- A `burdin test` build allocates through the checking allocator: a test that returns success but leaves
  allocations live is reported with `test leak: <n> allocation(s) leaked, <bytes> byte(s)` and one
  `allocated at <path>:<line>:<column>` line per block, and fails. A test that already failed keeps its own
  verdict. `spec/allocators.md` AL9.
- One test binary is built per selected package. It contains that package's tests in declaration order; the index on
  the command line selects one. Only the selected package's tests are declared and checked, so a dependency's tests
  never leak into the entry package's build.
- Independent tests run concurrently. `-j N`/`--jobs N` sets the number of test processes at once; the default is
  the number of available CPUs. `-j 1` is sequential. Compilation itself is sequential.
- The report is deterministic: failures and the summary are printed after every test has finished, in package order
  and then declaration order, regardless of completion order.
- A test's stdout and stderr are captured. By default they are printed only when the test fails, under
  `---- test "name" stdout ----` and `---- test "name" stderr ----` headings. The heading is followed by
  `---- test "name": <reason> ----` when the failure needs an explanation (a timeout, or an unmet
  `should_panic`/`should_fail`). `--verbose` prints the same sections for passing tests and a `PASS <name>` line.
- A test that passes prints nothing in the default mode.

## Isolation

Every test process starts with:

- a fresh, empty temporary directory as its working directory, removed when the test finishes, whether it passed or
  failed. Two tests never share a working directory, so a file one test writes is not visible to another.
- an empty environment, except for these variables:
  - `PATH`, copied from the runner, so `process.run` can find commands;
  - `TMPDIR`, `TMP`, and `TEMP`, set to the test's working directory, so a program that writes temporary files
    cleans them up with the directory;
  - `BURDIN_TEST`, the test's display name;
  - on Windows, the variables the C runtime needs to start: `SystemRoot`, `SystemDrive`, `windir`, `ComSpec`,
    `PATHEXT`, `NUMBER_OF_PROCESSORS`, `PROCESSOR_ARCHITECTURE`, and `OS`, when the parent has them.

A test may rely on its working directory being empty and private and on the environment above. It must not rely on
any other variable, on the working directory's path, or on `env.args()`: the runner reserves the arguments (the
program name and the test index are how the dispatcher finds the test).

The runner removes the test binary directory and every test working directory when `burdin test` exits, including
after a failure.

## Discovery and selection

The target is a `.bd` file or a package directory.

- By default only the entry package's tests run. Dependency tests do not run, because a dependency may be shared and
  its tests may not hold for this build.
- `-p <name>` selects a package of the build; it may repeat and duplicates are removed. The packages are the entry
  package and every package reachable through `[dependencies]` path entries. The embedded standard library has no
  directory and cannot be selected.
- `--workspace` selects the entry package and every path dependency. In this stage a *workspace* is the build's
  package graph; a manifest-level workspace with independent members is stage 3 (`stage1-subset.md`), so
  `--workspace` is exactly those packages. `-p` and `--workspace` cannot be combined.
- `--filter <text>` keeps the tests whose displayed name contains `text`, matched case-sensitively. A filter that
  matches nothing is not an error: the run reports `0 passed, 0 failed`. `--ignored` and `--include-ignored`
  apply after the filter.
- When more than one package is selected, a test's displayed name is `<package>::<test name>` so two packages may
  use the same test name. A single selected package keeps the plain test name. `FAIL`, `PASS`, `IGNORED`, and the
  captured-output headings all use the displayed name.
- A test in a package with no `src/main.bd` still runs: test mode does not require `main`.

## Output and exit status

The report goes to stdout; compilation diagnostics go to stderr.

```
---- test "fails loudly" stdout ----
visible stdout
---- test "fails loudly" stderr ----
visible stderr
FAIL fails loudly
test result: 1 passed, 2 failed
```

`IGNORED <name>` appears only with `--verbose`. The summary is always
`test result: <passed> passed, <failed> failed`, with `, <ignored> ignored` appended when at least one test was
skipped because of `[ignore]`. Timeouts count as failures.

The exit status is 0 when every run test passed (and no test failed), 1 when a test failed or compilation failed,
and 2 for a usage error (an unknown package, a bad `-j`, conflicting flags) or a driver failure after loading.

## Differential tests

A test marked `[differential]` runs the same body twice: once through the comptime interpreter while the package is
compiled (`spec/comptime.md`), and once as the compiled test under the runner. The test passes when the two runs
produce the same outcome, so an interpreter/compiled divergence is caught automatically (issue #444).

- The outcome is the body's `Ok` or `Err`: the interpreter's return value and the compiled dispatcher's exit status
  0 or 1. A body that returns `Err` in both runs passes, just as `should_fail` would treat it; agreement is the
  expectation, not success. This stage compares only `Ok`/`Err`; the dynamic error type carries no comparable value.
- A compiled run that ends any other way (a panic, an abort, a signal, another exit status) while the interpreter
  returned `Ok` or `Err` is a divergence, and the report gives the two outcomes.
- The compile-time run happens after every `comptime` construct in the body has been evaluated and rewritten
  (`spec/comptime.md`, CT13), under the same purity (CT9), panic (CT10), budget (CT11), and profile rules (a
  `--release` test wraps integer overflow, CT1) as any other compile-time evaluation. A body the interpreter cannot
  execute fails compilation: an impure operation is E0224, a panic is E0222, a failed `comptime assert` is E0223,
  and an exhausted budget is E0225, each reported at the test item. Consequently a `[differential]` body cannot
  print, read files or the environment, run processes, call C, or panic at compile time.
- A body must be deterministic and may not depend on behavior the language leaves unspecified. In particular,
  `Map` iteration is insertion order at compile time and hash order in compiled code (`spec/comptime.md`, CT9), so a
  differential test that observes it can report a divergence; the language does not promise otherwise.
- `differential` states no expectation of its own, so it cannot be combined with `should_panic` or `should_fail`
  (E0170). `ignore` and `timeout` still apply. An ignored differential test is compiled and checked like any other,
  so its body is still executed at compile time; only the compiled run is skipped.
- One test binary is still built per package and each test still runs in its own process; the differential
  comparison uses the `Ok`/`Err` exit status the dispatcher already produces.

## Tests

| Behavior | Test |
| --- | --- |
| `[ignore]`, the ignored count, `--ignored`, `--include-ignored` | `tests/m10_test_runner/attributes_ignore.bd`, `tests/check_test_runner.py` |
| `[should_panic]`, `[should_panic("text")]`, `[should_fail]` | `tests/m10_test_runner/attributes_should_panic.bd`, `tests/check_test_runner.py` |
| `[timeout(ms)]` | `tests/m10_test_runner/attributes_timeout.bd`, `tests/check_test_runner.py` |
| `[differential]`: agreement, `Ok` and `Err` | `tests/m10_test_runner/attributes_differential.bd`, `tests/check_test_runner.py` |
| `[differential]`: a divergent compiled run | `tests/runner/differential/`, `tests/check_test_runner.py` |
| `[differential]`: a body the interpreter cannot run (E0224) | `tests/m10_test_runner/differential_impure.bd` (`//@ test-compile-fail`) |
| Invalid attributes (E0170) | `tests/m10_test_runner/attributes_invalid.bd`, `tests/m10_test_runner/differential_invalid.bd` (`//@ test-compile-fail`) |
| Parallel execution and deterministic failure order | `tests/check_test_runner.py` |
| Output capture, `--verbose` | `tests/check_test_runner.py` |
| Fresh working directory, clean environment, cleanup | `tests/check_test_runner.py` |
| `-p`, `--workspace`, `--filter`, dependency tests | `tests/check_test_runner.py` |
