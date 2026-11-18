# `burdin bindgen`

`burdin bindgen` moves declarations between C and Burdin in both directions. The import side reads a C header and
writes a Burdin module of raw declarations: `extern "C"` prototypes, `repr(C)` structs, type aliases, enumerations,
and constants. The export side (`--export`) writes a C header for what a Burdin package exports.

The import side reads the header through **libclang**, the C API of the clang compiler, which `burdin bindgen`
loads at run time. The compiler itself never links libclang and does not need it: `cargo build` and every other
command work on a machine without it.

The generated module is declarations only. It carries no wrappers, no ownership, and no safety: it is what the C
header says, with `CPtr<T>` for a pointer and `unsafe` at every use. A safe wrapper is the programmer's work, on top
of these declarations.

## BG1. libclang is found and loaded at run time

The loader looks in this order, and uses the first file that opens and has every function this tool needs:

1. `BURDIN_LIBCLANG`, when it is set. It is then the **only** candidate, so a wrong path is reported instead of
   being silently replaced by another library.
2. `LIBCLANG_PATH`, either the library file itself or a directory.
3. `$LLVM_SYS_211_PREFIX/lib` and `$LLVM_SYS_211_PREFIX/bin`.
4. The directory `llvm-config --libdir` prints (from `$LLVM_SYS_211_PREFIX/bin/llvm-config`, or `llvm-config` on
   `PATH`).
5. On Unix, every `/usr/lib/llvm-*` tree, `llvm-21` first, then the usual multi-arch and Homebrew directories; on
   Windows, `C:\Program Files\LLVM\bin`.
6. Finally the dynamic loader's own search path: `libclang-21.so.1`, `libclang.so.1`, `libclang.so`,
   `libclang.dylib`, or `libclang.dll`.

`libclang-cpp` is the C++ API and is never used. When no library is usable the command exits 2 and prints what it
tried plus how to install libclang (`libclang-21-dev` on Debian and Ubuntu, `brew install llvm@21` on macOS, the
LLVM release archive on Windows).

## BG2. Only the header named on the command line is generated

`burdin bindgen <header> --out <file>` parses `<header>` as C (`-x c`), with `-I`, `-D`, and `--clang-arg` passed to
the C front end, so `#include`, the preprocessor's conditional state, and the system headers behave exactly as they
do for the C compiler. The header's own declarations become the module; the module's first line names the header
exactly as it was given.

A declaration from an included header is not generated as an item of its own. It is either collapsed to what it is
made of (a typedef such as `size_t`, an enumeration to its integer type) or declared on demand, when it has an
identity of its own:

| Reached as | Generated |
| --- | --- |
| a struct used **by value** in this header (a field, a parameter, a result) | the struct, in full, with its fields |
| a struct used **only through a pointer** | a declaration with no fields, and a note saying it is declared elsewhere |
| an opaque struct (only ever declared, never defined) | a declaration with no fields |
| an enumeration | the integer type the header's values need |

A typedef declared in the header being read keeps its name (`type shape_id = u32`), which is what a reader of the C
header expects to find. A typedef from an included header is only what it stands for, except for the portable names
in BG3.

## BG3. C types and their Burdin spelling

| C | Burdin |
| --- | --- |
| `char` | `CChar` (an `i8`) |
| `signed char`, `unsigned char` | `i8`, `u8` |
| `short`, `unsigned short` | `i16`, `u16` |
| `int`, `unsigned int` | `i32`, `u32` |
| `long`, `unsigned long` | `i32`/`u32` when `long` is 32 bits, `i64`/`u64` otherwise |
| `long long`, `unsigned long long` | `i64`, `u64` |
| `float`, `double` | `f32`, `f64` |
| `_Bool` | `u8`, because Burdin's `bool` is one bit |
| `wchar_t` | `u16` or `i32`, by its size |
| `size_t`, `uintptr_t` | `usize` |
| `ssize_t`, `ptrdiff_t`, `intptr_t` | `isize` |
| `int8_t` ... `uint64_t` | `i8` ... `u64` |
| `T *` | `CPtr<T>`; `void *` is `CPtr<CVoid>`, `char *` is `CPtr<CChar>` |
| `T[n]` | `[T; n]` |
| `T f(...)` (a function pointer) | `CPtr<CVoid>` (BG6) |
| an enumeration | its integer type, or the alias the header declares (BG5) |

The fixed-width and pointer-sized names in the table are their primitive wherever they are declared, because that is
what they are. `long` follows the target the C front end is configured for, which is the host: 32 bits on Windows
and 64 bits elsewhere.

