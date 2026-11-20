# The Burdin test runner

This file is the behavior of `burdin test`: the `test` items of a package, their attributes, and the doc tests of
the code blocks in its doc comments. The syntax is in
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
  the number of available CPUs. `-j 1` is sequential. The same `-j` bounds the threads that compile the test
  binary (parsing and code generation); the report does not depend on it.
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
- a closed standard input, so `Stdin.read_line` and `Stdin.read_to_string` see the end of the input and a test
  never waits for a person.
- an empty environment, except for these variables:
  - `PATH`, copied from the runner, so `process.run` can find commands;
  - `TMPDIR`, `TMP`, and `TEMP`, set to the test's working directory, so a program that writes temporary files
    cleans them up with the directory;
  - `BURDIN_TEST`, the test's display name;
  - `BURDIN_SNAPSHOTS`, for a `test` block, the file its `assert_snapshot` calls record into
    ([Snapshot tests](#snapshot-tests), SN2);
  - on Windows, the variables the C runtime needs to start: `SystemRoot`, `SystemDrive`, `windir`, `ComSpec`,
    `PATHEXT`, `NUMBER_OF_PROCESSORS`, `PROCESSOR_ARCHITECTURE`, and `OS`, when the parent has them.

A test may rely on its working directory being empty and private and on the environment above. It must not rely on
any other variable, on the working directory's path, or on `env.args()`: the runner reserves the arguments (the
program name and the test index are how the dispatcher finds the test).

The runner removes the test binary directory and every test working directory when `burdin test` exits, including
after a failure.

The binary `burdin test` runs is also available on its own: `burdin build --tests <target> -o <exe>` compiles the test
blocks and the dispatcher instead of `main` and writes the executable where `-o` says, with no runner, capture, or
isolation. `<exe> <index>` then runs the test at `index` in the order `burdin test --list` prints them. The debug
adapter builds a single test this way and starts the debugger on it with the index as the program's first argument
(`spec/debug.md`, DBG10-DBG11).

## Discovery and selection

The target is a `.bd` file or a package directory.

- By default only the entry package's tests run. Dependency tests do not run, because a dependency may be shared and
  its tests may not hold for this build.
- `-p <name>` selects a package of the build; it may repeat and duplicates are removed. The packages are the entry
  package and every package reachable through `[dependencies]` path entries. The embedded standard library has no
  directory and cannot be selected.
- In a manifest-level workspace (`spec/workspaces.md`), `-p` and `--workspace` select members instead, and
  `--exclude` leaves members out: the selected members run together with one report, and their shared outputs (the
  `--failed` record) live under the workspace root's `target/` (WS6, WS11). The members are the build's roots, so a
  package two members reach is loaded once. Outside a workspace the rules above apply unchanged (WS7).
- `--workspace` selects the entry package and every path dependency. A *workspace* here is the build's package
  graph; in a manifest-level workspace it is the member set instead (`spec/workspaces.md`, WS11). `-p` and
  `--workspace` cannot be combined.
- When more than one package is selected, a test's displayed name is `<package>::<test name>` so two packages may
  use the same test name. A single selected package keeps the plain test name. `FAIL`, `PASS`, `IGNORED`, and the
  captured-output headings all use the displayed name.
- A test in a package with no `src/main.bd` still runs: test mode does not require `main`.
- `--doc` drops the `test` blocks instead of adding anything: only the doc tests are selected, listed, and run, and
  no `test` block is compiled. Every other option applies to the doc tests like any other test.

### The full path

Every test has a *full path*, which is what filters match and what `--list` prints:

```
<package>::<module>.<submodule>.<test name>
```

The module part is the dotted path of the file inside the package's `src` directory (`geometry.shapes` for
`src/geometry/shapes.bd`); it is empty, with its dot, for a test in the package's entry file (`src/main.bd` or
`src/lib.bd`) and for a single-file target. The `<package>::` prefix is present only when more than one package is
selected, like the displayed name. So `area of circle` in `src/geometry/shapes.bd` has the full path
`geometry.shapes.area of circle` (or `app::geometry.shapes.area of circle` in a `--workspace` run), and a test in
`src/main.bd` is just `alpha`. The displayed name in the report stays the plain test name. Tests are discovered
package by package, in build order; inside a package, file by file in the order the driver reads the modules
(alphabetical by path), and inside a file in declaration order.

