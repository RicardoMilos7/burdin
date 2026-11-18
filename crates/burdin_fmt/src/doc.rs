//! The document model the formatter lays code out with, and the printer that turns it into text.
//!
//! This is the classic "group, indent, line" model (Wadler; the same shape as Prettier's): a [`Doc::Group`] prints on
//! one line when it fits in the remaining width and breaks all of its [`Doc::Line`]s otherwise. Two additions serve
//! comments and call arguments: [`Doc::Suffix`] is a trailing comment that moves to the end of the printed line
//! (so it can never swallow the code after it), and [`Doc::Alt`] tries layouts in order (the first that fits wins),
//! which is how the last argument of a call, a closure or a struct literal, can stay on the line of the call.

pub const INDENT: usize = 4;

#[derive(Clone, Debug)]
pub enum Doc {
    Nil,
    /// Text without line breaks.
    Text(String),
    /// Text that may contain line breaks, printed exactly as written (a multi-line block comment or string). It
    /// forces every enclosing group to break.
    Verbatim(String),
    Concat(Vec<Doc>),
    /// Contents that print flat when they fit. The flag is set by [`prepare`] when the contents cannot be flat.
    Group(Box<Doc>, bool),
    Indent(Box<Doc>),
    /// A space when the enclosing group is flat, a line break when it is broken.
    Line,
    /// Nothing when the enclosing group is flat, a line break when it is broken.
    Soft,
    /// A line break in every mode; every enclosing group breaks.
    Hard,
    /// `(when the group is broken, when it is flat)`.
    IfBreak(Box<Doc>, Box<Doc>),
    /// A trailing comment: printed at the end of the current line. If more text follows it inside a group, the group
    /// breaks, so the comment is never followed by code on its line.
    Suffix(String),
    /// Layouts in order of preference; the last is the fallback. Breaks inside the alternatives do not break the
    /// groups around the `Alt`.
    Alt(Vec<Doc>),
}

pub fn text(s: impl Into<String>) -> Doc {
    let s = s.into();
    if s.is_empty() { Doc::Nil } else { Doc::Text(s) }
}

pub fn concat(parts: Vec<Doc>) -> Doc {
    Doc::Concat(parts)
}

pub fn group(doc: Doc) -> Doc {
    Doc::Group(Box::new(doc), false)
}

/// A group that is already broken: its lines are newlines.
pub fn broken_group(doc: Doc) -> Doc {
    Doc::Group(Box::new(doc), true)
}

pub fn indent(doc: Doc) -> Doc {
    Doc::Indent(Box::new(doc))
}

pub fn if_break(broken: Doc, flat: Doc) -> Doc {
    Doc::IfBreak(Box::new(broken), Box::new(flat))
}

pub fn space() -> Doc {
    Doc::Text(" ".to_string())
}

impl Doc {
    pub fn is_nil(&self) -> bool {
        match self {
            Doc::Nil => true,
            Doc::Concat(parts) => parts.iter().all(Doc::is_nil),
            _ => false,
        }
    }

    /// Whether printing this document must produce a line break (a hard break, or text with newlines), looking
    /// through everything but the alternatives of an [`Doc::Alt`].
    pub fn has_forced_break(&self) -> bool {
        match self {
            Doc::Hard => true,
            Doc::Verbatim(s) => s.contains('\n'),
            Doc::Concat(parts) => parts.iter().any(Doc::has_forced_break),
            Doc::Group(inner, _) => inner.has_forced_break(),
            Doc::Indent(inner) => inner.has_forced_break(),
            Doc::IfBreak(a, b) => a.has_forced_break() && b.has_forced_break(),
            Doc::Alt(alts) => alts.last().is_some_and(Doc::has_forced_break),
            _ => false,
        }
    }
}

// ----- preparing: which groups cannot be flat -------------------------------------------------------------------

#[derive(Default)]
struct Prep {
    /// The number of groups open right now.
    depth: usize,
    /// Groups with an index below this one (counting from the outermost) must break.
    force_below: usize,
    /// A trailing comment was printed while this many groups were open (reduced as groups close); the next text
    /// breaks those groups.
    pending: Option<usize>,
}

impl Prep {
    fn force_all_open(&mut self) {
        self.force_below = self.force_below.max(self.depth);
    }

    fn text_seen(&mut self) {
        if let Some(open) = self.pending.take() {
            self.force_below = self.force_below.max(open);
        }
    }
}

/// Marks every group that must break: those with a forced line break inside, and those where text follows a
/// trailing comment. Call it once before [`print`].
pub fn prepare(doc: &mut Doc) {
    walk(doc, &mut Prep::default());
}

