//! Source files, spans, and diagnostics.
//!
//! Every diagnostic carries an error code from the registry in `tests/README.md`, a severity, and a primary span.
//! It may also carry related notes (with their own spans) and suggestions (edits with an applicability, `spec/fix.md`), which
//! the JSON format exposes for an editor's code actions (issue #435).
//!
//! Four formats are rendered from the same data (issues #434, #642, #643):
//!
//! - `human`: the default. It renders cargo/gleam-style snippets (`error[E0000]: message`, `  --> path:LINE:COL`,
//!   source lines with a line-number gutter, `^`/`-` underlines, labels, and numbered diff suggestions), colors
//!   when asked to, and wraps long messages and source lines to a fixed width. When color is on and the terminal
//!   supports hyperlinks, the code links to its entry in the error reference (issue #644).
//! - `short`: exactly one plain line per diagnostic, with no notes, suggestions, or color. This is the
//!   machine-readable format the test scripts parse.
//! - `json`: one version 1 JSON object per diagnostic on one line (`json.rs`), with spans, labels, notes, help,
//!   children, related information, suggestions with edits, the link to the error reference, and the rendered
//!   human text. Colors and wrapping never apply to it.
//! - `sarif`: one SARIF 2.1.0 log for all the diagnostics (`sarif.rs`), for code-scanning services.
//!
//! Diagnostics are sorted by file and byte offset (then by code, message, and insertion order, so equal spans are
//! stable), and the optional error limit is applied after sorting, which keeps the output deterministic
//! (issue #436).

use std::fmt::Write as _;

pub mod edit;
mod json;
mod sarif;

pub use json::{JSON_VERSION, Json};
pub use sarif::{Rule, SARIF_SCHEMA, SARIF_VERSION, Tool};

/// The root of the hosted error reference and lint reference. A code's page is `<URL_BASE>/errors/<code>` (and
/// `<URL_BASE>/lints/<code>` for a lint), so every link ends in the code. The scheme is stable across versions:
/// `tests/check_error_docs.py` pins it (`spec/diagnostics.md`, DG12).
pub const URL_BASE: &str = "https://ricardomilos7.github.io/burdin";

/// Index of a file in a [`SourceMap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub u32);

/// A byte range in one source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub file: FileId,
    pub lo: u32,
    pub hi: u32,
}

impl Span {
    pub fn new(file: FileId, lo: u32, hi: u32) -> Span {
        Span { file, lo, hi }
    }

    /// The smallest span covering both spans (which must be in the same file).
    pub fn to(self, other: Span) -> Span {
        Span { file: self.file, lo: self.lo.min(other.lo), hi: self.hi.max(other.hi) }
    }
}

pub struct SourceFile {
    pub path: String,
    pub text: String,
    line_starts: Vec<u32>,
}

impl SourceFile {
    pub fn new(path: String, text: String) -> SourceFile {
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        SourceFile { path, text, line_starts }
    }

    /// 1-based line and column of a byte offset. The column counts characters, not bytes.
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let position = self.position(offset);
        (position.line, position.column)
    }

    /// The byte range `[start, end)` (without trailing `\r\n`) of a 1-based line.
    pub fn line_byte_range(&self, line: u32) -> (usize, usize) {
        let index = (line as usize).saturating_sub(1);
        let Some(&start) = self.line_starts.get(index) else { return (self.text.len(), self.text.len()) };
        let mut end = self.line_starts.get(index + 1).map_or(self.text.len(), |&next| next as usize);
        while end > start as usize && matches!(self.text.as_bytes()[end - 1], b'\n' | b'\r') {
            end -= 1;
        }
        (start as usize, end)
    }

    /// The text of a 1-based line, without its line break. An empty string past the last line.
    pub fn line_text(&self, line: u32) -> &str {
        let (start, end) = self.line_byte_range(line);
        &self.text[start..end]
    }

    /// How many lines the file has.
    pub fn line_count(&self) -> u32 {
        self.line_starts.len() as u32
    }

    /// The line of a byte offset and its column counted three ways, all 1-based: in characters (Unicode scalar
    /// values, what the human format prints), in UTF-8 bytes, and in UTF-16 code units.
    pub fn position(&self, offset: u32) -> Position {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let start = self.line_starts[line] as usize;
        let end = (offset as usize).min(self.text.len());
        let (column, utf16) = match self.text.get(start..end) {
            Some(prefix) => (prefix.chars().count(), prefix.chars().map(char::len_utf16).sum()),
            None => (end - start, end - start),
        };
        Position {
            line: line as u32 + 1,
            column: column as u32 + 1,
            utf8_column: (end - start) as u32 + 1,
            utf16_column: utf16 as u32 + 1,
        }
    }
}

/// A line and the columns of one offset. Every number is 1-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    /// Unicode scalar values from the start of the line.
    pub column: u32,
    /// UTF-8 bytes from the start of the line.
    pub utf8_column: u32,
    /// UTF-16 code units from the start of the line.
    pub utf16_column: u32,
}

#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn add(&mut self, path: String, text: String) -> FileId {
        self.files.push(SourceFile::new(path, text));
        FileId(self.files.len() as u32 - 1)
    }

    pub fn get(&self, id: FileId) -> &SourceFile {
        &self.files[id.0 as usize]
    }

    /// Replaces the text of a file, keeping its id and path. The query library uses it when an editor changes a
    /// file, so the ids the syntax trees carry stay valid.
    pub fn set_text(&mut self, id: FileId, text: String) {
        let file = &mut self.files[id.0 as usize];
        *file = SourceFile::new(std::mem::take(&mut file.path), text);
    }

    /// How many files the map holds.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Every source file with its id, in the order they were added.
    pub fn iter(&self) -> impl Iterator<Item = (FileId, &SourceFile)> {
        self.files.iter().enumerate().map(|(i, file)| (FileId(i as u32), file))
    }

    /// The first source file, if any.
    pub fn first(&self) -> Option<(FileId, &SourceFile)> {
        self.iter().next()
    }
}

/// An error code from the registry in `tests/README.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Code(pub &'static str);

