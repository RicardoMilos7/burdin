//! Source files, spans, and diagnostics.
//!
//! Every diagnostic carries an error code from the registry in `tests/README.md`, a severity, and a primary span.
//! It may also carry related notes (with their own spans) and suggestions (a replacement edit for one span), which
//! the JSON format exposes for an editor's code actions (issue #435).
//!
//! Three formats are rendered from the same data (issue #434):
//!
//! - `human`: the default. It colors the location, the severity, and the code when asked to, and wraps long
//!   messages to a fixed width. Its first line is always
//!   `path:LINE:COL: error[E0000]: message`, which the conformance runner reads.
//! - `short`: exactly one plain line per diagnostic, with no notes, suggestions, or color. This is the
//!   machine-readable format the test scripts parse.
//! - `json`: one JSON object per diagnostic on one line, with the primary span, notes, and suggestions. Colors
//!   and wrapping never apply to it.
//!
//! Diagnostics are sorted by file and byte offset (then by code, message, and insertion order, so equal spans are
//! stable), and the optional error limit is applied after sorting, which keeps the output deterministic
//! (issue #436).

use std::fmt::Write as _;

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
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let start = self.line_starts[line] as usize;
        let end = (offset as usize).min(self.text.len());
        let col = self.text.get(start..end).map(|s| s.chars().count()).unwrap_or(end - start);
        (line as u32 + 1, col as u32 + 1)
    }
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
    /// Not in the registry: a construct the language has but this compiler does not implement yet.
    /// Every remaining site is assigned to a milestone in `spec/e9000-audit.md`, and `tests/check_e9000.py`
    /// fails when a site is unassigned, stale, or belongs to a milestone that has shipped.
    pub const UNSUPPORTED: Code = Code("E9000");
}

/// How serious a diagnostic is. Only errors are produced today; the data model carries the severity so the
/// JSON output and the human header do not have to assume it.
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

/// A fix for one span. `span.lo == span.hi` inserts `replacement`; an empty replacement deletes the span. This is
/// the shape an editor needs for a quick-fix code action.
#[derive(Clone, Debug)]
pub struct Suggestion {
    pub message: String,
    pub span: Span,
    pub replacement: String,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub code: Code,
    pub severity: Severity,
    pub span: Span,
    pub message: String,
    pub notes: Vec<Note>,
    pub suggestions: Vec<Suggestion>,
}

impl Diagnostic {
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
        self.suggestions.push(Suggestion { message: message.into(), span, replacement: replacement.into() });
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
        self.list.push(Diagnostic {
            code,
            severity: Severity::Error,
            span,
            message: message.into(),
            notes: Vec::new(),
            suggestions: Vec::new(),
        });
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
        for d in &sorted[..shown] {
            match options.format {
                DiagnosticFormat::Human => render_human(&mut text, d, sources, options),
                DiagnosticFormat::Short => render_short(&mut text, d, sources),
                DiagnosticFormat::Json => render_json(&mut text, d, sources),
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
    /// One JSON object per line, with structured spans, notes, and suggestions.
    Json,
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
}

impl Default for RenderOptions {
    fn default() -> RenderOptions {
        RenderOptions { format: DiagnosticFormat::Human, color: false, width: 80, error_limit: None }
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
        format!("{LOCATION}{location}{RESET}: {SEVERITY}{severity}{RESET}[{CODE}{code}{RESET}]: ")
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
    for suggestion in &d.suggestions {
        let plain = String::from("  = help: ");
        let prefix = if options.color { format!("  = {HELP}help{RESET}: ") } else { plain.clone() };
        push_wrapped(out, &prefix, plain.chars().count(), &suggestion.message, options.width, DETAIL_INDENT);
    }
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

fn render_json(out: &mut String, d: &Diagnostic, sources: &SourceMap) {
    let _ = write!(
        out,
        "{{\"type\":\"diagnostic\",\"severity\":\"{}\",\"code\":\"{}\",\"span\":{},\"message\":\"{}\"",
        d.severity.as_str(),
        d.code.0,
        json_span(sources, d.span),
        json_escape(&d.message)
    );
    out.push_str(",\"notes\":[");
    for (i, note) in d.notes.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "{{\"message\":\"{}\",\"span\":", json_escape(&note.message));
        match note.span {
            Some(span) => out.push_str(&json_span(sources, span)),
            None => out.push_str("null"),
        }
        out.push('}');
    }
    out.push_str("],\"suggestions\":[");
    for (i, suggestion) in d.suggestions.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"message\":\"{}\",\"span\":{},\"replacement\":\"{}\"}}",
            json_escape(&suggestion.message),
            json_span(sources, suggestion.span),
            json_escape(&suggestion.replacement)
        );
    }
    out.push_str("]}\n");
}

