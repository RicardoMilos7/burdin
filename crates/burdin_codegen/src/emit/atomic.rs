//! Atomic operations: the `__atomic_*` primitives of `std.atomic` and the reference count of `Shared<T>`
//! (`spec/shared.md`, SH1 and SH7).
//!
//! The primitives are sequentially consistent, which is the simplest order to reason about and the one the standard
//! library documents. A `Shared<T>` count uses the weakest orders that are still correct: a relaxed increment, a
//! release decrement, and an acquire fence in the thread that frees the cell.

use super::{Codegen, GenResult, Mode, llvm_err};
use burdin_tir::{self as tir, AtomicOp, AtomicWidth};
use inkwell::values::{BasicValue, BasicValueEnum, IntValue, PointerValue};
use inkwell::{AtomicOrdering, AtomicRMWBinOp};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// Calls the atomic primitive `op` on a word of `width`.
    pub(super) fn atomic_primitive(
        &mut self,
        op: AtomicOp,
        width: AtomicWidth,
        args: &'a [tir::Expr],
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let seq = AtomicOrdering::SequentiallyConsistent;
        if op == AtomicOp::Fence {
            self.builder.build_fence(seq, false, "").map_err(llvm_err)?;
            return Ok(None);
        }
        let word = match width {
            AtomicWidth::W32 => self.cx.i32_type(),
            AtomicWidth::W64 => self.cx.i64_type(),
        };
        let mut values = Vec::new();
        for arg in args {
            let value = self.expr_mode(arg, Mode::Use)?;
            values.push(self.need(value, &arg.ty));
        }
        let ptr = values[0].into_pointer_value();
        let bytes = width.bits() / 8;
        match op {
            AtomicOp::Load => {
                let load = self.builder.build_load(word, ptr, "atomic.load").map_err(llvm_err)?;
                let inst = load.as_instruction_value().expect("a load is an instruction");
                inst.set_atomic_ordering(seq).map_err(|e| e.to_string())?;
                inst.set_alignment(bytes).map_err(|e| e.to_string())?;
                Ok(Some(load))
            }
            AtomicOp::Store => {
                let store = self.builder.build_store(ptr, values[1]).map_err(llvm_err)?;
                store.set_atomic_ordering(seq).map_err(|e| e.to_string())?;
                store.set_alignment(bytes).map_err(|e| e.to_string())?;
                Ok(None)
            }
            AtomicOp::Cas => {
                let pair = self
                    .builder
                    .build_cmpxchg(ptr, values[1].into_int_value(), values[2].into_int_value(), seq, seq)
                    .map_err(llvm_err)?;
                let old = self.builder.build_extract_value(pair, 0, "atomic.old").map_err(llvm_err)?;
                Ok(Some(old))
            }
            AtomicOp::Fence => unreachable!("handled above"),
            _ => {
                let rmw = match op {
                    AtomicOp::Swap => AtomicRMWBinOp::Xchg,
                    AtomicOp::Add => AtomicRMWBinOp::Add,
                    AtomicOp::Sub => AtomicRMWBinOp::Sub,
                    AtomicOp::And => AtomicRMWBinOp::And,
                    AtomicOp::Or => AtomicRMWBinOp::Or,
                    AtomicOp::Xor => AtomicRMWBinOp::Xor,
                    _ => unreachable!("the other operations are handled above"),
                };
                let old = self.builder.build_atomicrmw(rmw, ptr, values[1].into_int_value(), seq).map_err(llvm_err)?;
                Ok(Some(old.into()))
            }
        }
    }

    /// Adds one to the count of a `Shared<T>` cell at `count_at`, relaxed: a new handle is made from an existing one,
    /// so nothing has to be ordered by the increment.
    pub(super) fn shared_count_inc(&mut self, count_at: PointerValue<'ctx>) -> GenResult<()> {
        let one = self.usize_ty.const_int(1, false);
        self.builder
            .build_atomicrmw(AtomicRMWBinOp::Add, count_at, one, AtomicOrdering::Monotonic)
            .map_err(llvm_err)?;
        Ok(())
    }

    /// Takes one from the count at `count_at` with release order and returns whether this handle was the last. The
    /// caller then frees the cell after [`Self::shared_acquire`].
    pub(super) fn shared_count_dec(&mut self, count_at: PointerValue<'ctx>) -> GenResult<IntValue<'ctx>> {
        let one = self.usize_ty.const_int(1, false);
        let old = self
            .builder
            .build_atomicrmw(AtomicRMWBinOp::Sub, count_at, one, AtomicOrdering::Release)
            .map_err(llvm_err)?;
        self.builder.build_int_compare(inkwell::IntPredicate::EQ, old, one, "shared.last").map_err(llvm_err)
    }

    /// The fence that makes everything the other handles did to the value visible before the last handle drops it.
    pub(super) fn shared_acquire(&mut self) -> GenResult<()> {
        self.builder.build_fence(AtomicOrdering::Acquire, false, "").map_err(llvm_err)?;
        Ok(())
    }

    /// The count at `count_at`, read atomically.
    pub(super) fn shared_count_load(&mut self, count_at: PointerValue<'ctx>) -> GenResult<BasicValueEnum<'ctx>> {
        let load = self.builder.build_load(self.usize_ty, count_at, "shared.count").map_err(llvm_err)?;
        let inst = load.as_instruction_value().expect("a load is an instruction");
        inst.set_atomic_ordering(AtomicOrdering::Acquire).map_err(|e| e.to_string())?;
        inst.set_alignment(8).map_err(|e| e.to_string())?;
        Ok(load)
    }
}