fn walk(doc: &mut Doc, st: &mut Prep) {
    match doc {
        Doc::Nil => {}
        Doc::Text(s) => {
            if !s.is_empty() {
                st.text_seen();
            }
        }
        Doc::Verbatim(s) => {
            st.text_seen();
            if s.contains('\n') {
                st.force_all_open();
            }
        }
        Doc::Concat(parts) => {
            for part in parts {
                walk(part, st);
            }
        }
        Doc::Indent(inner) => walk(inner, st),
        Doc::Group(inner, broken) => {
            let index = st.depth;
            let already_broken = *broken;
            st.depth += 1;
            walk(inner, st);
            st.depth -= 1;
            if already_broken {
                // A group made broken on purpose (the magic trailing comma) breaks the groups around it too.
                st.force_all_open();
            }
            if index < st.force_below {
                *broken = true;
                st.force_below = index;
            }
            if let Some(open) = st.pending {
                st.pending = Some(open.min(index));
            }
        }
        Doc::Line => st.text_seen(),
        Doc::Soft => {}
        Doc::Hard => {
            st.text_seen();
            st.force_all_open();
        }
        Doc::IfBreak(broken, flat) => {
            walk(broken, st);
            walk(flat, st);
        }
        Doc::Suffix(_) => st.pending = Some(st.depth),
        Doc::Alt(alts) => {
            let (force_below, pending) = (st.force_below, st.pending);
            for alt in alts {
                st.force_below = force_below;
                st.pending = pending;
                walk(alt, st);
            }
            st.force_below = force_below;
            st.pending = None;
        }
    }
}

// ----- printing ------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Flat,
    Break,
}

type Cmd<'a> = (usize, Mode, &'a Doc);

fn width_of(s: &str) -> usize {
    s.chars().count()
}

/// Whether the commands `next`, followed by what is already on the stack, print up to their first line break
/// within `remaining` columns.
fn fits(next: Cmd<'_>, rest: &[Cmd<'_>], mut remaining: isize) -> bool {
    let mut stack: Vec<(Mode, &Doc)> = vec![(next.1, next.2)];
    let mut rest_index = rest.len();
    loop {
        if remaining < 0 {
            return false;
        }
        let (mode, doc) = match stack.pop() {
            Some(entry) => entry,
            None => {
                if rest_index == 0 {
                    return true;
                }
                rest_index -= 1;
                (rest[rest_index].1, rest[rest_index].2)
            }
        };
        match doc {
            Doc::Nil | Doc::Suffix(_) => {}
            Doc::Text(s) => remaining -= width_of(s) as isize,
            Doc::Verbatim(s) => match s.find('\n') {
                Some(at) => return remaining - width_of(&s[..at]) as isize >= 0,
                None => remaining -= width_of(s) as isize,
            },
            Doc::Concat(parts) => {
                for part in parts.iter().rev() {
                    stack.push((mode, part));
                }
            }
            Doc::Indent(inner) => stack.push((mode, inner)),
            Doc::Group(inner, broken) => stack.push((if *broken { Mode::Break } else { mode }, inner)),
            Doc::Line => match mode {
                Mode::Break => return true,
                Mode::Flat => remaining -= 1,
            },
            Doc::Soft => {
                if mode == Mode::Break {
                    return true;
                }
            }
            Doc::Hard => return true,
            Doc::IfBreak(broken, flat) => stack.push((mode, if mode == Mode::Break { broken } else { flat })),
            Doc::Alt(alts) => {
                let chosen = if mode == Mode::Break { alts.last() } else { alts.first() };
                if let Some(chosen) = chosen {
                    stack.push((mode, chosen));
                }
            }
        }
    }
}

