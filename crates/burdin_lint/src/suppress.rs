//! Suppression comments (`spec/lint.md`, LN6-LN8):
//!
//! ```text
//! // lint: allow(unused_variables, dead_code)      on its own line: the next item, statement, or match arm
//! let x = 1  // lint: allow(unused_variables)       after code: that line
//! // lint: allow-file(non_snake_case)               anywhere: the whole file
//! ```
//!
//! The lexer drops comments, so they are read from the gaps between tokens: a gap holds only whitespace and
//! comments, and a `//` inside a string is part of a token and never seen.

use crate::registry::Lint;
use crate::walk::{Visit, for_each_fn, item_span, walk_block, walk_expr};
use burdin_diag::{Diagnostics, FileId, Span};
use burdin_syntax::ast::*;

/// One suppression: the lints it names (`None` means `all`) and the byte range it covers.
struct Rule {
    lints: Vec<Lint>,
    all: bool,
    lo: u32,
    hi: u32,
}

/// The suppressions of one file.
#[derive(Default)]
pub struct Suppressions {
    rules: Vec<Rule>,
}

impl Suppressions {
    /// Whether a diagnostic of `lint` whose primary span is `span` is suppressed.
    pub fn covers(&self, lint: Lint, span: Span) -> bool {
        self.rules.iter().any(|r| (r.all || r.lints.contains(&lint)) && span.lo >= r.lo && span.lo < r.hi.max(r.lo + 1))
    }
}

/// A suppression comment found in a gap.
struct Comment {
    /// Byte offset of the comment's first character.
    lo: usize,
    lints: Vec<String>,
    file_wide: bool,
}

/// Reads the suppressions of `file`. Names that are not lints become warnings in `problems`.
pub fn read(file: FileId, text: &str, ast: &File, problems: &mut Diagnostics) -> Suppressions {
    let mut scratch = Diagnostics::default();
    let lexed = burdin_syntax::lexer::lex(file, text, &mut scratch);
    let mut comments = Vec::new();
    let mut cursor = 0usize;
    for token in &lexed.tokens {
        let (lo, hi) = (token.span.lo as usize, token.span.hi as usize);
        if lo >= cursor && lo <= text.len() {
            scan_gap(text, cursor, lo, &mut comments);
        }
        cursor = cursor.max(hi);
    }
    scan_gap(text, cursor, text.len(), &mut comments);
    if comments.is_empty() {
        return Suppressions::default();
    }

    let nodes = node_spans(ast);
    let mut rules = Vec::new();
    for comment in comments {
        let mut lints = Vec::new();
        let mut all = false;
        for name in &comment.lints {
            if name == "all" {
                all = true;
            } else if let Some(lint) = Lint::from_name(name) {
                lints.push(lint);
            } else {
                let at = Span::new(file, comment.lo as u32, comment.lo as u32 + 1);
                problems.warning(
                    UNKNOWN_LINT_CODE,
                    at,
                    format!("unknown lint `{name}` in a suppression comment (`burdin lint --list` shows every lint)"),
                );
            }
        }
        let (lo, hi) = if comment.file_wide {
            (0, u32::MAX)
        } else if let Some(range) = trailing_range(text, comment.lo) {
            range
        } else {
            next_unit(text, comment.lo, &nodes)
        };
        rules.push(Rule { lints, all, lo, hi });
    }
    Suppressions { rules }
}

/// The code of the warning for an unknown lint name; it is not one of the configurable lints.
pub const UNKNOWN_LINT_CODE: burdin_diag::Code = burdin_diag::Code("L0000");

/// Finds the suppression comments in `text[from..to]`, which holds only whitespace and comments.
fn scan_gap(text: &str, from: usize, to: usize, out: &mut Vec<Comment>) {
    let bytes = text.as_bytes();
    let mut i = from;
    while i + 1 < to {
        if bytes[i] == b'/' && bytes[i + 1] == b'/' {
            let end = text[i..to].find('\n').map_or(to, |n| i + n);
            parse_comment(&text[i + 2..end], i, out);
            i = end;
        } else if bytes[i] == b'/' && bytes[i + 1] == b'*' {
            let end = text[i + 2..to].find("*/").map_or(to, |n| i + 2 + n);
            parse_comment(&text[i + 2..end], i, out);
            i = (end + 2).min(to);
        } else {
            i += 1;
        }
    }
}