impl Code {
    pub const SYNTAX: Code = Code("E0001");
    pub const UNKNOWN_NAME: Code = Code("E0002");
    pub const TYPE_MISMATCH: Code = Code("E0003");
    pub const ARG_COUNT: Code = Code("E0004");
    pub const IMMUTABLE: Code = Code("E0005");
    pub const NON_EXHAUSTIVE: Code = Code("E0006");
    /// A type, interface, function, constant, test, type parameter, field, variant, member, or import that
    /// repeats a name already declared in its namespace.
    pub const DUPLICATE_NAME: Code = Code("E0007");
    pub const LITERAL_RANGE: Code = Code("E0008");
    /// A declaration that depends on itself: a constant, a type alias, or a type that contains itself.
    pub const CYCLIC_DECLARATION: Code = Code("E0009");
    /// A second `..` in one pattern (`spec/rest-patterns.md`, RP3).
    pub const REST_REPEATED: Code = Code("E0010");
    /// A `..` in a pattern that stands for no element or field (RP4).
    pub const REST_IGNORES_NOTHING: Code = Code("E0011");
    /// A `..` before a field, or a field after a `..`, in a struct pattern (RP3).
    pub const REST_POSITION: Code = Code("E0012");
    /// A second `..base` in one struct literal (`spec/struct-update.md`, SU1).
    pub const BASE_REPEATED: Code = Code("E0013");
    /// A `..base` that is followed by a field or a separator (SU1).
    pub const BASE_POSITION: Code = Code("E0014");
    /// A `..base` on a struct-like enum variant (SU7).
    pub const BASE_VARIANT: Code = Code("E0015");
    /// A `..name` in a pattern that is not an array or slice pattern: a tuple has no rest binding (`spec/array-patterns.md`,
    /// AP6).
    pub const NAMED_REST: Code = Code("E0016");
    pub const MOVED: Code = Code("E0100");
    pub const MOVE_FROM_BORROW: Code = Code("E0101");
    pub const PARTIAL_MOVE_FROM_DROP: Code = Code("E0103");
    pub const MOVED_IN_LOOP: Code = Code("E0104");
    pub const EXCLUSIVITY: Code = Code("E0110");
    pub const BORROW_CONFLICT: Code = Code("E0111");
    pub const MUT_OF_IMMUTABLE: Code = Code("E0113");
    pub const MISSING_MUT_MARKER: Code = Code("E0114");
    pub const UNNEEDED_MUT_MARKER: Code = Code("E0115");
    pub const SECOND_CLASS_POSITION: Code = Code("E0120");
    pub const RETURN_MULTIPLE_ORIGINS: Code = Code("E0121");
    pub const RETURN_LOCAL_ORIGIN: Code = Code("E0122");
    pub const VIEW_OUTLIVES_SOURCE: Code = Code("E0123");
    pub const DERIVE_REQUIREMENT: Code = Code("E0130");
    /// A `Drop` implementation that is not exactly one method named `drop`.
    pub const INVALID_DROP_IMPL: Code = Code("E0131");
    pub const MISSING_INTERFACE_METHOD: Code = Code("E0140");
    pub const BOUND_NOT_SATISFIED: Code = Code("E0141");
    pub const ORPHAN_RULE: Code = Code("E0142");
    /// Two implementations of one interface for one type that apply to a common instance.
    pub const OVERLAPPING_IMPLS: Code = Code("E0143");
    /// An interface implementation's associated type bindings are wrong: a missing binding, a name the interface
    /// does not declare, or a binding in a block with no interface.
    pub const ASSOC_BINDING: Code = Code("E0144");
    /// An associated type projection that cannot be resolved: no interface declares that associated type, or more
    /// than one does and none is named.
    pub const ASSOC_PROJECTION: Code = Code("E0145");
    pub const PRIVATE_ITEM: Code = Code("E0150");
    pub const QUESTION_OUTSIDE: Code = Code("E0160");
    pub const UNSAFE_REQUIRED: Code = Code("E0161");
    /// A type or convention that cannot cross the `extern "C"` boundary.
    pub const C_ABI: Code = Code("E0162");
    /// An `extern` declaration with a runtime-reserved symbol, a generic function, or a signature that
    /// conflicts with an earlier declaration of the same symbol.
    pub const INVALID_EXTERN: Code = Code("E0163");
    /// A `repr` other than `repr(C)`.
    pub const UNSUPPORTED_REPR: Code = Code("E0164");
    /// A `test` attribute that is unknown, repeated, has a value it does not take, or conflicts with another.
    pub const TEST_ATTRIBUTE: Code = Code("E0170");
    /// A closure that borrows its captures used where it could outlive them.
    pub const CLOSURE_ESCAPES: Code = Code("E0171");
    /// A snapshot assertion written wrongly: an inline snapshot `@"..."` outside `assert_snapshot`, or an
    /// `assert_snapshot` call whose arguments are not a value, an optional inline snapshot, and `display:`.
    pub const INVALID_SNAPSHOT: Code = Code("E0172");
    /// An invalid `fuzz` block: a parameter list other than one `[u8]` parameter, a result type, or a name used
    /// by two fuzz blocks of one package.
    pub const INVALID_FUZZ: Code = Code("E0173");
    /// Type arguments given to a type that declares no type parameters.
    pub const NOT_A_GENERIC_TYPE: Code = Code("E0180");
    /// A format specifier that is malformed or does not apply to the formatted type.
    pub const BAD_FORMAT_SPEC: Code = Code("E0190");
    /// A placeholder that names an argument the format call does not have.
    pub const FORMAT_ARG_MISSING: Code = Code("E0191");
    /// An argument of a format call that no placeholder uses.
    pub const FORMAT_ARG_UNUSED: Code = Code("E0192");
    /// A placeholder whose value does not implement `Display` (or `Debug` for `{:?}`).
    pub const NOT_FORMATTABLE: Code = Code("E0193");
    /// A format call with arguments whose format string is not a string literal.
    pub const FORMAT_NOT_LITERAL: Code = Code("E0194");
    /// An argument of a format call that several placeholders use but that is neither a place nor `Copy`, so it
    /// cannot be evaluated once and reused.
    pub const FORMAT_ARG_REUSED: Code = Code("E0195");
    /// `any` or `impl` applied to something that is not an interface.
    pub const NOT_AN_INTERFACE: Code = Code("E0200");
    /// `impl Interface` outside a parameter list.
    pub const IMPL_NOT_PARAMETER: Code = Code("E0201");
    /// A function returning `impl Interface` whose concrete type cannot be found: its body returns through itself.
    pub const OPAQUE_UNKNOWN: Code = Code("E0205");
    /// An interface that cannot be used as `any` because it is not object-safe.
    pub const NOT_OBJECT_SAFE: Code = Code("E0202");
    /// An invalid `error` declaration: a `from` variant without exactly one tuple field, `from` on a generic
    /// error enum, or a message placeholder that does not name a payload field.
    pub const INVALID_ERROR: Code = Code("E0210");
    /// Two variants of one error enum converting from the same source type.
    pub const DUPLICATE_FROM: Code = Code("E0211");
    /// A missing error conversion: `?` between two error types with no `From` block, or a conversion into the
    /// dynamic `AnyError` from a type that does not implement it.
    pub const ERROR_CONVERSION: Code = Code("E0212");
    /// An invalid default argument: a default before a required parameter, on a `mut` parameter, in an
    /// interface signature, depending on a type parameter, or not a constant expression.
    pub const INVALID_DEFAULT: Code = Code("E0213");
    /// An invalid named argument: a name no parameter has, a parameter named twice, or a positional argument
    /// after a named one.
    pub const INVALID_NAMED_ARGUMENT: Code = Code("E0214");
    /// An extension method that a method of its receiver type shadows. A warning: the method is called and the
    /// extension is unreachable while the method exists (`spec/extensions.md`, EX4).
    pub const EXTENSION_SHADOWED: Code = Code("E0215");
    /// An invalid extension method declaration: no `self` first parameter, a receiver that is not a type name,
    /// or a `where` clause naming something that is not one of its type parameters.
    pub const INVALID_EXTENSION: Code = Code("E0216");
    /// A construct compile-time evaluation or unrolling cannot allow (`spec/const-eval.md`, CE5, `spec/reflection.md`, RFL4):
    /// `break` or `continue` leaving the unrolled loop over `Type.fields` or `Type.variants` (RFL4), or a constant or
    /// static initializer that names a static (GL1).
    pub const INVALID_CONST_EVAL: Code = Code("E0220");
    /// A `const` whose type cannot be materialized in the program (CE3).
    pub const NOT_MATERIALIZABLE: Code = Code("E0221");
    /// Constant evaluation panicked (CE7).
    pub const CONST_PANIC: Code = Code("E0222");
    /// An impure operation during constant evaluation (CE6).
    pub const CONST_IMPURE: Code = Code("E0224");
    /// Constant evaluation exceeded its step budget or call depth (CE8).
    pub const CONST_BUDGET: Code = Code("E0225");
    /// A reflection member used where it does not apply: `fields` or `variants` on a type that is neither a struct
    /// nor an enum (`spec/reflection.md`, RFL2).
    pub const INVALID_REFLECTION: Code = Code("E0230");
    /// A type declares a name the reflection members reserve: an associated function or an enum variant named
    /// `name`, `size`, `align`, `fields`, or `variants` (RFL3).
    pub const RESERVED_MEMBER: Code = Code("E0231");
    /// A computed field access `x.{name}` whose name is not a compile-time string (RFL5).
    pub const INVALID_COMPUTED_FIELD: Code = Code("E0232");
    /// An invalid derive declaration or use: a derive named like a built-in derive, or `struct T: Name` with no
    /// interface `Name` in scope (RFL8, RFL9).
    pub const INVALID_DERIVE: Code = Code("E0233");
    /// An interface after `:` on a struct or enum declaration that the declaration cannot implement by itself: it
    /// has a method with no default body and no `derive` declaration supplies one (`Drop`, `From`, `Display`, ...).
    /// `spec/derives.md`.
    pub const NOT_DERIVABLE: Code = Code("E0234");
    /// `embed` could not read its file: the file is missing, unreadable, or not UTF-8 text (`spec/const-eval.md`,
    /// CE12).
    pub const EMBED_FAILED: Code = Code("E0240");
    /// An invalid `embed` use: the path is not a string literal, is absolute, or has `..`, or the expected
    /// `[u8; N]` is not the file's length (CE12).
    pub const INVALID_EMBED: Code = Code("E0241");
    /// An invalid const generic declaration or argument: a type where a value is expected or the other way round,
    /// a const parameter of a type that is not an integer, `bool`, or `char`, or a value that does not fit the
    /// parameter's type (`spec/const-generics.md`, CG1-CG2).
    pub const INVALID_CONST_GENERIC: Code = Code("E0250");
    /// A const expression that cannot be evaluated at compile time: an unknown name, overflow, or a division by
    /// zero (CG4).
    pub const CONST_EVAL: Code = Code("E0251");
    /// A const argument that cannot be inferred from the call (CG6).
    pub const CONST_INFER: Code = Code("E0252");
    /// The build configuration is invalid: a dependency has a `build.bd` that the entry package's
    /// `[build] allow` list does not name, or the list names a package that is not a dependency
    /// (`spec/build-scripts.md`, BS2).
    pub const BUILD_CONFIG: Code = Code("E0260");
    /// A build script failed: it did not compile, could not be run, or ended with a non-zero status
    /// (`spec/build-scripts.md`, BS3).
    pub const BUILD_SCRIPT: Code = Code("E0261");
    /// A build script's generated modules are invalid: a path that is not a module name, a module that
    /// conflicts with a source module, or an output directory that cannot be read (BS4).
    pub const BUILD_OUTPUT: Code = Code("E0262");
    /// `burdin bindgen --export` cannot write a C declaration for an exported function: a type with no C spelling,
    /// a name that is not a C identifier, or two exports that share a C name (`spec/bindgen.md`, BG11).
    pub const EXPORT_HEADER: Code = Code("E0263");
    /// A Rust crate in `[rust.crates]` could not be bound: an invalid `[rust]` table, a toolchain that is not the
    /// pinned one, a crate that rustdoc or cargo could not build, or a shim whose layout checks failed
    /// (`spec/rust-interop.md`, RI7-RI9).
    pub const RUST_INTEROP: Code = Code("E0264");
    /// An invalid `[lib]` table, or a static or shared library with a `src/main.bd` (`spec/library.md`, LB1 and LB4).
    pub const LIBRARY_CONFIG: Code = Code("E0265");
    /// A type argument of an allocator-aware container that does not implement `Allocator`
    /// (`spec/allocators.md`, AL3).
    pub const NOT_AN_ALLOCATOR: Code = Code("E0270");
    /// An invalid default type parameter: one on a parameter that is not trailing, or on a declaration that does
    /// not take defaults (`spec/allocators.md`, AL5).
    pub const INVALID_TYPE_DEFAULT: Code = Code("E0271");
    /// A container with a stored allocator created with `new`, which has no allocator value to store; the
    /// program must call `new_in` (`spec/allocators.md`, AL4).
    pub const MISSING_ALLOCATOR: Code = Code("E0272");
    /// A value that is not `Send` (or `Sync`) where one is required: a task's closure, a value sent through a
    /// channel, or a shared value (`spec/shared.md`, SH4-SH6).
    pub const NOT_SEND: Code = Code("E0280");
    /// An invalid `Send` or `Sync` declaration: not written `unsafe`, naming a type that is not a struct or enum
    /// of this package, or an `unsafe` block for another interface (`spec/shared.md`, SH5).
    pub const INVALID_MARKER_IMPL: Code = Code("E0281");
    /// An invalid `blocking` marker: on a declaration that is not an `extern "C"` function, or two declarations
    /// of one C symbol that disagree about it (`spec/io.md`, IO4).
    pub const INVALID_BLOCKING: Code = Code("E0290");
    /// A `#!` shebang line anywhere but the first line of a file (`spec/scripts.md`, SC2).
    pub const SHEBANG_PLACEMENT: Code = Code("E0300");
    /// A warning: a form the package's edition deprecates and the next edition removes (`spec/fix.md`, FX11).
    pub const DEPRECATED: Code = Code("E0301");
    /// A `static` declared in a function body: statics are module items (`spec/globals.md`, GL1).
    pub const STATIC_ITEM: Code = Code("E0302");
    /// A static whose type is not plain data: it owns memory or has a destructor, so it would need dropping at exit
    /// (`spec/globals.md`, GL2).
    pub const STATIC_NOT_PLAIN: Code = Code("E0303");
    /// A borrow of a `static mut`: no reference may alias it (`spec/globals.md`, GL4).
    pub const STATIC_MUT_BORROW: Code = Code("E0304");
    /// A bare `Some`, `None`, `Ok`, or `Err`: `Option` and `Result` provide no bare names (`spec/target-typing.md`, TT13).
    pub const REMOVED_PRELUDE_NAME: Code = Code("E0305");
    /// Not in the registry: a construct the language has but this compiler does not implement yet.
    /// Every remaining site is assigned to a milestone in `spec/e9000-audit.md`, and `tests/check_e9000.py`
    /// fails when a site is unassigned, stale, or belongs to a milestone that has shipped.
    pub const UNSUPPORTED: Code = Code(UNSUPPORTED_CODE);

    /// The link to this code's entry in the hosted reference (`<URL_BASE>/errors/<code>`, or `/lints/<code>` for a
    /// lint), or `None` for a code the reference does not explain: `E9000` and anything that is not `E` or `L`
    /// followed by four digits.
    pub fn url(self) -> Option<String> {
        code_url(self.0)
    }
}

/// The code of [`Code::UNSUPPORTED`] as a plain string. [`code_url`] compares against this rather than the
/// constant, because `tests/check_e9000.py` audits every textual use of `Code::UNSUPPORTED` as a diagnostic site
/// and the link rule is not one.
const UNSUPPORTED_CODE: &str = "E9000";

/// [`Code::url`] for a code that is not a `&'static str`, such as one read from the error reference.
pub fn code_url(code: &str) -> Option<String> {
    let bytes = code.as_bytes();
    if bytes.len() != 5 || !bytes[1..].iter().all(u8::is_ascii_digit) || code == UNSUPPORTED_CODE {
        return None;
    }
    match bytes[0] {
        b'E' => Some(format!("{URL_BASE}/errors/{code}")),
        b'L' => Some(format!("{URL_BASE}/lints/{code}")),
        _ => None,
    }
}

/// The user-facing error-code reference, embedded so `burdin explain` and the REPL's `:explain` cannot disagree
/// with the page that `tests/check_error_docs.py` checks (issue #437).
const ERROR_DOCS: &str = include_str!("../../../docs/error-codes.md");

