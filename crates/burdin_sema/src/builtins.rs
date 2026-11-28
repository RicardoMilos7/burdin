//! The built-in functions, types, and methods.

use crate::globals::Globals;
use burdin_tir::{Builtin, FloatOp, FloatTy, FnKind, FnSig, IntTy, Method, ParamSig, RtOp, Signature, Ty};
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
        "__dir_open" => Builtin::DirOpen,
        "__dir_next" => Builtin::DirNext,
        "__dir_close" => Builtin::DirClose,
        "__create_dir" => Builtin::CreateDir,
        "__remove_dir" => Builtin::RemoveDir,
        "__remove_file" => Builtin::RemoveFile,
        "__rename" => Builtin::RenamePath,
        "__metadata" => Builtin::Metadata,
        "__canonicalize" => Builtin::Canonicalize,
        "__io_kind" => Builtin::IoErrorKind,
        "__dns_lookup" => Builtin::DnsLookup,
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
        "__try_alloc" => Builtin::RawTryAlloc,
        "__try_realloc" => Builtin::RawTryRealloc,
        "__alloc_count" => Builtin::AllocCount,
        "__view_as_text" => Builtin::ViewAsText,
        "__bytes_into_string" => Builtin::BytesIntoString,
        _ => {
            return RtOp::ALL
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, op)| Builtin::Rt(*op))
                .or_else(|| burdin_tir::AtomicOp::from_name(name));
        }
    })
}

/// The signature of a task runtime primitive (`spec/tasks.md`, TK2): raw pointers and `usize`, and for the two that
/// start something an owned `FnOnce(): ()` closure.
fn rt_sig(op: RtOp, globals: &Globals) -> Signature {
    let ptr = || Ty::CPtr(Box::new(Ty::Int(IntTy::U8)));
    let plain = ParamSig::plain;
    let closure = || ParamSig {
        own: true,
        ..plain(Ty::Fn {
            sig: Rc::new(FnSig { params: Vec::new(), ret: Box::new(Ty::Unit) }),
            kind: FnKind::FnOnce,
            borrowed: false,
            send: burdin_tir::FnSend::unknown(),
        })
    };
    let usize_ty = || Ty::Int(IntTy::Usize);
    let i32_ty = || Ty::Int(IntTy::I32);
    let i64_ty = || Ty::Int(IntTy::I64);
    let u64_ty = || Ty::Int(IntTy::U64);
    let (params, ret) = match op {
        RtOp::ThreadStart => (vec![closure()], Ty::Unit),
        RtOp::ThreadYield | RtOp::SpinLock | RtOp::SpinUnlock => (Vec::new(), Ty::Unit),
        RtOp::MutexNew | RtOp::CondNew | RtOp::CtxMain | RtOp::TlsGet | RtOp::GlobalGet => (Vec::new(), ptr()),
        RtOp::MutexLock
        | RtOp::MutexUnlock
        | RtOp::MutexFree
        | RtOp::CondSignal
        | RtOp::CondBroadcast
        | RtOp::CondFree
        | RtOp::CtxFree
        | RtOp::TlsSet
        | RtOp::GlobalSet => (vec![plain(ptr())], Ty::Unit),
        RtOp::CondWait | RtOp::CtxSwitch => (vec![plain(ptr()), plain(ptr())], Ty::Unit),
        RtOp::CtxNew => (vec![plain(usize_ty()), closure()], ptr()),
        RtOp::CpuCount | RtOp::ConfigGet => (Vec::new(), usize_ty()),
        RtOp::ConfigSet => (vec![plain(usize_ty())], Ty::Unit),
        // The reactor (`spec/io.md`, IO2). A wait record is a raw pointer the standard library lays out
        // (`Wait`, `reactor.bd`); the reactor primitives only link records through their first word.
        RtOp::IoNew => (Vec::new(), ptr()),
        RtOp::IoWatch => (vec![plain(ptr()), plain(i64_ty()), plain(i32_ty()), plain(ptr())], i32_ty()),
        RtOp::IoUnwatch => (vec![plain(ptr()), plain(i64_ty())], Ty::Unit),
        RtOp::IoPoll => (vec![plain(ptr()), plain(i64_ty())], ptr()),
        RtOp::IoWake | RtOp::IoGlobalSet => (vec![plain(ptr())], Ty::Unit),
        RtOp::IoGlobalGet => (Vec::new(), ptr()),
        // `__pool_submit(io, wait, f)`: the helper runs `f`; completion goes through `wait`.
        RtOp::PoolSubmit => (vec![plain(ptr()), plain(ptr()), closure()], Ty::Unit),
        // `__pool_submit_raw(io, wait, code, env)`: the closure as its two raw words.
        RtOp::PoolSubmitRaw => (vec![plain(ptr()), plain(ptr()), plain(ptr()), plain(ptr())], Ty::Unit),
        // Sockets (`spec/io.md`, IO5). A descriptor is an `i64`, because a Windows `SOCKET` is pointer-sized.
        // The socket type is 1 for TCP and 2 for UDP; the event mask is 1 for readable and 2 for writable.
        RtOp::NetSocket => (vec![plain(i32_ty())], i64_ty()),
        RtOp::NetBind => (vec![plain(i64_ty()), plain(Ty::Str), plain(i32_ty())], i64_ty()),
        RtOp::NetListen => (vec![plain(i64_ty()), plain(i32_ty())], i64_ty()),
        RtOp::NetAccept => (vec![plain(i64_ty())], i64_ty()),
        RtOp::NetConnect => (vec![plain(i64_ty()), plain(Ty::Str), plain(i32_ty())], i64_ty()),
        RtOp::NetSend | RtOp::NetSendTo => {
            let mut params = vec![plain(i64_ty()), plain(Ty::Slice(Box::new(Ty::Int(IntTy::U8))))];
            if op == RtOp::NetSendTo {
                params.push(plain(Ty::Str));
                params.push(plain(i32_ty()));
            }
            (params, i64_ty())
        }
        RtOp::NetRecv | RtOp::NetRecvFrom => {
            let mut buf = ParamSig::plain(globals.list_of(Ty::Int(IntTy::U8)));
            buf.by_ref = true;
            buf.mut_param = true;
            buf.exclusive = true;
            let params = vec![plain(i64_ty()), buf, plain(usize_ty())];
            (params, i64_ty())
        }
        RtOp::NetClose => (vec![plain(i64_ty())], i64_ty()),
        RtOp::NetShutdown => (vec![plain(i64_ty()), plain(i32_ty())], i64_ty()),
        RtOp::NetError => (vec![plain(i64_ty())], i32_ty()),
        RtOp::NetSetNoDelay => (vec![plain(i64_ty()), plain(i32_ty())], i64_ty()),
        RtOp::NetLocalAddr | RtOp::NetPeerAddr => (vec![plain(i64_ty())], u64_ty()),
        RtOp::NetLastPeer => (Vec::new(), u64_ty()),
        RtOp::NetLastError => (Vec::new(), i32_ty()),
    };
    Signature { params, ret, has_self: false, ret_origins: Vec::new() }
}

