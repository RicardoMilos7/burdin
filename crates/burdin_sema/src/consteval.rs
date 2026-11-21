//! Const generics (`spec/const-generics.md`): the integer values that stand where a type argument does, and the
//! array lengths written with them.
//!
//! A const argument is a [`Ty::Const`], so the instance keys, the substitution, and the unification that already
//! work for types work for values too. Inside a generic body a const parameter is `Ty::Param(name)`, as a type
//! parameter is; an array length or an argument computed from one (`R * C`) cannot be a number yet, so it is
//! interned here as an expression and stands for it in the type (`Ty::Const(SYM_CONST + id)`, and
//! `Ty::Array(_, SYM_LEN + id)`). Substituting the parameters folds the expression to a number.

use crate::generics::Bound;
use crate::globals::{Global, Globals};
use burdin_diag::{Code, Diagnostics};
use burdin_syntax::ast;
use burdin_tir::Ty;

/// Array lengths from here up are interned expressions, not lengths.
pub(crate) const SYM_LEN: u64 = 1 << 62;
/// Const arguments from here up are interned expressions, not values.
pub(crate) const SYM_CONST: i128 = 1 << 100;
/// The longest array a program can have.
const MAX_LEN: i128 = u32::MAX as i128;
/// How deep a constant may refer to other constants before the evaluator gives up.
const MAX_DEPTH: usize = 32;

/// A const expression with the names resolved: a number, a const parameter, or arithmetic on those.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConstExpr {
    Lit(i128),
    Param(String),
    Neg(Box<ConstExpr>),
    Bin(ast::BinOp, Box<ConstExpr>, Box<ConstExpr>),
}

impl Globals {
    /// The value or the interned expression a const argument stands for, or `None` for a type.
    pub(crate) fn const_expr_of(&self, ty: &Ty) -> Option<ConstExpr> {
        match ty {
            Ty::Const(v) if *v < SYM_CONST => Some(ConstExpr::Lit(*v)),
            Ty::Const(v) => self.sym_exprs.borrow().get((*v - SYM_CONST) as usize).cloned(),
            Ty::Param(p) => Some(ConstExpr::Param(p.to_string())),
            _ => None,
        }
    }

    fn intern(&self, e: ConstExpr) -> usize {
        let mut table = self.sym_exprs.borrow_mut();
        match table.iter().position(|x| *x == e) {
            Some(i) => i,
            None => {
                table.push(e);
                table.len() - 1
            }
        }
    }

    /// The type that stands for a folded const expression.
    fn const_ty(&self, e: ConstExpr) -> Ty {
        match e {
            ConstExpr::Lit(v) => Ty::Const(v),
            ConstExpr::Param(p) => Ty::Param(p.into()),
            other => Ty::Const(SYM_CONST + self.intern(other) as i128),
        }
    }

    /// The interned expression behind a symbolic array length.
    pub(crate) fn len_expr(&self, len: u64) -> Option<ConstExpr> {
        self.sym_exprs.borrow().get((len - SYM_LEN) as usize).cloned()
    }

    /// The array length a folded expression stands for: a number in range, or the interned expression. `None`
    /// when the number is out of range.
    fn len_of(&self, e: ConstExpr) -> Option<u64> {
        match e {
            ConstExpr::Lit(n) if (0..=MAX_LEN).contains(&n) => Some(n as u64),
            ConstExpr::Lit(_) => None,
            other => Some(SYM_LEN + self.intern(other) as u64),
        }
    }

