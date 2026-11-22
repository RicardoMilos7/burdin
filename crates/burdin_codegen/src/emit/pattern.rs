//! Patterns: testing them against a value and binding what they name.

use super::{Codegen, GenResult, llvm_err};
use burdin_tir::{self as tir, Pat, Ty};
use inkwell::IntPredicate;
use inkwell::values::{IntValue, PointerValue};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// Collects the paths, relative to the pattern's root, of bindings that take their value by moving it.
    pub(super) fn pattern_moved_paths(&self, pat: &Pat, ty: &Ty, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        match pat {
            Pat::Binding(_, tir::BindMode::Move) | Pat::At(_, tir::BindMode::Move, _) => out.push(path.clone()),
            Pat::At(_, _, inner) => self.pattern_moved_paths(inner, ty, path, out),
            Pat::Or(alts) => {
                if let Some(first) = alts.first() {
                    self.pattern_moved_paths(first, ty, path, out);
                }
            }
            Pat::Some(inner) => {
                if let Ty::Option(inner_ty) = ty {
                    let inner_ty = (**inner_ty).clone();
                    path.push(0);
                    self.pattern_moved_paths(inner, &inner_ty, path, out);
                    path.pop();
                }
            }
            Pat::Ok(inner) => {
                if let Ty::Result(ok, _) = ty {
                    let ok = (**ok).clone();
                    path.push(0);
                    self.pattern_moved_paths(inner, &ok, path, out);
                    path.pop();
                }
            }
            Pat::Err(inner) => {
                if let Ty::Result(_, err) = ty {
                    let err = (**err).clone();
                    path.push(1);
                    self.pattern_moved_paths(inner, &err, path, out);
                    path.pop();
                }
            }
            Pat::Product(subs) => {
                let tys = self.field_tys(ty);
                for (i, sub) in subs.iter().enumerate() {
                    let Some(sub_ty) = tys.get(i).cloned() else { continue };
                    path.push(i);
                    self.pattern_moved_paths(sub, &sub_ty, path, out);
                    path.pop();
                }
            }
            Pat::Variant(index, subs) => {
                if let Ty::Enum(a) = ty {
                    let tys = self.variant_tys(a.id, *index);
                    for (i, sub) in subs.iter().enumerate() {
                        let Some(sub_ty) = tys.get(i).cloned() else { continue };
                        path.push(*index);
                        path.push(i);
                        self.pattern_moved_paths(sub, &sub_ty, path, out);
                        path.pop();
                        path.pop();
                    }
                }
            }
            _ => {}
        }
    }

    /// Whether the value at `place` (of type `ty`) matches `pat`. Reads memory only; never has side effects.
    pub(super) fn pat_test(
        &mut self,
        pat: &Pat,
        place: Option<PointerValue<'ctx>>,
        ty: &Ty,
    ) -> GenResult<IntValue<'ctx>> {
        let bool_ty = self.cx.bool_type();
        let yes = bool_ty.const_int(1, false);
        let load_int = |cg: &mut Self| -> GenResult<IntValue<'ctx>> {
            let t = cg.llvm_ty(ty).expect("a literal pattern has a scalar type");
            let ptr = place.expect("a scalar has storage");
            Ok(cg.builder.build_load(t, ptr, "").map_err(llvm_err)?.into_int_value())
        };
        match pat {
            Pat::Wild | Pat::Binding(..) => Ok(yes),
            Pat::At(_, _, inner) => self.pat_test(inner, place, ty),
            Pat::Bool(b) => {
                let v = load_int(self)?;
                let c = bool_ty.const_int(u64::from(*b), false);
                self.builder.build_int_compare(IntPredicate::EQ, v, c, "").map_err(llvm_err)
            }
            Pat::Int(n) => {
                let v = load_int(self)?;
                let c = super::types::int_const(v.get_type(), *n);
                self.builder.build_int_compare(IntPredicate::EQ, v, c, "").map_err(llvm_err)
            }
            Pat::Char(ch) => {
                let v = load_int(self)?;
                let c = v.get_type().const_int(u64::from(*ch as u32), false);
                self.builder.build_int_compare(IntPredicate::EQ, v, c, "").map_err(llvm_err)
            }
            Pat::Str(s) => {
                let ptr = place.expect("a string has storage");
                let v = self.builder.build_load(self.str_ty, ptr, "").map_err(llvm_err)?.into_struct_value();
                let p = self.builder.build_extract_value(v, 0, "").map_err(llvm_err)?;
                let n = self.builder.build_extract_value(v, 1, "").map_err(llvm_err)?;
                let literal = self.str_literal(s.as_bytes());
                let lp = literal.get_field_at_index(0).expect("str has a pointer");
                let ln = literal.get_field_at_index(1).expect("str has a length");
                let call = self
                    .builder
                    .build_call(self.rt.str_eq, &[p.into(), n.into(), lp.into(), ln.into()], "")
                    .map_err(llvm_err)?;
                Ok(call.try_as_basic_value().basic().expect("`str_eq` returns a flag").into_int_value())
            }
            Pat::Range(lo, hi) => {
                let v = load_int(self)?;
                let signed = matches!(ty, Ty::Int(i) if i.signed());
                let (ge, le) = if signed {
                    (IntPredicate::SGE, IntPredicate::SLE)
                } else {
                    (IntPredicate::UGE, IntPredicate::ULE)
                };
                let lo_c = super::types::int_const(v.get_type(), *lo);
                let hi_c = super::types::int_const(v.get_type(), *hi);
                let a = self.builder.build_int_compare(ge, v, lo_c, "").map_err(llvm_err)?;
                let b = self.builder.build_int_compare(le, v, hi_c, "").map_err(llvm_err)?;
                self.builder.build_and(a, b, "").map_err(llvm_err)
            }
            Pat::Or(alts) => {
                let mut acc = bool_ty.const_int(0, false);
                for alt in alts {
                    let t = self.pat_test(alt, place, ty)?;
                    acc = self.builder.build_or(acc, t, "").map_err(llvm_err)?;
                }
                Ok(acc)
            }
            Pat::Product(subs) => {
                let (Some(ptr), Some(t)) = (place, self.llvm_ty(ty)) else { return Ok(yes) };
                let tys = self.field_tys(ty);
                let mut acc = yes;
                for (i, sub) in subs.iter().enumerate() {
                    if matches!(sub, Pat::Wild | Pat::Binding(..)) {
                        continue;
                    }
                    let p = self.builder.build_struct_gep(t.into_struct_type(), ptr, i as u32, "").map_err(llvm_err)?;
                    let ok = self.pat_test(sub, Some(p), &tys[i])?;
                    acc = self.builder.build_and(acc, ok, "").map_err(llvm_err)?;
                }
                Ok(acc)
            }
            Pat::Variant(index, subs) => {
                let Ty::Enum(a) = ty else { unreachable!("checked: a variant pattern on an enum") };
                let et = self.enum_ty(a.id);
                let ptr = place.expect("an enum has storage");
                let tag_ptr = self.builder.build_struct_gep(et, ptr, 0, "").map_err(llvm_err)?;
                let i32_ty = self.cx.i32_type();
                let tag = self.builder.build_load(i32_ty, tag_ptr, "tag").map_err(llvm_err)?.into_int_value();
                let want = i32_ty.const_int(*index as u64, false);
                let tag_ok = self.builder.build_int_compare(IntPredicate::EQ, tag, want, "").map_err(llvm_err)?;
                if subs.iter().all(|s| matches!(s, Pat::Wild | Pat::Binding(..))) {
                    return Ok(tag_ok);
                }
                let vt = self.variant_ty(a.id, *index);
                let tys = self.variant_tys(a.id, *index);
                let payload = self.builder.build_struct_gep(et, ptr, 1, "").map_err(llvm_err)?;
                let mut acc = yes;
                for (i, sub) in subs.iter().enumerate() {
                    if matches!(sub, Pat::Wild | Pat::Binding(..)) {
                        continue;
                    }
                    let p = self.builder.build_struct_gep(vt, payload, i as u32, "").map_err(llvm_err)?;
                    let ok = self.pat_test(sub, Some(p), &tys[i])?;
                    acc = self.builder.build_and(acc, ok, "").map_err(llvm_err)?;
                }
                // `select`, not `and`: the payload belongs to another variant when the tag differs.
                let no = bool_ty.const_int(0, false);
                Ok(self.builder.build_select(tag_ok, acc, no, "").map_err(llvm_err)?.into_int_value())
            }
            Pat::None | Pat::Some(_) => {
                let Ty::Option(inner) = ty else { unreachable!("checked: an Option pattern on an Option") };
                let opt_ty = self.option_ty(inner);
                let ptr = place.expect("an Option has storage");
                let tag_ptr = self.builder.build_struct_gep(opt_ty, ptr, 0, "").map_err(llvm_err)?;
                let tag = self.builder.build_load(bool_ty, tag_ptr, "is_some").map_err(llvm_err)?.into_int_value();
                match pat {
                    Pat::None => self.builder.build_not(tag, "").map_err(llvm_err),
                    Pat::Some(sub) => {
                        let payload = if self.llvm_ty(inner).is_some() {
                            Some(self.builder.build_struct_gep(opt_ty, ptr, 1, "").map_err(llvm_err)?)
                        } else {
                            None
                        };
                        let sub_ok = self.pat_test(sub, payload, inner)?;
                        // `select`, not `and`: the payload of `None` is uninitialized.
                        let no = bool_ty.const_int(0, false);
                        Ok(self.builder.build_select(tag, sub_ok, no, "").map_err(llvm_err)?.into_int_value())
                    }
                    _ => unreachable!(),
                }
            }
            Pat::Ok(sub) | Pat::Err(sub) => {
                let Ty::Result(ok, err) = ty else { unreachable!("checked: a Result pattern on a Result") };
                let payload_ty = if matches!(pat, Pat::Ok(_)) { ok } else { err };
                let want_ok = matches!(pat, Pat::Ok(_));
                let rt = self.result_ty(ok, err);
                let ptr = place.expect("a Result has storage");
                let tag_ptr = self.builder.build_struct_gep(rt, ptr, 0, "").map_err(llvm_err)?;
                let tag = self.builder.build_load(bool_ty, tag_ptr, "is_ok").map_err(llvm_err)?.into_int_value();
                let tag_ok = if want_ok { tag } else { self.builder.build_not(tag, "").map_err(llvm_err)? };
                let payload = if self.llvm_ty(payload_ty).is_some() {
                    let index = if want_ok { 1 } else { 2 };
                    Some(self.builder.build_struct_gep(rt, ptr, index, "").map_err(llvm_err)?)
                } else {
                    None
                };
                let sub_ok = self.pat_test(sub, payload, payload_ty)?;
                // `select`, not `and`: the other payload is uninitialized.
                let no = bool_ty.const_int(0, false);
                Ok(self.builder.build_select(tag_ok, sub_ok, no, "").map_err(llvm_err)?.into_int_value())
            }
        }
    }

    /// Stores the parts of the value at `place` into the bindings of `pat` (which matched). A `Move` binding takes
    /// its part out of the scrutinee, clearing the drop flags of the paths it moves (MV8).
    pub(super) fn pat_bind(
        &mut self,
        pat: &Pat,
        place: Option<PointerValue<'ctx>>,
        ty: &Ty,
        base: Option<&(tir::LocalId, Vec<usize>)>,
        path: &mut Vec<usize>,
    ) -> GenResult<()> {
        match pat {
            Pat::Binding(id, mode) => self.bind(*id, *mode, place, ty, base, path),
            Pat::At(id, mode, inner) => {
                self.bind(*id, *mode, place, ty, base, path)?;
                self.pat_bind(inner, place, ty, base, path)
            }
            Pat::Some(sub) => {
                let Ty::Option(inner) = ty else { unreachable!() };
                let payload = match (place, self.llvm_ty(inner)) {
                    (Some(ptr), Some(_)) => {
                        Some(self.builder.build_struct_gep(self.option_ty(inner), ptr, 1, "").map_err(llvm_err)?)
                    }
                    _ => None,
                };
                path.push(0);
                self.pat_bind(sub, payload, inner, base, path)?;
                path.pop();
                Ok(())
            }
            Pat::Ok(sub) | Pat::Err(sub) => {
                let Ty::Result(ok, err) = ty else { unreachable!() };
                let ok_side = matches!(pat, Pat::Ok(_));
                let payload_ty = if ok_side { ok } else { err };
                let index = if ok_side { 1 } else { 2 };
                let payload = match (place, self.llvm_ty(payload_ty)) {
                    (Some(ptr), Some(_)) => {
                        Some(self.builder.build_struct_gep(self.result_ty(ok, err), ptr, index, "").map_err(llvm_err)?)
                    }
                    _ => None,
                };
                path.push(if ok_side { 0 } else { 1 });
                self.pat_bind(sub, payload, payload_ty, base, path)?;
                path.pop();
                Ok(())
            }
            Pat::Product(subs) => {
                let (Some(ptr), Some(t)) = (place, self.llvm_ty(ty)) else { return Ok(()) };
                let tys = self.field_tys(ty);
                for (i, sub) in subs.iter().enumerate() {
                    let p = self.builder.build_struct_gep(t.into_struct_type(), ptr, i as u32, "").map_err(llvm_err)?;
                    path.push(i);
                    self.pat_bind(sub, Some(p), &tys[i], base, path)?;
                    path.pop();
                }
                Ok(())
            }
            Pat::Variant(index, subs) => {
                let Ty::Enum(a) = ty else { unreachable!() };
                let Some(ptr) = place else { return Ok(()) };
                if subs.is_empty() {
                    return Ok(());
                }
                let payload = self.builder.build_struct_gep(self.enum_ty(a.id), ptr, 1, "").map_err(llvm_err)?;
                let vt = self.variant_ty(a.id, *index);
                let tys = self.variant_tys(a.id, *index);
                for (i, sub) in subs.iter().enumerate() {
                    let p = self.builder.build_struct_gep(vt, payload, i as u32, "").map_err(llvm_err)?;
                    path.push(*index);
                    path.push(i);
                    self.pat_bind(sub, Some(p), &tys[i], base, path)?;
                    path.pop();
                    path.pop();
                }
                Ok(())
            }
            // Or-patterns have no bindings (checked).
            _ => Ok(()),
        }
    }

    pub(super) fn bind(
        &mut self,
        id: tir::LocalId,
        mode: tir::BindMode,
        place: Option<PointerValue<'ctx>>,
        ty: &Ty,
        base: Option<&(tir::LocalId, Vec<usize>)>,
        path: &[usize],
    ) -> GenResult<()> {
        if let (Some(slot), Some(ptr), Some(t)) = (self.slots[id.0], place, self.llvm_ty(ty)) {
            let v = self.builder.build_load(t, ptr, "").map_err(llvm_err)?;
            self.builder.build_store(slot, v).map_err(llvm_err)?;
        }
        self.set_initialized(id, &[]);
        if mode == tir::BindMode::Move
            && let Some((root, base_path)) = base
        {
            let mut full = base_path.clone();
            full.extend_from_slice(path);
            self.mark_moved(*root, &full);
        }
        Ok(())
    }
}

