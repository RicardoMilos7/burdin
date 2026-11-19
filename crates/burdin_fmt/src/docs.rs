//! Code blocks in doc comments (spec/fmt.md, FM10). A fenced block in a run of `///` lines whose fence is empty or
//! says `burdin` or `bd` (attributes may follow: `burdin,no_run`) holds Burdin code, and the formatter formats it
//! like the rest of the file: as a list of items when it parses as one, otherwise as the statements of a function
//! body. A block it cannot parse, one whose formatted form is not the same program, and one with a hidden `# `
//! setup line (the marker is not Burdin text, and the line it starts belongs to the reader's setup, not to the
//! formatter) stay as written, and so does a block tagged with any other word (`text`, `console`, ...).

use crate::cursor::Fmt;
use crate::trivia::Comment;
use crate::verify;
use crate::{Options, layout_text};

/// The name of the function a block of statements is wrapped in while it is formatted.
const WRAPPER: &str = "fn __doc() {";

/// The columns a code block may use inside `///` comments at a given brace depth.
pub fn available_width(width: usize, depth: usize) -> usize {
    width.saturating_sub(depth * 4 + 4).max(20)
}

/// Rewrites the code blocks of the `///` runs among `comments`. Comments that are not part of a code block come back
/// unchanged. `width` is the number of columns left for the code.
pub fn reformat(comments: Vec<Comment>, width: usize) -> Vec<Comment> {
    let mut out: Vec<Comment> = Vec::with_capacity(comments.len());
    let mut i = 0;
    while i < comments.len() {
        if !is_doc_line(&comments[i]) {
            out.push(comments[i].clone());
            i += 1;
            continue;
        }
        let end = run_end(&comments, i);
        out.extend(reformat_run(&comments[i..end], width));
        i = end;
    }
    out
}

/// The end of the run of `///` lines that starts at `start`: each line is on the line after the one before.
fn run_end(comments: &[Comment], start: usize) -> usize {
    let mut end = start + 1;
    while end < comments.len() && is_doc_line(&comments[end]) && comments[end].nl_before == 1 {
        end += 1;
    }
    end
}

