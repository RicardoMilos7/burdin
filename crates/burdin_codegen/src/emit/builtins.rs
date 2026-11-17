//! Built-in functions and the methods of the primitive types.

use super::{Codegen, GenResult, Mode, llvm_err, undef};
use burdin_diag::Span;
use burdin_tir::{self as tir, BinOp, Builtin, FloatOp, FloatTy, Method, Ty};
use inkwell::intrinsics::Intrinsic;
use inkwell::module::Linkage;
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, FloatValue, FunctionValue};
use inkwell::{FloatPredicate, IntPredicate};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    pub(super) fn builtin(
        &mut self,
        b: Builtin,
        args: &'a [tir::Expr],
        ty: &Ty,
        span: Span,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        match b {
            Builtin::ComptimeAssert => {
                return Err("`comptime assert` reached code generation; the comptime pass must replace it".into());
            }
            // The program is compiled for the machine the compiler runs on.
            Builtin::TargetOs => return Ok(Some(self.str_literal(burdin_tir::target_os().as_bytes()).into())),
            Builtin::TargetArch => return Ok(Some(self.str_literal(burdin_tir::target_arch().as_bytes()).into())),
            // `__io_kind(code)`: the portable kind of an operating system code (`spec/errors.md`, ERR11).
            // The table is the target's, and the variant is found by name so a reordered `IoErrorKind` is still
            // classified correctly.
            Builtin::IoErrorKind => {
                let code = self.expr_mode(&args[0], Mode::Use)?;
                let code = self.need_int(code, &Ty::Int(tir::IntTy::I32));
                // A standard library without an `IoErrorKind` enum types this builtin as unit, and the call
                // needs no value.
                let Ty::Enum(a) = ty else { return Ok(None) };
                let variants = &self.program.enums[a.id].variants;
                let index = |name: &str| variants.iter().position(|v| v.name == name);
                let i32_ty = self.cx.i32_type();
                // A code the table does not know is `Other`; a standard library without that name falls back
                // to the first variant rather than panicking.
                let mut tag: BasicValueEnum<'ctx> = i32_ty.const_int(index("Other").unwrap_or(0) as u64, false).into();
                for (os, name) in tir::ioerror::table(self.platform) {
                    let Some(variant) = index(name) else { continue };
                    let is = self
                        .builder
                        .build_int_compare(IntPredicate::EQ, code, i32_ty.const_int(*os as u64, false), "")
                        .map_err(llvm_err)?;
                    let kind: BasicValueEnum<'ctx> = i32_ty.const_int(variant as u64, false).into();
                    tag = self.builder.build_select(is, kind, tag, "io.kind").map_err(llvm_err)?;
                }
                let et = self.enum_ty(a.id);
                let value = self
                    .builder
                    .build_insert_value(et.const_zero(), tag, 0, "io.kind.value")
                    .map_err(llvm_err)?
                    .into_struct_value();
                return Ok(Some(value.into()));
            }
            Builtin::CPtrNull => {
                return Ok(Some(self.cx.ptr_type(inkwell::AddressSpace::default()).const_null().into()));
            }
            // `__view_as_text(view)`: `[u8]` and `str` are the same pointer-and-length value.
            Builtin::ViewAsText => return self.expr_mode(&args[0], Mode::Use),
            Builtin::CStringFrom => {
                let s = self.expr_mode(&args[0], Mode::Use)?;
                let s = self.need(s, &Ty::Str).into_struct_value();
                let ptr = self.builder.build_extract_value(s, 0, "ptr").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(s, 1, "len").map_err(llvm_err)?;
                let call = self
                    .builder
                    .build_call(self.rt.cstring_from, &[ptr.into(), len.into()], "cstring")
                    .map_err(llvm_err)?;
                return Ok(call.try_as_basic_value().basic());
            }
            Builtin::CStrToString => {
                let ptr =
                    self.expr_mode(&args[0], Mode::Use)?.expect("a C string pointer has a value").into_pointer_value();
                let len = self
                    .builder
                    .build_call(self.rt.strlen, &[ptr.into()], "cstr.len")
                    .map_err(llvm_err)?
                    .try_as_basic_value()
                    .basic()
                    .expect("strlen returns a length");
                let out = self.alloca(self.string_ty.into(), "string")?;
                self.builder
                    .build_call(self.rt.str_to_string, &[ptr.into(), len.into(), out.into()], "")
                    .map_err(llvm_err)?;
                return Ok(Some(self.builder.build_load(self.string_ty, out, "").map_err(llvm_err)?));
            }
            Builtin::FloatStr => {
                let v = self.expr_mode(&args[0], Mode::Use)?.expect("a float has a value").into_float_value();
                let prec = self.expr_mode(&args[1], Mode::Use)?.expect("an integer has a value").into_int_value();
                let single = self.expr_mode(&args[2], Mode::Use)?.expect("a bool has a value").into_int_value();
                let out = self.alloca(self.string_ty.into(), "string")?;
                self.builder
                    .build_call(self.rt.f64_to_string, &[v.into(), prec.into(), single.into(), out.into()], "")
                    .map_err(llvm_err)?;
                return Ok(Some(self.builder.build_load(self.string_ty, out, "").map_err(llvm_err)?));
            }
            Builtin::FloatFromBits(ft) => {
                let bits = self.expr_mode(&args[0], Mode::Use)?.expect("an integer has a value").into_int_value();
                let target: BasicTypeEnum<'ctx> = match ft {
                    FloatTy::F32 => self.cx.f32_type().into(),
                    FloatTy::F64 => self.cx.f64_type().into(),
                };
                return Ok(Some(self.builder.build_bit_cast(bits, target, "float").map_err(llvm_err)?));
            }
            Builtin::StringNew => {
                let out = self.alloca(self.string_ty.into(), "string")?;
                self.builder.build_call(self.rt.string_new, &[out.into()], "").map_err(llvm_err)?;
                return Ok(Some(self.builder.build_load(self.string_ty, out, "").map_err(llvm_err)?));
            }
            Builtin::StringFrom => {
                let s = self.expr_mode(&args[0], Mode::Use)?;
                let s = self.need(s, &Ty::Str).into_struct_value();
                let ptr = self.builder.build_extract_value(s, 0, "ptr").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(s, 1, "len").map_err(llvm_err)?;
                let out = self.alloca(self.string_ty.into(), "string")?;
                self.builder
                    .build_call(self.rt.str_to_string, &[ptr.into(), len.into(), out.into()], "")
                    .map_err(llvm_err)?;
                return Ok(Some(self.builder.build_load(self.string_ty, out, "").map_err(llvm_err)?));
            }
            Builtin::HeapNew => {
                let value = self.expr_mode(&args[0], Mode::Move)?;
                let inner = args[0].ty.clone();
                let size = self.llvm_ty(&inner).map(|t| self.target_data.get_abi_size(&t)).unwrap_or(0);
                let size = self.usize_ty.const_int(size.max(1), false);
                let align = self.value_align(&inner);
                let ptr = self.global_alloc(size, Some(span))?;
                if self.llvm_ty(&inner).is_some() {
                    let v = self.need(value, &inner);
                    self.builder.build_store(ptr, v).map_err(llvm_err)?;
                }
                let _ = align;
                return Ok(Some(ptr.into()));
            }
            // `Heap.new_in(alloc, value)`: the allocator value is stored beside the pointer.
            Builtin::HeapNewIn => {
                let alloc_ty = args[0].ty.clone();
                let inner = args[1].ty.clone();
                let alloc_value = self.expr_mode(&args[0], Mode::Move)?;
                // A generic body instantiated with `Global` names the default allocator here: the value is
                // zero-sized and nothing is stored (`spec/allocators.md`, AL3).
                if self.is_global_allocator(&alloc_ty) {
                    let value = self.expr_mode(&args[1], Mode::Move)?;
                    let _ = value;
                    let size = self.llvm_ty(&inner).map(|t| self.target_data.get_abi_size(&t)).unwrap_or(0).max(1);
                    let size = self.usize_ty.const_int(size, false);
                    let ptr = self.global_alloc(size, Some(span))?;
                    return Ok(Some(ptr.into()));
                }
                let value = self.expr_mode(&args[1], Mode::Move)?;
                let _ = alloc_value;
                let alloc = burdin_tir::Alloc::Value(Box::new(alloc_ty));
                let heap_ty = self.heap_ty(&alloc).into_struct_type();
                let slot = self.alloca(heap_ty.into(), "heap.value")?;
                let alloc_field = self.builder.build_struct_gep(heap_ty, slot, 1, "heap.alloc").map_err(llvm_err)?;
                let alloc_value = self.need(alloc_value, alloc.allocator_ty().expect("stored allocator"));
                self.builder.build_store(alloc_field, alloc_value).map_err(llvm_err)?;
                let size = self.llvm_ty(&inner).map(|t| self.target_data.get_abi_size(&t)).unwrap_or(0).max(1);
                let size = self.usize_ty.const_int(size, false);
                let align = self.value_align(&inner);
                let ptr = self.alloc_in(&alloc, Some(alloc_field), size, align, Some(span))?;
                if self.llvm_ty(&inner).is_some() {
                    let v = self.need(value, &inner);
                    self.builder.build_store(ptr, v).map_err(llvm_err)?;
                }
                let ptr_field = self.builder.build_struct_gep(heap_ty, slot, 0, "heap.ptr").map_err(llvm_err)?;
                self.builder.build_store(ptr_field, ptr).map_err(llvm_err)?;
                return Ok(Some(self.builder.build_load(heap_ty, slot, "").map_err(llvm_err)?));
            }
            // `List<T>.with_capacity(n)` and `List<T>.with_capacity_in(n, alloc)`: an empty list with room for
            // `n` elements. A stored allocator value is placed in the list's own field before the buffer is
            // allocated through it.
            Builtin::ListWithCapacity | Builtin::ListWithCapacityIn => {
                let n = self.expr_mode(&args[0], Mode::Use)?;
                let n = self.need_int(n, &Ty::Int(burdin_tir::IntTy::Usize));
                let Ty::List(elem, alloc) = ty else { return Err("`with_capacity` on a non-list type".into()) };
                let (elem, alloc) = ((**elem).clone(), alloc.clone());
                if b == Builtin::ListWithCapacityIn {
                    let alloc_value = self.expr_mode(&args[1], Mode::Move)?;
                    if !alloc.is_global() {
                        let seq_ty = self.seq_ty(&alloc);
                        let slot = self.alloca(seq_ty.into(), "list.capacity")?;
                        let alloc_field =
                            self.builder.build_struct_gep(seq_ty, slot, 3, "list.alloc").map_err(llvm_err)?;
                        let alloc_value = self.need(alloc_value, alloc.allocator_ty().expect("stored allocator"));
                        self.builder.build_store(alloc_field, alloc_value).map_err(llvm_err)?;
                        let value = self.list_with_capacity(n, &elem, &alloc, Some(alloc_field), span)?;
                        self.builder.build_store(slot, value).map_err(llvm_err)?;
                        return Ok(Some(self.builder.build_load(seq_ty, slot, "").map_err(llvm_err)?));
                    }
                }
                let value = self.list_with_capacity(n, &elem, &alloc, None, span)?;
                return Ok(Some(value.into()));
            }
            Builtin::ListNew | Builtin::MapNew => {
                return Ok(Some(self.string_ty.const_zero().into()));
            }
            // `Map<K, V>.with_capacity(n)`: an empty map whose table holds `n` entries without growing.
            Builtin::MapWithCapacity => {
                let capacity = self.expr_mode(&args[0], Mode::Use)?;
                let capacity = self.need_int(capacity, &Ty::Int(burdin_tir::IntTy::Usize));
                let Ty::Map(key, value, _) = ty else { return Ok(None) };
                let (key, value) = ((**key).clone(), (**value).clone());
                let cap = self.map_capacity_for(capacity, span)?;
                let map_ty = self.llvm_ty(ty).expect("a map has a type");
                let slot = self.alloca(map_ty, "map.with_capacity")?;
                self.builder.build_store(slot, map_ty.const_zero()).map_err(llvm_err)?;
                let mut seq = self.seq_load_at(slot, &burdin_tir::Alloc::Global)?;
                self.map_rehash_to(&mut seq, &key, &value, cap, span)?;
                self.seq_store(&seq)?;
                return Ok(Some(self.builder.build_load(map_ty, slot, "").map_err(llvm_err)?));
            }
            // `List<T>.new_in(alloc)` and `Map<K, V>.new_in(alloc)`: the empty container with the allocator value.
            Builtin::ListNewIn | Builtin::MapNewIn => {
                let alloc_ty = args[0].ty.clone();
                let alloc_value = self.expr_mode(&args[0], Mode::Move)?;
                let global = self.is_global_allocator(&alloc_ty);
                let alloc =
                    if global { burdin_tir::Alloc::Global } else { burdin_tir::Alloc::Value(Box::new(alloc_ty)) };
                let seq_ty = self.seq_ty(&alloc);
                let agg = seq_ty.const_zero();
                if global {
                    let _ = alloc_value;
                    return Ok(Some(agg.into()));
                }
                let alloc_value = self.need(alloc_value, alloc.allocator_ty().expect("stored allocator"));
                let agg =
                    self.builder.build_insert_value(agg, alloc_value, 3, "").map_err(llvm_err)?.into_struct_value();
                return Ok(Some(agg.into()));
            }
            // The `Global` allocator's primitives. The site is the call's span, which points into the allocator's
            // implementation, so a leak of a container's buffer names the code that allocated it.
            Builtin::RawAlloc => {
                let size = self.expr_mode(&args[0], Mode::Use)?;
                let size = self.need_int(size, &Ty::Int(burdin_tir::IntTy::Usize));
                let _align = self.expr_mode(&args[1], Mode::Use)?;
                let p = self.global_alloc(size, Some(span))?;
                return Ok(Some(p.into()));
            }
            Builtin::RawRealloc => {
                let p = self.expr_mode(&args[0], Mode::Use)?;
                let p = self.need(p, &args[0].ty).into_pointer_value();
                let _old = self.expr_mode(&args[1], Mode::Use)?;
                let new_size = self.expr_mode(&args[2], Mode::Use)?;
                let new_size = self.need_int(new_size, &Ty::Int(burdin_tir::IntTy::Usize));
                let _align = self.expr_mode(&args[3], Mode::Use)?;
                let p = self.global_realloc(p, new_size, Some(span))?;
                return Ok(Some(p.into()));
            }
            Builtin::RawDealloc => {
                let p = self.expr_mode(&args[0], Mode::Use)?;
                let p = self.need(p, &args[0].ty).into_pointer_value();
                let _size = self.expr_mode(&args[1], Mode::Use)?;
                let _align = self.expr_mode(&args[2], Mode::Use)?;
                self.global_dealloc(p)?;
                return Ok(None);
            }
            // `Shared.new(value)`: a cell `{ count: usize, value }`.
            Builtin::SharedNew => {
                let value = self.expr_mode(&args[0], Mode::Move)?;
                let inner = args[0].ty.clone();
                let inner_size = self.llvm_ty(&inner).map(|t| self.target_data.get_abi_size(&t)).unwrap_or(0);
                let size = self.usize_ty.const_int(8 + inner_size.max(1), false);
                let p = self.global_alloc(size, Some(span))?;
                self.builder.build_store(p, self.usize_ty.const_int(1, false)).map_err(llvm_err)?;
                if self.llvm_ty(&inner).is_some() {
                    let eight = self.usize_ty.const_int(8, false);
                    let value_at = self.offset_ptr(p, eight)?;
                    let v = self.need(value, &inner);
                    self.builder.build_store(value_at, v).map_err(llvm_err)?;
                }
                return Ok(Some(p.into()));
            }
            Builtin::Swap => {
                let ty = args[0].ty.clone();
                let (Some(a), Some(b)) = (self.place_ptr(&args[0])?, self.place_ptr(&args[1])?) else {
                    return Ok(None);
                };
                if let Some(t) = self.llvm_ty(&ty) {
                    let va = self.builder.build_load(t, a, "swap.a").map_err(llvm_err)?;
                    let vb = self.builder.build_load(t, b, "swap.b").map_err(llvm_err)?;
                    self.builder.build_store(a, vb).map_err(llvm_err)?;
                    self.builder.build_store(b, va).map_err(llvm_err)?;
                }
                return Ok(None);
            }
            Builtin::Replace => {
                let ty = args[0].ty.clone();
                let Some(slot) = self.place_ptr(&args[0])? else { return Ok(None) };
                let Some(t) = self.llvm_ty(&ty) else {
                    self.expr_mode(&args[1], Mode::Move)?;
                    return Ok(None);
                };
                let old = self.builder.build_load(t, slot, "replace.old").map_err(llvm_err)?;
                let value = self.expr_mode(&args[1], Mode::Move)?;
                let value = self.need(value, &ty);
                self.builder.build_store(slot, value).map_err(llvm_err)?;
                return Ok(Some(old));
            }
            Builtin::RunCommand => {
                let s = self.expr_mode(&args[0], Mode::Use)?;
                let s = self.need(s, &Ty::Str).into_struct_value();
                let ptr = self.builder.build_extract_value(s, 0, "").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(s, 1, "").map_err(llvm_err)?;
                let err_slot = self.alloca(self.cx.i32_type().into(), "run.err")?;
                let call = self
                    .builder
                    .build_call(self.rt.run, &[ptr.into(), len.into(), err_slot.into()], "")
                    .map_err(llvm_err)?;
                let status = call.try_as_basic_value().basic().expect("run returns a status").into_int_value();
                let failed = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, status, self.cx.i32_type().const_all_ones(), "")
                    .map_err(llvm_err)?;
                let ok = self.builder.build_not(failed, "").map_err(llvm_err)?;
                let code = self.builder.build_load(self.cx.i32_type(), err_slot, "run.code").map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("run returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let mut value =
                    self.builder.build_insert_value(rt.const_zero(), ok, 0, "").map_err(llvm_err)?.into_struct_value();
                if self.llvm_ty(ok_ty).is_some() {
                    value =
                        self.builder.build_insert_value(value, status, 1, "").map_err(llvm_err)?.into_struct_value();
                }
                value = self.builder.build_insert_value(value, code, 2, "").map_err(llvm_err)?.into_struct_value();
                return Ok(Some(value.into()));
            }
            Builtin::EnvList => {
                let out = self.alloca(self.string_ty.into(), "env.list")?;
                self.builder.build_call(self.rt.env_list, &[out.into()], "").map_err(llvm_err)?;
                return Ok(Some(self.builder.build_load(self.string_ty, out, "").map_err(llvm_err)?));
            }
            Builtin::ProcessSpawn => {
                let program = self.expr_mode(&args[0], Mode::Use)?;
                let program = self.need(program, &Ty::Str).into_struct_value();
                let pptr = self.builder.build_extract_value(program, 0, "spawn.program").map_err(llvm_err)?;
                let plen = self.builder.build_extract_value(program, 1, "spawn.program.len").map_err(llvm_err)?;
                let list = self.expr_mode(&args[1], Mode::Use)?;
                let list_ty = args[1].ty.clone();
                let list = self.need(list, &list_ty).into_struct_value();
                let aptr = self.builder.build_extract_value(list, 0, "spawn.args").map_err(llvm_err)?;
                let alen = self.builder.build_extract_value(list, 1, "spawn.args.len").map_err(llvm_err)?;
                let list = self.expr_mode(&args[2], Mode::Use)?;
                let env_ty = args[2].ty.clone();
                let list = self.need(list, &env_ty).into_struct_value();
                let eptr = self.builder.build_extract_value(list, 0, "spawn.env").map_err(llvm_err)?;
                let elen = self.builder.build_extract_value(list, 1, "spawn.env.len").map_err(llvm_err)?;
                let (cptr, clen) = self.str_parts(&args[3])?;
                let (nptr, nlen) = self.str_parts(&args[4])?;
                let modes: Vec<BasicMetadataValueEnum<'ctx>> = (5..8)
                    .map(|i| {
                        let value = self.expr_mode(&args[i], Mode::Use)?;
                        Ok(self.need_int(value, &Ty::Int(burdin_tir::IntTy::I32)).into())
                    })
                    .collect::<GenResult<Vec<_>>>()?;
                let raw = self.alloca(self.cx.i64_type().array_type(5).into(), "spawn.raw")?;
                let mut values: Vec<BasicMetadataValueEnum<'ctx>> =
                    vec![pptr.into(), plen.into(), aptr.into(), alen.into(), eptr.into(), elen.into()];
                values.push(cptr.into());
                values.push(clen.into());
                values.push(nptr.into());
                values.push(nlen.into());
                values.extend(modes);
                values.push(raw.into());
                let call = self.builder.build_call(self.rt.process_spawn, &values, "").map_err(llvm_err)?;
                let status =
                    call.try_as_basic_value().basic().expect("process_spawn returns a status").into_int_value();
                let zero = self.cx.i32_type().const_zero();
                let ok = self.builder.build_int_compare(IntPredicate::EQ, status, zero, "").map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("process_spawn returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let slot = self.alloca(rt.into(), "spawn.result")?;
                let good = self.new_block("spawn.ok");
                let bad = self.new_block("spawn.err");
                let end = self.new_block("spawn.end");
                self.builder.build_conditional_branch(ok, good, bad).map_err(llvm_err)?;
                self.builder.position_at_end(good);
                let tuple_ty = self.llvm_ty(ok_ty).expect("the child tuple has a value");
                let mut tuple = undef(tuple_ty).into_struct_value();
                for i in 0..5u64 {
                    let at = self.offset_ptr(raw, self.usize_ty.const_int(i * 8, false))?;
                    let field = self.builder.build_load(self.cx.i64_type(), at, "").map_err(llvm_err)?;
                    let field = if i == 0 {
                        self.builder
                            .build_int_truncate(field.into_int_value(), self.cx.i32_type(), "")
                            .map_err(llvm_err)?
                            .into()
                    } else {
                        field
                    };
                    tuple = self
                        .builder
                        .build_insert_value(tuple, field, i as u32, "")
                        .map_err(llvm_err)?
                        .into_struct_value();
                }
                let mut value = self
                    .builder
                    .build_insert_value(rt.const_zero(), self.cx.bool_type().const_int(1, false), 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                value = self.builder.build_insert_value(value, tuple, 1, "").map_err(llvm_err)?.into_struct_value();
                self.builder.build_store(slot, value).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(bad);
                self.builder.build_store(slot, rt.const_zero()).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                return Ok(Some(self.builder.build_load(rt, slot, "").map_err(llvm_err)?));
            }
            Builtin::ProcessWait => {
                let pid = self.expr_mode(&args[0], Mode::Use)?;
                let pid = self.need_int(pid, &Ty::Int(burdin_tir::IntTy::I32));
                let process = self.expr_mode(&args[1], Mode::Use)?;
                let process = self.need_int(process, &Ty::Int(burdin_tir::IntTy::Usize));
                let call = self
                    .builder
                    .build_call(self.rt.process_wait, &[pid.into(), process.into()], "wait")
                    .map_err(llvm_err)?;
                return Ok(call.try_as_basic_value().basic());
            }
            Builtin::ProcessClose => {
                let process = self.expr_mode(&args[0], Mode::Use)?;
                let process = self.need_int(process, &Ty::Int(burdin_tir::IntTy::Usize));
                self.builder.build_call(self.rt.process_close, &[process.into()], "").map_err(llvm_err)?;
                return Ok(None);
            }
            Builtin::PipeReadToString => {
                let handle = self.expr_mode(&args[0], Mode::Use)?;
                let handle = self.need_int(handle, &Ty::Int(burdin_tir::IntTy::Usize));
                let out = self.alloca(self.string_ty.into(), "pipe.string")?;
                self.builder.build_call(self.rt.string_new, &[out.into()], "").map_err(llvm_err)?;
                let call = self
                    .builder
                    .build_call(self.rt.pipe_read_to_string, &[handle.into(), out.into()], "")
                    .map_err(llvm_err)?;
                let status = call.try_as_basic_value().basic().expect("a pipe read returns a status").into_int_value();
                let zero = self.cx.i32_type().const_zero();
                let ok = self.builder.build_int_compare(IntPredicate::EQ, status, zero, "").map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("a pipe read returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let slot = self.alloca(rt.into(), "pipe.result")?;
                let good = self.new_block("pipe.ok");
                let bad = self.new_block("pipe.err");
                let end = self.new_block("pipe.end");
                self.builder.build_conditional_branch(ok, good, bad).map_err(llvm_err)?;
                self.builder.position_at_end(good);
                let text = self.builder.build_load(self.string_ty, out, "").map_err(llvm_err)?;
                let mut value = self
                    .builder
                    .build_insert_value(rt.const_zero(), self.cx.bool_type().const_int(1, false), 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                value = self.builder.build_insert_value(value, text, 1, "").map_err(llvm_err)?.into_struct_value();
                self.builder.build_store(slot, value).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(bad);
                self.drop_value(out, &Ty::String)?;
                self.builder.build_store(slot, rt.const_zero()).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                return Ok(Some(self.builder.build_load(rt, slot, "").map_err(llvm_err)?));
            }
            Builtin::PipeReadLine => {
                let handle = self.expr_mode(&args[0], Mode::Use)?;
                let handle = self.need_int(handle, &Ty::Int(burdin_tir::IntTy::Usize));
                let out = self.alloca(self.string_ty.into(), "pipe.line")?;
                self.builder.build_call(self.rt.string_new, &[out.into()], "").map_err(llvm_err)?;
                let status = self
                    .builder
                    .build_call(self.rt.pipe_read_line, &[handle.into(), out.into()], "")
                    .map_err(llvm_err)?
                    .try_as_basic_value()
                    .basic()
                    .expect("a pipe read returns a status")
                    .into_int_value();
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("a pipe read returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let slot = self.alloca(rt.into(), "pipe.result")?;
                let i32_ty = self.cx.i32_type();
                let good = self.new_block("pipe.line.ok");
                let bad = self.new_block("pipe.line.err");
                let end = self.new_block("pipe.line.end");
                let failed = self
                    .builder
                    .build_int_compare(IntPredicate::SLT, status, i32_ty.const_zero(), "")
                    .map_err(llvm_err)?;
                self.builder.build_conditional_branch(failed, bad, good).map_err(llvm_err)?;
                self.builder.position_at_end(good);
                let text = self.builder.build_load(self.string_ty, out, "").map_err(llvm_err)?;
                let Ty::Option(inner) = &**ok_ty else { unreachable!("a pipe read returns Option<String>") };
                let eof = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, status, i32_ty.const_int(1, false), "")
                    .map_err(llvm_err)?;
                let some = self.builder.build_not(eof, "some").map_err(llvm_err)?;
                let opt_ty = self.option_ty(inner);
                let option = self
                    .builder
                    .build_insert_value(opt_ty.const_zero(), some, 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                let option =
                    self.builder.build_insert_value(option, text, 1, "").map_err(llvm_err)?.into_struct_value();
                let mut value = self
                    .builder
                    .build_insert_value(rt.const_zero(), self.cx.bool_type().const_int(1, false), 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                value = self.builder.build_insert_value(value, option, 1, "").map_err(llvm_err)?.into_struct_value();
                self.builder.build_store(slot, value).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(bad);
                self.drop_value(out, &Ty::String)?;
                self.builder.build_store(slot, rt.const_zero()).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                return Ok(Some(self.builder.build_load(rt, slot, "").map_err(llvm_err)?));
            }
            Builtin::PipeReadBoth => {
                let first = self.expr_mode(&args[0], Mode::Use)?;
                let first = self.need_int(first, &Ty::Int(burdin_tir::IntTy::Usize));
                let second = self.expr_mode(&args[1], Mode::Use)?;
                let second = self.need_int(second, &Ty::Int(burdin_tir::IntTy::Usize));
                let array_ty = self.string_ty.array_type(2);
                let out = self.alloca(array_ty.into(), "both.strings")?;
                let zero = self.usize_ty.const_zero();
                let first_slot =
                    unsafe { self.builder.build_in_bounds_gep(array_ty, out, &[zero, zero], "both.first") }
                        .map_err(llvm_err)?;
                let second_slot = unsafe {
                    self.builder.build_in_bounds_gep(
                        array_ty,
                        out,
                        &[zero, self.usize_ty.const_int(1, false)],
                        "both.second",
                    )
                }
                .map_err(llvm_err)?;
                self.builder.build_call(self.rt.string_new, &[first_slot.into()], "").map_err(llvm_err)?;
                self.builder.build_call(self.rt.string_new, &[second_slot.into()], "").map_err(llvm_err)?;
                let status = self
                    .builder
                    .build_call(self.rt.pipe_read_both, &[first.into(), second.into(), out.into()], "")
                    .map_err(llvm_err)?
                    .try_as_basic_value()
                    .basic()
                    .expect("a pipe read returns a status")
                    .into_int_value();
                let zero_i32 = self.cx.i32_type().const_zero();
                let ok = self.builder.build_int_compare(IntPredicate::EQ, status, zero_i32, "").map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("a pipe read returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let slot = self.alloca(rt.into(), "both.result")?;
                let good = self.new_block("both.ok");
                let bad = self.new_block("both.err");
                let end = self.new_block("both.end");
                self.builder.build_conditional_branch(ok, good, bad).map_err(llvm_err)?;
                self.builder.position_at_end(good);
                let tuple_ty = self.llvm_ty(ok_ty).expect("the pair has a value");
                let mut tuple = undef(tuple_ty).into_struct_value();
                let first_value = self.builder.build_load(self.string_ty, first_slot, "").map_err(llvm_err)?;
                let second_value = self.builder.build_load(self.string_ty, second_slot, "").map_err(llvm_err)?;
                tuple =
                    self.builder.build_insert_value(tuple, first_value, 0, "").map_err(llvm_err)?.into_struct_value();
                tuple =
                    self.builder.build_insert_value(tuple, second_value, 1, "").map_err(llvm_err)?.into_struct_value();
                let mut value = self
                    .builder
                    .build_insert_value(rt.const_zero(), self.cx.bool_type().const_int(1, false), 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                value = self.builder.build_insert_value(value, tuple, 1, "").map_err(llvm_err)?.into_struct_value();
                self.builder.build_store(slot, value).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(bad);
                self.drop_value(first_slot, &Ty::String)?;
                self.drop_value(second_slot, &Ty::String)?;
                self.builder.build_store(slot, rt.const_zero()).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                return Ok(Some(self.builder.build_load(rt, slot, "").map_err(llvm_err)?));
            }
            Builtin::PipeWrite => {
                let handle = self.expr_mode(&args[0], Mode::Use)?;
                let handle = self.need_int(handle, &Ty::Int(burdin_tir::IntTy::Usize));
                let (ptr, len) = self.str_parts(&args[1])?;
                let call = self
                    .builder
                    .build_call(self.rt.pipe_write, &[handle.into(), ptr.into(), len.into()], "")
                    .map_err(llvm_err)?;
                return Ok(call.try_as_basic_value().basic());
            }
            Builtin::PipeClose => {
                let handle = self.expr_mode(&args[0], Mode::Use)?;
                let handle = self.need_int(handle, &Ty::Int(burdin_tir::IntTy::Usize));
                self.builder.build_call(self.rt.pipe_close, &[handle.into()], "").map_err(llvm_err)?;
                return Ok(None);
            }
            Builtin::IntParse(int_ty) => {
                let s = self.expr_mode(&args[0], Mode::Use)?;
                let s = self.need(s, &Ty::Str).into_struct_value();
                let ptr = self.builder.build_extract_value(s, 0, "").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(s, 1, "").map_err(llvm_err)?;
                let out = self.alloca(self.cx.i64_type().into(), "parse.out")?;
                let f = if int_ty.signed() { self.rt.parse_i64 } else { self.rt.parse_u64 };
                let call = self.builder.build_call(f, &[ptr.into(), len.into(), out.into()], "").map_err(llvm_err)?;
                let mut ok = call.try_as_basic_value().basic().expect("parse returns a flag").into_int_value();
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("parse returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let raw = self.builder.build_load(self.cx.i64_type(), out, "").map_err(llvm_err)?.into_int_value();
                // The runtime parses into 64 bits; a number that does not fit the narrower type is an error, not a
                // truncated value.
                let mut narrowed = None;
                if let Ty::Int(i) = &**ok_ty
                    && i.bits() != 64
                {
                    let small = self.builder.build_int_truncate(raw, self.int_ty(*i), "").map_err(llvm_err)?;
                    let back = if i.signed() {
                        self.builder.build_int_s_extend(small, self.cx.i64_type(), "").map_err(llvm_err)?
                    } else {
                        self.builder.build_int_z_extend(small, self.cx.i64_type(), "").map_err(llvm_err)?
                    };
                    let fits = self.builder.build_int_compare(IntPredicate::EQ, back, raw, "").map_err(llvm_err)?;
                    ok = self.builder.build_and(ok, fits, "").map_err(llvm_err)?;
                    narrowed = Some(small);
                }
                let mut good =
                    self.builder.build_insert_value(rt.const_zero(), ok, 0, "").map_err(llvm_err)?.into_struct_value();
                if self.llvm_ty(ok_ty).is_some() {
                    let value = narrowed.unwrap_or(raw);
                    good = self.builder.build_insert_value(good, value, 1, "").map_err(llvm_err)?.into_struct_value();
                }
                return Ok(Some(good.into()));
            }
            Builtin::ReadToString => {
                let path = self.expr_mode(&args[0], Mode::Use)?;
                let path = self.need(path, &Ty::Str).into_struct_value();
                let ptr = self.builder.build_extract_value(path, 0, "").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(path, 1, "").map_err(llvm_err)?;
                let out = self.alloca(self.string_ty.into(), "read.string")?;
                self.builder.build_call(self.rt.string_new, &[out.into()], "").map_err(llvm_err)?;
                let call = self
                    .builder
                    .build_call(self.rt.read_to_string, &[ptr.into(), len.into(), out.into()], "")
                    .map_err(llvm_err)?;
                let code = call.try_as_basic_value().basic().expect("read returns a code").into_int_value();
                let ok = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, code, self.cx.i32_type().const_zero(), "read.ok")
                    .map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("read_to_string returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let slot = self.alloca(rt.into(), "read.out")?;
                let good = self.new_block("read.ok");
                let bad = self.new_block("read.err");
                let end = self.new_block("read.end");
                self.builder.build_conditional_branch(ok, good, bad).map_err(llvm_err)?;
                self.builder.position_at_end(good);
                let text = self.builder.build_load(self.string_ty, out, "").map_err(llvm_err)?;
                let mut value = self
                    .builder
                    .build_insert_value(rt.const_zero(), self.cx.bool_type().const_int(1, false), 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                value = self.builder.build_insert_value(value, text, 1, "").map_err(llvm_err)?.into_struct_value();
                self.builder.build_store(slot, value).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(bad);
                self.drop_value(out, &Ty::String)?;
                let mut failure = self
                    .builder
                    .build_insert_value(rt.const_zero(), self.cx.bool_type().const_zero(), 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                failure = self.builder.build_insert_value(failure, code, 2, "").map_err(llvm_err)?.into_struct_value();
                self.builder.build_store(slot, failure).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                return Ok(Some(self.builder.build_load(rt, slot, "").map_err(llvm_err)?));
            }
            Builtin::WriteFile => {
                let path = self.expr_mode(&args[0], Mode::Use)?;
                let path = self.need(path, &Ty::Str).into_struct_value();
                let text = self.expr_mode(&args[1], Mode::Use)?;
                let text = self.need(text, &Ty::Str).into_struct_value();
                let pptr = self.builder.build_extract_value(path, 0, "").map_err(llvm_err)?;
                let plen = self.builder.build_extract_value(path, 1, "").map_err(llvm_err)?;
                let tptr = self.builder.build_extract_value(text, 0, "").map_err(llvm_err)?;
                let tlen = self.builder.build_extract_value(text, 1, "").map_err(llvm_err)?;
                let call = self
                    .builder
                    .build_call(self.rt.write_file, &[pptr.into(), plen.into(), tptr.into(), tlen.into()], "")
                    .map_err(llvm_err)?;
                let code = call.try_as_basic_value().basic().expect("write returns a code").into_int_value();
                let ok = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, code, self.cx.i32_type().const_zero(), "write.ok")
                    .map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("write_file returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let mut value =
                    self.builder.build_insert_value(rt.const_zero(), ok, 0, "").map_err(llvm_err)?.into_struct_value();
                value = self.builder.build_insert_value(value, code, 2, "").map_err(llvm_err)?.into_struct_value();
                return Ok(Some(value.into()));
            }
            // The file-handle primitives (`spec/stdlib.md`, "Files and file handles"). Each reports a descriptor,
            // a byte count, a position, or zero for success; a negative result is the failure.
            Builtin::FileOpen => {
                let (ptr, len) = self.str_parts(&args[0])?;
                let mut values: Vec<BasicMetadataValueEnum> = vec![ptr.into(), len.into()];
                let i32_ty = self.cx.i32_type();
                for arg in &args[1..7] {
                    let v = self.expr_mode(arg, Mode::Use)?;
                    let v = self.need(v, &Ty::Bool).into_int_value();
                    let v = self.builder.build_int_z_extend(v, i32_ty, "").map_err(llvm_err)?;
                    values.push(v.into());
                }
                let fd = self
                    .builder
                    .build_call(self.rt.file_open, &values, "")
                    .map_err(llvm_err)?
                    .try_as_basic_value()
                    .basic()
                    .expect("open returns a descriptor")
                    .into_int_value();
                let ok = self
                    .builder
                    .build_int_compare(IntPredicate::SGE, fd, i32_ty.const_zero(), "opened")
                    .map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("file_open returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let mut value =
                    self.builder.build_insert_value(rt.const_zero(), ok, 0, "").map_err(llvm_err)?.into_struct_value();
                value = self.builder.build_insert_value(value, fd, 1, "").map_err(llvm_err)?.into_struct_value();
                return Ok(Some(value.into()));
            }
            Builtin::FileRead => {
                let fd = self.expr_mode(&args[0], Mode::Use)?;
                let fd = self.need_int(fd, &Ty::Int(burdin_tir::IntTy::I32));
                let Some(buf) = self.place_ptr(&args[1])? else {
                    return Err("the file read buffer is not a place".into());
                };
                let max = self.expr_mode(&args[2], Mode::Use)?;
                let max = self.need_int(max, &Ty::Int(burdin_tir::IntTy::Usize));
                let n = self
                    .builder
                    .build_call(self.rt.file_read, &[fd.into(), buf.into(), max.into()], "")
                    .map_err(llvm_err)?
                    .try_as_basic_value()
                    .basic()
                    .expect("file_read returns a count")
                    .into_int_value();
                let ok = self
                    .builder
                    .build_int_compare(IntPredicate::SGE, n, self.usize_ty.const_zero(), "read")
                    .map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("file_read returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let mut value =
                    self.builder.build_insert_value(rt.const_zero(), ok, 0, "").map_err(llvm_err)?.into_struct_value();
                value = self.builder.build_insert_value(value, n, 1, "").map_err(llvm_err)?.into_struct_value();
                return Ok(Some(value.into()));
            }
            Builtin::FileWrite => {
                let fd = self.expr_mode(&args[0], Mode::Use)?;
                let fd = self.need_int(fd, &Ty::Int(burdin_tir::IntTy::I32));
                let data = self.expr_mode(&args[1], Mode::Use)?;
                let data = self.need(data, &args[1].ty).into_struct_value();
                let ptr = self.builder.build_extract_value(data, 0, "").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(data, 1, "").map_err(llvm_err)?;
                let n = self
                    .builder
                    .build_call(self.rt.file_write, &[fd.into(), ptr.into(), len.into()], "")
                    .map_err(llvm_err)?
                    .try_as_basic_value()
                    .basic()
                    .expect("file_write returns a count")
                    .into_int_value();
                let ok = self
                    .builder
                    .build_int_compare(IntPredicate::SGE, n, self.usize_ty.const_zero(), "written")
                    .map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("file_write returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let mut value =
                    self.builder.build_insert_value(rt.const_zero(), ok, 0, "").map_err(llvm_err)?.into_struct_value();
                value = self.builder.build_insert_value(value, n, 1, "").map_err(llvm_err)?.into_struct_value();
                return Ok(Some(value.into()));
            }
            Builtin::FileSeek => {
                let fd = self.expr_mode(&args[0], Mode::Use)?;
                let fd = self.need_int(fd, &Ty::Int(burdin_tir::IntTy::I32));
                let offset = self.expr_mode(&args[1], Mode::Use)?;
                let offset = self.need_int(offset, &Ty::Int(burdin_tir::IntTy::I64));
                let whence = self.expr_mode(&args[2], Mode::Use)?;
                let whence = self.need_int(whence, &Ty::Int(burdin_tir::IntTy::I32));
                let position = self
                    .builder
                    .build_call(self.rt.file_seek, &[fd.into(), offset.into(), whence.into()], "")
                    .map_err(llvm_err)?
                    .try_as_basic_value()
                    .basic()
                    .expect("file_seek returns a position")
                    .into_int_value();
                let ok = self
                    .builder
                    .build_int_compare(IntPredicate::SGE, position, self.cx.i64_type().const_zero(), "position")
                    .map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("file_seek returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let mut value =
                    self.builder.build_insert_value(rt.const_zero(), ok, 0, "").map_err(llvm_err)?.into_struct_value();
                value = self.builder.build_insert_value(value, position, 1, "").map_err(llvm_err)?.into_struct_value();
                return Ok(Some(value.into()));
            }
            Builtin::FileError => {
                let code = self
                    .builder
                    .build_call(self.rt.file_error, &[], "file.error")
                    .map_err(llvm_err)?
                    .try_as_basic_value()
                    .basic()
                    .expect("file_error returns a code");
                return Ok(Some(code));
            }
            Builtin::FileSync | Builtin::FileSetLen | Builtin::FileClose => {
                let fd = self.expr_mode(&args[0], Mode::Use)?;
                let fd = self.need_int(fd, &Ty::Int(burdin_tir::IntTy::I32));
                let mut values: Vec<BasicMetadataValueEnum> = vec![fd.into()];
                if b == Builtin::FileSetLen {
                    let size = self.expr_mode(&args[1], Mode::Use)?;
                    let size = self.need_int(size, &Ty::Int(burdin_tir::IntTy::U64));
                    values.push(size.into());
                }
                let function = match b {
                    Builtin::FileSync => self.rt.file_sync,
                    Builtin::FileSetLen => self.rt.file_set_len,
                    _ => self.rt.file_close,
                };
                let status = self
                    .builder
                    .build_call(function, &values, "")
                    .map_err(llvm_err)?
                    .try_as_basic_value()
                    .basic()
                    .expect("a file operation returns a status")
                    .into_int_value();
                let ok = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, status, self.cx.i32_type().const_zero(), "ok")
                    .map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("a file operation returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let value =
                    self.builder.build_insert_value(rt.const_zero(), ok, 0, "").map_err(llvm_err)?.into_struct_value();
                return Ok(Some(value.into()));
            }
            Builtin::StdoutWrite
            | Builtin::StdoutWriteLine
            | Builtin::StderrWrite
            | Builtin::StderrWriteLine
            | Builtin::StdoutWriteBytes
            | Builtin::StderrWriteBytes => {
                // A `str` and a `[u8]` are the same `{ ptr, len }` pair, so one path writes both.
                let text = self.expr_mode(&args[0], Mode::Use)?;
                let text = self.need(text, &args[0].ty).into_struct_value();
                let ptr = self.builder.build_extract_value(text, 0, "ptr").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(text, 1, "len").map_err(llvm_err)?;
                let (fd, newline) = match b {
                    Builtin::StdoutWrite | Builtin::StdoutWriteBytes => (1, 0),
                    Builtin::StdoutWriteLine => (1, 1),
                    Builtin::StderrWrite | Builtin::StderrWriteBytes => (2, 0),
                    Builtin::StderrWriteLine => (2, 1),
                    _ => unreachable!("only the four write builtins match"),
                };
                let i32_ty = self.cx.i32_type();
                let args: [BasicMetadataValueEnum; 4] = [
                    i32_ty.const_int(fd, false).into(),
                    ptr.into(),
                    len.into(),
                    i32_ty.const_int(newline, false).into(),
                ];
                self.builder.build_call(self.rt.print, &args, "").map_err(llvm_err)?;
                return Ok(None);
            }
            Builtin::StdinAvail => {
                let avail = self
                    .builder
                    .build_call(self.rt.in_avail, &[], "avail")
                    .map_err(llvm_err)?
                    .try_as_basic_value()
                    .basic()
                    .expect("in_avail returns a count");
                return Ok(Some(avail));
            }
            Builtin::StdinNextByte => {
                let byte = self
                    .builder
                    .build_call(self.rt.in_next, &[], "byte")
                    .map_err(llvm_err)?
                    .try_as_basic_value()
                    .basic()
                    .expect("in_next returns a byte");
                return Ok(Some(byte));
            }
            Builtin::StdinError => {
                let code = self
                    .builder
                    .build_call(self.rt.in_error, &[], "stdin.error")
                    .map_err(llvm_err)?
                    .try_as_basic_value()
                    .basic()
                    .expect("in_error returns a code");
                return Ok(Some(code));
            }
            Builtin::StdoutFlush => {
                self.builder.build_call(self.rt.flush, &[], "").map_err(llvm_err)?;
                return Ok(None);
            }
            Builtin::IsTerminal => {
                let fd = self.expr_mode(&args[0], Mode::Use)?;
                let fd = self.need_int(fd, &Ty::Int(burdin_tir::IntTy::I32));
                let tty = self
                    .builder
                    .build_call(self.rt.is_terminal, &[fd.into()], "tty")
                    .map_err(llvm_err)?
                    .try_as_basic_value()
                    .basic()
                    .expect("is_terminal returns a bool");
                return Ok(Some(tty));
            }
            Builtin::StdinReadLine => {
                let out = self.alloca(self.string_ty.into(), "stdin.line")?;
                self.builder.build_call(self.rt.string_new, &[out.into()], "").map_err(llvm_err)?;
                let err_slot = self.alloca(self.cx.i32_type().into(), "stdin.err")?;
                let status = self
                    .builder
                    .build_call(self.rt.in_read_line, &[out.into(), err_slot.into()], "status")
                    .map_err(llvm_err)?
                    .try_as_basic_value()
                    .basic()
                    .expect("read_line returns a status")
                    .into_int_value();
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("read_line returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let slot = self.alloca(rt.into(), "stdin.out")?;
                let i32_ty = self.cx.i32_type();
                let good = self.new_block("stdin.ok");
                let bad = self.new_block("stdin.err");
                let end = self.new_block("stdin.end");
                let failed = self
                    .builder
                    .build_int_compare(IntPredicate::SLT, status, i32_ty.const_zero(), "failed")
                    .map_err(llvm_err)?;
                self.builder.build_conditional_branch(failed, bad, good).map_err(llvm_err)?;
                self.builder.position_at_end(good);
                let text = self.builder.build_load(self.string_ty, out, "").map_err(llvm_err)?;
                // The runtime's 1 is the end of input with nothing read; a line of zero bytes is still a line.
                let Ty::Option(inner) = &**ok_ty else { unreachable!("read_line returns Option<String>") };
                let eof = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, status, i32_ty.const_int(1, false), "eof")
                    .map_err(llvm_err)?;
                let some = self.builder.build_not(eof, "some").map_err(llvm_err)?;
                let opt_ty = self.option_ty(inner);
                let option = self
                    .builder
                    .build_insert_value(opt_ty.const_zero(), some, 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                let option =
                    self.builder.build_insert_value(option, text, 1, "").map_err(llvm_err)?.into_struct_value();
                let mut value = self
                    .builder
                    .build_insert_value(rt.const_zero(), self.cx.bool_type().const_int(1, false), 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                value = self.builder.build_insert_value(value, option, 1, "").map_err(llvm_err)?.into_struct_value();
                self.builder.build_store(slot, value).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(bad);
                self.drop_value(out, &Ty::String)?;
                let code = self.builder.build_load(self.cx.i32_type(), err_slot, "stdin.code").map_err(llvm_err)?;
                let mut failure = self
                    .builder
                    .build_insert_value(rt.const_zero(), self.cx.bool_type().const_zero(), 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                failure = self.builder.build_insert_value(failure, code, 2, "").map_err(llvm_err)?.into_struct_value();
                self.builder.build_store(slot, failure).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                return Ok(Some(self.builder.build_load(rt, slot, "").map_err(llvm_err)?));
            }
            Builtin::StdinReadToString => {
                let out = self.alloca(self.string_ty.into(), "stdin.text")?;
                self.builder.build_call(self.rt.string_new, &[out.into()], "").map_err(llvm_err)?;
                let err_slot = self.alloca(self.cx.i32_type().into(), "stdin.err")?;
                let status = self
                    .builder
                    .build_call(self.rt.in_read_to_string, &[out.into(), err_slot.into()], "status")
                    .map_err(llvm_err)?
                    .try_as_basic_value()
                    .basic()
                    .expect("read_to_string returns a status")
                    .into_int_value();
                let ok = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, status, self.cx.i32_type().const_zero(), "ok")
                    .map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("read_to_string returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let slot = self.alloca(rt.into(), "stdin.out")?;
                let good = self.new_block("stdin.ok");
                let bad = self.new_block("stdin.err");
                let end = self.new_block("stdin.end");
                self.builder.build_conditional_branch(ok, good, bad).map_err(llvm_err)?;
                self.builder.position_at_end(good);
                let text = self.builder.build_load(self.string_ty, out, "").map_err(llvm_err)?;
                let mut value = self
                    .builder
                    .build_insert_value(rt.const_zero(), self.cx.bool_type().const_int(1, false), 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                value = self.builder.build_insert_value(value, text, 1, "").map_err(llvm_err)?.into_struct_value();
                self.builder.build_store(slot, value).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(bad);
                self.drop_value(out, &Ty::String)?;
                let code = self.builder.build_load(self.cx.i32_type(), err_slot, "stdin.code").map_err(llvm_err)?;
                let mut failure = self
                    .builder
                    .build_insert_value(rt.const_zero(), self.cx.bool_type().const_zero(), 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                failure = self.builder.build_insert_value(failure, code, 2, "").map_err(llvm_err)?.into_struct_value();
                self.builder.build_store(slot, failure).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                return Ok(Some(self.builder.build_load(rt, slot, "").map_err(llvm_err)?));
            }
            // The directory and metadata primitives (`spec/stdlib.md`, issue #411). Each reports the operating
            // system's error code, with zero for success; the `std` wrappers build the `IoError`.
            Builtin::ReadDir => {
                let (ptr, len) = self.str_parts(&args[0])?;
                let out = self.alloca(self.string_ty.into(), "dir.list")?;
                let call = self
                    .builder
                    .build_call(self.rt.read_dir, &[ptr.into(), len.into(), out.into()], "")
                    .map_err(llvm_err)?;
                let status = call.try_as_basic_value().basic().expect("read_dir returns a status").into_int_value();
                let zero = self.cx.i32_type().const_zero();
                let ok = self.builder.build_int_compare(IntPredicate::EQ, status, zero, "").map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("read_dir returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let slot = self.alloca(rt.into(), "dir.out")?;
                let good = self.new_block("dir.ok");
                let bad = self.new_block("dir.err");
                let end = self.new_block("dir.end");
                self.builder.build_conditional_branch(ok, good, bad).map_err(llvm_err)?;
                self.builder.position_at_end(good);
                let list = self.builder.build_load(self.string_ty, out, "").map_err(llvm_err)?;
                let mut value = self
                    .builder
                    .build_insert_value(rt.const_zero(), self.cx.bool_type().const_int(1, false), 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                value = self.builder.build_insert_value(value, list, 1, "").map_err(llvm_err)?.into_struct_value();
                self.builder.build_store(slot, value).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(bad);
                // The runtime leaves an empty list behind, so dropping it is safe and frees nothing.
                self.drop_value(out, &Ty::List(Box::new(Ty::String), burdin_tir::Alloc::Global))?;
                let mut failure = self
                    .builder
                    .build_insert_value(rt.const_zero(), self.cx.bool_type().const_zero(), 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                failure =
                    self.builder.build_insert_value(failure, status, 2, "").map_err(llvm_err)?.into_struct_value();
                self.builder.build_store(slot, failure).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                return Ok(Some(self.builder.build_load(rt, slot, "").map_err(llvm_err)?));
            }
            Builtin::CreateDir | Builtin::RemoveDir | Builtin::RemoveFile | Builtin::RenamePath => {
                let (ptr, len) = self.str_parts(&args[0])?;
                let mut values: Vec<BasicMetadataValueEnum> = vec![ptr.into(), len.into()];
                if b == Builtin::RenamePath {
                    let (to, to_len) = self.str_parts(&args[1])?;
                    values.push(to.into());
                    values.push(to_len.into());
                }
                let function = match b {
                    Builtin::CreateDir => self.rt.create_dir,
                    Builtin::RemoveDir => self.rt.remove_dir,
                    Builtin::RemoveFile => self.rt.remove_file,
                    _ => self.rt.rename,
                };
                let call = self.builder.build_call(function, &values, "").map_err(llvm_err)?;
                let status =
                    call.try_as_basic_value().basic().expect("a path change returns a status").into_int_value();
                let zero = self.cx.i32_type().const_zero();
                let ok = self.builder.build_int_compare(IntPredicate::EQ, status, zero, "").map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("a path change returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let mut value =
                    self.builder.build_insert_value(rt.const_zero(), ok, 0, "").map_err(llvm_err)?.into_struct_value();
                value = self.builder.build_insert_value(value, status, 2, "").map_err(llvm_err)?.into_struct_value();
                return Ok(Some(value.into()));
            }
            Builtin::Metadata => {
                let (ptr, len) = self.str_parts(&args[0])?;
                let raw = self.alloca(self.cx.i64_type().array_type(5).into(), "meta.raw")?;
                let call = self
                    .builder
                    .build_call(self.rt.metadata, &[ptr.into(), len.into(), raw.into()], "")
                    .map_err(llvm_err)?;
                let status = call.try_as_basic_value().basic().expect("metadata returns a status").into_int_value();
                let zero = self.cx.i32_type().const_zero();
                let ok = self.builder.build_int_compare(IntPredicate::EQ, status, zero, "").map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("metadata returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let slot = self.alloca(rt.into(), "meta.out")?;
                let good = self.new_block("meta.ok");
                let bad = self.new_block("meta.err");
                let end = self.new_block("meta.end");
                self.builder.build_conditional_branch(ok, good, bad).map_err(llvm_err)?;
                self.builder.position_at_end(good);
                let tuple_ty = self.llvm_ty(ok_ty).expect("a tuple has a value");
                let mut tuple = undef(tuple_ty).into_struct_value();
                for i in 0..5u64 {
                    let at = self.offset_ptr(raw, self.usize_ty.const_int(i * 8, false))?;
                    let field = self.builder.build_load(self.cx.i64_type(), at, "").map_err(llvm_err)?;
                    tuple = self
                        .builder
                        .build_insert_value(tuple, field, i as u32, "")
                        .map_err(llvm_err)?
                        .into_struct_value();
                }
                let mut value = self
                    .builder
                    .build_insert_value(rt.const_zero(), self.cx.bool_type().const_int(1, false), 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                value = self.builder.build_insert_value(value, tuple, 1, "").map_err(llvm_err)?.into_struct_value();
                self.builder.build_store(slot, value).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(bad);
                let mut failure = self
                    .builder
                    .build_insert_value(rt.const_zero(), self.cx.bool_type().const_zero(), 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                failure =
                    self.builder.build_insert_value(failure, status, 2, "").map_err(llvm_err)?.into_struct_value();
                self.builder.build_store(slot, failure).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                return Ok(Some(self.builder.build_load(rt, slot, "").map_err(llvm_err)?));
            }
            Builtin::Canonicalize => {
                let (ptr, len) = self.str_parts(&args[0])?;
                let out = self.alloca(self.string_ty.into(), "canon.string")?;
                let call = self
                    .builder
                    .build_call(self.rt.canonicalize, &[ptr.into(), len.into(), out.into()], "")
                    .map_err(llvm_err)?;
                let status = call.try_as_basic_value().basic().expect("canonicalize returns a status").into_int_value();
                let zero = self.cx.i32_type().const_zero();
                let ok = self.builder.build_int_compare(IntPredicate::EQ, status, zero, "").map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("canonicalize returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let slot = self.alloca(rt.into(), "canon.out")?;
                let good = self.new_block("canon.ok");
                let bad = self.new_block("canon.err");
                let end = self.new_block("canon.end");
                self.builder.build_conditional_branch(ok, good, bad).map_err(llvm_err)?;
                self.builder.position_at_end(good);
                let text = self.builder.build_load(self.string_ty, out, "").map_err(llvm_err)?;
                let mut value = self
                    .builder
                    .build_insert_value(rt.const_zero(), self.cx.bool_type().const_int(1, false), 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                value = self.builder.build_insert_value(value, text, 1, "").map_err(llvm_err)?.into_struct_value();
                self.builder.build_store(slot, value).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(bad);
                self.drop_value(out, &Ty::String)?;
                let mut failure = self
                    .builder
                    .build_insert_value(rt.const_zero(), self.cx.bool_type().const_zero(), 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                failure =
                    self.builder.build_insert_value(failure, status, 2, "").map_err(llvm_err)?.into_struct_value();
                self.builder.build_store(slot, failure).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                return Ok(Some(self.builder.build_load(rt, slot, "").map_err(llvm_err)?));
            }
            Builtin::Clock => {
                let kind = self.expr_mode(&args[0], Mode::Use)?;
                let kind = self.need_int(kind, &Ty::Int(burdin_tir::IntTy::I32));
                let call = self.builder.build_call(self.rt.clock, &[kind.into()], "").map_err(llvm_err)?;
                return Ok(Some(call.try_as_basic_value().basic().expect("clock returns nanoseconds")));
            }
            Builtin::Entropy => {
                let call = self.builder.build_call(self.rt.entropy, &[], "").map_err(llvm_err)?;
                return Ok(Some(call.try_as_basic_value().basic().expect("entropy returns bits")));
            }
            Builtin::Sleep => {
                let nanos = self.expr_mode(&args[0], Mode::Use)?;
                let nanos = self.need_int(nanos, &Ty::Int(burdin_tir::IntTy::U64));
                self.builder.build_call(self.rt.sleep, &[nanos.into()], "").map_err(llvm_err)?;
                return Ok(None);
            }
            Builtin::EnvArgs => {
                let out = self.alloca(self.string_ty.into(), "args.list")?;
                let out = self
                    .builder
                    .build_pointer_cast(out, self.cx.ptr_type(inkwell::AddressSpace::default()), "")
                    .map_err(llvm_err)?;
                self.builder.build_call(self.rt.args, &[out.into()], "").map_err(llvm_err)?;
                return Ok(Some(self.builder.build_load(self.string_ty, out, "").map_err(llvm_err)?));
            }
            Builtin::EnvGet | Builtin::CurrentDir => {
                let out = self.alloca(self.string_ty.into(), "env.out")?;
                self.builder.build_store(out, self.string_ty.const_zero()).map_err(llvm_err)?;
                let call = if b == Builtin::EnvGet {
                    let s = self.expr_mode(&args[0], Mode::Use)?;
                    let s = self.need(s, &Ty::Str).into_struct_value();
                    let ptr = self.builder.build_extract_value(s, 0, "env.name").map_err(llvm_err)?;
                    let len = self.builder.build_extract_value(s, 1, "env.name.len").map_err(llvm_err)?;
                    self.builder.build_call(self.rt.env_get, &[ptr.into(), len.into(), out.into()], "")
                } else {
                    self.builder.build_call(self.rt.current_dir, &[out.into()], "")
                }
                .map_err(llvm_err)?;
                let found = call.try_as_basic_value().basic().expect("the read returns a flag").into_int_value();
                let found = self
                    .builder
                    .build_int_compare(IntPredicate::NE, found, self.cx.i32_type().const_zero(), "")
                    .map_err(llvm_err)?;
                let list = self.builder.build_load(self.string_ty, out, "env.bytes").map_err(llvm_err)?;
                let inner = Ty::List(Box::new(Ty::Int(burdin_tir::IntTy::U8)), burdin_tir::Alloc::Global);
                let opt_ty = self.option_ty(&inner);
                let agg = self
                    .builder
                    .build_insert_value(opt_ty.const_zero(), found, 0, "env.some")
                    .map_err(llvm_err)?
                    .into_struct_value();
                let agg =
                    self.builder.build_insert_value(agg, list, 1, "env.value").map_err(llvm_err)?.into_struct_value();
                return Ok(Some(agg.into()));
            }
            Builtin::EnvSet | Builtin::EnvSetBytes => {
                let name = self.expr_mode(&args[0], Mode::Use)?;
                let name = self.need(name, &Ty::Str).into_struct_value();
                let nptr = self.builder.build_extract_value(name, 0, "env.name").map_err(llvm_err)?;
                let nlen = self.builder.build_extract_value(name, 1, "env.name.len").map_err(llvm_err)?;
                let value_ty = args[1].ty.clone();
                let value = self.expr_mode(&args[1], Mode::Use)?;
                let value = self.need(value, &value_ty).into_struct_value();
                let vptr = self.builder.build_extract_value(value, 0, "env.value").map_err(llvm_err)?;
                let vlen = self.builder.build_extract_value(value, 1, "env.value.len").map_err(llvm_err)?;
                let call = self
                    .builder
                    .build_call(self.rt.env_set, &[nptr.into(), nlen.into(), vptr.into(), vlen.into()], "")
                    .map_err(llvm_err)?;
                let status = call.try_as_basic_value().basic().expect("env_set returns a status");
                return Ok(Some(status));
            }
            Builtin::EnvRemove => {
                let name = self.expr_mode(&args[0], Mode::Use)?;
                let name = self.need(name, &Ty::Str).into_struct_value();
                let ptr = self.builder.build_extract_value(name, 0, "env.name").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(name, 1, "env.name.len").map_err(llvm_err)?;
                let call =
                    self.builder.build_call(self.rt.env_remove, &[ptr.into(), len.into()], "").map_err(llvm_err)?;
                return Ok(call.try_as_basic_value().basic());
            }
            Builtin::SetCurrentDir | Builtin::SetCurrentDirBytes => {
                let ty = args[0].ty.clone();
                let path = self.expr_mode(&args[0], Mode::Use)?;
                let path = self.need(path, &ty).into_struct_value();
                let ptr = self.builder.build_extract_value(path, 0, "cwd.path").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(path, 1, "cwd.path.len").map_err(llvm_err)?;
                let call = self
                    .builder
                    .build_call(self.rt.set_current_dir, &[ptr.into(), len.into()], "")
                    .map_err(llvm_err)?;
                return Ok(call.try_as_basic_value().basic());
            }
            Builtin::ProcessExit => {
                let code = self.expr_mode(&args[0], Mode::Use)?;
                let code = self.need_int(code, &Ty::Int(burdin_tir::IntTy::I32));
                self.builder.build_call(self.rt.exit, &[code.into()], "").map_err(llvm_err)?;
                self.builder.build_unreachable().map_err(llvm_err)?;
                self.continue_dead();
                return Ok(None);
            }
            Builtin::Assert => {
                let cond = self.expr_mode(&args[0], Mode::Use)?;
                let cond = self.need_int(cond, &Ty::Bool);
                self.check(cond, "assertion failed", span)?;
                return Ok(None);
            }
            Builtin::AssertEq => {
                let ty = args[0].ty.clone();
                let l = self.expr_mode(&args[0], Mode::Use)?;
                let r = self.expr_mode(&args[1], Mode::Use)?;
                let lp = self.spill(l, &ty)?;
                let rp = self.spill(r, &ty)?;
                if let (Some(lp), Some(rp)) = (lp, rp) {
                    let eq = self.eq_at(lp, rp, &ty)?;
                    self.check(eq, "assertion failed: the two values differ", span)?;
                }
                return Ok(None);
            }
            Builtin::Unreachable => {
                let msg = self.str_literal(b"unreachable");
                let ptr = msg.get_field_at_index(0).expect("str has a pointer");
                let len = msg.get_field_at_index(1).expect("str has a length");
                self.emit_panic(ptr, len, span)?;
                self.continue_dead();
                return Ok(None);
            }
            _ => {}
        }
        let s = self.expr_mode(&args[0], Mode::Use)?;
        let s = self.need(s, &Ty::Str).into_struct_value();
        let ptr = self.builder.build_extract_value(s, 0, "ptr").map_err(llvm_err)?;
        let len = self.builder.build_extract_value(s, 1, "len").map_err(llvm_err)?;
        let (fd, newline) = match b {
            Builtin::Print => (1, 0),
            Builtin::Println => (1, 1),
            Builtin::Eprint => (2, 0),
            Builtin::Eprintln => (2, 1),
            Builtin::Panic => {
                self.emit_panic(ptr, len, span)?;
                self.continue_dead();
                return Ok(None);
            }
            _ => unreachable!("handled above"),
        };
        let i32_ty = self.cx.i32_type();
        let args: [BasicMetadataValueEnum; 4] =
            [i32_ty.const_int(fd, false).into(), ptr.into(), len.into(), i32_ty.const_int(newline, false).into()];
        self.builder.build_call(self.rt.print, &args, "").map_err(llvm_err)?;
        Ok(None)
    }

    pub(super) fn method(
        &mut self,
        m: Method,
        args: &'a [tir::Expr],
        ty: &Ty,
        span: Span,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        if let Method::Float(op) = m {
            return self.float_method(op, args);
        }
        // `iter_mut` builds the same value as `iter`: the mutable iterators share the borrowing ones' layout.
        if matches!(m, Method::Iter | Method::IterMut | Method::IntoIter) {
            return self.iter_value(args, m == Method::IntoIter);
        }
        if m == Method::IterNext {
            return self.iter_next(args, ty);
        }
        if m == Method::IterNextBack {
            return self.iter_next_back(args, ty);
        }
        if m == Method::AsPtr {
            // A `CString` receiver that is not a place is a temporary of the statement: keep it in a slot so its
            // buffer is freed at the end of the statement, like `view_parts` does for a temporary `String`.
            let recv_ty = args[0].ty.clone();
            let ptr_ty = self.cx.ptr_type(inkwell::AddressSpace::default());
            let storage = match self.direct_place_ptr(&args[0])? {
                Some(ptr) => ptr,
                None => {
                    let v = self.expr_mode(&args[0], Mode::Use)?;
                    let slot = self.alloca(ptr_ty.into(), "cstring")?;
                    let v = self.need(v, &recv_ty);
                    self.builder.build_store(slot, v).map_err(llvm_err)?;
                    self.add_temp_slot(slot, &recv_ty);
                    slot
                }
            };
            let p = self.builder.build_load(ptr_ty, storage, "cstring.ptr").map_err(llvm_err)?;
            return Ok(Some(p));
        }
        if m == Method::SharedGet {
            let p = self.shared_pointer(&args[0])?;
            let eight = self.usize_ty.const_int(8, false);
            return Ok(Some(self.offset_ptr(p, eight)?.into()));
        }
        if m == Method::SharedStrongCount {
            let p = self.shared_pointer(&args[0])?;
            let count = self.builder.build_load(self.usize_ty, p, "shared.count").map_err(llvm_err)?;
            return Ok(Some(count));
        }
        if m == Method::Clone {
            let recv_ty = args[0].ty.clone();
            // A place is cloned from its storage. A reference dereference is a place even when the reference
            // came from a call (a user `Index`), so the referent is read without a temporary drop
            // (`spec/operators.md`, OP9). A non-place receiver is evaluated once into a temporary.
            let src = match self.place_ptr(&args[0])? {
                Some(ptr) => ptr,
                None => {
                    let v = self.expr_mode(&args[0], Mode::Use)?;
                    let t = self.llvm_ty(&recv_ty).expect("a cloned value has a type");
                    let slot = self.alloca(t, "clone.src")?;
                    let v = self.need(v, &recv_ty);
                    self.builder.build_store(slot, v).map_err(llvm_err)?;
                    self.add_temp_slot(slot, &recv_ty);
                    slot
                }
            };
            let t = self.llvm_ty(ty).expect("a cloned value has a type");
            let dst = self.alloca(t, "clone.dst")?;
            let glue = self.clone_glue(ty);
            self.builder.build_call(glue, &[dst.into(), src.into()], "").map_err(llvm_err)?;
            return Ok(Some(self.builder.build_load(t, dst, "").map_err(llvm_err)?));
        }
        if m == Method::IsNull {
            let v = self.expr_mode(&args[0], Mode::Use)?;
            let p = self.need(v, &args[0].ty).into_pointer_value();
            let is = self.builder.build_is_null(p, "").map_err(llvm_err)?;
            return Ok(Some(is.into()));
        }
        if m == Method::Hash {
            let recv_ty = args[0].ty.clone();
            let v = self.expr_mode(&args[0], Mode::Use)?;
            let start = self.cx.i64_type().const_int(0xcbf2_9ce4_8422_2325, false);
            let hash = match self.spill(v, &recv_ty)? {
                Some(ptr) => self.hash_at(ptr, &recv_ty, start)?,
                None => start,
            };
            return Ok(Some(hash.into()));
        }
        if m == Method::PushChar {
            let recv = self.place_ptr(&args[0])?.ok_or("`push` needs a place receiver")?;
            let c = self.expr_mode(&args[1], Mode::Use)?;
            let c = self.need_int(c, &Ty::Char);
            self.builder.build_call(self.rt.string_push_char, &[recv.into(), c.into()], "").map_err(llvm_err)?;
            return Ok(None);
        }
        if m == Method::Chars || m == Method::AsBytes {
            // The characters and the bytes are both a view of the text's storage (`[u8]` and `str` share the
            // pointer-and-length layout).
            let (ptr, len) = self.view_parts(&args[0])?;
            return Ok(Some(self.make_str(ptr, len)?.into()));
        }
        if m == Method::FindStr {
            let (ptr, len) = self.view_parts(&args[0])?;
            let needle = self.expr_mode(&args[1], Mode::Use)?;
            let needle = self.need(needle, &Ty::Str).into_struct_value();
            let nptr = self.builder.build_extract_value(needle, 0, "").map_err(llvm_err)?;
            let nlen = self.builder.build_extract_value(needle, 1, "").map_err(llvm_err)?;
            let call = self
                .builder
                .build_call(self.rt.str_find_str, &[ptr.into(), len.into(), nptr.into(), nlen.into()], "")
                .map_err(llvm_err)?;
            let idx = call.try_as_basic_value().basic().expect("`find_str` returns an index").into_int_value();
            let i64_ty = self.cx.i64_type();
            let found =
                self.builder.build_int_compare(IntPredicate::SGE, idx, i64_ty.const_zero(), "").map_err(llvm_err)?;
            let Ty::Option(inner) = ty else { unreachable!("`find_str` returns an `Option`") };
            let opt_ty = self.option_ty(inner);
            let payload = self.builder.build_int_truncate(idx, self.usize_ty, "").map_err(llvm_err)?;
            let agg = self
                .builder
                .build_insert_value(opt_ty.const_zero(), found, 0, "")
                .map_err(llvm_err)?
                .into_struct_value();
            let agg = self.builder.build_insert_value(agg, payload, 1, "").map_err(llvm_err)?.into_struct_value();
            return Ok(Some(agg.into()));
        }
        if matches!(
            m,
            Method::Push
                | Method::ListPop
                | Method::ListLast
                | Method::ListFirst
                | Method::ListFirstMut
                | Method::ListLastMut
                | Method::ListGet
                | Method::ListGetMut
                | Method::ListCapacity
                | Method::ListIsEmpty
                | Method::ListReserve
                | Method::ListReserveExact
                | Method::ListShrinkToFit
                | Method::ListSwap
                | Method::ListSwapRemove
                | Method::ListTruncate
                | Method::ListSplitOff
                | Method::ListAppend
                | Method::ListDrain
                | Method::ListExtendFromSlice
                | Method::ListInsert
                | Method::ListRemove
                | Method::ListContains
                | Method::ListSort
                | Method::Clear
                | Method::MapInsert
                | Method::MapGetOrInsert
                | Method::MapGetOr
                | Method::MapEntry
                | Method::MapEntryOrInsert
                | Method::MapEntryOrInsertWith
                | Method::MapGet
                | Method::MapGetMut
                | Method::MapRemove
                | Method::MapRemoveEntry
                | Method::MapRetain
                | Method::MapReserve
                | Method::MapShrinkToFit
                | Method::MapContainsKey
        ) {
            return self.collection_method(m, args, ty, span);
        }
        if matches!(m, Method::Unwrap | Method::Expect | Method::IsSome | Method::IsNone | Method::IsOk | Method::IsErr)
        {
            return self.result_method(m, args, ty, span);
        }
        if m == Method::PushStr {
            let recv = self.place_ptr(&args[0])?.ok_or("`push_str` needs a place receiver")?;
            let s = self.expr_mode(&args[1], Mode::Use)?;
            let s = self.need(s, &Ty::Str).into_struct_value();
            let ptr = self.builder.build_extract_value(s, 0, "ptr").map_err(llvm_err)?;
            let len = self.builder.build_extract_value(s, 1, "len").map_err(llvm_err)?;
            self.builder
                .build_call(self.rt.string_push_str, &[recv.into(), ptr.into(), len.into()], "")
                .map_err(llvm_err)?;
            return Ok(None);
        }
        if m == Method::AsStr {
            let (ptr, len) = self.view_parts(&args[0])?;
            return Ok(Some(self.make_str(ptr, len)?.into()));
        }
        if m == Method::Len {
            let recv_ty = args[0].ty.clone();
            let v: BasicValueEnum<'ctx> = match &recv_ty {
                Ty::Array(_, n) => {
                    // Evaluate the receiver once; a temporary array drops at the end of the statement.
                    self.place_ptr(&args[0])?;
                    self.usize_ty.const_int(*n, false).into()
                }
                Ty::Str | Ty::String | Ty::Slice(_) => {
                    let (_, len) = self.view_parts(&args[0])?;
                    len
                }
                Ty::List(_, _) | Ty::Map(..) => {
                    let (_, len) = self.view_parts(&args[0])?;
                    len
                }
                _ => return Err(format!("unsupported method `len` on `{recv_ty}`")),
            };
            return Ok(Some(v));
        }
        if m == Method::StartsWith {
            let (ptr, len) = self.view_parts(&args[0])?;
            let prefix = self.expr_mode(&args[1], Mode::Use)?;
            let prefix = self.need(prefix, &Ty::Str).into_struct_value();
            let pptr = self.builder.build_extract_value(prefix, 0, "").map_err(llvm_err)?;
            let plen = self.builder.build_extract_value(prefix, 1, "").map_err(llvm_err)?;
            let call = self
                .builder
                .build_call(self.rt.str_starts_with, &[ptr.into(), len.into(), pptr.into(), plen.into()], "")
                .map_err(llvm_err)?;
            return Ok(call.try_as_basic_value().basic());
        }
        if m == Method::Find {
            let (ptr, len) = self.view_parts(&args[0])?;
            let c = self.expr_mode(&args[1], Mode::Use)?;
            let c = self.need(c, &Ty::Char).into_int_value();
            let call = self
                .builder
                .build_call(self.rt.str_find_char, &[ptr.into(), len.into(), c.into()], "")
                .map_err(llvm_err)?;
            let idx = call.try_as_basic_value().basic().expect("`find` returns an index").into_int_value();
            let i64_ty = self.cx.i64_type();
            let found =
                self.builder.build_int_compare(IntPredicate::SGE, idx, i64_ty.const_zero(), "").map_err(llvm_err)?;
            let Ty::Option(inner) = ty else { unreachable!("`find` returns an `Option`") };
            let opt_ty = self.option_ty(inner);
            let payload = self.builder.build_int_truncate(idx, self.usize_ty, "").map_err(llvm_err)?;
            let agg = self
                .builder
                .build_insert_value(opt_ty.const_zero(), found, 0, "")
                .map_err(llvm_err)?
                .into_struct_value();
            let agg = self.builder.build_insert_value(agg, payload, 1, "").map_err(llvm_err)?.into_struct_value();
            return Ok(Some(agg.into()));
        }
        let recv_ty = &args[0].ty;
        let mut values = Vec::new();
        for a in args {
            let v = self.expr_mode(a, Mode::Use)?;
            values.push(self.need(v, &a.ty));
        }
        let recv = values[0];
        let v: BasicValueEnum<'ctx> = match (m, recv_ty) {
            (Method::ToString, _) => self.string_of(recv, recv_ty)?,
            (Method::WrappingAdd | Method::WrappingSub | Method::WrappingMul, _) => {
                let op = wrapping_op(m);
                self.wrapping(op, recv.into_int_value(), values[1].into_int_value())?.into()
            }
            (Method::CheckedAdd | Method::CheckedSub | Method::CheckedMul, Ty::Int(i)) => {
                let op = wrapping_op(m);
                let (r, overflow) = self.with_overflow(op, recv.into_int_value(), values[1].into_int_value(), *i)?;
                let Ty::Option(inner) = ty else { unreachable!("checked_* returns an Option") };
                let opt_ty = self.option_ty(inner);
                let is_some = self.builder.build_not(overflow, "").map_err(llvm_err)?;
                let agg = self
                    .builder
                    .build_insert_value(undef(opt_ty.into()).into_struct_value(), is_some, 0, "")
                    .map_err(llvm_err)?;
                self.builder.build_insert_value(agg, r, 1, "").map_err(llvm_err)?.into_struct_value().into()
            }
            (Method::Min | Method::Max, Ty::Int(i)) => {
                let (x, y) = (recv.into_int_value(), values[1].into_int_value());
                let pred = match (m, i.signed()) {
                    (Method::Min, true) => IntPredicate::SLT,
                    (Method::Min, false) => IntPredicate::ULT,
                    (_, true) => IntPredicate::SGT,
                    (_, false) => IntPredicate::UGT,
                };
                let c = self.builder.build_int_compare(pred, x, y, "").map_err(llvm_err)?;
                self.builder.build_select(c, x, y, "").map_err(llvm_err)?
            }
            (Method::Abs, Ty::Int(i)) => {
                let x = recv.into_int_value();
                let neg = self.negate(x, *i, span)?;
                let is_neg = self
                    .builder
                    .build_int_compare(IntPredicate::SLT, x, x.get_type().const_zero(), "")
                    .map_err(llvm_err)?;
                self.builder.build_select(is_neg, neg, x, "").map_err(llvm_err)?
            }
            _ => return Err(format!("unsupported method {m:?} on `{recv_ty}`")),
        };
        Ok(Some(v))
    }

    /// The floating-point methods the compiler provides (`spec/stdlib.md`, "Float functions"). An exactly
    /// rounded operation lowers to an LLVM intrinsic; one with no intrinsic calls the C math library.
    fn float_method(&mut self, op: FloatOp, args: &'a [tir::Expr]) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let ft = match &args[0].ty {
            Ty::Float(f) => *f,
            other => return Err(format!("a floating-point method on `{other}`")),
        };
        let mut values = Vec::with_capacity(args.len());
        for a in args {
            let v = self.expr_mode(a, Mode::Use)?;
            values.push(self.need(v, &a.ty));
        }
        let t: BasicTypeEnum<'ctx> = match ft {
            FloatTy::F32 => self.cx.f32_type().into(),
            FloatTy::F64 => self.cx.f64_type().into(),
        };
        if op == FloatOp::ToBits {
            let bits_ty: BasicTypeEnum<'ctx> = match ft {
                FloatTy::F32 => self.cx.i32_type().into(),
                FloatTy::F64 => self.cx.i64_type().into(),
            };
            let bits = self.builder.build_bit_cast(values[0].into_float_value(), bits_ty, "bits").map_err(llvm_err)?;
            return Ok(Some(bits));
        }
        let call_args: Vec<BasicMetadataValueEnum<'ctx>> = values.iter().map(|v| (*v).into()).collect();
        let math = |base: &str| c_math_name(ft, base);
        let v = match op {
            FloatOp::Abs => self.intrinsic_call("llvm.fabs", &[t], &call_args)?,
            FloatOp::Sqrt => self.intrinsic_call("llvm.sqrt", &[t], &call_args)?,
            FloatOp::Cbrt => {
                let name = math("cbrt");
                self.c_math_call(&name, &[t], t, &call_args)?
            }
            FloatOp::Powi => self.intrinsic_call("llvm.powi", &[t, self.cx.i32_type().into()], &call_args)?,
            FloatOp::Powf => self.intrinsic_call("llvm.pow", &[t], &call_args)?,
            FloatOp::Exp => self.intrinsic_call("llvm.exp", &[t], &call_args)?,
            FloatOp::Exp2 => self.intrinsic_call("llvm.exp2", &[t], &call_args)?,
            FloatOp::Ln => self.intrinsic_call("llvm.log", &[t], &call_args)?,
            FloatOp::Log2 => self.intrinsic_call("llvm.log2", &[t], &call_args)?,
            FloatOp::Log10 => self.intrinsic_call("llvm.log10", &[t], &call_args)?,
            FloatOp::Floor => self.intrinsic_call("llvm.floor", &[t], &call_args)?,
            FloatOp::Ceil => self.intrinsic_call("llvm.ceil", &[t], &call_args)?,
            FloatOp::Round => self.intrinsic_call("llvm.round", &[t], &call_args)?,
            FloatOp::RoundTiesEven => self.round_ties_even(values[0].into_float_value())?,
            FloatOp::Trunc => self.intrinsic_call("llvm.trunc", &[t], &call_args)?,
            FloatOp::Sin => self.intrinsic_call("llvm.sin", &[t], &call_args)?,
            FloatOp::Cos => self.intrinsic_call("llvm.cos", &[t], &call_args)?,
            FloatOp::Tan => self.intrinsic_call("llvm.tan", &[t], &call_args)?,
            FloatOp::Asin => self.intrinsic_call("llvm.asin", &[t], &call_args)?,
            FloatOp::Acos => self.intrinsic_call("llvm.acos", &[t], &call_args)?,
            FloatOp::Atan => self.intrinsic_call("llvm.atan", &[t], &call_args)?,
            FloatOp::Atan2 => self.intrinsic_call("llvm.atan2", &[t], &call_args)?,
            FloatOp::Sinh => self.intrinsic_call("llvm.sinh", &[t], &call_args)?,
            FloatOp::Cosh => self.intrinsic_call("llvm.cosh", &[t], &call_args)?,
            FloatOp::Tanh => self.intrinsic_call("llvm.tanh", &[t], &call_args)?,
            FloatOp::Hypot => {
                let name = math("hypot");
                self.c_math_call(&name, &[t, t], t, &call_args)?
            }
            FloatOp::MulAdd => self.intrinsic_call("llvm.fma", &[t], &call_args)?,
            FloatOp::Copysign => self.intrinsic_call("llvm.copysign", &[t], &call_args)?,
            FloatOp::ToBits => unreachable!("handled before the call"),
        };
        Ok(Some(v))
    }

    /// Calls an LLVM intrinsic, declaring it for the parameter types that choose an overloaded one.
    fn intrinsic_call(
        &mut self,
        name: &str,
        params: &[BasicTypeEnum<'ctx>],
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> GenResult<BasicValueEnum<'ctx>> {
        let intrinsic = Intrinsic::find(name).ok_or_else(|| format!("missing LLVM intrinsic `{name}`"))?;
        let decl = intrinsic
            .get_declaration(self.module, params)
            .ok_or_else(|| format!("cannot declare the LLVM intrinsic `{name}`"))?;
        self.call_value(decl, args)
    }

    /// `round_ties_even` without `llvm.roundeven`, whose lowering calls the C library's `roundeven`: the MSVC
    /// CRT has no such symbol. Round halfway cases away from zero, then step a tie toward zero when that leaves
    /// an odd integer. Every step is exact, and none depends on the current rounding mode. The `copysign` keeps
    /// a result of zero negative when the input was (`(-0.5).round_ties_even()` is `-0.0`).
    fn round_ties_even(&mut self, x: FloatValue<'ctx>) -> GenResult<BasicValueEnum<'ctx>> {
        let float_ty = x.get_type();
        let t: BasicTypeEnum<'ctx> = float_ty.into();
        let rounded = self.intrinsic_call("llvm.round", &[t], &[x.into()])?.into_float_value();
        let diff = self.builder.build_float_sub(x, rounded, "round.diff").map_err(llvm_err)?;
        let abs_diff = self.intrinsic_call("llvm.fabs", &[t], &[diff.into()])?.into_float_value();
        let half = float_ty.const_float(0.5);
        let one = float_ty.const_float(1.0);
        let is_tie =
            self.builder.build_float_compare(FloatPredicate::OEQ, abs_diff, half, "round.tie").map_err(llvm_err)?;
        let halved = self.builder.build_float_mul(rounded, half, "round.halved").map_err(llvm_err)?;
        let truncated = self.intrinsic_call("llvm.trunc", &[t], &[halved.into()])?.into_float_value();
        let is_even =
            self.builder.build_float_compare(FloatPredicate::OEQ, halved, truncated, "round.even").map_err(llvm_err)?;
        let is_odd = self.builder.build_not(is_even, "round.odd").map_err(llvm_err)?;
        let fix = self.builder.build_and(is_tie, is_odd, "round.fix").map_err(llvm_err)?;
        let signed_one = self.intrinsic_call("llvm.copysign", &[t], &[one.into(), x.into()])?.into_float_value();
        let stepped = self.builder.build_float_sub(rounded, signed_one, "round.stepped").map_err(llvm_err)?;
        let stepped = self.intrinsic_call("llvm.copysign", &[t], &[stepped.into(), x.into()])?;
        let rounded_value: BasicValueEnum<'ctx> = rounded.into();
        self.builder.build_select(fix, stepped, rounded_value, "round.even").map_err(llvm_err)
    }

    /// Calls the C math library function `name`, declaring it with the given signature.
    fn c_math_call(
        &mut self,
        name: &str,
        params: &[BasicTypeEnum<'ctx>],
        ret: BasicTypeEnum<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> GenResult<BasicValueEnum<'ctx>> {
        let param_tys: Vec<BasicMetadataTypeEnum<'ctx>> = params.iter().map(|t| (*t).into()).collect();
        let fn_ty = ret.fn_type(&param_tys, false);
        let decl = match self.module.get_function(name) {
            Some(existing) if existing.get_type() == fn_ty => existing,
            Some(_) => return Err(format!("the C math function `{name}` conflicts with another declaration")),
            None => self.module.add_function(name, fn_ty, Some(Linkage::External)),
        };
        self.call_value(decl, args)
    }

    fn call_value(
        &mut self,
        f: FunctionValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> GenResult<BasicValueEnum<'ctx>> {
        let call = self.builder.build_call(f, args, "").map_err(llvm_err)?;
        call.try_as_basic_value().basic().ok_or_else(|| format!("`{}` returns a value", f.get_name().to_string_lossy()))
    }

    /// The pointer and length of a `str` expression's bytes.
    fn str_parts(&mut self, e: &'a tir::Expr) -> GenResult<(BasicValueEnum<'ctx>, BasicValueEnum<'ctx>)> {
        let s = self.expr_mode(e, Mode::Use)?;
        let s = self.need(s, &Ty::Str).into_struct_value();
        let ptr = self.builder.build_extract_value(s, 0, "").map_err(llvm_err)?;
        let len = self.builder.build_extract_value(s, 1, "").map_err(llvm_err)?;
        Ok((ptr, len))
    }

    pub(super) fn string_of(&mut self, v: BasicValueEnum<'ctx>, ty: &Ty) -> GenResult<BasicValueEnum<'ctx>> {
        let out = self.alloca(self.string_ty.into(), "string")?;
        let i64_ty = self.cx.i64_type();
        match ty {
            Ty::Int(i) => {
                let wide = self
                    .builder
                    .build_int_cast_sign_flag(v.into_int_value(), i64_ty, i.signed(), "")
                    .map_err(llvm_err)?;
                let f = if i.signed() { self.rt.i64_to_string } else { self.rt.u64_to_string };
                self.builder.build_call(f, &[wide.into(), out.into()], "").map_err(llvm_err)?;
            }
            Ty::Char => {
                self.builder.build_call(self.rt.char_to_string, &[v.into(), out.into()], "").map_err(llvm_err)?;
            }
            Ty::Bool | Ty::Str => {
                let s = if *ty == Ty::Bool {
                    let t = self.str_literal(b"true");
                    let f = self.str_literal(b"false");
                    self.builder.build_select(v.into_int_value(), t, f, "").map_err(llvm_err)?.into_struct_value()
                } else {
                    v.into_struct_value()
                };
                let ptr = self.builder.build_extract_value(s, 0, "").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(s, 1, "").map_err(llvm_err)?;
                self.builder
                    .build_call(self.rt.str_to_string, &[ptr.into(), len.into(), out.into()], "")
                    .map_err(llvm_err)?;
            }
            _ => return Err(format!("unsupported to_string on `{ty}`")),
        }
        self.builder.build_load(self.string_ty, out, "").map_err(llvm_err)
    }
}

pub(super) fn wrapping_op(m: Method) -> BinOp {
    match m {
        Method::WrappingAdd | Method::CheckedAdd => BinOp::Add,
        Method::WrappingSub | Method::CheckedSub => BinOp::Sub,
        _ => BinOp::Mul,
    }
}

/// The C math library symbol for `base` at the given precision: the single-precision functions take an `f`
/// suffix (`sinf`, `atan2f`), as C99 defines them.
fn c_math_name(ft: FloatTy, base: &str) -> String {
    match ft {
        FloatTy::F32 => format!("{base}f"),
        FloatTy::F64 => base.to_string(),
    }
}