/// The locals a pattern binds, in source order.
pub(super) fn collect_bindings(pat: &Pat, out: &mut Vec<tir::LocalId>) {
    match pat {
        Pat::Wild | Pat::Bool(_) | Pat::Int(_) | Pat::Char(_) | Pat::Str(_) | Pat::Range(..) | Pat::None => {}
        Pat::Binding(id, _) => out.push(*id),
        Pat::At(id, _, inner) => {
            out.push(*id);
            collect_bindings(inner, out);
        }
        Pat::Or(alts) => alts.iter().for_each(|a| collect_bindings(a, out)),
        Pat::Some(inner) => collect_bindings(inner, out),
        Pat::Ok(inner) | Pat::Err(inner) => collect_bindings(inner, out),
        Pat::Product(subs) | Pat::Variant(_, subs) => subs.iter().for_each(|s| collect_bindings(s, out)),
    }
}

/// Whether a pattern takes any part of the scrutinee by move.
pub(super) fn pattern_moves(pat: &Pat) -> bool {
    match pat {
        Pat::Binding(_, mode) => *mode == tir::BindMode::Move,
        Pat::At(_, mode, inner) => *mode == tir::BindMode::Move || pattern_moves(inner),
        Pat::Or(alts) => alts.iter().any(pattern_moves),
        Pat::Some(inner) => pattern_moves(inner),
        Pat::Ok(inner) | Pat::Err(inner) => pattern_moves(inner),
        Pat::Product(subs) | Pat::Variant(_, subs) => subs.iter().any(pattern_moves),
        _ => false,
    }
}
