//! The session model (`spec/repl.md`, RP1-RP2): the virtual file, its chunks, and the classification of an
//! input.
//!
//! The session is the entry module file, `main.bd`, of one in-memory package: `fn main() {` first, one chunk of
//! statements per accepted body input inside it, then the accepted items in input order below. The file is
//! regenerated from the chunks after every input, so the accepted part of a session is exactly the text a build
//! of it would compile, and a diagnostic's line is the line of the chunk it names.
//!
//! `main` comes first for a reason the spec records: the interpreter's values name closures and vtables by
//! index, and the session's own declarations come before any item's in those lists, so appending items keeps
//! every index the session's bindings hold stable.

use burdin_diag::{Diagnostics, FileId};

/// The path of the session's file, the name diagnostics show.
pub const SESSION_FILE: &str = "main.bd";

/// The name of the function whose body grows: the session reserves it (`fn main` is the compiled program's entry).
pub const SESSION_FN: &str = "main";

/// The local an expression's value is printed through.
const PRINT_VALUE: &str = "__repl_v";

/// `fn main() {`, the file's first line.
const MAIN_OPEN: &str = "fn main() {";

/// One accepted input unit.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Chunk {
    /// Module content: items and `use` declarations, verbatim.
    Item(String),
    /// Statements of the session function, verbatim or with the print wrap.
    Body(String),
}

/// The session so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    chunks: Vec<Chunk>,
}

/// Where a chunk lands in the session file: its byte range, and the number of the line it starts on.
pub struct Placement {
    pub lo: u32,
    pub hi: u32,
}

/// The text of the session file as it stands.
pub fn text(session: &Session) -> String {
    let mut out = String::new();
    out.push_str(MAIN_OPEN);
    out.push('\n');
    for chunk in &session.chunks {
        if let Chunk::Body(text) = chunk {
            out.push_str(text);
            out.push('\n');
        }
    }
    out.push_str("}\n");
    for chunk in &session.chunks {
        if let Chunk::Item(text) = chunk {
            out.push('\n');
            out.push_str(text);
            out.push('\n');
        }
    }
    out
}

/// The byte range of body chunk `index` (the index into the session's chunks, as
/// [`Session::push_body`] left them) in the file [`text`] builds. `None` for an item chunk.
pub fn body_place(session: &Session, index: usize) -> Option<Placement> {
    let mut offset = MAIN_OPEN.len() as u32 + 1;
    for (at, chunk) in session.chunks.iter().enumerate() {
        let Chunk::Body(text) = chunk else { continue };
        let placement = Placement { lo: offset, hi: offset + text.len() as u32 + 1 };
        if at == index {
            return Some(placement);
        }
        offset = placement.hi;
    }
    None
}

impl Session {
    /// The number of chunks.
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Appends module content (an item input, or `:load`'s file).
    pub fn push_item(&mut self, text: &str) {
        self.chunks.push(Chunk::Item(text.to_string()));
    }

    /// Appends one body input.
    pub fn push_body(&mut self, text: &str) {
        self.chunks.push(Chunk::Body(text.to_string()));
    }

    /// The text of the last chunk, which must be a body chunk (`None` otherwise).
    pub fn last_body(&self) -> Option<&str> {
        match self.chunks.last() {
            Some(Chunk::Body(text)) => Some(text),
            _ => None,
        }
    }

    /// Drops the last chunk (`None` when the session is empty).
    pub fn pop(&mut self) -> Option<String> {
        self.chunks.pop().map(|chunk| match chunk {
            Chunk::Item(text) | Chunk::Body(text) => text,
        })
    }

    /// Rewrites the last chunk, which must be a body chunk, around its trailing expression `expr` (the byte
    /// range in the chunk's own text): the expression moves into `__repl_v`, and with `print` a `println`
    /// prints it with `Debug` (`spec/repl.md`, RP5). The expression's own lines and columns are unchanged;
    /// everything the wrap adds sits on its own lines, so a diagnostic of the expression points at the text the
    /// user typed. `Err` when the range is not inside the chunk (the caller then keeps the chunk as it is).
    pub fn wrap_last(&mut self, expr: (u32, u32), print: bool) -> Result<(), ()> {
        let Some(Chunk::Body(text)) = self.chunks.last() else { return Err(()) };
        let text = text.clone();
        let (lo, hi) = (expr.0 as usize, expr.1 as usize);
        if lo > hi || hi > text.len() || !text.is_char_boundary(lo) || !text.is_char_boundary(hi) {
            return Err(());
        }
        let mut before = String::new();
        if lo > 0 {
            before.push_str(&text[..lo]);
            if !before.ends_with('\n') {
                let trimmed = before.trim_end_matches([' ', '\t']).len();
                before.truncate(trimmed);
                before.push('\n');
            }
        }
        let mut wrapped = format!("{before}let {PRINT_VALUE} =\n{}", &text[lo..hi]);
        if print {
            wrapped.push_str("\nprintln(\"{(__repl_v):?}\")");
        }
        *self.chunks.last_mut().expect("checked above") = Chunk::Body(wrapped);
        Ok(())
    }
}

/// What one input is (`spec/repl.md`, RP2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    /// Nothing but white space and comments: accepted, nothing appended.
    Nothing,
    /// Module content.
    Item,
    /// Statements of the session function.
    Body,
    /// Syntactically incomplete: keep reading lines.
    Incomplete,
    /// Not parseable: appended as statements so the front end reports it in the session's context.
    Bad,
}