/// The section of the error reference for `code`, without the HTML comments the check uses to delimit examples.
/// `None` when the code is not in the reference.
pub fn explain_section(code: &str) -> Option<String> {
    let code = code.to_ascii_uppercase();
    let heading = format!("## {code}:");
    let start = ERROR_DOCS.find(&heading)?;
    let rest = &ERROR_DOCS[start..];
    let end = rest[3..].find("\n## ").map_or(rest.len(), |i| 3 + i);
    Some(
        rest[..end]
            .trim_end()
            .lines()
            .filter(|line| !line.trim_start().starts_with("<!--"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// The step-by-step fix for `code` from the error reference, without the Markdown marker: the paragraph that
/// follows `**How to fix:**` (`spec/check.md`, CK11). `None` when the code has no entry or no fix paragraph.
pub fn explain_fix(code: &str) -> Option<String> {
    let section = explain_section(code)?;
    let marker = "**How to fix:**";
    let start = section.find(marker)? + marker.len();
    let rest = &section[start..];
    let end = rest.find("\n\n").unwrap_or(rest.len());
    // One paragraph on one line: the renderer prefixes it with `fix:` and its own indentation.
    let text = rest[..end].split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

/// The JSON source location of a span, as the `json` diagnostic format writes it (`docs/diagnostics.md`, DG2). The
/// ownership event list (`spec/check.md`, CK10) uses it too, so every tool sees the same span shape.
pub fn span_json(sources: &SourceMap, span: Span) -> Json {
    json::location(sources, span)
}

/// Every heading in the reference, one per line, as `E0001: Syntax error`.
pub fn code_list() -> String {
    let mut out = String::new();
    for line in ERROR_DOCS.lines() {
        if heading_code(line).is_some() {
            out.push_str(line.trim_start_matches("## "));
            out.push('\n');
        }
    }
    out
}

/// The code a reference heading names, if the line is one.
fn heading_code(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("## ")?;
    let bytes = rest.as_bytes();
    if bytes.len() >= 6 && bytes[0] == b'E' && bytes[1..5].iter().all(u8::is_ascii_digit) && bytes[5] == b':' {
        Some(&rest[..5])
    } else {
        None
    }
}

/// How serious a diagnostic is. A warning is printed but does not stop compilation; the one warning today is a
/// shadowed extension method (E0215, `spec/extensions.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// A related piece of information. A note with a span points at another place in the source (for example where a
/// value was moved); a note without one is free-standing.
#[derive(Clone, Debug)]
pub struct Note {
    pub span: Option<Span>,
    pub message: String,
}

/// Text attached to a span of the diagnostic's own source. The human format renders it under the underlined
/// source line, and machine formats expose it to editors.
#[derive(Clone, Debug)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

/// How safe a suggestion is to apply without a person reading it (`spec/fix.md`, FX1). `burdin fix` and
/// `burdin lint --fix` apply only [`Applicability::MachineApplicable`]; an editor offers every level as a quick fix
/// and prefers the machine-applicable one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applicability {
    /// The edit is what the program means: applying it cannot change what a correct program does.
    MachineApplicable,
    /// The edit is probably what was meant, but it can change what the program does (a `.clone()` that copies a
    /// value, for example), so a person decides.
    MaybeIncorrect,
    /// The edit contains text a person has to fill in (`todo()` in a stub body, for example).
    HasPlaceholders,
    /// Nothing is known about the edit.
    Unspecified,
}

impl Applicability {
    /// The name in the documentation and in the `//@ suggest:` test directive.
    pub fn as_str(self) -> &'static str {
        match self {
            Applicability::MachineApplicable => "machine-applicable",
            Applicability::MaybeIncorrect => "maybe-incorrect",
            Applicability::HasPlaceholders => "has-placeholders",
            Applicability::Unspecified => "unspecified",
        }
    }

    /// The value of a suggestion's `applicability` field in the version 1 JSON format. Version 1 shipped with
    /// `"machine"` and `"manual"`, so those two keep their spelling; `"manual"` is [`Applicability::Unspecified`].
    pub fn json_name(self) -> &'static str {
        match self {
            Applicability::MachineApplicable => "machine",
            Applicability::MaybeIncorrect => "maybe-incorrect",
            Applicability::HasPlaceholders => "has-placeholders",
            Applicability::Unspecified => "manual",
        }
    }
}

/// One text edit: replace `span` with `replacement`. `span.lo == span.hi` inserts; an empty replacement deletes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub span: Span,
    pub replacement: String,
}

/// A fix: a message, the edits that make it (they apply together, all or none), and how safe it is. This is the
/// one edit model the compiler, the linter, `burdin fix`, the refactorings, and the language server's quick fixes
/// share (`spec/fix.md`, FX1).
#[derive(Clone, Debug)]
pub struct Suggestion {
    pub message: String,
    /// At least one edit. The edits of one suggestion never overlap.
    pub edits: Vec<Edit>,
    pub applicability: Applicability,
}

impl Suggestion {
    /// A suggestion that `burdin fix` and `burdin lint --fix` apply on their own.
    pub fn is_machine_applicable(&self) -> bool {
        self.applicability == Applicability::MachineApplicable
    }

    /// The first edit's span: where an editor anchors the fix.
    pub fn span(&self) -> Span {
        self.edits.first().map_or(Span::new(FileId(0), 0, 0), |e| e.span)
    }
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub code: Code,
    pub severity: Severity,
    pub span: Span,
    pub message: String,
    pub notes: Vec<Note>,
    pub suggestions: Vec<Suggestion>,
    /// Texts attached to spans (the primary span or others).
    pub labels: Vec<Label>,
    /// Advice that is not an edit, one line each.
    pub help: Vec<String>,
    /// Diagnostics that belong to this one, each with its own code, severity, and span.
    pub children: Vec<Diagnostic>,
}

impl Diagnostic {
    pub fn new(code: Code, severity: Severity, span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            code,
            severity,
            span,
            message: message.into(),
            notes: Vec::new(),
            suggestions: Vec::new(),
            labels: Vec::new(),
            help: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Attaches a label: `message` is the short text for `span`.
    pub fn label(&mut self, span: Span, message: impl Into<String>) -> &mut Diagnostic {
        self.labels.push(Label { span, message: message.into() });
        self
    }

    /// Attaches a line of advice that is not an edit.
    pub fn help(&mut self, message: impl Into<String>) -> &mut Diagnostic {
        self.help.push(message.into());
        self
    }

    /// Attaches a child diagnostic.
    pub fn child(&mut self, child: Diagnostic) -> &mut Diagnostic {
        self.children.push(child);
        self
    }

    /// Attaches a note that points at `span`.
    pub fn note(&mut self, span: Span, message: impl Into<String>) -> &mut Diagnostic {
        self.notes.push(Note { span: Some(span), message: message.into() });
        self
    }

    /// Attaches a note that does not point at a particular span.
    pub fn note_without_span(&mut self, message: impl Into<String>) -> &mut Diagnostic {
        self.notes.push(Note { span: None, message: message.into() });
        self
    }

    /// Attaches a suggested edit whose safety is not known: replace `span` with `replacement`.
    pub fn suggest(
        &mut self,
        span: Span,
        message: impl Into<String>,
        replacement: impl Into<String>,
    ) -> &mut Diagnostic {
        self.suggest_with(span, message, replacement, Applicability::Unspecified)
    }

    /// Like [`Diagnostic::suggest`], for an edit `burdin fix` and `burdin lint --fix` may apply on their own.
    pub fn suggest_safe(
        &mut self,
        span: Span,
        message: impl Into<String>,
        replacement: impl Into<String>,
    ) -> &mut Diagnostic {
        self.suggest_with(span, message, replacement, Applicability::MachineApplicable)
    }

    /// A one-edit suggestion with an explicit applicability.
    pub fn suggest_with(
        &mut self,
        span: Span,
        message: impl Into<String>,
        replacement: impl Into<String>,
        applicability: Applicability,
    ) -> &mut Diagnostic {
        let edit = Edit { span, replacement: replacement.into() };
        self.suggest_edits(message, vec![edit], applicability)
    }

    /// A suggestion made of several edits that apply together.
    pub fn suggest_edits(
        &mut self,
        message: impl Into<String>,
        edits: Vec<Edit>,
        applicability: Applicability,
    ) -> &mut Diagnostic {
        self.suggestions.push(Suggestion { message: message.into(), edits, applicability });
        self
    }
}

#[derive(Default)]
pub struct Diagnostics {
    list: Vec<Diagnostic>,
}

impl Diagnostics {
    /// Records an error and returns it, so notes and suggestions can be attached in the same expression:
    /// `diags.error(code, span, "message").suggest(...);`
    pub fn error(&mut self, code: Code, span: Span, message: impl Into<String>) -> &mut Diagnostic {
        self.list.push(Diagnostic::new(code, Severity::Error, span, message));
        self.list.last_mut().expect("a diagnostic was just pushed")
    }

    /// Records a warning. A warning is printed like an error but never stops compilation
    /// (`Diagnostics::has_errors` ignores it).
    pub fn warning(&mut self, code: Code, span: Span, message: impl Into<String>) -> &mut Diagnostic {
        self.list.push(Diagnostic::new(code, Severity::Warning, span, message));
        self.list.last_mut().expect("a diagnostic was just pushed")
    }

    /// Moves every diagnostic of `other` into this list, skipping exact duplicates (same code, span, and message).
    /// A generic body is checked once against its bounds and once per instantiation, so the same mistake can be
    /// found more than once, while distinct errors sharing a span (such as macro or expansion errors) are all kept.
    pub fn absorb(&mut self, other: Diagnostics) {
        for d in other.list {
            if !self.list.iter().any(|e| e.code == d.code && e.span == d.span && e.message == d.message) {
                self.list.push(d);
            }
        }
    }

    pub fn has_errors(&self) -> bool {
        self.list.iter().any(|d| d.severity == Severity::Error)
    }

    /// Whether at least one warning is waiting to be printed. Compilation continues.
    pub fn has_warnings(&self) -> bool {
        self.list.iter().any(|d| d.severity == Severity::Warning)
    }

    /// The number of errors in the list, whatever an error limit shows.
    pub fn error_count(&self) -> usize {
        self.list.iter().filter(|d| d.severity == Severity::Error).count()
    }

    /// The number of warnings in the list, whatever an error limit shows.
    pub fn warning_count(&self) -> usize {
        self.list.iter().filter(|d| d.severity == Severity::Warning).count()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Drops every diagnostic after the first `len`.
    pub fn truncate(&mut self, len: usize) {
        self.list.truncate(len);
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Adds a finished diagnostic. The linter builds its diagnostics first and filters them by configuration and
    /// suppression comments before they join the list.
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.list.push(diagnostic);
    }

    /// The diagnostics, in the order they were recorded.
    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.list
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.list.iter()
    }

    /// All diagnostics in source order, one plain line each. This is [`DiagnosticFormat::Short`] with no limit;
    /// the benchmark harness and older callers use it, and the driver uses [`Diagnostics::render_with`].
    pub fn render(&self, sources: &SourceMap) -> String {
        let options = RenderOptions { format: DiagnosticFormat::Short, ..RenderOptions::default() };
        self.render_with(sources, &options).text
    }

    /// Renders the diagnostics in the format and options asked for. The diagnostics are sorted first, so equal
    /// spans have a stable order, and the error limit is applied after sorting (issue #436).
    pub fn render_with(&self, sources: &SourceMap, options: &RenderOptions) -> Rendered {
        let sorted = self.sorted();
        let total = sorted.len();
        let shown = options.error_limit.unwrap_or(total).min(total);
        let mut text = String::new();
        if options.format == DiagnosticFormat::Sarif {
            text = sarif::log(&sorted[..shown], sources, &options.tool, total - shown);
        } else {
            for (i, d) in sorted[..shown].iter().enumerate() {
                match options.format {
                    DiagnosticFormat::Human => {
                        if i > 0 {
                            text.push('\n');
                        }
                        render_human(&mut text, d, sources, options);
                    }
                    DiagnosticFormat::Short => render_short(&mut text, d, sources),
                    DiagnosticFormat::Json => {
                        json::diagnostic(d, sources, rendered_text(d, sources)).write(&mut text);
                        text.push('\n');
                    }
                    DiagnosticFormat::Sarif => {}
                }
            }
        }
        Rendered { text, shown, total }
    }

    /// Sorted by file, then byte offset, then code, message, and insertion order. The insertion index makes ties
    /// deterministic even when the message and code are equal.
    fn sorted(&self) -> Vec<&Diagnostic> {
        let mut sorted: Vec<(usize, &Diagnostic)> = self.list.iter().enumerate().collect();
        sorted.sort_by(|(ia, a), (ib, b)| {
            (a.span.file.0, a.span.lo, a.span.hi, a.code.0, a.message.as_str(), *ia).cmp(&(
                b.span.file.0,
                b.span.lo,
                b.span.hi,
                b.code.0,
                b.message.as_str(),
                *ib,
            ))
        });
        sorted.into_iter().map(|(_, d)| d).collect()
    }
}

/// The presentation format of a rendered diagnostic.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiagnosticFormat {
    /// Colored (when enabled) and wrapped, with one line for each note and suggestion.
    #[default]
    Human,
    /// One plain line per diagnostic, in the form `path:LINE:COL: error[E0000]: message`.
    Short,
    /// One version 1 JSON object per line (`docs/diagnostics.md`).
    Json,
    /// One SARIF 2.1.0 log for the whole run.
    Sarif,
}

#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub format: DiagnosticFormat,
    /// Color the location, the severity, and the code. Only [`DiagnosticFormat::Human`] uses it.
    pub color: bool,
    /// Maximum line width in characters for the human format; 0 disables wrapping.
    pub width: usize,
    /// Print at most this many diagnostics after sorting; `None` prints all of them.
    pub error_limit: Option<usize>,
    /// Link the code of a human diagnostic to its entry in the error reference with an OSC 8 hyperlink. It applies
    /// only together with `color`: the driver turns it on when color is on and the terminal supports hyperlinks.
    pub hyperlinks: bool,
    /// The tool and rules a SARIF log names.
    pub tool: Tool,
}

impl Default for RenderOptions {
    fn default() -> RenderOptions {
        RenderOptions {
            format: DiagnosticFormat::Human,
            color: false,
            width: 80,
            error_limit: None,
            hyperlinks: false,
            tool: Tool::default(),
        }
    }
}

/// What a render produced: the text and the counts the driver needs for its summary line.
pub struct Rendered {
    pub text: String,
    pub shown: usize,
    pub total: usize,
}

// The ANSI styles. They are applied only in the human format and only when color is enabled, and they never
// change the visible text.
const RESET: &str = "\x1b[0m";
const SEVERITY_ERROR: &str = "\x1b[1;31m";
const SEVERITY_WARNING: &str = "\x1b[1;33m";
const GUTTER: &str = "\x1b[1;34m";
const SECONDARY: &str = "\x1b[1;34m";
const NOTE: &str = "\x1b[1;36m";
const HELP: &str = "\x1b[1;32m";
const DIFF_DEL: &str = "\x1b[1;31m";
const DIFF_ADD: &str = "\x1b[1;32m";
// An OSC 8 hyperlink is `ESC ] 8 ; ; URL ST text ESC ] 8 ; ; ST`; `ST` is `ESC \`.
const OSC8_OPEN: &str = "\x1b]8;;";
const OSC8_END: &str = "\x1b\\";
const OSC8_CLOSE: &str = "\x1b]8;;\x1b\\";
const TAB_WIDTH: usize = 4;

