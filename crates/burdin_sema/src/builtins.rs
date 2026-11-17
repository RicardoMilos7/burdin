//! The built-in functions, types, and methods.

use crate::globals::Globals;
use burdin_tir::{Builtin, FloatTy, FnKind, FnSig, IntTy, Method, ParamSig, Signature, Ty};
use std::rc::Rc;

pub(crate) fn builtin(name: &str) -> Option<Builtin> {
    Some(match name {
        "print" => Builtin::Print,
        "println" => Builtin::Println,
        "eprint" => Builtin::Eprint,
        "eprintln" => Builtin::Eprintln,
        "panic" => Builtin::Panic,
        "__read_to_string" => Builtin::ReadToString,
        "__write_file" => Builtin::WriteFile,
        "__stdout_write" => Builtin::StdoutWrite,
        "__stdout_write_line" => Builtin::StdoutWriteLine,
        "__stdout_flush" => Builtin::StdoutFlush,
        "__stdout_write_bytes" => Builtin::StdoutWriteBytes,
        "__stderr_write_bytes" => Builtin::StderrWriteBytes,
        "__stderr_write" => Builtin::StderrWrite,
        "__stderr_write_line" => Builtin::StderrWriteLine,
        "__is_terminal" => Builtin::IsTerminal,
        "__stdin_read_line" => Builtin::StdinReadLine,
        "__stdin_read_to_string" => Builtin::StdinReadToString,
        "__stdin_avail" => Builtin::StdinAvail,
        "__stdin_next_byte" => Builtin::StdinNextByte,
        "__read_dir" => Builtin::ReadDir,
        "__create_dir" => Builtin::CreateDir,
        "__remove_dir" => Builtin::RemoveDir,
        "__remove_file" => Builtin::RemoveFile,
        "__rename" => Builtin::RenamePath,
        "__metadata" => Builtin::Metadata,
        "__canonicalize" => Builtin::Canonicalize,
        "__args" => Builtin::EnvArgs,
        "__env_get" => Builtin::EnvGet,
        "__env_set" => Builtin::EnvSet,
        "__env_set_bytes" => Builtin::EnvSetBytes,
        "__env_remove" => Builtin::EnvRemove,
        "__current_dir" => Builtin::CurrentDir,
        "__set_current_dir" => Builtin::SetCurrentDir,
        "__set_current_dir_bytes" => Builtin::SetCurrentDirBytes,
        "__float_str" => Builtin::FloatStr,
        "__exit" => Builtin::ProcessExit,
        "__run" => Builtin::RunCommand,
        "__target_os" => Builtin::TargetOs,
        "__target_arch" => Builtin::TargetArch,
        "__alloc" => Builtin::RawAlloc,
        "__realloc" => Builtin::RawRealloc,
        "__dealloc" => Builtin::RawDealloc,
        _ => return None,
    })
}

