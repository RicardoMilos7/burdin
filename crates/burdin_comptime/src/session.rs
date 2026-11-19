//! The evaluation of a REPL session (`spec/repl.md`, RP3-RP6): one function's body, statements appended over
//! time, with the bindings' values carried between inputs and printing allowed as the one side effect.
//!
//! The engine is the comptime interpreter, so a session's output is byte-for-byte what the compiled program
//! prints: the `Formatter` machinery is standard-library code the interpreter runs like any other function.
//! Everything the interpreter cannot do keeps the compile-time fault (`Fault::Impure`), which the REPL reports
//! with a pointer to `:compile`.
//!
//! This module is the typed tree's public boundary for the REPL crate: `Program`, `FuncId`, and `Value` come in
//! and go out, and nothing here is used by ordinary compilation.

use crate::interp::{ConstState, Fault as Inner, Interp, Output, Unwind};
use crate::value::Value;
use burdin_diag::Span;
use burdin_tir::{FuncId, LocalId, LocalKind, Pat, Program, Stmt, Ty};

/// The values of a session's bindings, by their local ids (`spec/repl.md`, RP4). A binding moved out of by a
/// later input carries [`Value::Moved`], so moves persist across lines exactly as in one function body.
#[derive(Clone, Debug, Default)]
pub struct Bindings(pub(crate) Vec<(LocalId, Value)>);

impl Bindings {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Why an input's evaluation stopped. The code is the one ordinary compile-time evaluation reports: a panic
/// (E0222), an impure operation (E0224), or the step budget (E0225); the REPL adds the two shapes compilation
/// never produces.
#[derive(Clone, Debug)]
pub enum Fault {
    /// The input panicked: the message.
    Panic(String, Span),
    /// An operation the interpreter cannot do: the description and where it was written.
    Impure(String, Span),
    /// The step budget ran out: the message.
    Budget(String, Span),
    /// A `return` (or a loop jump) left the session's function.
    ControlFlow(String, Span),
    /// Something the checks should have ruled out.
    Internal(String, Span),
}

impl Fault {
    /// The message the REPL prints after the code's phrase, without the code prefix.
    pub fn detail(&self) -> String {
        match self {
            Fault::Panic(msg, _) | Fault::Budget(msg, _) | Fault::Internal(msg, _) => msg.clone(),
            Fault::Impure(msg, _) => {
                format!("{msg}; the interpreter cannot do this, but `:compile` builds the session and runs it natively")
            }
            Fault::ControlFlow(msg, _) => msg.clone(),
        }
    }

