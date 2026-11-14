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
        span: Span,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        match b {
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
        if m == Method::Clone {
            let recv_ty = args[0].ty.clone();
            let src = match self.direct_place_ptr(&args[0])? {
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
