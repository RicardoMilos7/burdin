//! The built-in types' `Serialize` and `Deserialize` implementations (spec/serialization.md, SER14).
//!
//! A built-in type has no impl block (an impl block names a struct or enum of its package), so the checker answers
//! for it: a scalar, a string, bytes, a `List`, a `Map<String, V>`, an `Option`, a `Result`, and a tuple of 2 to 4
//! parts implement both interfaces when their parts do. A call writes or reads the value through the standard
//! library's helper for its shape, the same way `builtin_format_method` formats one (`fncx/format.rs`).

use crate::generics::{Bound, Env};
use crate::globals::Globals;
use burdin_tir::{FloatTy, IntTy, Ty};

impl Globals {
    /// Whether a built-in type implements `Serialize` (`deserialize` false) or `Deserialize` (`deserialize` true)
    /// without an impl block. `None` for a struct or enum, which implements them with a block or a derive.
    pub(crate) fn builtin_serial(&self, ty: &Ty, iface: usize, deserialize: bool, env: &Env) -> Option<bool> {
        let part = |t: &Ty| self.satisfies_with(t, &Bound::User(iface, Vec::new()), env, &[]);
        Some(match ty {
            Ty::Error | Ty::Never => true,
            Ty::Unit | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Float(_) | Ty::String => true,
            // `str` is written but never read: a reader returns an owned `String` (SER11).
            Ty::Str => !deserialize,
            // The bytes shape is `List<u8>` and needs no separate case: `u8` implements both interfaces.
            Ty::List(t, alloc) => alloc.is_global() && part(t),
            Ty::Map(k, v, alloc) => alloc.is_global() && **k == Ty::String && part(v),
            Ty::Option(t) => part(t),
            Ty::Result(t, e) => part(t) && part(e),
            Ty::Tuple(ts) => (2..=4).contains(&ts.len()) && ts.iter().all(part),
            Ty::Struct(_) | Ty::Enum(_) => return None,
            _ => false,
        })
    }

    /// The shape name of a built-in type, which names its helpers: `i32` for `i32`, `bytes` for `List<u8>`, `list`
    /// for any other `List`, `tuple2` for a pair. `None` for a type that has no shape.
    pub(crate) fn serial_shape(ty: &Ty) -> Option<String> {
        Some(match ty {
            Ty::Unit => "unit".to_string(),
            Ty::Bool => "bool".to_string(),
            Ty::Char => "char".to_string(),
            Ty::Int(i) => i.name().to_string(),
            Ty::Float(FloatTy::F32) => "f32".to_string(),
            Ty::Float(FloatTy::F64) => "f64".to_string(),
            Ty::String => "string".to_string(),
            Ty::Str => "str".to_string(),
            Ty::List(t, _) if **t == Ty::Int(IntTy::U8) => "bytes".to_string(),
            Ty::List(..) => "list".to_string(),
            Ty::Map(..) => "map".to_string(),
            Ty::Option(_) => "option".to_string(),
            Ty::Result(..) => "result".to_string(),
            Ty::Tuple(ts) if (2..=4).contains(&ts.len()) => format!("tuple{}", ts.len()),
            _ => return None,
        })
    }

    /// The standard library's receiver-first helper that `value.serialize(mut s)` calls for a built-in type
    /// (`serialize_value_<shape>` in `std/src/serialize.bd`).
    pub(crate) fn serialize_helper(ty: &Ty) -> Option<String> {
        Some(format!("serialize_value_{}", Self::serial_shape(ty)?))
    }

    /// The standard library's helper that `T.deserialize(mut d)` calls for a built-in type: the `deserialize_<shape>`
    /// function of `std/src/serialize.bd`, where a string is read by `deserialize_str`.
    pub(crate) fn deserialize_helper(ty: &Ty) -> Option<String> {
        let shape = Self::serial_shape(ty)?;
        Some(match shape.as_str() {
            "string" => "deserialize_str".to_string(),
            _ => format!("deserialize_{shape}"),
        })
    }
}
