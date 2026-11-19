//! Inlining a local and inlining a function (RF11, RF12). Both are only done when the result means the same thing:
//! an inlined local has one use and an initializer that reads nothing a write could change, and an inlined function
//! has one call and takes arguments that are names or literals, so moving them into the body cannot reorder effects.

use crate::tree::{self, Role};
use crate::{Engine, Outcome, apply, refuse};
use burdin_query::DefinitionKind;
use burdin_syntax::ast::{self, Block, Item, Stmt};
use std::collections::BTreeMap;

/// Inlines the local whose declaration or use is at `lo..hi` of `path`, replacing its one use with the value it was
/// given and removing the `let` (RF11).
pub fn variable(engine: &mut Engine<'_>, path: &str, lo: u32, hi: u32) -> Outcome {
    let parsed = engine.ast(path)?;
    let text = parsed.text.to_string();
    let Some(function) = engine.function_at(&parsed.ast, lo, hi) else {
        return refuse("the selection is not inside a function body");
    };
    let locals = engine.analysis.locals(path);
    // The position names the local three ways: it is inside its binding, inside the `let` that declares it, or on a
    // use of it.
    let binding = |l: &burdin_query::LocalInfo| l.at.lo <= lo && hi <= l.at.hi;
    let mut local = locals.iter().find(|l| binding(l)).cloned();
    if local.is_none() {
        let mut declared: Option<(u32, u32)> = None;
        tree::each_block(&function.body, &mut |block| {
            for stmt in &block.stmts {
                let span = tree::stmt_span(stmt);
                if span.lo <= lo
                    && hi <= span.hi
                    && let Stmt::Let { pattern, .. } = stmt
                    && let ast::PatternKind::Binding { name, .. } = &pattern.kind
                {
                    declared = Some((name.span.lo, name.span.hi));
                }
            }
        });
        if let Some((binding_lo, binding_hi)) = declared {
            local = locals.iter().find(|l| l.at.lo == binding_lo && l.at.hi == binding_hi).cloned();
        }
    }
    if local.is_none() {
        local = engine
            .analysis
            .resolve_at(path, lo)
            .and_then(|r| r.definition)
            .and_then(|definition| locals.iter().find(|l| l.at == definition).cloned());
    }
    let Some(local) = local else { return refuse("no local is declared or used at the selection") };
    if local.is_parameter {
        return refuse(format!("`{}` is a parameter; a parameter cannot be inlined", local.name));
    }
    if local.mutable {
        return refuse(format!("`{}` is mutable; an inlined mutable local would change what is written", local.name));
    }
    let binding_at = local.at.clone();
    // The `let` that declares it.
    let mut declaration: Option<(&Stmt, &Block)> = None;
    tree::each_block(&function.body, &mut |block| {
        for stmt in &block.stmts {
            if let Stmt::Let { pattern, .. } = stmt
                && pattern.span.lo == binding_at.lo
                && pattern.span.hi == binding_at.hi
            {
                declaration = Some((stmt, block));
            }
        }
    });
    let Some((stmt, _)) = declaration else {
        return refuse(format!("`{}` is not declared by a `let` in this function", local.name));
    };
    let Some((_, init)) = crate::extract::plain_binding(stmt) else {
        return refuse(format!("`{}` is bound inside a pattern; only a plain `let` can be inlined", local.name));
    };
    let init_text = text[init.span.lo as usize..init.span.hi as usize].to_string();
    if !tree::is_pure(init) {
        return refuse(format!(
            "the value given to `{}` is a computation, which must keep running exactly where it was written",
            local.name
        ));
    }
    for (_, name) in tree::identifiers(&init_text) {
        if locals.iter().any(|l| l.name == name && l.mutable) {
            return refuse(format!(
                "the value given to `{}` reads `{name}`, which is mutable; moving the read could see another value",
                local.name
            ));
        }
    }
    // Exactly one use, after the declaration.
    let definition = binding_at.clone();
    let uses: Vec<_> = engine
        .analysis
        .occurrences()
        .into_iter()
        .filter(|o| {
            o.at.path == path
                && o.kind != DefinitionKind::Function
                && o.definition.as_ref() == Some(&definition)
                && !(definition.lo <= o.at.lo && o.at.hi <= definition.hi)
        })
        .collect();
    match uses.len() {
        0 => return refuse(format!("`{}` is never used, so there is nothing to inline it into", local.name)),
        1 => {}
        n => {
            return refuse(format!(
                "`{}` is used {n} times; inlining it would run its value more than once, so only one use is inlined",
                local.name
            ));
        }
    }
    let use_at = uses[0].at.clone();
    // The use must run exactly when the declaration runs.
    let Some(steps) = tree::path_to(&function.body, use_at.lo, use_at.hi) else {
        return refuse("the use is not one whole expression");
    };
    for step in &steps {
        if let tree::Step::Expr { role, .. } = step
            && !role.runs_once()
        {
            return refuse(format!("the use cannot take the value in its place: {}", role.why_not()));
        }
    }
    let role = role_of(&function.body, use_at.lo, use_at.hi);
    let replacement = tree::wrap(&init_text, tree::is_atomic(init), role);
    // The `let` has to be alone on its lines, so removing it removes exactly it.
    let stmt_span = tree::stmt_span(stmt);
    let start = stmt_span.lo as usize;
    let Some(indent) = tree::indent_of(&text, start) else {
        return refuse("the `let` does not start its line; put it on a line of its own first");
    };
    if !tree::ends_line(&text, stmt_span.hi as usize) {
        return refuse("something else follows the `let` on its line; put it on a line of its own first");
    }
    let removal_start = tree::line_start(&text, start);
    let removal_end = tree::line_end(&text, stmt_span.hi as usize);
    let _ = indent;
    let mut edited = BTreeMap::new();
    let new_text = apply(
        &text,
        vec![(removal_start, removal_end, String::new()), (use_at.lo as usize, use_at.hi as usize, replacement)],
    );
    edited.insert(path.to_string(), new_text);
    engine.verify(edited, &std::collections::HashMap::new())
}

