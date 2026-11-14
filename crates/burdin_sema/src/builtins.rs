//! The built-in functions, types, and methods.

use burdin_tir::{Builtin, FloatTy, IntTy, Method, ParamSig, Signature, Ty};

pub(crate) fn builtin(name: &str) -> Option<Builtin> {
    Some(match name {
        "print" => Builtin::Print,
        "println" => Builtin::Println,
        "eprint" => Builtin::Eprint,
        "eprintln" => Builtin::Eprintln,
        "panic" => Builtin::Panic,
        "__read_to_string" => Builtin::ReadToString,
        "__write_file" => Builtin::WriteFile,
        "__args" => Builtin::EnvArgs,
        "__exit" => Builtin::ProcessExit,
        "__run" => Builtin::RunCommand,
        _ => return None,
    })
}

pub(crate) fn builtin_sig(b: Builtin) -> Signature {
    let ret = match b {
        Builtin::Panic | Builtin::Unreachable | Builtin::ProcessExit => Ty::Never,
        Builtin::StringNew | Builtin::StringFrom | Builtin::CStrToString => Ty::String,
        Builtin::CStringFrom => Ty::CString,
        Builtin::ReadToString => Ty::Result(Box::new(Ty::String), Box::new(Ty::Unit)),
        Builtin::WriteFile => Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Unit)),
        Builtin::EnvArgs => Ty::List(Box::new(Ty::String)),
        Builtin::RunCommand => Ty::Result(Box::new(Ty::Int(IntTy::I32)), Box::new(Ty::Unit)),
        _ => Ty::Unit,
    };
    let params = match b {
        Builtin::StringNew | Builtin::EnvArgs | Builtin::Unreachable => Vec::new(),
        Builtin::StringFrom | Builtin::CStringFrom => vec![ParamSig::plain(Ty::Str)],
        Builtin::CStrToString => vec![ParamSig::plain(Ty::CPtr(Box::new(Ty::Int(IntTy::I8))))],
        Builtin::WriteFile => vec![ParamSig::plain(Ty::Str), ParamSig::plain(Ty::Str)],
        Builtin::ReadToString | Builtin::RunCommand => vec![ParamSig::plain(Ty::Str)],
        Builtin::Assert => vec![ParamSig::plain(Ty::Bool)],
        Builtin::ProcessExit => vec![ParamSig::plain(Ty::Int(IntTy::I32))],
        // `assert_eq` checks its two arguments specially: their type is inferred, not fixed.
        Builtin::AssertEq => Vec::new(),
        // `Heap.new` is checked separately: its result type depends on the value.
        Builtin::HeapNew => vec![ParamSig::plain(Ty::Error)],
        // `swap` and `replace` check their arguments separately.
        Builtin::Swap | Builtin::Replace => Vec::new(),
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

/// A parameter the callee takes ownership of (the element of `List.push`, the key and value of
/// `Map.insert`).
fn value_param(ty: Ty) -> ParamSig {
    ParamSig { ty, by_ref: false, mut_param: false, own: true, exclusive: true }
}

/// The built-in method `name` on a receiver of type `recv`: which method it is, its parameters after the
/// receiver, and its result. `None` when `recv` has no such method.
pub(crate) fn method(name: &str, recv: &Ty) -> Option<(Method, Vec<ParamSig>, Ty)> {
    let plain = |ty: Ty| ParamSig::plain(ty);
    let elem = |t: &Ty| t.clone();
    Some(match (name, recv) {
        ("to_string", Ty::Int(_) | Ty::Bool | Ty::Char | Ty::Str) => (Method::ToString, vec![], Ty::String),
        ("wrapping_add", Ty::Int(_)) => (Method::WrappingAdd, vec![plain(recv.clone())], recv.clone()),
        ("wrapping_sub", Ty::Int(_)) => (Method::WrappingSub, vec![plain(recv.clone())], recv.clone()),
        ("wrapping_mul", Ty::Int(_)) => (Method::WrappingMul, vec![plain(recv.clone())], recv.clone()),
        ("checked_add", Ty::Int(_)) => {
            (Method::CheckedAdd, vec![plain(recv.clone())], Ty::Option(Box::new(recv.clone())))
        }
        ("checked_sub", Ty::Int(_)) => {
            (Method::CheckedSub, vec![plain(recv.clone())], Ty::Option(Box::new(recv.clone())))
        }
        ("checked_mul", Ty::Int(_)) => {
            (Method::CheckedMul, vec![plain(recv.clone())], Ty::Option(Box::new(recv.clone())))
        }
        ("min", Ty::Int(_)) => (Method::Min, vec![plain(recv.clone())], recv.clone()),
        ("max", Ty::Int(_)) => (Method::Max, vec![plain(recv.clone())], recv.clone()),
        ("abs", Ty::Int(i)) if i.signed() => (Method::Abs, vec![], recv.clone()),
        ("push_str", Ty::String) => (Method::PushStr, vec![plain(Ty::Str)], Ty::Unit),
        ("push", Ty::String) => (Method::PushChar, vec![plain(Ty::Char)], Ty::Unit),
        ("push", Ty::List(t)) => (Method::Push, vec![value_param(elem(t))], Ty::Unit),
        ("pop", Ty::List(t)) => (Method::ListPop, vec![], Ty::Option(t.clone())),
        ("last", Ty::List(t)) => {
            (Method::ListLast, vec![], Ty::Option(Box::new(Ty::Ref { mutable: false, inner: t.clone() })))
        }
        ("insert", Ty::List(t)) => {
            (Method::ListInsert, vec![plain(Ty::Int(IntTy::Usize)), value_param(elem(t))], Ty::Unit)
        }
        ("remove", Ty::List(t)) => (Method::ListRemove, vec![plain(Ty::Int(IntTy::Usize))], elem(t)),
        ("contains", Ty::List(t)) => (Method::ListContains, vec![plain(elem(t))], Ty::Bool),
        ("sort", Ty::List(_)) => (Method::ListSort, vec![], Ty::Unit),
        ("clear", Ty::List(_) | Ty::Map(..)) => (Method::Clear, vec![], Ty::Unit),
        ("insert", Ty::Map(k, v)) => (Method::MapInsert, vec![value_param(elem(k)), value_param(elem(v))], Ty::Unit),
        ("get", Ty::Map(k, v)) => {
            let view = Ty::Ref { mutable: false, inner: v.clone() };
            (Method::MapGet, vec![plain(elem(k))], Ty::Option(Box::new(view)))
        }
        ("remove", Ty::Map(k, v)) => (Method::MapRemove, vec![plain(elem(k))], Ty::Option(v.clone())),
        ("contains_key", Ty::Map(k, _)) => (Method::MapContainsKey, vec![plain(elem(k))], Ty::Bool),
        ("unwrap", Ty::Option(t)) => (Method::Unwrap, vec![], elem(t)),
        ("unwrap", Ty::Result(t, _)) => (Method::Unwrap, vec![], elem(t)),
        ("expect", Ty::Option(t)) => (Method::Expect, vec![plain(Ty::Str)], elem(t)),
        ("expect", Ty::Result(t, _)) => (Method::Expect, vec![plain(Ty::Str)], elem(t)),
        ("is_some", Ty::Option(_)) => (Method::IsSome, vec![], Ty::Bool),
        ("is_none", Ty::Option(_)) => (Method::IsNone, vec![], Ty::Bool),
        ("is_ok", Ty::Result(..)) => (Method::IsOk, vec![], Ty::Bool),
        ("is_err", Ty::Result(..)) => (Method::IsErr, vec![], Ty::Bool),
        ("as_str", Ty::String | Ty::Str) => (Method::AsStr, vec![], Ty::Str),
        ("len", Ty::Str | Ty::String | Ty::Slice(_) | Ty::Array(_, _) | Ty::List(_) | Ty::Map(..)) => {
            (Method::Len, vec![], Ty::Int(IntTy::Usize))
        }
        ("starts_with", Ty::Str | Ty::String) => (Method::StartsWith, vec![plain(Ty::Str)], Ty::Bool),
        ("find", Ty::Str | Ty::String) => {
            (Method::Find, vec![plain(Ty::Char)], Ty::Option(Box::new(Ty::Int(IntTy::Usize))))
        }
        ("find_str", Ty::Str | Ty::String) => {
            (Method::FindStr, vec![plain(Ty::Str)], Ty::Option(Box::new(Ty::Int(IntTy::Usize))))
        }
        ("chars", Ty::Str | Ty::String) => (Method::Chars, vec![], Ty::Chars),
        ("as_ptr", Ty::CString) => (Method::AsPtr, vec![], Ty::CPtr(Box::new(Ty::Int(IntTy::I8)))),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_type_names_resolve() {
        assert_eq!(builtin_type("i32"), Some(Ty::Int(IntTy::I32)));
        assert_eq!(builtin_type("usize"), Some(Ty::Int(IntTy::Usize)));
        assert_eq!(builtin_type("f64"), Some(Ty::Float(FloatTy::F64)));
        assert_eq!(builtin_type("bool"), Some(Ty::Bool));
        assert_eq!(builtin_type("char"), Some(Ty::Char));
        assert_eq!(builtin_type("String"), Some(Ty::String));
        assert_eq!(builtin_type("CString"), Some(Ty::CString));
        assert_eq!(builtin_type("CChar"), Some(Ty::Int(IntTy::I8)));
        assert_eq!(builtin_type("CSize"), Some(Ty::Int(IntTy::Usize)));
        assert_eq!(builtin_type("CVoid"), Some(Ty::Unit));
        assert_eq!(builtin_type("Point"), None);
    }

    #[test]
    fn method_resolution_dispatches_on_the_receiver() {
        let i32_ = Ty::Int(IntTy::I32);
        let (found, params, ret) = method("to_string", &i32_).unwrap();
        assert_eq!(found, Method::ToString);
        assert!(params.is_empty());
        assert_eq!(ret, Ty::String);
        assert!(method("to_string", &Ty::String).is_none(), "String already is a string");
        assert!(method("abs", &Ty::Int(IntTy::U32)).is_none(), "unsigned integers have no abs");
        assert!(method("min", &Ty::Float(FloatTy::F64)).is_none(), "min is integer-only");
        assert!(method("nope", &i32_).is_none());

        let list = Ty::List(Box::new(i32_.clone()));
        let (found, params, ret) = method("push", &list).unwrap();
        assert_eq!(found, Method::Push);
        assert!(params[0].own, "push takes its element by ownership");
        assert_eq!(ret, Ty::Unit);
        let (found, _, ret) = method("pop", &list).unwrap();
        assert_eq!(found, Method::ListPop);
        assert_eq!(ret, Ty::Option(Box::new(i32_.clone())));
        let (found, _, ret) = method("last", &list).unwrap();
        assert_eq!(found, Method::ListLast);
        assert_eq!(ret, Ty::Option(Box::new(Ty::Ref { mutable: false, inner: Box::new(i32_.clone()) })));

        let (found, _, ret) = method("checked_add", &i32_).unwrap();
        assert_eq!(found, Method::CheckedAdd);
        assert_eq!(ret, Ty::Option(Box::new(i32_.clone())));
        let (found, _, ret) = method("unwrap", &Ty::Option(Box::new(i32_.clone()))).unwrap();
        assert_eq!(found, Method::Unwrap);
        assert_eq!(ret, i32_);
    }
}
