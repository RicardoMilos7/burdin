# E9000 audit

`E9000` is the temporary "not implemented in this compiler yet" code. It is not in the registry: every error a
program can cause on purpose has a permanent code, and `E9000` marks only a language construct (or a compiler limit
that is not a language rule) whose milestone has not shipped. This page lists every remaining site, the construct it rejects, the milestone that owns it, and the
issue that removes it. `tests/check_e9000.py` keeps the page and the compiler in agreement.

The rule is one sentence: **`E9000` is never used for a construct whose milestone is done.** When a milestone lands,
its sites and their rows here disappear in the same change. The check fails when a site has no row, a row has no
site, a row names a milestone that CI already runs, or a row names no issue. Each site in the source carries a
trailing `// e9000: <id>` comment; the id is the first column below.

The definition of the code in `burdin_diag/src/lib.rs` and the doc comments that mention it are the mechanism, not
sites. Sites removed in this audit (stage 0 diagnostics that had no future milestone) are recorded at the end of the
page.

## Sites

| ID | File | Construct | Owner | Issue |
| --- | --- | --- | --- | --- |
| `move-runtime-index-match` | `crates/burdin_check/src/moves.rs` | Matching a pattern that moves out of a non-`Copy` element through a runtime index (`match xs[i] { ... }`, non-constant `i`) | Stage 1 | #493 |
| `move-runtime-index-place` | `crates/burdin_check/src/moves.rs` | Moving a non-`Copy` value through a runtime index (`let x = xs[i]`, non-constant `i`) | Stage 1 | #493 |
| `declare-generic-interface` | `crates/burdin_sema/src/declare.rs` | A generic interface (`interface Iterator<T> { ... }`) | m13 | #390 |
| `declare-generic-alias` | `crates/burdin_sema/src/declare.rs` | A generic type alias (`type Pair<T> = (T, T)`) | m21 | #392 |
| `declare-where-non-param` | `crates/burdin_sema/src/declare.rs` | A `where` predicate on something other than a type parameter, such as an associated-type projection | Stage 1 | #394 |
| `call-non-callable` | `crates/burdin_sema/src/fncx/call.rs` | Calling a closure value or another non-name expression | m12 | #88 |
| `for-range-literal-pattern` | `crates/burdin_sema/src/fncx/control.rs` | A `for` pattern other than a binding or `_` over a literal range (`for (a, b) in 0..n`) | m13 | #102 |
| `for-range-value-pattern` | `crates/burdin_sema/src/fncx/control.rs` | The same over a stored range value | m13 | #102 |
| `array-repeat-length` | `crates/burdin_sema/src/fncx/expr.rs` | `[value; count]` where `count` is not an integer literal | m21 | #403 |
| `const-expression` | `crates/burdin_sema/src/fncx/item.rs` | A `const` initializer beyond literals, other constants, operators, and casts | m20 | #163 |
| `function-value` | `crates/burdin_sema/src/fncx/literals.rs` | Using a function as a value | m12 | #88 |
| `builtin-function-value` | `crates/burdin_sema/src/fncx/literals.rs` | Using a built-in function as a value | m12 | #88 |
| `some-function-value` | `crates/burdin_sema/src/fncx/literals.rs` | Using `Some` as a function value | m12 | #88 |
| `unsupported-helper` | `crates/burdin_sema/src/fncx/mod.rs` | The shared `unsupported()` helper that reports the `E9000` sites in expression checking; not a construct | helper | - |
| `pattern-constant` | `crates/burdin_sema/src/fncx/pattern.rs` | A constant used as a pattern | Stage 1 | #493 |
| `pattern-or-binding` | `crates/burdin_sema/src/fncx/pattern.rs` | A binding inside an alternative of an or-pattern | Stage 1 | #493 |
| `bound-non-path` | `crates/burdin_sema/src/generics.rs` | A bound that is not a type path (a tuple, reference, array, or other type form) | Stage 1 | #399 |
| `bound-empty` | `crates/burdin_sema/src/generics.rs` | An empty bound path (a defensive branch; the parser cannot produce one today) | Stage 1 | #399 |
| `bound-generic-args` | `crates/burdin_sema/src/generics.rs` | A bound with type arguments (`T: Iface<U>`) | m13 | #390 |
| `bound-drop-from` | `crates/burdin_sema/src/generics.rs` | `Drop` or `From` used as a bound | Stage 1 | #399 |
| `bound-qualified-generic` | `crates/burdin_sema/src/generics.rs` | A qualified bound with type arguments (`T: api.Iface<U>`) | m13 | #390 |
| `impl-where-concrete` | `crates/burdin_sema/src/impls.rs` | A `where` clause on a non-generic impl block | Stage 1 | #394 |
| `impl-builtin-hand` | `crates/burdin_sema/src/impls.rs` | A built-in interface implemented by hand (`Type: Eq { ... }`) | m19 | #402 |
| `impl-interface-path-kind` | `crates/burdin_sema/src/impls.rs` | An interface path that is not a type path | Stage 1 | #400 |
| `impl-generic-interface` | `crates/burdin_sema/src/impls.rs` | Implementing a generic interface | m13 | #390 |
| `impl-specific-args` | `crates/burdin_sema/src/impls.rs` | An impl block for specific type arguments (`Stack<i32> { ... }`) | m21 | #395 |
| `impl-builtin-generic` | `crates/burdin_sema/src/impls.rs` | A built-in interface implemented for a generic type | m21 | #401 |
| `impl-where-non-param` | `crates/burdin_sema/src/impls.rs` | A `where` predicate on something other than a type parameter in an impl block | Stage 1 | #394 |
| `impl-from-generic-method` | `crates/burdin_sema/src/impls.rs` | A generic `from` method in a `From` implementation | m13 | #391 |
| `impl-method-generic` | `crates/burdin_sema/src/impls.rs` | A generic method (`fn map<U>(...)`) | m13 | #391 |
| `instances-limit` | `crates/burdin_sema/src/instances.rs` | The monomorphization limit (the diagnostic fires when a generic type or function instantiates itself without end). This is a compiler safety limit, not a language construct, so it has no feature issue; the limit should get a permanent code when it is next reworked | Stage 1 | #493 |
| `type-array-length` | `crates/burdin_sema/src/types.rs` | An array type length that is not an integer literal (`[i32; N]`, `[i32; R * C]`) | m21 | #403 |

