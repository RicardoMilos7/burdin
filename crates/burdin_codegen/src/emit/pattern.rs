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
            Pat::Slice { prefix, rest, suffix } => {
                // A slice's bindings are views and move nothing (`spec/array-patterns.md`, AP9).
                let Ty::Array(elem, n) = ty else { return };
                let elem = (**elem).clone();
                let suffix_start = (*n as usize).saturating_sub(suffix.len());
                for (i, sub) in prefix.iter().enumerate() {
                    path.push(i);
                    self.pattern_moved_paths(sub, &elem, path, out);
                    path.pop();
                }
                if let Some(Pat::Binding(_, tir::BindMode::Move)) = rest.as_deref() {
                    for i in prefix.len()..suffix_start {
                        let mut moved = path.clone();
                        moved.push(i);
                        out.push(moved);
                    }
                }
                for (j, sub) in suffix.iter().enumerate() {
                    path.push(suffix_start + j);
                    self.pattern_moved_paths(sub, &elem, path, out);
                    path.pop();
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
                let Ty::Enum(a) = ty else { unreachable!("checked: a variant pattern on an enum, found {ty:?}") };
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
            Pat::Slice { prefix, rest, suffix } => self.pat_test_slice(prefix, rest.as_deref(), suffix, place, ty),
        }
    }

    /// `[prefix, rest, suffix]` against an array or slice (`spec/array-patterns.md`, AP11). An array's length is in its
    /// type, so its elements are tested where they are. A slice's length is tested first, and its elements are read only
    /// once that test has passed, so a test never reads past the end of the view.
    fn pat_test_slice(
        &mut self,
        prefix: &[Pat],
        rest: Option<&Pat>,
        suffix: &[Pat],
        place: Option<PointerValue<'ctx>>,
        ty: &Ty,
    ) -> GenResult<IntValue<'ctx>> {
        let bool_ty = self.cx.bool_type();
        let yes = bool_ty.const_int(1, false);
        let no = bool_ty.const_int(0, false);
        let tested = |p: &Pat| !matches!(p, Pat::Wild | Pat::Binding(..));
        let any_tested = prefix.iter().chain(suffix).any(tested);
        match ty {
            Ty::Array(elem, n) => {
                let (elem, n) = ((**elem).clone(), *n as usize);
                // A zero-sized element has no storage, and its patterns test nothing.
                let (Some(ptr), Some(array_t), Some(_)) = (place, self.llvm_ty(ty), self.llvm_ty(&elem)) else {
                    return Ok(yes);
                };
                if !any_tested {
                    return Ok(yes);
                }
                let zero = self.usize_ty.const_int(0, false);
                let mut acc = yes;
                let suffix_start = n - suffix.len();
                let positions =
                    prefix.iter().enumerate().chain(suffix.iter().enumerate().map(|(j, p)| (suffix_start + j, p)));
                for (i, sub) in positions {
                    if !tested(sub) {
                        continue;
                    }
                    let index = self.usize_ty.const_int(i as u64, false);
                    // SAFETY (LLVM): `i` is an element of the array (the checker fixed its length).
                    let p = unsafe { self.builder.build_in_bounds_gep(array_t, ptr, &[zero, index], "") }
                        .map_err(llvm_err)?;
                    let ok = self.pat_test(sub, Some(p), &elem)?;
                    acc = self.builder.build_and(acc, ok, "").map_err(llvm_err)?;
                }
                Ok(acc)
            }
            Ty::Slice(elem) => {
                let elem = (**elem).clone();
                let Some(view_ptr) = place else { return Ok(yes) };
                let view = self.builder.build_load(self.str_ty, view_ptr, "").map_err(llvm_err)?.into_struct_value();
                let data =
                    self.builder.build_extract_value(view, 0, "slice.ptr").map_err(llvm_err)?.into_pointer_value();
                let len = self.builder.build_extract_value(view, 1, "slice.len").map_err(llvm_err)?.into_int_value();
                let named = prefix.len() + suffix.len();
                let len_ok = if rest.is_some() {
                    let least = self.usize_ty.const_int(named as u64, false);
                    self.builder.build_int_compare(IntPredicate::UGE, len, least, "").map_err(llvm_err)?
                } else {
                    let exact = self.usize_ty.const_int(prefix.len() as u64, false);
                    self.builder.build_int_compare(IntPredicate::EQ, len, exact, "").map_err(llvm_err)?
                };
                let Some(elem_t) = self.llvm_ty(&elem).filter(|_| any_tested) else { return Ok(len_ok) };
                // The elements are read in a block of their own, entered only when the length is right.
                let from = self.builder.get_insert_block().expect("a block is open");
                let check = self.new_block("slice.check");
                let done = self.new_block("slice.done");
                self.builder.build_conditional_branch(len_ok, check, done).map_err(llvm_err)?;
                self.builder.position_at_end(check);
                let mut acc = yes;
                for (j, sub) in prefix.iter().enumerate() {
                    if !tested(sub) {
                        continue;
                    }
                    let index = self.usize_ty.const_int(j as u64, false);
                    // SAFETY (LLVM): the length test above put `j` inside the view.
                    let p =
                        unsafe { self.builder.build_in_bounds_gep(elem_t, data, &[index], "") }.map_err(llvm_err)?;
                    let ok = self.pat_test(sub, Some(p), &elem)?;
                    acc = self.builder.build_and(acc, ok, "").map_err(llvm_err)?;
                }
                for (k, sub) in suffix.iter().enumerate() {
                    if !tested(sub) {
                        continue;
                    }
                    let back = self.usize_ty.const_int((suffix.len() - k) as u64, false);
                    let index = self.builder.build_int_sub(len, back, "").map_err(llvm_err)?;
                    // SAFETY (LLVM): the length test above put `len - back` inside the view.
                    let p =
                        unsafe { self.builder.build_in_bounds_gep(elem_t, data, &[index], "") }.map_err(llvm_err)?;
                    let ok = self.pat_test(sub, Some(p), &elem)?;
                    acc = self.builder.build_and(acc, ok, "").map_err(llvm_err)?;
                }
                let end = self.builder.get_insert_block().expect("a block is open");
                self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
                self.builder.position_at_end(done);
                let phi = self.builder.build_phi(bool_ty, "").map_err(llvm_err)?;
                phi.add_incoming(&[(&no, from), (&acc, end)]);
                Ok(phi.as_basic_value().into_int_value())
            }
            _ => Ok(yes),
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
            Pat::Slice { prefix, rest, suffix } => {
                self.pat_bind_slice(prefix, rest.as_deref(), suffix, place, ty, base, path)
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

    /// Binds the parts of an array or slice that `pat` names (`spec/array-patterns.md`, AP6 and AP9). A rest binding of
    /// an array is a copy of its elements, and each element it takes is a partial move of the array; a rest binding of
    /// a slice is a view of the middle, which moves nothing.
    #[allow(clippy::too_many_arguments)]
    fn pat_bind_slice(
        &mut self,
        prefix: &[Pat],
        rest: Option<&Pat>,
        suffix: &[Pat],
        place: Option<PointerValue<'ctx>>,
        ty: &Ty,
        base: Option<&(tir::LocalId, Vec<usize>)>,
        path: &mut Vec<usize>,
    ) -> GenResult<()> {
        let Some(place) = place else { return Ok(()) };
        match ty {
            Ty::Array(elem, n) => {
                let (elem, n) = ((**elem).clone(), *n as usize);
                let elem_t = self.llvm_ty(&elem);
                let array_t = self.llvm_ty(ty);
                let zero = self.usize_ty.const_int(0, false);
                let suffix_start = n - suffix.len();
                let element = |cg: &mut Self, i: usize| -> GenResult<Option<PointerValue<'ctx>>> {
                    match (array_t, elem_t) {
                        (Some(at), Some(_)) => {
                            let index = cg.usize_ty.const_int(i as u64, false);
                            // SAFETY (LLVM): `i` is an element of the array, as the checker fixed its length.
                            Ok(Some(
                                unsafe { cg.builder.build_in_bounds_gep(at, place, &[zero, index], "") }
                                    .map_err(llvm_err)?,
                            ))
                        }
                        _ => Ok(None),
                    }
                };
                for (i, sub) in prefix.iter().enumerate() {
                    let p = element(self, i)?;
                    path.push(i);
                    self.pat_bind(sub, p, &elem, base, path)?;
                    path.pop();
                }
                if let Some(Pat::Binding(id, mode)) = rest {
                    // The rest is an array of the elements between the named ones: its storage is their address.
                    let rest_ty = Ty::Array(Box::new(elem.clone()), (suffix_start - prefix.len()) as u64);
                    let p = element(self, prefix.len())?;
                    self.bind(*id, *mode, p, &rest_ty, None, &[])?;
                    if *mode == tir::BindMode::Move
                        && let Some((root, base_path)) = base
                    {
                        for i in prefix.len()..suffix_start {
                            let mut full = base_path.clone();
                            full.extend_from_slice(path);
                            full.push(i);
                            self.mark_moved(*root, &full);
                        }
                    }
                }
                for (j, sub) in suffix.iter().enumerate() {
                    let p = element(self, suffix_start + j)?;
                    path.push(suffix_start + j);
                    self.pat_bind(sub, p, &elem, base, path)?;
                    path.pop();
                }
                Ok(())
            }
            Ty::Slice(elem) => {
                let elem = (**elem).clone();
                let elem_t = self.llvm_ty(&elem);
                let view = self.builder.build_load(self.str_ty, place, "").map_err(llvm_err)?.into_struct_value();
                let data =
                    self.builder.build_extract_value(view, 0, "slice.ptr").map_err(llvm_err)?.into_pointer_value();
                let len = self.builder.build_extract_value(view, 1, "slice.len").map_err(llvm_err)?.into_int_value();
                let element = |cg: &mut Self, index: IntValue<'ctx>| -> GenResult<Option<PointerValue<'ctx>>> {
                    match elem_t {
                        Some(et) => Ok(Some(
                            unsafe { cg.builder.build_in_bounds_gep(et, data, &[index], "") }.map_err(llvm_err)?,
                        )),
                        None => Ok(None),
                    }
                };
                for (j, sub) in prefix.iter().enumerate() {
                    let index = self.usize_ty.const_int(j as u64, false);
                    let p = element(self, index)?;
                    self.pat_bind(sub, p, &elem, None, &mut Vec::new())?;
                }
                let named = prefix.len() + suffix.len();
                if let Some(Pat::Binding(id, mode)) = rest {
                    // The view of the middle: the data moved past the named prefix, and the length less the named.
                    let first = self.usize_ty.const_int(prefix.len() as u64, false);
                    let middle_data = match elem_t {
                        Some(et) => {
                            unsafe { self.builder.build_in_bounds_gep(et, data, &[first], "") }.map_err(llvm_err)?
                        }
                        None => data,
                    };
                    let count = self.usize_ty.const_int(named as u64, false);
                    let middle_len = self.builder.build_int_sub(len, count, "").map_err(llvm_err)?;
                    let mut agg = self.str_ty.get_undef();
                    agg =
                        self.builder.build_insert_value(agg, middle_data, 0, "").map_err(llvm_err)?.into_struct_value();
                    agg =
                        self.builder.build_insert_value(agg, middle_len, 1, "").map_err(llvm_err)?.into_struct_value();
                    let slot = self.alloca(self.str_ty.into(), "slice.rest")?;
                    self.builder.build_store(slot, agg).map_err(llvm_err)?;
                    self.bind(*id, *mode, Some(slot), &Ty::Slice(Box::new(elem.clone())), None, &[])?;
                }
                for (k, sub) in suffix.iter().enumerate() {
                    let back = self.usize_ty.const_int((suffix.len() - k) as u64, false);
                    let index = self.builder.build_int_sub(len, back, "").map_err(llvm_err)?;
                    let p = element(self, index)?;
                    self.pat_bind(sub, p, &elem, None, &mut Vec::new())?;
                }
                Ok(())
            }
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
        Pat::Wild | Pat::Bool(_) | Pat::Int(_) | Pat::Char(_) | Pat::Str(_) | Pat::Range(..) => {}
        Pat::Binding(id, _) => out.push(*id),
        Pat::At(id, _, inner) => {
            out.push(*id);
            collect_bindings(inner, out);
        }
        Pat::Or(alts) => alts.iter().for_each(|a| collect_bindings(a, out)),
        Pat::Product(subs) | Pat::Variant(_, subs) => subs.iter().for_each(|s| collect_bindings(s, out)),
        Pat::Slice { prefix, rest, suffix } => {
            for s in prefix.iter().chain(rest.as_deref()).chain(suffix) {
                collect_bindings(s, out);
            }
        }
    }
}

/// Whether a pattern takes any part of the scrutinee by move.
pub(super) fn pattern_moves(pat: &Pat) -> bool {
    match pat {
        Pat::Binding(_, mode) => *mode == tir::BindMode::Move,
        Pat::At(_, mode, inner) => *mode == tir::BindMode::Move || pattern_moves(inner),
        Pat::Or(alts) => alts.iter().any(pattern_moves),
        Pat::Product(subs) | Pat::Variant(_, subs) => subs.iter().any(pattern_moves),
        Pat::Slice { prefix, rest, suffix } => prefix.iter().chain(rest.as_deref()).chain(suffix).any(pattern_moves),
        _ => false,
    }
}