/// Sorts an input by how the session takes it: an item when it parses as module content, statements when it
/// parses as a block, incomplete when every parse error is at the input's end (the parser decides, not a count
/// of brackets), bad otherwise.
pub fn classify(input: &str) -> Input {
    if blank(input) {
        return Input::Nothing;
    }
    let (file_lexed, mut file_diags) = lexed(input);
    let unterminated = file_lexed.truncated;
    let file = burdin_syntax::parser::parse_file(FileId(0), file_lexed, &mut file_diags);
    let has_items = !file.items.is_empty() || !file.uses.is_empty() || !file.tests.is_empty();
    if !file_diags.has_errors() && has_items {
        return Input::Item;
    }
    let wrapped = format!("{{\n{input}\n}}");
    let (wrapped_lexed, mut wrapped_diags) = lexed(&wrapped);
    let block = burdin_syntax::parser::parse_block(FileId(0), wrapped_lexed, &mut wrapped_diags);
    if block.is_some() && !wrapped_diags.has_errors() {
        return Input::Body;
    }
    // The lexer stops at an unterminated string or block comment, and both may legally continue on the next
    // line (`spec/grammar.md`, FM2), so that is incompleteness too.
    if unterminated {
        return Input::Incomplete;
    }
    // Every error past the input's own text: for the item shape that is the end of the input, and for the
    // statement shape the closing brace the probe added, standing for whatever the next line may turn out to
    // be. Another line may complete the input; the parser decides, and a mid-input error never gets here.
    let end = input.len() as u32;
    if all_at(&file_diags, end) || all_at(&wrapped_diags, wrapped.len() as u32 - 1) {
        return Input::Incomplete;
    }
    Input::Bad
}

/// Whether an input is nothing but white space and comments.
fn blank(input: &str) -> bool {
    let (lexed, diags) = lexed(input);
    !lexed.truncated
        && diags.is_empty()
        && lexed.tokens.iter().all(|t| matches!(t.kind, burdin_syntax::token::TokenKind::Eof))
}

/// Lexes `text` into a scratch file: the tokens and the diagnostics.
fn lexed(text: &str) -> (burdin_syntax::lexer::LexResult, Diagnostics) {
    let mut diags = Diagnostics::default();
    let lexed = burdin_syntax::lexer::lex(FileId(0), text, &mut diags);
    (lexed, diags)
}