## Why the owners differ

- `m12` (closures) owns function values and calls: the first-class function surface arrives with closures.
- `m13` (iterators) owns generic interfaces and generic methods, because `Iterator<T>`, `Iterable<T>`, and their
  adapters are the first user-written `interface Name<T>` uses.
- `m19` (operators) owns a built-in interface implemented by hand: the operator interfaces are the first built-in
  interfaces a program implements itself, and #402 decides the rule for all of them.
- `m20` (comptime) owns arbitrary constant expressions; #163 unifies `const` evaluation with comptime.
- `m21` (reflection, user derives, const generics) owns the remaining type-system forms: generic aliases, impls for
  specific instances, built-in interfaces for generic types, and const-generic array lengths.
- `Stage 1` is the milestone for the cross-cutting work with no single feature milestone: the remaining pattern
  forms (a constant pattern and a binding shared by or-alternatives), the unsupported bound forms, associated-type
  `where` clauses, and the runtime-index move forms. Their issue links are specific where one exists, and #493 is
  the umbrella.
- A qualified interface path (#400) is grouped with the generic-interface sites it usually appears with (#390): a
  qualified path with type arguments needs both, and the non-path form is the remaining #400 work.

## Removing a site

When a feature lands, in the same change:

1. remove the `E9000` branch (and its `// e9000:` marker) and implement the construct;
2. remove its row from the table above;
3. add the conformance test and the spec rule that the milestone requires.

`tests/check_e9000.py` fails if steps 1 and 2 disagree, or if the row's milestone is the one that just shipped.

## Sites removed in this audit

These were `E9000` uses for stage 0 rules or surface whose milestone was already done. They now have permanent codes
and compile-fail tests, or are implemented; nothing here was assigned to a future milestone.

| Construct | New code | Test |
| --- | --- | --- |
| A type, interface, function, constant, test, type parameter, field, variant, member, or import declared more than once | `E0007` | `m03_typecheck/duplicate_values`, `m04_data/duplicate_type`, `m07_generics/duplicate_type_parameter`, `duplicate_member`, `m08_modules/duplicate_import` |
| A constant that depends on itself | `E0009` | `m03_typecheck/constant_cycle` |
| A type alias cycle or a type that contains itself | `E0009` | `m04_data/cyclic_types` |
| A `Drop` implementation that is not exactly one `drop` method | `E0131` | `m05_ownership/drop_impl_shape` |
| An `extern` block for an ABI other than `"C"` | `E0162` | `m08_modules/extern_invalid` |
| A runtime-reserved `extern` symbol, a generic `extern` function, or a conflicting declaration signature | `E0163` | `m08_modules/extern_invalid`, `extern_signature_conflict` |
| A `repr` other than `repr(C)` | `E0164` | `m08_modules/repr_other` |
| Type arguments on a non-generic type | `E0180` | `m07_generics/type_args_on_non_generic` |
| A string literal pattern, which the grammar's `Literal` already includes | implemented (no code) | `m04_data/string_patterns`, `nonexhaustive_string` |
| An inclusive slice range (`s[a..=b]`), which `spec/stdlib.md` already promises | implemented (no code) | `m09_stdlib/slice_inclusive`, `slice_inclusive_bounds`, `slice_inclusive_reversed` |
| Comparing strings, a branch made unreachable when string ordering landed | removed (dead code) | `m09_stdlib/string_order` |
