//! Compile-time execution (`spec/comptime.md`): evaluates every constant, every `comptime` expression, `comptime
//! if` condition, `comptime for` iterable, and `comptime assert`, specializes the functions with `comptime`
//! parameters, and leaves a program with no comptime construct for code generation (CT13).
//!
//! The engine is an interpreter over the typed tree (CT1, issue #158). It runs after the memory checks, so it only
//! sees programs they accepted, and it depends only on `burdin_tir` and `burdin_diag`.

mod interp;
mod prims;
mod reify;
mod session;
mod value;

use burdin_diag::{Code, Diagnostics, Span};
use burdin_tir::visit::{Visit, VisitMut, walk_expr, walk_expr_mut};
use burdin_tir::{
    BindMode, Block, Expr, ExprKind as K, FuncId, Function, Local, LocalId, LocalKind, Pat, Program, Signature, Stmt,
    TestOutcome, Ty, TypeDefs,
};
use interp::{ConstState, Fault, Interp};
pub use session::{Bindings, Fault as SessionFault};
pub use session::{drop_bindings, eval_stmts, session_function};
use std::collections::HashMap;
pub use value::{Place, Root, Step, Value};

/// The default step budget of one evaluation (CT11).
pub const DEFAULT_STEPS: u64 = 10_000_000;

/// More specializations of one function than this is the budget error E0225 (CT4).
pub const MAX_SPECIALIZATIONS: usize = 1024;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// A `--release` build: integer overflow wraps instead of panicking (CT1).
    pub release: bool,
    /// The step budget of each evaluation (CT11).
    pub steps: u64,
}

impl Default for Options {
    fn default() -> Options {
        Options { release: false, steps: DEFAULT_STEPS }
    }
}

/// Runs every compile-time evaluation of `program` and rewrites it so that no comptime construct is left.
/// Failures are reported to `diags`; the program is then only fit for more diagnostics, not for code generation.
pub fn evaluate(program: &mut Program, opts: Options, diags: &mut Diagnostics) {
    let mut consts: Vec<ConstState> = program.consts.iter().map(|_| ConstState::Pending).collect();
    evaluate_consts(program, &mut consts, opts, diags);
    let mut rw = Rewriter {
        p: program,
        consts,
        opts,
        diags,
        new_funcs: Vec::new(),
        specs: HashMap::new(),
        spec_counts: HashMap::new(),
    };
    let mut bodies: Vec<(usize, Block)> = Vec::new();
    for (i, f) in rw.p.functions.iter().enumerate() {
        if f.symbolic || rw.is_template(FuncId(i)) || !contains_comptime(&f.body) {
            continue;
        }
        let mut body = f.body.clone();
        rw.rewrite_block(&f.locals, &[], &mut body);
        bodies.push((i, body));
    }
    let mut closure_bodies: Vec<(usize, Block)> = Vec::new();
    for (i, c) in rw.p.closures.iter().enumerate() {
        if !contains_comptime(&c.func.body) {
            continue;
        }
        let mut body = c.func.body.clone();
        rw.rewrite_block(&c.func.locals, &[], &mut body);
        closure_bodies.push((i, body));
    }
    // A specialization's body is rewritten once, with its comptime parameters bound; rewriting it may create more.
    let mut done = 0;
    while done < rw.new_funcs.len() {
        let (env, locals) = (rw.new_funcs[done].env.clone(), rw.new_funcs[done].func.locals.clone());
        let mut body = std::mem::replace(&mut rw.new_funcs[done].func.body, Block { stmts: Vec::new(), tail: None });
        rw.rewrite_block(&locals, &env, &mut body);
        rw.new_funcs[done].func.body = body;
        done += 1;
    }
    let Rewriter { consts, new_funcs, .. } = rw;
    let templates: Vec<usize> =
        (0..program.functions.len()).filter(|&i| program.signatures[i].params.iter().any(|p| p.comptime)).collect();
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
                diags.error(Code::COMPTIME_IMPURE, program.consts[i].span, internal_message(&msg));
            }
        }
    }
    for (i, body) in bodies {
        program.functions[i].body = body;
    }
    for (i, body) in closure_bodies {
        program.closures[i].func.body = body;
    }
    for i in templates {
        // Every call now goes to a specialization; the template itself has no code (CT13).
        program.functions[i].symbolic = true;
    }
    for spec in new_funcs {
        program.functions.push(spec.func);
        program.signatures.push(spec.sig);
    }
}

