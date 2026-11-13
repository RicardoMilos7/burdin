//! Name resolution and type checking. Produces the typed tree in [`tir`].
//!
//! Milestone 2 scope: functions, constants, the primitive types, arrays, arithmetic, bindings and assignment, control
//! flow (`if`, `while`, `loop`, `for` over ranges, `match` on primitives and `Option`), the built-in methods of
//! numbers, and the output and panic built-ins. Everything else is reported with `Code::UNSUPPORTED` until its
//! milestone.

pub mod tir;

use burdin_diag::{Code, Diagnostics, FileId, Span};
use burdin_syntax::ast;
use std::collections::HashMap;
use tir::{BinOp, Builtin, ConstId, FloatTy, FuncId, IntTy, LocalId, Method, Pat, Ty, UnOp};

struct Signature {
    params: Vec<Ty>,
    ret: Ty,
}

enum Callee {
    User(FuncId),
    Builtin(Builtin),
}

#[derive(Clone, Copy)]
enum Global {
    Func(FuncId),
    Const(ConstId),
}

struct Globals {
    names: HashMap<String, Global>,
    sigs: Vec<Signature>,
    const_tys: Vec<Ty>,
}

/// Checks a parsed file. Returns `None` if any error was reported (here or earlier).
pub fn check(file: FileId, ast: &ast::File, diags: &mut Diagnostics) -> Option<tir::Program> {
    let mut globals = Globals { names: HashMap::new(), sigs: Vec::new(), const_tys: Vec::new() };
    let mut fn_decls = Vec::new();
    let mut const_decls = Vec::new();

    for item in &ast.items {
        let name = match item {
            ast::Item::Fn(f) => &f.name,
            ast::Item::Const(c) => &c.name,
        };
        if globals.names.contains_key(&name.name) || builtin(&name.name).is_some() {
            diags.error(Code::UNSUPPORTED, name.span, format!("`{}` is defined more than once", name.name));
            continue;
        }
        match item {
            ast::Item::Fn(f) => {
                let sig = fn_signature(f, diags);
                globals.names.insert(f.name.name.clone(), Global::Func(FuncId(fn_decls.len())));
                globals.sigs.push(sig);
                fn_decls.push(f);
            }
            ast::Item::Const(c) => {
                let ty = resolve_type(&c.ty, diags);
                globals.names.insert(c.name.name.clone(), Global::Const(ConstId(const_decls.len())));
                globals.const_tys.push(ty);
                const_decls.push(c);
            }
        }
    }

    let main = match globals.names.get("main") {
        Some(&Global::Func(id)) => {
            let f = fn_decls[id.0];
            if !f.params.is_empty() || globals.sigs[id.0].ret != Ty::Unit {
                diags.error(Code::TYPE_MISMATCH, f.name.span, "`main` must take no parameters and return nothing");
            }
            Some(id)
        }
        _ => {
            diags.error(Code::UNKNOWN_NAME, Span::new(file, 0, 0), "the program has no `main` function");
            None
        }
    };

    let mut consts = Vec::new();
    for (i, c) in const_decls.iter().enumerate() {
        let ty = globals.const_tys[i].clone();
        let mut cx = FnCx::new(&globals, diags, Ty::Unit);
        let value = cx.expr(&c.value, Some(&ty));
        let value = cx.expect_ty(value, &ty);
        if !is_const_expr(&value) {
            cx.diags.error(
                Code::UNSUPPORTED,
                c.value.span,
                "only literals, other constants, operators, and casts are allowed in a constant for now",
            );
        }
        consts.push(tir::Const { name: c.name.name.clone(), value });
    }
    check_const_cycles(&consts, &const_decls, diags);

    let mut functions = Vec::new();
    for (i, f) in fn_decls.iter().enumerate() {
        let ret = globals.sigs[i].ret.clone();
        let mut cx = FnCx::new(&globals, diags, ret.clone());
        let mut params = Vec::new();
        for (p, ty) in f.params.iter().zip(&globals.sigs[i].params) {
            if let ast::ParamKind::Named { name, .. } = &p.kind {
                params.push(cx.declare(&name.name, ty.clone(), false));
            }
        }
        let (body, body_ty) = cx.block(&f.body, Some(&ret));
        if !compatible(&body_ty, &ret) {
            let span = match f.body.stmts.last() {
                Some(ast::Stmt::Expr(e)) => e.span,
                Some(ast::Stmt::Let { span, .. }) => *span,
                None => f.body.span,
            };
            cx.diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`{}` returns `{ret}`, but its body produces `{body_ty}`", f.name.name),
            );
        }
        let locals = cx.locals;
        functions.push(tir::Function { name: f.name.name.clone(), params, ret, locals, body });
    }

    if diags.has_errors() {
        return None;
    }
    Some(tir::Program { functions, consts, main: main? })
}

fn fn_signature(f: &ast::FnDecl, diags: &mut Diagnostics) -> Signature {
    if !f.generics.is_empty() || !f.where_clause.is_empty() {
        diags.error(Code::UNSUPPORTED, f.name.span, "generic functions are not implemented in this compiler yet");
    }
    if f.is_unsafe {
        diags.error(Code::UNSUPPORTED, f.name.span, "`unsafe fn` is not implemented in this compiler yet");
    }
    let mut params = Vec::new();
    for p in &f.params {
        if p.convention != ast::Convention::Read {
            diags.error(
                Code::UNSUPPORTED,
                p.span,
                "`mut` and `own` parameters are not implemented in this compiler yet",
            );
        }
        match &p.kind {
            ast::ParamKind::SelfParam => {
                diags.error(Code::UNSUPPORTED, p.span, "`self` parameters are not implemented in this compiler yet");
            }
            ast::ParamKind::Named { ty, .. } => params.push(resolve_type(ty, diags)),
        }
    }
    let ret = f.ret.as_ref().map(|t| resolve_type(t, diags)).unwrap_or(Ty::Unit);
    Signature { params, ret }
}

