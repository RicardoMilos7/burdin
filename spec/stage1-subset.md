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
3. It closes a stage 0 gap that would otherwise be a soundness or consistency problem. Issue #48 closed the known
   stage 0 gaps in stage 0 itself (view lifetime, ranges as values, the tracked move forms, the generic gaps,
   `pub use` and module visibility, `repr(C)`, and the C pointers and signatures); what remains is listed under
   "Fixes that ship with stage 1".

Everything else waits for stage 2 (tasks and concurrency) or stage 3 (tooling and ecosystem), even if it is already
designed.

**Done means:** the stage 1 milestones pass on Linux, macOS, and Windows, the whole stage 0 suite still passes, and
every stage 0 gap listed in "Fixes that ship with stage 1" has a registry code and tests.

## What stage 1 adds

| Area | Included | Weight |
| --- | --- | --- |
| Closures | `|a, b| ...` expressions, typed parameters and returns, capture by borrow or move, second-class chains, passing and returning closures | L |
| Iteration | `Iterable<T>` and `Iterator<T>`, `next`, `iter()`/`into_iter()`, user-defined `for`, lazy `map`/`filter`/`take`/`fold`/`sum`/`count`/`to_list`, eager `List` methods, ranges as iterable values | L |
| Formatting | String interpolation with `{name}` and `{expr}`, escaped `{{`/`}}`, `Display` and `Debug`, format specifiers, generalized `print`/`println`/`format` | M |
| `any` and `impl` | Existential types in signatures, boxing, dynamic dispatch, downcast, object safety rules | L |
| Errors | `error` declarations with payloads and `from`, generated messages, dynamic `Error`, default `Result<T>`, `.context(...)`, traces in debug builds, `fn main(): Result<()>`, typed test failures | L |
| Arguments | Default arguments, named arguments at call sites, not in interface signatures | S |
| Extension methods | `fn Type.name(self, ...)` on any local or imported type, resolution order, conflict diagnostics | M |
| Operators | Overloadable arithmetic, comparison, unary, and `Index`/`IndexMut` through interfaces; built-in operators unchanged | M |
| Comptime | An interpreter over the typed tree (no MIR, decision 6), `comptime` blocks/parameters/values, `comptime if`/`for`, `const` evaluation, `comptime assert`, a step budget | L |
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
| 13 | `m13_iterators` | `Iterable`/`Iterator`, `next`, `iter`/`into_iter`, user `for`, lazy and eager chains, ranges through the protocol | `for` over a user type, a lazy chain with no intermediate list, `(0..n).iter()` |
| 14 | `m14_format` | Interpolation, `Display`/`Debug`, specifiers, `format`, generalized output | A formatted string with named and positional values; a type's `Display` used by `to_string` |
| 15 | `m15_any` | `any` and `impl` in signatures, boxing, dynamic dispatch, downcast | A `List<any>` of mixed types and a downcast that checks its type |
| 16 | `m16_errors` | `error` declarations, `from`, messages, dynamic `Error`, `Result<T>` = `Result<T, Error>`, `context`, traces, `main(): Result<T>`, typed asserts | A `?` chain that converts two error types; a printed error with a source chain; a test that fails with a message |
| 17 | `m17_arguments` | Default and named arguments on functions and methods | A call that omits a default; a call with reordered named arguments |
| 18 | `m18_extension` | Extension methods, resolution order, shadowing diagnostics | An extension on `String` called with dot syntax; a method that shadows it |
| 19 | `m19_operators` | Overloadable operators and `Index`/`IndexMut` through interfaces | A `Vec`-like type with `+` and `[]`; a built-in operator still behaving as before |
| 20 | `m20_comptime` | The typed-tree interpreter (`burdin_comptime`), `comptime` blocks/params/values, `comptime if`/`for`, const evaluation, step budget | A const table built at compile time; a compile-time assertion; a loop past the budget rejected |
| 21 | `m21_reflect` | Reflection members, user derive bodies, const generics, `embed` | A derive written in Burdin; `Matrix<T, const R: usize, const C: usize>` |
| 22 | `m22_build` | `build.bd`, dependency grants, generated sources, `bindgen` invocation | A build script writes a generated module that the package imports |
| 23 | `m23_allocators` | Allocator parameters, `Arena`, `Shared<T>`, leak detection in tests, allocator-aware containers | A list with an arena allocator; a leaked allocation fails a test; `Shared<T>` outlives a scope |
| 24+ | `m24_stdlib` | `Set`, `Deque`, priority heap, Unicode, paths, time, random, the serialization framework's foundation | The containers and text helpers the author's projects need, each with tests |

