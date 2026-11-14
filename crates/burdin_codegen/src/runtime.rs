//! The runtime, generated as LLVM IR into every program: the process entry point (`main`), output, `String`
//! allocation and conversions, `Heap` allocation, and the panic path. It calls libc directly: `write`, `malloc`,
//! `realloc`, `free`, and `abort`, and on Windows the CRT's `_write`, `_setmode`, and `_set_abort_behavior`. Output
//! is unbuffered, so there is nothing to flush when a panic aborts.
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

/// The runtime functions the generated code calls.
pub(crate) struct Runtime<'ctx> {
    /// `(fd: i32, ptr, len: usize, newline: i32)`
    pub print: FunctionValue<'ctx>,
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
    /// `(size: usize)`: a `Heap` allocation, at least one byte, that panics on failure.
    pub alloc: FunctionValue<'ctx>,
    /// `(ptr)`: frees a `Heap` allocation.
    pub free: FunctionValue<'ctx>,
    /// `(ptr, len: usize, out: *String)`
    pub str_to_string: FunctionValue<'ctx>,
    /// `(v: i64, out: *String)`
    pub i64_to_string: FunctionValue<'ctx>,
    /// `(v: i64, out: *String)`
    pub u64_to_string: FunctionValue<'ctx>,
    /// `(c: i32, out: *String)`
    pub char_to_string: FunctionValue<'ctx>,
    /// `(ptr, len: usize, prefix, prefix_len: usize) -> i1`: whether the string starts with `prefix`.
    pub str_starts_with: FunctionValue<'ctx>,
    /// `(ptr, len: usize, c: i32) -> i64`: the byte index of the first `c`, or -1.
    pub str_find_char: FunctionValue<'ctx>,
}

/// Generates the runtime into `module`. `windows` selects the Windows CRT functions.
pub(crate) fn generate<'ctx>(
    cx: &'ctx Context,
    module: &Module<'ctx>,
    usize_ty: IntType<'ctx>,
    string_ty: StructType<'ctx>,
    windows: bool,
) -> GenResult<Runtime<'ctx>> {
    let mut g = Gen::new(cx, module, usize_ty, string_ty, windows);
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
    if windows {
        // Output is binary-safe: no newline translation (tests/README.md, assumption 10).
        let setmode = libc(module, "_setmode", i32_ty.fn_type(&[i32_ty.into(), i32_ty.into()], false));
        const O_BINARY: u64 = 0x8000;
        for fd in 0..3 {
            let args = [i32_ty.const_int(fd, false).into(), i32_ty.const_int(O_BINARY, false).into()];
            b.build_call(setmode, &args, "").map_err(llvm_err)?;
        }
    }
    b.build_call(program_main, &[], "").map_err(llvm_err)?;
    b.build_return(Some(&i32_ty.const_zero())).map_err(llvm_err)?;
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
    usize_ty: IntType<'ctx>,
    ptr_ty: PointerType<'ctx>,
    string_ty: StructType<'ctx>,
    noreturn: Attribute,
    globals: usize,
}

/// Helpers used while generating the other functions.
struct Helpers<'ctx> {
    write_all: FunctionValue<'ctx>,
    die: FunctionValue<'ctx>,
    write_u64: FunctionValue<'ctx>,
    digits: FunctionValue<'ctx>,
    alloc_string: FunctionValue<'ctx>,
    panic_start: FunctionValue<'ctx>,
}

impl<'ctx, 'a> Gen<'ctx, 'a> {
    fn new(
        cx: &'ctx Context,
        module: &'a Module<'ctx>,
        usize_ty: IntType<'ctx>,
        string_ty: StructType<'ctx>,
        windows: bool,
    ) -> Self {
        Gen {
            cx,
            module,
            b: cx.create_builder(),
            windows,
            usize_ty,
            ptr_ty: cx.ptr_type(AddressSpace::default()),
            string_ty,
            noreturn: cx.create_enum_attribute(Attribute::get_named_enum_kind_id("noreturn"), 0),
            globals: 0,
        }
    }

