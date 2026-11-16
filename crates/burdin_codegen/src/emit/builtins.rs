//! Built-in functions and the methods of the primitive types.

use super::{Codegen, GenResult, Mode, llvm_err, undef};
use burdin_diag::Span;
use burdin_tir::{self as tir, BinOp, Builtin, Method, Ty};
use inkwell::IntPredicate;
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    pub(super) fn builtin(
        &mut self,
        b: Builtin,
        args: &'a [tir::Expr],
        ty: &Ty,
        span: Span,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        match b {
            Builtin::CPtrNull => {
                return Ok(Some(self.cx.ptr_type(inkwell::AddressSpace::default()).const_null().into()));
            }
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
                let call = self.builder.build_call(self.rt.alloc, &[size.into()], "heap").map_err(llvm_err)?;
                let ptr = call.try_as_basic_value().basic().expect("alloc returns a pointer").into_pointer_value();
                if self.llvm_ty(&inner).is_some() {
                    let v = self.need(value, &inner);
                    self.builder.build_store(ptr, v).map_err(llvm_err)?;
                }
                return Ok(Some(ptr.into()));
            }
            Builtin::ListNew | Builtin::MapNew => {
                return Ok(Some(self.string_ty.const_zero().into()));
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
                let call = self.builder.build_call(self.rt.run, &[ptr.into(), len.into()], "").map_err(llvm_err)?;
                let status = call.try_as_basic_value().basic().expect("run returns a status").into_int_value();
                let failed = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, status, self.cx.i32_type().const_all_ones(), "")
                    .map_err(llvm_err)?;
                let ok = self.builder.build_not(failed, "").map_err(llvm_err)?;
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("run returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let mut value =
                    self.builder.build_insert_value(rt.const_zero(), ok, 0, "").map_err(llvm_err)?.into_struct_value();
                if self.llvm_ty(ok_ty).is_some() {
                    value =
                        self.builder.build_insert_value(value, status, 1, "").map_err(llvm_err)?.into_struct_value();
                }
                return Ok(Some(value.into()));
            }
            Builtin::IntParse(int_ty) => {
                let s = self.expr_mode(&args[0], Mode::Use)?;
                let s = self.need(s, &Ty::Str).into_struct_value();
                let ptr = self.builder.build_extract_value(s, 0, "").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(s, 1, "").map_err(llvm_err)?;
                let out = self.alloca(self.cx.i64_type().into(), "parse.out")?;
                let f = if int_ty.signed() { self.rt.parse_i64 } else { self.rt.parse_u64 };
                let call = self.builder.build_call(f, &[ptr.into(), len.into(), out.into()], "").map_err(llvm_err)?;
                let ok = call.try_as_basic_value().basic().expect("parse returns a flag").into_int_value();
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("parse returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let mut good =
                    self.builder.build_insert_value(rt.const_zero(), ok, 0, "").map_err(llvm_err)?.into_struct_value();
                if self.llvm_ty(ok_ty).is_some() {
                    let raw = self.builder.build_load(self.cx.i64_type(), out, "").map_err(llvm_err)?.into_int_value();
                    let value = if let Ty::Int(i) = &**ok_ty {
                        if i.bits() == 64 {
                            raw
                        } else {
                            self.builder.build_int_truncate(raw, self.int_ty(*i), "").map_err(llvm_err)?
                        }
                    } else {
                        raw
                    };
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
                let ok = call.try_as_basic_value().basic().expect("read returns a flag").into_int_value();
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
                self.builder.build_store(slot, rt.const_zero()).map_err(llvm_err)?;
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
                let ok = call.try_as_basic_value().basic().expect("write returns a flag").into_int_value();
                let Ty::Result(ok_ty, err_ty) = ty else { unreachable!("write_file returns a Result") };
                let rt = self.result_ty(ok_ty, err_ty);
                let value =
                    self.builder.build_insert_value(rt.const_zero(), ok, 0, "").map_err(llvm_err)?.into_struct_value();
                return Ok(Some(value.into()));
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
        if matches!(m, Method::Iter | Method::IntoIter) {
            return self.iter_value(args, m == Method::IntoIter);
        }
        if m == Method::IterNext {
            return self.iter_next(args, ty);
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
        if m == Method::Chars {
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
                | Method::ListInsert
                | Method::ListRemove
                | Method::ListContains
                | Method::ListSort
                | Method::Clear
                | Method::MapInsert
                | Method::MapGet
                | Method::MapRemove
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
                Ty::List(_) | Ty::Map(..) => {
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
