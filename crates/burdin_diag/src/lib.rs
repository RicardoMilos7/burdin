//! Source files, spans, and diagnostics.
//!
//! Every diagnostic carries an error code from the registry in `tests/README.md`, a severity, and a primary span.
//! It may also carry related notes (with their own spans) and suggestions (a replacement edit for one span), which
//! the JSON format exposes for an editor's code actions (issue #435).
//!
//! Four formats are rendered from the same data (issues #434, #642, #643):
//!
//! - `human`: the default. It colors the location, the severity, and the code when asked to, and wraps long
//!   messages to a fixed width. Its first line is always
//!   `path:LINE:COL: error[E0000]: message`, which the conformance runner reads. When color is on and the
//!   terminal supports hyperlinks, the code links to its entry in the error reference (issue #644).
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
    /// `any` or `impl` applied to something that is not an interface.
    pub const NOT_AN_INTERFACE: Code = Code("E0200");
    /// `impl Interface` outside a parameter list.
    pub const IMPL_NOT_PARAMETER: Code = Code("E0201");
    /// An interface that cannot be used as `any` because it is not object-safe.
    pub const NOT_OBJECT_SAFE: Code = Code("E0202");
    /// An invalid `error` declaration: a `from` variant without exactly one tuple field, `from` on a generic
    /// error enum, or a message placeholder that does not name a payload field.
    pub const INVALID_ERROR: Code = Code("E0210");
    /// Two variants of one error enum converting from the same source type.
    pub const DUPLICATE_FROM: Code = Code("E0211");
    /// A missing error conversion: `?` between two error types with no `From` block, or a conversion into the
    /// dynamic `Error` from a type that does not implement it.
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
    /// An invalid comptime construct (`spec/comptime.md`): a runtime local used in a compile-time context,
    /// `comptime` on a parameter that cannot take it, a comptime-parameter function used as a value, `break` or
    /// `continue` leaving a `comptime for`, a `mut` `comptime for` binding, or a non-literal assertion message.
    pub const INVALID_COMPTIME: Code = Code("E0220");
    /// A compile-time value whose type cannot be materialized in the program (CT3).
    pub const NOT_MATERIALIZABLE: Code = Code("E0221");
    /// Compile-time evaluation panicked (CT10).
    pub const COMPTIME_PANIC: Code = Code("E0222");
    /// A `comptime assert` failed (CT8).
    pub const COMPTIME_ASSERT: Code = Code("E0223");
    /// An impure operation during compile-time evaluation (CT9).
    pub const COMPTIME_IMPURE: Code = Code("E0224");
    /// Compile-time evaluation exceeded its step budget, call depth, or specialization limit (CT11).
    pub const COMPTIME_BUDGET: Code = Code("E0225");
    /// A reflection member used where it does not apply: `fields` or `variants` on a type that is neither a struct
    /// nor an enum (`spec/reflection.md`, RFL2).
    pub const INVALID_REFLECTION: Code = Code("E0230");
    /// A type declares a name the reflection members reserve: an associated function or an enum variant named
    /// `name`, `size`, `align`, `fields`, or `variants` (RFL3).
    pub const RESERVED_MEMBER: Code = Code("E0231");
    /// A computed field access `x.{name}` whose name is not a compile-time string (RFL5).
    pub const INVALID_COMPUTED_FIELD: Code = Code("E0232");
    /// An invalid derive declaration or use: a derive named like a built-in derive, or `derives (Name)` with no
    /// interface `Name` in scope (RFL8, RFL9).
    pub const INVALID_DERIVE: Code = Code("E0233");
    /// `embed` could not read its file: the file is missing, unreadable, or not UTF-8 text (`spec/comptime.md`,
    /// CT15).
    pub const EMBED_FAILED: Code = Code("E0240");
    /// An invalid `embed` use: the path is not a string literal, is absolute, or has `..`, or the expected
    /// `[u8; N]` is not the file's length (CT15).
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
    /// An interface implementation the compiler provides directly (such as `Display` for a primitive) and
    /// therefore cannot dispatch through a vtable.
    pub const BUILT_IN_IMPL: Code = Code("E0204");
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

/// Text attached to a span of the diagnostic's own source. A label is for a machine consumer that marks the span
/// in an editor; the human format has no source snippets, so it does not print labels.
#[derive(Clone, Debug)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

/// A fix for one span. `span.lo == span.hi` inserts `replacement`; an empty replacement deletes the span. This is
/// the shape an editor needs for a quick-fix code action.
#[derive(Clone, Debug)]
pub struct Suggestion {
    pub message: String,
    pub span: Span,
    pub replacement: String,
    /// The edit is safe to apply without review (`burdin lint --fix`, `spec/lint.md`, LN12): it cannot change what
    /// the program means, and the driver checks that the edited program still compiles before writing it.
    pub machine_applicable: bool,
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

