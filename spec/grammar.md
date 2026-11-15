# Burdin grammar (stage 0)

This is the grammar the stage 0 compiler must accept. It covers exactly the stage 0 subset. It is derived from the
conformance tests in `tests/`, so every construct below is used by at least one test, and every construct those tests
use is here. Features deferred to stage 1 (closures, string interpolation, `any`/`impl` types, `error` declarations,
comptime, derive declarations, default and named arguments, extension methods, tasks) are reserved but not parsed.

Notation: `"x"` is a token, `A B` a sequence, `A | B` a choice, `[A]` optional, `{A}` zero or more, `(A)` grouping.
`NL` means a newline between two tokens (see Newlines).

## 1. Lexical structure

Source files are UTF-8. Line endings may be `\n` or `\r\n`.

**Comments** are ignored: `// ...` to the end of the line (including `///` and the test directives `//@`, `//>`, `//~`),
and `/* ... */` (block comments do not nest).

**Identifiers**: `[A-Za-z_][A-Za-z0-9_]*`, ASCII only. The single underscore `_` is the wildcard token, not an identifier.

**Keywords** (reserved):

```
as  break  const  continue  else  enum  extern  false  fn  for  if  in  interface  let  loop  match
mut  orelse  own  priv  return  self  Self  struct  true  type  unsafe  use  where  while
```

Reserved for stage 1, rejected as identifiers now: `any  comptime  derive  impl`.

Contextual keywords, meaningful only in one position and otherwise ordinary identifiers: `test` (item start, followed
by a string), `derives` (after a struct or enum name), `requires` (after an interface name), `repr` (after a struct
name), and `pub` (immediately before `use`). The attribute names of a
test item (`ignore`, `timeout`, `should_panic`, `should_fail`) are contextual only inside the `[...]` after the test
name; the checker rejects an unknown name with E0170 ([`test-runner.md`](test-runner.md)).

**Integer literals**: decimal `123`, hex `0xFF`, octal `0o17`, binary `0b1010`, with `_` allowed between digits. No suffixes;
the type comes from context, or from the other operand when the literal is on the left of a binary operator, and
defaults to `i32` otherwise. A leading `-` is the unary operator, not part of the literal. `/` and `%` truncate toward
zero; see `stage0-subset.md` for the rest of the numeric behavior.

**Float literals**: digits `.` digits with an optional exponent (`2.5`, `3.99`, `1.0e3`), and digits with an exponent (`1e3`).
A `.` must be followed by a digit to make a float, so `0..3` is `0`, `..`, `3`, and `t.0.1` is `t`, `.`, `0`, `.`, `1`:
immediately after a `.` token, a numeric literal is lexed as plain digits.

**Char literals**: `'x'`, with escapes `\n \r \t \\ \' \0`. One Unicode scalar value.

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
5. `return` and `break` take a value only if one starts on the same line.

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

Item          = TestItem | UseItem | ImplBlock
              | [ "priv" ] ( FnItem | StructItem | EnumItem | InterfaceItem | TypeAlias | ConstItem | ExternItem ) ;

FnSig         = [ "unsafe" ] "fn" IDENT [ GenericParams ] "(" [ ParamList ] ")" [ ":" Type ] [ WhereClause ] ;
FnItem        = FnSig Block ;
ParamList     = Param { "," Param } [ "," ] ;
Param         = SelfParam | [ Convention ] IDENT ":" Type ;
SelfParam     = [ Convention ] "self" ;                 (* only as the first parameter, only inside impl/interface *)
Convention    = "mut" | "own" ;                         (* no convention = read-only *)

GenericParams = "<" GenericParam { "," GenericParam } [ "," ] ">" ;
GenericParam  = IDENT [ ":" Bounds ] ;
Bounds        = TypePath { "+" TypePath } ;
WhereClause   = "where" WherePred { "," WherePred } ;
WherePred     = Type ":" Bounds ;

StructItem    = "struct" IDENT [ GenericParams ] [ "repr(C)" ] [ Derives ] "{" [ FieldList ] "}" ;
FieldList     = Field { Sep Field } [ Sep ] ;
Field         = [ "priv" ] IDENT ":" Type ;
Derives       = "derives" "(" TypePath { "," TypePath } [ "," ] ")" ;

EnumItem      = "enum" IDENT [ GenericParams ] [ Derives ] "{" [ VariantList ] "}" ;
VariantList   = Variant { Sep Variant } [ Sep ] ;
Variant       = IDENT [ "(" Type { "," Type } [ "," ] ")" | "{" [ FieldList ] "}" ] ;

