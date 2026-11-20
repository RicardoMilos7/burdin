# Inspecting what the compiler generated

Stage 3 (issue #595). Four commands and one flag answer "what did the compiler do with my program?" without reading the
compiler: `burdin expand` prints the source after expansion, `--emit` writes the artifacts of a build, `burdin asm` prints
one function's assembly, `burdin bloat` says where a binary's size comes from, and `burdin check --print-instances` lists
the generic instances and why each exists. Rule IDs are INS1-INS19; the tests are `tests/check_inspect.py` and the unit
tests of the `burdin_inspect` crate. All of them read what the compiler built and change nothing: they share the front end
of `burdin build` (`spec/query.md`, QL4) and never add a pass of their own to it.

| Command | Issue | Rules |
| --- | --- | --- |
| `burdin expand <target> [--item <path>] [--instances]` | #597 | INS1-INS6 |
| `burdin build --emit <kinds> [--intel \| --att]` | #598 | INS7-INS8 |
| `burdin asm <target> [<function>]` | #598 | INS9-INS11 |
| `burdin check <target> --print-instances` | #602 | INS12-INS15 |
| `burdin bloat <target>` | #600 | INS16-INS19 |

## INS1. `burdin expand`

`burdin expand <file.bd | package directory>` runs the front end of a build (build scripts, parsing, name resolution and
type checking, the memory checks, compile-time evaluation) and prints Burdin source for the program with the compiler's
expansions applied. It stops before code generation, so it needs no linker. A program with errors prints no source: the
diagnostics go to standard error as for `burdin build` and the status is 1.

The expansion starts from the text of each source file and replaces only what the compiler rewrote, so everything it did
not touch (names, comments, layout, what a `comptime` argument stands for) is the author's own text. The result goes
through `burdin fmt` (`--no-format` prints it as built), so the one style applies to it. A single file prints as the
file; a package prints each file after a `// ==== <path> ====` line, with the path relative to the package directory and
`/` separators. The standard library and the dependencies' files are not printed.

`--item <path>` prints one item instead of the files: a function, a type, a constant, an interface, a method as
`Type.method`, an extension as `Receiver.method`, or a method a user derive generated for a type. `<path>` is the name,
the name with its module (`shapes.area`), or with the package too (`app.shapes.area`). A name that matches nothing is
status 1 and lists the items; one that matches several lists them too. `--item` and `--instances` cannot be combined
(status 2).

`--instances` prints, after the files, each reachable instance of a generic function or method of the program's own
files: its declaration, with the typed tree of that instance, under a `// instance <name>` comment (`largest<i32>`,
`Bag<String>.index`). It is for reading. The declaration keeps its type parameters, so this part is not a program of its
own and is not part of the guarantee of INS6.

Tests: `tests/inspect/expand_mix.bd` and the goldens in `tests/inspect/golden/expand/`, `check_expand_item_and_instances`,
`check_expand_package`, `check_expand_failures`.

## INS2. What `comptime` and reflection become

- `comptime E` becomes the value `E` evaluated to, when the value has a literal form: a number, `bool`, `char`, `str`, a
  `String` made from a literal, an array, a tuple, `Some`/`None`/`Ok`/`Err`, a struct or enum value of such parts, or a
  constant. A number whose type is not `i32` is written `(64 as usize)`, so the type survives; a number that is not an
  `i32` and does not fit one stays `comptime E` where nothing but the expression fixes its type, because the language has
  no literal suffixes. Inside a `{...}` hole of an interpolated string a number is parenthesized (`{(10)}`, which is not
  the positional hole `{10}`), and a string literal cannot be written there, so the `comptime` stays.
- `comptime if` becomes the block of the branch it chose; the other branch is dropped without being read.
- `comptime for` becomes one block per item. Each block holds the loop's pattern as a `let` with the item's value, when
  the body still reads it, and the body with that item's results: `field.name`, `field.index`, `Self.name`, and
  `comptime` expressions inside it are the literals they evaluated to for that item.
- `x.{name}` becomes `x.field`, the field the compile-time name chose.
- A `comptime assert` has already held; in a body it becomes `{}` and a module-level one becomes a comment.
- A constant's initializer becomes the value it evaluated to (`const SIZE: usize = 4 * 16` is `64`), written where the
  constant's own type fixes the types of its parts.
- A call whose callee has a `comptime` parameter, and a function that declares one, are left as written: each call is a
  specialization, and its source is the generic form.

Anything with no literal form is left as written, never guessed, so an expansion never claims more than the compiler
established.

Tests: `m20_comptime__*` and `m21_reflect__*` goldens, `expand_mix.golden`.

## INS3. Operators, arguments, and extension calls

- An operator that resolved to an interface method is that method's call: `a + b` is `a.add(b)`, `-a` is `a.neg()`, and
  `a[i]` is `(*a.index(i))` (the method returns a reference and the operator reads through it). An operator on the left
  of an assignment (`a[i] = v`), and one the compiler evaluates itself (numbers, and the derived `==`, `<`), stay.
- A call with named arguments or omitted defaults lists every argument, in parameter order: `greet(times: 5)` is
  `greet("world", 5)`. An omitted argument is written as the default's value, not its text, because the text may name
  something that is not in scope at the call. A call that cannot be matched argument for argument (a specialization) is
  left as written.
- An extension call stays a method call. The issue asks for the call "resolved to its function", but an extension is not a
  value (`spec/extensions.md`, EX1: `shout(x)` is E0002), so no function form exists to print, and the method call is
  already the resolved one.

Tests: `m17_arguments__*` and `m19_operators__*` goldens, `expand_mix.golden`.

## INS4. User derives

A derive written in Burdin (`derive Name { ... }`, `spec/reflection.md`, RFL8) generates `Type: Name { ... }` for each
type that names it. The expansion writes that impl block after the type, with each method's body expanded for the type
(INS2: the loop over `Self.fields` unrolled, `self.{field.name}` as `self.x`, `Self.name` as the literal), removes the
name from the type's `: A, B` list, and replaces the `derive` declaration by a comment once every type that named it has
its impl block. A generic type keeps `Name` in its list (its impl is per instance), and then the declaration stays too.

Tests: `m21_reflect__derive_describe.golden`, `--item Point.describe`.

## INS5. Built-in derives

`Copy`, `Clone`, `Eq`, `Ord`, `Hash`, `Default`, `Debug`, `Serialize`, and `Deserialize` have no source form that the
compiler would accept back: their code is structural (`spec/derives.md`, `spec/serialization.md`), and writing the impl
by hand is E9000 (issue #402). The type keeps them in
its `: A, B` list and a comment after it says so, so the expansion states where behavior comes from that is not in
the text.

An interface listed for its default methods (`spec/derives.md`) is source already and stays in the list.

Tests: every golden with a built-in derive.

## INS6. The expansion is a program

The output is Burdin source, and `burdin check` accepts it wherever it accepted the input: an expansion that did not
re-check would be a printer lying about what the compiler built. `tests/check_inspect.py` expands every program of
`m17_arguments`, `m19_operators`, `m20_comptime`, and `m21_reflect` that builds, checks the expansion with `burdin check`,
and, for a program that runs, runs both and compares what they print. A rewrite that cannot be written as source is
skipped (INS2), so the guarantee costs completeness, never correctness. Line and column numbers of untouched code do not
change, so a panic reported in an expanded program names the same lines as in the original when the layout allows.

Tests: `check_expand_reparse`.

## INS7. `--emit <kinds>`

`burdin build --emit <kinds>` writes artifacts next to the output, one file per kind, named after the output's stem
(`-o app` gives `app.ll`; `app.exe` on Windows gives `app.ll`, not `app.exe.ll`). Kinds are separated by commas and may
repeat; the build still produces the executable.

| Kind | File | Content |
| --- | --- | --- |
| `tir` | `.tir` | the typed tree of the program's own functions and closures, after sema and compile-time evaluation: a debug dump, not a stable format (spans print as `@<file>:<lo>..<hi>`) |
| `llvm-ir` | `.ll` | the LLVM IR code generation produced, before optimization |
| `llvm-ir-opt` | `.opt.ll` | the IR after the profile's optimization pipeline, what the backend compiles |
| `llvm-bc` | `.bc` | the optimized module as bitcode |
| `asm` | `.s` | the assembly, with the debug line table directives |
| `obj` | `.o` (`.obj` with MSVC) | the object file |
| `deps` | `.deps` | the files the build read besides the sources (`embed`), one per line: the content of `--emit-deps` |

`llvm-bc`, `asm`, `obj`, and `llvm-ir-opt` describe one module, so a build that asks for any of them uses one code
generation unit instead of splitting a large program. `--emit-ir <file.ll>` still writes the unoptimized IR where the
file says.

Tests: `check_emit`.

## INS8. `--emit` and its options

`--intel` and `--att` choose the x86 assembly dialect of `--emit asm` (the default is the target's: AT&T on x86); on a target
that is not x86 they change nothing. Using either without `--emit asm` is a usage error (status 2), as is an unknown kind
(the message lists the kinds) and `--emit` on any command but `build`. A program with errors writes none of the files.

Tests: `check_emit`.

## INS9. `burdin asm`

`burdin asm <target> [<function>]` builds the program as `burdin build --release` does (`--debug` builds the debug profile)
with debug information, writes the assembly to a temporary directory, and prints the one function's part, from its label
to the end of the function. The function is named as the program names it: `main`, `module.name`, `Type.method`, an
extension as `Receiver.method`, or an instance with its type arguments such as `largest<i32>`; `::` is accepted for `.`.
A name matches a function if it is the function's whole name or a `.`-separated tail of it, and a whole-name match beats a
tail match (`main` is the program's, not `util.main`). Without a function the command lists the functions that have code,
the program's own first and the standard library's marked `[std]`.

Tests: `check_asm`.

## INS10. The form of the output

Each run of instructions that came from one source line is preceded by a comment with that line's text and its place
(`file:line`), taken from the line table, and the directives only the assembler reads are dropped. `--no-source` leaves the
source out. `--intel` and `--att` choose the dialect (INS8). `--llvm` prints the function's LLVM IR after optimization
(`define ...`) instead of assembly, built without debug metadata so the IR is readable.

Tests: `check_asm`.

## INS11. `burdin asm` refusals

A generic function named without type arguments is a refusal (status 2) that lists its instances. An unknown name is a
refusal (status 2) that says how to list the functions; a name that matches several functions lists them; a function that
exists but has no code (nothing calls it, so none was generated) says so. A program with errors reports the diagnostics
(status 1). Usage errors (a missing target, an unknown option, a second function) are status 2.

Tests: `check_asm`.

## INS12. `burdin check --print-instances`

After a clean check, `--print-instances` prints the monomorphization report to standard output (nothing else is printed
by a clean check, so the output is the report). It is the checked program's instances, so it costs no extra compile.
`--format json` prints the report as one JSON object on one line with `"version": 1`; `--format` without
`--print-instances` is a usage error. A program with errors prints diagnostics and no report.

Tests: `check_print_instances`, `tests/inspect/golden/print_instances*.golden`.

## INS13. What is listed

The first line counts function instances and generics, then type instances and generics. Each generic function or method
is a group named without its type arguments (`largest`, `Stack.push`) with its instances, and each generic struct or enum a
group with its instances, the generic with the most instances first (a stable sort, so ties keep the order the program first needed them). An instance is listed when code
generation emits it. The standard library checks every body it declares, so it creates instances nothing reaches; they are
counted on a `not listed:` line and not listed.

Tests: `check_print_instances`.

## INS14. Why an instance exists

Sema allocates an instance when a call or a type needs it, and keeps no record of why. The reason is derived from the
checked program: the call graph of the typed tree is walked breadth-first from the code that is not an instance (every
ordinary function, the program's own before the standard library's), and the first call that reaches an instance is the
call that required it. A function instance lists that call (`required at file:line:col in <function>`) and, while the
function is itself an instance, the call that required that one, down to the first function that is not generic. A
breadth-first walk gives the shortest chain and ties go to the earlier function and call, so the answer is the same on
every run. A type instance names the first function whose signature or locals mention it. An instance with no caller (a
`Drop` implementation, a vtable entry) has an empty chain.

Tests: `check_print_instances`.

## INS15. The JSON report

`{"version":1, "function_instances", "type_instances", "unreachable_function_instances", "unreachable_type_instances",
"functions": [...], "types": [...]}`. A group is `{"generic", "count", "instances": [...]}`; a function instance is
`{"name", "declared": "file:line", "std", "required_by": [{"function", "at"}]}` and a type instance is `{"name", "kind":
"struct" | "enum", "first_use": {"function", "at"}}`. The schema is versioned: a field is added without bumping `version`, never changed or
removed.

Tests: `check_print_instances`.

## INS16. `burdin bloat`

`burdin bloat <target>` builds the program as `burdin build --release` does (`--debug` for the debug profile) as one object
file, reads the size of each symbol in it, and reports where the size comes from. It measures the object, not the linked
executable, whose total is printed as a reference: linking adds the C library's startup code, which is not the program's.
A program with errors reports the diagnostics (status 1).

Tests: `check_bloat`.

## INS17. What is measured

- `executable`, the size of the linked file; `code`, the bytes of function symbols and how many; `data`, the bytes of data
  symbols and how many.
- `largest functions`, the `--top n` (default 20) largest, with their share of the code.
- `size per package` and `size per module`, the sizes of the functions each owns, with a module's path as written
  (`std fmt`, `app (root)`).
- `size per generic function (instances added up)`: the sum of a generic's instances, how many there are, and the largest
  instance, `--top n` of them.
- `code that is not a source function`: the shared drop and clone glue, the runtime (`burdin_rt_*`), and anything else.

A symbol is a source function when its name is `burdin.<function>` for a function of the program (or a closure), which is
how code generation names it; the module is the one the function's file belongs to.

Tests: `check_bloat`.

## INS18. The formats

The human form is the sections above as tables with sizes in bytes with `,` separators and shares of the code. `--format
json` prints one object with `"version": 1` and the same facts: `title`, `executable`, `code`, `data`, `functions` (the
count), `largest`, `packages`, `modules`, `generics` (`generic`, `instances`, `size`, `largest`), `glue`, `runtime`, and `other`. The
numbers depend on the target, the LLVM version, and the program, so the tests check the structure and the relations
(sorted, summed, counted), not the values.

Tests: `check_bloat`.

## INS19. Object files

The object reader understands ELF, Mach-O, and COFF (the three the compiler produces) and reads only symbol sizes: from
the symbol table's `st_size` on ELF, from the distance to the next symbol in the section on Mach-O (which has no size
field), and from the section sizes and the symbol table on COFF. A file the reader does not recognize is an error (status
2) that says so.

Tests: the unit tests of `burdin_inspect::object` (synthetic files in each format), `check_bloat` (the format of the
platform it runs on).

## Choices that are not obvious

- **Patch the text, do not print the tree.** The issue describes `expand` as a printer over the rewritten typed tree. The
  tree has lost the source's layout and comments, spells string interpolation as calls to the formatter, and cannot name
  things that are only in scope in another module, so a printer over it would be a second pretty-printer for the whole
  language that must also pass the type checker. Patching keeps the author's text, and uses the typed tree for what only
  it knows: what a `comptime` evaluated to, what an operator resolved to, which branch was chosen.
- **A rewrite that cannot be printed is skipped.** The guarantee that the output checks (INS6) is the point of the
  command; an expansion that guessed would turn a debugging aid into a source of wrong beliefs.
- **The reasons for instances are derived.** Recording them in sema would touch the hottest path of the checker for a
  report one command prints; the call graph the checked program already holds gives the same answer.
- **`bloat` reads the object.** The executable's symbol table is stripped in release builds and platform linkers
  rearrange sections; the object file has every symbol with its size before the linker touches it.
- **`asm` cuts, it does not compile one function.** LLVM has no per-function output, and a single function compiled alone
  would differ from the one in the program (inlining, specialization). The whole program's assembly is generated and the
  function is cut out, so what prints is what runs.

## Test coverage

| Rule | Tests |
| --- | --- |
| INS1 | `tests/check_inspect.py`: `check_expand_goldens`, `check_expand_item_and_instances`, `check_expand_package`, `check_expand_failures` |
| INS2-INS5 | the goldens `tests/inspect/golden/expand/*.golden` |
| INS6 | `check_expand_reparse` over `m17_arguments`, `m19_operators`, `m20_comptime`, `m21_reflect` |
| INS7-INS8 | `check_emit`, the unit tests of `driver/src/emit.rs` |
| INS9-INS11 | `check_asm`, the unit tests of `burdin_inspect::asm` |
| INS12-INS15 | `check_print_instances`, the unit tests of `burdin_inspect::instances` |
| INS16-INS19 | `check_bloat`, the unit tests of `burdin_inspect::bloat` and `::object` |
