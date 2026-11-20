//! The interpreter over the typed tree (`spec/comptime.md`, CT1). It only runs programs the memory checks accepted,
//! and it follows the tree's own ownership facts: a place read in a move context is moved out of, a parameter
//! passed by address aliases the caller's place, and owned locals are dropped at the end of their scope.

use crate::memory::Memory;
use crate::value::{Place, Root, Step, Value};
use burdin_diag::Span;
use burdin_tir::{
    self as tir, BindMode, Block, Expr, ExprKind as K, FuncId, Local, LocalKind, Pat, Program, Stmt, Ty, TypeDefs,
};
use std::collections::HashMap;

/// Why an evaluation stopped without a value.
#[derive(Debug)]
pub(crate) enum Fault {
    /// A panic (CT10): the message and where it happened.
    Panic(String, Span),
    /// An operation comptime does not allow (CT9).
    Impure(String, Span),
    /// The step budget, the call depth, or the specialization limit (CT11).
    Budget(String, Span),
    /// A failed `comptime assert` (CT8).
    Assert(String, Span),
    /// A constant reached itself while it was being evaluated (CT7): the constant's id.
    Cycle(usize, Span),
    /// A constant this evaluation needs already failed, and that failure was reported.
    Reported,
    /// Something the checks should have ruled out; reported rather than crashing the compiler.
    Internal(String, Span),
}

pub(crate) enum Unwind {
    Break,
    Continue,
    Return(Value),
    Fault(Fault),
}

impl From<Fault> for Unwind {
    fn from(f: Fault) -> Unwind {
        Unwind::Fault(f)
    }
}

impl Fault {
    /// The message a `burdin test --interpret` report shows, and where it happened.
    pub fn message(&self) -> &str {
        match self {
            Fault::Panic(m, _) | Fault::Impure(m, _) | Fault::Budget(m, _) | Fault::Assert(m, _) => m,
            Fault::Internal(m, _) => m,
            Fault::Cycle(..) | Fault::Reported => "compile-time evaluation reached itself",
        }
    }

    pub fn span(&self) -> Option<Span> {
        match self {
            Fault::Panic(_, s)
            | Fault::Impure(_, s)
            | Fault::Budget(_, s)
            | Fault::Assert(_, s)
            | Fault::Internal(_, s)
            | Fault::Cycle(_, s) => Some(*s),
            Fault::Reported => None,
        }
    }
}

pub(crate) type R<T> = Result<T, Unwind>;

/// How an expression's value is used: moved into a new owner, or only read.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Move,
    Use,
}

pub(crate) enum Slot {
    Val(Value),
    /// A parameter or capture passed by address: the caller's place.
    Alias(Place),
}

struct Frame<'p> {
    slots: Vec<Slot>,
    locals: &'p [Local],
    /// The temporaries of the statements being evaluated, with their types.
    temps: Vec<(Value, Ty)>,
    /// The locals bound in the open scopes, in binding order, for drops at scope end.
    scope: Vec<usize>,
}

/// A constant's evaluation state (CT7).
pub(crate) enum ConstState {
    Pending,
    Running,
    Done(Value),
    Failed,
}

pub(crate) const MAX_DEPTH: usize = 512;

/// Where the interpreter's output builtins write: nowhere (compile-time evaluation, CT9) or the real standard
/// output and error of the process (a REPL session, `spec/repl.md` RP3).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Output {
    /// Nowhere: printing is the impure fault a compile-time evaluation reports (CT9).
    Silent,
    /// The real standard output and error of the process (a REPL session, `spec/repl.md` RP3).
    Live,
    /// Nowhere, but printing is allowed: `burdin test --interpret` captures a test's output the way a compiled run
    /// does, so the test's own prints are dropped rather than refused (`spec/test-runner.md`, IN6).
    Swallow,
}

pub(crate) struct Interp<'p, 'c> {
    pub(crate) p: &'p Program,
    pub(crate) consts: &'c mut Vec<ConstState>,
    frames: Vec<Frame<'p>>,
    pub(crate) steps: u64,
    pub(crate) budget: u64,
    pub(crate) release: bool,
    pub(crate) out: Output,
    drop_cache: HashMap<Ty, bool>,
    /// The C heap the run models: `burdin test --interpret` (`spec/test-runner.md`, IN1). Elsewhere the allocation
    /// builtins stay impure, because comptime is pure (CT9).
    pub(crate) mem: Memory,
    pub(crate) raw_memory: bool,
}

impl<'p, 'c> Interp<'p, 'c> {
    pub(crate) fn new(p: &'p Program, consts: &'c mut Vec<ConstState>, budget: u64, release: bool) -> Self {
        Self::with_output(p, consts, budget, release, Output::Silent)
    }

    pub(crate) fn with_output(
        p: &'p Program,
        consts: &'c mut Vec<ConstState>,
        budget: u64,
        release: bool,
        out: Output,
    ) -> Self {
        Interp {
            p,
            consts,
            frames: Vec::new(),
            steps: 0,
            budget,
            release,
            out,
            drop_cache: HashMap::new(),
            mem: Memory::default(),
            raw_memory: false,
        }
    }

