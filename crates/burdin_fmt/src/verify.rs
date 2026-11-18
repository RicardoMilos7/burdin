//! The safety net under the formatter: the formatted program must parse to the same syntax tree as the original, and
//! every comment must still be there. A formatter that fails this check has a bug, and its output is never written.

use crate::trivia;
use burdin_diag::{Diagnostics, FileId};
use burdin_syntax::ast::UseDecl;

/// Compares the program before and after formatting. `Err` says what changed.
pub fn same_program(before: &str, after: &str) -> Result<(), String> {
    let a = tree_signature(before)?;
    let b = tree_signature(after)?;
    if a != b {
        let at = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
        let context = |s: &str| s.chars().skip(at.saturating_sub(40)).take(120).collect::<String>();
        return Err(format!(
            "the formatted program parses to a different syntax tree\n  before: …{}\n  after:  …{}",
            context(&a),
            context(&b)
        ));
    }
    let (ca, cb) = (comment_signature(before)?, comment_signature(after)?);
    if ca != cb {
        let missing = ca.iter().find(|c| !cb.contains(c));
        let extra = cb.iter().find(|c| !ca.contains(c));
        return Err(format!("the formatted program changed its comments (missing: {missing:?}, new: {extra:?})"));
    }
    Ok(())
}

/// The syntax tree as text with the source positions removed. `use` declarations are compared by what they import,
/// because the formatter merges and sorts them.
fn tree_signature(src: &str) -> Result<String, String> {
    let mut diags = Diagnostics::default();
    let file = burdin_syntax::parse(FileId(0), src, &mut diags);
    if diags.has_errors() {
        return Err("the program does not parse".to_string());
    }
    let mut out = scrub_spans(&format!("{:?}", file.items));
    out.push('\n');
    out.push_str(&scrub_spans(&format!("{:?}", file.tests)));
    out.push('\n');
    let mut imports: Vec<String> = file.uses.iter().flat_map(imports_of).collect();
    imports.sort();
    imports.dedup();
    out.push_str(&imports.join("\n"));
    Ok(out)
}

/// What a `use` declaration imports, one line per name.
fn imports_of(u: &UseDecl) -> Vec<String> {
    let path: Vec<&str> = u.path.iter().map(|i| i.name.as_str()).collect();
    let path = path.join(".");
    let visibility = if u.is_pub { "pub" } else { "priv" };
    if u.items.is_empty() {
        let alias = u.alias.as_ref().map_or("", |a| a.name.as_str());
        return vec![format!("{visibility} {path} as {alias}")];
    }
    u.items.iter().map(|item| format!("{visibility} {path}.{{{}}}", item.name)).collect()
}

/// Replaces every `Span { file: FileId(n), lo: n, hi: n }` of a debug dump with `_`.
fn scrub_spans(dump: &str) -> String {
    const START: &str = "Span { file: FileId(";
    let mut out = String::with_capacity(dump.len());
    let mut rest = dump;
    while let Some(at) = rest.find(START) {
        out.push_str(&rest[..at]);
        match rest[at..].find('}') {
            Some(end) => {
                out.push('_');
                rest = &rest[at + end + 1..];
            }
            None => {
                rest = &rest[at..];
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Every comment of the source, sorted, except the lines of code inside Burdin fenced blocks of doc comments (the
/// formatter reformats those; their code is checked as part of the tree) which are replaced by a count of lines.
fn comment_signature(src: &str) -> Result<Vec<String>, String> {
    let lexed = trivia::lex(src)?;
    let mut out = Vec::new();
    for tok in &lexed.toks {
        for list in [&tok.lead, &tok.trail] {
            let skip = crate::docs::code_lines(list);
            for (index, comment) in list.iter().enumerate() {
                if !skip.contains(&index) {
                    out.push(comment.text.clone());
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_changes_are_the_same_program() {
        assert!(same_program("fn main() { f(1,2) }\n", "fn main() {\n    f(1, 2)\n}\n").is_ok());
    }

    #[test]
    fn a_changed_token_is_found() {
        let err = same_program("fn main() { f(1) }\n", "fn main() { f(2) }\n").unwrap_err();
        assert!(err.contains("different syntax tree"), "{err}");
    }

    #[test]
    fn a_lost_comment_is_found() {
        let err = same_program("// a\nfn main() {}\n", "fn main() {}\n").unwrap_err();
        assert!(err.contains("comments"), "{err}");
    }

    #[test]
    fn use_declarations_are_compared_by_what_they_import() {
        assert!(same_program("use a.{b}\nuse a.{c}\n", "use a.{b, c}\n").is_ok());
        assert!(same_program("use a.b\n", "use a.{b}\n").is_err());
    }
}
