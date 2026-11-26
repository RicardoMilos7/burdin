//! Materialization (`spec/const-eval.md`, CE3): a compile-time value turned back into a typed-tree expression that
//! builds the same value at run time.

use crate::value::Value;
use burdin_diag::Span;
use burdin_tir::{Builtin, Expr, ExprKind as K, Program, Ty};

/// The expression that rebuilds `v`, a value of type `ty`. `Err` names what could not be materialized; the
/// checker rejects such types, so it only happens on an internal inconsistency.
pub(crate) fn reify(p: &Program, v: &Value, ty: &Ty, span: Span) -> Result<Expr, String> {
    let mk = |kind: K| Expr { kind, ty: ty.clone(), span };
    let v = crate::prims::shallow(v);
    Ok(match (ty, v) {
        (Ty::Unit, _) => mk(K::Unit),
        (Ty::Bool, Value::Bool(b)) => mk(K::Bool(*b)),
        (Ty::Char, Value::Char(c)) => mk(K::Char(*c)),
        (Ty::Int(_), Value::Int(n)) => mk(K::Int(*n)),
        (Ty::Float(_), Value::Float(f)) => mk(K::Float(*f)),
        (Ty::Str, Value::Str(_) | Value::String(_)) => mk(K::Str(text(v)?)),
        (Ty::String, Value::Str(_) | Value::String(_)) => {
            let lit = Expr { kind: K::Str(text(v)?), ty: Ty::Str, span };
            mk(K::Builtin(Builtin::StringFrom, vec![lit]))
        }
        (Ty::Tuple(ts), Value::Tuple(fs)) if ts.len() == fs.len() => {
            let mut out = Vec::with_capacity(fs.len());
            for (f, t) in fs.iter().zip(ts) {
                out.push(reify(p, f, t, span)?);
            }
            mk(K::Tuple(out))
        }
        (Ty::Struct(a), Value::Tuple(fs)) => {
            let defs = &p.structs[a.id].fields;
            let mut out = Vec::with_capacity(fs.len());
            for (i, (f, d)) in fs.iter().zip(defs).enumerate() {
                out.push((i, reify(p, f, &d.ty, span)?));
            }
            mk(K::Struct(out))
        }
        (Ty::Enum(a), Value::Variant(vi, fs)) => {
            let defs = &p.enums[a.id].variants[*vi].fields;
            let mut out = Vec::with_capacity(fs.len());
            for (i, (f, d)) in fs.iter().zip(defs).enumerate() {
                out.push((i, reify(p, f, &d.ty, span)?));
            }
            mk(K::Variant(*vi, out))
        }
        (Ty::Array(t, _), Value::Array(xs)) => {
            let mut out = Vec::with_capacity(xs.len());
            for x in xs {
                out.push(reify(p, x, t, span)?);
            }
            mk(K::Array(out))
        }
        (Ty::Range(t, inclusive), Value::Tuple(ends)) if ends.len() == 2 => mk(K::Range {
            lo: Box::new(reify(p, &ends[0], t, span)?),
            hi: Box::new(reify(p, &ends[1], t, span)?),
            inclusive: *inclusive,
        }),
        (ty, v) => return Err(format!("a value of type `{ty}` cannot be materialized from {v:?}")),
    })
}

fn text(v: &Value) -> Result<String, String> {
    String::from_utf8(v.bytes().to_vec()).map_err(|_| "a string that is not valid UTF-8".to_string())
}
