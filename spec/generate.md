# Declaration generation

Issue #280 adds declaration generation to comptime: a module-scope `comptime generate` block is evaluated at compile
time, its value is Burdin source text, and the compiler adds the declarations that text holds to the module. This page
is the exact behavior; the tests are `tests/generate/`. Rule IDs are GN1-GN10. The decision (decision 34 in
`tests/README.md`) is the first slice of the design the issue asked for: source text as the mechanism, names that are
not hygienic, generators that see only hand-written declarations, and structs, enums, and functions as the only
generated items.

## GN1. What generation is

A `comptime generate { ... }` item at module scope runs its block at compile time. The block's value is a `String` of
Burdin source. The compiler parses that text as module items and adds them to the module where the block stands. It
is the only way comptime creates declarations: `spec/comptime.md` CT13 says code generation never sees a comptime
construct, and the declarations this page adds are ordinary declarations once generation is done.

It is not a macro system. The text is an ordinary string, so there is no quasi-quoting and no hygiene (GN6).

## GN2. Syntax and the block

```
struct Point {
    x: i32
    y: i32
}

comptime generate {
    let mut names = String.new()
    comptime for field in Point.fields {
        names.push_str(field.name)
        names.push_str(",")
    }
    format("fn field_names(): String {{\n    String.from(\"{}\")\n}}\n", names)
}
```

- The form is valid only at module scope. A `comptime generate` in a function body, an impl block, a nested block, or
  any other expression position is **E0243**, reported at the `comptime` keyword.
- The block is the body of a function with no parameters and the return type `String`. It sees the module's
  declarations and imports, its own locals, and reflection (`spec/reflection.md`, RFL2-RFL4). Its value must be a
  `String`; any other type is E0003 at the value.
- The block follows the rules of any function body: the memory rules apply, and so do the compile-time rules of
  `spec/comptime.md` CT9-CT11 (purity, panics, and the step budget).
- Every string literal is an interpolated string (`spec/format.md`), so a brace in the generated text is written `{{`
  or `}}`, and a quote is written `\"`.

## GN3. Order of generation

Generation runs in the front end, once per analysis, before the program is checked as a whole. Generators are taken in
module order, and within a module in source order. For the whole set of generators:

1. **Check.** Each generator, and every function it calls, is checked against the hand-written declarations only. No
   generated declaration exists yet, so a generator that refers to one fails with the ordinary unknown-name error
   (E0002). These checks use fresh caches, not the build's check cache (GN8). The errors of this step that count are
   those inside a generator's block and inside any hand-written function it reaches, directly or through calls. The
   other errors of this step are discarded: they come from bodies that refer to generated names, and step 4 reports
   the ones that remain.
2. **Evaluate.** If step 1 reports no error that counts, each generator runs in turn under the compile-time rules. Its
   value is the text it generates (GN7 says what a failing one reports).
3. **Declare.** Each text is parsed (GN4), and its declarations are added to the module of its block, after the
   hand-written declarations. The order of the declarations in a module does not change what they mean.
4. **Check.** The program is checked as a whole, with the generated declarations, and the rest of the pipeline runs as
   usual.

So a generator never sees the declarations another generator makes, and a generated declaration is visible to every
hand-written body, to every other generated declaration, and to other modules through imports, as an item of its
module.

## GN4. The generated text

The text is parsed as the items of a module. The items it may hold are `struct`, `enum`, and `fn` declarations, with
whatever visibility the text writes. An empty text declares nothing and is accepted.

Any other item is E0242 at the block: a `const`, a `use`, an `impl` block, an `interface`, a `derive`, a `type` alias,
a `test` or `bench` item, a `comptime assert`, or a nested `comptime generate`. So is a text that does not parse. The
message gives the parser's message and the line and column inside the text.

## GN5. Names a generator may not use

A generator never sees a generated name (GN3, step 1). A generated name is visible to the hand-written code and to the
other generated declarations, but not to any generator. A reference from a generator to one is the ordinary unknown
name (E0002) at the reference, and it stops generation (GN7).