fn parse_comment(body: &str, lo: usize, out: &mut Vec<Comment>) {
    // `///` doc comments carry one more slash.
    let body = body.trim_start_matches('/').trim();
    let Some(rest) = body.strip_prefix("lint:") else { return };
    let rest = rest.trim();
    let (file_wide, rest) = match rest.strip_prefix("allow-file") {
        Some(r) => (true, r),
        None => match rest.strip_prefix("allow") {
            Some(r) => (false, r),
            None => return,
        },
    };
    let rest = rest.trim_start();
    let Some(rest) = rest.strip_prefix('(') else { return };
    let Some(close) = rest.find(')') else { return };
    let lints: Vec<String> = rest[..close].split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if !lints.is_empty() {
        out.push(Comment { lo, lints, file_wide });
    }
}

/// A comment that follows code on its line covers that line.
fn trailing_range(text: &str, comment_lo: usize) -> Option<(u32, u32)> {
    let line_start = text[..comment_lo].rfind('\n').map_or(0, |n| n + 1);
    if text[line_start..comment_lo].trim().is_empty() {
        return None;
    }
    let line_end = text[comment_lo..].find('\n').map_or(text.len(), |n| comment_lo + n);
    Some((line_start as u32, line_end as u32))
}

/// A comment on its own line covers the next item, statement, or match arm: the largest node that starts on the first
/// line of code after the comment. With no such node it covers that line.
fn next_unit(text: &str, comment_lo: usize, nodes: &[(u32, u32)]) -> (u32, u32) {
    let mut line_start = text[comment_lo..].find('\n').map_or(text.len(), |n| comment_lo + n + 1);
    // Skip blank lines and further comment lines: stacked comments all describe the same unit.
    loop {
        if line_start >= text.len() {
            return (text.len() as u32, text.len() as u32);
        }
        let line_end = text[line_start..].find('\n').map_or(text.len(), |n| line_start + n);
        let line = text[line_start..line_end].trim_start();
        if line.is_empty() || line.starts_with("//") || line.starts_with("/*") {
            line_start = (line_end + 1).min(text.len());
            if line_end >= text.len() {
                return (text.len() as u32, text.len() as u32);
            }
            continue;
        }
        let line_end = line_end as u32;
        let line_start = line_start as u32;
        let best =
            nodes.iter().filter(|(lo, _)| *lo >= line_start && *lo < line_end).max_by_key(|(lo, hi)| (hi - lo, *hi));
        return match best {
            Some((lo, hi)) => (line_start.min(*lo), (*hi).max(line_end)),
            None => (line_start, line_end),
        };
    }
}

