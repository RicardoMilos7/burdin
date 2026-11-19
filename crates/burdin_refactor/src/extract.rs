//! Extracting an expression into a local, and a stretch of statements into a function (RF9, RF10). The parameters of
//! an extracted function follow the memory rules from what the selection does to each local it uses: a local it moves
//! becomes `own`, one it writes or borrows exclusively becomes `mut`, and every other one is read, which is the
//! default convention (`spec/memory-rules.md`, MV1). A selection that cannot become a function without changing what
//! the program does is refused with the reason.

use crate::tree;
use crate::{Engine, Outcome, Refusal, apply, refuse};
use burdin_query::{DefinitionKind, OwnershipEventKind};
use burdin_syntax::ast::{self, Block, ExprKind, PatternKind, Stmt};
use std::collections::BTreeMap;

/// A run of whole statements in one block.
#[derive(Clone, Copy)]
struct Run<'a> {
    block: &'a Block,
    first: usize,
    last: usize,
}

impl<'a> Run<'a> {
    fn stmts(&self) -> &'a [Stmt] {
        &self.block.stmts[self.first..=self.last]
    }
}

/// The innermost block with a run of whole statements that covers exactly `lo..hi`.
fn run_at<'a>(body: &'a Block, lo: u32, hi: u32) -> Option<Run<'a>> {
    let mut best: Option<Run<'a>> = None;
    tree::each_block(body, &mut |block| {
        let spans: Vec<(u32, u32)> = block
            .stmts
            .iter()
            .map(|s| {
                let s = tree::stmt_span(s);
                (s.lo, s.hi)
            })
            .collect();
        // The statements the selection starts in and ends in, in the innermost block that has them.
        let Some(first) = spans.iter().rposition(|s| s.0 <= lo) else { return };
        let Some(last) = spans.iter().position(|s| hi <= s.1) else { return };
        if first <= last {
            let size = block.span.hi - block.span.lo;
            if best.is_none_or(|b| size < b.block.span.hi - b.block.span.lo) {
                best = Some(Run { block, first, last });
            }
        }
    });
    best
}

/// Extracts the expression at exactly `lo..hi` of `path` into a new local named `name`, inserted as a `let` before
/// the statement that holds it (RF9).
pub fn variable(engine: &mut Engine<'_>, path: &str, lo: u32, hi: u32, name: &str) -> Outcome {
    if !tree::is_identifier(name) {
        return refuse(format!("`{name}` is not a name a local can have"));
    }
    let parsed = engine.ast(path)?;
    let text = parsed.text.to_string();
    let Some(function) = engine.function_at(&parsed.ast, lo, hi) else {
        return refuse("the selection is not inside a function body");
    };
    let Some(expr) = tree::expr_covering(&function.body, lo, hi) else {
        return refuse("the selection is not inside one expression");
    };
    let (lo, hi) = (expr.span.lo, expr.span.hi);
    let Some(steps) = tree::path_to(&function.body, lo, hi) else {
        return refuse("the selection is not one whole expression");
    };
    // The expression must run exactly when the statement it moves out of runs.
    for step in &steps {
        if let tree::Step::Expr { role, .. } = step
            && !role.runs_once()
        {
            return refuse(format!("the selection cannot become a local before the statement: {}", role.why_not()));
        }
    }
    // A place that is written, or borrowed exclusively, is not a value to name.
    if let Some(parent) = tree::parent_of(&function.body, lo, hi) {
        match &parent.kind {
            ExprKind::Assign(lhs, _) if lhs.span.lo == lo && lhs.span.hi == hi => {
                return refuse("the selection is assigned to; extract the value on the right instead");
            }
            ExprKind::Unary(ast::UnOp::RefMut, x) if x.span.lo == lo && x.span.hi == hi => {
                return refuse("the selection is borrowed exclusively; extract what is borrowed instead");
            }
            _ => {}
        }
    }
    // The statement whose line the `let` goes before.
    let Some(tree::Step::Block { stmt_span, .. }) = steps.iter().rev().find(|s| matches!(s, tree::Step::Block { .. }))
    else {
        return refuse("the selection is not inside a statement");
    };
    let Some(indent) = tree::indent_of(&text, stmt_span.lo as usize) else {
        return refuse("the statement holding the selection does not start its line; put it on a line of its own");
    };
    let at = tree::line_start(&text, stmt_span.lo as usize);
    // The name must be free in the function.
    if engine.analysis.locals(path).iter().any(|l| l.name == name)
        || mentions(&text[function.body.span.lo as usize..function.body.span.hi as usize], name)
    {
        return refuse(format!("the function already uses the name `{name}`; pass another one"));
    }
    let value = &text[lo as usize..hi as usize];
    let insertion = format!("{indent}let {name} = {value}\n");
    let mut edited = BTreeMap::new();
    let new_text = apply(&text, vec![(at, at, insertion), (lo as usize, hi as usize, name.to_string())]);
    edited.insert(path.to_string(), new_text);
    engine.verify(edited, &std::collections::HashMap::new())
}