Milestone 21 is the riskiest and may split (for example `m21a_reflection`, `m21b_derives`); milestone 20 shipped
whole. A split is a decision to record in `HANDOFF.md`, not a silent change.

Milestone 12 is complete: `tests/m12_closures` and every earlier folder pass in debug and release, and the closure
rules are recorded in `spec/memory-rules.md` CL1-CL11 and `spec/grammar.md` section 4.

Milestone 13 is complete: `tests/m13_iterators` and every earlier folder pass in debug and release, the protocol is
recorded in `spec/iterators.md` IT4-IT12, and the surface is in `spec/stdlib.md`.

Milestone 14 is complete: `tests/m14_format` and every earlier folder pass in debug and release, and the rules are
recorded in `spec/format.md` FM1-FM10 (with the decision that format strings are checked and lowered at compile
time), `spec/derives.md` for `Debug`, and `spec/stdlib.md` for the output functions.

Milestone 15 is complete: `tests/m15_any` and every earlier folder pass in debug and release, and the rules are
recorded in `spec/existentials.md` AN1-AN12 (the decision that `any Interface` is an owned heap box with a vtable,
`impl Interface` is a hidden generic parameter, and `is`/`downcast` are compiler-known). The new diagnostics are
E0200 (not an interface), E0201 (`impl` outside a parameter list), E0202 (not object-safe), and E0204 (a
compiler-provided implementation has no function to dispatch to); `tests/README.md`, `docs/error-codes.md`, and
`spec/grammar.md` are updated with them.

Milestone 16 is complete: `tests/m16_errors` and every earlier folder pass in debug and release, and the rules
are recorded in `spec/errors.md` ERR1-ERR10. The new diagnostics are E0210 (invalid error declaration), E0211
(duplicate `from` conversion), and E0212 (missing error conversion); `tests/README.md`, `docs/error-codes.md`, and
`spec/grammar.md` are updated with them.

Milestone 17 is complete: `tests/m17_arguments` and every earlier folder pass in debug and release, and the rules
are recorded in `spec/arguments.md` AG1-AG7 (the decisions that a default is a constant expression checked at the
declaration and materialized at the call site, that arguments are evaluated left to right in parameter order, that
named arguments are matched to parameter names, and that interface signatures and `extern` declarations take no
defaults). The new diagnostics are E0213 (invalid default) and E0214 (invalid named argument).

Milestone 18 is complete: `tests/m18_extension` and every earlier folder pass in debug and release, and the rules
are recorded in `spec/extensions.md` EX1-EX8 (the decisions that an extension is visible in its declaring module and
in modules that import it by name, that a call tries the type's methods before a visible extension, and that a
shadowed extension gets a warning at its declaration). The new diagnostics are E0215 (a warning: a shadowed
extension method) and E0216 (an invalid extension declaration or import).

Milestone 19 is complete: `tests/m19_operators` and every earlier folder pass in debug and release, and the rules
are recorded in `spec/operators.md` OP1-OP13 (the decisions that the operator interfaces are ordinary `std`
interfaces declared in `std.ops`, with same-type operands and no mixed types; that the built-in operators and the
`Eq`/`Ord` derives always win; that `PartialEq`/`PartialOrd` are new interfaces rather than the derive-only `Eq` and
`Ord`; that `Index<T>`/`IndexMut<T>` name the element type, take a `usize`, and return `&T`/`&mut T`; and that
compound assignment and shifts are not overloadable; issue #277 later made compound assignment take the operator's interface, `spec/compound-assignment.md` CA5). The existing diagnostics E0141 (a missing operator or index
implementation), E0113 (`index_mut` through an immutable place), E0101 (moving out of an indexed element), and E0003
(wrong operand types) cover the new rejections; no new error code was needed.