/// The spans of everything a comment can introduce: items, methods, fields, parameters, statements, and arms.
fn node_spans(ast: &File) -> Vec<(u32, u32)> {
    struct Nodes(Vec<(u32, u32)>);
    impl Visit for Nodes {
        fn stmt(&mut self, s: &Stmt) {
            match s {
                Stmt::Let { span, .. } => self.0.push((span.lo, span.hi)),
                Stmt::Expr(e) => self.0.push((e.span.lo, e.span.hi)),
            }
            crate::walk::walk_stmt(self, s);
        }
        fn expr(&mut self, e: &Expr) {
            if let ExprKind::Match(_, arms) = &e.kind {
                for arm in arms {
                    self.0.push((arm.span.lo, arm.span.hi));
                }
            }
            walk_expr(self, e);
        }
        fn block(&mut self, b: &Block) {
            walk_block(self, b);
        }
    }
    let mut nodes = Nodes(Vec::new());
    for item in &ast.items {
        let span = item_span(item);
        nodes.0.push((span.lo, span.hi));
        match item {
            Item::Struct(s) => {
                nodes.0.extend(s.fields.iter().map(|f| (f.name.span.lo, f.ty.span.hi)));
            }
            Item::Enum(e) => {
                for v in &e.variants {
                    nodes.0.push((v.name.span.lo, v.name.span.hi));
                    if let VariantKind::Struct(fields) = &v.kind {
                        nodes.0.extend(fields.iter().map(|f| (f.name.span.lo, f.ty.span.hi)));
                    }
                }
            }
            Item::Extern(b) => nodes.0.extend(b.decls.iter().map(|d| (d.span.lo, d.span.hi))),
            _ => {}
        }
    }
    for_each_fn(ast, &|_| false, &mut |_, f| {
        nodes.0.push((f.span.lo, f.span.hi));
        for p in &f.params {
            nodes.0.push((p.span.lo, p.span.hi));
        }
        nodes.block(&f.body);
    });
    for u in &ast.uses {
        nodes.0.push((u.span.lo, u.span.hi));
    }
    for c in ast.items.iter().filter_map(|i| if let Item::Const(c) = i { Some(c) } else { None }) {
        nodes.expr(&c.value);
    }
    nodes.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(src: &str) -> (Suppressions, Diagnostics) {
        let mut diags = Diagnostics::default();
        let mut parse_diags = Diagnostics::default();
        let ast = burdin_syntax::parse(FileId(0), src, &mut parse_diags);
        assert!(!parse_diags.has_errors(), "the test source parses");
        let s = read(FileId(0), src, &ast, &mut diags);
        (s, diags)
    }

    fn at(src: &str, needle: &str) -> Span {
        let lo = src.find(needle).expect("needle") as u32;
        Span::new(FileId(0), lo, lo + needle.len() as u32)
    }

    #[test]
    fn a_comment_on_its_own_line_covers_the_next_item() {
        let src = "// lint: allow(dead_code)\nfn first() {\n    let a = 1\n}\nfn second() {\n    let b = 2\n}\n";
        let (s, _) = rules(src);
        assert!(s.covers(Lint::DeadCode, at(src, "let a")));
        assert!(!s.covers(Lint::DeadCode, at(src, "let b")));
        assert!(!s.covers(Lint::UnusedMut, at(src, "let a")));
    }

    #[test]
    fn a_trailing_comment_covers_its_line() {
        let src = "fn f() {\n    let a = 1 // lint: allow(unused_variables)\n    let b = 2\n}\n";
        let (s, _) = rules(src);
        assert!(s.covers(Lint::UnusedVariables, at(src, "a = 1")));
        assert!(!s.covers(Lint::UnusedVariables, at(src, "b = 2")));
    }

    #[test]
    fn a_file_comment_covers_everything_and_all_names_every_lint() {
        let src = "// lint: allow-file(non_snake_case)\nfn f() {}\n// lint: allow(all)\nfn g() {}\n";
        let (s, _) = rules(src);
        assert!(s.covers(Lint::NonSnakeCase, at(src, "f()")));
        assert!(!s.covers(Lint::DeadCode, at(src, "f()")));
        assert!(s.covers(Lint::DeadCode, at(src, "g()")));
    }

    #[test]
    fn a_marker_inside_a_string_is_not_a_comment() {
        let src = "fn f() {\n    let s = \"// lint: allow(all)\"\n    let t = 1\n}\n";
        let (s, d) = rules(src);
        assert!(!s.covers(Lint::DeadCode, at(src, "let t")));
        assert!(d.is_empty());
    }

    #[test]
    fn an_unknown_name_is_reported() {
        let (_, d) = rules("// lint: allow(no_such_lint)\nfn f() {}\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d.iter().next().unwrap().code, UNKNOWN_LINT_CODE);
    }

    #[test]
    fn a_match_arm_comment_covers_that_arm() {
        let src = "fn f(x: i32) {\n    match x {\n        1 => {}\n        // lint: allow(else_on_local_enum)\n        else => {}\n    }\n}\n";
        let (s, _) = rules(src);
        assert!(s.covers(Lint::ElseOnLocalEnum, at(src, "else =>")));
        assert!(!s.covers(Lint::ElseOnLocalEnum, at(src, "1 =>")));
    }
}