    /// The error code the REPL reports this fault with, from the registry in `tests/README.md`.
    pub fn code(&self) -> burdin_diag::Code {
        match self {
            Fault::Panic(..) => burdin_diag::Code::COMPTIME_PANIC,
            Fault::Impure(..) => burdin_diag::Code::COMPTIME_IMPURE,
            Fault::Budget(..) => burdin_diag::Code::COMPTIME_BUDGET,
            Fault::ControlFlow(..) => burdin_diag::Code::COMPTIME_IMPURE,
            Fault::Internal(..) => burdin_diag::Code::COMPTIME_IMPURE,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Fault::Panic(_, at)
            | Fault::Impure(_, at)
            | Fault::Budget(_, at)
            | Fault::ControlFlow(_, at)
            | Fault::Internal(_, at) => *at,
        }
    }
}

impl From<Inner> for Fault {
    fn from(f: Inner) -> Fault {
        match f {
            Inner::Panic(msg, at) => Fault::Panic(msg, at),
            Inner::Impure(msg, at) => Fault::Impure(msg, at),
            Inner::Budget(msg, at) => Fault::Budget(msg, at),
            Inner::Internal(msg, at) => Fault::Internal(msg, at),
            // A constant this evaluation read already failed, a failed `comptime assert`, or a cycle: the
            // session's constants are re-evaluated from their materialized literals and its asserts were checked
            // by the analysis, so this cannot happen, but a silent success would be worse than reporting it.
            other => Fault::Internal(format!("{other:?}"), Span::new(burdin_diag::FileId(0), 0, 0)),
        }
    }
}

/// The local ids a body's top-level `let` patterns bind, in declaration order: the bindings that persist
/// between the inputs of a session. Bindings of sub-scopes (a `match` arm, a `for` body) are not top-level
/// statements and never persist.
fn binding_ids(body: &burdin_tir::Block) -> Vec<LocalId> {
    let mut ids = Vec::new();
    for stmt in &body.stmts {
        if let Stmt::Let(pat, ..) = stmt {
            pattern_ids(pat, &mut ids);
        }
    }
    ids
}

/// The ids `pat` binds, in pattern order.
fn pattern_ids(pat: &Pat, out: &mut Vec<LocalId>) {
    match pat {
        Pat::Binding(id, _) => out.push(*id),
        Pat::At(id, _, inner) => {
            out.push(*id);
            pattern_ids(inner, out);
        }
        Pat::Some(inner) | Pat::Ok(inner) | Pat::Err(inner) => pattern_ids(inner, out),
        Pat::Product(subs) | Pat::Variant(_, subs) => {
            for sub in subs {
                pattern_ids(sub, out);
            }
        }
        Pat::Or(alts) => {
            for alt in alts {
                pattern_ids(alt, out);
            }
        }
        Pat::Wild | Pat::Bool(_) | Pat::Int(_) | Pat::Char(_) | Pat::Str(_) | Pat::Range(..) | Pat::None => {}
    }
}

/// Evaluates `body.stmts[from..]`, the statements one input appended to the session's function `main`, against
/// `bindings`, and updates `bindings` with every value the body's top-level lets hold afterwards. Printing is
/// real output (RP3); the bindings of earlier inputs are restored first, so a place's identity (a
/// [`Value::Ref`] into the body's locals) stays valid from one input to the next.
pub fn eval_stmts(
    program: &Program,
    main: FuncId,
    from: usize,
    bindings: &mut Bindings,
    opts: crate::Options,
) -> Result<(), Fault> {
    let func = &program.functions[main.0];
    let body = &func.body;
    let items = body.stmts.len() + usize::from(body.tail.is_some());
    debug_assert!(from <= items, "the session asked to evaluate past the end of the body");
    let ids = binding_ids(body);
    let span = body.stmts.get(from).map_or_else(|| body.tail.as_ref().map_or(func.span, |t| t.span), stmt_span);
    let mut consts: Vec<ConstState> = program.consts.iter().map(|_| ConstState::Pending).collect();
    let slots = {
        let mut it = Interp::with_output(program, &mut consts, opts.steps, opts.release, Output::Live);
        let run = it.session_run(func.locals.as_slice(), &bindings.0, body, from.min(items), span);
        match run {
            Ok(slots) => slots,
            Err(u) => return Err(session_fault(u, span)),
        }
    };
    *bindings = Bindings(ids.into_iter().filter_map(|id| slot_value(&slots[id.0]).map(|v| (id, v))).collect());
    Ok(())
}

/// Runs the drops of the session's live bindings, last-declared first (RP6: `:reset`). A user `Drop`
/// implementation runs exactly as the end of the compiled function's scope would run it, printing included.
pub fn drop_bindings(
    program: &Program,
    main: FuncId,
    bindings: &mut Bindings,
    opts: crate::Options,
) -> Result<(), Fault> {
    let func = &program.functions[main.0];
    let mut consts: Vec<ConstState> = program.consts.iter().map(|_| ConstState::Pending).collect();
    let mut it = Interp::with_output(program, &mut consts, opts.steps, opts.release, Output::Live);
    let mut dropped = Vec::new();
    for (id, value) in bindings.0.iter().rev() {
        let local = func.locals.get(id.0).expect("a binding of the body");
        let value = match value {
            Value::Moved => {
                dropped.push((*id, Value::Moved));
                continue;
            }
            other => other.clone(),
        };
        let span = local.span;
        if !matches!(local.kind, LocalKind::Owned | LocalKind::OwnParam | LocalKind::Scrutinee) {
            dropped.push((*id, Value::Moved));
            continue;
        }
        let ty: Ty = local.ty.clone();
        if let Err(u) = it.drop_value(value, &ty, span) {
            return Err(session_fault(u, span));
        }
        dropped.push((*id, Value::Moved));
    }
    *bindings = Bindings(dropped);
    Ok(())
}

/// The value a slot holds, or `None` for an alias (a parameter passed by address; the session's function has
/// none).
fn slot_value(slot: &crate::interp::Slot) -> Option<Value> {
    match slot {
        crate::interp::Slot::Val(v) => Some(v.clone()),
        crate::interp::Slot::Alias(_) => None,
    }
}

/// An unwind out of the session's statements, as the fault the REPL reports: a fault keeps its own span, and
/// control flow that left the session function (only a `return` can, the checks reject the loop jumps) reports
/// at the first statement of the input.
fn session_fault(u: Unwind, at: Span) -> Fault {
    match u {
        Unwind::Fault(f) => f.into(),
        Unwind::Return(_) => {
            Fault::ControlFlow("a `return` cannot leave the session's function; the input was discarded".into(), at)
        }
        Unwind::Break | Unwind::Continue => {
            Fault::ControlFlow("a `break` or `continue` left its loop; the input was discarded".into(), at)
        }
    }
}

/// The span of one statement, for the runner's budget accounting.
fn stmt_span(s: &Stmt) -> Span {
    match s {
        Stmt::Let(_, init, _) => init.span,
        Stmt::Expr(e) => e.span,
    }
}

/// The id of the session's function, found by its name among the functions of `file`. The function's name is
/// qualified by its module (`main.main` for an in-memory `main.bd`), so the last segment is what a caller writes.
/// `None` when the analysis failed (the REPL then never evaluates).
pub fn session_function(program: &Program, name: &str, file: burdin_diag::FileId) -> Option<FuncId> {
    program
        .functions
        .iter()
        .position(|f| !f.symbolic && f.span.file == file && f.name.rsplit('.').next() == Some(name))
        .map(FuncId)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_binds_every_id_once_in_order() {
        use burdin_tir::BindMode;
        let mut ids = Vec::new();
        pattern_ids(
            &Pat::Product(vec![
                Pat::Binding(LocalId(1), BindMode::Copy),
                Pat::Ok(Box::new(Pat::At(
                    LocalId(2),
                    BindMode::Move,
                    Box::new(Pat::Binding(LocalId(3), BindMode::Move)),
                ))),
            ]),
            &mut ids,
        );
        assert_eq!(ids, [LocalId(1), LocalId(2), LocalId(3)]);
        let mut ids = Vec::new();
        pattern_ids(&Pat::None, &mut ids);
        pattern_ids(&Pat::Range(0, 1), &mut ids);
        assert!(ids.is_empty());
    }
}