fn severity_style(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => SEVERITY_ERROR,
        Severity::Warning => SEVERITY_WARNING,
    }
}

fn char_display_width(ch: char) -> usize {
    if ch == '\t' {
        return TAB_WIDTH;
    }
    let c = ch as u32;
    if ch.is_control()
        || matches!(
            c,
            0x0300..=0x036F
                | 0x1AB0..=0x1AFF
                | 0x1DC0..=0x1DFF
                | 0x200B..=0x200F
                | 0x202A..=0x202E
                | 0x2060..=0x206F
                | 0x20D0..=0x20FF
                | 0xFE00..=0xFE0F
                | 0xFE20..=0xFE2F
                | 0xFEFF
                | 0xE0100..=0xE01EF
        )
    {
        return 0;
    }
    if matches!(
        c,
        0x1100..=0x115F
            | 0x231A..=0x231B
            | 0x2329..=0x232A
            | 0x23E9..=0x23EC
            | 0x23F0
            | 0x23F3
            | 0x25FD..=0x25FE
            | 0x2614..=0x2615
            | 0x2648..=0x2653
            | 0x267F
            | 0x2693
            | 0x26A1
            | 0x26AA..=0x26AB
            | 0x26BD..=0x26BE
            | 0x26C4..=0x26C5
            | 0x26CE
            | 0x26D4
            | 0x26EA
            | 0x26F2..=0x26F3
            | 0x26F5
            | 0x26FA
            | 0x26FD
            | 0x2705
            | 0x270A..=0x270B
            | 0x2728
            | 0x274C
            | 0x274E
            | 0x2753..=0x2755
            | 0x2757
            | 0x2795..=0x2797
            | 0x27B0
            | 0x27BF
            | 0x2B1B..=0x2B1C
            | 0x2B50
            | 0x2B55
            | 0x2E80..=0x303E
            | 0x3040..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE6F
            | 0xFF01..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F000..=0x1FAFF
            | 0x20000..=0x3FFFD
    ) {
        return 2;
    }
    1
}

fn display_width(text: &str) -> usize {
    text.chars().map(char_display_width).sum()
}

fn expand_tabs(raw: &str) -> String {
    if !raw.contains('\t') {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len() + 8);
    for ch in raw.chars() {
        if ch == '\t' {
            out.push_str("    ");
        } else {
            out.push(ch);
        }
    }
    out
}

/// Expands `\t` to spaces and maps each byte offset in `raw` to its 0-based visual display column.
fn expand_line_columns(raw: &str) -> (String, Vec<(usize, usize)>) {
    let mut expanded = String::with_capacity(raw.len() + 8);
    let mut map = Vec::with_capacity(raw.len() + 1);
    let mut col = 0usize;
    for (byte_idx, ch) in raw.char_indices() {
        map.push((byte_idx, col));
        if ch == '\t' {
            expanded.push_str("    ");
            col += TAB_WIDTH;
        } else {
            expanded.push(ch);
            col += char_display_width(ch);
        }
    }
    map.push((raw.len(), col));
    (expanded, map)
}

fn byte_to_vis_col(map: &[(usize, usize)], byte_in_line: usize) -> usize {
    match map.binary_search_by_key(&byte_in_line, |&(b, _)| b) {
        Ok(i) => map[i].1,
        Err(i) => {
            if i == 0 {
                0
            } else {
                map[i - 1].1
            }
        }
    }
}

fn render_short(out: &mut String, d: &Diagnostic, sources: &SourceMap) {
    let file = sources.get(d.span.file);
    let (line, column) = file.line_col(d.span.lo);
    let _ = writeln!(out, "{}:{}:{}: {}[{}]: {}", file.path, line, column, d.severity.as_str(), d.code.0, d.message);
}

#[derive(Clone)]
struct SnippetAnn {
    file: FileId,
    start_line: u32,
    start_vis: usize,
    end_line: u32,
    end_vis: usize,
    is_primary: bool,
    message: String,
}

#[derive(Clone)]
struct SingleLineLabel {
    start: usize,
    end: usize,
    is_primary: bool,
    message: String,
}

#[derive(Clone)]
struct MultiLineLabel {
    start_line: u32,
    end_line: u32,
    end_vis: usize,
    is_primary: bool,
    message: String,
}

type ExpandedLine = (String, Vec<(usize, usize)>, usize);
type LineCache = std::collections::HashMap<(FileId, u32), ExpandedLine>;
type DiffLine = (char, u32, String);
type SuggestionDiffs<'a> = Vec<(&'a Suggestion, Vec<DiffLine>)>;

fn build_snippet_ann(
    span: Span,
    is_primary: bool,
    message: String,
    sources: &SourceMap,
    line_cache: &mut LineCache,
) -> SnippetAnn {
    let file = sources.get(span.file);
    let mut hi = (span.hi as usize).min(file.text.len());
    let lo = (span.lo as usize).min(hi);
    let start_pos = file.position(lo as u32);
    if hi > lo && file.text.as_bytes().get(hi - 1) == Some(&b'\n') {
        let end_pos = file.position(hi as u32);
        if end_pos.line > start_pos.line && end_pos.column == 1 {
            hi -= 1;
            if hi > lo && file.text.as_bytes().get(hi - 1) == Some(&b'\r') {
                hi -= 1;
            }
        }
    }
    let end_pos = file.position(hi as u32);
    let mut vis_for = |line: u32, byte_abs: usize| -> usize {
        let (line_start, line_end) = file.line_byte_range(line);
        let rel = byte_abs.saturating_sub(line_start).min(line_end.saturating_sub(line_start));
        let entry = line_cache.entry((span.file, line)).or_insert_with(|| {
            let raw = file.line_text(line);
            let (exp, map) = expand_line_columns(raw);
            let w = map.last().map_or(0, |&(_, c)| c);
            (exp, map, w)
        });
        byte_to_vis_col(&entry.1, rel)
    };
    let start_vis = vis_for(start_pos.line, lo);
    let end_vis = vis_for(end_pos.line, hi);
    SnippetAnn {
        file: span.file,
        start_line: start_pos.line,
        start_vis,
        end_line: end_pos.line,
        end_vis,
        is_primary,
        message,
    }
}

fn render_human(out: &mut String, d: &Diagnostic, sources: &SourceMap, options: &RenderOptions) {
    let sev_style = severity_style(d.severity);
    let severity = d.severity.as_str();
    let code = d.code.0;
    let plain_prefix = format!("{severity}[{code}]: ");
    let prefix = if options.color {
        // The link wraps the code and nothing else, so the visible text is the same with or without it.
        let linked_code = match d.code.url().filter(|_| options.hyperlinks) {
            Some(url) => format!("{OSC8_OPEN}{url}{OSC8_END}{sev_style}{code}{RESET}{OSC8_CLOSE}"),
            None => format!("{sev_style}{code}{RESET}"),
        };
        format!("{sev_style}{severity}{RESET}[{linked_code}]: ")
    } else {
        plain_prefix.clone()
    };
    push_wrapped(out, &prefix, display_width(&plain_prefix), &d.message, options.width, HEADER_INDENT);

    let mut line_cache: LineCache = LineCache::new();
    let mut anns: Vec<SnippetAnn> = Vec::new();
    let mut primary_labels = d.labels.iter().filter(|l| l.span == d.span);
    let primary_msg = primary_labels.next().map(|l| l.message.clone()).unwrap_or_default();
    anns.push(build_snippet_ann(d.span, true, primary_msg, sources, &mut line_cache));
    for extra in primary_labels {
        anns.push(build_snippet_ann(extra.span, true, extra.message.clone(), sources, &mut line_cache));
    }
    for label in d.labels.iter().filter(|l| l.span != d.span) {
        anns.push(build_snippet_ann(label.span, false, label.message.clone(), sources, &mut line_cache));
    }
    for note in &d.notes {
        if let Some(span) = note.span {
            anns.push(build_snippet_ann(span, span == d.span, note.message.clone(), sources, &mut line_cache));
        }
    }

    let suggestion_diffs: SuggestionDiffs<'_> =
        d.suggestions.iter().map(|s| (s, edit::inline_diff_numbered(sources, &s.edits))).collect();

    let max_ann_line = anns.iter().map(|a| a.end_line).max().unwrap_or(1);
    let max_diff_line =
        suggestion_diffs.iter().flat_map(|(_, lines)| lines.iter().map(|&(_, l, _)| l)).max().unwrap_or(1);
    let gutter_width = max_ann_line.max(max_diff_line).to_string().len().max(1);
    let pad = " ".repeat(gutter_width);

    // Group annotations by file, primary file first.
    let mut file_order: Vec<FileId> = vec![d.span.file];
    for ann in &anns {
        if !file_order.contains(&ann.file) {
            file_order.push(ann.file);
        }
    }

    for (file_idx, &file_id) in file_order.iter().enumerate() {
        let file = sources.get(file_id);
        let anchor_span = if file_idx == 0 {
            d.span
        } else {
            anns.iter().find(|a| a.file == file_id).map_or(d.span, |a| {
                // Anchor on the earliest span in that file.
                let mut best = None;
                for l in &d.labels {
                    if l.span.file == file_id {
                        best = Some(best.map_or(l.span, |b: Span| if l.span.lo < b.lo { l.span } else { b }));
                    }
                }
                for n in &d.notes {
                    if let Some(s) = n.span
                        && s.file == file_id
                    {
                        best = Some(best.map_or(s, |b: Span| if s.lo < b.lo { s } else { b }));
                    }
                }
                let _ = a;
                best.unwrap_or(d.span)
            })
        };
        let (anchor_line, anchor_col) = file.line_col(anchor_span.lo);
        if options.color {
            let _ = writeln!(out, "{pad}{GUTTER}-->{RESET} {}:{}:{}", file.path, anchor_line, anchor_col);
        } else {
            let _ = writeln!(out, "{pad}--> {}:{}:{}", file.path, anchor_line, anchor_col);
        }

        if file.text.is_empty() || (anchor_span.lo as usize) > file.text.len() {
            continue;
        }

        let file_anns: Vec<&SnippetAnn> = anns.iter().filter(|a| a.file == file_id).collect();
        render_file_snippet(out, file_id, &file_anns, &line_cache, gutter_width, sev_style, options);
    }

    // Spanless notes and plain help lines.
    for note in d.notes.iter().filter(|n| n.span.is_none()) {
        let plain = format!("{pad} = note: ");
        let pfx = if options.color { format!("{pad} {GUTTER}={RESET} {NOTE}note{RESET}: ") } else { plain.clone() };
        let indent = format!("{pad}         ");
        push_wrapped(out, &pfx, display_width(&plain), &note.message, options.width, &indent);
    }
    for msg in &d.help {
        let plain = format!("{pad} = help: ");
        let pfx = if options.color { format!("{pad} {GUTTER}={RESET} {HELP}help{RESET}: ") } else { plain.clone() };
        let indent = format!("{pad}         ");
        push_wrapped(out, &pfx, display_width(&plain), msg, options.width, &indent);
    }

    // Suggestions: numbered inline diff when edits apply, otherwise a plain `= help:` line.
    for (suggestion, diff) in &suggestion_diffs {
        if diff.is_empty() {
            let plain = format!("{pad} = help: ");
            let pfx = if options.color { format!("{pad} {GUTTER}={RESET} {HELP}help{RESET}: ") } else { plain.clone() };
            let indent = format!("{pad}         ");
            push_wrapped(out, &pfx, display_width(&plain), &suggestion.message, options.width, &indent);
            continue;
        }
        let plain_help = String::from("help: ");
        let help_pfx = if options.color { format!("{HELP}help{RESET}: ") } else { plain_help.clone() };
        push_wrapped(out, &help_pfx, plain_help.len(), &suggestion.message, options.width, "      ");
        write_gutter_bar(out, &pad, "", options.color);
        for (sign, line_no, text) in diff {
            let expanded = expand_tabs(text);
            let num = format!("{line_no:>gutter_width$}");
            let body = expanded.trim_end();
            if options.color {
                let sign_style = if *sign == '-' { DIFF_DEL } else { DIFF_ADD };
                if body.is_empty() {
                    let _ = writeln!(out, "{GUTTER}{num}{RESET} {sign_style}{sign}{RESET}");
                } else {
                    let _ = writeln!(out, "{GUTTER}{num}{RESET} {sign_style}{sign} {body}{RESET}");
                }
            } else if body.is_empty() {
                let _ = writeln!(out, "{num} {sign}");
            } else {
                let _ = writeln!(out, "{num} {sign} {body}");
            }
        }
        write_gutter_bar(out, &pad, "", options.color);
    }

    for child in &d.children {
        out.push('\n');
        render_human(out, child, sources, options);
    }
}

