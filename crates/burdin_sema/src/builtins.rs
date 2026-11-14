//! The built-in functions, types, and methods.

use burdin_tir::{Builtin, FloatTy, IntTy, Method, ParamSig, Signature, Ty};

pub(crate) fn builtin(name: &str) -> Option<Builtin> {
    Some(match name {
        "print" => Builtin::Print,
        "println" => Builtin::Println,
        "eprint" => Builtin::Eprint,
        "eprintln" => Builtin::Eprintln,
        "panic" => Builtin::Panic,
        _ => return None,
    })
}

pub(crate) fn builtin_sig(b: Builtin) -> Signature {
    let ret = match b {
        Builtin::Panic => Ty::Never,
        Builtin::StringNew | Builtin::StringFrom => Ty::String,
        Builtin::CStringFrom => Ty::CString,
        _ => Ty::Unit,
    };
    let params = match b {
        Builtin::StringNew => Vec::new(),
        Builtin::StringFrom | Builtin::CStringFrom => vec![ParamSig::plain(Ty::Str)],
        // `Heap.new` is checked separately: its result type depends on the value.
        Builtin::HeapNew => vec![ParamSig::plain(Ty::Error)],
        _ => vec![ParamSig::plain(Ty::Str)],
    };
    Signature { params, ret, has_self: false, ret_origin: None }
}

pub(crate) fn int_ty(name: &str) -> Option<IntTy> {
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

/// The built-in types that are written as a plain name.
pub(crate) fn builtin_type(name: &str) -> Option<Ty> {
    if let Some(i) = int_ty(name) {
        return Some(Ty::Int(i));
    }
    Some(match name {
        "str" => Ty::Str,
        "bool" => Ty::Bool,
        "char" => Ty::Char,
        "f32" => Ty::Float(FloatTy::F32),
        "f64" => Ty::Float(FloatTy::F64),
        "String" => Ty::String,
        // The C bridge. `CChar` is a signed byte and `CSize` is `size_t`; `CVoid` only appears as the pointee
        // of a `CPtr`, where it has no value of its own.
        "CString" => Ty::CString,
        "CChar" => Ty::Int(IntTy::I8),
        "CSize" => Ty::Int(IntTy::Usize),
        "CVoid" => Ty::Unit,
        _ => return None,
    })
}

pub(crate) fn method(name: &str) -> Option<Method> {
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
        "push_str" => Method::PushStr,
        "as_str" => Method::AsStr,
        "len" => Method::Len,
        "starts_with" => Method::StartsWith,
        "find" => Method::Find,
        "as_ptr" => Method::AsPtr,
        _ => return None,
    })
}

/// The parameter types (after the receiver) and the result type of a built-in method, if `recv` has it.
pub(crate) fn method_sig(m: Method, recv: &Ty) -> Option<(Vec<Ty>, Ty)> {
    match (m, recv) {
        (Method::ToString, Ty::Int(_) | Ty::Bool | Ty::Char | Ty::Str) => Some((vec![], Ty::String)),
        (Method::WrappingAdd | Method::WrappingSub | Method::WrappingMul | Method::Min | Method::Max, Ty::Int(_)) => {
            Some((vec![recv.clone()], recv.clone()))
        }
        (Method::CheckedAdd | Method::CheckedSub | Method::CheckedMul, Ty::Int(_)) => {
            Some((vec![recv.clone()], Ty::Option(Box::new(recv.clone()))))
        }
        (Method::Abs, Ty::Int(i)) if i.signed() => Some((vec![], recv.clone())),
        (Method::PushStr, Ty::String) => Some((vec![Ty::Str], Ty::Unit)),
        (Method::AsStr, Ty::String | Ty::Str) => Some((vec![], Ty::Str)),
        (Method::Len, Ty::Str | Ty::String | Ty::Slice(_) | Ty::Array(_, _)) => Some((vec![], Ty::Int(IntTy::Usize))),
        (Method::StartsWith, Ty::Str | Ty::String) => Some((vec![Ty::Str], Ty::Bool)),
        (Method::Find, Ty::Str | Ty::String) => Some((vec![Ty::Char], Ty::Option(Box::new(Ty::Int(IntTy::Usize))))),
        (Method::AsPtr, Ty::CString) => Some((vec![], Ty::CPtr(Box::new(Ty::Int(IntTy::I8))))),
        _ => None,
    }
}