/// The start and end of every run of `///` lines among `comments`.
fn runs(comments: &[Comment]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < comments.len() {
        if is_doc_line(&comments[i]) {
            let end = run_end(comments, i);
            out.push((i, end));
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

fn is_doc_line(c: &Comment) -> bool {
    c.line && c.text.starts_with("///") && !c.text.starts_with("////")
}

/// The text of a `///` line without the marker and the one space after it.
fn body(c: &Comment) -> &str {
    let rest = &c.text[3..];
    rest.strip_prefix(' ').unwrap_or(rest)
}

/// Whether a fence's info string says the block is Burdin code: the first word is empty, `burdin`, or `bd`, the
/// same word `burdin doc` and the doc tests read (`spec/doc.md` DC6). Attributes may follow it (`burdin,no_run`).
fn is_burdin_fence(info: &str) -> bool {
    let word = info.split([',', ' ', '\t']).next().unwrap_or("");
    matches!(word, "" | "burdin" | "bd")
}

/// Whether a line of a block is a hidden setup line (`# ` or a bare `#`, DC6): the formatter does not touch it.
fn is_hidden_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed == "#" || trimmed.starts_with("# ")
}

/// A fenced block of a run: the indices of its two fence lines, and whether it holds Burdin code.
struct Block {
    open: usize,
    close: usize,
    burdin: bool,
}

/// The fenced blocks of a run of `///` lines. An opening fence with no closing fence is not a block.
fn blocks_in(run: &[Comment]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < run.len() {
        let Some(info) = body(&run[i]).strip_prefix("```") else {
            i += 1;
            continue;
        };
        let Some(close) = (i + 1..run.len()).find(|&k| body(&run[k]).trim_end() == "```") else {
            break;
        };
        out.push(Block { open: i, close, burdin: is_burdin_fence(info.trim()) });
        i = close + 1;
    }
    out
}

/// The indices of the comments that are lines of code inside a Burdin fenced block of a doc comment: the text the
/// formatter is allowed to change.
pub fn code_lines(comments: &[Comment]) -> Vec<usize> {
    let mut out = Vec::new();
    for (lo, hi) in runs(comments) {
        for block in blocks_in(&comments[lo..hi]) {
            if block.burdin {
                out.extend(lo + block.open + 1..lo + block.close);
            }
        }
    }
    out
}

fn reformat_run(run: &[Comment], width: usize) -> Vec<Comment> {
    let mut out: Vec<Comment> = Vec::with_capacity(run.len());
    let mut next = 0;
    for block in blocks_in(run) {
        out.extend(run[next..=block.open].iter().cloned());
        let code: Vec<&str> = run[block.open + 1..block.close].iter().map(body).collect();
        let formatted = if block.burdin && !code.iter().any(|line| is_hidden_line(line)) {
            format_code(&code.join("\n"), width)
        } else {
            None
        };
        match formatted {
            Some(text) => {
                // The new lines take the line breaks of the old block: the first keeps the break before it and the
                // last the break after it.
                let template = &run[block.open + 1];
                let lines: Vec<&str> = text.lines().collect();
                for (k, l) in lines.iter().enumerate() {
                    let mut c = template.clone();
                    c.text = if l.is_empty() { "///".to_string() } else { format!("/// {l}") };
                    c.nl_before = 1;
                    c.nl_after = 1;
                    if k + 1 == lines.len() {
                        c.nl_after = run[block.close].nl_before;
                    }
                    out.push(c);
                }
            }
            None => out.extend(run[block.open + 1..block.close].iter().cloned()),
        }
        out.push(run[block.close].clone());
        next = block.close + 1;
    }
    out.extend(run[next..].iter().cloned());
    out
}

/// Formats the code of one block, or returns `None` to leave it alone. The result has no final line break.
pub fn format_code(code: &str, width: usize) -> Option<String> {
    if code.trim().is_empty() {
        return None;
    }
    let source = format!("{code}\n");
    if let Some(out) = format_items(&source, width) {
        return Some(out);
    }
    // Statements: format them as the body of a function, which indents them by one level.
    let wrapped = format!("{WRAPPER}\n{code}\n}}\n");
    let out = format_items(&wrapped, width + 4)?;
    let lines: Vec<&str> = out.lines().collect();
    if lines.len() < 2 || lines[0] != WRAPPER || lines[lines.len() - 1] != "}" {
        return None;
    }
    let mut body = Vec::new();
    for line in &lines[1..lines.len() - 1] {
        if line.is_empty() {
            body.push(String::new());
        } else {
            body.push(line.strip_prefix("    ")?.to_string());
        }
    }
    let text = body.join("\n");
    // The statements, indented again and wrapped, must be the same program as the formatted wrapper.
    let rewrapped = format!("{WRAPPER}\n{}\n}}\n", indented(&text));
    if verify::same_program(&wrapped, &rewrapped).is_err() {
        return None;
    }
    Some(text)
}

fn indented(text: &str) -> String {
    text.lines().map(|l| if l.is_empty() { String::new() } else { format!("    {l}") }).collect::<Vec<_>>().join("\n")
}

/// Formats `source` as a whole file at `width` columns and checks the result: the same program, and the same text
/// when formatted again. Code blocks nested in the code are not formatted.
fn format_items(source: &str, width: usize) -> Option<String> {
    let options = Options::default();
    let once = layout_text(source, &options, width, false).ok()?;
    verify::same_program(source, &once).ok()?;
    let twice = layout_text(&once, &options, width, false).ok()?;
    if twice != once {
        return None;
    }
    Some(once.trim_end().to_string())
}

impl Fmt<'_> {
    /// The comments of a token that starts a line, with the code blocks of its doc comments formatted.
    pub fn with_doc_blocks(&self, comments: Vec<Comment>) -> Vec<Comment> {
        if !self.docs || !comments.iter().any(is_doc_line) {
            return comments;
        }
        reformat(comments, available_width(self.width, self.depth))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_are_formatted_as_items() {
        let out = format_code("fn  add(a:i32,b:i32):i32{a+b}", 100).expect("formats");
        assert_eq!(out, "fn add(a: i32, b: i32): i32 {\n    a + b\n}");
    }

    #[test]
    fn statements_are_formatted_as_a_body() {
        let out = format_code("let x=1\nprintln( \"{x}\" )", 100).expect("formats");
        assert_eq!(out, "let x = 1\nprintln(\"{x}\")");
    }

    #[test]
    fn code_that_does_not_parse_is_left_alone() {
        assert_eq!(format_code("let = = 1", 100), None);
    }

    #[test]
    fn blank_lines_inside_a_block_stay() {
        let out = format_code("let a=1\n\nlet b=2", 100).expect("formats");
        assert_eq!(out, "let a = 1\n\nlet b = 2");
    }

    #[test]
    fn attributes_after_the_language_still_name_burdin() {
        assert!(is_burdin_fence(""));
        assert!(is_burdin_fence("burdin"));
        assert!(is_burdin_fence("bd"));
        assert!(is_burdin_fence(" burdin "));
        assert!(is_burdin_fence("burdin,no_run"));
        assert!(is_burdin_fence("burdin compile_fail E0003"));
        assert!(!is_burdin_fence("no_run"));
        assert!(!is_burdin_fence("text"));
        assert!(!is_burdin_fence("console"));
    }

    #[test]
    fn hidden_setup_lines_are_recognized() {
        assert!(is_hidden_line("# use app.{Point}"));
        assert!(is_hidden_line("#"));
        assert!(is_hidden_line("    # nested setup"));
        assert!(!is_hidden_line("let x = 1"));
        assert!(!is_hidden_line("#not hidden"));
        assert!(!is_hidden_line(""));
    }

    #[test]
    fn a_block_with_hidden_lines_is_left_as_written() {
        // The `# ` marker is not Burdin text, so the formatter cannot check the block it starts against the
        // formatted program; the whole block keeps the shape its author gave it.
        let comments = |text: &str| -> Vec<Comment> {
            text.lines()
                .map(|line| Comment {
                    text: format!("///{line}"),
                    line: true,
                    lo: 0,
                    hi: 0,
                    nl_before: 1,
                    nl_after: 1,
                    space_after: false,
                })
                .collect()
        };
        let before = comments("\n Doc.\n ```\n # use app.{f}\n assert_eq( f( 3 ) , 6 )\n ```");
        let after = reformat(before, 100);
        let texts: Vec<&str> = after.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(
            texts,
            ["///", "/// Doc.", "/// ```", "/// # use app.{f}", "/// assert_eq( f( 3 ) , 6 )", "/// ```"]
        );
        // Without a hidden line the same block is formatted.
        let before = comments("\n Doc.\n ```\n assert_eq( f( 3 ) , 6 )\n ```");
        let after = reformat(before, 100);
        let texts: Vec<&str> = after.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["///", "/// Doc.", "/// ```", "/// assert_eq(f(3), 6)", "/// ```"]);
    }
}
