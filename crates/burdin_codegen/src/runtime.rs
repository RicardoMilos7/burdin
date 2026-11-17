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
//! the message line keeps its stage 0 format. The runtime frames above the panicking function are skipped. On Unix
//! the trace uses libc's `backtrace`/`backtrace_symbols_fd` and needs the `.eh_frame` unwind tables the code
//! generator adds to every function; on Windows it uses `RtlCaptureStackBackTrace` (addresses only, because symbolizing
//! needs a PDB the driver does not produce).
//!
//! **Stack overflow.** The main thread gets an alternate signal stack and a `SIGSEGV` handler on Unix (an
//! `AddVectoredExceptionHandler` filter for `EXCEPTION_STACK_OVERFLOW` on Windows), so a deep recursion ends with
//! `panic: stack overflow` instead of a raw fault.
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
use inkwell::{AddressSpace, IntPredicate};

/// The standard-output buffer size. A terminal is line buffered, so the size only matters for pipes and files,
/// where larger writes are what make printing a lot of output cheap.
const OUT_BUF: u32 = 8192;
/// The standard-input buffer size. Reads take whole blocks, so a program that reads line by line does not call
/// the operating system once per line.
const IN_BUF: u32 = 4096;
/// The number of frames a panic backtrace captures.
const BACKTRACE_FRAMES: u32 = 64;
/// The size of the alternate signal stack, so a `SIGSEGV` handler still runs after the main stack is gone.
const ALT_STACK: u32 = 64 * 1024;
/// The checking allocator's header: `{ next, size, site, site_len, magic }`.
const HEADER: u64 = 40;
/// The magic word that marks a live checking-allocator block.
const MAGIC: u64 = 0x4255_5244_4e41_4c4c;

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
/// `WIN32_FIND_DATAA` is 320 bytes with its 4-byte alignment.
const FIND_DATA_WORDS: u32 = 40;
/// `FILE_ATTRIBUTE_REPARSE_POINT`, the bit that marks a link or junction on Windows.
const FILE_ATTRIBUTE_REPARSE_POINT: u64 = 0x400;
/// `INVALID_FILE_ATTRIBUTES`, what `GetFileAttributesA` returns on failure.
const INVALID_FILE_ATTRIBUTES: u64 = 0xFFFF_FFFF;
/// `MOVEFILE_REPLACE_EXISTING`: `MoveFileExA` replaces an existing destination, as POSIX `rename` does.
const MOVEFILE_REPLACE_EXISTING: u64 = 1;
/// The `S_IFMT` mask and the file, directory, and link kinds it selects (`_S_IFMT` and the `_S_IF*` values on
/// Windows agree with the POSIX ones for these three kinds).
const S_IFMT: u64 = 0o170000;
const S_IFREG: u64 = 0o100000;
const S_IFDIR: u64 = 0o040000;
const S_IFLNK: u64 = 0o120000;

