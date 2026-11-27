//! Ids, built-in functions and methods, and operators.

use crate::ty::{FloatTy, IntTy};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FuncId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LocalId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConstId(pub usize);

/// The assertion a `Builtin::Assertion` performs (`spec/stdlib.md`, "Memory helpers and assertions"). The
/// arguments are the condition (or the two compared values) followed, in the message form, by the formatted
/// message as a `str`. The `Debug*` kinds are dropped from a release build without evaluating their arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssertKind {
    /// `assert(cond, message)`: panics unless the condition holds.
    Assert,
    /// `assert_eq(a, b, message)`: panics when the two values differ.
    Eq,
    /// `assert_ne(a, b, message)`: panics when the two values are equal.
    Ne,
    /// `debug_assert(cond[, message])`: `Assert`, removed in a release build.
    DebugAssert,
    /// `debug_assert_eq(a, b[, message])`: `Eq`, removed in a release build.
    DebugAssertEq,
    /// `debug_assert_ne(a, b[, message])`: `Ne`, removed in a release build.
    DebugAssertNe,
}

impl AssertKind {
    /// Whether this assertion is dropped from a release build (`debug_assert*`).
    pub fn is_debug(self) -> bool {
        matches!(self, AssertKind::DebugAssert | AssertKind::DebugAssertEq | AssertKind::DebugAssertNe)
    }

    /// Whether the assertion compares two values (`a, b`) instead of taking one condition.
    pub fn compares_values(self) -> bool {
        matches!(self, AssertKind::Eq | AssertKind::Ne | AssertKind::DebugAssertEq | AssertKind::DebugAssertNe)
    }