fn write_gutter_bar(out: &mut String, pad: &str, sidebar: &str, color: bool) {
    if color {
        if sidebar.is_empty() {
            let _ = writeln!(out, "{pad} {GUTTER}|{RESET}");
        } else {
            let _ = writeln!(out, "{pad} {GUTTER}| {sidebar}{RESET}");
        }
    } else if sidebar.is_empty() {
        let _ = writeln!(out, "{pad} |");
    } else {
        let _ = writeln!(out, "{pad} | {}", sidebar.trim_end());
    }
}

fn render_file_snippet(
    out: &mut String,
    file_id: FileId,
    anns: &[&SnippetAnn],
    line_cache: &LineCache,
    gutter_width: usize,
    sev_style: &str,
    options: &RenderOptions,
) {
    let pad = " ".repeat(gutter_width);
    let mut singles: std::collections::BTreeMap<u32, Vec<SingleLineLabel>> = std::collections::BTreeMap::new();
    let mut multis: Vec<MultiLineLabel> = Vec::new();
    let mut lines_set: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();

    for ann in anns {
        if ann.start_line == ann.end_line {
            let start = ann.start_vis;
            let len = ann.end_vis.saturating_sub(start).max(1);
            singles.entry(ann.start_line).or_default().push(SingleLineLabel {
                start,
                end: start + len,
                is_primary: ann.is_primary,
                message: ann.message.clone(),
            });
            lines_set.insert(ann.start_line);
        } else {
            multis.push(MultiLineLabel {
                start_line: ann.start_line,
                end_line: ann.end_line,
                end_vis: ann.end_vis,
                is_primary: ann.is_primary,
                message: ann.message.clone(),
            });
            lines_set.insert(ann.start_line);
            lines_set.insert(ann.end_line);
        }
    }

    let has_multiline = !multis.is_empty();
    write_gutter_bar(out, &pad, "", options.color);

    let mut prev_line: Option<u32> = None;
    for line in lines_set {
        if let Some(prev) = prev_line
            && line > prev + 1
        {
            if options.color {
                let _ = writeln!(out, "{GUTTER}...{RESET}");
            } else {
                let _ = writeln!(out, "...");
            }
        }
        prev_line = Some(line);

        let sidebar_char = if !has_multiline {
            None
        } else if multis.iter().any(|m| m.start_line == line) {
            Some('/')
        } else if multis.iter().any(|m| line > m.start_line && line <= m.end_line) {
            Some('|')
        } else {
            Some(' ')
        };
        let cont_sidebar = if !has_multiline {
            None
        } else if multis.iter().any(|m| line >= m.start_line && line < m.end_line)
            || multis.iter().any(|m| m.end_line == line)
        {
            Some('|')
        } else {
            Some(' ')
        };

        let (expanded, _, line_vis_width) = line_cache
            .get(&(file_id, line))
            .map(|(e, m, w)| (e.as_str(), m, *w))
            .unwrap_or_else(|| unreachable!("line was cached by build_snippet_ann"));
        let line_singles = singles.get(&line).map(Vec::as_slice).unwrap_or(&[]);
        let max_label_col = line_singles.iter().map(|l| l.end).max().unwrap_or(0);
        let total_vis_width = line_vis_width.max(max_label_col);

        let extra_sidebar = if has_multiline { 2 } else { 0 };
        let prefix_width = gutter_width + 3 + extra_sidebar;
        let chunks = split_expanded_into_chunks(expanded, total_vis_width, options.width, prefix_width);

        for (chunk_idx, chunk) in chunks.iter().enumerate() {
            let num_text = if chunk_idx == 0 { format!("{line:>gutter_width$}") } else { pad.clone() };
            let sb = if chunk_idx == 0 { sidebar_char } else { cont_sidebar };
            let sb_str = sb.map(|c| format!("{c} ")).unwrap_or_default();
            if options.color {
                let sb_colored = if sb_str.is_empty() { String::new() } else { format!("{GUTTER}{sb_str}{RESET}") };
                if chunk.text.is_empty() && sb_str.trim().is_empty() {
                    let _ = writeln!(out, "{GUTTER}{num_text} |{RESET}");
                } else {
                    let _ = writeln!(out, "{GUTTER}{num_text} |{RESET} {sb_colored}{}", chunk.text);
                }
            } else if chunk.text.is_empty() && sb_str.trim().is_empty() {
                let _ = writeln!(out, "{num_text} |");
            } else {
                let _ = writeln!(out, "{num_text} | {sb_str}{}", chunk.text);
            }

            let underline_sb = if has_multiline {
                if multis.iter().any(|m| m.start_line <= line && line < m.end_line)
                    || (multis.iter().any(|m| m.end_line == line) && !line_singles.is_empty())
                {
                    "| "
                } else {
                    "  "
                }
            } else {
                ""
            };
            render_single_labels_for_chunk(
                out,
                line_singles,
                chunk.col_start,
                chunk.col_end,
                chunk_idx + 1 == chunks.len(),
                &pad,
                underline_sb,
                prefix_width,
                sev_style,
                options,
            );
        }

        // Render multi-line span underlines ending on `line`.
        for m in multis.iter().filter(|m| m.end_line == line) {
            let last_chunk = chunks.last().expect("at least one chunk");
            let rel_end = m.end_vis.saturating_sub(last_chunk.col_start);
            let marker = if m.is_primary { '^' } else { '-' };
            let style = if m.is_primary { sev_style } else { SECONDARY };
            let underscores = "_".repeat(rel_end);
            let underline_part = format!("|{underscores}{marker}");
            let msg = m.message.as_str();
            if msg.is_empty() {
                if options.color {
                    let _ = writeln!(out, "{pad} {GUTTER}|{RESET} {style}{underline_part}{RESET}");
                } else {
                    let _ = writeln!(out, "{pad} | {underline_part}");
                }
            } else {
                let raw_pfx = format!("{pad} | {underline_part} ");
                let styled_pfx = if options.color {
                    format!("{pad} {GUTTER}|{RESET} {style}{underline_part}{RESET} ")
                } else {
                    raw_pfx.clone()
                };
                let msg_text = if options.color { format!("{style}{msg}{RESET}") } else { msg.to_string() };
                // Wrap long label messages if needed while preserving the gutter prefix.
                let cont_indent = format!("{pad} | {sb}", sb = " ".repeat(underline_part.len() + 1));
                let _ = msg_text;
                push_styled_wrapped(
                    out,
                    &styled_pfx,
                    raw_pfx.len(),
                    msg,
                    if options.color { style } else { "" },
                    options.width,
                    &cont_indent,
                    &pad,
                    "",
                    options.color,
                );
            }
        }
    }

    write_gutter_bar(out, &pad, "", options.color);
}

struct LineChunk {
    text: String,
    col_start: usize,
    col_end: usize,
}

/// Splits an expanded source line into visual chunks of at most `avail` display columns when `width > 0`.
fn split_expanded_into_chunks(
    expanded: &str,
    total_vis_width: usize,
    width: usize,
    prefix_width: usize,
) -> Vec<LineChunk> {
    if width == 0 {
        return vec![LineChunk { text: expanded.to_string(), col_start: 0, col_end: total_vis_width }];
    }
    let avail = width.saturating_sub(prefix_width).max(1);
    if total_vis_width <= avail {
        return vec![LineChunk { text: expanded.to_string(), col_start: 0, col_end: total_vis_width }];
    }
    let mut chunks = Vec::new();
    let mut cur_text = String::new();
    let mut chunk_start_col = 0usize;
    let mut cur_col = 0usize;
    for ch in expanded.chars() {
        let w = char_display_width(ch);
        if cur_col - chunk_start_col + w > avail && !cur_text.is_empty() {
            chunks.push(LineChunk {
                text: std::mem::take(&mut cur_text),
                col_start: chunk_start_col,
                col_end: cur_col,
            });
            chunk_start_col = cur_col;
        }
        cur_text.push(ch);
        cur_col += w;
    }
    if !cur_text.is_empty() || cur_col < total_vis_width {
        // Handle any trailing EOL caret columns past the text itself.
        let end_col = cur_col.max(total_vis_width);
        if end_col - chunk_start_col <= avail {
            chunks.push(LineChunk { text: cur_text, col_start: chunk_start_col, col_end: end_col });
        } else {
            if !cur_text.is_empty() {
                chunks.push(LineChunk { text: cur_text, col_start: chunk_start_col, col_end: cur_col });
                chunk_start_col = cur_col;
            }
            chunks.push(LineChunk { text: String::new(), col_start: chunk_start_col, col_end: end_col });
        }
    }
    if chunks.is_empty() {
        chunks.push(LineChunk { text: String::new(), col_start: 0, col_end: 0 });
    }
    chunks
}

#[allow(clippy::too_many_arguments)]
fn render_single_labels_for_chunk(
    out: &mut String,
    labels: &[SingleLineLabel],
    col_start: usize,
    col_end: usize,
    is_last_chunk: bool,
    pad: &str,
    sidebar: &str,
    prefix_width: usize,
    sev_style: &str,
    options: &RenderOptions,
) {
    // Keep only labels whose underline overlaps this chunk (or reaches EOL in the last chunk).
    let mut active: Vec<&SingleLineLabel> = labels
        .iter()
        .filter(|l| {
            l.start < col_end && l.end > col_start || (is_last_chunk && l.start == col_end && l.start >= col_start)
        })
        .collect();
    if active.is_empty() {
        return;
    }
    active.sort_by_key(|l| (l.start, !l.is_primary, l.end));

    // Partition into non-overlapping tracks on this chunk.
    let mut tracks: Vec<Vec<&SingleLineLabel>> = Vec::new();
    for label in active {
        let placed = tracks.iter_mut().find(|track| track.iter().all(|other| label.start >= other.end));
        if let Some(track) = placed {
            track.push(label);
        } else {
            tracks.push(vec![label]);
        }
    }

    for track in tracks {
        let row_end =
            track
                .iter()
                .map(|l| {
                    if is_last_chunk && l.start == col_end && l.start >= col_start { l.end } else { l.end.min(col_end) }
                })
                .max()
                .unwrap_or(col_start);
        let rel_len = row_end.saturating_sub(col_start);
        let mut cells: Vec<(char, bool)> = vec![(' ', false); rel_len];
        for l in &track {
            let s = l.start.saturating_sub(col_start);
            let e = l.end.saturating_sub(col_start).min(rel_len);
            let ch = if l.is_primary { '^' } else { '-' };
            for cell in cells.iter_mut().take(e).skip(s) {
                *cell = (ch, l.is_primary);
            }
        }

        // Only attach label messages on the chunk where the label actually ends.
        let ending_labels: Vec<&&SingleLineLabel> =
            track.iter().filter(|l| !l.message.is_empty() && (l.end <= col_end || is_last_chunk)).collect();
        let inline_label = track.last().filter(|l| !l.message.is_empty() && (l.end <= col_end || is_last_chunk));
        let deferred: Vec<&&SingleLineLabel> =
            ending_labels.iter().copied().filter(|l| inline_label.is_none_or(|inl| !std::ptr::eq(**l, *inl))).collect();

        let (plain_u, styled_u) = format_underline_cells(&cells, sev_style, options.color);
        if let Some(inl) = inline_label {
            let style = if inl.is_primary { sev_style } else { SECONDARY };
            let raw_pfx = format!("{pad} | {sidebar}{plain_u} ");
            let styled_pfx = if options.color {
                let sb = if sidebar.is_empty() { String::new() } else { format!("{GUTTER}{sidebar}{RESET}") };
                format!("{pad} {GUTTER}|{RESET} {sb}{styled_u} ")
            } else {
                raw_pfx.clone()
            };
            let cont_spaces = " ".repeat(plain_u.len() + 1);
            let cont_indent = format!("{pad} | {sidebar}{cont_spaces}");
            push_styled_wrapped(
                out,
                &styled_pfx,
                prefix_width + plain_u.len() + 1,
                &inl.message,
                if options.color { style } else { "" },
                options.width,
                &cont_indent,
                pad,
                sidebar,
                options.color,
            );
        } else if options.color {
            let sb = if sidebar.is_empty() { String::new() } else { format!("{GUTTER}{sidebar}{RESET}") };
            let _ = writeln!(out, "{pad} {GUTTER}|{RESET} {sb}{styled_u}");
        } else {
            let _ = writeln!(out, "{pad} | {sidebar}{plain_u}");
        }

        // Drop down vertical bars and messages from right to left for earlier labels on this track.
        for idx in (0..deferred.len()).rev() {
            let target = deferred[idx];
            let bar_cols: Vec<(usize, bool)> =
                deferred[..=idx].iter().map(|l| (l.start.saturating_sub(col_start), l.is_primary)).collect();
            write_connector_line(out, pad, sidebar, &bar_cols, sev_style, options.color);

            let target_col = target.start.saturating_sub(col_start);
            let mut prefix_cells: Vec<(char, bool)> = vec![(' ', false); target_col];
            for &(col, prim) in &bar_cols[..idx] {
                if col < target_col {
                    prefix_cells[col] = ('|', prim);
                }
            }
            let (plain_pre, styled_pre) = format_underline_cells(&prefix_cells, sev_style, options.color);
            let style = if target.is_primary { sev_style } else { SECONDARY };
            let raw_len = prefix_width + target_col;
            let styled_pfx = if options.color {
                let sb = if sidebar.is_empty() { String::new() } else { format!("{GUTTER}{sidebar}{RESET}") };
                format!("{pad} {GUTTER}|{RESET} {sb}{styled_pre}")
            } else {
                format!("{pad} | {sidebar}{plain_pre}")
            };
            let cont_indent = format!("{pad} | {sidebar}{plain_pre}");
            push_styled_wrapped(
                out,
                &styled_pfx,
                raw_len,
                &target.message,
                if options.color { style } else { "" },
                options.width,
                &cont_indent,
                pad,
                sidebar,
                options.color,
            );
        }
    }
}

