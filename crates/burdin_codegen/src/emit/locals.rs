//! Drop flags and the addresses of locals and their fields.

use super::place::{constant_index, is_tracked_place};
use super::{Codegen, GenResult, llvm_err};
use burdin_tir::{self as tir, Ty};
use inkwell::values::PointerValue;

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    pub(super) fn flag_for(&self, id: tir::LocalId, path: &[usize]) -> Option<PointerValue<'ctx>> {
        self.flags.iter().find(|(p, _)| p.local == id && p.path.as_slice() == path).map(|(_, f)| *f)
    }

    pub(super) fn has_inner_flag(&self, id: tir::LocalId, path: &[usize]) -> bool {
        self.flags.iter().any(|(p, _)| p.local == id && p.path.len() > path.len() && p.path.starts_with(path))
    }

    /// Marks the place at `path` as moved: every flag at or inside it is cleared.
    pub(super) fn mark_moved(&mut self, id: tir::LocalId, path: &[usize]) {
        let zero = self.cx.bool_type().const_int(0, false);
        let flags: Vec<PointerValue<'ctx>> =
            self.flags.iter().filter(|(p, _)| p.local == id && p.path.starts_with(path)).map(|(_, f)| *f).collect();
        for f in flags {
            self.builder.build_store(f, zero).map_err(llvm_err).expect("storing a flag cannot fail");
        }
    }

    /// Marks the place at `path` (and everything inside it) as initialized again.
    pub(super) fn set_initialized(&mut self, id: tir::LocalId, path: &[usize]) {
        let one = self.cx.bool_type().const_int(1, false);
        let flags: Vec<PointerValue<'ctx>> =
            self.flags.iter().filter(|(p, _)| p.local == id && p.path.starts_with(path)).map(|(_, f)| *f).collect();
        for f in flags {
            self.builder.build_store(f, one).map_err(llvm_err).expect("storing a flag cannot fail");
        }
        // The whole local just became readable again, so its debug location starts here too (DBG4).
        if path.is_empty() {
            self.debug_reopen(id);
        }
    }

    /// The address of `local` plus a field path: struct and tuple fields, `Option` payloads (`[0]`), and enum
    /// variant fields (`[variant, field]`).
    pub(super) fn local_address(&mut self, id: tir::LocalId, path: &[usize]) -> GenResult<Option<PointerValue<'ctx>>> {
        let Some(mut ptr) = self.slots[id.0] else { return Ok(None) };
        let mut ty = self.locals[id.0].ty.clone();
        let mut i = 0;
        while i < path.len() {
            match &ty {
                _ if self.list_info(&ty).is_some() => {
                    let (elem, alloc) = self.list_info(&ty).expect("checked by the guard");
                    let (elem, alloc) = (&elem, &alloc);
                    let ptr_ty = self.cx.ptr_type(inkwell::AddressSpace::default());
                    let seq_ty = self.seq_ty(alloc);
                    let data_field = self.builder.build_struct_gep(seq_ty, ptr, 0, "list.ptr").map_err(llvm_err)?;
                    let data = self.builder.build_load(ptr_ty, data_field, "").map_err(llvm_err)?.into_pointer_value();
                    let Some(elem_t) = self.llvm_ty(elem) else { return Ok(None) };
                    let index = self.usize_ty.const_int(path[i] as u64, false);
                    // SAFETY (LLVM): the move checker only records constant, non-negative indices.
                    ptr = unsafe { self.builder.build_in_bounds_gep(elem_t, data, &[index], "") }.map_err(llvm_err)?;
                    ty = (**elem).clone();
                    i += 1;
                }
                Ty::Struct(_) | Ty::Tuple(_) => {
                    let t = self.llvm_ty(&ty).expect("an aggregate has a type").into_struct_type();
                    let index = path[i];
                    ptr = self.builder.build_struct_gep(t, ptr, index as u32, "").map_err(llvm_err)?;
                    ty = self.field_tys(&ty)[index].clone();
                    i += 1;
                }
                Ty::Enum(a) => {
                    let vi = path[i];
                    let fi = path[i + 1];
                    let et = self.enum_ty(a.id);
                    let payload = self.builder.build_struct_gep(et, ptr, 1, "").map_err(llvm_err)?;
                    let vt = self.variant_ty(a.id, vi);
                    ptr = self.builder.build_struct_gep(vt, payload, fi as u32, "").map_err(llvm_err)?;
                    ty = self.variant_tys(a.id, vi)[fi].clone();
                    i += 2;
                }
                Ty::Array(elem, _) => {
                    let array_t = self.llvm_ty(&ty).expect("an array has a type");
                    let index = path[i];
                    let zero = self.usize_ty.const_int(0, false);
                    let index = self.usize_ty.const_int(index as u64, false);
                    // SAFETY (LLVM): the move checker only records constant, non-negative indices.
                    ptr = unsafe { self.builder.build_in_bounds_gep(array_t, ptr, &[zero, index], "") }
                        .map_err(llvm_err)?;
                    ty = (**elem).clone();
                    i += 1;
                }
                Ty::Heap(inner, alloc) if path[i] == tir::MOVED_DEREF => {
                    let ptr_ty = self.cx.ptr_type(inkwell::AddressSpace::default());
                    let slot = match alloc.allocator_ty() {
                        None => ptr,
                        Some(_) => {
                            let heap_ty = self.heap_ty(alloc).into_struct_type();
                            self.builder.build_struct_gep(heap_ty, ptr, 0, "heap.ptr").map_err(llvm_err)?
                        }
                    };
                    ptr = self.builder.build_load(ptr_ty, slot, "heap").map_err(llvm_err)?.into_pointer_value();
                    ty = (**inner).clone();
                    i += 1;
                }
                _ => return Ok(None),
            }
        }
        Ok(Some(ptr))
    }

    /// The local and field path of a place expression, if it starts at a local. As in the move checker, the path
    /// follows fields, constant array and `List` indices, and `Heap` pointee fields; a runtime index or a slice
    /// element cuts it.
    pub(super) fn place_path(&self, e: &tir::Expr) -> Option<(tir::LocalId, Vec<usize>)> {
        use tir::ExprKind as K;
        match &e.kind {
            K::Local(id) => Some((*id, Vec::new())),
            K::Field(base, i) => {
                let (root, mut path) = self.place_path(base)?;
                if is_tracked_place(&self.defs(), base) {
                    path.push(*i);
                }
                Some((root, path))
            }
            K::Index(base, index) => {
                let (root, mut path) = self.place_path(base)?;
                if is_tracked_place(&self.defs(), base)
                    && (matches!(base.ty, Ty::Array(..)) || self.list_info(&base.ty).is_some())
                    && let Some(index) = constant_index(index)
                {
                    path.push(index);
                }
                Some((root, path))
            }
            K::Deref(base) => {
                let (root, mut path) = self.place_path(base)?;
                if is_tracked_place(&self.defs(), base) && matches!(base.ty, Ty::Heap(_, _)) {
                    path.push(tir::MOVED_DEREF);
                }
                Some((root, path))
            }
            K::DerefRef(base) => self.place_path(base),
            _ => None,
        }
    }
}