/// The signature of an atomic primitive (`spec/shared.md`, SH7): a raw pointer to a word of the width's unsigned
/// type, and values of that type.
fn atomic_sig(op: burdin_tir::AtomicOp, width: burdin_tir::AtomicWidth) -> Signature {
    use burdin_tir::AtomicOp;
    let word = || match width {
        burdin_tir::AtomicWidth::W32 => Ty::Int(IntTy::U32),
        burdin_tir::AtomicWidth::W64 => Ty::Int(IntTy::U64),
    };
    let ptr = || ParamSig::plain(Ty::CPtr(Box::new(word())));
    let value = || ParamSig::plain(word());
    let (params, ret) = match op {
        AtomicOp::Load => (vec![ptr()], word()),
        AtomicOp::Store => (vec![ptr(), value()], Ty::Unit),
        AtomicOp::Cas => (vec![ptr(), value(), value()], word()),
        AtomicOp::Fence => (Vec::new(), Ty::Unit),
        _ => (vec![ptr(), value()], word()),
    };
    Signature { params, ret, has_self: false, ret_origins: Vec::new() }
}

pub(crate) fn builtin_sig(b: Builtin, globals: &Globals) -> Signature {
    if let Builtin::Rt(op) = b {
        return rt_sig(op, globals);
    }
    if let Builtin::Atomic(op, width) = b {
        return atomic_sig(op, width);
    }
    // The I/O primitives report the operating system's own error code; the `std` wrappers turn it into the
    // `IoError` of `spec/errors.md` ERR10 with `__io_kind` for the portable kind (ERR11).
    let os_code = || Ty::Int(IntTy::I32);
    let ret = match b {
        Builtin::Panic | Builtin::Unreachable | Builtin::ProcessExit => Ty::Never,
        Builtin::TargetOs | Builtin::TargetArch => Ty::Str,
        Builtin::StringNew
        | Builtin::StringFrom
        | Builtin::CStrToString
        | Builtin::FloatStr
        | Builtin::BytesIntoString => Ty::String,
        Builtin::ViewAsText => Ty::Str,
        Builtin::ReadToString | Builtin::DnsLookup => globals.result_ty(Ty::String, os_code()),
        Builtin::WriteFile => globals.result_ty(Ty::Unit, os_code()),
        Builtin::FileOpen => globals.result_ty(Ty::Int(IntTy::I32), os_code()),
        Builtin::FileRead | Builtin::FileWrite => globals.result_ty(Ty::Int(IntTy::Usize), os_code()),
        Builtin::FileSeek => globals.result_ty(Ty::Int(IntTy::I64), os_code()),
        Builtin::FileSync | Builtin::FileSetLen | Builtin::FileClose => globals.result_ty(Ty::Unit, os_code()),
        Builtin::StdinReadLine => globals.result_ty(globals.option_ty(Ty::String), os_code()),
        Builtin::StdinReadToString => globals.result_ty(Ty::String, os_code()),
        Builtin::IsTerminal => Ty::Bool,
        Builtin::StdinAvail => Ty::Int(IntTy::Isize),
        Builtin::StdinNextByte => Ty::Int(IntTy::U8),
        Builtin::StdinError => Ty::Int(IntTy::I32),
        Builtin::FileError => Ty::Int(IntTy::I32),
        Builtin::ReadDir => globals.result_ty(globals.list_of(Ty::String), os_code()),
        Builtin::DirOpen => globals.result_ty(Ty::Int(IntTy::U64), os_code()),
        Builtin::DirNext => globals.result_ty(Ty::Bool, os_code()),
        Builtin::DirClose => Ty::Unit,
        Builtin::CreateDir | Builtin::RemoveDir | Builtin::RemoveFile | Builtin::RenamePath => {
            globals.result_ty(Ty::Unit, os_code())
        }
        // `(size: u64, mode: u64, modified: i64, accessed: i64, flags: u64)`: the raw `stat` fields, which
        // `std` turns into `Metadata`.
        Builtin::Metadata => globals.result_ty(
            Ty::Tuple(vec![
                Ty::Int(IntTy::U64),
                Ty::Int(IntTy::U64),
                Ty::Int(IntTy::I64),
                Ty::Int(IntTy::I64),
                Ty::Int(IntTy::U64),
            ]),
            os_code(),
        ),
        Builtin::Canonicalize => globals.result_ty(Ty::String, os_code()),
        Builtin::Clock => Ty::Int(IntTy::I64),
        Builtin::Entropy => Ty::Int(IntTy::U64),
        Builtin::EnvArgs | Builtin::EnvList => globals.list_of(Ty::String),
        Builtin::EnvGet | Builtin::CurrentDir => globals.option_ty(globals.list_of(Ty::Int(IntTy::U8))),
        Builtin::EnvSet
        | Builtin::EnvSetBytes
        | Builtin::EnvRemove
        | Builtin::SetCurrentDir
        | Builtin::SetCurrentDirBytes
        | Builtin::PipeWrite => Ty::Int(IntTy::I32),
        Builtin::ProcessWait => Ty::Int(IntTy::I64),
        // `(pid: i32, process: usize, stdin: usize, stdout: usize, stderr: usize)`: the child's identity and the
        // pipe ends `std` wraps. `process` is the Windows process handle and 0 on Unix.
        Builtin::ProcessSpawn => globals.result_ty(
            Ty::Tuple(vec![
                Ty::Int(IntTy::I32),
                Ty::Int(IntTy::Usize),
                Ty::Int(IntTy::Usize),
                Ty::Int(IntTy::Usize),
                Ty::Int(IntTy::Usize),
            ]),
            os_code(),
        ),
        // The process and pipe primitives report 0 when their runtime path has no OS code to give; the `std`
        // wrappers turn 0 into the `Other` kind and `os_code() == None` (`spec/errors.md`, ERR10).
        Builtin::PipeReadToString => globals.result_ty(Ty::String, os_code()),
        Builtin::PipeReadLine => globals.result_ty(globals.option_ty(Ty::String), os_code()),
        Builtin::PipeReadBoth => globals.result_ty(Ty::Tuple(vec![Ty::String, Ty::String]), os_code()),
        Builtin::RunCommand => globals.result_ty(Ty::Int(IntTy::I32), os_code()),
        // `IoErrorKind` is the enum the standard library declares; a standard library without that enum (or
        // with a struct of the same name) leaves the builtin untyped, and a caller then fails to type-check
        // instead of the code generator meeting an unexpected type.
        Builtin::IoErrorKind => match &globals.io_error_kind_ty {
            Some(Ty::Enum(_)) => globals.io_error_kind_ty.clone().unwrap(),
            _ => Ty::Unit,
        },
        Builtin::RawAlloc | Builtin::RawRealloc | Builtin::RawTryAlloc | Builtin::RawTryRealloc => {
            Ty::CPtr(Box::new(Ty::Int(IntTy::U8)))
        }
        Builtin::FloatFromBits(ft) => Ty::Float(ft),
        Builtin::AllocCount => Ty::Int(IntTy::U64),
        Builtin::StringWithCapacity => Ty::String,
        // `String.try_with_capacity(n)` (`spec/allocators.md`, AL11): the error is the standard library's `AllocError`.
        Builtin::StringTryWithCapacity => {
            globals.result_ty(Ty::String, globals.alloc_error_ty.clone().unwrap_or(Ty::Error))
        }
        _ => Ty::Unit,
    };
    let params = match b {
        Builtin::StringNew
        | Builtin::AllocCount
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
            ParamSig::plain(globals.list_of(Ty::String)),
            ParamSig::plain(globals.list_of(Ty::String)),
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
        Builtin::BytesIntoString => {
            vec![ParamSig { own: true, ..ParamSig::plain(globals.list_of(Ty::Int(IntTy::U8))) }]
        }
        // The environment primitives take the raw text; the `std` wrappers add the UTF-8 checks and the error
        // values (`spec/stdlib.md`, "Environment variables and the working directory").
        Builtin::EnvGet | Builtin::EnvRemove | Builtin::SetCurrentDir => vec![ParamSig::plain(Ty::Str)],
        Builtin::EnvSet => vec![ParamSig::plain(Ty::Str), ParamSig::plain(Ty::Str)],
        Builtin::EnvSetBytes => {
            vec![ParamSig::plain(Ty::Str), ParamSig::plain(Ty::Slice(Box::new(Ty::Int(IntTy::U8))))]
        }
        Builtin::SetCurrentDirBytes => vec![ParamSig::plain(Ty::Slice(Box::new(Ty::Int(IntTy::U8))))],
        Builtin::StringFrom => vec![ParamSig::plain(Ty::Str)],
        Builtin::StringWithCapacity | Builtin::StringTryWithCapacity => vec![ParamSig::plain(Ty::Int(IntTy::Usize))],
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
            let mut buf = ParamSig::plain(globals.list_of(Ty::Int(IntTy::U8)));
            buf.by_ref = true;
            buf.mut_param = true;
            buf.exclusive = true;
            vec![ParamSig::plain(Ty::Int(IntTy::I32)), buf, ParamSig::plain(Ty::Int(IntTy::Usize))]
        }
        Builtin::FileWrite => {
            vec![ParamSig::plain(Ty::Int(IntTy::I32)), ParamSig::plain(Ty::Slice(Box::new(Ty::Int(IntTy::U8))))]
        }
        // The name is the caller's string: the primitive replaces its contents, reusing its buffer.
        Builtin::DirNext => {
            let mut name = ParamSig::plain(Ty::String);
            name.by_ref = true;
            name.mut_param = true;
            name.exclusive = true;
            vec![ParamSig::plain(Ty::Int(IntTy::U64)), name]
        }
        Builtin::DirClose => vec![ParamSig::plain(Ty::Int(IntTy::U64))],
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
        Builtin::ReadToString | Builtin::RunCommand | Builtin::DnsLookup => vec![ParamSig::plain(Ty::Str)],
        Builtin::IoErrorKind => vec![ParamSig::plain(Ty::Int(IntTy::I32))],
        Builtin::ProcessExit => vec![ParamSig::plain(Ty::Int(IntTy::I32))],
        Builtin::RawAlloc | Builtin::RawTryAlloc => {
            vec![ParamSig::plain(Ty::Int(IntTy::Usize)), ParamSig::plain(Ty::Int(IntTy::Usize))]
        }
        Builtin::RawRealloc | Builtin::RawTryRealloc => vec![
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
        Builtin::Swap
        | Builtin::Replace
        | Builtin::RawRead
        | Builtin::RawWrite
        | Builtin::SliceRaw
        | Builtin::IterMutRaw
        | Builtin::IntStep
        | Builtin::HashKey => Vec::new(),
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
        "i128" => IntTy::I128,
        "isize" => IntTy::Isize,
        "u8" => IntTy::U8,
        "u16" => IntTy::U16,
        "u32" => IntTy::U32,
        "u64" => IntTy::U64,
        "u128" => IntTy::U128,
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

/// `Result<(), AllocError>`, what a fallible reservation returns (`spec/allocators.md`, AL11).
fn alloc_result(g: &Globals) -> Ty {
    g.result_ty(Ty::Unit, g.alloc_error_ty.clone().unwrap_or(Ty::Error))
}

/// The built-in methods of a `Map<K, V, A>` (`recv`): its iterators, whose views read the table in place. Every
/// other method of a map is written in Burdin in `std/src/std.bd`.
fn map_method(_g: &Globals, name: &str, _recv: &Ty, k: &Ty, v: &Ty) -> Option<(Method, Vec<ParamSig>, Ty)> {
    let (k, v) = (&Box::new(k.clone()), &Box::new(v.clone()));
    Some(match (name, ()) {
        ("iter", ()) => (Method::Iter, vec![], Ty::MapIter(k.clone(), v.clone())),
        ("iter_mut", ()) => (Method::IterMut, vec![], Ty::MapIterMut(k.clone(), v.clone())),
        _ => return None,
    })
}

/// The built-in method `name` on a receiver of type `recv`: which method it is, its parameters after the
/// receiver, and its result. `None` when `recv` has no such method.
pub(crate) fn method(g: &Globals, name: &str, recv: &Ty) -> Option<(Method, Vec<ParamSig>, Ty)> {
    // The floating-point functions the compiler provides (`spec/stdlib.md`, "Float functions"). The rest of the
    // float surface is an extension method in `std`.
    if matches!(recv, Ty::Float(_)) {
        return float_method(name, recv);
    }
    if let Some((k, v, _)) = g.map_parts(recv) {
        return map_method(g, name, recv, &k, &v);
    }
    let plain = |ty: Ty| ParamSig::plain(ty);
    let elem = |t: &Ty| t.clone();
    Some(match (name, recv) {
        ("to_string", Ty::Int(_) | Ty::Bool | Ty::Char | Ty::Str) => (Method::ToString, vec![], Ty::String),
        ("wrapping_add", Ty::Int(_)) => (Method::WrappingAdd, vec![plain(recv.clone())], recv.clone()),
        ("wrapping_sub", Ty::Int(_)) => (Method::WrappingSub, vec![plain(recv.clone())], recv.clone()),
        ("wrapping_mul", Ty::Int(_)) => (Method::WrappingMul, vec![plain(recv.clone())], recv.clone()),
        ("checked_add", Ty::Int(_)) => (Method::CheckedAdd, vec![plain(recv.clone())], g.option_ty(recv.clone())),
        ("checked_sub", Ty::Int(_)) => (Method::CheckedSub, vec![plain(recv.clone())], g.option_ty(recv.clone())),
        ("checked_mul", Ty::Int(_)) => (Method::CheckedMul, vec![plain(recv.clone())], g.option_ty(recv.clone())),
        ("min", Ty::Int(_)) => (Method::Min, vec![plain(recv.clone())], recv.clone()),
        ("max", Ty::Int(_)) => (Method::Max, vec![plain(recv.clone())], recv.clone()),
        ("abs", Ty::Int(i)) if i.signed() => (Method::Abs, vec![], recv.clone()),
        ("push_str", Ty::String) => (Method::PushStr, vec![plain(Ty::Str)], Ty::Unit),
        ("push", Ty::String) => (Method::PushChar, vec![plain(Ty::Char)], Ty::Unit),
        ("as_str", Ty::String | Ty::Str) => (Method::AsStr, vec![], Ty::Str),
        // The slice methods the compiler implements (the algorithms that write through a slice are in `std`).
        // `get(range)` is resolved by the call checker from the argument's type.
        ("get", Ty::Slice(t)) => {
            let view = Ty::Ref { mutable: false, inner: t.clone() };
            (Method::SliceGet, vec![plain(Ty::Int(IntTy::Usize))], g.option_ty(view))
        }
        // The slice's own `last` and `to_list`: they win over the `Iterator` defaults of the same name.
        ("last", Ty::Slice(t)) => {
            let view = Ty::Ref { mutable: false, inner: t.clone() };
            (Method::SliceLast, vec![], g.option_ty(view))
        }
        ("to_list", Ty::Slice(t)) => (Method::SliceToList, vec![], g.list_of((**t).clone())),
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
                send: burdin_tir::FnSend::unknown(),
            };
            (Method::SliceSortBy, vec![value_param(less)], Ty::Unit)
        }
        ("as_bytes", Ty::String | Ty::Str) => (Method::AsBytes, vec![], Ty::Slice(Box::new(Ty::Int(IntTy::U8)))),
        // The buffer operations of `String` (`spec/stdlib.md`, "String capacity and editing").
        ("capacity", Ty::String) => (Method::StringCapacity, vec![], Ty::Int(IntTy::Usize)),
        ("reserve", Ty::String) => (Method::StringReserve, vec![plain(Ty::Int(IntTy::Usize))], Ty::Unit),
        ("try_reserve", Ty::String) => (Method::StringTryReserve, vec![plain(Ty::Int(IntTy::Usize))], alloc_result(g)),
        ("shrink_to_fit", Ty::String) => (Method::StringShrinkToFit, vec![], Ty::Unit),
        ("insert_str", Ty::String) => {
            (Method::StringInsertStr, vec![plain(Ty::Int(IntTy::Usize)), plain(Ty::Str)], Ty::Unit)
        }
        ("replace_range", Ty::String) => (
            Method::StringReplaceRange,
            vec![plain(Ty::Int(IntTy::Usize)), plain(Ty::Int(IntTy::Usize)), plain(Ty::Str)],
            Ty::Unit,
        ),
        ("len", Ty::Str | Ty::String | Ty::Slice(_) | Ty::Array(_, _)) => (Method::Len, vec![], Ty::Int(IntTy::Usize)),
        ("starts_with", Ty::Str | Ty::String) => (Method::StartsWith, vec![plain(Ty::Str)], Ty::Bool),
        ("find", Ty::Str | Ty::String) => (Method::Find, vec![plain(Ty::Char)], g.option_ty(Ty::Int(IntTy::Usize))),
        ("find_str", Ty::Str | Ty::String) => {
            (Method::FindStr, vec![plain(Ty::Str)], g.option_ty(Ty::Int(IntTy::Usize)))
        }
        ("is_null", Ty::CPtr(_)) => (Method::IsNull, vec![], Ty::Bool),
        // The iteration protocol's entry points (spec/iterators.md). `next` is checked separately because a
        // slice yields its element by value when it is `Copy`.
        ("iter", Ty::Array(elem, _)) => (Method::Iter, vec![], Ty::Slice(elem.clone())),
        ("iter", Ty::Slice(_)) => (Method::Iter, vec![], recv.clone()),
        // The mutable iterators yield `&mut T` and borrow the collection exclusively (IT15).
        ("iter_mut", Ty::Array(elem, _)) => (Method::IterMut, vec![], Ty::IterMut(elem.clone())),
        ("into_iter", Ty::Array(elem, n)) => (Method::IntoIter, vec![], g.array_into_iter_ty((**elem).clone(), *n)),
        _ => return None,
    })
}

