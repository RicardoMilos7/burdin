//! Control flow: loops, `if`, and `match`.

use super::pattern::{collect_bindings, pattern_moves};
use super::{Codegen, GenResult, LoopTargets, Mode, llvm_err};
use burdin_tir::{self as tir, Pat, Ty};
use inkwell::basic_block::BasicBlock;
use inkwell::values::BasicValueEnum;
use inkwell::{AddressSpace, IntPredicate};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// Drops every scope above `depth`, from the innermost out; used by `break`, `continue`, and `return` (DR6).
    pub(super) fn drop_scopes_above(&mut self, depth: usize) -> GenResult<()> {
        for index in (depth..self.scopes.len()).rev() {
            self.drop_scope(index)?;
        }
        Ok(())
    }

    pub(super) fn loop_body(
        &mut self,
        body: &'a tir::Block,
        continue_bb: BasicBlock<'ctx>,
        break_bb: BasicBlock<'ctx>,
    ) -> GenResult<()> {
        self.loops.push(LoopTargets {
            continue_bb,
            break_bb,
            temp_depth: self.temps.len(),
            scope_depth: self.scopes.len(),
        });
        self.block(body, Mode::Use)?;
        self.loops.pop();
        Ok(())
    }

    pub(super) fn for_range(
        &mut self,
        var: Option<tir::LocalId>,
        lo: &'a tir::Expr,
        hi: &'a tir::Expr,
        inclusive: bool,
        body: &'a tir::Block,
    ) -> GenResult<()> {
        let Ty::Int(ity) = lo.ty.clone() else {
            // The range diverges (for example `return..x`); nothing after it runs.
            self.expr_mode(lo, Mode::Use)?;
            self.expr_mode(hi, Mode::Use)?;
            return Ok(());
        };
        let lo_v = self.expr_mode(lo, Mode::Use)?;
        let lo_v = self.need_int(lo_v, &lo.ty);
        let hi_v = self.expr_mode(hi, Mode::Use)?;
        let hi_v = self.need_int(hi_v, &lo.ty);
        // `for i in 0..xs.len()` gives `i` a range that proves `xs[i]` in bounds for the whole body (issue #428).
        let fact = match (&lo.kind, &hi.kind, var, inclusive) {
            (tir::ExprKind::Int(0), tir::ExprKind::Method(tir::Method::Len, args), Some(var), false) => {
                match args.as_slice() {
                    [base] => match &base.kind {
                        tir::ExprKind::Local(collection) => Some((var, *collection)),
                        _ => None,
                    },
                    _ => None,
                }
            }
            _ => None,
        };
        self.range_loop(var, ity, inclusive, lo_v, hi_v, fact, body)
    }

    /// `for i in range`: evaluates the range value once and iterates its endpoints.
    pub(super) fn for_range_value(
        &mut self,
        var: Option<tir::LocalId>,
        range: &'a tir::Expr,
        body: &'a tir::Block,
    ) -> GenResult<()> {
        let (elem, inclusive) = match &range.ty {
            Ty::Range(inner, inclusive) => ((**inner).clone(), *inclusive),
            // The range already failed to type-check; evaluate it for its side effects and stop.
            _ => {
                self.expr_mode(range, Mode::Use)?;
                return Ok(());
            }
        };
        let Ty::Int(ity) = elem else {
            self.expr_mode(range, Mode::Use)?;
            return Ok(());
        };
        let value = self.expr_mode(range, Mode::Use)?;
        let value = self.need(value, &range.ty);
        let value = value.into_struct_value();
        let lo_v = self.builder.build_extract_value(value, 0, "range.lo").map_err(llvm_err)?.into_int_value();
        let hi_v = self.builder.build_extract_value(value, 1, "range.hi").map_err(llvm_err)?.into_int_value();
        self.range_loop(var, ity, inclusive, lo_v, hi_v, None, body)
    }

    /// The shared loop for both range forms. `fact` is a bounds-check fact the body may use.
    #[allow(clippy::too_many_arguments)]
    fn range_loop(
        &mut self,
        var: Option<tir::LocalId>,
        ity: burdin_tir::IntTy,
        inclusive: bool,
        lo_v: inkwell::values::IntValue<'ctx>,
        hi_v: inkwell::values::IntValue<'ctx>,
        fact: Option<(tir::LocalId, tir::LocalId)>,
        body: &'a tir::Block,
    ) -> GenResult<()> {
        let signed = ity.signed();
        let counter = self.alloca(self.int_ty(ity).into(), "for.counter")?;
        self.builder.build_store(counter, lo_v).map_err(llvm_err)?;
        let header = self.new_block("for.cond");
        let body_bb = self.new_block("for.body");
        let latch = self.new_block("for.next");
        let exit = self.new_block("for.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;

        self.builder.position_at_end(header);
        let cur = self.builder.build_load(self.int_ty(ity), counter, "i").map_err(llvm_err)?.into_int_value();
        let pred = match (inclusive, signed) {
            (true, true) => IntPredicate::SLE,
            (true, false) => IntPredicate::ULE,
            (false, true) => IntPredicate::SLT,
            (false, false) => IntPredicate::ULT,
        };
        let go = self.builder.build_int_compare(pred, cur, hi_v, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body_bb, exit).map_err(llvm_err)?;

        self.builder.position_at_end(body_bb);
        if let Some(var) = var
            && let Some(slot) = self.slots[var.0]
        {
            self.builder.build_store(slot, cur).map_err(llvm_err)?;
        }
        if let Some(fact) = fact {
            self.bounds_facts.push(fact);
        }
        self.loop_body(body, latch, exit)?;
        if fact.is_some() {
            self.bounds_facts.pop();
        }
        self.builder.build_unconditional_branch(latch).map_err(llvm_err)?;

        // `cur < hi` (or `cur != hi` for an inclusive range) holds here, so the increment cannot overflow.
        self.builder.position_at_end(latch);
        let cur = self.builder.build_load(self.int_ty(ity), counter, "i").map_err(llvm_err)?.into_int_value();
        let inc = self.new_block("for.inc");
        if inclusive {
            let last = self.builder.build_int_compare(IntPredicate::EQ, cur, hi_v, "").map_err(llvm_err)?;
            self.builder.build_conditional_branch(last, exit, inc).map_err(llvm_err)?;
        } else {
            self.builder.build_unconditional_branch(inc).map_err(llvm_err)?;
        }
        self.builder.position_at_end(inc);
        let one = self.int_ty(ity).const_int(1, false);
        let next = self.builder.build_int_add(cur, one, "").map_err(llvm_err)?;
        self.builder.build_store(counter, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;

        self.builder.position_at_end(exit);
        Ok(())
    }

    pub(super) fn if_expr(
        &mut self,
        cond: &'a tir::Expr,
        then: &'a tir::Block,
        other: Option<&'a tir::Expr>,
        ty: &Ty,
        mode: Mode,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let c = self.expr_mode(cond, Mode::Use)?;
        let c = self.need_int(c, &Ty::Bool);
        let result = match self.llvm_ty(ty) {
            Some(t) => Some(self.alloca(t, "if.result")?),
            None => None,
        };
        let then_bb = self.new_block("if.then");
        let else_bb = self.new_block("if.else");
        let merge = self.new_block("if.end");
        self.builder.build_conditional_branch(c, then_bb, else_bb).map_err(llvm_err)?;

        self.builder.position_at_end(then_bb);
        let v = self.block(then, mode)?;
        if let (Some(slot), Some(v)) = (result, v) {
            self.builder.build_store(slot, v).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(merge).map_err(llvm_err)?;

        self.builder.position_at_end(else_bb);
        if let Some(other) = other {
            let v = self.expr_mode(other, mode)?;
            if let (Some(slot), Some(v)) = (result, v) {
                self.builder.build_store(slot, v).map_err(llvm_err)?;
            }
        }
        self.builder.build_unconditional_branch(merge).map_err(llvm_err)?;

        self.builder.position_at_end(merge);
        match (result, self.llvm_ty(ty)) {
            (Some(slot), Some(t)) => Ok(Some(self.builder.build_load(t, slot, "").map_err(llvm_err)?)),
            _ => Ok(None),
        }
    }

    pub(super) fn match_expr(
        &mut self,
        scrutinee: &'a tir::Expr,
        arms: &'a [tir::Arm],
        ty: &Ty,
        mode: Mode,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        // A place scrutinee is matched in place: moving bindings take parts out of it (MV8). A temporary is
        // spilled; it is dropped at the end of the statement unless an arm moves parts out of it (which the
        // generated code cannot track), in which case its ownership passes to the bindings.
        let direct = self.direct_place_ptr(scrutinee)?;
        let place = match direct {
            Some(ptr) => Some(ptr),
            None => {
                let v = self.expr_mode(scrutinee, Mode::Use)?;
                let slot = match self.llvm_ty(&scrutinee.ty) {
                    Some(t) => {
                        let slot = self.alloca(t, "match.value")?;
                        let v = self.need(v, &scrutinee.ty);
                        self.builder.build_store(slot, v).map_err(llvm_err)?;
                        Some(slot)
                    }
                    None => None,
                };
                if arms.iter().all(|a| !pattern_moves(&a.pat)) {
                    self.add_temp_slot(
                        slot.unwrap_or_else(|| self.cx.ptr_type(AddressSpace::default()).get_undef()),
                        &scrutinee.ty,
                    );
                }
                slot
            }
        };
        let base = if direct.is_some() { self.place_path(scrutinee) } else { None };
        let result = match self.llvm_ty(ty) {
            Some(t) => Some(self.alloca(t, "match.result")?),
            None => None,
        };
        let merge = self.new_block("match.end");
        for arm in arms {
            let arm_bb = self.new_block("match.arm");
            let next = self.new_block("match.next");
            let ok = self.pat_test(&arm.pat, place, &scrutinee.ty)?;
            self.builder.build_conditional_branch(ok, arm_bb, next).map_err(llvm_err)?;
            self.builder.position_at_end(arm_bb);
            self.scopes.push(Vec::new());
            self.pat_bind(&arm.pat, place, &scrutinee.ty, base.as_ref(), &mut Vec::new())?;
            let mut bound = Vec::new();
            collect_bindings(&arm.pat, &mut bound);
            for id in bound {
                self.scopes.last_mut().expect("an arm scope is open").push(id);
            }
            if let Some(guard) = &arm.guard {
                let g = self.expr_mode(guard, Mode::Use)?;
                let g = self.need_int(g, &Ty::Bool);
                let body_bb = self.new_block("match.body");
                let fail_bb = self.new_block("match.guard.fail");
                self.builder.build_conditional_branch(g, body_bb, fail_bb).map_err(llvm_err)?;
                // A failed guard does not take the arm: the parts the pattern bound are still owned by the
                // scrutinee, so put their drop flags back. The binding slots are discarded unread.
                self.builder.position_at_end(fail_bb);
                if let Some((root, base)) = &base {
                    let mut paths = Vec::new();
                    self.pattern_moved_paths(&arm.pat, &scrutinee.ty, &mut Vec::new(), &mut paths);
                    for path in paths {
                        let mut full = base.clone();
                        full.extend(path);
                        self.set_initialized(*root, &full);
                    }
                }
                self.builder.build_unconditional_branch(next).map_err(llvm_err)?;
                self.builder.position_at_end(body_bb);
            }
            let v = self.expr_mode(&arm.body, mode)?;
            // The arm's bindings drop at the end of the arm (MV8).
            self.drop_scope(self.scopes.len() - 1)?;
            self.scopes.pop();
            if let (Some(slot), Some(v)) = (result, v) {
                self.builder.build_store(slot, v).map_err(llvm_err)?;
            }
            self.builder.build_unconditional_branch(merge).map_err(llvm_err)?;
            self.builder.position_at_end(next);
        }
        // The checker proved the arms exhaustive.
        self.builder.build_unreachable().map_err(llvm_err)?;
        self.builder.position_at_end(merge);
        match (result, self.llvm_ty(ty)) {
            (Some(slot), Some(t)) => Ok(Some(self.builder.build_load(t, slot, "").map_err(llvm_err)?)),
            _ => Ok(None),
        }
    }

    /// `for pattern in array`: binds each element in order.
    pub(super) fn for_array(&mut self, pat: &'a Pat, array: &'a tir::Expr, body: &'a tir::Block) -> GenResult<()> {
        let Ty::Array(elem, n) = &array.ty else {
            // The array expression diverges; nothing after it runs.
            self.expr_mode(array, Mode::Use)?;
            return Ok(());
        };
        let base = self.place_ptr(array)?;
        let array_t = self.llvm_ty(&array.ty);
        let counter = self.alloca(self.usize_ty.into(), "for.index")?;
        self.builder.build_store(counter, self.usize_ty.const_zero()).map_err(llvm_err)?;
        let header = self.new_block("for.cond");
        let body_bb = self.new_block("for.body");
        let latch = self.new_block("for.next");
        let exit = self.new_block("for.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;

        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, counter, "i").map_err(llvm_err)?.into_int_value();
        let len = self.usize_ty.const_int(*n, false);
        let go = self.builder.build_int_compare(IntPredicate::ULT, i, len, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body_bb, exit).map_err(llvm_err)?;

        self.builder.position_at_end(body_bb);
        let elem_ptr = match (base, array_t) {
            (Some(base), Some(t)) => {
                let zero = self.usize_ty.const_zero();
                // SAFETY (LLVM): i < n.
                Some(unsafe { self.builder.build_in_bounds_gep(t, base, &[zero, i], "") }.map_err(llvm_err)?)
            }
            _ => None,
        };
        self.pat_bind(pat, elem_ptr, elem, None, &mut Vec::new())?;
        self.loop_body(body, latch, exit)?;
        self.builder.build_unconditional_branch(latch).map_err(llvm_err)?;

        self.builder.position_at_end(latch);
        let i = self.builder.build_load(self.usize_ty, counter, "i").map_err(llvm_err)?.into_int_value();
        let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(counter, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(exit);
        Ok(())
    }

    /// `for pattern in list`: binds each element in order; the list is borrowed.
    pub(super) fn for_list(&mut self, pat: &'a Pat, list: &'a tir::Expr, body: &'a tir::Block) -> GenResult<()> {
        let Ty::List(elem) = &list.ty else {
            self.expr_mode(list, Mode::Use)?;
            return Ok(());
        };
        let elem = (**elem).clone();
        let v = self.expr_mode(list, Mode::Use)?;
        let s = self.need(v, &list.ty).into_struct_value();
        let base = self.builder.build_extract_value(s, 0, "list.ptr").map_err(llvm_err)?.into_pointer_value();
        let len = self.builder.build_extract_value(s, 1, "list.len").map_err(llvm_err)?.into_int_value();
        let counter = self.alloca(self.usize_ty.into(), "for.index")?;
        self.builder.build_store(counter, self.usize_ty.const_zero()).map_err(llvm_err)?;
        let header = self.new_block("for.cond");
        let body_bb = self.new_block("for.body");
        let latch = self.new_block("for.next");
        let exit = self.new_block("for.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;

        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, counter, "i").map_err(llvm_err)?.into_int_value();
        let go = self.builder.build_int_compare(IntPredicate::ULT, i, len, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body_bb, exit).map_err(llvm_err)?;

        self.builder.position_at_end(body_bb);
        let elem_ptr = match self.llvm_ty(&elem) {
            Some(t) => {
                // SAFETY (LLVM): i < len.
                Some(unsafe { self.builder.build_in_bounds_gep(t, base, &[i], "") }.map_err(llvm_err)?)
            }
            None => None,
        };
        self.pat_bind(pat, elem_ptr, &elem, None, &mut Vec::new())?;
        self.loop_body(body, latch, exit)?;
        self.builder.build_unconditional_branch(latch).map_err(llvm_err)?;

        self.builder.position_at_end(latch);
        let i = self.builder.build_load(self.usize_ty, counter, "i").map_err(llvm_err)?.into_int_value();
        let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(counter, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(exit);
        Ok(())
    }

    /// `for pattern in map`: binds each live `(key, value)` pair; the map is borrowed. The hash table's slots are
    /// scanned in order and empty ones are skipped, so the iteration order is unspecified.
    pub(super) fn for_map(&mut self, pat: &'a Pat, map: &'a tir::Expr, body: &'a tir::Block) -> GenResult<()> {
        let Ty::Map(key, value) = &map.ty else {
            self.expr_mode(map, Mode::Use)?;
            return Ok(());
        };
        let pair = Ty::Tuple(vec![(**key).clone(), (**value).clone()]);
        let entry = Self::map_entry_ty(key, value);
        let entry_t = self.llvm_ty(&entry).expect("a map entry has a type").into_struct_type();
        let v = self.expr_mode(map, Mode::Use)?;
        let s = self.need(v, &map.ty).into_struct_value();
        let base = self.builder.build_extract_value(s, 0, "map.ptr").map_err(llvm_err)?.into_pointer_value();
        let cap = self.builder.build_extract_value(s, 2, "map.cap").map_err(llvm_err)?.into_int_value();
        let counter = self.alloca(self.usize_ty.into(), "for.index")?;
        self.builder.build_store(counter, self.usize_ty.const_zero()).map_err(llvm_err)?;
        let header = self.new_block("for.cond");
        let body_bb = self.new_block("for.body");
        let bind = self.new_block("for.bind");
        let latch = self.new_block("for.next");
        let exit = self.new_block("for.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;

        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, counter, "i").map_err(llvm_err)?.into_int_value();
        let go = self.builder.build_int_compare(IntPredicate::ULT, i, cap, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body_bb, exit).map_err(llvm_err)?;

        self.builder.position_at_end(body_bb);
        // SAFETY (LLVM): i < cap.
        let slot = unsafe { self.builder.build_in_bounds_gep(entry_t, base, &[i], "") }.map_err(llvm_err)?;
        let hash_field = self.builder.build_struct_gep(entry_t, slot, 2, "").map_err(llvm_err)?;
        let hash =
            self.builder.build_load(self.cx.i64_type(), hash_field, "map.hash").map_err(llvm_err)?.into_int_value();
        let occupied = self
            .builder
            .build_int_compare(IntPredicate::NE, hash, self.cx.i64_type().const_zero(), "")
            .map_err(llvm_err)?;
        self.builder.build_conditional_branch(occupied, bind, latch).map_err(llvm_err)?;

        self.builder.position_at_end(bind);
        // The entry is `(key, value, hash)`; a `(key, value)` tuple shares the first two fields' layout, so the
        // pattern binds views into the map at the same addresses the key and value occupy.
        self.pat_bind(pat, Some(slot), &pair, None, &mut Vec::new())?;
        self.loop_body(body, latch, exit)?;
        self.builder.build_unconditional_branch(latch).map_err(llvm_err)?;

        self.builder.position_at_end(latch);
        let i = self.builder.build_load(self.usize_ty, counter, "i").map_err(llvm_err)?.into_int_value();
        let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(counter, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(exit);
        Ok(())
    }

    /// `for pattern in string.chars()`: binds each UTF-8 character in order.
    pub(super) fn for_chars(&mut self, pat: &'a Pat, string: &'a tir::Expr, body: &'a tir::Block) -> GenResult<()> {
        let v = self.expr_mode(string, Mode::Use)?;
        let s = self.need(v, &Ty::Str).into_struct_value();
        let base = self.builder.build_extract_value(s, 0, "chars.ptr").map_err(llvm_err)?.into_pointer_value();
        let len = self.builder.build_extract_value(s, 1, "chars.len").map_err(llvm_err)?.into_int_value();
        let counter = self.alloca(self.usize_ty.into(), "for.index")?;
        self.builder.build_store(counter, self.usize_ty.const_zero()).map_err(llvm_err)?;
        let header = self.new_block("for.cond");
        let body_bb = self.new_block("for.body");
        let latch = self.new_block("for.next");
        let exit = self.new_block("for.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;

        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, counter, "i").map_err(llvm_err)?.into_int_value();
        let go = self.builder.build_int_compare(IntPredicate::ULT, i, len, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body_bb, exit).map_err(llvm_err)?;

        self.builder.position_at_end(body_bb);
        // SAFETY (LLVM): i < len.
        let p = unsafe { self.builder.build_in_bounds_gep(self.cx.i8_type(), base, &[i], "") }.map_err(llvm_err)?;
        let b0 = self.builder.build_load(self.cx.i8_type(), p, "b0").map_err(llvm_err)?.into_int_value();
        let b0_wide = self.builder.build_int_z_extend(b0, self.cx.i32_type(), "").map_err(llvm_err)?;
        let i32_ty = self.cx.i32_type();
        let cont = |cg: &mut Self, index: u64| -> GenResult<inkwell::values::IntValue<'ctx>> {
            let at = cg.builder.build_int_add(i, cg.usize_ty.const_int(index, false), "").map_err(llvm_err)?;
            let p = unsafe { cg.builder.build_in_bounds_gep(cg.cx.i8_type(), base, &[at], "") }.map_err(llvm_err)?;
            let b = cg.builder.build_load(cg.cx.i8_type(), p, "").map_err(llvm_err)?.into_int_value();
            cg.builder.build_int_z_extend(b, i32_ty, "").map_err(llvm_err)
        };
        let cont1 = cont(self, 1)?;
        let cont2 = cont(self, 2)?;
        let cont3 = cont(self, 3)?;
        let mask = |cg: &mut Self, m: u64, want: u64| -> GenResult<inkwell::values::IntValue<'ctx>> {
            let v = cg.builder.build_and(b0_wide, i32_ty.const_int(m, false), "").map_err(llvm_err)?;
            cg.builder.build_int_compare(IntPredicate::EQ, v, i32_ty.const_int(want, false), "").map_err(llvm_err)
        };
        let is_1 = self
            .builder
            .build_int_compare(IntPredicate::ULT, b0_wide, i32_ty.const_int(0x80, false), "")
            .map_err(llvm_err)?;
        let is_2 = mask(self, 0xE0, 0xC0)?;
        let is_3 = mask(self, 0xF0, 0xE0)?;
        let is_4 = mask(self, 0xF8, 0xF0)?;
        let low = |cg: &mut Self,
                   v: inkwell::values::IntValue<'ctx>,
                   m: u64,
                   shift: u64|
         -> GenResult<inkwell::values::IntValue<'ctx>> {
            let a = cg.builder.build_and(v, i32_ty.const_int(m, false), "").map_err(llvm_err)?;
            cg.builder.build_left_shift(a, i32_ty.const_int(shift, false), "").map_err(llvm_err)
        };
        let cont_mask = |cg: &mut Self,
                         v: inkwell::values::IntValue<'ctx>,
                         shift: u64|
         -> GenResult<inkwell::values::IntValue<'ctx>> { low(cg, v, 0x3F, shift) };
        let four = low(self, b0_wide, 0x07, 18)?;
        let three = low(self, b0_wide, 0x0F, 12)?;
        let two = low(self, b0_wide, 0x1F, 6)?;
        let m1_12 = cont_mask(self, cont1, 12)?;
        let m1_6 = cont_mask(self, cont1, 6)?;
        let m1_0 = cont_mask(self, cont1, 0)?;
        let m2_6 = cont_mask(self, cont2, 6)?;
        let m2_0 = cont_mask(self, cont2, 0)?;
        let m3_0 = cont_mask(self, cont3, 0)?;
        let cp4 = self.builder.build_or(four, m1_12, "").map_err(llvm_err)?;
        let cp4 = self.builder.build_or(cp4, m2_6, "").map_err(llvm_err)?;
        let cp4 = self.builder.build_or(cp4, m3_0, "").map_err(llvm_err)?;
        let cp3 = self.builder.build_or(three, m1_6, "").map_err(llvm_err)?;
        let cp3 = self.builder.build_or(cp3, m2_0, "").map_err(llvm_err)?;
        let cp2 = self.builder.build_or(two, m1_0, "").map_err(llvm_err)?;
        let cp = self.builder.build_select(is_2, cp2, cp4, "").map_err(llvm_err)?.into_int_value();
        let cp = self.builder.build_select(is_3, cp3, cp, "").map_err(llvm_err)?.into_int_value();
        let cp = self.builder.build_select(is_1, b0_wide, cp, "").map_err(llvm_err)?.into_int_value();
        let cp = self.builder.build_int_truncate(cp, self.cx.i32_type(), "").map_err(llvm_err)?;
        let slot = self.alloca(self.cx.i32_type().into(), "for.char")?;
        self.builder.build_store(slot, cp).map_err(llvm_err)?;
        self.pat_bind(pat, Some(slot), &Ty::Char, None, &mut Vec::new())?;
        let w2 = self
            .builder
            .build_select(is_2, self.usize_ty.const_int(2, false), self.usize_ty.const_int(1, false), "")
            .map_err(llvm_err)?
            .into_int_value();
        let w3 = self
            .builder
            .build_select(is_3, self.usize_ty.const_int(3, false), w2, "")
            .map_err(llvm_err)?
            .into_int_value();
        let w4 = self
            .builder
            .build_select(is_4, self.usize_ty.const_int(4, false), w3, "")
            .map_err(llvm_err)?
            .into_int_value();
        let width = self
            .builder
            .build_select(is_1, self.usize_ty.const_int(1, false), w4, "")
            .map_err(llvm_err)?
            .into_int_value();
        self.loop_body(body, latch, exit)?;
        self.builder.build_unconditional_branch(latch).map_err(llvm_err)?;

        self.builder.position_at_end(latch);
        let i = self.builder.build_load(self.usize_ty, counter, "i").map_err(llvm_err)?.into_int_value();
        let next = self.builder.build_int_add(i, width, "").map_err(llvm_err)?;
        self.builder.build_store(counter, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(exit);
        Ok(())
    }
    pub(super) fn for_slice(&mut self, pat: &'a Pat, slice: &'a tir::Expr, body: &'a tir::Block) -> GenResult<()> {
        let Ty::Slice(elem) = &slice.ty else {
            // The slice expression diverges; nothing after it runs.
            self.expr_mode(slice, Mode::Use)?;
            return Ok(());
        };
        let elem = (**elem).clone();
        let v = self.expr_mode(slice, Mode::Use)?;
        let s = self.need(v, &slice.ty).into_struct_value();
        let base = self.builder.build_extract_value(s, 0, "slice.ptr").map_err(llvm_err)?.into_pointer_value();
        let len = self.builder.build_extract_value(s, 1, "slice.len").map_err(llvm_err)?.into_int_value();
        let counter = self.alloca(self.usize_ty.into(), "for.index")?;
        self.builder.build_store(counter, self.usize_ty.const_zero()).map_err(llvm_err)?;
        let header = self.new_block("for.cond");
        let body_bb = self.new_block("for.body");
        let latch = self.new_block("for.next");
        let exit = self.new_block("for.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;

        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, counter, "i").map_err(llvm_err)?.into_int_value();
        let go = self.builder.build_int_compare(IntPredicate::ULT, i, len, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body_bb, exit).map_err(llvm_err)?;

        self.builder.position_at_end(body_bb);
        let elem_ptr = match self.llvm_ty(&elem) {
            Some(t) => {
                // SAFETY (LLVM): i < len.
                Some(unsafe { self.builder.build_in_bounds_gep(t, base, &[i], "") }.map_err(llvm_err)?)
            }
            None => None,
        };
        self.pat_bind(pat, elem_ptr, &elem, None, &mut Vec::new())?;
        self.loop_body(body, latch, exit)?;
        self.builder.build_unconditional_branch(latch).map_err(llvm_err)?;

        self.builder.position_at_end(latch);
        let i = self.builder.build_load(self.usize_ty, counter, "i").map_err(llvm_err)?.into_int_value();
        let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(counter, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(exit);
        Ok(())
    }
}
