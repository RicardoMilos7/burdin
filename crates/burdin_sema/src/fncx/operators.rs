//! Assignment and the unary and binary operators.

use super::{FnCx, error_expr, mk};
use crate::generics::{Bound, SYMBOLIC_CALL};
use crate::globals::adt_key;
use crate::types::compatible;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::place::{is_place_expr, place_root};
use burdin_tir::{self as tir, BinOp, IntTy, Ty, UnOp};

impl FnCx<'_> {
    pub(super) fn assign(&mut self, lhs: &ast::Expr, rhs: &ast::Expr, span: Span) -> tir::Expr {
        // The left side is written, so an index expression on a user type lowers to `index_mut`
        // (`spec/operators.md`, OP10). The right side is an ordinary value expression.
        let saved = self.write_place;
        self.write_place = true;
        let place = self.expr(lhs, None);
        self.write_place = saved;
        let rhs = self.expr(rhs, Some(&place.ty));
        let rhs = self.expect_ty(rhs, &place.ty);
        if place.ty != Ty::Error {
            if !is_place_expr(&place) {
                self.diags.error(
                    Code::IMMUTABLE,
                    lhs.span,
                    "the left side of `=` is not a variable or an element of one",
                );
            } else if !self.place_mutable(&place) {
                let name = place_root(&place).map(|root| self.locals[root.0].name.clone());
                let whose = name.unwrap_or_else(|| "this place".to_string());
                self.diags.error(Code::IMMUTABLE, lhs.span, format!("cannot assign to `{whose}`: it is not mutable"));
            }
        }
        mk(tir::ExprKind::Assign(Box::new(place), Box::new(rhs)), Ty::Unit, span)
    }

    /// Checks two operands that must have the same type. A literal on the left takes its type from the right.
    pub(super) fn operands(&mut self, l: &ast::Expr, r: &ast::Expr, hint: Option<&Ty>) -> (tir::Expr, tir::Expr) {
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

    pub(super) fn binary(
        &mut self,
        op: ast::BinOp,
        l: &ast::Expr,
        r: &ast::Expr,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
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
                if matches!(op, B::Add | B::Sub | B::Eq | B::Ne) && !is_untyped_literal(l) {
                    let left = self.expr(l, if comparison { None } else { expect });
                    if let Ty::CPtr(_) = left.ty.clone() {
                        return self.pointer_binary(op, left, r, span);
                    }
                    let hint = if left.ty.is_numeric() {
                        Some(left.ty.clone())
                    } else {
                        expect.filter(|t| t.is_numeric()).cloned()
                    };
                    let right = self.expr(r, hint.as_ref());
                    return self.binary_checked(op, left, right, span, op_name);
                }
                let (l, r) = self.operands(l, r, if comparison { None } else { expect });
                self.binary_checked(op, l, r, span, op_name)
            }
        }
    }

    /// The type checks of a binary operator whose two operands are already checked.
    fn binary_checked(&mut self, op: ast::BinOp, l: tir::Expr, r: tir::Expr, span: Span, op_name: &str) -> tir::Expr {
        use ast::BinOp as B;
        {
            {
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
                    B::Eq | B::Ne => self.globals.has_eq_in(&ty, &self.bounds),
                    B::Lt | B::Gt | B::Le | B::Ge => {
                        matches!(ty, Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Char)
                            || self.globals.has_ord_in(&ty, &self.bounds)
                    }
                    _ => unreachable!("handled above"),
                };
                if !ok && !matches!(ty, Ty::Error | Ty::Never) {
                    // A user type takes part through its operator interface (`spec/operators.md`, OP3-OP6).
                    if let Some(call) = self.overloaded_binary(op, l.clone(), r.clone(), span) {
                        return call;
                    }
                    let iface = self.op_interface(op).map(|(_, _, name)| name);
                    match &ty {
                        // A derive is the type's own comparison; when it does not hold, a field is the reason and
                        // the stage 0 diagnostic (E0003) stays (`spec/derives.md`).
                        Ty::Struct(_) | Ty::Enum(_) if self.derives_comparison(&ty, op) => {
                            self.diags.error(
                                Code::TYPE_MISMATCH,
                                span,
                                format!("`{op_name}` cannot be applied to `{ty}`"),
                            );
                        }
                        Ty::Param(_) | Ty::Struct(_) | Ty::Enum(_) => {
                            let reason = match (&ty, iface) {
                                (Ty::Param(_), Some(name)) => format!(": its bounds do not provide `{name}`"),
                                (_, Some(name)) => format!(": it does not implement `{name}`"),
                                _ => String::new(),
                            };
                            self.diags.error(
                                Code::BOUND_NOT_SATISFIED,
                                span,
                                format!("`{op_name}` is not available on `{ty}`{reason}"),
                            );
                        }
                        _ => {
                            self.diags.error(
                                Code::TYPE_MISMATCH,
                                span,
                                format!("`{op_name}` cannot be applied to `{ty}`"),
                            );
                        }
                    }
                    return error_expr(span);
                }
                let result = if matches!(op, B::Eq | B::Ne | B::Lt | B::Gt | B::Le | B::Ge) { Ty::Bool } else { ty };
                mk(tir::ExprKind::Binary(bin_op(op), Box::new(l), Box::new(r)), result, span)
            }
        }
    }

    /// Whether the type's own derived comparison is the one that just failed: `Eq` for `==`/`!=`, `Ord` for the
    /// ordering operators. Such a failure names a field, not a missing interface.
    fn derives_comparison(&self, ty: &Ty, op: ast::BinOp) -> bool {
        use ast::BinOp as B;
        let derives = match ty {
            Ty::Struct(a) => &self.globals.structs[a.id].derives,
            Ty::Enum(a) => &self.globals.enums[a.id].derives,
            _ => return false,
        };
        match op {
            B::Eq | B::Ne => derives.eq,
            B::Lt | B::Gt | B::Le | B::Ge => derives.ord,
            _ => false,
        }
    }

    /// The standard library interface and method an overloadable binary operator uses (`spec/operators.md`, OP3),
    /// as `(interface, method name, interface name)`.
    fn op_interface(&self, op: ast::BinOp) -> Option<(usize, &'static str, &'static str)> {
        let ops = self.globals.op_ifaces?;
        use ast::BinOp as B;
        Some(match op {
            B::Add => (ops.add, "add", "Add"),
            B::Sub => (ops.sub, "sub", "Sub"),
            B::Mul => (ops.mul, "mul", "Mul"),
            B::Div => (ops.div, "div", "Div"),
            B::Rem => (ops.rem, "rem", "Rem"),
            B::BitAnd => (ops.bitand, "bitand", "BitAnd"),
            B::BitOr => (ops.bitor, "bitor", "BitOr"),
            B::BitXor => (ops.bitxor, "bitxor", "BitXor"),
            B::Eq => (ops.partial_eq, "eq", "PartialEq"),
            B::Ne => (ops.partial_eq, "ne", "PartialEq"),
            B::Lt => (ops.partial_ord, "lt", "PartialOrd"),
            B::Gt => (ops.partial_ord, "gt", "PartialOrd"),
            B::Le => (ops.partial_ord, "le", "PartialOrd"),
            B::Ge => (ops.partial_ord, "ge", "PartialOrd"),
            _ => return None,
        })
    }

    /// Lowers `l op r` to its interface method call when the built-in operator does not apply. `None` when the
    /// left type is not a struct, enum, or type parameter, or does not implement the interface.
    fn overloaded_binary(&mut self, op: ast::BinOp, l: tir::Expr, r: tir::Expr, span: Span) -> Option<tir::Expr> {
        let (iface, method, _) = self.op_interface(op)?;
        match l.ty.clone() {
            Ty::Param(_) => {
                let (sig, _, _) = self.globals.bound_method(&l.ty, method, &self.bounds)?;
                if sig.params.len() != 2 || !sig.has_self {
                    return None;
                }
                let ret = sig.ret.clone();
                Some(mk(tir::ExprKind::Call(SYMBOLIC_CALL, vec![l, r]), ret, span))
            }
            ty @ (Ty::Struct(_) | Ty::Enum(_)) => {
                let key = adt_key(&ty)?;
                if !self.globals.satisfies(&ty, &Bound::User(iface, Vec::new()), &self.bounds) {
                    return None;
                }
                let f = *self.globals.methods.get(&(key, method.to_string()))?;
                let sig = &self.globals.sigs[f.0];
                if sig.params.len() != 2 || !sig.has_self {
                    return None;
                }
                let ret = sig.ret.clone();
                Some(mk(tir::ExprKind::Call(f, vec![l, r]), ret, span))
            }
            _ => None,
        }
    }

    /// Lowers `-a` or `!a` to its interface method call when the built-in operator does not apply.
    fn overloaded_unary(&mut self, iface: usize, method: &str, e: tir::Expr, span: Span) -> Option<tir::Expr> {
        match e.ty.clone() {
            Ty::Param(_) => {
                let (sig, _, _) = self.globals.bound_method(&e.ty, method, &self.bounds)?;
                if sig.params.len() != 1 || !sig.has_self {
                    return None;
                }
                let ret = sig.ret.clone();
                Some(mk(tir::ExprKind::Call(SYMBOLIC_CALL, vec![e]), ret, span))
            }
            ty @ (Ty::Struct(_) | Ty::Enum(_)) => {
                let key = adt_key(&ty)?;
                if !self.globals.satisfies(&ty, &Bound::User(iface, Vec::new()), &self.bounds) {
                    return None;
                }
                let f = *self.globals.methods.get(&(key, method.to_string()))?;
                let sig = &self.globals.sigs[f.0];
                if sig.params.len() != 1 || !sig.has_self {
                    return None;
                }
                let ret = sig.ret.clone();
                Some(mk(tir::ExprKind::Call(f, vec![e]), ret, span))
            }
            _ => None,
        }
    }

    /// Pointer equality, element offsets, and pointer difference (issue #72). The operands are checked here
    /// because pointer arithmetic is not the same-type arithmetic of the ordinary operators.
    fn pointer_binary(&mut self, op: ast::BinOp, l: tir::Expr, r: &ast::Expr, span: Span) -> tir::Expr {
        use ast::BinOp as B;
        let ptr_ty = l.ty.clone();
        let op_name = op_text(op);
        match op {
            B::Eq | B::Ne => {
                let r = self.expr(r, Some(&ptr_ty));
                let r = self.expect_ty(r, &ptr_ty);
                mk(tir::ExprKind::Binary(bin_op(op), Box::new(l), Box::new(r)), Ty::Bool, span)
            }
            B::Add | B::Sub => {
                let r = self.expr(r, None);
                match &r.ty {
                    Ty::Int(_) => mk(tir::ExprKind::Binary(bin_op(op), Box::new(l), Box::new(r)), ptr_ty, span),
                    Ty::CPtr(_) if op == B::Sub => {
                        let r = self.expect_ty(r, &ptr_ty);
                        mk(tir::ExprKind::Binary(bin_op(op), Box::new(l), Box::new(r)), Ty::Int(IntTy::Isize), span)
                    }
                    other if *other == Ty::Error => error_expr(span),
                    other => {
                        self.diags.error(
                            Code::TYPE_MISMATCH,
                            r.span,
                            format!("`{op_name}` on `{ptr_ty}` needs an integer offset, found `{other}`"),
                        );
                        error_expr(span)
                    }
                }
            }
            _ => error_expr(span),
        }
    }

    pub(super) fn unary(&mut self, op: ast::UnOp, operand: &ast::Expr, expect: Option<&Ty>, span: Span) -> tir::Expr {
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
                    // A user type takes part through `Neg` (`spec/operators.md`, OP4).
                    if let Some(ops) = self.globals.op_ifaces
                        && let Some(call) = self.overloaded_unary(ops.neg, "neg", e.clone(), span)
                    {
                        return call;
                    }
                    match &e.ty {
                        Ty::Param(_) | Ty::Struct(_) | Ty::Enum(_) => {
                            let reason = match &e.ty {
                                Ty::Param(_) => "its bounds do not provide `Neg`",
                                _ => "it does not implement `Neg`",
                            };
                            self.diags.error(
                                Code::BOUND_NOT_SATISFIED,
                                span,
                                format!("`-` is not available on `{}`: {reason}", e.ty),
                            );
                        }
                        other => {
                            self.diags.error(Code::TYPE_MISMATCH, span, format!("cannot negate a `{other}`"));
                        }
                    }
                    return error_expr(span);
                }
                let ty = e.ty.clone();
                mk(tir::ExprKind::Unary(UnOp::Neg, Box::new(e)), ty, span)
            }
            ast::UnOp::Not => {
                let e = self.expr(operand, expect.filter(|t| t.is_int() || **t == Ty::Bool));
                if !matches!(e.ty, Ty::Bool | Ty::Int(_) | Ty::Error | Ty::Never) {
                    // A user type takes part through `Not` (`spec/operators.md`, OP4).
                    if let Some(ops) = self.globals.op_ifaces
                        && let Some(call) = self.overloaded_unary(ops.not, "not", e.clone(), span)
                    {
                        return call;
                    }
                    match &e.ty {
                        Ty::Param(_) | Ty::Struct(_) | Ty::Enum(_) => {
                            let reason = match &e.ty {
                                Ty::Param(_) => "its bounds do not provide `Not`",
                                _ => "it does not implement `Not`",
                            };
                            self.diags.error(
                                Code::BOUND_NOT_SATISFIED,
                                span,
                                format!("`!` is not available on `{}`: {reason}", e.ty),
                            );
                        }
                        other => {
                            self.diags.error(Code::TYPE_MISMATCH, span, format!("`!` cannot be applied to `{other}`"));
                        }
                    }
                    return error_expr(span);
                }
                let ty = e.ty.clone();
                mk(tir::ExprKind::Unary(UnOp::Not, Box::new(e)), ty, span)
            }
            ast::UnOp::Deref => {
                let e = self.expr(operand, None);
                match &e.ty {
                    Ty::Heap(inner, _) => {
                        let inner = (**inner).clone();
                        mk(tir::ExprKind::Deref(Box::new(e)), inner, span)
                    }
                    Ty::CPtr(inner) => {
                        let inner = (**inner).clone();
                        mk(tir::ExprKind::PtrDeref(Box::new(e)), inner, span)
                    }
                    Ty::Ref { inner, .. } => {
                        let inner = (**inner).clone();
                        mk(tir::ExprKind::DerefRef(Box::new(e)), inner, span)
                    }
                    Ty::Error | Ty::Never => error_expr(span),
                    other => {
                        self.diags.error(
                            Code::TYPE_MISMATCH,
                            span,
                            format!(
                                "cannot dereference a `{other}`; `*` applies to `Heap<T>`, `CPtr<T>`, and references"
                            ),
                        );
                        error_expr(span)
                    }
                }
            }
            ast::UnOp::Ref | ast::UnOp::RefMut => {
                let mutable = op == ast::UnOp::RefMut;
                // `&mut place` writes through its place, so an index on a user type lowers to `index_mut`.
                let saved = self.write_place;
                self.write_place = mutable;
                let place = self.expr(operand, None);
                self.write_place = saved;
                if place.ty == Ty::Error {
                    return error_expr(span);
                }
                if !is_place_expr(&place) {
                    self.diags.error(Code::TYPE_MISMATCH, span, "`&` needs a place to borrow");
                    return error_expr(span);
                }
                if mutable && !self.place_mutable(&place) {
                    self.diags.error(
                        Code::MUT_OF_IMMUTABLE,
                        span,
                        "cannot take a mutable reference to a place that is not mutable",
                    );
                }
                let ty = Ty::Ref { mutable, inner: Box::new(place.ty.clone()) };
                mk(tir::ExprKind::Ref { place: Box::new(place), mutable }, ty, span)
            }
        }
    }
}