### Name filters

`burdin test <target> [<filter>...]` runs the tests whose full path contains any filter. A filter is matched
case-sensitively as a substring; no filter selects every test. Because the package and module names are part of the
path, a filter can select a module (`geometry`) or a package (`util::`). `--filter <text>` is the older spelling of a
positional filter and may repeat; the two are the same.

- `--exact` makes every filter and every `--skip` pattern compare equal to the whole full path instead of being
  contained in it. `--exact` with no filter and no `--skip` changes nothing.
- `--skip <pattern>` (repeatable) removes the tests whose full path contains `pattern` (equals it, with `--exact`),
  after the filters have selected: a test is run when some filter selects it and no `--skip` pattern does.
- Filters apply after discovery, across every selected package, so `-p`, `--workspace`, and filters combine.
- `--ignored` and `--include-ignored` apply after the name rules.
- A filter that matches nothing is not an error: the exit status is 0, the summary reads
  `test result: 0 passed, 0 failed, <n> filtered out`, and `note: no test matches the filters` goes to standard
  error.

The *filtered out* count in the summary is the number of discovered tests that the name filters, `--skip`,
`--failed`, and `--shard` left out. `--ignored` drops the tests that are not ignored without counting them.

### Selection order

The steps run in this order: the name filters and `--skip`; `--failed`; the ignore mode (`--ignored`); then
`--shard`, which numbers what is left. Tests that reach the end are run, or listed with `--list`.

## Listing tests

`burdin test --list` compiles the selected packages, discovers the tests, applies the selection rules, and prints
them without running anything. The exit status is 0; compile errors and usage errors behave as in a run. A listing
shows ignored tests (with their `ignore` attribute) unless `--ignored` is given, which keeps only them;
`--include-ignored` has no effect. `--list` cannot be combined with `--junit`.

The human format prints one line per selected test, its full path followed by its attributes in the source syntax
and in a fixed order (`ignore`, `timeout(ms)`, `should_panic[("text")]`, `should_fail`, `differential`), then a
count line:

```
geometry.area
geometry.perimeter [should_fail]
beta [ignore, timeout(500)]
delta panics [should_panic("boom")]
4 tests, 3 filtered out
```

The count is `<n> test` or `<n> tests`, with `, <k> filtered out` when the selection left tests out.

