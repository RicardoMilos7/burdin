//! Constant evaluation (`spec/const-eval.md`): evaluates every `const` and `static` initializer and every
//! compile-time condition of an `if`, and leaves a program with no compile-time construct for code generation
//! (CE10). The same interpreter runs `burdin repl` sessions and `burdin test --interpret`.
//!
//! The engine is an interpreter over the typed tree (CE1, issue #158). It runs after the memory checks, so it only
//! sees programs they accepted, and it depends only on `burdin_tir` and `burdin_diag`.

mod interp;
mod memory;
mod prims;
mod reify;
mod session;
mod value;

use burdin_diag::{Code, Diagnostics, FileId, Span};
use burdin_tir::visit::{Visit, VisitMut, walk_expr, walk_expr_mut};
use burdin_tir::{
    Block, ConstId, ConstKind, Expr, ExprKind as K, FuncId, Function, Local, Program, Signature, Stmt, Ty,
};
use interp::{ConstState, Fault, Interp};
pub use session::{Bindings, Fault as SessionFault};
pub use session::{drop_bindings, eval_stmts, session_function};
pub use value::{Place, Root, Step, Value};

/// How one `burdin test --interpret` case ended (`spec/test-runner.md`, "Interpreting a test").
pub enum Interpreted {
    /// The body ran to the end and freed everything it allocated.
    Passed,
    /// The test failed: an assertion, a panic, or one of the strict memory checks, with its message and location.
    Failed(String, Span),
    /// The test needs something the interpreter does not model (a C function, a thread, a file), so it was not
    /// checked. The reason names what it needed.
    Skipped(String),
}

/// Runs one `test` block in the interpreter, with the modeled C heap and its strict checks (`spec/test-runner.md`,
/// IN1-IN5). A test that calls C, starts a thread, or touches the file system cannot run here, so it is skipped with
/// the reason instead of failing.
pub fn interpret_test(program: &Program, func: FuncId, opts: Options) -> Interpreted {
    let mut consts: Vec<ConstState> = program.consts.iter().map(|_| ConstState::Pending).collect();
    let mut it = Interp::with_output(
        program,
        &mut consts,
        opts.steps,
        opts.overflow_checks,
        opts.debug_assertions,
        interp::Output::Swallow,
    );
    it.raw_memory = true;
    let span = program.functions[func.0].span;
    match it.call(func, None, &[], span) {
        // The wrapper returns `Result<(), AssertError>`; `Err` is a failed assertion.
        Ok(Value::Res(Err(_))) => return Interpreted::Failed("the test returned an error".to_string(), span),
        Ok(_) => {}
        Err(interp::Unwind::Fault(f)) => {
            let message = f.message().to_string();
            return match f {
                // A skipped test's reason reads as what it needed ("calling the C function `getpid`"), without the
                // compile-time rule that does not apply to an interpreted run.
                Fault::Impure(..) => {
                    Interpreted::Skipped(match message.split(" is not allowed at compile time").next() {
                        Some(what) => what.to_string(),
                        None => message,
                    })
                }
                _ => Interpreted::Failed(message, f.span().unwrap_or(span)),
            };
        }
        Err(_) => return Interpreted::Failed("control flow left the function".to_string(), span),
    }
    // Every allocation the test made must have been freed (IN5). The report points at the allocation.
    match it.mem.leaks().first() {
        Some((allocated, size)) => Interpreted::Failed(format!("leaked {size} byte(s) allocated here"), *allocated),
        None => Interpreted::Passed,
    }
}

/// The default step budget of one evaluation (CE8).
pub const DEFAULT_STEPS: u64 = 10_000_000;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Integer overflow panics (`spec/const-eval.md`, CE1); when off it wraps, as `release` did before profiles.
    pub overflow_checks: bool,
    /// `debug_assert*` runs; when off it is removed, as a profile without debug assertions does.
    pub debug_assertions: bool,
    /// The step budget of each evaluation (CE8).
    pub steps: u64,
}

impl Default for Options {
    fn default() -> Options {
        Options { overflow_checks: true, debug_assertions: true, steps: DEFAULT_STEPS }
    }
}

