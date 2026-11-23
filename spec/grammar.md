# Burdin grammar (stage 0, plus stage 1 closures, iterators, formatting, and existentials)

This is the grammar the compiler accepts. It covers exactly the stage 0 subset plus milestone 12's closures,
milestone 13's member names, milestone 14's string interpolation (the details are in `format.md`), milestone 15's
`any`/`impl` existential types (the details are in `existentials.md`), milestone 16's `error` declarations
(the details are in `errors.md`), milestone 17's default and named arguments (the details are in
`arguments.md`), milestone 18's extension methods (the details are in `extensions.md`), milestone 20's comptime
forms (the details are in `comptime.md`), and milestone 21's `derive` item, const parameters, computed field access,
generic type aliases, impl blocks for specific type arguments, and associated types (the details are in
`reflection.md`, `const-generics.md`, `type-aliases.md`, `impls.md`, and `associated-types.md`). It is derived from
the conformance tests in `tests/`, so every construct
below is used by at least one test, and every construct those tests use is here. Features deferred to later stage 1
milestones (tasks) are reserved but not parsed.

Notation: `"x"` is a token, `A B` a sequence, `A | B` a choice, `[A]` optional, `{A}` zero or more, `(A)` grouping.
`NL` means a newline between two tokens (see Newlines).

## 1. Lexical structure

Source files are UTF-8. Line endings may be `\n` or `\r\n`.

**Comments** are ignored: `// ...` to the end of the line (including `///` and the test directives `//@`, `//>`, `//~`),
and `/* ... */` (block comments do not nest).

**Shebang.** A file whose first two bytes are `#!` is a script: the whole first line is skipped like a comment, so
`#!/usr/bin/env burdin run` is not part of the program (`spec/scripts.md`, SC1). `#!` anywhere else is E0300
(SC2); `#` is not otherwise part of the language.

**Identifiers**: `[A-Za-z_][A-Za-z0-9_]*`, ASCII only. The single underscore `_` is the wildcard token, not an identifier.

**Keywords** (reserved):

```
any  as  break  comptime  const  continue  derive  else  enum  error  extern  false  fn  for  if  impl  in  interface
let  loop  match  mut  orelse  own  priv  return  self  Self  struct  true  type  unsafe  use  where  while
```

`derive` starts a user derive declaration (`DeriveItem`, `reflection.md`, RFL8). No keyword is an identifier, except
immediately after a `.` in a member position, where it is a member name: that is how `.any(...)` reaches the
`Iterator` default method declared as `any_item` (`iterators.md`, IT9), and why a member may be spelled `any` or
`impl` now that both are keywords.

Contextual keywords, meaningful only in one position and otherwise ordinary identifiers: `test` (item start, followed
by a string), `bench` (item start, followed by a string; [`bench.md`](bench.md), BN1), `fuzz` (item start, followed
by a name and a parameter list; [`fuzz.md`](fuzz.md), FZ1),
`requires` (after an interface name), `repr` (after a struct
name), `from` (after an error variant's payload), `pub` (immediately before `use`), `select` (an expression start,
followed by `{` on the same line, `channels.md` CH6), `default` (the head of a `select` arm), `blocking` (before `fn`
or `unsafe fn` inside an `extern "C" { ... }` block, `io.md` IO4; before a `fn` anywhere else it is reported as
E0290), and `Fn`/`FnMut`/`FnOnce` (a closure type when followed by `(`). The attribute names of a test item (`ignore`,
`timeout`, `should_panic`, `should_fail`, `differential`) are contextual only inside the `[...]` after the test name; the checker rejects an
unknown name with E0170 ([`test-runner.md`](test-runner.md)). A bench item takes no attributes in this stage, so
anything after its name is a syntax error.