fn format_underline_cells(cells: &[(char, bool)], sev_style: &str, color: bool) -> (String, String) {
    let plain: String = cells.iter().map(|&(c, _)| c).collect();
    if !color {
        return (plain.clone(), plain);
    }
    let mut styled = String::new();
    let mut active_style: Option<&str> = None;
    for &(ch, is_primary) in cells {
        let want = if ch == ' ' {
            None
        } else if is_primary {
            Some(sev_style)
        } else {
            Some(SECONDARY)
        };
        if want != active_style {
            if active_style.is_some() {
                styled.push_str(RESET);
            }
            if let Some(s) = want {
                styled.push_str(s);
            }
            active_style = want;
        }
        styled.push(ch);
    }
    if active_style.is_some() {
        styled.push_str(RESET);
    }
    (plain, styled)
}

fn write_connector_line(
    out: &mut String,
    pad: &str,
    sidebar: &str,
    bars: &[(usize, bool)],
    sev_style: &str,
    color: bool,
) {
    let max_col = bars.iter().map(|&(c, _)| c).max().unwrap_or(0);
    let mut cells = vec![(' ', false); max_col + 1];
    for &(col, prim) in bars {
        cells[col] = ('|', prim);
    }
    let (plain, styled) = format_underline_cells(&cells, sev_style, color);
    if color {
        let sb = if sidebar.is_empty() { String::new() } else { format!("{GUTTER}{sidebar}{RESET}") };
        let _ = writeln!(out, "{pad} {GUTTER}|{RESET} {sb}{styled}");
    } else {
        let _ = writeln!(out, "{pad} | {sidebar}{plain}");
    }
}

#[allow(clippy::too_many_arguments)]
fn push_styled_wrapped(
    out: &mut String,
    prefix: &str,
    prefix_width: usize,
    text: &str,
    style: &str,
    width: usize,
    cont_indent: &str,
    pad: &str,
    sidebar: &str,
    color: bool,
) {
    let wrap_text_styled = |line: &str| {
        if style.is_empty() { line.to_string() } else { format!("{style}{line}{RESET}") }
    };
    let styled_cont = if color {
        let after_bar = cont_indent.strip_prefix(&format!("{pad} | ")).unwrap_or(cont_indent);
        let after_sb = after_bar.strip_prefix(sidebar).unwrap_or(after_bar);
        let sb = if sidebar.is_empty() { String::new() } else { format!("{GUTTER}{sidebar}{RESET}") };
        format!("{pad} {GUTTER}|{RESET} {sb}{after_sb}")
    } else {
        cont_indent.to_string()
    };
    if width == 0 {
        let _ = writeln!(out, "{prefix}{}", wrap_text_styled(text));
        return;
    }
    let indent_width = display_width(cont_indent);
    let rest = width.saturating_sub(indent_width).max(1);
    if prefix_width + 1 >= width {
        let _ = writeln!(out, "{}", prefix.trim_end());
        for line in wrap_text(text, rest, rest) {
            let _ = writeln!(out, "{styled_cont}{}", wrap_text_styled(&line));
        }
        return;
    }
    let first = width - prefix_width;
    for (i, line) in wrap_text(text, first, rest).iter().enumerate() {
        if i == 0 {
            let _ = writeln!(out, "{prefix}{}", wrap_text_styled(line));
        } else {
            let _ = writeln!(out, "{styled_cont}{}", wrap_text_styled(line));
        }
    }
}

/// The human text of one diagnostic as the `rendered` field of a JSON diagnostic carries it: no color, no links,
/// and no wrapping, so it does not depend on the terminal the compiler ran in.
fn rendered_text(d: &Diagnostic, sources: &SourceMap) -> String {
    let mut out = String::new();
    let options = RenderOptions { width: 0, ..RenderOptions::default() };
    render_human(&mut out, d, sources, &options);
    out
}

const HEADER_INDENT: &str = "    ";

/// Writes `text` after `prefix`, wrapping it so that no line is wider than `width` characters. `prefix_width` is
/// the visible width of `prefix` (without the ANSI styles), and `indent` starts every continuation line.
fn push_wrapped(out: &mut String, prefix: &str, prefix_width: usize, text: &str, width: usize, indent: &str) {
    if text.is_empty() {
        let _ = writeln!(out, "{}", prefix.trim_end());
        return;
    }
    if width == 0 {
        let _ = writeln!(out, "{prefix}{text}");
        return;
    }
    let indent_width = display_width(indent);
    let rest = width.saturating_sub(indent_width).max(1);
    if prefix_width + 1 >= width {
        // The prefix alone uses the line; put the message underneath rather than splitting it a character at a
        // time after the prefix.
        let _ = writeln!(out, "{}", prefix.trim_end());
        for line in wrap_text(text, rest, rest) {
            let _ = writeln!(out, "{indent}{line}");
        }
        return;
    }
    let first = width - prefix_width;
    for (i, line) in wrap_text(text, first, rest).iter().enumerate() {
        if i == 0 {
            let _ = writeln!(out, "{prefix}{line}");
        } else {
            let _ = writeln!(out, "{indent}{line}");
        }
    }
}

