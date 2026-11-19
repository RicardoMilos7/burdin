//! The runtime, generated as LLVM IR into every program: the process entry point (`main`), output, `String`
//! allocation and conversions, `Heap` allocation, and the panic path. It calls libc directly: `write`, `malloc`,
//! `realloc`, `free`, and `abort`, and on Windows the CRT's `_write`, `_setmode`, and `_set_abort_behavior`.
//!
//! **Output.** Standard output is block buffered in an 8 KiB buffer (`burdin_rt_out_write`), and flushed through the
//! last newline of a write when fd 1 is a terminal (line buffering, `isatty`). Standard error stays unbuffered. The
//! buffer is flushed by `burdin_rt_flush`, which is registered with `atexit`, called by `process.exit` through it,
//! and called by the panic path, the stack-overflow handler, and `burdin_rt_die` before the process ends. The
//! `stdout`/`stderr` handles of `std/io.bd` call `burdin_rt_print` too, so they share these rules.
//!
//! **Input.** Standard input is block buffered in a 4 KiB buffer (`burdin_rt_in_fill`). `burdin_rt_in_read_line`
//! and `burdin_rt_in_read_to_string` consume it, so a line longer than one block and a block that holds more than
//! the caller asked for are both handled. `burdin_rt_is_terminal` answers `isatty` for the stream handles.
//!
//! **Panics.** A panic prints `path:LINE:COL: panic: message` and then a symbolized `stack backtrace:` section, so
//! the message line keeps its stage 0 format. Every panic path first calls `burdin_panic_begin`, the stable,
//! externally visible breakpoint symbol (`spec/debug.md`, DBG1), with the stack still intact. The runtime frames
//! above the panicking function are skipped. On Unix the trace uses libc's `backtrace`/`backtrace_symbols_fd` and
//! needs the `.eh_frame` unwind tables the code generator adds to every function; on Windows it uses
//! `RtlCaptureStackBackTrace`. When the program was built with `-g`, the code generator also defines
//! `burdin_rt_frame_info` over a table of one entry per emitted function (`spec/debug.md`, DBG2), and the backtrace
//! prints `  #N  at <file>:<line> in <Burdin path>` for every frame the table names and `  #N  0x<address>` for the
//! rest; without it the frames print as `backtrace_symbols_fd` does on Unix and as addresses on Windows.
//!
//! **Stack overflow.** The main thread gets an alternate signal stack and a `SIGSEGV` handler on Unix (an
//! `AddVectoredExceptionHandler` filter for `EXCEPTION_STACK_OVERFLOW` on Windows, with a 64 KiB stack guarantee so
//! the filter always has room to flush and report), so a deep recursion ends with `panic: stack overflow` instead
//! of a raw fault.
//!
//! The functions are internal to the module and keep the names they had as C functions (`burdin_rt_*`).

use crate::emit::{GenResult, llvm_err};
use inkwell::attributes::{Attribute, AttributeLoc};
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::types::{BasicMetadataTypeEnum, FunctionType, IntType, PointerType, StructType};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue, PointerValue};
use inkwell::{AddressSpace, AtomicOrdering, IntPredicate};

/// The standard-output buffer size. A terminal is line buffered, so the size only matters for pipes and files,
/// where larger writes are what make printing a lot of output cheap.
const OUT_BUF: u32 = 8192;
/// The standard-input buffer size. Reads take whole blocks, so a program that reads line by line does not call
/// the operating system once per line.
const IN_BUF: u32 = 4096;
/// The number of frames a panic backtrace captures.
const BACKTRACE_FRAMES: u32 = 64;
/// How long a calibration batch must run before its timing decides the iterations per sample (`define_bench_main`):
/// a millisecond is far above the clock's granularity, so a fast bench is not estimated as one clock tick per call.
const CALIBRATION_NS: u64 = 1_000_000;
/// The calibration batch stops growing here; a bench whose batch never reaches [`CALIBRATION_NS`] measures one
/// iteration per sample instead of looping forever.
const BATCH_MAX: u64 = 1 << 30;
/// The size of the alternate signal stack, so a `SIGSEGV` handler still runs after the main stack is gone.
const ALT_STACK: u32 = 64 * 1024;
/// The stack guarantee the Windows guard reserves, so the vectored filter still has room after the main stack is
/// gone. The system's default leaves only a few kilobytes once the exception dispatcher has run, which is marginal
/// for the flush and the write; the handler could fault and end the process without its message.
const STACK_GUARANTEE: u32 = 64 * 1024;
/// The checking allocator's header: `{ next, size, site, site_len, magic }`.
const HEADER: u64 = 40;
/// The magic word that marks a live checking-allocator block.
const MAGIC: u64 = 0x4255_5244_4e41_4c4c;
/// The magic word of a block that was taken off the live list on purpose (`untrack`): it is still freed normally, but
/// a leak report does not mention it.
const UNTRACKED_MAGIC: u64 = MAGIC + 1;

/// The offset of `d_name` in a `struct dirent`, read from the host's own C layout.
#[cfg(unix)]
const DIRENT_NAME_OFFSET: u64 = std::mem::offset_of!(libc::dirent, d_name) as u64;
/// Windows has no `dirent`; its directory listing reads `WIN32_FIND_DATAA` instead.
#[cfg(windows)]
const DIRENT_NAME_OFFSET: u64 = 0;
/// The size of a `struct stat`, rounded up to whole 8-byte words so the buffer is 8-byte aligned.
const STAT_WORDS: u32 = (std::mem::size_of::<libc::stat>() as u32).div_ceil(8);
/// The fields the runtime reads from `struct stat`, as the host's C layout puts them.
const STAT_SIZE_OFFSET: u64 = std::mem::offset_of!(libc::stat, st_size) as u64;
const STAT_MODE_OFFSET: u64 = std::mem::offset_of!(libc::stat, st_mode) as u64;
const STAT_ATIME_OFFSET: u64 = std::mem::offset_of!(libc::stat, st_atime) as u64;
const STAT_MTIME_OFFSET: u64 = std::mem::offset_of!(libc::stat, st_mtime) as u64;
/// `st_mode` is 16 bits on macOS and Windows, and 32 on Linux.
#[cfg(any(target_os = "macos", windows))]
const STAT_MODE_BYTES: u32 = 2;
#[cfg(not(any(target_os = "macos", windows)))]
const STAT_MODE_BYTES: u32 = 4;
/// The Windows `WIN32_FIND_DATAA` layout: `dwFileAttributes` first and `cFileName` after the fixed fields.
const FIND_DATA_FILE_NAME_OFFSET: u64 = 44;
/// `sockaddr_in` is 16 bytes on every supported platform. The address bytes start at offset 4; the family's
/// offset differs because Darwin's structure begins with `sin_len`.
const SOCKADDR_PORT_OFFSET: u64 = 2;
const SOCKADDR_ADDR_OFFSET: u64 = 4;
const SOCKADDR_SIZE: u64 = 16;
/// The fields of `struct addrinfo` the resolver reads. The Boost/Winsock `ADDRINFOA` has the same 48-byte layout
/// on 64-bit Windows with 32-bit `int` fields; the Unix structure's offsets come from the host header.
#[cfg(unix)]
const ADDRINFO_FAMILY_OFFSET: u64 = std::mem::offset_of!(libc::addrinfo, ai_family) as u64;
#[cfg(windows)]
const ADDRINFO_FAMILY_OFFSET: u64 = 4;
#[cfg(unix)]
const ADDRINFO_ADDR_OFFSET: u64 = std::mem::offset_of!(libc::addrinfo, ai_addr) as u64;
#[cfg(windows)]
const ADDRINFO_ADDR_OFFSET: u64 = 32;
#[cfg(unix)]
const ADDRINFO_NEXT_OFFSET: u64 = std::mem::offset_of!(libc::addrinfo, ai_next) as u64;
#[cfg(windows)]
const ADDRINFO_NEXT_OFFSET: u64 = 40;
#[cfg(unix)]
const ADDRINFO_SIZE: u64 = std::mem::size_of::<libc::addrinfo>() as u64;
#[cfg(windows)]
const ADDRINFO_SIZE: u64 = 48;
/// `addrinfo.ai_flags`, at the start of the structure on both platforms.
const ADDRINFO_FLAGS_OFFSET: u64 = 0;
/// `AI_PASSIVE`: resolve a wildcard address for `bind`.
const AI_PASSIVE: i32 = 1;
const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;
#[cfg(unix)]
const SOL_SOCKET: i32 = libc::SOL_SOCKET;
#[cfg(windows)]
const SOL_SOCKET: i32 = 0xffff;
#[cfg(unix)]
const SO_ERROR: i32 = libc::SO_ERROR;
#[cfg(windows)]
const SO_ERROR: i32 = 0x1007;
#[cfg(unix)]
const SO_REUSEADDR: i32 = libc::SO_REUSEADDR;
#[cfg(windows)]
const SO_REUSEADDR: i32 = 0x0004;
const IPPROTO_TCP: i32 = 6;
#[cfg(unix)]
const TCP_NODELAY: i32 = libc::TCP_NODELAY;
#[cfg(windows)]
const TCP_NODELAY: i32 = 0x0001;
/// `ioctlsocket`'s "set non-blocking" request. Only Windows uses it; the value is defined everywhere so the
/// same function body can name it.
const FIONBIO: i32 = 0x8004_667e_u32 as i32;
/// `fcntl`'s "get the file status flags" and "set them", and the non-blocking bit among them. `libc` exports
/// these on Unix only, so a build on Windows (where the socket path uses `ioctlsocket`) still gets a value for the
/// branch it never runs.
#[cfg(unix)]
const F_GETFL: i32 = libc::F_GETFL;
#[cfg(unix)]
const F_SETFL: i32 = libc::F_SETFL;
#[cfg(unix)]
const O_NONBLOCK: i32 = libc::O_NONBLOCK;
#[cfg(windows)]
const F_GETFL: i32 = 3;
#[cfg(windows)]
const F_SETFL: i32 = 4;
#[cfg(windows)]
const O_NONBLOCK: i32 = 0x800;
/// The `errno` values that mean "try again": a non-blocking call would block, a connect is still in progress,
/// or a signal interrupted the call.
#[cfg(unix)]
const RETRY_CODES: [i32; 4] = [libc::EAGAIN, libc::EWOULDBLOCK, libc::EINPROGRESS, libc::EINTR];
/// The Winsock equivalents, reported by `WSAGetLastError`.
#[cfg(windows)]
const RETRY_CODES: [i32; 5] = [10035, 10036, 10004, 10037, 10055];
/// `WIN32_FIND_DATAA` is 320 bytes with its 4-byte alignment.
const FIND_DATA_WORDS: u32 = 40;
/// `FILE_ATTRIBUTE_REPARSE_POINT`, the bit that marks a link or junction on Windows.
const FILE_ATTRIBUTE_REPARSE_POINT: u64 = 0x400;
/// `INVALID_FILE_ATTRIBUTES`, what `GetFileAttributesA` returns on failure.
const INVALID_FILE_ATTRIBUTES: u64 = 0xFFFF_FFFF;
/// `MOVEFILE_REPLACE_EXISTING`: `MoveFileExA` replaces an existing destination, as POSIX `rename` does.
const MOVEFILE_REPLACE_EXISTING: u64 = 1;
/// `ERROR_GEN_FAILURE`: what the runtime reports when a failing Windows call sets neither `_doserrno` nor
/// `GetLastError`.
const ERROR_GEN_FAILURE: u32 = 31;
/// `ERROR_NO_MORE_FILES`: `FindNextFileA`'s normal end of a listing, not a failure.
const ERROR_NO_MORE_FILES: u32 = 18;
/// The `S_IFMT` mask and the file, directory, and link kinds it selects (`_S_IFMT` and the `_S_IF*` values on
/// Windows agree with the POSIX ones for these three kinds).
const S_IFMT: u64 = 0o170000;
const S_IFREG: u64 = 0o100000;
const S_IFDIR: u64 = 0o040000;
const S_IFLNK: u64 = 0o120000;

/// The `open` flags a file handle uses. Linux, macOS, and the Windows CRT disagree on every value except the
/// access mode, so the runtime picks the set for the system it generates for. `binary` is `O_BINARY` on Windows
/// (a file is opened without newline translation) and zero elsewhere, where there is no such flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OpenFlagConstants {
    read_only: u32,
    write_only: u32,
    read_write: u32,
    append: u32,
    truncate: u32,
    create: u32,
    exclusive: u32,
    binary: u32,
}

fn open_flag_constants(windows: bool, darwin: bool) -> OpenFlagConstants {
    if windows {
        OpenFlagConstants {
            read_only: 0x0000,
            write_only: 0x0001,
            read_write: 0x0002,
            append: 0x0008,
            truncate: 0x0200,
            create: 0x0100,
            exclusive: 0x0400,
            binary: 0x8000,
        }
    } else if darwin {
        OpenFlagConstants {
            read_only: 0,
            write_only: 1,
            read_write: 2,
            append: 0x0008,
            truncate: 0x0400,
            create: 0x0200,
            exclusive: 0x0800,
            binary: 0,
        }
    } else {
        OpenFlagConstants {
            read_only: 0,
            write_only: 1,
            read_write: 2,
            append: 0o2000,
            truncate: 0o1000,
            create: 0o100,
            exclusive: 0o200,
            binary: 0,
        }
    }
}

/// The mode `open` gives a file it creates: the usual 0666 (the umask applies), or the CRT's two permission bits
/// on Windows, where every other bit is ignored.
fn open_create_mode(windows: bool) -> u64 {
    if windows { 0o600 } else { 0o666 }
}

/// The runtime functions the generated code calls.
pub(crate) struct Runtime<'ctx> {
    /// `(fd: i32, ptr, len: usize, newline: i32)`
    pub print: FunctionValue<'ctx>,
    /// `()`: writes the standard-output buffer to fd 1 and empties it.
    pub flush: FunctionValue<'ctx>,
    /// `(fd: i32) -> i1`: whether the descriptor is a terminal.
    pub is_terminal: FunctionValue<'ctx>,
    /// `(out: *String, err: *i32) -> i32`: reads one line of standard input; 0 is a line (or the last one), 1
    /// is the end of input with nothing read, and -1 is an error, whose OS code is stored in `err`.
    pub in_read_line: FunctionValue<'ctx>,
    /// `(out: *String, err: *i32) -> i32`: reads the rest of standard input; 0 is success and -1 an error,
    /// whose OS code is stored in `err`.
    pub in_read_to_string: FunctionValue<'ctx>,
    /// `() -> isize`: how many standard-input bytes are buffered, reading a block when none are; 0 is the end of
    /// the input and -1 a read error, whose OS code `in_error` then reports.
    pub in_avail: FunctionValue<'ctx>,
    /// `() -> i32`: the OS code of the last standard-input read error.
    pub in_error: FunctionValue<'ctx>,
    /// `() -> i8`: takes the next buffered standard-input byte.
    pub in_next: FunctionValue<'ctx>,
    /// `(msg, msg_len: usize, file, file_len: usize, line: i32, col: i32)`, never returns.
    pub panic: FunctionValue<'ctx>,
    /// `(index: i64, len: i64, file, file_len: usize, line: i32, col: i32)`, never returns.
    pub panic_bounds: FunctionValue<'ctx>,
    /// `(ptr, cap: usize)`
    pub string_free: FunctionValue<'ctx>,
    /// `(out: *String)`: an empty `String`.
    pub string_new: FunctionValue<'ctx>,
    /// `(dst: *String, src: *String)`: a copy of `src`'s buffer.
    pub string_clone: FunctionValue<'ctx>,
    /// `(s: *String, ptr, len: usize)`: appends `len` bytes, growing the buffer when needed.
    pub string_push_str: FunctionValue<'ctx>,
    /// `(size: usize, site, site_len: usize)`: an allocation of at least one byte that panics on failure. In a
    /// test build it is the checking allocator, which records the source location for a leak report.
    pub alloc: FunctionValue<'ctx>,
    /// `(ptr)`: frees an allocation made by `alloc`.
    pub free: FunctionValue<'ctx>,
    /// `() -> i32`: reports the allocations still live in a test build, and returns 1 when there are any. It is
    /// only present when the checking allocator is in use (`spec/allocators.md`, AL9).
    pub leak_check: Option<FunctionValue<'ctx>>,
    /// `(ptr)`: takes a live block off the checking allocator's list, for state that lasts as long as the process
    /// (the environment of a scheduler worker thread). Only present with the checking allocator.
    pub untrack: Option<FunctionValue<'ctx>>,
    /// `(ptr, len: usize, out: *String)`
    pub str_to_string: FunctionValue<'ctx>,
    /// `(v: i64, out: *String)`
    pub i64_to_string: FunctionValue<'ctx>,
    /// `(v: f64, prec: i32, single: bool, out: *String)`: the text of a float, fixed with `prec` digits after the
    /// point, or (`prec < 0`) the shortest scientific text that reads back as the same number.
    pub f64_to_string: FunctionValue<'ctx>,
    /// `(v: i64, out: *String)`
    pub u64_to_string: FunctionValue<'ctx>,
    /// `(c: i32, out: *String)`
    pub char_to_string: FunctionValue<'ctx>,
    /// `(ptr, len: usize, prefix, prefix_len: usize) -> i1`: whether the string starts with `prefix`.
    pub str_starts_with: FunctionValue<'ctx>,
    /// `(ptr, len: usize, c: i32) -> i64`: the byte index of the first `c`, or -1.
    pub str_find_char: FunctionValue<'ctx>,
    /// `(ptr, len: usize) -> ptr`: a NUL-terminated copy of the bytes in the C heap.
    pub cstring_from: FunctionValue<'ctx>,
    /// `(ptr) -> usize`: the length of a NUL-terminated C string (libc `strlen`).
    pub strlen: FunctionValue<'ctx>,
    /// `(ptr) -> ptr`: a fresh copy of a NUL-terminated C string, for `CString.clone`.
    pub cstring_clone: FunctionValue<'ctx>,
    /// `(ptr, bytes: usize, site, site_len: usize) -> ptr`: resizes an allocation, panicking when it fails.
    pub realloc: FunctionValue<'ctx>,
    /// `(s: *String, c: i32)`: appends one UTF-8 code point.
    pub string_push_char: FunctionValue<'ctx>,
    /// `(ptr, len, ptr2, len2) -> i1`: byte equality.
    pub str_eq: FunctionValue<'ctx>,
    /// `(ptr, len, ptr2, len2) -> i32`: -1, 0, or 1.
    pub str_cmp: FunctionValue<'ctx>,
    /// `(acc: i64, ptr, len: usize) -> i64`: FNV-1a over `len` bytes, continuing from `acc`.
    pub str_hash: FunctionValue<'ctx>,
    /// `(ptr, len, needle, needle_len) -> i64`: the first byte index, or -1.
    pub str_find_str: FunctionValue<'ctx>,
    /// `(ptr, len, out: *i64) -> i1`: parses a signed decimal integer.
    pub parse_i64: FunctionValue<'ctx>,
    /// `(ptr, len, out: *i64) -> i1`: parses an unsigned decimal integer.
    pub parse_u64: FunctionValue<'ctx>,
    /// `(ptr, len, out: *String) -> i32`: reads a whole file; 0, or the OS error code.
    pub read_to_string: FunctionValue<'ctx>,
    /// `(path, path_len, text, text_len) -> i32`: writes a whole file; 0, or the OS error code.
    pub write_file: FunctionValue<'ctx>,
    /// `(path, path_len, read, write, append, truncate, create, create_new) -> i32`: a file descriptor, or -1.
    pub file_open: FunctionValue<'ctx>,
    /// `(fd: i32, out: *List<u8>, max: usize) -> isize`: appends up to `max` bytes and returns the count, or -1.
    pub file_read: FunctionValue<'ctx>,
    /// `(fd: i32, data, len: usize) -> isize`: writes a prefix of the bytes and returns the count, or -1.
    pub file_write: FunctionValue<'ctx>,
    /// `(fd: i32, offset: i64, whence: i32) -> i64`: the new position, or -1.
    pub file_seek: FunctionValue<'ctx>,
    /// `(fd: i32) -> i32`: makes the file's contents durable; 0 on success.
    pub file_sync: FunctionValue<'ctx>,
    /// `(fd: i32, size: u64) -> i32`: sets the file's length; 0 on success.
    pub file_set_len: FunctionValue<'ctx>,
    /// `(fd: i32) -> i32`: closes the descriptor; 0 on success.
    pub file_close: FunctionValue<'ctx>,
    /// `() -> i32`: the OS code of the last failed file operation.
    pub file_error: FunctionValue<'ctx>,
    /// `(path, path_len, out: *List<String>) -> i32`: the names in a directory; 0, or the OS error code.
    pub read_dir: FunctionValue<'ctx>,
    /// `(path, path_len) -> i32`: creates one directory; 0, or the OS error code.
    pub create_dir: FunctionValue<'ctx>,
    /// `(path, path_len) -> i32`: removes an empty directory; 0, or the OS error code.
    pub remove_dir: FunctionValue<'ctx>,
    /// `(path, path_len) -> i32`: removes a file or a symbolic link; 0, or the OS error code.
    pub remove_file: FunctionValue<'ctx>,
    /// `(from, from_len, to, to_len) -> i32`: renames a file or a directory, replacing `to`; 0, or the OS error
    /// code.
    pub rename: FunctionValue<'ctx>,
    /// `(path, path_len, out: *RawMetadata) -> i32`: the entry's size, mode, times, and kind; 0, or the OS error
    /// code.
    pub metadata: FunctionValue<'ctx>,
    /// `(path, path_len, out: *String) -> i32`: the absolute, normalized path; 0, or the OS error code.
    pub canonicalize: FunctionValue<'ctx>,
    /// `(out: *List<String>)`: the process arguments, program name first.
    pub args: FunctionValue<'ctx>,
    /// `(kind: i32) -> i64`: nanoseconds from the real-time clock (kind 0) or the monotonic clock.
    pub clock: FunctionValue<'ctx>,
    /// `() -> i64`: 64 random bits from the operating system.
    pub entropy: FunctionValue<'ctx>,
    /// `(nanos: i64)`: suspends the thread.
    pub sleep: FunctionValue<'ctx>,
    /// `(out: *List<String>)`: every environment entry as `NAME=VALUE`, in the order the system reports them.
    pub env_list: FunctionValue<'ctx>,
    /// `(program, program_len, args, args_len, env, env_len, cwd, cwd_len, command_line, command_line_len,
    /// stdin, stdout, stderr, out: *[i64; 5]) -> i32`: starts a child without a shell. The tuple is
    /// `(pid, process, stdin, stdout, stderr)`; `process` is the Windows process handle and 0 on Unix, and each
    /// pipe handle is 0 when the stream is not piped.
    pub process_spawn: FunctionValue<'ctx>,
    /// `(pid: i32, process: usize) -> i64`: the exit code, `-1` for a failure to wait, or `-(signal + 1)`.
    pub process_wait: FunctionValue<'ctx>,
    /// `(process: usize)`: releases the Windows process handle; a no-op on Unix.
    pub process_close: FunctionValue<'ctx>,
    /// `(handle: usize, out: *String) -> i32`: every byte left in the pipe; 0 on success and -1 on failure.
    pub pipe_read_to_string: FunctionValue<'ctx>,
    /// `(handle: usize, out: *String) -> i32`: one line without its terminator; 0 for a line, 1 for the end with
    /// nothing read, and -1 for a failure.
    pub pipe_read_line: FunctionValue<'ctx>,
    /// `(first: usize, second: usize, out: *[String; 2]) -> i32`: both pipes read to the end in one pass.
    pub pipe_read_both: FunctionValue<'ctx>,
    /// `(handle: usize, ptr, len: usize) -> i32`: writes every byte; 0 on success and -1 on failure.
    pub pipe_write: FunctionValue<'ctx>,
    /// `(handle: usize)`: closes a pipe end; a no-op for handle 0.
    pub pipe_close: FunctionValue<'ctx>,
    /// `(name, name_len, out: *List<u8>) -> i32`: the raw bytes of an environment variable; 1 when found.
    pub env_get: FunctionValue<'ctx>,
    /// `(name, name_len, value, value_len) -> i32`: 0 on success, 1 for an invalid name, 2 for an invalid value,
    /// and 3 for an operating-system failure.
    pub env_set: FunctionValue<'ctx>,
    /// `(name, name_len) -> i32`: 0 on success, 1 for an invalid name, and 3 for an operating-system failure.
    pub env_remove: FunctionValue<'ctx>,
    /// `(out: *List<u8>) -> i32`: the raw bytes of the working directory; 1 when read.
    pub current_dir: FunctionValue<'ctx>,
    /// `(path, path_len) -> i32`: 0 on success, 1 for an invalid path, and 3 for an operating-system failure.
    pub set_current_dir: FunctionValue<'ctx>,
    /// `(code: i32)`: ends the process.
    pub exit: FunctionValue<'ctx>,
    /// `(command, command_len, err: *i32) -> i32`: runs a shell command; -1 on failure with the OS error code
    /// in `err`, and the exit code otherwise.
    pub run: FunctionValue<'ctx>,
}

/// Generates the runtime into `module`. `windows` selects the Windows CRT functions and `darwin` the macOS signal
/// structures.
pub(crate) fn generate<'ctx>(
    cx: &'ctx Context,
    module: &Module<'ctx>,
    usize_ty: IntType<'ctx>,
    string_ty: StructType<'ctx>,
    windows: bool,
    darwin: bool,
    checking: bool,
) -> GenResult<Runtime<'ctx>> {
    let mut g = Gen::new(cx, module, usize_ty, string_ty, windows, darwin, checking);
    g.generate()
}

/// Defines the process entry point `main`, which calls the program's `main` and returns 0.
pub(crate) fn define_main<'ctx>(
    cx: &'ctx Context,
    module: &Module<'ctx>,
    program_main: FunctionValue<'ctx>,
    windows: bool,
) -> GenResult<()> {
    let i32_ty = cx.i32_type();
    let ptr_ty = cx.ptr_type(AddressSpace::default());
    let f = module.add_function("main", i32_ty.fn_type(&[i32_ty.into(), ptr_ty.into()], false), None);
    let b = cx.create_builder();
    b.position_at_end(cx.append_basic_block(f, "entry"));
    prepare_main(&b, module, f, windows)?;
    b.build_call(program_main, &[], "").map_err(llvm_err)?;
    b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;
    Ok(())
}

/// Defines the process entry point of a `burdin test` binary. The driver starts it once per test with the test's
/// index as the only argument; this selects that test, calls it, and returns 0 when it passed and 1 when an
/// `assert` failed. Running one test per process is what makes a panic abort only the running test. A missing or
/// unknown index returns 2 or 3, which the driver reports as an error of its own.
pub(crate) fn define_test_main<'ctx>(
    cx: &'ctx Context,
    module: &Module<'ctx>,
    tests: &[FunctionValue<'ctx>],
    leak_check: Option<FunctionValue<'ctx>>,
    windows: bool,
) -> GenResult<()> {
    let i32_ty = cx.i32_type();
    let ptr_ty = cx.ptr_type(AddressSpace::default());
    let f = module.add_function("main", i32_ty.fn_type(&[i32_ty.into(), ptr_ty.into()], false), None);
    let b = cx.create_builder();
    b.position_at_end(cx.append_basic_block(f, "entry"));
    prepare_main(&b, module, f, windows)?;

    let argc = f.get_nth_param(0).expect("main has argc").into_int_value();
    let argv = f.get_nth_param(1).expect("main has argv").into_pointer_value();
    let has_arg =
        b.build_int_compare(IntPredicate::SGT, argc, i32_ty.const_int(1, false), "has_arg").map_err(llvm_err)?;
    let parse = cx.append_basic_block(f, "parse");
    let missing = cx.append_basic_block(f, "missing");
    b.build_conditional_branch(has_arg, parse, missing).map_err(llvm_err)?;
    b.position_at_end(missing);
    b.build_return(Some(&i32_ty.const_int(2, false))).map_err(llvm_err)?;

    b.position_at_end(parse);
    let slot =
        unsafe { b.build_in_bounds_gep(ptr_ty, argv, &[i32_ty.const_int(1, false)], "arg") }.map_err(llvm_err)?;
    let arg = b.build_load(ptr_ty, slot, "arg").map_err(llvm_err)?.into_pointer_value();
    // `atoi` returns `int` on every CRT, so the index does not depend on the size of C's `long`.
    let atoi = libc(module, "atoi", i32_ty.fn_type(&[ptr_ty.into()], false));
    let index = b
        .build_call(atoi, &[arg.into()], "index")
        .map_err(llvm_err)?
        .try_as_basic_value()
        .basic()
        .expect("atoi returns an int")
        .into_int_value();

    for (i, test) in tests.iter().enumerate() {
        let case = cx.append_basic_block(f, &format!("test.{i}"));
        let next = cx.append_basic_block(f, &format!("next.{i}"));
        let is =
            b.build_int_compare(IntPredicate::EQ, index, i32_ty.const_int(i as u64, false), "").map_err(llvm_err)?;
        b.build_conditional_branch(is, case, next).map_err(llvm_err)?;
        b.position_at_end(case);
        let result = b
            .build_call(*test, &[], "result")
            .map_err(llvm_err)?
            .try_as_basic_value()
            .basic()
            .expect("a test returns its Result")
            .into_struct_value();
        let ok = b.build_extract_value(result, 0, "ok").map_err(llvm_err)?.into_int_value();
        // A test that passed is checked for leaked allocations: the checking allocator reports them on standard
        // error and the test fails (`spec/allocators.md`, AL9). A test that already failed keeps its own verdict.
        let pass = cx.append_basic_block(f, &format!("pass.{i}"));
        let fail = cx.append_basic_block(f, &format!("fail.{i}"));
        b.build_conditional_branch(ok, pass, fail).map_err(llvm_err)?;
        b.position_at_end(pass);
        let code = match leak_check {
            Some(check) => {
                let leaked = b
                    .build_call(check, &[], "leaked")
                    .map_err(llvm_err)?
                    .try_as_basic_value()
                    .basic()
                    .expect("the leak check returns an int")
                    .into_int_value();
                let nonzero =
                    b.build_int_compare(IntPredicate::NE, leaked, i32_ty.const_zero(), "").map_err(llvm_err)?;
                b.build_select(nonzero, i32_ty.const_int(1, false), i32_ty.const_zero(), "code")
                    .map_err(llvm_err)?
                    .into_int_value()
            }
            None => i32_ty.const_zero(),
        };
        b.build_return(Some(&code)).map_err(llvm_err)?;
        b.position_at_end(fail);
        b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;
        b.position_at_end(next);
    }
    b.build_return(Some(&i32_ty.const_int(3, false))).map_err(llvm_err)?;
    Ok(())
}

/// The process entry point of a `burdin bench` binary (`spec/bench.md`, BN4). The runner starts it once per
/// measured bench with the bench's index and a mode:
///
/// * `<index> measure <warmup-ms> <sample-count> <sample-ms>`: warms the bench up for the given milliseconds,
///   times one call to choose how many iterations one sample holds, collects that many samples, and prints one
///   machine-readable line with the iteration count and each sample's total nanoseconds.
/// * `<index> profile`: calls the bench in a loop with no timing at all, so an external profiler can attach
///   (`spec/bench.md`, BN10); the runner stops the process when the requested time is up.
///
/// The result line is `burdin-bench-result: <iters> <sample-1> <sample-2> ...`, written through the buffered
/// standard output (`burdin_rt_print`) and flushed by the runtime's `atexit`. The runner reads the last such line,
/// so a bench that prints is tolerated, even though it should not. A missing argument returns 2, an unknown index
/// 3, an unknown mode 4.
pub(crate) fn define_bench_main<'ctx>(
    cx: &'ctx Context,
    module: &Module<'ctx>,
    benches: &[FunctionValue<'ctx>],
    rt: &Runtime<'ctx>,
    windows: bool,
) -> GenResult<()> {
    let (i32_ty, i64_ty, ptr_ty, void_ty) =
        (cx.i32_type(), cx.i64_type(), cx.ptr_type(AddressSpace::default()), cx.void_type());
    let run_for = bench_run_for(cx, module, rt.clock)?;
    let sample = bench_sample(cx, module, rt.clock)?;
    let f = module.add_function("main", i32_ty.fn_type(&[i32_ty.into(), ptr_ty.into()], false), None);
    let b = cx.create_builder();
    b.position_at_end(cx.append_basic_block(f, "entry"));
    prepare_main(&b, module, f, windows)?;
    let argc = f.get_nth_param(0).expect("main has argc").into_int_value();
    let argv = f.get_nth_param(1).expect("main has argv").into_pointer_value();
    let has_mode =
        b.build_int_compare(IntPredicate::SGT, argc, i32_ty.const_int(2, false), "has.mode").map_err(llvm_err)?;
    let parse = cx.append_basic_block(f, "parse");
    let missing = cx.append_basic_block(f, "missing");
    b.build_conditional_branch(has_mode, parse, missing).map_err(llvm_err)?;
    b.position_at_end(missing);
    b.build_return(Some(&i32_ty.const_int(2, false))).map_err(llvm_err)?;

    b.position_at_end(parse);
    let slot =
        unsafe { b.build_in_bounds_gep(ptr_ty, argv, &[i32_ty.const_int(1, false)], "arg") }.map_err(llvm_err)?;
    let arg = b.build_load(ptr_ty, slot, "index.arg").map_err(llvm_err)?.into_pointer_value();
    // `atoi` returns `int` on every CRT, so the index does not depend on the size of C's `long`.
    let atoi = libc(module, "atoi", i32_ty.fn_type(&[ptr_ty.into()], false));
    let index = b
        .build_call(atoi, &[arg.into()], "index")
        .map_err(llvm_err)?
        .try_as_basic_value()
        .basic()
        .expect("atoi returns an int")
        .into_int_value();
    let mode_slot =
        unsafe { b.build_in_bounds_gep(ptr_ty, argv, &[i32_ty.const_int(2, false)], "arg") }.map_err(llvm_err)?;
    let mode = b.build_load(ptr_ty, mode_slot, "mode.arg").map_err(llvm_err)?.into_pointer_value();

    let bad_mode = cx.append_basic_block(f, "bad.mode");
    let missing_args = cx.append_basic_block(f, "missing.args");
    for (i, bench) in benches.iter().enumerate() {
        let bench_ptr = bench.as_global_value().as_pointer_value();
        let case = cx.append_basic_block(f, &format!("bench.{i}"));
        let next = cx.append_basic_block(f, &format!("next.{i}"));
        let is =
            b.build_int_compare(IntPredicate::EQ, index, i32_ty.const_int(i as u64, false), "").map_err(llvm_err)?;
        b.build_conditional_branch(is, case, next).map_err(llvm_err)?;
        b.position_at_end(case);
        // The mode is spelled by the runner; its first letter is enough to tell the two apart.
        let first = b.build_load(cx.i8_type(), mode, "mode.0").map_err(llvm_err)?.into_int_value();
        let is_profile = b
            .build_int_compare(IntPredicate::EQ, first, cx.i8_type().const_int(b'p' as u64, false), "")
            .map_err(llvm_err)?;
        let is_measure = b
            .build_int_compare(IntPredicate::EQ, first, cx.i8_type().const_int(b'm' as u64, false), "")
            .map_err(llvm_err)?;
        let profile = cx.append_basic_block(f, &format!("profile.{i}"));
        let maybe_measure = cx.append_basic_block(f, &format!("mode.{i}"));
        let measure = cx.append_basic_block(f, &format!("measure.{i}"));
        b.build_conditional_branch(is_profile, profile, maybe_measure).map_err(llvm_err)?;
        b.position_at_end(maybe_measure);
        b.build_conditional_branch(is_measure, measure, bad_mode).map_err(llvm_err)?;

        // Profiling: call the bench forever, with no clock in the loop at all.
        b.position_at_end(profile);
        let bench_ty = void_ty.fn_type(&[], false);
        b.build_indirect_call(bench_ty, bench_ptr, &[], "").map_err(llvm_err)?;
        b.build_unconditional_branch(profile).map_err(llvm_err)?;

        b.position_at_end(measure);
        let has_args =
            b.build_int_compare(IntPredicate::SGT, argc, i32_ty.const_int(5, false), "has.args").map_err(llvm_err)?;
        let args = cx.append_basic_block(f, &format!("args.{i}"));
        b.build_conditional_branch(has_args, args, missing_args).map_err(llvm_err)?;
        b.position_at_end(args);
        let read_int = |n: u64, name: &str| -> GenResult<IntValue<'ctx>> {
            let slot = unsafe { b.build_in_bounds_gep(ptr_ty, argv, &[i32_ty.const_int(n, false)], name) }
                .map_err(llvm_err)?;
            let arg = b.build_load(ptr_ty, slot, name).map_err(llvm_err)?.into_pointer_value();
            let value = b
                .build_call(atoi, &[arg.into()], name)
                .map_err(llvm_err)?
                .try_as_basic_value()
                .basic()
                .expect("atoi returns an int")
                .into_int_value();
            b.build_int_cast_sign_flag(value, i64_ty, true, name).map_err(llvm_err)
        };
        let warmup = read_int(3, "warmup")?;
        let samples = read_int(4, "samples")?;
        let sample_ms = read_int(5, "sample.ms")?;
        let nanos_per_ms = i64_ty.const_int(1_000_000, false);
        let warmup_ns = b.build_int_mul(warmup, nanos_per_ms, "warmup.ns").map_err(llvm_err)?;
        let target_ns = b.build_int_mul(sample_ms, nanos_per_ms, "target.ns").map_err(llvm_err)?;
        // Warm up (at least one call), then time growing batches until one takes a millisecond, so the clock's
        // granularity cannot swamp the estimate: a call measured alone would look ~25 ns however fast it is.
        b.build_call(run_for, &[bench_ptr.into(), warmup_ns.into()], "").map_err(llvm_err)?;
        let batch_at = b.build_alloca(i64_ty, "batch").map_err(llvm_err)?;
        b.build_store(batch_at, i64_ty.const_int(1, false)).map_err(llvm_err)?;
        let taken_at = b.build_alloca(i64_ty, "taken").map_err(llvm_err)?;
        b.build_store(taken_at, i64_ty.const_zero()).map_err(llvm_err)?;
        let cal_head = cx.append_basic_block(f, &format!("calibrate.{i}"));
        let cal_grow = cx.append_basic_block(f, &format!("calibrate.grow.{i}"));
        let cal_done = cx.append_basic_block(f, &format!("calibrated.{i}"));
        b.build_unconditional_branch(cal_head).map_err(llvm_err)?;
        b.position_at_end(cal_head);
        let batch = b.build_load(i64_ty, batch_at, "batch").map_err(llvm_err)?.into_int_value();
        let taken = b
            .build_call(sample, &[bench_ptr.into(), batch.into()], "cal")
            .map_err(llvm_err)?
            .try_as_basic_value()
            .basic()
            .expect("sample returns i64")
            .into_int_value();
        b.build_store(taken_at, taken).map_err(llvm_err)?;
        let long_enough = b
            .build_int_compare(IntPredicate::SGE, taken, i64_ty.const_int(CALIBRATION_NS, false), "")
            .map_err(llvm_err)?;
        let too_many =
            b.build_int_compare(IntPredicate::SGE, batch, i64_ty.const_int(BATCH_MAX, false), "").map_err(llvm_err)?;
        let ready = b.build_or(long_enough, too_many, "").map_err(llvm_err)?;
        b.build_conditional_branch(ready, cal_done, cal_grow).map_err(llvm_err)?;
        b.position_at_end(cal_grow);
        let grown = b.build_int_mul(batch, i64_ty.const_int(4, false), "").map_err(llvm_err)?;
        b.build_store(batch_at, grown).map_err(llvm_err)?;
        b.build_unconditional_branch(cal_head).map_err(llvm_err)?;
        b.position_at_end(cal_done);
        let batch = b.build_load(i64_ty, batch_at, "batch").map_err(llvm_err)?.into_int_value();
        let taken = b.build_load(i64_ty, taken_at, "taken").map_err(llvm_err)?.into_int_value();
        // A clock that read no time at all (the coarsest timers) must not divide by zero.
        let idle = b.build_int_compare(IntPredicate::SLT, taken, i64_ty.const_int(1, false), "").map_err(llvm_err)?;
        let taken =
            b.build_select(idle, i64_ty.const_int(1, false), taken, "taken").map_err(llvm_err)?.into_int_value();
        // `iters = target_ns * batch / taken`, split so the product cannot overflow: the batches that stop the
        // loop take at least a millisecond, so `target_ns / taken` is at most about a million.
        let whole = b.build_int_unsigned_div(target_ns, taken, "").map_err(llvm_err)?;
        let rest = b.build_int_unsigned_rem(target_ns, taken, "").map_err(llvm_err)?;
        let scaled = b.build_int_mul(rest, batch, "").map_err(llvm_err)?;
        let part = b.build_int_unsigned_div(scaled, taken, "").map_err(llvm_err)?;
        let whole_batch = b.build_int_mul(whole, batch, "").map_err(llvm_err)?;
        let iters = b.build_int_add(whole_batch, part, "iters").map_err(llvm_err)?;
        let none = b.build_int_compare(IntPredicate::SLT, iters, i64_ty.const_int(1, false), "").map_err(llvm_err)?;
        let iters =
            b.build_select(none, i64_ty.const_int(1, false), iters, "iters").map_err(llvm_err)?.into_int_value();
        // Collect one total per sample, then print the result line.
        let buf = b.build_array_alloca(i64_ty, samples, "sample.buf").map_err(llvm_err)?;
        let at = b.build_alloca(i64_ty, "i").map_err(llvm_err)?;
        b.build_store(at, i64_ty.const_zero()).map_err(llvm_err)?;
        let head = cx.append_basic_block(f, &format!("sample.{i}"));
        let body = cx.append_basic_block(f, &format!("sample.body.{i}"));
        let report = cx.append_basic_block(f, &format!("report.{i}"));
        b.build_unconditional_branch(head).map_err(llvm_err)?;
        b.position_at_end(head);
        let i = b.build_load(i64_ty, at, "i").map_err(llvm_err)?.into_int_value();
        let done = b.build_int_compare(IntPredicate::SGE, i, samples, "").map_err(llvm_err)?;
        b.build_conditional_branch(done, report, body).map_err(llvm_err)?;
        b.position_at_end(body);
        let total = b
            .build_call(sample, &[bench_ptr.into(), iters.into()], "total")
            .map_err(llvm_err)?
            .try_as_basic_value()
            .basic()
            .expect("sample returns i64")
            .into_int_value();
        let slot = unsafe { b.build_in_bounds_gep(i64_ty, buf, &[i], "slot") }.map_err(llvm_err)?;
        b.build_store(slot, total).map_err(llvm_err)?;
        b.build_store(at, b.build_int_add(i, i64_ty.const_int(1, false), "").map_err(llvm_err)?).map_err(llvm_err)?;
        b.build_unconditional_branch(head).map_err(llvm_err)?;

        b.position_at_end(report);
        let marker = bench_bytes(cx, module, "burdin.bench.marker", b"burdin-bench-result: ");
        bench_print_str(&b, rt.print, marker, b"burdin-bench-result: ".len(), false)?;
        bench_print_num(cx, &b, rt, iters)?;
        let j_at = b.build_alloca(i64_ty, "j").map_err(llvm_err)?;
        b.build_store(j_at, i64_ty.const_zero()).map_err(llvm_err)?;
        let num_head = cx.append_basic_block(f, &format!("nums.{i}"));
        let num_body = cx.append_basic_block(f, &format!("nums.body.{i}"));
        let finish = cx.append_basic_block(f, &format!("finish.{i}"));
        b.build_unconditional_branch(num_head).map_err(llvm_err)?;
        b.position_at_end(num_head);
        let j = b.build_load(i64_ty, j_at, "j").map_err(llvm_err)?.into_int_value();
        let printed = b.build_int_compare(IntPredicate::SGE, j, samples, "").map_err(llvm_err)?;
        b.build_conditional_branch(printed, finish, num_body).map_err(llvm_err)?;
        b.position_at_end(num_body);
        let space = bench_bytes(cx, module, "burdin.bench.space", b" ");
        bench_print_str(&b, rt.print, space, 1, false)?;
        let slot = unsafe { b.build_in_bounds_gep(i64_ty, buf, &[j], "value") }.map_err(llvm_err)?;
        let value = b.build_load(i64_ty, slot, "sample").map_err(llvm_err)?.into_int_value();
        bench_print_num(cx, &b, rt, value)?;
        b.build_store(j_at, b.build_int_add(j, i64_ty.const_int(1, false), "").map_err(llvm_err)?).map_err(llvm_err)?;
        b.build_unconditional_branch(num_head).map_err(llvm_err)?;
        b.position_at_end(finish);
        // The trailing newline completes the line; the runtime's `atexit` flush flushes the buffer.
        b.build_call(
            rt.print,
            &[
                i32_ty.const_int(1, false).into(),
                marker.into(),
                i64_ty.const_int(0, false).into(),
                i32_ty.const_int(1, false).into(),
            ],
            "",
        )
        .map_err(llvm_err)?;
        b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;
        b.position_at_end(next);
    }
    // After the last bench's dispatch chain: an unknown index. The shared mode-failure blocks end here too, so
    // each is terminated once however many benches there are.
    b.build_return(Some(&i32_ty.const_int(3, false))).map_err(llvm_err)?;
    b.position_at_end(missing_args);
    b.build_return(Some(&i32_ty.const_int(2, false))).map_err(llvm_err)?;
    b.position_at_end(bad_mode);
    b.build_return(Some(&i32_ty.const_int(4, false))).map_err(llvm_err)?;
    Ok(())
}

/// Keeps the timing loops out of `main`: inlined there, the optimizer sees which bench the pointer names, inlines
/// it, and deletes the loop of an empty bench, which then measures no time at all.
fn mark_noinline<'ctx>(cx: &'ctx Context, f: FunctionValue<'ctx>) {
    f.add_attribute(AttributeLoc::Function, cx.create_enum_attribute(Attribute::get_named_enum_kind_id("noinline"), 0));
}

/// `burdin_bench_run_for(bench: ptr, nanos: i64)`: calls the bench until `nanos` of the monotonic clock have
/// passed, always at least once, so a zero warmup still primes the code once (`spec/bench.md`, BN4).
fn bench_run_for<'ctx>(
    cx: &'ctx Context,
    module: &Module<'ctx>,
    clock: FunctionValue<'ctx>,
) -> GenResult<FunctionValue<'ctx>> {
    let (i64_ty, ptr_ty, void_ty) = (cx.i64_type(), cx.ptr_type(AddressSpace::default()), cx.void_type());
    let f = module.add_function("burdin_bench_run_for", void_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false), None);
    mark_noinline(cx, f);
    let bench = f.get_nth_param(0).expect("the bench pointer").into_pointer_value();
    let nanos = f.get_nth_param(1).expect("the nanosecond budget").into_int_value();
    let b = cx.create_builder();
    b.position_at_end(cx.append_basic_block(f, "entry"));
    let start = clock_now(&b, clock, "start")?;
    let head = cx.append_basic_block(f, "loop");
    let done = cx.append_basic_block(f, "done");
    b.build_unconditional_branch(head).map_err(llvm_err)?;
    b.position_at_end(head);
    b.build_indirect_call(void_ty.fn_type(&[], false), bench, &[], "").map_err(llvm_err)?;
    let now = clock_now(&b, clock, "now")?;
    let elapsed = b.build_int_sub(now, start, "elapsed").map_err(llvm_err)?;
    let over = b.build_int_compare(IntPredicate::SGE, elapsed, nanos, "").map_err(llvm_err)?;
    b.build_conditional_branch(over, done, head).map_err(llvm_err)?;
    b.position_at_end(done);
    b.build_return(None).map_err(llvm_err)?;
    Ok(f)
}

/// `burdin_bench_sample(bench: ptr, iters: i64) -> i64`: one timed sample — `iters` calls of the bench and the
/// total nanoseconds they took (`spec/bench.md`, BN4).
fn bench_sample<'ctx>(
    cx: &'ctx Context,
    module: &Module<'ctx>,
    clock: FunctionValue<'ctx>,
) -> GenResult<FunctionValue<'ctx>> {
    let (i64_ty, ptr_ty, void_ty) = (cx.i64_type(), cx.ptr_type(AddressSpace::default()), cx.void_type());
    let f = module.add_function("burdin_bench_sample", i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false), None);
    mark_noinline(cx, f);
    let bench = f.get_nth_param(0).expect("the bench pointer").into_pointer_value();
    let iters = f.get_nth_param(1).expect("the iteration count").into_int_value();
    let b = cx.create_builder();
    b.position_at_end(cx.append_basic_block(f, "entry"));
    let start = clock_now(&b, clock, "start")?;
    let at = b.build_alloca(i64_ty, "count").map_err(llvm_err)?;
    b.build_store(at, i64_ty.const_zero()).map_err(llvm_err)?;
    let head = cx.append_basic_block(f, "loop");
    let body = cx.append_basic_block(f, "body");
    let done = cx.append_basic_block(f, "done");
    b.build_unconditional_branch(head).map_err(llvm_err)?;
    b.position_at_end(head);
    let count = b.build_load(i64_ty, at, "count").map_err(llvm_err)?.into_int_value();
    let over = b.build_int_compare(IntPredicate::SGE, count, iters, "").map_err(llvm_err)?;
    b.build_conditional_branch(over, done, body).map_err(llvm_err)?;
    b.position_at_end(body);
    b.build_indirect_call(void_ty.fn_type(&[], false), bench, &[], "").map_err(llvm_err)?;
    b.build_store(at, b.build_int_add(count, i64_ty.const_int(1, false), "").map_err(llvm_err)?).map_err(llvm_err)?;
    b.build_unconditional_branch(head).map_err(llvm_err)?;
    b.position_at_end(done);
    let now = clock_now(&b, clock, "now")?;
    b.build_return(Some(&b.build_int_sub(now, start, "total").map_err(llvm_err)?)).map_err(llvm_err)?;
    Ok(f)
}

/// The monotonic clock's current nanoseconds.
fn clock_now<'ctx>(b: &Builder<'ctx>, clock: FunctionValue<'ctx>, name: &str) -> GenResult<IntValue<'ctx>> {
    let i32_ty = b.get_insert_block().expect("positioned").get_context().i32_type();
    let now = b.build_call(clock, &[i32_ty.const_int(1, false).into()], name).map_err(llvm_err)?;
    Ok(now.try_as_basic_value().basic().expect("the clock returns i64").into_int_value())
}

/// A private constant global holding `bytes`, named so the module's IR stays readable.
fn bench_bytes<'ctx>(cx: &'ctx Context, module: &Module<'ctx>, name: &str, bytes: &[u8]) -> PointerValue<'ctx> {
    let data = cx.const_string(bytes, false);
    let global = module.add_global(data.get_type(), None, name);
    global.set_initializer(&data);
    global.set_constant(true);
    global.set_linkage(Linkage::Private);
    global.set_unnamed_addr(true);
    global.as_pointer_value()
}

/// Writes `bytes` to the buffered standard output through `print` (`burdin_rt_print`), with a trailing newline
/// when `newline` is set.
fn bench_print_str<'ctx>(
    b: &Builder<'ctx>,
    print: FunctionValue<'ctx>,
    ptr: PointerValue<'ctx>,
    len: usize,
    newline: bool,
) -> GenResult<()> {
    let cx = b.get_insert_block().expect("positioned").get_context();
    let (i32_ty, i64_ty) = (cx.i32_type(), cx.i64_type());
    b.build_call(
        print,
        &[
            i32_ty.const_int(1, false).into(),
            ptr.into(),
            i64_ty.const_int(len as u64, false).into(),
            i32_ty.const_int(u64::from(newline), false).into(),
        ],
        "",
    )
    .map_err(llvm_err)?;
    Ok(())
}

/// Writes `value` as decimal text to the buffered standard output: `i64_to_string` into a fresh `String`, its
/// bytes through `print`, then the string's buffer freed.
fn bench_print_num<'ctx>(
    cx: &'ctx Context,
    b: &Builder<'ctx>,
    rt: &Runtime<'ctx>,
    value: IntValue<'ctx>,
) -> GenResult<()> {
    let (i32_ty, i64_ty, ptr_ty) = (cx.i32_type(), cx.i64_type(), cx.ptr_type(AddressSpace::default()));
    let string_ty = cx.struct_type(&[ptr_ty.into(), i64_ty.into(), i64_ty.into()], false);
    let out = b.build_alloca(string_ty, "num").map_err(llvm_err)?;
    b.build_call(rt.i64_to_string, &[value.into(), out.into()], "").map_err(llvm_err)?;
    let s = b.build_load(string_ty, out, "num").map_err(llvm_err)?.into_struct_value();
    let ptr = b.build_extract_value(s, 0, "ptr").map_err(llvm_err)?;
    let len = b.build_extract_value(s, 1, "len").map_err(llvm_err)?;
    let cap = b.build_extract_value(s, 2, "cap").map_err(llvm_err)?;
    b.build_call(
        rt.print,
        &[i32_ty.const_int(1, false).into(), ptr.into(), len.into(), i32_ty.const_int(0, false).into()],
        "",
    )
    .map_err(llvm_err)?;
    b.build_call(rt.string_free, &[ptr.into(), cap.into()], "").map_err(llvm_err)?;
    Ok(())
}

/// Saves `argc`/`argv` for `env.args()` and switches the standard handles to binary mode on Windows.
fn prepare_main<'ctx>(
    b: &Builder<'ctx>,
    module: &Module<'ctx>,
    f: FunctionValue<'ctx>,
    windows: bool,
) -> GenResult<()> {
    let cx = b.get_insert_block().expect("positioned").get_context();
    let i32_ty = cx.i32_type();
    let ptr_ty = cx.ptr_type(AddressSpace::default());
    let argc = f.get_nth_param(0).expect("main has argc").into_int_value();
    let argv = f.get_nth_param(1).expect("main has argv").into_pointer_value();
    if let Some(global) = module.get_global("burdin_rt_argc") {
        b.build_store(global.as_pointer_value(), argc).map_err(llvm_err)?;
    }
    if let Some(global) = module.get_global("burdin_rt_argv") {
        b.build_store(global.as_pointer_value(), argv).map_err(llvm_err)?;
    }
    if windows {
        // Output is binary-safe: no newline translation (tests/README.md, assumption 10).
        let setmode = libc(module, "_setmode", i32_ty.fn_type(&[i32_ty.into(), i32_ty.into()], false));
        const O_BINARY: u64 = 0x8000;
        for fd in 0..3 {
            let args = [i32_ty.const_int(fd, false).into(), i32_ty.const_int(O_BINARY, false).into()];
            b.build_call(setmode, &args, "").map_err(llvm_err)?;
        }
    }
    // A terminal gets line buffering, so interactive programs see each prompt; a pipe or file is block buffered.
    let isatty = if windows {
        libc(module, "_isatty", i32_ty.fn_type(&[i32_ty.into()], false))
    } else {
        libc(module, "isatty", i32_ty.fn_type(&[i32_ty.into()], false))
    };
    let tty = b
        .build_call(isatty, &[i32_ty.const_int(1, false).into()], "isatty")
        .map_err(llvm_err)?
        .try_as_basic_value()
        .basic()
        .expect("isatty returns an int")
        .into_int_value();
    let line_buffered =
        b.build_int_compare(IntPredicate::NE, tty, i32_ty.const_zero(), "line_buffered").map_err(llvm_err)?;
    if let Some(global) = module.get_global("burdin_rt_out_line_buffered") {
        b.build_store(global.as_pointer_value(), line_buffered).map_err(llvm_err)?;
    }
    // `atexit` flushes the standard-output buffer on a normal return or `process.exit`.
    if let Some(flush) = module.get_function("burdin_rt_flush_locked") {
        let atexit = libc(module, "atexit", i32_ty.fn_type(&[ptr_ty.into()], false));
        b.build_call(atexit, &[flush.as_global_value().as_pointer_value().into()], "").map_err(llvm_err)?;
    }
    if let Some(guard) = module.get_function("burdin_rt_install_stack_guard") {
        b.build_call(guard, &[], "").map_err(llvm_err)?;
    }
    Ok(())
}

/// The libc function `name`, declared on first use.
fn libc<'ctx>(module: &Module<'ctx>, name: &str, ty: FunctionType<'ctx>) -> FunctionValue<'ctx> {
    module.get_function(name).unwrap_or_else(|| module.add_function(name, ty, Some(Linkage::External)))
}

struct Gen<'ctx, 'a> {
    cx: &'ctx Context,
    module: &'a Module<'ctx>,
    b: Builder<'ctx>,
    windows: bool,
    darwin: bool,
    usize_ty: IntType<'ctx>,
    ptr_ty: PointerType<'ctx>,
    string_ty: StructType<'ctx>,
    noreturn: Attribute,
    globals: usize,
    /// A `burdin test` build: the checking allocator records live allocations and reports leaks.
    checking: bool,
    /// The allocator functions, created before the helpers that allocate (a `String` buffer, for example).
    fn_alloc: Option<FunctionValue<'ctx>>,
    fn_free: Option<FunctionValue<'ctx>>,
    fn_realloc: Option<FunctionValue<'ctx>>,
    /// `(acquire, release)` for the spin locks that keep the shared runtime state consistent when tasks run on
    /// several threads: the output buffer and, in a test build, the live-allocation list.
    spin: Option<(FunctionValue<'ctx>, FunctionValue<'ctx>)>,
}

/// Helpers used while generating the other functions.
struct Helpers<'ctx> {
    write_all: FunctionValue<'ctx>,
    flush: FunctionValue<'ctx>,
    die: FunctionValue<'ctx>,
    write_u64: FunctionValue<'ctx>,
    digits: FunctionValue<'ctx>,
    alloc_string: FunctionValue<'ctx>,
    panic_start: FunctionValue<'ctx>,
    print_backtrace: FunctionValue<'ctx>,
    panic_begin: FunctionValue<'ctx>,
}

/// The helpers that exist before the allocator, which they report an out-of-memory panic through.
struct BaseHelpers<'ctx> {
    write_all: FunctionValue<'ctx>,
    flush: FunctionValue<'ctx>,
    die: FunctionValue<'ctx>,
    write_u64: FunctionValue<'ctx>,
    digits: FunctionValue<'ctx>,
    panic_start: FunctionValue<'ctx>,
    print_backtrace: FunctionValue<'ctx>,
    panic_begin: FunctionValue<'ctx>,
}

impl<'ctx> BaseHelpers<'ctx> {
    fn with_alloc_string(self, alloc_string: FunctionValue<'ctx>) -> Helpers<'ctx> {
        Helpers {
            write_all: self.write_all,
            flush: self.flush,
            die: self.die,
            write_u64: self.write_u64,
            digits: self.digits,
            alloc_string,
            panic_start: self.panic_start,
            print_backtrace: self.print_backtrace,
            panic_begin: self.panic_begin,
        }
    }
}

impl<'ctx, 'a> Gen<'ctx, 'a> {
    fn new(
        cx: &'ctx Context,
        module: &'a Module<'ctx>,
        usize_ty: IntType<'ctx>,
        string_ty: StructType<'ctx>,
        windows: bool,
        darwin: bool,
        checking: bool,
    ) -> Self {
        Gen {
            cx,
            module,
            b: cx.create_builder(),
            windows,
            darwin,
            usize_ty,
            ptr_ty: cx.ptr_type(AddressSpace::default()),
            string_ty,
            noreturn: cx.create_enum_attribute(Attribute::get_named_enum_kind_id("noreturn"), 0),
            globals: 0,
            checking,
            fn_alloc: None,
            fn_free: None,
            fn_realloc: None,
            spin: None,
        }
    }

    fn generate(&mut self) -> GenResult<Runtime<'ctx>> {
        if self.checking {
            let live = self.module.add_global(self.ptr_ty, None, "burdin_rt_live");
            live.set_initializer(&self.ptr_ty.const_null());
            live.set_linkage(Linkage::Internal);
            let lock = self.module.add_global(self.i32(), None, "burdin_rt_live_lock");
            lock.set_initializer(&self.i32().const_zero());
            lock.set_linkage(Linkage::Internal);
        }
        let out_lock = self.module.add_global(self.i32(), None, "burdin_rt_out_lock");
        out_lock.set_initializer(&self.i32().const_zero());
        out_lock.set_linkage(Linkage::Internal);
        self.spin = Some(self.spin_functions()?);
        let h0 = self.base_helpers()?;
        let alloc = self.heap_alloc(&h0)?;
        let free = self.heap_free(&h0)?;
        let realloc = self.heap_realloc(&h0, alloc, free)?;
        self.fn_alloc = Some(alloc);
        self.fn_free = Some(free);
        self.fn_realloc = Some(realloc);
        let leak_check = if self.checking { Some(self.leak_check(&h0)?) } else { None };
        let untrack = if self.checking { Some(self.heap_untrack()?) } else { None };
        let alloc_string = self.alloc_string(&h0)?;
        let h = h0.with_alloc_string(alloc_string);
        let string_push_str = self.string_push_str(&h)?;
        let string_push_char = self.string_push_char(string_push_str)?;
        let cstring_from = self.cstring_from(&h)?;
        // The arguments `define_main` receives, saved for `env.args()`.
        let argc_ty = self.i32();
        let argc = self.module.add_global(argc_ty, None, "burdin_rt_argc");
        argc.set_initializer(&argc_ty.const_zero());
        argc.set_linkage(Linkage::Internal);
        let argv = self.module.add_global(self.ptr_ty, None, "burdin_rt_argv");
        argv.set_initializer(&self.ptr_ty.const_null());
        argv.set_linkage(Linkage::Internal);
        // The standard-input block buffer, shared by every read of standard input, and the last read error's
        // OS code, which `__stdin_avail`'s failure path reports through `__stdin_error`. The file operations keep
        // their last error code the same way, for `__file_error`.
        let in_fill = self.in_fill()?;
        let in_error = self.module.add_global(argc_ty, None, "burdin_rt_in_err");
        in_error.set_initializer(&argc_ty.const_zero());
        in_error.set_linkage(Linkage::Internal);
        let in_error_ptr = in_error.as_pointer_value();
        let file_error = self.module.add_global(argc_ty, None, "burdin_rt_file_err");
        file_error.set_initializer(&argc_ty.const_zero());
        file_error.set_linkage(Linkage::Internal);
        let file_error_ptr = file_error.as_pointer_value();
        // The socket layer's per-thread results: the system code of the last failure and the address of the last
        // datagram a `recv_from` returned (`spec/io.md`, IO5).
        let net_error = self.module.add_global(argc_ty, None, "burdin_rt_net_err");
        net_error.set_initializer(&argc_ty.const_zero());
        net_error.set_thread_local(true);
        net_error.set_linkage(Linkage::Internal);
        let net_peer = self.module.add_global(self.i64(), None, "burdin_rt_net_peer");
        net_peer.set_initializer(&self.i64().const_zero());
        net_peer.set_thread_local(true);
        net_peer.set_linkage(Linkage::Internal);
        let wsa = self.module.add_global(self.i32(), None, "burdin_rt_wsa");
        wsa.set_initializer(&self.i32().const_zero());
        wsa.set_linkage(Linkage::Internal);
        let clock = self.clock()?;
        self.net_functions(string_push_str, string_push_char)?;
        Ok(Runtime {
            print: self.print(&h)?,
            flush: self.module.get_function("burdin_rt_flush_locked").expect("the locked flush exists"),
            is_terminal: self.is_terminal()?,
            in_read_line: self.in_read_line(string_push_str, in_fill)?,
            in_read_to_string: self.in_read_to_string(string_push_str, in_fill)?,
            in_avail: self.in_avail(in_fill, in_error_ptr)?,
            in_error: self.in_error(in_error_ptr)?,
            in_next: self.in_next()?,
            panic: self.panic(&h)?,
            panic_bounds: self.panic_bounds(&h)?,
            string_free: self.string_free()?,
            string_new: self.string_new()?,
            string_clone: self.string_clone(&h)?,
            string_push_str,
            alloc,
            free,
            leak_check,
            untrack,
            str_to_string: self.str_to_string(&h)?,
            i64_to_string: self.int_to_string(&h, true)?,
            f64_to_string: self.f64_to_string(&h)?,
            u64_to_string: self.int_to_string(&h, false)?,
            char_to_string: self.char_to_string(&h)?,
            str_starts_with: self.str_starts_with()?,
            str_find_char: self.str_find_char()?,
            cstring_from,
            strlen: libc(self.module, "strlen", self.usize_ty.fn_type(&[self.ptr_ty.into()], false)),
            cstring_clone: self.cstring_clone(&h, cstring_from)?,
            realloc,
            string_push_char,
            str_eq: self.str_eq()?,
            str_cmp: self.str_cmp()?,
            str_hash: self.str_hash()?,
            str_find_str: self.str_find_str()?,
            parse_i64: self.parse_int(&h, true)?,
            parse_u64: self.parse_int(&h, false)?,
            read_to_string: self.read_to_string(&h, alloc_string, string_push_str)?,
            write_file: self.write_file(&h)?,
            file_open: self.file_open(file_error_ptr)?,
            file_read: self.file_read(file_error_ptr)?,
            file_write: self.file_write(file_error_ptr)?,
            file_seek: self.file_seek(file_error_ptr)?,
            file_sync: self.file_sync(file_error_ptr)?,
            file_set_len: self.file_set_len(file_error_ptr)?,
            file_close: self.file_close(file_error_ptr)?,
            file_error: self.file_error(file_error_ptr)?,
            read_dir: self.read_dir(&h)?,
            create_dir: self.create_dir()?,
            remove_dir: self.remove_dir()?,
            remove_file: self.remove_file()?,
            rename: self.rename()?,
            metadata: self.metadata()?,
            canonicalize: self.canonicalize(&h)?,
            args: self.args(argc.as_pointer_value(), argv.as_pointer_value(), alloc_string)?,
            clock,
            entropy: self.entropy(clock)?,
            sleep: self.sleep()?,
            env_list: self.env_list(alloc_string)?,
            process_spawn: self.process_spawn()?,
            process_wait: self.process_wait()?,
            process_close: self.process_close()?,
            pipe_read_to_string: self.pipe_read_to_string(string_push_str)?,
            pipe_read_line: self.pipe_read_line(string_push_str)?,
            pipe_read_both: self.pipe_read_both(string_push_str)?,
            pipe_write: self.pipe_write()?,
            pipe_close: self.pipe_close()?,
            env_get: self.env_get()?,
            env_set: self.env_set()?,
            env_remove: self.env_remove()?,
            current_dir: self.current_dir()?,
            set_current_dir: self.set_current_dir()?,
            exit: self.exit()?,
            run: self.run_command()?,
        })
    }

    /// `spin_acquire(lock: *i32)` and `spin_release(lock: *i32)`: a test-and-set lock. The critical sections it
    /// guards are a few instructions long, so waiting gives up the thread's time slice instead of sleeping.
    fn spin_functions(&mut self) -> GenResult<(FunctionValue<'ctx>, FunctionValue<'ctx>)> {
        let (i32_ty, ptr_ty) = (self.i32(), self.ptr_ty);
        let void = self.cx.void_type();
        let yield_now = if self.windows {
            libc(self.module, "SwitchToThread", i32_ty.fn_type(&[], false))
        } else {
            libc(self.module, "sched_yield", i32_ty.fn_type(&[], false))
        };
        let acquire = self.function("burdin_rt_spin_acquire", void.fn_type(&[ptr_ty.into()], false));
        let lock = Self::ptr_param(acquire, 0);
        let attempt = self.block(acquire, "attempt");
        let wait = self.block(acquire, "wait");
        let done = self.block(acquire, "done");
        self.b.build_unconditional_branch(attempt).map_err(llvm_err)?;
        self.b.position_at_end(attempt);
        let result = self
            .b
            .build_cmpxchg(
                lock,
                i32_ty.const_zero(),
                i32_ty.const_int(1, false),
                AtomicOrdering::Acquire,
                AtomicOrdering::Monotonic,
            )
            .map_err(llvm_err)?;
        let won = self.b.build_extract_value(result, 1, "won").map_err(llvm_err)?.into_int_value();
        self.b.build_conditional_branch(won, done, wait).map_err(llvm_err)?;
        self.b.position_at_end(wait);
        self.call(yield_now, &[])?;
        self.b.build_unconditional_branch(attempt).map_err(llvm_err)?;
        self.b.position_at_end(done);
        self.b.build_return(None).map_err(llvm_err)?;

        let release = self.function("burdin_rt_spin_release", void.fn_type(&[ptr_ty.into()], false));
        let store = self.b.build_store(Self::ptr_param(release, 0), i32_ty.const_zero()).map_err(llvm_err)?;
        store.set_atomic_ordering(AtomicOrdering::Release).map_err(|e| format!("LLVM builder error: {e}"))?;
        store.set_alignment(4).map_err(|e| format!("LLVM builder error: {e}"))?;
        self.b.build_return(None).map_err(llvm_err)?;
        Ok((acquire, release))
    }

    /// Takes the spin lock stored in the global `name`.
    fn lock_global(&mut self, name: &str) -> GenResult<()> {
        let lock = self.module.get_global(name).expect("the lock exists").as_pointer_value();
        let (acquire, _) = self.spin.expect("the spin functions exist");
        self.call(acquire, &[lock.into()])?;
        Ok(())
    }

    /// Releases the spin lock stored in the global `name`.
    fn unlock_global(&mut self, name: &str) -> GenResult<()> {
        let lock = self.module.get_global(name).expect("the lock exists").as_pointer_value();
        let (_, release) = self.spin.expect("the spin functions exist");
        self.call(release, &[lock.into()])?;
        Ok(())
    }

    fn base_helpers(&mut self) -> GenResult<BaseHelpers<'ctx>> {
        let write_all = self.write_all()?;
        let globals = self.output_globals();
        let flush = self.flush(write_all, globals)?;
        // The flush the program itself calls (`stdout.flush()` and the exit hook) takes the output lock; the
        // panic and signal paths call the plain one, because the thread that panicked may hold the lock.
        let locked = self.function("burdin_rt_flush_locked", self.cx.void_type().fn_type(&[], false));
        self.lock_global("burdin_rt_out_lock")?;
        self.call(flush, &[])?;
        self.unlock_global("burdin_rt_out_lock")?;
        self.b.build_return(None).map_err(llvm_err)?;
        let _ = locked;
        let _out_write = self.out_write(flush, globals)?;
        let die = self.die(flush)?;
        let panic_begin = self.panic_begin()?;
        self.install_stack_guard(flush, panic_begin)?;
        let digits = self.digits()?;
        let write_u64 = self.write_u64(write_all, digits)?;
        let hex_digits = self.hex_digits()?;
        self.write_hex(write_all, hex_digits)?;
        let print_backtrace = self.print_backtrace()?;
        let panic_start = self.panic_start(write_all, write_u64, flush)?;
        Ok(BaseHelpers { write_all, flush, die, write_u64, digits, panic_start, print_backtrace, panic_begin })
    }

    /// The globals behind the standard-output buffer, created on first use.
    fn output_globals(&mut self) -> (PointerValue<'ctx>, PointerValue<'ctx>, PointerValue<'ctx>) {
        let buf_ty = self.cx.i8_type().array_type(OUT_BUF);
        let buf = self.module.get_global("burdin_rt_out_buf").unwrap_or_else(|| {
            let g = self.module.add_global(buf_ty, None, "burdin_rt_out_buf");
            g.set_initializer(&buf_ty.const_zero());
            g.set_linkage(Linkage::Internal);
            g
        });
        let len = self.module.get_global("burdin_rt_out_len").unwrap_or_else(|| {
            let g = self.module.add_global(self.usize_ty, None, "burdin_rt_out_len");
            g.set_initializer(&self.usize_ty.const_zero());
            g.set_linkage(Linkage::Internal);
            g
        });
        let line = self.module.get_global("burdin_rt_out_line_buffered").unwrap_or_else(|| {
            let g = self.module.add_global(self.cx.bool_type(), None, "burdin_rt_out_line_buffered");
            g.set_initializer(&self.cx.bool_type().const_zero());
            g.set_linkage(Linkage::Internal);
            g
        });
        (buf.as_pointer_value(), len.as_pointer_value(), line.as_pointer_value())
    }

    // ----- small helpers ---------------------------------------------------------------

    fn i8(&self) -> IntType<'ctx> {
        self.cx.i8_type()
    }

    fn i32(&self) -> IntType<'ctx> {
        self.cx.i32_type()
    }

    fn i64(&self) -> IntType<'ctx> {
        self.cx.i64_type()
    }

    /// Adds an internal function and positions the builder at its `entry` block.
    fn function(&self, name: &str, ty: FunctionType<'ctx>) -> FunctionValue<'ctx> {
        let f = self.module.add_function(name, ty, Some(Linkage::Internal));
        self.b.position_at_end(self.cx.append_basic_block(f, "entry"));
        f
    }

    fn block(&self, f: FunctionValue<'ctx>, name: &str) -> BasicBlock<'ctx> {
        self.cx.append_basic_block(f, name)
    }

    fn int_param(f: FunctionValue<'ctx>, i: u32) -> IntValue<'ctx> {
        f.get_nth_param(i).expect("parameter count matches the signature").into_int_value()
    }

    fn ptr_param(f: FunctionValue<'ctx>, i: u32) -> PointerValue<'ctx> {
        f.get_nth_param(i).expect("parameter count matches the signature").into_pointer_value()
    }

    fn call(
        &self,
        f: FunctionValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        Ok(self.b.build_call(f, args, "").map_err(llvm_err)?.try_as_basic_value().basic())
    }

    /// `ptr + offset` bytes.
    fn offset(&self, ptr: PointerValue<'ctx>, offset: IntValue<'ctx>) -> GenResult<PointerValue<'ctx>> {
        unsafe { self.b.build_in_bounds_gep(self.i8(), ptr, &[offset], "") }.map_err(llvm_err)
    }

    /// `end - start` in bytes, as a `usize`.
    fn distance(&self, start: PointerValue<'ctx>, end: PointerValue<'ctx>) -> GenResult<IntValue<'ctx>> {
        let s = self.b.build_ptr_to_int(start, self.usize_ty, "").map_err(llvm_err)?;
        let e = self.b.build_ptr_to_int(end, self.usize_ty, "").map_err(llvm_err)?;
        self.b.build_int_sub(e, s, "").map_err(llvm_err)
    }

    /// The address of the platform's `errno`: `__errno_location` on Linux and `__error` on macOS.
    fn errno_ptr(&mut self) -> GenResult<PointerValue<'ctx>> {
        let symbol = if self.darwin { "__error" } else { "__errno_location" };
        let getter = libc(self.module, symbol, self.ptr_ty.fn_type(&[], false));
        Ok(self.call(getter, &[])?.expect("the errno accessor returns a pointer").into_pointer_value())
    }

    /// The operating system error code of the call that just failed, as an `i32` (`spec/errors.md`, ERR11).
    ///
    /// On Unix it is `errno`. On Windows it is the Win32 code: `GetLastError` for a Win32 call, and
    /// `_doserrno` for a CRT call, whose `errno` value is only the CRT's own classification. A failure that
    /// sets neither reports `ERROR_GEN_FAILURE`, and one that leaves `errno` zero reports `EIO`, so a nonzero
    /// code always means a failure.
    fn last_error(&mut self, from_crt: bool) -> GenResult<IntValue<'ctx>> {
        let i32_ty = self.i32();
        let zero = i32_ty.const_zero();
        if self.windows {
            let fallback = i32_ty.const_int(u64::from(ERROR_GEN_FAILURE), false);
            // `_get_doserrno` writes an `unsigned long`, which is 32 bits on Windows.
            let slot = self.b.build_alloca(i32_ty, "dos.errno").map_err(llvm_err)?;
            let getter = libc(self.module, "_get_doserrno", i32_ty.fn_type(&[self.ptr_ty.into()], false));
            self.call(getter, &[slot.into()])?;
            let dos = self.b.build_load(i32_ty, slot, "dos.errno").map_err(llvm_err)?.into_int_value();
            let get_last = libc(self.module, "GetLastError", i32_ty.fn_type(&[], false));
            let last = self.call(get_last, &[])?.expect("GetLastError returns a code").into_int_value();
            let base = if from_crt {
                let unset = self.b.build_int_compare(IntPredicate::EQ, dos, zero, "no.dos").map_err(llvm_err)?;
                self.b.build_select(unset, last, dos, "crt.error").map_err(llvm_err)?.into_int_value()
            } else {
                last
            };
            let unset = self.b.build_int_compare(IntPredicate::EQ, base, zero, "no.error").map_err(llvm_err)?;
            Ok(self.b.build_select(unset, fallback, base, "os.error").map_err(llvm_err)?.into_int_value())
        } else {
            let at = self.errno_ptr()?;
            let code = self.b.build_load(i32_ty, at, "errno").map_err(llvm_err)?.into_int_value();
            let unset = self.b.build_int_compare(IntPredicate::EQ, code, zero, "no.errno").map_err(llvm_err)?;
            let eio = i32_ty.const_int(libc::EIO as u64, false);
            Ok(self.b.build_select(unset, eio, code, "os.error").map_err(llvm_err)?.into_int_value())
        }
    }

    /// A stack buffer of `n` bytes; call it in the `entry` block only.
    fn buffer(&self, n: u32) -> GenResult<PointerValue<'ctx>> {
        self.b.build_alloca(self.i8().array_type(n), "buf").map_err(llvm_err)
    }

    /// A private constant global holding `bytes`.
    fn bytes(&mut self, bytes: &[u8]) -> PointerValue<'ctx> {
        let data = self.cx.const_string(bytes, false);
        let global = self.module.add_global(data.get_type(), None, &format!("rt.str.{}", self.globals));
        self.globals += 1;
        global.set_initializer(&data);
        global.set_constant(true);
        global.set_linkage(Linkage::Private);
        global.set_unnamed_addr(true);
        global.as_pointer_value()
    }

    /// Writes the constant `bytes` to `fd`.
    fn write_const(&mut self, h_write_all: FunctionValue<'ctx>, fd: u64, bytes: &[u8]) -> GenResult<()> {
        let ptr = self.bytes(bytes);
        let fd = self.i32().const_int(fd, false);
        let len = self.usize_ty.const_int(bytes.len() as u64, false);
        self.call(h_write_all, &[fd.into(), ptr.into(), len.into()])?;
        Ok(())
    }

    fn mark_noreturn(&self, f: FunctionValue<'ctx>) {
        f.add_attribute(AttributeLoc::Function, self.noreturn);
    }

    // ----- output ----------------------------------------------------------------------

    /// `write_all(fd: i32, ptr, len: usize)`: writes all bytes, in chunks of at most 1 GiB; stops on an error.
    fn write_all(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (i32_ty, usize_ty, ptr_ty) = (self.i32(), self.usize_ty, self.ptr_ty);
        let write = if self.windows {
            libc(self.module, "_write", i32_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), i32_ty.into()], false))
        } else {
            libc(self.module, "write", usize_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), usize_ty.into()], false))
        };
        let void = self.cx.void_type();
        let f =
            self.function("burdin_rt_write_all", void.fn_type(&[i32_ty.into(), ptr_ty.into(), usize_ty.into()], false));
        let (fd, ptr0, len0) = (Self::int_param(f, 0), Self::ptr_param(f, 1), Self::int_param(f, 2));
        let entry = self.b.get_insert_block().expect("positioned");
        let lp = self.block(f, "loop");
        let body = self.block(f, "body");
        let next = self.block(f, "next");
        let done = self.block(f, "done");
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;

        self.b.position_at_end(lp);
        let ptr = self.b.build_phi(ptr_ty, "ptr").map_err(llvm_err)?;
        let len = self.b.build_phi(usize_ty, "len").map_err(llvm_err)?;
        let len_v = len.as_basic_value().into_int_value();
        let ptr_v = ptr.as_basic_value().into_pointer_value();
        let empty = self.b.build_int_compare(IntPredicate::EQ, len_v, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(empty, done, body).map_err(llvm_err)?;

        self.b.position_at_end(body);
        let max = usize_ty.const_int(0x4000_0000, false);
        let big = self.b.build_int_compare(IntPredicate::UGT, len_v, max, "").map_err(llvm_err)?;
        let chunk = self.b.build_select(big, max, len_v, "chunk").map_err(llvm_err)?.into_int_value();
        let written = if self.windows {
            let c = self.b.build_int_truncate(chunk, i32_ty, "").map_err(llvm_err)?;
            let r = self.call(write, &[fd.into(), ptr_v.into(), c.into()])?.expect("_write returns int");
            self.b.build_int_s_extend(r.into_int_value(), usize_ty, "").map_err(llvm_err)?
        } else {
            self.call(write, &[fd.into(), ptr_v.into(), chunk.into()])?.expect("write returns ssize_t").into_int_value()
        };
        let failed =
            self.b.build_int_compare(IntPredicate::SLE, written, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(failed, done, next).map_err(llvm_err)?;

        self.b.position_at_end(next);
        let ptr2 = self.offset(ptr_v, written)?;
        let len2 = self.b.build_int_sub(len_v, written, "").map_err(llvm_err)?;
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;
        ptr.add_incoming(&[(&ptr0, entry), (&ptr2, next)]);
        len.add_incoming(&[(&len0, entry), (&len2, next)]);

        self.b.position_at_end(done);
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `flush()`: writes the standard-output buffer to fd 1 and empties it. Called by `atexit`, `process.exit`,
    /// the panic path, and the stack-overflow handler.
    fn flush(
        &mut self,
        write_all: FunctionValue<'ctx>,
        globals: (PointerValue<'ctx>, PointerValue<'ctx>, PointerValue<'ctx>),
    ) -> GenResult<FunctionValue<'ctx>> {
        let (buf, len_global, _line) = globals;
        let (i32_ty, usize_ty) = (self.i32(), self.usize_ty);
        let void = self.cx.void_type();
        let f = self.function("burdin_rt_flush", void.fn_type(&[], false));
        let empty = self.block(f, "flush.empty");
        let write = self.block(f, "flush.write");
        let len = self.b.build_load(usize_ty, len_global, "len").map_err(llvm_err)?.into_int_value();
        let none = self.b.build_int_compare(IntPredicate::EQ, len, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(none, empty, write).map_err(llvm_err)?;
        self.b.position_at_end(write);
        self.call(write_all, &[i32_ty.const_int(1, false).into(), buf.into(), len.into()])?;
        self.b.build_store(len_global, usize_ty.const_zero()).map_err(llvm_err)?;
        self.b.build_unconditional_branch(empty).map_err(llvm_err)?;
        self.b.position_at_end(empty);
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `out_write(ptr, len: usize)`: appends to the standard-output buffer, flushing it when it is full.
    fn out_write(
        &mut self,
        flush: FunctionValue<'ctx>,
        globals: (PointerValue<'ctx>, PointerValue<'ctx>, PointerValue<'ctx>),
    ) -> GenResult<FunctionValue<'ctx>> {
        let (buf, len_global, _line) = globals;
        let (usize_ty, ptr_ty) = (self.usize_ty, self.ptr_ty);
        let void = self.cx.void_type();
        let ty = void.fn_type(&[ptr_ty.into(), usize_ty.into()], false);
        let f = self.function("burdin_rt_out_write", ty);
        let entry = self.b.get_insert_block().expect("positioned");
        let lp = self.block(f, "loop");
        let check = self.block(f, "check");
        let copy = self.block(f, "copy");
        let full = self.block(f, "full");
        let done = self.block(f, "done");
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;

        self.b.position_at_end(lp);
        let ptr = self.b.build_phi(ptr_ty, "ptr").map_err(llvm_err)?;
        let len = self.b.build_phi(usize_ty, "len").map_err(llvm_err)?;
        let ptr_v = ptr.as_basic_value().into_pointer_value();
        let len_v = len.as_basic_value().into_int_value();
        let empty = self.b.build_int_compare(IntPredicate::EQ, len_v, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(empty, done, check).map_err(llvm_err)?;

        self.b.position_at_end(check);
        let old = self.b.build_load(usize_ty, len_global, "old_len").map_err(llvm_err)?.into_int_value();
        let space = self.b.build_int_sub(usize_ty.const_int(u64::from(OUT_BUF), false), old, "").map_err(llvm_err)?;
        let is_full = self.b.build_int_compare(IntPredicate::EQ, space, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(is_full, full, copy).map_err(llvm_err)?;

        self.b.position_at_end(full);
        self.call(flush, &[])?;
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;

        self.b.position_at_end(copy);
        let old = self.b.build_load(usize_ty, len_global, "old_len").map_err(llvm_err)?.into_int_value();
        let space = self.b.build_int_sub(usize_ty.const_int(u64::from(OUT_BUF), false), old, "").map_err(llvm_err)?;
        let take_less = self.b.build_int_compare(IntPredicate::ULT, space, len_v, "").map_err(llvm_err)?;
        let space_bv: BasicValueEnum<'ctx> = space.into();
        let len_bv: BasicValueEnum<'ctx> = len_v.into();
        let take = self.b.build_select(take_less, space_bv, len_bv, "take").map_err(llvm_err)?.into_int_value();
        let dst = self.offset(buf, old)?;
        self.b.build_memcpy(dst, 1, ptr_v, 1, take).map_err(|e| format!("LLVM builder error: {e}"))?;
        let new_len = self.b.build_int_add(old, take, "").map_err(llvm_err)?;
        self.b.build_store(len_global, new_len).map_err(llvm_err)?;
        let next_ptr = self.offset(ptr_v, take)?;
        let next_len = self.b.build_int_sub(len_v, take, "").map_err(llvm_err)?;
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;
        ptr.add_incoming(&[(&Self::ptr_param(f, 0), entry), (&next_ptr, copy), (&ptr_v, full)]);
        len.add_incoming(&[(&Self::int_param(f, 1), entry), (&next_len, copy), (&len_v, full)]);

        self.b.position_at_end(done);
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `print(fd: i32, ptr, len: usize, newline: i32)`: standard output goes through the buffer (and is flushed
    /// after a newline when the output is a terminal); standard error is written directly.
    fn print(&mut self, h: &Helpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (i32_ty, usize_ty, ptr_ty) = (self.i32(), self.usize_ty, self.ptr_ty);
        let void = self.cx.void_type();
        let ty = void.fn_type(&[i32_ty.into(), ptr_ty.into(), usize_ty.into(), i32_ty.into()], false);
        let f = self.function("burdin_rt_print", ty);
        let fd = Self::int_param(f, 0);
        let out_write = self.module.get_function("burdin_rt_out_write").expect("out_write exists");
        let stdout = self.block(f, "stdout");
        let stderr = self.block(f, "stderr");
        let done = self.block(f, "done");
        let is_stdout =
            self.b.build_int_compare(IntPredicate::EQ, fd, i32_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(is_stdout, stdout, stderr).map_err(llvm_err)?;

        // The buffer is shared by every thread that prints, so a write holds the output lock until its text and
        // newline are in the buffer (and flushed, on a terminal).
        let unlock = self.block(f, "stdout.unlock");
        self.b.position_at_end(stdout);
        self.lock_global("burdin_rt_out_lock")?;
        self.call(out_write, &[Self::ptr_param(f, 1).into(), Self::int_param(f, 2).into()])?;
        let out_nl = self.block(f, "stdout.newline");
        let out_flush = self.block(f, "stdout.flush");
        let nl = self
            .b
            .build_int_compare(IntPredicate::NE, Self::int_param(f, 3), i32_ty.const_zero(), "")
            .map_err(llvm_err)?;
        self.b.build_conditional_branch(nl, out_nl, unlock).map_err(llvm_err)?;
        self.b.position_at_end(out_nl);
        let nl_ptr = self.bytes(b"\n");
        self.call(out_write, &[nl_ptr.into(), usize_ty.const_int(1, false).into()])?;
        let (_, _, line_global) = self.output_globals();
        let line = self.b.build_load(self.cx.bool_type(), line_global, "line").map_err(llvm_err)?.into_int_value();
        self.b.build_conditional_branch(line, out_flush, unlock).map_err(llvm_err)?;
        self.b.position_at_end(out_flush);
        self.call(h.flush, &[])?;
        self.b.build_unconditional_branch(unlock).map_err(llvm_err)?;
        self.b.position_at_end(unlock);
        self.unlock_global("burdin_rt_out_lock")?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;

        self.b.position_at_end(stderr);
        self.call(h.write_all, &[fd.into(), Self::ptr_param(f, 1).into(), Self::int_param(f, 2).into()])?;
        let err_nl = self.block(f, "stderr.newline");
        let nl = self
            .b
            .build_int_compare(IntPredicate::NE, Self::int_param(f, 3), i32_ty.const_zero(), "")
            .map_err(llvm_err)?;
        self.b.build_conditional_branch(nl, err_nl, done).map_err(llvm_err)?;
        self.b.position_at_end(err_nl);
        let nl_ptr = self.bytes(b"\n");
        self.call(h.write_all, &[fd.into(), nl_ptr.into(), usize_ty.const_int(1, false).into()])?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;

        self.b.position_at_end(done);
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// The globals behind the standard-input buffer, created on first use: the block, the next byte, and the
    /// number of bytes the block holds.
    fn in_globals(&mut self) -> (PointerValue<'ctx>, PointerValue<'ctx>, PointerValue<'ctx>) {
        let buf_ty = self.cx.i8_type().array_type(IN_BUF);
        let buf = self.module.get_global("burdin_rt_in_buf").unwrap_or_else(|| {
            let g = self.module.add_global(buf_ty, None, "burdin_rt_in_buf");
            g.set_initializer(&buf_ty.const_zero());
            g.set_linkage(Linkage::Internal);
            g
        });
        let pos = self.module.get_global("burdin_rt_in_pos").unwrap_or_else(|| {
            let g = self.module.add_global(self.usize_ty, None, "burdin_rt_in_pos");
            g.set_initializer(&self.usize_ty.const_zero());
            g.set_linkage(Linkage::Internal);
            g
        });
        let len = self.module.get_global("burdin_rt_in_len").unwrap_or_else(|| {
            let g = self.module.add_global(self.usize_ty, None, "burdin_rt_in_len");
            g.set_initializer(&self.usize_ty.const_zero());
            g.set_linkage(Linkage::Internal);
            g
        });
        (buf.as_pointer_value(), pos.as_pointer_value(), len.as_pointer_value())
    }

    /// `is_terminal(fd: i32) -> i1`: whether the descriptor is a terminal.
    fn is_terminal(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let i32_ty = self.i32();
        let bool_ty = self.cx.bool_type();
        let f = self.function("burdin_rt_is_terminal", bool_ty.fn_type(&[i32_ty.into()], false));
        let isatty = if self.windows {
            libc(self.module, "_isatty", i32_ty.fn_type(&[i32_ty.into()], false))
        } else {
            libc(self.module, "isatty", i32_ty.fn_type(&[i32_ty.into()], false))
        };
        let r = self.call(isatty, &[Self::int_param(f, 0).into()])?.expect("isatty returns an int").into_int_value();
        let tty = self.b.build_int_compare(IntPredicate::NE, r, i32_ty.const_zero(), "tty").map_err(llvm_err)?;
        self.b.build_return(Some(&tty)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `in_fill(err: *i32) -> i32`: makes the input block hold at least one byte. 1 means data, 0 the end of
    /// input, and -1 a read error, whose OS code is stored in `err`.
    fn in_fill(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (buf, pos, len) = self.in_globals();
        let (i32_ty, usize_ty, ptr_ty) = (self.i32(), self.usize_ty, self.ptr_ty);
        let f = self.function("burdin_rt_in_fill", i32_ty.fn_type(&[ptr_ty.into()], false));
        let err = Self::ptr_param(f, 0);
        let have = self.block(f, "have");
        let read = self.block(f, "read");
        let got = self.block(f, "got");
        let none = self.block(f, "none");
        let eof = self.block(f, "eof");
        let failed = self.block(f, "failed");
        let pos_v = self.b.build_load(usize_ty, pos, "pos").map_err(llvm_err)?.into_int_value();
        let len_v = self.b.build_load(usize_ty, len, "len").map_err(llvm_err)?.into_int_value();
        let ready = self.b.build_int_compare(IntPredicate::ULT, pos_v, len_v, "ready").map_err(llvm_err)?;
        self.b.build_conditional_branch(ready, have, read).map_err(llvm_err)?;

        self.b.position_at_end(have);
        self.b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;

        self.b.position_at_end(read);
        let read_fn = if self.windows {
            libc(self.module, "_read", i32_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), i32_ty.into()], false))
        } else {
            libc(self.module, "read", usize_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), usize_ty.into()], false))
        };
        let count: BasicMetadataValueEnum<'ctx> = if self.windows {
            i32_ty.const_int(u64::from(IN_BUF), false).into()
        } else {
            usize_ty.const_int(u64::from(IN_BUF), false).into()
        };
        let n = self
            .b
            .build_call(read_fn, &[i32_ty.const_zero().into(), buf.into(), count], "n")
            .map_err(llvm_err)?
            .try_as_basic_value()
            .basic()
            .expect("read returns a count");
        let n = if self.windows {
            self.b.build_int_s_extend(n.into_int_value(), usize_ty, "n64").map_err(llvm_err)?
        } else {
            n.into_int_value()
        };
        let positive = self.b.build_int_compare(IntPredicate::SGT, n, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let empty = self.b.build_int_compare(IntPredicate::EQ, n, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(positive, got, none).map_err(llvm_err)?;

        self.b.position_at_end(got);
        self.b.build_store(pos, usize_ty.const_zero()).map_err(llvm_err)?;
        self.b.build_store(len, n).map_err(llvm_err)?;
        self.b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;

        self.b.position_at_end(none);
        self.b.build_conditional_branch(empty, eof, failed).map_err(llvm_err)?;
        self.b.position_at_end(eof);
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;
        self.b.position_at_end(failed);
        let code = self.last_error(true)?;
        self.b.build_store(err, code).map_err(llvm_err)?;
        self.b.build_return(Some(&i32_ty.const_int(0xFFFF_FFFF, false))).map_err(llvm_err)?;
        Ok(f)
    }

    /// `in_avail() -> isize`: the number of buffered bytes, after reading a block when the buffer is empty. 0 is the
    /// end of the input and -1 a read error, whose OS code `in_error` then reports.
    fn in_avail(&mut self, fill: FunctionValue<'ctx>, in_error: PointerValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (_, pos, len) = self.in_globals();
        let (i32_ty, usize_ty) = (self.i32(), self.usize_ty);
        let f = self.function("burdin_rt_in_avail", usize_ty.fn_type(&[], false));
        let ok = self.block(f, "ok");
        let end = self.block(f, "end");
        let error = self.block(f, "error");
        let check = self.block(f, "check");
        let err_slot = self.b.build_alloca(i32_ty, "in.err").map_err(llvm_err)?;
        let r = self.call(fill, &[err_slot.into()])?.expect("fill returns a status").into_int_value();
        let failed = self.b.build_int_compare(IntPredicate::SLT, r, i32_ty.const_zero(), "failed").map_err(llvm_err)?;
        self.b.build_conditional_branch(failed, error, check).map_err(llvm_err)?;
        self.b.position_at_end(check);
        let ended = self.b.build_int_compare(IntPredicate::EQ, r, i32_ty.const_zero(), "ended").map_err(llvm_err)?;
        self.b.build_conditional_branch(ended, end, ok).map_err(llvm_err)?;
        self.b.position_at_end(ok);
        let pos_v = self.b.build_load(usize_ty, pos, "pos").map_err(llvm_err)?.into_int_value();
        let len_v = self.b.build_load(usize_ty, len, "len").map_err(llvm_err)?.into_int_value();
        let count = self.b.build_int_sub(len_v, pos_v, "count").map_err(llvm_err)?;
        self.b.build_return(Some(&count)).map_err(llvm_err)?;
        self.b.position_at_end(end);
        self.b.build_return(Some(&usize_ty.const_zero())).map_err(llvm_err)?;
        self.b.position_at_end(error);
        let code = self.b.build_load(i32_ty, err_slot, "in.code").map_err(llvm_err)?.into_int_value();
        self.b.build_store(in_error, code).map_err(llvm_err)?;
        self.b.build_return(Some(&usize_ty.const_all_ones())).map_err(llvm_err)?;
        Ok(f)
    }

    /// `in_error() -> i32`: the OS code of the last standard-input read error.
    fn in_error(&mut self, global: PointerValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let i32_ty = self.i32();
        let f = self.function("burdin_rt_in_error", i32_ty.fn_type(&[], false));
        let code = self.b.build_load(i32_ty, global, "in.error").map_err(llvm_err)?;
        self.b.build_return(Some(&code)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `in_next() -> i8`: takes the next byte of the standard-input block. The caller has checked `in_avail`.
    fn in_next(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (buf, pos, _) = self.in_globals();
        let (usize_ty, i8_ty) = (self.usize_ty, self.i8());
        let f = self.function("burdin_rt_in_next", i8_ty.fn_type(&[], false));
        let pos_v = self.b.build_load(usize_ty, pos, "pos").map_err(llvm_err)?.into_int_value();
        let at = self.offset(buf, pos_v)?;
        let byte = self.b.build_load(i8_ty, at, "byte").map_err(llvm_err)?;
        let next = self.b.build_int_add(pos_v, usize_ty.const_int(1, false), "next").map_err(llvm_err)?;
        self.b.build_store(pos, next).map_err(llvm_err)?;
        self.b.build_return(Some(&byte)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `in_read_line(out: *String, err: *i32) -> i32`: appends one line of standard input to `out`, without its
    /// newline (and without the `\r` of a `\r\n` pair). 0 is a line (the last one may be unterminated), 1 is the
    /// end of input with nothing read, and -1 is a read error, whose OS code is stored in `err`.
    fn in_read_line(&mut self, push: FunctionValue<'ctx>, fill: FunctionValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (buf, pos, len) = self.in_globals();
        let (i32_ty, usize_ty, ptr_ty, i8_ty) = (self.i32(), self.usize_ty, self.ptr_ty, self.i8());
        let f = self.function("burdin_rt_in_read_line", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
        let out = Self::ptr_param(f, 0);
        let err = Self::ptr_param(f, 1);
        let start_slot = self.b.build_alloca(usize_ty, "start").map_err(llvm_err)?;
        let scan_slot = self.b.build_alloca(usize_ty, "scan").map_err(llvm_err)?;
        let lp = self.block(f, "loop");
        let init = self.block(f, "scan.init");
        let head = self.block(f, "scan.head");
        let body = self.block(f, "scan.body");
        let next = self.block(f, "scan.next");
        let found = self.block(f, "found");
        let strip = self.block(f, "strip");
        let stripped = self.block(f, "stripped");
        let line_done = self.block(f, "line.done");
        let rest = self.block(f, "rest");
        let need = self.block(f, "need");
        let filled = self.block(f, "filled");
        let eof = self.block(f, "eof");
        let eof_line = self.block(f, "eof.line");
        let eof_empty = self.block(f, "eof.empty");
        let error = self.block(f, "error");
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;

        self.b.position_at_end(lp);
        let pos_v = self.b.build_load(usize_ty, pos, "pos").map_err(llvm_err)?.into_int_value();
        let len_v = self.b.build_load(usize_ty, len, "len").map_err(llvm_err)?.into_int_value();
        let at_end = self.b.build_int_compare(IntPredicate::EQ, pos_v, len_v, "end").map_err(llvm_err)?;
        self.b.build_conditional_branch(at_end, need, init).map_err(llvm_err)?;

        self.b.position_at_end(init);
        let pos_v = self.b.build_load(usize_ty, pos, "pos").map_err(llvm_err)?.into_int_value();
        self.b.build_store(start_slot, pos_v).map_err(llvm_err)?;
        self.b.build_store(scan_slot, pos_v).map_err(llvm_err)?;
        self.b.build_unconditional_branch(head).map_err(llvm_err)?;

        self.b.position_at_end(head);
        let p = self.b.build_load(usize_ty, scan_slot, "p").map_err(llvm_err)?.into_int_value();
        let len_v = self.b.build_load(usize_ty, len, "len").map_err(llvm_err)?.into_int_value();
        let end = self.b.build_int_compare(IntPredicate::EQ, p, len_v, "end").map_err(llvm_err)?;
        self.b.build_conditional_branch(end, rest, body).map_err(llvm_err)?;

        self.b.position_at_end(body);
        let at = self.offset(buf, p)?;
        let byte = self.b.build_load(i8_ty, at, "byte").map_err(llvm_err)?.into_int_value();
        let newline =
            self.b.build_int_compare(IntPredicate::EQ, byte, i8_ty.const_int(10, false), "nl").map_err(llvm_err)?;
        self.b.build_conditional_branch(newline, found, next).map_err(llvm_err)?;

        self.b.position_at_end(next);
        let p2 = self.b.build_int_add(p, usize_ty.const_int(1, false), "next").map_err(llvm_err)?;
        self.b.build_store(scan_slot, p2).map_err(llvm_err)?;
        self.b.build_unconditional_branch(head).map_err(llvm_err)?;

        self.b.position_at_end(found);
        let start = self.b.build_load(usize_ty, start_slot, "start").map_err(llvm_err)?.into_int_value();
        let p = self.b.build_load(usize_ty, scan_slot, "p").map_err(llvm_err)?.into_int_value();
        let count = self.b.build_int_sub(p, start, "count").map_err(llvm_err)?;
        let src = self.offset(buf, start)?;
        self.call(push, &[out.into(), src.into(), count.into()])?;
        let after = self.b.build_int_add(p, usize_ty.const_int(1, false), "after").map_err(llvm_err)?;
        self.b.build_store(pos, after).map_err(llvm_err)?;
        // A `\r\n` terminator leaves a `\r` at the end of the line; drop it, as `lines()` does.
        let len_field = self.b.build_struct_gep(self.string_ty, out, 1, "line.len").map_err(llvm_err)?;
        let n = self.b.build_load(usize_ty, len_field, "n").map_err(llvm_err)?.into_int_value();
        let has = self.b.build_int_compare(IntPredicate::UGT, n, usize_ty.const_zero(), "has").map_err(llvm_err)?;
        self.b.build_conditional_branch(has, strip, line_done).map_err(llvm_err)?;

        self.b.position_at_end(strip);
        let n = self.b.build_load(usize_ty, len_field, "n").map_err(llvm_err)?.into_int_value();
        let last = self.b.build_int_sub(n, usize_ty.const_int(1, false), "last").map_err(llvm_err)?;
        let data_field = self.b.build_struct_gep(self.string_ty, out, 0, "line.ptr").map_err(llvm_err)?;
        let data = self.b.build_load(ptr_ty, data_field, "data").map_err(llvm_err)?.into_pointer_value();
        let last_at = self.offset(data, last)?;
        let c = self.b.build_load(i8_ty, last_at, "last.byte").map_err(llvm_err)?.into_int_value();
        let cr = self.b.build_int_compare(IntPredicate::EQ, c, i8_ty.const_int(13, false), "cr").map_err(llvm_err)?;
        self.b.build_conditional_branch(cr, stripped, line_done).map_err(llvm_err)?;

        self.b.position_at_end(stripped);
        self.b.build_store(len_field, last).map_err(llvm_err)?;
        self.b.build_unconditional_branch(line_done).map_err(llvm_err)?;

        self.b.position_at_end(line_done);
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;

        self.b.position_at_end(rest);
        let start = self.b.build_load(usize_ty, start_slot, "start").map_err(llvm_err)?.into_int_value();
        let len_v = self.b.build_load(usize_ty, len, "len").map_err(llvm_err)?.into_int_value();
        let count = self.b.build_int_sub(len_v, start, "count").map_err(llvm_err)?;
        let src = self.offset(buf, start)?;
        self.call(push, &[out.into(), src.into(), count.into()])?;
        self.b.build_store(pos, len_v).map_err(llvm_err)?;
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;

        self.b.position_at_end(need);
        let r = self.call(fill, &[err.into()])?.expect("fill returns a status").into_int_value();
        let failed = self.b.build_int_compare(IntPredicate::SLT, r, i32_ty.const_zero(), "failed").map_err(llvm_err)?;
        let ended = self.b.build_int_compare(IntPredicate::EQ, r, i32_ty.const_zero(), "ended").map_err(llvm_err)?;
        self.b.build_conditional_branch(failed, error, filled).map_err(llvm_err)?;

        self.b.position_at_end(filled);
        self.b.build_conditional_branch(ended, eof, lp).map_err(llvm_err)?;

        self.b.position_at_end(eof);
        let len_field = self.b.build_struct_gep(self.string_ty, out, 1, "line.len").map_err(llvm_err)?;
        let n = self.b.build_load(usize_ty, len_field, "n").map_err(llvm_err)?.into_int_value();
        let has = self.b.build_int_compare(IntPredicate::UGT, n, usize_ty.const_zero(), "has").map_err(llvm_err)?;
        self.b.build_conditional_branch(has, eof_line, eof_empty).map_err(llvm_err)?;

        self.b.position_at_end(eof_line);
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;
        self.b.position_at_end(eof_empty);
        self.b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;
        self.b.position_at_end(error);
        self.b.build_return(Some(&i32_ty.const_int(0xFFFF_FFFF, false))).map_err(llvm_err)?;
        Ok(f)
    }

    /// `in_read_to_string(out: *String, err: *i32) -> i32`: appends the rest of standard input to `out`. 0 is
    /// success and -1 a read error, whose OS code is stored in `err`.
    fn in_read_to_string(
        &mut self,
        push: FunctionValue<'ctx>,
        fill: FunctionValue<'ctx>,
    ) -> GenResult<FunctionValue<'ctx>> {
        let (buf, pos, len) = self.in_globals();
        let (i32_ty, usize_ty, ptr_ty) = (self.i32(), self.usize_ty, self.ptr_ty);
        let f = self.function("burdin_rt_in_read_to_string", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
        let out = Self::ptr_param(f, 0);
        let err = Self::ptr_param(f, 1);
        let lp = self.block(f, "loop");
        let append = self.block(f, "append");
        let need = self.block(f, "need");
        let filled = self.block(f, "filled");
        let done = self.block(f, "done");
        let error = self.block(f, "error");
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;

        self.b.position_at_end(lp);
        let pos_v = self.b.build_load(usize_ty, pos, "pos").map_err(llvm_err)?.into_int_value();
        let len_v = self.b.build_load(usize_ty, len, "len").map_err(llvm_err)?.into_int_value();
        let at_end = self.b.build_int_compare(IntPredicate::EQ, pos_v, len_v, "end").map_err(llvm_err)?;
        self.b.build_conditional_branch(at_end, need, append).map_err(llvm_err)?;

        self.b.position_at_end(append);
        let pos_v = self.b.build_load(usize_ty, pos, "pos").map_err(llvm_err)?.into_int_value();
        let len_v = self.b.build_load(usize_ty, len, "len").map_err(llvm_err)?.into_int_value();
        let count = self.b.build_int_sub(len_v, pos_v, "count").map_err(llvm_err)?;
        let src = self.offset(buf, pos_v)?;
        self.call(push, &[out.into(), src.into(), count.into()])?;
        self.b.build_store(pos, len_v).map_err(llvm_err)?;
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;

        self.b.position_at_end(need);
        let r = self.call(fill, &[err.into()])?.expect("fill returns a status").into_int_value();
        let failed = self.b.build_int_compare(IntPredicate::SLT, r, i32_ty.const_zero(), "failed").map_err(llvm_err)?;
        let ended = self.b.build_int_compare(IntPredicate::EQ, r, i32_ty.const_zero(), "ended").map_err(llvm_err)?;
        self.b.build_conditional_branch(failed, error, filled).map_err(llvm_err)?;
        self.b.position_at_end(filled);
        self.b.build_conditional_branch(ended, done, lp).map_err(llvm_err)?;

        self.b.position_at_end(done);
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;
        self.b.position_at_end(error);
        self.b.build_return(Some(&i32_ty.const_int(0xFFFF_FFFF, false))).map_err(llvm_err)?;
        Ok(f)
    }

    /// `digits(v: i64, end) -> ptr`: writes the decimal digits of `v` (unsigned) backwards, ending just before `end`,
    /// and returns the first digit. Writes at most 20 bytes.
    fn digits(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (i64_ty, ptr_ty) = (self.i64(), self.ptr_ty);
        let f = self.function("burdin_rt_digits", ptr_ty.fn_type(&[i64_ty.into(), ptr_ty.into()], false));
        let entry = self.b.get_insert_block().expect("positioned");
        let lp = self.block(f, "loop");
        let done = self.block(f, "done");
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;

        self.b.position_at_end(lp);
        let v = self.b.build_phi(i64_ty, "v").map_err(llvm_err)?;
        let end = self.b.build_phi(ptr_ty, "end").map_err(llvm_err)?;
        let v_v = v.as_basic_value().into_int_value();
        let at = self.offset(end.as_basic_value().into_pointer_value(), self.usize_ty.const_all_ones())?;
        let ten = i64_ty.const_int(10, false);
        let digit = self.b.build_int_unsigned_rem(v_v, ten, "").map_err(llvm_err)?;
        let digit = self.b.build_int_truncate(digit, self.i8(), "").map_err(llvm_err)?;
        let ch = self.b.build_int_add(digit, self.i8().const_int(u64::from(b'0'), false), "").map_err(llvm_err)?;
        self.b.build_store(at, ch).map_err(llvm_err)?;
        let rest = self.b.build_int_unsigned_div(v_v, ten, "").map_err(llvm_err)?;
        let more = self.b.build_int_compare(IntPredicate::NE, rest, i64_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(more, lp, done).map_err(llvm_err)?;
        v.add_incoming(&[(&Self::int_param(f, 0), entry), (&rest, lp)]);
        end.add_incoming(&[(&Self::ptr_param(f, 1), entry), (&at, lp)]);

        self.b.position_at_end(done);
        self.b.build_return(Some(&at)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `write_u64(fd: i32, v: i64)`: writes `v` (unsigned) in decimal.
    fn write_u64(
        &mut self,
        write_all: FunctionValue<'ctx>,
        digits: FunctionValue<'ctx>,
    ) -> GenResult<FunctionValue<'ctx>> {
        let void = self.cx.void_type();
        let f = self.function("burdin_rt_write_u64", void.fn_type(&[self.i32().into(), self.i64().into()], false));
        let buf = self.buffer(24)?;
        let end = self.offset(buf, self.usize_ty.const_int(24, false))?;
        let start = self.call(digits, &[Self::int_param(f, 1).into(), end.into()])?.expect("digits returns a pointer");
        let start = start.into_pointer_value();
        let len = self.distance(start, end)?;
        self.call(write_all, &[Self::int_param(f, 0).into(), start.into(), len.into()])?;
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `hex_digits(v: i64, end) -> ptr`: writes the hexadecimal digits of `v` backwards, ending just before `end`,
    /// and returns the first digit. Writes at most 16 bytes.
    fn hex_digits(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (i64_ty, ptr_ty, i8_ty) = (self.i64(), self.ptr_ty, self.i8());
        let f = self.function("burdin_rt_hex_digits", ptr_ty.fn_type(&[i64_ty.into(), ptr_ty.into()], false));
        let entry = self.b.get_insert_block().expect("positioned");
        let lp = self.block(f, "loop");
        let done = self.block(f, "done");
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;
        self.b.position_at_end(lp);
        let v = self.b.build_phi(i64_ty, "v").map_err(llvm_err)?;
        let end = self.b.build_phi(ptr_ty, "end").map_err(llvm_err)?;
        let v_v = v.as_basic_value().into_int_value();
        let at = self.offset(end.as_basic_value().into_pointer_value(), self.usize_ty.const_all_ones())?;
        let digit = self.b.build_and(v_v, i64_ty.const_int(15, false), "").map_err(llvm_err)?;
        let digit = self.b.build_int_truncate(digit, i8_ty, "").map_err(llvm_err)?;
        let ten = i8_ty.const_int(10, false);
        let below = self.b.build_int_compare(IntPredicate::ULT, digit, ten, "").map_err(llvm_err)?;
        let number = self.b.build_int_add(digit, i8_ty.const_int(u64::from(b'0'), false), "").map_err(llvm_err)?;
        let letter = self.b.build_int_add(digit, i8_ty.const_int(u64::from(b'a') - 10, false), "").map_err(llvm_err)?;
        let number_bv: BasicValueEnum<'ctx> = number.into();
        let letter_bv: BasicValueEnum<'ctx> = letter.into();
        let ch = self.b.build_select(below, number_bv, letter_bv, "ch").map_err(llvm_err)?.into_int_value();
        self.b.build_store(at, ch).map_err(llvm_err)?;
        let rest = self.b.build_right_shift(v_v, i64_ty.const_int(4, false), false, "").map_err(llvm_err)?;
        let more = self.b.build_int_compare(IntPredicate::NE, rest, i64_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(more, lp, done).map_err(llvm_err)?;
        v.add_incoming(&[(&Self::int_param(f, 0), entry), (&rest, lp)]);
        end.add_incoming(&[(&Self::ptr_param(f, 1), entry), (&at, lp)]);
        self.b.position_at_end(done);
        self.b.build_return(Some(&at)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `write_hex(fd: i32, v: i64)`: writes `v` (unsigned) in hexadecimal, on Windows only.
    fn write_hex(
        &mut self,
        write_all: FunctionValue<'ctx>,
        hex_digits: FunctionValue<'ctx>,
    ) -> GenResult<FunctionValue<'ctx>> {
        let void = self.cx.void_type();
        let f = self.function("burdin_rt_write_hex", void.fn_type(&[self.i32().into(), self.i64().into()], false));
        let buf = self.buffer(20)?;
        let end = self.offset(buf, self.usize_ty.const_int(20, false))?;
        let start =
            self.call(hex_digits, &[Self::int_param(f, 1).into(), end.into()])?.expect("hex_digits returns a pointer");
        let start = start.into_pointer_value();
        let len = self.distance(start, end)?;
        self.call(write_all, &[Self::int_param(f, 0).into(), start.into(), len.into()])?;
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    // ----- panics ----------------------------------------------------------------------

    /// `die()`: flushes standard output, then aborts the process, without the Windows "abnormal program
    /// termination" message.
    fn die(&mut self, flush: FunctionValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let void = self.cx.void_type();
        let abort = libc(self.module, "abort", void.fn_type(&[], false));
        self.mark_noreturn(abort);
        let f = self.function("burdin_rt_die", void.fn_type(&[], false));
        self.mark_noreturn(f);
        self.call(flush, &[])?;
        if self.windows {
            let i32_ty = self.i32();
            let set = libc(self.module, "_set_abort_behavior", i32_ty.fn_type(&[i32_ty.into(), i32_ty.into()], false));
            const WRITE_ABORT_MSG_AND_CALL_REPORTFAULT: u64 = 0x1 | 0x2;
            let args =
                [i32_ty.const_zero().into(), i32_ty.const_int(WRITE_ABORT_MSG_AND_CALL_REPORTFAULT, false).into()];
            self.call(set, &args)?;
        }
        self.call(abort, &[])?;
        self.b.build_unreachable().map_err(llvm_err)?;
        Ok(f)
    }

    /// `burdin_panic_begin()`: the stable entry symbol every panic path calls before it prints or aborts
    /// (`spec/debug.md`, DBG1), so `break burdin_panic_begin` stops with the full stack. It is externally visible
    /// (the linker and `globaldce` keep it even in a program that cannot panic) and empty: it is a breakpoint, not
    /// code, and `noinline` keeps it from being folded into the caller.
    fn panic_begin(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let f = self.module.add_function(
            "burdin_panic_begin",
            self.cx.void_type().fn_type(&[], false),
            Some(Linkage::External),
        );
        let noinline = self.cx.create_enum_attribute(Attribute::get_named_enum_kind_id("noinline"), 0);
        f.add_attribute(AttributeLoc::Function, noinline);
        self.b.position_at_end(self.cx.append_basic_block(f, "entry"));
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `panic_start(file, file_len: usize, line: i32, col: i32)`: flushes standard output, then writes
    /// `path:LINE:COL: panic: ` to stderr.
    fn panic_start(
        &mut self,
        write_all: FunctionValue<'ctx>,
        write_u64: FunctionValue<'ctx>,
        flush: FunctionValue<'ctx>,
    ) -> GenResult<FunctionValue<'ctx>> {
        let void = self.cx.void_type();
        let (i32_ty, i64_ty) = (self.i32(), self.i64());
        let ty = void.fn_type(&[self.ptr_ty.into(), self.usize_ty.into(), i32_ty.into(), i32_ty.into()], false);
        let f = self.function("burdin_rt_panic_start", ty);
        self.call(flush, &[])?;
        let stderr = i32_ty.const_int(2, false);
        self.call(write_all, &[stderr.into(), Self::ptr_param(f, 0).into(), Self::int_param(f, 1).into()])?;
        self.write_const(write_all, 2, b":")?;
        let line = self.b.build_int_z_extend(Self::int_param(f, 2), i64_ty, "").map_err(llvm_err)?;
        self.call(write_u64, &[stderr.into(), line.into()])?;
        self.write_const(write_all, 2, b":")?;
        let col = self.b.build_int_z_extend(Self::int_param(f, 3), i64_ty, "").map_err(llvm_err)?;
        self.call(write_u64, &[stderr.into(), col.into()])?;
        self.write_const(write_all, 2, b": panic: ")?;
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// The parameter types shared by the panic functions after their first two.
    fn location_params(&self) -> [BasicMetadataTypeEnum<'ctx>; 4] {
        [self.ptr_ty.into(), self.usize_ty.into(), self.i32().into(), self.i32().into()]
    }

    /// Calls `panic_start` with parameters 2..6 of `f`.
    fn start_panic(&self, h: &Helpers<'ctx>, f: FunctionValue<'ctx>) -> GenResult<()> {
        let args: Vec<BasicMetadataValueEnum<'ctx>> =
            (2..6).map(|i| f.get_nth_param(i).expect("panic functions have 6 parameters").into()).collect();
        self.call(h.panic_start, &args)?;
        Ok(())
    }

    /// `panic(msg, msg_len: usize, file, file_len: usize, line: i32, col: i32)`
    fn panic(&mut self, h: &Helpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let params = [&[self.ptr_ty.into(), self.usize_ty.into()][..], &self.location_params()[..]].concat();
        let f = self.function("burdin_rt_panic", self.cx.void_type().fn_type(&params, false));
        self.mark_noreturn(f);
        self.call(h.panic_begin, &[])?;
        self.start_panic(h, f)?;
        let stderr = self.i32().const_int(2, false);
        self.call(h.write_all, &[stderr.into(), Self::ptr_param(f, 0).into(), Self::int_param(f, 1).into()])?;
        self.write_const(h.write_all, 2, b"\n")?;
        self.call(h.print_backtrace, &[])?;
        self.call(h.die, &[])?;
        self.b.build_unreachable().map_err(llvm_err)?;
        Ok(f)
    }

    /// `panic_bounds(index: i64, len: i64, file, file_len: usize, line: i32, col: i32)`
    fn panic_bounds(&mut self, h: &Helpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let params = [&[self.i64().into(), self.i64().into()][..], &self.location_params()[..]].concat();
        let f = self.function("burdin_rt_panic_bounds", self.cx.void_type().fn_type(&params, false));
        self.mark_noreturn(f);
        self.call(h.panic_begin, &[])?;
        self.start_panic(h, f)?;
        let stderr = self.i32().const_int(2, false);
        self.write_const(h.write_all, 2, b"index out of bounds: the index is ")?;
        self.call(h.write_u64, &[stderr.into(), Self::int_param(f, 0).into()])?;
        self.write_const(h.write_all, 2, b" but the length is ")?;
        self.call(h.write_u64, &[stderr.into(), Self::int_param(f, 1).into()])?;
        self.write_const(h.write_all, 2, b"\n")?;
        self.call(h.print_backtrace, &[])?;
        self.call(h.die, &[])?;
        self.b.build_unreachable().map_err(llvm_err)?;
        Ok(f)
    }

    // ----- backtraces and the stack guard ----------------------------------------------

    /// The frame table entry the code generator emits with `-g` (`spec/debug.md`, DBG2): the function's address,
    /// its name and its source file as pointer-and-length pairs, the declaration line, and whether the backtrace
    /// stops after this frame (the process entry point, above which only the C runtime and libc are left, whose
    /// addresses the table would misattribute). `emit/frames.rs` builds the same struct; the two definitions must
    /// stay in sync.
    fn frame_entry_ty(&self) -> StructType<'ctx> {
        let (ptr_ty, usize_ty, i32_ty, i1_ty) = (self.ptr_ty, self.usize_ty, self.i32(), self.cx.bool_type());
        self.cx.struct_type(
            &[
                ptr_ty.into(),
                ptr_ty.into(),
                usize_ty.into(),
                ptr_ty.into(),
                usize_ty.into(),
                i32_ty.into(),
                i1_ty.into(),
            ],
            false,
        )
    }

    /// `print_backtrace()`: writes `stack backtrace:` and the frames above the panic runtime to stderr. The first
    /// two frames (this function and `burdin_rt_panic`) are hidden; on Windows the capture skips three. A frame the
    /// `-g` table names prints `  #N  at <file>:<line> in <name>`; every other frame prints its raw address.
    fn print_backtrace(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let void = self.cx.void_type();
        let f = self.function("burdin_rt_print_backtrace", void.fn_type(&[], false));
        if self.windows {
            self.print_backtrace_windows(f)?;
        } else {
            self.print_backtrace_unix(f)?;
        }
        Ok(f)
    }

    /// One line of the backtrace: `  #index` and then either the symbolized entry or the raw address. The entry
    /// point's own frame (marked `stop`) is not printed: it is the C wrapper above the program's `main`, and the
    /// trace ends after the frame before it.
    fn print_frame_at(
        &mut self,
        f: FunctionValue<'ctx>,
        info: PointerValue<'ctx>,
        index: IntValue<'ctx>,
        addr: IntValue<'ctx>,
        continue_bb: BasicBlock<'ctx>,
        stop_bb: BasicBlock<'ctx>,
    ) -> GenResult<()> {
        let write_all = self.module.get_function("burdin_rt_write_all").expect("write_all exists");
        let write_u64 = self.module.get_function("burdin_rt_write_u64").expect("write_u64 exists");
        let write_hex = self.module.get_function("burdin_rt_write_hex").expect("write_hex exists");
        let (i64_ty, i32_ty) = (self.i64(), self.i32());
        let entry = self.frame_entry_ty();
        let stderr = i32_ty.const_int(2, false);
        // A frame the table does not know (every frame without `-g`) answers `null`: it prints its raw address and
        // has no `stop` flag to read. The entry point is not printed; it ends the trace.
        let info_i = self.b.build_ptr_to_int(info, i64_ty, "").map_err(llvm_err)?;
        let known =
            self.b.build_int_compare(IntPredicate::NE, info_i, i64_ty.const_zero(), "known").map_err(llvm_err)?;
        let check_stop = self.block(f, "frame.check.stop");
        let line_bb = self.block(f, "frame.line");
        self.b.build_conditional_branch(known, check_stop, line_bb).map_err(llvm_err)?;
        self.b.position_at_end(check_stop);
        let stop_ptr = self.b.build_struct_gep(entry, info, 6, "stop").map_err(llvm_err)?;
        let stop = self.b.build_load(self.cx.bool_type(), stop_ptr, "").map_err(llvm_err)?.into_int_value();
        let is_stop = self
            .b
            .build_int_compare(IntPredicate::NE, stop, self.cx.bool_type().const_zero(), "is.stop")
            .map_err(llvm_err)?;
        self.b.build_conditional_branch(is_stop, stop_bb, line_bb).map_err(llvm_err)?;
        self.b.position_at_end(line_bb);
        self.write_const(write_all, 2, b"  #")?;
        let index64 = self.b.build_int_z_extend(index, i64_ty, "").map_err(llvm_err)?;
        self.call(write_u64, &[stderr.into(), index64.into()])?;
        let sym = self.block(f, "frame.sym");
        let raw = self.block(f, "frame.raw");
        let done = self.block(f, "frame.done");
        self.b.build_conditional_branch(known, sym, raw).map_err(llvm_err)?;
        self.b.position_at_end(sym);
        let name_ptr = self.b.build_struct_gep(entry, info, 1, "name").map_err(llvm_err)?;
        let name = self.b.build_load(self.ptr_ty, name_ptr, "").map_err(llvm_err)?.into_pointer_value();
        let name_len = self.b.build_struct_gep(entry, info, 2, "name.len").map_err(llvm_err)?;
        let name_len = self.b.build_load(self.usize_ty, name_len, "").map_err(llvm_err)?.into_int_value();
        let file_ptr = self.b.build_struct_gep(entry, info, 3, "file").map_err(llvm_err)?;
        let file = self.b.build_load(self.ptr_ty, file_ptr, "").map_err(llvm_err)?.into_pointer_value();
        let file_len = self.b.build_struct_gep(entry, info, 4, "file.len").map_err(llvm_err)?;
        let file_len = self.b.build_load(self.usize_ty, file_len, "").map_err(llvm_err)?.into_int_value();
        let line_ptr = self.b.build_struct_gep(entry, info, 5, "line").map_err(llvm_err)?;
        let line = self.b.build_load(i32_ty, line_ptr, "").map_err(llvm_err)?.into_int_value();
        self.write_const(write_all, 2, b"  at ")?;
        self.call(write_all, &[stderr.into(), file.into(), file_len.into()])?;
        self.write_const(write_all, 2, b":")?;
        let line64 = self.b.build_int_z_extend(line, i64_ty, "").map_err(llvm_err)?;
        self.call(write_u64, &[stderr.into(), line64.into()])?;
        self.write_const(write_all, 2, b" in ")?;
        self.call(write_all, &[stderr.into(), name.into(), name_len.into()])?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(raw);
        self.write_const(write_all, 2, b"  0x")?;
        self.call(write_hex, &[stderr.into(), addr.into()])?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(done);
        self.write_const(write_all, 2, b"\n")?;
        self.b.build_unconditional_branch(continue_bb).map_err(llvm_err)?;
        Ok(())
    }

    fn print_backtrace_unix(&mut self, f: FunctionValue<'ctx>) -> GenResult<()> {
        let (i32_ty, ptr_ty) = (self.i32(), self.ptr_ty);
        let backtrace = libc(self.module, "backtrace", i32_ty.fn_type(&[ptr_ty.into(), i32_ty.into()], false));
        let symbols = libc(
            self.module,
            "backtrace_symbols_fd",
            self.cx.void_type().fn_type(&[ptr_ty.into(), i32_ty.into(), i32_ty.into()], false),
        );
        // The frame table lookup the code generator defines (`spec/debug.md`, DBG2): `null` when the binary was
        // built without `-g`, which is what keeps the trace below byte-identical to the pre-table format.
        let frame_info = self.module.add_function(
            "burdin_rt_frame_info",
            self.ptr_ty.fn_type(&[self.i64().into()], false),
            Some(Linkage::External),
        );
        let write_all = self.module.get_function("burdin_rt_write_all").expect("write_all exists");
        let buf = self.b.build_alloca(ptr_ty.array_type(BACKTRACE_FRAMES), "frames").map_err(llvm_err)?;
        let n = self
            .call(backtrace, &[buf.into(), i32_ty.const_int(u64::from(BACKTRACE_FRAMES), false).into()])?
            .expect("backtrace returns a count")
            .into_int_value();
        let hidden = i32_ty.const_int(2, false);
        let enough = self.b.build_int_compare(IntPredicate::SGT, n, hidden, "enough").map_err(llvm_err)?;
        let show = self.block(f, "show");
        let table = self.block(f, "table");
        let legacy = self.block(f, "legacy");
        let done = self.block(f, "done");
        self.b.build_conditional_branch(enough, show, done).map_err(llvm_err)?;
        self.b.position_at_end(show);
        self.write_const(write_all, 2, b"stack backtrace:\n")?;
        // Probe the first visible frame: when the table names it, every frame goes through the same printer.
        let first = unsafe { self.b.build_in_bounds_gep(ptr_ty, buf, &[hidden], "first") }.map_err(llvm_err)?;
        let first = self.b.build_load(ptr_ty, first, "").map_err(llvm_err)?.into_pointer_value();
        let first = self.b.build_ptr_to_int(first, self.i64(), "first").map_err(llvm_err)?;
        let info = self.call(frame_info, &[first.into()])?.expect("the lookup returns an entry pointer");
        let info_i = self.b.build_ptr_to_int(info.into_pointer_value(), self.i64(), "").map_err(llvm_err)?;
        let has =
            self.b.build_int_compare(IntPredicate::NE, info_i, self.i64().const_zero(), "known").map_err(llvm_err)?;
        self.b.build_conditional_branch(has, table, legacy).map_err(llvm_err)?;
        self.b.position_at_end(table);
        let index = self.b.build_alloca(i32_ty, "i").map_err(llvm_err)?;
        self.b.build_store(index, hidden).map_err(llvm_err)?;
        let header = self.block(f, "frame.header");
        let body = self.block(f, "frame.body");
        let latch = self.block(f, "frame.next");
        self.b.build_unconditional_branch(header).map_err(llvm_err)?;
        self.b.position_at_end(header);
        let i = self.b.build_load(i32_ty, index, "i").map_err(llvm_err)?.into_int_value();
        let more = self.b.build_int_compare(IntPredicate::SLT, i, n, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(more, body, done).map_err(llvm_err)?;
        self.b.position_at_end(body);
        let frame = unsafe { self.b.build_in_bounds_gep(ptr_ty, buf, &[i], "frame") }.map_err(llvm_err)?;
        let frame = self.b.build_load(ptr_ty, frame, "").map_err(llvm_err)?.into_pointer_value();
        let addr = self.b.build_ptr_to_int(frame, self.i64(), "addr").map_err(llvm_err)?;
        let info = self.call(frame_info, &[addr.into()])?.expect("the lookup returns an entry pointer");
        let shown = self.b.build_int_sub(i, hidden, "").map_err(llvm_err)?;
        self.print_frame_at(f, info.into_pointer_value(), shown, addr, latch, done)?;
        self.b.position_at_end(latch);
        let next = self.b.build_int_add(i, i32_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_store(index, next).map_err(llvm_err)?;
        self.b.build_unconditional_branch(header).map_err(llvm_err)?;
        self.b.position_at_end(legacy);
        // No frame table (no `-g`): exactly the stage 0 trace.
        let start = unsafe { self.b.build_in_bounds_gep(ptr_ty, buf, &[hidden], "start") }.map_err(llvm_err)?;
        let count = self.b.build_int_sub(n, hidden, "count").map_err(llvm_err)?;
        self.call(symbols, &[start.into(), count.into(), i32_ty.const_int(2, false).into()])?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(done);
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(())
    }

    fn print_backtrace_windows(&mut self, f: FunctionValue<'ctx>) -> GenResult<()> {
        let (i32_ty, ptr_ty, i64_ty) = (self.i32(), self.ptr_ty, self.i64());
        let capture = libc(
            self.module,
            "RtlCaptureStackBackTrace",
            self.cx.i16_type().fn_type(&[i32_ty.into(), i32_ty.into(), ptr_ty.into(), ptr_ty.into()], false),
        );
        let frame_info = self.module.add_function(
            "burdin_rt_frame_info",
            self.ptr_ty.fn_type(&[self.i64().into()], false),
            Some(Linkage::External),
        );
        let write_all = self.module.get_function("burdin_rt_write_all").expect("write_all exists");
        let buf = self.b.build_alloca(ptr_ty.array_type(BACKTRACE_FRAMES), "frames").map_err(llvm_err)?;
        // Skip `RtlCaptureStackBackTrace`, this function, and `burdin_rt_panic`; the runtime frames are hidden
        // like on Unix.
        let hidden = i32_ty.const_int(3, false);
        let n = self
            .call(
                capture,
                &[
                    hidden.into(),
                    i32_ty.const_int(u64::from(BACKTRACE_FRAMES), false).into(),
                    buf.into(),
                    ptr_ty.const_null().into(),
                ],
            )?
            .expect("RtlCaptureStackBackTrace returns a count")
            .into_int_value();
        let n = self.b.build_int_z_extend(n, i32_ty, "n").map_err(llvm_err)?;
        let enough = self.b.build_int_compare(IntPredicate::SGT, n, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let show = self.block(f, "show");
        let done = self.block(f, "done");
        self.b.build_conditional_branch(enough, show, done).map_err(llvm_err)?;
        self.b.position_at_end(show);
        self.write_const(write_all, 2, b"stack backtrace:\n")?;
        // Without `-g` the lookup answers `null` and every frame falls to its raw address, which is the stage 0
        // Windows trace; there is nothing else to preserve.
        let zero = i32_ty.const_zero();
        let index = self.b.build_alloca(i32_ty, "index").map_err(llvm_err)?;
        self.b.build_store(index, zero).map_err(llvm_err)?;
        let header = self.block(f, "frame.header");
        let body = self.block(f, "frame.body");
        let latch = self.block(f, "frame.next");
        self.b.build_unconditional_branch(header).map_err(llvm_err)?;
        self.b.position_at_end(header);
        let i = self.b.build_load(i32_ty, index, "i").map_err(llvm_err)?.into_int_value();
        let more = self.b.build_int_compare(IntPredicate::SLT, i, n, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(more, body, done).map_err(llvm_err)?;
        self.b.position_at_end(body);
        let frame = unsafe { self.b.build_in_bounds_gep(ptr_ty, buf, &[i], "frame") }.map_err(llvm_err)?;
        let frame = self.b.build_load(ptr_ty, frame, "").map_err(llvm_err)?.into_pointer_value();
        let addr = self.b.build_ptr_to_int(frame, i64_ty, "addr").map_err(llvm_err)?;
        let info = self.call(frame_info, &[addr.into()])?.expect("the lookup returns an entry pointer");
        self.print_frame_at(f, info.into_pointer_value(), i, addr, latch, done)?;
        self.b.position_at_end(latch);
        let next = self.b.build_int_add(i, i32_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_store(index, next).map_err(llvm_err)?;
        self.b.build_unconditional_branch(header).map_err(llvm_err)?;
        self.b.position_at_end(done);
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(())
    }

    /// `install_stack_guard()`: makes the main thread's stack overflow a clear panic instead of a raw fault.
    fn install_stack_guard(
        &mut self,
        flush: FunctionValue<'ctx>,
        panic_begin: FunctionValue<'ctx>,
    ) -> GenResult<FunctionValue<'ctx>> {
        let void = self.cx.void_type();
        let write_all = self.module.get_function("burdin_rt_write_all").expect("write_all exists");
        let in_handler = self.module.add_global(self.i32(), None, "burdin_rt_in_handler");
        in_handler.set_initializer(&self.i32().const_zero());
        in_handler.set_linkage(Linkage::Internal);
        let handler_ty = if self.windows {
            self.i32().fn_type(&[self.ptr_ty.into()], false)
        } else {
            // A `SA_SIGINFO` handler: the fault address in `siginfo_t` tells a stack overflow from another fault.
            void.fn_type(&[self.i32().into(), self.ptr_ty.into(), self.ptr_ty.into()], false)
        };
        let handler = self.module.add_function("burdin_rt_stack_overflow", handler_ty, Some(Linkage::Internal));
        let f = self.function("burdin_rt_install_stack_guard", void.fn_type(&[], false));
        if self.windows {
            self.install_stack_guard_windows(handler)?;
        } else {
            self.install_stack_guard_unix(handler)?;
        }
        // The handler itself: print a stable message and leave without running any drops, like a panic. The
        // Windows filter sees every exception, so it first makes sure this is a stack overflow.
        self.b.position_at_end(self.cx.append_basic_block(handler, "entry"));
        let overflow = if self.windows {
            let record = self
                .b
                .build_load(self.ptr_ty, Self::ptr_param(handler, 0), "record")
                .map_err(llvm_err)?
                .into_pointer_value();
            let code = self.b.build_load(self.i32(), record, "code").map_err(llvm_err)?.into_int_value();
            Some(
                self.b
                    .build_int_compare(
                        IntPredicate::EQ,
                        code,
                        self.i32().const_int(0xC000_00FD, false),
                        "stack_overflow",
                    )
                    .map_err(llvm_err)?,
            )
        } else {
            // A fault inside the guard region below the main thread's stack is a stack overflow; anything else
            // (a null `CPtr`, a bug) gets its own message.
            let info = Self::ptr_param(handler, 1);
            let offset = if self.darwin { 24 } else { 16 };
            let addr_ptr = self.offset(info, self.usize_ty.const_int(offset, false))?;
            let addr = self.b.build_load(self.ptr_ty, addr_ptr, "fault").map_err(llvm_err)?.into_pointer_value();
            let addr = self.b.build_ptr_to_int(addr, self.usize_ty, "addr").map_err(llvm_err)?;
            let lo = self
                .module
                .get_global("burdin_rt_stack_lo")
                .map(|g| g.as_pointer_value())
                .expect("the guard installs the stack bounds");
            let lo = self.b.build_load(self.usize_ty, lo, "stack_lo").map_err(llvm_err)?.into_int_value();
            let margin = self.usize_ty.const_int(1 << 20, false);
            let low = self.b.build_int_sub(lo, margin, "low").map_err(llvm_err)?;
            let above = self.b.build_int_compare(IntPredicate::UGE, addr, low, "").map_err(llvm_err)?;
            let below = self.b.build_int_compare(IntPredicate::ULT, addr, lo, "").map_err(llvm_err)?;
            let main_overflow = self.b.build_and(above, below, "main_overflow").map_err(llvm_err)?;
            // A task runs on its own stack, with an inaccessible guard region below it. The context switch
            // records where the running stack starts (`taskrt.rs`), so a fault in that region is an overflow too.
            let cur = self
                .module
                .get_global("burdin_rt_cur_lo")
                .map(|g| g.as_pointer_value())
                .expect("the guard defines the running stack's bounds");
            let cur = self.b.build_load(self.usize_ty, cur, "cur_lo").map_err(llvm_err)?.into_int_value();
            let in_task = self
                .b
                .build_int_compare(IntPredicate::NE, cur, self.usize_ty.const_zero(), "in_task")
                .map_err(llvm_err)?;
            let guard = self.usize_ty.const_int(crate::taskrt::GUARD, false);
            let guard_low = self.b.build_int_sub(cur, guard, "guard_low").map_err(llvm_err)?;
            let above = self.b.build_int_compare(IntPredicate::UGE, addr, guard_low, "").map_err(llvm_err)?;
            let below = self.b.build_int_compare(IntPredicate::ULT, addr, cur, "").map_err(llvm_err)?;
            let in_guard = self.b.build_and(above, below, "").map_err(llvm_err)?;
            let task_overflow = self.b.build_and(in_task, in_guard, "task_overflow").map_err(llvm_err)?;
            Some(self.b.build_or(main_overflow, task_overflow, "stack_overflow").map_err(llvm_err)?)
        };
        let guard = in_handler.as_pointer_value();
        let check = self.block(handler, "check");
        let first = self.block(handler, "first");
        let nested = self.block(handler, "nested");
        let exit = self.exit_function();
        match overflow {
            Some(overflow) if self.windows => {
                // A vectored filter sees every exception: anything that is not a stack overflow goes on to the
                // operating system's own handling.
                let pass = self.block(handler, "pass");
                self.b.build_conditional_branch(overflow, check, pass).map_err(llvm_err)?;
                self.b.position_at_end(pass);
                self.b.build_return(Some(&self.i32().const_zero())).map_err(llvm_err)?;
            }
            Some(overflow) => {
                let other = self.block(handler, "other");
                self.b.build_conditional_branch(overflow, check, other).map_err(llvm_err)?;
                self.b.position_at_end(other);
                self.call(panic_begin, &[])?;
                self.call(flush, &[])?;
                self.write_const(write_all, 2, b"panic: invalid memory access\n")?;
                self.call(exit, &[self.i32().const_int(134, false).into()])?;
                self.b.build_unreachable().map_err(llvm_err)?;
            }
            None => {
                self.b.build_unconditional_branch(check).map_err(llvm_err)?;
            }
        }
        self.b.position_at_end(check);
        let seen = self.b.build_load(self.i32(), guard, "seen").map_err(llvm_err)?.into_int_value();
        let again = self
            .b
            .build_int_compare(IntPredicate::NE, seen, self.i32().const_zero(), "in_handler")
            .map_err(llvm_err)?;
        self.b.build_conditional_branch(again, nested, first).map_err(llvm_err)?;
        self.b.position_at_end(first);
        self.b.build_store(guard, self.i32().const_int(1, false)).map_err(llvm_err)?;
        self.call(panic_begin, &[])?;
        self.call(flush, &[])?;
        self.write_const(write_all, 2, b"panic: stack overflow\n")?;
        self.call(exit, &[self.i32().const_int(134, false).into()])?;
        self.b.build_unreachable().map_err(llvm_err)?;
        // Reentered because the handler itself faulted: leave without trying to print again.
        self.b.position_at_end(nested);
        self.call(exit, &[self.i32().const_int(134, false).into()])?;
        self.b.build_unreachable().map_err(llvm_err)?;
        Ok(f)
    }

    fn install_stack_guard_unix(&mut self, handler: FunctionValue<'ctx>) -> GenResult<()> {
        let (i32_ty, usize_ty, ptr_ty) = (self.i32(), self.usize_ty, self.ptr_ty);
        let memset =
            libc(self.module, "memset", ptr_ty.fn_type(&[ptr_ty.into(), i32_ty.into(), usize_ty.into()], false));
        let lo_global = self.module.add_global(usize_ty, None, "burdin_rt_stack_lo");
        lo_global.set_initializer(&usize_ty.const_zero());
        lo_global.set_linkage(Linkage::Internal);
        // The low end of the usable stack a thread is running a task on, or 0 (`taskrt.rs`). It is external so the
        // task runtime, linked in later, shares it.
        let cur_lo = self.module.add_global(usize_ty, None, "burdin_rt_cur_lo");
        cur_lo.set_initializer(&usize_ty.const_zero());
        cur_lo.set_thread_local(true);
        // The bounds of the main thread's stack tell the handler whether a fault is a stack overflow.
        let pthread_self = libc(self.module, "pthread_self", ptr_ty.fn_type(&[], false));
        let thread = self.call(pthread_self, &[])?.expect("pthread_self returns a thread").into_pointer_value();
        if self.darwin {
            let top_of = libc(self.module, "pthread_get_stackaddr_np", ptr_ty.fn_type(&[ptr_ty.into()], false));
            let size_of = libc(self.module, "pthread_get_stacksize_np", usize_ty.fn_type(&[ptr_ty.into()], false));
            let top = self.call(top_of, &[thread.into()])?.expect("a stack address").into_pointer_value();
            let size = self.call(size_of, &[thread.into()])?.expect("a stack size").into_int_value();
            let top = self.b.build_ptr_to_int(top, usize_ty, "stack_top").map_err(llvm_err)?;
            let lo = self.b.build_int_sub(top, size, "stack_lo").map_err(llvm_err)?;
            self.b.build_store(lo_global.as_pointer_value(), lo).map_err(llvm_err)?;
        } else {
            let getattr =
                libc(self.module, "pthread_getattr_np", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
            let attr = self.b.build_alloca(usize_ty.array_type(16), "attr").map_err(llvm_err)?;
            self.call(memset, &[attr.into(), i32_ty.const_zero().into(), usize_ty.const_int(128, false).into()])?;
            self.call(getattr, &[thread.into(), attr.into()])?;
            let lo_slot = self.b.build_alloca(usize_ty, "lo_slot").map_err(llvm_err)?;
            let size_slot = self.b.build_alloca(usize_ty, "size_slot").map_err(llvm_err)?;
            let getstack = libc(
                self.module,
                "pthread_attr_getstack",
                i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into(), ptr_ty.into()], false),
            );
            self.call(getstack, &[attr.into(), lo_slot.into(), size_slot.into()])?;
            let destroy = libc(self.module, "pthread_attr_destroy", i32_ty.fn_type(&[ptr_ty.into()], false));
            self.call(destroy, &[attr.into()])?;
            let lo = self.b.build_load(usize_ty, lo_slot, "stack_lo").map_err(llvm_err)?.into_int_value();
            let size = self.b.build_load(usize_ty, size_slot, "stack_size").map_err(llvm_err)?.into_int_value();
            self.b.build_store(lo_global.as_pointer_value(), lo).map_err(llvm_err)?;
            let _ = size;
        }
        let sigaltstack = libc(self.module, "sigaltstack", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
        let sigaction =
            libc(self.module, "sigaction", i32_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), ptr_ty.into()], false));
        let alt = self.module.add_global(self.cx.i8_type().array_type(ALT_STACK), None, "burdin_rt_altstack");
        alt.set_initializer(&self.cx.i8_type().array_type(ALT_STACK).const_zero());
        alt.set_linkage(Linkage::Internal);
        let (sp_offset, flags_offset, size_offset) = if self.darwin { (0u64, 16u64, 8u64) } else { (0, 8, 16) };
        // stack_t: three pointer-sized fields, in the platform's order.
        let st = self.b.build_alloca(self.usize_ty.array_type(3), "stack_t").map_err(llvm_err)?;
        self.call(memset, &[st.into(), i32_ty.const_zero().into(), usize_ty.const_int(24, false).into()])?;
        let sp = self.offset(st, usize_ty.const_int(sp_offset, false))?;
        self.b.build_store(sp, alt.as_pointer_value()).map_err(llvm_err)?;
        let size = self.offset(st, usize_ty.const_int(size_offset, false))?;
        self.b.build_store(size, usize_ty.const_int(u64::from(ALT_STACK), false)).map_err(llvm_err)?;
        let flags = self.offset(st, usize_ty.const_int(flags_offset, false))?;
        self.b.build_store(flags, i32_ty.const_zero()).map_err(llvm_err)?;
        self.call(sigaltstack, &[st.into(), ptr_ty.const_null().into()])?;

        let (sa_flags_offset, onstack, reset, siginfo) =
            if self.darwin { (12u64, 0x0001u64, 0x0004u64, 0x0040u64) } else { (136, 0x0800_0000, 0x8000_0000, 0x4) };
        // struct sigaction: zeroed, with the handler and SA_ONSTACK | SA_RESETHAND set at the platform's offsets.
        let act = self.b.build_alloca(self.usize_ty.array_type(19), "sigaction").map_err(llvm_err)?;
        self.call(memset, &[act.into(), i32_ty.const_zero().into(), usize_ty.const_int(152, false).into()])?;
        self.b.build_store(act, handler.as_global_value().as_pointer_value()).map_err(llvm_err)?;
        let sa_flags = self.offset(act, usize_ty.const_int(sa_flags_offset, false))?;
        self.b.build_store(sa_flags, i32_ty.const_int(onstack | reset | siginfo, false)).map_err(llvm_err)?;
        const SIGSEGV: u64 = 11;
        self.call(sigaction, &[i32_ty.const_int(SIGSEGV, false).into(), act.into(), ptr_ty.const_null().into()])?;
        if self.darwin {
            // macOS reports an access to a mapped but inaccessible page, such as the guard below a task stack, as
            // `SIGBUS` rather than `SIGSEGV`.
            const SIGBUS_DARWIN: u64 = 10;
            self.call(
                sigaction,
                &[i32_ty.const_int(SIGBUS_DARWIN, false).into(), act.into(), ptr_ty.const_null().into()],
            )?;
        }
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(())
    }

    fn install_stack_guard_windows(&mut self, handler: FunctionValue<'ctx>) -> GenResult<()> {
        let (i32_ty, ptr_ty) = (self.i32(), self.ptr_ty);
        // The system's default guarantee is only a few kilobytes once the exception dispatcher has run, which leaves
        // the filter too little stack to flush and write reliably. Ask for the same room the Unix handler gets, so
        // the message is written every time instead of the process dying silently on some runs.
        let guarantee = libc(self.module, "SetThreadStackGuarantee", i32_ty.fn_type(&[ptr_ty.into()], false));
        let request = self.b.build_alloca(i32_ty, "stack_guarantee").map_err(llvm_err)?;
        self.b.build_store(request, i32_ty.const_int(u64::from(STACK_GUARANTEE), false)).map_err(llvm_err)?;
        self.call(guarantee, &[request.into()])?;
        let add =
            libc(self.module, "AddVectoredExceptionHandler", ptr_ty.fn_type(&[i32_ty.into(), ptr_ty.into()], false));
        self.call(add, &[i32_ty.const_int(1, false).into(), handler.as_global_value().as_pointer_value().into()])?;
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(())
    }

    /// `_exit(code)`, shared by the signal and exception handlers: it does not run `atexit` hooks, so the handler's
    /// own flush is the only one.
    fn exit_function(&mut self) -> FunctionValue<'ctx> {
        let exit = libc(self.module, "_exit", self.cx.void_type().fn_type(&[self.i32().into()], false));
        self.mark_noreturn(exit);
        exit
    }

    // ----- strings ---------------------------------------------------------------------

    /// `alloc_string(out: *String, bytes, len: usize)`: a new `String` holding a copy of the bytes. An empty string
    /// does not allocate (null pointer, capacity 0).
    fn alloc_string(&mut self, _h: &BaseHelpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty) = (self.usize_ty, self.ptr_ty);
        let void = self.cx.void_type();
        let f = self
            .function("burdin_rt_alloc_string", void.fn_type(&[ptr_ty.into(), ptr_ty.into(), usize_ty.into()], false));
        let (out, bytes, len) = (Self::ptr_param(f, 0), Self::ptr_param(f, 1), Self::int_param(f, 2));
        let field = |i: u32| self.b.build_struct_gep(self.string_ty, out, i, "").map_err(llvm_err);
        let (ptr_field, len_field, cap_field) = (field(0)?, field(1)?, field(2)?);
        self.b.build_store(ptr_field, ptr_ty.const_null()).map_err(llvm_err)?;
        self.b.build_store(len_field, len).map_err(llvm_err)?;
        self.b.build_store(cap_field, len).map_err(llvm_err)?;
        let alloc = self.block(f, "alloc");
        let copy = self.block(f, "copy");
        let done = self.block(f, "done");
        let empty = self.b.build_int_compare(IntPredicate::EQ, len, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(empty, done, alloc).map_err(llvm_err)?;

        self.b.position_at_end(alloc);
        let p = self.call_rt_alloc(len)?;
        self.b.build_unconditional_branch(copy).map_err(llvm_err)?;

        self.b.position_at_end(copy);
        self.b.build_memcpy(p, 1, bytes, 1, len).map_err(|e| format!("LLVM builder error: {e}"))?;
        self.b.build_store(ptr_field, p).map_err(llvm_err)?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;

        self.b.position_at_end(done);
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `string_free(ptr, cap: usize)`: frees the buffer of a `String` unless it never allocated.
    fn string_free(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty) = (self.usize_ty, self.ptr_ty);
        let void = self.cx.void_type();
        let f = self.function("burdin_rt_string_free", void.fn_type(&[ptr_ty.into(), usize_ty.into()], false));
        let release = self.block(f, "free");
        let done = self.block(f, "done");
        let empty = self
            .b
            .build_int_compare(IntPredicate::EQ, Self::int_param(f, 1), usize_ty.const_zero(), "")
            .map_err(llvm_err)?;
        self.b.build_conditional_branch(empty, done, release).map_err(llvm_err)?;
        self.b.position_at_end(release);
        self.call_rt_free(Self::ptr_param(f, 0))?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(done);
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `string_new(out: *String)`: an empty `String` (null buffer, no capacity).
    fn string_new(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let void = self.cx.void_type();
        let f = self.function("burdin_rt_string_new", void.fn_type(&[self.ptr_ty.into()], false));
        self.b.build_store(Self::ptr_param(f, 0), self.string_ty.const_zero()).map_err(llvm_err)?;
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `string_clone(dst: *String, src: *String)`: a new buffer holding a copy of `src`.
    fn string_clone(&mut self, h: &Helpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty) = (self.usize_ty, self.ptr_ty);
        let void = self.cx.void_type();
        let ty = void.fn_type(&[ptr_ty.into(), ptr_ty.into()], false);
        let f = self.function("burdin_rt_string_clone", ty);
        let src = Self::ptr_param(f, 1);
        let ptr_field = self.b.build_struct_gep(self.string_ty, src, 0, "").map_err(llvm_err)?;
        let len_field = self.b.build_struct_gep(self.string_ty, src, 1, "").map_err(llvm_err)?;
        let bytes = self.b.build_load(ptr_ty, ptr_field, "").map_err(llvm_err)?;
        let len = self.b.build_load(usize_ty, len_field, "").map_err(llvm_err)?;
        self.call(h.alloc_string, &[Self::ptr_param(f, 0).into(), bytes.into(), len.into()])?;
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `string_push_str(s: *String, ptr, len: usize)`: appends `len` bytes, growing the buffer to at least
    /// `len + len / 2 + 8` when it does not fit. An empty append does nothing.
    fn string_push_str(&mut self, h: &Helpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty) = (self.usize_ty, self.ptr_ty);
        let void = self.cx.void_type();
        let ty = void.fn_type(&[ptr_ty.into(), ptr_ty.into(), usize_ty.into()], false);
        let f = self.function("burdin_rt_string_push_str", ty);
        let (s, bytes, len) = (Self::ptr_param(f, 0), Self::ptr_param(f, 1), Self::int_param(f, 2));
        let ptr_field = self.b.build_struct_gep(self.string_ty, s, 0, "").map_err(llvm_err)?;
        let len_field = self.b.build_struct_gep(self.string_ty, s, 1, "").map_err(llvm_err)?;
        let cap_field = self.b.build_struct_gep(self.string_ty, s, 2, "").map_err(llvm_err)?;

        let done = self.block(f, "done");
        let grow = self.block(f, "grow");
        let oom = self.block(f, "oom");
        let check_cap = self.block(f, "check.cap");
        let copy = self.block(f, "copy");
        let resize = self.block(f, "resize");
        let resized = self.block(f, "resized");

        let empty = self.b.build_int_compare(IntPredicate::EQ, len, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(empty, done, grow).map_err(llvm_err)?;

        self.b.position_at_end(grow);
        let old_len = self.b.build_load(usize_ty, len_field, "").map_err(llvm_err)?.into_int_value();
        let cap = self.b.build_load(usize_ty, cap_field, "").map_err(llvm_err)?.into_int_value();
        let need = self.b.build_int_add(old_len, len, "").map_err(llvm_err)?;
        // A wrapped sum is an allocation that cannot be satisfied.
        let overflow = self.b.build_int_compare(IntPredicate::ULT, need, old_len, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(overflow, oom, check_cap).map_err(llvm_err)?;

        self.b.position_at_end(check_cap);
        let fits = self.b.build_int_compare(IntPredicate::ULE, need, cap, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(fits, copy, resize).map_err(llvm_err)?;

        self.b.position_at_end(resize);
        let half = self.b.build_right_shift(cap, usize_ty.const_int(1, false), false, "").map_err(llvm_err)?;
        let extra = self.b.build_int_add(half, usize_ty.const_int(8, false), "").map_err(llvm_err)?;
        let grown_cap = self.b.build_int_add(cap, extra, "").map_err(llvm_err)?;
        let too_small = self.b.build_int_compare(IntPredicate::ULT, grown_cap, need, "").map_err(llvm_err)?;
        let new_cap = self.b.build_select(too_small, need, grown_cap, "").map_err(llvm_err)?.into_int_value();
        let old_ptr = self.b.build_load(ptr_ty, ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        let grown = self.call_rt_realloc(old_ptr, new_cap)?;
        self.b.build_unconditional_branch(resized).map_err(llvm_err)?;

        self.b.position_at_end(resized);
        self.b.build_store(ptr_field, grown).map_err(llvm_err)?;
        self.b.build_store(cap_field, new_cap).map_err(llvm_err)?;
        self.b.build_unconditional_branch(copy).map_err(llvm_err)?;

        self.b.position_at_end(oom);
        self.write_const(h.write_all, 2, b"panic: out of memory\n")?;
        self.call(h.print_backtrace, &[])?;
        self.call(h.die, &[])?;
        self.b.build_unreachable().map_err(llvm_err)?;

        self.b.position_at_end(copy);
        let buf = self.b.build_load(ptr_ty, ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        let dest = self.offset(buf, old_len)?;
        self.b.build_memcpy(dest, 1, bytes, 1, len).map_err(|e| format!("LLVM builder error: {e}"))?;
        self.b.build_store(len_field, need).map_err(llvm_err)?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;

        self.b.position_at_end(done);
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// A runtime allocation with no recorded site (the runtime itself is allocating).
    fn call_rt_alloc(&mut self, size: IntValue<'ctx>) -> GenResult<PointerValue<'ctx>> {
        let f = self.fn_alloc.expect("the allocator is created before the helpers that allocate");
        let call = self.call(f, &[size.into(), self.ptr_ty.const_null().into(), self.usize_ty.const_zero().into()])?;
        Ok(call.expect("alloc returns a pointer").into_pointer_value())
    }

    /// A runtime free of a block `call_rt_alloc` or `call_rt_realloc` returned.
    fn call_rt_free(&mut self, ptr: PointerValue<'ctx>) -> GenResult<()> {
        let f = self.fn_free.expect("the allocator is created before the helpers that free");
        self.call(f, &[ptr.into()])?;
        Ok(())
    }

    /// A runtime reallocation with no recorded site.
    fn call_rt_realloc(&mut self, ptr: PointerValue<'ctx>, size: IntValue<'ctx>) -> GenResult<PointerValue<'ctx>> {
        let f = self.fn_realloc.expect("the allocator is created before the helpers that reallocate");
        let call = self
            .call(f, &[ptr.into(), size.into(), self.ptr_ty.const_null().into(), self.usize_ty.const_zero().into()])?;
        Ok(call.expect("realloc returns a pointer").into_pointer_value())
    }

    /// `alloc(size, site, site_len)`: a block of at least one byte for `Heap<T>`, a container, a `Shared<T>`, a
    /// box, or a `String`, or a panic. In a test build the checking allocator records the block and its source
    /// site, so `free` can detect a double free and the test can report a leak (`spec/allocators.md`, AL9).
    fn heap_alloc(&mut self, h: &BaseHelpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i64_ty) = (self.usize_ty, self.ptr_ty, self.i64());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let f =
            self.function("burdin_rt_alloc", ptr_ty.fn_type(&[usize_ty.into(), ptr_ty.into(), usize_ty.into()], false));
        let (size, site, site_len) = (Self::int_param(f, 0), Self::ptr_param(f, 1), Self::int_param(f, 2));
        let zero = self.b.build_int_compare(IntPredicate::EQ, size, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let size =
            self.b.build_select(zero, usize_ty.const_int(1, false), size, "").map_err(llvm_err)?.into_int_value();
        let total = if self.checking {
            self.b.build_int_add(size, usize_ty.const_int(HEADER, false), "alloc.total").map_err(llvm_err)?
        } else {
            size
        };
        // A wrapped size cannot be allocated.
        let wrap = self.b.build_int_compare(IntPredicate::ULT, total, size, "").map_err(llvm_err)?;
        let oom = self.block(f, "oom");
        let take = self.block(f, "alloc");
        let init = self.block(f, "init");
        let done = self.block(f, "done");
        self.b.build_conditional_branch(wrap, oom, take).map_err(llvm_err)?;
        self.b.position_at_end(take);
        let p = self.call(malloc, &[total.into()])?.expect("malloc returns a pointer").into_pointer_value();
        let null = self.b.build_is_null(p, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(null, oom, init).map_err(llvm_err)?;

        self.b.position_at_end(init);
        let data = if self.checking {
            // The header is `{ next: ptr, size: usize, site: ptr, site_len: usize, magic: i64 }`. Tasks allocate from
            // several threads, so the live list is updated under its lock.
            let live = self.module.get_global("burdin_rt_live").expect("the live list exists").as_pointer_value();
            self.lock_global("burdin_rt_live_lock")?;
            let head = self.b.build_load(ptr_ty, live, "live.head").map_err(llvm_err)?;
            self.b.build_store(p, head).map_err(llvm_err)?;
            let size_at = self.offset(p, usize_ty.const_int(8, false))?;
            self.b.build_store(size_at, size).map_err(llvm_err)?;
            let site_at = self.offset(p, usize_ty.const_int(16, false))?;
            self.b.build_store(site_at, site).map_err(llvm_err)?;
            let len_at = self.offset(p, usize_ty.const_int(24, false))?;
            self.b.build_store(len_at, site_len).map_err(llvm_err)?;
            let magic_at = self.offset(p, usize_ty.const_int(32, false))?;
            self.b.build_store(magic_at, i64_ty.const_int(MAGIC, false)).map_err(llvm_err)?;
            self.b.build_store(live, p).map_err(llvm_err)?;
            self.unlock_global("burdin_rt_live_lock")?;
            self.offset(p, usize_ty.const_int(HEADER, false))?
        } else {
            p
        };
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(oom);
        self.write_const(h.write_all, 2, b"panic: out of memory\n")?;
        self.call(h.print_backtrace, &[])?;
        self.call(h.die, &[])?;
        self.b.build_unreachable().map_err(llvm_err)?;
        self.b.position_at_end(done);
        self.b.build_return(Some(&data)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `free(ptr)`: releases an allocation. In a test build it checks the header first, reports a double free or
    /// a pointer that the allocator did not hand out, and unlinks the block from the live list.
    fn heap_free(&mut self, h: &BaseHelpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i64_ty) = (self.usize_ty, self.ptr_ty, self.i64());
        let void = self.cx.void_type();
        let libc_free = libc(self.module, "free", void.fn_type(&[ptr_ty.into()], false));
        let f = self.function("burdin_rt_free", void.fn_type(&[ptr_ty.into()], false));
        let ptr = Self::ptr_param(f, 0);
        if !self.checking {
            self.call(libc_free, &[ptr.into()])?;
            self.b.build_return(None).map_err(llvm_err)?;
            return Ok(f);
        }
        let done = self.block(f, "done");
        let check = self.block(f, "check");
        let unlink = self.block(f, "unlink");
        let untracked_check = self.block(f, "untracked.check");
        let untracked = self.block(f, "untracked");
        let bad = self.block(f, "bad");
        let walk = self.block(f, "walk");
        let step = self.block(f, "step");
        let advance = self.block(f, "advance");
        let found = self.block(f, "found");
        let null = self.b.build_is_null(ptr, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(null, done, check).map_err(llvm_err)?;

        self.b.position_at_end(check);
        let negative =
            self.b.build_int_sub(usize_ty.const_zero(), usize_ty.const_int(HEADER, false), "").map_err(llvm_err)?;
        let header = self.offset(ptr, negative)?;
        let magic_at = self.offset(header, usize_ty.const_int(32, false))?;
        let magic = self.b.build_load(i64_ty, magic_at, "magic").map_err(llvm_err)?.into_int_value();
        let ok =
            self.b.build_int_compare(IntPredicate::EQ, magic, i64_ty.const_int(MAGIC, false), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(ok, unlink, untracked_check).map_err(llvm_err)?;

        self.b.position_at_end(untracked_check);
        let is_untracked = self
            .b
            .build_int_compare(IntPredicate::EQ, magic, i64_ty.const_int(UNTRACKED_MAGIC, false), "")
            .map_err(llvm_err)?;
        self.b.build_conditional_branch(is_untracked, untracked, bad).map_err(llvm_err)?;

        self.b.position_at_end(untracked);
        self.b.build_store(magic_at, i64_ty.const_zero()).map_err(llvm_err)?;
        self.call(libc_free, &[header.into()])?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;

        self.b.position_at_end(bad);
        self.write_const(
            h.write_all,
            2,
            b"panic: invalid free (double free or a pointer the allocator did not give out)\n",
        )?;
        self.call(h.print_backtrace, &[])?;
        self.call(h.die, &[])?;
        self.b.build_unreachable().map_err(llvm_err)?;

        self.b.position_at_end(unlink);
        self.lock_global("burdin_rt_live_lock")?;
        let live = self.module.get_global("burdin_rt_live").expect("the live list exists").as_pointer_value();
        let prev = self.b.build_alloca(ptr_ty, "free.prev").map_err(llvm_err)?;
        self.b.build_store(prev, live).map_err(llvm_err)?;
        let cur = self.b.build_alloca(ptr_ty, "free.cur").map_err(llvm_err)?;
        let head = self.b.build_load(ptr_ty, live, "live.head").map_err(llvm_err)?;
        self.b.build_store(cur, head).map_err(llvm_err)?;
        self.b.build_unconditional_branch(walk).map_err(llvm_err)?;

        self.b.position_at_end(walk);
        let c = self.b.build_load(ptr_ty, cur, "").map_err(llvm_err)?.into_pointer_value();
        let c_int = self.b.build_ptr_to_int(c, usize_ty, "").map_err(llvm_err)?;
        let h_int = self.b.build_ptr_to_int(header, usize_ty, "").map_err(llvm_err)?;
        let is_it = self.b.build_int_compare(IntPredicate::EQ, c_int, h_int, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(is_it, found, step).map_err(llvm_err)?;

        self.b.position_at_end(step);
        let c = self.b.build_load(ptr_ty, cur, "").map_err(llvm_err)?.into_pointer_value();
        let c_null = self.b.build_is_null(c, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(c_null, bad, advance).map_err(llvm_err)?;

        self.b.position_at_end(advance);
        self.b.build_store(prev, c).map_err(llvm_err)?;
        let next = self.b.build_load(ptr_ty, c, "").map_err(llvm_err)?;
        self.b.build_store(cur, next).map_err(llvm_err)?;
        self.b.build_unconditional_branch(walk).map_err(llvm_err)?;

        self.b.position_at_end(found);
        let next = self.b.build_load(ptr_ty, header, "").map_err(llvm_err)?;
        let prev = self.b.build_load(ptr_ty, prev, "").map_err(llvm_err)?.into_pointer_value();
        self.b.build_store(prev, next).map_err(llvm_err)?;
        self.b.build_store(magic_at, i64_ty.const_zero()).map_err(llvm_err)?;
        self.unlock_global("burdin_rt_live_lock")?;
        self.call(libc_free, &[header.into()])?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;

        self.b.position_at_end(done);
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `untrack(ptr)`: unlinks a live block from the list the leak check reads and marks it, so it is freed later as
    /// usual but never reported. Only built for a test build.
    fn heap_untrack(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i64_ty) = (self.usize_ty, self.ptr_ty, self.i64());
        let void = self.cx.void_type();
        let f = self.function("burdin_rt_untrack", void.fn_type(&[ptr_ty.into()], false));
        let ptr = Self::ptr_param(f, 0);
        let walk = self.block(f, "walk");
        let step = self.block(f, "step");
        let found = self.block(f, "found");
        let done = self.block(f, "done");
        let negative =
            self.b.build_int_sub(usize_ty.const_zero(), usize_ty.const_int(HEADER, false), "").map_err(llvm_err)?;
        let header = self.offset(ptr, negative)?;
        let live = self.module.get_global("burdin_rt_live").expect("the live list exists").as_pointer_value();
        self.lock_global("burdin_rt_live_lock")?;
        let prev = self.b.build_alloca(ptr_ty, "untrack.prev").map_err(llvm_err)?;
        self.b.build_store(prev, live).map_err(llvm_err)?;
        let cur = self.b.build_alloca(ptr_ty, "untrack.cur").map_err(llvm_err)?;
        let head = self.b.build_load(ptr_ty, live, "live.head").map_err(llvm_err)?;
        self.b.build_store(cur, head).map_err(llvm_err)?;
        self.b.build_unconditional_branch(walk).map_err(llvm_err)?;

        self.b.position_at_end(walk);
        let c = self.b.build_load(ptr_ty, cur, "").map_err(llvm_err)?.into_pointer_value();
        let c_null = self.b.build_is_null(c, "").map_err(llvm_err)?;
        let advance = self.block(f, "advance");
        self.b.build_conditional_branch(c_null, done, step).map_err(llvm_err)?;

        self.b.position_at_end(step);
        let c = self.b.build_load(ptr_ty, cur, "").map_err(llvm_err)?.into_pointer_value();
        let c_int = self.b.build_ptr_to_int(c, usize_ty, "").map_err(llvm_err)?;
        let h_int = self.b.build_ptr_to_int(header, usize_ty, "").map_err(llvm_err)?;
        let is_it = self.b.build_int_compare(IntPredicate::EQ, c_int, h_int, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(is_it, found, advance).map_err(llvm_err)?;

        self.b.position_at_end(advance);
        let c = self.b.build_load(ptr_ty, cur, "").map_err(llvm_err)?.into_pointer_value();
        self.b.build_store(prev, c).map_err(llvm_err)?;
        let next = self.b.build_load(ptr_ty, c, "").map_err(llvm_err)?;
        self.b.build_store(cur, next).map_err(llvm_err)?;
        self.b.build_unconditional_branch(walk).map_err(llvm_err)?;

        self.b.position_at_end(found);
        let next = self.b.build_load(ptr_ty, header, "").map_err(llvm_err)?;
        let prev = self.b.build_load(ptr_ty, prev, "").map_err(llvm_err)?.into_pointer_value();
        self.b.build_store(prev, next).map_err(llvm_err)?;
        let magic_at = self.offset(header, usize_ty.const_int(32, false))?;
        self.b.build_store(magic_at, i64_ty.const_int(UNTRACKED_MAGIC, false)).map_err(llvm_err)?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;

        self.b.position_at_end(done);
        self.unlock_global("burdin_rt_live_lock")?;
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `realloc(ptr, size, site, site_len) -> ptr`: resizes an allocation, panicking when it fails. In a test
    /// build the new block is recorded and the old one released through the checking allocator.
    fn heap_realloc(
        &mut self,
        h: &BaseHelpers<'ctx>,
        alloc: FunctionValue<'ctx>,
        free: FunctionValue<'ctx>,
    ) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty) = (self.usize_ty, self.ptr_ty);
        let libc_realloc = libc(self.module, "realloc", ptr_ty.fn_type(&[ptr_ty.into(), usize_ty.into()], false));
        let f = self.function(
            "burdin_rt_realloc",
            ptr_ty.fn_type(&[ptr_ty.into(), usize_ty.into(), ptr_ty.into(), usize_ty.into()], false),
        );
        let (ptr, size, site, site_len) =
            (Self::ptr_param(f, 0), Self::int_param(f, 1), Self::ptr_param(f, 2), Self::int_param(f, 3));
        let zero = self.b.build_int_compare(IntPredicate::EQ, size, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let size =
            self.b.build_select(zero, usize_ty.const_int(1, false), size, "").map_err(llvm_err)?.into_int_value();
        if !self.checking {
            let p = self
                .call(libc_realloc, &[ptr.into(), size.into()])?
                .expect("realloc returns a pointer")
                .into_pointer_value();
            let null = self.b.build_is_null(p, "").map_err(llvm_err)?;
            let oom = self.block(f, "oom");
            let done = self.block(f, "done");
            self.b.build_conditional_branch(null, oom, done).map_err(llvm_err)?;
            self.b.position_at_end(oom);
            self.write_const(h.write_all, 2, b"panic: out of memory\n")?;
            self.call(h.print_backtrace, &[])?;
            self.call(h.die, &[])?;
            self.b.build_unreachable().map_err(llvm_err)?;
            self.b.position_at_end(done);
            self.b.build_return(Some(&p)).map_err(llvm_err)?;
            return Ok(f);
        }
        // The checking path: a fresh tracked block, a copy of the smaller size, and a tracked free of the old.
        let fresh = self.block(f, "fresh");
        let grow = self.block(f, "grow");
        let done = self.block(f, "done");
        let null = self.b.build_is_null(ptr, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(null, fresh, grow).map_err(llvm_err)?;
        self.b.position_at_end(fresh);
        let newp = self
            .call(alloc, &[size.into(), site.into(), site_len.into()])?
            .expect("alloc returns a pointer")
            .into_pointer_value();
        let fresh_end = self.b.get_insert_block().expect("positioned");
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;

        self.b.position_at_end(grow);
        let negative =
            self.b.build_int_sub(usize_ty.const_zero(), usize_ty.const_int(HEADER, false), "").map_err(llvm_err)?;
        let header = self.offset(ptr, negative)?;
        let old_at = self.offset(header, usize_ty.const_int(8, false))?;
        let old = self.b.build_load(usize_ty, old_at, "old.size").map_err(llvm_err)?.into_int_value();
        let newp2 = self
            .call(alloc, &[size.into(), site.into(), site_len.into()])?
            .expect("alloc returns a pointer")
            .into_pointer_value();
        let take = self
            .b
            .build_select(self.b.build_int_compare(IntPredicate::ULT, old, size, "").map_err(llvm_err)?, old, size, "")
            .map_err(llvm_err)?
            .into_int_value();
        self.b.build_memcpy(newp2, 1, ptr, 1, take).map_err(|e| format!("LLVM builder error: {e}"))?;
        self.call(free, &[ptr.into()])?;
        let grow_end = self.b.get_insert_block().expect("positioned");
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;

        self.b.position_at_end(done);
        let out = self.b.build_phi(ptr_ty, "realloc.out").map_err(llvm_err)?;
        out.add_incoming(&[(&newp, fresh_end), (&newp2, grow_end)]);
        self.b.build_return(Some(&out.as_basic_value())).map_err(llvm_err)?;
        Ok(f)
    }

    /// `() -> i32`: reports the allocations still live, with the site each was made at, and returns 1 when there
    /// are any (`spec/allocators.md`, AL9). A `burdin test` build calls this after a test that passed.
    fn leak_check(&mut self, h: &BaseHelpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let f = self.function("burdin_rt_leak_check", i32_ty.fn_type(&[], false));
        let live = self.module.get_global("burdin_rt_live").expect("the live list exists").as_pointer_value();
        let count = self.b.build_alloca(usize_ty, "leak.count").map_err(llvm_err)?;
        let bytes = self.b.build_alloca(usize_ty, "leak.bytes").map_err(llvm_err)?;
        let cur = self.b.build_alloca(ptr_ty, "leak.cur").map_err(llvm_err)?;
        self.b.build_store(count, usize_ty.const_zero()).map_err(llvm_err)?;
        self.b.build_store(bytes, usize_ty.const_zero()).map_err(llvm_err)?;
        let head = self.b.build_load(ptr_ty, live, "").map_err(llvm_err)?;
        self.b.build_store(cur, head).map_err(llvm_err)?;
        let cond = self.block(f, "leak.cond");
        let body = self.block(f, "leak.body");
        let end = self.block(f, "leak.end");
        let none = self.block(f, "leak.none");
        let report = self.block(f, "leak.report");
        self.b.build_unconditional_branch(cond).map_err(llvm_err)?;
        self.b.position_at_end(cond);
        let c = self.b.build_load(ptr_ty, cur, "").map_err(llvm_err)?.into_pointer_value();
        let done = self.b.build_is_null(c, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(done, end, body).map_err(llvm_err)?;
        self.b.position_at_end(body);
        let n = self.b.build_load(usize_ty, count, "").map_err(llvm_err)?.into_int_value();
        let one_more = self.b.build_int_add(n, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_store(count, one_more).map_err(llvm_err)?;
        let size_at = self.offset(c, usize_ty.const_int(8, false))?;
        let size = self.b.build_load(usize_ty, size_at, "").map_err(llvm_err)?.into_int_value();
        let b = self.b.build_load(usize_ty, bytes, "").map_err(llvm_err)?.into_int_value();
        self.b.build_store(bytes, self.b.build_int_add(b, size, "").map_err(llvm_err)?).map_err(llvm_err)?;
        let next = self.b.build_load(ptr_ty, c, "").map_err(llvm_err)?;
        self.b.build_store(cur, next).map_err(llvm_err)?;
        self.b.build_unconditional_branch(cond).map_err(llvm_err)?;

        self.b.position_at_end(end);
        let n = self.b.build_load(usize_ty, count, "").map_err(llvm_err)?.into_int_value();
        let empty = self.b.build_int_compare(IntPredicate::EQ, n, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(empty, none, report).map_err(llvm_err)?;

        self.b.position_at_end(none);
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;

        self.b.position_at_end(report);
        self.write_const(h.write_all, 2, b"test leak: ")?;
        self.call(h.write_u64, &[i32_ty.const_int(2, false).into(), n.into()])?;
        self.write_const(h.write_all, 2, b" allocation(s) leaked, ")?;
        let b = self.b.build_load(usize_ty, bytes, "").map_err(llvm_err)?.into_int_value();
        self.call(h.write_u64, &[i32_ty.const_int(2, false).into(), b.into()])?;
        self.write_const(h.write_all, 2, b" byte(s)\n")?;
        // One line per leaked block, with the site the compiler recorded.
        let head = self.b.build_load(ptr_ty, live, "").map_err(llvm_err)?;
        self.b.build_store(cur, head).map_err(llvm_err)?;
        let each = self.block(f, "leak.each");
        let each_site = self.block(f, "leak.each.site");
        let each_runtime = self.block(f, "leak.each.runtime");
        let each_after = self.block(f, "leak.each.after");
        let each_end = self.block(f, "leak.each.end");
        let leaked = self.block(f, "leak.each.leaked");
        self.b.build_unconditional_branch(each).map_err(llvm_err)?;
        self.b.position_at_end(each);
        let c = self.b.build_load(ptr_ty, cur, "").map_err(llvm_err)?.into_pointer_value();
        let done = self.b.build_is_null(c, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(done, each_end, leaked).map_err(llvm_err)?;
        self.b.position_at_end(leaked);
        self.write_const(h.write_all, 2, b"  allocated at ")?;
        let site_at = self.offset(c, usize_ty.const_int(16, false))?;
        let site = self.b.build_load(ptr_ty, site_at, "").map_err(llvm_err)?.into_pointer_value();
        let len_at = self.offset(c, usize_ty.const_int(24, false))?;
        let len = self.b.build_load(usize_ty, len_at, "").map_err(llvm_err)?.into_int_value();
        let has_site = self.b.build_int_compare(IntPredicate::NE, len, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(has_site, each_site, each_runtime).map_err(llvm_err)?;
        self.b.position_at_end(each_site);
        self.call(h.write_all, &[i32_ty.const_int(2, false).into(), site.into(), len.into()])?;
        self.b.build_unconditional_branch(each_after).map_err(llvm_err)?;
        self.b.position_at_end(each_runtime);
        self.write_const(h.write_all, 2, b"<runtime>")?;
        self.b.build_unconditional_branch(each_after).map_err(llvm_err)?;
        self.b.position_at_end(each_after);
        self.write_const(h.write_all, 2, b"\n")?;
        let next = self.b.build_load(ptr_ty, c, "").map_err(llvm_err)?;
        self.b.build_store(cur, next).map_err(llvm_err)?;
        self.b.build_unconditional_branch(each).map_err(llvm_err)?;

        self.b.position_at_end(each_end);
        self.b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;
        Ok(f)
    }

    /// `cstring_from(ptr, len: usize) -> ptr`: copies the bytes into a `malloc`ed buffer with a terminating NUL.
    fn cstring_from(&mut self, h: &Helpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i8_ty) = (self.usize_ty, self.ptr_ty, self.i8());
        let f = self.function("burdin_rt_cstring_from", ptr_ty.fn_type(&[ptr_ty.into(), usize_ty.into()], false));
        let (src, len) = (Self::ptr_param(f, 0), Self::int_param(f, 1));
        let one = usize_ty.const_int(1, false);
        let size = self.b.build_int_add(len, one, "size").map_err(llvm_err)?;
        // A wrapped size cannot be allocated.
        let overflow = self.b.build_int_compare(IntPredicate::ULT, size, len, "").map_err(llvm_err)?;
        let alloc = self.block(f, "alloc");
        let oom = self.block(f, "oom");
        let done = self.block(f, "done");
        self.b.build_conditional_branch(overflow, oom, alloc).map_err(llvm_err)?;

        self.b.position_at_end(oom);
        self.write_const(
            h.write_all,
            2,
            b"panic: out of memory
",
        )?;
        self.call(h.print_backtrace, &[])?;
        self.call(h.die, &[])?;
        self.b.build_unreachable().map_err(llvm_err)?;

        self.b.position_at_end(alloc);
        let p = self.call_rt_alloc(size)?;
        let copy = self.block(f, "copy");
        self.b.build_unconditional_branch(copy).map_err(llvm_err)?;

        self.b.position_at_end(copy);
        self.b.build_memcpy(p, 1, src, 1, len).map_err(|e| format!("LLVM builder error: {e}"))?;
        let end = self.offset(p, len)?;
        self.b.build_store(end, i8_ty.const_zero()).map_err(llvm_err)?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;

        self.b.position_at_end(done);
        self.b.build_return(Some(&p)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `cstring_clone(ptr) -> ptr`: copies a NUL-terminated C string with `strlen` and `cstring_from`.
    fn cstring_clone(
        &mut self,
        h: &Helpers<'ctx>,
        cstring_from: FunctionValue<'ctx>,
    ) -> GenResult<FunctionValue<'ctx>> {
        let (ptr_ty, usize_ty) = (self.ptr_ty, self.usize_ty);
        let strlen = libc(self.module, "strlen", usize_ty.fn_type(&[ptr_ty.into()], false));
        let f = self.function("burdin_rt_cstring_clone", ptr_ty.fn_type(&[ptr_ty.into()], false));
        let src = Self::ptr_param(f, 0);
        let len = self.call(strlen, &[src.into()])?.expect("strlen returns a size").into_int_value();
        let _ = h;
        let copy = self.call(cstring_from, &[src.into(), len.into()])?.expect("cstring_from returns a pointer");
        self.b.build_return(Some(&copy)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `str_to_string(ptr, len: usize, out: *String)`
    fn str_to_string(&mut self, h: &Helpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty) = (self.usize_ty, self.ptr_ty);
        let void = self.cx.void_type();
        let f = self
            .function("burdin_rt_str_to_string", void.fn_type(&[ptr_ty.into(), usize_ty.into(), ptr_ty.into()], false));
        let args = [Self::ptr_param(f, 2).into(), Self::ptr_param(f, 0).into(), Self::int_param(f, 1).into()];
        self.call(h.alloc_string, &args)?;
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `i64_to_string(v: i64, out: *String)` (`signed`) or `u64_to_string(v: i64, out: *String)`.
    fn int_to_string(&mut self, h: &Helpers<'ctx>, signed: bool) -> GenResult<FunctionValue<'ctx>> {
        let i64_ty = self.i64();
        let void = self.cx.void_type();
        let name = if signed { "burdin_rt_i64_to_string" } else { "burdin_rt_u64_to_string" };
        let f = self.function(name, void.fn_type(&[i64_ty.into(), self.ptr_ty.into()], false));
        let buf = self.buffer(24)?;
        let end = self.offset(buf, self.usize_ty.const_int(24, false))?;
        let v = Self::int_param(f, 0);
        let mut start;
        if signed {
            // Negate in unsigned arithmetic, so that i64::MIN works.
            let neg = self.b.build_int_compare(IntPredicate::SLT, v, i64_ty.const_zero(), "").map_err(llvm_err)?;
            let minus_v = self.b.build_int_sub(i64_ty.const_zero(), v, "").map_err(llvm_err)?;
            let u = self.b.build_select(neg, minus_v, v, "").map_err(llvm_err)?;
            start =
                self.call(h.digits, &[u.into(), end.into()])?.expect("digits returns a pointer").into_pointer_value();
            // At most 20 digits fit in the 24-byte buffer, so there is always room for the sign.
            let sign = self.offset(start, self.usize_ty.const_all_ones())?;
            self.b.build_store(sign, self.i8().const_int(u64::from(b'-'), false)).map_err(llvm_err)?;
            start = self.b.build_select(neg, sign, start, "").map_err(llvm_err)?.into_pointer_value();
        } else {
            start =
                self.call(h.digits, &[v.into(), end.into()])?.expect("digits returns a pointer").into_pointer_value();
        }
        let len = self.distance(start, end)?;
        self.call(h.alloc_string, &[Self::ptr_param(f, 1).into(), start.into(), len.into()])?;
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `f64_to_string(v: f64, prec: i32, single: bool, out: *String)`. With `prec >= 0` the text is `%.{prec}f`.
    /// Otherwise it is the shortest `%.{p}e` text that `strtod` reads back as the same value (compared as an
    /// `f32` when `single`), which `std`'s formatter turns into positional notation.
    fn f64_to_string(&mut self, h: &Helpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let f64_ty = self.cx.f64_type();
        let void = self.cx.void_type();
        let snprintf =
            libc(self.module, "snprintf", i32_ty.fn_type(&[ptr_ty.into(), usize_ty.into(), ptr_ty.into()], true));
        let strtod = libc(self.module, "strtod", f64_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
        let f = self.function(
            "burdin_rt_f64_to_string",
            void.fn_type(&[f64_ty.into(), i32_ty.into(), self.cx.bool_type().into(), ptr_ty.into()], false),
        );
        let v = f.get_nth_param(0).expect("a value").into_float_value();
        let (prec, single, out) = (Self::int_param(f, 1), Self::int_param(f, 2), Self::ptr_param(f, 3));
        const CAP: u32 = 640;
        let buf = self.buffer(CAP)?;
        let cap = usize_ty.const_int(u64::from(CAP), false);
        let fixed_fmt = self.bytes(b"%.*f ");
        let sci_fmt = self.bytes(b"%.*e ");
        let fixed = self.block(f, "fixed");
        let shortest = self.block(f, "shortest");
        let search = self.block(f, "search");
        let again = self.block(f, "again");
        let done = self.block(f, "done");
        let is_fixed = self.b.build_int_compare(IntPredicate::SGE, prec, i32_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(is_fixed, fixed, shortest).map_err(llvm_err)?;

        self.b.position_at_end(fixed);
        let n_fixed = self
            .call(snprintf, &[buf.into(), cap.into(), fixed_fmt.into(), prec.into(), v.into()])?
            .expect("snprintf returns a length")
            .into_int_value();
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;

        self.b.position_at_end(shortest);
        self.b.build_unconditional_branch(search).map_err(llvm_err)?;

        self.b.position_at_end(search);
        let p = self.b.build_phi(i32_ty, "p").map_err(llvm_err)?;
        let p_v = p.as_basic_value().into_int_value();
        let n_sci = self
            .call(snprintf, &[buf.into(), cap.into(), sci_fmt.into(), p_v.into(), v.into()])?
            .expect("snprintf returns a length")
            .into_int_value();
        let parsed = self
            .call(strtod, &[buf.into(), ptr_ty.const_null().into()])?
            .expect("strtod returns a double")
            .into_float_value();
        let same_f64 = self.b.build_float_compare(inkwell::FloatPredicate::OEQ, parsed, v, "").map_err(llvm_err)?;
        let f32_ty = self.cx.f32_type();
        let v32 = self.b.build_float_trunc(v, f32_ty, "").map_err(llvm_err)?;
        let parsed32 = self.b.build_float_trunc(parsed, f32_ty, "").map_err(llvm_err)?;
        let same_f32 = self.b.build_float_compare(inkwell::FloatPredicate::OEQ, parsed32, v32, "").map_err(llvm_err)?;
        let same = self.b.build_select(single, same_f32, same_f64, "").map_err(llvm_err)?.into_int_value();
        let last =
            self.b.build_int_compare(IntPredicate::SGE, p_v, i32_ty.const_int(17, false), "").map_err(llvm_err)?;
        let stop = self.b.build_or(same, last, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(stop, done, again).map_err(llvm_err)?;

        self.b.position_at_end(again);
        let next = self.b.build_int_add(p_v, i32_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_unconditional_branch(search).map_err(llvm_err)?;
        p.add_incoming(&[(&i32_ty.const_zero(), shortest), (&next, again)]);

        self.b.position_at_end(done);
        let n = self.b.build_phi(i32_ty, "n").map_err(llvm_err)?;
        n.add_incoming(&[(&n_fixed, fixed), (&n_sci, search)]);
        let len = self.b.build_int_z_extend(n.as_basic_value().into_int_value(), usize_ty, "").map_err(llvm_err)?;
        self.call(h.alloc_string, &[out.into(), buf.into(), len.into()])?;
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `char_to_string(c: i32, out: *String)`: the UTF-8 encoding of a Unicode scalar value.
    fn char_to_string(&mut self, h: &Helpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let i32_ty = self.i32();
        let void = self.cx.void_type();
        let f = self.function("burdin_rt_char_to_string", void.fn_type(&[i32_ty.into(), self.ptr_ty.into()], false));
        let buf = self.buffer(4)?;
        let c = Self::int_param(f, 0);
        let len = self.utf8_encode(f, c, buf)?;
        let args = [Self::ptr_param(f, 1).into(), buf.into(), len.into()];
        self.call(h.alloc_string, &args)?;
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// Writes the UTF-8 encoding of `c` into `buf` (four bytes) and returns its length (1 to 4).
    fn utf8_encode(
        &mut self,
        f: FunctionValue<'ctx>,
        c: IntValue<'ctx>,
        buf: PointerValue<'ctx>,
    ) -> GenResult<IntValue<'ctx>> {
        let (i8_ty, i32_ty, usize_ty) = (self.i8(), self.i32(), self.usize_ty);
        let join = self.block(f, "utf8.join");
        // (encoded length, smallest code point that needs the next length, leading-byte marker)
        let cases: [(u32, u64, u64); 4] = [(1, 0x80, 0x00), (2, 0x800, 0xC0), (3, 0x10000, 0xE0), (4, 0, 0xF0)];
        let mut incoming = Vec::new();
        for (n, limit, lead) in cases {
            let encode = self.block(f, &format!("utf8.{n}"));
            if n < 4 {
                let next = self.block(f, "test");
                let fits = self
                    .b
                    .build_int_compare(IntPredicate::ULT, c, i32_ty.const_int(limit, false), "")
                    .map_err(llvm_err)?;
                self.b.build_conditional_branch(fits, encode, next).map_err(llvm_err)?;
                self.b.position_at_end(next);
            } else {
                self.b.build_unconditional_branch(encode).map_err(llvm_err)?;
            }
            let test = self.b.get_insert_block().expect("positioned");
            self.b.position_at_end(encode);
            for i in 0..n {
                let shift = 6 * (n - 1 - i);
                let mut bits = self
                    .b
                    .build_right_shift(c, i32_ty.const_int(u64::from(shift), false), false, "")
                    .map_err(llvm_err)?;
                let marker = if i == 0 {
                    lead
                } else {
                    bits = self.b.build_and(bits, i32_ty.const_int(0x3F, false), "").map_err(llvm_err)?;
                    0x80
                };
                let bits = self.b.build_or(bits, i32_ty.const_int(marker, false), "").map_err(llvm_err)?;
                let byte = self.b.build_int_truncate(bits, i8_ty, "").map_err(llvm_err)?;
                let at = self.offset(buf, usize_ty.const_int(u64::from(i), false))?;
                self.b.build_store(at, byte).map_err(llvm_err)?;
            }
            self.b.build_unconditional_branch(join).map_err(llvm_err)?;
            incoming.push((usize_ty.const_int(u64::from(n), false), encode));
            self.b.position_at_end(test);
        }
        self.b.position_at_end(join);
        let len = self.b.build_phi(usize_ty, "len").map_err(llvm_err)?;
        for (v, bb) in &incoming {
            len.add_incoming(&[(v, *bb)]);
        }
        Ok(len.as_basic_value().into_int_value())
    }

    /// `str_starts_with(ptr, len: usize, prefix, prefix_len: usize) -> i1`
    fn str_starts_with(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let memcmp =
            libc(self.module, "memcmp", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into(), usize_ty.into()], false));
        let bool_ty = self.cx.bool_type();
        let f = self.function(
            "burdin_rt_str_starts_with",
            bool_ty.fn_type(&[ptr_ty.into(), usize_ty.into(), ptr_ty.into(), usize_ty.into()], false),
        );
        let (ptr, len, prefix, prefix_len) =
            (Self::ptr_param(f, 0), Self::int_param(f, 1), Self::ptr_param(f, 2), Self::int_param(f, 3));
        let check = self.block(f, "check");
        let no = self.block(f, "no");
        let done = self.block(f, "done");
        let fits = self.b.build_int_compare(IntPredicate::ULE, prefix_len, len, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(fits, check, no).map_err(llvm_err)?;

        self.b.position_at_end(check);
        let r = self.call(memcmp, &[ptr.into(), prefix.into(), prefix_len.into()])?.expect("memcmp returns int");
        let eq = self
            .b
            .build_int_compare(IntPredicate::EQ, r.into_int_value(), i32_ty.const_zero(), "")
            .map_err(llvm_err)?;
        let check_end = self.b.get_insert_block().expect("positioned");
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;

        self.b.position_at_end(no);
        let no_end = self.b.get_insert_block().expect("positioned");
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;

        self.b.position_at_end(done);
        let phi = self.b.build_phi(bool_ty, "starts_with").map_err(llvm_err)?;
        phi.add_incoming(&[(&eq, check_end), (&bool_ty.const_zero(), no_end)]);
        self.b.build_return(Some(&phi.as_basic_value())).map_err(llvm_err)?;
        Ok(f)
    }

    /// `str_find_char(ptr, len: usize, c: i32) -> i64`: the byte index of the first `c`, or -1. The char is
    /// encoded and searched for, so the result is a byte offset that slicing can use.
    fn str_find_char(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty, i64_ty) = (self.usize_ty, self.ptr_ty, self.i32(), self.i64());
        let memcmp =
            libc(self.module, "memcmp", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into(), usize_ty.into()], false));
        let f = self.function(
            "burdin_rt_str_find_char",
            i64_ty.fn_type(&[ptr_ty.into(), usize_ty.into(), i32_ty.into()], false),
        );
        let (ptr, len, c) = (Self::ptr_param(f, 0), Self::int_param(f, 1), Self::int_param(f, 2));
        let enc = self.buffer(4)?;
        let n = self.utf8_encode(f, c, enc)?;
        let start = self.b.get_insert_block().expect("positioned");

        let search = self.block(f, "search");
        let test = self.block(f, "test");
        let after = self.block(f, "next");
        let found = self.block(f, "found");
        let missing = self.block(f, "missing");
        self.b.build_unconditional_branch(search).map_err(llvm_err)?;

        self.b.position_at_end(search);
        let i = self.b.build_phi(usize_ty, "i").map_err(llvm_err)?;
        let i_v = i.as_basic_value().into_int_value();
        let end = self.b.build_int_add(i_v, n, "").map_err(llvm_err)?;
        let enough = self.b.build_int_compare(IntPredicate::ULE, end, len, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(enough, test, missing).map_err(llvm_err)?;

        self.b.position_at_end(test);
        let at = self.offset(ptr, i_v)?;
        let r = self.call(memcmp, &[at.into(), enc.into(), n.into()])?.expect("memcmp returns int");
        let eq = self
            .b
            .build_int_compare(IntPredicate::EQ, r.into_int_value(), i32_ty.const_zero(), "")
            .map_err(llvm_err)?;
        let next = self.b.build_int_add(i_v, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(eq, found, after).map_err(llvm_err)?;

        self.b.position_at_end(after);
        self.b.build_unconditional_branch(search).map_err(llvm_err)?;
        i.add_incoming(&[(&usize_ty.const_zero(), start), (&next, after)]);

        self.b.position_at_end(found);
        let at = self.b.build_int_z_extend(i_v, i64_ty, "").map_err(llvm_err)?;
        self.b.build_return(Some(&at)).map_err(llvm_err)?;

        self.b.position_at_end(missing);
        self.b.build_return(Some(&i64_ty.const_all_ones())).map_err(llvm_err)?;
        Ok(f)
    }

    // ----- collections and the prelude -------------------------------------------------

    /// `string_push_char(s: *String, c: i32)`: appends one UTF-8 code point.
    fn string_push_char(&mut self, push_str: FunctionValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let ty = self.cx.void_type().fn_type(&[self.ptr_ty.into(), self.i32().into()], false);
        let f = self.function("burdin_rt_string_push_char", ty);
        let enc = self.buffer(4)?;
        let n = self.utf8_encode(f, Self::int_param(f, 1), enc)?;
        self.call(push_str, &[Self::ptr_param(f, 0).into(), enc.into(), n.into()])?;
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `str_eq(ptr, len, ptr2, len2) -> i1`.
    fn str_eq(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty, bool_ty) = (self.usize_ty, self.ptr_ty, self.i32(), self.cx.bool_type());
        let memcmp =
            libc(self.module, "memcmp", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into(), usize_ty.into()], false));
        let params = [ptr_ty.into(), usize_ty.into(), ptr_ty.into(), usize_ty.into()];
        let f = self.function("burdin_rt_str_eq", bool_ty.fn_type(&params, false));
        let (a, alen, b, blen) =
            (Self::ptr_param(f, 0), Self::int_param(f, 1), Self::ptr_param(f, 2), Self::int_param(f, 3));
        let same_len = self.b.build_int_compare(IntPredicate::EQ, alen, blen, "").map_err(llvm_err)?;
        let check = self.block(f, "check");
        let no = self.block(f, "no");
        let yes = self.block(f, "yes");
        self.b.build_conditional_branch(same_len, check, no).map_err(llvm_err)?;
        self.b.position_at_end(check);
        let r = self.call(memcmp, &[a.into(), b.into(), alen.into()])?.expect("memcmp returns int").into_int_value();
        let eq = self.b.build_int_compare(IntPredicate::EQ, r, i32_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(eq, yes, no).map_err(llvm_err)?;
        self.b.position_at_end(yes);
        self.b.build_return(Some(&bool_ty.const_int(1, false))).map_err(llvm_err)?;
        self.b.position_at_end(no);
        self.b.build_return(Some(&bool_ty.const_zero())).map_err(llvm_err)?;
        Ok(f)
    }

    /// `str_cmp(ptr, len, ptr2, len2) -> i32`: byte order, then length order.
    fn str_cmp(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let memcmp =
            libc(self.module, "memcmp", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into(), usize_ty.into()], false));
        let params = [ptr_ty.into(), usize_ty.into(), ptr_ty.into(), usize_ty.into()];
        let f = self.function("burdin_rt_str_cmp", i32_ty.fn_type(&params, false));
        let (a, alen, b, blen) =
            (Self::ptr_param(f, 0), Self::int_param(f, 1), Self::ptr_param(f, 2), Self::int_param(f, 3));
        let min = self
            .b
            .build_select(
                self.b.build_int_compare(IntPredicate::ULT, alen, blen, "").map_err(llvm_err)?,
                alen,
                blen,
                "",
            )
            .map_err(llvm_err)?
            .into_int_value();
        let r = self.call(memcmp, &[a.into(), b.into(), min.into()])?.expect("memcmp returns int").into_int_value();
        let differ = self.b.build_int_compare(IntPredicate::NE, r, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let bytes = self.block(f, "bytes");
        let lengths = self.block(f, "lengths");
        self.b.build_conditional_branch(differ, bytes, lengths).map_err(llvm_err)?;
        self.b.position_at_end(bytes);
        let neg = self.b.build_int_compare(IntPredicate::SLT, r, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let sign =
            self.b.build_select(neg, i32_ty.const_all_ones(), i32_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_return(Some(&sign)).map_err(llvm_err)?;
        self.b.position_at_end(lengths);
        let lt = self.b.build_int_compare(IntPredicate::ULT, alen, blen, "").map_err(llvm_err)?;
        let eq = self.b.build_int_compare(IntPredicate::EQ, alen, blen, "").map_err(llvm_err)?;
        let less = self
            .b
            .build_select(lt, i32_ty.const_all_ones(), i32_ty.const_int(1, false), "")
            .map_err(llvm_err)?
            .into_int_value();
        let order = self.b.build_select(eq, i32_ty.const_zero(), less, "").map_err(llvm_err)?;
        self.b.build_return(Some(&order)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `str_hash(acc: i64, ptr, len: usize) -> i64`: FNV-1a over `len` bytes, continuing from `acc`.
    ///
    /// A map needs to hash its keys, and `String` keys hash by their bytes. Starting from the caller's accumulator
    /// lets a `String` field fold into a struct's hash in field order.
    fn str_hash(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i64_ty) = (self.usize_ty, self.ptr_ty, self.i64());
        let params = [i64_ty.into(), ptr_ty.into(), usize_ty.into()];
        let f = self.function("burdin_rt_str_hash", i64_ty.fn_type(&params, false));
        let (start, ptr, len) = (Self::int_param(f, 0), Self::ptr_param(f, 1), Self::int_param(f, 2));
        let cond = self.block(f, "cond");
        let body = self.block(f, "body");
        let done = self.block(f, "done");
        self.b.build_unconditional_branch(cond).map_err(llvm_err)?;

        self.b.position_at_end(cond);
        let i = self.b.build_phi(usize_ty, "i").map_err(llvm_err)?;
        let acc = self.b.build_phi(i64_ty, "acc").map_err(llvm_err)?;
        let i_v = i.as_basic_value().into_int_value();
        let acc_v = acc.as_basic_value().into_int_value();
        let go = self.b.build_int_compare(IntPredicate::ULT, i_v, len, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(go, body, done).map_err(llvm_err)?;

        self.b.position_at_end(body);
        let at = self.offset(ptr, i_v)?;
        let byte = self.b.build_load(self.i8(), at, "").map_err(llvm_err)?.into_int_value();
        let word = self.b.build_int_z_extend(byte, i64_ty, "").map_err(llvm_err)?;
        let mixed = self.b.build_xor(acc_v, word, "").map_err(llvm_err)?;
        let prime = i64_ty.const_int(0x0000_0100_0000_01b3, false);
        let next_acc = self.b.build_int_mul(mixed, prime, "").map_err(llvm_err)?;
        let next_i = self.b.build_int_add(i_v, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_unconditional_branch(cond).map_err(llvm_err)?;
        let entry = f.get_first_basic_block().expect("a function has an entry block");
        i.add_incoming(&[(&usize_ty.const_zero(), entry), (&next_i, body)]);
        acc.add_incoming(&[(&start, entry), (&next_acc, body)]);

        self.b.position_at_end(done);
        self.b.build_return(Some(&acc_v)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `str_find_str(ptr, len, needle, needle_len) -> i64`: the first byte index of `needle`, or -1.
    fn str_find_str(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty, i64_ty) = (self.usize_ty, self.ptr_ty, self.i32(), self.i64());
        let memcmp =
            libc(self.module, "memcmp", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into(), usize_ty.into()], false));
        let params = [ptr_ty.into(), usize_ty.into(), ptr_ty.into(), usize_ty.into()];
        let f = self.function("burdin_rt_str_find_str", i64_ty.fn_type(&params, false));
        let (ptr, len, needle, nlen) =
            (Self::ptr_param(f, 0), Self::int_param(f, 1), Self::ptr_param(f, 2), Self::int_param(f, 3));
        let empty = self.b.build_int_compare(IntPredicate::EQ, nlen, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let search = self.block(f, "search");
        let test = self.block(f, "test");
        let after = self.block(f, "next");
        let found = self.block(f, "found");
        let empty_bb = self.block(f, "empty");
        let missing = self.block(f, "missing");
        let start = self.b.get_insert_block().expect("positioned");
        self.b.build_conditional_branch(empty, empty_bb, search).map_err(llvm_err)?;
        self.b.position_at_end(empty_bb);
        self.b.build_return(Some(&i64_ty.const_zero())).map_err(llvm_err)?;

        self.b.position_at_end(search);
        let i = self.b.build_phi(usize_ty, "i").map_err(llvm_err)?;
        let i_v = i.as_basic_value().into_int_value();
        let end = self.b.build_int_add(i_v, nlen, "").map_err(llvm_err)?;
        let enough = self.b.build_int_compare(IntPredicate::ULE, end, len, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(enough, test, missing).map_err(llvm_err)?;
        self.b.position_at_end(test);
        let at = self.offset(ptr, i_v)?;
        let r = self.call(memcmp, &[at.into(), needle.into(), nlen.into()])?.expect("memcmp returns int");
        let same = self
            .b
            .build_int_compare(IntPredicate::EQ, r.into_int_value(), i32_ty.const_zero(), "")
            .map_err(llvm_err)?;
        let next = self.b.build_int_add(i_v, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(same, found, after).map_err(llvm_err)?;
        self.b.position_at_end(after);
        self.b.build_unconditional_branch(search).map_err(llvm_err)?;
        i.add_incoming(&[(&usize_ty.const_zero(), start), (&next, after)]);
        self.b.position_at_end(found);
        let at = self.b.build_int_z_extend(i_v, i64_ty, "").map_err(llvm_err)?;
        self.b.build_return(Some(&at)).map_err(llvm_err)?;
        self.b.position_at_end(missing);
        self.b.build_return(Some(&i64_ty.const_all_ones())).map_err(llvm_err)?;
        Ok(f)
    }

    /// `parse(ptr, len, out: *i64) -> i1`: a decimal integer with no sign (unsigned) or an optional `-`.
    fn parse_int(&mut self, _h: &Helpers<'ctx>, signed: bool) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i8_ty, i64_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i8(), self.i64(), self.i32());
        let name = if signed { "burdin_rt_parse_i64" } else { "burdin_rt_parse_u64" };
        let f =
            self.function(name, self.cx.bool_type().fn_type(&[ptr_ty.into(), usize_ty.into(), ptr_ty.into()], false));
        let (src, len, out) = (Self::ptr_param(f, 0), Self::int_param(f, 1), Self::ptr_param(f, 2));
        let empty = self.b.build_int_compare(IntPredicate::EQ, len, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let fail = self.block(f, "parse.fail");
        let sign = self.block(f, "parse.sign");
        let digits = self.block(f, "parse.digits");
        self.b.build_conditional_branch(empty, fail, sign).map_err(llvm_err)?;
        self.b.position_at_end(sign);
        let first = self.b.build_load(i8_ty, src, "first").map_err(llvm_err)?.into_int_value();
        let minus = self
            .b
            .build_int_compare(IntPredicate::EQ, first, i8_ty.const_int(u64::from(b'-'), false), "")
            .map_err(llvm_err)?;
        let neg =
            self.b.build_and(minus, self.cx.bool_type().const_int(u64::from(signed), false), "").map_err(llvm_err)?;
        let start = self
            .b
            .build_select(neg, usize_ty.const_int(1, false), usize_ty.const_zero(), "")
            .map_err(llvm_err)?
            .into_int_value();
        let only_sign = self.b.build_int_compare(IntPredicate::UGE, start, len, "").map_err(llvm_err)?;
        let after_sign = self.block(f, "parse.after.sign");
        self.b.build_conditional_branch(only_sign, fail, after_sign).map_err(llvm_err)?;
        self.b.position_at_end(after_sign);
        self.b.build_unconditional_branch(digits).map_err(llvm_err)?;

        self.b.position_at_end(digits);
        let i = self.b.build_phi(usize_ty, "i").map_err(llvm_err)?;
        let acc = self.b.build_phi(i64_ty, "acc").map_err(llvm_err)?;
        let i_v = i.as_basic_value().into_int_value();
        let acc_v = acc.as_basic_value().into_int_value();
        let more = self.b.build_int_compare(IntPredicate::ULT, i_v, len, "").map_err(llvm_err)?;
        let body = self.block(f, "parse.body");
        let done = self.block(f, "parse.done");
        self.b.build_conditional_branch(more, body, done).map_err(llvm_err)?;

        self.b.position_at_end(body);
        let at = self.offset(src, i_v)?;
        let ch = self.b.build_load(i8_ty, at, "").map_err(llvm_err)?.into_int_value();
        let digit = self.b.build_int_sub(ch, i8_ty.const_int(u64::from(b'0'), false), "").map_err(llvm_err)?;
        let digit = self.b.build_int_z_extend(digit, i64_ty, "").map_err(llvm_err)?;
        let bad =
            self.b.build_int_compare(IntPredicate::UGT, digit, i64_ty.const_int(9, false), "").map_err(llvm_err)?;
        let digit_bb = self.block(f, "parse.digit");
        self.b.build_conditional_branch(bad, fail, digit_bb).map_err(llvm_err)?;
        self.b.position_at_end(digit_bb);
        // The largest value that still fits: u64::MAX, i64::MAX, or 2^63 for a negative signed value.
        let limit = if !signed {
            i64_ty.const_all_ones()
        } else {
            self.b
                .build_select(neg, i64_ty.const_int(1 << 63, false), i64_ty.const_int(i64::MAX as u64, false), "")
                .map_err(llvm_err)?
                .into_int_value()
        };
        let space = self.b.build_int_sub(limit, digit, "").map_err(llvm_err)?;
        let allowed = self.b.build_int_unsigned_div(space, i64_ty.const_int(10, false), "").map_err(llvm_err)?;
        let too_big = self.b.build_int_compare(IntPredicate::UGT, acc_v, allowed, "").map_err(llvm_err)?;
        let next_bb = self.block(f, "parse.next");
        self.b.build_conditional_branch(too_big, fail, next_bb).map_err(llvm_err)?;
        self.b.position_at_end(next_bb);
        let ten = self.b.build_int_mul(acc_v, i64_ty.const_int(10, false), "").map_err(llvm_err)?;
        let next_acc = self.b.build_int_add(ten, digit, "").map_err(llvm_err)?;
        let next_i = self.b.build_int_add(i_v, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_unconditional_branch(digits).map_err(llvm_err)?;
        i.add_incoming(&[(&start, after_sign), (&next_i, next_bb)]);
        acc.add_incoming(&[(&i64_ty.const_zero(), after_sign), (&next_acc, next_bb)]);

        self.b.position_at_end(done);
        let value: BasicValueEnum<'ctx> = if signed {
            let negated = self.b.build_int_sub(i64_ty.const_zero(), acc_v, "").map_err(llvm_err)?;
            self.b.build_select(neg, negated, acc_v, "").map_err(llvm_err)?
        } else {
            acc_v.into()
        };
        self.b.build_store(out, value).map_err(llvm_err)?;
        self.b.build_return(Some(&self.cx.bool_type().const_int(1, false))).map_err(llvm_err)?;

        self.b.position_at_end(fail);
        self.b.build_return(Some(&self.cx.bool_type().const_zero())).map_err(llvm_err)?;
        let _ = i32_ty;
        Ok(f)
    }

    /// `read_to_string(path, path_len, out: *String) -> i32`: the file's bytes; 0, or the OS error code.
    fn read_to_string(
        &mut self,
        _h: &Helpers<'ctx>,
        alloc_string: FunctionValue<'ctx>,
        push_str: FunctionValue<'ctx>,
    ) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let fopen = libc(self.module, "fopen", ptr_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
        let fread = libc(
            self.module,
            "fread",
            usize_ty.fn_type(&[ptr_ty.into(), usize_ty.into(), usize_ty.into(), ptr_ty.into()], false),
        );
        let ferror = libc(self.module, "ferror", i32_ty.fn_type(&[ptr_ty.into()], false));
        let fclose = libc(self.module, "fclose", i32_ty.fn_type(&[ptr_ty.into()], false));
        let f = self.function(
            "burdin_rt_read_to_string",
            i32_ty.fn_type(&[ptr_ty.into(), usize_ty.into(), ptr_ty.into()], false),
        );
        let (path, plen, out) = (Self::ptr_param(f, 0), Self::int_param(f, 1), Self::ptr_param(f, 2));
        let cpath = self.cstring_of(path, plen, malloc)?;
        let mode = self.bytes(b"rb\0");
        let file =
            self.call(fopen, &[cpath.into(), mode.into()])?.expect("fopen returns a pointer").into_pointer_value();
        let null = self.b.build_is_null(file, "").map_err(llvm_err)?;
        let ok = self.block(f, "read.open");
        let fail = self.block(f, "read.fail");
        self.b.build_conditional_branch(null, fail, ok).map_err(llvm_err)?;
        self.b.position_at_end(fail);
        let code = self.last_error(true)?;
        self.call(free, &[cpath.into()])?;
        self.b.build_return(Some(&code)).map_err(llvm_err)?;

        self.b.position_at_end(ok);
        let buf = self.buffer(4096)?;
        let chunk = self.block(f, "read.chunk");
        let done = self.block(f, "read.done");
        self.b.build_unconditional_branch(chunk).map_err(llvm_err)?;
        self.b.position_at_end(chunk);
        let n = self
            .call(
                fread,
                &[buf.into(), usize_ty.const_int(1, false).into(), usize_ty.const_int(4096, false).into(), file.into()],
            )?
            .expect("fread returns a size")
            .into_int_value();
        let empty = self.b.build_int_compare(IntPredicate::EQ, n, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let append = self.block(f, "read.append");
        self.b.build_conditional_branch(empty, done, append).map_err(llvm_err)?;
        self.b.position_at_end(append);
        self.call(push_str, &[out.into(), buf.into(), n.into()])?;
        self.b.build_unconditional_branch(chunk).map_err(llvm_err)?;

        self.b.position_at_end(done);
        // The loop leaves the `String` in `out`; `alloc_string` is only used to zero it on failure paths.
        let _ = alloc_string;
        // `fread` reports a read error only through the stream's error flag, and the code is `errno`; keep it
        // before `fclose`, which may set its own.
        let bad = self.call(ferror, &[file.into()])?.expect("ferror returns a flag").into_int_value();
        let failed = self.b.build_int_compare(IntPredicate::NE, bad, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let code = self.last_error(true)?;
        self.call(fclose, &[file.into()])?;
        self.call(free, &[cpath.into()])?;
        let status = self.b.build_select(failed, code, i32_ty.const_zero(), "read.status").map_err(llvm_err)?;
        self.b.build_return(Some(&status)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `write_file(path, path_len, text, text_len) -> i32`: 0, or the OS error code.
    fn write_file(&mut self, _h: &Helpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let fopen = libc(self.module, "fopen", ptr_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
        let fwrite = libc(
            self.module,
            "fwrite",
            usize_ty.fn_type(&[ptr_ty.into(), usize_ty.into(), usize_ty.into(), ptr_ty.into()], false),
        );
        let fclose = libc(self.module, "fclose", i32_ty.fn_type(&[ptr_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let params = [ptr_ty.into(), usize_ty.into(), ptr_ty.into(), usize_ty.into()];
        let f = self.function("burdin_rt_write_file", i32_ty.fn_type(&params, false));
        let (path, plen, text, tlen) =
            (Self::ptr_param(f, 0), Self::int_param(f, 1), Self::ptr_param(f, 2), Self::int_param(f, 3));
        let cpath = self.cstring_of(path, plen, malloc)?;
        let mode = self.bytes(b"wb\0");
        let file =
            self.call(fopen, &[cpath.into(), mode.into()])?.expect("fopen returns a pointer").into_pointer_value();
        let null = self.b.build_is_null(file, "").map_err(llvm_err)?;
        let ok = self.block(f, "write.open");
        let fail = self.block(f, "write.fail");
        self.b.build_conditional_branch(null, fail, ok).map_err(llvm_err)?;
        self.b.position_at_end(fail);
        let code = self.last_error(true)?;
        self.call(free, &[cpath.into()])?;
        self.b.build_return(Some(&code)).map_err(llvm_err)?;

        self.b.position_at_end(ok);
        let written = self
            .call(fwrite, &[text.into(), usize_ty.const_int(1, false).into(), tlen.into(), file.into()])?
            .expect("fwrite returns a size")
            .into_int_value();
        let short = self.b.build_int_compare(IntPredicate::NE, written, tlen, "").map_err(llvm_err)?;
        // `fwrite` and `fclose` both report a failure through `errno`; keep the write's own code when it was
        // the write that failed, and the close's otherwise.
        let write_code = self.last_error(true)?;
        let closed = self.call(fclose, &[file.into()])?.expect("fclose returns an int").into_int_value();
        let close_code = self.last_error(true)?;
        self.call(free, &[cpath.into()])?;
        let close_failed =
            self.b.build_int_compare(IntPredicate::NE, closed, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let failed = self.b.build_or(short, close_failed, "write.failed").map_err(llvm_err)?;
        let code = self.b.build_select(short, write_code, close_code, "write.code").map_err(llvm_err)?.into_int_value();
        let status = self.b.build_select(failed, code, i32_ty.const_zero(), "write.status").map_err(llvm_err)?;
        self.b.build_return(Some(&status)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `file_open(path, path_len, read, write, append, truncate, create, create_new) -> i32`: `open`/`_open` with
    /// the flags the options name, returning the descriptor or -1. `create_new` implies `create`. The standard
    /// library rejects the option combinations the system would reject inconsistently (`spec/stdlib.md`, "Files and
    /// file handles").
    fn file_open(&mut self, file_error: PointerValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let params = [
            ptr_ty.into(),
            usize_ty.into(),
            i32_ty.into(),
            i32_ty.into(),
            i32_ty.into(),
            i32_ty.into(),
            i32_ty.into(),
            i32_ty.into(),
        ];
        let f = self.function("burdin_rt_file_open", i32_ty.fn_type(&params, false));
        let (path, plen) = (Self::ptr_param(f, 0), Self::int_param(f, 1));
        let options: Vec<IntValue<'ctx>> = (2..8).map(|i| Self::int_param(f, i)).collect();
        let c = open_flag_constants(self.windows, cfg!(target_os = "macos"));
        let zero = i32_ty.const_zero();
        let flag = |v: u32| i32_ty.const_int(u64::from(v), false);
        let nonzero = |v: IntValue<'ctx>| -> GenResult<IntValue<'ctx>> {
            self.b.build_int_compare(IntPredicate::NE, v, zero, "").map_err(llvm_err)
        };
        let read = nonzero(options[0])?;
        let write = nonzero(options[1])?;
        let append = nonzero(options[2])?;
        let truncate = nonzero(options[3])?;
        let create = nonzero(options[4])?;
        let create_new = nonzero(options[5])?;
        let writes = self.b.build_or(write, append, "writes").map_err(llvm_err)?;
        // `append` opens for writing too, and `read` with either is the read-write mode.
        let both = self.b.build_and(read, writes, "both").map_err(llvm_err)?;
        let plain = self
            .b
            .build_select(writes, flag(c.write_only), flag(c.read_only), "access")
            .map_err(llvm_err)?
            .into_int_value();
        let mut flags =
            self.b.build_select(both, flag(c.read_write), plain, "flags").map_err(llvm_err)?.into_int_value();
        let append_flag = self.b.build_select(append, flag(c.append), zero, "").map_err(llvm_err)?.into_int_value();
        flags = self.b.build_or(flags, append_flag, "").map_err(llvm_err)?;
        let truncate_flag =
            self.b.build_select(truncate, flag(c.truncate), zero, "").map_err(llvm_err)?.into_int_value();
        flags = self.b.build_or(flags, truncate_flag, "").map_err(llvm_err)?;
        // `create_new` is `create` plus `exclusive`; whichever is set, the file is made when it is missing.
        let exclusive = flag(c.create | c.exclusive);
        let created_plain = self.b.build_select(create, flag(c.create), zero, "").map_err(llvm_err)?.into_int_value();
        let created = self.b.build_select(create_new, exclusive, created_plain, "").map_err(llvm_err)?.into_int_value();
        flags = self.b.build_or(flags, created, "").map_err(llvm_err)?;
        if self.windows {
            flags = self.b.build_or(flags, flag(c.binary), "").map_err(llvm_err)?;
        }
        let cpath = self.cstring_of(path, plen, malloc)?;
        let mode = i32_ty.const_int(open_create_mode(self.windows), false);
        // `open` and `_open` are variadic: the mode is an unnamed argument. The declaration must say so, because
        // Darwin's arm64 ABI passes unnamed arguments on the stack; a fixed three-argument declaration would lose
        // the mode there and create the file with whatever the stack held.
        let open_ty = i32_ty.fn_type(&[ptr_ty.into(), i32_ty.into()], true);
        let fd = if self.windows {
            let open = libc(self.module, "_open", open_ty);
            self.call(open, &[cpath.into(), flags.into(), mode.into()])?
        } else {
            let open = libc(self.module, "open", open_ty);
            self.call(open, &[cpath.into(), flags.into(), mode.into()])?
        };
        let fd = fd.expect("open returns an int").into_int_value();
        // The descriptor is -1 on failure; keep `errno` before `free`, which may set its own.
        let code = self.last_error(true)?;
        self.call(free, &[cpath.into()])?;
        let failed =
            self.b.build_int_compare(IntPredicate::SLT, fd, i32_ty.const_zero(), "failed").map_err(llvm_err)?;
        let stored = self.b.build_select(failed, code, i32_ty.const_zero(), "file.code").map_err(llvm_err)?;
        self.b.build_store(file_error, stored).map_err(llvm_err)?;
        self.b.build_return(Some(&fd)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `file_read(fd, out: *List<u8>, max) -> isize`: appends up to `max` bytes read from `fd` to the list at
    /// `out`, growing its buffer, and returns how many. Zero is the end of the file and -1 a read error. The list's
    /// length changes only when bytes were read.
    fn file_read(&mut self, file_error: PointerValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let params = [i32_ty.into(), ptr_ty.into(), usize_ty.into()];
        let f = self.function("burdin_rt_file_read", usize_ty.fn_type(&params, false));
        let (fd, out, max) = (Self::int_param(f, 0), Self::ptr_param(f, 1), Self::int_param(f, 2));
        let ptr_field = self.b.build_struct_gep(self.string_ty, out, 0, "list.ptr").map_err(llvm_err)?;
        let len_field = self.b.build_struct_gep(self.string_ty, out, 1, "list.len").map_err(llvm_err)?;
        let cap_field = self.b.build_struct_gep(self.string_ty, out, 2, "list.cap").map_err(llvm_err)?;
        let zero = usize_ty.const_zero();
        let empty = self.b.build_int_compare(IntPredicate::EQ, max, zero, "empty").map_err(llvm_err)?;
        let check = self.block(f, "read.check");
        let none = self.block(f, "read.none");
        self.b.build_conditional_branch(empty, none, check).map_err(llvm_err)?;
        self.b.position_at_end(none);
        self.b.build_return(Some(&zero)).map_err(llvm_err)?;

        self.b.position_at_end(check);
        let len = self.b.build_load(usize_ty, len_field, "len").map_err(llvm_err)?.into_int_value();
        let cap = self.b.build_load(usize_ty, cap_field, "cap").map_err(llvm_err)?.into_int_value();
        let needed = self.b.build_int_add(len, max, "needed").map_err(llvm_err)?;
        let full = self.b.build_int_compare(IntPredicate::ULT, cap, needed, "full").map_err(llvm_err)?;
        let grow = self.block(f, "read.grow");
        let read = self.block(f, "read.read");
        self.b.build_conditional_branch(full, grow, read).map_err(llvm_err)?;

        // Grow the buffer like a `List` does: double it, at least to what the read needs, and at least four bytes.
        self.b.position_at_end(grow);
        let doubled = self.b.build_int_mul(cap, usize_ty.const_int(2, false), "doubled").map_err(llvm_err)?;
        let wide = self
            .b
            .build_select(
                self.b.build_int_compare(IntPredicate::ULT, doubled, needed, "").map_err(llvm_err)?,
                needed,
                doubled,
                "cap",
            )
            .map_err(llvm_err)?
            .into_int_value();
        let grown = self
            .b
            .build_select(
                self.b
                    .build_int_compare(IntPredicate::ULT, wide, usize_ty.const_int(4, false), "")
                    .map_err(llvm_err)?,
                usize_ty.const_int(4, false),
                wide,
                "cap",
            )
            .map_err(llvm_err)?
            .into_int_value();
        let data = self.b.build_load(ptr_ty, ptr_field, "ptr").map_err(llvm_err)?.into_pointer_value();
        let fresh = self.call_rt_realloc(data, grown)?;
        self.b.build_store(ptr_field, fresh).map_err(llvm_err)?;
        self.b.build_store(cap_field, grown).map_err(llvm_err)?;
        self.b.build_unconditional_branch(read).map_err(llvm_err)?;

        self.b.position_at_end(read);
        let len = self.b.build_load(usize_ty, len_field, "len").map_err(llvm_err)?.into_int_value();
        let data = self.b.build_load(ptr_ty, ptr_field, "ptr").map_err(llvm_err)?.into_pointer_value();
        let dest = self.offset(data, len)?;
        let n = if self.windows {
            let read =
                libc(self.module, "_read", i32_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), i32_ty.into()], false));
            let count = self.b.build_int_truncate(max, i32_ty, "count").map_err(llvm_err)?;
            let raw = self
                .call(read, &[fd.into(), dest.into(), count.into()])?
                .expect("_read returns an int")
                .into_int_value();
            self.b.build_int_s_extend(raw, usize_ty, "n").map_err(llvm_err)?
        } else {
            let read =
                libc(self.module, "read", usize_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), usize_ty.into()], false));
            self.call(read, &[fd.into(), dest.into(), max.into()])?.expect("read returns a size").into_int_value()
        };
        let failed = self.b.build_int_compare(IntPredicate::SLT, n, zero, "failed").map_err(llvm_err)?;
        let failed_block = self.block(f, "read.failed");
        let done = self.block(f, "read.done");
        self.b.build_conditional_branch(failed, failed_block, done).map_err(llvm_err)?;
        self.b.position_at_end(failed_block);
        let code = self.last_error(true)?;
        self.b.build_store(file_error, code).map_err(llvm_err)?;
        self.b.build_return(Some(&usize_ty.const_all_ones())).map_err(llvm_err)?;

        self.b.position_at_end(done);
        let next = self.b.build_int_add(len, n, "next").map_err(llvm_err)?;
        self.b.build_store(len_field, next).map_err(llvm_err)?;
        self.b.build_return(Some(&n)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `file_write(fd, data, len: usize) -> isize`: writes a prefix of `data` and returns how many bytes the system
    /// took, or -1. A short write is not an error; `Write.write_all` calls again for the rest.
    fn file_write(&mut self, file_error: PointerValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let params = [i32_ty.into(), ptr_ty.into(), usize_ty.into()];
        let f = self.function("burdin_rt_file_write", usize_ty.fn_type(&params, false));
        let (fd, data, len) = (Self::int_param(f, 0), Self::ptr_param(f, 1), Self::int_param(f, 2));
        let zero = usize_ty.const_zero();
        let n = if self.windows {
            let write =
                libc(self.module, "_write", i32_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), i32_ty.into()], false));
            let count = self.b.build_int_truncate(len, i32_ty, "count").map_err(llvm_err)?;
            let raw = self
                .call(write, &[fd.into(), data.into(), count.into()])?
                .expect("_write returns an int")
                .into_int_value();
            self.b.build_int_s_extend(raw, usize_ty, "n").map_err(llvm_err)?
        } else {
            let write =
                libc(self.module, "write", usize_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), usize_ty.into()], false));
            self.call(write, &[fd.into(), data.into(), len.into()])?.expect("write returns a size").into_int_value()
        };
        let code = self.last_error(true)?;
        let failed = self.b.build_int_compare(IntPredicate::SLT, n, zero, "failed").map_err(llvm_err)?;
        let stored = self.b.build_select(failed, code, i32_ty.const_zero(), "file.code").map_err(llvm_err)?;
        self.b.build_store(file_error, stored).map_err(llvm_err)?;
        self.b.build_return(Some(&n)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `file_seek(fd, offset: i64, whence: i32) -> i64`: the new position, or -1.
    fn file_seek(&mut self, file_error: PointerValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (i32_ty, i64_ty) = (self.i32(), self.i64());
        let params = [i32_ty.into(), i64_ty.into(), i32_ty.into()];
        let f = self.function("burdin_rt_file_seek", i64_ty.fn_type(&params, false));
        let (fd, offset, whence) = (Self::int_param(f, 0), Self::int_param(f, 1), Self::int_param(f, 2));
        let position = if self.windows {
            let seek =
                libc(self.module, "_lseeki64", i64_ty.fn_type(&[i32_ty.into(), i64_ty.into(), i32_ty.into()], false));
            self.call(seek, &[fd.into(), offset.into(), whence.into()])?
        } else {
            let seek =
                libc(self.module, "lseek", i64_ty.fn_type(&[i32_ty.into(), i64_ty.into(), i32_ty.into()], false));
            self.call(seek, &[fd.into(), offset.into(), whence.into()])?
        };
        let position = position.expect("lseek returns a position").into_int_value();
        let code = self.last_error(true)?;
        let minus_one = i64_ty.const_all_ones();
        let failed = self.b.build_int_compare(IntPredicate::EQ, position, minus_one, "failed").map_err(llvm_err)?;
        let stored = self.b.build_select(failed, code, i32_ty.const_zero(), "file.code").map_err(llvm_err)?;
        self.b.build_store(file_error, stored).map_err(llvm_err)?;
        self.b.build_return(Some(&position)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `file_sync(fd) -> i32`: `fsync` / `_commit`; 0 on success.
    fn file_sync(&mut self, file_error: PointerValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let i32_ty = self.i32();
        let f = self.function("burdin_rt_file_sync", i32_ty.fn_type(&[i32_ty.into()], false));
        let fd = Self::int_param(f, 0);
        let sync = if self.windows {
            let commit = libc(self.module, "_commit", i32_ty.fn_type(&[i32_ty.into()], false));
            self.call(commit, &[fd.into()])?
        } else {
            let fsync = libc(self.module, "fsync", i32_ty.fn_type(&[i32_ty.into()], false));
            self.call(fsync, &[fd.into()])?
        };
        let status = sync.expect("sync returns an int").into_int_value();
        let code = self.last_error(true)?;
        let failed = self.b.build_int_compare(IntPredicate::NE, status, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let stored = self.b.build_select(failed, code, i32_ty.const_zero(), "file.code").map_err(llvm_err)?;
        self.b.build_store(file_error, stored).map_err(llvm_err)?;
        self.b.build_return(Some(&status)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `file_set_len(fd, size: u64) -> i32`: `ftruncate` / `_chsize_s`; 0 on success.
    fn file_set_len(&mut self, file_error: PointerValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (i32_ty, i64_ty) = (self.i32(), self.i64());
        let params = [i32_ty.into(), i64_ty.into()];
        let f = self.function("burdin_rt_file_set_len", i32_ty.fn_type(&params, false));
        let (fd, size) = (Self::int_param(f, 0), Self::int_param(f, 1));
        let status = if self.windows {
            let chsize = libc(self.module, "_chsize_s", i32_ty.fn_type(&[i32_ty.into(), i64_ty.into()], false));
            self.call(chsize, &[fd.into(), size.into()])?
        } else {
            let truncate = libc(self.module, "ftruncate", i32_ty.fn_type(&[i32_ty.into(), i64_ty.into()], false));
            self.call(truncate, &[fd.into(), size.into()])?
        };
        let status = status.expect("the resize returns an int").into_int_value();
        let code = self.last_error(true)?;
        let failed = self.b.build_int_compare(IntPredicate::NE, status, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let stored = self.b.build_select(failed, code, i32_ty.const_zero(), "file.code").map_err(llvm_err)?;
        self.b.build_store(file_error, stored).map_err(llvm_err)?;
        self.b.build_return(Some(&status)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `file_close(fd) -> i32`: `close` / `_close`; 0 on success.
    fn file_close(&mut self, file_error: PointerValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let i32_ty = self.i32();
        let f = self.function("burdin_rt_file_close", i32_ty.fn_type(&[i32_ty.into()], false));
        let fd = Self::int_param(f, 0);
        let close = if self.windows {
            let close = libc(self.module, "_close", i32_ty.fn_type(&[i32_ty.into()], false));
            self.call(close, &[fd.into()])?
        } else {
            let close = libc(self.module, "close", i32_ty.fn_type(&[i32_ty.into()], false));
            self.call(close, &[fd.into()])?
        };
        let status = close.expect("close returns an int").into_int_value();
        let code = self.last_error(true)?;
        let failed = self.b.build_int_compare(IntPredicate::NE, status, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let stored = self.b.build_select(failed, code, i32_ty.const_zero(), "file.code").map_err(llvm_err)?;
        self.b.build_store(file_error, stored).map_err(llvm_err)?;
        self.b.build_return(Some(&status)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `file_error() -> i32`: the OS code of the last failed file operation.
    fn file_error(&mut self, global: PointerValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let i32_ty = self.i32();
        let f = self.function("burdin_rt_file_error", i32_ty.fn_type(&[], false));
        let code = self.b.build_load(i32_ty, global, "file.error").map_err(llvm_err)?;
        self.b.build_return(Some(&code)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `read_dir(path, path_len, out: *List<String>) -> i32`: the entry names; 0, or the OS error code. `.`
    /// and `..` are skipped. Every failure leaves an empty, valid list in `out`.
    fn read_dir(&mut self, h: &Helpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let params = [ptr_ty.into(), usize_ty.into(), ptr_ty.into()];
        let f = self.function("burdin_rt_read_dir", i32_ty.fn_type(&params, false));
        let (path, plen, out) = (Self::ptr_param(f, 0), Self::int_param(f, 1), Self::ptr_param(f, 2));
        for (i, name) in ["list.ptr", "list.len", "list.cap"].iter().enumerate() {
            let field = self.b.build_struct_gep(self.string_ty, out, i as u32, name).map_err(llvm_err)?;
            if i == 0 {
                self.b.build_store(field, ptr_ty.const_null()).map_err(llvm_err)?;
            } else {
                self.b.build_store(field, usize_ty.const_zero()).map_err(llvm_err)?;
            }
        }
        let cpath = self.cstring_of(path, plen, malloc)?;
        if self.windows {
            self.read_dir_windows(f, cpath, free, out, h)?;
        } else {
            self.read_dir_unix(f, cpath, free, out, h)?;
        }
        Ok(f)
    }

    /// The POSIX half of `read_dir`: `opendir`/`readdir`/`closedir` and `d_name` at the host's offset.
    fn read_dir_unix(
        &mut self,
        f: FunctionValue<'ctx>,
        path: PointerValue<'ctx>,
        free: FunctionValue<'ctx>,
        out: PointerValue<'ctx>,
        h: &Helpers<'ctx>,
    ) -> GenResult<()> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let opendir = libc(self.module, "opendir", ptr_ty.fn_type(&[ptr_ty.into()], false));
        let readdir = libc(self.module, "readdir", ptr_ty.fn_type(&[ptr_ty.into()], false));
        let closedir = libc(self.module, "closedir", i32_ty.fn_type(&[ptr_ty.into()], false));
        let strlen = libc(self.module, "strlen", usize_ty.fn_type(&[ptr_ty.into()], false));
        let dir = self.call(opendir, &[path.into()])?.expect("opendir returns a pointer").into_pointer_value();
        let missing = self.b.build_is_null(dir, "").map_err(llvm_err)?;
        let open = self.block(f, "read_dir.open");
        let fail = self.block(f, "read_dir.fail");
        self.b.build_conditional_branch(missing, fail, open).map_err(llvm_err)?;
        self.b.position_at_end(fail);
        let code = self.last_error(true)?;
        self.call(free, &[path.into()])?;
        self.b.build_return(Some(&code)).map_err(llvm_err)?;

        self.b.position_at_end(open);
        let scan = self.block(f, "read_dir.scan");
        let entry = self.block(f, "read_dir.entry");
        let append = self.block(f, "read_dir.append");
        let scan_end = self.block(f, "read_dir.scan_end");
        let read_fail = self.block(f, "read_dir.read_fail");
        let done = self.block(f, "read_dir.done");
        self.b.build_unconditional_branch(scan).map_err(llvm_err)?;

        self.b.position_at_end(scan);
        // `readdir` returns NULL both at the end of the listing and on an error, and only sets `errno` for the
        // error; zero it first so the two can be told apart.
        let errno = self.errno_ptr()?;
        self.b.build_store(errno, i32_ty.const_zero()).map_err(llvm_err)?;
        let ent = self.call(readdir, &[dir.into()])?.expect("readdir returns a pointer").into_pointer_value();
        let end = self.b.build_is_null(ent, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(end, scan_end, entry).map_err(llvm_err)?;

        self.b.position_at_end(scan_end);
        let code = self.b.build_load(i32_ty, errno, "readdir.errno").map_err(llvm_err)?.into_int_value();
        let bad = self.b.build_int_compare(IntPredicate::NE, code, i32_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(bad, read_fail, done).map_err(llvm_err)?;

        self.b.position_at_end(read_fail);
        self.call(free, &[path.into()])?;
        self.b.build_return(Some(&code)).map_err(llvm_err)?;

        self.b.position_at_end(entry);
        let name = self.offset(ent, usize_ty.const_int(DIRENT_NAME_OFFSET, false))?;
        let nlen = self.call(strlen, &[name.into()])?.expect("strlen returns a size").into_int_value();
        let skip = self.is_dot_or_dotdot(name, nlen)?;
        self.b.build_conditional_branch(skip, scan, append).map_err(llvm_err)?;

        self.b.position_at_end(append);
        self.list_append_name(f, out, name, nlen, h)?;
        self.b.build_unconditional_branch(scan).map_err(llvm_err)?;

        self.b.position_at_end(done);
        self.call(closedir, &[dir.into()])?;
        self.call(free, &[path.into()])?;
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;
        Ok(())
    }

    /// The Windows half of `read_dir`: `FindFirstFileA` over `path\*`, with `WIN32_FIND_DATAA`'s layout.
    fn read_dir_windows(
        &mut self,
        f: FunctionValue<'ctx>,
        path: PointerValue<'ctx>,
        free: FunctionValue<'ctx>,
        out: PointerValue<'ctx>,
        h: &Helpers<'ctx>,
    ) -> GenResult<()> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let strlen = libc(self.module, "strlen", usize_ty.fn_type(&[ptr_ty.into()], false));
        let find_first = libc(self.module, "FindFirstFileA", ptr_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
        let find_next = libc(self.module, "FindNextFileA", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
        let find_close = libc(self.module, "FindClose", i32_ty.fn_type(&[ptr_ty.into()], false));
        let len = self.call(strlen, &[path.into()])?.expect("strlen returns a size").into_int_value();
        // `path` + (`\` unless it already ends in a separator) + `*` + NUL. The empty path reads its own
        // NUL byte as the "last" one, which never needs a separator.
        let has_char = self.b.build_int_compare(IntPredicate::UGT, len, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let last_index = self
            .b
            .build_select(
                has_char,
                self.b.build_int_sub(len, usize_ty.const_int(1, false), "").map_err(llvm_err)?,
                usize_ty.const_zero(),
                "",
            )
            .map_err(llvm_err)?
            .into_int_value();
        let last_at = self.offset(path, last_index)?;
        let last = self.b.build_load(self.i8(), last_at, "path.last").map_err(llvm_err)?.into_int_value();
        let i8_ty = self.i8();
        let slash = self
            .b
            .build_int_compare(IntPredicate::EQ, last, i8_ty.const_int(u64::from(b'/'), false), "")
            .map_err(llvm_err)?;
        let backslash = self
            .b
            .build_int_compare(IntPredicate::EQ, last, i8_ty.const_int(u64::from(b'\\'), false), "")
            .map_err(llvm_err)?;
        let separator = self.b.build_or(slash, backslash, "").map_err(llvm_err)?;
        let needs_sep =
            self.b.build_and(has_char, self.b.build_not(separator, "").map_err(llvm_err)?, "").map_err(llvm_err)?;
        let extra = self
            .b
            .build_select(needs_sep, usize_ty.const_int(1, false), usize_ty.const_zero(), "")
            .map_err(llvm_err)?
            .into_int_value();
        let total = self.b.build_int_add(len, extra, "").map_err(llvm_err)?;
        let total = self.b.build_int_add(total, usize_ty.const_int(2, false), "").map_err(llvm_err)?;
        let pattern = self.call_rt_alloc(total)?;
        self.b.build_memcpy(pattern, 1, path, 1, len).map_err(|e| format!("LLVM builder error: {e}"))?;
        let sep_at = self.offset(pattern, len)?;
        let sep_byte = self
            .b
            .build_select(needs_sep, i8_ty.const_int(u64::from(b'\\'), false), i8_ty.const_zero(), "")
            .map_err(llvm_err)?;
        self.b.build_store(sep_at, sep_byte).map_err(llvm_err)?;
        let star_at = self.offset(pattern, self.b.build_int_add(len, extra, "").map_err(llvm_err)?)?;
        self.b.build_store(star_at, i8_ty.const_int(u64::from(b'*'), false)).map_err(llvm_err)?;
        let nul_at =
            self.offset(pattern, self.b.build_int_sub(total, usize_ty.const_int(1, false), "").map_err(llvm_err)?)?;
        self.b.build_store(nul_at, i8_ty.const_zero()).map_err(llvm_err)?;

        let data = self.b.build_alloca(self.i64().array_type(FIND_DATA_WORDS), "find.data").map_err(llvm_err)?;
        let handle = self
            .call(find_first, &[pattern.into(), data.into()])?
            .expect("FindFirstFileA returns a handle")
            .into_pointer_value();
        let handle_int = self.b.build_ptr_to_int(handle, usize_ty, "find.handle").map_err(llvm_err)?;
        let invalid =
            self.b.build_int_compare(IntPredicate::EQ, handle_int, usize_ty.const_all_ones(), "").map_err(llvm_err)?;
        let open = self.block(f, "read_dir.open");
        let fail = self.block(f, "read_dir.fail");
        self.b.build_conditional_branch(invalid, fail, open).map_err(llvm_err)?;
        self.b.position_at_end(fail);
        let code = self.last_error(false)?;
        self.call_rt_free(pattern)?;
        self.call(free, &[path.into()])?;
        self.b.build_return(Some(&code)).map_err(llvm_err)?;

        self.b.position_at_end(open);
        let entry = self.block(f, "read_dir.entry");
        let append = self.block(f, "read_dir.append");
        let next = self.block(f, "read_dir.next");
        let next_end = self.block(f, "read_dir.next_end");
        let next_fail = self.block(f, "read_dir.next_fail");
        let done = self.block(f, "read_dir.done");
        self.b.build_unconditional_branch(entry).map_err(llvm_err)?;

        self.b.position_at_end(entry);
        let name = self.offset(data, usize_ty.const_int(FIND_DATA_FILE_NAME_OFFSET, false))?;
        let nlen = self.call(strlen, &[name.into()])?.expect("strlen returns a size").into_int_value();
        let skip = self.is_dot_or_dotdot(name, nlen)?;
        self.b.build_conditional_branch(skip, next, append).map_err(llvm_err)?;

        self.b.position_at_end(append);
        self.list_append_name(f, out, name, nlen, h)?;
        self.b.build_unconditional_branch(next).map_err(llvm_err)?;

        self.b.position_at_end(next);
        let more = self
            .call(find_next, &[handle.into(), data.into()])?
            .expect("FindNextFileA returns a flag")
            .into_int_value();
        let has_more = self.b.build_int_compare(IntPredicate::NE, more, i32_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(has_more, entry, next_end).map_err(llvm_err)?;

        // `ERROR_NO_MORE_FILES` is the normal end of the listing; any other code is a failure.
        self.b.position_at_end(next_end);
        let code = self.last_error(false)?;
        let last = self
            .b
            .build_int_compare(
                IntPredicate::EQ,
                code,
                i32_ty.const_int(u64::from(ERROR_NO_MORE_FILES), false),
                "no.more",
            )
            .map_err(llvm_err)?;
        self.b.build_conditional_branch(last, done, next_fail).map_err(llvm_err)?;

        self.b.position_at_end(next_fail);
        self.call(find_close, &[handle.into()])?;
        self.call_rt_free(pattern)?;
        self.call(free, &[path.into()])?;
        self.b.build_return(Some(&code)).map_err(llvm_err)?;

        self.b.position_at_end(done);
        self.call(find_close, &[handle.into()])?;
        self.call_rt_free(pattern)?;
        self.call(free, &[path.into()])?;
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;
        Ok(())
    }

    /// Whether the NUL-terminated `name` of `nlen` bytes is `.` or `..`.
    fn is_dot_or_dotdot(&mut self, name: PointerValue<'ctx>, nlen: IntValue<'ctx>) -> GenResult<IntValue<'ctx>> {
        let (usize_ty, i8_ty) = (self.usize_ty, self.i8());
        let dot = i8_ty.const_int(u64::from(b'.'), false);
        let first = self.b.build_load(i8_ty, name, "name.first").map_err(llvm_err)?.into_int_value();
        let first_dot = self.b.build_int_compare(IntPredicate::EQ, first, dot, "").map_err(llvm_err)?;
        let one =
            self.b.build_int_compare(IntPredicate::EQ, nlen, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        let two =
            self.b.build_int_compare(IntPredicate::EQ, nlen, usize_ty.const_int(2, false), "").map_err(llvm_err)?;
        let second_at = self.offset(name, usize_ty.const_int(1, false))?;
        let second = self.b.build_load(i8_ty, second_at, "name.second").map_err(llvm_err)?.into_int_value();
        let second_dot = self.b.build_int_compare(IntPredicate::EQ, second, dot, "").map_err(llvm_err)?;
        let parent = self.b.build_and(two, second_dot, "").map_err(llvm_err)?;
        let dot_or_parent = self.b.build_or(one, parent, "").map_err(llvm_err)?;
        self.b.build_and(first_dot, dot_or_parent, "name.skip").map_err(llvm_err)
    }

    /// Appends the NUL-terminated `name` of `nlen` bytes to the `{ptr, len, cap}` list at `out`, growing the
    /// buffer when it is full. The builder is left in the block after the append.
    fn list_append_name(
        &mut self,
        f: FunctionValue<'ctx>,
        out: PointerValue<'ctx>,
        name: PointerValue<'ctx>,
        nlen: IntValue<'ctx>,
        h: &Helpers<'ctx>,
    ) -> GenResult<()> {
        let (usize_ty, ptr_ty) = (self.usize_ty, self.ptr_ty);
        let elem_size = 3 * u64::from(self.usize_ty.get_bit_width() / 8);
        let ptr_field = self.b.build_struct_gep(self.string_ty, out, 0, "list.ptr").map_err(llvm_err)?;
        let len_field = self.b.build_struct_gep(self.string_ty, out, 1, "list.len").map_err(llvm_err)?;
        let cap_field = self.b.build_struct_gep(self.string_ty, out, 2, "list.cap").map_err(llvm_err)?;
        let len = self.b.build_load(usize_ty, len_field, "").map_err(llvm_err)?.into_int_value();
        let cap = self.b.build_load(usize_ty, cap_field, "").map_err(llvm_err)?.into_int_value();
        let full = self.b.build_int_compare(IntPredicate::EQ, len, cap, "").map_err(llvm_err)?;
        let grow = self.block(f, "list.grow");
        let append = self.block(f, "list.append");
        self.b.build_conditional_branch(full, grow, append).map_err(llvm_err)?;

        self.b.position_at_end(grow);
        let empty = self.b.build_int_compare(IntPredicate::EQ, cap, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let twice = self.b.build_int_mul(cap, usize_ty.const_int(2, false), "").map_err(llvm_err)?;
        let grown = self
            .b
            .build_select(empty, usize_ty.const_int(8, false), twice, "list.cap")
            .map_err(llvm_err)?
            .into_int_value();
        let old = self.b.build_load(ptr_ty, ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        let bytes = self.b.build_int_mul(grown, usize_ty.const_int(elem_size, false), "").map_err(llvm_err)?;
        let fresh = self.call_rt_realloc(old, bytes)?;
        self.b.build_store(ptr_field, fresh).map_err(llvm_err)?;
        self.b.build_store(cap_field, grown).map_err(llvm_err)?;
        self.b.build_unconditional_branch(append).map_err(llvm_err)?;

        self.b.position_at_end(append);
        let len = self.b.build_load(usize_ty, len_field, "").map_err(llvm_err)?.into_int_value();
        let ptr = self.b.build_load(ptr_ty, ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        let elem = unsafe { self.b.build_in_bounds_gep(self.string_ty, ptr, &[len], "list.elem") }.map_err(llvm_err)?;
        self.call(h.alloc_string, &[elem.into(), name.into(), nlen.into()])?;
        let next = self.b.build_int_add(len, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_store(len_field, next).map_err(llvm_err)?;
        Ok(())
    }

    /// `create_dir(path, path_len) -> i32`: `mkdir`/`_mkdir`; 0, or the OS error code.
    fn create_dir(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let f = self.function("burdin_rt_create_dir", i32_ty.fn_type(&[ptr_ty.into(), usize_ty.into()], false));
        let (path, plen) = (Self::ptr_param(f, 0), Self::int_param(f, 1));
        let cpath = self.cstring_of(path, plen, malloc)?;
        let status = if self.windows {
            let mkdir = libc(self.module, "_mkdir", i32_ty.fn_type(&[ptr_ty.into()], false));
            self.call(mkdir, &[cpath.into()])?
        } else {
            let mkdir = libc(self.module, "mkdir", i32_ty.fn_type(&[ptr_ty.into(), i32_ty.into()], false));
            self.call(mkdir, &[cpath.into(), i32_ty.const_int(0o777, false).into()])?
        };
        let status = status.expect("mkdir returns an int").into_int_value();
        let code = self.last_error(true)?;
        self.call(free, &[cpath.into()])?;
        let ok = self.b.build_int_compare(IntPredicate::EQ, status, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let result = self.b.build_select(ok, i32_ty.const_zero(), code, "mkdir.status").map_err(llvm_err)?;
        self.b.build_return(Some(&result)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `remove_dir(path, path_len) -> i32`: `rmdir`/`_rmdir`.
    fn remove_dir(&mut self) -> GenResult<FunctionValue<'ctx>> {
        self.remove_path("burdin_rt_remove_dir", "rmdir", "_rmdir")
    }

    /// `remove_file(path, path_len) -> i32`: `unlink`/`_unlink` (a symbolic link is removed, not its target).
    fn remove_file(&mut self) -> GenResult<FunctionValue<'ctx>> {
        self.remove_path("burdin_rt_remove_file", "unlink", "_unlink")
    }

    /// A one-path removal: `(path, path_len) -> i32`; 0, or the OS error code.
    fn remove_path(&mut self, name: &str, unix: &str, windows: &str) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let f = self.function(name, i32_ty.fn_type(&[ptr_ty.into(), usize_ty.into()], false));
        let (path, plen) = (Self::ptr_param(f, 0), Self::int_param(f, 1));
        let cpath = self.cstring_of(path, plen, malloc)?;
        let symbol = if self.windows { windows } else { unix };
        let remove = libc(self.module, symbol, i32_ty.fn_type(&[ptr_ty.into()], false));
        let status = self.call(remove, &[cpath.into()])?.expect("the removal returns an int").into_int_value();
        let code = self.last_error(true)?;
        self.call(free, &[cpath.into()])?;
        let ok = self.b.build_int_compare(IntPredicate::EQ, status, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let result = self.b.build_select(ok, i32_ty.const_zero(), code, "remove.status").map_err(llvm_err)?;
        self.b.build_return(Some(&result)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `rename(from, from_len, to, to_len) -> i32`: `rename`, or `MoveFileExA` with
    /// `MOVEFILE_REPLACE_EXISTING` on Windows; 0, or the OS error code.
    fn rename(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let params = [ptr_ty.into(), usize_ty.into(), ptr_ty.into(), usize_ty.into()];
        let f = self.function("burdin_rt_rename", i32_ty.fn_type(&params, false));
        let (from, flen) = (Self::ptr_param(f, 0), Self::int_param(f, 1));
        let (to, tlen) = (Self::ptr_param(f, 2), Self::int_param(f, 3));
        let cfrom = self.cstring_of(from, flen, malloc)?;
        let cto = self.cstring_of(to, tlen, malloc)?;
        if self.windows {
            let move_file =
                libc(self.module, "MoveFileExA", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into(), i32_ty.into()], false));
            let ok = self
                .call(
                    move_file,
                    &[cfrom.into(), cto.into(), i32_ty.const_int(MOVEFILE_REPLACE_EXISTING, false).into()],
                )?
                .expect("MoveFileExA returns a flag")
                .into_int_value();
            let code = self.last_error(false)?;
            self.call(free, &[cfrom.into()])?;
            self.call(free, &[cto.into()])?;
            let moved = self.b.build_int_compare(IntPredicate::NE, ok, i32_ty.const_zero(), "").map_err(llvm_err)?;
            let status = self.b.build_select(moved, i32_ty.const_zero(), code, "rename.status").map_err(llvm_err)?;
            self.b.build_return(Some(&status)).map_err(llvm_err)?;
        } else {
            let rename = libc(self.module, "rename", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
            let status =
                self.call(rename, &[cfrom.into(), cto.into()])?.expect("rename returns an int").into_int_value();
            let code = self.last_error(true)?;
            self.call(free, &[cfrom.into()])?;
            self.call(free, &[cto.into()])?;
            let ok = self.b.build_int_compare(IntPredicate::EQ, status, i32_ty.const_zero(), "").map_err(llvm_err)?;
            let status = self.b.build_select(ok, i32_ty.const_zero(), code, "rename.status").map_err(llvm_err)?;
            self.b.build_return(Some(&status)).map_err(llvm_err)?;
        }
        Ok(f)
    }

    /// `metadata(path, path_len, out: *RawMetadata) -> i32`: five 8-byte words, `{ size, mode, modified,
    /// accessed, flags }`. The entry itself is read (`lstat` on Unix), so a symbolic link reports its own kind.
    fn metadata(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty, i64_ty) = (self.usize_ty, self.ptr_ty, self.i32(), self.i64());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let symbol = if self.windows {
            "_stat64"
        } else if self.darwin && cfg!(target_arch = "x86_64") {
            // The modern 64-bit-inode layout; plain `stat` is the legacy struct on Intel macOS.
            "lstat$INODE64"
        } else {
            "lstat"
        };
        let stat = libc(self.module, symbol, i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
        let params = [ptr_ty.into(), usize_ty.into(), ptr_ty.into()];
        let f = self.function("burdin_rt_metadata", i32_ty.fn_type(&params, false));
        let (path, plen, out) = (Self::ptr_param(f, 0), Self::int_param(f, 1), Self::ptr_param(f, 2));
        let cpath = self.cstring_of(path, plen, malloc)?;
        let buf = self.b.build_alloca(i64_ty.array_type(STAT_WORDS), "stat.buf").map_err(llvm_err)?;
        let status = self.call(stat, &[cpath.into(), buf.into()])?.expect("stat returns an int").into_int_value();
        let ok = self.b.build_int_compare(IntPredicate::EQ, status, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let fill = self.block(f, "meta.fill");
        let fail = self.block(f, "meta.fail");
        self.b.build_conditional_branch(ok, fill, fail).map_err(llvm_err)?;
        self.b.position_at_end(fail);
        let code = self.last_error(true)?;
        self.call(free, &[cpath.into()])?;
        self.b.build_return(Some(&code)).map_err(llvm_err)?;

        self.b.position_at_end(fill);
        let load = |b: &Builder<'ctx>, offset: u64, ty: IntType<'ctx>, name: &str| -> GenResult<IntValue<'ctx>> {
            let at = self.offset(buf, usize_ty.const_int(offset, false))?;
            Ok(b.build_load(ty, at, name).map_err(llvm_err)?.into_int_value())
        };
        let size = load(&self.b, STAT_SIZE_OFFSET, i64_ty, "stat.size")?;
        let mode_ty = if STAT_MODE_BYTES == 2 { self.cx.i16_type() } else { self.cx.i32_type() };
        let mode_raw = load(&self.b, STAT_MODE_OFFSET, mode_ty, "stat.mode")?;
        let mode = self.b.build_int_z_extend(mode_raw, i64_ty, "stat.mode.wide").map_err(llvm_err)?;
        let modified = load(&self.b, STAT_MTIME_OFFSET, i64_ty, "stat.mtime")?;
        let accessed = load(&self.b, STAT_ATIME_OFFSET, i64_ty, "stat.atime")?;
        let kind = self.b.build_and(mode, i64_ty.const_int(S_IFMT, false), "stat.kind").map_err(llvm_err)?;
        let is_file =
            self.b.build_int_compare(IntPredicate::EQ, kind, i64_ty.const_int(S_IFREG, false), "").map_err(llvm_err)?;
        let is_dir =
            self.b.build_int_compare(IntPredicate::EQ, kind, i64_ty.const_int(S_IFDIR, false), "").map_err(llvm_err)?;
        let is_symlink = if self.windows {
            // `_stat64` follows a link, so the kind comes from the entry's attributes instead.
            let attributes = libc(self.module, "GetFileAttributesA", i32_ty.fn_type(&[ptr_ty.into()], false));
            let attrs =
                self.call(attributes, &[cpath.into()])?.expect("GetFileAttributesA returns an int").into_int_value();
            let invalid = self
                .b
                .build_int_compare(IntPredicate::EQ, attrs, i32_ty.const_int(INVALID_FILE_ATTRIBUTES, false), "")
                .map_err(llvm_err)?;
            let reparse =
                self.b.build_and(attrs, i32_ty.const_int(FILE_ATTRIBUTE_REPARSE_POINT, false), "").map_err(llvm_err)?;
            let is_link =
                self.b.build_int_compare(IntPredicate::NE, reparse, i32_ty.const_zero(), "").map_err(llvm_err)?;
            self.b.build_and(self.b.build_not(invalid, "").map_err(llvm_err)?, is_link, "").map_err(llvm_err)?
        } else {
            self.b.build_int_compare(IntPredicate::EQ, kind, i64_ty.const_int(S_IFLNK, false), "").map_err(llvm_err)?
        };
        let no = self.cx.bool_type().const_zero();
        let file_flag = self.b.build_select(is_symlink, no, is_file, "").map_err(llvm_err)?.into_int_value();
        let dir_flag = self.b.build_select(is_symlink, no, is_dir, "").map_err(llvm_err)?.into_int_value();
        let file_flag = self.b.build_int_z_extend(file_flag, i64_ty, "").map_err(llvm_err)?;
        let dir_flag = self.b.build_int_z_extend(dir_flag, i64_ty, "").map_err(llvm_err)?;
        let link_flag = self.b.build_int_z_extend(is_symlink, i64_ty, "").map_err(llvm_err)?;
        let dir_flag = self.b.build_left_shift(dir_flag, i64_ty.const_int(1, false), "").map_err(llvm_err)?;
        let link_flag = self.b.build_left_shift(link_flag, i64_ty.const_int(2, false), "").map_err(llvm_err)?;
        let flags = self.b.build_or(file_flag, dir_flag, "").map_err(llvm_err)?;
        let flags = self.b.build_or(flags, link_flag, "").map_err(llvm_err)?;
        for (offset, value) in [(0, size), (8, mode), (16, modified), (24, accessed), (32, flags)] {
            let at = self.offset(out, usize_ty.const_int(offset, false))?;
            self.b.build_store(at, value).map_err(llvm_err)?;
        }
        self.call(free, &[cpath.into()])?;
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;
        Ok(f)
    }

    /// `canonicalize(path, path_len, out: *String) -> i32`: the absolute, normalized path, or -1. On Windows the
    /// result is lexical (`_fullpath`), and the path must exist; elsewhere it is `realpath`, which also resolves
    /// links.
    fn canonicalize(&mut self, h: &Helpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let params = [ptr_ty.into(), usize_ty.into(), ptr_ty.into()];
        let f = self.function("burdin_rt_canonicalize", i32_ty.fn_type(&params, false));
        let (path, plen, out) = (Self::ptr_param(f, 0), Self::int_param(f, 1), Self::ptr_param(f, 2));
        // Every failure leaves an empty, valid `String` behind.
        self.call(h.alloc_string, &[out.into(), ptr_ty.const_null().into(), usize_ty.const_zero().into()])?;
        let cpath = self.cstring_of(path, plen, malloc)?;
        if self.windows {
            // `_fullpath` succeeds even for a path that does not exist; `canonicalize` must not.
            let access = libc(self.module, "_access", i32_ty.fn_type(&[ptr_ty.into(), i32_ty.into()], false));
            let status = self
                .call(access, &[cpath.into(), i32_ty.const_zero().into()])?
                .expect("_access returns an int")
                .into_int_value();
            let missing =
                self.b.build_int_compare(IntPredicate::NE, status, i32_ty.const_zero(), "").map_err(llvm_err)?;
            let resolve = self.block(f, "canon.resolve");
            let fail = self.block(f, "canon.fail");
            self.b.build_conditional_branch(missing, fail, resolve).map_err(llvm_err)?;
            self.b.position_at_end(fail);
            let code = self.last_error(true)?;
            self.call(free, &[cpath.into()])?;
            self.b.build_return(Some(&code)).map_err(llvm_err)?;
            self.b.position_at_end(resolve);
        }
        let resolved = if self.windows {
            let full_path =
                libc(self.module, "_fullpath", ptr_ty.fn_type(&[ptr_ty.into(), ptr_ty.into(), usize_ty.into()], false));
            self.call(full_path, &[ptr_ty.const_null().into(), cpath.into(), usize_ty.const_zero().into()])?
                .expect("_fullpath returns a pointer")
                .into_pointer_value()
        } else {
            let realpath = libc(self.module, "realpath", ptr_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
            self.call(realpath, &[cpath.into(), ptr_ty.const_null().into()])?
                .expect("realpath returns a pointer")
                .into_pointer_value()
        };
        let missing = self.b.build_is_null(resolved, "").map_err(llvm_err)?;
        let ok = self.block(f, "canon.ok");
        let fail = self.block(f, "canon.fail");
        self.b.build_conditional_branch(missing, fail, ok).map_err(llvm_err)?;
        self.b.position_at_end(fail);
        let code = self.last_error(true)?;
        self.call(free, &[cpath.into()])?;
        self.b.build_return(Some(&code)).map_err(llvm_err)?;

        self.b.position_at_end(ok);
        let strlen = libc(self.module, "strlen", usize_ty.fn_type(&[ptr_ty.into()], false));
        let len = self.call(strlen, &[resolved.into()])?.expect("strlen returns a size").into_int_value();
        self.call(h.alloc_string, &[out.into(), resolved.into(), len.into()])?;
        self.call(free, &[resolved.into()])?;
        self.call(free, &[cpath.into()])?;
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;
        Ok(f)
    }

    /// The path bytes copied into a `malloc`ed buffer with a terminating NUL.
    fn cstring_of(
        &mut self,
        ptr: PointerValue<'ctx>,
        len: IntValue<'ctx>,
        malloc: FunctionValue<'ctx>,
    ) -> GenResult<PointerValue<'ctx>> {
        let usize_ty = self.usize_ty;
        let size = self.b.build_int_add(len, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        let p = self.call(malloc, &[size.into()])?.expect("malloc returns a pointer").into_pointer_value();
        self.b.build_memcpy(p, 1, ptr, 1, len).map_err(llvm_err)?;
        let end = self.offset(p, len)?;
        self.b.build_store(end, self.i8().const_zero()).map_err(llvm_err)?;
        Ok(p)
    }

    /// `args(out: *List<String>)`: `argv`, program name first, as owned strings.
    fn args(
        &mut self,
        argc_global: PointerValue<'ctx>,
        argv_global: PointerValue<'ctx>,
        alloc_string: FunctionValue<'ctx>,
    ) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let strlen = libc(self.module, "strlen", usize_ty.fn_type(&[ptr_ty.into()], false));
        let f = self.function("burdin_rt_args", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let out = Self::ptr_param(f, 0);
        let argc = self.b.build_load(i32_ty, argc_global, "argc").map_err(llvm_err)?.into_int_value();
        let n = self.b.build_int_z_extend(argc, usize_ty, "").map_err(llvm_err)?;
        let argv = self.b.build_load(ptr_ty, argv_global, "argv").map_err(llvm_err)?.into_pointer_value();
        let word = u64::from(self.usize_ty.get_bit_width() / 8);
        let elem_size = 3 * word;
        let bytes = self.b.build_int_mul(n, usize_ty.const_int(elem_size, false), "").map_err(llvm_err)?;
        let base = self.call_rt_alloc(bytes)?;
        let header = self.block(f, "args.cond");
        let body = self.block(f, "args.body");
        let exit = self.block(f, "args.end");
        let index = self.b.build_alloca(usize_ty, "args.i").map_err(llvm_err)?;
        self.b.build_store(index, usize_ty.const_zero()).map_err(llvm_err)?;
        self.b.build_unconditional_branch(header).map_err(llvm_err)?;
        self.b.position_at_end(header);
        let i = self.b.build_load(usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let go = self.b.build_int_compare(IntPredicate::ULT, i, n, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(go, body, exit).map_err(llvm_err)?;
        self.b.position_at_end(body);
        let slot = unsafe { self.b.build_in_bounds_gep(ptr_ty, argv, &[i], "arg.ptr") }.map_err(llvm_err)?;
        let cstr = self.b.build_load(ptr_ty, slot, "").map_err(llvm_err)?.into_pointer_value();
        let len = self.call(strlen, &[cstr.into()])?.expect("strlen returns a size").into_int_value();
        let elem = unsafe { self.b.build_in_bounds_gep(self.string_ty, base, &[i], "arg") }.map_err(llvm_err)?;
        self.call(alloc_string, &[elem.into(), cstr.into(), len.into()])?;
        let next = self.b.build_int_add(i, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_store(index, next).map_err(llvm_err)?;
        self.b.build_unconditional_branch(header).map_err(llvm_err)?;
        self.b.position_at_end(exit);
        let f0 = self.b.build_struct_gep(self.string_ty, out, 0, "").map_err(llvm_err)?;
        let f1 = self.b.build_struct_gep(self.string_ty, out, 1, "").map_err(llvm_err)?;
        let f2 = self.b.build_struct_gep(self.string_ty, out, 2, "").map_err(llvm_err)?;
        self.b.build_store(f0, base).map_err(llvm_err)?;
        self.b.build_store(f1, n).map_err(llvm_err)?;
        self.b.build_store(f2, n).map_err(llvm_err)?;
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `clock(kind: i32) -> i64`: nanoseconds from a system clock. Kind 0 is the real-time clock, counted from the
    /// Unix epoch (`clock_gettime(CLOCK_REALTIME)`, `GetSystemTimePreciseAsFileTime` on Windows); any other kind is
    /// the monotonic clock, counted from an unspecified start (`clock_gettime(CLOCK_MONOTONIC)`,
    /// `QueryPerformanceCounter` on Windows). An `i64` of nanoseconds covers the years 1678 to 2262.
    fn clock(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (ptr_ty, i32_ty, i64_ty) = (self.ptr_ty, self.i32(), self.i64());
        let f = self.function("burdin_rt_clock", i64_ty.fn_type(&[i32_ty.into()], false));
        let kind = Self::int_param(f, 0);
        let buf = self.b.build_alloca(i64_ty.array_type(2), "clock.buf").map_err(llvm_err)?;
        let nanos_per_second = i64_ty.const_int(1_000_000_000, false);
        let is_real = self.b.build_int_compare(IntPredicate::EQ, kind, i32_ty.const_zero(), "").map_err(llvm_err)?;
        if self.windows {
            let real = self.block(f, "clock.real");
            let mono = self.block(f, "clock.mono");
            self.b.build_conditional_branch(is_real, real, mono).map_err(llvm_err)?;
            self.b.position_at_end(real);
            let void_ty = self.cx.void_type();
            let precise = libc(self.module, "GetSystemTimePreciseAsFileTime", void_ty.fn_type(&[ptr_ty.into()], false));
            self.call(precise, &[buf.into()])?;
            let ticks = self.b.build_load(i64_ty, buf, "").map_err(llvm_err)?.into_int_value();
            // 100 ns ticks since 1601-01-01; the Unix epoch is 116444736000000000 ticks later.
            let since_epoch =
                self.b.build_int_sub(ticks, i64_ty.const_int(116_444_736_000_000_000, false), "").map_err(llvm_err)?;
            let nanos = self.b.build_int_mul(since_epoch, i64_ty.const_int(100, false), "").map_err(llvm_err)?;
            self.b.build_return(Some(&nanos)).map_err(llvm_err)?;
            self.b.position_at_end(mono);
            let counter_fn = libc(self.module, "QueryPerformanceCounter", i32_ty.fn_type(&[ptr_ty.into()], false));
            let frequency_fn = libc(self.module, "QueryPerformanceFrequency", i32_ty.fn_type(&[ptr_ty.into()], false));
            let freq_buf = self.b.build_alloca(i64_ty, "clock.freq").map_err(llvm_err)?;
            self.call(counter_fn, &[buf.into()])?;
            self.call(frequency_fn, &[freq_buf.into()])?;
            let counter = self.b.build_load(i64_ty, buf, "").map_err(llvm_err)?.into_int_value();
            let frequency = self.b.build_load(i64_ty, freq_buf, "").map_err(llvm_err)?.into_int_value();
            // `counter * 1e9 / frequency` without the overflow of the product: whole seconds, then the remainder.
            let seconds = self.b.build_int_unsigned_div(counter, frequency, "").map_err(llvm_err)?;
            let rest = self.b.build_int_unsigned_rem(counter, frequency, "").map_err(llvm_err)?;
            let whole = self.b.build_int_mul(seconds, nanos_per_second, "").map_err(llvm_err)?;
            let scaled = self.b.build_int_mul(rest, nanos_per_second, "").map_err(llvm_err)?;
            let part = self.b.build_int_unsigned_div(scaled, frequency, "").map_err(llvm_err)?;
            let nanos = self.b.build_int_add(whole, part, "").map_err(llvm_err)?;
            self.b.build_return(Some(&nanos)).map_err(llvm_err)?;
        } else {
            #[cfg(unix)]
            let monotonic = libc::CLOCK_MONOTONIC as u64;
            #[cfg(not(unix))]
            let monotonic = 1;
            let id = self
                .b
                .build_select(is_real, i32_ty.const_zero(), i32_ty.const_int(monotonic, false), "")
                .map_err(llvm_err)?;
            let clock_gettime =
                libc(self.module, "clock_gettime", i32_ty.fn_type(&[i32_ty.into(), ptr_ty.into()], false));
            self.call(clock_gettime, &[id.into(), buf.into()])?;
            let seconds = self.b.build_load(i64_ty, buf, "").map_err(llvm_err)?.into_int_value();
            let at = self.offset(buf, self.usize_ty.const_int(8, false))?;
            let fraction = self.b.build_load(i64_ty, at, "").map_err(llvm_err)?.into_int_value();
            let whole = self.b.build_int_mul(seconds, nanos_per_second, "").map_err(llvm_err)?;
            let nanos = self.b.build_int_add(whole, fraction, "").map_err(llvm_err)?;
            self.b.build_return(Some(&nanos)).map_err(llvm_err)?;
        }
        Ok(f)
    }

    /// `entropy() -> i64`: 64 random bits from the operating system (`getentropy`, `rand_s` on Windows). If the
    /// system refuses, the bits come from the real-time clock instead, so a seed is never a constant.
    fn entropy(&mut self, clock: FunctionValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (ptr_ty, i32_ty, i64_ty) = (self.ptr_ty, self.i32(), self.i64());
        let f = self.function("burdin_rt_entropy", i64_ty.fn_type(&[], false));
        let buf = self.b.build_alloca(i64_ty, "entropy.buf").map_err(llvm_err)?;
        self.b.build_store(buf, i64_ty.const_zero()).map_err(llvm_err)?;
        let status = if self.windows {
            // `rand_s` writes 32 bits at a time and returns an errno value, zero for success.
            let rand_s = libc(self.module, "rand_s", i32_ty.fn_type(&[ptr_ty.into()], false));
            let high = self.call(rand_s, &[buf.into()])?.expect("rand_s returns a status").into_int_value();
            let at = self.offset(buf, self.usize_ty.const_int(4, false))?;
            let low = self.call(rand_s, &[at.into()])?.expect("rand_s returns a status").into_int_value();
            self.b.build_or(high, low, "").map_err(llvm_err)?
        } else {
            let getentropy =
                libc(self.module, "getentropy", i32_ty.fn_type(&[ptr_ty.into(), self.usize_ty.into()], false));
            self.call(getentropy, &[buf.into(), self.usize_ty.const_int(8, false).into()])?
                .expect("getentropy returns a status")
                .into_int_value()
        };
        let ok = self.b.build_int_compare(IntPredicate::EQ, status, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let good = self.block(f, "entropy.ok");
        let fallback = self.block(f, "entropy.fallback");
        self.b.build_conditional_branch(ok, good, fallback).map_err(llvm_err)?;
        self.b.position_at_end(good);
        let bits = self.b.build_load(i64_ty, buf, "").map_err(llvm_err)?;
        self.b.build_return(Some(&bits)).map_err(llvm_err)?;
        self.b.position_at_end(fallback);
        let now = self.call(clock, &[i32_ty.const_zero().into()])?.expect("clock returns nanoseconds");
        self.b.build_return(Some(&now)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `sleep(nanos: i64)`: suspends the calling thread for about `nanos` nanoseconds (`nanosleep`, `Sleep` in whole
    /// milliseconds rounded up on Windows). A signal can end the Unix sleep early.
    fn sleep(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (ptr_ty, i32_ty, i64_ty) = (self.ptr_ty, self.i32(), self.i64());
        let f = self.function("burdin_rt_sleep", self.cx.void_type().fn_type(&[i64_ty.into()], false));
        let nanos = Self::int_param(f, 0);
        if self.windows {
            let millis_unit = i64_ty.const_int(1_000_000, false);
            let rounded = self.b.build_int_add(nanos, i64_ty.const_int(999_999, false), "").map_err(llvm_err)?;
            let millis = self.b.build_int_unsigned_div(rounded, millis_unit, "").map_err(llvm_err)?;
            let millis = self.b.build_int_truncate(millis, i32_ty, "").map_err(llvm_err)?;
            let sleep = libc(self.module, "Sleep", self.cx.void_type().fn_type(&[i32_ty.into()], false));
            self.call(sleep, &[millis.into()])?;
        } else {
            let buf = self.b.build_alloca(i64_ty.array_type(2), "sleep.buf").map_err(llvm_err)?;
            let per_second = i64_ty.const_int(1_000_000_000, false);
            let seconds = self.b.build_int_unsigned_div(nanos, per_second, "").map_err(llvm_err)?;
            let fraction = self.b.build_int_unsigned_rem(nanos, per_second, "").map_err(llvm_err)?;
            self.b.build_store(buf, seconds).map_err(llvm_err)?;
            let at = self.offset(buf, self.usize_ty.const_int(8, false))?;
            self.b.build_store(at, fraction).map_err(llvm_err)?;
            let nanosleep = libc(self.module, "nanosleep", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
            self.call(nanosleep, &[buf.into(), ptr_ty.const_null().into()])?;
        }
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `exit(code: i32)`: ends the process.
    fn exit(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let exit = libc(self.module, "exit", self.cx.void_type().fn_type(&[self.i32().into()], false));
        self.mark_noreturn(exit);
        Ok(exit)
    }

    /// `run(command, command_len, err: *i32) -> i32`: runs the command through the system shell. The result is
    /// -1 when the shell could not start, with the OS error code in `err`, and the command's exit code otherwise.
    fn run_command(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let system = libc(self.module, "system", i32_ty.fn_type(&[ptr_ty.into()], false));
        let params = [ptr_ty.into(), usize_ty.into(), ptr_ty.into()];
        let f = self.function("burdin_rt_run", i32_ty.fn_type(&params, false));
        let err = Self::ptr_param(f, 2);
        self.b.build_store(err, i32_ty.const_zero()).map_err(llvm_err)?;
        let cpath = self.cstring_of(Self::ptr_param(f, 0), Self::int_param(f, 1), malloc)?;
        let status = self.call(system, &[cpath.into()])?.expect("system returns a status").into_int_value();
        let minus_one = i32_ty.const_all_ones();
        let failed = self.b.build_int_compare(IntPredicate::EQ, status, minus_one, "").map_err(llvm_err)?;
        let code = self.last_error(true)?;
        let error = self.b.build_select(failed, code, i32_ty.const_zero(), "run.error").map_err(llvm_err)?;
        self.b.build_store(err, error).map_err(llvm_err)?;
        self.call(free, &[cpath.into()])?;
        if self.windows {
            self.b.build_return(Some(&status)).map_err(llvm_err)?;
            return Ok(f);
        }
        // POSIX `system` returns a wait status: the exit code is its second byte. -1 stays -1.
        let shift = self.b.build_right_shift(status, i32_ty.const_int(8, false), false, "").map_err(llvm_err)?;
        let code = self.b.build_and(shift, i32_ty.const_int(0xff, false), "").map_err(llvm_err)?;
        let result = self.b.build_select(failed, minus_one, code, "").map_err(llvm_err)?;
        self.b.build_return(Some(&result)).map_err(llvm_err)?;
        Ok(f)
    }

    // ----- child processes and pipes ---------------------------------------------------

    /// `env_list(out: *List<String>)`: every environment entry as `NAME=VALUE`.
    fn env_list(&mut self, alloc_string: FunctionValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let void = self.cx.void_type();
        let f = self.function("burdin_rt_env_list", void.fn_type(&[ptr_ty.into()], false));
        let out = Self::ptr_param(f, 0);
        let word = u64::from(self.usize_ty.get_bit_width() / 8);
        let elem_size = 3 * word;
        if self.windows {
            let get = libc(self.module, "GetEnvironmentStringsW", ptr_ty.fn_type(&[], false));
            let release = libc(self.module, "FreeEnvironmentStringsW", i32_ty.fn_type(&[ptr_ty.into()], false));
            let wcslen = libc(self.module, "wcslen", usize_ty.fn_type(&[ptr_ty.into()], false));
            let free = libc(self.module, "free", void.fn_type(&[ptr_ty.into()], false));
            let block = self.call(get, &[])?.expect("GetEnvironmentStringsW returns a block").into_pointer_value();
            let cursor = self.b.build_alloca(ptr_ty, "env.cursor").map_err(llvm_err)?;
            self.b.build_store(cursor, block).map_err(llvm_err)?;
            let count = self.b.build_alloca(usize_ty, "env.count").map_err(llvm_err)?;
            self.b.build_store(count, usize_ty.const_zero()).map_err(llvm_err)?;
            let head = self.block(f, "env.count.head");
            let body = self.block(f, "env.count.body");
            let counted = self.block(f, "env.count.done");
            self.b.build_unconditional_branch(head).map_err(llvm_err)?;
            self.b.position_at_end(head);
            let c = self.b.build_load(ptr_ty, cursor, "").map_err(llvm_err)?.into_pointer_value();
            let first = self.b.build_load(self.cx.i16_type(), c, "first").map_err(llvm_err)?.into_int_value();
            let end = self
                .b
                .build_int_compare(IntPredicate::EQ, first, self.cx.i16_type().const_zero(), "")
                .map_err(llvm_err)?;
            self.b.build_conditional_branch(end, counted, body).map_err(llvm_err)?;
            self.b.position_at_end(body);
            let units = self.call(wcslen, &[c.into()])?.expect("wcslen returns a length").into_int_value();
            let next_units = self.b.build_int_add(units, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
            let step = self.b.build_int_mul(next_units, usize_ty.const_int(2, false), "").map_err(llvm_err)?;
            let next = self.offset(c, step)?;
            self.b.build_store(cursor, next).map_err(llvm_err)?;
            let n = self.b.build_load(usize_ty, count, "").map_err(llvm_err)?.into_int_value();
            let n2 = self.b.build_int_add(n, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
            self.b.build_store(count, n2).map_err(llvm_err)?;
            self.b.build_unconditional_branch(head).map_err(llvm_err)?;

            self.b.position_at_end(counted);
            let capacity = self.b.build_load(usize_ty, count, "").map_err(llvm_err)?.into_int_value();
            let elems = self.max_usize(capacity, usize_ty.const_int(1, false))?;
            let bytes = self.b.build_int_mul(elems, usize_ty.const_int(elem_size, false), "").map_err(llvm_err)?;
            let base = self.call_rt_alloc(bytes)?;
            self.b.build_store(cursor, block).map_err(llvm_err)?;
            let placed = self.b.build_alloca(usize_ty, "env.placed").map_err(llvm_err)?;
            self.b.build_store(placed, usize_ty.const_zero()).map_err(llvm_err)?;
            let fill = self.block(f, "env.fill.head");
            let fill_body = self.block(f, "env.fill.body");
            let copy = self.block(f, "env.fill.copy");
            let skip = self.block(f, "env.fill.skip");
            let filled = self.block(f, "env.fill.done");
            self.b.build_unconditional_branch(fill).map_err(llvm_err)?;
            self.b.position_at_end(fill);
            let c = self.b.build_load(ptr_ty, cursor, "").map_err(llvm_err)?.into_pointer_value();
            let first = self.b.build_load(self.cx.i16_type(), c, "first").map_err(llvm_err)?.into_int_value();
            let end = self
                .b
                .build_int_compare(IntPredicate::EQ, first, self.cx.i16_type().const_zero(), "")
                .map_err(llvm_err)?;
            self.b.build_conditional_branch(end, filled, fill_body).map_err(llvm_err)?;
            self.b.position_at_end(fill_body);
            let units = self.call(wcslen, &[c.into()])?.expect("wcslen returns a length").into_int_value();
            let byte_len = self.b.build_alloca(usize_ty, "env.bytes").map_err(llvm_err)?;
            let data = self.wide_to_utf8(c, units, byte_len)?;
            let null = self.b.build_is_null(data, "").map_err(llvm_err)?;
            self.b.build_conditional_branch(null, skip, copy).map_err(llvm_err)?;
            self.b.position_at_end(copy);
            let i = self.b.build_load(usize_ty, placed, "").map_err(llvm_err)?.into_int_value();
            let elem =
                unsafe { self.b.build_in_bounds_gep(self.string_ty, base, &[i], "env.elem") }.map_err(llvm_err)?;
            let len = self.b.build_load(usize_ty, byte_len, "").map_err(llvm_err)?.into_int_value();
            self.call(alloc_string, &[elem.into(), data.into(), len.into()])?;
            self.call(free, &[data.into()])?;
            let i2 = self.b.build_int_add(i, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
            self.b.build_store(placed, i2).map_err(llvm_err)?;
            self.b.build_unconditional_branch(skip).map_err(llvm_err)?;
            self.b.position_at_end(skip);
            let c = self.b.build_load(ptr_ty, cursor, "").map_err(llvm_err)?.into_pointer_value();
            let units = self.call(wcslen, &[c.into()])?.expect("wcslen returns a length").into_int_value();
            let next_units = self.b.build_int_add(units, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
            let step = self.b.build_int_mul(next_units, usize_ty.const_int(2, false), "").map_err(llvm_err)?;
            let next = self.offset(c, step)?;
            self.b.build_store(cursor, next).map_err(llvm_err)?;
            self.b.build_unconditional_branch(fill).map_err(llvm_err)?;
            self.b.position_at_end(filled);
            self.call(release, &[block.into()])?;
            let len = self.b.build_load(usize_ty, placed, "").map_err(llvm_err)?.into_int_value();
            self.store_string_list(out, base, len, elems)?;
            self.b.build_return(None).map_err(llvm_err)?;
            return Ok(f);
        }

        let environ = self.environ_global();
        let entries = self.b.build_load(ptr_ty, environ, "environ").map_err(llvm_err)?.into_pointer_value();
        let strlen = libc(self.module, "strlen", usize_ty.fn_type(&[ptr_ty.into()], false));
        let index = self.b.build_alloca(usize_ty, "env.i").map_err(llvm_err)?;
        self.b.build_store(index, usize_ty.const_zero()).map_err(llvm_err)?;
        let count = self.b.build_alloca(usize_ty, "env.count").map_err(llvm_err)?;
        self.b.build_store(count, usize_ty.const_zero()).map_err(llvm_err)?;
        let head = self.block(f, "env.count.head");
        let body = self.block(f, "env.count.body");
        let counted = self.block(f, "env.count.done");
        self.b.build_unconditional_branch(head).map_err(llvm_err)?;
        self.b.position_at_end(head);
        let i = self.b.build_load(usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let slot = unsafe { self.b.build_in_bounds_gep(ptr_ty, entries, &[i], "env.slot") }.map_err(llvm_err)?;
        let entry = self.b.build_load(ptr_ty, slot, "").map_err(llvm_err)?.into_pointer_value();
        let end = self.b.build_is_null(entry, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(end, counted, body).map_err(llvm_err)?;
        self.b.position_at_end(body);
        let i2 = self.b.build_int_add(i, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_store(index, i2).map_err(llvm_err)?;
        let n = self.b.build_load(usize_ty, count, "").map_err(llvm_err)?.into_int_value();
        let n2 = self.b.build_int_add(n, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_store(count, n2).map_err(llvm_err)?;
        self.b.build_unconditional_branch(head).map_err(llvm_err)?;

        self.b.position_at_end(counted);
        let capacity = self.b.build_load(usize_ty, count, "").map_err(llvm_err)?.into_int_value();
        let elems = self.max_usize(capacity, usize_ty.const_int(1, false))?;
        let bytes = self.b.build_int_mul(elems, usize_ty.const_int(elem_size, false), "").map_err(llvm_err)?;
        let base = self.call_rt_alloc(bytes)?;
        self.b.build_store(index, usize_ty.const_zero()).map_err(llvm_err)?;
        let fill = self.block(f, "env.fill.head");
        let fill_body = self.block(f, "env.fill.body");
        let filled = self.block(f, "env.fill.done");
        self.b.build_unconditional_branch(fill).map_err(llvm_err)?;
        self.b.position_at_end(fill);
        let i = self.b.build_load(usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let in_range = self.b.build_int_compare(IntPredicate::ULT, i, capacity, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(in_range, fill_body, filled).map_err(llvm_err)?;
        self.b.position_at_end(fill_body);
        let slot = unsafe { self.b.build_in_bounds_gep(ptr_ty, entries, &[i], "env.slot") }.map_err(llvm_err)?;
        let entry = self.b.build_load(ptr_ty, slot, "").map_err(llvm_err)?.into_pointer_value();
        let len = self.call(strlen, &[entry.into()])?.expect("strlen returns a length").into_int_value();
        let elem = unsafe { self.b.build_in_bounds_gep(self.string_ty, base, &[i], "env.elem") }.map_err(llvm_err)?;
        self.call(alloc_string, &[elem.into(), entry.into(), len.into()])?;
        let i2 = self.b.build_int_add(i, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_store(index, i2).map_err(llvm_err)?;
        self.b.build_unconditional_branch(fill).map_err(llvm_err)?;
        self.b.position_at_end(filled);
        self.store_string_list(out, base, capacity, elems)?;
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// The `environ` global: the process environment as a null-terminated array of C strings.
    fn environ_global(&mut self) -> PointerValue<'ctx> {
        self.module
            .get_global("environ")
            .unwrap_or_else(|| {
                let g = self.module.add_global(self.ptr_ty, None, "environ");
                g.set_linkage(Linkage::External);
                g
            })
            .as_pointer_value()
    }

    /// Stores a `{ ptr, len, cap }` list header at `out`.
    fn store_string_list(
        &mut self,
        out: PointerValue<'ctx>,
        ptr: PointerValue<'ctx>,
        len: IntValue<'ctx>,
        cap: IntValue<'ctx>,
    ) -> GenResult<()> {
        let f0 = self.b.build_struct_gep(self.string_ty, out, 0, "").map_err(llvm_err)?;
        let f1 = self.b.build_struct_gep(self.string_ty, out, 1, "").map_err(llvm_err)?;
        let f2 = self.b.build_struct_gep(self.string_ty, out, 2, "").map_err(llvm_err)?;
        self.b.build_store(f0, ptr).map_err(llvm_err)?;
        self.b.build_store(f1, len).map_err(llvm_err)?;
        self.b.build_store(f2, cap).map_err(llvm_err)?;
        Ok(())
    }

    fn max_usize(&self, a: IntValue<'ctx>, b: IntValue<'ctx>) -> GenResult<IntValue<'ctx>> {
        let bigger = self.b.build_int_compare(IntPredicate::UGT, a, b, "").map_err(llvm_err)?;
        self.b.build_select(bigger, a, b, "").map_err(llvm_err).map(|v| v.into_int_value())
    }

    /// Loads a condition from an `i8` slot. The runtime's own flags are bytes, not the compiler's `i1` drop
    /// flags.
    fn load_flag(&self, slot: PointerValue<'ctx>) -> GenResult<IntValue<'ctx>> {
        let byte = self.b.build_load(self.i8(), slot, "").map_err(llvm_err)?.into_int_value();
        self.b.build_int_compare(IntPredicate::NE, byte, self.i8().const_zero(), "").map_err(llvm_err)
    }

    /// A Win32 `BOOL` (an `i32` that is zero or not) as a branch condition.
    fn truthy(&self, value: IntValue<'ctx>) -> GenResult<IntValue<'ctx>> {
        let zero = value.get_type().const_zero();
        self.b.build_int_compare(IntPredicate::NE, value, zero, "").map_err(llvm_err)
    }

    /// Whether any argument or environment entry in `list` contains a NUL byte.
    fn any_nul(
        &mut self,
        list: PointerValue<'ctx>,
        count: IntValue<'ctx>,
        initial: IntValue<'ctx>,
    ) -> GenResult<IntValue<'ctx>> {
        let usize_ty = self.usize_ty;
        let f = self.b.get_insert_block().and_then(|b| b.get_parent()).expect("positioned in a function");
        let index = self.b.build_alloca(usize_ty, "nul.i").map_err(llvm_err)?;
        self.b.build_store(index, usize_ty.const_zero()).map_err(llvm_err)?;
        let acc = self.b.build_alloca(self.i8(), "nul.acc").map_err(llvm_err)?;
        let initial = self.b.build_int_z_extend(initial, self.i8(), "").map_err(llvm_err)?;
        self.b.build_store(acc, initial).map_err(llvm_err)?;
        let head = self.block(f, "nul.head");
        let body = self.block(f, "nul.body");
        let done = self.block(f, "nul.done");
        self.b.build_unconditional_branch(head).map_err(llvm_err)?;
        self.b.position_at_end(head);
        let i = self.b.build_load(usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let in_range = self.b.build_int_compare(IntPredicate::ULT, i, count, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(in_range, body, done).map_err(llvm_err)?;
        self.b.position_at_end(body);
        let elem = unsafe { self.b.build_in_bounds_gep(self.string_ty, list, &[i], "nul.elem") }.map_err(llvm_err)?;
        let ptr_field = self.b.build_struct_gep(self.string_ty, elem, 0, "").map_err(llvm_err)?;
        let len_field = self.b.build_struct_gep(self.string_ty, elem, 1, "").map_err(llvm_err)?;
        let ptr = self.b.build_load(self.ptr_ty, ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        let len = self.b.build_load(usize_ty, len_field, "").map_err(llvm_err)?.into_int_value();
        let found = self.has_byte(ptr, len, 0)?;
        let prev = self.b.build_load(self.i8(), acc, "").map_err(llvm_err)?.into_int_value();
        let found = self.b.build_int_z_extend(found, self.i8(), "").map_err(llvm_err)?;
        let next = self.b.build_or(prev, found, "").map_err(llvm_err)?;
        self.b.build_store(acc, next).map_err(llvm_err)?;
        let i2 = self.b.build_int_add(i, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_store(index, i2).map_err(llvm_err)?;
        self.b.build_unconditional_branch(head).map_err(llvm_err)?;
        self.b.position_at_end(done);
        let result = self.b.build_load(self.i8(), acc, "nul.result").map_err(llvm_err)?.into_int_value();
        self.b.build_int_compare(IntPredicate::NE, result, self.i8().const_zero(), "").map_err(llvm_err)
    }

    /// Frees a `malloc`ed array of C strings.
    fn free_cstring_array(
        &mut self,
        f: FunctionValue<'ctx>,
        array: PointerValue<'ctx>,
        count: IntValue<'ctx>,
        free: FunctionValue<'ctx>,
    ) -> GenResult<()> {
        let (usize_ty, ptr_ty) = (self.usize_ty, self.ptr_ty);
        let index = self.b.build_alloca(usize_ty, "free.i").map_err(llvm_err)?;
        self.b.build_store(index, usize_ty.const_zero()).map_err(llvm_err)?;
        let head = self.block(f, "free.head");
        let body = self.block(f, "free.body");
        let done = self.block(f, "free.done");
        self.b.build_unconditional_branch(head).map_err(llvm_err)?;
        self.b.position_at_end(head);
        let i = self.b.build_load(usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let in_range = self.b.build_int_compare(IntPredicate::ULT, i, count, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(in_range, body, done).map_err(llvm_err)?;
        self.b.position_at_end(body);
        let slot = unsafe { self.b.build_in_bounds_gep(ptr_ty, array, &[i], "free.slot") }.map_err(llvm_err)?;
        let entry = self.b.build_load(ptr_ty, slot, "").map_err(llvm_err)?.into_pointer_value();
        self.call(free, &[entry.into()])?;
        let i2 = self.b.build_int_add(i, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_store(index, i2).map_err(llvm_err)?;
        self.b.build_unconditional_branch(head).map_err(llvm_err)?;
        self.b.position_at_end(done);
        Ok(())
    }

    /// Frees `array` and its entries when the array was allocated (`slot` holds null otherwise).
    fn free_cstring_array_if_present(
        &mut self,
        f: FunctionValue<'ctx>,
        slot: PointerValue<'ctx>,
        count: IntValue<'ctx>,
        free: FunctionValue<'ctx>,
    ) -> GenResult<()> {
        let array = self.b.build_load(self.ptr_ty, slot, "").map_err(llvm_err)?.into_pointer_value();
        let null = self.b.build_is_null(array, "").map_err(llvm_err)?;
        let go = self.block(f, "free.present");
        let done = self.block(f, "free.present.done");
        self.b.build_conditional_branch(null, done, go).map_err(llvm_err)?;
        self.b.position_at_end(go);
        self.free_cstring_array(f, array, count, free)?;
        self.call(free, &[array.into()])?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(done);
        Ok(())
    }

    /// `process_spawn`: starts a child without a shell.
    fn process_spawn(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let f = self.function(
            "burdin_rt_process_spawn",
            i32_ty.fn_type(
                &[
                    ptr_ty.into(),
                    usize_ty.into(),
                    ptr_ty.into(),
                    usize_ty.into(),
                    ptr_ty.into(),
                    usize_ty.into(),
                    ptr_ty.into(),
                    usize_ty.into(),
                    ptr_ty.into(),
                    usize_ty.into(),
                    i32_ty.into(),
                    i32_ty.into(),
                    i32_ty.into(),
                    ptr_ty.into(),
                ],
                false,
            ),
        );
        let program = Self::ptr_param(f, 0);
        let program_len = Self::int_param(f, 1);
        let args = Self::ptr_param(f, 2);
        let args_len = Self::int_param(f, 3);
        let env = Self::ptr_param(f, 4);
        let env_len = Self::int_param(f, 5);
        let cwd = Self::ptr_param(f, 6);
        let cwd_len = Self::int_param(f, 7);
        let cmdline = Self::ptr_param(f, 8);
        let cmdline_len = Self::int_param(f, 9);
        let stdin_mode = Self::int_param(f, 10);
        let stdout_mode = Self::int_param(f, 11);
        let stderr_mode = Self::int_param(f, 12);
        let out = Self::ptr_param(f, 13);
        // Output already written must reach the stream before the child writes to it.
        if let Some(flush) = self.module.get_function("burdin_rt_flush") {
            self.call(flush, &[])?;
        }
        let mut bad = self.has_byte(program, program_len, 0)?;
        let other = self.has_byte(cwd, cwd_len, 0)?;
        bad = self.b.build_or(bad, other, "").map_err(llvm_err)?;
        let other = self.has_byte(cmdline, cmdline_len, 0)?;
        bad = self.b.build_or(bad, other, "").map_err(llvm_err)?;
        bad = self.any_nul(args, args_len, bad)?;
        bad = self.any_nul(env, env_len, bad)?;
        let reject = self.block(f, "spawn.reject");
        let start = self.block(f, "spawn.start");
        self.b.build_conditional_branch(bad, reject, start).map_err(llvm_err)?;
        self.b.position_at_end(reject);
        self.b.build_return(Some(&i32_ty.const_all_ones())).map_err(llvm_err)?;
        self.b.position_at_end(start);
        if self.windows {
            self.process_spawn_windows(
                f,
                program,
                program_len,
                args,
                args_len,
                env,
                env_len,
                cwd,
                cwd_len,
                cmdline,
                cmdline_len,
                stdin_mode,
                stdout_mode,
                stderr_mode,
                out,
            )?;
        } else {
            self.process_spawn_unix(
                f,
                program,
                program_len,
                args,
                args_len,
                env,
                env_len,
                cwd,
                cwd_len,
                stdin_mode,
                stdout_mode,
                stderr_mode,
                out,
            )?;
        }
        Ok(f)
    }

    /// `process_spawn` on Unix: pipes, `fork`, and `execvp`, with the child's environment replacing `environ`.
    #[allow(clippy::too_many_arguments)]
    fn process_spawn_unix(
        &mut self,
        f: FunctionValue<'ctx>,
        program: PointerValue<'ctx>,
        program_len: IntValue<'ctx>,
        args: PointerValue<'ctx>,
        args_len: IntValue<'ctx>,
        env: PointerValue<'ctx>,
        env_len: IntValue<'ctx>,
        cwd: PointerValue<'ctx>,
        cwd_len: IntValue<'ctx>,
        stdin_mode: IntValue<'ctx>,
        stdout_mode: IntValue<'ctx>,
        stderr_mode: IntValue<'ctx>,
        out: PointerValue<'ctx>,
    ) -> GenResult<()> {
        let (usize_ty, ptr_ty, i32_ty, i64_ty, i8_ty) = (self.usize_ty, self.ptr_ty, self.i32(), self.i64(), self.i8());
        let void = self.cx.void_type();
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", void.fn_type(&[ptr_ty.into()], false));
        let pipe = libc(self.module, "pipe", i32_ty.fn_type(&[ptr_ty.into()], false));
        let fork = libc(self.module, "fork", i32_ty.fn_type(&[], false));
        let dup2 = libc(self.module, "dup2", i32_ty.fn_type(&[i32_ty.into(), i32_ty.into()], false));
        let close = libc(self.module, "close", i32_ty.fn_type(&[i32_ty.into()], false));
        let open = libc(self.module, "open", i32_ty.fn_type(&[ptr_ty.into(), i32_ty.into(), i32_ty.into()], false));
        let fcntl = libc(self.module, "fcntl", i32_ty.fn_type(&[i32_ty.into(), i32_ty.into(), i32_ty.into()], false));
        let chdir = libc(self.module, "chdir", i32_ty.fn_type(&[ptr_ty.into()], false));
        let execvp = libc(self.module, "execvp", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
        let quit = libc(self.module, "_exit", void.fn_type(&[i32_ty.into()], false));
        self.mark_noreturn(quit);
        let read = libc(self.module, "read", usize_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), usize_ty.into()], false));
        let waitpid =
            libc(self.module, "waitpid", i32_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), i32_ty.into()], false));

        let fail = self.block(f, "spawn.fail");
        let argv_slot = self.b.build_alloca(ptr_ty, "argv.slot").map_err(llvm_err)?;
        self.b.build_store(argv_slot, ptr_ty.const_null()).map_err(llvm_err)?;
        let envp_slot = self.b.build_alloca(ptr_ty, "envp.slot").map_err(llvm_err)?;
        self.b.build_store(envp_slot, ptr_ty.const_null()).map_err(llvm_err)?;
        let cwd_slot = self.b.build_alloca(ptr_ty, "cwd.slot").map_err(llvm_err)?;
        self.b.build_store(cwd_slot, ptr_ty.const_null()).map_err(llvm_err)?;
        let fds = self.b.build_alloca(i32_ty.array_type(2), "pipe.fds").map_err(llvm_err)?;
        let err_pipe = self.b.build_alloca(i32_ty.array_type(2), "err.fds").map_err(llvm_err)?;
        let minus_one = i32_ty.const_all_ones();
        let slot_i32 = |cg: &mut Self, n: &str| -> GenResult<PointerValue<'ctx>> {
            let p = cg.b.build_alloca(i32_ty, n).map_err(llvm_err)?;
            cg.b.build_store(p, minus_one).map_err(llvm_err)?;
            Ok(p)
        };
        let in_read = slot_i32(self, "in.read")?;
        let in_write = slot_i32(self, "in.write")?;
        let out_read = slot_i32(self, "out.read")?;
        let out_write = slot_i32(self, "out.write")?;
        let err_read = slot_i32(self, "err.read")?;
        let err_write = slot_i32(self, "err.write")?;
        let devnull = slot_i32(self, "devnull")?;
        let report_read = slot_i32(self, "report.read")?;
        let report_write = slot_i32(self, "report.write")?;
        let load_i32 = |cg: &mut Self, slot: PointerValue<'ctx>| -> GenResult<IntValue<'ctx>> {
            Ok(cg.b.build_load(i32_ty, slot, "").map_err(llvm_err)?.into_int_value())
        };
        let store_i32 = |cg: &mut Self, slot: PointerValue<'ctx>, value: IntValue<'ctx>| -> GenResult<()> {
            cg.b.build_store(slot, value).map_err(llvm_err)?;
            Ok(())
        };

        // argv: the program, the arguments, and a null terminator.
        let argc = self.b.build_int_add(args_len, usize_ty.const_int(1, false), "argc").map_err(llvm_err)?;
        let argv_words = self.b.build_int_add(argc, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        let argv_bytes = self.b.build_int_mul(argv_words, usize_ty.const_int(8, false), "").map_err(llvm_err)?;
        let argv = self.call(malloc, &[argv_bytes.into()])?.expect("malloc returns a pointer").into_pointer_value();
        self.b.build_store(argv_slot, argv).map_err(llvm_err)?;
        let program_c = self.cstring_of(program, program_len, malloc)?;
        let first_slot =
            unsafe { self.b.build_in_bounds_gep(ptr_ty, argv, &[usize_ty.const_zero()], "") }.map_err(llvm_err)?;
        self.b.build_store(first_slot, program_c).map_err(llvm_err)?;
        let index = self.b.build_alloca(usize_ty, "argv.i").map_err(llvm_err)?;
        self.b.build_store(index, usize_ty.const_zero()).map_err(llvm_err)?;
        let args_head = self.block(f, "argv.head");
        let args_body = self.block(f, "argv.body");
        let args_done = self.block(f, "argv.done");
        self.b.build_unconditional_branch(args_head).map_err(llvm_err)?;
        self.b.position_at_end(args_head);
        let i = self.b.build_load(usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let in_range = self.b.build_int_compare(IntPredicate::ULT, i, args_len, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(in_range, args_body, args_done).map_err(llvm_err)?;
        self.b.position_at_end(args_body);
        let elem = unsafe { self.b.build_in_bounds_gep(self.string_ty, args, &[i], "arg.elem") }.map_err(llvm_err)?;
        let arg_ptr_field = self.b.build_struct_gep(self.string_ty, elem, 0, "").map_err(llvm_err)?;
        let arg_len_field = self.b.build_struct_gep(self.string_ty, elem, 1, "").map_err(llvm_err)?;
        let arg_ptr = self.b.build_load(ptr_ty, arg_ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        let arg_len = self.b.build_load(usize_ty, arg_len_field, "").map_err(llvm_err)?.into_int_value();
        let arg_c = self.cstring_of(arg_ptr, arg_len, malloc)?;
        let next_index = self.b.build_int_add(i, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        let slot = unsafe { self.b.build_in_bounds_gep(ptr_ty, argv, &[next_index], "") }.map_err(llvm_err)?;
        self.b.build_store(slot, arg_c).map_err(llvm_err)?;
        self.b.build_store(index, next_index).map_err(llvm_err)?;
        self.b.build_unconditional_branch(args_head).map_err(llvm_err)?;
        self.b.position_at_end(args_done);
        let end_slot = unsafe { self.b.build_in_bounds_gep(ptr_ty, argv, &[argc], "") }.map_err(llvm_err)?;
        self.b.build_store(end_slot, ptr_ty.const_null()).map_err(llvm_err)?;

        // envp: the final environment and a null terminator.
        let envp_words = self.b.build_int_add(env_len, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        let envp_bytes = self.b.build_int_mul(envp_words, usize_ty.const_int(8, false), "").map_err(llvm_err)?;
        let envp = self.call(malloc, &[envp_bytes.into()])?.expect("malloc returns a pointer").into_pointer_value();
        self.b.build_store(envp_slot, envp).map_err(llvm_err)?;
        let env_index = self.b.build_alloca(usize_ty, "envp.i").map_err(llvm_err)?;
        self.b.build_store(env_index, usize_ty.const_zero()).map_err(llvm_err)?;
        let env_head = self.block(f, "envp.head");
        let env_body = self.block(f, "envp.body");
        let env_done = self.block(f, "envp.done");
        self.b.build_unconditional_branch(env_head).map_err(llvm_err)?;
        self.b.position_at_end(env_head);
        let i = self.b.build_load(usize_ty, env_index, "").map_err(llvm_err)?.into_int_value();
        let in_range = self.b.build_int_compare(IntPredicate::ULT, i, env_len, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(in_range, env_body, env_done).map_err(llvm_err)?;
        self.b.position_at_end(env_body);
        let elem = unsafe { self.b.build_in_bounds_gep(self.string_ty, env, &[i], "env.elem") }.map_err(llvm_err)?;
        let env_ptr_field = self.b.build_struct_gep(self.string_ty, elem, 0, "").map_err(llvm_err)?;
        let env_len_field = self.b.build_struct_gep(self.string_ty, elem, 1, "").map_err(llvm_err)?;
        let env_ptr = self.b.build_load(ptr_ty, env_ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        let entry_len = self.b.build_load(usize_ty, env_len_field, "").map_err(llvm_err)?.into_int_value();
        let entry_c = self.cstring_of(env_ptr, entry_len, malloc)?;
        let slot = unsafe { self.b.build_in_bounds_gep(ptr_ty, envp, &[i], "") }.map_err(llvm_err)?;
        self.b.build_store(slot, entry_c).map_err(llvm_err)?;
        let next_index = self.b.build_int_add(i, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_store(env_index, next_index).map_err(llvm_err)?;
        self.b.build_unconditional_branch(env_head).map_err(llvm_err)?;
        self.b.position_at_end(env_done);
        let end_slot = unsafe { self.b.build_in_bounds_gep(ptr_ty, envp, &[env_len], "") }.map_err(llvm_err)?;
        self.b.build_store(end_slot, ptr_ty.const_null()).map_err(llvm_err)?;
        let cwd_c = self.cstring_of(cwd, cwd_len, malloc)?;
        self.b.build_store(cwd_slot, cwd_c).map_err(llvm_err)?;

        // The error pipe reports a failed `exec`; it closes itself on a successful one.
        let r = self.call(pipe, &[err_pipe.into()])?.expect("pipe returns a status").into_int_value();
        let failed_pipe = self.b.build_int_compare(IntPredicate::SLT, r, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let have_report = self.block(f, "report.have");
        self.b.build_conditional_branch(failed_pipe, fail, have_report).map_err(llvm_err)?;
        self.b.position_at_end(have_report);
        let fds_ptr = |cg: &mut Self, at: u64| -> GenResult<PointerValue<'ctx>> {
            unsafe { cg.b.build_in_bounds_gep(i32_ty, err_pipe, &[usize_ty.const_int(at, false)], "") }
                .map_err(llvm_err)
        };
        let p0 = fds_ptr(self, 0)?;
        let p1 = fds_ptr(self, 1)?;
        let rfd = self.b.build_load(i32_ty, p0, "").map_err(llvm_err)?.into_int_value();
        let wfd = self.b.build_load(i32_ty, p1, "").map_err(llvm_err)?.into_int_value();
        store_i32(self, report_read, rfd)?;
        store_i32(self, report_write, wfd)?;
        self.call(fcntl, &[wfd.into(), i32_ty.const_int(2, false).into(), i32_ty.const_int(1, false).into()])?;

        // The standard streams.
        let pipe_slot = |cg: &mut Self, at: u64| -> GenResult<PointerValue<'ctx>> {
            unsafe { cg.b.build_in_bounds_gep(i32_ty, fds, &[usize_ty.const_int(at, false)], "") }.map_err(llvm_err)
        };
        // stdin
        let in_pipe = self.block(f, "stdin.pipe");
        let after_in = self.block(f, "stdin.after");
        let want =
            self.b.build_int_compare(IntPredicate::EQ, stdin_mode, i32_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(want, in_pipe, after_in).map_err(llvm_err)?;
        self.b.position_at_end(in_pipe);
        let r = self.call(pipe, &[fds.into()])?.expect("pipe returns a status").into_int_value();
        let bad_pipe = self.b.build_int_compare(IntPredicate::SLT, r, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let in_ok = self.block(f, "stdin.ok");
        self.b.build_conditional_branch(bad_pipe, fail, in_ok).map_err(llvm_err)?;
        self.b.position_at_end(in_ok);
        let p0 = pipe_slot(self, 0)?;
        let p1 = pipe_slot(self, 1)?;
        store_i32(self, in_read, self.b.build_load(i32_ty, p0, "").map_err(llvm_err)?.into_int_value())?;
        store_i32(self, in_write, self.b.build_load(i32_ty, p1, "").map_err(llvm_err)?.into_int_value())?;
        self.b.build_unconditional_branch(after_in).map_err(llvm_err)?;
        self.b.position_at_end(after_in);
        // stdout
        let out_pipe = self.block(f, "stdout.pipe");
        let after_out = self.block(f, "stdout.after");
        let want = self
            .b
            .build_int_compare(IntPredicate::EQ, stdout_mode, i32_ty.const_int(1, false), "")
            .map_err(llvm_err)?;
        self.b.build_conditional_branch(want, out_pipe, after_out).map_err(llvm_err)?;
        self.b.position_at_end(out_pipe);
        let r = self.call(pipe, &[fds.into()])?.expect("pipe returns a status").into_int_value();
        let bad_pipe = self.b.build_int_compare(IntPredicate::SLT, r, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let out_ok = self.block(f, "stdout.ok");
        self.b.build_conditional_branch(bad_pipe, fail, out_ok).map_err(llvm_err)?;
        self.b.position_at_end(out_ok);
        let p0 = pipe_slot(self, 0)?;
        let p1 = pipe_slot(self, 1)?;
        store_i32(self, out_read, self.b.build_load(i32_ty, p0, "").map_err(llvm_err)?.into_int_value())?;
        store_i32(self, out_write, self.b.build_load(i32_ty, p1, "").map_err(llvm_err)?.into_int_value())?;
        self.b.build_unconditional_branch(after_out).map_err(llvm_err)?;
        self.b.position_at_end(after_out);
        // stderr
        let err_pipe_bb = self.block(f, "stderr.pipe");
        let after_err = self.block(f, "stderr.after");
        let want = self
            .b
            .build_int_compare(IntPredicate::EQ, stderr_mode, i32_ty.const_int(1, false), "")
            .map_err(llvm_err)?;
        self.b.build_conditional_branch(want, err_pipe_bb, after_err).map_err(llvm_err)?;
        self.b.position_at_end(err_pipe_bb);
        let r = self.call(pipe, &[fds.into()])?.expect("pipe returns a status").into_int_value();
        let bad_pipe = self.b.build_int_compare(IntPredicate::SLT, r, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let err_ok = self.block(f, "stderr.ok");
        self.b.build_conditional_branch(bad_pipe, fail, err_ok).map_err(llvm_err)?;
        self.b.position_at_end(err_ok);
        let p0 = pipe_slot(self, 0)?;
        let p1 = pipe_slot(self, 1)?;
        store_i32(self, err_read, self.b.build_load(i32_ty, p0, "").map_err(llvm_err)?.into_int_value())?;
        store_i32(self, err_write, self.b.build_load(i32_ty, p1, "").map_err(llvm_err)?.into_int_value())?;
        self.b.build_unconditional_branch(after_err).map_err(llvm_err)?;
        self.b.position_at_end(after_err);
        // /dev/null for a null stream.
        let null_bb = self.block(f, "null.open");
        let after_null = self.block(f, "null.after");
        let wants_null = self
            .b
            .build_or(
                self.b
                    .build_or(
                        self.b
                            .build_int_compare(IntPredicate::EQ, stdin_mode, i32_ty.const_int(2, false), "")
                            .map_err(llvm_err)?,
                        self.b
                            .build_int_compare(IntPredicate::EQ, stdout_mode, i32_ty.const_int(2, false), "")
                            .map_err(llvm_err)?,
                        "",
                    )
                    .map_err(llvm_err)?,
                self.b
                    .build_int_compare(IntPredicate::EQ, stderr_mode, i32_ty.const_int(2, false), "")
                    .map_err(llvm_err)?,
                "",
            )
            .map_err(llvm_err)?;
        self.b.build_conditional_branch(wants_null, null_bb, after_null).map_err(llvm_err)?;
        self.b.position_at_end(null_bb);
        let devnull_path = self.bytes(b"/dev/null\0");
        let r = self
            .call(open, &[devnull_path.into(), i32_ty.const_int(2, false).into(), i32_ty.const_zero().into()])?
            .expect("open returns a descriptor")
            .into_int_value();
        let bad_open = self.b.build_int_compare(IntPredicate::SLT, r, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let opened = self.block(f, "null.opened");
        self.b.build_conditional_branch(bad_open, fail, opened).map_err(llvm_err)?;
        self.b.position_at_end(opened);
        store_i32(self, devnull, r)?;
        self.b.build_unconditional_branch(after_null).map_err(llvm_err)?;
        self.b.position_at_end(after_null);

        // Fork, and let the child become the program.
        let child = self.block(f, "fork.child");
        let parent = self.block(f, "fork.parent");
        let pid = self.call(fork, &[])?.expect("fork returns a pid").into_int_value();
        let negative = self.b.build_int_compare(IntPredicate::SLT, pid, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let zero = self.b.build_int_compare(IntPredicate::EQ, pid, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let forked = self.block(f, "fork.ok");
        self.b.build_conditional_branch(negative, fail, forked).map_err(llvm_err)?;
        self.b.position_at_end(forked);
        self.b.build_conditional_branch(zero, child, parent).map_err(llvm_err)?;

        // Child.
        self.b.position_at_end(child);
        let rfd = load_i32(self, report_read)?;
        self.call(close, &[rfd.into()])?;
        let dup_stream = |cg: &mut Self,
                          mode: IntValue<'ctx>,
                          child_end: PointerValue<'ctx>,
                          target: u64,
                          devnull: PointerValue<'ctx>|
         -> GenResult<()> {
            let pipe_bb = cg.block(f, "child.pipe");
            let null_bb = cg.block(f, "child.null");
            let done = cg.block(f, "child.done");
            let is_pipe =
                cg.b.build_int_compare(IntPredicate::EQ, mode, i32_ty.const_int(1, false), "").map_err(llvm_err)?;
            cg.b.build_conditional_branch(is_pipe, pipe_bb, null_bb).map_err(llvm_err)?;
            cg.b.position_at_end(pipe_bb);
            let fd = load_i32(cg, child_end)?;
            cg.call(dup2, &[fd.into(), i32_ty.const_int(target, false).into()])?;
            cg.b.build_unconditional_branch(done).map_err(llvm_err)?;
            cg.b.position_at_end(null_bb);
            let is_null =
                cg.b.build_int_compare(IntPredicate::EQ, mode, i32_ty.const_int(2, false), "").map_err(llvm_err)?;
            let open_null = cg.block(f, "child.open.null");
            cg.b.build_conditional_branch(is_null, open_null, done).map_err(llvm_err)?;
            cg.b.position_at_end(open_null);
            let fd = load_i32(cg, devnull)?;
            cg.call(dup2, &[fd.into(), i32_ty.const_int(target, false).into()])?;
            cg.b.build_unconditional_branch(done).map_err(llvm_err)?;
            cg.b.position_at_end(done);
            Ok(())
        };
        dup_stream(self, stdin_mode, in_read, 0, devnull)?;
        dup_stream(self, stdout_mode, out_write, 1, devnull)?;
        dup_stream(self, stderr_mode, err_write, 2, devnull)?;
        let child_fd = |cg: &mut Self, slot: PointerValue<'ctx>| -> GenResult<()> {
            let fd = load_i32(cg, slot)?;
            cg.call(close, &[fd.into()])?;
            Ok(())
        };
        child_fd(self, in_read)?;
        child_fd(self, in_write)?;
        child_fd(self, out_read)?;
        child_fd(self, out_write)?;
        child_fd(self, err_read)?;
        child_fd(self, err_write)?;
        child_fd(self, devnull)?;
        let has_cwd =
            self.b.build_int_compare(IntPredicate::NE, cwd_len, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let chdir_bb = self.block(f, "child.chdir");
        let after_chdir = self.block(f, "child.chdir.after");
        self.b.build_conditional_branch(has_cwd, chdir_bb, after_chdir).map_err(llvm_err)?;
        self.b.position_at_end(chdir_bb);
        let cwd_c = self.b.build_load(ptr_ty, cwd_slot, "").map_err(llvm_err)?.into_pointer_value();
        let r = self.call(chdir, &[cwd_c.into()])?.expect("chdir returns a status").into_int_value();
        let bad_chdir = self.b.build_int_compare(IntPredicate::NE, r, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let report = self.block(f, "child.report");
        self.b.build_conditional_branch(bad_chdir, report, after_chdir).map_err(llvm_err)?;
        self.b.position_at_end(after_chdir);
        // `execvp` reads `environ`, so the child's environment becomes the one the command searches and runs in.
        let environ = self.environ_global();
        let envp = self.b.build_load(ptr_ty, envp_slot, "").map_err(llvm_err)?.into_pointer_value();
        self.b.build_store(environ, envp).map_err(llvm_err)?;
        let argv = self.b.build_load(ptr_ty, argv_slot, "").map_err(llvm_err)?.into_pointer_value();
        let program_c = self.b.build_load(ptr_ty, argv, "").map_err(llvm_err)?.into_pointer_value();
        self.call(execvp, &[program_c.into(), argv.into()])?;
        self.b.build_unconditional_branch(report).map_err(llvm_err)?;
        self.b.position_at_end(report);
        let one = self.b.build_alloca(i8_ty, "one").map_err(llvm_err)?;
        self.b.build_store(one, i8_ty.const_int(1, false)).map_err(llvm_err)?;
        let wfd = load_i32(self, report_write)?;
        let write =
            libc(self.module, "write", usize_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), usize_ty.into()], false));
        self.call(write, &[wfd.into(), one.into(), usize_ty.const_int(1, false).into()])?;
        self.call(quit, &[i32_ty.const_int(127, false).into()])?;
        self.b.build_unreachable().map_err(llvm_err)?;

        // Parent.
        self.b.position_at_end(parent);
        let wfd = load_i32(self, report_write)?;
        self.call(close, &[wfd.into()])?;
        for slot in [in_read, out_write, err_write, devnull] {
            let fd = load_i32(self, slot)?;
            self.call(close, &[fd.into()])?;
        }
        let byte = self.b.build_alloca(i8_ty, "report.byte").map_err(llvm_err)?;
        let rfd = load_i32(self, report_read)?;
        let n = self
            .call(read, &[rfd.into(), byte.into(), usize_ty.const_int(1, false).into()])?
            .expect("read returns a count")
            .into_int_value();
        let positive = self.b.build_int_compare(IntPredicate::SGT, n, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let exec_failed = self.block(f, "exec.failed");
        let exec_ok = self.block(f, "exec.ok");
        self.b.build_conditional_branch(positive, exec_failed, exec_ok).map_err(llvm_err)?;
        self.b.position_at_end(exec_failed);
        let rfd = load_i32(self, report_read)?;
        self.call(close, &[rfd.into()])?;
        for slot in [in_write, out_read, err_read] {
            let fd = load_i32(self, slot)?;
            self.call(close, &[fd.into()])?;
        }
        let status = self.b.build_alloca(i32_ty, "reap.status").map_err(llvm_err)?;
        self.call(waitpid, &[pid.into(), status.into(), i32_ty.const_zero().into()])?;
        self.free_cstring_array_if_present(f, argv_slot, argc, free)?;
        self.free_cstring_array_if_present(f, envp_slot, env_len, free)?;
        let cwd_c = self.b.build_load(ptr_ty, cwd_slot, "").map_err(llvm_err)?.into_pointer_value();
        self.call(free, &[cwd_c.into()])?;
        self.b.build_return(Some(&i32_ty.const_all_ones())).map_err(llvm_err)?;

        self.b.position_at_end(exec_ok);
        let rfd = load_i32(self, report_read)?;
        self.call(close, &[rfd.into()])?;
        self.free_cstring_array_if_present(f, argv_slot, argc, free)?;
        self.free_cstring_array_if_present(f, envp_slot, env_len, free)?;
        let cwd_c = self.b.build_load(ptr_ty, cwd_slot, "").map_err(llvm_err)?.into_pointer_value();
        self.call(free, &[cwd_c.into()])?;
        let set_out = |cg: &mut Self, at: u64, value: IntValue<'ctx>| -> GenResult<()> {
            let slot = unsafe { cg.b.build_in_bounds_gep(i64_ty, out, &[usize_ty.const_int(at, false)], "spawn.out") }
                .map_err(llvm_err)?;
            let value = cg.b.build_int_z_extend(value, i64_ty, "").map_err(llvm_err)?;
            cg.b.build_store(slot, value).map_err(llvm_err)?;
            Ok(())
        };
        set_out(self, 0, pid)?;
        let zero64 = i64_ty.const_zero();
        let slot = unsafe { self.b.build_in_bounds_gep(i64_ty, out, &[usize_ty.const_int(1, false)], "") }
            .map_err(llvm_err)?;
        self.b.build_store(slot, zero64).map_err(llvm_err)?;
        for (at, mode, fd_slot) in
            [(2u64, stdin_mode, in_write), (3, stdout_mode, out_read), (4, stderr_mode, err_read)]
        {
            let piped =
                self.b.build_int_compare(IntPredicate::EQ, mode, i32_ty.const_int(1, false), "").map_err(llvm_err)?;
            let fd = load_i32(self, fd_slot)?;
            let fd64 = self.b.build_int_z_extend(fd, i64_ty, "").map_err(llvm_err)?;
            let value = self.b.build_select(piped, fd64, i64_ty.const_zero(), "").map_err(llvm_err)?.into_int_value();
            let slot = unsafe { self.b.build_in_bounds_gep(i64_ty, out, &[usize_ty.const_int(at, false)], "") }
                .map_err(llvm_err)?;
            self.b.build_store(slot, value).map_err(llvm_err)?;
        }
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;

        // Failure before the child exists: close whatever was opened and release what was allocated.
        self.b.position_at_end(fail);
        for slot in [in_read, in_write, out_read, out_write, err_read, err_write, devnull, report_read, report_write] {
            let fd = load_i32(self, slot)?;
            self.call(close, &[fd.into()])?;
        }
        self.free_cstring_array_if_present(f, argv_slot, argc, free)?;
        self.free_cstring_array_if_present(f, envp_slot, env_len, free)?;
        let cwd_c = self.b.build_load(ptr_ty, cwd_slot, "").map_err(llvm_err)?.into_pointer_value();
        self.call(free, &[cwd_c.into()])?;
        self.b.build_return(Some(&i32_ty.const_all_ones())).map_err(llvm_err)?;
        Ok(())
    }

    /// A private NUL-terminated UTF-16 constant.
    fn wide_literal(&mut self, text: &str) -> PointerValue<'ctx> {
        let values: Vec<inkwell::values::IntValue<'ctx>> = text
            .encode_utf16()
            .chain(std::iter::once(0))
            .map(|u| self.cx.i16_type().const_int(u64::from(u), false))
            .collect();
        let data = self.cx.i16_type().const_array(&values);
        let global = self.module.add_global(data.get_type(), None, &format!("rt.wide.{}", self.globals));
        self.globals += 1;
        global.set_initializer(&data);
        global.set_constant(true);
        global.set_linkage(Linkage::Private);
        global.set_unnamed_addr(true);
        global.as_pointer_value()
    }

    /// One inheritable pipe: returns `(child handle, parent CRT descriptor)`. The descriptor is -1 and the handle 0
    /// when the pipe could not be created.
    fn windows_pipe(&mut self, sa: PointerValue<'ctx>, stdin: bool) -> GenResult<(IntValue<'ctx>, IntValue<'ctx>)> {
        let (i32_ty, i64_ty, ptr_ty) = (self.i32(), self.i64(), self.ptr_ty);
        let create_pipe = libc(
            self.module,
            "CreatePipe",
            i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into(), ptr_ty.into(), i32_ty.into()], false),
        );
        let set_handle = libc(
            self.module,
            "SetHandleInformation",
            i32_ty.fn_type(&[i64_ty.into(), i32_ty.into(), i32_ty.into()], false),
        );
        let open_os = libc(self.module, "_open_osfhandle", i32_ty.fn_type(&[i64_ty.into(), i32_ty.into()], false));
        let close_handle = libc(self.module, "CloseHandle", i32_ty.fn_type(&[i64_ty.into()], false));
        let f = self.b.get_insert_block().and_then(|b| b.get_parent()).expect("positioned");
        let read = self.b.build_alloca(i64_ty, "pipe.read").map_err(llvm_err)?;
        let write = self.b.build_alloca(i64_ty, "pipe.write").map_err(llvm_err)?;
        self.b.build_store(read, i64_ty.const_zero()).map_err(llvm_err)?;
        self.b.build_store(write, i64_ty.const_zero()).map_err(llvm_err)?;
        let child = self.b.build_alloca(i64_ty, "pipe.child").map_err(llvm_err)?;
        self.b.build_store(child, i64_ty.const_zero()).map_err(llvm_err)?;
        let fd = self.b.build_alloca(i32_ty, "pipe.fd").map_err(llvm_err)?;
        self.b.build_store(fd, i32_ty.const_all_ones()).map_err(llvm_err)?;
        let ok = self
            .call(create_pipe, &[read.into(), write.into(), sa.into(), i32_ty.const_zero().into()])?
            .expect("CreatePipe returns a flag")
            .into_int_value();
        let ok = self.truthy(ok)?;
        let good = self.block(f, "pipe.good");
        let done = self.block(f, "pipe.done");
        self.b.build_conditional_branch(ok, good, done).map_err(llvm_err)?;
        self.b.position_at_end(good);
        let read_h = self.b.build_load(i64_ty, read, "").map_err(llvm_err)?.into_int_value();
        let write_h = self.b.build_load(i64_ty, write, "").map_err(llvm_err)?.into_int_value();
        let (child_h, parent_h) = if stdin { (read_h, write_h) } else { (write_h, read_h) };
        self.b.build_store(child, child_h).map_err(llvm_err)?;
        // The parent's end must not leak into the child.
        let set = self
            .call(set_handle, &[parent_h.into(), i32_ty.const_int(1, false).into(), i32_ty.const_zero().into()])?
            .expect("SetHandleInformation returns a flag")
            .into_int_value();
        let set = self.truthy(set)?;
        let opened = self.block(f, "pipe.opened");
        let cleanup = self.block(f, "pipe.cleanup");
        self.b.build_conditional_branch(set, opened, cleanup).map_err(llvm_err)?;
        self.b.position_at_end(opened);
        let handle_fd = self
            .call(open_os, &[parent_h.into(), i32_ty.const_int(0x8000, false).into()])?
            .expect("_open_osfhandle returns a descriptor")
            .into_int_value();
        let invalid =
            self.b.build_int_compare(IntPredicate::EQ, handle_fd, i32_ty.const_all_ones(), "").map_err(llvm_err)?;
        let keep = self.block(f, "pipe.keep");
        let drop_handle = self.block(f, "pipe.drop");
        self.b.build_conditional_branch(invalid, drop_handle, keep).map_err(llvm_err)?;
        self.b.position_at_end(keep);
        self.b.build_store(fd, handle_fd).map_err(llvm_err)?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(drop_handle);
        self.call(close_handle, &[parent_h.into()])?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(cleanup);
        self.call(close_handle, &[parent_h.into()])?;
        self.call(close_handle, &[child_h.into()])?;
        self.b.build_store(child, i64_ty.const_zero()).map_err(llvm_err)?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(done);
        let child = self.b.build_load(i64_ty, child, "child").map_err(llvm_err)?.into_int_value();
        let fd = self.b.build_load(i32_ty, fd, "fd").map_err(llvm_err)?.into_int_value();
        Ok((child, fd))
    }

    /// Opens `NUL` as an inheritable handle, or returns 0.
    fn windows_nul_handle(&mut self, sa: PointerValue<'ctx>, path: PointerValue<'ctx>) -> GenResult<IntValue<'ctx>> {
        let (i32_ty, i64_ty) = (self.i32(), self.i64());
        let create_file = libc(
            self.module,
            "CreateFileW",
            i64_ty.fn_type(
                &[
                    self.ptr_ty.into(),
                    i32_ty.into(),
                    i32_ty.into(),
                    self.ptr_ty.into(),
                    i32_ty.into(),
                    i32_ty.into(),
                    i64_ty.into(),
                ],
                false,
            ),
        );
        let handle = self
            .call(
                create_file,
                &[
                    path.into(),
                    i32_ty.const_int(0xC000_0000, false).into(),
                    i32_ty.const_int(3, false).into(),
                    sa.into(),
                    i32_ty.const_int(3, false).into(),
                    i32_ty.const_zero().into(),
                    i64_ty.const_zero().into(),
                ],
            )?
            .expect("CreateFileW returns a handle")
            .into_int_value();
        let invalid =
            self.b.build_int_compare(IntPredicate::EQ, handle, i64_ty.const_all_ones(), "").map_err(llvm_err)?;
        Ok(self.b.build_select(invalid, i64_ty.const_zero(), handle, "").map_err(llvm_err)?.into_int_value())
    }

    /// A duplicate of one of the parent's standard handles, made inheritable; 0 when it cannot be duplicated.
    fn windows_dup_std(&mut self, which: i32) -> GenResult<IntValue<'ctx>> {
        let (i32_ty, i64_ty) = (self.i32(), self.i64());
        let get = libc(self.module, "GetStdHandle", i64_ty.fn_type(&[i32_ty.into()], false));
        let dup = libc(
            self.module,
            "DuplicateHandle",
            i32_ty.fn_type(
                &[
                    i64_ty.into(),
                    i64_ty.into(),
                    i64_ty.into(),
                    self.ptr_ty.into(),
                    i32_ty.into(),
                    i32_ty.into(),
                    i32_ty.into(),
                ],
                false,
            ),
        );
        let handle = self
            .call(get, &[i32_ty.const_int(u64::from(which as u32), false).into()])?
            .expect("GetStdHandle returns a handle")
            .into_int_value();
        let target = self.b.build_alloca(i64_ty, "dup.handle").map_err(llvm_err)?;
        self.b.build_store(target, i64_ty.const_zero()).map_err(llvm_err)?;
        let current = i64_ty.const_all_ones();
        let ok = self
            .call(
                dup,
                &[
                    current.into(),
                    handle.into(),
                    current.into(),
                    target.into(),
                    i32_ty.const_zero().into(),
                    i32_ty.const_int(1, false).into(),
                    i32_ty.const_int(2, false).into(),
                ],
            )?
            .expect("DuplicateHandle returns a flag")
            .into_int_value();
        let ok = self.truthy(ok)?;
        let got = self.b.build_load(i64_ty, target, "").map_err(llvm_err)?.into_int_value();
        Ok(self.b.build_select(ok, got, i64_ty.const_zero(), "").map_err(llvm_err)?.into_int_value())
    }

    /// Connects one standard stream for `CreateProcessW`: a pipe, `NUL`, or a duplicate of the parent's handle.
    #[allow(clippy::too_many_arguments)]
    fn windows_pick_stream(
        &mut self,
        f: FunctionValue<'ctx>,
        mode: IntValue<'ctx>,
        stdin: bool,
        std_index: i32,
        sa: PointerValue<'ctx>,
        nul: PointerValue<'ctx>,
        fd_slot: PointerValue<'ctx>,
        child_slot: PointerValue<'ctx>,
        fail: BasicBlock<'ctx>,
    ) -> GenResult<()> {
        let (i32_ty, i64_ty) = (self.i32(), self.i64());
        let pipe_bb = self.block(f, "stream.pipe");
        let nul_bb = self.block(f, "stream.nul");
        let inherit_bb = self.block(f, "stream.inherit");
        let done = self.block(f, "stream.done");
        let is_pipe =
            self.b.build_int_compare(IntPredicate::EQ, mode, i32_ty.const_int(1, false), "").map_err(llvm_err)?;
        let is_nul =
            self.b.build_int_compare(IntPredicate::EQ, mode, i32_ty.const_int(2, false), "").map_err(llvm_err)?;
        let not_pipe = self.block(f, "stream.not.pipe");
        self.b.build_conditional_branch(is_pipe, pipe_bb, not_pipe).map_err(llvm_err)?;
        self.b.position_at_end(not_pipe);
        self.b.build_conditional_branch(is_nul, nul_bb, inherit_bb).map_err(llvm_err)?;

        self.b.position_at_end(pipe_bb);
        let (child, fd) = self.windows_pipe(sa, stdin)?;
        let good = self
            .b
            .build_and(
                self.b.build_int_compare(IntPredicate::NE, child, i64_ty.const_zero(), "").map_err(llvm_err)?,
                self.b.build_int_compare(IntPredicate::NE, fd, i32_ty.const_all_ones(), "").map_err(llvm_err)?,
                "",
            )
            .map_err(llvm_err)?;
        let store = self.block(f, "stream.pipe.store");
        self.b.build_conditional_branch(good, store, fail).map_err(llvm_err)?;
        self.b.position_at_end(store);
        self.b.build_store(fd_slot, fd).map_err(llvm_err)?;
        self.b.build_store(child_slot, child).map_err(llvm_err)?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;

        self.b.position_at_end(nul_bb);
        let child = self.windows_nul_handle(sa, nul)?;
        let good = self.b.build_int_compare(IntPredicate::NE, child, i64_ty.const_zero(), "").map_err(llvm_err)?;
        let store = self.block(f, "stream.nul.store");
        self.b.build_conditional_branch(good, store, fail).map_err(llvm_err)?;
        self.b.position_at_end(store);
        self.b.build_store(child_slot, child).map_err(llvm_err)?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;

        self.b.position_at_end(inherit_bb);
        let child = self.windows_dup_std(std_index)?;
        let good = self.b.build_int_compare(IntPredicate::NE, child, i64_ty.const_zero(), "").map_err(llvm_err)?;
        let store = self.block(f, "stream.inherit.store");
        self.b.build_conditional_branch(good, store, fail).map_err(llvm_err)?;
        self.b.position_at_end(store);
        self.b.build_store(child_slot, child).map_err(llvm_err)?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(done);
        Ok(())
    }

    /// `process_spawn` on Windows: `CreateProcessW` with a final environment block and inheritable pipes.
    #[allow(clippy::too_many_arguments)]
    fn process_spawn_windows(
        &mut self,
        f: FunctionValue<'ctx>,
        _program: PointerValue<'ctx>,
        _program_len: IntValue<'ctx>,
        _args: PointerValue<'ctx>,
        _args_len: IntValue<'ctx>,
        env: PointerValue<'ctx>,
        env_len: IntValue<'ctx>,
        cwd: PointerValue<'ctx>,
        cwd_len: IntValue<'ctx>,
        cmdline: PointerValue<'ctx>,
        cmdline_len: IntValue<'ctx>,
        stdin_mode: IntValue<'ctx>,
        stdout_mode: IntValue<'ctx>,
        stderr_mode: IntValue<'ctx>,
        out: PointerValue<'ctx>,
    ) -> GenResult<()> {
        let (usize_ty, ptr_ty, i32_ty, i64_ty) = (self.usize_ty, self.ptr_ty, self.i32(), self.i64());
        let void = self.cx.void_type();
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", void.fn_type(&[ptr_ty.into()], false));
        let memset =
            libc(self.module, "memset", ptr_ty.fn_type(&[ptr_ty.into(), i32_ty.into(), usize_ty.into()], false));
        let create = libc(
            self.module,
            "CreateProcessW",
            i32_ty.fn_type(
                &[
                    ptr_ty.into(),
                    ptr_ty.into(),
                    ptr_ty.into(),
                    ptr_ty.into(),
                    i32_ty.into(),
                    i32_ty.into(),
                    ptr_ty.into(),
                    ptr_ty.into(),
                    ptr_ty.into(),
                    ptr_ty.into(),
                ],
                false,
            ),
        );
        let close_handle = libc(self.module, "CloseHandle", i32_ty.fn_type(&[i64_ty.into()], false));
        let close_fd = libc(self.module, "_close", i32_ty.fn_type(&[i32_ty.into()], false));

        let fail = self.block(f, "spawn.fail");
        let fail_free = self.block(f, "spawn.fail.free");
        let env_block = self.b.build_alloca(ptr_ty, "env.block").map_err(llvm_err)?;
        self.b.build_store(env_block, ptr_ty.const_null()).map_err(llvm_err)?;
        let cmdline_wide = self.b.build_alloca(ptr_ty, "cmdline.wide").map_err(llvm_err)?;
        self.b.build_store(cmdline_wide, ptr_ty.const_null()).map_err(llvm_err)?;
        let cwd_wide = self.b.build_alloca(ptr_ty, "cwd.wide").map_err(llvm_err)?;
        self.b.build_store(cwd_wide, ptr_ty.const_null()).map_err(llvm_err)?;
        // The Windows structures and the stream bookkeeping live in the entry block, so every failure path can
        // release them.
        let sa = self.b.build_alloca(i64_ty.array_type(3), "sa").map_err(llvm_err)?;
        self.call(memset, &[sa.into(), i32_ty.const_zero().into(), usize_ty.const_int(24, false).into()])?;
        let at = self.offset(sa, usize_ty.const_zero())?;
        self.b.build_store(at, i32_ty.const_int(24, false)).map_err(llvm_err)?;
        let at = self.offset(sa, usize_ty.const_int(16, false))?;
        self.b.build_store(at, i32_ty.const_int(1, false)).map_err(llvm_err)?;
        let si = self.b.build_alloca(i64_ty.array_type(13), "startup").map_err(llvm_err)?;
        self.call(memset, &[si.into(), i32_ty.const_zero().into(), usize_ty.const_int(104, false).into()])?;
        let at = self.offset(si, usize_ty.const_zero())?;
        self.b.build_store(at, i32_ty.const_int(104, false)).map_err(llvm_err)?;
        let pi = self.b.build_alloca(i64_ty.array_type(3), "process.info").map_err(llvm_err)?;
        self.call(memset, &[pi.into(), i32_ty.const_zero().into(), usize_ty.const_int(24, false).into()])?;
        let fds = self.b.build_alloca(i32_ty.array_type(3), "std.fds").map_err(llvm_err)?;
        let child_handles = self.b.build_alloca(i64_ty.array_type(3), "std.handles").map_err(llvm_err)?;
        for i in 0..3u64 {
            let slot = unsafe { self.b.build_in_bounds_gep(i32_ty, fds, &[usize_ty.const_int(i, false)], "") }
                .map_err(llvm_err)?;
            self.b.build_store(slot, i32_ty.const_all_ones()).map_err(llvm_err)?;
            let slot =
                unsafe { self.b.build_in_bounds_gep(i64_ty, child_handles, &[usize_ty.const_int(i, false)], "") }
                    .map_err(llvm_err)?;
            self.b.build_store(slot, i64_ty.const_zero()).map_err(llvm_err)?;
        }

        // The environment block: `NAME=VALUE` strings, each followed by a NUL, and a final NUL.
        let total = self.b.build_alloca(usize_ty, "env.total").map_err(llvm_err)?;
        self.b.build_store(total, usize_ty.const_zero()).map_err(llvm_err)?;
        let index = self.b.build_alloca(usize_ty, "env.i").map_err(llvm_err)?;
        self.b.build_store(index, usize_ty.const_zero()).map_err(llvm_err)?;
        let units = self.b.build_alloca(usize_ty, "env.units").map_err(llvm_err)?;
        let pass1 = self.block(f, "env.pass1");
        let pass1_body = self.block(f, "env.pass1.body");
        let measured = self.block(f, "env.measured");
        self.b.build_unconditional_branch(pass1).map_err(llvm_err)?;
        self.b.position_at_end(pass1);
        let i = self.b.build_load(usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let in_range = self.b.build_int_compare(IntPredicate::ULT, i, env_len, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(in_range, pass1_body, measured).map_err(llvm_err)?;
        self.b.position_at_end(pass1_body);
        let elem = unsafe { self.b.build_in_bounds_gep(self.string_ty, env, &[i], "env.elem") }.map_err(llvm_err)?;
        let ptr_field = self.b.build_struct_gep(self.string_ty, elem, 0, "").map_err(llvm_err)?;
        let len_field = self.b.build_struct_gep(self.string_ty, elem, 1, "").map_err(llvm_err)?;
        let text = self.b.build_load(ptr_ty, ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        let text_len = self.b.build_load(usize_ty, len_field, "").map_err(llvm_err)?.into_int_value();
        let wide = self.utf8_to_wide(text, text_len, units)?;
        let null = self.b.build_is_null(wide, "").map_err(llvm_err)?;
        let measured_ok = self.block(f, "env.pass1.add");
        self.b.build_conditional_branch(null, fail, measured_ok).map_err(llvm_err)?;
        self.b.position_at_end(measured_ok);
        let count = self.b.build_load(usize_ty, units, "").map_err(llvm_err)?.into_int_value();
        let count = self.b.build_int_add(count, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        let sum = self.b.build_load(usize_ty, total, "").map_err(llvm_err)?.into_int_value();
        let sum = self.b.build_int_add(sum, count, "").map_err(llvm_err)?;
        self.b.build_store(total, sum).map_err(llvm_err)?;
        self.call(free, &[wide.into()])?;
        let next = self.b.build_int_add(i, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_store(index, next).map_err(llvm_err)?;
        self.b.build_unconditional_branch(pass1).map_err(llvm_err)?;

        self.b.position_at_end(measured);
        let sum = self.b.build_load(usize_ty, total, "").map_err(llvm_err)?.into_int_value();
        let with_end = self.b.build_int_add(sum, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        let block_units = self.max_usize(with_end, usize_ty.const_int(2, false))?;
        let block_bytes = self.b.build_int_mul(block_units, usize_ty.const_int(2, false), "").map_err(llvm_err)?;
        let block = self.call(malloc, &[block_bytes.into()])?.expect("malloc returns a pointer").into_pointer_value();
        self.b.build_store(env_block, block).map_err(llvm_err)?;
        self.call(memset, &[block.into(), i32_ty.const_zero().into(), block_bytes.into()])?;
        let cursor = self.b.build_alloca(ptr_ty, "env.cursor").map_err(llvm_err)?;
        self.b.build_store(cursor, block).map_err(llvm_err)?;
        self.b.build_store(index, usize_ty.const_zero()).map_err(llvm_err)?;
        let pass2 = self.block(f, "env.pass2");
        let pass2_body = self.block(f, "env.pass2.body");
        let built = self.block(f, "env.built");
        self.b.build_unconditional_branch(pass2).map_err(llvm_err)?;
        self.b.position_at_end(pass2);
        let i = self.b.build_load(usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let in_range = self.b.build_int_compare(IntPredicate::ULT, i, env_len, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(in_range, pass2_body, built).map_err(llvm_err)?;
        self.b.position_at_end(pass2_body);
        let elem = unsafe { self.b.build_in_bounds_gep(self.string_ty, env, &[i], "env.elem") }.map_err(llvm_err)?;
        let ptr_field = self.b.build_struct_gep(self.string_ty, elem, 0, "").map_err(llvm_err)?;
        let len_field = self.b.build_struct_gep(self.string_ty, elem, 1, "").map_err(llvm_err)?;
        let text = self.b.build_load(ptr_ty, ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        let text_len = self.b.build_load(usize_ty, len_field, "").map_err(llvm_err)?.into_int_value();
        let wide = self.utf8_to_wide(text, text_len, units)?;
        let null = self.b.build_is_null(wide, "").map_err(llvm_err)?;
        let copied = self.block(f, "env.pass2.copy");
        self.b.build_conditional_branch(null, fail, copied).map_err(llvm_err)?;
        self.b.position_at_end(copied);
        let count = self.b.build_load(usize_ty, units, "").map_err(llvm_err)?.into_int_value();
        let width = self.b.build_int_mul(count, usize_ty.const_int(2, false), "").map_err(llvm_err)?;
        let at = self.b.build_load(ptr_ty, cursor, "").map_err(llvm_err)?.into_pointer_value();
        self.b.build_memcpy(at, 1, wide, 1, width).map_err(llvm_err)?;
        self.call(free, &[wide.into()])?;
        let count = self.b.build_int_add(count, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        let step = self.b.build_int_mul(count, usize_ty.const_int(2, false), "").map_err(llvm_err)?;
        let next = self.offset(at, step)?;
        self.b.build_store(cursor, next).map_err(llvm_err)?;
        let next = self.b.build_int_add(i, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_store(index, next).map_err(llvm_err)?;
        self.b.build_unconditional_branch(pass2).map_err(llvm_err)?;

        self.b.position_at_end(built);
        // The command line and the working directory, in UTF-16.
        let line_units = self.b.build_alloca(usize_ty, "cmdline.units").map_err(llvm_err)?;
        let line = self.utf8_to_wide(cmdline, cmdline_len, line_units)?;
        let null = self.b.build_is_null(line, "").map_err(llvm_err)?;
        let line_ok = self.block(f, "cmdline.ok");
        self.b.build_conditional_branch(null, fail, line_ok).map_err(llvm_err)?;
        self.b.position_at_end(line_ok);
        self.b.build_store(cmdline_wide, line).map_err(llvm_err)?;
        let has_cwd =
            self.b.build_int_compare(IntPredicate::NE, cwd_len, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let cwd_bb = self.block(f, "cwd.convert");
        let cwd_done = self.block(f, "cwd.converted");
        self.b.build_conditional_branch(has_cwd, cwd_bb, cwd_done).map_err(llvm_err)?;
        self.b.position_at_end(cwd_bb);
        let cwd_units = self.b.build_alloca(usize_ty, "cwd.units").map_err(llvm_err)?;
        let wide = self.utf8_to_wide(cwd, cwd_len, cwd_units)?;
        let null = self.b.build_is_null(wide, "").map_err(llvm_err)?;
        let cwd_ok = self.block(f, "cwd.ok");
        self.b.build_conditional_branch(null, fail, cwd_ok).map_err(llvm_err)?;
        self.b.position_at_end(cwd_ok);
        self.b.build_store(cwd_wide, wide).map_err(llvm_err)?;
        self.b.build_unconditional_branch(cwd_done).map_err(llvm_err)?;

        self.b.position_at_end(cwd_done);
        let redirect = self
            .b
            .build_or(
                self.b
                    .build_or(
                        self.b
                            .build_int_compare(IntPredicate::NE, stdin_mode, i32_ty.const_zero(), "")
                            .map_err(llvm_err)?,
                        self.b
                            .build_int_compare(IntPredicate::NE, stdout_mode, i32_ty.const_zero(), "")
                            .map_err(llvm_err)?,
                        "",
                    )
                    .map_err(llvm_err)?,
                self.b.build_int_compare(IntPredicate::NE, stderr_mode, i32_ty.const_zero(), "").map_err(llvm_err)?,
                "",
            )
            .map_err(llvm_err)?;
        let plain = self.block(f, "spawn.plain");
        let redirected = self.block(f, "spawn.redirected");
        let launch = self.block(f, "spawn.launch");
        self.b.build_conditional_branch(redirect, redirected, plain).map_err(llvm_err)?;

        self.b.position_at_end(redirected);
        let at = self.offset(si, usize_ty.const_int(60, false))?;
        self.b.build_store(at, i32_ty.const_int(0x100, false)).map_err(llvm_err)?;
        let nul = self.wide_literal("NUL");
        let fd_slot = |cg: &mut Self, i: u64| -> GenResult<PointerValue<'ctx>> {
            unsafe { cg.b.build_in_bounds_gep(i32_ty, fds, &[usize_ty.const_int(i, false)], "") }.map_err(llvm_err)
        };
        let handle_slot = |cg: &mut Self, i: u64| -> GenResult<PointerValue<'ctx>> {
            unsafe { cg.b.build_in_bounds_gep(i64_ty, child_handles, &[usize_ty.const_int(i, false)], "") }
                .map_err(llvm_err)
        };
        let fd0 = fd_slot(self, 0)?;
        let fd1 = fd_slot(self, 1)?;
        let fd2 = fd_slot(self, 2)?;
        let h0 = handle_slot(self, 0)?;
        let h1 = handle_slot(self, 1)?;
        let h2 = handle_slot(self, 2)?;
        self.windows_pick_stream(f, stdin_mode, true, -10, sa, nul, fd0, h0, fail)?;
        self.windows_pick_stream(f, stdout_mode, false, -11, sa, nul, fd1, h1, fail)?;
        self.windows_pick_stream(f, stderr_mode, false, -12, sa, nul, fd2, h2, fail)?;
        for i in 0..3u64 {
            let slot = handle_slot(self, i)?;
            let handle = self.b.build_load(i64_ty, slot, "").map_err(llvm_err)?.into_int_value();
            let at = self.offset(si, usize_ty.const_int(80 + 8 * i, false))?;
            self.b.build_store(at, handle).map_err(llvm_err)?;
        }
        self.b.build_unconditional_branch(launch).map_err(llvm_err)?;

        self.b.position_at_end(plain);
        self.b.build_unconditional_branch(launch).map_err(llvm_err)?;

        self.b.position_at_end(launch);
        let inherit = self.b.build_int_z_extend(redirect, i32_ty, "").map_err(llvm_err)?;
        let block = self.b.build_load(ptr_ty, env_block, "").map_err(llvm_err)?.into_pointer_value();
        let line = self.b.build_load(ptr_ty, cmdline_wide, "").map_err(llvm_err)?.into_pointer_value();
        let dir = self.b.build_load(ptr_ty, cwd_wide, "").map_err(llvm_err)?.into_pointer_value();
        let created = self
            .call(
                create,
                &[
                    ptr_ty.const_null().into(),
                    line.into(),
                    ptr_ty.const_null().into(),
                    ptr_ty.const_null().into(),
                    inherit.into(),
                    i32_ty.const_int(0x400, false).into(),
                    block.into(),
                    dir.into(),
                    si.into(),
                    pi.into(),
                ],
            )?
            .expect("CreateProcessW returns a flag")
            .into_int_value();
        let created = self.truthy(created)?;
        let launched = self.block(f, "spawn.launched");
        self.b.build_conditional_branch(created, launched, fail).map_err(llvm_err)?;

        self.b.position_at_end(launched);
        let process = self.b.build_load(i64_ty, pi, "process").map_err(llvm_err)?.into_int_value();
        let thread_at = self.offset(pi, usize_ty.const_int(8, false))?;
        let thread = self.b.build_load(i64_ty, thread_at, "thread").map_err(llvm_err)?.into_int_value();
        self.call(close_handle, &[thread.into()])?;
        for i in 0..3u64 {
            let slot = handle_slot(self, i)?;
            let handle = self.b.build_load(i64_ty, slot, "").map_err(llvm_err)?.into_int_value();
            self.call(close_handle, &[handle.into()])?;
        }
        let pid_at = self.offset(pi, usize_ty.const_int(16, false))?;
        let pid = self.b.build_load(i32_ty, pid_at, "pid").map_err(llvm_err)?.into_int_value();
        let out_slot = |cg: &mut Self, at: u64| -> GenResult<PointerValue<'ctx>> {
            unsafe { cg.b.build_in_bounds_gep(i64_ty, out, &[usize_ty.const_int(at, false)], "") }.map_err(llvm_err)
        };
        let pid = self.b.build_int_z_extend(pid, i64_ty, "").map_err(llvm_err)?;
        let slot = out_slot(self, 0)?;
        self.b.build_store(slot, pid).map_err(llvm_err)?;
        let slot = out_slot(self, 1)?;
        self.b.build_store(slot, process).map_err(llvm_err)?;
        for i in 0..3u64 {
            let fd_slot = fd_slot(self, i)?;
            let fd = self.b.build_load(i32_ty, fd_slot, "").map_err(llvm_err)?.into_int_value();
            let piped =
                self.b.build_int_compare(IntPredicate::NE, fd, i32_ty.const_all_ones(), "").map_err(llvm_err)?;
            let fd = self.b.build_int_z_extend(fd, i64_ty, "").map_err(llvm_err)?;
            let value = self.b.build_select(piped, fd, i64_ty.const_zero(), "").map_err(llvm_err)?.into_int_value();
            let slot = out_slot(self, 2 + i)?;
            self.b.build_store(slot, value).map_err(llvm_err)?;
        }
        let block = self.b.build_load(ptr_ty, env_block, "").map_err(llvm_err)?.into_pointer_value();
        self.call(free, &[block.into()])?;
        let line = self.b.build_load(ptr_ty, cmdline_wide, "").map_err(llvm_err)?.into_pointer_value();
        self.call(free, &[line.into()])?;
        let dir = self.b.build_load(ptr_ty, cwd_wide, "").map_err(llvm_err)?.into_pointer_value();
        self.call(free, &[dir.into()])?;
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;

        self.b.position_at_end(fail);
        for i in 0..3u64 {
            let slot = handle_slot(self, i)?;
            let handle = self.b.build_load(i64_ty, slot, "").map_err(llvm_err)?.into_int_value();
            self.call(close_handle, &[handle.into()])?;
            let slot = fd_slot(self, i)?;
            let fd = self.b.build_load(i32_ty, slot, "").map_err(llvm_err)?.into_int_value();
            // `_close(-1)` would reach the CRT's invalid-parameter handler, so only a descriptor that was opened
            // is closed.
            let open = self.b.build_int_compare(IntPredicate::NE, fd, i32_ty.const_all_ones(), "").map_err(llvm_err)?;
            let close = self.block(f, "spawn.fail.close");
            let next = self.block(f, "spawn.fail.next");
            self.b.build_conditional_branch(open, close, next).map_err(llvm_err)?;
            self.b.position_at_end(close);
            self.call(close_fd, &[fd.into()])?;
            self.b.build_unconditional_branch(next).map_err(llvm_err)?;
            self.b.position_at_end(next);
        }
        self.b.build_unconditional_branch(fail_free).map_err(llvm_err)?;
        self.b.position_at_end(fail_free);
        let block = self.b.build_load(ptr_ty, env_block, "").map_err(llvm_err)?.into_pointer_value();
        self.call(free, &[block.into()])?;
        let line = self.b.build_load(ptr_ty, cmdline_wide, "").map_err(llvm_err)?.into_pointer_value();
        self.call(free, &[line.into()])?;
        let dir = self.b.build_load(ptr_ty, cwd_wide, "").map_err(llvm_err)?.into_pointer_value();
        self.call(free, &[dir.into()])?;
        self.b.build_return(Some(&i32_ty.const_all_ones())).map_err(llvm_err)?;
        Ok(())
    }

    /// `process_wait(pid: i32, process: usize) -> i64`.
    fn process_wait(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (i32_ty, i64_ty, usize_ty) = (self.i32(), self.i64(), self.usize_ty);
        let f = self.function("burdin_rt_process_wait", i64_ty.fn_type(&[i32_ty.into(), usize_ty.into()], false));
        let pid = Self::int_param(f, 0);
        let process = Self::int_param(f, 1);
        if self.windows {
            let wait = libc(self.module, "WaitForSingleObject", i32_ty.fn_type(&[i64_ty.into(), i32_ty.into()], false));
            let get =
                libc(self.module, "GetExitCodeProcess", i32_ty.fn_type(&[i64_ty.into(), self.ptr_ty.into()], false));
            let bad = self.block(f, "wait.bad");
            let waited = self.block(f, "wait.done");
            let zero =
                self.b.build_int_compare(IntPredicate::EQ, process, usize_ty.const_zero(), "").map_err(llvm_err)?;
            self.b.build_conditional_branch(zero, bad, waited).map_err(llvm_err)?;
            self.b.position_at_end(waited);
            let process = self.b.build_int_truncate(process, i64_ty, "process").map_err(llvm_err)?;
            let result = self
                .call(wait, &[process.into(), i32_ty.const_int(0xFFFF_FFFF, false).into()])?
                .expect("WaitForSingleObject returns a status")
                .into_int_value();
            let failed = self
                .b
                .build_int_compare(IntPredicate::EQ, result, i32_ty.const_int(0xFFFF_FFFF, false), "")
                .map_err(llvm_err)?;
            let code = self.b.build_alloca(i32_ty, "exit.code").map_err(llvm_err)?;
            self.b.build_store(code, i32_ty.const_zero()).map_err(llvm_err)?;
            let checked = self.block(f, "wait.checked");
            self.b.build_conditional_branch(failed, bad, checked).map_err(llvm_err)?;
            self.b.position_at_end(checked);
            let got = self
                .call(get, &[process.into(), code.into()])?
                .expect("GetExitCodeProcess returns a flag")
                .into_int_value();
            let no = self.b.build_int_compare(IntPredicate::EQ, got, i32_ty.const_zero(), "").map_err(llvm_err)?;
            let good = self.block(f, "wait.good");
            self.b.build_conditional_branch(no, bad, good).map_err(llvm_err)?;
            self.b.position_at_end(good);
            let code = self.b.build_load(i32_ty, code, "code").map_err(llvm_err)?.into_int_value();
            let code = self.b.build_int_z_extend(code, i64_ty, "code64").map_err(llvm_err)?;
            self.b.build_return(Some(&code)).map_err(llvm_err)?;
            self.b.position_at_end(bad);
            self.b.build_return(Some(&i64_ty.const_all_ones())).map_err(llvm_err)?;
            return Ok(f);
        }

        let waitpid =
            libc(self.module, "waitpid", i32_ty.fn_type(&[i32_ty.into(), self.ptr_ty.into(), i32_ty.into()], false));
        let status = self.b.build_alloca(i32_ty, "wait.status").map_err(llvm_err)?;
        self.b.build_store(status, i32_ty.const_zero()).map_err(llvm_err)?;
        let r = self
            .call(waitpid, &[pid.into(), status.into(), i32_ty.const_zero().into()])?
            .expect("waitpid returns a pid")
            .into_int_value();
        let bad = self.block(f, "wait.bad");
        let got = self.block(f, "wait.got");
        let failed = self.b.build_int_compare(IntPredicate::SLT, r, i32_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(failed, bad, got).map_err(llvm_err)?;
        self.b.position_at_end(got);
        let s = self.b.build_load(i32_ty, status, "status").map_err(llvm_err)?.into_int_value();
        let low = self.b.build_and(s, i32_ty.const_int(0x7f, false), "").map_err(llvm_err)?;
        let exited = self.b.build_int_compare(IntPredicate::EQ, low, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let stopped =
            self.b.build_int_compare(IntPredicate::EQ, low, i32_ty.const_int(0x7f, false), "").map_err(llvm_err)?;
        let code_bb = self.block(f, "wait.exited");
        let signal_bb = self.block(f, "wait.signaled");
        let stop_bb = self.block(f, "wait.stopped");
        self.b.build_conditional_branch(exited, code_bb, signal_bb).map_err(llvm_err)?;
        self.b.position_at_end(code_bb);
        let shifted = self.b.build_right_shift(s, i32_ty.const_int(8, false), false, "").map_err(llvm_err)?;
        let code = self.b.build_and(shifted, i32_ty.const_int(0xff, false), "").map_err(llvm_err)?;
        let code = self.b.build_int_z_extend(code, i64_ty, "code64").map_err(llvm_err)?;
        self.b.build_return(Some(&code)).map_err(llvm_err)?;
        self.b.position_at_end(signal_bb);
        let stopped_branch = self.block(f, "wait.check.stopped");
        self.b.build_conditional_branch(stopped, stop_bb, stopped_branch).map_err(llvm_err)?;
        self.b.position_at_end(stopped_branch);
        let signal = self.b.build_int_z_extend(low, i64_ty, "signal").map_err(llvm_err)?;
        let encoded = self.b.build_int_add(signal, i64_ty.const_int(1, false), "").map_err(llvm_err)?;
        let encoded = self.b.build_int_sub(i64_ty.const_zero(), encoded, "").map_err(llvm_err)?;
        self.b.build_return(Some(&encoded)).map_err(llvm_err)?;
        self.b.position_at_end(stop_bb);
        self.b.build_return(Some(&i64_ty.const_all_ones())).map_err(llvm_err)?;
        self.b.position_at_end(bad);
        self.b.build_return(Some(&i64_ty.const_all_ones())).map_err(llvm_err)?;
        Ok(f)
    }

    /// `process_close(process: usize)`: closes the Windows process handle.
    fn process_close(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, i64_ty, i32_ty) = (self.usize_ty, self.i64(), self.i32());
        let void = self.cx.void_type();
        let f = self.function("burdin_rt_process_close", void.fn_type(&[usize_ty.into()], false));
        let process = Self::int_param(f, 0);
        let done = self.block(f, "close.done");
        let zero = self.b.build_int_compare(IntPredicate::EQ, process, usize_ty.const_zero(), "").map_err(llvm_err)?;
        if self.windows {
            let close = libc(self.module, "CloseHandle", i32_ty.fn_type(&[i64_ty.into()], false));
            let close_bb = self.block(f, "close.handle");
            self.b.build_conditional_branch(zero, done, close_bb).map_err(llvm_err)?;
            self.b.position_at_end(close_bb);
            let process = self.b.build_int_truncate(process, i64_ty, "").map_err(llvm_err)?;
            self.call(close, &[process.into()])?;
        } else {
            let _ = zero;
        }
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(done);
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `pipe_close(handle: usize)`: closes a pipe end; handle 0 is a no-op.
    fn pipe_close(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, i32_ty) = (self.usize_ty, self.i32());
        let void = self.cx.void_type();
        let f = self.function("burdin_rt_pipe_close", void.fn_type(&[usize_ty.into()], false));
        let handle = Self::int_param(f, 0);
        let close = if self.windows {
            libc(self.module, "_close", i32_ty.fn_type(&[i32_ty.into()], false))
        } else {
            libc(self.module, "close", i32_ty.fn_type(&[i32_ty.into()], false))
        };
        let done = self.block(f, "pipe.close.done");
        let close_bb = self.block(f, "pipe.close.fd");
        let zero = self.b.build_int_compare(IntPredicate::EQ, handle, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(zero, done, close_bb).map_err(llvm_err)?;
        self.b.position_at_end(close_bb);
        let fd = self.b.build_int_truncate(handle, i32_ty, "fd").map_err(llvm_err)?;
        self.call(close, &[fd.into()])?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(done);
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(f)
    }

    /// `pipe_write(handle: usize, ptr, len: usize) -> i32`: writes every byte; 0 on success and -1 on failure.
    fn pipe_write(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let f = self.function(
            "burdin_rt_pipe_write",
            i32_ty.fn_type(&[usize_ty.into(), ptr_ty.into(), usize_ty.into()], false),
        );
        let fd = self.b.build_int_truncate(Self::int_param(f, 0), i32_ty, "fd").map_err(llvm_err)?;
        let ptr0 = Self::ptr_param(f, 1);
        let len0 = Self::int_param(f, 2);
        let write = if self.windows {
            libc(self.module, "_write", i32_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), i32_ty.into()], false))
        } else {
            libc(self.module, "write", usize_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), usize_ty.into()], false))
        };
        let entry = self.b.get_insert_block().expect("positioned");
        let lp = self.block(f, "write.loop");
        let body = self.block(f, "write.body");
        let next = self.block(f, "write.next");
        let done = self.block(f, "write.done");
        let failed = self.block(f, "write.failed");
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;
        self.b.position_at_end(lp);
        let ptr = self.b.build_phi(ptr_ty, "ptr").map_err(llvm_err)?;
        let len = self.b.build_phi(usize_ty, "len").map_err(llvm_err)?;
        let len_v = len.as_basic_value().into_int_value();
        let ptr_v = ptr.as_basic_value().into_pointer_value();
        let empty = self.b.build_int_compare(IntPredicate::EQ, len_v, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(empty, done, body).map_err(llvm_err)?;
        self.b.position_at_end(body);
        let max = usize_ty.const_int(0x4000_0000, false);
        let big = self.b.build_int_compare(IntPredicate::UGT, len_v, max, "").map_err(llvm_err)?;
        let chunk = self.b.build_select(big, max, len_v, "chunk").map_err(llvm_err)?.into_int_value();
        let written = if self.windows {
            let c = self.b.build_int_truncate(chunk, i32_ty, "").map_err(llvm_err)?;
            let r = self.call(write, &[fd.into(), ptr_v.into(), c.into()])?.expect("_write returns an int");
            self.b.build_int_s_extend(r.into_int_value(), usize_ty, "").map_err(llvm_err)?
        } else {
            self.call(write, &[fd.into(), ptr_v.into(), chunk.into()])?.expect("write returns a count").into_int_value()
        };
        let bad = self.b.build_int_compare(IntPredicate::SLE, written, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(bad, failed, next).map_err(llvm_err)?;
        self.b.position_at_end(next);
        let ptr2 = self.offset(ptr_v, written)?;
        let len2 = self.b.build_int_sub(len_v, written, "").map_err(llvm_err)?;
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;
        ptr.add_incoming(&[(&ptr0, entry), (&ptr2, next)]);
        len.add_incoming(&[(&len0, entry), (&len2, next)]);
        self.b.position_at_end(done);
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;
        self.b.position_at_end(failed);
        self.b.build_return(Some(&i32_ty.const_all_ones())).map_err(llvm_err)?;
        Ok(f)
    }

    /// The CRT read for a pipe: `(fd, buffer, count)`, returning the signed byte count.
    fn pipe_read_fn(&mut self) -> FunctionValue<'ctx> {
        if self.windows {
            libc(
                self.module,
                "_read",
                self.i32().fn_type(&[self.i32().into(), self.ptr_ty.into(), self.i32().into()], false),
            )
        } else {
            libc(
                self.module,
                "read",
                self.usize_ty.fn_type(&[self.i32().into(), self.ptr_ty.into(), self.usize_ty.into()], false),
            )
        }
    }

    /// `(fd, buffer, count)`, returning the count as a signed `usize` (Windows `_read` yields an int).
    fn pipe_read_call(
        &mut self,
        read: FunctionValue<'ctx>,
        fd: IntValue<'ctx>,
        buf: PointerValue<'ctx>,
        count: u64,
    ) -> GenResult<IntValue<'ctx>> {
        let (i32_ty, usize_ty) = (self.i32(), self.usize_ty);
        if self.windows {
            let n = self
                .call(read, &[fd.into(), buf.into(), i32_ty.const_int(count, false).into()])?
                .expect("_read returns an int");
            self.b.build_int_s_extend(n.into_int_value(), usize_ty, "").map_err(llvm_err)
        } else {
            Ok(self
                .call(read, &[fd.into(), buf.into(), usize_ty.const_int(count, false).into()])?
                .expect("read returns a count")
                .into_int_value())
        }
    }

    /// `pipe_read_to_string(handle: usize, out: *String) -> i32`: reads every byte left in the pipe.
    fn pipe_read_to_string(&mut self, push: FunctionValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let f =
            self.function("burdin_rt_pipe_read_to_string", i32_ty.fn_type(&[usize_ty.into(), ptr_ty.into()], false));
        let out = Self::ptr_param(f, 1);
        let fd = self.b.build_int_truncate(Self::int_param(f, 0), i32_ty, "fd").map_err(llvm_err)?;
        let read = self.pipe_read_fn();
        let buf = self.buffer(4096)?;
        let lp = self.block(f, "read.loop");
        let got = self.block(f, "read.got");
        let done = self.block(f, "read.done");
        let failed = self.block(f, "read.failed");
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;
        self.b.position_at_end(lp);
        let n = self.pipe_read_call(read, fd, buf, 4096)?;
        let positive = self.b.build_int_compare(IntPredicate::SGT, n, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let zero = self.b.build_int_compare(IntPredicate::EQ, n, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(positive, got, done).map_err(llvm_err)?;
        self.b.position_at_end(got);
        self.call(push, &[out.into(), buf.into(), n.into()])?;
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;
        self.b.position_at_end(done);
        let ended = self.block(f, "read.check.end");
        self.b.build_conditional_branch(zero, ended, failed).map_err(llvm_err)?;
        self.b.position_at_end(ended);
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;
        self.b.position_at_end(failed);
        self.b.build_return(Some(&i32_ty.const_all_ones())).map_err(llvm_err)?;
        Ok(f)
    }

    /// `pipe_read_line(handle: usize, out: *String) -> i32`: one line without its terminator.
    fn pipe_read_line(&mut self, push: FunctionValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty, i8_ty) = (self.usize_ty, self.ptr_ty, self.i32(), self.i8());
        let f = self.function("burdin_rt_pipe_read_line", i32_ty.fn_type(&[usize_ty.into(), ptr_ty.into()], false));
        let out = Self::ptr_param(f, 1);
        let fd = self.b.build_int_truncate(Self::int_param(f, 0), i32_ty, "fd").map_err(llvm_err)?;
        let read = self.pipe_read_fn();
        let byte = self.b.build_alloca(i8_ty, "line.byte").map_err(llvm_err)?;
        let lp = self.block(f, "line.loop");
        let got = self.block(f, "line.got");
        let newline = self.block(f, "line.newline");
        let strip = self.block(f, "line.strip");
        let stripped = self.block(f, "line.stripped");
        let line_done = self.block(f, "line.done");
        let eof = self.block(f, "line.eof");
        let eof_empty = self.block(f, "line.eof.empty");
        let eof_line = self.block(f, "line.eof.line");
        let failed = self.block(f, "line.failed");
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;
        self.b.position_at_end(lp);
        let n = self.pipe_read_call(read, fd, byte, 1)?;
        let negative = self.b.build_int_compare(IntPredicate::SLT, n, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let empty = self.b.build_int_compare(IntPredicate::EQ, n, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(negative, failed, got).map_err(llvm_err)?;
        self.b.position_at_end(got);
        self.b.build_conditional_branch(empty, eof, newline).map_err(llvm_err)?;
        self.b.position_at_end(newline);
        let c = self.b.build_load(i8_ty, byte, "byte").map_err(llvm_err)?.into_int_value();
        let nl = self.b.build_int_compare(IntPredicate::EQ, c, i8_ty.const_int(10, false), "").map_err(llvm_err)?;
        let push_bb = self.block(f, "line.push");
        self.b.build_conditional_branch(nl, strip, push_bb).map_err(llvm_err)?;
        self.b.position_at_end(push_bb);
        self.call(push, &[out.into(), byte.into(), usize_ty.const_int(1, false).into()])?;
        self.b.build_unconditional_branch(lp).map_err(llvm_err)?;
        // Drop the `\r` of a `\r\n` terminator.
        self.b.position_at_end(strip);
        let len_field = self.b.build_struct_gep(self.string_ty, out, 1, "line.len").map_err(llvm_err)?;
        let len = self.b.build_load(usize_ty, len_field, "").map_err(llvm_err)?.into_int_value();
        let has = self.b.build_int_compare(IntPredicate::UGT, len, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(has, stripped, line_done).map_err(llvm_err)?;
        self.b.position_at_end(stripped);
        let ptr_field = self.b.build_struct_gep(self.string_ty, out, 0, "line.ptr").map_err(llvm_err)?;
        let data = self.b.build_load(ptr_ty, ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        let last = self.b.build_int_sub(len, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        let at = self.offset(data, last)?;
        let last_byte = self.b.build_load(i8_ty, at, "").map_err(llvm_err)?.into_int_value();
        let cr =
            self.b.build_int_compare(IntPredicate::EQ, last_byte, i8_ty.const_int(13, false), "").map_err(llvm_err)?;
        let trimmed = self.block(f, "line.trimmed");
        self.b.build_conditional_branch(cr, trimmed, line_done).map_err(llvm_err)?;
        self.b.position_at_end(trimmed);
        self.b.build_store(len_field, last).map_err(llvm_err)?;
        self.b.build_unconditional_branch(line_done).map_err(llvm_err)?;
        self.b.position_at_end(line_done);
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;
        self.b.position_at_end(eof);
        let len_field = self.b.build_struct_gep(self.string_ty, out, 1, "line.len").map_err(llvm_err)?;
        let len = self.b.build_load(usize_ty, len_field, "").map_err(llvm_err)?.into_int_value();
        let has = self.b.build_int_compare(IntPredicate::UGT, len, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(has, eof_line, eof_empty).map_err(llvm_err)?;
        self.b.position_at_end(eof_line);
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;
        self.b.position_at_end(eof_empty);
        self.b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;
        self.b.position_at_end(failed);
        self.b.build_return(Some(&i32_ty.const_all_ones())).map_err(llvm_err)?;
        Ok(f)
    }

    /// `pipe_read_both(first: usize, second: usize, out: *[String; 2]) -> i32`: reads two pipes together, so a
    /// child writing a lot to one of them cannot block the other.
    fn pipe_read_both(&mut self, push: FunctionValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty, i16_ty) = (self.usize_ty, self.ptr_ty, self.i32(), self.cx.i16_type());
        let f = self.function(
            "burdin_rt_pipe_read_both",
            i32_ty.fn_type(&[usize_ty.into(), usize_ty.into(), ptr_ty.into()], false),
        );
        let first_handle = Self::int_param(f, 0);
        let second_handle = Self::int_param(f, 1);
        let out = Self::ptr_param(f, 2);
        let out0 = unsafe { self.b.build_in_bounds_gep(self.string_ty, out, &[usize_ty.const_zero()], "out0") }
            .map_err(llvm_err)?;
        let out1 = unsafe { self.b.build_in_bounds_gep(self.string_ty, out, &[usize_ty.const_int(1, false)], "out1") }
            .map_err(llvm_err)?;
        let buf = self.buffer(4096)?;
        let open0 = self.b.build_alloca(self.i8(), "open0").map_err(llvm_err)?;
        let open1 = self.b.build_alloca(self.i8(), "open1").map_err(llvm_err)?;
        let progress = self.b.build_alloca(self.i8(), "progress").map_err(llvm_err)?;
        self.b.build_store(open0, self.i8().const_int(1, false)).map_err(llvm_err)?;
        self.b.build_store(open1, self.i8().const_int(1, false)).map_err(llvm_err)?;
        let lp = self.block(f, "both.loop");
        let done = self.block(f, "both.done");
        let failed = self.block(f, "both.failed");
        let first_check = self.block(f, "both.first.check");
        let first_eof = self.block(f, "both.first.eof");
        let first_push = self.block(f, "both.first.push");
        let second_check = self.block(f, "both.second.check");
        let second_eof = self.block(f, "both.second.eof");
        let second_push = self.block(f, "both.second.push");

        if self.windows {
            let i64_ty = self.i64();
            let get = libc(self.module, "_get_osfhandle", i64_ty.fn_type(&[i32_ty.into()], false));
            let peek = libc(
                self.module,
                "PeekNamedPipe",
                i32_ty.fn_type(
                    &[i64_ty.into(), ptr_ty.into(), i32_ty.into(), ptr_ty.into(), ptr_ty.into(), ptr_ty.into()],
                    false,
                ),
            );
            let sleep = libc(self.module, "Sleep", self.cx.void_type().fn_type(&[i32_ty.into()], false));
            let fd0 = self.b.build_int_truncate(first_handle, i32_ty, "fd0").map_err(llvm_err)?;
            let fd1 = self.b.build_int_truncate(second_handle, i32_ty, "fd1").map_err(llvm_err)?;
            let h0 = self.call(get, &[fd0.into()])?.expect("_get_osfhandle returns a handle").into_int_value();
            let h1 = self.call(get, &[fd1.into()])?.expect("_get_osfhandle returns a handle").into_int_value();
            let read = self.pipe_read_fn();
            let avail = self.b.build_alloca(i32_ty, "avail").map_err(llvm_err)?;
            let first_take = self.block(f, "both.first.take");
            let second_take = self.block(f, "both.second.take");
            let after = self.block(f, "both.after");
            let slept = self.block(f, "both.sleep");

            self.b.build_unconditional_branch(lp).map_err(llvm_err)?;
            self.b.position_at_end(lp);
            let a = self.load_flag(open0)?;
            let b = self.load_flag(open1)?;
            let any = self.b.build_or(a, b, "any.open").map_err(llvm_err)?;
            let step = self.block(f, "both.step");
            self.b.build_conditional_branch(any, step, done).map_err(llvm_err)?;

            self.b.position_at_end(step);
            self.b.build_store(progress, self.i8().const_zero()).map_err(llvm_err)?;
            self.b.build_unconditional_branch(first_check).map_err(llvm_err)?;

            self.b.position_at_end(first_check);
            let live = self.load_flag(open0)?;
            let first_peek = self.block(f, "both.first.peek");
            self.b.build_conditional_branch(live, first_peek, second_check).map_err(llvm_err)?;

            self.b.position_at_end(first_peek);
            self.b.build_store(avail, i32_ty.const_zero()).map_err(llvm_err)?;
            let ok = self
                .call(
                    peek,
                    &[
                        h0.into(),
                        ptr_ty.const_null().into(),
                        i32_ty.const_zero().into(),
                        ptr_ty.const_null().into(),
                        avail.into(),
                        ptr_ty.const_null().into(),
                    ],
                )?
                .expect("PeekNamedPipe returns a flag")
                .into_int_value();
            let have = self.b.build_int_compare(IntPredicate::NE, ok, i32_ty.const_zero(), "").map_err(llvm_err)?;
            let first_have = self.block(f, "both.first.have");
            self.b.build_conditional_branch(have, first_have, first_eof).map_err(llvm_err)?;

            self.b.position_at_end(first_have);
            let n = self.b.build_load(i32_ty, avail, "").map_err(llvm_err)?.into_int_value();
            let has = self.b.build_int_compare(IntPredicate::SGT, n, i32_ty.const_zero(), "").map_err(llvm_err)?;
            self.b.build_conditional_branch(has, first_take, second_check).map_err(llvm_err)?;

            self.b.position_at_end(first_take);
            let cap = self
                .b
                .build_select(
                    self.b
                        .build_int_compare(IntPredicate::UGT, n, i32_ty.const_int(4096, false), "")
                        .map_err(llvm_err)?,
                    i32_ty.const_int(4096, false),
                    n,
                    "take",
                )
                .map_err(llvm_err)?
                .into_int_value();
            let got = self
                .call(read, &[fd0.into(), buf.into(), cap.into()])?
                .expect("_read returns a count")
                .into_int_value();
            let positive =
                self.b.build_int_compare(IntPredicate::SGT, got, i32_ty.const_zero(), "").map_err(llvm_err)?;
            let zero = self.b.build_int_compare(IntPredicate::EQ, got, i32_ty.const_zero(), "").map_err(llvm_err)?;
            let read_ok = self.block(f, "both.first.read");
            let read_zero = self.block(f, "both.first.zero");
            self.b.build_conditional_branch(positive, read_ok, read_zero).map_err(llvm_err)?;
            self.b.position_at_end(read_zero);
            self.b.build_conditional_branch(zero, first_eof, failed).map_err(llvm_err)?;
            self.b.position_at_end(read_ok);
            let count = self.b.build_int_z_extend(got, usize_ty, "count").map_err(llvm_err)?;
            self.call(push, &[out0.into(), buf.into(), count.into()])?;
            self.b.build_store(progress, self.i8().const_int(1, false)).map_err(llvm_err)?;
            self.b.build_unconditional_branch(first_push).map_err(llvm_err)?;

            self.b.position_at_end(first_push);
            self.b.build_unconditional_branch(second_check).map_err(llvm_err)?;
            self.b.position_at_end(first_eof);
            self.b.build_store(open0, self.i8().const_zero()).map_err(llvm_err)?;
            self.b.build_unconditional_branch(second_check).map_err(llvm_err)?;

            self.b.position_at_end(second_check);
            let live = self.load_flag(open1)?;
            let second_peek = self.block(f, "both.second.peek");
            self.b.build_conditional_branch(live, second_peek, after).map_err(llvm_err)?;

            self.b.position_at_end(second_peek);
            self.b.build_store(avail, i32_ty.const_zero()).map_err(llvm_err)?;
            let ok = self
                .call(
                    peek,
                    &[
                        h1.into(),
                        ptr_ty.const_null().into(),
                        i32_ty.const_zero().into(),
                        ptr_ty.const_null().into(),
                        avail.into(),
                        ptr_ty.const_null().into(),
                    ],
                )?
                .expect("PeekNamedPipe returns a flag")
                .into_int_value();
            let have = self.b.build_int_compare(IntPredicate::NE, ok, i32_ty.const_zero(), "").map_err(llvm_err)?;
            let second_have = self.block(f, "both.second.have");
            self.b.build_conditional_branch(have, second_have, second_eof).map_err(llvm_err)?;

            self.b.position_at_end(second_have);
            let n = self.b.build_load(i32_ty, avail, "").map_err(llvm_err)?.into_int_value();
            let has = self.b.build_int_compare(IntPredicate::SGT, n, i32_ty.const_zero(), "").map_err(llvm_err)?;
            self.b.build_conditional_branch(has, second_take, after).map_err(llvm_err)?;

            self.b.position_at_end(second_take);
            let cap = self
                .b
                .build_select(
                    self.b
                        .build_int_compare(IntPredicate::UGT, n, i32_ty.const_int(4096, false), "")
                        .map_err(llvm_err)?,
                    i32_ty.const_int(4096, false),
                    n,
                    "take",
                )
                .map_err(llvm_err)?
                .into_int_value();
            let got = self
                .call(read, &[fd1.into(), buf.into(), cap.into()])?
                .expect("_read returns a count")
                .into_int_value();
            let positive =
                self.b.build_int_compare(IntPredicate::SGT, got, i32_ty.const_zero(), "").map_err(llvm_err)?;
            let zero = self.b.build_int_compare(IntPredicate::EQ, got, i32_ty.const_zero(), "").map_err(llvm_err)?;
            let read_ok = self.block(f, "both.second.read");
            let read_zero = self.block(f, "both.second.zero");
            self.b.build_conditional_branch(positive, read_ok, read_zero).map_err(llvm_err)?;
            self.b.position_at_end(read_zero);
            self.b.build_conditional_branch(zero, second_eof, failed).map_err(llvm_err)?;
            self.b.position_at_end(read_ok);
            let count = self.b.build_int_z_extend(got, usize_ty, "count").map_err(llvm_err)?;
            self.call(push, &[out1.into(), buf.into(), count.into()])?;
            self.b.build_store(progress, self.i8().const_int(1, false)).map_err(llvm_err)?;
            self.b.build_unconditional_branch(second_push).map_err(llvm_err)?;

            self.b.position_at_end(second_push);
            self.b.build_unconditional_branch(after).map_err(llvm_err)?;
            self.b.position_at_end(second_eof);
            self.b.build_store(open1, self.i8().const_zero()).map_err(llvm_err)?;
            self.b.build_unconditional_branch(after).map_err(llvm_err)?;

            self.b.position_at_end(after);
            let moved = self.load_flag(progress)?;
            let idle = self.b.build_not(moved, "").map_err(llvm_err)?;
            self.b.build_conditional_branch(idle, slept, lp).map_err(llvm_err)?;
            self.b.position_at_end(slept);
            self.call(sleep, &[i32_ty.const_int(1, false).into()])?;
            self.b.build_unconditional_branch(lp).map_err(llvm_err)?;
        } else {
            let pollfd_ty = self.cx.struct_type(&[i32_ty.into(), i16_ty.into(), i16_ty.into()], false);
            let array_ty = pollfd_ty.array_type(2);
            let pollfds = self.b.build_alloca(array_ty, "pollfds").map_err(llvm_err)?;
            let entry = |cg: &mut Self, i: u64| -> GenResult<PointerValue<'ctx>> {
                unsafe {
                    cg.b.build_in_bounds_gep(
                        array_ty,
                        pollfds,
                        &[usize_ty.const_zero(), usize_ty.const_int(i, false)],
                        "",
                    )
                }
                .map_err(llvm_err)
            };
            for (i, handle) in [first_handle, second_handle].into_iter().enumerate() {
                let p = entry(self, i as u64)?;
                let fd_field = self.b.build_struct_gep(pollfd_ty, p, 0, "").map_err(llvm_err)?;
                let events = self.b.build_struct_gep(pollfd_ty, p, 1, "").map_err(llvm_err)?;
                let revents = self.b.build_struct_gep(pollfd_ty, p, 2, "").map_err(llvm_err)?;
                let fd = self.b.build_int_truncate(handle, i32_ty, "fd").map_err(llvm_err)?;
                self.b.build_store(fd_field, fd).map_err(llvm_err)?;
                self.b.build_store(events, i16_ty.const_int(1, false)).map_err(llvm_err)?;
                self.b.build_store(revents, i16_ty.const_zero()).map_err(llvm_err)?;
            }
            let poll =
                libc(self.module, "poll", i32_ty.fn_type(&[ptr_ty.into(), usize_ty.into(), i32_ty.into()], false));
            let read = self.pipe_read_fn();
            let fd0 = self.b.build_int_truncate(first_handle, i32_ty, "fd0").map_err(llvm_err)?;
            let fd1 = self.b.build_int_truncate(second_handle, i32_ty, "fd1").map_err(llvm_err)?;
            let ready0 = self.b.build_alloca(i32_ty, "ready0").map_err(llvm_err)?;
            let ready1 = self.b.build_alloca(i32_ty, "ready1").map_err(llvm_err)?;
            let first_read = self.block(f, "both.first.read");
            let second_read = self.block(f, "both.second.read");

            self.b.build_unconditional_branch(lp).map_err(llvm_err)?;
            self.b.position_at_end(lp);
            let a = self.load_flag(open0)?;
            let b = self.load_flag(open1)?;
            let any = self.b.build_or(a, b, "any.open").map_err(llvm_err)?;
            let step = self.block(f, "both.step");
            self.b.build_conditional_branch(any, step, done).map_err(llvm_err)?;

            self.b.position_at_end(step);
            let r = self
                .call(poll, &[pollfds.into(), usize_ty.const_int(2, false).into(), i32_ty.const_all_ones().into()])?
                .expect("poll returns a count")
                .into_int_value();
            let bad = self.b.build_int_compare(IntPredicate::SLT, r, i32_ty.const_zero(), "").map_err(llvm_err)?;
            let polled = self.block(f, "both.polled");
            self.b.build_conditional_branch(bad, failed, polled).map_err(llvm_err)?;

            self.b.position_at_end(polled);
            let first = entry(self, 0)?;
            let first_revents = self.b.build_struct_gep(pollfd_ty, first, 2, "").map_err(llvm_err)?;
            let revents = self.b.build_load(i16_ty, first_revents, "").map_err(llvm_err)?.into_int_value();
            let revents = self.b.build_int_z_extend(revents, i32_ty, "").map_err(llvm_err)?;
            self.b.build_store(ready0, revents).map_err(llvm_err)?;
            let second = entry(self, 1)?;
            let second_revents = self.b.build_struct_gep(pollfd_ty, second, 2, "").map_err(llvm_err)?;
            let revents = self.b.build_load(i16_ty, second_revents, "").map_err(llvm_err)?.into_int_value();
            let revents = self.b.build_int_z_extend(revents, i32_ty, "").map_err(llvm_err)?;
            self.b.build_store(ready1, revents).map_err(llvm_err)?;
            self.b.build_unconditional_branch(first_check).map_err(llvm_err)?;

            self.b.position_at_end(first_check);
            let live = self.load_flag(open0)?;
            let revents = self.b.build_load(i32_ty, ready0, "").map_err(llvm_err)?.into_int_value();
            let ready =
                self.b.build_int_compare(IntPredicate::NE, revents, i32_ty.const_zero(), "").map_err(llvm_err)?;
            let go = self.b.build_and(live, ready, "").map_err(llvm_err)?;
            self.b.build_conditional_branch(go, first_read, second_check).map_err(llvm_err)?;

            self.b.position_at_end(first_read);
            let got = self.pipe_read_call(read, fd0, buf, 4096)?;
            let positive =
                self.b.build_int_compare(IntPredicate::SGT, got, usize_ty.const_zero(), "").map_err(llvm_err)?;
            let zero = self.b.build_int_compare(IntPredicate::EQ, got, usize_ty.const_zero(), "").map_err(llvm_err)?;
            let read_ok = self.block(f, "both.first.got");
            let read_zero = self.block(f, "both.first.zero");
            self.b.build_conditional_branch(positive, read_ok, read_zero).map_err(llvm_err)?;
            self.b.position_at_end(read_zero);
            self.b.build_conditional_branch(zero, first_eof, failed).map_err(llvm_err)?;
            self.b.position_at_end(read_ok);
            self.call(push, &[out0.into(), buf.into(), got.into()])?;
            self.b.build_unconditional_branch(first_push).map_err(llvm_err)?;

            self.b.position_at_end(first_push);
            self.b.build_unconditional_branch(second_check).map_err(llvm_err)?;
            self.b.position_at_end(first_eof);
            self.b.build_store(open0, self.i8().const_zero()).map_err(llvm_err)?;
            let first = entry(self, 0)?;
            let fd_field = self.b.build_struct_gep(pollfd_ty, first, 0, "").map_err(llvm_err)?;
            self.b.build_store(fd_field, i32_ty.const_all_ones()).map_err(llvm_err)?;
            self.b.build_unconditional_branch(second_check).map_err(llvm_err)?;

            self.b.position_at_end(second_check);
            let live = self.load_flag(open1)?;
            let revents = self.b.build_load(i32_ty, ready1, "").map_err(llvm_err)?.into_int_value();
            let ready =
                self.b.build_int_compare(IntPredicate::NE, revents, i32_ty.const_zero(), "").map_err(llvm_err)?;
            let go = self.b.build_and(live, ready, "").map_err(llvm_err)?;
            self.b.build_conditional_branch(go, second_read, lp).map_err(llvm_err)?;

            self.b.position_at_end(second_read);
            let got = self.pipe_read_call(read, fd1, buf, 4096)?;
            let positive =
                self.b.build_int_compare(IntPredicate::SGT, got, usize_ty.const_zero(), "").map_err(llvm_err)?;
            let zero = self.b.build_int_compare(IntPredicate::EQ, got, usize_ty.const_zero(), "").map_err(llvm_err)?;
            let read_ok = self.block(f, "both.second.got");
            let read_zero = self.block(f, "both.second.zero");
            self.b.build_conditional_branch(positive, read_ok, read_zero).map_err(llvm_err)?;
            self.b.position_at_end(read_zero);
            self.b.build_conditional_branch(zero, second_eof, failed).map_err(llvm_err)?;
            self.b.position_at_end(read_ok);
            self.call(push, &[out1.into(), buf.into(), got.into()])?;
            self.b.build_unconditional_branch(second_push).map_err(llvm_err)?;

            self.b.position_at_end(second_push);
            self.b.build_unconditional_branch(lp).map_err(llvm_err)?;
            self.b.position_at_end(second_eof);
            self.b.build_store(open1, self.i8().const_zero()).map_err(llvm_err)?;
            let second = entry(self, 1)?;
            let fd_field = self.b.build_struct_gep(pollfd_ty, second, 0, "").map_err(llvm_err)?;
            self.b.build_store(fd_field, i32_ty.const_all_ones()).map_err(llvm_err)?;
            self.b.build_unconditional_branch(lp).map_err(llvm_err)?;
        }

        self.b.position_at_end(done);
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;
        self.b.position_at_end(failed);
        self.b.build_return(Some(&i32_ty.const_all_ones())).map_err(llvm_err)?;
        Ok(f)
    }

    /// Whether `ptr[0..len]` contains `byte` (libc `memchr`).
    fn has_byte(&mut self, ptr: PointerValue<'ctx>, len: IntValue<'ctx>, byte: u8) -> GenResult<IntValue<'ctx>> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let memchr =
            libc(self.module, "memchr", ptr_ty.fn_type(&[ptr_ty.into(), i32_ty.into(), usize_ty.into()], false));
        let found = self
            .call(memchr, &[ptr.into(), i32_ty.const_int(u64::from(byte), false).into(), len.into()])?
            .expect("memchr returns a pointer")
            .into_pointer_value();
        let null = self.b.build_is_null(found, "").map_err(llvm_err)?;
        self.b.build_not(null, "").map_err(llvm_err)
    }

    /// Stores the `{ ptr, len, cap }` words of a byte list at `out`.
    fn store_byte_list(
        &mut self,
        out: PointerValue<'ctx>,
        ptr: PointerValue<'ctx>,
        len: IntValue<'ctx>,
    ) -> GenResult<()> {
        let f0 = self.b.build_struct_gep(self.string_ty, out, 0, "").map_err(llvm_err)?;
        let f1 = self.b.build_struct_gep(self.string_ty, out, 1, "").map_err(llvm_err)?;
        let f2 = self.b.build_struct_gep(self.string_ty, out, 2, "").map_err(llvm_err)?;
        self.b.build_store(f0, ptr).map_err(llvm_err)?;
        self.b.build_store(f1, len).map_err(llvm_err)?;
        self.b.build_store(f2, len).map_err(llvm_err)?;
        Ok(())
    }

    /// Copies `len` bytes from `src` into a fresh Burdin allocation and stores it at `out`.
    fn copy_to_byte_list(
        &mut self,
        out: PointerValue<'ctx>,
        src: PointerValue<'ctx>,
        len: IntValue<'ctx>,
    ) -> GenResult<()> {
        let buf = self.call_rt_alloc(len)?;
        self.b.build_memcpy(buf, 1, src, 1, len).map_err(llvm_err)?;
        self.store_byte_list(out, buf, len)
    }

    /// Stores an empty list at `out` and returns zero from the current function.
    fn return_missing(&mut self, out: PointerValue<'ctx>) -> GenResult<()> {
        let zero = self.string_ty.const_zero();
        self.b.build_store(out, zero).map_err(llvm_err)?;
        self.b.build_return(Some(&self.i32().const_zero())).map_err(llvm_err)?;
        Ok(())
    }

    /// `env_get(name, name_len, out: *List<u8>) -> i32`: copies the value's bytes into `out`; 1 when the variable
    /// exists. A name with an embedded NUL can never name an entry.
    fn env_get(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let f =
            self.function("burdin_rt_env_get", i32_ty.fn_type(&[ptr_ty.into(), usize_ty.into(), ptr_ty.into()], false));
        let (name, name_len, out) = (Self::ptr_param(f, 0), Self::int_param(f, 1), Self::ptr_param(f, 2));
        let nul = self.has_byte(name, name_len, 0)?;
        let found = self.block(f, "env.get");
        let missing = self.block(f, "env.missing");
        self.b.build_conditional_branch(nul, missing, found).map_err(llvm_err)?;
        self.b.position_at_end(missing);
        self.return_missing(out)?;
        self.b.position_at_end(found);
        if self.windows {
            self.env_get_windows(f, name, name_len, out)?;
        } else {
            self.env_get_unix(f, name, name_len, out)?;
        }
        Ok(f)
    }

    fn env_get_unix(
        &mut self,
        f: FunctionValue<'ctx>,
        name: PointerValue<'ctx>,
        name_len: IntValue<'ctx>,
        out: PointerValue<'ctx>,
    ) -> GenResult<()> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let getenv = libc(self.module, "getenv", ptr_ty.fn_type(&[ptr_ty.into()], false));
        let strlen = libc(self.module, "strlen", usize_ty.fn_type(&[ptr_ty.into()], false));
        let cname = self.cstring_of(name, name_len, malloc)?;
        let value = self.call(getenv, &[cname.into()])?.expect("getenv returns a pointer").into_pointer_value();
        self.call(free, &[cname.into()])?;
        let null = self.b.build_is_null(value, "").map_err(llvm_err)?;
        let present = self.block(f, "env.present");
        let missing = self.block(f, "env.unset");
        self.b.build_conditional_branch(null, missing, present).map_err(llvm_err)?;
        self.b.position_at_end(missing);
        self.return_missing(out)?;
        self.b.position_at_end(present);
        let len = self.call(strlen, &[value.into()])?.expect("strlen returns a size").into_int_value();
        self.copy_to_byte_list(out, value, len)?;
        self.b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;
        Ok(())
    }

    fn env_get_windows(
        &mut self,
        f: FunctionValue<'ctx>,
        name: PointerValue<'ctx>,
        name_len: IntValue<'ctx>,
        out: PointerValue<'ctx>,
    ) -> GenResult<()> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let get = libc(
            self.module,
            "GetEnvironmentVariableW",
            i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into(), i32_ty.into()], false),
        );
        let units = self.b.build_alloca(usize_ty, "env.name.units").map_err(llvm_err)?;
        let wide_name = self.utf8_to_wide(name, name_len, units)?;
        let wide_null = self.b.build_is_null(wide_name, "").map_err(llvm_err)?;
        let lookup = self.block(f, "env.lookup");
        let fail = self.block(f, "env.fail");
        self.b.build_conditional_branch(wide_null, fail, lookup).map_err(llvm_err)?;
        self.b.position_at_end(lookup);
        let needed = self
            .call(get, &[wide_name.into(), ptr_ty.const_null().into(), i32_ty.const_zero().into()])?
            .expect("GetEnvironmentVariableW returns a size")
            .into_int_value();
        let none = self.b.build_int_compare(IntPredicate::EQ, needed, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let alloc = self.block(f, "env.alloc");
        let free_name = self.block(f, "env.free.name");
        self.b.build_conditional_branch(none, free_name, alloc).map_err(llvm_err)?;
        self.b.position_at_end(free_name);
        self.call(free, &[wide_name.into()])?;
        self.b.build_unconditional_branch(fail).map_err(llvm_err)?;
        self.b.position_at_end(alloc);
        let units_needed = self.b.build_int_z_extend(needed, usize_ty, "").map_err(llvm_err)?;
        let size = self.b.build_int_mul(units_needed, usize_ty.const_int(2, false), "").map_err(llvm_err)?;
        let wbuf = self.call(malloc, &[size.into()])?.expect("malloc returns a pointer").into_pointer_value();
        let buf_null = self.b.build_is_null(wbuf, "").map_err(llvm_err)?;
        let second = self.block(f, "env.second");
        self.b.build_conditional_branch(buf_null, free_name, second).map_err(llvm_err)?;
        self.b.position_at_end(second);
        let n = self
            .call(get, &[wide_name.into(), wbuf.into(), needed.into()])?
            .expect("GetEnvironmentVariableW returns a length")
            .into_int_value();
        self.call(free, &[wide_name.into()])?;
        let empty = self.b.build_int_compare(IntPredicate::EQ, n, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let check_empty = self.block(f, "env.check.empty");
        let convert = self.block(f, "env.convert");
        self.b.build_conditional_branch(empty, check_empty, convert).map_err(llvm_err)?;
        self.b.position_at_end(check_empty);
        // A value of length zero is an empty variable when the first call asked for one WCHAR (the terminator)
        // and a variable that disappeared otherwise.
        let vanished =
            self.b.build_int_compare(IntPredicate::UGT, needed, i32_ty.const_int(1, false), "").map_err(llvm_err)?;
        let empty_ok = self.block(f, "env.empty");
        let free_buf = self.block(f, "env.free.buf");
        self.b.build_conditional_branch(vanished, free_buf, empty_ok).map_err(llvm_err)?;
        self.b.position_at_end(free_buf);
        self.call(free, &[wbuf.into()])?;
        self.b.build_unconditional_branch(fail).map_err(llvm_err)?;
        self.b.position_at_end(empty_ok);
        self.call(free, &[wbuf.into()])?;
        let zero = self.string_ty.const_zero();
        self.b.build_store(out, zero).map_err(llvm_err)?;
        self.b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;
        self.b.position_at_end(convert);
        let bytes = self.b.build_alloca(usize_ty, "env.bytes").map_err(llvm_err)?;
        let data = self.wide_to_utf8(wbuf, n, bytes)?;
        self.call(free, &[wbuf.into()])?;
        let data_null = self.b.build_is_null(data, "").map_err(llvm_err)?;
        let copy = self.block(f, "env.copy");
        self.b.build_conditional_branch(data_null, fail, copy).map_err(llvm_err)?;
        self.b.position_at_end(copy);
        let len = self.b.build_load(usize_ty, bytes, "env.len").map_err(llvm_err)?.into_int_value();
        self.copy_to_byte_list(out, data, len)?;
        self.call(free, &[data.into()])?;
        self.b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;
        self.b.position_at_end(fail);
        self.return_missing(out)?;
        Ok(())
    }

    /// `env_set(name, name_len, value, value_len) -> i32`: 0 on success, 1 for an invalid name, 2 for an invalid
    /// value (an embedded NUL, or on Windows bytes that are not UTF-8), and 3 when the system refuses.
    fn env_set(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let f = self.function(
            "burdin_rt_env_set",
            i32_ty.fn_type(&[ptr_ty.into(), usize_ty.into(), ptr_ty.into(), usize_ty.into()], false),
        );
        let (name, name_len, value, value_len) =
            (Self::ptr_param(f, 0), Self::int_param(f, 1), Self::ptr_param(f, 2), Self::int_param(f, 3));
        let empty =
            self.b.build_int_compare(IntPredicate::EQ, name_len, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let eq = self.has_byte(name, name_len, b'=')?;
        let nul = self.has_byte(name, name_len, 0)?;
        let bad = self.b.build_or(self.b.build_or(empty, eq, "").map_err(llvm_err)?, nul, "").map_err(llvm_err)?;
        let bad_name = self.block(f, "env.bad.name");
        let check_value = self.block(f, "env.check.value");
        self.b.build_conditional_branch(bad, bad_name, check_value).map_err(llvm_err)?;
        self.b.position_at_end(bad_name);
        self.b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;
        self.b.position_at_end(check_value);
        let vnul = self.has_byte(value, value_len, 0)?;
        let bad_value = self.block(f, "env.bad.value");
        let store = self.block(f, "env.store");
        self.b.build_conditional_branch(vnul, bad_value, store).map_err(llvm_err)?;
        self.b.position_at_end(bad_value);
        self.b.build_return(Some(&i32_ty.const_int(2, false))).map_err(llvm_err)?;
        self.b.position_at_end(store);
        if self.windows {
            self.env_set_windows(f, name, name_len, value, value_len)?;
        } else {
            self.env_set_unix(name, name_len, value, value_len)?;
        }
        Ok(f)
    }

    fn env_set_unix(
        &mut self,
        name: PointerValue<'ctx>,
        name_len: IntValue<'ctx>,
        value: PointerValue<'ctx>,
        value_len: IntValue<'ctx>,
    ) -> GenResult<()> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let setenv = libc(self.module, "setenv", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into(), i32_ty.into()], false));
        let cname = self.cstring_of(name, name_len, malloc)?;
        let cvalue = self.cstring_of(value, value_len, malloc)?;
        let rc = self
            .call(setenv, &[cname.into(), cvalue.into(), i32_ty.const_int(1, false).into()])?
            .expect("setenv returns an int")
            .into_int_value();
        self.call(free, &[cname.into()])?;
        self.call(free, &[cvalue.into()])?;
        let ok = self.b.build_int_compare(IntPredicate::EQ, rc, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let status = self.b.build_select(ok, i32_ty.const_zero(), i32_ty.const_int(3, false), "").map_err(llvm_err)?;
        self.b.build_return(Some(&status)).map_err(llvm_err)?;
        Ok(())
    }

    fn env_set_windows(
        &mut self,
        f: FunctionValue<'ctx>,
        name: PointerValue<'ctx>,
        name_len: IntValue<'ctx>,
        value: PointerValue<'ctx>,
        value_len: IntValue<'ctx>,
    ) -> GenResult<()> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let set = libc(self.module, "SetEnvironmentVariableW", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
        let units = self.b.build_alloca(usize_ty, "env.name.units").map_err(llvm_err)?;
        let wide_name = self.utf8_to_wide(name, name_len, units)?;
        let name_null = self.b.build_is_null(wide_name, "").map_err(llvm_err)?;
        let bad_name = self.block(f, "env.bad.name.utf8");
        let value_bb = self.block(f, "env.value.utf8");
        self.b.build_conditional_branch(name_null, bad_name, value_bb).map_err(llvm_err)?;
        self.b.position_at_end(bad_name);
        self.b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;
        self.b.position_at_end(value_bb);
        let vunits = self.b.build_alloca(usize_ty, "env.value.units").map_err(llvm_err)?;
        let wide_value = self.utf8_to_wide(value, value_len, vunits)?;
        let value_null = self.b.build_is_null(wide_value, "").map_err(llvm_err)?;
        let bad_value = self.block(f, "env.bad.value.utf8");
        let call_set = self.block(f, "env.call.set");
        self.b.build_conditional_branch(value_null, bad_value, call_set).map_err(llvm_err)?;
        self.b.position_at_end(bad_value);
        self.call(free, &[wide_name.into()])?;
        self.b.build_return(Some(&i32_ty.const_int(2, false))).map_err(llvm_err)?;
        self.b.position_at_end(call_set);
        let rc = self
            .call(set, &[wide_name.into(), wide_value.into()])?
            .expect("SetEnvironmentVariableW returns a flag")
            .into_int_value();
        self.call(free, &[wide_name.into()])?;
        self.call(free, &[wide_value.into()])?;
        let failed = self.b.build_int_compare(IntPredicate::EQ, rc, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let status =
            self.b.build_select(failed, i32_ty.const_int(3, false), i32_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_return(Some(&status)).map_err(llvm_err)?;
        Ok(())
    }

    /// `env_remove(name, name_len) -> i32`: 0 on success, 1 for an invalid name, and 3 when the system refuses.
    fn env_remove(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let f = self.function("burdin_rt_env_remove", i32_ty.fn_type(&[ptr_ty.into(), usize_ty.into()], false));
        let (name, name_len) = (Self::ptr_param(f, 0), Self::int_param(f, 1));
        let empty =
            self.b.build_int_compare(IntPredicate::EQ, name_len, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let eq = self.has_byte(name, name_len, b'=')?;
        let nul = self.has_byte(name, name_len, 0)?;
        let bad = self.b.build_or(self.b.build_or(empty, eq, "").map_err(llvm_err)?, nul, "").map_err(llvm_err)?;
        let bad_name = self.block(f, "env.remove.bad.name");
        let remove = self.block(f, "env.remove.do");
        self.b.build_conditional_branch(bad, bad_name, remove).map_err(llvm_err)?;
        self.b.position_at_end(bad_name);
        self.b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;
        self.b.position_at_end(remove);
        if self.windows {
            self.env_remove_windows(f, name, name_len)?;
        } else {
            self.env_remove_unix(name, name_len)?;
        }
        Ok(f)
    }

    fn env_remove_unix(&mut self, name: PointerValue<'ctx>, name_len: IntValue<'ctx>) -> GenResult<()> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let unsetenv = libc(self.module, "unsetenv", i32_ty.fn_type(&[ptr_ty.into()], false));
        let cname = self.cstring_of(name, name_len, malloc)?;
        let rc = self.call(unsetenv, &[cname.into()])?.expect("unsetenv returns an int").into_int_value();
        self.call(free, &[cname.into()])?;
        let ok = self.b.build_int_compare(IntPredicate::EQ, rc, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let status = self.b.build_select(ok, i32_ty.const_zero(), i32_ty.const_int(3, false), "").map_err(llvm_err)?;
        self.b.build_return(Some(&status)).map_err(llvm_err)?;
        Ok(())
    }

    fn env_remove_windows(
        &mut self,
        f: FunctionValue<'ctx>,
        name: PointerValue<'ctx>,
        name_len: IntValue<'ctx>,
    ) -> GenResult<()> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let set = libc(self.module, "SetEnvironmentVariableW", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
        let last_error = libc(self.module, "GetLastError", i32_ty.fn_type(&[], false));
        let units = self.b.build_alloca(usize_ty, "env.name.units").map_err(llvm_err)?;
        let wide_name = self.utf8_to_wide(name, name_len, units)?;
        let name_null = self.b.build_is_null(wide_name, "").map_err(llvm_err)?;
        let bad_name = self.block(f, "env.remove.bad.utf8");
        let call_set = self.block(f, "env.remove.set");
        self.b.build_conditional_branch(name_null, bad_name, call_set).map_err(llvm_err)?;
        self.b.position_at_end(bad_name);
        self.b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;
        self.b.position_at_end(call_set);
        let rc = self
            .call(set, &[wide_name.into(), ptr_ty.const_null().into()])?
            .expect("SetEnvironmentVariableW returns a flag")
            .into_int_value();
        let error = self.call(last_error, &[])?.expect("GetLastError returns a code").into_int_value();
        self.call(free, &[wide_name.into()])?;
        // Deleting a variable that is not there is a success, as it is on Unix.
        let failed = self.b.build_int_compare(IntPredicate::EQ, rc, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let absent =
            self.b.build_int_compare(IntPredicate::EQ, error, i32_ty.const_int(203, false), "").map_err(llvm_err)?;
        let ok = self.b.build_or(self.b.build_not(failed, "").map_err(llvm_err)?, absent, "").map_err(llvm_err)?;
        let status = self.b.build_select(ok, i32_ty.const_zero(), i32_ty.const_int(3, false), "").map_err(llvm_err)?;
        self.b.build_return(Some(&status)).map_err(llvm_err)?;
        Ok(())
    }

    /// `current_dir(out: *List<u8>) -> i32`: copies the working directory's bytes into `out`; 1 when read.
    fn current_dir(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (ptr_ty, i32_ty) = (self.ptr_ty, self.i32());
        let f = self.function("burdin_rt_current_dir", i32_ty.fn_type(&[ptr_ty.into()], false));
        let out = Self::ptr_param(f, 0);
        if self.windows {
            self.current_dir_windows(f, out)?;
        } else {
            self.current_dir_unix(f, out)?;
        }
        Ok(f)
    }

    fn current_dir_unix(&mut self, f: FunctionValue<'ctx>, out: PointerValue<'ctx>) -> GenResult<()> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let getcwd = libc(self.module, "getcwd", ptr_ty.fn_type(&[ptr_ty.into(), usize_ty.into()], false));
        let strlen = libc(self.module, "strlen", usize_ty.fn_type(&[ptr_ty.into()], false));
        let buf = self
            .call(getcwd, &[ptr_ty.const_null().into(), usize_ty.const_zero().into()])?
            .expect("getcwd returns a pointer")
            .into_pointer_value();
        let null = self.b.build_is_null(buf, "").map_err(llvm_err)?;
        let read = self.block(f, "cwd.read");
        let missing = self.block(f, "cwd.missing");
        self.b.build_conditional_branch(null, missing, read).map_err(llvm_err)?;
        self.b.position_at_end(missing);
        self.return_missing(out)?;
        self.b.position_at_end(read);
        let len = self.call(strlen, &[buf.into()])?.expect("strlen returns a size").into_int_value();
        self.copy_to_byte_list(out, buf, len)?;
        self.call(free, &[buf.into()])?;
        self.b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;
        Ok(())
    }

    fn current_dir_windows(&mut self, f: FunctionValue<'ctx>, out: PointerValue<'ctx>) -> GenResult<()> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let get = libc(self.module, "GetCurrentDirectoryW", i32_ty.fn_type(&[i32_ty.into(), ptr_ty.into()], false));
        let needed = self
            .call(get, &[i32_ty.const_zero().into(), ptr_ty.const_null().into()])?
            .expect("GetCurrentDirectoryW returns a size")
            .into_int_value();
        let none = self.b.build_int_compare(IntPredicate::EQ, needed, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let alloc = self.block(f, "cwd.alloc");
        let missing = self.block(f, "cwd.missing");
        self.b.build_conditional_branch(none, missing, alloc).map_err(llvm_err)?;
        self.b.position_at_end(alloc);
        let units = self.b.build_int_z_extend(needed, usize_ty, "").map_err(llvm_err)?;
        let size = self.b.build_int_mul(units, usize_ty.const_int(2, false), "").map_err(llvm_err)?;
        let wbuf = self.call(malloc, &[size.into()])?.expect("malloc returns a pointer").into_pointer_value();
        let buf_null = self.b.build_is_null(wbuf, "").map_err(llvm_err)?;
        let read = self.block(f, "cwd.read");
        self.b.build_conditional_branch(buf_null, missing, read).map_err(llvm_err)?;
        self.b.position_at_end(read);
        let n = self
            .call(get, &[needed.into(), wbuf.into()])?
            .expect("GetCurrentDirectoryW returns a length")
            .into_int_value();
        let failed = self.b.build_int_compare(IntPredicate::EQ, n, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let convert = self.block(f, "cwd.convert");
        let free_buf = self.block(f, "cwd.free");
        self.b.build_conditional_branch(failed, free_buf, convert).map_err(llvm_err)?;
        self.b.position_at_end(free_buf);
        self.call(free, &[wbuf.into()])?;
        self.b.build_unconditional_branch(missing).map_err(llvm_err)?;
        self.b.position_at_end(convert);
        let bytes = self.b.build_alloca(usize_ty, "cwd.bytes").map_err(llvm_err)?;
        let data = self.wide_to_utf8(wbuf, n, bytes)?;
        self.call(free, &[wbuf.into()])?;
        let data_null = self.b.build_is_null(data, "").map_err(llvm_err)?;
        let copy = self.block(f, "cwd.copy");
        self.b.build_conditional_branch(data_null, missing, copy).map_err(llvm_err)?;
        self.b.position_at_end(copy);
        let len = self.b.build_load(usize_ty, bytes, "cwd.len").map_err(llvm_err)?.into_int_value();
        self.copy_to_byte_list(out, data, len)?;
        self.call(free, &[data.into()])?;
        self.b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;
        self.b.position_at_end(missing);
        self.return_missing(out)?;
        Ok(())
    }

    /// `set_current_dir(path, path_len) -> i32`: 0 on success, 1 for an invalid path, and 3 when the system refuses.
    fn set_current_dir(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let f = self.function("burdin_rt_set_current_dir", i32_ty.fn_type(&[ptr_ty.into(), usize_ty.into()], false));
        let (path, path_len) = (Self::ptr_param(f, 0), Self::int_param(f, 1));
        let nul = self.has_byte(path, path_len, 0)?;
        let bad = self.block(f, "cwd.bad");
        let change = self.block(f, "cwd.change");
        self.b.build_conditional_branch(nul, bad, change).map_err(llvm_err)?;
        self.b.position_at_end(bad);
        self.b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;
        self.b.position_at_end(change);
        if self.windows {
            self.set_current_dir_windows(f, path, path_len)?;
        } else {
            self.set_current_dir_unix(path, path_len)?;
        }
        Ok(f)
    }

    fn set_current_dir_unix(&mut self, path: PointerValue<'ctx>, path_len: IntValue<'ctx>) -> GenResult<()> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let chdir = libc(self.module, "chdir", i32_ty.fn_type(&[ptr_ty.into()], false));
        let cpath = self.cstring_of(path, path_len, malloc)?;
        let rc = self.call(chdir, &[cpath.into()])?.expect("chdir returns an int").into_int_value();
        self.call(free, &[cpath.into()])?;
        let ok = self.b.build_int_compare(IntPredicate::EQ, rc, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let status = self.b.build_select(ok, i32_ty.const_zero(), i32_ty.const_int(3, false), "").map_err(llvm_err)?;
        self.b.build_return(Some(&status)).map_err(llvm_err)?;
        Ok(())
    }

    fn set_current_dir_windows(
        &mut self,
        f: FunctionValue<'ctx>,
        path: PointerValue<'ctx>,
        path_len: IntValue<'ctx>,
    ) -> GenResult<()> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let set = libc(self.module, "SetCurrentDirectoryW", i32_ty.fn_type(&[ptr_ty.into()], false));
        let units = self.b.build_alloca(usize_ty, "cwd.units").map_err(llvm_err)?;
        let wide = self.utf8_to_wide(path, path_len, units)?;
        let wide_null = self.b.build_is_null(wide, "").map_err(llvm_err)?;
        let bad = self.block(f, "cwd.bad.utf8");
        let call_set = self.block(f, "cwd.call.set");
        self.b.build_conditional_branch(wide_null, bad, call_set).map_err(llvm_err)?;
        self.b.position_at_end(bad);
        self.b.build_return(Some(&i32_ty.const_int(1, false))).map_err(llvm_err)?;
        self.b.position_at_end(call_set);
        let rc = self.call(set, &[wide.into()])?.expect("SetCurrentDirectoryW returns a flag").into_int_value();
        self.call(free, &[wide.into()])?;
        let failed = self.b.build_int_compare(IntPredicate::EQ, rc, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let status =
            self.b.build_select(failed, i32_ty.const_int(3, false), i32_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_return(Some(&status)).map_err(llvm_err)?;
        Ok(())
    }

    /// Converts UTF-8 bytes to a `malloc`ed NUL-terminated UTF-16 string. Returns a null pointer at run time when
    /// the bytes are not valid UTF-8 or the allocation fails, and stores the code-unit count in `units_out`.
    fn utf8_to_wide(
        &mut self,
        ptr: PointerValue<'ctx>,
        len: IntValue<'ctx>,
        units_out: PointerValue<'ctx>,
    ) -> GenResult<PointerValue<'ctx>> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let convert = libc(
            self.module,
            "MultiByteToWideChar",
            i32_ty.fn_type(
                &[i32_ty.into(), i32_ty.into(), ptr_ty.into(), i32_ty.into(), ptr_ty.into(), i32_ty.into()],
                false,
            ),
        );
        let f = self.b.get_insert_block().and_then(|b| b.get_parent()).expect("positioned in a function");
        let result = self.b.build_alloca(ptr_ty, "wide.out").map_err(llvm_err)?;
        self.b.build_store(result, ptr_ty.const_null()).map_err(llvm_err)?;
        self.b.build_store(units_out, usize_ty.const_zero()).map_err(llvm_err)?;
        let done = self.block(f, "wide.done");
        // An empty string has no code units, and `MultiByteToWideChar` reports zero for it; allocate the
        // terminating NUL directly.
        let empty = self.b.build_int_compare(IntPredicate::EQ, len, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let empty_bb = self.block(f, "wide.empty");
        let count = self.block(f, "wide.count");
        self.b.build_conditional_branch(empty, empty_bb, count).map_err(llvm_err)?;
        self.b.position_at_end(empty_bb);
        let two = usize_ty.const_int(2, false);
        let zero_wide = self.call(malloc, &[two.into()])?.expect("malloc returns a pointer").into_pointer_value();
        let zero_null = self.b.build_is_null(zero_wide, "").map_err(llvm_err)?;
        let empty_store = self.block(f, "wide.empty.store");
        self.b.build_conditional_branch(zero_null, done, empty_store).map_err(llvm_err)?;
        self.b.position_at_end(empty_store);
        self.b.build_store(zero_wide, self.cx.i16_type().const_zero()).map_err(llvm_err)?;
        self.b.build_store(result, zero_wide).map_err(llvm_err)?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(count);
        let (code_page, flags) = (i32_ty.const_int(65001, false), i32_ty.const_int(0x8, false));
        let len32 = self.b.build_int_truncate(len, i32_ty, "").map_err(llvm_err)?;
        let needed = self
            .call(
                convert,
                &[
                    code_page.into(),
                    flags.into(),
                    ptr.into(),
                    len32.into(),
                    ptr_ty.const_null().into(),
                    i32_ty.const_zero().into(),
                ],
            )?
            .expect("MultiByteToWideChar returns a count")
            .into_int_value();
        let none = self.b.build_int_compare(IntPredicate::EQ, needed, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let alloc = self.block(f, "wide.alloc");
        self.b.build_conditional_branch(none, done, alloc).map_err(llvm_err)?;
        self.b.position_at_end(alloc);
        let units = self.b.build_int_z_extend(needed, usize_ty, "").map_err(llvm_err)?;
        let size = self.b.build_int_add(units, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        let bytes = self.b.build_int_mul(size, usize_ty.const_int(2, false), "").map_err(llvm_err)?;
        let wide = self.call(malloc, &[bytes.into()])?.expect("malloc returns a pointer").into_pointer_value();
        let null = self.b.build_is_null(wide, "").map_err(llvm_err)?;
        let fill = self.block(f, "wide.fill");
        self.b.build_conditional_branch(null, done, fill).map_err(llvm_err)?;
        self.b.position_at_end(fill);
        let converted = self
            .call(convert, &[code_page.into(), flags.into(), ptr.into(), len32.into(), wide.into(), needed.into()])?
            .expect("MultiByteToWideChar returns a count")
            .into_int_value();
        let bad = self.b.build_int_compare(IntPredicate::EQ, converted, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let store = self.block(f, "wide.store");
        let free_wide = self.block(f, "wide.free");
        self.b.build_conditional_branch(bad, free_wide, store).map_err(llvm_err)?;
        self.b.position_at_end(free_wide);
        self.call(free, &[wide.into()])?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(store);
        let term = unsafe { self.b.build_in_bounds_gep(self.cx.i16_type(), wide, &[units], "") }.map_err(llvm_err)?;
        self.b.build_store(term, self.cx.i16_type().const_zero()).map_err(llvm_err)?;
        self.b.build_store(units_out, units).map_err(llvm_err)?;
        self.b.build_store(result, wide).map_err(llvm_err)?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(done);
        self.b.build_load(ptr_ty, result, "wide.result").map_err(llvm_err).map(|v| v.into_pointer_value())
    }

    /// Converts `units` UTF-16 code units to a `malloc`ed UTF-8 buffer, storing the byte count in `bytes_out`.
    /// Returns a null pointer at run time when the units are not valid Unicode or the allocation fails.
    fn wide_to_utf8(
        &mut self,
        wide: PointerValue<'ctx>,
        units: IntValue<'ctx>,
        bytes_out: PointerValue<'ctx>,
    ) -> GenResult<PointerValue<'ctx>> {
        let (ptr_ty, usize_ty, i32_ty) = (self.ptr_ty, self.usize_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let convert = libc(
            self.module,
            "WideCharToMultiByte",
            i32_ty.fn_type(
                &[
                    i32_ty.into(),
                    i32_ty.into(),
                    ptr_ty.into(),
                    i32_ty.into(),
                    ptr_ty.into(),
                    i32_ty.into(),
                    ptr_ty.into(),
                    ptr_ty.into(),
                ],
                false,
            ),
        );
        let f = self.b.get_insert_block().and_then(|b| b.get_parent()).expect("positioned in a function");
        let result = self.b.build_alloca(ptr_ty, "utf8.out").map_err(llvm_err)?;
        self.b.build_store(result, ptr_ty.const_null()).map_err(llvm_err)?;
        self.b.build_store(bytes_out, usize_ty.const_zero()).map_err(llvm_err)?;
        let (code_page, flags) = (i32_ty.const_int(65001, false), i32_ty.const_int(0x80, false));
        let units32 = self.b.build_int_truncate(units, i32_ty, "").map_err(llvm_err)?;
        let needed = self
            .call(
                convert,
                &[
                    code_page.into(),
                    flags.into(),
                    wide.into(),
                    units32.into(),
                    ptr_ty.const_null().into(),
                    i32_ty.const_zero().into(),
                    ptr_ty.const_null().into(),
                    ptr_ty.const_null().into(),
                ],
            )?
            .expect("WideCharToMultiByte returns a count")
            .into_int_value();
        let none = self.b.build_int_compare(IntPredicate::EQ, needed, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let alloc = self.block(f, "utf8.alloc");
        let done = self.block(f, "utf8.done");
        self.b.build_conditional_branch(none, done, alloc).map_err(llvm_err)?;
        self.b.position_at_end(alloc);
        let size = self.b.build_int_z_extend(needed, usize_ty, "").map_err(llvm_err)?;
        let bytes = self.call(malloc, &[size.into()])?.expect("malloc returns a pointer").into_pointer_value();
        let null = self.b.build_is_null(bytes, "").map_err(llvm_err)?;
        let fill = self.block(f, "utf8.fill");
        self.b.build_conditional_branch(null, done, fill).map_err(llvm_err)?;
        self.b.position_at_end(fill);
        let converted = self
            .call(
                convert,
                &[
                    code_page.into(),
                    flags.into(),
                    wide.into(),
                    units32.into(),
                    bytes.into(),
                    needed.into(),
                    ptr_ty.const_null().into(),
                    ptr_ty.const_null().into(),
                ],
            )?
            .expect("WideCharToMultiByte returns a count")
            .into_int_value();
        let bad = self.b.build_int_compare(IntPredicate::EQ, converted, i32_ty.const_zero(), "").map_err(llvm_err)?;
        let store = self.block(f, "utf8.store");
        let free_bytes = self.block(f, "utf8.free");
        self.b.build_conditional_branch(bad, free_bytes, store).map_err(llvm_err)?;
        self.b.position_at_end(free_bytes);
        self.call(free, &[bytes.into()])?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(store);
        self.b.build_store(bytes_out, size).map_err(llvm_err)?;
        self.b.build_store(result, bytes).map_err(llvm_err)?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(done);
        self.b.build_load(ptr_ty, result, "utf8.result").map_err(llvm_err).map(|v| v.into_pointer_value())
    }

    // ----- sockets and name resolution (`spec/io.md`, IO5-IO6) -------------------------------------------------
    //
    // The socket primitives report one result for every operation: `>= 0` is success (a descriptor, a byte count,
    // or zero), `-1` means the non-blocking call would block (or a connect is still in progress), and `-2` means
    // failure, with the system's code in the thread-local `burdin_rt_net_err`.

    /// The descriptor type of the platform: `SOCKET` is 64 bits on 64-bit Windows, `int` on Unix.
    fn fd_ty(&self) -> IntType<'ctx> {
        if self.windows { self.i64() } else { self.i32() }
    }

    /// The return type of the calls that report a byte count or `-1`: `int` on Windows and `ssize_t` on Unix.
    /// Declaring the real width matters: a 32-bit `-1` read as 64 bits would look like a byte count.
    fn count_ty(&self) -> IntType<'ctx> {
        if self.windows { self.i32() } else { self.i64() }
    }

    /// An integer widened to `i64` (by sign extension when it is narrower).
    fn widen_i64(&mut self, value: IntValue<'ctx>) -> GenResult<IntValue<'ctx>> {
        if value.get_type().get_bit_width() == 64 {
            Ok(value)
        } else {
            self.b.build_int_s_extend(value, self.i64(), "").map_err(llvm_err)
        }
    }

    /// An integer converted to `usize` without changing its value (supported targets have a 64-bit `usize`).
    fn widen_usize(&mut self, value: IntValue<'ctx>) -> GenResult<IntValue<'ctx>> {
        let from = value.get_type().get_bit_width();
        let to = self.usize_ty.get_bit_width();
        if from == to {
            Ok(value)
        } else if from < to {
            self.b.build_int_z_extend(value, self.usize_ty, "").map_err(llvm_err)
        } else {
            self.b.build_int_truncate(value, self.usize_ty, "").map_err(llvm_err)
        }
    }

    /// The system code of the socket failure that just happened: `WSAGetLastError` on Windows, `errno` on Unix.
    /// It is stored in `@burdin_rt_net_err` so `__net_last_error` can report it.
    fn net_capture_error(&mut self) -> GenResult<IntValue<'ctx>> {
        let i32_ty = self.i32();
        let code = if self.windows {
            let get = libc(self.module, "WSAGetLastError", i32_ty.fn_type(&[], false));
            self.call(get, &[])?.expect("WSAGetLastError returns a code").into_int_value()
        } else {
            let at = self.errno_ptr()?;
            self.b.build_load(i32_ty, at, "errno").map_err(llvm_err)?.into_int_value()
        };
        let slot = self.module.get_global("burdin_rt_net_err").expect("the socket error slot exists");
        self.b.build_store(slot.as_pointer_value(), code).map_err(llvm_err)?;
        Ok(code)
    }

    /// The result of a socket operation whose raw outcome is `raw`: negative is a failure, which becomes `-1` when
    /// the system says to retry and `-2` otherwise; anything else passes through.
    fn net_result(&mut self, raw: IntValue<'ctx>) -> GenResult<IntValue<'ctx>> {
        let i64_ty = self.i64();
        let raw = if raw.get_type().get_bit_width() < 64 {
            self.b.build_int_s_extend(raw, i64_ty, "net.raw").map_err(llvm_err)?
        } else {
            raw
        };
        let code = self.net_capture_error()?;
        let failed =
            self.b.build_int_compare(IntPredicate::SLT, raw, i64_ty.const_zero(), "net.failed").map_err(llvm_err)?;
        let mut retry = self
            .b
            .build_int_compare(
                IntPredicate::EQ,
                code,
                self.i32().const_int(u64::from(RETRY_CODES[0] as u32), false),
                "",
            )
            .map_err(llvm_err)?;
        for c in &RETRY_CODES[1..] {
            let same = self
                .b
                .build_int_compare(IntPredicate::EQ, code, self.i32().const_int(u64::from(*c as u32), false), "")
                .map_err(llvm_err)?;
            retry = self.b.build_or(retry, same, "").map_err(llvm_err)?;
        }
        let failure = self
            .b
            .build_select(retry, i64_ty.const_int(u64::MAX, true), i64_ty.const_int(u64::MAX - 1, true), "net.why")
            .map_err(llvm_err)?
            .into_int_value();
        Ok(self.b.build_select(failed, failure, raw, "net.out").map_err(llvm_err)?.into_int_value())
    }

    /// On Windows, Winsock must be started before the first socket call. `WSAStartup` is refcounted, so a
    /// concurrent second call is harmless; the flag only saves the repeated call.
    fn net_startup(&mut self) -> GenResult<()> {
        if !self.windows {
            return Ok(());
        }
        let i32_ty = self.i32();
        let started = self.module.get_global("burdin_rt_wsa").expect("the Winsock flag exists").as_pointer_value();
        let flag = self.b.build_load(i32_ty, started, "wsa.started").map_err(llvm_err)?.into_int_value();
        let done = self.block(self.current_function(), "wsa.done");
        let begin = self.block(self.current_function(), "wsa.begin");
        let is = self.b.build_int_compare(IntPredicate::NE, flag, i32_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(is, done, begin).map_err(llvm_err)?;
        self.b.position_at_end(begin);
        let data = self.b.build_alloca(i32_ty.array_type(102), "wsa.data").map_err(llvm_err)?;
        let startup = libc(self.module, "WSAStartup", i32_ty.fn_type(&[i32_ty.into(), self.ptr_ty.into()], false));
        self.call(startup, &[i32_ty.const_int(0x202, false).into(), data.into()])?;
        self.b.build_store(started, i32_ty.const_int(1, false)).map_err(llvm_err)?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(done);
        Ok(())
    }

    /// The function value of the current function, for the small helpers that need its blocks.
    fn current_function(&self) -> FunctionValue<'ctx> {
        self.b.get_insert_block().and_then(|b| b.get_parent()).expect("the builder is inside a function")
    }

    /// `resolve(host, len, port, out_sockaddr, passive) -> i64`: the first IPv4 address of `host` as a 16-byte
    /// `sockaddr_in` with the port filled in. `host` with length 0 is the wildcard address for `bind`.
    fn net_resolve(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (i64_ty, ptr_ty, i32_ty) = (self.i64(), self.ptr_ty, self.i32());
        let params = [ptr_ty.into(), i64_ty.into(), i32_ty.into(), ptr_ty.into(), i32_ty.into()];
        let f = self.function("burdin_rt_net_resolve", i64_ty.fn_type(&params, false));
        let (host, len, port, out, passive) = (
            Self::ptr_param(f, 0),
            Self::int_param(f, 1),
            Self::int_param(f, 2),
            Self::ptr_param(f, 3),
            Self::int_param(f, 4),
        );
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[self.usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let hints = self.b.build_alloca(self.i8().array_type(ADDRINFO_SIZE as u32), "ai.hints").map_err(llvm_err)?;
        self.b
            .build_memset(hints, 8, self.i8().const_zero(), self.usize_ty.const_int(ADDRINFO_SIZE, false))
            .map_err(llvm_err)?;
        let family = self.offset(hints, self.usize_ty.const_int(ADDRINFO_FAMILY_OFFSET, false))?;
        self.b.build_store(family, i32_ty.const_int(u64::from(AF_INET as u32), false)).map_err(llvm_err)?;
        let flags = self.offset(hints, self.usize_ty.const_int(ADDRINFO_FLAGS_OFFSET, false))?;
        let passive_flag = self
            .b
            .build_select(
                self.b.build_int_compare(IntPredicate::NE, passive, i32_ty.const_zero(), "").map_err(llvm_err)?,
                i32_ty.const_int(u64::from(AI_PASSIVE as u32), false),
                i32_ty.const_zero(),
                "ai.flags",
            )
            .map_err(llvm_err)?
            .into_int_value();
        self.b.build_store(flags, passive_flag).map_err(llvm_err)?;
        let result = self.b.build_alloca(ptr_ty, "ai.result").map_err(llvm_err)?;
        self.b.build_store(result, ptr_ty.const_null()).map_err(llvm_err)?;
        let named = self.block(f, "resolve.named");
        let resolved = self.block(f, "resolve.resolved");
        let missing =
            self.b.build_int_compare(IntPredicate::EQ, len, i64_ty.const_zero(), "wildcard").map_err(llvm_err)?;
        let host_slot = self.b.build_alloca(ptr_ty, "host.nul").map_err(llvm_err)?;
        self.b.build_store(host_slot, ptr_ty.const_null()).map_err(llvm_err)?;
        self.b.build_conditional_branch(missing, resolved, named).map_err(llvm_err)?;
        self.b.position_at_end(named);
        let nul = self.cstring_of(host, len, malloc)?;
        self.b.build_store(host_slot, nul).map_err(llvm_err)?;
        self.b.build_unconditional_branch(resolved).map_err(llvm_err)?;
        self.b.position_at_end(resolved);
        let node = self.b.build_load(ptr_ty, host_slot, "node").map_err(llvm_err)?.into_pointer_value();
        let getaddrinfo = libc(
            self.module,
            "getaddrinfo",
            i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into(), ptr_ty.into(), ptr_ty.into()], false),
        );
        let code = self
            .call(getaddrinfo, &[node.into(), ptr_ty.const_null().into(), hints.into(), result.into()])?
            .expect("getaddrinfo returns a code")
            .into_int_value();
        let freeaddrinfo = libc(self.module, "freeaddrinfo", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let bad =
            self.b.build_int_compare(IntPredicate::NE, code, i32_ty.const_zero(), "resolve.bad").map_err(llvm_err)?;
        let fail = self.block(f, "resolve.fail");
        let found = self.block(f, "resolve.found");
        self.b.build_conditional_branch(bad, fail, found).map_err(llvm_err)?;

        self.b.position_at_end(fail);
        let slot = self.module.get_global("burdin_rt_net_err").expect("the socket error slot exists");
        self.b.build_store(slot.as_pointer_value(), code).map_err(llvm_err)?;
        self.call(free, &[node.into()])?;
        self.b.build_return(Some(&i64_ty.const_int(u64::MAX - 1, true))).map_err(llvm_err)?;

        self.b.position_at_end(found);
        let first = self.b.build_load(ptr_ty, result, "ai.first").map_err(llvm_err)?.into_pointer_value();
        let empty = self.b.build_is_null(first, "ai.none").map_err(llvm_err)?;
        let none = self.block(f, "resolve.none");
        let copy = self.block(f, "resolve.copy");
        self.b.build_conditional_branch(empty, none, copy).map_err(llvm_err)?;
        self.b.position_at_end(none);
        let no_name: i32 = if self.windows { 11001 } else { -2 }; // WSAHOST_NOT_FOUND / EAI_NONAME
        self.b.build_store(slot.as_pointer_value(), i32_ty.const_int(no_name as u32 as u64, true)).map_err(llvm_err)?;
        self.call(free, &[node.into()])?;
        self.b.build_return(Some(&i64_ty.const_int(u64::MAX - 1, true))).map_err(llvm_err)?;

        self.b.position_at_end(copy);
        let addr_at = self.offset(first, self.usize_ty.const_int(ADDRINFO_ADDR_OFFSET, false))?;
        let addr = self.b.build_load(ptr_ty, addr_at, "ai.addr").map_err(llvm_err)?.into_pointer_value();
        self.b.build_memcpy(out, 1, addr, 1, self.usize_ty.const_int(SOCKADDR_SIZE, false)).map_err(llvm_err)?;
        // The port is in network byte order: the high byte first.
        let high = self
            .b
            .build_int_truncate(
                self.b.build_right_shift(port, i32_ty.const_int(8, false), false, "").map_err(llvm_err)?,
                self.i8(),
                "",
            )
            .map_err(llvm_err)?;
        let low = self.b.build_int_truncate(port, self.i8(), "").map_err(llvm_err)?;
        let high_at = self.offset(out, self.usize_ty.const_int(SOCKADDR_PORT_OFFSET, false))?;
        let low_at = self.offset(out, self.usize_ty.const_int(SOCKADDR_PORT_OFFSET + 1, false))?;
        self.b.build_store(high_at, high).map_err(llvm_err)?;
        self.b.build_store(low_at, low).map_err(llvm_err)?;
        self.call(freeaddrinfo, &[first.into()])?;
        self.call(free, &[node.into()])?;
        self.b.build_return(Some(&i64_ty.const_zero())).map_err(llvm_err)?;
        Ok(f)
    }

    /// The runtime's socket functions.
    fn net_functions(
        &mut self,
        string_push_str: FunctionValue<'ctx>,
        string_push_char: FunctionValue<'ctx>,
    ) -> GenResult<()> {
        let (i64_ty, ptr_ty, i32_ty) = (self.i64(), self.ptr_ty, self.i32());
        let void_ty = self.cx.void_type();
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[self.usize_ty.into()], false));
        let free = libc(self.module, "free", void_ty.fn_type(&[ptr_ty.into()], false));
        let fd_ty = self.fd_ty();

        let resolve = self.net_resolve()?;

        // `net_socket(kind)`: 1 is a stream socket and 2 a datagram socket.
        let f = self.function("burdin_rt_net_socket", i64_ty.fn_type(&[i32_ty.into()], false));
        let kind = Self::int_param(f, 0);
        self.net_startup()?;
        let socket = libc(self.module, "socket", fd_ty.fn_type(&[i32_ty.into(), i32_ty.into(), i32_ty.into()], false));
        let type_ = self
            .b
            .build_select(
                self.b.build_int_compare(IntPredicate::EQ, kind, i32_ty.const_int(2, false), "").map_err(llvm_err)?,
                i32_ty.const_int(u64::from(SOCK_DGRAM as u32), false),
                i32_ty.const_int(u64::from(SOCK_STREAM as u32), false),
                "sock.type",
            )
            .map_err(llvm_err)?
            .into_int_value();
        let raw = self
            .call(
                socket,
                &[i32_ty.const_int(u64::from(AF_INET as u32), false).into(), type_.into(), i32_ty.const_zero().into()],
            )?
            .expect("socket returns a descriptor")
            .into_int_value();
        let fd = self.widen_i64(raw)?;
        let failed =
            self.b.build_int_compare(IntPredicate::SLT, fd, i64_ty.const_zero(), "sock.failed").map_err(llvm_err)?;
        let fail = self.block(f, "sock.fail");
        let ok = self.block(f, "sock.ok");
        self.b.build_conditional_branch(failed, fail, ok).map_err(llvm_err)?;
        self.b.position_at_end(fail);
        self.net_capture_error()?;
        self.b.build_return(Some(&i64_ty.const_int(u64::MAX - 1, true))).map_err(llvm_err)?;
        self.b.position_at_end(ok);
        // Non-blocking is what makes a wait on the reactor possible.
        if self.windows {
            let on = self.b.build_alloca(i32_ty, "nonblock").map_err(llvm_err)?;
            self.b.build_store(on, i32_ty.const_int(1, false)).map_err(llvm_err)?;
            let ioctlsocket =
                libc(self.module, "ioctlsocket", i32_ty.fn_type(&[i64_ty.into(), i32_ty.into(), ptr_ty.into()], false));
            self.call(ioctlsocket, &[raw.into(), i32_ty.const_int(FIONBIO as u32 as u64, true).into(), on.into()])?;
        } else {
            // The runtime's file primitives already declare `fcntl` with an `int` descriptor, so this uses the
            // same declaration and truncates the socket descriptor for it.
            let fcntl = libc(self.module, "fcntl", i32_ty.fn_type(&[i32_ty.into(), i32_ty.into()], true));
            let small = self.b.build_int_truncate(fd, i32_ty, "fd32").map_err(llvm_err)?;
            let current = self
                .call(fcntl, &[small.into(), i32_ty.const_int(F_GETFL as u32 as u64, false).into()])?
                .expect("fcntl returns flags")
                .into_int_value();
            let flags =
                self.b.build_or(current, i32_ty.const_int(O_NONBLOCK as u32 as u64, false), "").map_err(llvm_err)?;
            self.call(fcntl, &[small.into(), i32_ty.const_int(F_SETFL as u32 as u64, false).into(), flags.into()])?;
            // A TCP socket takes `SO_REUSEADDR`, so a quick restart can bind a port a test just used. Windows
            // gives the option a different meaning (it permits hijacking), so it is not set there.
            let one = self.b.build_alloca(i32_ty, "reuse").map_err(llvm_err)?;
            self.b.build_store(one, i32_ty.const_int(1, false)).map_err(llvm_err)?;
            let setsockopt = libc(
                self.module,
                "setsockopt",
                i32_ty.fn_type(&[i64_ty.into(), i32_ty.into(), i32_ty.into(), ptr_ty.into(), i32_ty.into()], false),
            );
            self.call(
                setsockopt,
                &[
                    fd.into(),
                    i32_ty.const_int(u64::from(SOL_SOCKET as u32), false).into(),
                    i32_ty.const_int(u64::from(SO_REUSEADDR as u32), false).into(),
                    one.into(),
                    i32_ty.const_int(4, false).into(),
                ],
            )?;
        }
        self.b.build_return(Some(&fd)).map_err(llvm_err)?;

        // `net_bind(fd, host, len, port)`: `net_resolve` fills a `sockaddr_in`.
        let f = self.function(
            "burdin_rt_net_bind",
            i64_ty.fn_type(&[i64_ty.into(), ptr_ty.into(), i64_ty.into(), i32_ty.into()], false),
        );
        let (fd, host, len, port) =
            (Self::int_param(f, 0), Self::ptr_param(f, 1), Self::int_param(f, 2), Self::int_param(f, 3));
        let sockaddr = self.b.build_alloca(self.i8().array_type(SOCKADDR_SIZE as u32), "sockaddr").map_err(llvm_err)?;
        let resolved = self
            .call(resolve, &[host.into(), len.into(), port.into(), sockaddr.into(), i32_ty.const_int(1, false).into()])?
            .expect("resolve returns a code")
            .into_int_value();
        let bad = self
            .b
            .build_int_compare(IntPredicate::SLT, resolved, i64_ty.const_zero(), "bind.resolve")
            .map_err(llvm_err)?;
        let fail = self.block(f, "bind.fail");
        let go = self.block(f, "bind.go");
        self.b.build_conditional_branch(bad, fail, go).map_err(llvm_err)?;
        self.b.position_at_end(fail);
        self.b.build_return(Some(&i64_ty.const_int(u64::MAX - 1, true))).map_err(llvm_err)?;
        self.b.position_at_end(go);
        let bind = libc(self.module, "bind", i32_ty.fn_type(&[i64_ty.into(), ptr_ty.into(), i32_ty.into()], false));
        let raw = self
            .call(bind, &[fd.into(), sockaddr.into(), i32_ty.const_int(SOCKADDR_SIZE, false).into()])?
            .expect("bind returns a code")
            .into_int_value();
        let out = self.net_result(raw)?;
        self.b.build_return(Some(&out)).map_err(llvm_err)?;

        // `net_listen(fd, backlog)`.
        let f = self.function("burdin_rt_net_listen", i64_ty.fn_type(&[i64_ty.into(), i32_ty.into()], false));
        let (fd, backlog) = (Self::int_param(f, 0), Self::int_param(f, 1));
        let listen = libc(self.module, "listen", i32_ty.fn_type(&[i64_ty.into(), i32_ty.into()], false));
        let raw = self.call(listen, &[fd.into(), backlog.into()])?.expect("listen returns a code").into_int_value();
        let out = self.net_result(raw)?;
        self.b.build_return(Some(&out)).map_err(llvm_err)?;

        // `net_accept(fd)`: the new descriptor, or a retry code.
        let f = self.function("burdin_rt_net_accept", i64_ty.fn_type(&[i64_ty.into()], false));
        let fd = Self::int_param(f, 0);
        let accept = libc(self.module, "accept", fd_ty.fn_type(&[i64_ty.into(), ptr_ty.into(), ptr_ty.into()], false));
        let raw = self
            .call(accept, &[fd.into(), ptr_ty.const_null().into(), ptr_ty.const_null().into()])?
            .expect("accept returns a descriptor")
            .into_int_value();
        let raw = self.widen_i64(raw)?;
        let out = self.net_result(raw)?;
        let failed =
            self.b.build_int_compare(IntPredicate::SLT, out, i64_ty.const_zero(), "accept.failed").map_err(llvm_err)?;
        let fail = self.block(f, "accept.fail");
        let ok = self.block(f, "accept.ok");
        self.b.build_conditional_branch(failed, fail, ok).map_err(llvm_err)?;
        self.b.position_at_end(fail);
        self.b.build_return(Some(&out)).map_err(llvm_err)?;
        self.b.position_at_end(ok);
        // The accepted descriptor inherits the listening socket's non-blocking mode on Linux but not on macOS or
        // Windows, so it is set again.
        if self.windows {
            let on = self.b.build_alloca(i32_ty, "nonblock").map_err(llvm_err)?;
            self.b.build_store(on, i32_ty.const_int(1, false)).map_err(llvm_err)?;
            let ioctlsocket =
                libc(self.module, "ioctlsocket", i32_ty.fn_type(&[i64_ty.into(), i32_ty.into(), ptr_ty.into()], false));
            self.call(ioctlsocket, &[out.into(), i32_ty.const_int(FIONBIO as u32 as u64, true).into(), on.into()])?;
        } else {
            let fcntl = libc(self.module, "fcntl", i32_ty.fn_type(&[i32_ty.into(), i32_ty.into()], true));
            let small = self.b.build_int_truncate(out, i32_ty, "fd32").map_err(llvm_err)?;
            let current = self
                .call(fcntl, &[small.into(), i32_ty.const_int(F_GETFL as u32 as u64, false).into()])?
                .expect("fcntl returns flags")
                .into_int_value();
            let flags =
                self.b.build_or(current, i32_ty.const_int(O_NONBLOCK as u32 as u64, false), "").map_err(llvm_err)?;
            self.call(fcntl, &[small.into(), i32_ty.const_int(F_SETFL as u32 as u64, false).into(), flags.into()])?;
        }
        self.b.build_return(Some(&out)).map_err(llvm_err)?;

        // `net_connect(fd, host, len, port)`.
        let f = self.function(
            "burdin_rt_net_connect",
            i64_ty.fn_type(&[i64_ty.into(), ptr_ty.into(), i64_ty.into(), i32_ty.into()], false),
        );
        let (fd, host, len, port) =
            (Self::int_param(f, 0), Self::ptr_param(f, 1), Self::int_param(f, 2), Self::int_param(f, 3));
        let sockaddr = self.b.build_alloca(self.i8().array_type(SOCKADDR_SIZE as u32), "sockaddr").map_err(llvm_err)?;
        self.net_startup()?;
        let resolved = self
            .call(resolve, &[host.into(), len.into(), port.into(), sockaddr.into(), i32_ty.const_zero().into()])?
            .expect("resolve returns a code")
            .into_int_value();
        let bad = self
            .b
            .build_int_compare(IntPredicate::SLT, resolved, i64_ty.const_zero(), "connect.resolve")
            .map_err(llvm_err)?;
        let fail = self.block(f, "connect.fail");
        let go = self.block(f, "connect.go");
        self.b.build_conditional_branch(bad, fail, go).map_err(llvm_err)?;
        self.b.position_at_end(fail);
        self.b.build_return(Some(&i64_ty.const_int(u64::MAX - 1, true))).map_err(llvm_err)?;
        self.b.position_at_end(go);
        let connect =
            libc(self.module, "connect", i32_ty.fn_type(&[i64_ty.into(), ptr_ty.into(), i32_ty.into()], false));
        let raw = self
            .call(connect, &[fd.into(), sockaddr.into(), i32_ty.const_int(SOCKADDR_SIZE, false).into()])?
            .expect("connect returns a code")
            .into_int_value();
        let out = self.net_result(raw)?;
        self.b.build_return(Some(&out)).map_err(llvm_err)?;

        // `net_send(fd, data, len)`.
        let f =
            self.function("burdin_rt_net_send", i64_ty.fn_type(&[i64_ty.into(), ptr_ty.into(), i64_ty.into()], false));
        let (fd, data, len) = (Self::int_param(f, 0), Self::ptr_param(f, 1), Self::int_param(f, 2));
        let send = libc(
            self.module,
            "send",
            self.count_ty().fn_type(&[i64_ty.into(), ptr_ty.into(), i64_ty.into(), i32_ty.into()], false),
        );
        let raw = self
            .call(send, &[fd.into(), data.into(), len.into(), i32_ty.const_zero().into()])?
            .expect("send returns a count")
            .into_int_value();
        let out = self.net_result(raw)?;
        self.b.build_return(Some(&out)).map_err(llvm_err)?;

        // `net_send_to(fd, data, len, host, host_len, port)`.
        let f = self.function(
            "burdin_rt_net_send_to",
            i64_ty.fn_type(
                &[i64_ty.into(), ptr_ty.into(), i64_ty.into(), ptr_ty.into(), i64_ty.into(), i32_ty.into()],
                false,
            ),
        );
        let (fd, data, len, host, host_len, port) = (
            Self::int_param(f, 0),
            Self::ptr_param(f, 1),
            Self::int_param(f, 2),
            Self::ptr_param(f, 3),
            Self::int_param(f, 4),
            Self::int_param(f, 5),
        );
        let sockaddr = self.b.build_alloca(self.i8().array_type(SOCKADDR_SIZE as u32), "sockaddr").map_err(llvm_err)?;
        let resolved = self
            .call(resolve, &[host.into(), host_len.into(), port.into(), sockaddr.into(), i32_ty.const_zero().into()])?
            .expect("resolve returns a code")
            .into_int_value();
        let bad = self
            .b
            .build_int_compare(IntPredicate::SLT, resolved, i64_ty.const_zero(), "sendto.resolve")
            .map_err(llvm_err)?;
        let fail = self.block(f, "sendto.fail");
        let go = self.block(f, "sendto.go");
        self.b.build_conditional_branch(bad, fail, go).map_err(llvm_err)?;
        self.b.position_at_end(fail);
        self.b.build_return(Some(&i64_ty.const_int(u64::MAX - 1, true))).map_err(llvm_err)?;
        self.b.position_at_end(go);
        let sendto = libc(
            self.module,
            "sendto",
            self.count_ty().fn_type(
                &[i64_ty.into(), ptr_ty.into(), i64_ty.into(), i32_ty.into(), ptr_ty.into(), i32_ty.into()],
                false,
            ),
        );
        let raw = self
            .call(
                sendto,
                &[
                    fd.into(),
                    data.into(),
                    len.into(),
                    i32_ty.const_zero().into(),
                    sockaddr.into(),
                    i32_ty.const_int(SOCKADDR_SIZE, false).into(),
                ],
            )?
            .expect("sendto returns a count")
            .into_int_value();
        let out = self.net_result(raw)?;
        self.b.build_return(Some(&out)).map_err(llvm_err)?;

        // `net_recv(fd, out: *List<u8>, max)` and `net_recv_from`: append what arrived, like `file_read`.
        for (name, with_peer) in [("burdin_rt_net_recv", false), ("burdin_rt_net_recv_from", true)] {
            let f = self.function(name, i64_ty.fn_type(&[i64_ty.into(), ptr_ty.into(), i64_ty.into()], false));
            let (fd, out, max) = (Self::int_param(f, 0), Self::ptr_param(f, 1), Self::int_param(f, 2));
            let ptr_field = self.b.build_struct_gep(self.string_ty, out, 0, "list.ptr").map_err(llvm_err)?;
            let len_field = self.b.build_struct_gep(self.string_ty, out, 1, "list.len").map_err(llvm_err)?;
            let cap_field = self.b.build_struct_gep(self.string_ty, out, 2, "list.cap").map_err(llvm_err)?;
            let zero = i64_ty.const_zero();
            let empty = self.b.build_int_compare(IntPredicate::EQ, max, zero, "empty").map_err(llvm_err)?;
            let check = self.block(f, "recv.check");
            let none = self.block(f, "recv.none");
            self.b.build_conditional_branch(empty, none, check).map_err(llvm_err)?;
            self.b.position_at_end(none);
            self.b.build_return(Some(&zero)).map_err(llvm_err)?;

            self.b.position_at_end(check);
            let len = self.b.build_load(self.usize_ty, len_field, "len").map_err(llvm_err)?.into_int_value();
            let cap = self.b.build_load(self.usize_ty, cap_field, "cap").map_err(llvm_err)?.into_int_value();
            let max_usize = self.widen_usize(max)?;
            let needed = self.b.build_int_add(len, max_usize, "needed").map_err(llvm_err)?;
            let full = self.b.build_int_compare(IntPredicate::ULT, cap, needed, "full").map_err(llvm_err)?;
            let grow = self.block(f, "recv.grow");
            let read = self.block(f, "recv.read");
            self.b.build_conditional_branch(full, grow, read).map_err(llvm_err)?;

            self.b.position_at_end(grow);
            let doubled = self.b.build_int_mul(cap, self.usize_ty.const_int(2, false), "doubled").map_err(llvm_err)?;
            let wide = self
                .b
                .build_select(
                    self.b.build_int_compare(IntPredicate::ULT, doubled, needed, "").map_err(llvm_err)?,
                    needed,
                    doubled,
                    "cap",
                )
                .map_err(llvm_err)?
                .into_int_value();
            let grown = self
                .b
                .build_select(
                    self.b
                        .build_int_compare(IntPredicate::ULT, wide, self.usize_ty.const_int(4, false), "")
                        .map_err(llvm_err)?,
                    self.usize_ty.const_int(4, false),
                    wide,
                    "cap",
                )
                .map_err(llvm_err)?
                .into_int_value();
            let data = self.b.build_load(ptr_ty, ptr_field, "ptr").map_err(llvm_err)?.into_pointer_value();
            let fresh = self.call_rt_realloc(data, grown)?;
            self.b.build_store(ptr_field, fresh).map_err(llvm_err)?;
            self.b.build_store(cap_field, grown).map_err(llvm_err)?;
            self.b.build_unconditional_branch(read).map_err(llvm_err)?;

            self.b.position_at_end(read);
            let len = self.b.build_load(self.usize_ty, len_field, "len").map_err(llvm_err)?.into_int_value();
            let data = self.b.build_load(ptr_ty, ptr_field, "ptr").map_err(llvm_err)?.into_pointer_value();
            let dest = self.offset(data, len)?;
            let raw = if with_peer {
                let addr =
                    self.b.build_alloca(self.i8().array_type(SOCKADDR_SIZE as u32), "peer.addr").map_err(llvm_err)?;
                let addr_len = self.b.build_alloca(i32_ty, "peer.len").map_err(llvm_err)?;
                self.b.build_store(addr_len, i32_ty.const_int(SOCKADDR_SIZE, false)).map_err(llvm_err)?;
                let recvfrom = libc(
                    self.module,
                    "recvfrom",
                    self.count_ty().fn_type(
                        &[i64_ty.into(), ptr_ty.into(), i64_ty.into(), i32_ty.into(), ptr_ty.into(), ptr_ty.into()],
                        false,
                    ),
                );
                let raw = self
                    .call(
                        recvfrom,
                        &[fd.into(), dest.into(), max.into(), i32_ty.const_zero().into(), addr.into(), addr_len.into()],
                    )?
                    .expect("recvfrom returns a count")
                    .into_int_value();
                // Pack the sender: bits 0-31 the address in big-endian order, bits 32-63 the port.
                let peer_slot = self.module.get_global("burdin_rt_net_peer").expect("the peer slot exists");
                let packed = self.pack_address(addr)?;
                self.b.build_store(peer_slot.as_pointer_value(), packed).map_err(llvm_err)?;
                raw
            } else {
                let recv = libc(
                    self.module,
                    "recv",
                    self.count_ty().fn_type(&[i64_ty.into(), ptr_ty.into(), i64_ty.into(), i32_ty.into()], false),
                );
                self.call(recv, &[fd.into(), dest.into(), max.into(), i32_ty.const_zero().into()])?
                    .expect("recv returns a count")
                    .into_int_value()
            };
            let out_code = self.net_result(raw)?;
            let failed =
                self.b.build_int_compare(IntPredicate::SLT, out_code, zero, "recv.failed").map_err(llvm_err)?;
            let fail = self.block(f, "recv.fail");
            let done = self.block(f, "recv.done");
            self.b.build_conditional_branch(failed, fail, done).map_err(llvm_err)?;
            self.b.position_at_end(fail);
            self.b.build_return(Some(&out_code)).map_err(llvm_err)?;
            self.b.position_at_end(done);
            let count = self.b.build_int_truncate(out_code, self.usize_ty, "count").map_err(llvm_err)?;
            let next = self.b.build_int_add(len, count, "next").map_err(llvm_err)?;
            self.b.build_store(len_field, next).map_err(llvm_err)?;
            self.b.build_return(Some(&out_code)).map_err(llvm_err)?;
        }

        // `net_close(fd)`.
        let f = self.function("burdin_rt_net_close", i64_ty.fn_type(&[i64_ty.into()], false));
        let fd = Self::int_param(f, 0);
        // The runtime already declares the Unix `close` for the file primitives, with an `int` descriptor; the
        // socket descriptor is truncated for it. Windows closes a socket with its own call.
        let raw = if self.windows {
            let close = libc(self.module, "closesocket", i32_ty.fn_type(&[i64_ty.into()], false));
            self.call(close, &[fd.into()])?.expect("closesocket returns a code").into_int_value()
        } else {
            let close = libc(self.module, "close", i32_ty.fn_type(&[i32_ty.into()], false));
            let small = self.b.build_int_truncate(fd, i32_ty, "fd32").map_err(llvm_err)?;
            self.call(close, &[small.into()])?.expect("close returns a code").into_int_value()
        };
        let out = self.net_result(raw)?;
        self.b.build_return(Some(&out)).map_err(llvm_err)?;

        // `net_shutdown(fd, how)`: 1 is the read side, 2 the write side, and 3 both.
        let f = self.function("burdin_rt_net_shutdown", i64_ty.fn_type(&[i64_ty.into(), i32_ty.into()], false));
        let (fd, how) = (Self::int_param(f, 0), Self::int_param(f, 1));
        let shut = self
            .b
            .build_select(
                self.b.build_int_compare(IntPredicate::EQ, how, i32_ty.const_int(1, false), "").map_err(llvm_err)?,
                i32_ty.const_zero(),
                self.b
                    .build_select(
                        self.b
                            .build_int_compare(IntPredicate::EQ, how, i32_ty.const_int(2, false), "")
                            .map_err(llvm_err)?,
                        i32_ty.const_int(1, false),
                        i32_ty.const_int(2, false),
                        "",
                    )
                    .map_err(llvm_err)?
                    .into_int_value(),
                "shut.how",
            )
            .map_err(llvm_err)?
            .into_int_value();
        let shutdown = libc(self.module, "shutdown", i32_ty.fn_type(&[i64_ty.into(), i32_ty.into()], false));
        let raw = self.call(shutdown, &[fd.into(), shut.into()])?.expect("shutdown returns a code").into_int_value();
        let out = self.net_result(raw)?;
        self.b.build_return(Some(&out)).map_err(llvm_err)?;

        // `net_error(fd)`: `SO_ERROR`, the error a completed non-blocking connect reports.
        let f = self.function("burdin_rt_net_error", i32_ty.fn_type(&[i64_ty.into()], false));
        let fd = Self::int_param(f, 0);
        let value = self.b.build_alloca(i32_ty, "so.error").map_err(llvm_err)?;
        let length = self.b.build_alloca(i32_ty, "so.len").map_err(llvm_err)?;
        self.b.build_store(value, i32_ty.const_zero()).map_err(llvm_err)?;
        self.b.build_store(length, i32_ty.const_int(4, false)).map_err(llvm_err)?;
        let getsockopt = libc(
            self.module,
            "getsockopt",
            i32_ty.fn_type(&[i64_ty.into(), i32_ty.into(), i32_ty.into(), ptr_ty.into(), ptr_ty.into()], false),
        );
        self.call(
            getsockopt,
            &[
                fd.into(),
                i32_ty.const_int(u64::from(SOL_SOCKET as u32), false).into(),
                i32_ty.const_int(u64::from(SO_ERROR as u32), false).into(),
                value.into(),
                length.into(),
            ],
        )?;
        let code = self.b.build_load(i32_ty, value, "so.error").map_err(llvm_err)?.into_int_value();
        self.b.build_return(Some(&code)).map_err(llvm_err)?;

        // `net_set_nodelay(fd, on)`.
        let f = self.function("burdin_rt_net_set_nodelay", i64_ty.fn_type(&[i64_ty.into(), i32_ty.into()], false));
        let (fd, on) = (Self::int_param(f, 0), Self::int_param(f, 1));
        let setsockopt = libc(
            self.module,
            "setsockopt",
            i32_ty.fn_type(&[i64_ty.into(), i32_ty.into(), i32_ty.into(), ptr_ty.into(), i32_ty.into()], false),
        );
        let option = self.b.build_alloca(i32_ty, "nodelay").map_err(llvm_err)?;
        self.b.build_store(option, on).map_err(llvm_err)?;
        let raw = self
            .call(
                setsockopt,
                &[
                    fd.into(),
                    i32_ty.const_int(u64::from(IPPROTO_TCP as u32), false).into(),
                    i32_ty.const_int(u64::from(TCP_NODELAY as u32), false).into(),
                    option.into(),
                    i32_ty.const_int(4, false).into(),
                ],
            )?
            .expect("setsockopt returns a code")
            .into_int_value();
        let out = self.net_result(raw)?;
        self.b.build_return(Some(&out)).map_err(llvm_err)?;

        // `net_local_addr(fd)` and `net_peer_addr(fd)`: the packed address, or -1.
        for (name, peer) in [("burdin_rt_net_local_addr", false), ("burdin_rt_net_peer_addr", true)] {
            let f = self.function(name, i64_ty.fn_type(&[i64_ty.into()], false));
            let fd = Self::int_param(f, 0);
            let addr = self.b.build_alloca(self.i8().array_type(SOCKADDR_SIZE as u32), "addr").map_err(llvm_err)?;
            let length = self.b.build_alloca(i32_ty, "addr.len").map_err(llvm_err)?;
            self.b.build_store(length, i32_ty.const_int(SOCKADDR_SIZE, false)).map_err(llvm_err)?;
            let call_name = if peer { "getpeername" } else { "getsockname" };
            let get =
                libc(self.module, call_name, i32_ty.fn_type(&[i64_ty.into(), ptr_ty.into(), ptr_ty.into()], false));
            let raw = self
                .call(get, &[fd.into(), addr.into(), length.into()])?
                .expect("the address call returns a code")
                .into_int_value();
            let bad = self
                .b
                .build_int_compare(IntPredicate::SLT, raw, i32_ty.const_zero(), "addr.failed")
                .map_err(llvm_err)?;
            let fail = self.block(f, "addr.fail");
            let ok = self.block(f, "addr.ok");
            self.b.build_conditional_branch(bad, fail, ok).map_err(llvm_err)?;
            self.b.position_at_end(fail);
            self.b.build_return(Some(&i64_ty.const_int(u64::MAX, true))).map_err(llvm_err)?;
            self.b.position_at_end(ok);
            let packed = self.pack_address(addr)?;
            self.b.build_return(Some(&packed)).map_err(llvm_err)?;
        }

        // `net_last_peer()` and `net_last_error()`.
        let _f = self.function("burdin_rt_net_last_peer", i64_ty.fn_type(&[], false));
        let slot = self.module.get_global("burdin_rt_net_peer").expect("the peer slot exists");
        let peer = self.b.build_load(i64_ty, slot.as_pointer_value(), "peer").map_err(llvm_err)?.into_int_value();
        self.b.build_return(Some(&peer)).map_err(llvm_err)?;
        let _f = self.function("burdin_rt_net_last_error", i32_ty.fn_type(&[], false));
        let slot = self.module.get_global("burdin_rt_net_err").expect("the socket error slot exists");
        let code = self.b.build_load(i32_ty, slot.as_pointer_value(), "net.err").map_err(llvm_err)?.into_int_value();
        self.b.build_return(Some(&code)).map_err(llvm_err)?;

        // `dns_lookup(host, len, out: *String) -> i32`: one `a.b.c.d` line per address, `0` or the resolver's
        // code. The caller runs it on the helper pool, so a slow resolver never holds a scheduler worker.
        let f = self
            .function("burdin_rt_dns_lookup", i32_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), ptr_ty.into()], false));
        let (host, len, out) = (Self::ptr_param(f, 0), Self::int_param(f, 1), Self::ptr_param(f, 2));
        self.net_startup()?;
        let hints = self.b.build_alloca(self.i8().array_type(ADDRINFO_SIZE as u32), "dns.hints").map_err(llvm_err)?;
        self.b
            .build_memset(hints, 8, self.i8().const_zero(), self.usize_ty.const_int(ADDRINFO_SIZE, false))
            .map_err(llvm_err)?;
        let family = self.offset(hints, self.usize_ty.const_int(ADDRINFO_FAMILY_OFFSET, false))?;
        self.b.build_store(family, i32_ty.const_int(u64::from(AF_INET as u32), false)).map_err(llvm_err)?;
        let node = self.cstring_of(host, len, malloc)?;
        let head = self.b.build_alloca(ptr_ty, "dns.head").map_err(llvm_err)?;
        let cursor = self.b.build_alloca(ptr_ty, "dns.cursor").map_err(llvm_err)?;
        self.b.build_store(head, ptr_ty.const_null()).map_err(llvm_err)?;
        let getaddrinfo = libc(
            self.module,
            "getaddrinfo",
            i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into(), ptr_ty.into(), ptr_ty.into()], false),
        );
        let code = self
            .call(getaddrinfo, &[node.into(), ptr_ty.const_null().into(), hints.into(), head.into()])?
            .expect("getaddrinfo returns a code")
            .into_int_value();
        self.call(free, &[node.into()])?;
        let freeaddrinfo = libc(self.module, "freeaddrinfo", void_ty.fn_type(&[ptr_ty.into()], false));
        let list = self.b.build_load(ptr_ty, head, "dns.list").map_err(llvm_err)?.into_pointer_value();
        self.b.build_store(cursor, list).map_err(llvm_err)?;
        let bad = self.b.build_int_compare(IntPredicate::NE, code, i32_ty.const_zero(), "dns.bad").map_err(llvm_err)?;
        let fail = self.block(f, "dns.fail");
        let walk = self.block(f, "dns.walk");
        let next = self.block(f, "dns.next");
        let done = self.block(f, "dns.done");
        self.b.build_conditional_branch(bad, fail, walk).map_err(llvm_err)?;
        self.b.position_at_end(fail);
        self.b.build_return(Some(&code)).map_err(llvm_err)?;
        self.b.position_at_end(walk);
        let entry = self.b.build_load(ptr_ty, cursor, "dns.entry").map_err(llvm_err)?.into_pointer_value();
        let at_end = self.b.build_is_null(entry, "dns.end").map_err(llvm_err)?;
        self.b.build_conditional_branch(at_end, done, next).map_err(llvm_err)?;
        self.b.position_at_end(next);
        let addr_at = self.offset(entry, self.usize_ty.const_int(ADDRINFO_ADDR_OFFSET, false))?;
        let addr = self.b.build_load(ptr_ty, addr_at, "dns.addr").map_err(llvm_err)?.into_pointer_value();
        // `inet_ntop` takes the 4-byte address, not the whole `sockaddr_in`. Winsock's `InetNtopA` is a macro for
        // it, and `ws2_32.lib` exports the `inet_ntop` symbol, so the same name works on both platforms.
        let in_addr = self.offset(addr, self.usize_ty.const_int(SOCKADDR_ADDR_OFFSET, false))?;
        let text = self.b.build_alloca(self.i8().array_type(16), "dns.text").map_err(llvm_err)?;
        let inet = libc(
            self.module,
            "inet_ntop",
            ptr_ty.fn_type(&[i32_ty.into(), ptr_ty.into(), ptr_ty.into(), i32_ty.into()], false),
        );
        self.call(
            inet,
            &[
                i32_ty.const_int(u64::from(AF_INET as u32), false).into(),
                in_addr.into(),
                text.into(),
                i32_ty.const_int(16, false).into(),
            ],
        )?;
        let strlen = libc(self.module, "strlen", self.usize_ty.fn_type(&[ptr_ty.into()], false));
        let text_len = self.call(strlen, &[text.into()])?.expect("strlen returns a size").into_int_value();
        self.call(string_push_str, &[out.into(), text.into(), text_len.into()])?;
        self.call(string_push_char, &[out.into(), i32_ty.const_int(10, false).into()])?;
        let next_at = self.offset(entry, self.usize_ty.const_int(ADDRINFO_NEXT_OFFSET, false))?;
        let following = self.b.build_load(ptr_ty, next_at, "dns.next").map_err(llvm_err)?.into_pointer_value();
        self.b.build_store(cursor, following).map_err(llvm_err)?;
        self.b.build_unconditional_branch(walk).map_err(llvm_err)?;
        self.b.position_at_end(done);
        self.call(freeaddrinfo, &[list.into()])?;
        self.b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;
        Ok(())
    }

    /// The packed form of a `sockaddr_in`: bits 0-31 the address in big-endian order (`a<<24 | b<<16 | c<<8 | d`)
    /// and bits 32-63 the port.
    fn pack_address(&mut self, addr: PointerValue<'ctx>) -> GenResult<IntValue<'ctx>> {
        let i8_ty = self.i8();
        let i64_ty = self.i64();
        let mut value = i64_ty.const_zero();
        for i in 0..4u64 {
            let at = self.offset(addr, self.usize_ty.const_int(SOCKADDR_ADDR_OFFSET + i, false))?;
            let byte = self.b.build_load(i8_ty, at, "").map_err(llvm_err)?.into_int_value();
            let byte = self.b.build_int_z_extend(byte, i64_ty, "").map_err(llvm_err)?;
            let shift = i64_ty.const_int((3 - i) * 8, false);
            let shifted = self.b.build_left_shift(byte, shift, "").map_err(llvm_err)?;
            value = self.b.build_or(value, shifted, "").map_err(llvm_err)?;
        }
        let high_at = self.offset(addr, self.usize_ty.const_int(SOCKADDR_PORT_OFFSET, false))?;
        let low_at = self.offset(addr, self.usize_ty.const_int(SOCKADDR_PORT_OFFSET + 1, false))?;
        let high = self.b.build_load(i8_ty, high_at, "").map_err(llvm_err)?.into_int_value();
        let low = self.b.build_load(i8_ty, low_at, "").map_err(llvm_err)?.into_int_value();
        let high = self.b.build_int_z_extend(high, i64_ty, "").map_err(llvm_err)?;
        let low = self.b.build_int_z_extend(low, i64_ty, "").map_err(llvm_err)?;
        let port = self
            .b
            .build_int_add(self.b.build_left_shift(high, i64_ty.const_int(8, false), "").map_err(llvm_err)?, low, "")
            .map_err(llvm_err)?;
        self.b
            .build_int_add(value, self.b.build_left_shift(port, i64_ty.const_int(32, false), "").map_err(llvm_err)?, "")
            .map_err(llvm_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Darwin's arm64 ABI passes unnamed arguments on the stack, so `open`'s mode argument only reaches the C
    /// library when the declaration is variadic. The Windows CRT's `_open` is variadic too.
    #[test]
    fn file_open_is_declared_variadic() {
        for windows in [false, true] {
            let cx = Context::create();
            let module = cx.create_module("runtime");
            let ptr = cx.ptr_type(AddressSpace::default());
            let usize_ty = cx.i64_type();
            let string_ty = cx.struct_type(&[ptr.into(), usize_ty.into(), usize_ty.into()], false);
            generate(&cx, &module, usize_ty, string_ty, windows, false, false).expect("the runtime is generated");
            let symbol = if windows { "_open" } else { "open" };
            let open = module.get_function(symbol).unwrap_or_else(|| panic!("{symbol} is declared"));
            assert!(open.get_type().is_var_arg(), "{symbol} must be variadic to pass the create mode");
        }
    }

    /// The open flags differ on every platform, and only the host's can be exercised. The values come from the
    /// systems' C headers: Linux's `O_*`, the different macOS `O_*`, and the Windows CRT's `_O_*`.
    #[test]
    fn open_flags_match_each_platform() {
        assert_eq!(
            open_flag_constants(false, false),
            OpenFlagConstants {
                read_only: 0,
                write_only: 1,
                read_write: 2,
                append: 0o2000,
                truncate: 0o1000,
                create: 0o100,
                exclusive: 0o200,
                binary: 0,
            },
            "Linux"
        );
        assert_eq!(
            open_flag_constants(false, true),
            OpenFlagConstants {
                read_only: 0,
                write_only: 1,
                read_write: 2,
                append: 0x0008,
                truncate: 0x0400,
                create: 0x0200,
                exclusive: 0x0800,
                binary: 0,
            },
            "macOS"
        );
        assert_eq!(
            open_flag_constants(true, false),
            OpenFlagConstants {
                read_only: 0x0000,
                write_only: 0x0001,
                read_write: 0x0002,
                append: 0x0008,
                truncate: 0x0200,
                create: 0x0100,
                exclusive: 0x0400,
                binary: 0x8000,
            },
            "the Windows CRT"
        );
        assert_eq!(open_create_mode(false), 0o666, "Unix honors the umask over these bits");
        assert_eq!(open_create_mode(true), 0o600, "the CRT takes only _S_IREAD and _S_IWRITE");
    }

    /// The Windows structures the runtime fills by byte offset, mirrored so a wrong offset fails on any host.
    #[repr(C)]
    struct WindowsSecurityAttributes {
        length: u32,
        descriptor: *mut std::ffi::c_void,
        inherit: i32,
    }

    #[repr(C)]
    struct WindowsStartupInfoW {
        cb: u32,
        reserved: *mut std::ffi::c_void,
        desktop: *mut std::ffi::c_void,
        title: *mut std::ffi::c_void,
        x: u32,
        y: u32,
        x_size: u32,
        y_size: u32,
        x_chars: u32,
        y_chars: u32,
        fill: u32,
        flags: u32,
        show: u16,
        reserved2: u16,
        reserved2_ptr: *mut u8,
        std_input: *mut std::ffi::c_void,
        std_output: *mut std::ffi::c_void,
        std_error: *mut std::ffi::c_void,
    }

    #[repr(C)]
    struct WindowsProcessInformation {
        process: *mut std::ffi::c_void,
        thread: *mut std::ffi::c_void,
        pid: u32,
        thread_id: u32,
    }

    /// `CreateProcessW` receives `SECURITY_ATTRIBUTES`, `STARTUPINFOW`, and `PROCESS_INFORMATION` through
    /// raw buffers whose fields the runtime writes at fixed offsets, so the sizes and offsets are checked on the
    /// host even though only Windows executes the spawn path.
    #[test]
    fn windows_spawn_structures_match_the_runtime_offsets() {
        use std::mem::{offset_of, size_of};
        assert_eq!(size_of::<WindowsSecurityAttributes>(), 24);
        assert_eq!(offset_of!(WindowsSecurityAttributes, descriptor), 8);
        assert_eq!(offset_of!(WindowsSecurityAttributes, inherit), 16);
        assert_eq!(size_of::<WindowsStartupInfoW>(), 104);
        assert_eq!(offset_of!(WindowsStartupInfoW, flags), 60);
        assert_eq!(offset_of!(WindowsStartupInfoW, std_input), 80);
        assert_eq!(offset_of!(WindowsStartupInfoW, std_output), 88);
        assert_eq!(offset_of!(WindowsStartupInfoW, std_error), 96);
        assert_eq!(size_of::<WindowsProcessInformation>(), 24);
        assert_eq!(offset_of!(WindowsProcessInformation, process), 0);
        assert_eq!(offset_of!(WindowsProcessInformation, thread), 8);
        assert_eq!(offset_of!(WindowsProcessInformation, pid), 16);
    }

    /// The runtime must verify on both platform paths. The environment and working-directory functions are the
    /// only part of the runtime with two implementations, and the suite can execute only the host's, so this
    /// checks the IR of the other one too.
    #[test]
    fn the_runtime_verifies_on_every_platform_path() {
        for windows in [false, true] {
            for checking in [false, true] {
                let cx = Context::create();
                let module = cx.create_module("runtime");
                let ptr = cx.ptr_type(AddressSpace::default());
                let usize_ty = cx.i64_type();
                let string_ty = cx.struct_type(&[ptr.into(), usize_ty.into(), usize_ty.into()], false);
                generate(&cx, &module, usize_ty, string_ty, windows, false, checking)
                    .unwrap_or_else(|e| panic!("generating the runtime (windows={windows}) failed: {e}"));
                module.verify().unwrap_or_else(|e| {
                    panic!("the runtime (windows={windows}, checking={checking}) does not verify: {e}")
                });
            }
        }
    }

    #[cfg(unix)]
    use std::ffi::CStr;
    use std::ffi::{CString, c_char, c_int, c_void};

    /// The host's own `stat`/`lstat`, with the same symbol the runtime calls. The test reads the fields the
    /// runtime reads through `STAT_*_OFFSET`, so a wrong layout fails on the host that has it.
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    unsafe extern "C" {
        #[link_name = "lstat$INODE64"]
        fn host_lstat(path: *const c_char, buf: *mut c_void) -> c_int;
    }
    #[cfg(all(unix, not(all(target_os = "macos", target_arch = "x86_64"))))]
    unsafe extern "C" {
        #[link_name = "lstat"]
        fn host_lstat(path: *const c_char, buf: *mut c_void) -> c_int;
    }
    #[cfg(windows)]
    unsafe extern "C" {
        #[link_name = "_stat64"]
        fn host_lstat(path: *const c_char, buf: *mut c_void) -> c_int;
    }

    fn stat_probe(path: &std::path::Path) -> [i64; STAT_WORDS as usize] {
        let cpath = CString::new(path.to_str().expect("a UTF-8 temporary path")).expect("no NUL in the path");
        let mut buf = [0i64; STAT_WORDS as usize];
        // SAFETY: `buf` is `size_of::<libc::stat>()` bytes rounded up to whole words and 8-byte aligned, which
        // is what the C function writes; the path is NUL-terminated.
        let status = unsafe { host_lstat(cpath.as_ptr(), buf.as_mut_ptr().cast()) };
        assert_eq!(status, 0, "stat of {}", path.display());
        buf
    }

    fn field(buf: &[i64], offset: u64, bytes: u32) -> i64 {
        // SAFETY: every offset this module reads is inside the buffer (checked by `stat_layout_fits_the_buffer`),
        // and a `stat` result is a plain sequence of integers.
        unsafe {
            let at = (buf.as_ptr() as *const u8).add(offset as usize);
            match bytes {
                2 => i64::from(*(at as *const i16)),
                4 => i64::from(*(at as *const i32)),
                _ => *(at as *const i64),
            }
        }
    }

    #[test]
    fn stat_layout_fits_the_buffer() {
        let bytes = u64::from(STAT_WORDS) * 8;
        assert!(STAT_SIZE_OFFSET + 8 <= bytes, "st_size is inside the buffer");
        assert!(STAT_MODE_OFFSET + u64::from(STAT_MODE_BYTES) <= bytes, "st_mode is inside the buffer");
        assert!(STAT_ATIME_OFFSET + 8 <= bytes, "st_atime is inside the buffer");
        assert!(STAT_MTIME_OFFSET + 8 <= bytes, "st_mtime is inside the buffer");
        assert!(u64::from(STAT_WORDS) * 8 >= std::mem::size_of::<libc::stat>() as u64);
    }

    #[test]
    fn stat_layout_matches_the_host() {
        let dir = std::env::temp_dir().join(format!("burdin-stat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        let path = dir.join("probe.txt");
        std::fs::write(&path, b"12345").expect("a probe file");
        let buf = stat_probe(&path);
        assert_eq!(field(&buf, STAT_SIZE_OFFSET, 8), 5, "st_size is the file's length");
        let meta = std::fs::metadata(&path).expect("the probe file's metadata");
        assert_eq!(field(&buf, STAT_SIZE_OFFSET, 8) as u64, meta.len());
        let modified = meta.modified().expect("a modification time");
        let seconds = modified.duration_since(std::time::UNIX_EPOCH).expect("after the epoch").as_secs() as i64;
        assert_eq!(field(&buf, STAT_MTIME_OFFSET, 8), seconds, "st_mtime is in seconds since the epoch");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = field(&buf, STAT_MODE_OFFSET, STAT_MODE_BYTES) as u32;
            assert_eq!(mode & 0o7777, meta.permissions().mode() & 0o7777, "st_mode is the file's mode");
            let kind = mode & 0o170000;
            assert_eq!(kind, 0o100000, "a regular file");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Reading a directory through `readdir` at `DIRENT_NAME_OFFSET` yields the entry names. The listing order is
    /// the system's, so the test creates a known entry and checks the names it reads include `.`, `..`, and that
    /// entry; a wrong offset reads garbage instead.
    #[cfg(unix)]
    #[test]
    fn dirent_name_offset_matches_the_host() {
        unsafe extern "C" {
            fn opendir(path: *const c_char) -> *mut c_void;
            fn readdir(dir: *mut c_void) -> *mut c_void;
            fn closedir(dir: *mut c_void) -> c_int;
        }
        let dir = std::env::temp_dir().join(format!("burdin-dirent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        std::fs::write(dir.join("probe-name.txt"), b"x").expect("a probe file");
        let cpath = CString::new(dir.to_str().expect("a UTF-8 temporary path")).expect("no NUL in the path");
        let mut names = Vec::new();
        // SAFETY: `opendir` gets a NUL-terminated path, `readdir` returns a `struct dirent` whose `d_name` this
        // module reads at the host's offset, and the stream is closed once.
        unsafe {
            let stream = opendir(cpath.as_ptr());
            assert!(!stream.is_null(), "opendir({})", dir.display());
            loop {
                let entry = readdir(stream);
                if entry.is_null() {
                    break;
                }
                let name = CStr::from_ptr((entry as *const u8).add(DIRENT_NAME_OFFSET as usize) as *const c_char);
                names.push(name.to_string_lossy().into_owned());
            }
            closedir(stream);
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(names.iter().any(|name| name == "."), "the listing has `.`: {names:?}");
        assert!(names.iter().any(|name| name == ".."), "the listing has `..`: {names:?}");
        assert!(names.iter().any(|name| name == "probe-name.txt"), "the entry name is read: {names:?}");
    }

    /// The Windows filter runs on what is left of the main stack, after the exception dispatcher has used part of
    /// it. The system's default guarantee leaves too little for the flush and the write, so the guard reserves a
    /// large one before it registers the filter; without it the process can die without its message (which made
    /// `m02_basics/stack_overflow_aborts` flaky on Windows).
    #[test]
    fn the_windows_stack_guard_reserves_room_for_its_handler() {
        let cx = Context::create();
        let module = cx.create_module("runtime");
        let ptr_ty = cx.ptr_type(AddressSpace::default());
        let usize_ty = cx.i64_type();
        let string_ty = cx.struct_type(&[ptr_ty.into(), usize_ty.into(), usize_ty.into()], false);
        generate(&cx, &module, usize_ty, string_ty, true, false, false).expect("the runtime generates");
        let ir = module.print_to_string().to_string();
        let guarantee =
            ir.find("call i32 @SetThreadStackGuarantee").expect("the Windows guard reserves a stack guarantee");
        let filter = ir.find("call ptr @AddVectoredExceptionHandler").expect("the Windows guard installs its filter");
        assert!(guarantee < filter, "the guarantee is reserved before the filter is installed:\n{ir}");
        assert!(
            ir.contains(&format!("store i32 {STACK_GUARANTEE}, ptr")),
            "the guarantee is {STACK_GUARANTEE} bytes:\n{ir}"
        );
    }
}