    /// Whether a failed comparison means the values were equal (`assert_ne`) rather than different.
    pub fn expects_difference(self) -> bool {
        matches!(self, AssertKind::Ne | AssertKind::DebugAssertNe)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Print,
    Println,
    Eprint,
    Eprintln,
    Panic,
    /// `String.new()`: an empty string.
    StringNew,
    /// `String.from(str)`: a new string with a copy of the bytes.
    StringFrom,
    /// `Heap.new(value)`: allocates and moves `value` into a fresh heap allocation.
    HeapNew,
    /// `List<T>.new_in(alloc)`: an empty list whose buffer uses the allocator value (`spec/allocators.md`, AL5).
    ListNewIn,
    /// `List<T>.with_capacity(n)`: an empty list with room for `n` elements in the default allocator.
    ListWithCapacity,
    /// `List<T>.with_capacity_in(n, alloc)`: an empty list with room for `n` elements, allocated through the
    /// allocator value, which the list then stores.
    ListWithCapacityIn,
    /// `List<T>.try_with_capacity(n)`: `with_capacity` that returns `Err(AllocError)` instead of panicking when
    /// the buffer cannot be had (`spec/allocators.md`, AL11).
    ListTryWithCapacity,
    /// `List<T, A>.try_with_capacity_in(n, alloc)`: the fallible `with_capacity_in`. On failure the allocator value
    /// is dropped, as the call took ownership of it.
    ListTryWithCapacityIn,
    /// `Map<K, V>.new_in(alloc)`.
    MapNewIn,
    /// `Heap.new_in(alloc, value)`: one heap allocation through the allocator value.
    HeapNewIn,
    /// `__alloc(size, align, site)`: a `Global` allocation, panicking when it fails. The site is the source
    /// location the checking allocator records for a leak report (`spec/allocators.md`, AL9).
    RawAlloc,
    /// `__realloc(ptr, old_size, new_size, align, site)`: grows or shrinks a `Global` allocation.
    RawRealloc,
    /// `__dealloc(ptr, size, align)`: frees a `Global` allocation.
    RawDealloc,
    /// `__try_alloc(size, align)`: a `Global` allocation that returns a null pointer when it fails instead of
    /// panicking (`spec/allocators.md`, AL11).
    RawTryAlloc,
    /// `__try_realloc(ptr, old_size, new_size, align)`: a `Global` reallocation that returns a null pointer when it
    /// fails, and then leaves the old block valid (`spec/allocators.md`, AL11).
    RawTryRealloc,
    /// `__alloc_count()`: how many blocks the checking allocator of a `burdin test` build has handed out since the
    /// program started, a reallocation's fresh block included; 0 in any other build (`spec/allocators.md`, AL9).
    AllocCount,
    /// `CPtr.null()`: a null C pointer, typed by the expected type.
    CPtrNull,
    /// `CStr.to_string(ptr)`: copies a NUL-terminated C string into a `String`.
    CStrToString,
    /// `List<T>.new()`: an empty list.
    ListNew,
    /// `Map<K, V>.new()`: an empty map.
    MapNew,
    /// `Map<K, V>.with_capacity(n)`: an empty map whose slot table holds `n` entries without growing.
    MapWithCapacity,
    /// `Map<K, V>.try_with_capacity(n)`: the fallible `with_capacity` (`spec/allocators.md`, AL11).
    MapTryWithCapacity,
    /// `i32.parse(str)` (and every integer type): the number, or an error.
    IntParse(IntTy),
    /// `read_to_string(path)`: the file's bytes as a `String`.
    ReadToString,
    /// `write_file(path, text)`: writes the bytes.
    WriteFile,
    /// `__file_open(path, read, write, append, truncate, create, create_new)`: opens a file and returns its
    /// descriptor (`spec/stdlib.md`, "Files and file handles").
    FileOpen,
    /// `__file_read(fd, buf, max)`: appends up to `max` bytes of the file to `buf` and returns how many.
    FileRead,
    /// `__file_write(fd, data)`: writes a prefix of `data` and returns how many bytes it took.
    FileWrite,
    /// `__file_seek(fd, offset, whence)`: moves the position and returns it (`whence` is 0, 1, or 2).
    FileSeek,
    /// `__file_sync(fd)`: makes the file's contents durable.
    FileSync,
    /// `__file_set_len(fd, size)`: truncates the file, or extends it with zeros.
    FileSetLen,
    /// `__file_close(fd)`: closes the descriptor.
    FileClose,
    /// `__file_error()`: the OS code of the last failed file operation (`spec/errors.md`, ERR11).
    FileError,
    /// `__stdout_write(text)`: appends to the standard-output buffer.
    StdoutWrite,
    /// `__stdout_write_line(text)`: appends `text` and a newline to the standard-output buffer.
    StdoutWriteLine,
    /// `__stdout_flush()`: writes the standard-output buffer now.
    StdoutFlush,
    /// `__stdout_write_bytes(data)`: appends the bytes of a `[u8]` to the standard-output buffer.
    StdoutWriteBytes,
    /// `__stderr_write_bytes(data)`: writes the bytes of a `[u8]` to standard error, unbuffered.
    StderrWriteBytes,
    /// `__stderr_write(text)`: writes `text` to standard error, unbuffered.
    StderrWrite,
    /// `__stderr_write_line(text)`: writes `text` and a newline to standard error, unbuffered.
    StderrWriteLine,
    /// `__is_terminal(fd)`: whether the file descriptor is a terminal.
    IsTerminal,
    /// `__stdin_read_line()`: one line of standard input without its terminator, or `None` at end of input.
    StdinReadLine,
    /// `__stdin_read_to_string()`: the rest of standard input as a `String`.
    StdinReadToString,
    /// `__stdin_avail()`: how many bytes of standard input are buffered, reading a block first when none are; 0 is
    /// the end of the input and -1 a read error.
    StdinAvail,
    /// `__stdin_next_byte()`: takes the next buffered byte of standard input (only after `__stdin_avail` was positive).
    StdinNextByte,
    /// `__stdin_error()`: the OS code of the last standard-input read error (`spec/errors.md`, ERR11).
    StdinError,
    /// `__read_dir(path)`: the names of the entries in a directory (`spec/stdlib.md`, issue #411).
    ReadDir,
    /// `__create_dir(path)`: creates one directory.
    CreateDir,
    /// `__remove_dir(path)`: removes an empty directory.
    RemoveDir,
    /// `__remove_file(path)`: removes a file or a symbolic link.
    RemoveFile,
    /// `__rename(from, to)`: renames a file or a directory, replacing `to`.
    RenamePath,
    /// `__metadata(path)`: `(size, mode, modified, accessed, flags)`, the system's `stat` fields.
    Metadata,
    /// `__canonicalize(path)`: the absolute, normalized path.
    Canonicalize,
    /// `__io_kind(code)`: the portable `IoErrorKind` of an operating system error code (`spec/errors.md`,
    /// ERR11). The compiler knows the target's table (`ioerror`).
    IoErrorKind,
    /// `__dns_lookup(host) -> Result<String, i32>`: resolves a host name to its IPv4 addresses. The text is one
    /// `a.b.c.d` per line; the error is the system code (`spec/io.md`, IO6). `std` runs it on the helper pool.
    DnsLookup,
    /// `__clock(kind)`: nanoseconds from the real-time clock (kind 0, since the Unix epoch) or the monotonic clock
    /// (any other kind, from an unspecified start) (`spec/stdlib.md`, issue #195).
    Clock,
    /// `__entropy()`: 64 random bits from the operating system.
    Entropy,
    /// `__sleep(nanos)`: suspends the thread for about that long.
    Sleep,
    /// `env.args()`: the process arguments, program name first.
    EnvArgs,
    /// `__env_get(name)`: the raw bytes of an environment variable, or `None` when it is unset (or, on
    /// Windows, not valid Unicode). `spec/stdlib.md`, "Environment variables and the working directory".
    EnvGet,
    /// `__env_set(name, value)`: sets a variable from bytes. The result is `0` on success, `1` for an invalid
    /// name, `2` for an invalid value, and `3` for an operating-system failure.
    EnvSet,
    /// `__env_set_bytes(name, value)`: the same with a byte view instead of a `str`.
    EnvSetBytes,
    /// `__env_remove(name)`: removes a variable. The result is `0` on success, `1` for an invalid name, and `3`
    /// for an operating-system failure.
    EnvRemove,
    /// `__current_dir()`: the raw bytes of the working directory, or `None` when it cannot be read.
    CurrentDir,
    /// `__set_current_dir(path)`: changes the working directory. The result is `0` on success, `1` for an
    /// invalid path, and `3` for an operating-system failure.
    SetCurrentDir,
    /// `__set_current_dir_bytes(path)`: the same with a byte view instead of a `str`.
    SetCurrentDirBytes,
    /// `process.exit(code)`: ends the process.
    ProcessExit,
    /// `assert`, `assert_eq`, `assert_ne`, and their message and `debug_` forms: panics when the assertion
    /// fails (`spec/stdlib.md`, "Memory helpers and assertions").
    Assertion(AssertKind),
    /// `unreachable()`: panics.
    Unreachable,
    /// `process.run(command)`: runs a command through the system shell.
    RunCommand,
    /// `__env_list()`: every environment entry as `NAME=VALUE`, in the order the system reports them.
    EnvList,
    /// `__process_spawn(program, args, env, cwd, command_line, stdin, stdout, stderr)`: starts a child without a
    /// shell. The tuple is `(pid, process, stdin, stdout, stderr)`; `process` is the Windows process handle (0 on
    /// Unix) and each pipe handle is 0 when the stream is not piped. `command_line` is the quoted Windows command
    /// line and is ignored on Unix.
    ProcessSpawn,
    /// `__process_wait(pid, process)`: waits for the child. The result is the exit code, `-1` for a failure to
    /// wait, or `-(signal + 1)` when a signal ended the child.
    ProcessWait,
    /// `__process_close(process)`: releases the Windows process handle. It does nothing on Unix.
    ProcessClose,
    /// `__pipe_read_to_string(handle)`: every byte left in the pipe.
    PipeReadToString,
    /// `__pipe_read_line(handle)`: one line without its terminator, or `None` at the end of the pipe.
    PipeReadLine,
    /// `__pipe_read_both(first, second)`: both pipes read to the end, in one pass, so a child writing a lot to one
    /// of them cannot block the other.
    PipeReadBoth,
    /// `__pipe_write(handle, text)`: writes every byte. The result is `0` on success and `-1` on failure.
    PipeWrite,
    /// `__pipe_close(handle)`: closes a pipe end. It is a no-op for handle 0.
    PipeClose,
    /// `__float_str(v, prec, single)`: the text of a float (see the runtime's `f64_to_string`).
    FloatStr,
    /// `mem.swap(a, b)`: exchanges two places.
    Swap,
    /// `mem.replace(place, value)`: stores a value and returns the old one.
    Replace,
    /// `__read(p)`: moves the value out of the memory `p` points at, leaving the bytes as they were. The caller owns
    /// the result and must not read the memory again until it is written.
    RawRead,
    /// `__write(p, value)`: moves `value` into the memory `p` points at without dropping what was there.
    RawWrite,
    /// `__slice_raw(p, n)`: the `[T]` view of the `n` elements at `p`. It borrows from nothing the checker can see;
    /// `unsafe` is the promise that the memory outlives the view.
    SliceRaw,
    /// `__iter_mut_raw(p, n)`: the `IterMut<T>` over the `n` elements at `p` (`unsafe`).
    IterMutRaw,
    /// `__int_step(x, delta)`: the integer `x` moved by `delta` (1 or -1), wrapping. `std`'s range iterators step
    /// with it, since `T` has no arithmetic of its own.
    IntStep,
    /// `__hash_key(&k)`: the structural hash of the value `k` references, the fold `Map` stores for its keys
    /// (`spec/stdlib.md`, "Map": FNV-1a from the offset basis). `std`'s `Map` hashes through it.
    HashKey,
    /// `target.os()`'s primitive: `"linux"`, `"macos"`, or `"windows"`, the system the program is compiled for.
    TargetOs,
    /// `target.arch()`'s primitive: `"x86_64"` or `"aarch64"`.
    TargetArch,
    /// `f64.from_bits(bits)` and `f32.from_bits(bits)`: the value with those bits (`spec/stdlib.md`, "Float
    /// functions").
    FloatFromBits(FloatTy),
    /// `__view_as_text(view)`: reinterprets a `[u8]` view as a `str` without checking it. `std` uses it after
    /// validating the bytes (`str.from_utf8`); it requires `unsafe` (E0161).
    ViewAsText,
    /// `__bytes_into_string(own bytes)`: adopts an owned `List<u8>` as a `String` without copying or checking it.
    /// `std` uses it after validating the bytes (`String.from_utf8`); it requires `unsafe` (E0161).
    BytesIntoString,
    /// `String.with_capacity(n)`: an empty string whose buffer holds `n` bytes without growing.
    StringWithCapacity,
    /// `String.try_with_capacity(n)`: the fallible `with_capacity` (`spec/allocators.md`, AL11).
    StringTryWithCapacity,
    /// `__closure_own(f)`: the closure `f`, whose borrow captures the compiler tracks, as an owned `FnOnce` value. It
    /// is only for the standard library's `Scope.spawn`, which guarantees the closure ends before the borrowed
    /// places do (`spec/tasks.md`, TK7).
    ClosureOwn,
    /// `black_box(value)`: the same value through a barrier the optimizer cannot see through, so measured work is
    /// not deleted (`spec/bench.md`, BN9). The argument type is any first-class type; the result type is the same.
    BlackBox,
    /// One of the task runtime's primitives (`spec/tasks.md`, TK2): threads, locks, contexts, and the two slots the
    /// scheduler keeps.
    Rt(RtOp),
    /// One of the atomic primitives (`spec/shared.md`, SH7): a sequentially consistent operation on a 32-bit or
    /// 64-bit word that `std.atomic` builds the atomic types from.
    Atomic(AtomicOp, AtomicWidth),
}

/// The width of the word an atomic primitive works on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtomicWidth {
    W32,
    W64,
}