/// The role the expression `lo..hi` takes in its parent.
fn role_of(body: &Block, lo: u32, hi: u32) -> Role {
    let Some(parent) = tree::parent_of(body, lo, hi) else { return Role::Operand };
    for child in tree::children(parent) {
        if let tree::Child::Expr(role, e) = child
            && e.span.lo == lo
            && e.span.hi == hi
        {
            return role;
        }
    }
    Role::Operand
}

/// Inlines the function called at `lo..hi` or declared at it, replacing its one call with its body and removing the
/// declaration (RF12).
pub fn function(engine: &mut Engine<'_>, path: &str, lo: u32, hi: u32) -> Outcome {
    let parsed = engine.ast(path)?;
    let text = parsed.text.to_string();
    // The declaration, from an offset at the declaration or at the call.
    let mut declaration: Option<(usize, ast::FnDecl)> = None;
    for (index, item) in parsed.ast.items.iter().enumerate() {
        if let Item::Fn(decl) = item
            && decl.span.lo <= lo
            && hi <= decl.span.hi
        {
            declaration = Some((index, decl.clone()));
        }
    }
    let (item_index, decl) = match declaration {
        Some(found) => found,
        None => {
            let message = "the selection is not inside a function declaration; select the function to inline";
            return refuse(message);
        }
    };
    let name = decl.name.name.clone();
    if !decl.generics.is_empty() {
        return refuse(format!("`{name}` is generic; only a function with one instance can be inlined"));
    }
    if decl.params.iter().any(|p| matches!(p.kind, ast::ParamKind::SelfParam)) {
        return refuse(format!("`{name}` is a method; only a free function can be inlined"));
    }
    if decl.is_unsafe || decl.is_blocking {
        return refuse(format!("`{name}` is `unsafe` or `blocking`; its call is part of what it means"));
    }
    let flow = tree::control_flow(&decl.body);
    if flow.returns {
        return refuse(format!("`{name}` returns early; inlining it would move that `return` into the caller"));
    }
    if flow.self_value || flow.comptime {
        return refuse(format!("`{name}` uses `self` or `comptime`, which cannot move into the caller"));
    }
    // One call, not inside the function itself.
    let calls: Vec<_> = engine
        .analysis
        .calls()
        .into_iter()
        .filter(|c| {
            c.callee_location.as_ref().is_some_and(|l| l.path == path && l.lo == decl.span.lo && l.hi == decl.span.hi)
        })
        .collect();
    let call = match calls.len() {
        0 => return refuse(format!("`{name}` is never called, so there is nothing to inline it into")),
        1 => calls.into_iter().next().expect("one call"),
        n => return refuse(format!("`{name}` is called {n} times; only a function with one call site can be inlined")),
    };
    if decl.span.lo <= call.at.lo && call.at.hi <= decl.span.hi {
        return refuse(format!("`{name}` calls itself; inlining it would not end"));
    }
    // Each parameter keeps the argument it was given; a parameter the call left out keeps its default.
    let mut args: Vec<(String, String)> = Vec::new();
    for param in &decl.params {
        let ast::ParamKind::Named { name: param_name, .. } = &param.kind else { continue };
        let given = call
            .args
            .iter()
            .find(|(p, _)| p.name == param_name.name)
            .map(|(_, at)| text[at.lo as usize..at.hi as usize].to_string());
        let value = match (given, &param.default) {
            (Some(value), _) => value,
            (None, Some(default)) => text[default.span.lo as usize..default.span.hi as usize].to_string(),
            (None, None) => {
                return refuse(format!("the call does not pass `{}`, which has no default", param_name.name));
            }
        };
        if !is_simple(&value) {
            return refuse(format!(
                "the argument for `{}` is `{value}`, which is not a name or a literal; inlining it could reorder \
                 what the body does",
                param_name.name
            ));
        }
        args.push((param_name.name.clone(), value));
    }
    // Names the body declares itself, which a substitution must not take for a parameter use.
    let mutable_locals: Vec<String> = engine
        .analysis
        .locals(path)
        .into_iter()
        .filter(|l| !l.is_parameter && decl.body.span.lo <= l.at.lo && l.at.hi <= decl.body.span.hi)
        .map(|l| l.name)
        .collect();
    for (_, value) in &args {
        for (_, name) in tree::identifiers(value) {
            if mutable_locals.iter().any(|l| l == &name) {
                return refuse(format!(
                    "the argument `{value}` names `{name}`, which the body of `{name}` declares; the substitution \
                     would capture it"
                ));
            }
        }
    }
    let body_text = text[decl.body.span.lo as usize..decl.body.span.hi as usize].to_string();
    let inlined = substitute_names(&body_text, &args);
    // The call becomes the block. A statement keeps the block; a value takes it as one expression.
    let mut edited = BTreeMap::new();
    let item = tree::item_span(&parsed.ast.items[item_index]);
    let (remove_lo, remove_hi) = with_doc_comment(&text, item.lo as usize, item.hi as usize);
    let new_text =
        apply(&text, vec![(call.at.lo as usize, call.at.hi as usize, inlined), (remove_lo, remove_hi, String::new())]);
    edited.insert(path.to_string(), new_text);
    engine.verify(edited, &std::collections::HashMap::new())
}

