# Burdin stage 1 plan

Stage 0 is complete: the Rust compiler implements the core subset, all 133 conformance tests pass, and the memory
model is enforced (`spec/stage0-subset.md`, `spec/memory-rules.md`). Stage 1 grows the **same Rust compiler** with the
rest of the core language, then stops. It is a plan, not a commitment: each milestone gets its tests before its code,
and the plan changes when a decision changes.

Stage 1 does not weaken or replace anything stage 0 built. The memory model, the parameter conventions, second-class
references and views, drop elaboration, monomorphization, and the diagnostics registry stay as they are. The 16
decisions settled for stage 0 (issue #21: call-site markers, the `String`-to-`str` conversion, copyable views,
static literals, pattern bindings and `*h`, `for` borrowing, the standard-library surface, unit `main`, panic
output, binary-safe file I/O, `Drop` syntax, literals and division, declaration layout, temporary drops, derives,
and exclusivity places) are the baseline; a stage 1 feature must extend them, not add a parallel rule. New features
must reuse those rules, not add parallel ones.

## Inclusion rule

A feature is in stage 1 only if one of these is true:

1. It is needed to finish the core language as designed: closures, iterators, formatting, `any`/`impl`, `error`
   declarations, extension methods, default and named arguments, operator overloading, comptime, allocators.
2. A standard-library type or the author's own projects need it to be usable (a `for` loop over a user-defined
   collection, `List.map`, a `Result` returned from `main`, a test that reports a typed failure).
3. It closes a stage 0 gap that would otherwise be a soundness or consistency problem (a view outliving its source,
   ranges as values, the move forms rejected as E9000).

Everything else waits for stage 2 (tasks and concurrency) or stage 3 (tooling and ecosystem), even if it is already
designed.

**Done means:** the stage 1 milestones pass on Linux, macOS, and Windows, the whole stage 0 suite still passes, and
every stage 0 gap listed in "Fixes that ship with stage 1" has a registry code and tests.

## What stage 1 adds

| Area | Included | Weight |
| --- | --- | --- |
| Closures | `|a, b| ...` expressions, typed parameters and returns, capture by borrow or move, second-class chains, passing and returning closures | L |
| Iteration | `Iterable<T>` and `Iterator<T>`, `next`, `iter()`/`into_iter()`, user-defined `for`, lazy `map`/`filter`/`take`/`fold`/`sum`/`count`/`to_list`, eager `List` methods, ranges as values | L |
| Formatting | String interpolation with `{name}` and `{expr}`, escaped `{{`/`}}`, `Display` and `Debug`, format specifiers, generalized `print`/`println`/`format` | M |
| `any` and `impl` | Existential types in signatures, boxing, dynamic dispatch, downcast, object safety rules | L |
| Errors | `error` declarations with payloads and `from`, generated messages, dynamic `Error`, default `Result<T>`, `.context(...)`, traces in debug builds, `fn main(): Result<()>`, typed test failures | L |
| Arguments | Default arguments, named arguments at call sites, not in interface signatures | S |
| Extension methods | `fn Type.name(self, ...)` on any local or imported type, resolution order, conflict diagnostics | M |
| Operators | Overloadable arithmetic, comparison, unary, and `Index`/`IndexMut` through interfaces; built-in operators unchanged | M |
| Comptime | A MIR interpreter, `comptime` blocks/parameters/values, `comptime if`/`for`, `const` evaluation, `comptime assert`, a step budget | L |
| Reflection and derives | `Self.fields`/`variants`/`name`/`size`/`align`, user-written derive bodies, const generics, `embed` | L |
| Build scripts | `build.bd`, dependency grants, generated sources, invoking `bindgen` from the script | M |
| Allocators | Allocator parameters, `Arena`, `Shared<T>`, leak-detecting allocator in tests, allocator-aware containers | L |
| Standard library | `Set`, `Deque`, priority heap, Unicode helpers, path/time/random, formatting; the rest as projects need it | M |

## Milestones

The numbering continues stage 0's; the folder convention and the runner are unchanged. A milestone is done when its
folder and every earlier folder pass in debug and release.