impl AtomicWidth {
    pub fn bits(self) -> u32 {
        match self {
            AtomicWidth::W32 => 32,
            AtomicWidth::W64 => 64,
        }
    }
}

/// An atomic operation. Every one is sequentially consistent (`spec/shared.md`, SH7). Except `Load`, `Store`, `Cas`,
/// and `Fence`, an operation returns the value the word held before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtomicOp {
    /// `__atomic_load_u32(p)`, `__atomic_load_u64(p)`: the word's value.
    Load,
    /// `__atomic_store_u32(p, v)`: writes the word.
    Store,
    /// `__atomic_swap_u32(p, v)`: writes the word and returns the old value.
    Swap,
    /// `__atomic_add_u32(p, v)`: adds with wrap-around and returns the old value.
    Add,
    Sub,
    And,
    Or,
    Xor,
    /// `__atomic_cas_u32(p, expected, new)`: writes `new` when the word equals `expected`; returns the value the
    /// word held, so the exchange happened exactly when it equals `expected`.
    Cas,
    /// `__atomic_fence()`: a sequentially consistent fence.
    Fence,
}

impl AtomicOp {
    /// The primitive's name without the width suffix.
    pub fn name(self) -> &'static str {
        match self {
            AtomicOp::Load => "__atomic_load",
            AtomicOp::Store => "__atomic_store",
            AtomicOp::Swap => "__atomic_swap",
            AtomicOp::Add => "__atomic_add",
            AtomicOp::Sub => "__atomic_sub",
            AtomicOp::And => "__atomic_and",
            AtomicOp::Or => "__atomic_or",
            AtomicOp::Xor => "__atomic_xor",
            AtomicOp::Cas => "__atomic_cas",
            AtomicOp::Fence => "__atomic_fence",
        }
    }

    /// Every operation that works on a word (everything but the fence).
    pub const WORD_OPS: [AtomicOp; 9] = [
        AtomicOp::Load,
        AtomicOp::Store,
        AtomicOp::Swap,
        AtomicOp::Add,
        AtomicOp::Sub,
        AtomicOp::And,
        AtomicOp::Or,
        AtomicOp::Xor,
        AtomicOp::Cas,
    ];

    /// The operation and width an `__atomic_*` primitive name stands for.
    pub fn from_name(name: &str) -> Option<Builtin> {
        if name == "__atomic_fence" {
            return Some(Builtin::Atomic(AtomicOp::Fence, AtomicWidth::W64));
        }
        let (stem, width) = match name.rsplit_once('_')? {
            (stem, "u32") => (stem, AtomicWidth::W32),
            (stem, "u64") => (stem, AtomicWidth::W64),
            _ => return None,
        };
        AtomicOp::WORD_OPS.iter().find(|op| op.name() == stem).map(|op| Builtin::Atomic(*op, width))
    }
}