A type with no Burdin equivalent (a vector type, an unknown builtin) fails conversion, and the declaration that needs
it is skipped with a note naming it (BG9). Nothing is guessed.

## BG4. Structs, unions, and the layout check

A struct whose fields Burdin lays out exactly as C does becomes a `repr(C)` struct with the same fields. Its size
and alignment are checked at compile time, so a disagreement between the bindings and the C compiler is a compile
error rather than a wrong read:

```
struct Vec2 repr(C) {
    x: f64
    y: f64
}
comptime assert(Vec2.size() == 16, "`Vec2` must have size 16, as in the C header")
comptime assert(Vec2.align() == 8, "`Vec2` must have alignment 8, as in the C header")
```

A struct whose layout Burdin cannot spell field by field is declared as bytes of C's size and alignment, with a
comment listing every member and its offset. That covers:

- a **union** (all members share an offset);
- a **packed** struct, or any struct where a field's offset is not where Burdin would put it;
- a **bit-field** (`unsigned ready : 1`);
- a **flexible array member** (`uint8_t data[]`);
- an **anonymous struct or union member**, whose members belong to the outer struct;
- a field with no fixed offset, or no Burdin spelling.

The bytes are the word of the struct's alignment (`u8`, `u16`, `u32`, or `u64`) repeated to the size, so
`bytes: [u64; 2]` is a 16-byte, 8-aligned struct:

```
// `Number` is a C union: it is declared as words with C's size and alignment
// members: `i`: i32 at 0, `d`: f64 at 0, `bytes`: [u8; 12] at 0
struct Number repr(C) {
    bytes: [u64; 2]
}
```

A struct whose size is not a multiple of its alignment, or whose alignment (16 and up) has no Burdin integer, is
declared with no fields at all, and a note says its layout cannot be reproduced. It can still be used through
`CPtr<T>`, which is what C code does with it.

Type names are unique in the module. C keeps struct tags and function names in separate namespaces and Burdin does
not, so a tag that equals a function's name gains a trailing underscore (`struct open` → `struct open_`). A struct
or union nested inside another gets the name `<parent>_<field>` when it has no tag of its own.

## BG5. Enumerations

A named enumeration becomes an alias of its integer type plus one constant per enumerator:

```
type ShapeKind = i32
const SHAPE_POINT: i32 = 0
const SHAPE_LINE: i32 = 5
const SHAPE_POLY: i32 = 6
```

The integer type is the smallest of `i32`, `u32`, `i64`, and `u64` that holds every enumerator. C leaves the
underlying type to the implementation and only its size has to match, and the Itanium and MSVC ABIs pick different
signedness for the same values; choosing from the values makes the same header generate the same module on every
platform while the size, and therefore the layout of any struct that holds the enumeration, still matches C.

An enumeration with no tag contributes its constants and no type, unless a typedef in the same header names it, in
which case that name is the type. A forward declaration (`enum E;`) contributes nothing.

## BG6. Function pointers and variadic functions

Burdin has no function-pointer type across the C ABI, so every C function pointer is a `CPtr<CVoid>`, both as a
typedef and as a parameter or field, and a note says so:

```
type visit_fn = CPtr<CVoid>
```

This is a deliberate loss of type information, not an error: the pointer is passed through untouched, and calling
through it is the programmer's `unsafe` work. A `static inline` function whose parameter is a function pointer is
wrapped as BG8 describes.

A variadic function keeps `...` in the declaration, which is the one place a Burdin `extern "C"` declaration takes
it: `fn printf_like(format: CPtr<CChar>, ...): i32`.

A parameter declared as an array (`const int values[]`, `char name[16]`) is a pointer to its element, because that is
how C passes it: `values: CPtr<i32>`, `name: CPtr<CChar>`.

## BG7. Macros and constants

An object-like macro whose body is a constant expression becomes a `const`. The body is evaluated with C's rules:
integer literals with their suffixes (`u`, `l`, `ul`, `ll`, `ull`) and bases (`0x`, `0`, `0b`), character literals
(as their value), adjacent string literals concatenated, casts, arithmetic with the usual conversions, comparisons,
`?:`, and names resolved to other macros or to enumerators. A macro may name one declared later in the header; the
header's macros are collected before any is evaluated.