/// Whether `text` writes `name` as an identifier.
fn mentions(text: &str, name: &str) -> bool {
    tree::identifiers(text).iter().any(|(_, n)| n == name)
}

/// Extracts the statements at exactly `lo..hi` of `path` into a new function named `name`, declared after the
/// function they came from, and replaces them with a call (RF10).
pub fn function(engine: &mut Engine<'_>, path: &str, lo: u32, hi: u32, name: &str) -> Outcome {
    if !tree::is_identifier(name) {
        return refuse(format!("`{name}` is not a name a function can have"));
    }
    let parsed = engine.ast(path)?;
    let text = parsed.text.to_string();
    let Some(function) = engine.function_at(&parsed.ast, lo, hi) else {
        return refuse("the selection is not inside a function body");
    };
    let Some(run) = run_at(&function.body, lo, hi) else {
        return refuse(
            "the selection must be whole statements of one block, from the start of one to the end of another",
        );
    };
    let stmts = run.stmts();
    // The code must be able to leave the function.
    let mut selected = Block { stmts: stmts.to_vec(), span: function.body.span };
    selected.span = burdin_diag::Span::new(function.body.span.file, lo, hi);
    let flow = tree::control_flow(&selected);
    if let Some(reason) = flow.reason() {
        return refuse(format!("the selection cannot become a function: {reason}"));
    }
    // The statements' text, dedented from where they were.
    let start = tree::line_start(&text, lo as usize);
    let indent = &text[start..lo as usize];
    if !indent.chars().all(|c| c == ' ' || c == '\t') {
        return refuse("the selection must start at the beginning of a line");
    }
    let body_text = tree::reindent(&text[start..hi as usize], indent, "    ");
    // The locals the selection uses from outside it, in the order it first mentions them.
    let locals = engine.analysis.locals(path);
    let occurrences = engine.analysis.occurrences();
    let inside = |at: &burdin_query::Location| at.path == path && lo <= at.lo && at.hi <= hi;
    let mut params: Vec<(String, String, String)> = Vec::new();
    for occurrence in occurrences.iter().filter(|o| o.at.path == path && lo <= o.at.lo && o.at.hi <= hi) {
        if !matches!(occurrence.kind, DefinitionKind::Local | DefinitionKind::Parameter) {
            continue;
        }
        if occurrence.name.starts_with("__")
            || occurrence.definition.as_ref().is_some_and(inside)
            || params.iter().any(|(n, _, _)| n == &occurrence.name)
        {
            continue;
        }
        let Some(local) = locals.iter().find(|l| l.at == *occurrence.definition.as_ref().unwrap_or(&l.at)) else {
            return refuse(format!("`{}` is used in the selection but its type is unknown", occurrence.name));
        };
        if local.ty.is_empty() || local.ty.contains("__") {
            return refuse(format!("the type of `{}` is not one a parameter can take", occurrence.name));
        }
        params.push((
            occurrence.name.clone(),
            local.ty.clone(),
            convention(engine, path, lo, hi, &occurrence.name, local.mutable),
        ));
    }
    // A local the selection binds and the code after it uses cannot move into the function.
    for occurrence in &occurrences {
        if !inside(&occurrence.at) && occurrence.definition.as_ref().is_some_and(inside) {
            return refuse(format!(
                "the selection binds `{}`, which the code after it uses; a function cannot hand a local back",
                occurrence.name
            ));
        }
    }
    // The value the selection leaves, when its last statement is one.
    let last = &stmts[stmts.len() - 1];
    let tail = match last {
        Stmt::Expr(e) if !e.is_block_like() => {
            let ty = engine.analysis.expr_facts(path, e.span.lo, e.span.hi).map(|f| f.ty);
            match ty {
                Some(ty) if ty != "()" && ty != "!" => Some(ty),
                _ => None,
            }
        }
        _ => None,
    };
    // The call, and the function that is called.
    let mut call_args: Vec<String> = Vec::new();
    let mut param_text: Vec<String> = Vec::new();
    for (name, ty, convention) in &params {
        let marker = match convention.as_str() {
            "own" => "own ",
            "mut" => "mut ",
            _ => "",
        };
        param_text.push(format!("{marker}{name}: {ty}"));
        call_args.push(if convention == "mut" { format!("mut {name}") } else { name.clone() });
    }
    let call = format!("{name}({})", call_args.join(", "));
    let signature = match &tail {
        Some(ty) => format!("fn {name}({}): {ty} {{", param_text.join(", ")),
        None => format!("fn {name}({}) {{", param_text.join(", ")),
    };
    let new_function = format!("\n{signature}\n{body_text}\n}}\n");
    // The call replaces the statements, ending the block when it is the last one and the call's value is used.
    let call_line = match &tail {
        Some(_) => format!("{indent}{call}"),
        None => format!("{indent}{call}\n"),
    };
    let after_item = tree::item_span(
        parsed
            .ast
            .items
            .iter()
            .find(|i| tree::item_span(i).lo <= lo && hi <= tree::item_span(i).hi)
            .ok_or_else(|| Refusal("the selection is not inside a top-level item".to_string()))?,
    );
    let mut edited = BTreeMap::new();
    let new_text = apply(
        &text,
        vec![(start, hi as usize, call_line), (after_item.hi as usize, after_item.hi as usize, new_function)],
    );
    edited.insert(path.to_string(), new_text);
    engine.verify(edited, &std::collections::HashMap::new())
}

