//! Places: assignment and the address of an lvalue.

use super::{Codegen, GenResult, Mode, llvm_err};
use burdin_tir::{self as tir, IntTy, Ty};
use inkwell::values::{BasicValueEnum, PointerValue};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// Reads a place expression in `mode`; a move leaves the place uninitialized and frees a `Heap` whose pointee
    /// was moved out (MV1, MV6).
    pub(super) fn expr_place(&mut self, e: &'a tir::Expr, mode: Mode) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let Some(ptr) = self.place_ptr(e)? else { return Ok(None) };
        let Some(t) = self.llvm_ty(&e.ty) else { return Ok(None) };
        let v = self.builder.build_load(t, ptr, "").map_err(llvm_err)?;
        if mode == Mode::Move && !self.is_copy(&e.ty) {
            if let Some((root, path)) = self.place_path(e) {
                self.mark_moved(root, &path);
            }
            if matches!(e.kind, tir::ExprKind::Deref(_)) {
                // The pointee was moved out of the allocation: free it (MV6).
                self.builder.build_call(self.rt.free, &[ptr.into()], "").map_err(llvm_err)?;
            }
        }
        Ok(Some(v))
    }

    /// `place = value`: evaluates the value (by the caller), drops the old value (DR4), stores, and marks the place
    /// initialized again. A place behind an index or a `Heap` dereference is not tracked (neither is a place), so
    /// its old value is dropped through its address and no drop flag is touched.
    pub(super) fn assign(&mut self, place: &'a tir::Expr, v: Option<BasicValueEnum<'ctx>>) -> GenResult<()> {
        let Some(ptr) = self.place_ptr(place)? else { return Ok(()) };
        let Some(_t) = self.llvm_ty(&place.ty) else { return Ok(()) };
        let root = self.place_path(place);
        let crossing = !is_direct_place(place);
        if self.needs_drop(&place.ty) {
            match (&root, crossing) {
                (_, true) | (None, false) => self.drop_value(ptr, &place.ty)?,
                (Some((id, path)), false) => self.drop_at(*id, path, &place.ty)?,
            }
        }
        let v = self.need(v, &place.ty);
        self.builder.build_store(ptr, v).map_err(llvm_err)?;
        if !crossing && let Some((id, path)) = &root {
            self.set_initialized(*id, path);
        }
        Ok(())
    }

    /// The address of a place expression that starts at a local, without spilling anything.
    pub(super) fn direct_place_ptr(&mut self, e: &'a tir::Expr) -> GenResult<Option<PointerValue<'ctx>>> {
        if self.place_path(e).is_none() {
            return Ok(None);
        }
        self.place_ptr(e)
    }

    /// The address of a place expression (a local or an element of one). Other expressions are evaluated into a
    /// temporary slot. `None` for values without storage.
    pub(super) fn place_ptr(&mut self, e: &'a tir::Expr) -> GenResult<Option<PointerValue<'ctx>>> {
        match &e.kind {
            tir::ExprKind::Local(id) => Ok(self.slots[id.0]),
            tir::ExprKind::Field(base, index) => {
                let base_ptr = self.place_ptr(base)?;
                let (Some(ptr), Some(t)) = (base_ptr, self.llvm_ty(&base.ty)) else { return Ok(None) };
                let field =
                    self.builder.build_struct_gep(t.into_struct_type(), ptr, *index as u32, "").map_err(llvm_err)?;
                Ok(Some(field))
            }
            tir::ExprKind::Index(base, index) => match &base.ty {
                Ty::Array(_, len) => {
                    let len = *len;
                    let base_ptr = self.place_ptr(base)?;
                    let i = self.expr_mode(index, Mode::Use)?;
                    let i = self.need_int(i, &Ty::Int(IntTy::Usize));
                    self.bounds_check(i, self.usize_ty.const_int(len, false), e.span)?;
                    let (Some(base_ptr), Some(array_t)) = (base_ptr, self.llvm_ty(&base.ty)) else {
                        return Ok(None);
                    };
                    let zero = self.usize_ty.const_int(0, false);
                    // SAFETY (LLVM): the index was checked against the array length above.
                    let ptr = unsafe { self.builder.build_in_bounds_gep(array_t, base_ptr, &[zero, i], "") }
                        .map_err(llvm_err)?;
                    Ok(Some(ptr))
                }
                Ty::Slice(elem) => {
                    let elem = (**elem).clone();
                    let v = self.expr_mode(base, Mode::Use)?;
                    if self.llvm_ty(&base.ty).is_none() {
                        self.expr_mode(index, Mode::Use)?;
                        return Ok(None);
                    }
                    let s = self.need(v, &base.ty).into_struct_value();
                    let ptr =
                        self.builder.build_extract_value(s, 0, "slice.ptr").map_err(llvm_err)?.into_pointer_value();
                    let len = self.builder.build_extract_value(s, 1, "slice.len").map_err(llvm_err)?.into_int_value();
                    let i = self.expr_mode(index, Mode::Use)?;
                    let i = self.need_int(i, &Ty::Int(IntTy::Usize));
                    self.bounds_check(i, len, e.span)?;
                    match self.llvm_ty(&elem) {
                        Some(elem_t) => {
                            // SAFETY (LLVM): the index was checked against the length above.
                            let ptr =
                                unsafe { self.builder.build_in_bounds_gep(elem_t, ptr, &[i], "") }.map_err(llvm_err)?;
                            Ok(Some(ptr))
                        }
                        None => Ok(None),
                    }
                }
                _ => {
                    // The base diverged; this code is unreachable.
                    self.expr_mode(base, Mode::Use)?;
                    self.expr_mode(index, Mode::Use)?;
                    Ok(None)
                }
            },
            tir::ExprKind::DerefRef(base) => {
                let v = self.expr_mode(base, Mode::Use)?;
                match v {
                    Some(v) => Ok(Some(v.into_pointer_value())),
                    None => Ok(None),
                }
            }
            tir::ExprKind::Deref(base) => {
                let v = self.expr_mode(base, Mode::Use)?;
                match v {
                    Some(v) => Ok(Some(v.into_pointer_value())),
                    None => Ok(None),
                }
            }
            _ => {
                let v = self.expr_mode(e, Mode::Use)?;
                match self.llvm_ty(&e.ty) {
                    Some(t) => {
                        let slot = self.alloca(t, "tmp")?;
                        let v = self.need(v, &e.ty);
                        self.builder.build_store(slot, v).map_err(llvm_err)?;
                        // The value is a temporary of the statement that contains this expression (DR5).
                        self.add_temp_slot(slot, &e.ty);
                        Ok(Some(slot))
                    }
                    None => Ok(None),
                }
            }
        }
    }
}

/// A local, possibly followed by fields, indexing, and dereferences: its storage outlives the current statement.
pub(super) fn is_place(e: &tir::Expr) -> bool {
    match &e.kind {
        tir::ExprKind::Local(_) => true,
        tir::ExprKind::Field(base, _)
        | tir::ExprKind::Index(base, _)
        | tir::ExprKind::Deref(base)
        | tir::ExprKind::DerefRef(base) => is_place(base),
        _ => false,
    }
}

/// A place made only of a local and field selections. Indexing and `Heap` dereferences are not places, so a
/// field selection beyond one of them does not extend the drop-flag path.
pub(super) fn is_direct_place(e: &tir::Expr) -> bool {
    match &e.kind {
        tir::ExprKind::Local(_) => true,
        tir::ExprKind::Field(base, _) => is_direct_place(base),
        _ => false,
    }
}