/// Prints a prepared document in lines of at most `width` columns (a line that cannot be broken is longer).
pub fn print(doc: &Doc, width: usize) -> String {
    let mut out = String::new();
    let mut column = 0usize;
    let mut line_indent = 0usize;
    let mut suffixes: Vec<&str> = Vec::new();
    let mut stack: Vec<Cmd<'_>> = vec![(0, Mode::Break, doc)];

    /// Writes the pending trailing comments. A line comment runs to the end of its line, so the comment after it goes
    /// on a line of its own instead of becoming part of it.
    fn flush(out: &mut String, suffixes: &mut Vec<&str>, indent: usize) {
        let mut after_line_comment = false;
        for suffix in suffixes.drain(..) {
            if after_line_comment {
                out.push('\n');
                for _ in 0..indent {
                    out.push(' ');
                }
                out.push_str(suffix.trim_start());
            } else {
                out.push_str(suffix);
            }
            after_line_comment = suffix.trim_start().starts_with("//");
        }
    }

    fn newline(out: &mut String, column: &mut usize, suffixes: &mut Vec<&str>, line_indent: &mut usize, indent: usize) {
        flush(out, suffixes, *line_indent);
        while out.ends_with(' ') {
            out.pop();
        }
        out.push('\n');
        for _ in 0..indent {
            out.push(' ');
        }
        *column = indent;
        *line_indent = indent;
    }

    while let Some((indent, mode, doc)) = stack.pop() {
        match doc {
            Doc::Nil => {}
            Doc::Text(s) => {
                if s.starts_with("//") && !suffixes.is_empty() {
                    // A line comment of its own cannot share a line with the trailing comments waiting for it.
                    newline(&mut out, &mut column, &mut suffixes, &mut line_indent, indent);
                }
                out.push_str(s);
                column += width_of(s);
            }
            Doc::Verbatim(s) => {
                out.push_str(s);
                match s.rfind('\n') {
                    Some(at) => column = width_of(&s[at + 1..]),
                    None => column += width_of(s),
                }
            }
            Doc::Concat(parts) => {
                for part in parts.iter().rev() {
                    stack.push((indent, mode, part));
                }
            }
            Doc::Indent(inner) => stack.push((indent + INDENT, mode, inner)),
            Doc::Group(inner, broken) => match mode {
                Mode::Flat => stack.push((indent, if *broken { Mode::Break } else { Mode::Flat }, inner)),
                Mode::Break => {
                    let flat = (indent, Mode::Flat, &**inner);
                    if !*broken && fits(flat, &stack, width as isize - column as isize) {
                        stack.push(flat);
                    } else {
                        stack.push((indent, Mode::Break, inner));
                    }
                }
            },
            Doc::Alt(alts) => {
                let last = alts.len() - 1;
                let mut chosen = None;
                if mode == Mode::Flat {
                    chosen = Some((indent, Mode::Flat, &alts[0]));
                } else {
                    for alt in &alts[..last] {
                        if fits((indent, Mode::Flat, alt), &stack, width as isize - column as isize) {
                            chosen = Some((indent, Mode::Flat, alt));
                            break;
                        }
                    }
                }
                stack.push(chosen.unwrap_or((indent, Mode::Break, &alts[last])));
            }
            Doc::Line => match mode {
                Mode::Flat => {
                    out.push(' ');
                    column += 1;
                }
                Mode::Break => newline(&mut out, &mut column, &mut suffixes, &mut line_indent, indent),
            },
            Doc::Soft => {
                if mode == Mode::Break {
                    newline(&mut out, &mut column, &mut suffixes, &mut line_indent, indent);
                }
            }
            Doc::Hard => newline(&mut out, &mut column, &mut suffixes, &mut line_indent, indent),
            Doc::IfBreak(broken, flat) => stack.push((indent, mode, if mode == Mode::Break { broken } else { flat })),
            Doc::Suffix(s) => suffixes.push(s),
        }
    }
    flush(&mut out, &mut suffixes, line_indent);
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(mut doc: Doc, width: usize) -> String {
        prepare(&mut doc);
        print(&doc, width)
    }

    fn call(args: &[&str]) -> Doc {
        let mut parts = Vec::new();
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                parts.push(text(","));
                parts.push(Doc::Line);
            }
            parts.push(text(*arg));
        }
        group(concat(vec![
            text("f("),
            indent(concat(vec![Doc::Soft, concat(parts), if_break(text(","), Doc::Nil)])),
            Doc::Soft,
            text(")"),
        ]))
    }

    #[test]
    fn a_group_that_fits_stays_on_one_line() {
        assert_eq!(render(call(&["a", "b"]), 20), "f(a, b)");
    }

    #[test]
    fn a_group_that_does_not_fit_breaks_every_line() {
        assert_eq!(render(call(&["aaaa", "bbbb"]), 8), "f(\n    aaaa,\n    bbbb,\n)");
    }

    #[test]
    fn a_hard_break_breaks_the_enclosing_groups() {
        let doc = group(concat(vec![text("a"), Doc::Line, text("b"), Doc::Hard, text("c")]));
        assert_eq!(render(doc, 80), "a\nb\nc");
    }

    #[test]
    fn a_suffix_moves_to_the_end_of_the_line() {
        let doc = concat(vec![text("a"), Doc::Suffix(" // c".into()), text(" b"), Doc::Hard, text("d")]);
        assert_eq!(render(doc, 80), "a b // c\nd");
    }

    #[test]
    fn text_after_a_suffix_breaks_the_group() {
        let doc = group(concat(vec![text("a,"), Doc::Suffix(" // c".into()), Doc::Line, text("b")]));
        assert_eq!(render(doc, 80), "a, // c\nb");
    }

    #[test]
    fn a_suffix_at_the_end_of_a_group_does_not_break_it() {
        let doc =
            concat(vec![group(concat(vec![text("a"), Doc::Line, text("b"), Doc::Suffix(" // c".into())])), Doc::Hard]);
        assert_eq!(render(doc, 80), "a b // c\n");
    }

    #[test]
    fn alternatives_pick_the_first_that_fits() {
        let doc = Doc::Alt(vec![text("short"), text("a much longer layout"), text("x")]);
        assert_eq!(render(doc.clone(), 10), "short");
        let doc = Doc::Alt(vec![text("too long to fit"), text("fits"), text("x")]);
        assert_eq!(render(doc, 10), "fits");
    }

    #[test]
    fn trailing_spaces_are_trimmed() {
        let doc = concat(vec![text("a "), Doc::Hard, text("b")]);
        assert_eq!(render(doc, 80), "a\nb");
    }
}