    fn generate(&mut self) -> GenResult<Runtime<'ctx>> {
        let h = self.helpers()?;
        Ok(Runtime {
            print: self.print(&h)?,
            panic: self.panic(&h)?,
            panic_bounds: self.panic_bounds(&h)?,
            string_free: self.string_free()?,
            string_new: self.string_new()?,
            string_clone: self.string_clone(&h)?,
            string_push_str: self.string_push_str(&h)?,
            alloc: self.heap_alloc(&h)?,
            free: self.heap_free()?,
            str_to_string: self.str_to_string(&h)?,
            i64_to_string: self.int_to_string(&h, true)?,
            u64_to_string: self.int_to_string(&h, false)?,
            char_to_string: self.char_to_string(&h)?,
            str_starts_with: self.str_starts_with()?,
            str_find_char: self.str_find_char()?,
        })
    }

    fn helpers(&mut self) -> GenResult<Helpers<'ctx>> {
        let write_all = self.write_all()?;
        let die = self.die()?;
        let digits = self.digits()?;
        let write_u64 = self.write_u64(write_all, digits)?;
        let alloc_string = self.alloc_string(write_all, die)?;
        let panic_start = self.panic_start(write_all, write_u64)?;
        Ok(Helpers { write_all, die, write_u64, digits, alloc_string, panic_start })
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

    /// `print(fd: i32, ptr, len: usize, newline: i32)`
    fn print(&mut self, h: &Helpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (i32_ty, usize_ty, ptr_ty) = (self.i32(), self.usize_ty, self.ptr_ty);
        let void = self.cx.void_type();
        let ty = void.fn_type(&[i32_ty.into(), ptr_ty.into(), usize_ty.into(), i32_ty.into()], false);
        let f = self.function("burdin_rt_print", ty);
        let fd = Self::int_param(f, 0);
        self.call(h.write_all, &[fd.into(), Self::ptr_param(f, 1).into(), Self::int_param(f, 2).into()])?;
        let newline = self.block(f, "newline");
        let done = self.block(f, "done");
        let nl = self
            .b
            .build_int_compare(IntPredicate::NE, Self::int_param(f, 3), i32_ty.const_zero(), "")
            .map_err(llvm_err)?;
        self.b.build_conditional_branch(nl, newline, done).map_err(llvm_err)?;
        self.b.position_at_end(newline);
        let ptr = self.bytes(b"\n");
        self.call(h.write_all, &[fd.into(), ptr.into(), usize_ty.const_int(1, false).into()])?;
        self.b.build_unconditional_branch(done).map_err(llvm_err)?;
        self.b.position_at_end(done);
        self.b.build_return(None).map_err(llvm_err)?;
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

    // ----- panics ----------------------------------------------------------------------

    /// `die()`: aborts the process, without the Windows "abnormal program termination" message.
    fn die(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let void = self.cx.void_type();
        let abort = libc(self.module, "abort", void.fn_type(&[], false));
        self.mark_noreturn(abort);
        let f = self.function("burdin_rt_die", void.fn_type(&[], false));
        self.mark_noreturn(f);
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

    /// `panic_start(file, file_len: usize, line: i32, col: i32)`: writes `path:LINE:COL: panic: ` to stderr.
    fn panic_start(
        &mut self,
        write_all: FunctionValue<'ctx>,
        write_u64: FunctionValue<'ctx>,
    ) -> GenResult<FunctionValue<'ctx>> {
        let void = self.cx.void_type();
        let (i32_ty, i64_ty) = (self.i32(), self.i64());
        let ty = void.fn_type(&[self.ptr_ty.into(), self.usize_ty.into(), i32_ty.into(), i32_ty.into()], false);
        let f = self.function("burdin_rt_panic_start", ty);
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
        self.call(h.die, &[])?;
        self.b.build_unreachable().map_err(llvm_err)?;
        Ok(f)
    }

    // ----- strings ---------------------------------------------------------------------

    /// `alloc_string(out: *String, bytes, len: usize)`: a new `String` holding a copy of the bytes. An empty string
    /// does not allocate (null pointer, capacity 0).
    fn alloc_string(
        &mut self,
        write_all: FunctionValue<'ctx>,
        die: FunctionValue<'ctx>,
    ) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty) = (self.usize_ty, self.ptr_ty);
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
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
        let oom = self.block(f, "oom");
        let copy = self.block(f, "copy");
        let done = self.block(f, "done");
        let empty = self.b.build_int_compare(IntPredicate::EQ, len, usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.b.build_conditional_branch(empty, done, alloc).map_err(llvm_err)?;

        self.b.position_at_end(alloc);
        let p = self.call(malloc, &[len.into()])?.expect("malloc returns a pointer").into_pointer_value();
        let null = self.b.build_is_null(p, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(null, oom, copy).map_err(llvm_err)?;

        self.b.position_at_end(oom);
        self.write_const(write_all, 2, b"panic: out of memory\n")?;
        self.call(die, &[])?;
        self.b.build_unreachable().map_err(llvm_err)?;

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
        let free = libc(self.module, "free", void.fn_type(&[ptr_ty.into()], false));
        let f = self.function("burdin_rt_string_free", void.fn_type(&[ptr_ty.into(), usize_ty.into()], false));
        let release = self.block(f, "free");
        let done = self.block(f, "done");
        let empty = self
            .b
            .build_int_compare(IntPredicate::EQ, Self::int_param(f, 1), usize_ty.const_zero(), "")
            .map_err(llvm_err)?;
        self.b.build_conditional_branch(empty, done, release).map_err(llvm_err)?;
        self.b.position_at_end(release);
        self.call(free, &[Self::ptr_param(f, 0).into()])?;
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
        let realloc = libc(self.module, "realloc", ptr_ty.fn_type(&[ptr_ty.into(), usize_ty.into()], false));
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
        let grown = self
            .call(realloc, &[old_ptr.into(), new_cap.into()])?
            .expect("realloc returns a pointer")
            .into_pointer_value();
        let failed = self.b.build_is_null(grown, "").map_err(llvm_err)?;
        self.b.build_conditional_branch(failed, oom, resized).map_err(llvm_err)?;

        self.b.position_at_end(resized);
        self.b.build_store(ptr_field, grown).map_err(llvm_err)?;
        self.b.build_store(cap_field, new_cap).map_err(llvm_err)?;
        self.b.build_unconditional_branch(copy).map_err(llvm_err)?;

        self.b.position_at_end(oom);
        self.write_const(h.write_all, 2, b"panic: out of memory\n")?;
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

    /// `alloc(size: usize)`: a `malloc`ed block of at least one byte for `Heap<T>`, or a panic.
    fn heap_alloc(&mut self, h: &Helpers<'ctx>) -> GenResult<FunctionValue<'ctx>> {
        let (usize_ty, ptr_ty) = (self.usize_ty, self.ptr_ty);
        let malloc = libc(self.module, "malloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let f = self.function("burdin_rt_alloc", ptr_ty.fn_type(&[usize_ty.into()], false));
        let size = Self::int_param(f, 0);
        let zero = self.b.build_int_compare(IntPredicate::EQ, size, usize_ty.const_zero(), "").map_err(llvm_err)?;
        let size =
            self.b.build_select(zero, usize_ty.const_int(1, false), size, "").map_err(llvm_err)?.into_int_value();
        let p = self.call(malloc, &[size.into()])?.expect("malloc returns a pointer").into_pointer_value();
        let null = self.b.build_is_null(p, "").map_err(llvm_err)?;
        let oom = self.block(f, "oom");
        let done = self.block(f, "done");
        self.b.build_conditional_branch(null, oom, done).map_err(llvm_err)?;
        self.b.position_at_end(oom);
        self.write_const(h.write_all, 2, b"panic: out of memory\n")?;
        self.call(h.die, &[])?;
        self.b.build_unreachable().map_err(llvm_err)?;
        self.b.position_at_end(done);
        self.b.build_return(Some(&p)).map_err(llvm_err)?;
        Ok(f)
    }

    /// `free(ptr)`: releases a `Heap` allocation.
    fn heap_free(&mut self) -> GenResult<FunctionValue<'ctx>> {
        let void = self.cx.void_type();
        let libc_free = libc(self.module, "free", void.fn_type(&[self.ptr_ty.into()], false));
        let f = self.function("burdin_rt_free", void.fn_type(&[self.ptr_ty.into()], false));
        self.call(libc_free, &[Self::ptr_param(f, 0).into()])?;
        self.b.build_return(None).map_err(llvm_err)?;
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
}