/// A constant is expanded at every use, so it must not refer to itself, directly or through other constants.
fn check_const_cycles(consts: &[tir::Const], decls: &[&ast::ConstDecl], diags: &mut Diagnostics) {
    fn refs(e: &tir::Expr, out: &mut Vec<ConstId>) {
        use tir::ExprKind as K;
        match &e.kind {
            K::Const(id) => out.push(*id),
            K::Unary(_, a) | K::Cast(a) => refs(a, out),
            K::Binary(_, a, b) => {
                refs(a, out);
                refs(b, out);
            }
            K::If(c, then, other) => {
                refs(c, out);
                if let Some(t) = &then.tail {
                    refs(t, out);
                }
                if let Some(o) = other {
                    refs(o, out);
                }
            }
            K::Block(b) => {
                if let Some(t) = &b.tail {
                    refs(t, out);
                }
            }
            _ => {}
        }
    }
    let deps: Vec<Vec<ConstId>> = consts
        .iter()
        .map(|c| {
            let mut v = Vec::new();
            refs(&c.value, &mut v);
            v
        })
        .collect();
    for i in 0..consts.len() {
        if deps[i].iter().any(|d| reaches(d.0, i, &deps)) {
            diags.error(
                Code::UNSUPPORTED,
                decls[i].name.span,
                format!("constant `{}` depends on itself", decls[i].name.name),
            );
        }
    }
    fn reaches(from: usize, to: usize, deps: &[Vec<ConstId>]) -> bool {
        let mut seen = vec![false; deps.len()];
        let mut stack = vec![from];
        while let Some(n) = stack.pop() {
            if n == to {
                return true;
            }
            if !std::mem::replace(&mut seen[n], true) {
                stack.extend(deps[n].iter().map(|d| d.0));
            }
        }
        false
    }
}

fn is_const_expr(e: &tir::Expr) -> bool {
    use tir::ExprKind as K;
    match &e.kind {
        K::Unit | K::Bool(_) | K::Int(_) | K::Float(_) | K::Char(_) | K::Str(_) | K::Const(_) => true,
        K::Unary(_, a) | K::Cast(a) => is_const_expr(a),
        K::Binary(_, a, b) => is_const_expr(a) && is_const_expr(b),
        _ => e.ty == Ty::Error,
    }
}

fn builtin(name: &str) -> Option<Builtin> {
    Some(match name {
        "print" => Builtin::Print,
        "println" => Builtin::Println,
        "eprint" => Builtin::Eprint,
        "eprintln" => Builtin::Eprintln,
        "panic" => Builtin::Panic,
        _ => return None,
    })
}

fn builtin_sig(b: Builtin) -> Signature {
    let ret = if b == Builtin::Panic { Ty::Never } else { Ty::Unit };
    Signature { params: vec![Ty::Str], ret }
}

fn int_ty(name: &str) -> Option<IntTy> {
    Some(match name {
        "i8" => IntTy::I8,
        "i16" => IntTy::I16,
        "i32" => IntTy::I32,
        "i64" => IntTy::I64,
        "isize" => IntTy::Isize,
        "u8" => IntTy::U8,
        "u16" => IntTy::U16,
        "u32" => IntTy::U32,
        "u64" => IntTy::U64,
        "usize" => IntTy::Usize,
        _ => return None,
    })
}

fn resolve_type(ty: &ast::Type, diags: &mut Diagnostics) -> Ty {
    match &ty.kind {
        ast::TypeKind::Unit => Ty::Unit,
        ast::TypeKind::Never => Ty::Never,
        ast::TypeKind::Path(segments) if segments.len() == 1 => {
            let seg = &segments[0];
            let name = seg.name.name.as_str();
            if name == "Option" && seg.args.len() == 1 {
                return Ty::Option(Box::new(resolve_type(&seg.args[0], diags)));
            }
            if !seg.args.is_empty() {
                diags.error(Code::UNSUPPORTED, ty.span, "generic types are not implemented in this compiler yet");
                return Ty::Error;
            }
            if let Some(i) = int_ty(name) {
                return Ty::Int(i);
            }
            match name {
                "str" => Ty::Str,
                "bool" => Ty::Bool,
                "char" => Ty::Char,
                "f32" => Ty::Float(FloatTy::F32),
                "f64" => Ty::Float(FloatTy::F64),
                "String" => Ty::String,
                _ => {
                    diags.error(Code::UNKNOWN_NAME, ty.span, format!("unknown type `{name}`"));
                    Ty::Error
                }
            }
        }
        ast::TypeKind::Array(elem, len) => {
            let elem = resolve_type(elem, diags);
            match len.kind {
                ast::ExprKind::Int(n) if n <= u128::from(u32::MAX) => Ty::Array(Box::new(elem), n as u64),
                _ => {
                    diags.error(
                        Code::UNSUPPORTED,
                        len.span,
                        "array lengths other than integer literals are not implemented in this compiler yet",
                    );
                    Ty::Error
                }
            }
        }
        _ => {
            diags.error(Code::UNSUPPORTED, ty.span, "this type is not implemented in this compiler yet");
            Ty::Error
        }
    }
}

fn compatible(actual: &Ty, expected: &Ty) -> bool {
    actual == expected || *actual == Ty::Error || *expected == Ty::Error || *actual == Ty::Never
}

/// An expression whose type comes only from its context: a literal, or operators applied to literals.
fn is_untyped_literal(e: &ast::Expr) -> bool {
    use ast::ExprKind as K;
    match &e.kind {
        K::Int(_) | K::Float(_) => true,
        K::Unary(ast::UnOp::Neg | ast::UnOp::Not, a) => is_untyped_literal(a),
        K::Binary(op, a, b) => is_arith(*op) && is_untyped_literal(a) && is_untyped_literal(b),
        _ => false,
    }
}

fn is_arith(op: ast::BinOp) -> bool {
    use ast::BinOp as B;
    matches!(op, B::Add | B::Sub | B::Mul | B::Div | B::Rem | B::BitAnd | B::BitOr | B::BitXor)
}

fn bin_op(op: ast::BinOp) -> BinOp {
    use ast::BinOp as B;
    match op {
        B::Or => BinOp::Or,
        B::And => BinOp::And,
        B::Eq => BinOp::Eq,
        B::Ne => BinOp::Ne,
        B::Lt => BinOp::Lt,
        B::Gt => BinOp::Gt,
        B::Le => BinOp::Le,
        B::Ge => BinOp::Ge,
        B::BitOr => BinOp::BitOr,
        B::BitXor => BinOp::BitXor,
        B::BitAnd => BinOp::BitAnd,
        B::Shl => BinOp::Shl,
        B::Shr => BinOp::Shr,
        B::Add => BinOp::Add,
        B::Sub => BinOp::Sub,
        B::Mul => BinOp::Mul,
        B::Div => BinOp::Div,
        B::Rem => BinOp::Rem,
    }
}

