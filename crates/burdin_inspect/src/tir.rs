//! `--emit=tir`: the typed tree of the program's own functions, as a debug dump (`spec/inspect.md`, INS7).
//!
//! The dump is the derived `Debug` text of each function with its spans shortened to `@<file>:<lo>..<hi>`, one
//! function after another in program order. It is meant for reading what sema and the interpreter pass produced, and its
//! layout is not a stable format.

use crate::reach;
use burdin_diag::{FileId, SourceMap};
use burdin_tir::Program;

/// The dump of every function code generation emits that is declared outside the standard library, and of every
/// closure body inside one of them.
pub fn dump(program: &Program, sources: &SourceMap, is_std: &dyn Fn(FileId) -> bool) -> String {
    let reached = reach::functions(program);
    let mut out = String::new();
    out.push_str("// burdin typed tree: a debug dump, not a stable format\n");
    out.push_str("// spans print as @<file id>:<lo>..<hi>, byte offsets into the file\n");
    for (id, file) in sources.iter() {
        if !is_std(id) {
            out.push_str(&format!("// file {}: {}\n", id.0, file.path));
        }
    }
    let mut closures: Vec<usize> = Vec::new();
    for (i, f) in program.functions.iter().enumerate() {
        if !reached[i] || f.symbolic || is_std(f.span.file) {
            continue;
        }
        let (line, column) = sources.get(f.span.file).line_col(f.span.lo);
        out.push_str(&format!("\nfn {}  {}:{line}:{column}\n", f.name, sources.get(f.span.file).path));
        out.push_str(&format!("  ret: {}\n  params: {:?}\n", f.ret, f.params.iter().map(|p| p.0).collect::<Vec<_>>()));
        for (n, local) in f.locals.iter().enumerate() {
            let mutable = if local.mutable { " mut" } else { "" };
            out.push_str(&format!("  local #{n} {}: {}{mutable} ({:?})\n", local.name, local.ty, local.kind));
        }
        out.push_str("  body: ");
        out.push_str(&indent(&compact(&shorten_spans(&format!("{:#?}", f.body))), "  "));
        out.push('\n');
        collect_closures(&f.body, &mut closures);
    }
    for index in closures {
        let closure = &program.closures[index];
        if is_std(closure.span.file) {
            continue;
        }
        out.push_str(&format!("\nclosure {} ({})\n", closure.name, closure.kind.name()));
        for capture in &closure.captures {
            out.push_str(&format!("  capture #{} {:?}: {}\n", capture.local.0, capture.mode, capture.ty));
        }
        out.push_str("  body: ");
        out.push_str(&indent(&compact(&shorten_spans(&format!("{:#?}", closure.func.body))), "  "));
        out.push('\n');
    }
    out
}

/// The closures a block creates, in order, with those created inside closure bodies left to the caller's loop.
fn collect_closures(block: &burdin_tir::Block, out: &mut Vec<usize>) {
    use burdin_tir::visit::{Visit, walk_expr};
    struct Closures<'a>(&'a mut Vec<usize>);
    impl Visit for Closures<'_> {
        fn expr(&mut self, e: &burdin_tir::Expr) {
            if let burdin_tir::ExprKind::Closure { closure, .. } = &e.kind
                && !self.0.contains(closure)
            {
                self.0.push(*closure);
            }
            walk_expr(self, e);
        }
    }
    Closures(out).block(block);
}

/// Indents every line after the first.
fn indent(text: &str, by: &str) -> String {
    text.lines()
        .enumerate()
        .map(|(i, l)| if i == 0 { l.to_string() } else { format!("{by}{l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replaces each `Span { file: FileId(N), lo: A, hi: B }` of pretty-printed `Debug` output with `@N:A..B`.
pub fn shorten_spans(text: &str) -> String {
    let mut out = String::with_capacity(text.len() / 2);
    let mut rest = text;
    while let Some(at) = rest.find("Span {") {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        // The span's braces do not nest, so the first `}` ends it.
        let Some(end) = after.find('}') else { break };
        let body = &after[..end];
        let number = |key: &str| -> Option<&str> {
            let from = body.find(key)? + key.len();
            let digits = body[from..].trim_start();
            let len = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
            (len > 0).then(|| &digits[..len])
        };
        match (number("FileId("), number("lo: "), number("hi: ")) {
            (Some(file), Some(lo), Some(hi)) => out.push_str(&format!("@{file}:{lo}..{hi}")),
            _ => out.push_str(&after[..=end]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The widest line `compact` produces by folding a group onto one line.
const FOLD_WIDTH: usize = 110;

/// Folds each bracketed group of pretty-printed `Debug` output onto one line when everything inside it already fits
/// on one line and the result is at most [`FOLD_WIDTH`] columns wide, so a `LocalId` printed over three lines becomes
/// `LocalId(0)`.
pub fn compact(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    fold(&lines).join("\n")
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn closer_for(line: &str) -> Option<char> {
    match line.trim_end().chars().last()? {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

fn fold(lines: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let Some(closer) = closer_for(line) else {
            out.push(line.to_string());
            i += 1;
            continue;
        };
        // The group closes at the first later line with this line's indentation that starts with the closer.
        let indent = indent_of(line);
        let close =
            (i + 1..lines.len()).find(|&j| indent_of(lines[j]) == indent && lines[j].trim_start().starts_with(closer));
        let Some(close) = close else {
            out.push(line.to_string());
            i += 1;
            continue;
        };
        let inner = fold(&lines[i + 1..close]);
        let tail = lines[close].trim_start();
        // Foldable when every inner element is a single line, that is, each one stayed one line after folding.
        let one_line_each = inner.iter().all(|l| indent_of(l) == indent + 4);
        if one_line_each {
            let items: Vec<&str> = inner.iter().map(|l| l.trim().trim_end_matches(',')).collect();
            let open = line.trim_end();
            let folded = match closer {
                ')' => format!("{open}{}{tail}", items.join(", ")),
                _ if items.is_empty() => format!("{open}{tail}"),
                _ => format!("{open} {} {tail}", items.join(", ")),
            };
            if folded.len() <= FOLD_WIDTH {
                out.push(folded);
                i = close + 1;
                continue;
            }
        }
        out.push(line.to_string());
        out.extend(inner);
        out.push(lines[close].to_string());
        i = close + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_groups_fold_onto_one_line() {
        let pretty = "Binding(\n    LocalId(\n        0,\n    ),\n    Move,\n)";
        assert_eq!(compact(pretty), "Binding(LocalId(0), Move)");
        let record = "Expr {\n    kind: Unit,\n    ty: Unit,\n}";
        assert_eq!(compact(record), "Expr { kind: Unit, ty: Unit }");
        assert_eq!(compact("Block {\n    stmts: [],\n}"), "Block { stmts: [] }");
    }

    #[test]
    fn a_group_with_a_long_line_stays_open() {
        let long = "x".repeat(120);
        let pretty = format!("Str(\n    \"{long}\",\n)");
        assert_eq!(compact(&pretty), pretty);
    }

    #[test]
    fn spans_print_on_one_line() {
        let debug = "Expr {\n    span: Span {\n        file: FileId(\n            2,\n        ),\n        lo: 10,\n        hi: 24,\n    },\n}";
        assert_eq!(shorten_spans(debug), "Expr {\n    span: @2:10..24,\n}");
    }

    #[test]
    fn text_without_spans_is_unchanged() {
        assert_eq!(shorten_spans("Int(3)"), "Int(3)");
    }
}