    /// Resolves the names of a written const expression: const parameters of `subst` and constants.
    fn lower_const(
        &self,
        e: &ast::Expr,
        subst: &[(String, Ty)],
        module: usize,
        depth: usize,
        diags: &mut Diagnostics,
    ) -> Option<ConstExpr> {
        use ast::ExprKind as K;
        let mut fail = |message: String| {
            diags.error(Code::CONST_EVAL, e.span, message);
            None
        };
        match &e.kind {
            K::Int(n) => match i128::try_from(*n) {
                Ok(v) => Some(ConstExpr::Lit(v)),
                Err(_) => fail("this number is too large for a const expression".to_string()),
            },
            K::Bool(b) => Some(ConstExpr::Lit(i128::from(*b))),
            K::Char(c) => Some(ConstExpr::Lit(*c as i128)),
            K::Name { name, args } if args.is_empty() => {
                if let Some((_, ty)) = subst.iter().find(|(n, _)| *n == name.name) {
                    return match self.const_expr_of(ty) {
                        Some(expr) => Some(expr),
                        None => {
                            diags.error(
                                Code::INVALID_CONST_GENERIC,
                                e.span,
                                format!("`{}` is a type parameter, not a const parameter", name.name),
                            );
                            None
                        }
                    };
                }
                match self.value(module, &name.name) {
                    Some(Global::Const(id)) if depth < MAX_DEPTH => {
                        let (home, init) = self.const_inits[id.0];
                        self.lower_const(init, &[], home, depth + 1, diags)
                    }
                    Some(Global::Const(_)) => fail(format!("the constant `{}` refers to itself", name.name)),
                    _ => fail(format!(
                        "`{}` is not a const parameter or a constant, so it has no value while compiling",
                        name.name
                    )),
                }
            }
            K::Unary(ast::UnOp::Neg, a) => {
                Some(ConstExpr::Neg(Box::new(self.lower_const(a, subst, module, depth, diags)?)))
            }
            K::Binary(op, a, b)
                if matches!(
                    op,
                    ast::BinOp::Add
                        | ast::BinOp::Sub
                        | ast::BinOp::Mul
                        | ast::BinOp::Div
                        | ast::BinOp::Rem
                        | ast::BinOp::Shl
                        | ast::BinOp::Shr
                        | ast::BinOp::BitAnd
                        | ast::BinOp::BitOr
                        | ast::BinOp::BitXor
                ) =>
            {
                let a = self.lower_const(a, subst, module, depth, diags)?;
                let b = self.lower_const(b, subst, module, depth, diags)?;
                Some(ConstExpr::Bin(*op, Box::new(a), Box::new(b)))
            }
            _ => fail(
                "this expression is not supported in a const argument or array length: use numbers, const \
                 parameters, constants, and `+ - * / % << >> & | ^`"
                    .to_string(),
            ),
        }
    }

    /// Substitutes the parameters in `env` and folds what becomes a number. `Err` is the reason a fold failed.
    pub(crate) fn fold_const(&self, e: &ConstExpr, env: &[(String, Ty)]) -> Result<ConstExpr, String> {
        Ok(match e {
            ConstExpr::Lit(_) => e.clone(),
            ConstExpr::Param(p) => match env.iter().find(|(n, _)| n == p) {
                Some((_, ty)) => self.const_expr_of(ty).unwrap_or_else(|| e.clone()),
                None => e.clone(),
            },
            ConstExpr::Neg(a) => match self.fold_const(a, env)? {
                ConstExpr::Lit(v) => ConstExpr::Lit(v.checked_neg().ok_or("the value overflows")?),
                other => ConstExpr::Neg(Box::new(other)),
            },
            ConstExpr::Bin(op, a, b) => match (self.fold_const(a, env)?, self.fold_const(b, env)?) {
                (ConstExpr::Lit(x), ConstExpr::Lit(y)) => ConstExpr::Lit(apply(*op, x, y)?),
                (a, b) => ConstExpr::Bin(*op, Box::new(a), Box::new(b)),
            },
        })
    }