/// Runs every compile-time evaluation of `program` and rewrites it so that no compile-time construct is left: every
/// constant is replaced by its materialized value and every compile-time `if` by its taken branch.
/// Failures are reported to `diags`; the program is then only fit for more diagnostics, not for code generation.
pub fn evaluate(program: &mut Program, opts: Options, diags: &mut Diagnostics) {
    let mut consts: Vec<ConstState> = program.consts.iter().map(|_| ConstState::Pending).collect();
    evaluate_consts(program, &mut consts, opts, diags);
    let mut rw = Rewriter { p: program, consts, opts, diags };
    let mut bodies: Vec<(usize, Block)> = Vec::new();
    for (i, f) in rw.p.functions.iter().enumerate() {
        if f.symbolic || !contains_const_cond(&f.body) {
            continue;
        }
        let mut body = f.body.clone();
        rw.rewrite_block(&f.locals, &mut body);
        bodies.push((i, body));
    }
    let mut closure_bodies: Vec<(usize, Block)> = Vec::new();
    for (i, c) in rw.p.closures.iter().enumerate() {
        if !contains_const_cond(&c.func.body) {
            continue;
        }
        let mut body = c.func.body.clone();
        rw.rewrite_block(&c.func.locals, &mut body);
        closure_bodies.push((i, body));
    }
    let Rewriter { consts, .. } = rw;
    let mut values = Vec::with_capacity(consts.len());
    for (i, state) in consts.iter().enumerate() {
        if let ConstState::Done(v) = state {
            let c = &program.consts[i];
            values.push((i, reify::reify(program, v, &c.value.ty, c.value.span)));
        }
    }
    for (i, value) in values {
        match value {
            Ok(e) => {
                program.consts[i].value = e;
                program.consts[i].locals.clear();
            }
            Err(msg) => {
                diags.error(Code::CONST_IMPURE, program.consts[i].span, internal_message(&msg));
            }
        }
    }
    for (i, body) in bodies {
        program.functions[i].body = body;
    }
    for (i, body) in closure_bodies {
        program.closures[i].func.body = body;
    }
    program.static_init = static_init(program);
}

/// Builds the function that stores every static's value, in declaration order, before `main` runs (`spec/globals.md`,
/// GL1), and returns its id. A static's initializer is already a materialized value (CE3) and names no static (GL1),
/// so the order does not matter. `None` when the program declares no static.
fn static_init(program: &mut Program) -> Option<FuncId> {
    let statics: Vec<usize> =
        (0..program.consts.len()).filter(|&i| program.consts[i].kind != ConstKind::Const).collect();
    let first = *statics.first()?;
    let stmts = statics
        .iter()
        .map(|&i| {
            let c = &program.consts[i];
            let place = Expr { kind: K::Static(ConstId(i)), ty: c.value.ty.clone(), span: c.span };
            let store = K::Assign(Box::new(place), Box::new(c.value.clone()));
            Stmt::Expr(Expr { kind: store, ty: Ty::Unit, span: c.span })
        })
        .collect();
    let span = program.consts[first].span;
    let func = Function {
        name: "static.init".to_string(),
        span,
        params: Vec::new(),
        ret: Ty::Unit,
        locals: Vec::new(),
        body: Block { stmts, tail: None, close: span },
        moved: Vec::new(),
        debug_kills: Vec::new(),
        ret_origins: Vec::new(),
        symbolic: false,
        exported: false,
        events: Vec::new(),
        internal: false,
    };
    let id = FuncId(program.functions.len());
    program.functions.push(func);
    program.signatures.push(Signature { params: Vec::new(), ret: Ty::Unit, has_self: false, ret_origins: Vec::new() });
    Some(id)
}

/// Evaluates every constant in declaration order (CE2).
fn evaluate_consts(program: &Program, consts: &mut Vec<ConstState>, opts: Options, diags: &mut Diagnostics) {
    for id in 0..program.consts.len() {
        if !matches!(consts[id], ConstState::Pending) {
            continue;
        }
        let c = &program.consts[id];
        let result = {
            let mut it = Interp::new(program, consts, opts.steps, opts.overflow_checks, opts.debug_assertions);
            it.eval_root(&c.locals, &c.value)
        };
        match result {
            Ok(v) => consts[id] = ConstState::Done(v),
            Err(fault) => {
                consts[id] = ConstState::Failed;
                report(program, diags, fault, c.span, &format!("the constant `{}`", c.name));
            }
        }
    }
}