/// Runs the body of every `[differential]` test at compile time and records its `Ok`/`Err` outcome in the test
/// entry, for the runner to compare with the compiled run (`spec/test-runner.md`). The body must be one the
/// interpreter can execute: an impure operation (CT9), a panic (CT10), or an exhausted budget (CT11) is reported at
/// the test item, like any other failed compile-time evaluation. This runs after [`evaluate`], so the bodies it
/// sees have no comptime construct left.
pub fn evaluate_differential_tests(program: &mut Program, opts: Options, diags: &mut Diagnostics) {
    // Fresh constant states: after `evaluate` every constant initializer is a materialized literal, so evaluating
    // one again here costs no more than the literal itself.
    let mut consts: Vec<ConstState> = program.consts.iter().map(|_| ConstState::Pending).collect();
    let mut outcomes = Vec::with_capacity(program.tests.len());
    for test in &program.tests {
        if !test.attrs.differential {
            outcomes.push(None);
            continue;
        }
        let body = test.body;
        let span = program.functions[body.0].span;
        let result = {
            let mut it = Interp::new(program, &mut consts, opts.steps, opts.release);
            it.call_root(body, span)
        };
        let outcome = match result {
            Ok(Value::Res(Ok(_))) => Some(TestOutcome::Ok),
            Ok(Value::Res(Err(_))) => Some(TestOutcome::Err),
            Ok(_) => {
                diags.error(
                    Code::COMPTIME_IMPURE,
                    span,
                    internal_message("a differential test body did not return a `Result`"),
                );
                None
            }
            Err(fault) => {
                if matches!(fault, Fault::Reported) {
                    // `report` stays silent for a failure an earlier report already covered. The fresh constant
                    // states above make that impossible here, so leaving the outcome unset would hide a bug.
                    diags.error(
                        Code::COMPTIME_IMPURE,
                        span,
                        internal_message("a differential test body failed without a reported error"),
                    );
                } else {
                    report(program, diags, fault, span, &format!("the differential test `{}`", test.name));
                }
                None
            }
        };
        outcomes.push(outcome);
    }
    for (test, outcome) in program.tests.iter_mut().zip(outcomes) {
        test.comptime = outcome;
    }
}