pub(crate) fn builtin_sig(b: Builtin, globals: &Globals) -> Signature {
    // The I/O primitives report the standard library's `IoError` when it is there (`spec/errors.md`, ERR10). The
    // primitives produce a zeroed error value; the `std` wrappers replace it with the real variant.
    let io_error = || globals.io_error_ty.clone().unwrap_or(Ty::Unit);
    let ret = match b {
        Builtin::Panic | Builtin::Unreachable | Builtin::ProcessExit => Ty::Never,
        Builtin::TargetOs | Builtin::TargetArch => Ty::Str,
        Builtin::StringNew | Builtin::StringFrom | Builtin::CStrToString | Builtin::FloatStr => Ty::String,
        Builtin::CStringFrom => Ty::CString,
        Builtin::ReadToString => Ty::Result(Box::new(Ty::String), Box::new(io_error())),
        Builtin::WriteFile => Ty::Result(Box::new(Ty::Unit), Box::new(io_error())),
        Builtin::StdinReadLine => Ty::Result(Box::new(Ty::Option(Box::new(Ty::String))), Box::new(io_error())),
        Builtin::StdinReadToString => Ty::Result(Box::new(Ty::String), Box::new(io_error())),
        Builtin::IsTerminal => Ty::Bool,
        Builtin::StdinAvail => Ty::Int(IntTy::Isize),
        Builtin::StdinNextByte => Ty::Int(IntTy::U8),
        Builtin::ReadDir => {
            Ty::Result(Box::new(Ty::List(Box::new(Ty::String), burdin_tir::Alloc::Global)), Box::new(io_error()))
        }
        Builtin::CreateDir | Builtin::RemoveDir | Builtin::RemoveFile | Builtin::RenamePath => {
            Ty::Result(Box::new(Ty::Unit), Box::new(io_error()))
        }
        // `(size: u64, mode: u64, modified: i64, accessed: i64, flags: u64)`: the raw `stat` fields, which
        // `std` turns into `Metadata`.
        Builtin::Metadata => Ty::Result(
            Box::new(Ty::Tuple(vec![
                Ty::Int(IntTy::U64),
                Ty::Int(IntTy::U64),
                Ty::Int(IntTy::I64),
                Ty::Int(IntTy::I64),
                Ty::Int(IntTy::U64),
            ])),
            Box::new(io_error()),
        ),
        Builtin::Canonicalize => Ty::Result(Box::new(Ty::String), Box::new(io_error())),
        Builtin::EnvArgs => Ty::List(Box::new(Ty::String), burdin_tir::Alloc::Global),
        Builtin::EnvGet | Builtin::CurrentDir => {
            Ty::Option(Box::new(Ty::List(Box::new(Ty::Int(IntTy::U8)), burdin_tir::Alloc::Global)))
        }
        Builtin::EnvSet
        | Builtin::EnvSetBytes
        | Builtin::EnvRemove
        | Builtin::SetCurrentDir
        | Builtin::SetCurrentDirBytes => Ty::Int(IntTy::I32),
        Builtin::RunCommand => Ty::Result(Box::new(Ty::Int(IntTy::I32)), Box::new(io_error())),
        Builtin::RawAlloc | Builtin::RawRealloc => Ty::CPtr(Box::new(Ty::Int(IntTy::U8))),
        _ => Ty::Unit,
    };
    let params = match b {
        Builtin::StringNew
        | Builtin::EnvArgs
        | Builtin::Unreachable
        | Builtin::TargetOs
        | Builtin::TargetArch
        | Builtin::CurrentDir
        | Builtin::StdoutFlush
        | Builtin::StdinReadLine
        | Builtin::StdinReadToString
        | Builtin::StdinAvail
        | Builtin::StdinNextByte => Vec::new(),
        Builtin::IsTerminal => vec![ParamSig::plain(Ty::Int(IntTy::I32))],
        Builtin::StdoutWrite | Builtin::StdoutWriteLine | Builtin::StderrWrite | Builtin::StderrWriteLine => {
            vec![ParamSig::plain(Ty::Str)]
        }
        Builtin::StdoutWriteBytes | Builtin::StderrWriteBytes => {
            vec![ParamSig::plain(Ty::Slice(Box::new(Ty::Int(IntTy::U8))))]
        }
        // The environment primitives take the raw text; the `std` wrappers add the UTF-8 checks and the error
        // values (`spec/stdlib.md`, "Environment variables and the working directory").
        Builtin::EnvGet | Builtin::EnvRemove | Builtin::SetCurrentDir => vec![ParamSig::plain(Ty::Str)],
        Builtin::EnvSet => vec![ParamSig::plain(Ty::Str), ParamSig::plain(Ty::Str)],
        Builtin::EnvSetBytes => {
            vec![ParamSig::plain(Ty::Str), ParamSig::plain(Ty::Slice(Box::new(Ty::Int(IntTy::U8))))]
        }
        Builtin::SetCurrentDirBytes => vec![ParamSig::plain(Ty::Slice(Box::new(Ty::Int(IntTy::U8))))],
        Builtin::StringFrom | Builtin::CStringFrom => vec![ParamSig::plain(Ty::Str)],
        Builtin::CStrToString => vec![ParamSig::plain(Ty::CPtr(Box::new(Ty::Int(IntTy::I8))))],
        Builtin::WriteFile => vec![ParamSig::plain(Ty::Str), ParamSig::plain(Ty::Str)],
        Builtin::RenamePath => vec![ParamSig::plain(Ty::Str), ParamSig::plain(Ty::Str)],
        Builtin::ReadToString | Builtin::RunCommand => vec![ParamSig::plain(Ty::Str)],
        Builtin::Assert => vec![ParamSig::plain(Ty::Bool)],
        Builtin::ProcessExit => vec![ParamSig::plain(Ty::Int(IntTy::I32))],
        Builtin::RawAlloc => vec![ParamSig::plain(Ty::Int(IntTy::Usize)), ParamSig::plain(Ty::Int(IntTy::Usize))],
        Builtin::RawRealloc => vec![
            ParamSig::plain(Ty::CPtr(Box::new(Ty::Int(IntTy::U8)))),
            ParamSig::plain(Ty::Int(IntTy::Usize)),
            ParamSig::plain(Ty::Int(IntTy::Usize)),
            ParamSig::plain(Ty::Int(IntTy::Usize)),
        ],
        Builtin::RawDealloc => vec![
            ParamSig::plain(Ty::CPtr(Box::new(Ty::Int(IntTy::U8)))),
            ParamSig::plain(Ty::Int(IntTy::Usize)),
            ParamSig::plain(Ty::Int(IntTy::Usize)),
        ],
        Builtin::FloatStr => vec![
            ParamSig::plain(Ty::Float(FloatTy::F64)),
            ParamSig::plain(Ty::Int(IntTy::I32)),
            ParamSig::plain(Ty::Bool),
        ],
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
    ParamSig { own: true, exclusive: true, ..ParamSig::plain(ty) }
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
        ("push", Ty::List(t, _)) => (Method::Push, vec![value_param(elem(t))], Ty::Unit),
        ("pop", Ty::List(t, _)) => (Method::ListPop, vec![], Ty::Option(t.clone())),
        ("last", Ty::List(t, _)) => {
            (Method::ListLast, vec![], Ty::Option(Box::new(Ty::Ref { mutable: false, inner: t.clone() })))
        }
        ("insert", Ty::List(t, _)) => {
            (Method::ListInsert, vec![plain(Ty::Int(IntTy::Usize)), value_param(elem(t))], Ty::Unit)
        }
        ("remove", Ty::List(t, _)) => (Method::ListRemove, vec![plain(Ty::Int(IntTy::Usize))], elem(t)),
        ("contains", Ty::List(t, _)) => (Method::ListContains, vec![plain(elem(t))], Ty::Bool),
        ("sort", Ty::List(_, _)) => (Method::ListSort, vec![], Ty::Unit),
        ("clear", Ty::List(_, _) | Ty::Map(..)) => (Method::Clear, vec![], Ty::Unit),
        ("insert", Ty::Map(k, v, _)) => (Method::MapInsert, vec![value_param(elem(k)), value_param(elem(v))], Ty::Unit),
        ("get", Ty::Map(k, v, _)) => {
            let view = Ty::Ref { mutable: false, inner: v.clone() };
            (Method::MapGet, vec![plain(elem(k))], Ty::Option(Box::new(view)))
        }
        ("get_or_insert", Ty::Map(k, v, _)) => {
            let view = Ty::Ref { mutable: true, inner: v.clone() };
            (Method::MapGetOrInsert, vec![value_param(elem(k)), value_param(elem(v))], view)
        }
        ("get_mut", Ty::Map(k, v, _)) => {
            let view = Ty::Ref { mutable: true, inner: v.clone() };
            (Method::MapGetMut, vec![plain(elem(k))], Ty::Option(Box::new(view)))
        }
        // The entry-style API (`spec/stdlib.md`, "Map entry operations"): one probe, then insert or not.
        ("entry", Ty::Map(k, v, alloc)) => {
            (Method::MapEntry, vec![value_param(elem(k))], Ty::MapEntry(k.clone(), v.clone(), alloc.clone()))
        }
        ("or_insert", Ty::MapEntry(_, v, _)) => {
            let view = Ty::Ref { mutable: true, inner: v.clone() };
            (Method::MapEntryOrInsert, vec![value_param(elem(v))], view)
        }
        ("or_insert_with", Ty::MapEntry(_, v, _)) => {
            let view = Ty::Ref { mutable: true, inner: v.clone() };
            let make = Ty::Fn {
                sig: Rc::new(FnSig { params: Vec::new(), ret: v.clone() }),
                kind: FnKind::FnOnce,
                borrowed: false,
            };
            (Method::MapEntryOrInsertWith, vec![value_param(make)], view)
        }
        ("remove", Ty::Map(k, v, _)) => (Method::MapRemove, vec![plain(elem(k))], Ty::Option(v.clone())),
        ("contains_key", Ty::Map(k, _, _)) => (Method::MapContainsKey, vec![plain(elem(k))], Ty::Bool),
        ("unwrap", Ty::Option(t)) => (Method::Unwrap, vec![], elem(t)),
        ("unwrap", Ty::Result(t, _)) => (Method::Unwrap, vec![], elem(t)),
        ("expect", Ty::Option(t)) => (Method::Expect, vec![plain(Ty::Str)], elem(t)),
        ("expect", Ty::Result(t, _)) => (Method::Expect, vec![plain(Ty::Str)], elem(t)),
        ("is_some", Ty::Option(_)) => (Method::IsSome, vec![], Ty::Bool),
        ("is_none", Ty::Option(_)) => (Method::IsNone, vec![], Ty::Bool),
        ("is_ok", Ty::Result(..)) => (Method::IsOk, vec![], Ty::Bool),
        ("is_err", Ty::Result(..)) => (Method::IsErr, vec![], Ty::Bool),
        ("as_str", Ty::String | Ty::Str) => (Method::AsStr, vec![], Ty::Str),
        ("len", Ty::Str | Ty::String | Ty::Slice(_) | Ty::Array(_, _) | Ty::List(_, _) | Ty::Map(..)) => {
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
        ("is_null", Ty::CPtr(_)) => (Method::IsNull, vec![], Ty::Bool),
        // `Shared<T>` (`spec/allocators.md`, AL8).
        ("get", Ty::Shared(t)) => (Method::SharedGet, vec![], Ty::Ref { mutable: false, inner: t.clone() }),
        ("strong_count", Ty::Shared(_)) => (Method::SharedStrongCount, vec![], Ty::Int(IntTy::Usize)),
        // The iteration protocol's entry points (spec/iterators.md). `next` is checked separately because a
        // slice yields its element by value when it is `Copy`.
        ("iter", Ty::Array(elem, _)) => (Method::Iter, vec![], Ty::Slice(elem.clone())),
        ("iter", Ty::Slice(_)) => (Method::Iter, vec![], recv.clone()),
        ("iter", Ty::List(elem, _)) => (Method::Iter, vec![], Ty::Slice(elem.clone())),
        ("iter", Ty::Map(k, v, _)) => (Method::Iter, vec![], Ty::MapIter(k.clone(), v.clone())),
        ("iter", Ty::Chars) => (Method::Iter, vec![], Ty::Chars),
        ("iter", Ty::Range(elem, inclusive)) => (Method::Iter, vec![], Ty::RangeIter(elem.clone(), *inclusive)),
        ("into_iter", Ty::List(elem, alloc)) => {
            (Method::IntoIter, vec![], Ty::ListIntoIter(elem.clone(), alloc.clone()))
        }
        ("into_iter", Ty::Map(k, v, alloc)) => {
            (Method::IntoIter, vec![], Ty::MapIntoIter(k.clone(), v.clone(), alloc.clone()))
        }
        ("into_iter", Ty::Array(elem, n)) => (Method::IntoIter, vec![], Ty::ArrayIntoIter(elem.clone(), *n)),
        ("into_iter", Ty::Chars) => (Method::IntoIter, vec![], Ty::Chars),
        ("into_iter", Ty::Range(elem, inclusive)) => {
            (Method::IntoIter, vec![], Ty::RangeIter(elem.clone(), *inclusive))
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::globals::Globals;

    #[test]
    fn io_primitive_signatures_use_the_std_error() {
        let error = Ty::Int(IntTy::I64);
        let globals = Globals { io_error_ty: Some(error.clone()), ..Globals::default() };
        assert_eq!(
            builtin_sig(Builtin::ReadToString, &globals).ret,
            Ty::Result(Box::new(Ty::String), Box::new(error.clone()))
        );
        assert_eq!(
            builtin_sig(Builtin::WriteFile, &globals).ret,
            Ty::Result(Box::new(Ty::Unit), Box::new(error.clone()))
        );
        assert_eq!(
            builtin_sig(Builtin::RunCommand, &globals).ret,
            Ty::Result(Box::new(Ty::Int(IntTy::I32)), Box::new(error))
        );
        // Without a standard library the primitive falls back to the unit placeholder rather than failing.
        assert_eq!(
            builtin_sig(Builtin::ReadToString, &Globals::default()).ret,
            Ty::Result(Box::new(Ty::String), Box::new(Ty::Unit))
        );
    }

    #[test]
    fn environment_primitives_have_their_declared_signatures() {
        let bytes = Ty::List(Box::new(Ty::Int(IntTy::U8)), burdin_tir::Alloc::Global);
        let slice = Ty::Slice(Box::new(Ty::Int(IntTy::U8)));
        assert_eq!(builtin("__env_get"), Some(Builtin::EnvGet));
        assert_eq!(builtin("__env_set"), Some(Builtin::EnvSet));
        assert_eq!(builtin("__env_set_bytes"), Some(Builtin::EnvSetBytes));
        assert_eq!(builtin("__env_remove"), Some(Builtin::EnvRemove));
        assert_eq!(builtin("__current_dir"), Some(Builtin::CurrentDir));
        assert_eq!(builtin("__set_current_dir"), Some(Builtin::SetCurrentDir));
        assert_eq!(builtin("__set_current_dir_bytes"), Some(Builtin::SetCurrentDirBytes));

        let get = builtin_sig(Builtin::EnvGet, &Globals::default());
        assert_eq!(get.ret, Ty::Option(Box::new(bytes.clone())));
        assert_eq!(get.params.len(), 1);
        assert_eq!(get.params[0].ty, Ty::Str);
        let cwd = builtin_sig(Builtin::CurrentDir, &Globals::default());
        assert_eq!(cwd.ret, Ty::Option(Box::new(bytes)));
        assert!(cwd.params.is_empty());
        let set = builtin_sig(Builtin::EnvSetBytes, &Globals::default());
        assert_eq!(set.params.len(), 2);
        assert_eq!(set.params[0].ty, Ty::Str);
        assert_eq!(set.params[1].ty, slice.clone());
        assert_eq!(set.ret, Ty::Int(IntTy::I32));
        let cwd_bytes = builtin_sig(Builtin::SetCurrentDirBytes, &Globals::default());
        assert_eq!(cwd_bytes.params.len(), 1);
        assert_eq!(cwd_bytes.params[0].ty, slice);
        let remove = builtin_sig(Builtin::EnvRemove, &Globals::default());
        assert_eq!(remove.params.len(), 1);
        assert_eq!(remove.params[0].ty, Ty::Str);
    }

    #[test]
    fn stream_builtin_signatures() {
        let error = Ty::Int(IntTy::I64);
        let globals = Globals { io_error_ty: Some(error.clone()), ..Globals::default() };
        assert_eq!(builtin("__stdin_read_line"), Some(Builtin::StdinReadLine));
        assert_eq!(builtin("__stdin_read_to_string"), Some(Builtin::StdinReadToString));
        assert_eq!(builtin("__stderr_write_line"), Some(Builtin::StderrWriteLine));
        assert_eq!(builtin("__stdout_flush"), Some(Builtin::StdoutFlush));
        assert_eq!(builtin("__is_terminal"), Some(Builtin::IsTerminal));

        let line = builtin_sig(Builtin::StdinReadLine, &globals);
        assert!(line.params.is_empty());
        assert_eq!(line.ret, Ty::Result(Box::new(Ty::Option(Box::new(Ty::String))), Box::new(error.clone())));
        let all = builtin_sig(Builtin::StdinReadToString, &globals);
        assert!(all.params.is_empty());
        assert_eq!(all.ret, Ty::Result(Box::new(Ty::String), Box::new(error)));

        let write = builtin_sig(Builtin::StdoutWrite, &globals);
        assert_eq!(write.params.len(), 1);
        assert_eq!(write.params[0].ty, Ty::Str);
        assert_eq!(write.ret, Ty::Unit);
        assert!(builtin_sig(Builtin::StdoutFlush, &globals).params.is_empty());

        assert_eq!(builtin("__stdout_write_bytes"), Some(Builtin::StdoutWriteBytes));
        assert_eq!(builtin("__stderr_write_bytes"), Some(Builtin::StderrWriteBytes));
        let bytes = builtin_sig(Builtin::StderrWriteBytes, &globals);
        assert_eq!(bytes.params.len(), 1);
        assert_eq!(bytes.params[0].ty, Ty::Slice(Box::new(Ty::Int(IntTy::U8))));
        assert_eq!(bytes.ret, Ty::Unit);

        assert_eq!(builtin("__stdin_avail"), Some(Builtin::StdinAvail));
        assert_eq!(builtin("__stdin_next_byte"), Some(Builtin::StdinNextByte));
        let avail = builtin_sig(Builtin::StdinAvail, &globals);
        assert!(avail.params.is_empty());
        assert_eq!(avail.ret, Ty::Int(IntTy::Isize));
        let next = builtin_sig(Builtin::StdinNextByte, &globals);
        assert!(next.params.is_empty());
        assert_eq!(next.ret, Ty::Int(IntTy::U8));

        let tty = builtin_sig(Builtin::IsTerminal, &globals);
        assert_eq!(tty.params[0].ty, Ty::Int(IntTy::I32));
        assert_eq!(tty.ret, Ty::Bool);
    }

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

        let list = Ty::List(Box::new(i32_.clone()), burdin_tir::Alloc::Global);
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
