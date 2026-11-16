# Default and named arguments

Milestone 17 of stage 1 (`stage1-subset.md`). A function or method declares the value a call may omit, and a
call may fill a parameter by name. The rules below extend the call-site conventions of
[`memory-rules.md`](memory-rules.md) CV1-CV4 and the call syntax of [`grammar.md`](grammar.md); they do not add
a parallel set of conventions.

The tests are `tests/m17_arguments/`.

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