**Integer literals**: decimal `123`, hex `0xFF`, octal `0o17`, binary `0b1010`, with `_` allowed between digits. No suffixes;
the type comes from context, or from the other operand of a binary operator (on either side: `1 + b` and `b + 1` both
take `b`'s type), and defaults to `i32` otherwise. A leading `-` is the unary operator, not part of the literal. `/` and
`%` truncate toward zero; see `stage0-subset.md` for the rest of the numeric behavior.

**Float literals**: digits `.` digits with an optional exponent (`2.5`, `3.99`, `1.0e3`), and digits with an exponent (`1e3`).
A `.` must be followed by a digit to make a float, so `0..3` is `0`, `..`, `3`, and `t.0.1` is `t`, `.`, `0`, `.`, `1`:
immediately after a `.` token, a numeric literal is lexed as plain digits.

**Char literals**: `'x'`, with escapes `\n \r \t \\ \' \0`. One Unicode scalar value.

**Loop labels**: a quote, then an identifier, with no quote after the identifier, is a `LABEL` token (`'outer`). A quote
that closes after one character or name is a character literal instead: `'a'` and `'_'` are chars, and `'a:` is a
label (issue #275).

**String literals**: `"..."` with escapes `\n \r \t \\ \" \0`, and raw newlines allowed inside. There is no interpolation in
stage 0: `{` and `}` are ordinary characters.

**Punctuation and operators**:

```
(  )  [  ]  {  }  ,  :  ;  .  ..  ..=  =>  @  ?  =
+  -  *  /  %  &  |  ^  <<  >>  !  &&  ||  ==  !=  <  >  <=  >=
```

When the parser is inside generic arguments or parameters and meets `>>`, `>=`, or `>>=`, it splits the token so the first
`>` closes the list (`CPtr<CPtr<CChar>>`, `Result<List<Token>, i32>`).

## 2. Newlines

There are no statement terminators. Each token records whether a line break comes before it.

1. **Inside `(...)` and `[...]`** newlines are ignored. A `{` that starts a block, struct body, or match turns newline
   handling back on until its matching `}`.
2. **A statement ends** where its expression cannot continue. After a complete expression, the next token continues it only
   if it is on the same line, or if it starts with one of: `.`, `?`, `else`, `orelse`, `as`, `&&`, `||`.
   So a call's `(` or an index `[` must be on the same line as the callee, and a line starting with `-`, `*`, `&`, `(` or `[`
   starts a new statement.
3. **After a binary operator, `=`, `,`, `.`, `:`, `=>`, or an opening bracket**, the expression necessarily continues on the next line.
4. **Lists inside braces** (struct fields, enum variants, struct-literal fields, match arms) are separated by `,` or
   by a line break. A trailing separator is allowed. Interface methods and `extern` declarations follow one another
   with no separator, like top-level items. Lists inside `(...)`, `[...]` and `<...>` use commas only.
5. `return` and `break` take a value only if one starts on the same line. A label after `break` or `continue` must start
   on the same line too; a value after the label follows the same rule.

```
let total = a +
    b                    // one statement (rule 3)
let r = list
    .len()               // one statement (rule 2)
foo()
(x, y) = pair            // two statements: `(` is not a call here
```

## 3. Items

```
File          = { Item } ;

Item          = TestItem | BenchItem | FuzzItem | UseItem | ImplBlock | ComptimeAssert | ComptimeGenerate
              | [ "priv" ] ( FnItem | ExtensionItem | StructItem | EnumItem | ErrorItem | InterfaceItem | DeriveItem | TypeAlias | ConstItem | ExternItem ) ;

FnSig         = [ "unsafe" ] "fn" IDENT [ GenericParams ] "(" [ ParamList ] ")" [ ":" Type ] [ WhereClause ] ;
FnItem        = FnSig Block ;

(* `fn Type.name(self, ...)`: an extension method. The receiver is the type path before the last dot, including
   the type parameters it declares (`List<T>`), or a slice type (`fn [T].first(self)`, `fn [[T]].concat(self)`);
   `<...>` after the method name declares method-level parameters. `spec/extensions.md`, EX1. *)
ExtensionItem = [ "unsafe" ] "fn" ( TypePath | "[" Type "]" ) "." IDENT [ GenericParams ] "(" [ ParamList ] ")" [ ":" Type ] [ WhereClause ] Block ;
ParamList     = Param { "," Param } [ "," ] ;
Param         = SelfParam | [ "comptime" ] [ Convention ] IDENT ":" Type [ "=" Expr ] ;
                                                        (* `comptime` takes a compile-time value: comptime.md, CT4 *)
SelfParam     = [ Convention ] "self" ;                 (* only as the first parameter, only inside impl/interface *)
Convention    = "mut" | "own" ;                         (* no convention = read-only *)

GenericParams = "<" GenericParam { "," GenericParam } [ "," ] ">" ;
GenericParam  = IDENT [ ":" Bounds ] [ "=" Type ]   (* a default: only on a struct's or enum's trailing type
                                                        parameters, `spec/allocators.md`, AL5 *)
              | "const" IDENT ":" Type ;                (* a const parameter: `spec/const-generics.md`, CG1 *)
Bounds        = TypePath { "+" TypePath } ;
WhereClause   = "where" WherePred { "," WherePred } ;
WherePred     = Type ":" Bounds ;

StructItem    = "struct" IDENT [ GenericParams ] [ Derives ] [ "repr(C)" ] "{" [ FieldList ] "}" ;
FieldList     = Field { Sep Field } [ Sep ] ;
Field         = [ "priv" ] IDENT ":" Type ;
(* `: A, B` after the name: the interfaces the declaration implements by itself, each a built-in derive, a `derive`
   declaration, or an interface whose methods all have defaults. No trailing comma. `derives.md`, issue #825. *)
Derives       = ":" TypePath { "," TypePath } ;

(* `derive Name { fn ... }`: the methods a user derive generates; `struct T: Name` applies it. The body of a method
   is ordinary code that can use the reflection members. `spec/reflection.md`, RFL8-RFL9. *)
DeriveItem    = "derive" IDENT "{" { ImplFn } "}" ;

EnumItem      = "enum" IDENT [ GenericParams ] [ Derives ] "{" [ VariantList ] "}" ;
VariantList   = Variant { Sep Variant } [ Sep ] ;
Variant       = IDENT [ "(" Type { "," Type } [ "," ] ")" | "{" [ FieldList ] "}" ] ;

(* An error enum is an ordinary enum whose variants also carry a required message template, and optionally a
   `from` (the variant must then be a tuple variant with exactly one field). `spec/errors.md`, ERR1-ERR3. *)
ErrorItem     = "error" "enum" IDENT [ GenericParams ] [ Derives ] "{" [ ErrorVariantList ] "}" ;
ErrorVariantList = ErrorVariant { Sep ErrorVariant } [ Sep ] ;
ErrorVariant  = Variant [ "from" ] STRING ;

InterfaceItem = "interface" IDENT [ GenericParams ] [ "requires" Bounds ] "{" { InterfaceMember } "}" ;
InterfaceMember = InterfaceFn | AssociatedType ;
InterfaceFn   = FnSig [ Block ] ;                       (* a body makes it a default method *)
(* `type Name` declares an associated type; it takes no bound and no default in stage 1 (`associated-types.md`, AT1). *)
AssociatedType = "type" IDENT ;

ImplBlock     = [ "unsafe" ] Type [ ":" TypePath ] [ WhereClause ] "{" { ImplItem } "}" ;
                (* `unsafe` is required for `Send` and `Sync` and allowed for nothing else (E0281); spec/shared.md SH5 *)
ImplItem      = ImplFn | AssociatedTypeBinding ;
ImplFn        = [ "priv" ] FnItem ;
(* In a `Type: Interface` block, every associated type of the interface is bound exactly once
   (`associated-types.md`, AT2). An impl block with no interface cannot bind one (E0144). *)
AssociatedTypeBinding = "type" IDENT "=" Type ;

TypeAlias     = "type" IDENT [ GenericParams ] "=" Type ;
ConstItem     = "const" IDENT ":" Type "=" Expr ;

UseItem       = [ "pub" ] "use" IDENT { "." IDENT } [ "." "{" IDENT { "," IDENT } [ "," ] "}" | "as" IDENT ] ;   (* `pub` re-exports *)

ExternItem    = "extern" STRING ( "{" { [ "priv" ] ExternFnSig } "}" | ExternFnSig Block ) ;
ExternFnSig   = [ "blocking" ] [ "unsafe" ] "fn" IDENT "(" [ ExternParams ] ")" [ ":" Type ] ;   (* no generics, no `where` *)
ExternParams  = Param { "," Param } [ "," [ "..." ] ] ;   (* `...` only in a declaration *)

TestItem      = "test" STRING [ TestAttrs ] Block ;
TestAttrs     = "[" TestAttr { "," TestAttr } [ "," ] "]" ;
TestAttr      = IDENT [ "(" ( INT | STRING ) ")" ] ;   (* ignore, timeout(ms), should_panic[(message)], should_fail, differential *)

(* `bench.md`, BN1. Only `burdin bench` declares and checks these; every other build strips them. *)
BenchItem     = "bench" STRING Block ;

(* `fuzz.md`, FZ1. Only `burdin fuzz` and `burdin test` declare these; every other build strips them. The signature
   is fixed by the checker: one read-only `[u8]` parameter and no result (E0173). *)
FuzzItem      = "fuzz" IDENT [ GenericParams ] "(" [ ParamList ] ")" [ ":" Type ] [ WhereClause ] Block ;

Sep           = "," | NL ;
```

**Declaration layout.** The lists inside braces are separated by `,` or by a line break. The tests and the
recommended style put one struct field, one enum variant, and one struct-literal field per line; the parser accepts
both forms, so `struct P { x: i32, y: i32 }` and the one-per-line version mean the same thing. Struct literals,
variant constructors, argument lists, generic arguments, tuples, and arrays also use commas. Interface methods,
`extern` declarations, and top-level items need no separator: each one ends where the next begins.

**Default arguments.** A named parameter may end with `= Expr`, the value a call may omit. Only a function or
method with one implementation may declare one, the parameters with defaults must be the trailing ones, and the
expression must be a constant expression checked at the declaration (`arguments.md`, AG1-AG3). Interface method
signatures and `extern` declarations cannot (E0213).

**Type parameter defaults.** A struct's or enum's type parameter may end with `= Type`, the argument a use may
omit (`struct Buffer<A: Allocator = Global>`). The parameters with defaults must be the trailing ones, a const
parameter takes no default, and functions, interfaces, and type aliases cannot declare one (`allocators.md`,
AL5; E0271). A use may stop writing arguments at the first omitted default: `Buffer` is `Buffer<Global>`.

**Unique names and cycles.** A name is unique in its namespace: a repeated type, interface, function, constant,
test, type parameter, field, variant, method, extension method for the same receiver, or import is E0007. A declaration may not depend on itself: a constant
whose value mentions it, a type alias that resolves to itself, and a struct or enum that contains itself by value
are E0009 (put a recursive value behind `Heap<T>`).

**`main`.** The entry point is an ordinary function `fn main()` that takes no parameters and returns nothing, or
`fn main(): Result<...>` (written directly, not through an alias): an `Err` prints its message to standard error and
exits 1 (`spec/errors.md`, ERR8). A parameter or any other return type is E0003.

**`Drop`.** The only way to declare a drop method is `Type: Drop { fn drop(mut self) { ... } }`. The block has
exactly one method named `drop`; anything else is E0131. `drop` takes `mut self` and returns nothing, and any other
receiver or a return type is E0003. `Drop` is not derivable (E0002) and cannot be combined with `Copy` (E0130); the
memory rules are in `memory-rules.md` (DR8).

An item that starts with an identifier (not a keyword) is an impl block. `Type: Interface { ... }` implements an interface,
`Type { ... }` adds methods with no interface, and generics and `where` bounds go on either. The type of an impl block
is a type path, which may name the arguments of a generic type (`Stack<i32>`, `Pair<i32, B>`): the names in them that
are not types are the block's own parameters, and the block covers the instances they match (`impls.md` IM1-IM2).
An impl block cannot be written through a type alias (`type-aliases.md` AL5).

**`repr(C)`.** `struct Name repr(C) { ... }` gives a struct the C layout: fields in declaration order with C padding
and alignment, reported by `Type.size()` and `Type.align()`. It may be used behind `CPtr<T>` in the C bridge; passing
or returning it by value across the C ABI is E0162 (`stdlib.md`). `repr(...)` with another ABI is E0164.

**`extern "C"`.** A block declares C functions; a single `extern "C" fn` with a body exports a Burdin function under
its plain C symbol. The signature rules are in `stdlib.md`; `...` is only allowed in a declaration, and a variadic
parameter list on any other function is E0001. The convention must be `C` (E0162). A generic declaration in a block,
a runtime-reserved symbol, and a second declaration of one symbol with a different signature are E0163; an exported
function's generic or variadic form is E0162. `blocking` before the declaration marks a C function that may block:
a call runs on the helper pool and parks the calling task (`io.md`, IO4). It is allowed only in a block, not on a
variadic declaration, and every declaration of one symbol must agree about it, or it is E0290.

**`pub use` and module visibility.** `pub use` adds the imported names to the exporting module's exported names, so
another module or package can import them. `use <package>` names a dependency's root module (`src/<package>.bd` or
`src/lib.bd`); the dependency's other modules are hidden unless the root re-exports them. `pub use` of a
package-private item, and a public signature or field naming a package-private type, are E0150.

**Extension methods.** `fn Type.name(self, ...)` declares an extension method: the receiver is the type path before
the last dot, including the type arguments that declare its type parameters (`fn List<T>.sum(self)`), and a
`<...>` list after the method name declares method-level ones (`arguments.md`, GM1-GM6). An extension is visible
in its declaring module and in modules that import it by name (`use text.shout`, `use text.{shout}`); importing the
module alone does not bring it, and it cannot be imported `as` another name (E0216). A call tries the receiver type's
methods first and a visible extension last; a shadowed extension gets the warning E0215 at its declaration. The
rules are in
`extensions.md` EX1-EX8.

## 4. Types

```
Type          = TypePath
              | Projection                              (* an associated type, stage 1 (`associated-types.md`, AT3) *)
              | "any" TypePath                          (* stage 1, milestone 15 *)
              | "impl" TypePath                         (* stage 1, milestone 15; in a parameter list or a return type, and
                                                          nowhere else (existentials.md, AN4; the compiler still rejects a
                                                          return type until #926 lands) *)
              | ClosureType                             (* stage 1, milestone 12 *)
              | "(" ")"                                 (* unit *)
              | "(" Type "," [ Type { "," Type } [ "," ] ] ")"   (* tuple of two or more, or one with a trailing comma *)
              | "[" Type ";" Expr "]"                   (* array: [i32; 4] *)
              | "[" Type "]"                            (* slice view *)
              | "&" [ "mut" ] Type                      (* reference *)
              | "!" ;                                   (* never; written by the standard library only *)

TypePath      = IDENT [ GenericArgs ] { "." IDENT [ GenericArgs ] } ;
                (* `T.Name`, with `T` a type parameter or a type path segment, is parsed as this two-segment path and
                   reinterpreted as an unqualified associated type projection in the checker (`associated-types.md`,
                   AT3). *)
Projection    = "Self" "." IDENT
              | "<" Type "as" TypePath ">" "." IDENT ;  (* qualified: `<T as Iterator<i32>>.Item` *)
GenericArgs   = "<" GenericArg { "," GenericArg } [ "," ] ">" ;
GenericArg    = Type
              | [ "-" ] INT | "true" | "false" | CHAR   (* a const argument: spec/const-generics.md, CG2 *)
              | "{" Expr "}" ;
ClosureType   = ( "Fn" | "FnMut" | "FnOnce" ) "(" [ Type { "," Type } [ "," ] ] ")" [ ":" Type ] ;
```

`Fn`, `FnMut`, and `FnOnce` are compiler-known when written with a parameter list; a bare identifier with that
spelling is still an ordinary type name. A closure type with no `: Type` returns `()`. The three names form a
capability hierarchy for calling: a value of `Fn` may be used where `FnMut` or `FnOnce` is expected, and an `FnMut`
where `FnOnce` is expected, never the other way (`memory-rules.md` CL5). A closure type written in a signature is
owned; only a type parameter's `Fn` bound keeps the closure's concrete capture mode and lets a borrowing closure cross
the call (`memory-rules.md` CL9). A non-generic function name, a built-in function name, or `Some` is an expression of
a closure type when the context expects one (`memory-rules.md` CL11); `f(args)` calls any expression of a closure
type, including a field, an element, or a call result.

`any I` is an owned, first-class existential: a two-word box whose data is a heap allocation and whose vtable points
at the constant table for `(T, I)`. It is stored, moved, and dropped like any other owned value, satisfies `I` and
every interface `I requires`, and is never `Copy`, `Clone`, `Eq`, `Ord`, `Hash`, or `Default`
(`existentials.md` AN1, AN8). A concrete value is boxed implicitly wherever an `any I` is the expected type (an
annotation, an argument, a return, a field, an element, a `Some`/`Ok`/`Err` payload, or a match arm); the value must
implement `I` (E0141) and be first-class, so a view is E0120 (AN3). `any I` is allowed only for an object-safe
interface: no generic methods, no method without a `self` receiver, no `own self`, no `Self` outside the receiver, no
view return, and every `requires` interface object-safe (E0202; AN2). `I` may be an interface path with type
arguments (`any Store<i32>`); `any` applied to a type, a type parameter, or a built-in bound such as `Copy` is E0200,
and boxing a primitive's compiler-provided `Display`/`Debug` implementation is E0204 (AN12).

`impl I` is accepted in a parameter list and as a return type (E0201 anywhere else). In a parameter list it is sugar
for one hidden type parameter with `I` as its bound, so `fn f(x: impl I)` is `fn f<T: I>(x: T)`, dispatch is static,
and `I` need not be object-safe. As a return type it is one concrete type that the body determines; callers see only
`I` (AN4). `any I` cannot appear in an `extern "C"` signature, as an `impl` target of a block, or in a pattern; use
`is`/`downcast` instead (AN10).

`i32`, `str`, `bool`, `String`, `Range<T>`, `RangeInclusive<T>`, `Self` and the other built-in names are ordinary
type paths resolved in the prelude. `Self` means the type of the enclosing impl block, there and in expressions and
patterns (`Self.new()`, `Self { ... }`, `Self.Variant`). Type arguments need a type that declares type parameters;
`Plain<i32>` where `Plain` is not generic is E0180.

## 5. Statements and blocks

```
Block         = "{" { Stmt } "}" ;
Stmt          = LetStmt | ExprStmt ;

LetStmt       = "let" Pattern [ ":" Type ] "=" Expr [ "else" Block ] ;    (* else only with a refutable pattern *)
ExprStmt      = Expr ;
```

A block's value is the value of its last statement when that is an expression statement, otherwise `()`. An expression
statement in any position may produce a value of any type; the value is dropped at the end of the statement.
Nested items inside blocks are not allowed in stage 0.

## 6. Expressions

```
Expr          = AssignExpr ;
AssignExpr    = OrElseExpr [ AssignOp AssignExpr ] ;    (* right associative; the left side must be a place *)
AssignOp      = "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" | "<<=" | ">>=" ;
OrElseExpr    = RangeExpr { "orelse" RangeExpr } ;
RangeExpr     = OrExpr [ ( ".." | "..=" ) OrExpr ] ;
(* then the binary levels below, loosest to tightest *)
```

Precedence from loosest to tightest:

| Level | Operators | Notes |
| --- | --- | --- |
| 1 | `=` `+=` `-=` `*=` `/=` `%=` `&=` `\|=` `^=` `<<=` `>>=` | right associative; statement-like, result is `()`; a compound operator is `spec/compound-assignment.md` (CA1) |
| 2 | `orelse` | left associative |
| 3 | `..` `..=` | not associative |
| 4 | `\|\|` | |
| 5 | `&&` | |
| 6 | `==` `!=` `<` `>` `<=` `>=` | not associative: `a < b < c` is an error |
| 7 | `\|` | |
| 8 | `^` | |
| 9 | `&` | |
| 10 | `<<` `>>` | |
| 11 | `+` `-` | |
| 12 | `*` `/` `%` | |
| 13 | `as` | `expr as Type` |
| 14 | unary `-` `!` `*` `&` `&mut` | prefix |
| 15 | postfix `?`, `.field`, `.0`, `call(...)`, `index[...]` | tightest |

```
UnaryExpr     = ( "-" | "!" | "*" | "&" [ "mut" ] ) UnaryExpr | ComptimeExpr | PostfixExpr ;
(* `comptime E` evaluates E while compiling (comptime.md, CT2); `comptime if` and `comptime for` select a branch
   and unroll a body (CT5, CT6); `comptime assert(...)` is checked while compiling (CT8). The operand of `comptime`
   is a unary expression, so `comptime f(x) + 1` adds 1 at run time to a compile-time `f(x)`. `comptime if let` is
   a syntax error. The message of an assertion parses as an expression, and the checker requires a string
   literal (E0220). *)
ComptimeExpr  = ComptimeAssert | "comptime" UnaryExpr ;
ComptimeAssert = "comptime" "assert" "(" Expr [ "," Expr ] [ "," ] ")" ;
(* `comptime generate { ... }` is an item, and only at module scope (E0243 elsewhere): its block is the body of a
   function that returns the `String` of source text the compiler parses as declarations (`spec/generate.md`, GN2).
   `generate` is a keyword only right after `comptime`, and is not reserved. *)
ComptimeGenerate = "comptime" "generate" Block ;
PostfixExpr   = Primary { "?" | "." IDENT [ GenericArgs ] | "." "{" Expr "}" | "." INT | "(" [ ArgList ] ")" | "[" Expr "]" } ;
                                                        (* `x.{name}`: a field chosen by a compile-time string, `spec/reflection.md` RFL5 *)
ArgList       = Arg { "," Arg } [ "," ] ;
Arg           = [ "mut" ] [ IDENT ":" ] Expr ;          (* `IDENT ":"` names the parameter; "mut" marks an argument passed to a mut parameter; `own` and read-only parameters take no marker *)

Primary       = Literal | PathExpr | "self" | "Self" | "(" ")" | "(" Expr ")" | TupleExpr
              | ArrayExpr | StructLiteral | Block | ClosureExpr | IfExpr | MatchExpr | LoopExpr | WhileExpr
              | ForExpr | SelectExpr | "unsafe" Block | "return" [ Expr ] | "break" [ LABEL ] [ Expr ]
              | "continue" [ LABEL ] | LabeledExpr | InlineSnapshot ;

Literal       = INT | FLOAT | CHAR | STRING | "true" | "false" ;
InlineSnapshot = "@" STRING ;                           (* no placeholders; only `assert_snapshot`'s second argument, E0172 (test-runner.md, SN4) *)
ClosureExpr   = "|" [ ClosureParam { "," ClosureParam } [ "," ] ] "|" [ ":" Type ] Expr
              | "||" [ ":" Type ] Expr ;                 (* zero parameters *)
ClosureParam  = IDENT [ ":" Type ] ;                     (* no convention; the callee owns each argument *)
PathExpr      = IDENT [ GenericArgs ] ;                 (* later segments are parsed as field accesses, see below *)
                                                         (* `Self` is a path for `Self.new()`, `Self.Variant`, and `Self { ... }` *)
TupleExpr     = "(" Expr "," [ Expr { "," Expr } [ "," ] ] ")" ;
ArrayExpr     = "[" [ Expr { "," Expr } [ "," ] ] "]" | "[" Expr ";" Expr "]" ;
StructLiteral = PathExpr { "." IDENT } "{" [ FieldInit { Sep FieldInit } [ Sep ] ] "}" ;
FieldInit     = IDENT [ ":" Expr ] ;                    (* `x` alone is shorthand for `x: x` *)

IfExpr        = "if" Cond Block [ "else" ( IfExpr | Block ) ] ;
Cond          = Expr | "let" Pattern "=" Expr ;
MatchExpr     = "match" Expr "{" { Arm } "}" ;
Arm           = ( "else" | Pattern [ "if" Expr ] ) "=>" ( Block | Expr ) [ Sep ] ;
SelectExpr    = "select" "{" { SelectArm } "}" ;      (* `select` is contextual and the `{` is on the same line; the parser
                                                         rewrites it to ordinary code, `channels.md` CH6-CH8 *)
SelectArm     = [ SelectBind "=" ] Expr "." "recv" "(" ")" [ "if" Expr ] "=>" ( Block | Expr ) [ Sep ]
              | [ SelectBind "=" ] Expr "." "send" "(" Expr ")" [ "if" Expr ] "=>" ( Block | Expr ) [ Sep ]
              | "default" "=>" ( Block | Expr ) [ Sep ] ;
SelectBind    = "_" | [ "mut" ] IDENT ;
LoopExpr      = "loop" Block ;
WhileExpr     = "while" Cond Block ;                      (* a `let` condition is `while let`, rule 8.11 *)
ForExpr       = "for" Pattern "in" [ "mut" ] Expr Block ;    (* `mut`: iterate `iter_mut()`, spec/iterators.md IT15 *)
LabeledExpr   = LABEL ":" ( LoopExpr | WhileExpr | ForExpr ) ;   (* the label names the loop for `break LABEL` and `continue LABEL` (rule 8.9); any other body is E0001 *)
```

A closure body is a single expression; a block is one of the expressions, so `|x| { ... }` is a closure whose body is
a block. A parameter without a type takes its type from the expected closure type (`Fn(...)`, a `Fn` bound, or a
written annotation); when neither gives one, the parameter must be annotated (E0003). The return type is written
after the closing `|` with the same `: Type` form as a function, and is inferred from the body when omitted. Newlines
are allowed inside `|...|`, between the closing `|` and `:`, and before the body.

A closure expression is a primary expression, so `|x| x + 1` captures the whole body and `(|x| x + 1)(2)` calls it.
A call whose callee is a value of a closure type is the ordinary call syntax `f(args)`; there is no separate call
operator. `||` is one token, the empty parameter list, at the start of an expression.

Arms are separated by `,` or a line break, and the separator may be left out after the last arm or after an arm whose
body is a block. `else` is the catch-all arm and must be last.

A range is `a..b` (exclusive) or `a..=b` (inclusive): an ordinary value of type `Range<T>` or `RangeInclusive<T>`.
Both are first-class, `Copy`, and iterable by `for`, and may be stored in a binding, field, or collection. The
endpoints must be integers, and both take their type from context, from the other endpoint, or default to `i32`.
The range operators bind looser than comparison, so `r == (0..4)` needs the parentheses. Both endpoints are
required: open-ended forms (`..b`, `a..`, `..`) do not exist in stage 0 and are a syntax error.
A range in an index (`s[a..b]`) is a slice view: on `str` and `String` both ends must be character boundaries, or
the slice panics (`spec/stdlib.md`, "Slicing and character boundaries").

Type arguments may follow a member name: `x.is<Circle>()` and `x.downcast<Circle>()` (AN6), a method or associated
function with type parameters of its own (`arguments.md`, TA1), and the `collect` of an iterator, `it.collect<Set<i32>>()`
(`iterators.md`, IT17). The `<` starts a type list when one parses and its
closing `>` is followed by `(`, `.`, or `{` — the same look-ahead as rule 8.2, without the uppercase-name requirement;
otherwise it is a comparison. A list on a callee that takes no type arguments is E0003 (`arguments.md`, TA5).

## 7. Patterns

```
Pattern       = OrPattern ;
OrPattern     = AtPattern { "|" AtPattern } ;
AtPattern     = IDENT "@" PrimaryPat | PrimaryPat ;
PrimaryPat    = "_"
              | [ "-" ] Literal [ "..=" [ "-" ] Literal ]        (* literal or inclusive range: 4..=9 *)
              | [ "mut" ] IDENT                                  (* binding, or a unit variant or constant in scope *)
              | PatPath
              | PatPath "(" [ Pattern { "," Pattern } [ "," ] ] ")"
              | PatPath "{" [ FieldPat { Sep FieldPat } [ Sep ] ] "}"
              | "(" [ Pattern { "," Pattern } [ "," ] ] ")" ;    (* tuple; `(p)` is just p *)
PatPath       = ( IDENT | "Self" ) { "." IDENT } ;               (* Some, Shape.Circle, Token.Num, Self.Variant *)
FieldPat      = IDENT [ ":" Pattern ] ;                          (* `w` alone is shorthand for `w: w` *)
```

A bare identifier pattern is a binding unless a unit variant or constant with that name is in scope (`None` is, through
the prelude). The linter requires bindings to be lowercase. A string literal pattern compares a `str` scrutinee byte
for byte; the domain of `str` is open, so it never makes a `match` exhaustive without an `else` or `_` arm, and a
`String` scrutinee needs `as_str` first. `tests/m04_data/string_patterns` and `nonexhaustive_string` witness both.

## 8. Disambiguation rules

1. **Struct literals are not allowed** directly in the head of `if`, `while`, `match`, `for`, or the right side of `let ... else`.
   Wrap them in parentheses there. Inside any `(...)`, `[...]` or block the restriction is lifted.
2. **Generic arguments in expressions.** After an identifier, a `<` starts generic arguments if a list of type syntax
   parses (types, const arguments, and the punctuation around them, so `a < b && c > (d)` is never one) and its closing
   `>` is followed by `.`, `(`, or `{` after an identifier that starts with an uppercase letter, or by `(` after one that
   starts with a lowercase letter (`f<i32>(x)`; `arguments.md`, TA1). Otherwise it is a comparison. This is why
   `List<i32>.new()` and `Stack<i32>.new().push(1)` parse, and `Level.Low < Level.High` is a comparison. The one shape
   that reads as a list by mistake is `f(a < b, c > (d))`: parenthesize the first comparison (`arguments.md`, TA6).
3. **Paths in expressions** are parsed as an identifier followed by field accesses. Whether `a.b.c` is a module path,
   an associated function, a variant, or a field access is decided during name resolution, not parsing.
4. **Call-site markers.** A named `mut` parameter is passed with `mut` before the argument (`add_to(mut c, 4)`);
   with a named argument the marker still comes first (`add_to(mut c: value)`). An `own` parameter and every
   read-only parameter are passed plainly. Receivers of `mut self` methods are borrowed mutably implicitly
   (`c.bump()`); a `mut` marker in receiver position is a syntax error. A missing marker for a `mut` parameter is
   E0114; a `mut` marker on an argument whose parameter is not `mut` (including `own`) is E0115.
5. **Closures** (`|x| ...`) are primary expressions (§6, `ClosureExpr`). At the start of an expression `||` is the
   empty parameter list, so `|| 0` is a closure with no parameters.
6. **An item that starts with an identifier** is an impl block. The parser decides between `Type: Interface {` and
   `Type {` by the token after the type.
7. **`else` after `let`** (`let Some(v) = e else { ... }`) is only valid when the pattern is refutable, and the block must diverge
   (`return`, `break`, `continue`, or a call that never returns). These two checks are semantic, not syntactic.
8. **Named arguments.** Inside an argument list (the `(...)` after a callee), an identifier followed by `:` names
   the parameter the argument fills: no expression of the grammar starts with `IDENT ":"`, so the look-ahead is
   unambiguous. `f(x: 1)` is a named argument, while `f(P { x: 1 })` passes a struct literal and `f(a.b)` passes
   a path. The name is matched and checked in `burdin_sema` (`arguments.md`, AG4).
9. **Loop labels** (`'outer: for ...`). `break 'outer` and `continue 'outer` name the nearest enclosing loop with that
   label in the same function body: a closure starts with no loops, so it cannot name one outside it (E0002, the label
   is not in scope there). A label no enclosing loop has is E0002, and a label that repeats the label of an enclosing
   loop is E0007. An unlabeled `break` or `continue` still names the innermost loop, whatever its label. A label goes
   only on a `while`, `loop`, or `for`; on any other expression it is E0001, and the expression is checked without it.
10. **Break values** (`let x = loop { break 42 }`). A `loop` gives back the value of its `break`s; `break` with no
    value is `break ()`. Its type is the expected type when the context gives one, otherwise the type of the first
    `break` value, and `!` when nothing breaks out of it. Every `break` of one loop must give the same type. A
    `while` or `for` can end by its condition or its items running out, so it always gives back `()` and takes no
    value: `break 1` in a `while` or `for` is E0003, while a value-less `break` there is the ordinary `break ()`.
    A labeled `break` carries its value to the loop it names, which must be a `loop` when it carries a value. A
    `break`'s value is computed before the scopes it leaves are dropped, like a `return` value
    (`spec/memory-rules.md`, DR6).
11. **`while let`** (`while let pattern = value { ... }`) runs its body for as long as the value matches the
    pattern, and it is the `while` form of `if let`. The value is evaluated again before each pass. The pattern's
    bindings are visible only in the body and are dropped at the end of each pass, and the scrutinee follows the
    `if let` rules (`spec/memory-rules.md`, MV8). The loop ends when the value does not match, so it takes no `else`
    (E0001). It is a `while` for rules 8.9 and 8.10: a label names it, and a `break` with a value is E0003. An
    pattern that always matches is E0003, because the loop would never end (`let ... else` refuses its `else` on such a
    pattern for the same reason). Witnesses: `tests/m02_basics/while_let.bd`
    (runs), `while_let_errors.bd` and `while_let_else.bd` (codes and lines), `tests/m05_ownership/while_let_moves.bd`
    (runs, drops per pass), and `while_let_moves_errors.bd` (a moved scrutinee is E0104).

## 9. What the parser does not decide

Name resolution, which paths are variants versus fields, whether a pattern identifier is a binding, mutability,
conventions, exhaustiveness, and everything in `memory-rules.md` are later phases. The parser's only job is to produce a
syntax tree that keeps spans and the line-break flags above, and to report E0001 for anything outside this grammar.
