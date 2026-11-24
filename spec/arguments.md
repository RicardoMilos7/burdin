# Default and named arguments

Milestone 17 of stage 1 (`stage1-subset.md`). A function or method declares the value a call may omit, and a
call may fill a parameter by name. The rules below extend the call-site conventions of
[`memory-rules.md`](memory-rules.md) CV1-CV4 and the call syntax of [`grammar.md`](grammar.md); they do not add
a parallel set of conventions.

The tests are `tests/m17_arguments/`. The call site's explicit type arguments (issue #847) are TA1-TA6, and a
method's own type parameters (issue #391) are GM1-GM6.

**AG1. Declaration.** A named parameter is written `name: Type = expr`. Only a function or method with one
implementation can declare one: not `self`, not an interface method signature, not an `extern "C"` declaration,
and not a `mut` parameter. A parameter with a default may not precede a parameter without one, so a positional
call can always fill the leading parameters. A default in any other position is E0213; the parameters with
defaults are the trailing ones.

```burdin
fn connect(host: str, port: u16 = 8080): Result<Conn> { ... }
connect("localhost")
connect("localhost", 9000)
```

An `extern "C" fn` exported by the program is an ordinary Burdin function to its callers, so it may declare
defaults; the C ABI it exports still has every parameter.

**AG2. Checking a default.** A default is checked once, at the declaration, against the parameter's type and in
the declaration's module scope, exactly as the same expression written in that module would be. The function's
parameters, `self`, and locals are not in scope, so a default cannot name one (that name is E0002). A default
must be a **constant expression**: a literal, another constant, an operator on constant expressions, or a cast —
the subset a `const` initializer accepts today. A call, a struct literal, a block, or any other expression is
E0213. Because it is a constant expression, a call that omits the argument inserts its value at the call site,
every time, with no side effects and no captured state. A parameter whose type mentions a type parameter cannot
have a default (E0213): the value is materialized at a concrete call, and inference does not use defaults.

**AG3. Conventions.** The call-site conventions of a parameter do not change because it has a default.
`own` parameters may have one: the value is a constant expression, and moving it into the callee is the ordinary
move of a temporary. A `mut` parameter cannot, because the callee writes to the caller's place and a default has
no place (AG1). A `mut` argument keeps its marker whether it is positional or named: `set(mut value: 3)`; a
read-only or `own` named argument takes no marker, and a stray marker is E0115 as usual (CV2-CV4).

**AG4. Named arguments.** An argument may be written `name: value`, where `name` is one of the callee's
parameter names. The receiver of a method is not a parameter name. Positional arguments fill the parameters in
order; named arguments may be written in any order, but once one argument is named every later one must be
named. A name no parameter has, a parameter named more than once, and a positional argument after a named one
are E0214. Named and positional arguments may be mixed while the positional ones come first:

```burdin
digits(2, c: 5)          // a = 2, c = 5, b takes its default
sub(b: 2, a: 5)          // 3
```

Named arguments are matched before generic inference, so they work on generic functions and generic methods
(`max_of(b: 9, a: 3)` infers `T` from both). They also work on interface methods, called statically or through
`any` (`a.add(x: 7)`). The compiler-known forms take no named arguments (E0214): built-in functions and
methods (`push(item: 1)`), the constructors `Some`, `Ok`, and `Err`, variant constructors, closure values (a
closure type has only types, not parameter names), and `extern` declarations. A format call's placeholders name
its arguments by position, never by a call-site name.

**AG5. Interface signatures.** An interface method signature declares no defaults (E0213). The signature is
shared by every implementation, and a default is a property of one concrete function; a default method (an
interface method with a body) is still an interface signature. An implementation method may declare defaults
for its own callers, but they are not inherited from the interface and not checked against it.

**AG6. Evaluation order.** Every argument expression is evaluated left to right **in parameter order**, not in
the order it was written; a default is evaluated in the position of the parameter it fills. Code generation
already emits a call's arguments in parameter order, so a named argument written out of order runs in its
parameter's position:

```burdin
f(c: log("third"), a: log("first"), b: log("second"))   // prints first, second, third
```

**AG7. Missing and extra arguments.** A parameter without a default is required. A call that omits it is E0004
("missing an argument for ..."), and a call with more positional arguments than parameters is E0004. A call
that names an unknown parameter reports E0214 instead of the count error, because the name is the mistake to
fix first.

## Explicit type arguments (issue #847)