/// Built-in method names available on `recv`, for diagnostic hints.
pub(crate) fn builtin_method_names(recv: &Ty) -> &'static [&'static str] {
    match recv {
        Ty::Int(i) if i.signed() => &[
            "to_string",
            "wrapping_add",
            "wrapping_sub",
            "wrapping_mul",
            "checked_add",
            "checked_sub",
            "checked_mul",
            "min",
            "max",
            "abs",
        ],
        Ty::Int(_) => &[
            "to_string",
            "wrapping_add",
            "wrapping_sub",
            "wrapping_mul",
            "checked_add",
            "checked_sub",
            "checked_mul",
            "min",
            "max",
        ],
        Ty::Float(_) => &[
            "abs",
            "sqrt",
            "cbrt",
            "powi",
            "powf",
            "exp",
            "exp2",
            "ln",
            "log2",
            "log10",
            "floor",
            "ceil",
            "round",
            "round_ties_even",
            "trunc",
            "sin",
            "cos",
            "tan",
            "asin",
            "acos",
            "atan",
            "atan2",
            "sinh",
            "cosh",
            "tanh",
            "hypot",
            "mul_add",
            "copysign",
            "to_bits",
        ],
        Ty::Bool | Ty::Char => &["to_string"],
        Ty::Str => &["len", "to_string", "as_str", "as_bytes", "starts_with", "find", "find_str", "chars"],
        Ty::String => &[
            "len",
            "push_str",
            "push",
            "as_str",
            "as_bytes",
            "capacity",
            "reserve",
            "try_reserve",
            "shrink_to_fit",
            "insert_str",
            "replace_range",
            "starts_with",
            "find",
            "find_str",
            "chars",
        ],
        Ty::Slice(_) => &[
            "len",
            "get",
            "last",
            "to_list",
            "swap",
            "fill",
            "copy_from_slice",
            "clone_from_slice",
            "chunks",
            "chunks_exact",
            "windows",
            "iter",
        ],
        Ty::Array(..) => &["len", "iter", "iter_mut", "into_iter"],
        Ty::CPtr(_) => &["is_null"],
        _ => &[],
    }
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

    use crate::testutil::{lang_globals, settled};

    /// `builtin_sig(b)` with `Option` and `Result` settled, and the `Globals` that name its types.
    fn sig_of(b: Builtin) -> (Signature, Globals) {
        // An instance whose argument is another instance needs a second round, as the allocator's passes do.
        let mut g = lang_globals();
        for _ in 0..3 {
            builtin_sig(b, &g);
            g = settled(g);
        }
        (builtin_sig(b, &g), g)
    }

    /// `method(name, recv)` with `Option` and `Result` settled, and the `Globals` that name its types.
    fn lang_method(name: &str, recv: &Ty) -> (Option<(Method, Vec<ParamSig>, Ty)>, Globals) {
        let mut g = lang_globals();
        for _ in 0..3 {
            super::method(&g, name, recv);
            g = settled(g);
        }
        (super::method(&g, name, recv), g)
    }

    /// `method` for a receiver that needs no `Option` or `Result`.
    fn method(name: &str, recv: &Ty) -> Option<(Method, Vec<ParamSig>, Ty)> {
        super::method(&Globals::default(), name, recv)
    }

    fn ref_(inner: Ty, mutable: bool) -> Ty {
        Ty::Ref { mutable, inner: Box::new(inner) }
    }

    #[test]
    fn io_primitive_signatures_use_the_operating_system_code() {
        // The primitives report the OS code itself; `std` maps it with `__io_kind` (spec/errors.md, ERR10-ERR11).
        let code = Ty::Int(IntTy::I32);
        for (b, ok) in [
            (Builtin::ReadToString, Ty::String),
            (Builtin::WriteFile, Ty::Unit),
            (Builtin::RunCommand, Ty::Int(IntTy::I32)),
        ] {
            let (sig, g) = sig_of(b);
            assert_eq!(g.result_parts(&sig.ret), Some((ok, code.clone())));
        }
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

        let (open, g) = sig_of(Builtin::FileOpen);
        assert_eq!(open.params[0].ty, Ty::Str);
        assert_eq!(open.params.len(), 7);
        assert!(open.params[1..].iter().all(|p| p.ty == Ty::Bool));
        assert_eq!(g.result_parts(&open.ret), Some((Ty::Int(IntTy::I32), error.clone())));

        let (read, g) = sig_of(Builtin::FileRead);
        assert_eq!(read.params[0].ty, Ty::Int(IntTy::I32));
        assert!(read.params[1].by_ref, "the primitive appends to the caller's list");
        assert!(read.params[1].mut_param, "the caller writes the buffer as `mut`");
        assert!(read.params[1].exclusive);
        assert_eq!(read.params[2].ty, Ty::Int(IntTy::Usize));
        assert_eq!(g.result_parts(&read.ret), Some((Ty::Int(IntTy::Usize), error.clone())));

        let (write, g) = sig_of(Builtin::FileWrite);
        assert_eq!(write.params[0].ty, Ty::Int(IntTy::I32));
        assert_eq!(write.params[1].ty, Ty::Slice(Box::new(Ty::Int(IntTy::U8))));
        assert_eq!(g.result_parts(&write.ret), Some((Ty::Int(IntTy::Usize), error.clone())));

        let (seek, g) = sig_of(Builtin::FileSeek);
        assert_eq!(seek.params.len(), 3);
        assert_eq!(seek.params[1].ty, Ty::Int(IntTy::I64));
        assert_eq!(g.result_parts(&seek.ret), Some((Ty::Int(IntTy::I64), error.clone())));

        for b in [Builtin::FileSync, Builtin::FileSetLen, Builtin::FileClose] {
            let (sig, g) = sig_of(b);
            assert_eq!(g.result_parts(&sig.ret), Some((Ty::Unit, error.clone())));
        }
        assert_eq!(builtin_sig(Builtin::FileError, &globals).ret, error);
        assert!(builtin_sig(Builtin::FileError, &globals).params.is_empty());
    }

    #[test]
    fn directory_primitives_take_a_handle_and_reuse_the_caller_name() {
        let error = Ty::Int(IntTy::I32);
        for (name, found) in
            [("__dir_open", Builtin::DirOpen), ("__dir_next", Builtin::DirNext), ("__dir_close", Builtin::DirClose)]
        {
            assert_eq!(builtin(name), Some(found));
        }

        let (open, g) = sig_of(Builtin::DirOpen);
        assert_eq!(open.params.len(), 1);
        assert_eq!(open.params[0].ty, Ty::Str);
        assert_eq!(g.result_parts(&open.ret), Some((Ty::Int(IntTy::U64), error.clone())));

        let (next, g) = sig_of(Builtin::DirNext);
        assert_eq!(next.params[0].ty, Ty::Int(IntTy::U64));
        assert_eq!(next.params[1].ty, Ty::String);
        assert!(next.params[1].by_ref, "the primitive overwrites the caller's string");
        assert!(next.params[1].mut_param);
        assert!(next.params[1].exclusive);
        assert_eq!(g.result_parts(&next.ret), Some((Ty::Bool, error)));

        let (close, _) = sig_of(Builtin::DirClose);
        assert_eq!(close.params.len(), 1);
        assert_eq!(close.params[0].ty, Ty::Int(IntTy::U64));
        assert_eq!(close.ret, Ty::Unit);
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
        assert_eq!(builtin("__bytes_into_string"), Some(Builtin::BytesIntoString));
        let globals = Globals::default();
        let sig = builtin_sig(Builtin::BytesIntoString, &globals);
        assert_eq!(sig.params.len(), 1);
        assert!(sig.params[0].own);
        assert_eq!(sig.params[0].ty, globals.list_of(Ty::Int(IntTy::U8)));
        assert_eq!(sig.ret, Ty::String);
    }

    #[test]
    fn string_capacity_and_editing_signatures() {
        let usize_ = Ty::Int(IntTy::Usize);
        let (found, params, ret) = method("capacity", &Ty::String).unwrap();
        assert_eq!(found, Method::StringCapacity);
        assert!(params.is_empty());
        assert_eq!(ret, usize_);
        assert!(!found.mutates_receiver(), "`capacity` only reads the buffer");
        for name in ["reserve", "shrink_to_fit", "insert_str", "replace_range"] {
            let (found, _, ret) = method(name, &Ty::String).unwrap();
            assert_eq!(ret, Ty::Unit, "{name}");
            assert!(found.mutates_receiver(), "{name}");
        }
        let (_, params, _) = method("replace_range", &Ty::String).unwrap();
        assert_eq!(params.len(), 3);
        assert_eq!(params[2].ty, Ty::Str);
        assert!(!params[2].own, "the inserted text is a view");
        let sig = builtin_sig(Builtin::StringWithCapacity, &Globals::default());
        assert_eq!(sig.ret, Ty::String);
        assert_eq!(sig.params[0].ty, usize_);
    }

    #[test]
    fn environment_primitives_have_their_declared_signatures() {
        let slice = Ty::Slice(Box::new(Ty::Int(IntTy::U8)));
        assert_eq!(builtin("__env_get"), Some(Builtin::EnvGet));
        assert_eq!(builtin("__env_set"), Some(Builtin::EnvSet));
        assert_eq!(builtin("__env_set_bytes"), Some(Builtin::EnvSetBytes));
        assert_eq!(builtin("__env_remove"), Some(Builtin::EnvRemove));
        assert_eq!(builtin("__current_dir"), Some(Builtin::CurrentDir));
        assert_eq!(builtin("__set_current_dir"), Some(Builtin::SetCurrentDir));
        assert_eq!(builtin("__set_current_dir_bytes"), Some(Builtin::SetCurrentDirBytes));

        let (get, _) = sig_of(Builtin::EnvGet);
        assert_eq!(get.params.len(), 1);
        assert_eq!(get.params[0].ty, Ty::Str);
        let (cwd, _) = sig_of(Builtin::CurrentDir);
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

        let (spawn, g) = sig_of(Builtin::ProcessSpawn);
        assert_eq!(spawn.params.len(), 8);
        assert_eq!(spawn.params[0].ty, Ty::Str);
        assert_eq!(spawn.params[3].ty, Ty::Str);
        assert_eq!(spawn.params[4].ty, Ty::Str);
        for param in spawn.params.iter().skip(5) {
            assert_eq!(param.ty, Ty::Int(IntTy::I32));
        }
        assert!(g.result_parts(&spawn.ret).is_some());

        let wait = builtin_sig(Builtin::ProcessWait, &Globals::default());
        assert_eq!(wait.params.len(), 2);
        assert_eq!(wait.params[0].ty, Ty::Int(IntTy::I32));
        assert_eq!(wait.params[1].ty, usize_.clone());
        assert_eq!(wait.ret, Ty::Int(IntTy::I64));

        let close = builtin_sig(Builtin::ProcessClose, &Globals::default());
        assert_eq!(close.params.len(), 1);
        assert_eq!(close.params[0].ty, usize_.clone());
        assert_eq!(close.ret, Ty::Unit);

        let (line, g) = sig_of(Builtin::PipeReadLine);
        assert_eq!(line.params.len(), 1);
        assert_eq!(line.params[0].ty, usize_.clone());
        assert!(g.result_parts(&line.ret).is_some());

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

        let (line, g) = sig_of(Builtin::StdinReadLine);
        assert!(line.params.is_empty());
        let (ok, err) = g.result_parts(&line.ret).expect("`read_line` returns a `Result`");
        assert_eq!(g.option_payload(&ok), Some(Ty::String));
        assert_eq!(err, error);
        let (all, g) = sig_of(Builtin::StdinReadToString);
        assert!(all.params.is_empty());
        assert_eq!(g.result_parts(&all.ret), Some((Ty::String, error)));

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
        let (got, g) = lang_method("get", &slice);
        let (found, params, ret) = got.unwrap();
        assert_eq!(found, Method::SliceGet);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].ty, Ty::Int(IntTy::Usize));
        assert_eq!(g.option_payload(&ret), Some(ref_(i32_.clone(), false)));

        // The slice's own `last` and `to_list` win over the `Iterator` defaults of the same name.
        assert_eq!(method("last", &slice).unwrap().0, Method::SliceLast);
        assert!(method("to_list", &slice).is_some());

        // The window iterators carry their element and, for chunks, the exactness flag.
        assert_eq!(method("chunks", &slice).unwrap().2, Ty::SliceChunks(Box::new(i32_.clone()), false));
        assert_eq!(method("chunks_exact", &slice).unwrap().2, Ty::SliceChunks(Box::new(i32_.clone()), true));
        assert_eq!(method("windows", &slice).unwrap().2, Ty::SliceWindows(Box::new(i32_.clone())));

        // The mutating methods take the indices and the value they are given.
        assert_eq!(method("swap", &slice).unwrap().0, Method::SliceSwap);
        assert_eq!(method("fill", &slice).unwrap().1[0].ty, i32_.clone());
        assert_eq!(method("copy_from_slice", &slice).unwrap().1[0].ty, slice.clone());

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
    fn builtin_type_names_resolve() {
        assert_eq!(builtin_type("i32"), Some(Ty::Int(IntTy::I32)));
        assert_eq!(builtin_type("usize"), Some(Ty::Int(IntTy::Usize)));
        assert_eq!(builtin_type("f64"), Some(Ty::Float(FloatTy::F64)));
        assert_eq!(builtin_type("bool"), Some(Ty::Bool));
        assert_eq!(builtin_type("char"), Some(Ty::Char));
        assert_eq!(builtin_type("String"), Some(Ty::String));
        assert_eq!(builtin_type("CChar"), Some(Ty::Int(IntTy::I8)));
        assert_eq!(builtin_type("CSize"), Some(Ty::Int(IntTy::Usize)));
        assert_eq!(builtin_type("CVoid"), Some(Ty::Unit));
        assert_eq!(builtin_type("Point"), None);
    }

    #[test]
    fn a_maps_only_built_in_methods_are_its_iterators() {
        let i32_ = Ty::Int(IntTy::I32);
        let mut g = lang_globals();
        let m = g.test_map(Ty::String, i32_.clone());
        let method = |name: &str| super::method(&g, name, &m);
        let (found, params, ret) = method("iter").expect("iter");
        assert_eq!(found, Method::Iter);
        assert!(params.is_empty());
        assert_eq!(ret, Ty::MapIter(Box::new(Ty::String), Box::new(i32_.clone())));
        let (found, _, _) = method("iter_mut").expect("iter_mut");
        assert_eq!(found, Method::IterMut);
        // Every other method is written in Burdin in `std`, so the compiler's table has no entry for it.
        for name in [
            "insert",
            "get",
            "get_mut",
            "get_or",
            "get_or_insert",
            "remove",
            "contains_key",
            "retain",
            "len",
            "into_iter",
        ] {
            assert!(method(name).is_none(), "{name} is a method of `std`'s `Map`");
        }
    }

    /// Every method `std/src/builtins.bd` declares with a `self` exists in the compiler's method table with the same
    /// number of parameters, so the declarations the tools read cannot drift from what the compiler provides.
    #[test]
    fn the_declarations_in_builtins_bd_match_the_method_table() {
        use burdin_diag::{Diagnostics, FileId};
        use burdin_syntax::ast::{Item, ParamKind, TypeKind};
        use burdin_tir::Alloc;
        let source = include_str!("../../../std/src/builtins.bd");
        let ast = burdin_syntax::parse(FileId(0), source, &mut Diagnostics::default());
        let param = |n: &str| Ty::Param(n.into());
        let mut checked = 0;
        let mut problems = Vec::new();
        let mut g = lang_globals();
        for item in &ast.items {
            let Item::Extension(x) = item else { continue };
            if !x.func.params.iter().any(|p| matches!(p.kind, ParamKind::SelfParam)) {
                continue;
            }
            let recv = match &x.recv.kind {
                TypeKind::Slice(_) => Ty::Slice(Box::new(param("T"))),
                TypeKind::Array(..) => Ty::Array(Box::new(param("T")), 3),
                TypeKind::Path(segments) => match segments[0].name.name.as_str() {
                    // `List` is a struct of `std` now; its methods are written in Burdin.
                    "List" => continue,
                    "Map" => g.test_map(param("K"), param("V")),
                    "Heap" => Ty::Heap(Box::new(param("T")), Alloc::Global),
                    // `Option` and `Result` are enums of `std`, written in Burdin; `builtins.bd` declares neither.
                    "Option" | "Result" => continue,
                    other => match builtin_type(other) {
                        Some(ty) => ty,
                        None => continue,
                    },
                },
                _ => continue,
            };
            let name = &x.func.name.name;
            // Provided outside the method table: `clone` through the `Clone` rules, a float's `to_string` through the
            // formatter, and `context` by the call checker (`spec/errors.md`).
            if matches!((name.as_str(), &recv), ("clone", _) | ("to_string", Ty::Float(_)) | ("context", _)) {
                continue;
            }
            let declared = x.func.params.len() - 1;
            checked += 1;
            match super::method(&g, name, &recv) {
                None => problems.push(format!("{recv}.{name}: declared in builtins.bd, not provided by the compiler")),
                Some((_, params, _)) if params.len() != declared => problems.push(format!(
                    "{recv}.{name}: builtins.bd declares {declared} parameters, the compiler takes {}",
                    params.len()
                )),
                Some(_) => {}
            }
        }
        assert!(checked > 100, "only {checked} declarations were checked");
        assert!(problems.is_empty(), "{} drifted: {}", problems.len(), problems.join("; "));
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

        let (got, g) = lang_method("checked_add", &i32_);
        let (found, _, ret) = got.unwrap();
        assert_eq!(found, Method::CheckedAdd);
        assert_eq!(g.option_payload(&ret), Some(i32_.clone()));
        // `unwrap` is not a compiler method: `std` writes it in Burdin, as an extension of `Option`.
        let (got, _) = lang_method("unwrap", &g.option_ty(i32_.clone()));
        assert!(got.is_none());
    }
}