| # | Folder | Scope | Exit |
| --- | --- | --- | --- |
| 12 | `m12_closures` | Closure expressions, capture analysis, calling, storing, returning, passing to functions | A closure borrows and a closure moves; a stored closure outlives its scope; using a captured moved value is rejected |
| 13 | `m13_iterators` | `Iterable`/`Iterator`, `next`, `iter`/`into_iter`, user `for`, lazy and eager chains, ranges as values | `for` over a user type, a lazy chain with no intermediate list, a range stored in a binding |
| 14 | `m14_format` | Interpolation, `Display`/`Debug`, specifiers, `format`, generalized output | A formatted string with named and positional values; a type's `Display` used by `to_string` |
| 15 | `m15_any` | `any` and `impl` in signatures, boxing, dynamic dispatch, downcast | A `List<any>` of mixed types and a downcast that checks its type |
| 16 | `m16_errors` | `error` declarations, `from`, messages, dynamic `Error`, default `Result<T>`, `context`, traces, `main(): Result<()>`, typed asserts | A `?` chain that converts two error types; a printed error with a source chain; a test that fails with a message |
| 17 | `m17_arguments` | Default and named arguments on functions and methods | A call that omits a default; a call with reordered named arguments |
| 18 | `m18_extension` | Extension methods, resolution order, shadowing diagnostics | An extension on `String` called with dot syntax; a method that shadows it |
| 19 | `m19_operators` | Overloadable operators and `Index`/`IndexMut` through interfaces | A `Vec`-like type with `+` and `[]`; a built-in operator still behaving as before |
| 20 | `m20_comptime` | MIR (or the chosen interpreter), `comptime` blocks/params/values, `comptime if`/`for`, const evaluation, step budget | A const table built at compile time; a compile-time assertion; a loop past the budget rejected |
| 21 | `m21_reflect` | Reflection members, user derive bodies, const generics, `embed` | A derive written in Burdin; `Matrix<T, const R: usize, const C: usize>` |
| 22 | `m22_build` | `build.bd`, dependency grants, generated sources, `bindgen` invocation | A build script writes a generated module that the package imports |
| 23 | `m23_allocators` | Allocator parameters, `Arena`, `Shared<T>`, leak detection in tests, allocator-aware containers | A list with an arena allocator; a leaked allocation fails a test; `Shared<T>` outlives a scope |
| 24+ | `m24_stdlib` | `Set`, `Deque`, priority heap, Unicode, paths, time, random, the serialization framework's foundation | The containers and text helpers the author's projects need, each with tests |

Milestones 20 and 21 are the riskiest and may split (for example `m20a_mir`, `m20b_comptime_core`,
`m21a_reflection`, `m21b_derives`). A split is a decision to record in `HANDOFF.md`, not a silent change.

## Fixes that ship with stage 1

These are stage 0 gaps that stage 1 features would otherwise build on. Each gets a registry code, a test, and a spec
update **before** the code, per the working rules.

1. **A view can outlive the storage it borrows from** (HANDOFF problem 11). Decide the rule and its code (for example
   `E0123`): `let r = first_of(Pair { a: 1, b: 2 })`, `let r = { let p = ...; first_of(p) }`, and
   `let t = "x".to_string().as_str()`. The simplest rule that fits the checker: a second-class result may not derive
   from a temporary or from a local that dies before the binding, and a view stored in a local may not outlive the
   statement that created its source.
2. **Ranges as values** (HANDOFF problem 16). `a..b` and `a..=b` become ordinary values of a range type, iterable by
   `for`, storable in fields and collections, and able to appear in patterns. This comes with milestone 13.
3. **The move forms rejected as E9000** (HANDOFF problem 10): moving a non-`Copy` value through indexing or a `Heap`
   dereference, moving out of `match *h`, and a moving pattern with a guard on a temporary scrutinee. Extend the move
   checker instead of rejecting the program.
4. **Generic gaps** (HANDOFF item 4 of the ninth session): inference for a static method of a generic type from its
   own arguments, bounds checked when a generic type is instantiated, a view type as a type argument being
   second-class (SC3), interface method signatures in an impl compared with the interface's, and `==`/`<` on symbolic
   instances in a generic body.