/// An expression whose type comes only from its context: a literal, or operators applied to literals.
pub(super) fn is_untyped_literal(e: &ast::Expr) -> bool {
    use ast::ExprKind as K;
    match &e.kind {
        K::Int(_) | K::Float(_) => true,
        K::Unary(ast::UnOp::Neg | ast::UnOp::Not, a) => is_untyped_literal(a),
        K::Binary(op, a, b) => is_arith(*op) && is_untyped_literal(a) && is_untyped_literal(b),
        _ => false,
    }
}

pub(super) fn is_arith(op: ast::BinOp) -> bool {
    use ast::BinOp as B;
    matches!(op, B::Add | B::Sub | B::Mul | B::Div | B::Rem | B::BitAnd | B::BitOr | B::BitXor)
}

pub(super) fn bin_op(op: ast::BinOp) -> BinOp {
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

pub(super) fn can_cast(from: &Ty, to: &Ty) -> bool {
    let scalar = |t: &Ty| matches!(t, Ty::Int(_) | Ty::Float(_));
    match (from, to) {
        (Ty::Error | Ty::Never, _) | (_, Ty::Error) => true,
        _ if from == to => true,
        (f, t) if scalar(f) && scalar(t) => true,
        (Ty::Bool | Ty::Char, Ty::Int(_)) => true,
        // `u32 as char` does not check that the value is a Unicode scalar value; `u32.to_char()` does.
        (Ty::Int(IntTy::U8 | IntTy::U32), Ty::Char) => true,
        // Raw pointers cast to and from integers, and to each other (issue #72).
        (Ty::CPtr(_), Ty::CPtr(_) | Ty::Int(_)) | (Ty::Int(_), Ty::CPtr(_)) => true,
        // The address of a place as a raw pointer, for C out-parameters.
        (Ty::Ref { .. }, Ty::CPtr(_)) => true,
        _ => false,
    }
}

pub(super) fn op_text(op: ast::BinOp) -> &'static str {
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