/// Whether a stretch of text is a name, a literal, or a field of one: what can move without reordering effects.
fn is_simple(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    let mut diags = burdin_diag::Diagnostics::default();
    let file = burdin_syntax::parse(burdin_diag::FileId(0), &format!("fn f() {{ {text} }}\n"), &mut diags);
    if diags.has_errors() {
        return false;
    }
    let Some(Item::Fn(f)) = file.items.first() else { return false };
    match f.body.stmts.as_slice() {
        [Stmt::Expr(e)] => tree::is_pure(e),
        _ => false,
    }
}

/// `text` with each identifier that names a parameter replaced by what the call passed. A field name after a `.`
/// is not a parameter use, and neither is a named-argument label.
fn substitute_names(text: &str, args: &[(String, String)]) -> String {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for (at, name) in tree::identifiers(text) {
        let Some((_, value)) = args.iter().find(|(p, _)| p == &name) else { continue };
        let before = text[..at].trim_end().chars().next_back();
        if before == Some('.') {
            continue;
        }
        let after = text[at + name.len()..].trim_start();
        if after.starts_with(':') && !after.starts_with("::") {
            continue;
        }
        edits.push((at, at + name.len(), value.clone()));
    }
    crate::apply(text, edits)
}

/// The span of an item together with the doc comment lines directly above it, and the line break after it, so
/// removing the item leaves no empty line where it was.
fn with_doc_comment(text: &str, lo: usize, hi: usize) -> (usize, usize) {
    let mut start = tree::line_start(text, lo);
    while start > 0 {
        let previous_end = start - 1;
        let previous_start = tree::line_start(text, previous_end);
        let line = text[previous_start..previous_end].trim();
        if !line.starts_with("///") {
            break;
        }
        start = previous_start;
    }
    (start, tree::line_end(text, hi))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{after, db, span_of};

    #[test]
    fn inlines_a_local_with_one_use() {
        let source = "fn main() {\n    let a = 2\n    let total = a * 3\n    println(\"{total}\")\n}\n";
        let mut db = db(&[("main.bd", source)]);
        let (lo, hi) = span_of(source, "let a = 2", 0);
        let mut engine = Engine::new(&mut db).expect("engine");
        let text = after(variable(&mut engine, "main.bd", lo, hi), "main.bd", source).expect("inlined");
        assert_eq!(text, "fn main() {\n    let total = 2 * 3\n    println(\"{total}\")\n}\n");
    }

    #[test]
    fn refuses_a_local_with_two_uses() {
        let source = "fn main() {\n    let a = 2\n    let total = a * a\n    println(\"{total}\")\n}\n";
        let mut db = db(&[("main.bd", source)]);
        let (lo, hi) = span_of(source, "let a = 2", 0);
        let mut engine = Engine::new(&mut db).expect("engine");
        let message = after(variable(&mut engine, "main.bd", lo, hi), "main.bd", source).expect_err("refused");
        assert!(message.contains("2 times"), "{message}");
    }

    #[test]
    fn inlines_a_function_with_one_call() {
        let source =
            "fn twice(n: i32): i32 {\n    n * 2\n}\n\nfn main() {\n    let x = twice(3)\n    println(\"{x}\")\n}\n";
        let mut db = db(&[("main.bd", source)]);
        let (lo, hi) = span_of(source, "fn twice", 0);
        let mut engine = Engine::new(&mut db).expect("engine");
        let text = after(function(&mut engine, "main.bd", lo, hi), "main.bd", source).expect("inlined");
        assert!(text.contains("let x = {"), "{text}");
        assert!(!text.contains("fn twice"), "{text}");
    }
}