    fn defs(&self) -> TypeDefs<'p> {
        TypeDefs { structs: &self.p.structs, enums: &self.p.enums }
    }

    pub(crate) fn is_copy(&self, ty: &Ty) -> bool {
        self.defs().is_copy(ty)
    }

    /// Evaluates `e` in a fresh frame with `locals`, the compile-time-known locals in `env` already bound.
    pub(crate) fn eval_root(
        &mut self,
        locals: &'p [Local],
        env: &[(tir::LocalId, Value)],
        e: &'p Expr,
    ) -> Result<Value, Fault> {
        self.root(locals, env, e.span, |it| it.eval(e, Mode::Move))
    }

    /// The items of a `comptime for` iterable, evaluated like [`Interp::eval_root`].
    pub(crate) fn items_root(
        &mut self,
        locals: &'p [Local],
        env: &[(tir::LocalId, Value)],
        e: &'p Expr,
    ) -> Result<Vec<Value>, Fault> {
        self.root(locals, env, e.span, |it| it.comptime_items(e))
    }

    fn root<T>(
        &mut self,
        locals: &'p [Local],
        env: &[(tir::LocalId, Value)],
        span: Span,
        f: impl FnOnce(&mut Self) -> R<T>,
    ) -> Result<T, Fault> {
        let mut slots: Vec<Slot> = locals.iter().map(|_| Slot::Val(Value::Moved)).collect();
        for (id, v) in env {
            slots[id.0] = Slot::Val(v.clone());
        }
        self.frames.push(Frame { slots, locals, temps: Vec::new(), scope: Vec::new() });
        let result = match f(self) {
            Ok(v) => self.end_temps(0, span).map(|_| v),
            Err(u) => Err(u),
        };
        self.frames.pop();
        match result {
            Ok(v) => Ok(v),
            Err(Unwind::Fault(f)) => Err(f),
            Err(Unwind::Return(_)) | Err(Unwind::Break) | Err(Unwind::Continue) => {
                Err(Fault::Internal("control flow left a comptime expression".into(), span))
            }
        }
    }

    pub(crate) fn step(&mut self, span: Span) -> Result<(), Fault> {
        self.charge(1, span)
    }

    /// Spends `n` steps of the budget (CT11).
    pub(crate) fn charge(&mut self, n: u64, span: Span) -> Result<(), Fault> {
        self.steps = self.steps.saturating_add(n);
        if self.steps > self.budget {
            return Err(Fault::Budget(
                format!(
                    "compile-time evaluation ran for more than {} steps; raise the budget with `--comptime-steps` if the computation is meant to be this long",
                    self.budget
                ),
                span,
            ));
        }
        Ok(())
    }

    fn frame(&mut self) -> &mut Frame<'p> {
        self.frames.last_mut().expect("a frame is open")
    }

    fn cur(&self) -> usize {
        self.frames.len() - 1
    }

    // ----- places --------------------------------------------------------------------------------------

    fn temp(&mut self, v: Value, ty: &Ty) -> Place {
        let frame = self.cur();
        let f = self.frame();
        f.temps.push((v, ty.clone()));
        Place { frame, root: Root::Temp(f.temps.len() - 1), path: Vec::new() }
    }

    pub(crate) fn place_mut(&mut self, p: &Place, span: Span) -> R<&mut Value> {
        let frame = &mut self.frames[p.frame];
        let mut cur: &mut Value = match p.root {
            Root::Local(i) => match &mut frame.slots[i] {
                Slot::Val(v) => v,
                Slot::Alias(_) => return Err(Fault::Internal("an alias inside a place".into(), span).into()),
            },
            Root::Temp(i) => &mut frame.temps[i].0,
        };
        for step in &p.path {
            cur = step_mut(cur, *step, span)?;
        }
        Ok(cur)
    }

    /// `*p`: the value a `CPtr<T>` points at, with the strict checks of `spec/test-runner.md` (IN2-IN4).
    pub(crate) fn ptr_deref(&mut self, ptr: &'p Expr, pointee: &Ty, span: Span) -> R<Value> {
        let Some(scalar) = crate::memory::scalar(pointee) else {
            return Err(impure(&format!("dereferencing a `CPtr<{pointee}>`"), span));
        };
        let addr = self.eval(ptr, Mode::Use)?.int();
        let n = self.mem.read(addr, scalar, span).map_err(Unwind::Fault)?;
        Ok(match pointee {
            Ty::Bool => Value::Bool(n != 0),
            Ty::Char => Value::Char(char::from_u32(n as u32).unwrap_or('\u{fffd}')),
            Ty::Float(tir::FloatTy::F32) => Value::Float(f64::from(f32::from_bits(n as u32))),
            Ty::Float(_) => Value::Float(f64::from_bits(n as u64)),
            _ => Value::Int(n),
        })
    }

    /// `*p = v`: stores `v`'s bytes through the pointer, with the same checks.
    fn ptr_write(&mut self, addr: i128, pointee: &Ty, v: Value, span: Span) -> R<()> {
        let Some(scalar) = crate::memory::scalar(pointee) else {
            return Err(impure(&format!("storing through a `CPtr<{pointee}>`"), span));
        };
        let n = match pointee {
            Ty::Float(_) => crate::memory::float_bits(pointee, v.float()),
            Ty::CPtr(_) => {
                self.drop_value(v.clone(), pointee, span)?;
                v.int()
            }
            _ => v.int(),
        };
        self.mem.write(addr, scalar, n, span).map_err(Unwind::Fault)
    }

    fn read_place(&mut self, p: &Place, take: bool, span: Span) -> R<Value> {
        let v = self.place_mut(p, span)?;
        if take { Ok(std::mem::replace(v, Value::Moved)) } else { Ok(v.clone()) }
    }

    /// The place an expression denotes. A value that is not a place is stored in a temporary first.
    pub(crate) fn eval_place(&mut self, e: &'p Expr) -> R<Place> {
        match &e.kind {
            K::Local(id) => {
                let frame = self.cur();
                Ok(match &self.frames[frame].slots[id.0] {
                    Slot::Alias(p) => p.clone(),
                    Slot::Val(_) => Place { frame, root: Root::Local(id.0), path: Vec::new() },
                })
            }
            K::Field(base, n) => {
                let mut p = self.eval_place(base)?;
                p.path.push(Step::Field(*n));
                Ok(p)
            }
            K::Index(base, index) => {
                let mut p = self.eval_place(base)?;
                let i = self.eval(index, Mode::Use)?.int();
                p.path.push(Step::Index(usize::try_from(i).unwrap_or(usize::MAX)));
                Ok(p)
            }
            K::Deref(base) => {
                let mut p = self.eval_place(base)?;
                p.path.push(Step::Deref);
                Ok(p)
            }
            K::DerefRef(r) => match self.eval(r, Mode::Use)? {
                Value::Ref(p) => Ok(p),
                Value::Borrow(v) => Ok(self.temp(*v, &e.ty)),
                v => Ok(self.temp(v, &e.ty)),
            },
            // A store through `*p` is handled by the assignment itself; a field or an index of it reads the
            // pointee into a temporary, so `(*p).field` works (`spec/test-runner.md`, IN2).
            K::PtrDeref(_) => {
                let v = self.eval(e, Mode::Use)?;
                Ok(self.temp(v, &e.ty))
            }
            _ => {
                let v = self.eval(e, Mode::Move)?;
                Ok(self.temp(v, &e.ty))
            }
        }
    }

    fn is_place(e: &Expr) -> bool {
        matches!(e.kind, K::Local(_) | K::Field(..) | K::Index(..) | K::Deref(_) | K::DerefRef(_))
    }

    // ----- expressions ---------------------------------------------------------------------------------

    pub(crate) fn eval(&mut self, e: &'p Expr, mode: Mode) -> R<Value> {
        self.step(e.span)?;
        let span = e.span;
        Ok(match &e.kind {
            K::Unit => Value::Unit,
            K::Bool(b) => Value::Bool(*b),
            K::Int(v) => Value::Int(*v),
            K::Float(v) => Value::Float(match e.ty {
                Ty::Float(tir::FloatTy::F32) => f64::from(*v as f32),
                _ => *v,
            }),
            K::Char(c) => Value::Char(*c),
            K::Str(s) => Value::str(s),
            K::Local(_) | K::Field(..) | K::Index(..) | K::Deref(_) | K::DerefRef(_) => {
                let take = mode == Mode::Move && !self.is_copy(&e.ty);
                // A view of a temporary that a reference produced reads through the reference.
                let p = self.eval_place(e)?;
                let v = self.read_place(&p, take, span)?;
                if v.is_moved() {
                    return Err(Fault::Internal("a moved value was read".into(), span).into());
                }
                v
            }
            K::Const(id) => self.const_value(id.0, span)?,
            K::Call(f, args) => self.call(*f, None, args, span)?,
            K::ExternCall(n, _) => {
                let name = self.p.externs.get(*n).map(|x| x.symbol.clone()).unwrap_or_default();
                return Err(impure(&format!("calling the C function `{name}`"), span));
            }
            K::Builtin(b, args) => self.builtin(*b, args, &e.ty, span)?,
            K::Method(m, args) => self.method(*m, args, &e.ty, span)?,
            K::Closure { closure, captures } => {
                let mut env = Vec::with_capacity(captures.len());
                for c in captures {
                    let place = Expr { kind: K::Local(c.local), ty: c.ty.clone(), span };
                    let v = match c.mode {
                        tir::CaptureMode::Move => {
                            let p = self.local_place(c.local.0);
                            let take = !self.is_copy(&c.ty);
                            self.read_place(&p, take, span)?
                        }
                        _ => {
                            let _ = &place;
                            Value::Ref(self.local_place(c.local.0))
                        }
                    };
                    env.push(v);
                }
                Value::Closure(*closure, env)
            }
            K::ClosureCall { callee, args, kind } => self.closure_call(callee, args, *kind, span)?,
            K::BoxAny { value, vtable } => Value::Any(*vtable, Box::new(self.eval(value, Mode::Move)?)),
            K::AnyCall { recv, slot, args, .. } => {
                let p = self.eval_place(recv)?;
                let vt = match self.place_mut(&p, span)?.clone().deref() {
                    Value::Any(vt, _) => vt,
                    _ => return Err(Fault::Internal("a dynamic call on a value that is not boxed".into(), span).into()),
                };
                let func = self.p.any_vtables[vt].entries[*slot];
                let mut recv_place = p;
                recv_place.path.push(Step::Deref);
                self.call(func, Some(recv_place), args, span)?
            }
            K::AnyIs { value, type_id, .. } => match self.eval(value, Mode::Use)?.deref() {
                Value::Any(vt, _) => Value::Bool(self.p.any_vtables[vt].type_id == *type_id),
                _ => return Err(Fault::Internal("`is` on a value that is not boxed".into(), span).into()),
            },
            K::AnyDowncast { value, type_id, .. } => match self.eval(value, Mode::Move)? {
                Value::Any(vt, inner) if self.p.any_vtables[vt].type_id == *type_id => Value::Opt(Some(inner)),
                other => {
                    self.drop_value(other, &value.ty, span)?;
                    Value::Opt(None)
                }
            },
            K::StringAsStr(inner) => {
                let v = self.eval(inner, Mode::Use)?;
                Value::Str(v.bytes().into())
            }
            K::Block(b) => self.block(b, mode)?,
            K::If(c, then, other) => {
                if self.eval(c, Mode::Use)?.bool() {
                    self.block(then, mode)?
                } else if let Some(o) = other {
                    self.eval(o, mode)?
                } else {
                    Value::Unit
                }
            }
            K::While(c, body) => {
                while self.eval(c, Mode::Use)?.bool() {
                    match self.block(body, Mode::Use) {
                        Ok(_) | Err(Unwind::Continue) => {}
                        Err(Unwind::Break) => break,
                        Err(u) => return Err(u),
                    }
                }
                Value::Unit
            }
            K::Loop(body) => loop {
                match self.block(body, Mode::Use) {
                    Ok(_) | Err(Unwind::Continue) => {}
                    Err(Unwind::Break) => break Value::Unit,
                    Err(u) => return Err(u),
                }
            },
            K::ForRange { var, lo, hi, inclusive, body } => {
                let lo = self.eval(lo, Mode::Use)?.int();
                let hi = self.eval(hi, Mode::Use)?.int();
                let end = if *inclusive { hi } else { hi - 1 };
                self.int_loop(*var, lo, end, body)?;
                Value::Unit
            }
            K::ForRangeValue { var, range, body } => {
                let (lo, hi) = range_bounds(&self.eval(range, Mode::Use)?);
                let inclusive = matches!(range.ty, Ty::Range(_, true));
                let end = if inclusive { hi } else { hi - 1 };
                self.int_loop(*var, lo, end, body)?;
                Value::Unit
            }
            K::ForArray { pat, array: src, body }
            | K::ForSlice { pat, slice: src, body }
            | K::ForList { pat, list: src, body } => {
                let items: Vec<Value> = self.eval(src, Mode::Use)?.elems().to_vec();
                // A loop over a place iterates copies of its elements (the place is borrowed); a loop over a
                // temporary array owns the elements, which are dropped like the array's.
                let item_ty = match &src.ty {
                    Ty::Array(t, _) if !Self::is_place(src) => (**t).clone(),
                    _ => Ty::Unit,
                };
                self.items_loop(pat, items, &item_ty, body, span)?;
                Value::Unit
            }
            K::ForMap { pat, map, body } => {
                let pairs = match self.eval(map, Mode::Use)?.deref() {
                    Value::Map(pairs) => pairs,
                    _ => return Err(Fault::Internal("a map loop over a value that is not a map".into(), span).into()),
                };
                let items = pairs.into_iter().map(|(k, v)| Value::Tuple(vec![k, v])).collect();
                self.items_loop(pat, items, &Ty::Unit, body, span)?;
                Value::Unit
            }
            K::ForChars { pat, string, body } => {
                let v = self.eval(string, Mode::Use)?;
                let bytes = match v.deref() {
                    Value::Chars(b, at) => b[at..].to_vec(),
                    other => other.bytes().to_vec(),
                };
                let items = String::from_utf8_lossy(&bytes).chars().map(Value::Char).collect();
                self.items_loop(pat, items, &Ty::Char, body, span)?;
                Value::Unit
            }
            K::ComptimeFor { pat, iter, body } => {
                let items = self.comptime_items(iter)?;
                let item_ty = match &iter.ty {
                    Ty::Array(t, _) | Ty::List(t, _) | Ty::Range(t, _) => (**t).clone(),
                    other => other.clone(),
                };
                self.items_loop(pat, items, &item_ty, body, span)?;
                Value::Unit
            }
            K::Match(scrutinee, arms) => self.match_expr(scrutinee, arms, mode, span)?,
            K::Break => return Err(Unwind::Break),
            K::Continue => return Err(Unwind::Continue),
            K::Return(v) => {
                let v = match v {
                    Some(v) => self.eval(v, Mode::Move)?,
                    None => Value::Unit,
                };
                return Err(Unwind::Return(v));
            }
            K::Assign(lhs, rhs) => {
                if let K::PtrDeref(ptr) = &lhs.kind {
                    let v = self.eval(rhs, Mode::Move)?;
                    let addr = self.eval(ptr, Mode::Use)?.int();
                    self.ptr_write(addr, &lhs.ty, v, span)?;
                    return Ok(Value::Unit);
                }
                let v = self.eval(rhs, Mode::Move)?;
                let p = self.eval_place(lhs)?;
                let old = std::mem::replace(self.place_mut(&p, span)?, v);
                self.drop_value(old, &lhs.ty, span)?;
                Value::Unit
            }
            K::Binary(op, l, r) => self.binary(*op, l, r, span)?,
            K::Unary(op, inner) => {
                let v = self.eval(inner, Mode::Use)?;
                self.unary(*op, v, &e.ty, span)?
            }
            K::Cast(inner) => {
                let v = self.eval(inner, Mode::Use)?;
                self.cast(v, &inner.ty, &e.ty, span)?
            }
            K::LayoutOf { ty, alignment } => {
                let (size, align) = crate::prims::layout(self.p, ty);
                Value::Int(i128::from(if *alignment { align } else { size }))
            }
            K::Range { lo, hi, .. } => {
                let lo = self.eval(lo, Mode::Use)?;
                let hi = self.eval(hi, Mode::Use)?;
                Value::Tuple(vec![lo, hi])
            }
            K::PtrDeref(ptr) => self.ptr_deref(ptr, &e.ty, span)?,
            K::Ref { place, mutable } => {
                if *mutable {
                    Value::Ref(self.eval_place(place)?)
                } else {
                    Value::Borrow(Box::new(self.eval(place, Mode::Use)?.deref()))
                }
            }
            K::Slice { base, lo, hi, inclusive } => {
                let b = self.eval(base, Mode::Use)?.deref();
                let lo = self.eval(lo, Mode::Use)?.int();
                let hi = self.eval(hi, Mode::Use)?.int();
                self.slice(b, lo, hi, *inclusive, span)?
            }
            K::Array(es) => {
                let mut out = Vec::with_capacity(es.len());
                for x in es {
                    out.push(self.eval(x, Mode::Move)?);
                }
                Value::Array(out)
            }
            K::ArrayRepeat(x, n) => {
                let v = self.eval(x, Mode::Move)?;
                self.charge(*n, span)?;
                Value::Array(vec![v; *n as usize])
            }
            K::Tuple(es) => {
                let mut out = Vec::with_capacity(es.len());
                for x in es {
                    out.push(self.eval(x, Mode::Move)?);
                }
                Value::Tuple(out)
            }
            K::Struct(fields) => {
                let n = match &e.ty {
                    Ty::Struct(a) => self.p.structs[a.id].fields.len(),
                    _ => fields.len(),
                };
                let mut out = vec![Value::Moved; n];
                for (i, x) in fields {
                    out[*i] = self.eval(x, Mode::Move)?;
                }
                Value::Tuple(out)
            }
            K::Variant(v, fields) => {
                let n = match &e.ty {
                    Ty::Enum(a) => self.p.enums[a.id].variants[*v].fields.len(),
                    _ => fields.len(),
                };
                let mut out = vec![Value::Moved; n];
                for (i, x) in fields {
                    out[*i] = self.eval(x, Mode::Move)?;
                }
                Value::Variant(*v, out)
            }
            K::Some(x) => Value::Opt(Some(Box::new(self.eval(x, Mode::Move)?))),
            K::None => Value::Opt(None),
            K::Ok(x) => Value::Res(Ok(Box::new(self.eval(x, Mode::Move)?))),
            K::Err(x) => Value::Res(Err(Box::new(self.eval(x, Mode::Move)?))),
            K::Comptime(x) => self.eval(x, mode)?,
        })
    }

    fn local_place(&self, id: usize) -> Place {
        let frame = self.cur();
        match &self.frames[frame].slots[id] {
            Slot::Alias(p) => p.clone(),
            Slot::Val(_) => Place { frame, root: Root::Local(id), path: Vec::new() },
        }
    }

    fn const_value(&mut self, id: usize, span: Span) -> R<Value> {
        match &self.consts[id] {
            ConstState::Done(v) => return Ok(v.clone()),
            ConstState::Running => return Err(Fault::Cycle(id, span).into()),
            ConstState::Failed => return Err(Fault::Reported.into()),
            ConstState::Pending => {}
        }
        self.consts[id] = ConstState::Running;
        let c = &self.p.consts[id];
        let slots = c.locals.iter().map(|_| Slot::Val(Value::Moved)).collect();
        if self.frames.len() > MAX_DEPTH {
            return Err(depth(span));
        }
        self.frames.push(Frame { slots, locals: &c.locals, temps: Vec::new(), scope: Vec::new() });
        let result = self.eval(&c.value, Mode::Move);
        let result = match result {
            Ok(v) => self.end_temps(0, span).map(|_| v),
            Err(u) => Err(u),
        };
        self.frames.pop();
        match result {
            Ok(v) => {
                self.consts[id] = ConstState::Done(v.clone());
                Ok(v)
            }
            Err(u) => {
                self.consts[id] = ConstState::Failed;
                Err(u)
            }
        }
    }

    pub(crate) fn comptime_items(&mut self, iter: &'p Expr) -> R<Vec<Value>> {
        let v = self.eval(iter, Mode::Move)?;
        Ok(match &iter.ty {
            Ty::Range(_, inclusive) => {
                let (lo, hi) = range_bounds(&v);
                let end = if *inclusive { hi } else { hi - 1 };
                if end < lo {
                    Vec::new()
                } else {
                    self.charge(u64::try_from(end - lo + 1).unwrap_or(u64::MAX), iter.span)?;
                    (lo..=end).map(Value::Int).collect()
                }
            }
            _ => match v {
                Value::Array(items) | Value::List(items) => items,
                other => other.elems().to_vec(),
            },
        })
    }

    fn int_loop(&mut self, var: Option<tir::LocalId>, lo: i128, end: i128, body: &'p Block) -> R<()> {
        let mut i = lo;
        while i <= end {
            let mark = self.frame().scope.len();
            if let Some(var) = var {
                self.frame().slots[var.0] = Slot::Val(Value::Int(i));
            }
            let r = self.block(body, Mode::Use);
            self.end_scope(mark, body_span(body))?;
            match r {
                Ok(_) | Err(Unwind::Continue) => {}
                Err(Unwind::Break) => break,
                Err(u) => return Err(u),
            }
            i += 1;
        }
        Ok(())
    }

    fn items_loop(&mut self, pat: &'p Pat, items: Vec<Value>, ty: &Ty, body: &'p Block, span: Span) -> R<()> {
        for item in items {
            self.step(span)?;
            let mark = self.frame().scope.len();
            let temps = self.frame().temps.len();
            let p = self.temp(item, ty);
            self.bind(pat, &p, true, span)?;
            let r = self.block(body, Mode::Use);
            self.end_scope(mark, span)?;
            self.end_temps(temps, span)?;
            match r {
                Ok(_) | Err(Unwind::Continue) => {}
                Err(Unwind::Break) => break,
                Err(u) => return Err(u),
            }
        }
        Ok(())
    }

    // ----- blocks and statements ------------------------------------------------------------------------

    pub(crate) fn block(&mut self, b: &'p Block, mode: Mode) -> R<Value> {
        let mark = self.frame().scope.len();
        let result = self.block_inner(b, mode);
        match result {
            // A panic aborts: no drops run (DR10).
            Err(Unwind::Fault(f)) => Err(Unwind::Fault(f)),
            other => {
                self.end_scope(mark, body_span(b))?;
                other
            }
        }
    }

    fn block_inner(&mut self, b: &'p Block, mode: Mode) -> R<Value> {
        for s in &b.stmts {
            let temps = self.frame().temps.len();
            self.stmt(s)?;
            self.end_temps(temps, stmt_span(s))?;
        }
        match &b.tail {
            Some(t) => {
                let temps = self.frame().temps.len();
                let v = self.eval(t, mode)?;
                self.end_temps(temps, t.span)?;
                Ok(v)
            }
            None => Ok(Value::Unit),
        }
    }

    /// One statement of a block: a `let`, or an expression whose value is dropped (DR5, the statement's
    /// temporaries are the caller's to end).
    pub(crate) fn stmt(&mut self, s: &'p Stmt) -> R<()> {
        match s {
            Stmt::Let(pat, init, other) => {
                let v = self.eval(init, Mode::Move)?;
                if let Pat::Binding(id, _) = pat {
                    self.frame().slots[id.0] = Slot::Val(v);
                    self.frame().scope.push(id.0);
                } else {
                    let p = self.temp(v, &init.ty);
                    let v = self.place_mut(&p, init.span)?.clone();
                    if pattern_matches(pat, &v) {
                        self.bind(pat, &p, true, init.span)?;
                    } else {
                        let other = other.as_ref().ok_or_else(|| {
                            Unwind::from(Fault::Internal("a refutable `let` without `else`".into(), init.span))
                        })?;
                        self.block(other, Mode::Use)?;
                        return Err(Fault::Internal("the `else` of `let` finished normally".into(), init.span).into());
                    }
                }
                Ok(())
            }
            Stmt::Expr(e) => {
                let v = self.eval(e, Mode::Move)?;
                self.drop_value(v, &e.ty, e.span)?;
                Ok(())
            }
        }
    }

    /// Drops the statement's temporaries from `mark` on, last first (DR5).
    fn end_temps(&mut self, mark: usize, span: Span) -> R<()> {
        while self.frame().temps.len() > mark {
            let (v, ty) = self.frame().temps.pop().expect("checked above");
            self.drop_value(v, &ty, span)?;
        }
        Ok(())
    }

    /// Drops the locals bound since `mark`, last first (DR1).
    fn end_scope(&mut self, mark: usize, span: Span) -> R<()> {
        while self.frame().scope.len() > mark {
            let id = self.frame().scope.pop().expect("checked above");
            let local = &self.frames.last().expect("a frame is open").locals[id];
            if !matches!(local.kind, LocalKind::Owned | LocalKind::OwnParam | LocalKind::Scrutinee) {
                self.frame().slots[id] = Slot::Val(Value::Moved);
                continue;
            }
            let ty = local.ty.clone();
            let v = match std::mem::replace(&mut self.frame().slots[id], Slot::Val(Value::Moved)) {
                Slot::Val(v) => v,
                Slot::Alias(_) => continue,
            };
            self.drop_value(v, &ty, span)?;
        }
        Ok(())
    }

    // ----- patterns ----------------------------------------------------------------------------------

    /// Binds the locals of `pat` from the value at `p`. With `take`, a binding that moves takes its part out of
    /// the place; otherwise every binding copies.
    fn bind(&mut self, pat: &'p Pat, p: &Place, take: bool, span: Span) -> R<()> {
        match pat {
            Pat::Wild | Pat::Bool(_) | Pat::Int(_) | Pat::Char(_) | Pat::Str(_) | Pat::Range(..) | Pat::None => Ok(()),
            Pat::Binding(id, mode) => {
                let v = self.read_place(p, take && *mode == BindMode::Move, span)?;
                self.frame().slots[id.0] = Slot::Val(v);
                self.frame().scope.push(id.0);
                Ok(())
            }
            Pat::At(id, mode, inner) => {
                self.bind(inner, p, take, span)?;
                let v = self.read_place(p, take && *mode == BindMode::Move, span)?;
                self.frame().slots[id.0] = Slot::Val(v);
                self.frame().scope.push(id.0);
                Ok(())
            }
            Pat::Or(alts) => {
                let v = self.place_mut(p, span)?.clone();
                match alts.iter().find(|a| pattern_matches(a, &v)) {
                    Some(a) => self.bind(a, p, take, span),
                    None => Ok(()),
                }
            }
            Pat::Some(inner) | Pat::Ok(inner) | Pat::Err(inner) => {
                let mut q = p.clone();
                q.path.push(Step::Payload);
                self.bind(inner, &q, take, span)
            }
            Pat::Product(subs) | Pat::Variant(_, subs) => {
                for (i, sub) in subs.iter().enumerate() {
                    let mut q = p.clone();
                    q.path.push(Step::Field(i));
                    self.bind(sub, &q, take, span)?;
                }
                Ok(())
            }
        }
    }

    fn match_expr(&mut self, scrutinee: &'p Expr, arms: &'p [tir::Arm], mode: Mode, span: Span) -> R<Value> {
        let p = if Self::is_place(scrutinee) {
            self.eval_place(scrutinee)?
        } else {
            let v = self.eval(scrutinee, Mode::Move)?;
            self.temp(v, &scrutinee.ty)
        };
        let value = self.place_mut(&p, span)?.clone();
        for arm in arms {
            if !pattern_matches(&arm.pat, &value) {
                continue;
            }
            let mark = self.frame().scope.len();
            if let Some(guard) = &arm.guard {
                // The guard sees copies; the bindings move only once the arm is chosen.
                self.bind(&arm.pat, &p, false, span)?;
                let ok = self.eval(guard, Mode::Use)?.bool();
                while self.frame().scope.len() > mark {
                    let id = self.frame().scope.pop().expect("checked above");
                    self.frame().slots[id] = Slot::Val(Value::Moved);
                }
                if !ok {
                    continue;
                }
            }
            self.bind(&arm.pat, &p, true, span)?;
            let r = self.eval(&arm.body, mode);
            if let Err(Unwind::Fault(f)) = r {
                return Err(Unwind::Fault(f));
            }
            self.end_scope(mark, arm.body.span)?;
            return r;
        }
        Err(Fault::Internal("no `match` arm matched".into(), span).into())
    }

    // ----- calls ---------------------------------------------------------------------------------------

    /// Calls `f` with no arguments as a complete evaluation, for a `[differential]` test body
    /// (`spec/test-runner.md`). A `break` or `continue` that leaves the call is an internal error, as it is for a
    /// comptime expression; a `return` is the call's value.
    pub(crate) fn call_root(&mut self, f: FuncId, span: Span) -> Result<Value, Fault> {
        match self.call(f, None, &[], span) {
            Ok(v) => Ok(v),
            Err(Unwind::Fault(fault)) => Err(fault),
            Err(_) => Err(Fault::Internal("control flow left a differential test".into(), span)),
        }
    }

    /// Calls function `f`. `recv` is the place of a receiver passed by address (a dynamic call's box); the
    /// arguments follow the callee's conventions.
    pub(crate) fn call(&mut self, f: FuncId, recv: Option<Place>, args: &'p [Expr], span: Span) -> R<Value> {
        let func = &self.p.functions[f.0];
        if func.symbolic {
            return Err(Fault::Internal(format!("`{}` has no body", func.name), span).into());
        }
        let mut slots: Vec<Slot> = func.locals.iter().map(|_| Slot::Val(Value::Moved)).collect();
        let mut params = func.params.iter();
        if let Some(recv) = recv {
            let first = params.next().expect("a method has a receiver");
            slots[first.0] = Slot::Alias(recv);
        }
        let mut borrowed_temps: Vec<usize> = Vec::new();
        for (param, arg) in params.zip(args) {
            let local = &func.locals[param.0];
            slots[param.0] = if local.by_ref && Self::is_place(arg) {
                Slot::Alias(self.eval_place(arg)?)
            } else {
                if local.by_ref {
                    borrowed_temps.push(param.0);
                }
                let v = self.eval(arg, Mode::Move)?;
                Slot::Val(v)
            };
        }
        let (v, mut slots) = self.run(func.locals.as_slice(), &func.body, slots, span)?;
        // An argument passed by address that was a temporary belongs to the caller's statement (DR5).
        for id in borrowed_temps {
            if let Slot::Val(v) = std::mem::replace(&mut slots[id], Slot::Val(Value::Moved)) {
                let ty = func.locals[id].ty.clone();
                self.frame().temps.push((v, ty));
            }
        }
        Ok(v)
    }

    /// Runs a body in a new frame; returns its value and the final slots. Owned parameters still holding a value
    /// are dropped when it returns (DR3).
    fn run(&mut self, locals: &'p [Local], body: &'p Block, slots: Vec<Slot>, span: Span) -> R<(Value, Vec<Slot>)> {
        if self.frames.len() > MAX_DEPTH {
            return Err(depth(span));
        }
        self.step(span)?;
        let owned: Vec<usize> = (0..locals.len())
            .filter(|&i| locals[i].kind == LocalKind::OwnParam && matches!(slots[i], Slot::Val(ref v) if !v.is_moved()))
            .collect();
        self.frames.push(Frame { slots, locals, temps: Vec::new(), scope: owned });
        let r = match self.block(body, Mode::Move) {
            Ok(v) | Err(Unwind::Return(v)) => Ok(v),
            Err(Unwind::Break) | Err(Unwind::Continue) => {
                Err(Fault::Internal("`break` left a function".into(), span).into())
            }
            Err(u) => Err(u),
        };
        let r = match r {
            Ok(v) => self.end_scope(0, span).map(|_| v),
            Err(u) => Err(u),
        };
        let frame = self.frames.pop().expect("pushed above");
        r.map(|v| (v, frame.slots))
    }

    /// Runs `body`'s statements from item `from` on (the tail included, as one more item) in a fresh root
    /// frame with `env` pre-bound, for the REPL's session (`spec/repl.md`, RP4). Unlike [`Interp::run`] the
    /// top-level scope is not closed afterwards: the session's bindings outlive one input, so their values
    /// leave through the returned slots and nothing is dropped here. Sub-blocks still open, close, and drop as
    /// in any function body. The tail runs like the function's return sees it: evaluated, its value dropped.
    pub(crate) fn session_run(
        &mut self,
        locals: &'p [Local],
        env: &[(tir::LocalId, Value)],
        body: &'p burdin_tir::Block,
        from: usize,
        span: Span,
    ) -> R<Vec<Slot>> {
        if self.frames.len() > MAX_DEPTH {
            return Err(depth(span));
        }
        let mut slots: Vec<Slot> = locals.iter().map(|_| Slot::Val(Value::Moved)).collect();
        for (id, v) in env {
            slots[id.0] = Slot::Val(v.clone());
        }
        self.frames.push(Frame { slots, locals, temps: Vec::new(), scope: Vec::new() });
        let items = body.stmts.len() + usize::from(body.tail.is_some());
        let mut result = Ok(());
        for s in body.stmts.iter().skip(from.min(body.stmts.len())) {
            let temps = self.frame().temps.len();
            result = self.stmt(s).and_then(|_| self.end_temps(temps, stmt_span(s)));
            if result.is_err() {
                break;
            }
        }
        if result.is_ok()
            && from < items
            && let Some(tail) = &body.tail
        {
            let temps = self.frame().temps.len();
            result = self
                .eval(tail, Mode::Move)
                .and_then(|v| self.drop_value(v, &tail.ty, tail.span).and_then(|_| self.end_temps(temps, tail.span)));
        }
        // A fault unwinds: no drops run (DR10). A return or a loop jump leaving the statements is the caller's
        // to report; the slots still move out so partial reads cannot dangle.
        let frame = self.frames.pop().expect("pushed above");
        result.map(|_| frame.slots)
    }

    fn closure_call(&mut self, callee: &'p Expr, args: &'p [Expr], kind: tir::FnKind, span: Span) -> R<Value> {
        let (index, callee_place, env) = if kind == tir::FnKind::FnOnce {
            match self.eval(callee, Mode::Move)? {
                Value::Closure(i, env) => (i, None, Some(env)),
                _ => return Err(Fault::Internal("calling a value that is not a closure".into(), span).into()),
            }
        } else {
            let p = self.eval_place(callee)?;
            match self.place_mut(&p, span)?.clone().deref() {
                Value::Closure(i, _) => (i, Some(p), None),
                _ => return Err(Fault::Internal("calling a value that is not a closure".into(), span).into()),
            }
        };
        let clos = &self.p.closures[index];
        let func = &clos.func;
        let mut slots: Vec<Slot> = func.locals.iter().map(|_| Slot::Val(Value::Moved)).collect();
        let mut env = env;
        for (j, cap) in clos.captures.iter().enumerate() {
            slots[cap.local.0] = match (&mut env, &callee_place) {
                // A consuming call owns the record: a borrow capture still aliases the captured place, and a
                // move capture becomes the body's own value, dropped when the body ends.
                (Some(env), _) => match std::mem::replace(&mut env[j], Value::Moved) {
                    Value::Ref(target) => Slot::Alias(target),
                    v => Slot::Val(v),
                },
                // A call through a place keeps the record: every capture is a place, the captured one or the
                // record's own field, so the body's writes persist between calls.
                (None, Some(p)) => {
                    let mut q = p.clone();
                    q.path.push(Step::Env(j));
                    match self.place_mut(&q, span)?.clone() {
                        Value::Ref(target) => Slot::Alias(target),
                        _ => Slot::Alias(q),
                    }
                }
                (None, None) => Slot::Val(Value::Moved),
            };
        }
        let params: Vec<usize> =
            func.params.iter().map(|p| p.0).filter(|p| !clos.captures.iter().any(|c| c.local.0 == *p)).collect();
        for (param, arg) in params.into_iter().zip(args) {
            slots[param] = Slot::Val(self.eval(arg, Mode::Move)?);
        }
        let (v, _) = self.run(func.locals.as_slice(), &func.body, slots, span)?;
        Ok(v)
    }

    // ----- drops --------------------------------------------------------------------------------------

    /// Whether dropping a value of `ty` can run a user `Drop` implementation; other drops only free memory,
    /// which the interpreter does by forgetting the value.
    fn has_user_drop(&mut self, ty: &Ty) -> bool {
        if let Some(&known) = self.drop_cache.get(ty) {
            return known;
        }
        self.drop_cache.insert(ty.clone(), false);
        let result = match ty {
            Ty::Struct(a) => {
                let s = &self.p.structs[a.id];
                s.drop_method.is_some() || s.fields.iter().any(|f| self.has_user_drop(&f.ty))
            }
            Ty::Enum(a) => {
                let e = &self.p.enums[a.id];
                e.drop_method.is_some() || e.variants.iter().any(|v| v.fields.iter().any(|f| self.has_user_drop(&f.ty)))
            }
            Ty::Tuple(ts) => ts.iter().any(|t| self.has_user_drop(t)),
            Ty::Array(t, _)
            | Ty::List(t, _)
            | Ty::Option(t)
            | Ty::Heap(t, _)
            | Ty::ListIntoIter(t, _)
            | Ty::ArrayIntoIter(t, _) => self.has_user_drop(t),
            Ty::Result(a, b) | Ty::Map(a, b, _) | Ty::MapIntoIter(a, b, _) => {
                self.has_user_drop(a) || self.has_user_drop(b)
            }
            Ty::Any(_) | Ty::Fn { .. } => true,
            _ => false,
        };
        self.drop_cache.insert(ty.clone(), result);
        result
    }

    /// Drops an owned value of `ty`: its `Drop` implementation first, then its parts (DR8).
    pub(crate) fn drop_value(&mut self, v: Value, ty: &Ty, span: Span) -> R<()> {
        if v.is_moved() || !self.has_user_drop(ty) {
            return Ok(());
        }
        match (ty, v) {
            (Ty::Struct(a), v) => {
                let mut v = v;
                if let Some(f) = self.p.structs[a.id].drop_method {
                    v = self.call_drop(f, v, span)?;
                }
                if let Value::Tuple(fields) = v {
                    let tys: Vec<Ty> = self.p.structs[a.id].fields.iter().map(|f| f.ty.clone()).collect();
                    for (fv, ft) in fields.into_iter().zip(tys) {
                        self.drop_value(fv, &ft, span)?;
                    }
                }
            }
            (Ty::Enum(a), v) => {
                let mut v = v;
                if let Some(f) = self.p.enums[a.id].drop_method {
                    v = self.call_drop(f, v, span)?;
                }
                if let Value::Variant(i, fields) = v {
                    let tys: Vec<Ty> = self.p.enums[a.id].variants[i].fields.iter().map(|f| f.ty.clone()).collect();
                    for (fv, ft) in fields.into_iter().zip(tys) {
                        self.drop_value(fv, &ft, span)?;
                    }
                }
            }
            (Ty::Tuple(ts), Value::Tuple(vs)) => {
                for (fv, ft) in vs.into_iter().zip(ts) {
                    self.drop_value(fv, ft, span)?;
                }
            }
            (Ty::Array(t, _) | Ty::List(t, _), Value::Array(vs) | Value::List(vs)) => {
                for fv in vs {
                    self.drop_value(fv, t, span)?;
                }
            }
            (Ty::ListIntoIter(t, _) | Ty::ArrayIntoIter(t, _), Value::Items(vs)) => {
                for fv in vs {
                    self.drop_value(fv, t, span)?;
                }
            }
            (Ty::Option(t) | Ty::Heap(t, _), Value::Opt(Some(b)) | Value::Heap(b)) => self.drop_value(*b, t, span)?,
            (Ty::Result(t, _), Value::Res(Ok(b))) | (Ty::Result(_, t), Value::Res(Err(b))) => {
                self.drop_value(*b, t, span)?
            }
            (Ty::Map(k, val, _), Value::Map(pairs)) => {
                for (kv, vv) in pairs {
                    self.drop_value(kv, k, span)?;
                    self.drop_value(vv, val, span)?;
                }
            }
            (Ty::MapIntoIter(k, val, _), Value::Items(pairs)) => {
                for pair in pairs {
                    if let Value::Tuple(mut kv) = pair {
                        let vv = kv.pop().unwrap_or(Value::Moved);
                        let key = kv.pop().unwrap_or(Value::Moved);
                        self.drop_value(key, k, span)?;
                        self.drop_value(vv, val, span)?;
                    }
                }
            }
            (Ty::Any(_), Value::Any(vt, inner)) => {
                let concrete = self.p.any_vtables[vt].concrete.clone();
                self.drop_value(*inner, &concrete, span)?;
            }
            (Ty::Fn { .. }, Value::Closure(i, env)) => {
                let caps: Vec<(tir::CaptureMode, Ty)> =
                    self.p.closures[i].captures.iter().map(|c| (c.mode, c.ty.clone())).collect();
                for (v, (mode, t)) in env.into_iter().zip(caps) {
                    if mode == tir::CaptureMode::Move {
                        self.drop_value(v, &t, span)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Runs a `Drop` implementation on `v` and returns what is left of it for the field drops.
    fn call_drop(&mut self, f: FuncId, v: Value, span: Span) -> R<Value> {
        let func = &self.p.functions[f.0];
        let mut slots: Vec<Slot> = func.locals.iter().map(|_| Slot::Val(Value::Moved)).collect();
        let this = func.params[0].0;
        slots[this] = Slot::Val(v);
        let (_, mut slots) = self.run(func.locals.as_slice(), &func.body, slots, span)?;
        Ok(match std::mem::replace(&mut slots[this], Slot::Val(Value::Moved)) {
            Slot::Val(v) => v,
            Slot::Alias(_) => Value::Moved,
        })
    }
}

/// One step of a place path into a value. A shared reference on the way is looked through.
fn step_mut(v: &mut Value, step: Step, span: Span) -> R<&mut Value> {
    let v = match v {
        Value::Borrow(inner) => &mut **inner,
        other => other,
    };
    // Check the shape first, so the borrow below is only taken when it succeeds.
    let len = match (step, &*v) {
        (Step::Field(i), Value::Tuple(fs) | Value::Variant(_, fs)) if i < fs.len() => None,
        (Step::Index(_), Value::Array(xs) | Value::List(xs)) => Some(xs.len()),
        (Step::Index(_), Value::Slice(_, lo, hi)) => Some(hi - lo),
        (Step::Deref, Value::Heap(_) | Value::Any(..)) => None,
        (Step::Payload, Value::Opt(Some(_)) | Value::Res(_)) => None,
        (Step::Env(i), Value::Closure(_, env)) if i < env.len() => None,
        (step, v) => return Err(Fault::Internal(format!("cannot follow {step:?} into {v:?}"), span).into()),
    };
    if let (Step::Index(i), Some(len)) = (step, len)
        && i >= len
    {
        return Err(out_of_bounds(i, len, span));
    }
    Ok(match (step, v) {
        (Step::Field(i), Value::Tuple(fs) | Value::Variant(_, fs)) => &mut fs[i],
        (Step::Index(i), Value::Array(xs) | Value::List(xs)) => &mut xs[i],
        (Step::Index(i), Value::Slice(xs, lo, _)) => {
            let at = *lo + i;
            &mut std::rc::Rc::make_mut(xs)[at]
        }
        (Step::Deref, Value::Heap(b) | Value::Any(_, b)) => &mut **b,
        (Step::Payload, Value::Opt(Some(b)) | Value::Res(Ok(b) | Err(b))) => &mut **b,
        (Step::Env(i), Value::Closure(_, env)) => &mut env[i],
        _ => unreachable!("checked above"),
    })
}

fn out_of_bounds(i: usize, len: usize, span: Span) -> Unwind {
    crate::prims::bounds(i as i128, len, span)
}

pub(crate) fn impure(what: &str, span: Span) -> Unwind {
    Fault::Impure(format!("{what} is not allowed at compile time: comptime is pure (spec/comptime.md, CT9)"), span)
        .into()
}

fn depth(span: Span) -> Unwind {
    Fault::Budget(format!("compile-time evaluation nested more than {MAX_DEPTH} calls"), span).into()
}

pub(crate) fn range_bounds(v: &Value) -> (i128, i128) {
    match v {
        Value::Tuple(ends) if ends.len() == 2 => (ends[0].int(), ends[1].int()),
        Value::Borrow(inner) => range_bounds(inner),
        _ => (0, -1),
    }
}

fn body_span(b: &Block) -> Span {
    match (&b.tail, b.stmts.last()) {
        (Some(t), _) => t.span,
        (None, Some(s)) => stmt_span(s),
        (None, None) => Span::new(burdin_diag::FileId(0), 0, 0),
    }
}

fn stmt_span(s: &Stmt) -> Span {
    match s {
        Stmt::Let(_, init, _) => init.span,
        Stmt::Expr(e) => e.span,
    }
}

/// Whether `v` matches `pat`, without binding anything.
pub(crate) fn pattern_matches(pat: &Pat, v: &Value) -> bool {
    let v = match v {
        Value::Borrow(inner) => &**inner,
        other => other,
    };
    match pat {
        Pat::Wild | Pat::Binding(..) => true,
        Pat::At(_, _, inner) => pattern_matches(inner, v),
        Pat::Bool(b) => matches!(v, Value::Bool(x) if x == b),
        Pat::Int(i) => matches!(v, Value::Int(x) if x == i),
        Pat::Char(c) => matches!(v, Value::Char(x) if x == c),
        Pat::Str(s) => matches!(v, Value::Str(_) | Value::String(_)) && v.bytes() == s.as_bytes(),
        Pat::Range(lo, hi) => {
            let x = match v {
                Value::Int(x) => *x,
                Value::Char(c) => i128::from(*c as u32),
                _ => return false,
            };
            *lo <= x && x <= *hi
        }
        Pat::Or(alts) => alts.iter().any(|a| pattern_matches(a, v)),
        Pat::Some(inner) => matches!(v, Value::Opt(Some(x)) if pattern_matches(inner, x)),
        Pat::None => matches!(v, Value::Opt(None)),
        Pat::Ok(inner) => matches!(v, Value::Res(Ok(x)) if pattern_matches(inner, x)),
        Pat::Err(inner) => matches!(v, Value::Res(Err(x)) if pattern_matches(inner, x)),
        Pat::Product(subs) => match v {
            Value::Tuple(fs) => subs.iter().zip(fs).all(|(p, f)| pattern_matches(p, f)),
            _ => false,
        },
        Pat::Variant(i, subs) => match v {
            Value::Variant(j, fs) => i == j && subs.iter().zip(fs).all(|(p, f)| pattern_matches(p, f)),
            _ => false,
        },
    }
}
