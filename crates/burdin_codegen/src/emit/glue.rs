//! Drop and clone glue.

use super::{Codegen, GenResult, Glue, SavedContext, llvm_err};
use burdin_tir::Ty;
use inkwell::basic_block::BasicBlock;
use inkwell::module::Linkage;
use inkwell::types::BasicType;
use inkwell::values::{FunctionValue, IntValue, PointerValue};
use inkwell::{AddressSpace, IntPredicate};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    pub(super) fn drop_glue(&mut self, ty: &Ty) -> FunctionValue<'ctx> {
        let key = self.drop_key(ty);
        if let Some(f) =
            self.glue.iter().find(|(k, key2, _, _)| *k == Glue::Drop && *key2 == key).map(|(_, _, _, f)| *f)
        {
            return f;
        }
        let void = self.cx.void_type();
        let ptr = self.cx.ptr_type(AddressSpace::default());
        let name = format!("burdin.drop.{}", self.glue.len());
        let f = self.module.add_function(&name, void.fn_type(&[ptr.into()], false), Some(Linkage::Internal));
        self.glue.push((Glue::Drop, key, ty.clone(), f));
        self.pending_glue.push((Glue::Drop, ty.clone(), f));
        f
    }

    pub(super) fn clone_glue(&mut self, ty: &Ty) -> FunctionValue<'ctx> {
        let key = self.structural_key(ty);
        if let Some(f) =
            self.glue.iter().find(|(k, key2, _, _)| *k == Glue::Clone && *key2 == key).map(|(_, _, _, f)| *f)
        {
            return f;
        }
        let void = self.cx.void_type();
        let ptr = self.cx.ptr_type(AddressSpace::default());
        let name = format!("burdin.clone.{}", self.glue.len());
        let f =
            self.module.add_function(&name, void.fn_type(&[ptr.into(), ptr.into()], false), Some(Linkage::Internal));
        self.glue.push((Glue::Clone, key, ty.clone(), f));
        self.pending_glue.push((Glue::Clone, ty.clone(), f));
        f
    }

    /// The identity of a type's drop glue: a user `Drop` method belongs to one type, and a type without one drops
    /// exactly what its (recursively keyed) fields drop, so identical shapes share their glue (issue #426).
    fn drop_key(&self, ty: &Ty) -> String {
        match ty {
            Ty::Struct(a) if self.program.structs[a.id].drop_method.is_some() => format!("drop.{}", a.name),
            Ty::Enum(a) if self.program.enums[a.id].drop_method.is_some() => format!("drop.{}", a.name),
            _ => self.structural_key(ty),
        }
    }

    /// A type's shape, ignoring the names of structs and enums: two types with the same key clone field by field
    /// the same way and have the same generated layout. A type that contains itself (through `Heap`) is cut off
    /// with `rec`, so the key terminates and still distinguishes the non-recursive parts.
    fn structural_key(&self, ty: &Ty) -> String {
        self.structural_key_in(ty, &mut Vec::new())
    }

    fn structural_key_in(&self, ty: &Ty, seen: &mut Vec<String>) -> String {
        let identity = format!("{ty:?}");
        if seen.contains(&identity) {
            return "rec".to_string();
        }
        seen.push(identity);
        let key = self.structural_key_uncached(ty, seen);
        seen.pop();
        key
    }

    fn structural_key_uncached(&self, ty: &Ty, seen: &mut Vec<String>) -> String {
        match ty {
            Ty::Tuple(fields) => {
                let parts: Vec<String> = fields.iter().map(|f| self.structural_key_in(f, seen)).collect();
                format!("({})", parts.join(","))
            }
            Ty::Array(inner, n) => format!("[{},{}]", self.structural_key_in(inner, seen), n),
            Ty::Option(inner) => format!("Option<{}>", self.structural_key_in(inner, seen)),
            Ty::Result(ok, err) => {
                format!("Result<{},{}>", self.structural_key_in(ok, seen), self.structural_key_in(err, seen))
            }
            Ty::List(inner) => format!("List<{}>", self.structural_key_in(inner, seen)),
            Ty::Map(k, v) => {
                format!("Map<{},{}>", self.structural_key_in(k, seen), self.structural_key_in(v, seen))
            }
            Ty::Heap(inner) => format!("Heap<{}>", self.structural_key_in(inner, seen)),
            Ty::Range(inner, inclusive) => {
                format!("Range<{},{inclusive}>", self.structural_key_in(inner, seen))
            }
            Ty::RangeIter(inner, inclusive) => {
                format!("RangeIter<{},{inclusive}>", self.structural_key_in(inner, seen))
            }
            Ty::ListIntoIter(inner) => format!("ListIntoIter<{}>", self.structural_key_in(inner, seen)),
            Ty::ArrayIntoIter(inner, n) => format!("ArrayIntoIter<{},{}>", self.structural_key_in(inner, seen), n),
            Ty::MapIter(k, v) => {
                format!("MapIter<{},{}>", self.structural_key_in(k, seen), self.structural_key_in(v, seen))
            }
            Ty::MapIntoIter(k, v) => {
                format!("MapIntoIter<{},{}>", self.structural_key_in(k, seen), self.structural_key_in(v, seen))
            }
            Ty::Ref { mutable, inner } => {
                format!("&{}{}", if *mutable { "mut " } else { "" }, self.structural_key_in(inner, seen))
            }
            Ty::Struct(a) => {
                let mut parts = Vec::new();
                for field in &self.program.structs[a.id].fields {
                    parts.push(self.structural_key_in(&field.ty, seen));
                }
                format!("struct({})", parts.join(","))
            }
            Ty::Enum(a) => {
                let mut variants = Vec::new();
                for variant in &self.program.enums[a.id].variants {
                    let mut fields = Vec::new();
                    for field in &variant.fields {
                        fields.push(self.structural_key_in(&field.ty, seen));
                    }
                    variants.push(fields.join(","));
                }
                format!("enum({})", variants.join("|"))
            }
            other => format!("{other:?}"),
        }
    }

    pub(super) fn drop_glue_body(&mut self, value: FunctionValue<'ctx>, ty: &Ty) -> GenResult<()> {
        self.start_glue(value)?;
        let ptr = value.get_nth_param(0).expect("the drop glue takes one pointer").into_pointer_value();
        self.drop_value(ptr, ty)?;
        self.finish_glue()?;
        Ok(())
    }

    pub(super) fn clone_glue_body(&mut self, value: FunctionValue<'ctx>, ty: &Ty) -> GenResult<()> {
        self.start_glue(value)?;
        let dst = value.get_nth_param(0).expect("clone takes a destination").into_pointer_value();
        let src = value.get_nth_param(1).expect("clone takes a source").into_pointer_value();
        self.clone_value(dst, src, ty)?;
        self.finish_glue()?;
        Ok(())
    }

    pub(super) fn start_glue(&mut self, value: FunctionValue<'ctx>) -> GenResult<()> {
        let saved_position = self.builder.get_insert_block();
        let saved_entry = self.entry_builder.get_insert_block();
        self.context = Some(SavedContext {
            position: saved_position,
            entry: saved_entry,
            function: self.function,
            locals: self.locals,
            ret: self.ret,
            scopes: std::mem::take(&mut self.scopes),
            temps: std::mem::take(&mut self.temps),
            flags: std::mem::take(&mut self.flags),
            loops: std::mem::take(&mut self.loops),
            debug_scope: self.debug.as_ref().and_then(|db| db.scope),
            debug_file: self.debug.as_ref().and_then(|db| db.file),
            debug_entry: self.debug_entry,
            dead_locals: std::mem::take(&mut self.dead_locals),
            bounds_facts: std::mem::take(&mut self.bounds_facts),
            return_env: self.return_env.take(),
            return_consume: self.return_consume.take(),
        });
        let entry = self.cx.append_basic_block(value, "entry");
        let body = self.cx.append_basic_block(value, "body");
        self.entry_builder.position_at_end(entry);
        self.builder.position_at_end(body);
        // Glue is compiler-generated: it has no source locations and no debug variables.
        if let Some(db) = self.debug.as_mut() {
            db.scope = None;
            db.file = None;
        }
        self.debug_entry = None;
        self.builder.unset_current_debug_location();
        self.function = Some(value);
        self.locals = &[];
        self.ret = &Ty::Unit;
        self.scopes = vec![Vec::new()];
        self.temps = vec![Vec::new()];
        self.loops.clear();
        self.flags.clear();
        self.return_consume = None;
        Ok(())
    }

    pub(super) fn finish_glue(&mut self) -> GenResult<()> {
        let value = self.current_fn();
        self.builder.build_return(None).map_err(llvm_err)?;
        let entry = value.get_first_basic_block().expect("glue has an entry block");
        self.entry_builder
            .build_unconditional_branch(entry.get_next_basic_block().expect("glue has a body"))
            .map_err(llvm_err)?;
        let ctx = self.context.take().expect("inside a glue body");
        self.function = ctx.function;
        self.locals = ctx.locals;
        self.ret = ctx.ret;
        self.scopes = ctx.scopes;
        self.temps = ctx.temps;
        self.flags = ctx.flags;
        self.loops = ctx.loops;
        if let Some(db) = self.debug.as_mut() {
            db.scope = ctx.debug_scope;
            db.file = ctx.debug_file;
        }
        self.debug_entry = ctx.debug_entry;
        self.dead_locals = ctx.dead_locals;
        self.bounds_facts = ctx.bounds_facts;
        self.return_env = ctx.return_env;
        self.return_consume = ctx.return_consume;
        if let Some(position) = ctx.position {
            self.builder.position_at_end(position);
        }
        if let Some(entry) = ctx.entry {
            self.entry_builder.position_at_end(entry);
        }
        Ok(())
    }

    /// Drops the fully initialized value at `addr` (DR2). Does not look at drop flags: the caller knows the value
    /// is initialized, or has already checked the flag for it.
    pub(super) fn drop_value(&mut self, addr: PointerValue<'ctx>, ty: &Ty) -> GenResult<()> {
        let ptr_ty = self.cx.ptr_type(AddressSpace::default());
        match ty {
            Ty::Unit | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Float(_) | Ty::Str | Ty::Never | Ty::Error => Ok(()),
            // The endpoints of a range are integers, so a range owns nothing.
            Ty::Range(..) => Ok(()),
            Ty::Param(_) => Ok(()),
            // A reference, a slice, or a raw pointer is a view of someone else's storage: dropping it does
            // nothing. The `Chars` iterator is a view of a `str` too, and a `MapIter` only borrows its map.
            Ty::Ref { .. } | Ty::Slice(_) | Ty::CPtr(_) | Ty::Chars | Ty::MapIter(..) | Ty::RangeIter(..) => Ok(()),
            Ty::ListIntoIter(elem) => {
                let elem = (**elem).clone();
                self.drop_list_into_iter(addr, &elem)
            }
            Ty::MapIntoIter(key, value) => {
                let (key, value) = ((**key).clone(), (**value).clone());
                self.drop_map_into_iter(addr, &key, &value)
            }
            Ty::ArrayIntoIter(elem, n) => {
                let (elem, n) = ((**elem).clone(), *n);
                if n == 0 || !self.needs_drop(&elem) || self.llvm_ty(&elem).is_none() {
                    return Ok(());
                }
                let index_field = self
                    .builder
                    .build_struct_gep(self.iter_array_ty(&elem, n), addr, 1, "iter.index")
                    .map_err(llvm_err)?;
                let start = self.builder.build_load(self.usize_ty, index_field, "").map_err(llvm_err)?.into_int_value();
                let slots = self
                    .builder
                    .build_struct_gep(self.iter_array_ty(&elem, n), addr, 0, "iter.array")
                    .map_err(llvm_err)?;
                let elem_t = self.llvm_ty(&elem).expect("checked above");
                let array_t = elem_t.array_type(n as u32);
                let index = self.alloca(self.usize_ty.into(), "array.iter.drop.i")?;
                self.builder.build_store(index, start).map_err(llvm_err)?;
                let header = self.new_block("array.iter.drop.cond");
                let body = self.new_block("array.iter.drop.body");
                let exit = self.new_block("array.iter.drop.end");
                self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                self.builder.position_at_end(header);
                let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
                let go = self
                    .builder
                    .build_int_compare(IntPredicate::ULT, i, self.usize_ty.const_int(n, false), "")
                    .map_err(llvm_err)?;
                self.builder.build_conditional_branch(go, body, exit).map_err(llvm_err)?;
                self.builder.position_at_end(body);
                let zero = self.usize_ty.const_zero();
                // SAFETY (LLVM): index <= i < n.
                let p =
                    unsafe { self.builder.build_in_bounds_gep(array_t, slots, &[zero, i], "") }.map_err(llvm_err)?;
                self.drop_value(p, &elem)?;
                let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                self.builder.build_store(index, next).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                self.builder.position_at_end(exit);
                Ok(())
            }
            // A closure owns its capture record; its destructor drops the moved-in captures and frees it.
            Ty::Fn { .. } => {
                let v =
                    self.builder.build_load(self.closure_ty, addr, "closure").map_err(llvm_err)?.into_struct_value();
                let env = self.builder.build_extract_value(v, 1, "").map_err(llvm_err)?.into_pointer_value();
                let drop = self.builder.build_extract_value(v, 2, "").map_err(llvm_err)?.into_pointer_value();
                let drop_ty = self.cx.void_type().fn_type(&[ptr_ty.into()], false);
                self.builder.build_indirect_call(drop_ty, drop, &[env.into()], "").map_err(llvm_err)?;
                Ok(())
            }
            Ty::CString => {
                let p = self.builder.build_load(ptr_ty, addr, "cstring").map_err(llvm_err)?.into_pointer_value();
                self.builder.build_call(self.rt.free, &[p.into()], "").map_err(llvm_err)?;
                Ok(())
            }
            Ty::String => {
                let s = self.builder.build_load(self.string_ty, addr, "s").map_err(llvm_err)?.into_struct_value();
                let p = self.builder.build_extract_value(s, 0, "").map_err(llvm_err)?;
                let cap = self.builder.build_extract_value(s, 2, "").map_err(llvm_err)?;
                self.builder.build_call(self.rt.string_free, &[p.into(), cap.into()], "").map_err(llvm_err)?;
                Ok(())
            }
            Ty::Heap(inner) => {
                let inner = (**inner).clone();
                let p = self.builder.build_load(ptr_ty, addr, "heap").map_err(llvm_err)?.into_pointer_value();
                let is_null = self.builder.build_is_null(p, "").map_err(llvm_err)?;
                let alive = self.builder.build_not(is_null, "").map_err(llvm_err)?;
                self.if_then(alive, "heap.drop", move |cg| {
                    if cg.needs_drop(&inner) {
                        let glue = cg.drop_glue(&inner);
                        cg.builder.build_call(glue, &[p.into()], "").map_err(llvm_err)?;
                    }
                    cg.builder.build_call(cg.rt.free, &[p.into()], "").map_err(llvm_err)?;
                    Ok(())
                })
            }
            Ty::Struct(a) => {
                if let Some(m) = self.program.structs[a.id].drop_method {
                    self.builder.build_call(self.functions[m.0], &[addr.into()], "").map_err(llvm_err)?;
                }
                let st = self.llvm_ty(ty).expect("a struct has a type").into_struct_type();
                for (i, fty) in self.field_tys(ty).iter().enumerate() {
                    if !self.needs_drop(fty) {
                        continue;
                    }
                    let f = self.builder.build_struct_gep(st, addr, i as u32, "").map_err(llvm_err)?;
                    self.drop_value(f, fty)?;
                }
                Ok(())
            }
            Ty::Tuple(ts) => {
                let ts = ts.clone();
                let st = self.llvm_ty(ty).expect("a tuple has a type").into_struct_type();
                for (i, fty) in ts.iter().enumerate() {
                    if !self.needs_drop(fty) {
                        continue;
                    }
                    let f = self.builder.build_struct_gep(st, addr, i as u32, "").map_err(llvm_err)?;
                    self.drop_value(f, fty)?;
                }
                Ok(())
            }
            Ty::Enum(a) => {
                let id = a.id;
                if let Some(m) = self.program.enums[id].drop_method {
                    self.builder.build_call(self.functions[m.0], &[addr.into()], "").map_err(llvm_err)?;
                }
                self.drop_enum_fields(addr, id)
            }
            Ty::Option(inner) => {
                let inner = (**inner).clone();
                let ot = self.option_ty(&inner);
                let tag_ptr = self.builder.build_struct_gep(ot, addr, 0, "is_some").map_err(llvm_err)?;
                let tag = self.builder.build_load(self.cx.bool_type(), tag_ptr, "").map_err(llvm_err)?.into_int_value();
                self.if_then(tag, "option.drop", move |cg| {
                    if cg.llvm_ty(&inner).is_some() && cg.needs_drop(&inner) {
                        let p = cg.builder.build_struct_gep(ot, addr, 1, "").map_err(llvm_err)?;
                        cg.drop_value(p, &inner)?;
                    }
                    Ok(())
                })
            }
            Ty::Result(ok, err) => {
                let (ok, err) = ((**ok).clone(), (**err).clone());
                let rt = self.result_ty(&ok, &err);
                let tag_ptr = self.builder.build_struct_gep(rt, addr, 0, "is_ok").map_err(llvm_err)?;
                let tag = self.builder.build_load(self.cx.bool_type(), tag_ptr, "").map_err(llvm_err)?.into_int_value();
                self.if_then(tag, "result.drop.ok", move |cg| {
                    if cg.needs_drop(&ok) {
                        let p = cg.builder.build_struct_gep(rt, addr, 1, "").map_err(llvm_err)?;
                        cg.drop_value(p, &ok)?;
                    }
                    Ok(())
                })?;
                let tag_ptr = self.builder.build_struct_gep(rt, addr, 0, "is_ok").map_err(llvm_err)?;
                let tag = self.builder.build_load(self.cx.bool_type(), tag_ptr, "").map_err(llvm_err)?.into_int_value();
                let is_err = self.builder.build_not(tag, "").map_err(llvm_err)?;
                self.if_then(is_err, "result.drop.err", move |cg| {
                    if cg.needs_drop(&err) {
                        let p = cg.builder.build_struct_gep(rt, addr, 2, "").map_err(llvm_err)?;
                        cg.drop_value(p, &err)?;
                    }
                    Ok(())
                })
            }
            Ty::List(elem) => {
                let elem = (**elem).clone();
                self.drop_sequence(addr, &elem, "list")
            }
            Ty::Map(key, value) => {
                let (key, value) = ((**key).clone(), (**value).clone());
                self.drop_map(addr, &key, &value)
            }
            Ty::Array(elem, n) => {
                let n = *n;
                if n == 0 || !self.needs_drop(elem) {
                    return Ok(());
                }
                let elem = (**elem).clone();
                let array_t = self.llvm_ty(ty).expect("an array has a type");
                let zero = self.usize_ty.const_zero();
                let index = self.alloca(self.usize_ty.into(), "drop.i")?;
                self.builder.build_store(index, self.usize_ty.const_zero()).map_err(llvm_err)?;
                let header = self.new_block("array.drop.cond");
                let body = self.new_block("array.drop.body");
                let exit = self.new_block("array.drop.end");
                self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                self.builder.position_at_end(header);
                let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
                let go = self
                    .builder
                    .build_int_compare(IntPredicate::ULT, i, self.usize_ty.const_int(n, false), "")
                    .map_err(llvm_err)?;
                self.builder.build_conditional_branch(go, body, exit).map_err(llvm_err)?;
                self.builder.position_at_end(body);
                // SAFETY (LLVM): i < n. The first index walks the array value from the pointer; the second
                // selects the element.
                let p = unsafe { self.builder.build_in_bounds_gep(array_t, addr, &[zero, i], "") }.map_err(llvm_err)?;
                self.drop_value(p, &elem)?;
                let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                self.builder.build_store(index, next).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                self.builder.position_at_end(exit);
                Ok(())
            }
        }
    }

    /// Switches on the tag of the enum at `addr` and drops the active variant's fields, in field order.
    pub(super) fn drop_enum_fields(&mut self, addr: PointerValue<'ctx>, id: usize) -> GenResult<()> {
        let et = self.enum_ty(id);
        let tag_ptr = self.builder.build_struct_gep(et, addr, 0, "tag").map_err(llvm_err)?;
        let tag = self.builder.build_load(self.cx.i32_type(), tag_ptr, "").map_err(llvm_err)?.into_int_value();
        let end = self.new_block("enum.drop.end");
        let mut cases = Vec::new();
        for (vi, v) in self.program.enums[id].variants.iter().enumerate() {
            if v.fields.is_empty() {
                continue;
            }
            cases.push((vi, self.new_block("enum.drop")));
        }
        let switch_cases: Vec<(IntValue<'ctx>, BasicBlock<'ctx>)> =
            cases.iter().map(|(vi, bb)| (self.cx.i32_type().const_int(*vi as u64, false), *bb)).collect();
        self.builder.build_switch(tag, end, &switch_cases).map_err(llvm_err)?;
        for (vi, case) in cases {
            self.builder.position_at_end(case);
            let payload = self.builder.build_struct_gep(et, addr, 1, "").map_err(llvm_err)?;
            let vt = self.variant_ty(id, vi);
            for (fi, fty) in self.variant_tys(id, vi).iter().enumerate() {
                if !self.needs_drop(fty) {
                    continue;
                }
                let f = self.builder.build_struct_gep(vt, payload, fi as u32, "").map_err(llvm_err)?;
                self.drop_value(f, fty)?;
            }
            self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        }
        self.builder.position_at_end(end);
        Ok(())
    }

    /// Drops the elements of a list or map at `addr` in index order, then frees its buffer. A list and a map
    /// have the layout `{ ptr, len, cap }`, like a `String`.
    fn drop_sequence(&mut self, addr: PointerValue<'ctx>, elem: &Ty, name: &str) -> GenResult<()> {
        let ptr_ty = self.cx.ptr_type(AddressSpace::default());
        let ptr_field = self.builder.build_struct_gep(self.string_ty, addr, 0, "").map_err(llvm_err)?;
        let len_field = self.builder.build_struct_gep(self.string_ty, addr, 1, "").map_err(llvm_err)?;
        let base = self.builder.build_load(ptr_ty, ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        let len = self.builder.build_load(self.usize_ty, len_field, "").map_err(llvm_err)?.into_int_value();
        if self.needs_drop(elem) && self.llvm_ty(elem).is_some() {
            let elem = elem.clone();
            let elem_t = self.llvm_ty(&elem).expect("checked above");
            let index = self.alloca(self.usize_ty.into(), "drop.i")?;
            self.builder.build_store(index, self.usize_ty.const_zero()).map_err(llvm_err)?;
            let header = self.new_block(&format!("{name}.drop.cond"));
            let body = self.new_block(&format!("{name}.drop.body"));
            let exit = self.new_block(&format!("{name}.drop.end"));
            self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
            self.builder.position_at_end(header);
            let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
            let go = self.builder.build_int_compare(IntPredicate::ULT, i, len, "").map_err(llvm_err)?;
            self.builder.build_conditional_branch(go, body, exit).map_err(llvm_err)?;
            self.builder.position_at_end(body);
            // SAFETY (LLVM): i < len.
            let p = unsafe { self.builder.build_in_bounds_gep(elem_t, base, &[i], "") }.map_err(llvm_err)?;
            self.drop_value(p, &elem)?;
            let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
            self.builder.build_store(index, next).map_err(llvm_err)?;
            self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
            self.builder.position_at_end(exit);
        }
        self.builder.build_call(self.rt.free, &[base.into()], "").map_err(llvm_err)?;
        Ok(())
    }

    /// Deep-copies the value at `src` into `dst` (the compiler-derived `clone`).
    pub(super) fn clone_value(&mut self, dst: PointerValue<'ctx>, src: PointerValue<'ctx>, ty: &Ty) -> GenResult<()> {
        if self.llvm_ty(ty).is_none() {
            return Ok(());
        }
        if self.is_copy(ty) {
            return self.copy_bits(dst, src, ty);
        }
        let ptr_ty = self.cx.ptr_type(AddressSpace::default());
        match ty {
            Ty::String => {
                self.builder.build_call(self.rt.string_clone, &[dst.into(), src.into()], "").map_err(llvm_err)?;
                Ok(())
            }
            Ty::CString => {
                let p = self.builder.build_load(ptr_ty, src, "cstring").map_err(llvm_err)?.into_pointer_value();
                let call = self.builder.build_call(self.rt.cstring_clone, &[p.into()], "").map_err(llvm_err)?;
                let copy = call.try_as_basic_value().basic().expect("`cstring_clone` returns a pointer");
                self.builder.build_store(dst, copy).map_err(llvm_err)?;
                Ok(())
            }
            Ty::Heap(inner) => {
                let inner = (**inner).clone();
                let p = self.builder.build_load(ptr_ty, src, "heap").map_err(llvm_err)?.into_pointer_value();
                let size = self.llvm_ty(&inner).map(|t| self.target_data.get_abi_size(&t)).unwrap_or(0).max(1);
                let size = self.usize_ty.const_int(size, false);
                let call = self.builder.build_call(self.rt.alloc, &[size.into()], "heap").map_err(llvm_err)?;
                let newp = call.try_as_basic_value().basic().expect("alloc returns a pointer").into_pointer_value();
                self.clone_value(newp, p, &inner)?;
                self.builder.build_store(dst, newp).map_err(llvm_err)?;
                Ok(())
            }
            Ty::Struct(_) | Ty::Tuple(_) => {
                let st = self.llvm_ty(ty).expect("an aggregate has a type").into_struct_type();
                for (i, fty) in self.field_tys(ty).iter().enumerate() {
                    if self.llvm_ty(fty).is_none() {
                        continue;
                    }
                    let d = self.builder.build_struct_gep(st, dst, i as u32, "").map_err(llvm_err)?;
                    let s = self.builder.build_struct_gep(st, src, i as u32, "").map_err(llvm_err)?;
                    self.clone_value(d, s, fty)?;
                }
                Ok(())
            }
            Ty::Enum(a) => self.clone_enum_fields(dst, src, a.id),
            Ty::Option(inner) => {
                let inner = (**inner).clone();
                let ot = self.option_ty(&inner);
                let src_tag = self.builder.build_struct_gep(ot, src, 0, "").map_err(llvm_err)?;
                let dst_tag = self.builder.build_struct_gep(ot, dst, 0, "").map_err(llvm_err)?;
                let tag = self
                    .builder
                    .build_load(self.cx.bool_type(), src_tag, "is_some")
                    .map_err(llvm_err)?
                    .into_int_value();
                self.builder.build_store(dst_tag, tag).map_err(llvm_err)?;
                if self.llvm_ty(&inner).is_some() {
                    self.if_then(tag, "option.clone", move |cg| {
                        let s = cg.builder.build_struct_gep(ot, src, 1, "").map_err(llvm_err)?;
                        let d = cg.builder.build_struct_gep(ot, dst, 1, "").map_err(llvm_err)?;
                        cg.clone_value(d, s, &inner)
                    })?;
                }
                Ok(())
            }
            Ty::Result(ok, err) => {
                let (ok, err) = ((**ok).clone(), (**err).clone());
                let rt = self.result_ty(&ok, &err);
                let src_tag_ptr = self.builder.build_struct_gep(rt, src, 0, "").map_err(llvm_err)?;
                let dst_tag_ptr = self.builder.build_struct_gep(rt, dst, 0, "").map_err(llvm_err)?;
                let tag = self
                    .builder
                    .build_load(self.cx.bool_type(), src_tag_ptr, "is_ok")
                    .map_err(llvm_err)?
                    .into_int_value();
                self.builder.build_store(dst_tag_ptr, tag).map_err(llvm_err)?;
                if self.llvm_ty(&ok).is_some() && self.llvm_ty(&err).is_some() {
                    let ok_bb = self.new_block("result.clone.ok");
                    let err_bb = self.new_block("result.clone.err");
                    let end = self.new_block("result.clone.end");
                    self.builder.build_conditional_branch(tag, ok_bb, err_bb).map_err(llvm_err)?;
                    self.builder.position_at_end(ok_bb);
                    let s = self.builder.build_struct_gep(rt, src, 1, "").map_err(llvm_err)?;
                    let d = self.builder.build_struct_gep(rt, dst, 1, "").map_err(llvm_err)?;
                    self.clone_value(d, s, &ok)?;
                    self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                    self.builder.position_at_end(err_bb);
                    let s = self.builder.build_struct_gep(rt, src, 2, "").map_err(llvm_err)?;
                    let d = self.builder.build_struct_gep(rt, dst, 2, "").map_err(llvm_err)?;
                    self.clone_value(d, s, &err)?;
                    self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                    self.builder.position_at_end(end);
                } else if self.llvm_ty(&ok).is_some() {
                    self.if_then(tag, "result.clone.ok", move |cg| {
                        let s = cg.builder.build_struct_gep(rt, src, 1, "").map_err(llvm_err)?;
                        let d = cg.builder.build_struct_gep(rt, dst, 1, "").map_err(llvm_err)?;
                        cg.clone_value(d, s, &ok)
                    })?;
                } else if self.llvm_ty(&err).is_some() {
                    let is_err = self.builder.build_not(tag, "").map_err(llvm_err)?;
                    self.if_then(is_err, "result.clone.err", move |cg| {
                        let s = cg.builder.build_struct_gep(rt, src, 2, "").map_err(llvm_err)?;
                        let d = cg.builder.build_struct_gep(rt, dst, 2, "").map_err(llvm_err)?;
                        cg.clone_value(d, s, &err)
                    })?;
                }
                Ok(())
            }
            Ty::List(elem) => {
                let elem = (**elem).clone();
                self.clone_sequence(dst, src, &elem)
            }
            Ty::Map(key, value) => {
                let (key, value) = ((**key).clone(), (**value).clone());
                self.clone_map(dst, src, &key, &value)
            }
            Ty::Chars => self.copy_bits(dst, src, ty),
            // A closure is not `Clone` (CL10); this is unreachable in a checked program.
            Ty::Fn { .. } => Ok(()),
            Ty::Array(elem, n) => {
                let n = *n;
                let elem = (**elem).clone();
                let array_t = self.llvm_ty(ty).expect("an array has a type");
                let zero = self.usize_ty.const_zero();
                let index = self.alloca(self.usize_ty.into(), "clone.i")?;
                self.builder.build_store(index, self.usize_ty.const_zero()).map_err(llvm_err)?;
                let header = self.new_block("array.clone.cond");
                let body = self.new_block("array.clone.body");
                let exit = self.new_block("array.clone.end");
                self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                self.builder.position_at_end(header);
                let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
                let go = self
                    .builder
                    .build_int_compare(IntPredicate::ULT, i, self.usize_ty.const_int(n, false), "")
                    .map_err(llvm_err)?;
                self.builder.build_conditional_branch(go, body, exit).map_err(llvm_err)?;
                self.builder.position_at_end(body);
                // SAFETY (LLVM): i < n. The first index walks the array value from the pointer.
                let s = unsafe { self.builder.build_in_bounds_gep(array_t, src, &[zero, i], "") }.map_err(llvm_err)?;
                let d = unsafe { self.builder.build_in_bounds_gep(array_t, dst, &[zero, i], "") }.map_err(llvm_err)?;
                self.clone_value(d, s, &elem)?;
                let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                self.builder.build_store(index, next).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                self.builder.position_at_end(exit);
                Ok(())
            }
            _ => self.copy_bits(dst, src, ty),
        }
    }

    /// Deep-copies a list or map buffer: allocates `cap` elements and clones the first `len`.
    fn clone_sequence(&mut self, dst: PointerValue<'ctx>, src: PointerValue<'ctx>, elem: &Ty) -> GenResult<()> {
        let ptr_ty = self.cx.ptr_type(AddressSpace::default());
        let usize_ty = self.usize_ty;
        let field = |cg: &mut Self, base: PointerValue<'ctx>, i: u32| -> GenResult<PointerValue<'ctx>> {
            cg.builder.build_struct_gep(cg.string_ty, base, i, "").map_err(llvm_err)
        };
        let src_ptr_field = field(self, src, 0)?;
        let src_len_field = field(self, src, 1)?;
        let src_cap_field = field(self, src, 2)?;
        let base = self.builder.build_load(ptr_ty, src_ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        let len = self.builder.build_load(usize_ty, src_len_field, "").map_err(llvm_err)?.into_int_value();
        let cap = self.builder.build_load(usize_ty, src_cap_field, "").map_err(llvm_err)?.into_int_value();
        let size = self.llvm_ty(elem).map(|t| self.target_data.get_abi_size(&t)).unwrap_or(0).max(1);
        let bytes = self.builder.build_int_mul(cap, usize_ty.const_int(size, false), "").map_err(llvm_err)?;
        let at_least_one = self
            .builder
            .build_select(
                self.builder.build_int_compare(IntPredicate::EQ, bytes, usize_ty.const_zero(), "").map_err(llvm_err)?,
                usize_ty.const_int(1, false),
                bytes,
                "",
            )
            .map_err(llvm_err)?
            .into_int_value();
        let call = self.builder.build_call(self.rt.alloc, &[at_least_one.into()], "copy").map_err(llvm_err)?;
        let new_base = call.try_as_basic_value().basic().expect("alloc returns a pointer").into_pointer_value();
        if let Some(elem_t) = self.llvm_ty(elem) {
            let index = self.alloca(usize_ty.into(), "clone.i")?;
            self.builder.build_store(index, usize_ty.const_zero()).map_err(llvm_err)?;
            let header = self.new_block("seq.clone.cond");
            let body = self.new_block("seq.clone.body");
            let exit = self.new_block("seq.clone.end");
            self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
            self.builder.position_at_end(header);
            let i = self.builder.build_load(usize_ty, index, "").map_err(llvm_err)?.into_int_value();
            let go = self.builder.build_int_compare(IntPredicate::ULT, i, len, "").map_err(llvm_err)?;
            self.builder.build_conditional_branch(go, body, exit).map_err(llvm_err)?;
            self.builder.position_at_end(body);
            // SAFETY (LLVM): i < len <= cap.
            let s = unsafe { self.builder.build_in_bounds_gep(elem_t, base, &[i], "") }.map_err(llvm_err)?;
            let d = unsafe { self.builder.build_in_bounds_gep(elem_t, new_base, &[i], "") }.map_err(llvm_err)?;
            self.clone_value(d, s, elem)?;
            let next = self.builder.build_int_add(i, usize_ty.const_int(1, false), "").map_err(llvm_err)?;
            self.builder.build_store(index, next).map_err(llvm_err)?;
            self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
            self.builder.position_at_end(exit);
        }
        let dst_ptr_field = field(self, dst, 0)?;
        let dst_len_field = field(self, dst, 1)?;
        let dst_cap_field = field(self, dst, 2)?;
        self.builder.build_store(dst_ptr_field, new_base).map_err(llvm_err)?;
        self.builder.build_store(dst_len_field, len).map_err(llvm_err)?;
        self.builder.build_store(dst_cap_field, cap).map_err(llvm_err)?;
        Ok(())
    }

    pub(super) fn clone_enum_fields(
        &mut self,
        dst: PointerValue<'ctx>,
        src: PointerValue<'ctx>,
        id: usize,
    ) -> GenResult<()> {
        let et = self.enum_ty(id);
        let src_tag_ptr = self.builder.build_struct_gep(et, src, 0, "").map_err(llvm_err)?;
        let dst_tag_ptr = self.builder.build_struct_gep(et, dst, 0, "").map_err(llvm_err)?;
        let tag = self.builder.build_load(self.cx.i32_type(), src_tag_ptr, "tag").map_err(llvm_err)?.into_int_value();
        self.builder.build_store(dst_tag_ptr, tag).map_err(llvm_err)?;
        let end = self.new_block("enum.clone.end");
        let mut cases = Vec::new();
        for (vi, v) in self.program.enums[id].variants.iter().enumerate() {
            if v.fields.is_empty() {
                continue;
            }
            cases.push((vi, self.new_block("enum.clone")));
        }
        let switch_cases: Vec<(IntValue<'ctx>, BasicBlock<'ctx>)> =
            cases.iter().map(|(vi, bb)| (self.cx.i32_type().const_int(*vi as u64, false), *bb)).collect();
        self.builder.build_switch(tag, end, &switch_cases).map_err(llvm_err)?;
        for (vi, case) in cases {
            self.builder.position_at_end(case);
            let sp = self.builder.build_struct_gep(et, src, 1, "").map_err(llvm_err)?;
            let dp = self.builder.build_struct_gep(et, dst, 1, "").map_err(llvm_err)?;
            let vt = self.variant_ty(id, vi);
            for (fi, fty) in self.variant_tys(id, vi).iter().enumerate() {
                if self.llvm_ty(fty).is_none() {
                    continue;
                }
                let s = self.builder.build_struct_gep(vt, sp, fi as u32, "").map_err(llvm_err)?;
                let d = self.builder.build_struct_gep(vt, dp, fi as u32, "").map_err(llvm_err)?;
                self.clone_value(d, s, fty)?;
            }
            self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        }
        self.builder.position_at_end(end);
        Ok(())
    }

    /// A bitwise copy of a value of `ty`.
    pub(super) fn copy_bits(&mut self, dst: PointerValue<'ctx>, src: PointerValue<'ctx>, ty: &Ty) -> GenResult<()> {
        let Some(t) = self.llvm_ty(ty) else { return Ok(()) };
        let size = self.target_data.get_abi_size(&t);
        if size == 0 {
            return Ok(());
        }
        let size = self.usize_ty.const_int(size, false);
        self.builder.build_memcpy(dst, 1, src, 1, size).map_err(llvm_err)?;
        Ok(())
    }
}