InterfaceItem = "interface" IDENT [ GenericParams ] [ "requires" Bounds ] "{" { InterfaceFn } "}" ;
InterfaceFn   = FnSig [ Block ] ;                       (* a body makes it a default method *)

ImplBlock     = Type [ ":" TypePath ] [ WhereClause ] "{" { ImplFn } "}" ;
ImplFn        = [ "priv" ] FnItem ;

TypeAlias     = "type" IDENT [ GenericParams ] "=" Type ;
ConstItem     = "const" IDENT ":" Type "=" Expr ;

UseItem       = [ "pub" ] "use" IDENT { "." IDENT } [ "." "{" IDENT { "," IDENT } [ "," ] "}" | "as" IDENT ] ;   (* `pub` re-exports *)

ExternItem    = "extern" STRING ( "{" { [ "priv" ] ExternFnSig } "}" | ExternFnSig Block ) ;
ExternFnSig   = [ "unsafe" ] "fn" IDENT "(" [ ExternParams ] ")" [ ":" Type ] ;   (* no generics, no `where` *)
ExternParams  = Param { "," Param } [ "," [ "..." ] ] ;   (* `...` only in a declaration *)

TestItem      = "test" STRING [ TestAttrs ] Block ;
TestAttrs     = "[" TestAttr { "," TestAttr } [ "," ] "]" ;
TestAttr      = IDENT [ "(" ( INT | STRING ) ")" ] ;   (* ignore, timeout(ms), should_panic[(message)], should_fail *)

Sep           = "," | NL ;
```

**Declaration layout.** The lists inside braces are separated by `,` or by a line break. The tests and the
recommended style put one struct field, one enum variant, and one struct-literal field per line; the parser accepts
both forms, so `struct P { x: i32, y: i32 }` and the one-per-line version mean the same thing. Struct literals,
variant constructors, argument lists, generic arguments, tuples, and arrays also use commas. Interface methods,
`extern` declarations, and top-level items need no separator: each one ends where the next begins.

**`main`.** In stage 0 the entry point is an ordinary function `fn main()`: it takes no parameters and returns
nothing. A parameter or a non-unit return type is E0003. Stage 1 adds `fn main(): Result<...>` (m16).

**`Drop`.** The only way to declare a drop method is `Type: Drop { fn drop(mut self) { ... } }`. The block has
exactly one method named `drop`; `drop` takes `mut self` and returns nothing, and any other receiver or a return type
is E0003. `Drop` is not derivable (E0002) and cannot be combined with `Copy` (E0130); the memory rules are in
`memory-rules.md` (DR8).

An item that starts with an identifier (not a keyword) is an impl block. `Type: Interface { ... }` implements an interface,
`Type { ... }` adds methods with no interface, and generics and `where` bounds go on either.

**`repr(C)`.** `struct Name repr(C) { ... }` gives a struct the C layout: fields in declaration order with C padding
and alignment, reported by `Type.size()` and `Type.align()`. It may be used behind `CPtr<T>` in the C bridge; passing
or returning it by value across the C ABI is E0162 (`stdlib.md`). `repr(...)` with another ABI is E9000.

**`extern "C"`.** A block declares C functions; a single `extern "C" fn` with a body exports a Burdin function under
its plain C symbol. The signature rules are in `stdlib.md`; `...` is only allowed in a declaration, and a variadic
parameter list on any other function is E0001.

**`pub use` and module visibility.** `pub use` adds the imported names to the exporting module's exported names, so
another module or package can import them. `use <package>` names a dependency's root module (`src/<package>.bd` or
`src/lib.bd`); the dependency's other modules are hidden unless the root re-exports them. `pub use` of a
package-private item, and a public signature or field naming a package-private type, are E0150.

## 4. Types

```
Type          = TypePath
              | "(" ")"                                 (* unit *)
              | "(" Type "," [ Type { "," Type } [ "," ] ] ")"   (* tuple of two or more, or one with a trailing comma *)
              | "[" Type ";" Expr "]"                   (* array: [i32; 4] *)
              | "[" Type "]"                            (* slice view *)
              | "&" [ "mut" ] Type                      (* reference *)
              | "!" ;                                   (* never; written by the standard library only *)

