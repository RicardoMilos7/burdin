//! `burdin fmt`: the formatter (issue #229, `spec/fmt.md`). It has no options: one style, 4-space indentation, lines of
//! at most 120 columns, and every comment kept. It reads source with the compiler's lexer, lays it out with its own
//! parser (which keeps what the compiler's syntax tree throws away: comments, parentheses, the order of `use`
//! declarations, how a `select` was written), and checks its own output with the compiler's parser before returning
//! it.

mod blocks;
mod cursor;
pub mod doc;
mod docs;
mod exprs;
mod items;
mod layout;
mod trivia;
mod types;
mod uses;
mod verify;

use burdin_diag::{Diagnostics, FileId};
use cursor::Fmt;
use doc::Doc;
use items::Unit;

pub use uses::UseGroups;

/// The width of a line in columns.
pub const WIDTH: usize = 120;

/// How many times the layout may be repeated before the formatter gives up on reaching a fixed point.
const MAX_ROUNDS: usize = 4;

/// What the formatter needs to know besides the source. There is no style to choose.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Which names a `use` declaration may start with to count as the standard library or a dependency, for grouping.
    pub groups: UseGroups,
}

pub enum FormatError {
    /// The source does not parse. Nothing is formatted; these are the compiler's own diagnostics.
    Syntax(Diagnostics),
    /// The source parses but the formatter could not lay it out, or its output was not the same program. This is a
    /// bug in the formatter; the file must be left alone.
    Internal(String),
}

impl std::fmt::Debug for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatError::Syntax(_) => write!(f, "the source has syntax errors"),
            FormatError::Internal(message) => write!(f, "internal formatter error: {message}"),
        }
    }
}

/// Formats one source file. The result ends with exactly one line break (an empty file stays empty) and uses the
/// line endings of the input.
pub fn format_source(src: &str, options: &Options) -> Result<String, FormatError> {
    format_with(src, options, None)
}

/// Formats the items that overlap the byte range `lo..hi` and leaves the rest of the file as it is (FM11).
pub fn format_range(src: &str, lo: usize, hi: usize, options: &Options) -> Result<String, FormatError> {
    format_with(src, options, Some((lo, hi)))
}

/// The layout of `src` without the checks of [`format_source`], for debugging the formatter itself.
#[doc(hidden)]
pub fn format_unverified(src: &str, options: &Options) -> Result<String, String> {
    layout_impl(src, options, WIDTH, true, None)
}

/// The source text in the form the formatter works on: line breaks are `\n`. Returns whether the file used `\r\n`.
fn normalize(src: &str) -> (String, bool) {
    let crlf = src.contains("\r\n") && !src.replace("\r\n", "").contains('\n');
    if crlf { (src.replace("\r\n", "\n"), true) } else { (src.to_string(), false) }
}

fn format_with(src: &str, options: &Options, range: Option<(usize, usize)>) -> Result<String, FormatError> {
    let (text, crlf) = normalize(src);
    let range = range.map(|(lo, hi)| {
        // Offsets are into the original text; `\r` bytes before them do not exist in the normalized text.
        let shift = |at: usize| if crlf { at - src[..at.min(src.len())].matches('\r').count() } else { at };
        (shift(lo.min(src.len())), shift(hi.min(src.len())))
    });
    let mut diags = Diagnostics::default();
    burdin_syntax::parse(FileId(0), &text, &mut diags);
    if diags.has_errors() {
        return Err(FormatError::Syntax(diags));
    }
    let mut out = layout_impl(&text, options, WIDTH, true, range).map_err(FormatError::Internal)?;
    if let Err(message) = verify::same_program(&text, &out) {
        return Err(FormatError::Internal(message));
    }
    if range.is_none() {
        // A comment in an odd place can move to where it reads differently the second time (it was between two
        // tokens, and is now at the end of a line). Laying the output out again until it stops changing makes the
        // result a fixed point (FM1) however the source was written; every round is checked against the source.
        let mut rounds = 0;
        loop {
            let next = layout_impl(&out, options, WIDTH, true, None).map_err(FormatError::Internal)?;
            if next == out {
                break;
            }
            if let Err(message) = verify::same_program(&text, &next) {
                return Err(FormatError::Internal(message));
            }
            rounds += 1;
            if rounds >= MAX_ROUNDS {
                return Err(FormatError::Internal("the layout does not settle: it changes on every pass".to_string()));
            }
            out = next;
        }
    }
    Ok(if crlf { out.replace('\n', "\r\n") } else { out })
}