/// A floating-point function the compiler provides on `f64` and `f32` (`spec/stdlib.md`, "Float functions").
/// The receiver's type picks the precision: an exactly rounded operation lowers to an LLVM intrinsic, and one
/// without an intrinsic calls the C math library. The functions `std` can write in Burdin are extension methods
/// there instead (`min`, `max`, `clamp`, `signum`, and the predicates).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatOp {
    Abs,
    Sqrt,
    Cbrt,
    Powi,
    Powf,
    Exp,
    Exp2,
    Ln,
    Log2,
    Log10,
    Floor,
    Ceil,
    Round,
    RoundTiesEven,
    Trunc,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Sinh,
    Cosh,
    Tanh,
    Hypot,
    MulAdd,
    Copysign,
    /// `to_bits(self)`: the bit pattern as an unsigned integer of the same width.
    ToBits,
}

/// The primitives the task runtime generates (`spec/tasks.md`, TK2). Every one is a plain function of integers and
/// raw pointers, except the two that take an owned `FnOnce(): ()` closure to run elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RtOp {
    /// `__thread_start(f)`: runs `f` on a new detached OS thread.
    ThreadStart,
    /// `__thread_yield()`: gives up the OS thread's time slice.
    ThreadYield,
    /// `__mutex_new() -> CPtr<u8>`, `__mutex_lock(m)`, `__mutex_unlock(m)`, `__mutex_free(m)`.
    MutexNew,
    MutexLock,
    MutexUnlock,
    MutexFree,
    /// `__cond_new() -> CPtr<u8>`, `__cond_wait(c, m)`, `__cond_signal(c)`, `__cond_broadcast(c)`, `__cond_free(c)`.
    CondNew,
    CondWait,
    CondSignal,
    CondBroadcast,
    CondFree,
    /// `__ctx_main() -> CPtr<u8>`: a context that stands for the calling thread's own stack.
    CtxMain,
    /// `__ctx_new(stack_size, f) -> CPtr<u8>`: a context with its own stack that starts by running `f`.
    CtxNew,
    /// `__ctx_switch(from, to)`: saves the running context in `from` and continues `to`.
    CtxSwitch,
    /// `__ctx_free(ctx)`: releases a context that is not running, with its stack.
    CtxFree,
    /// `__tls_get() -> CPtr<u8>`, `__tls_set(p)`: the calling thread's worker slot.
    TlsGet,
    TlsSet,
    /// `__global_get() -> CPtr<u8>`, `__global_set(p)`: the process-wide scheduler slot.
    GlobalGet,
    GlobalSet,
    /// `__spin_lock()`, `__spin_unlock()`: the lock that guards one-time initialization.
    SpinLock,
    SpinUnlock,
    /// `__config_get() -> usize`, `__config_set(n)`: the worker count the program asked for, or 0.
    ConfigGet,
    ConfigSet,
    /// `__cpu_count() -> usize`: the number of processors the program may use.
    CpuCount,
    /// `__io_new() -> CPtr<u8>`: the reactor's operating-system facility (`spec/io.md`, IO2).
    IoNew,
    /// `__io_watch(io, fd, events, wait) -> i32`: registers readiness for `events` (1 read, 2 write) one time.
    IoWatch,
    /// `__io_unwatch(io, fd)`: removes a registration that has not fired.
    IoUnwatch,
    /// `__io_poll(io, timeout_ms) -> CPtr<u8>`: waits; the result is a list of the waiting records that fired,
    /// linked through their first word, each carrying event kind 1 (ready) or 2 (an offloaded call finished).
    IoPoll,
    /// `__io_wake(io)`: makes a pending or future poll return.
    IoWake,
    /// `__io_global_get() -> CPtr<u8>`, `__io_global_set(p)`: the process-wide reactor slot.
    IoGlobalGet,
    IoGlobalSet,
    /// `__pool_submit(io, wait, f)`: runs `f` on a helper thread; the thread queues `wait` as a completion.
    PoolSubmit,
    /// `__pool_submit_raw(io, wait, code, env)`: the same, for the compiler's `blocking` extern calls, which
    /// generate the thunk themselves (`spec/io.md`, IO4).
    PoolSubmitRaw,
    /// The socket primitives (`spec/io.md`, IO5). Success is `>= 0` (bytes, a descriptor, or 0), `-1` is
    /// would-block or an operation in progress, and `-2` is a failure whose system code
    /// `__net_last_error()` reports.
    NetSocket,
    NetBind,
    NetListen,
    NetAccept,
    NetConnect,
    NetSend,
    NetRecv,
    NetSendTo,
    NetRecvFrom,
    NetClose,
    NetShutdown,
    NetError,
    NetSetNoDelay,
    NetLocalAddr,
    NetPeerAddr,
    /// `__net_last_peer() -> u64`: the packed address of the datagram `__net_recv_from` just returned.
    NetLastPeer,
    /// `__net_last_error() -> i32`: the system code of the last failed socket primitive on this thread.
    NetLastError,
}