    /// The length an array type written `[T; len]` has, as a number or, inside a generic body, the interned
    /// expression. Reports E0251 and returns `None` when it cannot be known.
    pub(crate) fn eval_array_len(
        &self,
        len: &ast::Expr,
        subst: &[(String, Ty)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> Option<u64> {
        let lowered = self.lower_const(len, subst, module, 0, diags)?;
        let folded = match self.fold_const(&lowered, &[]) {
            Ok(folded) => folded,
            Err(reason) => {
                diags.error(Code::CONST_EVAL, len.span, format!("the array length cannot be computed: {reason}"));
                return None;
            }
        };
        let value = match &folded {
            ConstExpr::Lit(v) => Some(*v),
            _ => None,
        };
        let result = self.len_of(folded);
        if let (None, Some(v)) = (result, value) {
            diags.error(
                Code::CONST_EVAL,
                len.span,
                format!("the array length {v} is out of range: a length is from 0 to {MAX_LEN}"),
            );
        }
        result
    }

    /// The const argument `arg` of a parameter declared `const NAME: declared`, as a type.
    pub(crate) fn eval_const_arg(
        &self,
        arg: &ast::Type,
        declared: &Ty,
        subst: &[(String, Ty)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> Ty {
        let expr = match &arg.kind {
            ast::TypeKind::Const(e) => self.lower_const(e, subst, module, 0, diags),
            // A bare name is a const parameter in scope or a constant.
            ast::TypeKind::Path(segments)
                if segments.len() == 1
                    && segments[0].args.is_empty()
                    && !subst.iter().any(|(n, _)| *n == segments[0].name.name)
                    && !matches!(self.value(module, &segments[0].name.name), Some(Global::Const(_))) =>
            {
                diags.error(
                    Code::INVALID_CONST_GENERIC,
                    arg.span,
                    format!(
                        "a value is expected here, but `{}` is not a const parameter or a constant",
                        segments[0].name.name
                    ),
                );
                None
            }
            ast::TypeKind::Path(segments) if segments.len() == 1 && segments[0].args.is_empty() => {
                let name = ast::Expr {
                    kind: ast::ExprKind::Name { name: segments[0].name.clone(), args: Vec::new() },
                    span: arg.span,
                };
                self.lower_const(&name, subst, module, 0, diags)
            }
            _ => {
                diags.error(Code::INVALID_CONST_GENERIC, arg.span, "a type is given where a const value is expected");
                None
            }
        };
        let Some(expr) = expr else { return Ty::Error };
        let folded = match self.fold_const(&expr, &[]) {
            Ok(folded) => folded,
            Err(reason) => {
                diags.error(Code::CONST_EVAL, arg.span, format!("the const argument cannot be computed: {reason}"));
                return Ty::Error;
            }
        };
        if let ConstExpr::Lit(v) = folded {
            let fits = match declared {
                // Compared in the type's order, where a `u128` bit pattern above `i128::MAX` is in range (I128-1).
                Ty::Int(int) => (int.key(int.min())..=int.key(int.max())).contains(&int.key(v)),
                Ty::Bool => v == 0 || v == 1,
                _ => true,
            };
            if !fits {
                diags.error(
                    Code::INVALID_CONST_GENERIC,
                    arg.span,
                    format!("the value {v} does not fit the const parameter's type `{declared}`"),
                );
                return Ty::Error;
            }
        }
        self.const_ty(folded)
    }

    /// `ty` with the const parameters of `env` replaced, when `ty` is an interned const expression.
    pub(crate) fn subst_const(&self, value: i128, env: &[(String, Ty)]) -> Ty {
        let Some(expr) = self.const_expr_of(&Ty::Const(value)) else { return Ty::Error };
        match self.fold_const(&expr, env) {
            Ok(folded) => self.const_ty(folded),
            Err(_) => Ty::Error,
        }
    }

    /// An array length with the const parameters of `env` replaced; `None` when the result is not a valid
    /// length.
    pub(crate) fn subst_len(&self, len: u64, env: &[(String, Ty)]) -> Option<u64> {
        let expr = self.len_expr(len)?;
        self.len_of(self.fold_const(&expr, env).ok()?)
    }

    /// Binds the const parameter an array length names to the length of the actual array, so that
    /// `fn first<T, const N: usize>(a: [T; N])` infers `N` from the argument.
    pub(crate) fn unify_len(&self, pattern: u64, actual: u64, bindings: &mut Vec<(String, Ty)>) {
        if pattern >= SYM_LEN
            && actual < SYM_LEN
            && let Some(ConstExpr::Param(name)) = self.len_expr(pattern)
            && !bindings.iter().any(|(m, _)| *m == name)
        {
            bindings.push((name, Ty::Const(i128::from(actual))));
        }
    }

    /// Whether the declared bounds of a parameter say it is a const parameter, and of which type.
    pub(crate) fn const_param_ty(bounds: &[Bound]) -> Option<&Ty> {
        bounds.iter().find_map(|b| match b {
            Bound::ConstOf(ty) => Some(ty),
            _ => None,
        })
    }

    /// The bounds of one generic parameter: its declared interfaces, or, for a const parameter, its type.
    pub(crate) fn generic_param_bounds(
        &self,
        param: &ast::GenericParam,
        subst: &[(String, Ty)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> Vec<Bound> {
        match &param.const_ty {
            Some(ty) => {
                let resolved = self.resolve_type_in(ty, None, subst, module, diags);
                let valid = matches!(resolved, Ty::Int(_) | Ty::Bool | Ty::Char | Ty::Error);
                if !valid {
                    diags.error(
                        Code::INVALID_CONST_GENERIC,
                        ty.span,
                        format!("a const parameter has an integer, `bool`, or `char` type, not `{resolved}`"),
                    );
                }
                vec![Bound::ConstOf(if valid { resolved } else { Ty::Error })]
            }
            None => param.bounds.iter().filter_map(|b| self.resolve_bound(b, subst, module, diags)).collect(),
        }
    }
}

/// One arithmetic operation of a const expression, with overflow and division by zero as errors.
fn apply(op: ast::BinOp, x: i128, y: i128) -> Result<i128, String> {
    use ast::BinOp as B;
    let overflow = || "the value overflows".to_string();
    match op {
        B::Add => x.checked_add(y).ok_or_else(overflow),
        B::Sub => x.checked_sub(y).ok_or_else(overflow),
        B::Mul => x.checked_mul(y).ok_or_else(overflow),
        B::Div | B::Rem if y == 0 => Err("division by zero".to_string()),
        B::Div => x.checked_div(y).ok_or_else(overflow),
        B::Rem => x.checked_rem(y).ok_or_else(overflow),
        B::Shl | B::Shr if !(0..127).contains(&y) => Err("the shift amount is out of range".to_string()),
        B::Shl => x.checked_shl(y as u32).ok_or_else(overflow),
        B::Shr => Ok(x >> y),
        B::BitAnd => Ok(x & y),
        B::BitOr => Ok(x | y),
        B::BitXor => Ok(x ^ y),
        _ => Err("this operator is not allowed in a const expression".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_arithmetic_on_numbers() {
        let g = Globals::default();
        let e = ConstExpr::Bin(ast::BinOp::Mul, Box::new(ConstExpr::Lit(3)), Box::new(ConstExpr::Lit(4)));
        assert_eq!(g.fold_const(&e, &[]), Ok(ConstExpr::Lit(12)));
    }

    #[test]
    fn division_by_zero_and_overflow_are_errors() {
        let g = Globals::default();
        let div = ConstExpr::Bin(ast::BinOp::Div, Box::new(ConstExpr::Lit(1)), Box::new(ConstExpr::Lit(0)));
        assert!(g.fold_const(&div, &[]).is_err());
        let big = ConstExpr::Bin(ast::BinOp::Mul, Box::new(ConstExpr::Lit(i128::MAX)), Box::new(ConstExpr::Lit(2)));
        assert!(g.fold_const(&big, &[]).is_err());
    }

    #[test]
    fn a_parameter_stays_symbolic_until_it_is_substituted() {
        let g = Globals::default();
        let e = ConstExpr::Bin(
            ast::BinOp::Mul,
            Box::new(ConstExpr::Param("R".into())),
            Box::new(ConstExpr::Param("C".into())),
        );
        assert_eq!(g.fold_const(&e, &[]).unwrap(), e);
        let env = vec![("R".to_string(), Ty::Const(2)), ("C".to_string(), Ty::Const(3))];
        assert_eq!(g.fold_const(&e, &env), Ok(ConstExpr::Lit(6)));
    }

    #[test]
    fn symbolic_lengths_are_interned_once_and_substituted() {
        let g = Globals::default();
        let e = ConstExpr::Neg(Box::new(ConstExpr::Param("N".into())));
        let a = g.len_of(e.clone()).unwrap();
        let b = g.len_of(e).unwrap();
        assert_eq!(a, b);
        assert!(a >= SYM_LEN);
        let env = vec![("N".to_string(), Ty::Const(-3))];
        assert_eq!(g.subst_len(a, &env), Some(3));
    }

    #[test]
    fn a_length_parameter_is_inferred_from_an_array() {
        let g = Globals::default();
        let len = g.len_of(ConstExpr::Param("N".into())).unwrap();
        let mut bindings = Vec::new();
        g.unify_len(len, 5, &mut bindings);
        assert_eq!(bindings, vec![("N".to_string(), Ty::Const(5))]);
    }
}
