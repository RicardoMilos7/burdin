# Changelog

Burdin has no backward-compatibility promise (see `Burdin Language Design.md`). Entries are written per milestone; the
GitHub milestones and the roadmap track the work in progress. A milestone closes with an entry here (see the
close-out checklist in `AGENTS.md`).

## Unreleased — Stage 3

### The query-based compiler library (issue #241, sub-issues #645-#649)

- The front end is a library with a memo (new crate `burdin_query`, `spec/query.md`): file texts in, diagnostics and
  the typed program out, with everything an edit cannot affect reused. `burdin build`, `run`, `test`, and `lint` run
  their front end through it (`burdin_driver::front_end`), so the command line and a tool share one implementation.
- The engine is an in-house memo, not salsa (QL1): the typed tree and the checker's registries are `Rc`/`RefCell`
  graphs, and salsa's storage wants `Send + Sync` values; the memo is a table keyed by 128-bit fingerprints.
- Granularity (QL2): a file's parse is keyed by its text, and a function or constant body is keyed by the text of its
  declaration span, the declaration fingerprint (every file's text with the parser-recorded body spans elided), and
  the pass chain. A body edit rechecks that body; a declaration edit rechecks every body. The memory checks and
  compile-time evaluation are whole-program passes on every analysis (QL3).
- An edit inside one function body reparses only that body (QL6): the parser records every body span, the new block
  is parsed alone, and the tree's spans after the edit are moved. `main`, `select`, a file with syntax errors, and an
  edit that is not strictly inside one body fall back to a full parse.
- The tool API (QL7) is plain data: a lossless syntax tree with tokens and trivia, the diagnostics of one file, and
  `resolve_at`, `type_at`, `signature_of`, and `ownership_of` at a byte offset. `spec/query.md` is the behavior, with
  a golden file of answers for the fixture package.
- Cancellation and the budget (QL8): `CancelHandle` stops an analysis from another thread and leaves the database
  usable; `Budget { bodies }` (40,000 by default) evicts the least recently used checked bodies, and `Options::no_cache`
  turns the memo off for a one-shot build.
- Parity (QL4): the answer of an edited database equals a fresh one (`tests/equivalence.rs`, random edits over the
  conformance corpus) and the library reports the command line's error diagnostics for every conformance test that
  must fail to compile (`crates/burdin_driver/tests/parity.rs`).
- Counters (QL5) make the work visible: `Db::stats()` per analysis and `Db::counters()` in total. The benchmark gains
  the query cases (QL9): `cargo bench -p burdin_bench -- query` measures a keystroke edit and a cold analysis on a
  generated package of one, two, and four modules, and `--smoke` fails unless a keystroke rechecks exactly one body.
- Tests: `crates/burdin_query/tests/` (`incremental.rs`, `reparse.rs`, `equivalence.rs`, `api.rs` with
  `fixtures/api.golden`, `stress.rs`, `memory.rs` with a counting allocator), the driver's `parity.rs`, and the new
  unit tests of `db.rs` and `spans.rs`.
### Machine-readable diagnostics (issue #236)

- The JSON format is frozen as version 1 (issue #642, `spec/diagnostics.md` DG2-DG7). Every object carries
  `"version": 1`. A diagnostic has its `code`, `severity`, `message`, `url`, primary `span`, `labels`, `notes`, `help`,
  `related` (the notes that have a span, shaped like the Language Server Protocol's `relatedInformation`), `children`,
  `suggestions` (each with `applicability` and a list of `edits`), and the `rendered` human text. A location has line
  and column in Unicode scalar values, UTF-8 bytes, and UTF-16 code units, and byte offsets. The schema is
  `docs/diagnostic.schema.json` (closed, so any change to the output changes it) and `docs/diagnostics.md` documents
  the fields and the versioning rule. `tests/check_diagnostics.py` validates every line printed for every failing
  conformance test against the schema, checks that mutated objects are rejected, and compares goldens in
  `tests/diagnostics/`. The model gained `labels`, `help`, and `children`; the human format is unchanged.
- `--diagnostic-format sarif` writes one SARIF 2.1.0 log (issue #643, DG8-DG11): the error-code registry and the lints
  as rules with a link to each entry, results with locations, related locations, and fixes, and `invocations` saying
  whether the run succeeded. `burdin build` and `burdin lint` write the log even when nothing was found; `run` and
  `test` write one only when the compilation reported something; `burdin fmt` refuses the format. A subset schema in
  `tests/diagnostics/sarif-subset.schema.json` and a golden result list prove the shape.
- Every diagnostic links to its entry in the hosted error reference (issue #644, DG12-DG14):
  `https://ricardomilos7.github.io/burdin/errors/E0003` (`/lints/L0007` for a lint), a scheme that ends in the code
  and is pinned by `tests/check_error_docs.py`. It is the `url` of JSON, the `helpUri` of SARIF, a line printed by
  `burdin explain`, and an OSC 8 hyperlink on the code in colored human output when the terminal supports it
  (`FORCE_HYPERLINK=1` or `0` decides). The short format stays one plain line.

### `burdin test` filters, `--list`, reports, reruns, and shards (issue #235)

- **Filters (#638).** `burdin test <target> [<filter>...]` runs the tests whose full path (`module.submodule.test name`,
  prefixed `<package>::` when several packages run) contains any filter; `--filter` is the same thing. `--exact`
  requires an equal path, `--skip <pattern>` (repeatable) leaves tests out, and the summary appends
  `, N filtered out`. `TestEntry` now carries the module path and the span of the test's name.
- **Listing (#639).** `burdin test --list` prints each selected test with its attributes and a count, without running
  anything; `--list --format json` prints one object per test with package, module, name, path, file, line, and
  attributes, for editors and CI.
- **Reports (#640).** `--format json` streams `suite_started`, `test_started`, `test_passed`/`test_failed` (with
  duration, reason, and captured output), `test_ignored`, and `suite_finished` events, one JSON object per line, with a
  versioned schema in `spec/test-runner.md`. `--junit <file>` writes a JUnit XML report. The human output does not
  change.
- **Reruns, shards, and slow tests (#641).** `--failed` reruns the tests that failed in the previous run, kept in
  `<package>/target/burdin/<profile>/test-failed.txt`; `--shard <i>/<n>` runs a deterministic round-robin slice; tests
  that ran at least `--slow-threshold` milliseconds (default 5000) are listed before the summary.
- The runner is split into `test_runner.rs` (compile, execute), `test_runner/select.rs` (selection and the failed
  record), and `test_runner/report.rs` (human, list, JSON, and JUnit output). `tests/check_test_runner.py` gains about
  150 black-box checks over the fixtures `tests/runner/selection/` and `tests/runner/slow/`, and the driver has unit
  tests for each rule.

### Tooling: `burdin check` (issue #237)

- `burdin check <file.bd | package directory>` type-checks without building: it runs the front end of `burdin build`
  (build scripts, parsing, name resolution and type checking, the return-origin, move, and borrow checks, and
  compile-time evaluation) and stops before code generation, so it needs no linker and writes no file. It is for fast CI
  gates and editor feedback (`spec/check.md`, CK1-CK8).
- The diagnostics, formats, and exit statuses are those of a build: a clean check prints nothing and exits 0, errors exit
  1 and end with `error: could not check <target> (N errors)`, and a usage error exits 2. `--tests` also checks `test`
  blocks and accepts a library package without `src/main.bd`; `--release` and `--comptime-steps` are as for a build;
  the options that only matter for an output (`-o`, `--emit-ir`, `-g`, `--lto`, `--link-lib`, ...) are rejected.
- `burdin check` reuses `front_end`, the shared front end that `build`, `run`, `test`, and `lint` also call; that
  front end runs through the query-based compiler library (the entry above).
- The error reporting of a failed compilation moved out of `compile` into `report_errors`, and the diagnostics options
  of the command line into `parse_diagnostic_option`, so the commands share them.
- `crates/burdin_driver/src/check.rs` has unit tests for the command line; `tests/check_check.py` black-boxes the
  command: the fixtures and golden outputs under `tests/check/`, equality with `burdin build`'s diagnostics, every
  conformance test (a `compile-fail` program must report exactly its markers, every other program must be accepted),
  no linker needed and nothing written, and the command line. It runs in every CI job.

### Tooling: `burdin new` (issue #228)

- `burdin new <name>` creates the package directory `<name>` with `burdin.toml` (`[package]` with the name and version
  `0.1.0`), `src/main.bd` (a hello-world `main`), and a `.gitignore` covering `target/` and the executable
  `burdin build .` leaves in the package directory. The package name is the last component of the path and must be an
  identifier that is not a reserved word and is not `std`; missing parent directories are created.
- It never overwrites: an existing directory (empty or not), file, or link is refused with exit status 2 before anything
  is written, and a failure while writing removes the directory it created. Usage errors and I/O failures also exit 2.
- `crates/burdin_driver/src/new.rs` has unit tests for the files, names, nesting, and refusals; `tests/check_new.py`
  black-boxes the command (golden contents, the new package builds, runs, and tests, refusals leave existing files
  untouched, invalid names create nothing, the command line) and runs in every CI job.

### `burdin fmt` (issue #229)

- `burdin fmt` formats Burdin source in one opinionated style with no options (`spec/fmt.md`): four-space indentation,
  lines of at most 120 columns, a magic trailing comma that keeps a list one element per line, and blank lines kept (at
  most one in a row). It runs on files, directories, and packages, rewrites only the files that change, and is
  idempotent. `--check` lists the files that are not formatted and exits 1, for CI (issues #229, #463).
- Comments are kept in place: own-line comments stay on their own lines, trailing comments stay at the end of their
  line, and a comment before a closing bracket stays inside it. A comment the grammar gives no line for moves to the end
  of the line; none is dropped or merged (issue #460).
- Breaking follows the width: a bracketed list breaks one element per line with a trailing comma, the last closure or
  literal argument of a call stays on its line, binary operators break before `&&`, `||`, and `orelse` and after the
  others, and a method chain of two calls or more breaks before each `.` (issue #461).
- `use` declarations are merged where the meaning is the same, sorted, and grouped into the standard library, the
  dependencies in `burdin.toml`, and the package, with comments kept; a plain `use a.b` is never merged into
  `use a.{b}` because it may name a module (issue #659).
- The code in the fenced blocks of `///` comments (an empty fence, `burdin`, or `bd`) is formatted as items or as the
  statements of a function; a block tagged with another word, or that does not parse, is left alone (issue #660).
- `--stdin` formats standard input to standard output for editors, `--stdin-filepath` names it and finds its package,
  and `--range lo:hi` formats only the top-level items that overlap a byte range (issue #463).
- The formatter checks itself: its output must parse to the same syntax tree with the same comments, and is laid out
  again until it stops changing, or the file is left alone and the failure reported (issue #462).
- The new crate `burdin_fmt` depends only on `burdin_diag` and `burdin_syntax`; the look-ahead rules the formatter
  shares with the parser moved to `burdin_syntax::lookahead`.
- Tests: `crates/burdin_fmt` (unit tests, `tests/corpus.rs` over every `.bd` file of `std/` and `tests/`,
  `tests/fuzz.rs` with 20,000 mutations in every CI job and a long session in the scheduled `fuzz` job, and
  `tests/regressions.rs`), and `tests/check_fmt.py` with the fixtures in `tests/fmt/`, which every CI job runs.

### `burdin lint` (issue #230)

- `burdin lint [path]` is a linter that is separate from the compiler (new crate `burdin_lint`, called only by the
  driver). It checks the program the compiler already accepted and reports lints `L0001`-`L0017` (plus `L0000` for an
  unknown lint name in a configuration or a suppression comment); `docs/lints.md` documents each one with an example
  that `tests/check_lint.py` runs, and `spec/lint.md` (LN1-LN32) is the behavior.
- Naming conventions: `non_snake_case`, `non_pascal_case`, `non_screaming_case`, with the suggested name in a note
  (issue #233). Syntax-tree lints: `else_on_local_enum` (an `else`/`_` arm that hides a new variant of an enum of the
  same package, with the explicit-variants form as the way to ignore it; issue #231), `unused_imports`,
  `unused_variables`, `unused_parameters`, `unreachable_code`, and `dead_code` for `priv` items (issue #465).
- Typed-tree lints: `eager_chain` (a long chain of eager list methods that builds a list per step suggests `.iter()`;
  issue #232), `redundant_clone`, `unused_mut`, `large_copy_in_loop`, `borrow_could_own`, `loop_invariant_alloc`, and
  `needless_to_list` (issue #661).
- Levels are `allow`, `warn`, and `deny`, set per lint or for the `all` and `warnings` groups, in the manifest `[lint]`
  table or on the command line, with `// lint: allow(...)` comments above an item, trailing on a line, or for the whole
  file with `allow-file` (issue #464). A denied lint makes the exit code 1.
- `burdin lint --fix` applies the machine-applicable suggestions in rounds, rechecks that the result still compiles,
  previews the unified diff (`--dry-run` stops there), and writes every file or none (issue #466).
- `tests/check_lint.py` runs the documented examples, the fixtures in `tests/lint/` (naming, `else`, eager chains,
  unused code, configuration, ownership, generated code, generic instances, `--fix` before and after, and the
  package fixtures), the all-or-nothing write, and the manifest and command-line checks; every CI job runs it, and
  `docs/lints.md` ships in the release archive.
- Two small changes to the shared crates support it: a typed `Local` carries its source span, and a diagnostic's
  suggestion can be marked machine-applicable (`Diagnostic::suggest_safe`).

### `burdin doc` (issues #234, #467-#469, #656-#658)

- `burdin doc [path]` writes the documented interface of a package from its `///` comments and `//!` module comments
  (new crate `burdin_doc`, called only by the driver): a self-contained static HTML site (default
  `<package>/target/doc`), versioned JSON (`--output-format json`), or a per-module coverage report (`--coverage`).
  `spec/doc.md` (DC1-DC23) is the behavior, and doc tests remain separate work (issues #250, #650-#652).
- The site has an overview, a page per module, a page per item, a sidebar, cross-links from signatures and doc comments,
  and client-side search. Its stylesheet and search script are compiled into the binary and copied under `static/`, and
  the index is a script (`search-index.js`), so the site loads nothing from another server and works from disk
  (issue #467).
- Doc comments are a Markdown subset (headings, lists, tables, quotes, fenced code, emphasis, links). Text is always
  escaped; a `javascript:` or other unsafe address is dropped. Intra-doc links `[Type]`, `[module.fn]`,
  ``[`Type::method`]``, and the explicit `[text]`-with-a-target and `[text][path]` forms resolve like code paths from a
  comment's module; an
  unresolved link is `D0001` and a link to a `priv` item is `D0002`, both warnings that `--deny-warnings` turns into a
  failure (issue #468).
- `priv` items, fields, and methods are left out by default and shown, marked, with `--private`; `pub use` re-exports
  are listed on the module page and "re-exported by" on the item (issue #469).
- A type's page lists its methods, its operators written as they are written (`a + b`, `-a`), its extension methods
  with the module that declares them and the `use` a caller needs, its derived and implemented interfaces, and the
  methods of impl blocks for a specific instance (`Stack<i32>`) (issue #658).
- The JSON output carries `format_version: 1`; the schema has modules, items, members, implementations, link
  resolutions, spans, and member origins (issue #656).
- `--coverage` prints documented and undocumented items per module, and the `missing_docs` lint (`L0017`, allow by
  default) reports the same items from the same reading of the source (issue #657).
- The output directory is guarded: `burdin doc` refuses a directory that holds files it did not write (status 2), keeps
  a `.burdin-doc` marker, and removes the pages of items that are gone. A package that does not compile is reported and
  not documented (status 1).
- Unit tests in `burdin_syntax::docs` and `burdin_doc`, the fixtures in `tests/doc/` with golden pages in
  `tests/doc/expect/`, and `tests/check_doc.py`, which every CI job runs.

## Unreleased — Stage 2

### Milestone 2.4: asynchronous I/O (issue #220)

- Sockets, file operations, name resolution, and `blocking` extern calls look blocking but park only the calling
  task: its worker runs other tasks while the system works. On the main thread and on `thread.spawn` threads the
  same calls block the thread. There is no `async`/`await` and no second API (`spec/io.md` IO1).
- The reactor is one interface with three implementations: `epoll` on Linux, `kqueue` on macOS, and an I/O completion
  port with `WSAPoll` on Windows. A reactor thread waits for socket readiness or the nearest deadline and wakes the
  waiting task; the event facility, its wakeup, and a small helper pool are compiler runtime primitives
  (`crates/burdin_codegen/src/taskrt/reactor_*.ll`), and the timers and scheduler integration are Burdin
  (`std/src/reactor.bd`). A wait is claimed exactly once, and each descriptor registration is one-shot (issue #222;
  IO2).
- File and directory operations run on the helper pool, so a blocking disk call does not hold a worker. The pool is
  wrapped as `__offload`, which is sound without a `Send` bound because the call does not return before the helper
  is done (issue #224; IO3).
- `extern "C" { blocking fn ... }` marks a C function that may block: the compiler marshals its arguments, generates
  a thunk, and runs it on the helper pool. A `blocking` marker outside an `extern` block, on a variadic declaration,
  or contradicting another declaration of the same symbol is E0290 (issue #223; IO4).
- `std.net` adds `TcpListener`, `TcpStream`, `UdpSocket`, and `SocketAddr`, with blocking-looking `accept`, `read`,
  `write`, `recv_from`, and `send_to`; `TcpStream` implements `Read` and `Write`; read and write timeouts are
  deadlines on the reactor's timer list; `lookup_host` resolves names on the helper pool. Errors are
  `IoError.Net`, and `IoErrorKind` gains the connection, address, and reachability kinds (issues #408, #225; IO5-IO6).
- `time.sleep` inside a task parks on the reactor's timer instead of holding its worker (IO7).
- `tests/s2_io` has 21 tests, including a TCP echo server, UDP, DNS, timeouts, a FIFO that would deadlock without
  the helper pool, and two stress tests that every CI job repeats 20 times in both profiles. `spec/io.md` records
  the decisions IO1-IO8.

### Milestone 2.3: shared state (issue #213)

- `Shared<T>` counts its handles atomically in every program: relaxed increment on `clone`, release decrement on drop,
  and an acquire fence before the last handle frees the value (issue #215; `spec/shared.md` SH1-SH3).
- `std.atomic` adds `AtomicBool`, `AtomicI32`, `AtomicU32`, `AtomicI64`, `AtomicU64`, `AtomicIsize`, and `AtomicUsize`
  with `load`, `store`, `swap`, `compare_exchange`, and the `fetch_*` operations, all sequentially consistent, over new
  compiler builtins. The module is generated by `tests/gen_atomic_std.py` (issue #217; SH7).
- `Mutex<T>` and `RwLock<T>` (`std/src/sync.bd`) guard a value through closures: `m.lock(|v| ...)`, `try_lock`, `read`,
  `write`, `try_read`, `try_write`, `get_mut`, and `into_inner`. A waiting task parks and a waiting thread blocks, a
  waiting writer holds back new readers, and nothing is poisoned because a panic aborts the process (issues #216, #407;
  SH8-SH10).
- `Send` and `Sync` (`std.marker`) are derived from a type's parts and required by `task.spawn`, `thread.spawn`,
  `Scope.spawn`, a task's result, and `Channel<T>`. A value that is not `Send` is E0280, whose message follows the chain
  down to the field or capture that decides it; `unsafe Type: Send { }` declares one by hand, and a marker impl that is
  not `unsafe`, or an `unsafe` impl of another interface, is E0281 (issues #214, #218, #406; SH4-SH6).
- A task's closure carries whether it is `Send`; a raw-pointer dereference view now takes the origin of the storage that
  holds the pointer, so a handle type can return `&T` from a method.
- `tests/s2_shared` has 26 tests (atomics, mutex and read-write lock behavior, waiting with one worker, derived and
  declared `Send`/`Sync`, the E0280 and E0281 errors, leak checks, and stress tests). Every CI job repeats the stress
  tests 20 times in both profiles, which `tests/check_ci.py` checks (issue #219).

### Milestone 2.2: channels and `select` (issue #207)

- `Channel<T>` (`std/src/channel.bd`) is a cloneable handle to a bounded (`Channel<T>.bounded(n)`, `n >= 1`) or unbounded
  (`.unbounded()`) multi-producer, multi-consumer queue. `send` moves a value in and waits while a bounded channel is
  full; `recv` waits while it is empty and returns `None` once it is closed and empty; `try_send` and `try_recv` never
  wait; `close` wakes every waiter; `for v in ch` receives until the close. A `send` on a closed channel hands the value
  back. A task that waits parks and a thread blocks, so channels connect tasks, threads, and the main thread
  (issues #208, #209; `spec/channels.md` CH1-CH5).
- `select { v = rx.recv() => ..., tx.send(x) => ..., default => ... }` waits on several channel operations and runs
  exactly one arm. It is a contextual keyword that the parser rewrites into ordinary code over a few members of
  `Channel`, so `break`, `continue`, `return`, and `?` in an arm mean what they mean elsewhere. The first ready arm in
  source order runs, `default` makes the `select` non-blocking, and an arm guard (`if cond`) switches an arm off, which
  is how a loop stops selecting on a closed channel (issues #208, #210; CH6-CH8).
- A sent value is moved, the channel owns it until it is received, and a value still queued when the last handle goes
  is dropped with it; `Channel<&T>` is E0120 and `ch.send(&x)` is E0003. Using a sent value is E0100 (issue #211; CH4).
- All channel state lives under the scheduler's lock, so a wake-up cannot be lost: a waiter registers before it gives up
  the lock, and a `select` reads a change counter before it looks at its arms (CH10).
- `tests/s2_channels` has 36 tests (producer/consumer, close, bounded and unbounded, `select` with two ready arms,
  `default`, guards, the move and borrow errors, and the stress tests). `tests/run_tests.py --repeat N` runs each test
  N times, and every CI job repeats the channel stress tests 20 times in both profiles, which `tests/check_ci.py` checks
  (issue #212).

### Milestone 2.1: tasks and the scheduler (issue #198)

- `task.spawn(|| ...)` runs a closure as a task on a work-stealing pool of worker threads and returns a `JoinHandle<T>`
  (`join`, `is_finished`; dropping an unjoined handle detaches the task). A captured value moves into its task
  (issues #199, #200).
- The scheduler is Burdin code in `std/src/task.bd`: per-worker queues and parking, stealing one task from the head of
  another worker's queue, wake-ups that cannot be lost, a pool started on first use and sized by `task.set_workers`
  (issues #201, #301-#303). `tests/s2_tasks` has stress, fairness, one-worker, and idle-wake-up tests that run
  repeatedly in CI (issue #304).
- Tasks run on their own 256 KiB stacks with a 64 KiB guard region; running off the end is a clean
  `panic: stack overflow` (issue #202). `task.yield()` puts the caller behind the other ready tasks (issue #203).
- `task.scope(|s| { s.spawn(|| ...) })` starts tasks that may borrow places of the surrounding code and waits for them.
  The exclusivity checker keeps what a task borrows unavailable until the scope body ends: two tasks writing one place,
  or the body touching a borrowed place, is E0111; a task borrowing a local of the body, or a borrowing closure passed
  by name, is E0171 (issue #204; `spec/memory-rules.md` SK1-SK3).
- A panic in a task aborts the process (issue #205). `thread.spawn` runs a closure on an operating-system thread
  (`Thread<T>`: `join`, `is_finished`) (issue #405). `spec/tasks.md` records the decisions TK1-TK12, including that
  the derived `Send` marker waits for milestone 2.3.
- The runtime is thread-safe where tasks reach it: output is written under a lock, the checking allocator's live list
  is locked, and a task-stack overflow is recognized by the signal handler. The task primitives are LLVM IR text linked
  in only by a program that uses them (`crates/burdin_codegen/src/taskrt/`). Code generation emits a closure body only
  when a reachable body creates it, which also shrinks every program that does not use tasks.
- Written for Linux, macOS, and Windows on x86-64 and arm64; run so far only on Linux x86-64 (debug and release).

## Unreleased — Infrastructure

- The Windows stack-overflow guard reserves a 64 KiB stack guarantee (`SetThreadStackGuarantee`) before it registers
  its vectored filter. The system's default left the filter only a few kilobytes once the exception dispatcher had
  run, so the handler could fault before printing `panic: stack overflow`, which made
  `tests/m02_basics/stack_overflow_aborts.bd` flaky on the `windows` CI job; the guard now has the same room the
  Unix alternate signal stack gives its handler.
- Mutable iteration over `List`, arrays, `Map` values, and `Deque` (issue #709).
  - `for x in mut xs` and `xs.iter_mut()` yield `&mut T` (a map yields `(&K, &mut V)`): `*x = ...` rewrites each
    element in place. The collection is borrowed exclusively for the loop, so pushing, inserting, reading it, or
    starting a second mutable loop inside the body is E0111; the collection must be a mutable place (E0113).
    `Deque` declares `iter_mut` in `std`; a user type joins by declaring `iter_mut(mut self)`.
  - The mutable iterators are ordinary iterators, so `enumerate`, `take`, `skip`, and the other adapters apply.
    `zip` and `chain` work over them; `rev` does not (the mutable iterators are not double-ended), and slices
    have no mutable form.
  - The borrow checker now records the match binding and the target of `held = x` as holders of the loans they
    copy, so a view copied out of a loop, or out of `get_mut`, keeps the collection borrowed (E0111).
  - A mutable view `&mut X` satisfies the interface bounds `X` does, like a shared one.
  - The rules are `spec/iterators.md` and `spec/memory-rules.md` IT15; the tests are `tests/m24_stdlib/iter_mut_*`.
- Allocator follow-up (sub-issues #331-#333 of #182): the interface and metadata decision, the global libc allocator,
  and drop/clone glue through a stored allocator are verified end to end.
  - `List.contains`, `Map.get`, `Map.remove`, and `Map.contains_key` borrow their key, as `spec/stdlib.md` says,
    but the inlined implementation spilled a temporary key without recording it for drop, so a temporary `String`
    key leaked. The key is now read through a place: a local stays borrowed, and a temporary drops at the end of
    its statement.
  - `tests/m23_allocators/clone_through_allocator.bd` checks list, map, heap, and `String` clone and drop, plus
    temporary lookup keys, under the checking allocator.
- Differential tests between comptime and compiled code (issue #444).
  - A test item may carry `[differential]`: the compiler runs its body through the comptime interpreter after the
    CT13 rewrite, and `burdin test` requires the compiled test to produce the same `Ok`/`Err` outcome. A different
    outcome, or a compiled run that aborts, fails the test with a note naming both outcomes. Agreement passes even
    when both runs return `Err`; this stage compares only the outcome, because the dynamic error type carries no
    comparable value.
  - The body must be one the interpreter can execute: an impure operation is E0224, a panic is E0222, and an
    exhausted budget is E0225, each reported at the test item. `differential` cannot be combined with
    `should_panic` or `should_fail` (E0170). `[ignore]` and `[timeout(ms)]` still apply.
  - The behavior is in `spec/test-runner.md`. The conformance tests are
    `tests/m10_test_runner/attributes_differential.bd`, `differential_invalid.bd`, and `differential_impure.bd`;
    `tests/check_test_runner.py` runs `tests/runner/differential/`, whose divergent test depends on the
    unspecified `Map` iteration order, proving the check catches a real interpreter/compiled divergence.
- Every check and test tool under `tests/` is Python 3 (`tests/run_tests.py`, `tests/check_*.py`,
  `tests/build_release.py`, `tests/release_notes.py`, `tests/bench_regression.py`, `tests/fuzz.py`, and the shared
  `tests/harness.py`), one implementation for Linux, macOS, and Windows instead of the former shell scripts and the
  `check-tests.ps1` duplicate. The conformance runner keeps the same directives, diagnostics, output order, and exit
  codes, and runs tests in parallel by default (`-j N`; results are still printed in test order). CI invokes Python
  directly on all three platforms. This removes the Git Bash process-spawn cost that dominated the Windows CI job:
  the full suite takes 3 s instead of 31 s on WSL with the same compiler, and `check_docs.py` walks links in-process.
- Audit every E9000 site and assign it to a milestone (issue #84).
  - `spec/e9000-audit.md` lists every remaining `Code::UNSUPPORTED` site with the construct, the owning milestone,
    and the issue that removes it. Each site carries a `// e9000: <id>` marker, and `tests/check_e9000.py` fails
    when the source and the table disagree, when a row is stale, or when a site names a milestone that CI already
    runs. Every CI job runs it.
  - The stage 0 diagnostics that had no future milestone now have permanent codes and compile-fail tests: E0007 (a
    name declared or imported more than once; `duplicate_values`, `duplicate_type`, `duplicate_type_parameter`,
    `duplicate_member`, `duplicate_import`), E0009 (a cyclic or infinitely sized declaration; `constant_cycle`,
    `cyclic_types`), E0131 (invalid `Drop`; `drop_impl_shape`), E0163 (invalid `extern`; `extern_invalid`,
    `extern_signature_conflict`), E0164 (unsupported `repr`; `repr_other`), and E0180 (type arguments on a
    non-generic type; `type_args_on_non_generic`). The registry, `docs/error-codes.md`, and their examples are
    updated.
  - String literal patterns are implemented: a `str` scrutinee compares byte for byte, including slice views, the
    empty string, or-patterns, and guards. The domain is open, so a `match` on strings still needs an `else` or `_`
    arm (`m04_data/string_patterns`, `nonexhaustive_string`).
  - An inclusive slice range (`s[a..=b]`) is implemented; `spec/stdlib.md` already promised it. The slice end is
    checked as `hi < len` before it is advanced, so the maximum inclusive end panics as out of bounds rather than
    wrapping, and a reversed inclusive range (`a[2..=1]`) still panics. `m09_stdlib/slice_inclusive`,
    `slice_inclusive_bounds`, and `slice_inclusive_reversed` witness it. A dead "comparing strings" branch was
    removed.
  - The remaining E9000 sites are assigned to m19 (hand-built built-in interfaces), m20 (constant expressions),
    m21 (the remaining type-system forms), or Stage 1 for the cross-cutting pattern, bound, `where`,
    function-value, and runtime-index move forms.
- Close the stage 0 correctness gaps carried into stage 1 (issue #48, sub-issues #49, #53, #58, #64, #71, #77,
  #85, #86, #87).
  - **View lifetime (#49).** RT5 and the new E0123 reject a view stored in a `let` or an assignment that derives
    from a temporary or from a local of an inner block; static data, parameters, and same-or-enclosing-block locals
    are allowed. `match`/`if let`/`for` bindings are exempt. `burdin_check/src/origins.rs` now tracks parameter,
    local, and temporary origins with block ancestry; the rule is in `spec/memory-rules.md` RT5 and
    `docs/error-codes.md`, with `m06_conventions/view_lifetime_*` covering both outcomes.
  - **Ranges as values (#53).** `a..b` is a `Range<T>` and `a..=b` a `RangeInclusive<T>`: `Copy`, equal, ordered,
    hashable, cloneable, droppable, storable in fields and containers, and iterable with `for i in r` without
    borrowing the range. Endpoints are integers of one type; `..` has the lowest precedence. The type and its
    structural behavior are in `spec/grammar.md`, `spec/stdlib.md`, and `Ty::Range`; open-ended forms (`..b`, `a..`,
    `..`) do not exist. `range_values` (storage, clone, equality, hash, Map key, iteration),
    `range_endpoints_are_integers`, and `range_forms` (loop forms and the inclusive maximum) witness it.
  - **Move checker (#58).** Constant array and `List` indices, heap pointee fields (`(*h).a`), `match *h`, and a
    moving pattern with a guard on a temporary scrutinee are tracked instead of E9000. A temporary scrutinee is
    moved into a compiler-made local, so its other fields drop once and a failed guard restores the arm's moves. An
    index expression is evaluated once; a tracked move uses a constant index. A runtime index is still E9000.
    `spec/memory-rules.md` MV6/MV8 and `m05_ownership/move_index_*`, `index_evaluated_once`, `move_heap_field*`,
    `match_heap_pointee*`, `match_guard_temporary*`, `match_temporary_fields_drop`.
  - **Generic gaps (#64).** A generic static method infers its type arguments from its own parameters; a generic
    type's bounds are checked at instantiation (E0141); a view type argument makes the instance second-class (SC3);
    an impl method is compared with the interface method including `unsafe` (E0003); `==`/`<` work on symbolic
    instances whose bounds provide them; `Self` resolves in expressions, literals, patterns, and type paths. Tests
    in `m07_generics` (`infer_static_method`, `type_bounds_at_instantiation`, `view_instance_*`,
    `impl_signature_mismatch`, `symbolic_derives`, `self_in_expressions`, `self_outside_method`).
  - **Modules (#77).** `pub use` re-exports modules and items, with import resolution iterated to a fixpoint; a
    dependency's non-root modules are hidden unless its root re-exports them; `pub use` of a package-private item
    and a public signature or field naming a package-private type are E0150. `m08_modules/reexports`,
    `reexports_hidden`, `pub_use_private`, `public_signature_leaks`.
  - **C bridge (#71: #72-#76, #489, #490).** `CPtr<T>` gained load/store (`*p`), `.is_null()`, equality, element
    offsets and difference, and casts to/from integers and other pointers, all inside `unsafe`; creating, holding,
    passing, moving, or using a `CPtr` outside `unsafe` is E0161. `CString.clone` deep-copies the buffer.
    Out-parameters work through `&mut x as CPtr<...>`, and the cast ends the loan. `struct Name repr(C)` has the C
    layout and `Type.size()`/`Type.align()`; `...` declares a variadic C function; `extern "C" fn` exports a Burdin
    function under its plain C symbol with the C ABI rules (E0162). Aggregates cross the ABI through `CPtr<T>` only
    (by value is E0162), C `_Bool` is declared `u8`, and C `char` is `CChar`. There are no `union` declarations or
    function-pointer types: a union is bytes behind `CPtr<T>` and a function pointer is stored as `CPtr<CVoid>`
    (`spec/stdlib.md`). Tests: `cptr_operations`, `cptr_requires_unsafe`, `c_out_parameter`, `c_abi_structs`,
    `cstring_clone`, `export_c_abi`, `export_c_abi_invalid`, `export_c_abi_panics`; docs in `spec/stdlib.md`,
    `spec/grammar.md`, and `docs/error-codes.md`.
  - **`CStr` is an alias, not a view type (#83).** `CStr` remains a `std` alias for `CPtr<CChar>`: one type
    under two names, first-class and `Copy`, with no length, origin, or conversion rules of its own. There is no
    implicit conversion between text and a C string in either direction (E0003), `p.to_string()` and every other
    string method are E0002, and the only conversions are the explicit `CString.from(s).as_ptr()` and the
    associated `CStr.to_string(p)`. Every use keeps requiring `unsafe`. The rule and its rationale are in
    `spec/stdlib.md`; `m08_modules/cstr_is_an_alias` (run), `cstr_no_implicit_conversion`, and
    `cstr_is_not_a_str` witness it.
  - **Coverage (#85, #86, #87).** Run and compile-fail tests for `unsafe fn` declarations and unsafe interface
    methods (`unsafe_fn`, `unsafe_fn_call_requires_unsafe`, `unsafe_interface_method`,
    `unsafe_interface_mismatch`), and an audit of the `spec/memory-rules.md` test matrix against the suite, with US
    extended to the new pointer and unsafe-function tests.
  - The suite is 247 tests (245 in release); `tests/check_tests.py`, `check_unit_tests.py`, `check_docs.py`,
    `check_error_docs.py`, `check_diagnostics.py`, `check_codegen_opt.py`, `check_build_flags.py`,
    `check_parallel.py`, `check_test_runner.py`, and `check_debug_info.py` are green, and `cargo fmt --check`,
    `cargo clippy -- -D warnings`, and `cargo test` pass.
- `Map` is an open-addressing hash table (issue #82). The linear scan became linear probing in the map's existing
  three-word buffer: a zero hash marks an empty slot, the table doubles and rehashes at a 3/4 load factor, and a
  removal shifts the rest of the probe cluster back instead of leaving tombstones. Keys still need only `==`: they
  are hashed structurally (integers, floats, text by its bytes, tuples, structs, enums, `Option`, `Result`, and
  ranges), so `Hash` is not required. The API, the unspecified iteration order, and the iteration, drop, and clone
  behavior are unchanged. A 200,000-key insert-and-lookup program drops from 9.8 s to 0.01 s in a release build, and
  the million-key `m09_stdlib/map_many_keys` witness takes 0.07 s; `map_key_types` and `map_remove_and_clear` cover
  aggregate keys, deletion, reinsertion, and `clear`. The implementation is described in `spec/stdlib.md`.
- The release pipeline (issue #452, sub-issues #453-#456): a `Release` workflow builds the six supported targets
  natively (`x86_64`/`aarch64` Linux, macOS, and Windows), smoke-tests `build`, `run`, and `test`, writes
  `burdin-<version>-<target>.tar.gz` archives and SHA-256 checksums, verifies that two checkout paths produce the
  same Linux archive, and publishes only from a `v<version>` tag or an explicit manual `publish`. A manual run
  defaults to a dry run, and the nightly schedule never publishes. `install.sh` and `install.ps1` install a release
  after verifying `SHA256SUMS`, with `BURDIN_VERSION`, `BURDIN_CHANNEL`, `BURDIN_INSTALL_DIR`, and
  `BURDIN_RELEASE_DIR` for the rehearsal path. `burdin --version` gained a `channel:` line (`dev`, `nightly`, or
  `stable`) backed by `BURDIN_CHANNEL` and `BURDIN_VERSION`; the workspace version became `0.1.0`. The version
  scheme (SemVer for the compiler, editions versioned separately), channel policy, breaking-change rule, artifact
  protocol, reproducibility rules, and rehearsal checklist are in `docs/releasing.md`.
  `tests/build-release.sh` builds and packages a target (and proves reproducibility), `tests/release-notes.sh`
  extracts a changelog section and checks its links, and `tests/check-release.sh` guards the workflow, the archive,
  the checksums, both installers, and the metadata in every CI job. No release has been cut.
- Dependency and license audit (issue #449). `Cargo.lock` is committed and pins every external crate by exact
  version, crates.io source, and checksum, and every direct requirement is written `=x.y.z`.
  `tests/dependencies.txt` is the reviewed inventory: one line per external crate with its license and why the
  compiler needs it (18 crates today, all transitive through `inkwell` 0.11 and `llvm-sys` 211). `tests/check-deps.sh`
  fails on an unreviewed crate, a version change, a lockfile without a checksum, a non-exact requirement, a
  non-permissive license, or a license that disagrees with the crate's own metadata. Every CI job runs it after
  `cargo build` with `BURDIN_DEPS_REQUIRE_METADATA=1`; `tests/check-ci.sh` guards the step, its position, and the
  inventory. `docs/dependencies.md` records the policy and the review, including LLVM's host-supplied
  Apache-2.0 WITH LLVM-exception license.

- The platform support matrix and its known limitations are documented in
  [`docs/platform-support.md`](docs/platform-support.md) (issue #457): the tested and untested OS/architecture pairs,
  the runtime features each CI job proves, and the gaps (no PDB and no symbolized backtraces on Windows, the untested
  Linux and Windows arm64 hosts, the untested libc assumptions, and the terminal-only output path). The page is linked
  from this file and the documentation index. `tests/check-platform-support.sh` checks the matrix against
  `ci.yml`, compiles and runs the page's example, and proves binary-safe standard output and file I/O byte-for-byte;
  every CI job runs it.

- Compiler and runtime debug/observability/performance work (issue #421, sub-issues #422-#432):
  `-g` emits source-level DWARF for functions, parameters, locals, and every expression's line and column
  (`tests/check-debug-info.sh`, which also proves a gdb breakpoint and local read); panics print a symbolized
  `stack backtrace:` after the unchanged message line in both profiles; standard output is block buffered (line
  buffered on a terminal) and flushed by `atexit`, `process.exit`, panics, and the stack-overflow handler; the main
  thread has an alternate-signal-stack guard that turns unbounded recursion into `panic: stack overflow`; generic
  glue is shared by structural type shape; drop flags are omitted for locals the move checker proves dead at every
  exit; constant and `0..len` loop indices skip their bounds checks; `--lto`, `--pgo-generate`, and `--pgo-use`
  expose LLVM LTO and PGO; `--strip` and size budgets (128 KiB hello, 512 KiB std-heavy) cover release binaries;
  `burdin --version` prints the version, target, LLVM version, commit, and profile; and modules are parsed in
  parallel with byte-identical diagnostics (`BURDIN_JOBS`). Each has a CI check: `check-debug-info.sh`,
  `check-codegen-opt.sh`, `check-build-flags.sh`, and `check-parallel.sh`.
- `burdin test` gained the runner ergonomics of issue #438 (sub-issues #439-#443, #445). Tests may carry
  `[ignore]`, `[timeout(ms)]`, `[should_panic]`/`[should_panic("text")]`, and `[should_fail]` attributes; an invalid
  attribute is E0170. Tests run concurrently (`-j`/`--jobs`, default: the CPU count) with a deterministic report,
  each in its own fresh temporary working directory with a clean environment and cleaned up afterwards. Output is
  captured and printed only for failures, with `--verbose` to show it always. `-p`, `--workspace`, and `--filter`
  select packages and tests, and dependency tests do not run by default. The rules are in `spec/test-runner.md`;
  `tests/m10_test_runner` adds four conformance tests and `tests/check-test-runner.sh` checks the runner black-box;
  every CI job runs it (issues #438-#443). `tests/check-tests.sh` and `.ps1` now fail when a registered error code
  has no compile-fail test and no documented exception, so a new diagnostic cannot ship untested (issue #445).
  Differential tests between comptime and compiled code (issue #444) remain blocked on the comptime interpreter
  (milestone 20); `spec/test-runner.md` records their intended shape.
- Settle the 16 assumptions in `tests/README.md` (issue #21, sub-issues #22-#40). The decisions are now rules in
  `spec/grammar.md`, `spec/memory-rules.md`, two new spec files (`spec/stdlib.md` for the standard-library surface
  and `spec/derives.md` for the exact derived members and FNV-1a hash values), and `spec/stage0-subset.md` /
  `spec/stage1-subset.md`; `tests/README.md` links each decision to its section. 40 conformance tests witness them.
  Four implementation gaps closed with the decisions: `Drop` rejects a read-only or `own` receiver (E0003),
  `CPtr.null` rejects arguments (E0004), the ordering operators work on `str` and `String`, and an invalid
  `extern "C"` type or convention is E0162.
- Retire the claude.ai copies of the design and stage 0 documents (issue #44): the repository files are canonical and
  say so, the external copies are recorded as historical, and the design document marks its snippets as illustrative.
- The driver contract is documented in a new root `README.md`: `burdin build`, `run`, and `test`, the diagnostic
  format, `--emit-ir`, `--link-lib` (including how `LLVM-C` resolves through `llvm-config`), `BURDIN_CC`, and the
  test output format. The file is the documentation index, and `tests/check-docs.sh` checks its links and the
  documented behavior; every CI job runs it (issue #42).
- Pin the Rust toolchain to `1.100.0` with the `clippy` and `rustfmt` components in `rust-toolchain.toml`, so
  `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings` are reproducible from a clean machine.
  CI installs and verifies that toolchain, and `tests/check-ci.sh` fails if the pin or its verification goes missing
  (issue #19).
- CI gained a `macos` job on `macos-latest` (Apple Silicon): Homebrew's `llvm@21`, the pinned Rust toolchain, and the
  same checks as Linux, including every conformance milestone m01-m11. `tests/check-platform.sh` proves the emitted
  LLVM triple follows the host and that `usize`/`isize` are 64-bit; it runs on every job. `tests/check-ci.sh` guards
  all three jobs. The driver resolves `--link-lib LLVM-C` for Homebrew's `libLLVM.dylib` layout, finds `llvm-config`
  through `LLVM_SYS_211_PREFIX`, and its resolution logic has unit tests (issues #16, #17, #18).
- CI gained a `windows` job on `windows-latest`: the MSVC environment through `ilammy/msvc-dev-cmd`, the official
  LLVM 21 Windows archive, and the same checks as the other jobs, including every conformance milestone m01-m11 and a
  `BURDIN_CC=cl` run. LLVM is linked as a shared library on every platform, because the static Windows archives crash
  in the dynamic-CRT Rust binary; the driver maps `--link-lib LLVM-C` to `LLVM-C.lib` and reports a missing `cl` with
  an actionable message. `process.run` exit codes are covered on both the POSIX and Windows CRTs (issues #12, #13,
  #14, #15).
- Add `cargo bench -p burdin_bench`: generated inputs measure the lexer, the parser, name resolution and type
  checking, and code generation at three sizes, reporting median time per iteration. `--smoke` runs every stage
  once with generous time budgets and is part of CI, and `HANDOFF.md` records the first baseline (issue #41).
- CI caches the cargo registry, `target/`, and LLVM 21 in all three jobs. The cargo cache is keyed by
  `rust-toolchain.toml` and `Cargo.lock`, so a toolchain or dependency change starts fresh; Linux caches the installed
  LLVM tree (including the multiarch `libLLVM` it symlinks to), macOS caches the Homebrew bottle downloads, and
  Windows caches the unpacked official archive. `tests/check-ci.sh` fails if any of those caches goes missing
  (issue #46).
- The front-end crates have focused Rust unit tests next to the conformance suite: the lexer token stream, parser
  recovery, type unification, exhaustiveness, the type and place questions in `burdin_tir`, the origin and move
  state in `burdin_check`, and diagnostics. CI runs `cargo test --workspace` in every job, and
  `tests/check-unit-tests.sh` fails when a front-end crate or one of the four named areas loses its tests
  (issue #45).
- Every CI job runs the finished conformance milestones in the release profile as well as debug, covering release
  code generation; tests marked `//@ debug-only` (the m02 overflow abort test) are skipped. `tests/check-ci.sh` fails
  if a job loses its release step, if the runner stops selecting the profile or skipping debug-only tests, or if no
  test is marked debug-only (issue #47).
- Diagnostics gained presentation and tooling (issues #433-#437). The human format colors the location, severity,
  and code when stderr is a terminal, honors `--color`, `--no-color`, and `NO_COLOR`, and wraps to `--width`,
  `COLUMNS`, or 80; `--diagnostic-format short` is the one-line machine format and `json` is one JSON object per
  diagnostic plus a summary object. Diagnostics carry notes and suggestions with spans and replacement edits
  (E0100/E0110 attach notes; E0114/E0115 attach the suggested `mut` edit). Output is sorted deterministically, and
  `--error-limit` truncates after sorting while still counting every error (unlimited by default). The new
  `docs/error-codes.md` documents every registry code with a compiled example and a fix, `burdin explain <code>`
  prints an entry, and every CI job runs `tests/check-diagnostics.sh` and `tests/check-error-docs.sh`. Unit tests
  cover the formats, color decision, wrapping, JSON shape, ordering, and limit in `burdin_diag` and `burdin_driver`.
- The compiler benchmarks gained regression tracking (issue #446): `cargo bench -p burdin_bench -- --save FILE`
  records a run's medians in a plain-text baseline, and `--baseline FILE` compares a later run with it, reporting
  every case more than `--threshold` percent slower (default 25) and exiting 1. `tests/bench-regression.sh` runs the
  full suite, and every CI job records and compares a baseline kept in `actions/cache`, keyed by the runner OS and
  architecture, the pinned toolchain, and `Cargo.lock`; a main push updates it and a pull request compares against
  the last main run. The comparison is advisory until the runner numbers are stable. `tests/check-ci.sh` fails if
  the step, the script, the cache, or the harness modes go missing, and `cargo test -p burdin_bench` covers the
  baseline format and the threshold arithmetic.
- The lexer and parser are fuzzed (issues #288 and #447). `burdin_fuzz` is a dependency-free, deterministic harness
  on the pinned stable toolchain: `burdin-fuzz-syntax` replays the committed corpus and then mutates it with a fixed
  seed, catching panics and watching for an input that stops making progress. `crates/burdin_fuzz/corpus/syntax/`
  holds the hand-written edge cases and every input that found a panic, and `cargo test --workspace` replays them
  plus 20,000 deterministic mutations in every CI job, so a reintroduced panic fails. A scheduled `fuzz` job runs
  a ten-minute session seeded by the run id (so each run explores new inputs) and uploads any found input as the
  `fuzz-crashes` artifact; committing it with the fix keeps the regression covered. `tests/check-ci.sh` fails if
  the schedule, the fuzz job, the long session, the artifact upload, or the corpus goes missing.

## Unreleased — Stage 1

Planned in `spec/stage1-subset.md`; tracked by milestones m12-m24 and the GitHub roadmap.

### Milestone 23 — allocators, `Arena`, and `Shared` (issue #180, sub-issues #181-#188)

- Decision (#181): an allocator is an ordinary value and an ordinary generic parameter (`A: Allocator`, or
  `own a: impl Allocator`), and `List<T, A = Global>`, `Map<K, V, A = Global>`, and `Heap<T, A = Global>` take
  it as their trailing type argument. `Global` is zero-sized and stored nowhere, so the stage 0 spellings and
  layouts are unchanged; any other allocator is stored as an owned field, given to `new_in` (`spec/allocators.md`,
  AL1-AL10).
- The `Allocator` interface and `Global` (#182): `std/src/alloc.bd` declares `allocate`, `reallocate`, and
  `deallocate` (all `mut self`, by address when stored) and the zero-sized `Global`, whose methods call the
  compiler's unsafe `__alloc`/`__realloc`/`__dealloc`.
- Allocator parameters (#183): `List<T>.new_in(alloc)`, `Map<K, V>.new_in(alloc)`, and `Heap.new_in(alloc, value)`
  move the value into the container; `new` for a container with a stored allocator is E0272 and a non-allocator type
  or value is E0270. Default type parameters (`struct Buffer<A: Allocator = Global>`) are resolved where the type
  is declared, with E0271 for a non-trailing default, a const parameter's default, and a default on a function,
  interface, or alias; a bare inherent or `Drop` impl covers every instance.
- `Arena` (#184): a first-class bump allocator with 4096-byte blocks, `bytes_allocated`, `reset`, and a `Drop` that
  resets it; `reallocate` copies into fresh arena memory and `deallocate` is a no-op. It is not second-class
  because every safe way to get an arena-backed buffer makes the container own the arena.
- `Shared<T>` (#185): a built-in non-atomic reference-counted cell `{count, value}` from `Global`; `.clone()`
  shares it, `.get()` borrows the value, `.strong_count()` counts, and the last handle drops the value and frees
  the cell. It is `Clone` for every `T` and never `Copy`; the cell's value is stored, so a view is E0120. Stage 2
  makes the count atomic and adds `Send`.
- Leak detection (#186): a `burdin test` build allocates through a checking allocator with a 40-byte header (next,
  size, site, magic) and a live list. A double free panics; a test that returns success with live allocations is
  reported as `test leak: <n> allocation(s) leaked, <bytes> byte(s)` with `allocated at <path>:<line>:<column>` for
  each block, and fails. `burdin build`/`run` keep the plain C heap.
- Allocator-aware containers (#187): `List`, `Map`, and `Heap` allocate, reallocate, free, clone, and iterate
  through the stored allocator, and the owning iterators carry it. Dropping a container frees its buffer through
  the allocator and then drops the allocator; cloning requires the allocator to be `Clone`.
- Tests (#188): `tests/m23_allocators/` adds 28 conformance tests (the default spelling and layout, allocator
  parameters and functions, arena-backed `List`/`Map`/`Heap`, arena reset and growth, `Shared` across scopes and in
  structs, default type parameters, owning iterators, cloning, the leak report, and the compile-fail cases of
  E0270-E0272, E0120, and E0224), plus the `tests/runner/leaks/` fixture and the leak checks in
  `tests/check_test_runner.py`. The suite is 577 tests. The checking allocator also exposed a temporary-drop leak in
  the non-consuming `Option`/`Result` methods, now fixed by reading the receiver through a place.
- Documentation: `spec/allocators.md` (new), `spec/grammar.md` (type parameter defaults), `spec/stdlib.md`,
  `spec/memory-rules.md` (DR8a), `spec/test-runner.md`, `spec/stage1-subset.md`, `tests/README.md` (decision 30),
  `docs/error-codes.md` (E0270-E0272), `README.md`, `AGENTS.md`, `CLAUDE.md`, `HANDOFF.md`, and CI's milestone list.

### Milestone 22 — build scripts (issue #173, sub-issues #174-#179)

- Decision (#174): `build.bd` next to a package's manifest is a standalone Burdin program the driver compiles and
  runs before the package's sources are read. The entry package's own script is trusted; a dependency's runs only
  when the entry package's manifest names it in `[build] allow`, because a script is arbitrary code execution. The
  grant is the whole permission: there is no OS sandbox, and only the generated-source directory is tracked
  (`spec/build-scripts.md`, BS1-BS8).
- Run before compilation (#175): the script is compiled in the build's profile, runs with the package directory as
  its working directory and `argv[1]`, the output directory `<package>/target/burdin/<profile>` as `argv[2]`, and
  the driver's directory first on `PATH`; its output goes to the compiler's stderr. A script that does not compile,
  cannot start, or ends non-zero stops the build with E0261. A package without a script builds as before.
- Generated modules (#176): every `.bd` under `<out>/src` is a module of the package under its relative path, with
  the package's own imports and visibility. A generated module that repeats a source module's path is E0262.
- Caching (#177): a content key per package and profile over the script, the manifest, the compiler binary, the
  files the script lists in `<out>/inputs`, the files its `embed` calls read, and the generated outputs. A changed
  or missing input or output reruns; touching does not. The declared inputs join `--emit-deps`.
- Minimal `bindgen` (#178): `burdin bindgen <header> --out <file>` generates `extern "C"` declarations, `repr(C)`
  structs, typedefs, enum constants, and object-like `#define`s. `#include`, function-pointer types, and unions are
  skipped with a comment; the full tool remains stage 3.
- Tests (#179): `tests/m22_build/` adds nine conformance tests (a generated module the package imports, a generated
  `test` block, a granted dependency, the compile-fail cases of E0260-E0262, a failing and a non-compiling script,
  and a bindgen run that calls libc `strlen`), with unit tests for the cache key and the header parser, and
  `tests/check_build_scripts.py` black-boxes the cache, `--emit-deps`, and the `bindgen` command line. The new
  diagnostics are E0260 (invalid build configuration), E0261 (a failed build script), and E0262 (invalid generated
  modules).
- Documentation: `spec/build-scripts.md` (new), `spec/stage1-subset.md`, `tests/README.md` (decision 29, the three
  codes), `docs/error-codes.md` (E0260-E0262), `README.md` (the driver contract and `[build] allow`),
  `CLAUDE.md`, `HANDOFF.md`, and CI's milestone list.

### Milestone 21 — reflection, user derives, const generics, and `embed` (issue #166, sub-issues #167-#172)

- Decisions (#167): reflection is five compile-time members of every type (`Type.name`, `.size`, `.align`, `.fields`,
  `.variants`), `x.{name}` is the field access by a compile-time name, and a user derive is `derive Name { fn ... }`,
  the methods of an impl block that `derives (Name)` registers (`spec/reflection.md` RFL1-RFL10).
- Reflection (#168): sema evaluates the members while it checks a body. `Type.fields` and `Type.variants` are arrays
  of the new `std` structs `FieldInfo` and `VariantInfo`; a `comptime for` over them is unrolled in sema, with the
  loop variable's `name`, `type_name`, `index`, and `arity` folded to literals; `x.{name}` resolves to the field.
  A function without `self` or a variant named like a member is E0231; a member of a type with neither fields nor
  variants is E0230; a computed name that is not a compile-time string is E0232.
- User derives (#169): the `derive` item, imports and `pub use` of derives next to their interface, and the
  generated impl (`Type<A>: Name where A: Name`) on structs and enums, generic or not. The built-in derives are
  unchanged, and a user derive cannot take their names (E0233).
- Const generics (#170): `const N: usize` parameters (integer, `bool`, `char`) on structs, enums, and functions;
  const arguments as literals, names, or `{ expr }`; a new `Ty::Const` so instances, keys, and inference work as for
  types; array lengths (`[T; R * C]`, `[0; N]`) are const expressions over numbers, const parameters, and constants;
  a const parameter is a value in its body and is inferred from an array argument, a generic type, or the expected
  type. E0250-E0252. The `type-array-length` and `array-repeat-length` E9000 sites are gone. An array literal passed
  to a generic function no longer takes the callee's unbound `T` as its element type (a bug const generics exposed).
- `embed` (#171): `embed("path")` is the file's contents as `str` or, with the expected type `[u8; N]`, as bytes,
  relative to the source file. The file is a build input: `burdin build --emit-deps <file>` lists them. A missing
  file is E0240 and a bad path or length is E0241 (`spec/comptime.md` CT15).
- Generic type aliases (#392): `type Pair<T> = (T, T)`, with parameters, bounds, and const parameters, expanded in
  every type position; an alias is the aliased type, so an impl block written through one is E0003
  (`spec/type-aliases.md` AL1-AL6). The `declare-generic-alias` E9000 site is gone.
- Impls for specific instances (#395): the arguments of an impl block are a pattern (`Stack<i32>`, `Pair<i32, B>`,
  `Holder<List<T>>`) and the block covers the instances it matches; the later of two overlapping impls of one
  interface is E0143, and a `Drop` block covers every instance (E0131 for one that names specific arguments)
  (`spec/impls.md` IM1-IM9). The `impl-specific-args` and `impl-builtin-generic` E9000 sites are gone.
- Built-in interfaces on generic types (#401): a derive holds for the instances whose parts allow it, which IM8
  records; writing one by hand stays E9000 under #402 for a plain and a generic type alike.
- Tests (#172): `tests/m21_reflect/` adds 25 conformance tests (a Burdin-written derive, reflection over fields and
  variants, a const-generic matrix, `embed`, generic aliases, impls for specific instances, and the compile-fail cases
  of E0230-E0233, E0240-E0241, E0250-E0252, E0131, and E0143), plus unit tests for the constant evaluator, the
  parser, impl matching, and the front end on source text. The suite is 537 tests.
- Documentation: `spec/reflection.md`, `spec/const-generics.md`, `spec/type-aliases.md`, and `spec/impls.md` (new),
  `spec/comptime.md` (CT15), `spec/grammar.md` (the `derive` item, const parameters and arguments, `x.{name}`, the
  type of an impl block), `spec/stage1-subset.md`, `spec/e9000-audit.md` (five rows removed),
  `tests/README.md` (decisions 26-28, the ten codes), `docs/error-codes.md` (E0143), `README.md` (`--emit-deps`),
  `Burdin Language Design.md`, `HANDOFF.md`, and `CLAUDE.md`.

### Milestone 20 — comptime (issue #157, sub-issues #158-#165 and #305-#308)

- Decision (#158): the comptime engine interprets the checked typed tree; there is no MIR (#159 closed as not
  chosen). The new crate `crates/burdin_comptime` depends only on `burdin_tir` and `burdin_diag` and runs after the
  memory checks and before code generation (`spec/comptime.md`, CT1).
- Syntax and sema: `comptime` is a keyword. `comptime E` and `comptime { ... }`, `comptime name: T` parameters,
  `comptime if`, `comptime for`, and `comptime assert(cond, "msg")` (also as a module item). Comptime code sees only
  constants, comptime parameters, `comptime for` bindings, and its own locals (E0220); a value that crosses into
  run time must be materializable (E0221). A `const` initializer is now any materializable expression, and the
  memory checks also run on const initializers.
- Interpreter (#160, #305-#308): every expression form, with owned values, places for `&mut`/`mut`, moves, user
  `Drop`, the profile's overflow rule, and the runtime's float text, integer parsing, hashing, and layout. Panics
  are E0222, a failed `comptime assert` is E0223, output/files/environment/processes/C are E0224, and the step
  budget (10,000,000, `--comptime-steps N`), call depth 512, and 1024 specializations per function are E0225. A
  constant cycle through calls is E0009.
- Rewriting: constants and comptime expressions become literals, `comptime if` keeps its selected branch, `comptime
  for` is unrolled, and each call to a function with comptime parameters goes to a specialization per argument
  value. `std/src/target.bd` adds `target.os()` and `target.arch()`. The driver runs the compiler on a thread with
  a 1 GiB reserved stack.
- Tests: `tests/m20_comptime/` adds 40 conformance tests, including `interp_*` tests that compare comptime and
  run-time results; `tests/check_build_flags.py` checks `--comptime-steps`. The suite is 512 tests.
- Documentation: `spec/comptime.md` (new, CT1-CT14), `spec/grammar.md`, `spec/stdlib.md`, `spec/stage1-subset.md`
  (decision 6 and the milestone note), `tests/README.md` (layout, E0220-E0225, E0009, and decision 25),
  `docs/error-codes.md`, `spec/e9000-audit.md` (the `const-expression` site is gone), `HANDOFF.md`, `CLAUDE.md`, and
  `AGENTS.md`.

### Milestone 19 — operator overloading and `Index` (issue #150, sub-issues #151-#156)

- Interfaces: `std/src/ops.bd` declares `Add`, `Sub`, `Mul`, `Div`, `Rem`, `BitAnd`, `BitOr`, `BitXor`, `Neg`,
  `Not`, `PartialEq`, `PartialOrd`, `Index<T>`, and `IndexMut<T>` as ordinary interfaces; the prelude makes them
  visible without a `use`. All operands are same-type, a type implements each interface once, arithmetic/unary
  methods borrow their operands, and `PartialEq`/`PartialOrd` provide `ne`/`le`/`gt`/`ge` as defaults. Comparisons
  use new interfaces rather than the derive-only `Eq`/`Ord`, so every stage 0 derive keeps its exact behavior.
- Resolution: the stage 0 built-in rule is always tried first, then a derived `Eq`/`Ord`, then the user type's
  interface impl, then a type parameter's bounds (`SYMBOLIC_CALL`, resolved by monomorphization). A missing
  implementation is the existing E0141 for a struct/enum/type parameter and E0003 for any other type; a failed
  derive keeps E0003. A literal never adopts a user type, so `v + 1` is E0003. `&&`, `||`, `=`, compound
  assignment, and the shifts are not overloadable (compound assignment stays E0001).
- `Index`/`IndexMut`: `Index<T>`/`IndexMut<T>` name the element type, take a `usize`, and return `&T`/`&mut T`, so
  `x[i]` lowers to `*x.index(i)` and `x[i] = v` to `*x.index_mut(i) = v`. A write-place mode selects `index_mut`
  for the left side of `=`, `&mut x[i]`, a `mut x[i]` argument, and an index inside a written place (`x[i].f = v`,
  nested `g[i][j] = v`). The receiver must be a mutable place (E0113); moving a non-`Copy` element out is E0101;
  indexing a user container without `Index`/`IndexMut` is E0141.
- Checker and backend: sema records the interface indices while installing the prelude, lowers operators and
  indexing to ordinary `Call` nodes, and threads a write-place mode through argument checking. The closure capture
  classifier follows calls under a field/index place, so a closure that writes `v[i]` is `FnMut`. The borrow
  checker treats a reference returned by a call and consumed by `*` as a temporary (no result loan) while tracking
  a re-borrow through the call's origin, so `v[i] = v[j] + 1` works and `&mut v[i]` still borrows `v`. Codegen now
  treats every reference dereference as a place, and `clone` reads it without registering a spurious temporary
  drop, so non-`Copy` elements are not freed twice.
- Tests: `tests/m19_operators/` adds 36 conformance tests: a `Vec`-like `+`/`[]` type, generic and nested index
  containers, write-place forms, closure capture, drop-on-assignment, the derived-wins rule, generic bounds, every
  missing-impl and mutability diagnostic, compound assignment, and stage 0 regression tests for
  integer/float/`str`/tuple/derived operators, overflow, division by zero, and shift overflow. A
  `burdin_tir::place` unit test pins that `*call()` is a rootless place. The suite is 472 tests.
- Documentation: `spec/operators.md` (new, OP1-OP13), `spec/stdlib.md`, `spec/stage1-subset.md` (decision 8 and the
  milestone note), `tests/README.md` (layout, the E0141 registry row, and decision 24), `docs/error-codes.md`,
  `spec/e9000-audit.md` (the hand-built built-in interface row moves to the #402 Stage 1 work), `HANDOFF.md`, and
  this file.

### Milestone 18 — extension methods (issue #143, sub-issues #144-#149)

- Declarations: `fn Type.name(self, ...)` at module scope adds a method to any type name in scope, local, imported,
  or built-in, without an impl block. The receiver's type arguments declare the extension's type parameters
  (`fn List<T>.largest(self): T where T: Copy + Ord`) and a `<...>` after the method name adds method-level ones;
  bounds come from inline method parameters and `where` predicates. `self`, `mut self`, and `own self` follow the
  method conventions. `priv` and `unsafe` apply as on any item. The parser recognizes the receiver path before the
  method name without backtracking (`generic_list_end`), and sema registers the declaration as an ordinary function
  whose first parameter is the receiver, monomorphized per instantiation like a generic function.
- Visibility and resolution: an extension is visible in its declaring module and in modules that import it by name
  (`use text.shout`, `use text.{shout}`); importing the module or another item does not bring it, an import is not
  re-exported, `pub use` re-exports it, and `priv` is package-private (E0150). `x.f()` tries the type's methods
  (inherent and interface, including the methods the compiler provides for built-in types) first and a visible
  extension last; the lookup stays local to the calling module.
- Shadowing: an extension whose receiver type already has a method of the same name is unreachable through dot
  syntax; the compiler reports the new warning E0215 at the extension's declaration, and the program still
  compiles. The driver now prints warnings and continues, and `//@ stderr:` works on `run` tests so a warning can be
  asserted.
- Diagnostics: E0216 (an extension without `self` first, type arguments on an intermediate receiver segment, a
  `where` predicate naming a non-parameter, or an import under another name) and E0007 for a second extension with
  the same receiver or a second import of one name. The registry, `docs/error-codes.md` (with the first
  `warning-example` block), and `burdin explain` cover both codes.
- Tests: `tests/m18_extension/` adds 32 conformance tests, including seven extension package targets: extensions
  on `String`, `i32`, `Option<i32>`, a user struct, an enum, a user generic type, and `List<T>`; `mut`/`own`
  receivers; view returns and their lifetime error; imports across modules and packages, a dependency's extension
  and a `pub use` re-export, an imported receiver type, duplicate and aliased imports, and the shadowing warning for
  an inherent, an interface, and a compiler-provided method. Parser unit tests cover the declaration syntax and sema
  unit tests cover receiver matching. The suite is 436 tests.
- Documentation: `spec/extensions.md` (new, EX1-EX8), `spec/grammar.md`, `spec/stage1-subset.md`, `tests/README.md`
  decision 23 and the registry, `docs/error-codes.md`, `tests/run_tests.py`, `HANDOFF.md`, and this file.

### Milestone 16 — errors (issue #126)

- `error enum` declarations: `error` is a keyword followed by `enum`; the declaration takes an ordinary enum's
  permissive layout, package visibility, `derives (...)`, and generics. Every variant carries a required message
  template whose `{...}` placeholders name its payload fields (positional for a tuple variant, by name for a struct
  variant; a unit variant has none), with the full format-specifier grammar. A tuple variant with one field may add
  `from`, which registers a conversion from that field's type.
- Lowering (`crates/burdin_syntax/src/error_item.rs`): an `error enum` becomes the same enum plus `Display` (from
  the templates), `Error` (the `std` marker interface, which requires `Display`), and one `From<Source>` impl per
  `from` variant, all generated as Burdin source before derive expansion. The checker, the memory checks, and code
  generation see only ordinary items, so an error value is moved, dropped, matched, stored, and printed like any
  enum. The new codes are E0210 (an invalid declaration: a `from` that is not a one-field tuple variant, `from` on
  a generic error enum, a placeholder that does not name a payload field) and E0211 (two variants converting from
  one source type).
- The dynamic `Error`: `std` declares `interface Error requires Display { }`, the struct
  `Error { value: any Error, frames: List<TraceFrame>, context: String }`, and
  `TraceFrame { file: String, line: i32, col: i32 }`. `Result<T>` written with one argument means
  `Result<T, Error>`; an error type is boxed implicitly wherever `Error` is expected (a value that does not
  implement `Error` is the ordinary E0003 mismatch there, and E0212 at `?`), so `?`, `Err(...)`, an annotation, and
  `.context(...)` all produce the dynamic error while keeping the original boxed value. `e.is<T>()`,
  `e.downcast<T>()` (which consumes the box), and `{}` inspect and print it.
- `.context(message)`: a compiler-known method on `Result<T, E>` returning `Result<T, Error>`; `Ok` passes through,
  and an `Err` records the message in front of the existing chain, keeps the boxed source, and returns E0212 when
  `E` is not an error type. `Option` has no `context`.
- Traces: a debug build records a `TraceFrame` when an error type converts into `Error` and at every `?` that
  propagates a `Result<..., Error>`, creation first; `.context` keeps the frames it receives. A release build
  allocates none and leaves `frames` empty, with the same `Error` layout in both profiles. `error.frames` is
  public and `TraceFrame` prints as `file:line:col`.
- `fn main(): Result<T>`: the parser renames the function to `__burdin_main_result` and appends a wrapper `main`
  that prints an `Err` with `Display` to standard error and exits 1; `Ok` prints nothing and exits 0. The `Result`
  must be written directly (an alias is E0003), and a `main` with parameters or another return type is still
  E0003.
- Typed test failures: in a `burdin test` build `assert`/`assert_eq` return `Result<(), AssertError>` and build
  `AssertError.Failed`/`FailedEq` with the call's source text, the values (formatted with `Debug`), and the call's
  `path:line:col`; the compiler appends the text and location at the call site, and the generated test wrapper
  prints the error to standard error before the runner reports the failure. A test body is compiled as a function
  returning `Result<()>`, the dynamic error, so the same `?` also accepts a file or parsing error; a normal build
  keeps the panicking assertions.
- Standard library: `std/src/std.bd` declares `Error`, `TraceFrame`, the `context`/`is`/`downcast` helpers, the
  `IoError` (`Read`/`Write`/`Run`), `ParseIntError` (`Invalid`), and `AssertError` error enums, and
  `__test_assert`/`__test_assert_eq`. `read_to_string`, `write_file`, `process.run`, and the integer parsers
  return the real error variants with their messages; the compiler primitives behind them produce a placeholder
  that the `std` wrappers replace. `orelse` accepts a closure handler: `|e| ...` for a `Result` and `|| ...` for
  an `Option` (issue #488).
- Tests: `tests/m16_errors/` adds 25 conformance tests covering `error` declarations and messages, `from` and `?`,
  the dynamic error and downcasts, `context` chains, debug traces, `main(): Result<T>`, typed assertions, the
  standard-library errors, the `orelse` handlers, and every new diagnostic. The suite is 379 tests (all pass in
  debug with one host skip; release passes all except the debug-only tests).
- Documentation: `spec/errors.md` (new), `spec/grammar.md`, `spec/stdlib.md`, `spec/test-runner.md`,
  `spec/stage1-subset.md`, `std/README.md`, `tests/README.md` decision 21 and the registry, `docs/error-codes.md`,
  `HANDOFF.md`, and this file.

### Milestone 17 — default and named arguments (issue #137, sub-issues #138-#142)

- Default arguments: a named parameter may be written `name: Type = expr`. The default is checked once at the
  declaration, against the parameter's type and in the declaration's module scope, and must be a constant
  expression (a literal, another constant, an operator on constants, or a cast). It is materialized at every call
  that omits the argument, so no state is captured and no code runs at the declaration. A default cannot depend on
  a type parameter, cannot be on a `mut` parameter (the callee writes to the caller's place), cannot appear in an
  interface method signature (shared by every implementation) or an `extern` declaration, and the parameters with
  defaults must be trailing. `burdin_sema` stores the checked expression in `ParamSig.default`; call checking
  inserts it.
- Named arguments: `f(name: value)` matches a parameter by name; positional arguments fill the leading parameters
  in order, named ones may be reordered, and a positional argument may not follow a named one. The receiver is not
  a name. Names work on functions, methods, associated functions, interface methods (static and through `any`),
  and generic functions and methods (matching happens before inference). Built-in functions and methods,
  `Some`/`Ok`/`Err`, variant constructors, closure values, and `extern` declarations take no named arguments. A
  missing required argument stays E0004.
- Evaluation order: every argument is evaluated left to right in parameter order, and a default in the position of
  the parameter it fills, so a reordered call runs in the callee's parameter order.
- Diagnostics: E0213 (invalid default: placement, `mut`, interface, `extern`, type parameter, or a non-constant
  expression) and E0214 (invalid named argument: unknown name, parameter named twice, positional after named, or a
  name on a callee that takes none). The registry, `docs/error-codes.md`, and `burdin explain` cover both.
- Tests: `tests/m17_arguments/` adds 25 conformance tests (defaults omitted and given, named arguments reordered,
  named `mut`/`own` arguments, parameter-order evaluation, a `str` view default, interface and generic calls, and
  every new diagnostic). Parser unit tests cover the default and named-argument syntax. The suite is 404 tests.
- Documentation: `spec/arguments.md` (new), `spec/grammar.md`, `spec/stage1-subset.md`, `tests/README.md`
  decision 22 and the registry, `docs/error-codes.md`, `HANDOFF.md`, and this file.

### Milestone 12 — closures (issue #88, sub-issues #89-#97)

- Closure expressions: `|a, b| expr`, `|a: i32|: bool { ... }`, and `||` (zero parameters), parsed at primary
  precedence and callable with the ordinary `f(args)` syntax. Parameter types come from annotations or from an
  expected closure type; the return type is written after the closing `|` or inferred from the body, and must be
  written when the body uses `return`.
- Closure interface types `Fn(A, B): R`, `FnMut(...)`, and `FnOnce(...)` in types, parameters, fields, containers,
  returns, and as generic bounds (`fn apply<F: Fn(i32): i32>(f: F, x: i32)`), monomorphized per closure type. A
  closure value may be used at a lower capability (`Fn` as `FnMut` as `FnOnce`), never the other way.
- Capture analysis: a closure captures the whole variables it uses and does not bind. It borrows when used in place
  and moves when an owned closure type is expected (annotation, field, element, return, or closure-interface
  parameter); a `Fn`/`FnMut` bound keeps its concrete borrow captures. Borrow captures are shared or mutable
  according to how the body writes them; a `FnOnce` bound or a move context captures by move. A closure that moves a
  captured non-Copy value is `FnOnce`, one that writes a capture is `FnMut`, otherwise `Fn`.
- Function values: a non-generic function name (including a built-in function) or `Some` used where a closure type
  is expected becomes a captureless `Fn` closure that forwards its owned arguments to the function with the
  function's own conventions. A function returning a view is E0171 and an `unsafe fn` is E0161; a generic or
  `extern` function as a value stays E9000 (Stage 1). The E9000 audit rows owned by m12 are removed.
- Memory rules CL1-CL11 in `spec/memory-rules.md`: borrowed captures are second-class, and so is a closure that
  moves a view into its record; neither can escape (E0171, registered in `tests/README.md` and
  `docs/error-codes.md`); their loans live until the closure's last use (E0111); `Fn` calls borrow, `FnMut` calls
  borrow exclusively (E0113), and `FnOnce` calls move the closure (E0100); moving a read-only parameter into a
  stored closure is E0101.
- Representation and code generation: a closure value is `{ code, env, drop }`; the capture record is heap-allocated
  and holds pointers for borrow captures and values for move captures. A body function takes the record and the
  call's consumed flag first, the record's destructor drops moved-in captures, and a `FnOnce` body frees the record
  it consumes. A `Fn`/`FnMut` value called through a weakened `FnOnce` type destroys the record after the call,
  including on an early `return`. Calls are indirect through the code pointer; drop and clone glue, `Option`/`Result`,
  arrays, `List`, and `Map` handle closure values. Fixed a latent array drop/clone GEP that strided by the whole
  array instead of one element (found by an array of closures).
- Tests: `tests/m12_closures/` adds 45 conformance tests (run, `burdin test`, and compile-fail) covering borrow and
  move captures, view captures, stored and returned closures, generic `Fn`/`FnMut`/`FnOnce`, weakened capabilities,
  function values, fields, arrays, lists, nesting, `self`, branches, drops, and every new diagnostic. The suite is
  292 tests (290 in release).
- Documentation: `spec/grammar.md` sections 1, 4, and 6, `spec/memory-rules.md` section 12, `spec/stage1-subset.md`,
  `spec/e9000-audit.md`, `tests/README.md`, `docs/error-codes.md`, and `HANDOFF.md`.

### Milestone 13 — iterators (issue #98, sub-issues #99-#108)

- Generic interfaces and generic methods in sema: `interface Name<T>` declares type parameters and bounds, an impl
  supplies the interface's arguments (`Type: Iterator<i32>`), and a method may have its own parameters
  (`fn map<U, F: FnMut(T): U>`). Each call instantiates the method for the types it infers, with method parameters
  first so a default method's `F` wins over an impl parameter of the same name. `Fn` bounds infer the parameters
  inside them (`F: FnMut(T): U` gives `U` from the closure), and a `where` clause is checked with the impl's own
  arguments substituted. A generic `From` method is E0003.
- The protocol in `std`: `interface Iterator<T> { fn next(mut self): Option<T> }` and the marker
  `interface Iterable<T> { }`, with lazy default methods `map`, `filter`, `take`, `skip`, `fold`, `count`,
  `any_item` (the reserved `any` is spelled `.any` at the call), `all`, `find`, and `to_list`. The adapter types
  `MapIter`, `FilterIter`, `TakeIter`, and `SkipIter` are generic structs whose `next` forwards to the source and
  the closure. Default methods are instantiated on demand for each receiver type, so a chain never instantiates
  every adapter's own adapters.
- Built-in implementations in the compiler: `iter()` and `into_iter()` on arrays, slices, `List`, `Map`, `Chars`,
  and ranges, and the compiler-known iterator types `ArrayIntoIter<T, N>`, `ListIntoIter<T>`, `MapIter<K, V>`,
  `MapIntoIter<K, V>`, and `RangeIter<T>`. A borrowing collection yields `T` when it is `Copy` and `&T` otherwise; a
  map yields `(&K, &V)` and `MapIntoIter` yields owned `(K, V)` and clears each entry it moves; `Chars.next`
  decodes one UTF-8 scalar value; an inclusive range iterator stops at its upper endpoint without computing
  `upper + 1`. `it.sum()` is compiler-known for integer items, and `next` on a non-mutable place is E0113.
- `for` over a user type or an iterator value lowers to `let mut __iter = e; loop { match __iter.next() { Some(pat)
  => body, None => break } }`. An `Iterable<T>` is iterated through `iter()` and its result must be an
  `Iterator<T>` (E0141); a type that is both an `Iterable` and an `Iterator` is iterated through `iter()`, because
  `for` borrows its source. The pattern must match every item (E0006). The stage 0 built-in loop forms and their
  rules are unchanged.
- Eager `List` methods `map`, `filter`, `fold`, `any`, `all`, and `find` in `std`: they borrow the list, pass the
  closure a view of each element, and return owned results (`filter` and `find` clone the element).
- Memory rules IT4-IT12 in the new `spec/iterators.md`: a chain is second-class (E0120, E0123), a view-typed
  parameter may be returned (RT2 now covers an `own` parameter whose type is a view), and a return-origin union
  counts only values that carry a view. Temporary drop flags (`tmp.live`) make a temporary created on one branch of
  a `match` drop only where it exists.
- Tests: `tests/m13_iterators/` adds 21 conformance tests (run and compile-fail) covering `for` over a user type,
  `iter`/`into_iter`/`next`, list, array, slice, map, `Chars`, and range iteration including the inclusive maximum,
  the lazy adapters and their laziness, the eager `List` methods, `sum`, generic iterator/iterable functions, and
  the second-class rejections. The suite is 313 tests (311 in release).
- Documentation: `spec/iterators.md` (new), `spec/memory-rules.md` section 7, `spec/stdlib.md`,
  `spec/stage1-subset.md`, `spec/e9000-audit.md`, `tests/README.md` decision 18, `CLAUDE.md`, and `HANDOFF.md`.


### Milestone 14 — formatting (issue #109, sub-issues #110-#116)

- The decision (`spec/format.md`, issue #110): format strings are a compile-time construct. A string literal's
  placeholders are checked against the call's arguments and each value's type when the program is compiled, and the
  whole string lowers to ordinary `Formatter` calls; nothing is parsed at run time. The page fixes the forms
  (`{expr}`, `{name}`, `{}`, `{n}`, `{expr:spec}`), the escapes (`{{`, `}}`), raw strings (`r"..."`, `r#"..."#`),
  multi-line literals, the specifier grammar, the output format of `Display` and `Debug`, floats, and the radixes.
- Lexer and parser (issue #111): a string literal lexes to a `Str` or an `Interp` token whose pieces are literal
  text and holes; each hole carries its own token stream and the raw specifier with its span, so a diagnostic points
  inside the placeholder. A placeholder's expression may hold brackets, character literals, and string literals, and
  the specifier starts at the first `:` outside brackets. An unterminated `{` or a stray `}` is E0001, reported once
  the string is known to end, and `r"..."` / `r#"..."#` are raw. The AST keeps each placeholder's parsed expression,
  the parsed `FormatSpec`, and spans; `crates/burdin_syntax/src/parser/format.rs` parses the specifier.
- `Display` and `Debug` (issue #112): `std/src/fmt.bd` declares the two interfaces, the `Formatter`, the built-in
  helpers (`__fmt_i64`, `__fmt_u64`, `__fmt_float`, `__fmt_str`, `__fmt_char`, `__fmt_bool`, `__fmt_*` debug forms,
  and the generic `__debug_option`/`__debug_result`/`__debug_list`/`__debug_heap`/`__debug_tuple2..4`), and the
  `__float_str` runtime binding. The compiler implements `Display`/`Debug` for integers, floats, `bool`, `char`,
  `str`, `String`, and `Debug` for `()`, tuples of 2 to 4, `Option`, `Result`, `List`, `Heap`, and `&T`, without an
  impl block. `derives (Debug)` generates the impl block as Burdin source at parse time
  (`crates/burdin_syntax/src/derive.rs`), so a derived `Debug` and a written one are the same; its diagnostics are
  suppressed in favor of E0130 at the offending field, and `Debug` joins the derive requirement table.
- Lowering (issue #113): an interpolated literal, `format(...)`, and `print`/`println`/`eprint`/`eprintln` with more
  than one argument lower to one block that builds the text in a `Formatter`. A user type writes to the formatter
  with `x.fmt(mut f)` / `x.debug(mut f)`; a specifier becomes `set_spec(...)` before the value and `reset_spec()`
  after it. `x.to_string()` formats any type that implements `Display` through the same path (floats included).
  A single-argument `println(text)` is unchanged, so `String`-to-`str` and existing programs behave as before.
- Specifiers (issue #114): width, fill, alignment (`<`, `^`, `>`), `+`, `#`, `0`, precision, and the `x`/`X`/`b`/`o`
  radixes, with `Formatter::pad`, `pad_plain`, and `pad_number` applying them. A signed value in a radix shows its
  two's complement in its own width. Floats are the shortest round-trip text (`snprintf("%.*e")` read back with
  `strtod`, in a new runtime function), or `snprintf("%.*f")` with a precision; `NaN`, `inf`, and `-inf` print as
  such. Precision counts characters for text and digits for floats.
- Compile-time checking (issue #115): every placeholder in a string is reported, not just the first. A missing
  argument, an argument no placeholder uses, a value that does not implement the interface, a specifier that does
  not apply, and a call with arguments whose format string is not a literal are the new codes E0191, E0192, E0193,
  E0190, and E0194. An argument used by more than one placeholder is evaluated once: a place is borrowed and a
  `Copy` value is copied; anything else is E0191. The code that resolves an ordinary method call was split so that
  an already-checked receiver can be used (`method_call_on`), which the lowering needs.
- Code generation emits only reachable functions (`crates/burdin_codegen/src/emit/reach.rs`): the embedded
  standard library used to be generated into every program, so new `std` code (as `fmt.bd` is) added drop flags and
  bounds checks to unrelated programs and made the IR proportional to the library rather than the program. A
  function nothing reaches now gets a two-line `unreachable` body; the roots are `main`, the tests, exported
  functions, `Drop` implementations, and closure bodies. Fixed with it: a function's prologue parameter stores used
  to inherit the previous function's debug location, which LLVM's verifier rejects under `-g`; the location is now
  cleared at each function start.
- Tests (issue #116): `tests/m14_format/` adds 17 conformance tests — interpolation with expressions, escapes, raw
  and multi-line strings, closures that capture into a placeholder, positional arguments and the shared-argument
  rule, every specifier for integers, floats, and text, a user `Display` (including one that honors `pad`), a
  derived `Debug` (structs, enums, generics, `Heap`), a hand-written `Debug` next to a `Display`, and compile-fail
  tests for every new code and for an unterminated brace. The suite is 330 tests (328 in release). The specs are
  `spec/format.md` (new), `spec/grammar.md`, `spec/stdlib.md`, `spec/derives.md`, and `spec/stage1-subset.md`; the
  registry, `docs/error-codes.md`, CI's milestone list, `tests/README.md`, `CLAUDE.md`, `HANDOFF.md`, and this file
  are updated with them.

### Milestone 15 — `any` and `impl` (issue #117)

- The decision (`spec/existentials.md`, AN1-AN12): `any Interface` is an owned, first-class two-word heap box
  `{data, vtable}` that owns its allocation and drops the boxed value through the vtable. A concrete value is boxed
  implicitly wherever an `any I` is the expected type (annotation, argument, return, field, element, `Some`/`Ok`/
  `Err` payload, match arm); the value must implement `I` (E0141) and be first-class, so boxing a view is E0120.
  `any I` satisfies `I` and every interface `I requires`, is never `Copy`, `Clone`, `Eq`, `Ord`, `Hash`, or
  `Default`, and formats through the vtable as `any Display`/`any Debug`.
- Object safety (E0202): an interface is usable as `any` only when no method has its own type parameters, every
  method has a `self` receiver that is not `own self`, no method mentions `Self` outside the receiver or returns a
  view, and every `requires` interface is object-safe. A generic interface instance (`any Store<i32>`) is allowed
  because the instance fixes its arguments. `impl Interface` is accepted only in a parameter list (E0201 anywhere
  else) and is sugar for one hidden type parameter with the interface as its bound, so dispatch is static and `I`
  need not be object-safe.
- Calls and downcasts: `x.m(args)` on `any I` lowers to an indirect call through the vtable slot, with the
  interface's `mut`/`own`/`unsafe` conventions applied. The compiler-known `x.is<T>(): bool` and
  `x.downcast<T>(): Option<T>` take a member type argument (`x.downcast<Circle>()`, the generic-argument spelling of
  grammar rule 8.2); `downcast` consumes the box, moves the value out on a match, and frees the allocation without
  running the drop again. A primitive's compiler-provided `Display`/`Debug` implementation has no vtable function, so
  boxing it is E0204.
- Code generation emits one private vtable constant per `(T, I)` pair with a program-wide type id, the drop glue, and
  one slot per interface method; calls are indirect through the slot. `is` and `downcast` compare the type id, and a
  downcast transfers ownership out of the allocation.
- Tests: `tests/m15_any/` adds 24 conformance tests (run and compile-fail) covering boxing at every expected
  position, a heterogeneous `List<any Shape>`, dispatch and `requires` methods, generic interface instances,
  `Display` through the box, moves and drops, downcasts (generic targets included), object safety, and the new
  diagnostics E0200, E0201, E0202, and E0204. The suite is 354 tests (352 in release).
- Documentation: `spec/existentials.md` (new), `spec/grammar.md` sections 1, 4, and 6, `spec/stage1-subset.md`,
  `tests/README.md` decision 20 and the error-code registry, `docs/error-codes.md`, the CI milestone list,
  `CLAUDE.md`, and this file.

## Stage 0 — 2026-11-14

- Milestones 1-11 complete: lexer and parser, primitives and control flow, type checking, structs/enums/match,
  moves/Drop/Heap, conventions and references, generics/interfaces/derives, modules/extern/unsafe, and the stage 0
  standard library.
- 134 conformance tests pass in debug; 133 pass in release plus one debug-only test. `process.run` reports the child's
  exit code on the POSIX and Windows CRTs.
- LLVM 21 backend through `inkwell`, linked as a shared library on every platform (`crates/burdin_codegen/build.rs`);
  the runtime is generated LLVM IR calling libc directly.
- `burdin build`, `burdin run`, and `burdin test`; the test runner runs one process per test and reports
  `test result: N passed, M failed`.
- `test` blocks, test-mode `assert`/`assert_eq` returning `Result<(), AssertError>`, and abort recovery.
- `std` embedded in the driver: `String`, `List`, `Map`, `Result` with `?`, file I/O, process/environment helpers,
  `CString`/`CPtr`/`CStr`, and the test-mode assertions.
- The m11 LLVM C API program builds and verifies a function through `extern "C"` and `--link-lib LLVM-C`.
- CI runs on the remote: the `linux` job on `ubuntu-24.04` installs LLVM 21 from apt.llvm.org and passes
  check-tests, fmt, clippy, unit tests, build, and every conformance milestone m01-m11. `tests/check-ci.sh`
  guards the workflow's milestone coverage and prerequisites.
