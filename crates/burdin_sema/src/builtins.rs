//! The built-in functions, types, and methods.

use crate::globals::Globals;
use burdin_tir::{Builtin, FloatOp, FloatTy, FnKind, FnSig, IntTy, Method, ParamSig, Signature, Ty};
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
        "__env_list" => Builtin::EnvList,
        "__process_spawn" => Builtin::ProcessSpawn,
        "__process_wait" => Builtin::ProcessWait,
        "__process_close" => Builtin::ProcessClose,
        "__pipe_read_to_string" => Builtin::PipeReadToString,
        "__pipe_read_line" => Builtin::PipeReadLine,
        "__pipe_read_both" => Builtin::PipeReadBoth,
        "__pipe_write" => Builtin::PipeWrite,
        "__pipe_close" => Builtin::PipeClose,
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
        Builtin::EnvArgs | Builtin::EnvList => Ty::List(Box::new(Ty::String), burdin_tir::Alloc::Global),
        Builtin::EnvGet | Builtin::CurrentDir => {
            Ty::Option(Box::new(Ty::List(Box::new(Ty::Int(IntTy::U8)), burdin_tir::Alloc::Global)))
        }
        Builtin::EnvSet
        | Builtin::EnvSetBytes
        | Builtin::EnvRemove
        | Builtin::SetCurrentDir
        | Builtin::SetCurrentDirBytes
        | Builtin::PipeWrite => Ty::Int(IntTy::I32),
        Builtin::ProcessWait => Ty::Int(IntTy::I64),
        // `(pid: i32, process: usize, stdin: usize, stdout: usize, stderr: usize)`: the child's identity and the
        // pipe ends `std` wraps. `process` is the Windows process handle and 0 on Unix.
        Builtin::ProcessSpawn => Ty::Result(
            Box::new(Ty::Tuple(vec![
                Ty::Int(IntTy::I32),
                Ty::Int(IntTy::Usize),
                Ty::Int(IntTy::Usize),
                Ty::Int(IntTy::Usize),
                Ty::Int(IntTy::Usize),
            ])),
            Box::new(io_error()),
        ),
        Builtin::PipeReadToString => Ty::Result(Box::new(Ty::String), Box::new(io_error())),
        Builtin::PipeReadLine => Ty::Result(Box::new(Ty::Option(Box::new(Ty::String))), Box::new(io_error())),
        Builtin::PipeReadBoth => Ty::Result(Box::new(Ty::Tuple(vec![Ty::String, Ty::String])), Box::new(io_error())),
        Builtin::RunCommand => Ty::Result(Box::new(Ty::Int(IntTy::I32)), Box::new(io_error())),
        Builtin::RawAlloc | Builtin::RawRealloc => Ty::CPtr(Box::new(Ty::Int(IntTy::U8))),
        Builtin::FloatFromBits(ft) => Ty::Float(ft),
        _ => Ty::Unit,
    };
    let params = match b {
        Builtin::StringNew
        | Builtin::EnvArgs
        | Builtin::EnvList
        | Builtin::Unreachable
        | Builtin::TargetOs
        | Builtin::TargetArch
        | Builtin::CurrentDir
        | Builtin::StdoutFlush
        | Builtin::StdinReadLine
        | Builtin::StdinReadToString
        | Builtin::StdinAvail
        | Builtin::StdinNextByte => Vec::new(),
        Builtin::ProcessClose | Builtin::PipeClose => vec![ParamSig::plain(Ty::Int(IntTy::Usize))],
        Builtin::ProcessWait => {
            vec![ParamSig::plain(Ty::Int(IntTy::I32)), ParamSig::plain(Ty::Int(IntTy::Usize))]
        }
        Builtin::PipeReadToString | Builtin::PipeReadLine => vec![ParamSig::plain(Ty::Int(IntTy::Usize))],
        Builtin::PipeReadBoth => {
            vec![ParamSig::plain(Ty::Int(IntTy::Usize)), ParamSig::plain(Ty::Int(IntTy::Usize))]
        }
        Builtin::PipeWrite => vec![ParamSig::plain(Ty::Int(IntTy::Usize)), ParamSig::plain(Ty::Str)],
        // `(program, args, environment, working directory, Windows command line, stdin, stdout, stderr)`: the
        // stream modes are 0 (inherit), 1 (piped), and 2 (null). `std` builds the environment and the command line.
        Builtin::ProcessSpawn => vec![
            ParamSig::plain(Ty::Str),
            ParamSig::plain(Ty::List(Box::new(Ty::String), burdin_tir::Alloc::Global)),
            ParamSig::plain(Ty::List(Box::new(Ty::String), burdin_tir::Alloc::Global)),
            ParamSig::plain(Ty::Str),
            ParamSig::plain(Ty::Str),
            ParamSig::plain(Ty::Int(IntTy::I32)),
            ParamSig::plain(Ty::Int(IntTy::I32)),
            ParamSig::plain(Ty::Int(IntTy::I32)),
        ],
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
        Builtin::FloatFromBits(FloatTy::F32) => vec![ParamSig::plain(Ty::Int(IntTy::U32))],
        Builtin::FloatFromBits(FloatTy::F64) => vec![ParamSig::plain(Ty::Int(IntTy::U64))],
        // `assert_eq` checks its two arguments specially: their type is inferred, not fixed.
        Builtin::AssertEq => Vec::new(),
        // `Heap.new` is checked separately: its result type depends on the value.
        Builtin::HeapNew => vec![ParamSig::plain(Ty::Error)],
        // `swap` and `replace` check their arguments separately.
        Builtin::Swap | Builtin::Replace => Vec::new(),
        _ => vec![ParamSig::plain(Ty::Str)],
    };
    Signature { params, ret, has_self: false, ret_origins: Vec::new() }
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
    // The floating-point functions the compiler provides (`spec/stdlib.md`, "Float functions"). The rest of the
    // float surface is an extension method in `std`.
    if matches!(recv, Ty::Float(_)) {
        return float_method(name, recv);
    }
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