/// The runtime functions the generated code calls.
pub(crate) struct Runtime<'ctx> {
    /// `(fd: i32, ptr, len: usize, newline: i32)`
    pub print: FunctionValue<'ctx>,
    /// `()`: writes the standard-output buffer to fd 1 and empties it.
    pub flush: FunctionValue<'ctx>,
    /// `(fd: i32) -> i1`: whether the descriptor is a terminal.
    pub is_terminal: FunctionValue<'ctx>,
    /// `(out: *String) -> i32`: reads one line of standard input; 0 is a line (or the last one), 1 is the end
    /// of input with nothing read, and -1 is an error.
    pub in_read_line: FunctionValue<'ctx>,
    /// `(out: *String) -> i32`: reads the rest of standard input; 0 is success and -1 an error.
    pub in_read_to_string: FunctionValue<'ctx>,
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
    /// `(ptr, len, out: *String) -> i1`: reads a whole file.
    pub read_to_string: FunctionValue<'ctx>,
    /// `(path, path_len, text, text_len) -> i1`: writes a whole file.
    pub write_file: FunctionValue<'ctx>,
    /// `(path, path_len, out: *List<String>) -> i32`: the names in a directory, or -1.
    pub read_dir: FunctionValue<'ctx>,
    /// `(path, path_len) -> i32`: creates one directory, or -1.
    pub create_dir: FunctionValue<'ctx>,
    /// `(path, path_len) -> i32`: removes an empty directory, or -1.
    pub remove_dir: FunctionValue<'ctx>,
    /// `(path, path_len) -> i32`: removes a file or a symbolic link, or -1.
    pub remove_file: FunctionValue<'ctx>,
    /// `(from, from_len, to, to_len) -> i32`: renames a file or a directory, replacing `to`; -1 on failure.
    pub rename: FunctionValue<'ctx>,
    /// `(path, path_len, out: *RawMetadata) -> i32`: the entry's size, mode, times, and kind, or -1.
    pub metadata: FunctionValue<'ctx>,
    /// `(path, path_len, out: *String) -> i32`: the absolute, normalized path, or -1.
    pub canonicalize: FunctionValue<'ctx>,
    /// `(out: *List<String>)`: the process arguments, program name first.
    pub args: FunctionValue<'ctx>,
    /// `(code: i32)`: ends the process.
    pub exit: FunctionValue<'ctx>,
    /// `(command, command_len) -> i32`: runs a shell command; -1 on failure, the exit code otherwise.
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
    if let Some(flush) = module.get_function("burdin_rt_flush") {
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
        }
    }

    fn generate(&mut self) -> GenResult<Runtime<'ctx>> {
        if self.checking {
            let live = self.module.add_global(self.ptr_ty, None, "burdin_rt_live");
            live.set_initializer(&self.ptr_ty.const_null());
            live.set_linkage(Linkage::Internal);
        }
        let h0 = self.base_helpers()?;
        let alloc = self.heap_alloc(&h0)?;
        let free = self.heap_free(&h0)?;
        let realloc = self.heap_realloc(&h0, alloc, free)?;
        self.fn_alloc = Some(alloc);
        self.fn_free = Some(free);
        self.fn_realloc = Some(realloc);
        let leak_check = if self.checking { Some(self.leak_check(&h0)?) } else { None };
        let alloc_string = self.alloc_string(&h0)?;
        let h = h0.with_alloc_string(alloc_string);
        let string_push_str = self.string_push_str(&h)?;
        let cstring_from = self.cstring_from(&h)?;
        // The arguments `define_main` receives, saved for `env.args()`.
        let argc_ty = self.i32();
        let argc = self.module.add_global(argc_ty, None, "burdin_rt_argc");
        argc.set_initializer(&argc_ty.const_zero());
        argc.set_linkage(Linkage::Internal);
        let argv = self.module.add_global(self.ptr_ty, None, "burdin_rt_argv");
        argv.set_initializer(&self.ptr_ty.const_null());
        argv.set_linkage(Linkage::Internal);
        // The standard-input block buffer, shared by every read of standard input.
        let in_fill = self.in_fill()?;
        Ok(Runtime {
            print: self.print(&h)?,
            flush: h.flush,
            is_terminal: self.is_terminal()?,
            in_read_line: self.in_read_line(string_push_str, in_fill)?,
            in_read_to_string: self.in_read_to_string(string_push_str, in_fill)?,
            panic: self.panic(&h)?,
            panic_bounds: self.panic_bounds(&h)?,
            string_free: self.string_free()?,
            string_new: self.string_new()?,
            string_clone: self.string_clone(&h)?,
            string_push_str,
            alloc,
            free,
            leak_check,
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
            string_push_char: self.string_push_char(string_push_str)?,
            str_eq: self.str_eq()?,
            str_cmp: self.str_cmp()?,
            str_hash: self.str_hash()?,
            str_find_str: self.str_find_str()?,
            parse_i64: self.parse_int(&h, true)?,
            parse_u64: self.parse_int(&h, false)?,
            read_to_string: self.read_to_string(&h, alloc_string, string_push_str)?,
            write_file: self.write_file(&h)?,
            read_dir: self.read_dir(&h)?,
            create_dir: self.create_dir()?,
            remove_dir: self.remove_dir()?,
            remove_file: self.remove_file()?,
            rename: self.rename()?,
            metadata: self.metadata()?,
            canonicalize: self.canonicalize(&h)?,
            args: self.args(argc.as_pointer_value(), argv.as_pointer_value(), alloc_string)?,
            exit: self.exit()?,
            run: self.run_command()?,
        })
    }

    fn base_helpers(&mut self) -> GenResult<BaseHelpers<'ctx>> {
        let write_all = self.write_all()?;
        let globals = self.output_globals();
        let flush = self.flush(write_all, globals)?;
        let _out_write = self.out_write(flush, globals)?;
        let die = self.die(flush)?;
        self.install_stack_guard(flush)?;
        let digits = self.digits()?;
        let write_u64 = self.write_u64(write_all, digits)?;
        let hex_digits = self.hex_digits()?;
        self.write_hex(write_all, hex_digits)?;
        let print_backtrace = self.print_backtrace()?;
        let panic_start = self.panic_start(write_all, write_u64, flush)?;
        Ok(BaseHelpers { write_all, flush, die, write_u64, digits, panic_start, print_backtrace })
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

        self.b.position_at_end(stdout);
        self.call(out_write, &[Self::ptr_param(f, 1).into(), Self::int_param(f, 2).into()])?;
        let out_nl = self.block(f, "stdout.newline");
        let out_flush = self.block(f, "stdout.flush");
        let nl = self
            .b
            .build_int_compare(IntPredicate::NE, Self::int_param(f, 3), i32_ty.const_zero(), "")
            .map_err(llvm_err)?;
        self.b.build_conditional_branch(nl, out_nl, done).map_err(llvm_err)?;
        self.b.position_at_end(out_nl);
        let nl_ptr = self.bytes(b"\n");
        self.call(out_write, &[nl_ptr.into(), usize_ty.const_int(1, false).into()])?;
        let (_, _, line_global) = self.output_globals();
        let line = self.b.build_load(self.cx.bool_type(), line_global, "line").map_err(llvm_err)?.into_int_value();
        self.b.build_conditional_branch(line, out_flush, done).map_err(llvm_err)?;
        self.b.position_at_end(out_flush);
        self.call(h.flush, &[])?;
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

    /// `in_fill() -> i32`: makes the input block hold at least one byte. 1 means data, 0 the end of input, and
    /// -1 a read error.
    fn in_fill(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (buf, pos, len) = self.in_globals();
        let (i32_ty, usize_ty, ptr_ty) = (self.i32(), self.usize_ty, self.ptr_ty);
        let f = self.function("burdin_rt_in_fill", i32_ty.fn_type(&[], false));
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
        self.b.build_return(Some(&i32_ty.const_int(0xFFFF_FFFF, false))).map_err(llvm_err)?;
        Ok(f)
    }

    /// `in_read_line(out: *String) -> i32`: appends one line of standard input to `out`, without its newline
    /// (and without the `\r` of a `\r\n` pair). 0 is a line (the last one may be unterminated), 1 is the end of
    /// input with nothing read, and -1 is a read error.
    fn in_read_line(&mut self, push: FunctionValue<'ctx>, fill: FunctionValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (buf, pos, len) = self.in_globals();
        let (i32_ty, usize_ty, ptr_ty, i8_ty) = (self.i32(), self.usize_ty, self.ptr_ty, self.i8());
        let f = self.function("burdin_rt_in_read_line", i32_ty.fn_type(&[ptr_ty.into()], false));
        let out = Self::ptr_param(f, 0);
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
        let r = self.call(fill, &[])?.expect("fill returns a status").into_int_value();
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

    /// `in_read_to_string(out: *String) -> i32`: appends the rest of standard input to `out`. 0 is success and
    /// -1 a read error.
    fn in_read_to_string(
        &mut self,
        push: FunctionValue<'ctx>,
        fill: FunctionValue<'ctx>,
    ) -> GenResult<FunctionValue<'ctx>> {
        let (buf, pos, len) = self.in_globals();
        let (i32_ty, usize_ty, ptr_ty) = (self.i32(), self.usize_ty, self.ptr_ty);
        let f = self.function("burdin_rt_in_read_to_string", i32_ty.fn_type(&[ptr_ty.into()], false));
        let out = Self::ptr_param(f, 0);
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
        let r = self.call(fill, &[])?.expect("fill returns a status").into_int_value();
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

    /// `print_backtrace()`: writes `stack backtrace:` and the frames above the panic runtime to stderr. The first
    /// two frames (this function and `burdin_rt_panic`) are hidden.
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

    fn print_backtrace_unix(&mut self, f: FunctionValue<'ctx>) -> GenResult<()> {
        let (i32_ty, ptr_ty) = (self.i32(), self.ptr_ty);
        let backtrace = libc(self.module, "backtrace", i32_ty.fn_type(&[ptr_ty.into(), i32_ty.into()], false));
        let symbols = libc(
            self.module,
            "backtrace_symbols_fd",
            self.cx.void_type().fn_type(&[ptr_ty.into(), i32_ty.into(), i32_ty.into()], false),
        );
        let buf = self.b.build_alloca(ptr_ty.array_type(BACKTRACE_FRAMES), "frames").map_err(llvm_err)?;
        let n = self
            .call(backtrace, &[buf.into(), i32_ty.const_int(u64::from(BACKTRACE_FRAMES), false).into()])?
            .expect("backtrace returns a count")
            .into_int_value();
        let hidden = i32_ty.const_int(2, false);
        let enough = self.b.build_int_compare(IntPredicate::SGT, n, hidden, "enough").map_err(llvm_err)?;
        let show = self.block(f, "show");
        let done = self.block(f, "done");
        self.b.build_conditional_branch(enough, show, done).map_err(llvm_err)?;
        self.b.position_at_end(show);
        self.write_const(
            self.module.get_function("burdin_rt_write_all").expect("write_all exists"),
            2,
            b"stack backtrace:\n",
        )?;
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
        let index = self.b.build_alloca(i32_ty, "index").map_err(llvm_err)?;
        self.b.build_store(index, i32_ty.const_zero()).map_err(llvm_err)?;
        let header = self.block(f, "frame.header");
        let body = self.block(f, "frame.body");
        self.b.build_unconditional_branch(header).map_err(llvm_err)?;
        self.b.position_at_end(header);
        let i = self.b.build_load(i32_ty, index, "i").map_err(llvm_err)?.into_int_value();
        let more = self.b.build_int_compare(IntPredicate::SLT, i, n, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(more, body, done).map_err(llvm_err)?;
        self.b.position_at_end(body);
        let address = unsafe { self.b.build_in_bounds_gep(ptr_ty, buf, &[i], "frame") }.map_err(llvm_err)?;
        let address = self.b.build_load(ptr_ty, address, "").map_err(llvm_err)?.into_pointer_value();
        let address = self.b.build_ptr_to_int(address, i64_ty, "address").map_err(llvm_err)?;
        self.write_const(write_all, 2, b"  #")?;
        let i64_i = self.b.build_int_s_extend(i, i64_ty, "").map_err(llvm_err)?;
        let write_u64 = self.module.get_function("burdin_rt_write_u64").expect("write_u64 exists");
        self.call(write_u64, &[i32_ty.const_int(2, false).into(), i64_i.into()])?;
        self.write_const(write_all, 2, b" 0x")?;
        let write_hex = self.module.get_function("burdin_rt_write_hex").expect("write_hex exists");
        self.call(write_hex, &[i32_ty.const_int(2, false).into(), address.into()])?;
        self.write_const(write_all, 2, b"\n")?;
        let next = self.b.build_int_add(i, i32_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.b.build_store(index, next).map_err(llvm_err)?;
        self.b.build_unconditional_branch(header).map_err(llvm_err)?;
        self.b.position_at_end(done);
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(())
    }

    /// `install_stack_guard()`: makes the main thread's stack overflow a clear panic instead of a raw fault.
    fn install_stack_guard(&mut self, flush: FunctionValue<'ctx>) -> GenResult<FunctionValue<'ctx>> {
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
            Some(self.b.build_and(above, below, "stack_overflow").map_err(llvm_err)?)
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
        self.b.build_return(None).map_err(llvm_err)?;
        Ok(())
    }

    fn install_stack_guard_windows(&mut self, handler: FunctionValue<'ctx>) -> GenResult<()> {
        let (i32_ty, ptr_ty) = (self.i32(), self.ptr_ty);
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
            // The header is `{ next: ptr, size: usize, site: ptr, site_len: usize, magic: i64 }`.
            let live = self.module.get_global("burdin_rt_live").expect("the live list exists").as_pointer_value();
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
        self.b.build_conditional_branch(ok, unlink, bad).map_err(llvm_err)?;

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
        self.call(libc_free, &[header.into()])?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;

        self.b.position_at_end(done);
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

    /// `read_to_string(path, path_len, out: *String) -> i1`: the file's bytes, or false.
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
        let fclose = libc(self.module, "fclose", i32_ty.fn_type(&[ptr_ty.into()], false));
        let f = self.function(
            "burdin_rt_read_to_string",
            self.cx.bool_type().fn_type(&[ptr_ty.into(), usize_ty.into(), ptr_ty.into()], false),
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
        self.call(free, &[cpath.into()])?;
        self.b.build_return(Some(&self.cx.bool_type().const_zero())).map_err(llvm_err)?;

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
        self.call(fclose, &[file.into()])?;
        self.call(free, &[cpath.into()])?;
        self.b.build_return(Some(&self.cx.bool_type().const_int(1, false))).map_err(llvm_err)?;
        Ok(f)
    }

    /// `write_file(path, path_len, text, text_len) -> i1`.
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
        let f = self.function("burdin_rt_write_file", self.cx.bool_type().fn_type(&params, false));
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
        self.call(free, &[cpath.into()])?;
        self.b.build_return(Some(&self.cx.bool_type().const_zero())).map_err(llvm_err)?;

        self.b.position_at_end(ok);
        let written = self
            .call(fwrite, &[text.into(), usize_ty.const_int(1, false).into(), tlen.into(), file.into()])?
            .expect("fwrite returns a size")
            .into_int_value();
        self.call(fclose, &[file.into()])?;
        self.call(free, &[cpath.into()])?;
        let all = self.b.build_int_compare(IntPredicate::EQ, written, tlen, "").map_err(llvm_err)?;
        self.b.build_return(Some(&all)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `read_dir(path, path_len, out: *List<String>) -> i32`: the entry names, or -1. `.` and `..` are
    /// skipped. Every failure leaves an empty, valid list in `out`.
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
        self.call(free, &[path.into()])?;
        self.b.build_return(Some(&i32_ty.const_all_ones())).map_err(llvm_err)?;

        self.b.position_at_end(open);
        let scan = self.block(f, "read_dir.scan");
        let entry = self.block(f, "read_dir.entry");
        let append = self.block(f, "read_dir.append");
        let done = self.block(f, "read_dir.done");
        self.b.build_unconditional_branch(scan).map_err(llvm_err)?;

        self.b.position_at_end(scan);
        let ent = self.call(readdir, &[dir.into()])?.expect("readdir returns a pointer").into_pointer_value();
        let end = self.b.build_is_null(ent, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(end, done, entry).map_err(llvm_err)?;

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
        self.call_rt_free(pattern)?;
        self.call(free, &[path.into()])?;
        self.b.build_return(Some(&i32_ty.const_all_ones())).map_err(llvm_err)?;

        self.b.position_at_end(open);
        let entry = self.block(f, "read_dir.entry");
        let append = self.block(f, "read_dir.append");
        let next = self.block(f, "read_dir.next");
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
        self.b.build_conditional_branch(has_more, entry, done).map_err(llvm_err)?;

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

    /// `create_dir(path, path_len) -> i32`: `mkdir`/`_mkdir`.
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
        self.call(free, &[cpath.into()])?;
        self.b.build_return(Some(&status.expect("mkdir returns an int").into_int_value())).map_err(llvm_err)?;
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

    /// A one-path removal: `(path, path_len) -> i32`, the C function's status.
    fn remove_path(&mut self, name: &str, unix: &str, windows: &str) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let f = self.function(name, i32_ty.fn_type(&[ptr_ty.into(), usize_ty.into()], false));
        let (path, plen) = (Self::ptr_param(f, 0), Self::int_param(f, 1));
        let cpath = self.cstring_of(path, plen, malloc)?;
        let symbol = if self.windows { windows } else { unix };
        let remove = libc(self.module, symbol, i32_ty.fn_type(&[ptr_ty.into()], false));
        let status = self.call(remove, &[cpath.into()])?;
        self.call(free, &[cpath.into()])?;
        self.b.build_return(Some(&status.expect("the removal returns an int").into_int_value())).map_err(llvm_err)?;
        Ok(f)
    }

    /// `rename(from, from_len, to, to_len) -> i32`: `rename`, or `MoveFileExA` with
    /// `MOVEFILE_REPLACE_EXISTING` on Windows.
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
            self.call(free, &[cfrom.into()])?;
            self.call(free, &[cto.into()])?;
            let moved = self.b.build_int_compare(IntPredicate::NE, ok, i32_ty.const_zero(), "").map_err(llvm_err)?;
            let status = self
                .b
                .build_select(moved, i32_ty.const_zero(), i32_ty.const_all_ones(), "")
                .map_err(llvm_err)?
                .into_int_value();
            self.b.build_return(Some(&status)).map_err(llvm_err)?;
        } else {
            let rename = libc(self.module, "rename", i32_ty.fn_type(&[ptr_ty.into(), ptr_ty.into()], false));
            let status = self.call(rename, &[cfrom.into(), cto.into()])?;
            self.call(free, &[cfrom.into()])?;
            self.call(free, &[cto.into()])?;
            self.b.build_return(Some(&status.expect("rename returns an int").into_int_value())).map_err(llvm_err)?;
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
        self.call(free, &[cpath.into()])?;
        self.b.build_return(Some(&i32_ty.const_all_ones())).map_err(llvm_err)?;

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
            self.call(free, &[cpath.into()])?;
            self.b.build_return(Some(&i32_ty.const_all_ones())).map_err(llvm_err)?;
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
        self.call(free, &[cpath.into()])?;
        self.b.build_return(Some(&i32_ty.const_all_ones())).map_err(llvm_err)?;

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

    /// `exit(code: i32)`: ends the process.
    fn exit(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let exit = libc(self.module, "exit", self.cx.void_type().fn_type(&[self.i32().into()], false));
        self.mark_noreturn(exit);
        Ok(exit)
    }

    /// `run(command, command_len) -> i32`: runs the command through the system shell. The result is -1 when the
    /// shell could not start, and the command's exit code otherwise.
    fn run_command(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty, i32_ty) = (self.usize_ty, self.ptr_ty, self.i32());
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let free = libc(self.module, "free", self.cx.void_type().fn_type(&[ptr_ty.into()], false));
        let system = libc(self.module, "system", i32_ty.fn_type(&[ptr_ty.into()], false));
        let f = self.function("burdin_rt_run", i32_ty.fn_type(&[ptr_ty.into(), usize_ty.into()], false));
        let cpath = self.cstring_of(Self::ptr_param(f, 0), Self::int_param(f, 1), malloc)?;
        let status = self.call(system, &[cpath.into()])?.expect("system returns a status").into_int_value();
        self.call(free, &[cpath.into()])?;
        if self.windows {
            self.b.build_return(Some(&status)).map_err(llvm_err)?;
            return Ok(f);
        }
        // POSIX `system` returns a wait status: the exit code is its second byte. -1 stays -1.
        let minus_one = i32_ty.const_all_ones();
        let failed = self.b.build_int_compare(IntPredicate::EQ, status, minus_one, "").map_err(llvm_err)?;
        let shift = self.b.build_right_shift(status, i32_ty.const_int(8, false), false, "").map_err(llvm_err)?;
        let code = self.b.build_and(shift, i32_ty.const_int(0xff, false), "").map_err(llvm_err)?;
        let result = self.b.build_select(failed, minus_one, code, "").map_err(llvm_err)?;
        self.b.build_return(Some(&result)).map_err(llvm_err)?;
        Ok(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