TypePath      = IDENT [ GenericArgs ] { "." IDENT [ GenericArgs ] } ;
GenericArgs   = "<" Type { "," Type } [ "," ] ">" ;
```

`i32`, `str`, `bool`, `String`, `Range<T>`, `RangeInclusive<T>`, `Self` and the other built-in names are ordinary
type paths resolved in the prelude. `Self` means the type of the enclosing impl block, there and in expressions and
patterns (`Self.new()`, `Self { ... }`, `Self.Variant`).

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
AssignExpr    = OrElseExpr [ "=" AssignExpr ] ;         (* right associative; the left side must be a place *)
OrElseExpr    = RangeExpr { "orelse" RangeExpr } ;
RangeExpr     = OrExpr [ ( ".." | "..=" ) OrExpr ] ;
(* then the binary levels below, loosest to tightest *)
```

Precedence from loosest to tightest:

| Level | Operators | Notes |
| --- | --- | --- |
| 1 | `=` | right associative; statement-like, result is `()` |
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
UnaryExpr     = ( "-" | "!" | "*" | "&" [ "mut" ] ) UnaryExpr | PostfixExpr ;
PostfixExpr   = Primary { "?" | "." IDENT | "." INT | "(" [ ArgList ] ")" | "[" Expr "]" } ;
ArgList       = Arg { "," Arg } [ "," ] ;
Arg           = [ "mut" ] Expr ;                        (* "mut" marks an argument passed to a mut parameter; `own` and read-only parameters take no marker *)

Primary       = Literal | PathExpr | "self" | "Self" | "(" ")" | "(" Expr ")" | TupleExpr
              | ArrayExpr | StructLiteral | Block | IfExpr | MatchExpr | LoopExpr | WhileExpr | ForExpr
              | "unsafe" Block | "return" [ Expr ] | "break" | "continue" ;

Literal       = INT | FLOAT | CHAR | STRING | "true" | "false" ;
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
LoopExpr      = "loop" Block ;
WhileExpr     = "while" Expr Block ;
ForExpr       = "for" Pattern "in" Expr Block ;
```

Arms are separated by `,` or a line break, and the separator may be left out after the last arm or after an arm whose
body is a block. `else` is the catch-all arm and must be last.

A range is `a..b` (exclusive) or `a..=b` (inclusive): an ordinary value of type `Range<T>` or `RangeInclusive<T>`.
Both are first-class, `Copy`, and iterable by `for`, and may be stored in a binding, field, or collection. The
endpoints must be integers, and both take their type from context, from the other endpoint, or default to `i32`.
The range operators bind looser than comparison, so `r == (0..4)` needs the parentheses.

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
the prelude). The linter requires bindings to be lowercase.

## 8. Disambiguation rules

1. **Struct literals are not allowed** directly in the head of `if`, `while`, `match`, `for`, or the right side of `let ... else`.
   Wrap them in parentheses there. Inside any `(...)`, `[...]` or block the restriction is lifted.
2. **Generic arguments in expressions.** After an identifier that starts with an uppercase letter, a `<` starts generic
   arguments if a type list parses and its closing `>` is followed by `.`, `(`, or `{`. Otherwise it is a comparison.
   This is why `List<i32>.new()` and `Stack<i32>.new().push(1)` parse, and `Level.Low < Level.High` is a comparison.
3. **Paths in expressions** are parsed as an identifier followed by field accesses. Whether `a.b.c` is a module path,
   an associated function, a variant, or a field access is decided during name resolution, not parsing.
4. **Call-site markers.** A named `mut` parameter is passed with `mut` before the argument (`add_to(mut c, 4)`); an
   `own` parameter and every read-only parameter are passed plainly. Receivers of `mut self` methods are borrowed
   mutably implicitly (`c.bump()`); a `mut` marker in receiver position is a syntax error. A missing marker for a
   `mut` parameter is E0114; a `mut` marker on an argument whose parameter is not `mut` (including `own`) is E0115.
5. **Closures** (`|x| ...`) are not parsed in stage 0; a leading `|` in expression position is a syntax error (E0001).
6. **An item that starts with an identifier** is an impl block. The parser decides between `Type: Interface {` and
   `Type {` by the token after the type.
7. **`else` after `let`** (`let Some(v) = e else { ... }`) is only valid when the pattern is refutable, and the block must diverge
   (`return`, `break`, `continue`, or a call that never returns). These two checks are semantic, not syntactic.

## 9. What the parser does not decide

Name resolution, which paths are variants versus fields, whether a pattern identifier is a binding, mutability,
conventions, exhaustiveness, and everything in `memory-rules.md` are later phases. The parser's only job is to produce a
syntax tree that keeps spans and the line-break flags above, and to report E0001 for anything outside this grammar.