impl RtOp {
    /// Every primitive, with the name `std` calls it by.
    pub const ALL: [(&'static str, RtOp); 50] = [
        ("__thread_start", RtOp::ThreadStart),
        ("__thread_yield", RtOp::ThreadYield),
        ("__mutex_new", RtOp::MutexNew),
        ("__mutex_lock", RtOp::MutexLock),
        ("__mutex_unlock", RtOp::MutexUnlock),
        ("__mutex_free", RtOp::MutexFree),
        ("__cond_new", RtOp::CondNew),
        ("__cond_wait", RtOp::CondWait),
        ("__cond_signal", RtOp::CondSignal),
        ("__cond_broadcast", RtOp::CondBroadcast),
        ("__cond_free", RtOp::CondFree),
        ("__ctx_main", RtOp::CtxMain),
        ("__ctx_new", RtOp::CtxNew),
        ("__ctx_switch", RtOp::CtxSwitch),
        ("__ctx_free", RtOp::CtxFree),
        ("__tls_get", RtOp::TlsGet),
        ("__tls_set", RtOp::TlsSet),
        ("__global_get", RtOp::GlobalGet),
        ("__global_set", RtOp::GlobalSet),
        ("__spin_lock", RtOp::SpinLock),
        ("__spin_unlock", RtOp::SpinUnlock),
        ("__config_get", RtOp::ConfigGet),
        ("__config_set", RtOp::ConfigSet),
        ("__cpu_count", RtOp::CpuCount),
        ("__io_new", RtOp::IoNew),
        ("__io_watch", RtOp::IoWatch),
        ("__io_unwatch", RtOp::IoUnwatch),
        ("__io_poll", RtOp::IoPoll),
        ("__io_wake", RtOp::IoWake),
        ("__io_global_get", RtOp::IoGlobalGet),
        ("__io_global_set", RtOp::IoGlobalSet),
        ("__pool_submit", RtOp::PoolSubmit),
        ("__pool_submit_raw", RtOp::PoolSubmitRaw),
        ("__net_socket", RtOp::NetSocket),
        ("__net_bind", RtOp::NetBind),
        ("__net_listen", RtOp::NetListen),
        ("__net_accept", RtOp::NetAccept),
        ("__net_connect", RtOp::NetConnect),
        ("__net_send", RtOp::NetSend),
        ("__net_recv", RtOp::NetRecv),
        ("__net_send_to", RtOp::NetSendTo),
        ("__net_recv_from", RtOp::NetRecvFrom),
        ("__net_close", RtOp::NetClose),
        ("__net_shutdown", RtOp::NetShutdown),
        ("__net_error", RtOp::NetError),
        ("__net_set_nodelay", RtOp::NetSetNoDelay),
        ("__net_local_addr", RtOp::NetLocalAddr),
        ("__net_peer_addr", RtOp::NetPeerAddr),
        ("__net_last_peer", RtOp::NetLastPeer),
        ("__net_last_error", RtOp::NetLastError),
    ];

    /// Whether the primitive consumes an owned closure argument.
    pub fn takes_closure(self) -> bool {
        matches!(self, RtOp::ThreadStart | RtOp::CtxNew | RtOp::PoolSubmit)
    }
}

/// Built-in methods of the primitive types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    ToString,
    WrappingAdd,
    WrappingSub,
    WrappingMul,
    CheckedAdd,
    CheckedSub,
    CheckedMul,
    Min,
    Max,
    Abs,
    /// An `f64` or `f32` function (`spec/stdlib.md`, "Float functions"); the receiver's type picks the
    /// precision.
    Float(FloatOp),
    /// `String.push_str(str)`: appends bytes, growing the buffer.
    PushStr,
    /// `String.push(char)`: appends one UTF-8 character.
    PushChar,
    /// `List<T>.push(value)`: appends an element.
    Push,
    /// `List<T>.pop(): Option<T>`: removes and returns the last element.
    ListPop,
    /// `List<T>.last(): Option<&T>`: a view of the last element.
    ListLast,
    /// `List<T>.first(): Option<&T>`: a view of the first element.
    ListFirst,
    /// `List<T>.first_mut(): Option<&mut T>`: a mutable view of the first element.
    ListFirstMut,
    /// `List<T>.last_mut(): Option<&mut T>`: a mutable view of the last element.
    ListLastMut,
    /// `List<T>.get(index): Option<&T>`: a view of one element, or `None` when the index is out of bounds.
    ListGet,
    /// `List<T>.get_mut(index): Option<&mut T>`: a mutable view of one element, or `None`.
    ListGetMut,
    /// `List<T>.capacity(): usize`: how many elements fit without growing.
    ListCapacity,
    /// `List<T>.is_empty(): bool`: whether the list has no elements.
    ListIsEmpty,
    /// `List<T>.reserve(additional)`: makes room for `additional` more elements.
    ListReserve,
    /// `List<T>.reserve_exact(additional)`: makes room for exactly `additional` more elements.
    ListReserveExact,
    /// `List<T>.try_reserve(additional): Result<(), AllocError>`: `reserve` that fails instead of panicking.
    ListTryReserve,
    /// `List<T>.try_reserve_exact(additional): Result<(), AllocError>`: the fallible `reserve_exact`.
    ListTryReserveExact,
    /// `List<T>.shrink_to_fit()`: releases the capacity past the length.
    ListShrinkToFit,
    /// `List<T>.swap(a, b)`: exchanges two elements.
    ListSwap,
    /// `List<T>.swap_remove(index): T`: removes one element by moving the last element into its place.
    ListSwapRemove,
    /// `List<T>.truncate(len)`: drops the elements from `len` on.
    ListTruncate,
    /// `List<T>.split_off(index): List<T>`: moves the tail into a new list with the default allocator.
    ListSplitOff,
    /// `List<T>.append(other)`: moves every element of `other` to the end, then frees `other`'s buffer.
    ListAppend,
    /// `List<T>.drain(range): ListIntoIter<T>`: removes a range and yields the removed elements as an owning
    /// iterator over a new buffer with the default allocator.
    ListDrain,
    /// `List<T>.extend_from_slice(other)`: appends a clone of every element of a slice view.
    ListExtendFromSlice,
    /// `List<T>.insert(index, value)`: inserts at `index`.
    ListInsert,
    /// `List<T>.remove(index): T`: removes and returns the element at `index`.
    ListRemove,
    /// `List<T>.contains(value): bool`.
    ListContains,
    /// `List<T>.sort()`: orders the elements with `Ord`.
    ListSort,
    /// `List<T>.clear()` and `Map<K, V>.clear()`: drops the elements and leaves the collection empty.
    Clear,
    /// `Map<K, V>.insert(key, value)`: adds or replaces a value.
    MapInsert,
    /// `Map<K, V>.get(key): Option<&V>`: a view of the value.
    MapGet,
    /// `Map<K, V>.get_mut(key): Option<&mut V>`: a mutable view of the value.
    MapGetMut,
    /// `Map<K, V>.get_or_insert(key, value): &mut V`: the value stored under `key`, inserting `value` first when
    /// the key is absent. An existing value is kept and the incoming key and value are dropped.
    MapGetOrInsert,
    /// `Map<K, V>.get_or(key, default): &V`: the read-only form of `get_or_insert`; an existing value is kept and
    /// the incoming key and default are dropped.
    MapGetOr,
    /// `Map<K, V>.remove_entry(key): Option<(K, V)>`: the stored key and value, moved out.
    MapRemoveEntry,
    /// `Map<K, V>.retain(f)`: keeps the entries `f` accepts and drops the rest.
    MapRetain,
    /// `Map<K, V>.reserve(additional)`: grows the slot table so it can hold `additional` more entries.
    MapReserve,
    /// `Map<K, V>.try_reserve(additional): Result<(), AllocError>`: `reserve` that fails instead of panicking.
    MapTryReserve,
    /// `Map<K, V>.shrink_to_fit()`: shrinks the slot table to the smallest one that holds the entries.
    MapShrinkToFit,
    /// `Map<K, V>.remove(key): Option<V>`.
    MapRemove,
    /// `Map<K, V>.contains_key(key): bool`.
    MapContainsKey,
    /// `str.find_str(needle): Option<usize>`: the byte index of a substring.
    FindStr,
    /// `String.as_str()`: a `str` view of the bytes.
    AsStr,
    /// `str.as_bytes()` and `String.as_bytes()`: the UTF-8 bytes as a `[u8]` view.
    AsBytes,
    /// `String.capacity(): usize`: the buffer's capacity in bytes.
    StringCapacity,
    /// `String.reserve(additional)`: grows the buffer to hold `additional` more bytes (`spec/stdlib.md`, "String
    /// capacity and editing").
    StringReserve,
    /// `String.try_reserve(additional): Result<(), AllocError>`: `reserve` that fails instead of panicking.
    StringTryReserve,
    /// `String.shrink_to_fit()`: shrinks the buffer to the current length.
    StringShrinkToFit,
    /// `String.insert_str(byte_idx, text)`: inserts text at a character boundary, shifting the tail.
    StringInsertStr,
    /// `String.replace_range(from, to, text)`: replaces the bytes `from..to`, both character boundaries, with text.
    StringReplaceRange,
    /// `x.len()` on `str`, `[T]`, `String`, and arrays.
    Len,
    /// `s.starts_with(prefix)` on `str` (and `String`).
    StartsWith,
    /// `s.find(c): Option<usize>` on `str` (and `String`); a byte index.
    Find,
    /// `CPtr<T>.is_null(): bool`: whether the raw pointer is null.
    IsNull,
    /// `xs.iter()`: the borrowing iterator of a collection, a range, or `Chars` (IT4).
    Iter,
    /// `xs.iter_mut()`: the mutable iterator of a `List`, an array, or a `Map` (IT15). It yields `&mut T` and borrows
    /// the collection exclusively.
    IterMut,
    /// `xs.into_iter()`: the owning iterator of a `List`, a `Map`, an array, a range, or `Chars` (IT5).
    IntoIter,
    /// `it.next()`: advances an iterator and returns its next item (IT6).
    IterNext,
    /// `it.next_back()`: takes the last item of a double-ended built-in iterator (IT14).
    IterNextBack,
    /// `x.clone()`: a deep copy derived by the compiler.
    Clone,
    /// `x.hash(): u64` of a type that derives `Hash`.
    Hash,
    /// `x.hash_from(state): u64`: folds `x` into the running hash `state` (`spec/stdlib.md`, HS3).
    HashFrom,
    /// `List<T>.as_slice(): [T]`: a view of the list's elements (`spec/stdlib.md`, "Slices").
    AsSlice,
    /// `[T].get(i): Option<&T>`: a view of the element at `i`, or `None` when it is out of bounds.
    SliceGet,
    /// `[T].last(): Option<&T>`: a view of the last element.
    SliceLast,
    /// `[T].to_list(): List<T>`: an owned list holding a clone of each element.
    SliceToList,
    /// `[T].get(range): Option<[T]>`: a view of the sub-slice, or `None` when the range is out of bounds or
    /// reversed. The argument is a `Range<usize>` or `RangeInclusive<usize>`.
    SliceGetRange,
    /// `[T].swap(a, b)`: exchanges two elements in place.
    SliceSwap,
    /// `[T].fill(value)`: overwrites every element with a clone of `value`.
    SliceFill,
    /// `[T].copy_from_slice(other)`: copies every element of `other` into the receiver.
    SliceCopyFromSlice,
    /// `[T].clone_from_slice(other)`: clones every element of `other` into the receiver.
    SliceCloneFromSlice,
    /// `[T].chunks(n)` and `[T].chunks_exact(n)`: the iterator over fixed-size windows of the slice.
    SliceChunks,
    /// `[T].windows(n)`: the iterator over the overlapping windows of the slice.
    SliceWindows,
    /// `[T].__sort_by(less)`: sorts the slice in place with a comparator that says whether its first argument is
    /// strictly less. Insertion sort, so the sort is stable. `std`'s `sort_by`/`sort`/`sort_by_key` call it.
    SliceSortBy,
}

