//! Source files, spans, and diagnostics.
//!
//! Every diagnostic carries an error code from the registry in `tests/README.md` and a span. Rendering produces
//! one line per diagnostic in the form `path:LINE:COL: error[E0000]: message`, which the test runner reads.

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
    pub const LITERAL_RANGE: Code = Code("E0008");
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
    pub const DERIVE_REQUIREMENT: Code = Code("E0130");
    pub const MISSING_INTERFACE_METHOD: Code = Code("E0140");
    pub const BOUND_NOT_SATISFIED: Code = Code("E0141");
    pub const ORPHAN_RULE: Code = Code("E0142");
    pub const PRIVATE_ITEM: Code = Code("E0150");
    pub const QUESTION_OUTSIDE: Code = Code("E0160");
    pub const UNSAFE_REQUIRED: Code = Code("E0161");
    /// Not in the registry: a construct the language has but this compiler does not implement yet.
    /// It disappears once stage 0 is complete.
    pub const UNSUPPORTED: Code = Code("E9000");
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub code: Code,
    pub span: Span,
    pub message: String,
}

#[derive(Default)]
pub struct Diagnostics {
    list: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn error(&mut self, code: Code, span: Span, message: impl Into<String>) {
        self.list.push(Diagnostic { code, span, message: message.into() });
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
        !self.list.is_empty()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.list.iter()
    }

    /// All diagnostics in source order, one per line.
    pub fn render(&self, sources: &SourceMap) -> String {
        let mut sorted: Vec<&Diagnostic> = self.list.iter().collect();
        sorted.sort_by_key(|d| (d.span.file, d.span.lo));
        let mut out = String::new();
        for d in sorted {
            let file = sources.get(d.span.file);
            let (line, col) = file.line_col(d.span.lo);
            let _ = writeln!(out, "{}:{}:{}: error[{}]: {}", file.path, line, col, d.code.0, d.message);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_column() {
        let f = SourceFile::new("a.bd".into(), "ab\ncé d\n".into());
        assert_eq!(f.line_col(0), (1, 1));
        assert_eq!(f.line_col(3), (2, 1));
        // 'é' is two bytes, so byte 6 (the space) is the third character of line 2.
        assert_eq!(f.line_col(6), (2, 3));
    }
}