/// The float functions the compiler provides: the ones that lower to an LLVM intrinsic or a C math library call.
/// `signum`, `log`, `fract`, `min`, `max`, `clamp`, `to_degrees`, `to_radians`, the predicates, and `total_cmp`
/// are extension methods in `std` (`spec/stdlib.md`, "Float functions").
fn float_method(name: &str, recv: &Ty) -> Option<(Method, Vec<ParamSig>, Ty)> {
    let ty = recv.clone();
    let plain = |t: Ty| ParamSig::plain(t);
    let unary = |op| (Method::Float(op), Vec::new(), ty.clone());
    let binary = |op| (Method::Float(op), vec![plain(ty.clone())], ty.clone());
    Some(match name {
        "abs" => unary(FloatOp::Abs),
        "sqrt" => unary(FloatOp::Sqrt),
        "cbrt" => unary(FloatOp::Cbrt),
        "powi" => (Method::Float(FloatOp::Powi), vec![plain(Ty::Int(IntTy::I32))], ty.clone()),
        "powf" => binary(FloatOp::Powf),
        "exp" => unary(FloatOp::Exp),
        "exp2" => unary(FloatOp::Exp2),
        "ln" => unary(FloatOp::Ln),
        "log2" => unary(FloatOp::Log2),
        "log10" => unary(FloatOp::Log10),
        "floor" => unary(FloatOp::Floor),
        "ceil" => unary(FloatOp::Ceil),
        "round" => unary(FloatOp::Round),
        "round_ties_even" => unary(FloatOp::RoundTiesEven),
        "trunc" => unary(FloatOp::Trunc),
        "sin" => unary(FloatOp::Sin),
        "cos" => unary(FloatOp::Cos),
        "tan" => unary(FloatOp::Tan),
        "asin" => unary(FloatOp::Asin),
        "acos" => unary(FloatOp::Acos),
        "atan" => unary(FloatOp::Atan),
        "atan2" => binary(FloatOp::Atan2),
        "sinh" => unary(FloatOp::Sinh),
        "cosh" => unary(FloatOp::Cosh),
        "tanh" => unary(FloatOp::Tanh),
        "hypot" => binary(FloatOp::Hypot),
        "mul_add" => {
            let args = vec![plain(ty.clone()), plain(ty.clone())];
            (Method::Float(FloatOp::MulAdd), args, ty.clone())
        }
        "copysign" => binary(FloatOp::Copysign),
        "to_bits" => {
            let bits = match recv {
                Ty::Float(FloatTy::F32) => Ty::Int(IntTy::U32),
                _ => Ty::Int(IntTy::U64),
            };
            (Method::Float(FloatOp::ToBits), Vec::new(), bits)
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
    fn process_primitives_have_their_declared_signatures() {
        let usize_ = Ty::Int(IntTy::Usize);
        let strings = Ty::List(Box::new(Ty::String), burdin_tir::Alloc::Global);
        for (name, b) in [
            ("__env_list", Builtin::EnvList),
            ("__process_spawn", Builtin::ProcessSpawn),
            ("__process_wait", Builtin::ProcessWait),
            ("__process_close", Builtin::ProcessClose),
            ("__pipe_read_to_string", Builtin::PipeReadToString),
            ("__pipe_read_line", Builtin::PipeReadLine),
            ("__pipe_read_both", Builtin::PipeReadBoth),
            ("__pipe_write", Builtin::PipeWrite),
            ("__pipe_close", Builtin::PipeClose),
        ] {
            assert_eq!(builtin(name), Some(b));
        }

        let list = builtin_sig(Builtin::EnvList, &Globals::default());
        assert!(list.params.is_empty());
        assert_eq!(list.ret, strings.clone());

        let spawn = builtin_sig(Builtin::ProcessSpawn, &Globals::default());
        assert_eq!(spawn.params.len(), 8);
        assert_eq!(spawn.params[0].ty, Ty::Str);
        assert_eq!(spawn.params[1].ty, strings.clone());
        assert_eq!(spawn.params[2].ty, strings.clone());
        assert_eq!(spawn.params[3].ty, Ty::Str);
        assert_eq!(spawn.params[4].ty, Ty::Str);
        for param in spawn.params.iter().skip(5) {
            assert_eq!(param.ty, Ty::Int(IntTy::I32));
        }
        assert!(matches!(&spawn.ret, Ty::Result(..)));

        let wait = builtin_sig(Builtin::ProcessWait, &Globals::default());
        assert_eq!(wait.params.len(), 2);
        assert_eq!(wait.params[0].ty, Ty::Int(IntTy::I32));
        assert_eq!(wait.params[1].ty, usize_.clone());
        assert_eq!(wait.ret, Ty::Int(IntTy::I64));

        let close = builtin_sig(Builtin::ProcessClose, &Globals::default());
        assert_eq!(close.params.len(), 1);
        assert_eq!(close.params[0].ty, usize_.clone());
        assert_eq!(close.ret, Ty::Unit);

        let line = builtin_sig(Builtin::PipeReadLine, &Globals::default());
        assert_eq!(line.params.len(), 1);
        assert_eq!(line.params[0].ty, usize_.clone());
        assert!(matches!(&line.ret, Ty::Result(..)));

        let both = builtin_sig(Builtin::PipeReadBoth, &Globals::default());
        assert_eq!(both.params.len(), 2);
        assert_eq!(both.params[1].ty, usize_.clone());

        let write = builtin_sig(Builtin::PipeWrite, &Globals::default());
        assert_eq!(write.params.len(), 2);
        assert_eq!(write.params[0].ty, usize_.clone());
        assert_eq!(write.params[1].ty, Ty::Str);
        assert_eq!(write.ret, Ty::Int(IntTy::I32));
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
    fn float_methods_map_to_compiler_operations() {
        let f64_ = Ty::Float(FloatTy::F64);
        let f32_ = Ty::Float(FloatTy::F32);
        let op = |name: &str, recv: &Ty| match method(name, recv) {
            Some((Method::Float(op), _, _)) => op,
            _ => panic!("`{name}` is not a compiler method on `{recv}`"),
        };
        assert_eq!(op("abs", &f64_), FloatOp::Abs);
        assert_eq!(op("sqrt", &f64_), FloatOp::Sqrt);
        assert_eq!(op("cbrt", &f64_), FloatOp::Cbrt);
        assert_eq!(op("powf", &f64_), FloatOp::Powf);
        assert_eq!(op("ln", &f64_), FloatOp::Ln);
        assert_eq!(op("round_ties_even", &f64_), FloatOp::RoundTiesEven);
        assert_eq!(op("copysign", &f64_), FloatOp::Copysign);
        assert_eq!(op("to_bits", &f64_), FloatOp::ToBits);
        // The same names exist on `f32`, with `f32` parameters and results.
        assert_eq!(op("sqrt", &f32_), FloatOp::Sqrt);
        let (_, params, ret) = method("powi", &f32_).unwrap();
        assert_eq!(params[0].ty, Ty::Int(IntTy::I32));
        assert_eq!(ret, f32_);
        let (_, params, ret) = method("mul_add", &f64_).unwrap();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].ty, f64_);
        assert_eq!(ret, f64_);
        let (_, params, ret) = method("to_bits", &f32_).unwrap();
        assert!(params.is_empty());
        assert_eq!(ret, Ty::Int(IntTy::U32));
        assert_eq!(method("to_bits", &f64_).unwrap().2, Ty::Int(IntTy::U64));
        // The functions `std` writes in Burdin are not compiler methods, and no float method leaks to integers.
        for name in ["signum", "log", "fract", "min", "max", "clamp", "to_degrees", "to_radians", "is_nan"] {
            assert!(method(name, &f64_).is_none(), "`{name}` is a std extension");
        }
        assert!(method("sqrt", &Ty::Int(IntTy::I32)).is_none());
        assert!(method("to_bits", &Ty::Int(IntTy::U64)).is_none());
    }

    #[test]
    fn float_from_bits_has_the_precision_width() {
        assert_eq!(
            builtin_sig(Builtin::FloatFromBits(FloatTy::F64), &Globals::default()).params[0].ty,
            Ty::Int(IntTy::U64)
        );
        assert_eq!(builtin_sig(Builtin::FloatFromBits(FloatTy::F64), &Globals::default()).ret, Ty::Float(FloatTy::F64));
        assert_eq!(
            builtin_sig(Builtin::FloatFromBits(FloatTy::F32), &Globals::default()).params[0].ty,
            Ty::Int(IntTy::U32)
        );
        assert_eq!(builtin_sig(Builtin::FloatFromBits(FloatTy::F32), &Globals::default()).ret, Ty::Float(FloatTy::F32));
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
        assert!(
            method("min", &Ty::Float(FloatTy::F64)).is_none(),
            "float `min` is a std extension method, not a compiler method"
        );
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