| C | Generated |
| --- | --- |
| `#define COUNT 10` | `const COUNT: i32 = 10` |
| `#define WIDE 5000000000` | `const WIDE: i64 = 5000000000` |
| `#define UNSIGNED 4000000000u` | `const UNSIGNED: u32 = 4000000000` |
| `#define RATIO 1.5` | `const RATIO: f64 = 1.5` |
| `#define GREETING "hello" " " "world"` | `const GREETING: str = "hello world"` |
| `#define CAST ((uint8_t)300)` | `const CAST: u8 = 44` |
| `#define BOTH (COUNT > 5 ? 1 : 2)` | `const BOTH: i32 = 1` |

A body that contains a literal but cannot be evaluated (`#define CALL getpid()`,
`#define NOT_A_CONSTANT sizeof(int)`) is skipped with a note. A body with no literal (`#define EMPTY`, a name that
resolves to nothing) is skipped silently, since it is usually a guard or a feature test. A string with a character
Burdin cannot spell is skipped with a note too.

A function-like macro (`#define SQUARE(x) ...`) is not a constant. The module carries one note counting them, rather
than one per macro, because a header full of them is normal.

A `const` C variable of arithmetic type whose value C can evaluate is a constant in the same way. Any other global
variable is skipped with a note: the C bridge declares functions, not variables.

`long` in a macro body follows the target, as in BG3.

## BG8. `static`, `static inline`, and `--wrappers`

A `static inline` function has no symbol to link against. With `--wrappers <file.c>`, bindgen writes a C file that
calls it and exports the call under `<name>__extern`, and the module declares that function instead:

```c
int add__extern(int arg0, int arg1) {
    return add(arg0, arg1);
}
```

The wrapper file starts with the `#include` line for the header: `--wrapper-include <text>` gives it (the default is
the header path as given on the command line). A parameter that is a function pointer is passed to the wrapper as
`void *` and cast back, because the pointer type has no spelling without its declarator.

A `static` function that is only declared (no body in the translation unit) is skipped with a note, since there is
nothing to call. A variadic `static inline` function, or one that returns a function pointer, cannot be wrapped and
is skipped with a note. Without `--wrappers`, every `static inline` function is skipped with a note that names the
option.

The generated wrapper file is always written when `--wrappers` is given, even when the header has no `static inline`
function, so a build script can compile it unconditionally. Link it with `--link-lib <file.o>` (or a static archive,
or a shared library); a `--link-lib` value that names a file is linked as it is.

## BG9. What is skipped, and how it is reported

Every declaration that is skipped or changed is named in a `// bindgen: <note>` line at the top of the module, and
the same line goes to stderr as a warning. A note names the declaration, its line in the header, and the reason:
`the function `in` at types.h:68 was skipped: its name is reserved or invalid`. Nothing is guessed at and nothing is
silently dropped except the three cases BG7 says are silent (a macro body with no literal, a builtin macro, and an
empty `#define`).

The reasons a declaration is skipped:

| Reason | Example |
| --- | --- |
| the name is a Burdin keyword | `int in(int type)` |
| the name is not a Burdin identifier | a macro whose name starts with a digit |
| a struct or union is passed or returned **by value** | `struct Pair make(void)`; the C bridge calls through `CPtr<T>` only |
| a parameter or result has no Burdin equivalent | an array of unknown length outside a parameter |
| a second declaration of a name already generated | a repeated prototype |
| a global variable that is not a constant | `extern int errno` |

When a field or a parameter is a Burdin keyword, the name is not dropped: it gains a trailing underscore
(`int type` becomes `type_`), because the field still has to be there. An unnamed parameter becomes `arg0`, `arg1`,
and so on, in order.

The command exits 0 when the header parsed, whatever was skipped, and 1 when clang reported an error in the header
(the errors are printed as clang formatted them). A usage or I/O error, and a libclang that cannot be loaded, exit 2.

## BG10. `--export`: a C header for what a package exports

`burdin bindgen --export <package or file> --out <header.h> [--guard <NAME>]` goes the other way. It checks the
package as `burdin test` does (a library package needs no `main`), then writes one C header for:

- every `extern "C" fn` **with a body** in a module of the entry package that is not `priv`, as a prototype;
- every `repr(C)` struct those prototypes reach, by pointer or by value, including through the fields of a struct it
  already declared, as a definition, with the structs in dependency order;
- a struct that is not `repr(C)` and is only reached through a pointer, as an opaque declaration (`struct Counter;`
  and `typedef struct Counter Counter;`), which is enough for the pointer;