Milestone 20 is complete: `tests/m20_comptime` and every earlier folder pass in debug and release, and the rules are
recorded in `spec/comptime.md` CT1-CT14 (the decisions that the engine interprets the checked typed tree in its own
crate, `burdin_comptime`, between the memory checks and code generation, with no MIR; that a compile-time value
crosses into the program only when its type is materializable; that each call of a function with `comptime`
parameters is specialized per argument value; that `const` evaluation is the same engine; that comptime is pure and
deterministic; and that each evaluation has a step budget, `--comptime-steps N`). The new diagnostics are E0220
(invalid comptime construct), E0221 (not materializable), E0222 (comptime panic), E0223 (failed `comptime
assert`), E0224 (impure operation), and E0225 (budget, depth, or specialization limit). `comptime` is a keyword
now, and `std` gained the `target` module. The `const-expression` E9000 site is gone.

Milestone 21 is complete: `tests/m21_reflect` and every earlier folder pass in debug and release, and the rules are
recorded in `spec/reflection.md` RFL1-RFL10, `spec/const-generics.md` CG1-CG9, `spec/comptime.md` CT15,
`spec/type-aliases.md` AL1-AL6, and `spec/impls.md` IM1-IM9. It did not split. The decisions: reflection is five compile-time members of every type, resolved by sema while a body is checked,
with `comptime for` over `Type.fields`/`variants` unrolled there (not in `burdin_comptime`), because `x.{name}` needs
the field's name to type; `x.{name}` is the computed field access syntax; a user derive is `derive Name { fn ... }`,
syntax for the methods of an impl block that `struct T: Name` registers for the type; const generics put values in
`Ty::Const` so instances and inference reuse the type machinery, with array lengths evaluated by sema (CG3); and
`embed` is a compile-time file read that records a build input (`--emit-deps`); a generic alias is a template
expanded where it is used and is the aliased type, so an impl written through one is E0003; the arguments of an impl
block are a pattern (`Stack<i32>`, `Pair<i32, B>`, `Holder<List<T>>`) and the impl covers the instances it matches,
with E0143 for the later of two overlapping impls of one interface and E0131 for a `Drop` block that names specific
arguments. The new diagnostics are E0230-E0233 (reflection and derives), E0240-E0241 (`embed`), E0250-E0252 (const
generics), and E0143 (overlapping impls). The `type-array-length`, `array-repeat-length`, `declare-generic-alias`,
`impl-specific-args`, and `impl-builtin-generic` E9000 sites are gone. The one built-in-interface row left is
`impl-builtin-hand` (#402), which applies to a plain type and a generic one alike.

Milestone 22 is complete: `tests/m22_build` and every earlier folder pass in debug and release, and the rules are
recorded in `spec/build-scripts.md` BS1-BS8. The decisions: `build.bd` is a standalone program at the package root,
compiled by the driver in the build profile and run before the sources are read; the package's own script is trusted
while a dependency's runs only when the entry package's manifest names it in `[build] allow` (E0260 otherwise); the
script receives the package directory as its working directory and as `argv[1]`, the output directory
`<package>/target/burdin/<profile>` as `argv[2]`, and the driver's directory first on `PATH`; every `.bd` under
`<out>/src` is a module of the package and a path that repeats a source module is E0262; a script failure is E0261
at the entry source; the cache is a content key per package and profile over the script, the manifest, the compiler
binary, the inputs listed in `<out>/inputs`, the script's `embed` files, and the generated outputs; and
`burdin bindgen <header> --out <file>` is the C declaration generator a script may invoke; the full tool
(`spec/bindgen.md`) landed with stage 3's issue #244 and a script uses it unchanged. A build script is native code
with no OS sandbox; the grant is the trust boundary.

Milestone 23 is complete: `tests/m23_allocators` and every earlier folder pass in debug and release, and the
rules are recorded in `spec/allocators.md` AL1-AL10. The decisions: an allocator is an ordinary generic parameter
(`A: Allocator`, or `own a: impl Allocator`), monomorphized like any other, and a container stores the allocator
value it is given as a field it owns; `Global` is the zero-sized default allocator and is stored nowhere, so
`List<T>`, `Map<K, V>`, and `Heap<T>` keep their stage 0 spelling and layout; a container with a stored
allocator is made with `new_in` and `new` is E0272 there (E0270 for a type or value that is not an allocator);
default type parameters are written on trailing struct/enum type parameters (`struct Buffer<A: Allocator =
Global>`), resolved where the type is declared (E0271 otherwise), and a bare inherent or `Drop` impl covers every
instance; `Arena` is a first-class bump allocator whose blocks are freed by reset and drop, not a second-class
type, because ownership of the arena value keeps every safe buffer alive; `Shared<T>` is a non-atomic
reference-counted cell now (atomic, with `Send`, since milestone 2.3), cloned by sharing and dropped once by its last
owner; and a `burdin test` build allocates through a checking allocator whose live list reports leaks with the
source site the compiler recorded, failing an otherwise passing test. The new diagnostics are E0270, E0271, and
E0272; there is no e9000 site for allocators.

Milestone 24 is in progress. `Set`, `Deque`, `PriorityHeap`, the Unicode character and text helpers, `Path` (Unix and
Windows rules), directories and metadata, time (`Duration`, `Instant`, `SystemTime`, `DateTime`, `Clock`), and `Random`
are implemented and listed in `spec/stdlib.md`. Issue #196 audits their tests: every public function of those types is
called by a conformance test, the gaps found (construction and `extend`, `into_list`, `clear`, `Set` with an allocator,
`Path` formatting, `Instant` arithmetic, the control-character predicates) are filled, and three tests use the types
together (`containers_graph_search`, `containers_comptime`, `roundout_text_and_time`). The milestone closes when issue
#189's remaining sub-issues are done.

## Fixes that ship with stage 1

Issue #48 closed most of the stage 0 gaps in stage 0: the view lifetime rule (RT5/E0123), ranges as values,
the tracked move forms, the generic inference/bounds/signature gaps, `Self` in expressions, `pub use` and the
module rules, `CString.clone`, and the C pointer and signature work (#71). These are what remains; each gets a
registry code, a test, and a spec update **before** the code, per the working rules.

1. **Moves through a runtime index** (HANDOFF problem 10). A constant array or `List` index and a `Heap` pointee
   field are tracked; `let x = xs[i]` with a non-constant `i` is still E9000 because the checker cannot tell which
   element moves. Extend the move checker (or the typed tree) instead of rejecting the program.
2. **C function pointers and unions.** `repr(C)` structs have a decided layout and cross the ABI through `CPtr<T>`;
   function-pointer types and `union` declarations are still missing. `burdin bindgen` (`spec/bindgen.md`, BG6 and
   BG4) declares a function pointer as a `CPtr<CVoid>` and a union as a byte blob, so they remain stage 1/3 work for
   Burdin programs that do not go through the bindings.
3. **Private modules.** Stage 0 has no syntax for a private module. The rule is that a dependency's non-root
   modules are hidden: only its root (`src/<package>.bd` or `src/lib.bd`) is reachable, and it re-exports what it
   wants with `pub use` (`m08_modules/reexports*`). A `priv` marker on a module can refine this in stage 1 if a
   project needs it.
4. **C ABI aggregates by value.** Passing or returning a `repr(C)` struct by value is E0162 because the platform
   coercions (register packing, `sret`) are not implemented. A bindgen target that must call such a C function
   would need that lowering.

## Decisions needed before milestone 15

**`any` and `impl` — settled with milestone 15.** The decision is `spec/existentials.md` AN1-AN12: `any Interface` is
an owned two-word heap box `{data, vtable}`, a concrete value is boxed implicitly wherever an expected type is known,
`any I` requires an object-safe `I` (E0202), `impl Interface` in a parameter list is sugar for a hidden generic
parameter with the interface as its bound, and `x.is<T>()`/`x.downcast<T>()` are compiler-known (the downcast
consumes the box). A primitive's compiler-provided `Display`/`Debug` implementation has no function to dispatch to,
so boxing it is E0204. The tests are `tests/m15_any/`; milestone 14's formatting decision remains in
`spec/format.md`.

## Decisions needed before milestone 12

These are the choices that shape the first three milestones. Settle them in the spec and the tests before writing
code; do not invent them inside the implementation.

1. **Closure representation and types — settled with milestone 12.** Closures are expressions of the
   compiler-known interface types `Fn(A, B): R`, `FnMut(A, B): R`, and `FnOnce(A, B): R`, and a type parameter may
   carry an `Fn`-family bound (`F: Fn(i32): i32`), which is monomorphized like any generic instance. A closure value
   is a boxed capture record with a code pointer and a destructor: first-class, storable, and returnable; the price
   is one heap allocation per closure rather than an inline record, chosen to keep stored, returned, and
   interface-typed closures uniform. A closure captures whole variables: by borrow when it is used in place, by move
   when an owned closure type is expected (a written annotation, a field, a return, or a closure-interface
   parameter). A closure with a borrow capture is second-class and cannot escape; its captures are loans held until
   the closure's last use. The body's use of the captures decides the kind: `FnOnce` if it moves one out, `FnMut` if
   it writes one, else `Fn`; a value may be used at a lower capability. The rules are `spec/memory-rules.md`
   CL1-CL11, the syntax is `spec/grammar.md` section 4, and the tests are `tests/m12_closures/`.
   A non-generic function name, a built-in function name, or `Some` may also be used where a closure type
   is expected: it becomes a captureless `Fn` value that forwards to the function (CL11). A generic or
   `extern` function as a value is still E9000 and belongs to the Stage 1 work.

2. **Iterator protocol.** Stage 0 interfaces have no associated types. Options:
   - a generic interface `Iterator<T> { fn next(mut self): Option<T> }` with `Iterable<T> { fn iter(self): ... }`
     (monomorphizes, no new language feature);
   - associated types, which the design's `Iterator` sketch implies and which are a separate language feature.

   Recommendation: `Iterator<T>`/`Iterable<T>` generics for stage 1, and keep associated types on the "when wanted"
   list. Write down whether `next` consumes, mutates, or borrows the iterator and how a lazy chain stays
   second-class.

   **Update (issue #393).** Associated types are now implemented as a general language feature
   (`spec/associated-types.md`, AT1-AT6: `type Name`, `type Name = Ty`, the three projection forms, object safety).
   The `Iterator<T>`/`Iterable<T>` protocol is unchanged: it stays generic, and `Iterator<T>` is not rewritten with
   an `Item` associated type.

3. **`any` and `impl` — settled with milestone 15.** The decision is `spec/existentials.md` AN1-AN12: the value is
   boxed, object safety is AN2, `impl Interface` in a parameter is a hidden generic, and `is`/`downcast` are the
   downcast spelling. Dynamic `Error` in milestone 16 builds on it.

4. **Error representation — settled with milestone 16.** An `error enum` lowers to an ordinary enum plus generated
   `Display` (from each variant's message template), `Error`, and one `From<Source>` per `from` variant, so an error
   value is an ordinary value for the checker, the memory checks, and code generation. `Result<T>` means
   `Result<T, Error>` (a rule specific to `Result`, not general default type arguments), where the dynamic `Error`
   is `{ value: any Error, frames: List<TraceFrame>, context: String }`; an error type is boxed implicitly where
   `Error` is expected, `.context(message)` prefixes the chain and keeps the boxed source, and a debug build records
   creation and propagation frames while a release build records none. A written `fn main(): Result<T>` prints an
   `Err` with `Display` to stderr and exits 1, and typed `assert` failures carry the condition text, the values, and
   the location. The rules are `spec/errors.md` ERR1-ERR10 and the tests are `tests/m16_errors/`; stage 0 keeps
   `fn main()` returning unit (issue #32; `stage0-subset.md`).

5. **Allocators — settled with milestone 23.** The allocator is an ordinary generic parameter bound by
   `Allocator` (`A: Allocator`, or `own a: impl Allocator`), monomorphized like any other, and `List<T, A>`,
   `Map<K, V, A>`, and `Heap<T, A>` take it as a trailing type argument defaulting to `Global`. `Global` is a
   zero-sized value stored nowhere, so the stage 0 spellings and layouts are unchanged; any other allocator
   (`Arena`) is stored in the container as an owned field, and `new_in` is the constructor that moves it in.
   Default type parameters on trailing struct/enum parameters make `List<T>` mean `List<T, Global>`. `Arena` is a
   first-class bump allocator with 4096-byte blocks and `reset`, not a second-class type: every safe way to get a
   buffer makes the container own the arena. `Shared<T>` is a non-atomic reference-counted cell in stage 1 (the
   `Rc` option), and milestone 2.3 made it atomic and added `Send` and the concurrency rules (`spec/shared.md`). The leak-detecting
   allocator is used by `burdin test` builds only: it records a 40-byte header with the source site the compiler
   assigned to the allocation, panics on a double free, and reports still-live blocks after an otherwise passing
   test (`test leak: ...`, `allocated at path:line:column`), failing it. The rules are `spec/allocators.md`
   AL1-AL10 and the tests are `tests/m23_allocators/`.

6. **Comptime execution — settled with milestone 20 (issue #158): interpret the typed tree.** The engine is
   `burdin_comptime`, an interpreter over `burdin_tir` that runs after the memory checks; there is no `burdin_mir`
   (issue #159 is closed as not chosen, and a MIR stays on the "when wanted" list). The reasons are in
   `spec/comptime.md`, CT1. The options that were considered:
   - introduce `burdin_mir` at milestone 20 (typed tree to MIR to LLVM), the design's architecture;
   - write an interpreter over `burdin_tir` first and add MIR later;
   - evaluate only the subset of expression forms the tests need, rejecting the rest with E9000.

   The second option was taken without the "add MIR later" part. The interpreter shares the memory rules with
   compiled code: it runs only programs the checks accepted, moves where the tree moves, and drops where compiled
   code drops (CT12). Differential tests between comptime and
   compiled code (issue #444) run the same test both ways and compare the `Ok`/`Err` outcome; the `[differential]`
   attribute and the runner's comparison are in [`test-runner.md`](test-runner.md#differential-tests), built on the
   interpreter.

7. **Default and named arguments — settled with milestone 17.** A parameter written `name: Type = expr` may be
   omitted at the call site. The default is a constant expression checked once at the declaration (the function's
   parameters and `self` are not in scope), it cannot depend on a type parameter, and only trailing parameters may
   have one; `mut` parameters, interface method signatures, and `extern` declarations cannot declare one (E0213).
   A call may write `name: value` for a function, method, interface method, associated function, or generic
   function/method; positional arguments fill the leading parameters and may not follow a named one, and an
   unknown or repeated name is E0214. Every argument is evaluated left to right in parameter order, a default in
   the position of its parameter. The rules are `spec/arguments.md` AG1-AG7 and the tests are
   `tests/m17_arguments/`.

8. **Operator overloading and `Index` — settled with milestone 19.** The operator interfaces are the ordinary
   `std` interfaces `Add`, `Sub`, `Mul`, `Div`, `Rem`, `BitAnd`, `BitOr`, `BitXor`, `Neg`, `Not`, `PartialEq`,
   `PartialOrd`, `Index<T>`, and `IndexMut<T>`, declared in `std/src/ops.bd` and visible through the prelude. Each
   takes the same type on both sides and is implemented at most once per type. The stage 0 built-in rule, and a
   derived `Eq`/`Ord`, is always tried first; a user type falls back to its interface implementation, and a type
   parameter to its bounds. Comparisons use new `PartialEq`/`PartialOrd` interfaces rather than the derive-only
   `Eq`/`Ord`, so every stage 0 derive keeps its exact behavior. `Index<T>`/`IndexMut<T>` name the element type,
   take a `usize`, and return `&T`/`&mut T`, which makes `x[i]` a place; a write selects `index_mut` and requires a
   mutable receiver. A literal never adopts a user type (`v + 1` is E0003). Compound assignment and the shift
   operators were not overloadable at this milestone (issue #277 changed compound assignment, CA5). The rules are `spec/operators.md` OP1-OP13 and the tests are
   `tests/m19_operators/`.

9. **Extension methods — settled with milestone 18.** An extension `fn Type.name(self, ...)` is visible in its
   declaring module and in modules that import it by name; importing a module or another item does not bring it.
   A call tries the type's methods (inherent and interface, including compiler-provided ones) before a visible
   extension. A method always wins over a same-named extension, which gets the warning E0215 at its declaration;
   E0216 reports an invalid declaration, and E0007 a duplicate receiver or import. The rules are
   `spec/extensions.md` EX1-EX8 and the tests are `tests/m18_extension/`.

## Not in stage 1

These are designed but belong to later stages; do not implement them early even when a stage 1 feature would be
easier with them:

- Tasks, `spawn`, `scope`, channels, `select`, `Mutex`, atomics, `thread.spawn`, the scheduler, cooperative
  preemption, and `Send`.
- `burdin doc`, `add`, the language server, the lockfile, multiple-package workspaces, and the package registry.
  (`burdin fmt` and `lint` have landed; `burdin bindgen` is `spec/bindgen.md`.)
- Doc tests, `bench` blocks, a custom debug backend, fallible allocation. (Declaration generation in comptime landed
  with issue #280: `spec/generate.md`.) (`i128`/`u128` landed with issue #273; `spec/int128.md`.)
- Declaration generation in comptime (new types or functions), doc tests, `bench` blocks, a custom debug backend.
  (`i128`/`u128` landed with issue #273; `spec/int128.md`. Fallible allocation landed with issue #278;
  `spec/allocators.md`, AL11.)
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

Milestone 2.1 is complete: `tests/s2_tasks` passes in debug and release. The decisions (task representation, stack
size, `Send` timing, panic behavior, scoped borrows, threads) are `spec/tasks.md` TK1-TK12 and the checker rules are
`spec/memory-rules.md` SK1-SK3. The scheduler is Burdin code in `std/src/task.bd` over about two dozen runtime
primitives (`crates/burdin_codegen/src/taskrt/`). It was written for Linux, macOS, and Windows on x86-64 and arm64 and
has been run only on Linux x86-64 so far; the CI jobs for the other platforms are the first run of the rest.

Milestone 2.2 is complete: `tests/s2_channels` passes in debug and release, and its stress tests run repeatedly in CI.
`Channel<T>` is a cloneable handle to a bounded or unbounded multi-producer, multi-consumer queue with an explicit
`close`; a sent value is moved, and a view type cannot be a channel's element (E0120). `select` is a contextual
keyword that the parser rewrites into ordinary code over a few members of `Channel`: the first ready arm in source
order runs, `default` makes it non-blocking, and an arm guard (`if cond`) switches an arm off. The decisions are
`spec/channels.md` CH1-CH10.

Milestone 2.3 is complete: `tests/s2_shared` passes in debug and release, and its stress tests run repeatedly in CI.
`Shared<T>` counts its handles atomically; `std.atomic` has seven atomic types (32- and 64-bit, sequentially consistent);
`Mutex<T>` and `RwLock<T>` guard a value through closures (`m.lock(|v| ...)`), park the waiting task, and cannot be
poisoned; and `Send` and `Sync` are derived from a type's parts, declared with `unsafe Type: Send { }` where the
compiler cannot see why a type is safe, and required by `task.spawn`, `thread.spawn`, `Scope.spawn`, and `Channel<T>`
(E0280, E0281). The decisions are `spec/shared.md` SH1-SH11.

Milestone 2.4 is complete: `tests/s2_io` passes in debug and release, and its stress tests run repeatedly in CI.
Sockets, file operations, name resolution, and `blocking` extern calls look blocking but park only the calling task:
a reactor (epoll on Linux, kqueue on macOS, an I/O completion port with `WSAPoll` on Windows) reports socket
readiness and timers, and a small helper pool runs file operations, name lookups, and the compiler-generated thunk
of a `blocking` extern. The surface is `std.net` (`TcpListener`, `TcpStream`, `UdpSocket`, `SocketAddr`,
`lookup_host`), the reactor is `std/src/reactor.bd`, and the decisions are `spec/io.md` IO1-IO8. IPv6 and an
overlapped per-operation Windows path are later refinements.

### Stage 3: tooling and ecosystem

| Area | Contents |
| --- | --- |
| Tooling | `burdin new`, `fmt` (opinionated, no options), `lint` (landed: `spec/lint.md`), `check` (landed: `spec/check.md`), bindgen for C headers (landed: `spec/bindgen.md`), `doc`, `add`, Rust interop through generated C-ABI shims |
| Compiler | A query-based compiler library shared by the CLI and a language server; a lockfile; multiple-package workspaces; cached object files; a possibly faster debug backend |
| Ecosystem | The serialization framework (`Serialize`/`Deserialize` derives and format packages), JSON/TOML/CSV, HTTP client and server, TLS, WebSockets, regex, compression, images, math, database drivers |
| Standard library | What `m24` did not ship: the serialization framework's foundation and anything the author's projects still need. `Set`, `Deque`, `PriorityHeap`, the Unicode helpers, `Path`, time, and `Random` are in `std` already |

The query-based compiler library has landed (`spec/query.md`, issue #241): the command line runs its front end
through it, and the memo, the tool API, cancellation, and the budget are in place. The lockfile and `burdin add`
have landed too (`spec/packages.md`, issues #238 and #239); cached object files landed too (`spec/target.md`, issue #243); the
package registry is still open. The build tool's command surface is recorded in `spec/build-tool.md` (issue #338),
and named build profiles (`dev`, `release`, `test`, `bench`, and custom) are in `spec/profiles.md` (issue #339).

### When wanted

`TryFrom` and `try_from` have landed for user types and integer types (`spec/stdlib.md`, "Conversions", issue
#274), and fallible allocation has landed for `List`, `Map`, `String`, and the allocator primitives
(`spec/allocators.md`, AL11, issue #278); the remaining items below are still wanted.

`i128`/`u128` have landed too (issue #273, `spec/int128.md`), and so has `break` with a value (issue #276,
`spec/grammar.md` rule 8.10), and compound assignment has landed for the built-in types (issue #277,
`spec/compound-assignment.md`). The remaining items are doc tests, `bench` blocks, fallible allocation,
compiler-inserted preemption checks, declaration generation in comptime, a MIR (decision 6), `Set`/`Deque` if they
slip past stage 1, and a package registry. Compound assignment on a user type (issue #277) takes the operator from its interface, as `a = a op b`.
`spec/grammar.md` rule 8.10). The remaining items are compound assignment, doc tests, `bench` blocks,
fallible allocation, compiler-inserted preemption checks, a MIR (decision 6), `Set`/`Deque` if they
compiler-inserted preemption checks, declaration generation in comptime, a MIR (decision 6), `Set`/`Deque` if they
slip past stage 1, and a package registry.

## Working on stage 1

- Read `tests/README.md` first; the format is the same, and a new language decision gets a numbered entry there
  before it gets a test.
- One milestone at a time, tests first. A milestone is done when its folder and every earlier folder pass in debug
  and release. Extend the CI milestone list in `.github/workflows/ci.yml` only when a milestone is finished.
- Every error gets a code in the registry in `tests/README.md` before the code that reports it. Do not use E9000 for
  a construct that a stage 1 milestone is about to implement. [`e9000-audit.md`](e9000-audit.md) lists every
  remaining E9000 site with the milestone and issue that remove it; `tests/check_e9000.py` fails when the list and
  the source disagree, or when a site names a milestone that has shipped.
- Keep the stage 0 checks local (one body, signatures only) and keep the compiler free of LLVM before codegen. If
  a MIR is ever introduced, put it between `burdin_tir` and `burdin_codegen`; the comptime engine
  (`burdin_comptime`) depends only on `burdin_tir` and `burdin_diag`, like the checks.
- Update `HANDOFF.md`, the design document, and this file when a decision changes, in that order.