fn can_cast(from: &Ty, to: &Ty) -> bool {
    let scalar = |t: &Ty| matches!(t, Ty::Int(_) | Ty::Float(_));
    match (from, to) {
        (Ty::Error | Ty::Never, _) | (_, Ty::Error) => true,
        _ if from == to => true,
        (f, t) if scalar(f) && scalar(t) => true,
        (Ty::Bool | Ty::Char, Ty::Int(_)) => true,
        (Ty::Int(IntTy::U8), Ty::Char) => true,
        _ => false,
    }
}

fn method(name: &str) -> Option<Method> {
    Some(match name {
        "to_string" => Method::ToString,
        "wrapping_add" => Method::WrappingAdd,
        "wrapping_sub" => Method::WrappingSub,
        "wrapping_mul" => Method::WrappingMul,
        "checked_add" => Method::CheckedAdd,
        "checked_sub" => Method::CheckedSub,
        "checked_mul" => Method::CheckedMul,
        "min" => Method::Min,
        "max" => Method::Max,
        "abs" => Method::Abs,
        _ => return None,
    })
}

/// The parameter types (after the receiver) and the result type of a built-in method, if `recv` has it.
fn method_sig(m: Method, recv: &Ty) -> Option<(Vec<Ty>, Ty)> {
    match (m, recv) {
        (Method::ToString, Ty::Int(_) | Ty::Bool | Ty::Char | Ty::Str) => Some((vec![], Ty::String)),
        (Method::WrappingAdd | Method::WrappingSub | Method::WrappingMul | Method::Min | Method::Max, Ty::Int(_)) => {
            Some((vec![recv.clone()], recv.clone()))
        }
        (Method::CheckedAdd | Method::CheckedSub | Method::CheckedMul, Ty::Int(_)) => {
            Some((vec![recv.clone()], Ty::Option(Box::new(recv.clone()))))
        }
        (Method::Abs, Ty::Int(i)) if i.signed() => Some((vec![], recv.clone())),
        _ => None,
    }
}

fn place_root(e: &tir::Expr) -> Option<LocalId> {
    match &e.kind {
        tir::ExprKind::Local(id) => Some(*id),
        tir::ExprKind::Index(base, _) => place_root(base),
        _ => None,
    }
}

fn irrefutable(p: &Pat) -> bool {
    match p {
        Pat::Wild | Pat::Binding(_) => true,
        Pat::At(_, p) => irrefutable(p),
        Pat::Or(alts) => alts.iter().any(irrefutable),
        _ => false,
    }
}

/// Whether the patterns together match every value of `ty`.
fn exhaustive(ty: &Ty, pats: &[&Pat]) -> bool {
    fn flatten<'p>(p: &'p Pat, out: &mut Vec<&'p Pat>) {
        match p {
            Pat::Or(alts) => alts.iter().for_each(|a| flatten(a, out)),
            Pat::At(_, inner) => flatten(inner, out),
            _ => out.push(p),
        }
    }
    let mut flat = Vec::new();
    for p in pats {
        flatten(p, &mut flat);
    }
    if flat.iter().any(|p| irrefutable(p)) {
        return true;
    }
    match ty {
        Ty::Error | Ty::Never => true,
        Ty::Bool => {
            flat.iter().any(|p| matches!(p, Pat::Bool(true))) && flat.iter().any(|p| matches!(p, Pat::Bool(false)))
        }
        Ty::Option(inner) => {
            let somes: Vec<&Pat> =
                flat.iter().filter_map(|p| if let Pat::Some(s) = p { Some(&**s) } else { None }).collect();
            flat.iter().any(|p| matches!(p, Pat::None)) && !somes.is_empty() && exhaustive(inner, &somes)
        }
        Ty::Int(_) | Ty::Char => {
            let (min, max) = match ty {
                Ty::Int(i) => (i.min(), i.max()),
                _ => (0, 0x10FFFF),
            };
            let mut ranges: Vec<(i128, i128)> = flat
                .iter()
                .filter_map(|p| match p {
                    Pat::Int(v) => Some((*v, *v)),
                    Pat::Char(c) => Some((*c as i128, *c as i128)),
                    Pat::Range(lo, hi) => Some((*lo, *hi)),
                    _ => None,
                })
                .collect();
            ranges.sort();
            let mut next = min;
            for (lo, hi) in ranges {
                if lo > next {
                    return false;
                }
                next = next.max(hi + 1);
            }
            next > max
        }
        _ => false,
    }
}

struct FnCx<'a> {
    globals: &'a Globals,
    diags: &'a mut Diagnostics,
    locals: Vec<tir::Local>,
    scopes: Vec<HashMap<String, LocalId>>,
    ret: Ty,
    /// One entry per enclosing loop: whether it contains a `break`.
    loops: Vec<bool>,
}

fn mk(kind: tir::ExprKind, ty: Ty, span: Span) -> tir::Expr {
    tir::Expr { kind, ty, span }
}

fn error_expr(span: Span) -> tir::Expr {
    mk(tir::ExprKind::Unit, Ty::Error, span)
}

impl<'a> FnCx<'a> {
    fn new(globals: &'a Globals, diags: &'a mut Diagnostics, ret: Ty) -> Self {
        FnCx { globals, diags, locals: Vec::new(), scopes: vec![HashMap::new()], ret, loops: Vec::new() }
    }

    fn declare(&mut self, name: &str, ty: Ty, mutable: bool) -> LocalId {
        let id = LocalId(self.locals.len());
        self.locals.push(tir::Local { name: name.to_string(), ty, mutable });
        self.scopes.last_mut().expect("a scope is open").insert(name.to_string(), id);
        id
    }

    fn lookup(&self, name: &str) -> Option<LocalId> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    fn unsupported(&mut self, span: Span, what: &str) -> tir::Expr {
        self.diags.error(Code::UNSUPPORTED, span, format!("{what} is not implemented in this compiler yet"));
        error_expr(span)
    }