/// Breaks `text` into lines of at most `first` display columns for the first line and `rest` for the others. Words
/// are kept whole when they fit; a word longer than its line is split at a character boundary.
fn wrap_text(text: &str, first: usize, rest: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut cur_width = 0usize;
    let mut limit = first.max(1);
    for word in text.split_whitespace() {
        let mut remaining: Vec<char> = word.chars().collect();
        loop {
            let space = usize::from(!current.is_empty());
            let word_width: usize = remaining.iter().copied().map(char_display_width).sum();
            if cur_width + space + word_width <= limit {
                if space == 1 {
                    current.push(' ');
                    cur_width += 1;
                }
                current.extend(remaining.iter());
                cur_width += word_width;
                break;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                cur_width = 0;
                limit = rest.max(1);
                continue;
            }
            let mut take = 0usize;
            let mut acc = 0usize;
            while take < remaining.len() {
                let w = char_display_width(remaining[take]);
                if take > 0 && acc + w > limit {
                    break;
                }
                acc += w;
                take += 1;
            }
            let take = take.max(1);
            lines.push(remaining.drain(..take).collect());
            limit = rest.max(1);
            if remaining.is_empty() {
                break;
            }
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

/// Escapes a string for a JSON document. The JSON format is a stable interface for editors, so control
/// characters and quotes must never leak into it.
pub fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if (ch as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_ansi(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            if ch == '\x1b' {
                for ch in chars.by_ref() {
                    if ch == 'm' {
                        break;
                    }
                }
            } else {
                out.push(ch);
            }
        }
        out
    }

    #[test]
    fn line_and_column() {
        let f = SourceFile::new("a.bd".into(), "ab\ncé d\n".into());
        assert_eq!(f.line_col(0), (1, 1));
        assert_eq!(f.line_col(3), (2, 1));
        // 'é' is two bytes, so byte 6 (the space) is the third character of line 2.
        assert_eq!(f.line_col(6), (2, 3));
        // A CRLF is one line break: the byte after it starts line 2.
        let crlf = SourceFile::new("w.bd".into(), "a\r\nb".into());
        assert_eq!(crlf.line_col(3), (2, 1));
    }

    #[test]
    fn span_to_covers_both_spans() {
        let a = Span::new(FileId(0), 4, 6);
        let b = Span::new(FileId(0), 1, 3);
        assert_eq!(a.to(b), Span::new(FileId(0), 1, 6));
        assert_eq!(b.to(a), Span::new(FileId(0), 1, 6));
    }

    #[test]
    fn render_sorts_by_file_and_offset() {
        let mut sources = SourceMap::default();
        let first = sources.add("a.bd".into(), "x\ny\n".into());
        let second = sources.add("b.bd".into(), "z\n".into());
        let mut d = Diagnostics::default();
        d.error(Code::TYPE_MISMATCH, Span::new(second, 0, 1), "later file");
        d.error(Code::SYNTAX, Span::new(first, 2, 3), "second line");
        d.error(Code::SYNTAX, Span::new(first, 0, 1), "first line");
        assert_eq!(
            d.render(&sources),
            "a.bd:1:1: error[E0001]: first line\n\
             a.bd:2:1: error[E0001]: second line\n\
             b.bd:1:1: error[E0003]: later file\n"
        );
    }

    #[test]
    fn equal_spans_are_ordered_by_code_message_and_insertion() {
        let mut sources = SourceMap::default();
        let file = sources.add("a.bd".into(), "x\n".into());
        let span = Span::new(file, 0, 1);
        let mut d = Diagnostics::default();
        d.error(Code::TYPE_MISMATCH, span, "b");
        d.error(Code::SYNTAX, span, "z");
        d.error(Code::SYNTAX, span, "a");
        let first = d.render(&sources);
        assert_eq!(
            first,
            "a.bd:1:1: error[E0001]: a\n\
             a.bd:1:1: error[E0001]: z\n\
             a.bd:1:1: error[E0003]: b\n"
        );
        // Reversing the insertion order gives the same text: the order is a property of the data, not of the
        // order the checks happened to run in.
        let mut e = Diagnostics::default();
        e.error(Code::SYNTAX, span, "a");
        e.error(Code::TYPE_MISMATCH, span, "b");
        e.error(Code::SYNTAX, span, "z");
        assert_eq!(e.render(&sources), first);
    }

    #[test]
    fn absorb_skips_exact_duplicates_but_keeps_different_messages() {
        let span = Span::new(FileId(0), 0, 1);
        let mut d = Diagnostics::default();
        d.error(Code::SYNTAX, span, "first");
        let mut other = Diagnostics::default();
        other.error(Code::SYNTAX, span, "first");
        other.error(Code::SYNTAX, span, "second message");
        other.error(Code::TYPE_MISMATCH, span, "different code");
        d.absorb(other);
        assert_eq!(d.len(), 3);
        assert_eq!(
            d.iter().map(|x| x.message.as_str()).collect::<Vec<_>>(),
            ["first", "second message", "different code"]
        );
    }

    #[test]
    fn short_is_one_plain_line_without_notes_or_suggestions() {
        let mut sources = SourceMap::default();
        let file = sources.add("a.bd".into(), "mut x\n".into());
        let mut d = Diagnostics::default();
        d.error(Code::MISSING_MUT_MARKER, Span::new(file, 0, 3), "pass the argument as `mut ...`")
            .note(Span::new(file, 0, 1), "declared here")
            .suggest(Span::new(file, 0, 0), "add `mut` before this argument", "mut ");
        let rendered = d.render_with(
            &sources,
            &RenderOptions { format: DiagnosticFormat::Short, color: true, ..RenderOptions::default() },
        );
        assert_eq!(rendered.text, "a.bd:1:1: error[E0114]: pass the argument as `mut ...`\n");
        assert_eq!(d.render(&sources), rendered.text);
    }

    #[test]
    fn human_renders_notes_and_suggestions() {
        let mut sources = SourceMap::default();
        let file = sources.add("a.bd".into(), "mut x\n".into());
        let mut d = Diagnostics::default();
        d.error(Code::MISSING_MUT_MARKER, Span::new(file, 4, 5), "pass the argument as `mut ...`")
            .label(Span::new(file, 4, 5), "needs `mut`")
            .note(Span::new(file, 0, 3), "declared here")
            .suggest(Span::new(file, 4, 4), "add `mut` before this argument", "mut ");
        let rendered = d.render_with(&sources, &RenderOptions { width: 0, ..RenderOptions::default() });
        assert_eq!(
            rendered.text,
            "error[E0114]: pass the argument as `mut ...`\n\
             \x20--> a.bd:1:5\n\
             \x20 |\n\
             1 | mut x\n\
             \x20 | --- ^ needs `mut`\n\
             \x20 | |\n\
             \x20 | declared here\n\
             \x20 |\n\
             help: add `mut` before this argument\n\
             \x20 |\n\
             1 - mut x\n\
             1 + mut mut x\n\
             \x20 |\n"
        );
    }

    #[test]
    fn human_colorizes_errors_warnings_gutter_and_suggestions_without_changing_the_text() {
        let mut sources = SourceMap::default();
        let file = sources.add("a.bd".into(), "let a = b\n".into());
        let mut d = Diagnostics::default();
        d.error(Code::TYPE_MISMATCH, Span::new(file, 8, 9), "expected `i32`, found `str`")
            .label(Span::new(file, 8, 9), "found here")
            .label(Span::new(file, 4, 5), "declared here")
            .suggest(Span::new(file, 8, 9), "use `0`", "0");
        let plain = d.render_with(&sources, &RenderOptions { color: false, width: 0, ..RenderOptions::default() });
        let colored = d.render_with(&sources, &RenderOptions { color: true, width: 0, ..RenderOptions::default() });
        assert!(colored.text.contains("\x1b[1;31merror\x1b[0m[\x1b[1;31mE0003\x1b[0m]"), "{}", colored.text);
        assert!(colored.text.contains("\x1b[1;34m-->\x1b[0m"), "{}", colored.text);
        assert!(colored.text.contains("\x1b[1;34m-\x1b[0m"), "{}", colored.text);
        assert!(colored.text.contains("\x1b[1;31m^\x1b[0m"), "{}", colored.text);
        assert!(colored.text.contains("\x1b[1;32mhelp\x1b[0m"), "{}", colored.text);
        assert_eq!(strip_ansi(&colored.text), plain.text);

        let mut w = Diagnostics::default();
        w.warning(Code("L0007"), Span::new(file, 4, 5), "unused");
        let w_plain = w.render_with(&sources, &RenderOptions { color: false, width: 0, ..RenderOptions::default() });
        let w_colored = w.render_with(&sources, &RenderOptions { color: true, width: 0, ..RenderOptions::default() });
        assert!(w_colored.text.contains("\x1b[1;33mwarning\x1b[0m[\x1b[1;33mL0007\x1b[0m]"), "{}", w_colored.text);
        assert_eq!(strip_ansi(&w_colored.text), w_plain.text);
    }

    #[test]
    fn human_wraps_long_messages_and_source_lines_keeping_underline_aligned() {
        let mut sources = SourceMap::default();
        let file =
            sources.add("a.bd".into(), "    this_identifier_is_far_too_long_to_fit_on_one_terminal_line(1)\n".into());
        let mut d = Diagnostics::default();
        d.error(
            Code::UNKNOWN_NAME,
            Span::new(file, 4, 63),
            "unknown name `this_identifier_is_far_too_long_to_fit_on_one_terminal_line`",
        )
        .label(Span::new(file, 4, 63), "not found in this scope");
        let rendered = d.render_with(&sources, &RenderOptions { width: 40, ..RenderOptions::default() });
        let lines: Vec<&str> = rendered.text.lines().collect();
        assert!(lines.len() > 4, "{}", rendered.text);
        for line in &lines {
            assert!(display_width(line) <= 40, "line is too wide: {line:?}");
        }
        assert!(lines[1].starts_with(HEADER_INDENT), "{:?}", rendered.text);
        assert!(rendered.text.contains("^^^"), "{}", rendered.text);
        let joined = rendered.text.lines().map(str::trim_start).collect::<Vec<_>>().join("");
        assert!(joined.contains("`this_identifier_is_far_too_long_to_fit_on_one_terminal_line`"), "{joined}");
    }

    #[test]
    fn human_renders_multiline_span_with_sidebar_and_ellipsis() {
        let mut sources = SourceMap::default();
        let text = "fn f() {\n    let x = foo(\n        1,\n        2,\n        3)\n}\n";
        let file = sources.add("m.bd".into(), text.into());
        let lo = text.find("foo(").unwrap() as u32;
        let hi = (text.find("3)").unwrap() + 2) as u32;
        let mut d = Diagnostics::default();
        d.error(Code::TYPE_MISMATCH, Span::new(file, lo, hi), "mismatched call")
            .label(Span::new(file, lo, hi), "this call returns `str`");
        let rendered = d.render_with(&sources, &RenderOptions { width: 0, ..RenderOptions::default() });
        assert_eq!(
            rendered.text,
            "error[E0003]: mismatched call\n\
             \x20--> m.bd:2:13\n\
             \x20 |\n\
             2 | /     let x = foo(\n\
             ...\n\
             5 | |         3)\n\
             \x20 | |__________^ this call returns `str`\n\
             \x20 |\n"
        );
    }

    #[test]
    fn human_renders_labels_in_two_files_and_line_insertion_suggestion() {
        let mut sources = SourceMap::default();
        let main = sources.add("src/main.bd".into(), "fn main() {\n    let h = http.Client.new()\n}\n".into());
        let other = sources.add("src/other.bd".into(), "pub struct Client {}\n".into());
        let mut d = Diagnostics::default();
        d.error(Code::UNKNOWN_NAME, Span::new(main, 24, 28), "unknown name `http`")
            .label(Span::new(main, 24, 28), "not found in this scope")
            .label(Span::new(other, 11, 17), "`Client` is defined here")
            .suggest_safe(Span::new(main, 0, 0), "import it", "use http\n");
        let rendered = d.render_with(&sources, &RenderOptions { width: 0, ..RenderOptions::default() });
        assert_eq!(
            rendered.text,
            "error[E0002]: unknown name `http`\n\
             \x20--> src/main.bd:2:13\n\
             \x20 |\n\
             2 |     let h = http.Client.new()\n\
             \x20 |             ^^^^ not found in this scope\n\
             \x20 |\n\
             \x20--> src/other.bd:1:12\n\
             \x20 |\n\
             1 | pub struct Client {}\n\
             \x20 |            ------ `Client` is defined here\n\
             \x20 |\n\
             help: import it\n\
             \x20 |\n\
             1 + use http\n\
             \x20 |\n"
        );
    }

    #[test]
    fn human_handles_tabs_and_wide_characters() {
        let mut sources = SourceMap::default();
        let text = "\tlet 中 = 😀 + oops\n";
        let file = sources.add("w.bd".into(), text.into());
        let emoji_lo = text.find('😀').unwrap() as u32;
        let emoji_hi = emoji_lo + '😀'.len_utf8() as u32;
        let oops_lo = text.find("oops").unwrap() as u32;
        let oops_hi = oops_lo + 4;
        let mut d = Diagnostics::default();
        d.error(Code::UNKNOWN_NAME, Span::new(file, oops_lo, oops_hi), "unknown name `oops`")
            .label(Span::new(file, oops_lo, oops_hi), "not found")
            .label(Span::new(file, emoji_lo, emoji_hi), "emoji here");
        let rendered = d.render_with(&sources, &RenderOptions { width: 0, ..RenderOptions::default() });
        // `\t` -> 4 spaces, `let ` -> 4, `中` -> 2, ` = ` -> 3 => col 13 (`😀`, width 2), ` + ` -> 3 => col 18 (`oops`, width 4).
        assert_eq!(
            rendered.text,
            "error[E0002]: unknown name `oops`\n\
             \x20--> w.bd:1:14\n\
             \x20 |\n\
             1 |     let 中 = 😀 + oops\n\
             \x20 |              --   ^^^^ not found\n\
             \x20 |              |\n\
             \x20 |              emoji here\n\
             \x20 |\n"
        );
    }

    #[test]
    fn wrapping_ignores_a_zero_width_and_keeps_utf8_whole() {
        let lines = wrap_text("alpha béta gamma", 12, 8);
        assert_eq!(lines, ["alpha béta", "gamma"]);
        let split = wrap_text("ééééé", 2, 2);
        assert_eq!(split, ["éé", "éé", "é"]);
    }

    fn json_of(d: &Diagnostics, sources: &SourceMap) -> String {
        d.render_with(
            sources,
            &RenderOptions { format: DiagnosticFormat::Json, color: true, ..RenderOptions::default() },
        )
        .text
    }

    #[test]
    fn json_is_version_1_with_every_documented_field_in_order() {
        let mut sources = SourceMap::default();
        let file = sources.add("a.bd".into(), "mut x\n".into());
        let mut d = Diagnostics::default();
        d.error(Code::MISSING_MUT_MARKER, Span::new(file, 4, 5), "pass the argument as `mut ...`")
            .note(Span::new(file, 0, 3), "declared here")
            .suggest(Span::new(file, 4, 4), "add `mut` before this argument", "mut ");
        let text = json_of(&d, &sources);
        assert!(!text.contains('\x1b'));
        assert_eq!(text.lines().count(), 1);
        let keys = [
            "\"type\":\"diagnostic\",\"version\":1,\"severity\":\"error\",\"code\":\"E0114\",\"message\":",
            ",\"url\":\"https://ricardomilos7.github.io/burdin/errors/E0114\",\"span\":{\"file\":\"a.bd\",\"line\":1,",
            ",\"labels\":[],\"notes\":[{\"message\":\"declared here\",\"span\":{\"file\":\"a.bd\"",
            ",\"help\":[],\"related\":[{\"span\":{\"file\":\"a.bd\"",
            ",\"children\":[],\"suggestions\":[{\"message\":\"add `mut` before this argument\",\"applicability\":\"manual\",",
            "\"edits\":[{\"span\":{\"file\":\"a.bd\",\"line\":1,\"column\":5,",
            "\"replacement\":\"mut \"}]}],\"rendered\":\"error[E0114]: pass the argument as `mut ...`\\n",
        ];
        let mut from = 0;
        for key in keys {
            let at = text[from..].find(key).unwrap_or_else(|| panic!("`{key}` missing or out of order in {text}"));
            from += at + key.len();
        }
    }

    #[test]
    fn json_represents_spanless_notes_deletions_and_machine_fixes() {
        let mut sources = SourceMap::default();
        let file = sources.add("a.bd".into(), "mut x\n".into());
        let mut d = Diagnostics::default();
        d.error(Code::UNNEEDED_MUT_MARKER, Span::new(file, 4, 5), "no marker")
            .note_without_span("see the guide")
            .suggest_safe(Span::new(file, 0, 4), "remove the `mut` marker", "");
        let text = json_of(&d, &sources);
        assert!(text.contains("\"notes\":[{\"message\":\"see the guide\",\"span\":null}]"), "{text}");
        assert!(text.contains("\"related\":[]"), "a note without a span is not related information: {text}");
        assert!(text.contains("\"applicability\":\"machine\""), "{text}");
        assert!(text.contains("\"replacement\":\"\"}]}]"), "{text}");
    }

    #[test]
    fn json_carries_labels_help_children_and_related_information() {
        let mut sources = SourceMap::default();
        let file = sources.add("a.bd".into(), "let a = b\n".into());
        let mut d = Diagnostics::default();
        let child = Diagnostic::new(Code::UNKNOWN_NAME, Severity::Warning, Span::new(file, 4, 5), "also unused");
        d.error(Code::TYPE_MISMATCH, Span::new(file, 8, 9), "mismatch")
            .label(Span::new(file, 8, 9), "found here")
            .label(Span::new(file, 4, 5), "declared here")
            .note(Span::new(file, 0, 3), "see this")
            .help("convert it")
            .child(child);
        let text = json_of(&d, &sources);
        assert!(text.contains("\"labels\":[{\"span\":{\"file\":\"a.bd\",\"line\":1,\"column\":9,"), "{text}");
        assert!(text.contains("\"message\":\"found here\",\"primary\":true}"), "{text}");
        assert!(text.contains("\"message\":\"declared here\",\"primary\":false}"), "{text}");
        assert!(text.contains("\"help\":[\"convert it\"]"), "{text}");
        assert!(text.contains("\"related\":[{\"span\":{\"file\":\"a.bd\",\"line\":1,\"column\":1,"), "{text}");
        assert!(
            text.contains("\"children\":[{\"severity\":\"warning\",\"code\":\"E0002\",\"message\":\"also unused\""),
            "{text}"
        );
        // A child has the body of a diagnostic and no envelope or rendered text of its own.
        assert_eq!(text.matches("\"type\":\"diagnostic\"").count(), 1);
        assert_eq!(text.matches("\"rendered\"").count(), 1);
        let human = d.render_with(&sources, &RenderOptions { width: 0, ..RenderOptions::default() }).text;
        assert_eq!(
            human,
            "error[E0003]: mismatch\n\
             \x20--> a.bd:1:9\n\
             \x20 |\n\
             1 | let a = b\n\
             \x20 | --- -   ^ found here\n\
             \x20 | |   |\n\
             \x20 | |   declared here\n\
             \x20 | |\n\
             \x20 | see this\n\
             \x20 |\n\
             \x20 = help: convert it\n\n\
             warning[E0002]: also unused\n\
             \x20--> a.bd:1:5\n\
             \x20 |\n\
             1 | let a = b\n\
             \x20 |     ^\n\
             \x20 |\n"
        );
    }

    /// A diagnostic that uses every field of the version 1 format: a primary span with a label, a second label, a
    /// note with a span and one without, help, a machine-applicable suggestion, and a child. The expected line is
    /// the committed golden `tests/diagnostics/composite.golden.json`, which `tests/check_diagnostics.py` also
    /// validates against the schema. `BURDIN_UPDATE_GOLDEN=1` rewrites it.
    #[test]
    fn json_golden_for_a_diagnostic_with_every_field() {
        let mut sources = SourceMap::default();
        let text = "let é = 𝄞 + oops\nlet other = é\n";
        let file = sources.add("composite.bd".into(), text.into());
        let at = |needle: &str, from: usize| {
            let lo = text[from..].find(needle).expect("the needle is in the text") + from;
            Span::new(file, lo as u32, (lo + needle.len()) as u32)
        };
        let oops = at("oops", 0);
        let declared = at("é", 0);
        let other = at("other", 0);
        let mut d = Diagnostics::default();
        let child = Diagnostic::new(Code("L0007"), Severity::Warning, other, "`other` is never read");
        d.error(Code::UNKNOWN_NAME, oops, "unknown name `oops`")
            .label(oops, "not found in this scope")
            .label(declared, "`é` is declared here")
            .note(at("é", 20), "`é` is used again here")
            .note_without_span("names are looked up in the enclosing scopes and the imports")
            .help("check the spelling")
            .suggest_safe(oops, "use `é` instead", "é")
            .child(child);
        let actual = json_of(&d, &sources);
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/diagnostics/composite.golden.json");
        if std::env::var_os("BURDIN_UPDATE_GOLDEN").is_some() {
            std::fs::write(path, &actual).expect("the golden file can be written");
        }
        let golden = std::fs::read_to_string(path).expect("the golden file exists").replace("\r\n", "\n");
        assert_eq!(
            actual, golden,
            "the JSON format changed: update docs/diagnostics.md and the schema, then rerun with BURDIN_UPDATE_GOLDEN=1"
        );
    }

    #[test]
    fn json_columns_count_characters_bytes_and_utf16_units() {
        // `é` is 2 bytes and 1 UTF-16 unit; `𝄞` is 4 bytes and 2 UTF-16 units.
        let mut sources = SourceMap::default();
        let file = sources.add("a.bd".into(), "é𝄞x\n".into());
        let mut d = Diagnostics::default();
        d.error(Code::SYNTAX, Span::new(file, 6, 7), "at x");
        let text = json_of(&d, &sources);
        assert!(
            text.contains(
                "\"span\":{\"file\":\"a.bd\",\"line\":1,\"column\":3,\"end_line\":1,\"end_column\":4,\
                 \"byte_start\":6,\"byte_end\":7,\"utf8_column\":7,\"utf16_column\":4,\
                 \"end_utf8_column\":8,\"end_utf16_column\":5}"
            ),
            "{text}"
        );
        let position = sources.get(file).position(6);
        assert_eq!((position.column, position.utf8_column, position.utf16_column), (3, 7, 4));
    }

    #[test]
    fn json_rendered_is_the_plain_unwrapped_human_text() {
        let mut sources = SourceMap::default();
        let file = sources.add("a.bd".into(), "x\n".into());
        let mut d = Diagnostics::default();
        d.error(Code::UNKNOWN_NAME, Span::new(file, 0, 1), "unknown name `a_very_long_identifier_that_does_not_fit`");
        let text = d
            .render_with(
                &sources,
                &RenderOptions { format: DiagnosticFormat::Json, color: true, width: 20, ..RenderOptions::default() },
            )
            .text;
        assert!(
            text.contains(
                "\"rendered\":\"error[E0002]: unknown name `a_very_long_identifier_that_does_not_fit`\\n --> a.bd:1:1\\n  |\\n1 | x\\n  | ^\\n  |\\n\""
            ),
            "{text}"
        );
    }

    #[test]
    fn codes_link_to_the_reference_except_those_it_does_not_explain() {
        assert_eq!(Code::TYPE_MISMATCH.url().as_deref(), Some("https://ricardomilos7.github.io/burdin/errors/E0003"));
        assert_eq!(Code("L0007").url().as_deref(), Some("https://ricardomilos7.github.io/burdin/lints/L0007"));
        assert_eq!(Code("E9000").url(), None);
        assert_eq!(Code("X1").url(), None);
        assert_eq!(Code("E00AB").url(), None);
    }

    #[test]
    fn human_links_the_code_only_with_color_and_hyperlinks_and_keeps_the_visible_text() {
        let mut sources = SourceMap::default();
        let file = sources.add("a.bd".into(), "x\n".into());
        let mut d = Diagnostics::default();
        d.error(Code::TYPE_MISMATCH, Span::new(file, 0, 1), "expected `i32`, found `str`");
        let render = |color, hyperlinks| {
            d.render_with(&sources, &RenderOptions { color, hyperlinks, width: 0, ..RenderOptions::default() }).text
        };
        let link =
            "\x1b]8;;https://ricardomilos7.github.io/burdin/errors/E0003\x1b\\\x1b[1;31mE0003\x1b[0m\x1b]8;;\x1b\\";
        assert!(render(true, true).contains(&format!("[{link}]")), "{}", render(true, true));
        assert!(!render(true, false).contains("\x1b]8"));
        // Without color there is no styling at all, links included.
        assert_eq!(render(false, true), render(false, false));
        assert!(!render(false, true).contains('\x1b'));
        assert_eq!(strip_ansi_and_links(&render(true, true)), render(false, false));
        // `E9000` has no page to link to.
        let mut e = Diagnostics::default();
        e.error(Code("E9000"), Span::new(file, 0, 1), "not yet");
        let text =
            e.render_with(&sources, &RenderOptions { color: true, hyperlinks: true, ..RenderOptions::default() }).text;
        assert!(!text.contains("\x1b]8"), "{text}");
    }

    fn strip_ansi_and_links(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '\x1b' {
                out.push(ch);
                continue;
            }
            match chars.next() {
                Some('[') => {
                    for ch in chars.by_ref() {
                        if ch == 'm' {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // An OSC sequence ends at `ESC \`.
                    while let Some(ch) = chars.next() {
                        if ch == '\x1b' {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    #[test]
    fn sarif_log_has_one_run_with_rules_results_related_locations_and_fixes() {
        let mut sources = SourceMap::default();
        let file = sources.add("src/a.bd".into(), "mut x\n".into());
        let mut d = Diagnostics::default();
        d.error(Code::MISSING_MUT_MARKER, Span::new(file, 4, 5), "pass the argument as `mut ...`")
            .note(Span::new(file, 0, 3), "declared here")
            .note_without_span("see the guide")
            .suggest_safe(Span::new(file, 4, 4), "add `mut` before this argument", "mut ");
        d.warning(Code("L0007"), Span::new(file, 4, 5), "unused");
        let tool = Tool {
            name: "burdin".into(),
            version: "1.2.3".into(),
            information_uri: "https://example.test".into(),
            rules: vec![Rule {
                id: "E0114".into(),
                name: "Missing mut marker".into(),
                help_uri: Code::MISSING_MUT_MARKER.url(),
            }],
        };
        let rendered = d.render_with(
            &sources,
            &RenderOptions { format: DiagnosticFormat::Sarif, tool, ..RenderOptions::default() },
        );
        let text = rendered.text;
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.starts_with(
            "{\"$schema\":\"https://json.schemastore.org/sarif-2.1.0.json\",\"version\":\"2.1.0\",\"runs\":[{"
        ));
        assert!(text.contains("\"tool\":{\"driver\":{\"name\":\"burdin\",\"version\":\"1.2.3\""), "{text}");
        assert!(text.contains("\"rules\":[{\"id\":\"E0114\",\"name\":\"Missing mut marker\""), "{text}");
        assert!(text.contains("\"helpUri\":\"https://ricardomilos7.github.io/burdin/errors/E0114\""), "{text}");
        // A code the registry does not list gets a rule of its own, so every result can point at one.
        assert!(text.contains("{\"id\":\"L0007\",\"name\":\"L0007\""), "{text}");
        assert!(text.contains("\"ruleId\":\"E0114\",\"ruleIndex\":0,\"level\":\"error\""), "{text}");
        assert!(text.contains("\"ruleId\":\"L0007\",\"ruleIndex\":1,\"level\":\"warning\""), "{text}");
        assert!(
            text.contains(
                "\"locations\":[{\"physicalLocation\":{\"artifactLocation\":{\"uri\":\"src/a.bd\",\
                 \"uriBaseId\":\"%SRCROOT%\"},\"region\":{\"startLine\":1,\"startColumn\":5,\"endLine\":1,\
                 \"endColumn\":6,\"byteOffset\":4,\"byteLength\":1}}}]"
            ),
            "{text}"
        );
        assert!(text.contains("\"relatedLocations\":[{\"physicalLocation\":"), "{text}");
        assert!(text.contains("\"message\":{\"text\":\"declared here\"}"), "{text}");
        assert!(
            text.contains(
                "\"fixes\":[{\"description\":{\"text\":\"add `mut` before this argument\"},\"artifactChanges\":"
            ),
            "{text}"
        );
        assert!(text.contains("\"insertedContent\":{\"text\":\"mut \"}"), "{text}");
        assert!(
            text.contains("\"properties\":{\"notes\":[\"see the guide\"],\"fixIsMachineApplicable\":[true]}"),
            "{text}"
        );
        assert!(text.contains("\"invocations\":[{\"executionSuccessful\":false}]"), "{text}");
        assert!(text.contains("\"columnKind\":\"unicodeCodePoints\""), "{text}");
        assert!(!text.contains("diagnosticsNotShown"), "{text}");
    }

    #[test]
    fn sarif_with_no_diagnostics_is_a_valid_empty_log_and_records_a_limit() {
        let mut sources = SourceMap::default();
        let file = sources.add("a.bd".into(), "x\n".into());
        let options = RenderOptions { format: DiagnosticFormat::Sarif, ..RenderOptions::default() };
        let empty = Diagnostics::default().render_with(&sources, &options).text;
        assert!(empty.contains("\"results\":[]"), "{empty}");
        assert!(empty.contains("\"invocations\":[{\"executionSuccessful\":true}]"), "{empty}");
        let mut d = Diagnostics::default();
        d.error(Code::SYNTAX, Span::new(file, 0, 1), "one");
        d.error(Code::SYNTAX, Span::new(file, 1, 2), "two");
        let limited = d.render_with(&sources, &RenderOptions { error_limit: Some(1), ..options }).text;
        assert!(limited.contains("\"properties\":{\"diagnosticsNotShown\":1}"), "{limited}");
    }

    #[test]
    fn json_escapes_quotes_backslashes_and_control_characters() {
        assert_eq!(json_escape("a\"b\\c\nd\te\u{1}"), "a\\\"b\\\\c\\nd\\te\\u0001");
        assert_eq!(json_escape("héllo"), "héllo");
    }

    #[test]
    fn error_limit_applies_after_sorting_and_reports_the_total() {
        let mut sources = SourceMap::default();
        let file = sources.add("a.bd".into(), "abcdefgh\n".into());
        let mut d = Diagnostics::default();
        d.error(Code::TYPE_MISMATCH, Span::new(file, 6, 7), "third");
        d.error(Code::SYNTAX, Span::new(file, 0, 1), "first");
        d.error(Code::UNKNOWN_NAME, Span::new(file, 3, 4), "second");
        let rendered = d.render_with(
            &sources,
            &RenderOptions { format: DiagnosticFormat::Short, error_limit: Some(2), ..RenderOptions::default() },
        );
        assert_eq!(rendered.total, 3);
        assert_eq!(rendered.shown, 2);
        assert_eq!(rendered.text, "a.bd:1:1: error[E0001]: first\na.bd:1:4: error[E0002]: second\n");
        let all =
            d.render_with(&sources, &RenderOptions { format: DiagnosticFormat::Short, ..RenderOptions::default() });
        assert_eq!(all.shown, 3);
        assert_eq!(all.text.lines().count(), 3);
    }
}
