//! Drop flags and the addresses of locals and their fields.

use super::place::is_direct_place;
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
    }

    /// The address of `local` plus a field path: struct and tuple fields, `Option` payloads (`[0]`), and enum
    /// variant fields (`[variant, field]`).
    pub(super) fn local_address(&mut self, id: tir::LocalId, path: &[usize]) -> GenResult<Option<PointerValue<'ctx>>> {
        let Some(mut ptr) = self.slots[id.0] else { return Ok(None) };
        let mut ty = self.locals[id.0].ty.clone();
        let mut i = 0;
        while i < path.len() {
            match &ty {
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
                Ty::Option(inner) => {
                    let ot = self.option_ty(inner);
                    ptr = self.builder.build_struct_gep(ot, ptr, 1, "").map_err(llvm_err)?;
                    ty = (**inner).clone();
                    i += 1;
                }
                _ => return Ok(None),
            }
        }
        Ok(Some(ptr))
    }

    /// The local and field path of a place expression, if it starts at a local. As in the move checker, a field
    /// selection beyond an index or a `Heap` dereference does not extend the path: neither is a place.
    pub(super) fn place_path(&self, e: &tir::Expr) -> Option<(tir::LocalId, Vec<usize>)> {
        match &e.kind {
            tir::ExprKind::Local(id) => Some((*id, Vec::new())),
            tir::ExprKind::Field(base, i) => {
                let (root, mut path) = self.place_path(base)?;
                if is_direct_place(base) {
                    path.push(*i);
                }
                Some((root, path))
            }
            tir::ExprKind::Index(base, _) | tir::ExprKind::Deref(base) | tir::ExprKind::DerefRef(base) => {
                self.place_path(base)
            }
            _ => None,
        }
    }
}