    fn mismatch(&mut self, span: Span, expected: &Ty, found: &Ty) {
        self.diags.error(Code::TYPE_MISMATCH, span, format!("expected `{expected}`, found `{found}`"));
    }

    /// Reports E0003 unless `e` has type `want` (or a type that converts to nothing at run time, like `!`).
    fn expect_ty(&mut self, e: tir::Expr, want: &Ty) -> tir::Expr {
        if !compatible(&e.ty, want) {
            self.mismatch(e.span, want, &e.ty);
        }
        e
    }

    /// Like `expect_ty`, but a `String` argument is borrowed where a `str` is expected.
    fn coerce_arg(&mut self, e: tir::Expr, want: &Ty) -> tir::Expr {
        if e.ty == Ty::String && *want == Ty::Str {
            let span = e.span;
            return mk(tir::ExprKind::StringAsStr(Box::new(e)), Ty::Str, span);
        }
        self.expect_ty(e, want)
    }

    // ----- blocks and statements ---------------------------------------------

    fn block(&mut self, b: &ast::Block, expect: Option<&Ty>) -> (tir::Block, Ty) {
        self.scopes.push(HashMap::new());
        let mut stmts = Vec::new();
        let mut tail = None;
        let mut diverges = false;
        for (i, s) in b.stmts.iter().enumerate() {
            let last = i + 1 == b.stmts.len();
            match s {
                ast::Stmt::Let { pattern, ty, init, else_block, span } => {
                    let declared = ty.as_ref().map(|t| resolve_type(t, self.diags));
                    let init = self.expr(init, declared.as_ref());
                    let init = match &declared {
                        Some(d) => self.expect_ty(init, d),
                        None => init,
                    };
                    diverges |= init.ty == Ty::Never;
                    if else_block.is_some() {
                        self.unsupported(*span, "`let ... else`");
                        continue;
                    }
                    let local_ty = declared.unwrap_or_else(|| init.ty.clone());
                    match &pattern.kind {
                        ast::PatternKind::Binding { name, mutable } => {
                            let id = self.declare(&name.name, local_ty, *mutable);
                            stmts.push(tir::Stmt::Let(id, init));
                        }
                        ast::PatternKind::Wild => stmts.push(tir::Stmt::Expr(init)),
                        _ => {
                            self.unsupported(pattern.span, "this `let` pattern");
                        }
                    }
                }
                ast::Stmt::Expr(e) => {
                    let e = self.expr(e, if last { expect } else { None });
                    diverges |= e.ty == Ty::Never;
                    if last {
                        tail = Some(Box::new(e));
                    } else {
                        stmts.push(tir::Stmt::Expr(e));
                    }
                }
            }
        }
        self.scopes.pop();
        let ty = match &tail {
            Some(e) if !(diverges && e.ty == Ty::Unit) => e.ty.clone(),
            _ if diverges => Ty::Never,
            _ => Ty::Unit,
        };
        (tir::Block { stmts, tail }, ty)
    }

    // ----- expressions -------------------------------------------------------