- every public integer, floating-point, string, and `bool` constant of the modules that export a function, as a
  `#define`, with the suffix that keeps it the right width (`U`, `LL`, `ULL`) and parentheses around a negative
  value.

The header includes `<stdbool.h>`, `<stddef.h>`, and `<stdint.h>`, and wraps its declarations in an
`#ifdef __cplusplus extern "C" { ... }` block so C++ can use it. Its include guard is `BURDIN_<STEM>_H` for an
output file named `<stem>.h`, or `--guard <NAME>`.

Types are spelled with the fixed-width names where Burdin has one: `i8` ... `u64` become `int8_t` ... `uint64_t`,
`isize` becomes `ptrdiff_t`, `usize` becomes `size_t`, `f32` and `f64` become `float` and `double`, a `bool` field
becomes `bool`, `CPtr<T>` becomes `struct T *` (or `T *` for a scalar), `CPtr<CChar>` becomes `char *`, `CPtr<CVoid>`
becomes `void *`, and a `[CChar; n]` field becomes `char name[n]`. A parameter or field whose name is not a C
identifier gains a trailing underscore.

The Burdin program's symbols must be visible to the C library that calls them. On Linux the driver links every
program with `-rdynamic`, so a shared library loaded at run time and linked with `--link-lib` finds them.

## BG11. What a C header cannot spell (E0263)

Some declarations have no place in a C header. Each is reported at its name as **E0263**, every one of them is
reported (not just the first), and no header is written:

| Declaration | Reason |
| --- | --- |
| an `extern "C" fn` named like a C keyword (`int`, `double`) | C reserves the name |
| a constant whose name is a C keyword or not a C identifier | the same |
| two exported functions, or a function and a constant, with the same C name | C has one namespace |
| a pointer to a generic struct (`CPtr<Wrapper<i32>>`) | C has no generics |
| a struct held **by value** that is not `repr(C)` | C cannot see its layout |
| a field whose name is not a C identifier | C cannot declare the name |
| a parameter or result with no C spelling | C has no such type |

The command exits 1 when any of them is reported, exactly like a compile error, and the package must be fixed before
a header can be written.

## Tests

`tests/bindgen/`:

| Fixture | Covers |
| --- | --- |
| `headers/types.h` | functions, typedefs, structs, enumerations, constants, an array parameter, an unnamed parameter, and a name Burdin reserves |
| `headers/layout.h` | a union, a packed struct, bit-fields, a flexible array member, an anonymous member, an alignment with no Burdin integer, and a struct nested in a struct |
| `headers/macros.h` | integer, character, float, string, cast, and conditional macros, an octal and a binary literal, suffixes, a function-like macro, and macros that are not constants |
| `headers/callbacks.h` | function-pointer typedefs and fields, a function pointer as a parameter, and a variadic function |
| `headers/inline.h` | `static inline` functions wrapped in C, a variadic one that cannot be, and a `static` declaration with no body |
| `headers/outer.h` and `headers/include/inner.h` | a struct from an included header used by value, one used only through a pointer, a constant built from an included macro, and `-I` |
| `headers/options.h` | `-D` and `--clang-arg` changing which declarations the preprocessor keeps |
| `headers/broken.h` | a header clang rejects: exit 1 with clang's error |
| `expected/*.bd`, `expected/inline_wrappers.c` | the golden modules and wrapper file, byte for byte (compared on every platform but Windows, whose C ABI the goldens do not assume) |
| `export/` | a package that exports functions, `repr(C)` structs, and constants, and the C program (`c/user.c`) that calls them through the generated header |
| `export/expected.h` | the golden export header |
| `export_invalid/` | five declarations a C header cannot spell, one E0263 each |

`tests/check_bindgen.py` runs all of it: it generates each golden, compiles the generated module and runs its
`comptime assert`s, builds the wrappers with the system C compiler and calls them through `--link-lib`, compiles a C
program and a C++ program against the export header, and on Linux builds a C shared library that calls the Burdin
exports and checks what it prints. It also checks the failure paths: a missing header and an unknown option exit 2,
a header clang rejects exits 1, and every E0263 is reported.

`tests/m22_build/bindgen` is the conformance test: a `build.bd` script invokes `burdin bindgen` on a header of its
own, and the generated module's constant, struct size, enumeration, and libc `strlen` call all work.

The parts the black-box check cannot reach are unit tests in the crate: `crates/burdin_bindgen/src/macros.rs` for
the evaluation of macro bodies, `model.rs` for the rendering of each layout, and `export.rs` for C's spelling of a
type.
