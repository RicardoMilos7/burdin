//! Panics and the run-time checks that raise them.

use super::{Codegen, GenResult, Mode, llvm_err};
use burdin_diag::Span;
use burdin_tir::{self as tir, Ty};
use inkwell::IntPredicate;
use inkwell::values::{BasicValueEnum, IntValue, LLVMTailCallKind};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// The file path (as a `str` pointer and length), line, and column of `span`, for the panic functions.
    pub(super) fn location(
        &mut self,
        span: Span,
    ) -> ((BasicValueEnum<'ctx>, BasicValueEnum<'ctx>), IntValue<'ctx>, IntValue<'ctx>) {
        let file = self.sources.get(span.file);
        let (line, col) = file.line_col(span.lo);
        let path = self.str_literal(file.path.clone().as_bytes());
        let ptr = path.get_field_at_index(0).expect("str has a pointer");
        let len = path.get_field_at_index(1).expect("str has a length");
        let i32_ty = self.cx.i32_type();
        ((ptr, len), i32_ty.const_int(u64::from(line), false), i32_ty.const_int(u64::from(col), false))
    }

    /// Calls the runtime's panic function and terminates the current block.
    pub(super) fn emit_panic(
        &mut self,
        msg_ptr: BasicValueEnum<'ctx>,
        msg_len: BasicValueEnum<'ctx>,
        span: Span,
    ) -> GenResult<()> {
        let (file, line, col) = self.location(span);
        let snip_text = if self.debug_assertions { self.panic_snippet(span) } else { String::new() };
        let snip = self.str_literal(snip_text.as_bytes());
        let snip_ptr = snip.get_field_at_index(0).expect("str has a pointer");
        let snip_len = snip.get_field_at_index(1).expect("str has a length");
        let args = [
            msg_ptr.into(),
            msg_len.into(),
            snip_ptr.into(),
            snip_len.into(),
            file.0.into(),
            file.1.into(),
            line.into(),
            col.into(),
        ];
        // `notail`: a tail call would erase the panicking function's frame from the backtrace, and the panic
        // function never returns, so LLVM would otherwise make it a jump.
        self.builder
            .build_call(self.rt.panic_ctx, &args, "")
            .map_err(llvm_err)?
            .set_tail_call_kind(LLVMTailCallKind::LLVMTailCallKindNoTail);
        self.builder.build_unreachable().map_err(llvm_err)?;
        Ok(())
    }

    fn panic_snippet(&self, span: Span) -> String {
        if span.lo >= span.hi {
            return String::new();
        }
        let sf = self.sources.get(span.file);
        let (line_no, col_no) = sf.line_col(span.lo);
        let Some(src_line) = sf.text.lines().nth(line_no.saturating_sub(1) as usize) else {
            return String::new();
        };
        if src_line.trim().is_empty() || src_line.len() > 200 {
            return String::new();
        }
        let num = line_no.to_string();
        let gutter = " ".repeat(num.len());
        let caret_pad = " ".repeat(col_no.saturating_sub(1) as usize);
        let width = ((span.hi - span.lo) as usize)
            .clamp(1, src_line.len().saturating_sub(col_no.saturating_sub(1) as usize).max(1));
        let carets = "^".repeat(width.min(40));
        format!("{gutter} |\n{num} | {src_line}\n{gutter} | {caret_pad}{carets}\n")
    }

    /// Guards an integer operation and panics with `verb` and operand values on overflow.
    pub(super) fn check_overflow(
        &mut self,
        ok: IntValue<'ctx>,
        verb: &str,
        lhs: IntValue<'ctx>,
        rhs: Option<IntValue<'ctx>>,
        signed: bool,
        span: Span,
    ) -> GenResult<()> {
        if lhs.get_type().get_bit_width() > 64 {
            return self.check(ok, &format!("attempt to {verb} with overflow"), span);
        }
        let fail = self.new_block("ovf.fail");
        let cont = self.new_block("ovf.ok");
        self.builder.build_conditional_branch(ok, cont, fail).map_err(llvm_err)?;
        self.builder.position_at_end(fail);
        let i64_ty = self.cx.i64_type();
        let i1_ty = self.cx.bool_type();
        let to_i64 = |v: IntValue<'ctx>| -> GenResult<IntValue<'ctx>> {
            if v.get_type().get_bit_width() == 64 {
                Ok(v)
            } else if signed {
                self.builder.build_int_s_extend(v, i64_ty, "").map_err(llvm_err)
            } else {
                self.builder.build_int_z_extend(v, i64_ty, "").map_err(llvm_err)
            }
        };
        let lhs64 = to_i64(lhs)?;
        let (rhs64, binary) = match rhs {
            Some(r) => (to_i64(r)?, i1_ty.const_int(1, false)),
            None => (i64_ty.const_zero(), i1_ty.const_zero()),
        };
        let v = self.str_literal(verb.as_bytes());
        let vp = v.get_field_at_index(0).expect("str ptr");
        let vl = v.get_field_at_index(1).expect("str len");
        let (file, line, col) = self.location(span);
        let args = [
            vp.into(),
            vl.into(),
            lhs64.into(),
            rhs64.into(),
            i1_ty.const_int(u64::from(signed), false).into(),
            binary.into(),
            file.0.into(),
            file.1.into(),
            line.into(),
            col.into(),
        ];
        self.builder
            .build_call(self.rt.panic_overflow, &args, "")
            .map_err(llvm_err)?
            .set_tail_call_kind(LLVMTailCallKind::LLVMTailCallKindNoTail);
        self.builder.build_unreachable().map_err(llvm_err)?;
        self.builder.position_at_end(cont);
        Ok(())
    }

    /// Panics with `msg` unless `ok` holds.
    pub(super) fn check(&mut self, ok: IntValue<'ctx>, msg: &str, span: Span) -> GenResult<()> {
        let pass = self.new_block("check.ok");
        let fail = self.new_block("check.fail");
        self.builder.build_conditional_branch(ok, pass, fail).map_err(llvm_err)?;
        self.builder.position_at_end(fail);
        let m = self.str_literal(msg.as_bytes());
        let ptr = m.get_field_at_index(0).expect("str has a pointer");
        let len = m.get_field_at_index(1).expect("str has a length");
        self.emit_panic(ptr, len, span)?;
        self.builder.position_at_end(pass);
        Ok(())
    }

    /// Panics unless `ok` holds. With `msg`, the panic reports the formatted message, which is evaluated in the
    /// failure block only (`assert(cond, "message {x}")`); without it, the panic reports `fallback`.
    pub(super) fn check_message(
        &mut self,
        ok: IntValue<'ctx>,
        msg: Option<&'a tir::Expr>,
        fallback: &str,
        span: Span,
    ) -> GenResult<()> {
        let pass = self.new_block("check.ok");
        let fail = self.new_block("check.fail");
        self.builder.build_conditional_branch(ok, pass, fail).map_err(llvm_err)?;
        self.builder.position_at_end(fail);
        let (ptr, len) = match msg {
            Some(m) => {
                let s = self.expr_mode(m, Mode::Use)?;
                let s = self.need(s, &Ty::Str).into_struct_value();
                let ptr = self.builder.build_extract_value(s, 0, "msg.ptr").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(s, 1, "msg.len").map_err(llvm_err)?;
                (ptr, len)
            }
            None => {
                let m = self.str_literal(fallback.as_bytes());
                (
                    m.get_field_at_index(0).expect("str has a pointer"),
                    m.get_field_at_index(1).expect("str has a length"),
                )
            }
        };
        self.emit_panic(ptr, len, span)?;
        self.builder.position_at_end(pass);
        Ok(())
    }

    /// Panics unless `index < len`.
    pub(super) fn bounds_check(&mut self, index: IntValue<'ctx>, len: IntValue<'ctx>, span: Span) -> GenResult<()> {
        let ok = self.builder.build_int_compare(IntPredicate::ULT, index, len, "").map_err(llvm_err)?;
        self.bounds_guard(ok, index, len, span)
    }

    /// Panics unless `index <= len` (the end of a slice may be exactly the length).
    pub(super) fn bounds_guard_ule(&mut self, index: IntValue<'ctx>, len: IntValue<'ctx>, span: Span) -> GenResult<()> {
        let ok = self.builder.build_int_compare(IntPredicate::ULE, index, len, "").map_err(llvm_err)?;
        self.bounds_guard(ok, index, len, span)
    }

    /// Continues when `ok` holds; otherwise panics with `index` and `len`.
    pub(super) fn bounds_guard(
        &mut self,
        ok: IntValue<'ctx>,
        index: IntValue<'ctx>,
        len: IntValue<'ctx>,
        span: Span,
    ) -> GenResult<()> {
        let pass = self.new_block("bounds.ok");
        let fail = self.new_block("bounds.fail");
        self.builder.build_conditional_branch(ok, pass, fail).map_err(llvm_err)?;
        self.builder.position_at_end(fail);
        let (file, line, col) = self.location(span);
        let i64_ty = self.cx.i64_type();
        let index = self.builder.build_int_z_extend_or_bit_cast(index, i64_ty, "").map_err(llvm_err)?;
        let len = self.builder.build_int_z_extend_or_bit_cast(len, i64_ty, "").map_err(llvm_err)?;
        let args = [index.into(), len.into(), file.0.into(), file.1.into(), line.into(), col.into()];
        self.builder
            .build_call(self.rt.panic_bounds, &args, "")
            .map_err(llvm_err)?
            .set_tail_call_kind(LLVMTailCallKind::LLVMTailCallKindNoTail);
        self.builder.build_unreachable().map_err(llvm_err)?;
        self.builder.position_at_end(pass);
        Ok(())
    }
}