    fn expr(&mut self, e: &ast::Expr, expect: Option<&Ty>) -> tir::Expr {
        use ast::ExprKind as K;
        let span = e.span;
        match &e.kind {
            K::Int(v) => self.int_literal(*v, false, expect, span),
            K::Float(text) => self.float_literal(text, false, expect, span),
            K::Char(c) => mk(tir::ExprKind::Char(*c), Ty::Char, span),
            K::Str(s) => mk(tir::ExprKind::Str(s.clone()), Ty::Str, span),
            K::Bool(b) => mk(tir::ExprKind::Bool(*b), Ty::Bool, span),
            K::Unit => mk(tir::ExprKind::Unit, Ty::Unit, span),
            K::Name(id) => self.name(id),
            K::Block(b) => {
                let (b, ty) = self.block(b, expect);
                mk(tir::ExprKind::Block(b), ty, span)
            }
            K::If { cond, then_block, else_expr } => self.if_expr(cond, then_block, else_expr.as_deref(), expect, span),
            K::While { cond, body } => {
                let cond = self.expr(cond, Some(&Ty::Bool));
                let cond = self.expect_ty(cond, &Ty::Bool);
                self.loops.push(false);
                let (body, _) = self.block(body, None);
                self.loops.pop();
                mk(tir::ExprKind::While(Box::new(cond), body), Ty::Unit, span)
            }
            K::Loop(body) => {
                self.loops.push(false);
                let (body, _) = self.block(body, None);
                let has_break = self.loops.pop().unwrap_or(false);
                mk(tir::ExprKind::Loop(body), if has_break { Ty::Unit } else { Ty::Never }, span)
            }
            K::For { pattern, iter, body } => self.for_expr(pattern, iter, body, span),
            K::Match(scrutinee, arms) => self.match_expr(scrutinee, arms, expect, span),
            K::Break | K::Continue => {
                let Some(has_break) = self.loops.last_mut() else {
                    let word = if matches!(e.kind, K::Break) { "break" } else { "continue" };
                    self.diags.error(Code::SYNTAX, span, format!("`{word}` outside of a loop"));
                    return error_expr(span);
                };
                if matches!(e.kind, K::Break) {
                    *has_break = true;
                    mk(tir::ExprKind::Break, Ty::Never, span)
                } else {
                    mk(tir::ExprKind::Continue, Ty::Never, span)
                }
            }
            K::Return(value) => {
                let ret = self.ret.clone();
                let value = match value {
                    Some(v) => {
                        let v = self.expr(v, Some(&ret));
                        Some(Box::new(self.expect_ty(v, &ret)))
                    }
                    None => {
                        if !compatible(&Ty::Unit, &ret) {
                            self.mismatch(span, &ret, &Ty::Unit);
                        }
                        None
                    }
                };
                mk(tir::ExprKind::Return(value), Ty::Never, span)
            }
            K::Assign(lhs, rhs) => self.assign(lhs, rhs, span),
            K::Binary(op, l, r) => self.binary(*op, l, r, expect, span),
            K::Unary(op, operand) => self.unary(*op, operand, expect, span),
            K::Cast(operand, ty) => {
                let target = resolve_type(ty, self.diags);
                let operand = self.expr(operand, None);
                if !can_cast(&operand.ty, &target) {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        span,
                        format!("cannot convert `{}` to `{target}` with `as`", operand.ty),
                    );
                    return error_expr(span);
                }
                mk(tir::ExprKind::Cast(Box::new(operand)), target, span)
            }
            K::Call(callee, args) => self.call(callee, args, span),
            K::Index(base, index) => self.index(base, index, span),
            K::Array(elems) => self.array(elems, expect, span),
            K::ArrayRepeat(value, count) => {
                let elem_expect = match expect {
                    Some(Ty::Array(t, _)) => Some(&**t),
                    _ => None,
                };
                let value = self.expr(value, elem_expect);
                let n = match count.kind {
                    K::Int(n) if n <= u128::from(u32::MAX) => n as u64,
                    _ => return self.unsupported(count.span, "array lengths other than integer literals"),
                };
                let ty = Ty::Array(Box::new(value.ty.clone()), n);
                mk(tir::ExprKind::ArrayRepeat(Box::new(value), n), ty, span)
            }
            K::Field(..) | K::TupleField(..) => self.unsupported(span, "field access"),
            K::Tuple(_) => self.unsupported(span, "tuples"),
            K::Range { .. } => self.unsupported(span, "ranges outside `for` loops"),
            K::OrElse(..) => self.unsupported(span, "`orelse`"),
            K::Try(_) => self.unsupported(span, "`?`"),
            K::SelfValue => self.unsupported(span, "`self`"),
            K::UnsafeBlock(_) => self.unsupported(span, "`unsafe` blocks"),
        }
    }

    fn int_literal(&mut self, v: u128, negative: bool, expect: Option<&Ty>, span: Span) -> tir::Expr {
        let ity = match expect {
            Some(Ty::Int(i)) => *i,
            _ => IntTy::I32,
        };
        match self.int_value(v, negative, ity, span) {
            Some(value) => mk(tir::ExprKind::Int(value), Ty::Int(ity), span),
            None => error_expr(span),
        }
    }

    /// The value of an integer literal of type `ity`, or E0008 if it does not fit.
    fn int_value(&mut self, v: u128, negative: bool, ity: IntTy, span: Span) -> Option<i128> {
        let value = i128::try_from(v).ok().map(|v| if negative { -v } else { v });
        match value {
            Some(value) if value >= ity.min() && value <= ity.max() => Some(value),
            _ => {
                let sign = if negative { "-" } else { "" };
                self.diags.error(
                    Code::LITERAL_RANGE,
                    span,
                    format!("literal `{sign}{v}` is out of range for `{}`", ity.name()),
                );
                None
            }
        }
    }

    fn float_literal(&mut self, text: &str, negative: bool, expect: Option<&Ty>, span: Span) -> tir::Expr {
        let fty = match expect {
            Some(Ty::Float(f)) => *f,
            _ => FloatTy::F64,
        };
        let Ok(v) = text.parse::<f64>() else {
            self.diags.error(Code::SYNTAX, span, format!("invalid float literal `{text}`"));
            return error_expr(span);
        };
        let v = if negative { -v } else { v };
        if !v.is_finite() || (fty == FloatTy::F32 && !(v as f32).is_finite()) {
            self.diags.error(Code::LITERAL_RANGE, span, format!("literal `{text}` is out of range for its type"));
            return error_expr(span);
        }
        mk(tir::ExprKind::Float(v), Ty::Float(fty), span)
    }

    fn name(&mut self, id: &ast::Ident) -> tir::Expr {
        if let Some(local) = self.lookup(&id.name) {
            return mk(tir::ExprKind::Local(local), self.locals[local.0].ty.clone(), id.span);
        }
        match self.globals.names.get(&id.name) {
            Some(Global::Const(c)) => mk(tir::ExprKind::Const(*c), self.globals.const_tys[c.0].clone(), id.span),
            Some(Global::Func(_)) => self.unsupported(id.span, "using a function as a value"),
            None if builtin(&id.name).is_some() => self.unsupported(id.span, "using a function as a value"),
            None if id.name == "Some" || id.name == "None" => self.unsupported(id.span, "building `Option` values"),
            None => {
                self.diags.error(Code::UNKNOWN_NAME, id.span, format!("unknown name `{}`", id.name));
                error_expr(id.span)
            }
        }
    }

    fn if_expr(
        &mut self,
        cond: &ast::Expr,
        then_block: &ast::Block,
        else_expr: Option<&ast::Expr>,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let cond = self.expr(cond, Some(&Ty::Bool));
        let cond = self.expect_ty(cond, &Ty::Bool);
        let (then, then_ty) = self.block(then_block, if else_expr.is_some() { expect } else { None });
        let Some(else_expr) = else_expr else {
            return mk(tir::ExprKind::If(Box::new(cond), then, None), Ty::Unit, span);
        };
        let else_expect = if then_ty == Ty::Never { expect.cloned() } else { Some(then_ty.clone()) };
        let other = self.expr(else_expr, else_expect.as_ref());
        let ty = self.unify(then_ty, &other.ty, other.span, "`if` and `else` have different types");
        mk(tir::ExprKind::If(Box::new(cond), then, Some(Box::new(other))), ty, span)
    }

    /// The type of two branches that must agree; reports E0003 at `span` when they do not.
    fn unify(&mut self, first: Ty, second: &Ty, span: Span, what: &str) -> Ty {
        if first == Ty::Never || first == Ty::Error {
            return second.clone();
        }
        if !compatible(second, &first) {
            self.diags.error(Code::TYPE_MISMATCH, span, format!("{what}: `{first}` and `{second}`"));
        }
        first
    }

    fn for_expr(&mut self, pattern: &ast::Pattern, iter: &ast::Expr, body: &ast::Block, span: Span) -> tir::Expr {
        let ast::ExprKind::Range { lo, hi, inclusive } = &iter.kind else {
            return self.unsupported(iter.span, "`for` over anything but an integer range");
        };
        let (lo, hi) = self.operands(lo, hi, None);
        let ty = if lo.ty == Ty::Never { hi.ty.clone() } else { lo.ty.clone() };
        let hi = self.expect_ty(hi, &ty);
        if !matches!(ty, Ty::Int(_) | Ty::Error) {
            self.diags.error(Code::TYPE_MISMATCH, iter.span, format!("cannot iterate over a range of `{ty}`"));
        }
        self.scopes.push(HashMap::new());
        let var = match &pattern.kind {
            ast::PatternKind::Binding { name, mutable } => Some(self.declare(&name.name, ty, *mutable)),
            ast::PatternKind::Wild => None,
            _ => {
                self.scopes.pop();
                return self.unsupported(pattern.span, "this `for` pattern");
            }
        };
        self.loops.push(false);
        let (body, _) = self.block(body, None);
        self.loops.pop();
        self.scopes.pop();
        let kind = tir::ExprKind::ForRange { var, lo: Box::new(lo), hi: Box::new(hi), inclusive: *inclusive, body };
        mk(kind, Ty::Unit, span)
    }

    fn match_expr(
        &mut self,
        scrutinee: &ast::Expr,
        arms: &[ast::MatchArm],
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let scrutinee = self.expr(scrutinee, None);
        let mut ty = Ty::Never;
        let mut checked = Vec::new();
        for arm in arms {
            self.scopes.push(HashMap::new());
            let pat = match &arm.pattern {
                Some(p) => self.pattern(p, &scrutinee.ty),
                None => Pat::Wild,
            };
            let guard = arm.guard.as_ref().map(|g| {
                let g = self.expr(g, Some(&Ty::Bool));
                self.expect_ty(g, &Ty::Bool)
            });
            let arm_expect = if ty == Ty::Never { expect.cloned() } else { Some(ty.clone()) };
            let body = self.expr(&arm.body, arm_expect.as_ref());
            ty = self.unify(ty, &body.ty, body.span, "match arms have different types");
            self.scopes.pop();
            checked.push(tir::Arm { pat, guard, body });
        }
        let unguarded: Vec<&Pat> = checked.iter().filter(|a| a.guard.is_none()).map(|a| &a.pat).collect();
        if !exhaustive(&scrutinee.ty, &unguarded) {
            self.diags.error(
                Code::NON_EXHAUSTIVE,
                span,
                format!("this `match` does not cover every value of `{}`", scrutinee.ty),
            );
        }
        mk(tir::ExprKind::Match(Box::new(scrutinee), checked), ty, span)
    }

    fn pattern(&mut self, p: &ast::Pattern, ty: &Ty) -> Pat {
        use ast::PatternKind as P;
        let pat_mismatch = |cx: &mut Self, what: &str| {
            if *ty != Ty::Error {
                cx.diags.error(Code::TYPE_MISMATCH, p.span, format!("expected `{ty}`, found {what}"));
            }
            Pat::Wild
        };
        match &p.kind {
            P::Wild => Pat::Wild,
            P::Binding { name, mutable } => {
                if name.name == "None" && !*mutable {
                    return match ty {
                        Ty::Option(_) => Pat::None,
                        _ => pat_mismatch(self, "`None`"),
                    };
                }
                if let Some(Global::Const(_)) = self.globals.names.get(&name.name) {
                    self.unsupported(p.span, "constants in patterns");
                    return Pat::Wild;
                }
                Pat::Binding(self.declare(&name.name, ty.clone(), *mutable))
            }
            P::At(name, inner) => {
                let inner = self.pattern(inner, ty);
                Pat::At(self.declare(&name.name, ty.clone(), false), Box::new(inner))
            }
            P::Lit(lit) => self.lit_pattern(lit, ty, p.span).unwrap_or(Pat::Wild),
            P::Range(lo, hi) => {
                let (Some(lo), Some(hi)) = (self.lit_pattern(lo, ty, p.span), self.lit_pattern(hi, ty, p.span)) else {
                    return Pat::Wild;
                };
                let value = |p: &Pat| match p {
                    Pat::Int(v) => Some(*v),
                    Pat::Char(c) => Some(*c as i128),
                    _ => None,
                };
                match (value(&lo), value(&hi)) {
                    (Some(lo), Some(hi)) if lo <= hi => Pat::Range(lo, hi),
                    (Some(_), Some(_)) => {
                        self.diags.error(Code::TYPE_MISMATCH, p.span, "this range pattern is empty");
                        Pat::Wild
                    }
                    _ => pat_mismatch(self, "a range pattern"),
                }
            }
            P::TupleStruct(path, elems) if path.len() == 1 && path[0].name == "Some" => match ty {
                Ty::Option(inner) if elems.len() == 1 => Pat::Some(Box::new(self.pattern(&elems[0], inner))),
                Ty::Option(_) => {
                    self.diags.error(Code::ARG_COUNT, p.span, "`Some` takes one pattern");
                    Pat::Wild
                }
                _ => pat_mismatch(self, "`Some(...)`"),
            },
            P::Or(alts) => {
                let before = self.locals.len();
                let alts: Vec<Pat> = alts.iter().map(|a| self.pattern(a, ty)).collect();
                if self.locals.len() != before {
                    self.unsupported(p.span, "bindings inside `|` patterns");
                }
                Pat::Or(alts)
            }
            P::Tuple(_) => {
                self.unsupported(p.span, "tuple patterns");
                Pat::Wild
            }
            P::Path(_) | P::TupleStruct(..) | P::Struct(..) => {
                self.unsupported(p.span, "enum and struct patterns");
                Pat::Wild
            }
        }
    }

    fn lit_pattern(&mut self, lit: &ast::LitPat, ty: &Ty, span: Span) -> Option<Pat> {
        match (lit, ty) {
            (_, Ty::Error) => None,
            (ast::LitPat::Int { value, negative }, Ty::Int(i)) => {
                self.int_value(*value, *negative, *i, span).map(Pat::Int)
            }
            (ast::LitPat::Bool(b), Ty::Bool) => Some(Pat::Bool(*b)),
            (ast::LitPat::Char(c), Ty::Char) => Some(Pat::Char(*c)),
            (ast::LitPat::Str(_), Ty::Str) => {
                self.unsupported(span, "string patterns");
                None
            }
            _ => {
                let what = match lit {
                    ast::LitPat::Int { .. } => "an integer literal",
                    ast::LitPat::Bool(_) => "a `bool` literal",
                    ast::LitPat::Char(_) => "a character literal",
                    ast::LitPat::Str(_) => "a string literal",
                };
                self.diags.error(Code::TYPE_MISMATCH, span, format!("expected `{ty}`, found {what}"));
                None
            }
        }
    }

    fn assign(&mut self, lhs: &ast::Expr, rhs: &ast::Expr, span: Span) -> tir::Expr {
        let place = self.expr(lhs, None);
        let rhs = self.expr(rhs, Some(&place.ty));
        let rhs = self.expect_ty(rhs, &place.ty);
        if place.ty != Ty::Error {
            match place_root(&place) {
                Some(root) if !self.locals[root.0].mutable => {
                    let name = self.locals[root.0].name.clone();
                    self.diags.error(
                        Code::IMMUTABLE,
                        lhs.span,
                        format!("cannot assign to `{name}`: it is not declared with `let mut`"),
                    );
                }
                Some(_) => {}
                None => {
                    self.diags.error(
                        Code::IMMUTABLE,
                        lhs.span,
                        "the left side of `=` is not a variable or an element of one",
                    );
                }
            }
        }
        mk(tir::ExprKind::Assign(Box::new(place), Box::new(rhs)), Ty::Unit, span)
    }

    /// Checks two operands that must have the same type. A literal on the left takes its type from the right.
    fn operands(&mut self, l: &ast::Expr, r: &ast::Expr, hint: Option<&Ty>) -> (tir::Expr, tir::Expr) {
        let numeric_hint = hint.filter(|t| t.is_numeric());
        if numeric_hint.is_none() && is_untyped_literal(l) && !is_untyped_literal(r) {
            let r = self.expr(r, None);
            let h = if r.ty.is_numeric() { Some(r.ty.clone()) } else { None };
            let l = self.expr(l, h.as_ref());
            return (l, r);
        }
        let l = self.expr(l, numeric_hint);
        let h = if l.ty.is_numeric() { Some(l.ty.clone()) } else { numeric_hint.cloned() };
        let r = self.expr(r, h.as_ref());
        (l, r)
    }

    fn binary(&mut self, op: ast::BinOp, l: &ast::Expr, r: &ast::Expr, expect: Option<&Ty>, span: Span) -> tir::Expr {
        use ast::BinOp as B;
        let op_name = op_text(op);
        match op {
            B::And | B::Or => {
                let l = self.expr(l, Some(&Ty::Bool));
                let l = self.expect_ty(l, &Ty::Bool);
                let r = self.expr(r, Some(&Ty::Bool));
                let r = self.expect_ty(r, &Ty::Bool);
                mk(tir::ExprKind::Binary(bin_op(op), Box::new(l), Box::new(r)), Ty::Bool, span)
            }
            B::Shl | B::Shr => {
                let l = self.expr(l, expect.filter(|t| t.is_int()));
                let r = self.expr(r, if l.ty.is_int() { Some(&l.ty) } else { None });
                for operand in [&l, &r] {
                    if !matches!(operand.ty, Ty::Int(_) | Ty::Error | Ty::Never) {
                        self.diags.error(
                            Code::TYPE_MISMATCH,
                            operand.span,
                            format!("`{op_name}` needs integer operands, found `{}`", operand.ty),
                        );
                    }
                }
                let ty = l.ty.clone();
                mk(tir::ExprKind::Binary(bin_op(op), Box::new(l), Box::new(r)), ty, span)
            }
            _ => {
                let comparison = matches!(op, B::Eq | B::Ne | B::Lt | B::Gt | B::Le | B::Ge);
                let (l, r) = self.operands(l, r, if comparison { None } else { expect });
                let ty = if l.ty == Ty::Never { r.ty.clone() } else { l.ty.clone() };
                if !compatible(&r.ty, &ty) {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        r.span,
                        format!("`{op_name}` needs operands of the same type, found `{}` and `{}`", l.ty, r.ty),
                    );
                    return error_expr(span);
                }
                let ok = match op {
                    B::Add | B::Sub | B::Mul | B::Div | B::Rem => ty.is_numeric(),
                    B::BitAnd | B::BitOr | B::BitXor => matches!(ty, Ty::Int(_) | Ty::Bool),
                    B::Eq | B::Ne | B::Lt | B::Gt | B::Le | B::Ge => {
                        matches!(ty, Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Char)
                    }
                    _ => unreachable!("handled above"),
                };
                if !ok && !matches!(ty, Ty::Error | Ty::Never) {
                    if matches!(ty, Ty::Str | Ty::String) && comparison {
                        return self.unsupported(span, "comparing strings");
                    }
                    self.diags.error(Code::TYPE_MISMATCH, span, format!("`{op_name}` cannot be applied to `{ty}`"));
                    return error_expr(span);
                }
                let result = if comparison { Ty::Bool } else { ty };
                mk(tir::ExprKind::Binary(bin_op(op), Box::new(l), Box::new(r)), result, span)
            }
        }
    }

    fn unary(&mut self, op: ast::UnOp, operand: &ast::Expr, expect: Option<&Ty>, span: Span) -> tir::Expr {
        match op {
            ast::UnOp::Neg => {
                // A minus sign directly before a literal is part of the literal's value, so `-128` fits in `i8`.
                match &operand.kind {
                    ast::ExprKind::Int(v) => {
                        if let Some(Ty::Int(i)) = expect
                            && !i.signed()
                        {
                            self.diags.error(Code::TYPE_MISMATCH, span, format!("cannot negate a `{}`", i.name()));
                            return error_expr(span);
                        }
                        return self.int_literal(*v, true, expect, span);
                    }
                    ast::ExprKind::Float(text) => return self.float_literal(text, true, expect, span),
                    _ => {}
                }
                let e = self.expr(operand, expect.filter(|t| t.is_numeric()));
                let ok = match &e.ty {
                    Ty::Int(i) => i.signed(),
                    Ty::Float(_) | Ty::Error | Ty::Never => true,
                    _ => false,
                };
                if !ok {
                    self.diags.error(Code::TYPE_MISMATCH, span, format!("cannot negate a `{}`", e.ty));
                    return error_expr(span);
                }
                let ty = e.ty.clone();
                mk(tir::ExprKind::Unary(UnOp::Neg, Box::new(e)), ty, span)
            }
            ast::UnOp::Not => {
                let e = self.expr(operand, expect.filter(|t| t.is_int() || **t == Ty::Bool));
                if !matches!(e.ty, Ty::Bool | Ty::Int(_) | Ty::Error | Ty::Never) {
                    self.diags.error(Code::TYPE_MISMATCH, span, format!("`!` cannot be applied to `{}`", e.ty));
                    return error_expr(span);
                }
                let ty = e.ty.clone();
                mk(tir::ExprKind::Unary(UnOp::Not, Box::new(e)), ty, span)
            }
            ast::UnOp::Deref | ast::UnOp::Ref | ast::UnOp::RefMut => self.unsupported(span, "references"),
        }
    }

    fn call(&mut self, callee: &ast::Expr, args: &[ast::Arg], span: Span) -> tir::Expr {
        if let ast::ExprKind::Field(recv, name) = &callee.kind {
            return self.method_call(recv, name, args, span);
        }
        let (params, ret, target) = match &callee.kind {
            ast::ExprKind::Name(id) if self.lookup(&id.name).is_none() => match self.globals.names.get(&id.name) {
                Some(Global::Func(f)) => {
                    let f = *f;
                    let sig = &self.globals.sigs[f.0];
                    (sig.params.clone(), sig.ret.clone(), Callee::User(f))
                }
                Some(Global::Const(_)) => {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        id.span,
                        format!("`{}` is a constant, not a function", id.name),
                    );
                    return self.args_then_error(args, span);
                }
                None => match builtin(&id.name) {
                    Some(b) => {
                        let sig = builtin_sig(b);
                        (sig.params, sig.ret, Callee::Builtin(b))
                    }
                    None if id.name == "Some" => {
                        self.unsupported(id.span, "building `Option` values");
                        return self.args_then_error(args, span);
                    }
                    None => {
                        self.diags.error(Code::UNKNOWN_NAME, id.span, format!("unknown function `{}`", id.name));
                        return self.args_then_error(args, span);
                    }
                },
            },
            _ => {
                self.unsupported(callee.span, "calling this expression");
                return self.args_then_error(args, span);
            }
        };
        let name = match &callee.kind {
            ast::ExprKind::Name(id) => id.name.clone(),
            _ => String::new(),
        };
        let Some(checked) = self.args(&name, &params, args, span) else { return error_expr(span) };
        let kind = match target {
            Callee::User(f) => tir::ExprKind::Call(f, checked),
            Callee::Builtin(b) => tir::ExprKind::Builtin(b, checked),
        };
        mk(kind, ret, span)
    }

    /// Checks call arguments against the parameter types. `None` after a wrong argument count (reported).
    fn args(&mut self, name: &str, params: &[Ty], args: &[ast::Arg], span: Span) -> Option<Vec<tir::Expr>> {
        if params.len() != args.len() {
            for a in args {
                self.expr(&a.expr, None);
            }
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`{name}` takes {} argument(s), but {} were given", params.len(), args.len()),
            );
            return None;
        }
        let mut checked = Vec::new();
        for (a, p) in args.iter().zip(params) {
            if a.mut_marker {
                self.unsupported(a.expr.span, "`mut` arguments");
            }
            let arg = self.expr(&a.expr, Some(p));
            checked.push(self.coerce_arg(arg, p));
        }
        Some(checked)
    }

    fn args_then_error(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        for a in args {
            self.expr(&a.expr, None);
        }
        error_expr(span)
    }

    fn method_call(&mut self, recv: &ast::Expr, name: &ast::Ident, args: &[ast::Arg], span: Span) -> tir::Expr {
        let recv = self.expr(recv, None);
        if recv.ty == Ty::Error {
            return self.args_then_error(args, span);
        }
        let Some((params, ret)) = method(&name.name).and_then(|m| method_sig(m, &recv.ty)) else {
            self.diags.error(Code::UNKNOWN_NAME, name.span, format!("no method `{}` on type `{}`", name.name, recv.ty));
            return self.args_then_error(args, span);
        };
        let m = method(&name.name).expect("checked above");
        let Some(mut checked) = self.args(&name.name, &params, args, span) else { return error_expr(span) };
        checked.insert(0, recv);
        mk(tir::ExprKind::Method(m, checked), ret, span)
    }

    fn index(&mut self, base: &ast::Expr, index: &ast::Expr, span: Span) -> tir::Expr {
        let base = self.expr(base, None);
        if matches!(index.kind, ast::ExprKind::Range { .. }) {
            return self.unsupported(index.span, "slicing");
        }
        let index = self.expr(index, Some(&Ty::Int(IntTy::Usize)));
        let index = self.expect_ty(index, &Ty::Int(IntTy::Usize));
        let ty = match &base.ty {
            Ty::Array(elem, _) => (**elem).clone(),
            Ty::Error => Ty::Error,
            other => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    base.span,
                    format!("cannot index into a value of type `{other}`"),
                );
                return error_expr(span);
            }
        };
        mk(tir::ExprKind::Index(Box::new(base), Box::new(index)), ty, span)
    }

    fn array(&mut self, elems: &[ast::Expr], expect: Option<&Ty>, span: Span) -> tir::Expr {
        let mut elem_ty = match expect {
            Some(Ty::Array(t, _)) => Some((**t).clone()),
            _ => None,
        };
        let mut checked = Vec::new();
        for e in elems {
            let c = self.expr(e, elem_ty.as_ref());
            let c = match &elem_ty {
                Some(t) => self.expect_ty(c, t),
                None => {
                    elem_ty = Some(c.ty.clone());
                    c
                }
            };
            checked.push(c);
        }
        let Some(elem_ty) = elem_ty else {
            self.diags.error(Code::TYPE_MISMATCH, span, "cannot infer the element type of an empty array");
            return error_expr(span);
        };
        let ty = Ty::Array(Box::new(elem_ty), checked.len() as u64);
        mk(tir::ExprKind::Array(checked), ty, span)
    }
}

fn op_text(op: ast::BinOp) -> &'static str {
    use ast::BinOp as B;
    match op {
        B::Or => "||",
        B::And => "&&",
        B::Eq => "==",
        B::Ne => "!=",
        B::Lt => "<",
        B::Gt => ">",
        B::Le => "<=",
        B::Ge => ">=",
        B::BitOr => "|",
        B::BitXor => "^",
        B::BitAnd => "&",
        B::Shl => "<<",
        B::Shr => ">>",
        B::Add => "+",
        B::Sub => "-",
        B::Mul => "*",
        B::Div => "/",
        B::Rem => "%",
    }
}