/// Lays out a whole file at `width` columns. `docs` says whether the code blocks of doc comments are formatted too.
fn layout_text(text: &str, options: &Options, width: usize, docs: bool) -> Result<String, String> {
    layout_impl(text, options, width, docs, None)
}

fn layout_impl(
    text: &str,
    options: &Options,
    width: usize,
    docs: bool,
    range: Option<(usize, usize)>,
) -> Result<String, String> {
    let lexed = trivia::lex(text)?;
    let mut fmt = Fmt::new(text, lexed.toks, lexed.raw, width);
    fmt.docs = docs;
    let (units, eof) = fmt.file().map_err(|e| e.0)?;
    if units.is_empty() && eof.is_empty() {
        return Ok(if range.is_some() { text.to_string() } else { String::new() });
    }
    let mut out = String::new();
    match range {
        None => {
            let mut parts: Vec<(bool, Doc)> = arrange_units(units, &options.groups);
            let mut last_end_comments = Vec::new();
            for c in &eof {
                last_end_comments.push((c.nl_before >= 2, c.text.clone()));
            }
            let mut doc_parts = Vec::new();
            for (i, (blank, d)) in parts.drain(..).enumerate() {
                if i > 0 {
                    doc_parts.push(Doc::Hard);
                    if blank {
                        doc_parts.push(Doc::Hard);
                    }
                }
                doc_parts.push(d);
            }
            let had_content = !doc_parts.is_empty();
            for (i, (blank, comment)) in last_end_comments.into_iter().enumerate() {
                if had_content || i > 0 {
                    doc_parts.push(Doc::Hard);
                    if blank {
                        doc_parts.push(Doc::Hard);
                    }
                }
                doc_parts.push(if comment.contains('\n') { Doc::Verbatim(comment) } else { doc::text(comment) });
            }
            let mut whole = doc::concat(doc_parts);
            doc::prepare(&mut whole);
            out.push_str(&doc::print(&whole, width));
            while out.ends_with('\n') || out.ends_with(' ') {
                out.pop();
            }
            out.push('\n');
        }
        Some((lo, hi)) => out = format_units_in_range(text, units, &eof, lo, hi, width),
    }
    Ok(out)
}

/// Turns the units of a file into documents in order, sorting and grouping each run of `use` declarations of one
/// visibility. The flag is whether a blank line goes before the document.
fn arrange_units(units: Vec<Unit>, groups: &UseGroups) -> Vec<(bool, Doc)> {
    let mut out = Vec::new();
    let mut run: Vec<uses::UseEntry> = Vec::new();
    let mut run_blank = false;
    let mut run_pub = false;
    let flush = |run: &mut Vec<uses::UseEntry>, blank: bool, out: &mut Vec<(bool, Doc)>| {
        if run.is_empty() {
            return;
        }
        for (i, (new_group, doc)) in uses::arrange(std::mem::take(run), groups).into_iter().enumerate() {
            out.push((if i == 0 { blank } else { new_group }, doc));
        }
    };
    for unit in units {
        match unit.use_entry {
            Some(entry) => {
                if !run.is_empty() && entry.info.is_pub != run_pub {
                    flush(&mut run, run_blank, &mut out);
                }
                if run.is_empty() {
                    run_blank = unit.blank_before;
                    run_pub = entry.info.is_pub;
                }
                run.push(entry);
            }
            None => {
                flush(&mut run, run_blank, &mut out);
                out.push((unit.blank_before, unit.doc));
            }
        }
    }
    flush(&mut run, run_blank, &mut out);
    out
}

/// Range formatting: each unit that overlaps `lo..hi` is formatted on its own, the others are copied as written.
fn format_units_in_range(
    text: &str,
    units: Vec<Unit>,
    eof: &[trivia::Comment],
    lo: usize,
    hi: usize,
    width: usize,
) -> String {
    let mut out = String::new();
    let mut cursor = 0usize;
    for mut unit in units {
        let overlaps = unit.start < hi.max(lo + 1) && unit.end > lo;
        out.push_str(&text[cursor..unit.start]);
        if overlaps {
            let mut doc = match unit.use_entry.take() {
                Some(entry) => doc::concat(vec![entry.lead, entry.info.doc(), entry.suffix]),
                None => unit.doc,
            };
            doc::prepare(&mut doc);
            out.push_str(doc::print(&doc, width).trim_end());
        } else {
            out.push_str(&text[unit.start..unit.end]);
        }
        cursor = unit.end;
    }
    out.push_str(&text[cursor..]);
    let _ = eof;
    out
}