A generated declaration repeats a name that is already declared in its module, or a name another generated declaration
declares, as any two declarations do: E0007, reported at the block (GN6).

## GN6. Hygiene and spans

Names in the generated text are not hygienic. They resolve in the module where the block stands, as the bodies of a
user `derive` do (`spec/reflection.md`, RFL8; `tests/README.md` decision 26). A generated declaration can therefore name
a hand-written type, and a hand-written name can be shadowed only by an error, never silently.

Every token of the generated text carries the span of the `comptime generate` keyword. So an error in a generated
declaration, a type mismatch, an unknown name, or a duplicate name, is reported at that line with its usual code. Each
such diagnostic is followed by a note naming the declarations the block produces, because the block is the only place
it can point. A parse error in the text is E0242 at the block (GN4).

## GN7. Failure stops generation

If step 1 has an error that counts (GN3), those errors are reported and generation stops. In step 2, a generator that
fails has its compile-time error reported at the block: E0222 for a panic, E0224 for an impure operation, E0225 for the
step budget. The other generators still run, so that each failure is reported, but no declaration is added. Then
generation stops: steps 3 and 4 do not run, and the rest of the program is not checked in that run. A text that fails
step 3 (GN4) is reported the same way, and step 4 does not run.

This is the one place where a run does not report every error in a file, which the rest of the compiler does. It is
needed because the errors of the rest of the program cannot be told apart from errors caused by a generator that
has not run yet. The other errors appear once the generators check.

## GN8. Caching

Generation is part of every analysis. Step 1 uses fresh caches, so a check computed without the generated declarations
is never stored for a later run. In step 4, the check cache keys each module on the text its generators produced, as well as on its hand-written
declarations. A changed generator therefore rechecks the bodies of that module, and a body is never reused from a run
in which its generated name did not exist. A generator's output is not cached across builds (`spec/comptime.md`
CT14 keeps comptime results out of the cache).

## GN9. Tools

The formatter, the linter, `burdin doc`, and `burdin expand` see the `comptime generate` block as it is written. No
generated declaration is in a file, so none of them shows one. The language server reports the diagnostics of a
generated declaration at the block (GN6).

## GN10. Costs

Each generator has its own step budget (`spec/comptime.md` CT11). Generation adds one check of the whole program per
analysis, because step 1 is a separate check. A program with no `comptime generate` block costs nothing extra.

## Diagnostics

| Code | Meaning |
| --- | --- |
| E0242 | Invalid generated text: it does not parse, or it declares an item other than a `struct`, `enum`, or `fn` (a `const`, `use`, `impl`, `interface`, `derive`, `type`, test, bench, `comptime assert`, or a nested `comptime generate`) |
| E0243 | `comptime generate` outside module scope |

The existing codes apply as they do elsewhere: E0002 for a generator's reference to a generated name, E0003 for a block
whose value is not a `String`, E0007 for a duplicate name, E0222, E0224, and E0225 for a generator's compile-time error,
each reported at the block.

## Tests

`tests/generate/` covers the rules: `functions` (GN1-GN3, a loop that generates functions), `struct`, `enum`,
`reflection` (a generator reads `Point.fields`), `two_generators` (module order and several blocks), `escapes`
(braces and quotes in the text), `empty` (an empty text), `parse_error` and `disallowed_item` (E0242), `not_module_scope`
(E0243), `wrong_type` (E0003), `panic` (E0222), `impure` (E0224), `budget` (E0225), `sees_generated` (E0002, GN5),
`duplicate_name` (E0007), and `error_in_generated` (an error in generated code at the block line, GN6).

## Not in this slice

- Generated items other than `struct`, `enum`, and `fn` (GN4): constants, impl blocks, interfaces, and derives.
- Generation from inside a function body (E0243).
- Hygiene (GN6), and generators that read generated declarations (GN3, GN5).
- The formatter, linter, documentation, and expansion showing the generated declarations (GN9).
