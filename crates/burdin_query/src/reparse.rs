//! Reparsing one function body when an edit stays inside it (`spec/query.md`, QL6).
//!
//! A body is a `{ ... }` block, and the parser reads one with no knowledge of what surrounds it, so when an edit
//! lies strictly inside one block and the new text of that block is still exactly one well-formed block, the rest
//! of the file parses to the same tree it did before: only its positions moved. The tree is then the old one with
//! the spans after the edit moved and that one body replaced. Anything else (an edit in a signature, between items,
//! one that unbalances the braces, a file that had syntax errors) is a full parse; so are the few bodies whose tree
//! the parser rewrites after the fact (`main` returning a `Result`, test bodies) or whose generated names depend on
//! the rest of the file (`select`).

use crate::db::{Parsed, diff_edit};
use burdin_diag::{FileId, Span};
use burdin_syntax::ast::{self, FnDecl, Item};
use burdin_syntax::spans::map_spans;

/// The syntax tree of `new_text`, built from `old` when the edit is local to one function body; `None` when the
/// file has to be parsed in full.
pub(crate) fn reparse_body(file: FileId, old: &Parsed, new_text: &str) -> Option<ast::File> {
    if !old.diags.is_empty() {
        return None;
    }
    let edit = diff_edit(file, &old.text, new_text);
    let delta = i64::from(edit.new_len) - i64::from(edit.hi - edit.lo);
    let body = old.ast.bodies.iter().copied().find(|b| b.lo < edit.lo && edit.hi < b.hi)?;
    let new_hi = usize::try_from(i64::from(body.hi) + delta).ok()?;
    let (old_body, new_body) =
        (old.text.get(body.lo as usize..body.hi as usize)?, new_text.get(body.lo as usize..new_hi)?);
    // A `select` names the variables it generates by counting the `select`s before it in the file.
    if old_body.contains("select") || new_body.contains("select") {
        return None;
    }
    let block = burdin_syntax::parse_block_range(file, new_text, body.lo as usize, new_hi)?;

    let mut ast = old.ast.clone();
    {
        let owners = fns_with_body(&mut ast, body);
        if owners.len() != 1 || owners[0].name.name == "main" {
            return None;
        }
    }
    map_spans(&mut ast, &mut |span: &mut Span| {
        if span.file != file || span.hi <= edit.lo {
            return;
        }
        if span.lo >= edit.hi {
            span.lo = (i64::from(span.lo) + delta) as u32;
            span.hi = (i64::from(span.hi) + delta) as u32;
        } else if span.lo <= body.lo && span.hi >= body.hi {
            // The body's ancestors (the function, its block) end where the new body ends.
            span.hi = (i64::from(span.hi) + delta) as u32;
        }
        // Anything else lies inside the old body, which is replaced below.
    });
    let new_span = Span::new(file, body.lo, new_hi as u32);
    let mut owners = fns_with_body(&mut ast, new_span);
    let [owner] = owners.as_mut_slice() else { return None };
    owner.body = block;
    Some(ast)
}

/// The function declarations whose body is the block at `body`, among the items a `fn` can appear in.
fn fns_with_body(file: &mut ast::File, body: Span) -> Vec<&mut FnDecl> {
    let mut found = Vec::new();
    for item in &mut file.items {
        match item {
            Item::Fn(d) => found.push(d),
            Item::Extension(e) => found.push(&mut e.func),
            Item::Impl(b) => found.extend(b.methods.iter_mut()),
            Item::Interface(i) => found.extend(i.methods.iter_mut().map(|m| &mut m.decl)),
            Item::Derive(d) => found.extend(d.methods.iter_mut()),
            Item::ExternFn(e) => found.push(&mut e.decl),
            _ => {}
        }
    }
    found.retain(|d| d.body.span == body);
    found
}