A call may give a generic callee its type arguments at the call: `f<i32>(x)`, `json.from_str<Data>(text)`,
`x.m<U>(a)`, `Type.f<U>(a)`, and `it.collect<C>()`. The rules below are the meaning of that syntax, which
[`grammar.md`](grammar.md) rule 8.2 and section 6 parse. The tests are `tests/m07_generics/explicit_type_args.bd`,
`explicit_type_args_errors.bd`, `comparison_not_type_args.bd`, `explicit_module_call/`, and, for `collect`,
`tests/m24_stdlib/collect_explicit.bd` and `collect_explicit_errors.bd`.

**TA1. Forms.** The list follows the callee's name and comes before the call's `(`. The callee is a bare function
name (`f<T>(...)`, or an uppercase one, `F<T>(...)`), a module-qualified function (`m.f<T>(...)`), a method
(`x.m<T>(...)`), or an associated function (`Type.f<T>(...)`, or `Type<A>.f<T>(...)` for a generic type written with
its own arguments). The list holds types and const arguments, as in `grammar.md` section 4. A lowercase bare name
takes a list only when a `(` follows its closing `>`; a member name takes one when a `.`, `(`, or `{` follows, as
before.

**TA2. Arity.** The list gives every type parameter of the callee, const parameters included, in declaration order.
A partial list is not accepted. A list of another length is E0003 ("takes N type arguments, but M were given"), and
a callee with no type parameters takes none (E0003, "takes no type arguments").

**TA3. Meaning.** An explicit argument is the type of its parameter, and inference treats it as known: the arguments
are checked against that instance, so an argument of another type is the ordinary mismatch (E0003), and each type is
checked against the bounds of its parameter as usual (E0141). A const argument gives a const parameter its value
(`const-generics.md`, CG2).

**TA4. `impl` parameters.** A parameter written `impl I` adds a hidden type parameter that the call infers
(`existentials.md`, AN4). A function with one takes no explicit type arguments (E0003), because the hidden parameter
has no name to write.

**TA5. Where the list applies.** It is accepted on a generic function of the program, on a module-qualified one, and
on a method or associated function of the program that has type parameters of its own, called on a value or through
a type. It is also accepted on the `collect` of an iterator, whose one type parameter is the target the call names
(`iterators.md`, IT17), on a concrete iterator and on a value of a type parameter bounded by `Iterator`. Every other
callee refuses it with E0003 and still has its arguments checked: a function with no type parameters, a built-in
function or method (`println`, `Some`, an iterator adapter), a built-in static function (`env.args`, `process.exit`,
`Heap.new`, `Shared.new`, `List.from`, `Map.from`, `CPtr.null`), a closure held in a local, a variant, and a method or
associated function with no parameters of its own. The `is` and `downcast` methods of an `any` value keep their own
rule (`existentials.md`, AN6).

**TA6. The look-ahead.** Rule 8.2 reads `f(a < b, c > (d))` as the list `<b, c>` followed by `(`, because both halves
are type syntax. A comparison written that way is parenthesized, `f((a < b), c > (d))`. Any other comparison that
contains an operator, a string, or a keyword between its `<` and a later `>` stays a comparison.

## Method-level type parameters (issue #391)

A method, associated function, or extension method may declare type parameters of its own. The rules below are their
declaration, inference, bounds, and monomorphization; `grammar.md` section 4 is the syntax and
`extensions.md` EX1 the extension form. The tests are `tests/m07_generics/generic_methods.bd` and
`generic_methods_errors.bd`, and `tests/m18_extension/method_type_params.bd`.

**GM1. Declaration.** A `<...>` list after the method's name declares its own parameters, with the inline bounds and
`const` parameters a function's list takes. The list follows the enclosing declaration's parameters (an impl block's,
an interface's, or an extension receiver's) and is a scope of its own: a method parameter of a name hides an impl
parameter of the same name, and repeating a name within the method's list is E0007. The parameters are monomorphized
per distinct instantiation, like a generic function's (GM5). `spec/existentials.md` AN2 still keeps a **generic method
out of an object-safe interface**: `any I` for an interface with one is E0202, because a vtable slot cannot hold a
method instantiated per call.