/// Reports a failed evaluation at `at`, the constant, condition, or call it belongs to.
fn report(p: &Program, diags: &mut Diagnostics, fault: Fault, at: Span, what: &str) {
    let (code, message, op) = match fault {
        Fault::Reported => return,
        Fault::Panic(msg, op) => {
            (Code::CONST_PANIC, format!("evaluating {what} at compile time panicked: {msg}"), Some(op))
        }
        Fault::Impure(msg, op) => (Code::CONST_IMPURE, msg, Some(op)),
        Fault::Budget(msg, op) => (Code::CONST_BUDGET, msg, Some(op)),
        Fault::Internal(msg, op) => (Code::CONST_IMPURE, internal_message(&msg), Some(op)),
        Fault::Cycle(id, used) => {
            let name = &p.consts[id].name;
            diags
                .error(
                    Code::CYCLIC_DECLARATION,
                    at,
                    format!("the constant `{name}` depends on itself: its value is needed while it is being computed"),
                )
                .note(used, format!("`{name}` is used here, through a call, while it is being computed"));
            return;
        }
    };
    let d = diags.error(code, at, message);
    if let Some(op) = op.filter(|op| *op != at) {
        d.note(op, "this operation failed while the compiler was evaluating it");
    }
}

fn internal_message(msg: &str) -> String {
    format!("internal error in the constant interpreter: {msg}; please report it")
}

/// Whether a block has any compile-time condition to evaluate.
fn contains_const_cond(b: &Block) -> bool {
    struct Find(bool);
    impl Visit for Find {
        fn expr(&mut self, e: &Expr) {
            if matches!(e.kind, K::ConstCond(_)) {
                self.0 = true;
            } else if !self.0 {
                walk_expr(self, e);
            }
        }
    }
    let mut f = Find(false);
    f.block(b);
    f.0
}

struct Rewriter<'a> {
    p: &'a Program,
    consts: Vec<ConstState>,
    opts: Options,
    diags: &'a mut Diagnostics,
}

impl Rewriter<'_> {
    fn eval(&mut self, locals: &[Local], e: &Expr) -> Result<Value, Fault> {
        let mut it = Interp::new(
            self.p,
            &mut self.consts,
            self.opts.steps,
            self.opts.overflow_checks,
            self.opts.debug_assertions,
        );
        it.eval_root(locals, e)
    }

    fn rewrite_block(&mut self, locals: &[Local], b: &mut Block) {
        let mut cx = BodyCx { rw: self, locals };
        cx.block(b);
    }
}

/// Rewrites one body: each `if` with a compile-time condition keeps only the branch the condition selects (CE4).
struct BodyCx<'r, 'a, 'l> {
    rw: &'r mut Rewriter<'a>,
    locals: &'l [Local],
}

impl BodyCx<'_, '_, '_> {
    fn eval(&mut self, e: &Expr) -> Option<Value> {
        match self.rw.eval(self.locals, e) {
            Ok(v) => Some(v),
            Err(fault) => {
                report(self.rw.p, self.rw.diags, fault, e.span, "this condition");
                None
            }
        }
    }
}

impl VisitMut for BodyCx<'_, '_, '_> {
    fn expr(&mut self, e: &mut Expr) {
        match &mut e.kind {
            K::If(cond, then, other) if matches!(cond.kind, K::ConstCond(_)) => {
                let K::ConstCond(c) = &cond.kind else { unreachable!("matched above") };
                let Some(v) = self.eval(c) else { return };
                // Only the selected branch is kept, and only it is rewritten (CE4).
                if v.bool() {
                    let then = std::mem::replace(
                        then,
                        Block { stmts: Vec::new(), tail: None, close: Span::new(FileId(0), 0, 0) },
                    );
                    e.kind = K::Block(then);
                } else {
                    match other.take() {
                        Some(o) => {
                            let ty = e.ty.clone();
                            *e = *o;
                            if e.ty != Ty::Never {
                                e.ty = ty;
                            }
                        }
                        None => e.kind = K::Unit,
                    }
                }
                self.expr(e);
            }
            _ => walk_expr_mut(self, e),
        }
    }
}