fn json_span(sources: &SourceMap, span: Span) -> String {
    let file = sources.get(span.file);
    let (line, column) = file.line_col(span.lo);
    let (end_line, end_column) = file.line_col(span.hi);
    format!(
        "{{\"file\":\"{}\",\"line\":{},\"column\":{},\"end_line\":{},\"end_column\":{},\"byte_start\":{},\"byte_end\":{}}}",
        json_escape(&file.path),
        line,
        column,
        end_line,
        end_column,
        span.lo,
        span.hi
    )
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

    #[test]
    fn json_contains_structured_spans_notes_and_suggestions() {
        let mut sources = SourceMap::default();
        let file = sources.add("a.bd".into(), "mut x\n".into());
        let mut d = Diagnostics::default();
        d.error(Code::MISSING_MUT_MARKER, Span::new(file, 4, 5), "pass the argument as `mut ...`")
            .note(Span::new(file, 0, 3), "declared here")
            .suggest(Span::new(file, 4, 4), "add `mut` before this argument", "mut ");
        let rendered = d.render_with(
            &sources,
            &RenderOptions { format: DiagnosticFormat::Json, color: true, ..RenderOptions::default() },
        );
        assert_eq!(
            rendered.text,
            "{\"type\":\"diagnostic\",\"severity\":\"error\",\"code\":\"E0114\",\
             \"span\":{\"file\":\"a.bd\",\"line\":1,\"column\":5,\"end_line\":1,\"end_column\":6,\
             \"byte_start\":4,\"byte_end\":5},\"message\":\"pass the argument as `mut ...`\",\
             \"notes\":[{\"message\":\"declared here\",\"span\":{\"file\":\"a.bd\",\"line\":1,\"column\":1,\
             \"end_line\":1,\"end_column\":4,\"byte_start\":0,\"byte_end\":3}}],\
             \"suggestions\":[{\"message\":\"add `mut` before this argument\",\"span\":{\"file\":\"a.bd\",\
             \"line\":1,\"column\":5,\"end_line\":1,\"end_column\":5,\"byte_start\":4,\"byte_end\":4},\
             \"replacement\":\"mut \"}]}\n"
        );
        assert!(!rendered.text.contains('\x1b'));
    }

    #[test]
    fn json_represents_spanless_notes_and_deletions() {
        let mut sources = SourceMap::default();
        let file = sources.add("a.bd".into(), "mut x\n".into());
        let mut d = Diagnostics::default();
        d.error(Code::UNNEEDED_MUT_MARKER, Span::new(file, 4, 5), "no marker")
            .note_without_span("see the guide")
            .suggest(Span::new(file, 0, 4), "remove the `mut` marker", "");
        let rendered =
            d.render_with(&sources, &RenderOptions { format: DiagnosticFormat::Json, ..RenderOptions::default() });
        assert_eq!(
            rendered.text,
            "{\"type\":\"diagnostic\",\"severity\":\"error\",\"code\":\"E0115\",\
             \"span\":{\"file\":\"a.bd\",\"line\":1,\"column\":5,\"end_line\":1,\"end_column\":6,\
             \"byte_start\":4,\"byte_end\":5},\"message\":\"no marker\",\
             \"notes\":[{\"message\":\"see the guide\",\"span\":null}],\
             \"suggestions\":[{\"message\":\"remove the `mut` marker\",\"span\":{\"file\":\"a.bd\",\
             \"line\":1,\"column\":1,\"end_line\":1,\"end_column\":5,\"byte_start\":0,\"byte_end\":4},\
             \"replacement\":\"\"}]}\n"
        );
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