**GM2. The impl's parameters are in scope.** Inside a method, the impl block's parameters stand for the instance's
arguments, exactly as they do in an impl body: `Pair<A>.zip<U>` uses `A` in its signature and `(self.a, u)` in its
body, and `Box<A>`'s methods may use `A` in a parameter, a return type, or another parameter's bound
(`f: Fn(A): U`). The same holds for an interface's parameters in a default method and an extension receiver's
parameters. Their bounds are the ones the enclosing declaration states (`Box<A> where A: Show { ... }`), so a method
that needs a bound on an impl parameter is written on an impl header that states it. A method's own `where` clause
names its own type parameters.

**GM3. Inference.** The method's type arguments are inferred from the call in argument order, exactly as a generic
function's are: the receiver binds an impl parameter, each argument binds what its type pattern matches, a parameter
that appears only in another parameter's `Fn` bound is inferred by matching that bound against the closure's
signature, and the expected type binds what remains. A parameter that nothing determines is E0003
("cannot tell what `U` stands for"). A closure parameter written as a function type with an open return
(`fn apply<U>(self, f: Fn(i32): U)`) still supplies the closure's parameter types; the return is the body's type, and
`|n| n + 1` is accepted without annotating `n`. Explicit type arguments are accepted on a method or associated
function with parameters of its own (TA1-TA5) and seed the inference.

**GM4. Bounds.** A method-level parameter's bounds come from its inline list and from the method's `where` clause. Every
inferred or written type argument is checked against them at the call, and the body is checked once symbolically with
the parameter's bounds (E0141 when an argument does not meet one). A bound on an impl parameter is checked where the
impl covers the instance, not again per method.

**GM5. Monomorphization.** Each distinct combination of the receiver's instance and the method's type arguments is one
instance with its own body and its own symbol; two calls with the same arguments share it, and calls with different
arguments get separate code. The symbolic check of the method's body runs once, with the impl's parameters and the
method's parameters opaque.

**GM6. Rejections.** A call that determines no argument is E0003, and one whose const parameter nothing determines is
E0252 (`const-generics.md` CG6). An argument that does not meet its bound is E0141. An explicit list on a method with
no parameters of its own, of the wrong length, or of a type that does not match is E0003 (TA2, TA5), and an interface
with a generic method is not object-safe (E0202).

## Struct literals of generic types (issue #396)

A struct literal names its type with a path, and a generic type takes its arguments there. The rules below are the
meaning of the literal; `grammar.md` section 4 (`StructLiteral`) is the syntax. The tests are
`tests/m21_reflect/generic_variant_literal.bd`, `generic_variant_literal_errors.bd`, `generic_variant_modules/` (run),
and `generic_variant_modules_errors/` (compile-fail); the parser's segment index is a unit test in
`crates/burdin_syntax/src/parser/mod.rs`, and the formatter's layout is a unit test in `crates/burdin_fmt/src/docs.rs`.

**SL1. Forms.** The arguments are written on one segment of the path, and they belong to that segment: the segment
that a `.` or the end of the path follows. `Wrapper<i32> { inner: 1 }` for a generic struct, `Shape<i32>.Rect { w: 2,
h: 3 }` for a struct-like variant of a generic enum (the arguments are on `Shape`), and `sh.Shape<i32>.Rect { ... }` or
`sh.Wrapper<i32> { ... }` for the same through a module path. At most one segment carries arguments. `Self` takes none.

**SL2. Inference.** A literal without arguments takes them from its field values and the expected type, exactly as
a tuple variant does (`Shape.Circle(3)`): each value binds the parameter its field's type names, and the expected type
binds what remains. A parameter that nothing determines is E0003 ("needs type arguments"). `Self.Rect { ... }` inside
the impl block of a generic enum names the instance the block is for.

**SL3. Explicit arguments.** Arguments on a type segment name the instance, and the literal is checked against that
instance: a field value of another type is E0003, and an argument list of the wrong length is E0003 (TA2). The
arguments belong to the type, never to the variant: `Shape.Rect<i32> { ... }` is E0003 ("takes no type arguments"),
as a call to a variant with arguments is (TA5). `Shape.Rect<i32>` parses, because the parser cannot tell a variant from
a type in a module path, and the checker decides.

**SL4. Checking.** A struct-like variant literal is checked as a struct variant of a plain enum: each field is given
exactly once (a missing or repeated field is E0003), a name that is not a field is E0002, a name that is not a variant
of the enum is E0002, and a tuple or unit variant has no named fields (E0003). Each instance is monomorphized like any
other, and a non-`Copy` field value is moved into the literal. `match` takes the variant back apart by its field names
(`Shape.Rect { w, h }`).