/// Evaluates every constant in declaration order (CT7).
fn evaluate_consts(program: &Program, consts: &mut Vec<ConstState>, opts: Options, diags: &mut Diagnostics) {
    for id in 0..program.consts.len() {
        if !matches!(consts[id], ConstState::Pending) {
            continue;
        }
        let c = &program.consts[id];
        let result = {
            let mut it = Interp::new(program, consts, opts.steps, opts.release);
            it.eval_root(&c.locals, &[], &c.value)
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

/// Reports a failed evaluation at `at`, the comptime expression, constant, or call it belongs to.
fn report(p: &Program, diags: &mut Diagnostics, fault: Fault, at: Span, what: &str) {
    let (code, message, op) = match fault {
        Fault::Reported => return,
        Fault::Panic(msg, op) => {
            (Code::COMPTIME_PANIC, format!("evaluating {what} at compile time panicked: {msg}"), Some(op))
        }
        Fault::Impure(msg, op) => (Code::COMPTIME_IMPURE, msg, Some(op)),
        Fault::Budget(msg, op) => (Code::COMPTIME_BUDGET, msg, Some(op)),
        Fault::Internal(msg, op) => (Code::COMPTIME_IMPURE, internal_message(&msg), Some(op)),
        Fault::Assert(msg, op) => {
            let message = if msg.is_empty() {
                "comptime assertion failed".to_string()
            } else {
                format!("comptime assertion failed: {msg}")
            };
            diags.error(Code::COMPTIME_ASSERT, op, message);
            return;
        }
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
    format!("internal error in the comptime interpreter: {msg}; please report it")
}

/// Whether a block has any comptime construct to evaluate.
fn contains_comptime(b: &Block) -> bool {
    struct Find(bool);
    impl Visit for Find {
        fn expr(&mut self, e: &Expr) {
            if matches!(e.kind, K::Comptime(_) | K::ComptimeFor { .. }) {
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

/// A specialization of a function with comptime parameters (CT4): the copy, its signature, and the values of the
/// parameters it was made for.
struct Spec {
    func: Function,
    sig: Signature,
    env: Vec<(LocalId, Value)>,
}

struct Rewriter<'a> {
    p: &'a Program,
    consts: Vec<ConstState>,
    opts: Options,
    diags: &'a mut Diagnostics,
    new_funcs: Vec<Spec>,
    /// Each specialization by its function and the printed values of its comptime arguments.
    specs: HashMap<(usize, String), FuncId>,
    spec_counts: HashMap<usize, usize>,
}

impl Rewriter<'_> {
    fn is_template(&self, f: FuncId) -> bool {
        self.p.signatures.get(f.0).is_some_and(|s| s.params.iter().any(|p| p.comptime))
    }

    fn eval(&mut self, locals: &[Local], env: &[(LocalId, Value)], e: &Expr) -> Result<Value, Fault> {
        let mut it = Interp::new(self.p, &mut self.consts, self.opts.steps, self.opts.release);
        it.eval_root(locals, env, e)
    }

    fn items(&mut self, locals: &[Local], env: &[(LocalId, Value)], e: &Expr) -> Result<Vec<Value>, Fault> {
        let mut it = Interp::new(self.p, &mut self.consts, self.opts.steps, self.opts.release);
        it.items_root(locals, env, e)
    }

    fn materialize(&mut self, v: &Value, ty: &Ty, span: Span) -> Option<Expr> {
        match reify::reify(self.p, v, ty, span) {
            Ok(e) => Some(e),
            Err(msg) => {
                self.diags.error(Code::COMPTIME_IMPURE, span, internal_message(&msg));
                None
            }
        }
    }

    fn rewrite_block(&mut self, locals: &[Local], env: &[(LocalId, Value)], b: &mut Block) {
        let mut cx = BodyCx { rw: self, locals, env: env.to_vec() };
        cx.block(b);
    }

    /// The specialization of template `f` for these comptime argument values, made on first use (CT4).
    fn specialize(&mut self, f: FuncId, values: Vec<Value>, span: Span) -> Option<FuncId> {
        let key = (f.0, format!("{values:?}"));
        if let Some(&id) = self.specs.get(&key) {
            return Some(id);
        }
        let count = self.spec_counts.entry(f.0).or_insert(0);
        *count += 1;
        let t = &self.p.functions[f.0];
        if *count > MAX_SPECIALIZATIONS {
            if *count == MAX_SPECIALIZATIONS + 1 {
                self.diags.error(
                    Code::COMPTIME_BUDGET,
                    span,
                    format!(
                        "`{}` needs more than {MAX_SPECIALIZATIONS} specializations for different comptime \
                         arguments; a recursion through a comptime parameter needs a `comptime if` that ends it \
                         (spec/comptime.md, CT4)",
                        t.name
                    ),
                );
            }
            return None;
        }
        let k = *count;
        let sig = &self.p.signatures[f.0];
        let mut func = Function {
            name: format!("{}.comptime{k}", t.name),
            span: t.span,
            params: Vec::new(),
            ret: t.ret.clone(),
            locals: t.locals.clone(),
            body: t.body.clone(),
            moved: t.moved.clone(),
            debug_kills: t.debug_kills.clone(),
            ret_origins: Vec::new(),
            symbolic: false,
            exported: false,
        };
        let mut new_sig =
            Signature { params: Vec::new(), ret: sig.ret.clone(), has_self: sig.has_self, ret_origins: Vec::new() };
        let mut env = Vec::new();
        let mut prefix = Vec::new();
        let mut values = values.into_iter();
        let mut kept = 0;
        for (i, (param, ps)) in t.params.iter().zip(&sig.params).enumerate() {
            if !ps.comptime {
                if t.ret_origins.contains(&i) {
                    func.ret_origins.push(kept);
                }
                if sig.ret_origins.contains(&i) {
                    new_sig.ret_origins.push(kept);
                }
                func.params.push(*param);
                new_sig.params.push(ps.clone());
                kept += 1;
                continue;
            }
            let v = values.next().unwrap_or(Value::Unit);
            // The parameter becomes a local of the copy, initialized from the value (CT4).
            let local = &mut func.locals[param.0];
            local.by_ref = false;
            local.kind = LocalKind::Owned;
            let ty = local.ty.clone();
            let init = self.materialize(&v, &ty, span)?;
            let defs = TypeDefs { structs: &self.p.structs, enums: &self.p.enums };
            let mode = if defs.is_copy(&ty) { BindMode::Copy } else { BindMode::Move };
            prefix.push(Stmt::Let(Pat::Binding(*param, mode), init, None));
            env.push((*param, v));
        }
        prefix.append(&mut func.body.stmts);
        func.body.stmts = prefix;
        let id = FuncId(self.p.functions.len() + self.new_funcs.len());
        self.new_funcs.push(Spec { func, sig: new_sig, env });
        self.specs.insert(key, id);
        Some(id)
    }
}

/// Rewrites one body: `env` holds the compile-time-known locals in scope (comptime parameters and `comptime for`
/// bindings) with their values.
struct BodyCx<'r, 'a, 'l> {
    rw: &'r mut Rewriter<'a>,
    locals: &'l [Local],
    env: Vec<(LocalId, Value)>,
}

impl BodyCx<'_, '_, '_> {
    fn eval(&mut self, e: &Expr) -> Option<Value> {
        match self.rw.eval(self.locals, &self.env, e) {
            Ok(v) => Some(v),
            Err(fault) => {
                report(self.rw.p, self.rw.diags, fault, e.span, "this expression");
                None
            }
        }
    }

    /// `comptime for`: one copy of the body per item, each starting with `let pattern = item` (CT6).
    fn unroll(&mut self, pat: &Pat, iter: &Expr, body: &Block, span: Span) -> Option<Expr> {
        let items = match self.rw.items(self.locals, &self.env, iter) {
            Ok(items) => items,
            Err(fault) => {
                report(self.rw.p, self.rw.diags, fault, iter.span, "this expression");
                return None;
            }
        };
        let item_ty = match &iter.ty {
            Ty::Array(t, _) | Ty::List(t, _) | Ty::Range(t, _) => (**t).clone(),
            other => other.clone(),
        };
        let mut copies = Vec::with_capacity(items.len());
        for item in items {
            let init = self.rw.materialize(&item, &item_ty, iter.span)?;
            let mut copy = body.clone();
            copy.stmts.insert(0, Stmt::Let(pat.clone(), init, None));
            let mark = self.env.len();
            destructure(pat, &item, &mut self.env);
            self.block(&mut copy);
            self.env.truncate(mark);
            let ty = copy.tail.as_ref().map_or(Ty::Unit, |t| t.ty.clone());
            copies.push(Stmt::Expr(Expr { kind: K::Block(copy), ty, span }));
        }
        Some(Expr { kind: K::Block(Block { stmts: copies, tail: None }), ty: Ty::Unit, span })
    }
}

impl VisitMut for BodyCx<'_, '_, '_> {
    fn expr(&mut self, e: &mut Expr) {
        match &mut e.kind {
            K::Comptime(inner) => {
                let Some(v) = self.eval(inner) else { return };
                if let Some(lit) = self.rw.materialize(&v, &e.ty, e.span) {
                    *e = lit;
                }
            }
            K::If(cond, then, other) if matches!(cond.kind, K::Comptime(_)) => {
                let K::Comptime(c) = &cond.kind else { unreachable!("matched above") };
                let Some(v) = self.eval(c) else { return };
                // Only the selected branch is kept, and only it is rewritten (CT5).
                if v.bool() {
                    let then = std::mem::replace(then, Block { stmts: Vec::new(), tail: None });
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
            K::ComptimeFor { pat, iter, body } => {
                let (pat, iter, body) = (pat.clone(), (**iter).clone(), body.clone());
                if let Some(unrolled) = self.unroll(&pat, &iter, &body, e.span) {
                    *e = unrolled;
                }
            }
            K::Call(f, args) if self.rw.is_template(*f) => {
                let f = *f;
                let sig_params: Vec<bool> = self.rw.p.signatures[f.0].params.iter().map(|p| p.comptime).collect();
                let mut values = Vec::new();
                let mut rest = Vec::new();
                let mut failed = false;
                for (arg, comptime) in std::mem::take(args).into_iter().zip(sig_params) {
                    if comptime {
                        match self.eval(&arg) {
                            Some(v) => values.push(v),
                            None => failed = true,
                        }
                    } else {
                        rest.push(arg);
                    }
                }
                for arg in &mut rest {
                    self.expr(arg);
                }
                let spec = if failed { None } else { self.rw.specialize(f, values, e.span) };
                e.kind = K::Call(spec.unwrap_or(f), rest);
            }
            _ => walk_expr_mut(self, e),
        }
    }
}

/// Binds the locals of an irrefutable pattern to the parts of `v`, for the compile-time environment.
fn destructure(pat: &Pat, v: &Value, out: &mut Vec<(LocalId, Value)>) {
    let v = prims::shallow(v);
    match pat {
        Pat::Binding(id, _) => out.push((*id, v.clone())),
        Pat::At(id, _, inner) => {
            out.push((*id, v.clone()));
            destructure(inner, v, out);
        }
        Pat::Some(inner) | Pat::Ok(inner) | Pat::Err(inner) => match v {
            Value::Opt(Some(x)) | Value::Res(Ok(x)) | Value::Res(Err(x)) => destructure(inner, x, out),
            _ => {}
        },
        Pat::Product(subs) | Pat::Variant(_, subs) => {
            if let Value::Tuple(fs) | Value::Variant(_, fs) = v {
                for (s, f) in subs.iter().zip(fs) {
                    destructure(s, f, out);
                }
            }
        }
        Pat::Or(alts) => {
            if let Some(a) = alts.iter().find(|a| interp::pattern_matches(a, v)) {
                destructure(a, v, out);
            }
        }
        _ => {}
    }
}