impl Method {
    /// Whether the method takes ownership of argument `index` (the receiver is argument 0).
    pub fn owns_argument(self, index: usize) -> bool {
        match self {
            Method::Push => index == 1,
            Method::ListInsert => index == 2,
            Method::ListAppend => index == 1,
            Method::MapInsert | Method::MapGetOrInsert | Method::MapGetOr => index == 1 || index == 2,
            // `retain` consumes the closure it is given.
            Method::MapRetain => index == 1,
            // The slice sort consumes its comparator.
            Method::SliceSortBy => index == 1,
            // `into_iter` takes the collection, the array, the map, or the range by value.
            Method::IntoIter => index == 0,
            _ => false,
        }
    }

    /// Whether the method changes its receiver, so the call borrows it exclusively.
    pub fn mutates_receiver(self) -> bool {
        matches!(
            self,
            Method::PushStr
                | Method::PushChar
                | Method::StringReserve
                | Method::StringTryReserve
                | Method::StringShrinkToFit
                | Method::StringInsertStr
                | Method::StringReplaceRange
                | Method::Push
                | Method::ListPop
                | Method::ListInsert
                | Method::ListRemove
                | Method::ListSort
                | Method::ListReserve
                | Method::ListReserveExact
                | Method::ListTryReserve
                | Method::ListTryReserveExact
                | Method::ListShrinkToFit
                | Method::ListSwap
                | Method::ListSwapRemove
                | Method::ListTruncate
                | Method::ListSplitOff
                | Method::ListAppend
                | Method::ListDrain
                | Method::ListExtendFromSlice
                | Method::ListGetMut
                | Method::ListFirstMut
                | Method::ListLastMut
                | Method::Clear
                | Method::MapInsert
                | Method::MapGetOrInsert
                | Method::MapGetOr
                | Method::MapGetMut
                | Method::MapRemove
                | Method::MapRemoveEntry
                | Method::MapRetain
                | Method::MapReserve
                | Method::MapTryReserve
                | Method::MapShrinkToFit
                | Method::IterMut
                | Method::IterNext
                | Method::IterNextBack
                | Method::SliceSwap
                | Method::SliceFill
                | Method::SliceCopyFromSlice
                | Method::SliceCloneFromSlice
                | Method::SliceSortBy
        )
    }