`--list --format json` prints one JSON object per selected test, one per line, in the same order, with these fields
(the schema is version 1, see [Machine-readable reports](#machine-readable-reports)):

| Field | Type | Meaning |
| --- | --- | --- |
| `package` | string | the package the test belongs to |
| `module` | string | the dotted module path, `""` for the entry file |
| `name` | string | the name written in `test "name"` |
| `path` | string | the full path, as a filter or `--exact` takes it |
| `file` | string | the source file, spelled as diagnostics spell it |
| `line` | number | the 1-based line of the test's name |
| `attributes` | object | `ignore`, `timeout_ms` (number or `null`), `should_panic`, `should_panic_message` (string or `null`), `should_fail`, `differential` |

The language server's test integration (issue #374) and CI sharding scripts read this form.

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
skipped because of `[ignore]` and `, <filtered out> filtered out` appended when the name filters, `--skip`,
`--failed`, or `--shard` left tests out (in that order). A run with none of these prints exactly the summary it
always has. Timeouts count as failures.

When tests ran for at least the slow threshold (below), one `slow test: <name> (<ms> ms)` line per such test, in
declaration order, comes just before the summary line, so the summary stays the last line.

The exit status is 0 when every run test passed (and no test failed), 1 when a test failed or compilation failed,
and 2 for a usage error (an unknown package, a bad `-j`, conflicting flags) or a driver failure after loading.

## Machine-readable reports

`burdin test --format json` replaces the human report with a stream of JSON objects, one per line on standard
output, written as the run progresses. Compilation diagnostics still go to standard error, and a run that fails to
compile or load prints no events. The exit status is the same as the human format's. `--junit <file>` writes a JUnit
XML report in addition to either format.

**Schema version 1.** The `version` field of `suite_started` is the version of every JSON line `burdin test` prints,
both the `--list` objects and the events. A later version only adds fields; a field changes meaning or goes away only
with a new version number. A reader ignores fields it does not know.

Every event has an `event` field. The events about one test carry the test's identity fields (`package`, `module`,
`name`, `path`, `file`, `line`, as in `--list`):

| `event` | When | Fields besides the identity |
| --- | --- | --- |
| `suite_started` | once, first | `version`, `packages` (the selected package names), `test_count` (selected tests, ignored ones included), `ignored`, `filtered_out` |
| `test_ignored` | once per ignored test, right after `suite_started` | |
| `test_started` | when a test's process starts | |
| `test_passed` | when it ends and passes | `duration_ms`; `stdout` and `stderr` only with `--verbose` |
| `test_failed` | when it ends and fails | `duration_ms`, `reason` (the explanation the human report puts in the `---- test "name": <reason> ----` heading, or `null`), `stdout`, `stderr` |
| `suite_finished` | once, last | `passed`, `failed`, `ignored`, `filtered_out`, `duration_ms`, `slow` (a list of `{"path", "duration_ms"}`, empty when none) |

Output is text: bytes that are not UTF-8 are replaced, and control characters are escaped as JSON requires. With
`-j 1` the events of different tests never overlap and follow declaration order; with more workers `test_started`
and `test_passed`/`test_failed` events arrive in completion order, one whole line at a time, and `path` ties them
together.

**JUnit.** The XML has the usual shape: a `testsuites` element (`name="burdin test"`, `tests`, `failures`, `skipped`,
`time` in seconds) holding one `testsuite` per package (`name`, `tests`, `failures`, `skipped`, `time`), each
holding one `testcase` per selected test (`classname` is the package and the module path joined with dots, `name` is
the test name, plus `file`, `line`, and `time`). A failed test (a timeout included) has a `failure` child whose
`message` is the reason (or `test failed`) and whose text is the captured standard error, and `system-out` and
`system-err` children with the captured output. An ignored test has a `skipped` child. Passing tests carry their
output only with `--verbose`. Characters XML cannot hold are replaced with U+FFFD. A report that cannot be written is
a driver failure: the message goes to standard error and the exit status is 2, whatever the tests did.

## Reruns, shards, and slow tests

- `--failed` runs only the tests that failed in the previous run of the same profile. The record is the file
  `<package>/target/burdin/<debug|release>/test-failed.txt`, next to the build-script output, in the entry package's
  directory (so `--failed` needs a package directory, not a single `.bd` file, and is a usage error otherwise).
  Every `burdin test` run that runs tests updates it: a test that ran and passed leaves the record, a test that
  failed enters it, a test that did not run keeps its status, and an entry whose test no longer exists is dropped
  (entries of packages the run did not select stay). A run that fails to compile leaves it alone, a green run with
  no record on disk writes nothing, and a `--list` run never writes. With `--failed` and no record at all, every
  test is selected and `note: no record of a previous run was found; running every selected test` goes to standard
  error; with a record that has no failures, nothing is selected and `note: the previous run had no failed tests`
  says why. A test is identified by its package and its package-local path, so the record does not depend on which
  packages a run selected. `--failed` combines with the name filters, `--skip`, and `--shard`.
- `--shard <i>/<n>` runs the `i`-th of `n` slices, counting `i` from 1 (`1 <= i <= n`, anything else is a usage
  error). The tests that survive the earlier selection steps are numbered from 0 in declaration order, and a test
  belongs to shard `(number mod n) + 1`: the slices are disjoint, cover every selected test, and depend only on the
  order of discovery, so each CI job computes its own slice with no coordination. Ignored tests are numbered too.
  The tests in other slices count as filtered out.
- `--slow-threshold <ms>` (default 5000, `0` disables) lists the tests whose run took at least that long: in the
  human summary as described above and in the JSON `suite_finished` event. It is only a report. `--timeout` and
  `timeout(ms)` still kill a hung test.

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

## Snapshot tests

A snapshot assertion compares a value's text with a copy the package keeps, so a test of a large or structured
output does not have to spell the expected text in an `assert_eq` (issue #616).

```
assert_snapshot(value)                              // the Debug text, kept in a file
assert_snapshot(value, display: true)               // the Display text
assert_snapshot(value, @"[1, 2, 3]")                // kept in the call itself: an inline snapshot
assert_snapshot(value, @r#"Point { x: 1, y: 2 }"#)  // a raw inline snapshot holds quotes and braces
```

- **SN1. The call.** `assert_snapshot` is a `std` function in the prelude of every build. Its first argument is
  the value, formatted with `Debug` (`{:?}`), or with `Display` when the named argument `display: true` is given;
  `display:` takes the literal `true` or `false`, because the format is decided when the test is compiled. The
  optional second argument is an inline snapshot. Any other argument list, and an inline snapshot `@"..."`
  anywhere but that second argument, is **E0172**. The call returns `()`: a test does not see whether its snapshot
  matched, and it goes on after a difference, so one run reports every changed snapshot of a test. Outside
  `burdin test` the call formats the value and records nothing.
- **SN2. Recording.** Each call appends a record of its location, its kind, and the formatted text to the file
  named by the variable `BURDIN_SNAPSHOTS`, which the runner sets for each `test` block process (in addition to the
  variables of [Isolation](#isolation)). The runner compares the records of each test, in the order the calls ran,
  after every test has finished. A test that ends before a call (a panic, an `Err`) records nothing for it.
- **SN3. Files.** A snapshot without an inline literal lives in `snapshots/<module>__<test>.snap` in the directory
  of the test's source file. `<module>` is the test's dotted module path, or the file's stem for a package's entry
  file and a one-file target (`main`, `lib`); in `<test>` every character other than an ASCII letter, a digit, `_`,
  and `-` becomes `_`. The second and later file snapshots of one test add `-2`, `-3`, ... before `.snap`. The file
  is a header between two `---` lines and then the snapshot followed by one newline:

  ```
  ---
  test: geometry.renders a board
  source: shapes.bd:41
  expression: assert_snapshot(board())
  ---
  ["x.o", ".x."]
  ```

  `test` is the test's package-local path, `source` the file name and line of the call, and `expression` the
  call's text on one line. Only what follows the header is compared; a line ending `\r\n` reads as `\n`.
- **SN4. Inline snapshots.** `@` followed by a string literal without placeholders is an inline snapshot; a text
  with quotes, braces, or backslashes is written as a raw string, `@r#"..."#`, and `@"text {x}"` is a syntax error
  that says so. A literal that starts with a line break is in *block form*: the break is dropped, the last line,
  which holds only the spaces before the closing quote, is dropped with the break before it, and those spaces are
  removed from the start of every other line. Any other literal is the snapshot exactly. The runner finds the
  literal from the call's location in the source file.
- **SN5. Differences.** A snapshot whose file is missing, or whose stored text differs from the recorded one, fails
  the test. The reason heading names the snapshot (`snapshot src/snapshots/main__board.snap does not match`, `...
  is missing`, `inline snapshot at src/main.bd:46 does not match`), and the test's captured stderr gets a unified
  diff from the stored text to the new one (only additions for a missing snapshot) and how to accept it. A file
  snapshot also writes the new file, with its header, as `<name>.snap.new` next to the `.snap`; a later run that
  matches removes it. A test that failed for its own reason keeps that reason, followed by the snapshot's.
- **SN6. `--bless`.** Every missing or different snapshot is accepted: the `.snap` file is written (and its
  `.snap.new` removed), and an inline snapshot's literal is replaced in the source by the shortest literal that
  holds the text: `@"text"` for one plain line, `@r#"..."#` (with as many `#` as the text needs) when it has a quote,
  a brace, a backslash, or a tab, and the block form indented four spaces past the call's line for more than one
  line. An accepted snapshot does not fail its test, and `note: updated <snapshot>` goes to standard error.
- **SN7. `--review`.** Each difference is shown on standard output, in declaration order, with a diff and the
  question `accept it? [y]es, [n]o, [a]ll, [q]uit: `, read from standard input: `y` accepts this snapshot as
  `--bless` would, `n` leaves it failing, `a` accepts this and every later one, and `q` (or the end of the input)
  declines this and every later one. `--bless` and `--review` cannot be combined, and both are usage errors on
  another command.
- **SN8. Stale snapshots.** When a run selected every test of its packages (no filter, `--skip`, `--failed`,
  `--shard`, `--ignored`, or `--doc`) and every test ran and passed, a `.snap` file in a `snapshots/` directory next
  to one of those packages' source files that no call of the run checked is reported on standard error as
  `warning: stale snapshot <path>: no test of this run checked it`. A stale snapshot does not change the exit
  status, and the runner never deletes one.
- **SN9.** The doc tests record no snapshots: an example is an ordinary program, so its `assert_snapshot` calls
  format their values and record nothing.

## Interpreting a test

`burdin test --interpret` runs the `test` blocks in the compiler's interpreter (`spec/comptime.md`, CT1) instead of
compiling them, with a model of the C heap and the checks a sanitizer gives a compiled program (issue #622). It needs
no linker, no sanitizer runtime, and no code generation, and a fault points at the source line that caused it.

- **IN1. What runs.** The front end parses and checks the package exactly as for a normal `burdin test`, so a
  package that does not compile fails as it always does. Then each selected `test` body runs in the interpreter,
  with output captured and dropped the way a compiled run captures it. Selection, `--filter`, `--skip`, `--exact`,
  `-p`, `--workspace`, `[ignore]`, and the summary line are the ones of a normal run; the summary gains
  `, N skipped` when a test needed something the interpreter does not model. `--interpret` cannot be combined with
  `--fuzz-replay` (a replay runs a compiled binary), with `--sanitize` (there is no compiled code), or with `--bless`
  and `--review` (a snapshot comparison reads the file system, so those tests are skipped). Interpreted tests run
  one at a time, so `-j` has no effect, and `--timeout` does not apply: the step budget of a compile-time evaluation
  (`--comptime-steps`, CT11) ends a test that runs away, reported as a failure of that test.
- **IN2. The heap.** `__alloc(size, align)` returns a pointer into a modeled block whose bytes are uninitialized,
  `__realloc` grows or shrinks it, and `__dealloc` frees it. A pointer is an integer, so `p + n` moves by `n`
  elements of `p`'s type, `p - q` is a distance in elements, and casts between `CPtr` and the integer types keep
  the value. `*p` reads and `*p = v` writes the pointee, for the scalar pointees `bool`, `char`, the integer types,
  `f32`, `f64`, and `CPtr`. A `CPtr` of any other pointee, a `CString`, and a C function are outside the model
  (IN6).
- **IN3. Bounds, alignment, and initialization.** A read or write must start at a non-negative offset of a block,
  end inside it (`size` bytes fit), begin at an address that is a multiple of the pointee's alignment and of the
  block's, and cover bytes that were written at least once. A violation is a failure of the test:
  ``out-of-bounds access: 4 byte(s) at offset 8 of a 8-byte allocation``, ``misaligned access: offset 1 of a
  8-byte-aligned block for a 8-byte type``, or `read of uninitialized memory`.
- **IN4. Freed memory.** A dereference of a freed block is `use after free: block N was freed`, and freeing a block
  twice is `double free: block N was already freed`. Freeing a null pointer or a pointer that is not the start of
  its block is a failure too.
- **IN5. Leaks.** A test that ends with a block it never freed fails with `leaked N byte(s) allocated here`, at the
  line of the `__alloc`. The check runs after the body finishes normally, so a test that failed earlier reports its
  own failure.
- **IN6. Skips.** A test that calls C, reads the environment, touches the file system, runs a thread or an atomic, or
  uses a type the model does not cover cannot be checked, so it is skipped and the report names what it needed:
  `SKIP reads the environment (reading an environment variable)`. A skip is not a pass and not a failure: the
  summary counts it separately, and a run whose tests are all skipped exits 0.

## Doc tests

The Burdin code blocks of a package's doc comments are tests too (issues #250, #650, #651, #652): every fenced
block that holds Burdin code in a `///` or `//!` comment of a selected package is extracted, built as its own
program, and run with the `test` blocks. `burdin test --doc` runs only them; a plain `burdin test` runs both.
The doc-comment reading is the one `burdin doc` uses ([`doc.md`](doc.md), DC1-DC4) and the Markdown is the one it
renders with (DC5-DC6), so a block a reader sees and the test the runner builds cannot disagree about where the
block starts or what its fence says.

### Which blocks are doc tests

- A fenced block whose info string's first word is empty, `burdin`, or `bd` (DC6): an untagged block, or one tagged
  with the attributes below after the word. A block tagged with any other word (`text`, `console`, `rust`, ...) is
  not a doc test, and neither is an empty block. The words after the language word — comma or space separated —
  are the block's attributes. An untagged block writes them after a comma: <code>```,ignore</code>. Blocks nested
  in a quote or a list are doc tests like any other.
- Every doc comment of the package's source files holds doc tests: module documentation (`//!`), items, fields,
  variants, methods, interface methods, extension methods, and `priv` items and `main` included. A comment the
  parser generated an item from (a `: Debug` after a type's name, an `error enum`'s impls) has none.
- A doc test's name is `<owner> (line N)`, where `N` is the 1-based line of the block's opening fence and the owner
  is the documented item's name as a report shows it (`Point.new`, `Shape.Circle`, `Iterator.next`), or the file's
  stem for module documentation. Its module is the file's dotted path inside `src`, empty for the package's entry
  file (`src/main.bd`, or the root module a library package starts from), exactly as a `test` block's is. Its full
  path is `<module>.<owner> (line N)`, prefixed with `<package>::` when more than one package runs, and its
  `file` and `line` are the doc comment's: the original source file and the fence's line, never a generated one.
- Doc tests are discovered package by package in build order, file by file in the order the driver reads the
  modules, and within a file in source order; a package's `test` blocks come before its doc tests in the report.
  Name filters, `--exact`, `--skip`, `--failed`, `--shard`, `--ignored`, `--list`, `--format`, `--junit`, and
  `--timeout` apply to them like any other test, and so does the `--failed` record.

### Doc-test attributes

| Attribute | Meaning |
| --- | --- |
| `ignore` | The block is not built and not run. It counts as *ignored*; `--ignored` and `--include-ignored` select it. |
| `no_run` | The block is compiled (the front end and the memory checks) but not run. Passing the compile is the whole test. |
| `should_panic` | The compiled example must end in a panic abort. |
| `compile_fail` | The example must not compile. `compile_fail E0123` must fail with that code; the failure note names the code it did fail with. |
| `text` | The fence's language word, not an attribute: a block tagged `text` is not Burdin and not a doc test. |

A word that is none of these warns on standard error, once per block, at the fence's line
(`warning: <file>:<line>: unknown code-block attribute `word``), and the block still runs. `compile_fail` takes the
error code that follows it (`compile_fail E0123`, space or comma separated); a code that is not `E` and four digits
warns and is ignored. The attributes are not exclusive; when more than one applies, `ignore` wins, then
`compile_fail` (a block that compiles fails the test before anything runs), then `no_run` (the example never runs,
so `should_panic` is never checked). There is no `timeout` attribute and no `should_fail` for a doc block: an
example is an ordinary program with no dispatcher to return an error.

### Building one doc test

- Each doc test becomes a one-file package of the build directory: a `burdin.toml` whose only dependency is the
  documented package, and a `src/main.bd` that holds the example. The dependency is what makes the example see the
  package's public API only: a `priv` item, field, or method cannot be named from an example, which is exactly what
  the doc tests are for. A package with no importable root module (`src/<name>.bd` or `src/lib.bd`) cannot be
  imported by its examples; the runner says so once on standard error and the examples still run, standing alone. A
  one-file target is a package of one module: it is copied into a package of the build directory (when its stem is
  an identifier), so an example imports it by its own stem.
- The example's lines are the block's lines with a hidden `# ` marker removed (`# use geometry.{Point}` compiles as
  `use geometry.{Point}`; DC6 says what `burdin doc` does with them). The example is the whole program when one of
  its lines declares `fn main`; otherwise its leading declarations — the longest prefix of its lines that parses as
  module items — are hoisted above a generated `fn main() { ... }` wrapper that holds the remaining statements, so
  an example may declare a helper and then call it. Lines after the first statement that are declarations do not
  parse in the wrapper and are reported as such.
- The generated program is padded with comment lines so the example lands on the line of its doc comment: a panic
  the compiled example raises reports the line a reader sees. The compiler's diagnostics are moved the same way,
  from the generated file to the original one, before they are shown; a diagnostic on a generated line (the wrapper,
  the padding) points at the fence. A failure reports the doc comment's file and line, never a generated path —
  except the raw backtrace of a run-time panic, which names the binary the process ran, while the report's headings
  carry the true location.
- The doc tests selected (and not ignored) are built in parallel: `-j` bounds how many build at once, and one build
  alone uses the machine's parallelism. A build whose front end fails decides the test without a process; one whose
  example is runnable is linked once and run once. Building never re-runs the documented package's build script
  (the generated manifest grants no `[build] allow`), so a module its script generates is not part of what an
  example can import.

### Running and reporting

- A doc test runs exactly like a `test` block: its own process, a fresh working directory, a clean environment with
  `BURDIN_TEST` set to its display name, captured output, and the `--timeout` default (it has no `timeout(ms)` of
  its own). The difference is the argument: a doc test's process takes no test index, because the program is the
  test. The checking allocator of a `test`-block build does not apply: an example is a normal program with `main`.
- A failing doc test is reported with its location in the headings, before its captured output and its `FAIL` line:

  ```
  ---- doc test "Point.new (line 12)" at src/shapes.bd:12: did not compile ----
  ---- doc test "Point.new (line 12)" at src/shapes.bd:12 stderr ----
  src/shapes.bd:13:5: error[E0002]: unknown name `distance_squared`
  FAIL Point.new (line 12)
  ```

  The reason rides in the first heading when there is one (`did not compile`, `expected the example to fail with
  E0123, but it failed with E0003`, `timed out after 250 ms`); the compile's diagnostics are the captured stderr.
  `PASS`, `IGNORED`, and the summary lines are the same as a `test` block's, so the counts hold both kinds.
- `--list` prints a doc test's full path and the attributes of its block in the fixed order `ignore`, `no_run`,
  `should_panic`, `compile_fail` (with its code when it has one). The JSON listing carries the same identity
  fields as a `test` block's — `file` and `line` are the doc comment's — and an `attributes` object of its own:
  `ignore`, `should_panic`, `no_run`, `compile_fail`, and `compile_fail_code` (string or `null`).
- The JSON event stream decides a doc test without a process at its build: a `no_run` or `compile_fail` example,
  or one that failed to compile, emits its `test_started` and `test_passed`/`test_failed` pair when the build
  finishes; one that will run emits `test_started` when the runner starts its process, like a `test` block.
- The exit statuses are a plain run's: 0 when every built and run example passed, 1 when one failed, 2 for a usage
  error. `--doc` with a package whose `test` blocks do not compile never compiles them: the doc tests are judged on
  their own.

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
| The full path; positional filters, `--exact`, `--skip`, and the filtered-out count (#638) | `tests/check_test_runner.py` (`tests/runner/selection/`, `tests/runner/workspace/`), unit tests in `crates/burdin_driver/src/test_runner/select.rs` |
| `--list`, human and JSON (#639) | `tests/check_test_runner.py` (golden listing, JSON fields), unit tests in `crates/burdin_driver/src/test_runner/report.rs` |
| `--format json` events and `--junit` (#640) | `tests/check_test_runner.py` (event sequence, escaping, XML structure, an unwritable report), unit tests in `report.rs` |
| `--failed`, `--shard`, `--slow-threshold` (#641) | `tests/check_test_runner.py` (`tests/runner/selection/`, `tests/runner/slow/`), unit tests in `select.rs` |
| Doc tests: extraction, wrapping, hoisting, and the line-true padding | `tests/runner/doctests/`, unit tests in `crates/burdin_driver/src/test_runner/doc_tests.rs` |
| Doc tests: a failure reports the doc comment's file and line (#650) | `tests/check_test_runner.py` (`tests/runner/doctests/`) |
| Doc-test attributes: `ignore`, `no_run`, `should_panic`, `compile_fail`, `compile_fail E0003`, unknown warns (#651) | `tests/runner/doctests/`, unit tests in `doc_tests.rs` |
| Hidden `# ` setup lines are compiled and hidden (#652) | `crates/burdin_doc/src/markdown.rs` (unit tests), `tests/doc/geometry`, `tests/check_doc.py`, `tests/check_test_runner.py` |
| Doc tests: `--doc`, one-file targets, a package without an importable root | `tests/runner/doctest_one.bd`, `tests/runner/doctests-bin/`, `tests/check_test_runner.py` |
| The `burdin doc` fixture's example runs green | `tests/check_test_runner.py` (the `tests/doc/geometry` doc test) |
| Snapshots: file and inline matches, a changed and a missing snapshot with a diff and a `.snap.new`, `--review`, `--bless` rewriting a file and an inline literal, and a stale snapshot (SN1-SN8, #616) | `tests/runner/snapshots/`, `tests/check_test_runner.py`, unit tests in `crates/burdin_driver/src/test_runner/snapshots.rs` |
| An inline snapshot outside `assert_snapshot`, bad `assert_snapshot` arguments (E0172) | `tests/m10_test_runner/snapshot_invalid.bd` (`//@ test-compile-fail`) |
| `--interpret`: an out-of-bounds store, use after free, double free, uninitialized read, misaligned store, and leak, each at its line (IN2-IN5) | `tests/interpret/bugs.bd`, `tests/check_interpret.py` |
| `--interpret`: correct code, std containers, iterators, and closures pass; a C test and an environment test are skipped with the reason (IN1, IN6) | `tests/interpret/passing.bd`, `tests/interpret/calls_c.bd`, `tests/check_interpret.py` |
| `--interpret`: ordinary test files give the verdicts a compiled run gives, std code included | `tests/m10_test_runner/all_pass.bd`, `assert_messages_typed.bd`, `helper_propagation.bd`, `tests/m24_stdlib/interpreted_stdlib.bd`, `tests/check_interpret.py` |
| `--interpret` combined with `--fuzz-replay`, `--sanitize`, `--bless`, or another command (IN1) | `tests/check_interpret.py` |