5. **Module and C gaps** (HANDOFF problem 13): pointer intrinsics beyond `null`/`as_ptr` (dereference, null test,
   comparison, arithmetic), C `bool`/`char`/structs/unions/function pointers/varargs, `unsafe` for holding a `CPtr`,
   public signatures using only public types, `pub use`, module visibility, `CString.clone`.
6. **`Self` in expressions** (`Self.new()`, `Self { ... }`) — cheap, and useful once interfaces and impls grow.

## Decisions needed before milestone 12

These are the choices that shape the first three milestones. Settle them in the spec and the tests before writing
code; do not invent them inside the implementation.

1. **Closure representation and types.** The design shows `|a, b| a + b`, borrowing captures, move captures for
   returned or stored closures, and passing closures to `filter`/`map`. Options:
   - anonymous closure types with a built-in family of interfaces (`Fn`, `FnMut`, `FnOnce`) and monomorphization;
   - closures as second-class values only (the simplest, but stored and returned closures need more);
   - all closures boxed (`Heap`-allocated), sacrificing zero cost.

   Recommendation: anonymous types, monomorphized, with `Fn`/`FnMut`/`FnOnce` as compiler-known interfaces; a
   captured-by-move closure that is stored or returned is owned, a captured-by-borrow closure is second-class. The
   capture rules (which variables, by what mode, and when a move is required) need a written rule like the memory
   rules document.

2. **Iterator protocol.** Stage 0 interfaces have no associated types. Options:
   - a generic interface `Iterator<T> { fn next(mut self): Option<T> }` with `Iterable<T> { fn iter(self): ... }`
     (monomorphizes, no new language feature);
   - associated types, which the design's `Iterator` sketch implies and which are a separate language feature.

   Recommendation: `Iterator<T>`/`Iterable<T>` generics for stage 1, and keep associated types on the "when wanted"
   list. Write down whether `next` consumes, mutates, or borrows the iterator and how a lazy chain stays
   second-class.

3. **`any` and `impl`.** Boxed or inline, object-safety rules, downcast syntax, and whether `impl Interface` in a
   parameter is sugar for a generic. Dynamic `Error` in milestone 16 depends on this answer.