/// Whether every diagnostic of `diags` sits at `at` or past it.
fn all_at(diags: &Diagnostics, at: u32) -> bool {
    diags.has_errors() && diags.iter().all(|d| d.span.lo >= at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_of(s: &Session) -> String {
        text(s)
    }

    fn chunk_text(s: &Session, index: usize) -> String {
        let _ = index;
        text(s)
    }

    #[test]
    fn the_session_file_has_the_documented_shape() {
        let mut s = Session::default();
        s.push_item("fn helper(): i32 { 1 }");
        s.push_body("let x = 1");
        s.push_item("use std.fmt");
        s.push_body("x + helper()");
        assert_eq!(
            chunk_text(&s, 0),
            "fn main() {\nlet x = 1\nx + helper()\n}\n\nfn helper(): i32 { 1 }\n\nuse std.fmt\n"
        );
    }

    #[test]
    fn a_body_chunk_place_names_its_lines() {
        let mut s = Session::default();
        s.push_item("fn helper(): i32 { 1 }");
        s.push_body("let x = 1");
        s.push_body("x + helper()\nx");
        let first = body_place(&s, 1).unwrap();
        assert_eq!(first.lo, 12);
        let second = body_place(&s, 2).unwrap();
        assert_eq!(second.lo, first.hi);
        assert!(body_place(&s, 0).is_none(), "an item chunk has no body place");
    }

    #[test]
    fn the_wrap_keeps_the_expression_lines_and_columns() {
        let mut s = Session::default();
        s.push_body("let a = 1\na + 2");
        let place = body_place(&s, 0).unwrap();
        // `a + 2` is the chunk's text from past the first statement and its newline on.
        let expr_lo = "let a = 1\n".len() as u32;
        let expr_hi = place.hi - place.lo - 1;
        s.wrap_last((expr_lo, expr_hi), true).unwrap();
        assert_eq!(
            chunk_text(&s, 0),
            "fn main() {\nlet a = 1\nlet __repl_v =\na + 2\nprintln(\"{(__repl_v):?}\")\n}\n"
        );
    }

    #[test]
    fn a_wrap_without_print_binds_without_printing() {
        let mut s = Session::default();
        s.push_body("1 + 2");
        let place = body_place(&s, 0).unwrap();
        s.wrap_last((0, place.hi - place.lo - 1), false).unwrap();
        assert_eq!(file_of(&s), "fn main() {\nlet __repl_v =\n1 + 2\n}\n");
    }

    #[test]
    fn the_wrap_starts_a_new_line_when_the_expression_shares_one() {
        let mut s = Session::default();
        s.push_body("let a = 1; a");
        let place = body_place(&s, 0).unwrap();
        let expr_lo = "let a = 1; ".len() as u32;
        s.wrap_last((expr_lo, place.hi - place.lo - 1), true).unwrap();
        assert_eq!(chunk_text(&s, 0), "fn main() {\nlet a = 1;\nlet __repl_v =\na\nprintln(\"{(__repl_v):?}\")\n}\n");
    }

    #[test]
    fn items_and_statements_are_told_apart() {
        assert_eq!(classify("fn f(): i32 { 1 }"), Input::Item);
        assert_eq!(classify("use std.fmt"), Input::Item);
        assert_eq!(classify("struct S { x: i32 }"), Input::Item);
        assert_eq!(classify("test \"t\" { }"), Input::Item);
        assert_eq!(classify("let x = 1"), Input::Body);
        assert_eq!(classify("x + 1"), Input::Body);
        assert_eq!(classify("println(\"hi\")"), Input::Body);
        assert_eq!(classify("x = 5"), Input::Body);
        assert_eq!(classify(""), Input::Nothing);
        assert_eq!(classify("// only a comment"), Input::Nothing);
    }

    #[test]
    fn incomplete_inputs_keep_reading() {
        // The parser decides: every error at the end of the input means another line may complete it.
        assert_eq!(classify("fn f() {"), Input::Incomplete);
        assert_eq!(classify("let x ="), Input::Incomplete);
        assert_eq!(classify("if x {"), Input::Incomplete);
        assert_eq!(classify("x ="), Input::Incomplete);
        assert_eq!(classify("x +"), Input::Incomplete);
        assert_eq!(classify("1 + 2 +"), Input::Incomplete);
        assert_eq!(classify("f(1"), Input::Incomplete);
        assert_eq!(classify("\"a string"), Input::Incomplete);
        assert_eq!(classify("/* a comment"), Input::Incomplete);
        assert_eq!(classify("match x {"), Input::Incomplete);
    }

    #[test]
    fn wrong_inputs_are_not_incomplete() {
        assert_eq!(classify("let = 5"), Input::Bad);
        assert_eq!(classify("}"), Input::Bad);
        assert_eq!(classify("let x = )"), Input::Bad);
        assert_eq!(classify("strct S { }"), Input::Bad);
        assert_eq!(classify("match x { 1 => }"), Input::Bad);
    }
}