/// The convention a parameter of an extracted function takes, from what the selection does to the local.
fn convention(engine: &Engine<'_>, path: &str, lo: u32, hi: u32, name: &str, mutable: bool) -> String {
    let mut moved = false;
    let mut written = false;
    for function in engine.analysis.ownership_events(path) {
        for event in function.events {
            if event.local.as_deref() != Some(name) || !(lo <= event.at.lo && event.at.hi <= hi) {
                continue;
            }
            match event.kind {
                OwnershipEventKind::Move { .. } => moved = true,
                OwnershipEventKind::BorrowMut => written = true,
                _ => {}
            }
        }
    }
    if moved {
        "own".to_string()
    } else if written || mutable {
        "mut".to_string()
    } else {
        "read".to_string()
    }
}

/// The `let` pattern of a statement that binds exactly `name`, when it is a plain binding.
pub fn plain_binding(stmt: &Stmt) -> Option<(&str, &ast::Expr)> {
    let Stmt::Let { pattern, init, .. } = stmt else { return None };
    match &pattern.kind {
        PatternKind::Binding { name, .. } => Some((&name.name, init)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{after, db, span_of};

    const SIMPLE: &str =
        "fn main() {\n    let a = 2\n    let b = 3\n    let total = a + b * 4\n    println(\"{total}\")\n}\n";

    #[test]
    fn extracts_an_expression_into_a_local() {
        let mut db = db(&[("main.bd", SIMPLE)]);
        let (lo, hi) = span_of(SIMPLE, "b * 4", 0);
        let mut engine = Engine::new(&mut db).expect("engine");
        let text = after(variable(&mut engine, "main.bd", lo, hi, "scaled"), "main.bd", SIMPLE).expect("extracted");
        assert_eq!(
            text,
            "fn main() {\n    let a = 2\n    let b = 3\n    let scaled = b * 4\n    let total = a + scaled\n    println(\"{total}\")\n}\n"
        );
    }

    #[test]
    fn refuses_an_expression_inside_a_loop() {
        let source = "fn main() {\n    let mut n = 0\n    while n < 10 {\n        n = n + 1 + 1\n    }\n}\n";
        let mut db = db(&[("main.bd", source)]);
        let (lo, hi) = span_of(source, "n + 1", 0);
        let mut engine = Engine::new(&mut db).expect("engine");
        let message = after(variable(&mut engine, "main.bd", lo, hi, "twice"), "main.bd", source).expect_err("refused");
        assert!(message.contains("loop"), "{message}");
    }

    #[test]
    fn extracts_statements_into_a_function() {
        let source = "fn main() {\n    let a = 2\n    let b = 3\n    let total = a + b\n    println(\"{total}\")\n}\n";
        let mut db = db(&[("main.bd", source)]);
        let (lo, _) = span_of(source, "let total = a + b", 0);
        let (_, hi) = span_of(source, "println(\"{total}\")", 0);
        let mut engine = Engine::new(&mut db).expect("engine");
        let text = after(function(&mut engine, "main.bd", lo, hi, "part"), "main.bd", source).expect("extracted");
        assert!(text.contains("fn part(a: i32, b: i32)"), "{text}");
        assert!(text.contains("part(a, b)"), "{text}");
        assert!(text.contains("let b = 3"), "{text}");
    }
}