4. **Error representation.** How `error enum` lowers (enum with payload fields and a generated message method), how
   `from` is registered (like `Type: From<Source>`), what a trace costs in debug, and the representation of the
   default `Result<T>` (a boxed `Error` behind a pointer, presumably). The `IoError`/`ParseIntError`/`AssertError`
   unit aliases become real error types here. Decided for `main`: `fn main(): Result<T>` in milestone 16 prints the
   returned error, with its context chain, to stderr and exits with a non-zero status; `Ok` exits 0 and prints
   nothing. Stage 0 keeps `fn main()` returning unit (issue #32; `stage0-subset.md`).

5. **Allocators.** The syntax for an allocator parameter, how it propagates through `List`/`Map`/`Heap`/`Arena`, what
   `Shared<T>` is (non-atomic `Rc` or atomic `Arc`; `Send` arrives in stage 2), and how the leak-detecting allocator
   reports the allocation site.

6. **Comptime execution.** The design says comptime reads a MIR so its behavior matches compiled code; stage 0 has no
   MIR. Options:
   - introduce `burdin_mir` at milestone 20 (typed tree to MIR to LLVM), the design's architecture;
   - write an interpreter over `burdin_tir` first and add MIR later;
   - evaluate only the subset of expression forms the tests need, rejecting the rest with E9000.

   Recommendation: decide before milestone 20, not in it. Whatever is chosen, the interpreter must share the memory
   rules with compiled code, so comptime `Drop`/move behavior matches. Differential tests between comptime and
   compiled code (issue #444) run the same test both ways and compare the results; their intended `[differential]`
   attribute and runner behavior are recorded in [`test-runner.md`](test-runner.md#deferred-differential-tests-issue-444),
   and they can be built only after the interpreter exists.

7. **Default and named arguments.** Evaluation order, interaction with `mut`/`own` conventions and exclusivity, and
   the rule that interface signatures cannot declare defaults.

8. **Operator overloading and `Index`.** The interface set and names (`Add`, `Sub`, `Mul`, `Div`, `Rem`, `Neg`,
   `Not`, comparisons, `Index`/`IndexMut`), whether compound assignment is included (currently "when wanted"), and
   how an overloaded operator interacts with the built-in one during type checking.

9. **Extension methods.** Where an extension is visible (imports), resolution after inherent and interface methods,
   and the diagnostic for shadowing.

## Not in stage 1

These are designed but belong to later stages; do not implement them early even when a stage 1 feature would be
easier with them:

- Tasks, `spawn`, `scope`, channels, `select`, `Mutex`, atomics, `thread.spawn`, the scheduler, cooperative
  preemption, and `Send`.
- `burdin fmt`, `lint`, `doc`, `add`, the language server, the lockfile, multiple-package workspaces, the package
  registry, and bindgen as a finished tool (a build script may invoke a minimal `bindgen`, but the generator itself
  is stage 3).
- Declaration generation in comptime (new types or functions), doc tests, `bench` blocks, a custom debug backend,
  fallible allocation, and `i128`/`u128`.
- Self-hosting. The compiler stays in Rust (decided).

## Later stages (roadmap)

### Stage 2: tasks and concurrency

The design's concurrency section. Suggested milestones, written the same way:

| # | Scope | Exit |
| --- | --- | --- |
| 2.1 | Tasks: `spawn(|| ...)`, the work-stealing scheduler in `std`, task stacks with guard pages, `yield()` | Two tasks run on a thread pool; a captured value moves into its task |
| 2.2 | Channels: `Channel<T>`, send/recv, `select` over several channels, closing | A producer/consumer pair through a channel; `select` with two ready cases |
| 2.3 | Sharing: `Shared<T>` (atomic), `Mutex<T>`, atomics, the derived `Send` marker | Shared state behind a mutex; a non-`Send` type rejected across a channel |
| 2.4 | I/O: blocking-looking sockets and files inside tasks, epoll/kqueue/IOCP, `blocking` extern functions | A TCP echo server and a file read, both inside tasks |

A panic in a task aborts the process, as everywhere else. `scope` closures may borrow disjoint places; the
exclusivity checker must learn about them.

### Stage 3: tooling and ecosystem

| Area | Contents |
| --- | --- |
| Tooling | `burdin new`, `fmt` (opinionated, no options), `lint` (the design's warnings), `doc`, `add`, bindgen for C headers, Rust interop through generated C-ABI shims |
| Compiler | A query-based compiler library shared by the CLI and a language server; a lockfile; multiple-package workspaces; cached object files; a possibly faster debug backend |
| Ecosystem | The serialization framework (`Serialize`/`Deserialize` derives and format packages), JSON/TOML/CSV, HTTP client and server, TLS, WebSockets, regex, compression, images, math, database drivers |
| Standard library | `Set`, `Deque`, priority heap, Unicode, paths, time, random, math — some may arrive earlier with `m24` |

### When wanted

`i128`/`u128`, `try_from`, labeled loops, `break` with a value, compound assignment, doc tests, `bench` blocks,
fallible allocation, compiler-inserted preemption checks, declaration generation in comptime, `Set`/`Deque` if they
slip past stage 1, and a package registry.

## Working on stage 1

- Read `tests/README.md` first; the format is the same, and a new language decision gets a numbered entry there
  before it gets a test.
- One milestone at a time, tests first. A milestone is done when its folder and every earlier folder pass in debug
  and release. Extend the CI milestone list in `.github/workflows/ci.yml` only when a milestone is finished.
- Every error gets a code in the registry in `tests/README.md` before the code that reports it. Do not use E9000 for
  a construct that a stage 1 milestone is about to implement.
- Keep the stage 0 checks local (one body, signatures only) and keep the compiler free of LLVM before codegen. If
  comptime needs a MIR, put it between `burdin_tir` and `burdin_codegen`; do not make the other crates depend on it
  before it exists.
- Update `HANDOFF.md`, the design document, and this file when a decision changes, in that order.
