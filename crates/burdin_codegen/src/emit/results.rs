//! The methods of `Option<T>` and `Result<T, E>`: `unwrap`, `expect`, and the tests.

use super::{Codegen, GenResult, Mode, llvm_err};
use burdin_diag::Span;
use burdin_tir::{self as tir, Method, Ty};
use inkwell::IntPredicate;
use inkwell::values::BasicValueEnum;

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// The methods of `Option` and `Result`.
    pub(super) fn result_method(
        &mut self,
        m: Method,
        args: &'a [tir::Expr],
        ty: &Ty,
        span: Span,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        // `unwrap` and `expect` consume the value; the tests only read its tag. A test's receiver is read through
        // a place, so a temporary (such as `read_to_string(p).is_ok()`) is registered for the statement's drops
        // and its payload is freed (`spec/allocators.md`, AL9).
        let consuming = matches!(m, Method::Unwrap | Method::Expect);
        let recv =
            if consuming { self.expr_mode(&args[0], Mode::Move)? } else { self.expr_place(&args[0], Mode::Use)? };
        let recv_ty = args[0].ty.clone();
        let value = match (&recv_ty, m) {
            (Ty::Option(_), Method::IsSome | Method::IsNone) => {
                let v = self.need(recv, &recv_ty).into_struct_value();
                let tag = self.builder.build_extract_value(v, 0, "is_some").map_err(llvm_err)?.into_int_value();
                Some(if m == Method::IsNone {
                    self.builder.build_not(tag, "").map_err(llvm_err)?.into()
                } else {
                    tag.into()
                })
            }
            (Ty::Result(..), Method::IsOk | Method::IsErr) => {
                let v = self.need(recv, &recv_ty).into_struct_value();
                let tag = self.builder.build_extract_value(v, 0, "is_ok").map_err(llvm_err)?.into_int_value();
                Some(if m == Method::IsErr {
                    self.builder.build_not(tag, "").map_err(llvm_err)?.into()
                } else {
                    tag.into()
                })
            }
            (Ty::Option(inner), Method::Unwrap | Method::Expect) => {
                let inner = (**inner).clone();
                let Some(value) = self.unwrap_payload(recv, &recv_ty, &inner, 1, m, args, span, "None")? else {
                    return Ok(None);
                };
                Some(value)
            }
            (Ty::Result(ok, _), Method::Unwrap | Method::Expect) => {
                let ok = (**ok).clone();
                let Some(value) = self.unwrap_payload(recv, &recv_ty, &ok, 1, m, args, span, "Err")? else {
                    return Ok(None);
                };
                Some(value)
            }
            _ => return Err(format!("unsupported method {m:?} on `{recv_ty}`")),
        };
        let _ = ty;
        Ok(value)
    }

    /// The payload of an `Option` or `Result`, panicking when the value is empty. Returns `None` when the payload
    /// has no runtime value.
    #[allow(clippy::too_many_arguments)]
    fn unwrap_payload(
        &mut self,
        recv: Option<BasicValueEnum<'ctx>>,
        recv_ty: &Ty,
        payload_ty: &Ty,
        field: u32,
        m: Method,
        args: &'a [tir::Expr],
        span: Span,
        empty: &str,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let Some(t) = self.llvm_ty(payload_ty) else {
            // The receiver is still evaluated for its effects; there is no payload value to return.
            return Ok(None);
        };
        let v = self.need(recv, recv_ty).into_struct_value();
        let tag = self.builder.build_extract_value(v, 0, "payload.present").map_err(llvm_err)?.into_int_value();
        let slot = self.alloca(t, "unwrap.out")?;
        let ok_bb = self.new_block("unwrap.ok");
        let fail_bb = self.new_block("unwrap.fail");
        let end = self.new_block("unwrap.end");
        self.builder.build_conditional_branch(tag, ok_bb, fail_bb).map_err(llvm_err)?;
        self.builder.position_at_end(ok_bb);
        let payload = self.builder.build_extract_value(v, field, "payload").map_err(llvm_err)?;
        self.builder.build_store(slot, payload).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(fail_bb);
        let (ptr, len) = match m {
            Method::Expect if args.len() > 1 => {
                let msg = self.expr_mode(&args[1], Mode::Use)?;
                let msg = self.need(msg, &Ty::Str).into_struct_value();
                let ptr = self.builder.build_extract_value(msg, 0, "msg.ptr").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(msg, 1, "msg.len").map_err(llvm_err)?;
                (ptr, len)
            }
            _ => {
                let msg = self.str_literal(format!("called `unwrap` on a `{empty}` value").as_bytes());
                let ptr = msg.get_field_at_index(0).expect("str has a pointer");
                let len = msg.get_field_at_index(1).expect("str has a length");
                (ptr, len)
            }
        };
        if let Ty::Result(_, err_ty) = recv_ty {
            match err_ty.as_ref() {
                Ty::Str | Ty::String => {
                    let err_val = self.builder.build_extract_value(v, 2, "err").map_err(llvm_err)?.into_struct_value();
                    let ep = self.builder.build_extract_value(err_val, 0, "err.ptr").map_err(llvm_err)?;
                    let el = self.builder.build_extract_value(err_val, 1, "err.len").map_err(llvm_err)?;
                    let (file, line, col) = self.location(span);
                    let call_args = [
                        ptr.into(),
                        len.into(),
                        ep.into(),
                        el.into(),
                        file.0.into(),
                        file.1.into(),
                        line.into(),
                        col.into(),
                    ];
                    self.builder
                        .build_call(self.rt.panic_with_str, &call_args, "")
                        .map_err(llvm_err)?
                        .set_tail_call_kind(inkwell::llvm_sys::LLVMTailCallKind::LLVMTailCallKindNoTail);
                    self.builder.build_unreachable().map_err(llvm_err)?;
                    self.builder.position_at_end(end);
                    return Ok(Some(self.builder.build_load(t, slot, "").map_err(llvm_err)?));
                }
                Ty::Int(ity) if ity.bits() <= 64 => {
                    let err_val = self.builder.build_extract_value(v, 2, "err").map_err(llvm_err)?.into_int_value();
                    let i64_ty = self.cx.i64_type();
                    let val64 = if err_val.get_type().get_bit_width() == 64 {
                        err_val
                    } else if ity.signed() {
                        self.builder.build_int_s_extend(err_val, i64_ty, "").map_err(llvm_err)?
                    } else {
                        self.builder.build_int_z_extend(err_val, i64_ty, "").map_err(llvm_err)?
                    };
                    let signed = self.cx.bool_type().const_int(u64::from(ity.signed()), false);
                    let (file, line, col) = self.location(span);
                    let call_args = [
                        ptr.into(),
                        len.into(),
                        val64.into(),
                        signed.into(),
                        file.0.into(),
                        file.1.into(),
                        line.into(),
                        col.into(),
                    ];
                    self.builder
                        .build_call(self.rt.panic_with_int, &call_args, "")
                        .map_err(llvm_err)?
                        .set_tail_call_kind(inkwell::llvm_sys::LLVMTailCallKind::LLVMTailCallKindNoTail);
                    self.builder.build_unreachable().map_err(llvm_err)?;
                    self.builder.position_at_end(end);
                    return Ok(Some(self.builder.build_load(t, slot, "").map_err(llvm_err)?));
                }
                _ => {}
            }
        }
        self.emit_panic(ptr, len, span)?;
        self.builder.position_at_end(end);
        Ok(Some(self.builder.build_load(t, slot, "").map_err(llvm_err)?))
    }
}

/// `Option` and `Result` tags are `i1`s; keep the predicate import used even if the tag is compared directly.
#[allow(dead_code)]
const _: IntPredicate = IntPredicate::EQ;