    /// Attaches a suggested edit: replace `span` with `replacement`.
    pub fn suggest(
        &mut self,
        span: Span,
        message: impl Into<String>,
        replacement: impl Into<String>,
    ) -> &mut Diagnostic {
        self.suggestions.push(Suggestion {
            message: message.into(),
            span,
            replacement: replacement.into(),
            machine_applicable: false,
        });
        self
    }

    /// Like [`Diagnostic::suggest`], for an edit `burdin lint --fix` may apply on its own.
    pub fn suggest_safe(
        &mut self,
        span: Span,
        message: impl Into<String>,
        replacement: impl Into<String>,
    ) -> &mut Diagnostic {
        self.suggestions.push(Suggestion {
            message: message.into(),
            span,
            replacement: replacement.into(),
            machine_applicable: true,
        });
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

    /// Moves every diagnostic of `other` into this list, skipping one that has the same code and span as a
    /// diagnostic already here. A generic body is checked once against its bounds and once per instantiation, so
    /// the same mistake can be found more than once.
    pub fn absorb(&mut self, other: Diagnostics) {
        for d in other.list {
            if !self.list.iter().any(|e| e.code == d.code && e.span == d.span) {
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
            for d in &sorted[..shown] {
                match options.format {
                    DiagnosticFormat::Human => render_human(&mut text, d, sources, options),
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
const LOCATION: &str = "\x1b[36m";
const SEVERITY: &str = "\x1b[1;31m";
const CODE: &str = "\x1b[33m";
const NOTE: &str = "\x1b[1;36m";
const HELP: &str = "\x1b[1;32m";
// An OSC 8 hyperlink is `ESC ] 8 ; ; URL ST text ESC ] 8 ; ; ST`; `ST` is `ESC \`.
const OSC8_OPEN: &str = "\x1b]8;;";
const OSC8_END: &str = "\x1b\\";
const OSC8_CLOSE: &str = "\x1b]8;;\x1b\\";

fn render_short(out: &mut String, d: &Diagnostic, sources: &SourceMap) {
    let file = sources.get(d.span.file);
    let (line, column) = file.line_col(d.span.lo);
    let _ = writeln!(out, "{}:{}:{}: {}[{}]: {}", file.path, line, column, d.severity.as_str(), d.code.0, d.message);
}

fn render_human(out: &mut String, d: &Diagnostic, sources: &SourceMap, options: &RenderOptions) {
    let file = sources.get(d.span.file);
    let (line, column) = file.line_col(d.span.lo);
    let location = format!("{}:{}:{}", file.path, line, column);
    let severity = d.severity.as_str();
    let code = d.code.0;
    let plain_prefix = format!("{location}: {severity}[{code}]: ");
    let prefix = if options.color {
        // The link wraps the code and nothing else, so the visible text is the same with or without it.
        let code = match d.code.url().filter(|_| options.hyperlinks) {
            Some(url) => format!("{OSC8_OPEN}{url}{OSC8_END}{CODE}{code}{RESET}{OSC8_CLOSE}"),
            None => format!("{CODE}{code}{RESET}"),
        };
        format!("{LOCATION}{location}{RESET}: {SEVERITY}{severity}{RESET}[{code}]: ")
    } else {
        plain_prefix.clone()
    };
    push_wrapped(out, &prefix, plain_prefix.chars().count(), &d.message, options.width, HEADER_INDENT);

    for note in &d.notes {
        let mut plain = String::from("  = note: ");
        let mut prefix = if options.color { format!("  = {NOTE}note{RESET}: ") } else { plain.clone() };
        if let Some(span) = note.span {
            let file = sources.get(span.file);
            let (line, column) = file.line_col(span.lo);
            let location = format!("{}:{}:{}: ", file.path, line, column);
            plain.push_str(&location);
            if options.color {
                let _ = write!(prefix, "{LOCATION}{location}{RESET}");
            } else {
                prefix.push_str(&location);
            }
        }
        push_wrapped(out, &prefix, plain.chars().count(), &note.message, options.width, DETAIL_INDENT);
    }
    let help = d.help.iter().chain(d.suggestions.iter().map(|suggestion| &suggestion.message));
    for message in help {
        let plain = String::from("  = help: ");
        let prefix = if options.color { format!("  = {HELP}help{RESET}: ") } else { plain.clone() };
        push_wrapped(out, &prefix, plain.chars().count(), message, options.width, DETAIL_INDENT);
    }
    for child in &d.children {
        let file = sources.get(child.span.file);
        let (line, column) = file.line_col(child.span.lo);
        let plain = format!("  = {}[{}]: {}:{}:{}: ", child.severity.as_str(), child.code.0, file.path, line, column);
        push_wrapped(out, &plain, plain.chars().count(), &child.message, options.width, DETAIL_INDENT);
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
const DETAIL_INDENT: &str = "          ";

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
    let indent_width = indent.chars().count();
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

/// Breaks `text` into lines of at most `first` characters for the first line and `rest` for the others. Words are
/// kept whole when they fit; a word longer than its line is split at a character boundary.
fn wrap_text(text: &str, first: usize, rest: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut limit = first.max(1);
    for word in text.split_whitespace() {
        let mut remaining: Vec<char> = word.chars().collect();
        loop {
            let space = usize::from(!current.is_empty());
            if current.chars().count() + space + remaining.len() <= limit {
                if space == 1 {
                    current.push(' ');
                }
                current.extend(remaining.iter());
                break;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                limit = rest.max(1);
                continue;
            }
            let take = limit.min(remaining.len());
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
    fn absorb_skips_duplicates_but_keeps_other_codes() {
        let span = Span::new(FileId(0), 0, 1);
        let mut d = Diagnostics::default();
        d.error(Code::SYNTAX, span, "first");
        let mut other = Diagnostics::default();
        other.error(Code::SYNTAX, span, "duplicate");
        other.error(Code::TYPE_MISMATCH, span, "different code");
        d.absorb(other);
        assert_eq!(d.len(), 2);
        assert_eq!(d.iter().map(|x| x.message.as_str()).collect::<Vec<_>>(), ["first", "different code"]);
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
        d.error(Code::MISSING_MUT_MARKER, Span::new(file, 0, 3), "pass the argument as `mut ...`")
            .note(Span::new(file, 0, 1), "declared here")
            .suggest(Span::new(file, 0, 0), "add `mut` before this argument", "mut ");
        let rendered = d.render_with(&sources, &RenderOptions { width: 0, ..RenderOptions::default() });
        assert_eq!(
            rendered.text,
            "a.bd:1:1: error[E0114]: pass the argument as `mut ...`\n\
             \x20 = note: a.bd:1:1: declared here\n\
             \x20 = help: add `mut` before this argument\n"
        );
    }

    #[test]
    fn human_colorizes_location_severity_and_code_without_changing_the_text() {
        let mut sources = SourceMap::default();
        let file = sources.add("a.bd".into(), "x\n".into());
        let mut d = Diagnostics::default();
        d.error(Code::TYPE_MISMATCH, Span::new(file, 0, 1), "expected `i32`, found `str`");
        let plain = d.render_with(&sources, &RenderOptions { color: false, width: 0, ..RenderOptions::default() });
        let colored = d.render_with(&sources, &RenderOptions { color: true, width: 0, ..RenderOptions::default() });
        assert!(colored.text.contains("\x1b[36ma.bd:1:1\x1b[0m"), "{}", colored.text);
        assert!(colored.text.contains("\x1b[1;31merror\x1b[0m"), "{}", colored.text);
        assert!(colored.text.contains("\x1b[33mE0003\x1b[0m"), "{}", colored.text);
        assert_eq!(strip_ansi(&colored.text), plain.text);
    }

    #[test]
    fn human_wraps_long_messages_to_the_width() {
        let mut sources = SourceMap::default();
        let file = sources.add("a.bd".into(), "x\n".into());
        let mut d = Diagnostics::default();
        d.error(Code::UNKNOWN_NAME, Span::new(file, 0, 1), "unknown name `a_very_long_identifier_that_does_not_fit`");
        let rendered = d.render_with(&sources, &RenderOptions { width: 40, ..RenderOptions::default() });
        let lines: Vec<&str> = rendered.text.lines().collect();
        assert!(lines.len() > 1, "{}", rendered.text);
        for line in &lines {
            assert!(line.chars().count() <= 40, "line is too long: {line:?}");
        }
        assert!(lines[1].starts_with(HEADER_INDENT), "{:?}", rendered.text);
        // Wrapping never inserts a character: the message is the lines with the break and indentation removed.
        let joined = rendered.text.lines().map(str::trim_start).collect::<Vec<_>>().join("");
        assert!(joined.contains("`a_very_long_identifier_that_does_not_fit`"), "{joined}");
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
            "\"replacement\":\"mut \"}]}],\"rendered\":\"a.bd:1:5: error[E0114]: pass the argument as `mut ...`\\n",
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
            "a.bd:1:9: error[E0003]: mismatch\n\
             \x20 = note: a.bd:1:1: see this\n\
             \x20 = help: convert it\n\
             \x20 = warning[E0002]: a.bd:1:5: also unused\n"
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
                "\"rendered\":\"a.bd:1:1: error[E0002]: unknown name `a_very_long_identifier_that_does_not_fit`\\n\""
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
            "\x1b]8;;https://ricardomilos7.github.io/burdin/errors/E0003\x1b\\\x1b[33mE0003\x1b[0m\x1b]8;;\x1b\\";
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