    /// Whether the result is a view derived from the receiver.
    pub fn result_borrows_receiver(self) -> bool {
        matches!(
            self,
            Method::ListLast
                | Method::ListFirst
                | Method::ListFirstMut
                | Method::ListLastMut
                | Method::ListGet
                | Method::ListGetMut
                | Method::MapGet
                | Method::MapGetMut
                | Method::MapGetOrInsert
                | Method::MapGetOr
                | Method::AsBytes
                | Method::Iter
                | Method::IterMut
                | Method::IterNext
                | Method::IterNextBack
                | Method::AsSlice
                | Method::SliceGet
                | Method::SliceLast
                | Method::SliceGetRange
                | Method::SliceChunks
                | Method::SliceWindows
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assertion_kinds_describe_their_shape() {
        for kind in [AssertKind::DebugAssert, AssertKind::DebugAssertEq, AssertKind::DebugAssertNe] {
            assert!(kind.is_debug(), "{kind:?}");
        }
        for kind in [AssertKind::Assert, AssertKind::Eq, AssertKind::Ne] {
            assert!(!kind.is_debug(), "{kind:?}");
        }
        assert!(!AssertKind::Assert.compares_values());
        assert!(!AssertKind::DebugAssert.compares_values());
        for kind in [AssertKind::Eq, AssertKind::Ne, AssertKind::DebugAssertEq, AssertKind::DebugAssertNe] {
            assert!(kind.compares_values(), "{kind:?}");
        }
        for kind in [AssertKind::Ne, AssertKind::DebugAssertNe] {
            assert!(kind.expects_difference(), "{kind:?}");
        }
        for kind in [AssertKind::Assert, AssertKind::Eq, AssertKind::DebugAssert, AssertKind::DebugAssertEq] {
            assert!(!kind.expects_difference(), "{kind:?}");
        }
    }

    #[test]
    fn only_value_parameters_are_owned() {
        assert!(Method::Push.owns_argument(1));
        assert!(!Method::Push.owns_argument(0));
        assert!(Method::ListInsert.owns_argument(2));
        assert!(Method::MapInsert.owns_argument(1));
        assert!(Method::MapInsert.owns_argument(2));
        assert!(Method::MapGetOrInsert.owns_argument(1));
        assert!(Method::MapGetOrInsert.owns_argument(2));
        assert!(Method::MapGetOr.owns_argument(1));
        assert!(Method::MapGetOr.owns_argument(2));
        assert!(Method::MapRetain.owns_argument(1));
        assert!(!Method::MapRemoveEntry.owns_argument(1));
        // `append` takes the other list by ownership; the capacity operations take only a count.
        assert!(Method::ListAppend.owns_argument(1));
        assert!(!Method::ListAppend.owns_argument(0));
        for method in [
            Method::ListReserve,
            Method::ListReserveExact,
            Method::ListTryReserve,
            Method::ListTryReserveExact,
            Method::ListTruncate,
            Method::ListDrain,
        ] {
            assert!(!method.owns_argument(0), "{method:?}");
            assert!(!method.owns_argument(1), "{method:?}");
        }
        assert!(!Method::Len.owns_argument(0));
        // The lookup methods borrow their key (`contains(self, value: T)`, `get(self, key: K)`).
        assert!(!Method::ListContains.owns_argument(1));
        assert!(!Method::MapGet.owns_argument(1));
        assert!(!Method::MapGetMut.owns_argument(1));
        assert!(!Method::MapRemove.owns_argument(1));
        assert!(!Method::MapContainsKey.owns_argument(1));
    }

    #[test]
    fn mutation_and_borrowed_results_follow_the_method() {
        for method in [
            Method::PushStr,
            Method::PushChar,
            Method::StringReserve,
            Method::StringTryReserve,
            Method::StringShrinkToFit,
            Method::StringInsertStr,
            Method::StringReplaceRange,
            Method::Push,
            Method::ListPop,
            Method::ListInsert,
            Method::ListRemove,
            Method::ListSort,
            Method::ListReserve,
            Method::ListReserveExact,
            Method::ListTryReserve,
            Method::ListTryReserveExact,
            Method::ListShrinkToFit,
            Method::ListSwap,
            Method::ListSwapRemove,
            Method::ListTruncate,
            Method::ListSplitOff,
            Method::ListAppend,
            Method::ListDrain,
            Method::ListExtendFromSlice,
            Method::ListGetMut,
            Method::ListFirstMut,
            Method::ListLastMut,
            Method::Clear,
            Method::MapInsert,
            Method::MapGetOrInsert,
            Method::MapGetOr,
            Method::MapGetMut,
            Method::MapRemove,
            Method::MapRemoveEntry,
            Method::MapRetain,
            Method::MapReserve,
            Method::MapTryReserve,
            Method::MapShrinkToFit,
            Method::IterMut,
        ] {
            assert!(method.mutates_receiver(), "{method:?}");
        }
        for method in [Method::ToString, Method::Len, Method::Clone, Method::ListLast, Method::MapGet, Method::AsStr] {
            assert!(!method.mutates_receiver(), "{method:?}");
        }
        // The capacity and length queries only read the list and hand out no view.
        for method in [Method::ListCapacity, Method::ListIsEmpty, Method::ListFirst, Method::ListGet] {
            assert!(!method.mutates_receiver(), "{method:?}");
        }
        for method in [Method::ListCapacity, Method::ListIsEmpty] {
            assert!(!method.result_borrows_receiver(), "{method:?}");
        }
        for method in [Method::ListFirst, Method::ListGet] {
            assert!(method.result_borrows_receiver(), "{method:?}");
        }
        for method in [
            Method::ListLast,
            Method::ListFirst,
            Method::ListFirstMut,
            Method::ListLastMut,
            Method::ListGet,
            Method::ListGetMut,
            Method::MapGet,
            Method::IterMut,
        ] {
            assert!(method.result_borrows_receiver(), "{method:?}");
        }
        for method in [Method::Push, Method::ListPop, Method::ListContains, Method::Len] {
            assert!(!method.result_borrows_receiver(), "{method:?}");
        }
        // `get_mut` and the `*_mut` accessors need a mutable receiver and hand out a view of it.
        for method in [Method::ListGetMut, Method::ListFirstMut, Method::ListLastMut] {
            assert!(method.mutates_receiver(), "{method:?}");
            assert!(method.result_borrows_receiver(), "{method:?}");
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}
