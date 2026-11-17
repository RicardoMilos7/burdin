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
        "__file_open" => Builtin::FileOpen,
        "__file_read" => Builtin::FileRead,
        "__file_write" => Builtin::FileWrite,
        "__file_seek" => Builtin::FileSeek,
        "__file_sync" => Builtin::FileSync,
        "__file_set_len" => Builtin::FileSetLen,
        "__file_close" => Builtin::FileClose,
        "__file_error" => Builtin::FileError,
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
        "__stdin_error" => Builtin::StdinError,
        "__read_dir" => Builtin::ReadDir,
        "__create_dir" => Builtin::CreateDir,
        "__remove_dir" => Builtin::RemoveDir,
        "__remove_file" => Builtin::RemoveFile,
        "__rename" => Builtin::RenamePath,
        "__metadata" => Builtin::Metadata,
        "__canonicalize" => Builtin::Canonicalize,
        "__io_kind" => Builtin::IoErrorKind,
        "__clock" => Builtin::Clock,
        "__entropy" => Builtin::Entropy,
        "__sleep" => Builtin::Sleep,
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
        "__view_as_text" => Builtin::ViewAsText,
        _ => return None,
    })
}

pub(crate) fn builtin_sig(b: Builtin, globals: &Globals) -> Signature {
    // The I/O primitives report the operating system's own error code; the `std` wrappers turn it into the
    // `IoError` of `spec/errors.md` ERR10 with `__io_kind` for the portable kind (ERR11).
    let os_code = || Ty::Int(IntTy::I32);
    let ret = match b {
        Builtin::Panic | Builtin::Unreachable | Builtin::ProcessExit => Ty::Never,
        Builtin::TargetOs | Builtin::TargetArch => Ty::Str,
        Builtin::StringNew | Builtin::StringFrom | Builtin::CStrToString | Builtin::FloatStr => Ty::String,
        Builtin::ViewAsText => Ty::Str,
        Builtin::CStringFrom => Ty::CString,
        Builtin::ReadToString => Ty::Result(Box::new(Ty::String), Box::new(os_code())),
        Builtin::WriteFile => Ty::Result(Box::new(Ty::Unit), Box::new(os_code())),
        Builtin::FileOpen => Ty::Result(Box::new(Ty::Int(IntTy::I32)), Box::new(os_code())),
        Builtin::FileRead | Builtin::FileWrite => Ty::Result(Box::new(Ty::Int(IntTy::Usize)), Box::new(os_code())),
        Builtin::FileSeek => Ty::Result(Box::new(Ty::Int(IntTy::I64)), Box::new(os_code())),
        Builtin::FileSync | Builtin::FileSetLen | Builtin::FileClose => {
            Ty::Result(Box::new(Ty::Unit), Box::new(os_code()))
        }
        Builtin::StdinReadLine => Ty::Result(Box::new(Ty::Option(Box::new(Ty::String))), Box::new(os_code())),
        Builtin::StdinReadToString => Ty::Result(Box::new(Ty::String), Box::new(os_code())),
        Builtin::IsTerminal => Ty::Bool,
        Builtin::StdinAvail => Ty::Int(IntTy::Isize),
        Builtin::StdinNextByte => Ty::Int(IntTy::U8),
        Builtin::StdinError => Ty::Int(IntTy::I32),
        Builtin::FileError => Ty::Int(IntTy::I32),
        Builtin::ReadDir => {
            Ty::Result(Box::new(Ty::List(Box::new(Ty::String), burdin_tir::Alloc::Global)), Box::new(os_code()))
        }
        Builtin::CreateDir | Builtin::RemoveDir | Builtin::RemoveFile | Builtin::RenamePath => {
            Ty::Result(Box::new(Ty::Unit), Box::new(os_code()))
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
            Box::new(os_code()),
        ),
        Builtin::Canonicalize => Ty::Result(Box::new(Ty::String), Box::new(os_code())),
        Builtin::Clock => Ty::Int(IntTy::I64),
        Builtin::Entropy => Ty::Int(IntTy::U64),
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
            Box::new(os_code()),
        ),
        // The process and pipe primitives report 0 when their runtime path has no OS code to give; the `std`
        // wrappers turn 0 into the `Other` kind and `os_code() == None` (`spec/errors.md`, ERR10).
        Builtin::PipeReadToString => Ty::Result(Box::new(Ty::String), Box::new(os_code())),
        Builtin::PipeReadLine => Ty::Result(Box::new(Ty::Option(Box::new(Ty::String))), Box::new(os_code())),
        Builtin::PipeReadBoth => Ty::Result(Box::new(Ty::Tuple(vec![Ty::String, Ty::String])), Box::new(os_code())),
        Builtin::RunCommand => Ty::Result(Box::new(Ty::Int(IntTy::I32)), Box::new(os_code())),
        // `IoErrorKind` is the enum the standard library declares; a standard library without that enum (or
        // with a struct of the same name) leaves the builtin untyped, and a caller then fails to type-check
        // instead of the code generator meeting an unexpected type.
        Builtin::IoErrorKind => match &globals.io_error_kind_ty {
            Some(Ty::Enum(_)) => globals.io_error_kind_ty.clone().unwrap(),
            _ => Ty::Unit,
        },
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
        | Builtin::Entropy
        | Builtin::CurrentDir
        | Builtin::StdoutFlush
        | Builtin::StdinReadLine
        | Builtin::StdinReadToString
        | Builtin::StdinAvail
        | Builtin::StdinNextByte
        | Builtin::StdinError
        | Builtin::FileError => Vec::new(),
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
        Builtin::IsTerminal | Builtin::Clock => vec![ParamSig::plain(Ty::Int(IntTy::I32))],
        Builtin::Sleep => vec![ParamSig::plain(Ty::Int(IntTy::U64))],
        Builtin::StdoutWrite | Builtin::StdoutWriteLine | Builtin::StderrWrite | Builtin::StderrWriteLine => {
            vec![ParamSig::plain(Ty::Str)]
        }
        Builtin::StdoutWriteBytes | Builtin::StderrWriteBytes | Builtin::ViewAsText => {
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
        Builtin::FileOpen => {
            let mut params = vec![ParamSig::plain(Ty::Str)];
            for _ in 0..6 {
                params.push(ParamSig::plain(Ty::Bool));
            }
            params
        }
        // The buffer is the caller's list: the primitive appends to it after growing it.
        Builtin::FileRead => {
            let mut buf = ParamSig::plain(Ty::List(Box::new(Ty::Int(IntTy::U8)), burdin_tir::Alloc::Global));
            buf.by_ref = true;
            buf.mut_param = true;
            buf.exclusive = true;
            vec![ParamSig::plain(Ty::Int(IntTy::I32)), buf, ParamSig::plain(Ty::Int(IntTy::Usize))]
        }
        Builtin::FileWrite => {
            vec![ParamSig::plain(Ty::Int(IntTy::I32)), ParamSig::plain(Ty::Slice(Box::new(Ty::Int(IntTy::U8))))]
        }
        Builtin::FileSeek => vec![
            ParamSig::plain(Ty::Int(IntTy::I32)),
            ParamSig::plain(Ty::Int(IntTy::I64)),
            ParamSig::plain(Ty::Int(IntTy::I32)),
        ],
        Builtin::FileSync | Builtin::FileClose => vec![ParamSig::plain(Ty::Int(IntTy::I32))],
        Builtin::FileSetLen => {
            vec![ParamSig::plain(Ty::Int(IntTy::I32)), ParamSig::plain(Ty::Int(IntTy::U64))]
        }
        Builtin::RenamePath => vec![ParamSig::plain(Ty::Str), ParamSig::plain(Ty::Str)],
        Builtin::ReadToString | Builtin::RunCommand => vec![ParamSig::plain(Ty::Str)],
        Builtin::IoErrorKind => vec![ParamSig::plain(Ty::Int(IntTy::I32))],
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

/// The type of a read-only map lookup's key argument. A `Map<String, V>` takes a `str`, so the key is hashed and
/// compared by bytes without allocating a `String` (`spec/stdlib.md`, "Borrowed-key lookup"); every other key
/// type is taken as it is.
fn lookup_key(k: &Ty) -> Ty {
    if *k == Ty::String { Ty::Str } else { k.clone() }
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
        ("first", Ty::List(t, _)) => {
            (Method::ListFirst, vec![], Ty::Option(Box::new(Ty::Ref { mutable: false, inner: t.clone() })))
        }
        ("first_mut", Ty::List(t, _)) => {
            (Method::ListFirstMut, vec![], Ty::Option(Box::new(Ty::Ref { mutable: true, inner: t.clone() })))
        }
        ("last_mut", Ty::List(t, _)) => {
            (Method::ListLastMut, vec![], Ty::Option(Box::new(Ty::Ref { mutable: true, inner: t.clone() })))
        }
        ("get", Ty::List(t, _)) => {
            let view = Ty::Ref { mutable: false, inner: t.clone() };
            (Method::ListGet, vec![plain(Ty::Int(IntTy::Usize))], Ty::Option(Box::new(view)))
        }
        ("get_mut", Ty::List(t, _)) => {
            let view = Ty::Ref { mutable: true, inner: t.clone() };
            (Method::ListGetMut, vec![plain(Ty::Int(IntTy::Usize))], Ty::Option(Box::new(view)))
        }
        ("capacity", Ty::List(_, _)) => (Method::ListCapacity, vec![], Ty::Int(IntTy::Usize)),
        ("is_empty", Ty::List(_, _)) => (Method::ListIsEmpty, vec![], Ty::Bool),
        ("reserve", Ty::List(_, _)) => (Method::ListReserve, vec![plain(Ty::Int(IntTy::Usize))], Ty::Unit),
        ("reserve_exact", Ty::List(_, _)) => (Method::ListReserveExact, vec![plain(Ty::Int(IntTy::Usize))], Ty::Unit),
        ("shrink_to_fit", Ty::List(_, _)) => (Method::ListShrinkToFit, vec![], Ty::Unit),
        ("swap", Ty::List(_, _)) => {
            (Method::ListSwap, vec![plain(Ty::Int(IntTy::Usize)), plain(Ty::Int(IntTy::Usize))], Ty::Unit)
        }
        ("swap_remove", Ty::List(t, _)) => (Method::ListSwapRemove, vec![plain(Ty::Int(IntTy::Usize))], elem(t)),
        ("truncate", Ty::List(_, _)) => (Method::ListTruncate, vec![plain(Ty::Int(IntTy::Usize))], Ty::Unit),
        // The tail moves into a new list with the default allocator: a stored allocator value cannot be duplicated.
        ("split_off", Ty::List(t, _)) => {
            (Method::ListSplitOff, vec![plain(Ty::Int(IntTy::Usize))], Ty::List(t.clone(), burdin_tir::Alloc::Global))
        }
        ("append", Ty::List(t, alloc)) => {
            (Method::ListAppend, vec![value_param(Ty::List(t.clone(), alloc.clone()))], Ty::Unit)
        }
        // `drain` is checked separately: it takes either range spelling (`call.rs`).
        ("drain", Ty::List(t, _)) => (
            Method::ListDrain,
            vec![plain(Ty::Range(Box::new(Ty::Int(IntTy::Usize)), false))],
            Ty::ListIntoIter(t.clone(), burdin_tir::Alloc::Global),
        ),
        ("extend_from_slice", Ty::List(t, _)) => {
            (Method::ListExtendFromSlice, vec![plain(Ty::Slice(t.clone()))], Ty::Unit)
        }
        ("insert", Ty::List(t, _)) => {
            (Method::ListInsert, vec![plain(Ty::Int(IntTy::Usize)), value_param(elem(t))], Ty::Unit)
        }
        ("remove", Ty::List(t, _)) => (Method::ListRemove, vec![plain(Ty::Int(IntTy::Usize))], elem(t)),
        ("contains", Ty::List(t, _)) => (Method::ListContains, vec![plain(elem(t))], Ty::Bool),
        ("sort", Ty::List(_, _)) => (Method::ListSort, vec![], Ty::Unit),
        ("clear", Ty::List(_, _) | Ty::Map(..)) => (Method::Clear, vec![], Ty::Unit),
        ("insert", Ty::Map(k, v, _)) => (Method::MapInsert, vec![value_param(elem(k)), value_param(elem(v))], Ty::Unit),
        // A `Map<String, V>` takes a `str` for its read-only lookups (`spec/stdlib.md`, "Borrowed-key lookup"): the
        // key is hashed and compared by bytes and no `String` is allocated. A `String` argument converts to `str`
        // through the one implicit conversion (IC1).
        ("get", Ty::Map(k, v, _)) => {
            let view = Ty::Ref { mutable: false, inner: v.clone() };
            (Method::MapGet, vec![plain(lookup_key(k))], Ty::Option(Box::new(view)))
        }
        ("get_or_insert", Ty::Map(k, v, _)) => {
            let view = Ty::Ref { mutable: true, inner: v.clone() };
            (Method::MapGetOrInsert, vec![value_param(elem(k)), value_param(elem(v))], view)
        }
        ("get_or", Ty::Map(k, v, _)) => {
            let view = Ty::Ref { mutable: false, inner: v.clone() };
            (Method::MapGetOr, vec![value_param(elem(k)), value_param(elem(v))], view)
        }
        ("get_mut", Ty::Map(k, v, _)) => {
            let view = Ty::Ref { mutable: true, inner: v.clone() };
            (Method::MapGetMut, vec![plain(lookup_key(k))], Ty::Option(Box::new(view)))
        }
        ("remove_entry", Ty::Map(k, v, _)) => {
            let pair = Ty::Tuple(vec![(**k).clone(), (**v).clone()]);
            (Method::MapRemoveEntry, vec![plain(lookup_key(k))], Ty::Option(Box::new(pair)))
        }
        ("retain", Ty::Map(k, v, _)) => {
            let keep = Ty::Fn {
                sig: Rc::new(FnSig {
                    params: vec![
                        Ty::Ref { mutable: false, inner: k.clone() },
                        Ty::Ref { mutable: false, inner: v.clone() },
                    ],
                    ret: Box::new(Ty::Bool),
                }),
                kind: FnKind::FnMut,
                borrowed: false,
            };
            (Method::MapRetain, vec![value_param(keep)], Ty::Unit)
        }
        ("reserve", Ty::Map(..)) => (Method::MapReserve, vec![plain(Ty::Int(IntTy::Usize))], Ty::Unit),
        ("shrink_to_fit", Ty::Map(..)) => (Method::MapShrinkToFit, vec![], Ty::Unit),
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
        ("remove", Ty::Map(k, v, _)) => (Method::MapRemove, vec![plain(lookup_key(k))], Ty::Option(v.clone())),
        ("contains_key", Ty::Map(k, _, _)) => (Method::MapContainsKey, vec![plain(lookup_key(k))], Ty::Bool),
        ("unwrap", Ty::Option(t)) => (Method::Unwrap, vec![], elem(t)),
        ("unwrap", Ty::Result(t, _)) => (Method::Unwrap, vec![], elem(t)),
        ("expect", Ty::Option(t)) => (Method::Expect, vec![plain(Ty::Str)], elem(t)),
        ("expect", Ty::Result(t, _)) => (Method::Expect, vec![plain(Ty::Str)], elem(t)),
        ("is_some", Ty::Option(_)) => (Method::IsSome, vec![], Ty::Bool),
        ("is_none", Ty::Option(_)) => (Method::IsNone, vec![], Ty::Bool),
        ("is_ok", Ty::Result(..)) => (Method::IsOk, vec![], Ty::Bool),
        ("is_err", Ty::Result(..)) => (Method::IsErr, vec![], Ty::Bool),
        ("as_str", Ty::String | Ty::Str) => (Method::AsStr, vec![], Ty::Str),
        // `List<T>.as_slice(): [T]`: a view of the list's elements (`spec/stdlib.md`, "Slices").
        ("as_slice", Ty::List(t, _)) => (Method::AsSlice, vec![], Ty::Slice(t.clone())),
        // The slice methods the compiler implements (the algorithms that write through a slice are in `std`).
        // `get(range)` is resolved by the call checker from the argument's type.
        ("get", Ty::Slice(t)) => {
            let view = Ty::Ref { mutable: false, inner: t.clone() };
            (Method::SliceGet, vec![plain(Ty::Int(IntTy::Usize))], Ty::Option(Box::new(view)))
        }
        // The slice's own `last` and `to_list`: they win over the `Iterator` defaults of the same name.
        ("last", Ty::Slice(t)) => {
            let view = Ty::Ref { mutable: false, inner: t.clone() };
            (Method::SliceLast, vec![], Ty::Option(Box::new(view)))
        }
        ("to_list", Ty::Slice(t)) => (Method::SliceToList, vec![], Ty::List(t.clone(), burdin_tir::Alloc::Global)),
        ("swap", Ty::Slice(_)) => {
            (Method::SliceSwap, vec![plain(Ty::Int(IntTy::Usize)), plain(Ty::Int(IntTy::Usize))], Ty::Unit)
        }
        ("fill", Ty::Slice(t)) => (Method::SliceFill, vec![plain(elem(t))], Ty::Unit),
        ("copy_from_slice", Ty::Slice(t)) => (Method::SliceCopyFromSlice, vec![plain(Ty::Slice(t.clone()))], Ty::Unit),
        ("clone_from_slice", Ty::Slice(t)) => {
            (Method::SliceCloneFromSlice, vec![plain(Ty::Slice(t.clone()))], Ty::Unit)
        }
        ("chunks", Ty::Slice(t)) => {
            (Method::SliceChunks, vec![plain(Ty::Int(IntTy::Usize))], Ty::SliceChunks(t.clone(), false))
        }
        ("chunks_exact", Ty::Slice(t)) => {
            (Method::SliceChunks, vec![plain(Ty::Int(IntTy::Usize))], Ty::SliceChunks(t.clone(), true))
        }
        ("windows", Ty::Slice(t)) => {
            (Method::SliceWindows, vec![plain(Ty::Int(IntTy::Usize))], Ty::SliceWindows(t.clone()))
        }
        // `[T].__sort_by(less)`: the insertion sort behind `std`'s `sort_by`, which takes an `Ordering`
        // comparator and adapts it to this `bool` one. The closure is the borrowing wrapper `std` builds, so
        // its type says a borrow capture (`spec/memory-rules.md`, CL3).
        ("__sort_by", Ty::Slice(t)) => {
            let less = Ty::Fn {
                sig: Rc::new(FnSig {
                    params: vec![
                        Ty::Ref { mutable: false, inner: t.clone() },
                        Ty::Ref { mutable: false, inner: t.clone() },
                    ],
                    ret: Box::new(Ty::Bool),
                }),
                kind: FnKind::FnMut,
                borrowed: true,
            };
            (Method::SliceSortBy, vec![value_param(less)], Ty::Unit)
        }
        ("as_bytes", Ty::String | Ty::Str) => (Method::AsBytes, vec![], Ty::Slice(Box::new(Ty::Int(IntTy::U8)))),
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
        // The mutable iterators yield `&mut T` and borrow the collection exclusively (IT15).
        ("iter_mut", Ty::List(elem, _) | Ty::Array(elem, _)) => (Method::IterMut, vec![], Ty::IterMut(elem.clone())),
        ("iter_mut", Ty::Map(k, v, _)) => (Method::IterMut, vec![], Ty::MapIterMut(k.clone(), v.clone())),
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
    fn io_primitive_signatures_use_the_operating_system_code() {
        // The primitives report the OS code itself; `std` maps it with `__io_kind` (spec/errors.md, ERR10-ERR11).
        let code = Ty::Int(IntTy::I32);
        let globals = Globals::default();
        assert_eq!(
            builtin_sig(Builtin::ReadToString, &globals).ret,
            Ty::Result(Box::new(Ty::String), Box::new(code.clone()))
        );
        assert_eq!(
            builtin_sig(Builtin::WriteFile, &globals).ret,
            Ty::Result(Box::new(Ty::Unit), Box::new(code.clone()))
        );
        assert_eq!(
            builtin_sig(Builtin::RunCommand, &globals).ret,
            Ty::Result(Box::new(Ty::Int(IntTy::I32)), Box::new(code.clone()))
        );
        // Without a standard library `__io_kind` has no result type to name.
        assert_eq!(builtin_sig(Builtin::IoErrorKind, &Globals::default()).ret, Ty::Unit);
        let kind = Ty::Enum(burdin_tir::AdtRef { id: 0, name: "IoErrorKind".into(), second_class: false });
        let globals = Globals { io_error_kind_ty: Some(kind.clone()), ..Globals::default() };
        let sig = builtin_sig(Builtin::IoErrorKind, &globals);
        assert_eq!(sig.ret, kind);
        assert_eq!(sig.params.len(), 1);
        assert_eq!(sig.params[0].ty, code);
        assert!(!sig.params[0].own);
        // A standard library whose `IoErrorKind` is not an enum leaves the builtin untyped.
        let not_an_enum = Globals { io_error_kind_ty: Some(Ty::Int(IntTy::U8)), ..Globals::default() };
        assert_eq!(builtin_sig(Builtin::IoErrorKind, &not_an_enum).ret, Ty::Unit);
    }

    #[test]
    fn file_primitive_signatures_take_the_handle_and_the_caller_buffer() {
        let error = Ty::Int(IntTy::I32);
        let globals = Globals::default();
        for (name, found) in [
            ("__file_open", Builtin::FileOpen),
            ("__file_read", Builtin::FileRead),
            ("__file_write", Builtin::FileWrite),
            ("__file_seek", Builtin::FileSeek),
            ("__file_sync", Builtin::FileSync),
            ("__file_set_len", Builtin::FileSetLen),
            ("__file_close", Builtin::FileClose),
            ("__file_error", Builtin::FileError),
        ] {
            assert_eq!(builtin(name), Some(found));
        }

        let open = builtin_sig(Builtin::FileOpen, &globals);
        assert_eq!(open.params[0].ty, Ty::Str);
        assert_eq!(open.params.len(), 7);
        assert!(open.params[1..].iter().all(|p| p.ty == Ty::Bool));
        assert_eq!(open.ret, Ty::Result(Box::new(Ty::Int(IntTy::I32)), Box::new(error.clone())));

        let read = builtin_sig(Builtin::FileRead, &globals);
        assert_eq!(read.params[0].ty, Ty::Int(IntTy::I32));
        assert_eq!(read.params[1].ty, Ty::List(Box::new(Ty::Int(IntTy::U8)), burdin_tir::Alloc::Global));
        assert!(read.params[1].by_ref, "the primitive appends to the caller's list");
        assert!(read.params[1].mut_param, "the caller writes the buffer as `mut`");
        assert!(read.params[1].exclusive);
        assert_eq!(read.params[2].ty, Ty::Int(IntTy::Usize));
        assert_eq!(read.ret, Ty::Result(Box::new(Ty::Int(IntTy::Usize)), Box::new(error.clone())));

        let write = builtin_sig(Builtin::FileWrite, &globals);
        assert_eq!(write.params[0].ty, Ty::Int(IntTy::I32));
        assert_eq!(write.params[1].ty, Ty::Slice(Box::new(Ty::Int(IntTy::U8))));
        assert_eq!(write.ret, Ty::Result(Box::new(Ty::Int(IntTy::Usize)), Box::new(error.clone())));

        let seek = builtin_sig(Builtin::FileSeek, &globals);
        assert_eq!(seek.params.len(), 3);
        assert_eq!(seek.params[1].ty, Ty::Int(IntTy::I64));
        assert_eq!(seek.ret, Ty::Result(Box::new(Ty::Int(IntTy::I64)), Box::new(error.clone())));

        for b in [Builtin::FileSync, Builtin::FileSetLen, Builtin::FileClose] {
            assert_eq!(builtin_sig(b, &globals).ret, Ty::Result(Box::new(Ty::Unit), Box::new(error.clone())));
        }
        assert_eq!(builtin_sig(Builtin::FileError, &globals).ret, error);
        assert!(builtin_sig(Builtin::FileError, &globals).params.is_empty());
    }

    #[test]
    fn string_bytes_and_view_cast_signatures() {
        let bytes = Ty::Slice(Box::new(Ty::Int(IntTy::U8)));
        for recv in [Ty::Str, Ty::String] {
            let (found, params, ret) = method("as_bytes", &recv).unwrap();
            assert_eq!(found, Method::AsBytes, "{recv}");
            assert!(params.is_empty());
            assert_eq!(ret, bytes);
            assert!(found.result_borrows_receiver(), "{recv}");
        }
        assert_eq!(builtin("__view_as_text"), Some(Builtin::ViewAsText));
        let sig = builtin_sig(Builtin::ViewAsText, &Globals::default());
        assert_eq!(sig.params.len(), 1);
        assert_eq!(sig.params[0].ty, bytes);
        assert_eq!(sig.ret, Ty::Str);
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
        let error = Ty::Int(IntTy::I32);
        let globals = Globals::default();
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
    fn slice_method_resolution() {
        let i32_ = Ty::Int(IntTy::I32);
        let slice = Ty::Slice(Box::new(i32_.clone()));

        // `get` is the checked element view; the range form is resolved by the call checker.
        let (found, params, ret) = method("get", &slice).unwrap();
        assert_eq!(found, Method::SliceGet);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].ty, Ty::Int(IntTy::Usize));
        assert_eq!(ret, Ty::Option(Box::new(Ty::Ref { mutable: false, inner: Box::new(i32_.clone()) })));

        // The slice's own `last` and `to_list` win over the `Iterator` defaults of the same name.
        assert_eq!(method("last", &slice).unwrap().0, Method::SliceLast);
        assert_eq!(method("to_list", &slice).unwrap().2, Ty::List(Box::new(i32_.clone()), burdin_tir::Alloc::Global));

        // The window iterators carry their element and, for chunks, the exactness flag.
        assert_eq!(method("chunks", &slice).unwrap().2, Ty::SliceChunks(Box::new(i32_.clone()), false));
        assert_eq!(method("chunks_exact", &slice).unwrap().2, Ty::SliceChunks(Box::new(i32_.clone()), true));
        assert_eq!(method("windows", &slice).unwrap().2, Ty::SliceWindows(Box::new(i32_.clone())));

        // The mutating methods take the indices and the value they are given.
        assert_eq!(method("swap", &slice).unwrap().0, Method::SliceSwap);
        assert_eq!(method("fill", &slice).unwrap().1[0].ty, i32_.clone());
        assert_eq!(method("copy_from_slice", &slice).unwrap().1[0].ty, slice.clone());

        // `List<T>.as_slice()` gives the same slice type.
        let list = Ty::List(Box::new(i32_.clone()), burdin_tir::Alloc::Global);
        let (found, params, ret) = method("as_slice", &list).unwrap();
        assert_eq!(found, Method::AsSlice);
        assert!(params.is_empty());
        assert_eq!(ret, slice);

        // The comparator sort is the compiler's own; `std` writes it with a leading `__`.
        let sort = method("__sort_by", &slice).unwrap();
        assert_eq!(sort.0, Method::SliceSortBy);
        assert!(sort.1[0].own, "the comparator is moved into the sort");
    }

    #[test]
    fn slice_method_flags() {
        assert!(Method::SliceSwap.mutates_receiver());
        assert!(Method::SliceFill.mutates_receiver());
        assert!(Method::SliceCopyFromSlice.mutates_receiver());
        assert!(Method::SliceCloneFromSlice.mutates_receiver());
        assert!(Method::SliceSortBy.mutates_receiver());
        assert!(!Method::SliceGet.mutates_receiver());
        assert!(!Method::SliceChunks.mutates_receiver());

        assert!(Method::AsSlice.result_borrows_receiver());
        assert!(Method::SliceGet.result_borrows_receiver());
        assert!(Method::SliceGetRange.result_borrows_receiver());
        assert!(Method::SliceLast.result_borrows_receiver());
        assert!(Method::SliceChunks.result_borrows_receiver());
        assert!(Method::SliceWindows.result_borrows_receiver());
        assert!(!Method::SliceToList.result_borrows_receiver());

        assert!(Method::SliceSortBy.owns_argument(1));
        assert!(!Method::SliceSortBy.owns_argument(0));
        assert!(!Method::SliceFill.owns_argument(1), "fill clones the value; the caller keeps it");
    }

    #[test]
    fn clock_entropy_and_sleep_signatures() {
        let globals = Globals::default();
        assert_eq!(builtin("__clock"), Some(Builtin::Clock));
        assert_eq!(builtin("__entropy"), Some(Builtin::Entropy));
        assert_eq!(builtin("__sleep"), Some(Builtin::Sleep));

        // `__clock(kind: i32): i64`: nanoseconds from the real-time (0) or the monotonic clock.
        let clock = builtin_sig(Builtin::Clock, &globals);
        assert_eq!(clock.params.len(), 1);
        assert_eq!(clock.params[0].ty, Ty::Int(IntTy::I32));
        assert_eq!(clock.ret, Ty::Int(IntTy::I64));
        // `__entropy(): u64`.
        let entropy = builtin_sig(Builtin::Entropy, &globals);
        assert!(entropy.params.is_empty());
        assert_eq!(entropy.ret, Ty::Int(IntTy::U64));
        // `__sleep(nanos: u64)`.
        let sleep = builtin_sig(Builtin::Sleep, &globals);
        assert_eq!(sleep.params.len(), 1);
        assert_eq!(sleep.params[0].ty, Ty::Int(IntTy::U64));
        assert_eq!(sleep.ret, Ty::Unit);
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
    fn list_capacity_and_access_methods_resolve() {
        let int = Ty::Int(IntTy::I32);
        let list = Ty::List(Box::new(int.clone()), burdin_tir::Alloc::Global);
        let (found, params, ret) = method("capacity", &list).unwrap();
        assert_eq!(found, Method::ListCapacity);
        assert!(params.is_empty());
        assert_eq!(ret, Ty::Int(IntTy::Usize));

        let (found, params, ret) = method("reserve_exact", &list).unwrap();
        assert_eq!(found, Method::ListReserveExact);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].ty, Ty::Int(IntTy::Usize));
        assert_eq!(ret, Ty::Unit);

        let (found, _, ret) = method("get_mut", &list).unwrap();
        assert_eq!(found, Method::ListGetMut);
        assert_eq!(ret, Ty::Option(Box::new(Ty::Ref { mutable: true, inner: Box::new(int.clone()) })));

        let (found, _, ret) = method("first", &list).unwrap();
        assert_eq!(found, Method::ListFirst);
        assert_eq!(ret, Ty::Option(Box::new(Ty::Ref { mutable: false, inner: Box::new(int.clone()) })));

        let (found, params, ret) = method("split_off", &list).unwrap();
        assert_eq!(found, Method::ListSplitOff);
        assert_eq!(params.len(), 1);
        assert_eq!(ret, Ty::List(Box::new(int.clone()), burdin_tir::Alloc::Global));
    }

    #[test]
    fn list_bulk_methods_resolve_for_every_allocator() {
        let int = Ty::Int(IntTy::I32);
        let arena = burdin_tir::Alloc::Value(Box::new(Ty::Int(IntTy::I64)));
        let list = Ty::List(Box::new(int.clone()), arena.clone());
        // `append` takes a list of the same type and owns it; `drain` yields an owning iterator with the default
        // allocator; `extend_from_slice` takes an element view.
        let (found, params, ret) = method("append", &list).unwrap();
        assert_eq!(found, Method::ListAppend);
        assert_eq!(params[0].ty, list);
        assert!(params[0].own);
        assert_eq!(ret, Ty::Unit);

        let (found, params, ret) = method("drain", &list).unwrap();
        assert_eq!(found, Method::ListDrain);
        assert_eq!(params[0].ty, Ty::Range(Box::new(Ty::Int(IntTy::Usize)), false));
        assert_eq!(ret, Ty::ListIntoIter(Box::new(int.clone()), burdin_tir::Alloc::Global));

        let (found, params, ret) = method("extend_from_slice", &list).unwrap();
        assert_eq!(found, Method::ListExtendFromSlice);
        assert_eq!(params[0].ty, Ty::Slice(Box::new(int.clone())));
        assert_eq!(ret, Ty::Unit);
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
    fn map_lookups_take_a_str_for_a_string_key() {
        let i32_ = Ty::Int(IntTy::I32);
        let m = Ty::Map(Box::new(Ty::String), Box::new(i32_.clone()), burdin_tir::Alloc::Global);
        for name in ["get", "get_mut", "contains_key", "remove", "remove_entry"] {
            let (_, params, _) = method(name, &m).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(params[0].ty, Ty::Str, "{name} takes a borrowed key");
            assert!(!params[0].own, "{name} borrows its key");
        }
        // The operations that store the key take the map's own key type.
        for name in ["insert", "get_or_insert", "get_or"] {
            let (_, params, _) = method(name, &m).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(params[0].ty, Ty::String, "{name} takes the stored key type");
            assert!(params[0].own, "{name} owns the key it stores");
        }
        let plain = Ty::Map(Box::new(i32_.clone()), Box::new(i32_.clone()), burdin_tir::Alloc::Global);
        let (_, params, _) = method("get", &plain).unwrap();
        assert_eq!(params[0].ty, i32_);
    }

    #[test]
    fn map_updates_and_capacity_are_built_in() {
        let i32_ = Ty::Int(IntTy::I32);
        let m = Ty::Map(Box::new(i32_.clone()), Box::new(i32_.clone()), burdin_tir::Alloc::Global);
        let (found, params, ret) = method("get_or", &m).unwrap();
        assert_eq!(found, Method::MapGetOr);
        assert!(params[0].own && params[1].own);
        assert_eq!(ret, Ty::Ref { mutable: false, inner: Box::new(i32_.clone()) });
        assert!(found.mutates_receiver() && found.result_borrows_receiver());

        let (found, params, ret) = method("remove_entry", &m).unwrap();
        assert_eq!(found, Method::MapRemoveEntry);
        assert!(!params[0].own);
        assert_eq!(ret, Ty::Option(Box::new(Ty::Tuple(vec![i32_.clone(), i32_.clone()]))));

        let (found, params, _) = method("retain", &m).unwrap();
        assert_eq!(found, Method::MapRetain);
        assert!(params[0].own && found.mutates_receiver());

        let (found, _, _) = method("reserve", &m).unwrap();
        assert_eq!(found, Method::MapReserve);
        let (found, _, _) = method("shrink_to_fit", &m).unwrap();
        assert_eq!(found, Method::MapShrinkToFit);
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
