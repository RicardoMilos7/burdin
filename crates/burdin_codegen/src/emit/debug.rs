//! Source-level debug information (`-g`): DWARF on Unix and the LLVM debug metadata the COFF backend turns into
//! CodeView on Windows.
//!
//! Each Burdin function becomes a `DW_TAG_subprogram` with its parameters and locals as `DW_TAG_formal_parameter`
//! and `DW_TAG_variable`, and every expression sets the builder's current debug location from its span, so a
//! debugger can stop on a Burdin line and read a local. Types are modeled for the built-in scalars, references,
//! arrays, tuples, structs, `Option`, `Result`, `str`, `String`, and the containers; an enum with a payload is
//! described as an unnamed value, because its payload is a sized byte blob in the generated layout.
//!
//! The metadata is attached to the module by [`DebugInfoBuilder::finalize`], which code generation calls after the
//! last function; the module carries the `Debug Info Version` flag LLVM requires.

use super::Codegen;
use burdin_diag::{FileId, Span};
use burdin_tir::{self as tir, Ty};
use inkwell::basic_block::BasicBlock;
use inkwell::context::Context;
use inkwell::debug_info::{
    AsDIScope, DICompileUnit, DIFile, DIFlagsConstants, DILocalVariable, DILocation, DIScope, DISubprogram, DIType,
    DWARFEmissionKind, DWARFSourceLanguage, DebugInfoBuilder,
};
use inkwell::module::{FlagBehavior, Module};
use inkwell::values::{AsValueRef, FunctionValue, PointerValue};
use std::collections::{HashMap, HashSet};

/// DWARF type encodings used for the built-in scalars.
const DW_ATE_BOOLEAN: u32 = 0x02;
const DW_ATE_FLOAT: u32 = 0x04;
const DW_ATE_SIGNED: u32 = 0x05;
const DW_ATE_UNSIGNED: u32 = 0x07;
const DW_ATE_UTF: u32 = 0x10;
/// `DW_OP_deref`: the variable's value is in memory at the address the value names.
const DW_OP_DEREF: i64 = 0x06;

/// The module's debug metadata: the builder, the compile unit, and the current function's scope.
pub(crate) struct DebugInfo<'ctx> {
    pub(crate) builder: DebugInfoBuilder<'ctx>,
    pub(crate) unit: DICompileUnit<'ctx>,
    /// One `DIFile` per source file, created on first use.
    files: HashMap<u32, DIFile<'ctx>>,
    /// Debug types by `Ty` spelling.
    types: HashMap<String, DIType<'ctx>>,
    /// Types being built, so a type that contains itself through a pointer terminates.
    building: HashSet<String>,
    /// The subprogram and file of the function being generated.
    pub(crate) scope: Option<DISubprogram<'ctx>>,
    pub(crate) file: Option<DIFile<'ctx>>,
    /// The debug location of the function's declaration, reused for the location records that carry no
    /// position of their own (a kill or a restore between statements).
    pub(crate) fn_location: Option<DILocation<'ctx>>,
    /// The debug variable of each local of the function being generated, indexed by local id.
    pub(crate) vars: Vec<Option<DILocalVariable<'ctx>>>,
    /// The whole-local kill sites of the function being generated (`spec/debug.md`, DBG4): the local and the
    /// span of the statement that moves it out.
    pub(crate) kills: Vec<(burdin_tir::LocalId, Span)>,
}

impl<'ctx> DebugInfo<'ctx> {
    pub(crate) fn new(module: &Module<'ctx>, is_optimized: bool, first_file: Option<(String, String)>) -> Self {
        let (filename, directory) = first_file.unwrap_or_else(|| ("burdin".to_string(), ".".to_string()));
        let (builder, unit) = module.create_debug_info_builder(
            true,
            DWARFSourceLanguage::C,
            &filename,
            &directory,
            "burdin",
            is_optimized,
            "",
            0,
            "",
            DWARFEmissionKind::Full,
            0,
            false,
            false,
            "",
            "",
        );
        // LLVM refuses to verify a module whose debug metadata lacks this flag (the value is the metadata
        // version, currently 3).
        let version = module.get_context().i32_type().const_int(3, false);
        module.add_basic_value_flag("Debug Info Version", FlagBehavior::Warning, version);
        DebugInfo {
            builder,
            unit,
            files: HashMap::new(),
            types: HashMap::new(),
            building: HashSet::new(),
            scope: None,
            file: None,
            fn_location: None,
            vars: Vec::new(),
            kills: Vec::new(),
        }
    }

    /// The `DIFile` for a source file.
    pub(crate) fn file(&mut self, id: FileId, path: &str) -> DIFile<'ctx> {
        if let Some(file) = self.files.get(&id.0) {
            return *file;
        }
        let (directory, name) = match path.rfind(['/', '\\']) {
            Some(i) => (path[..i].to_string(), path[i + 1..].to_string()),
            None => (".".to_string(), path.to_string()),
        };
        let file = self.builder.create_file(&name, &directory);
        self.files.insert(id.0, file);
        file
    }

    /// Ends the location of `var` from the end of `block`: a `dbg.value` naming an undefined value is LLVM's
    /// kill, and a debugger shows `<optimized out>` for the variable afterwards (`spec/debug.md`, DBG4).
    pub(crate) fn kill(
        &self,
        value: &inkwell::values::BasicValueEnum<'ctx>,
        var: DILocalVariable<'ctx>,
        location: DILocation<'ctx>,
        block: BasicBlock<'ctx>,
    ) {
        let expr = self.builder.create_expression(vec![]);
        // SAFETY: as `declare`, with `value` a constant of this module.
        unsafe {
            inkwell::llvm_sys::debuginfo::LLVMDIBuilderInsertDbgValueRecordAtEnd(
                self.builder.as_mut_ptr(),
                value.as_value_ref(),
                var.as_mut_ptr(),
                expr.as_mut_ptr(),
                location.as_mut_ptr(),
                block.as_mut_ptr(),
            );
        }
    }

    /// Restores the location of `var` to the storage `slot` at the end of `block`: a `dbg.value` naming
    /// the slot with `DW_OP_deref` says the variable's value lives in memory there, which is what `dbg.declare`
    /// said before a kill ended it (`spec/debug.md`, DBG4).
    pub(crate) fn reopen(
        &self,
        slot: PointerValue<'ctx>,
        var: DILocalVariable<'ctx>,
        location: DILocation<'ctx>,
        block: BasicBlock<'ctx>,
    ) {
        let expr = self.builder.create_expression(vec![DW_OP_DEREF]);
        // SAFETY: as `declare`.
        unsafe {
            inkwell::llvm_sys::debuginfo::LLVMDIBuilderInsertDbgValueRecordAtEnd(
                self.builder.as_mut_ptr(),
                slot.as_value_ref(),
                var.as_mut_ptr(),
                expr.as_mut_ptr(),
                location.as_mut_ptr(),
                block.as_mut_ptr(),
            );
        }
    }

    /// The location of `span` in the current function, or `None` outside a function.
    pub(crate) fn location(
        &mut self,
        cx: &'ctx Context,
        sources: &burdin_diag::SourceMap,
        span: Span,
    ) -> Option<DILocation<'ctx>> {
        let scope = self.scope?;
        let source = sources.get(span.file);
        self.file(span.file, &source.path);
        let (line, column) = source.line_col(span.lo);
        Some(self.builder.create_debug_location(cx, line, column, scope.as_debug_info_scope(), None))
    }
}

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// Starts debug info for `f`: a subprogram, its signature type, and its parameter and local variables.
    pub(super) fn debug_function(&mut self, value: FunctionValue<'ctx>, f: &'a tir::Function) {
        if self.debug.is_none() {
            return;
        }
        let source = self.sources.get(f.span.file);
        let (line, column) = source.line_col(f.span.lo);
        let path = source.path.clone();
        let ret = self.debug_type(&f.ret);
        let params: Vec<DIType<'ctx>> = f.params.iter().map(|p| self.debug_type(&f.locals[p.0].ty)).collect();
        let db = self.debug.as_mut().expect("debug info is on");
        let file = db.file(f.span.file, &path);
        let unit_scope = db.unit.as_debug_info_scope();
        let subroutine = db.builder.create_subroutine_type(file, Some(ret), &params, DIFlagsConstants::PUBLIC);
        let scope = db.builder.create_function(
            unit_scope,
            &f.name,
            Some(&format!("burdin.{}", f.name)),
            file,
            line,
            subroutine,
            false,
            true,
            line,
            DIFlagsConstants::PUBLIC,
            self.release,
        );
        db.scope = Some(scope);
        db.file = Some(file);
        value.set_subprogram(scope);
        self.debug_declare_locals(f, scope, file, line, column);
    }

    /// Creates the debug variables of every local and parameter. No `dbg.declare` is emitted: a declare pins a
    /// stack slot for the whole function and LLVM then drops every later location change, so the locations are
    /// driven by `dbg.value` records alone (`spec/debug.md`, DBG4). A parameter's location is restored right
    /// after its store (by the caller of this method); a local's location starts at its first initialization,
    /// ends at the move or scope end the kill sites name, and shows `<optimized out>` in between — which is
    /// nothing at all, since a variable without a record has no location yet.
    fn debug_declare_locals(
        &mut self,
        f: &'a tir::Function,
        scope: DISubprogram<'ctx>,
        file: DIFile<'ctx>,
        line: u32,
        column: u32,
    ) {
        let body_scope = scope.as_debug_info_scope();
        let mut var_of: Vec<Option<DILocalVariable<'ctx>>> = vec![None; f.locals.len()];
        for (index, local) in f.locals.iter().enumerate() {
            let Some(slot) = self.slots.get(index).copied().flatten() else { continue };
            let ty = self.debug_type(&local.ty);
            let is_param = f.params.iter().any(|p| p.0 == index);
            let var = {
                let db = self.debug.as_ref().expect("debug info is on");
                if is_param {
                    let arg_no = f.params.iter().position(|p| p.0 == index).map(|p| p as u32 + 1).unwrap_or(0);
                    db.builder.create_parameter_variable(
                        body_scope,
                        &local.name,
                        arg_no,
                        file,
                        line,
                        ty,
                        true,
                        DIFlagsConstants::ZERO,
                    )
                } else {
                    db.builder.create_auto_variable(
                        body_scope,
                        &local.name,
                        file,
                        line,
                        ty,
                        true,
                        DIFlagsConstants::ZERO,
                        8,
                    )
                }
            };
            var_of[index] = Some(var);
            let _ = slot;
        }
        let db = self.debug.as_mut().expect("debug info is on");
        let location = db.builder.create_debug_location(self.cx, line, column, body_scope, None);
        db.fn_location = Some(location);
        db.vars = var_of;
        db.kills = f.debug_kills.iter().map(|k| (k.local, k.span)).collect();
    }

    /// The debug variable of local `id`, when debug information is on and the local has storage.
    pub(super) fn debug_var(&self, id: tir::LocalId) -> Option<DILocalVariable<'ctx>> {
        let db = self.debug.as_ref()?;
        db.vars.get(id.0).copied().flatten()
    }

    /// Restores the location of local `id` at the current position: the place was just initialized, so what a
    /// debugger reads is the slot again (`spec/debug.md`, DBG4).
    pub(super) fn debug_reopen(&mut self, id: tir::LocalId) {
        let Some(slot) = self.slots.get(id.0).copied().flatten() else { return };
        let Some(var) = self.debug_var(id) else { return };
        let Some(block) = self.builder.get_insert_block() else { return };
        let db = self.debug.as_ref().expect("debug info is on");
        let location = db.fn_location.expect("the function's location exists");
        db.reopen(slot, var, location, block);
    }

    /// Ends the location of local `id` at the current position, before the value it names is dropped or freed.
    pub(super) fn debug_kill(&mut self, id: tir::LocalId) {
        let Some(var) = self.debug_var(id) else { return };
        let Some(block) = self.builder.get_insert_block() else { return };
        let undef = match self.llvm_ty(&self.locals[id.0].ty) {
            Some(t) => super::undef(t),
            None => self.cx.i64_type().get_undef().into(),
        };
        let db = self.debug.as_ref().expect("debug info is on");
        let location = db.fn_location.expect("the function's location exists");
        db.kill(&undef, var, location, block);
    }

    /// The locals whose kill site is inside `span`: the statement at `span` is the last one they are alive at
    /// (`spec/debug.md`, DBG4).
    pub(super) fn debug_kills_in(&self, span: Span) -> Vec<tir::LocalId> {
        let Some(db) = self.debug.as_ref() else { return Vec::new() };
        db.kills
            .iter()
            .filter(|(_, at)| at.file == span.file && at.lo >= span.lo && at.hi <= span.hi)
            .map(|(local, _)| *local)
            .collect()
    }

    /// The debug type of `ty`, memoized. Unsupported types become an unnamed type with their name.
    pub(super) fn debug_type(&mut self, ty: &Ty) -> DIType<'ctx> {
        let key = format!("{ty:?}");
        if let Some(found) = self.debug.as_ref().and_then(|db| db.types.get(&key)).copied() {
            return found;
        }
        if self.debug.as_ref().is_some_and(|db| db.building.contains(&key)) {
            return self.debug_unspecified(&Self::debug_name(ty));
        }
        if let Some(db) = self.debug.as_mut() {
            db.building.insert(key.clone());
        }
        let built = self.debug_type_uncached(ty);
        if let Some(db) = self.debug.as_mut() {
            db.building.remove(&key);
            db.types.insert(key, built);
        }
        built
    }

    fn debug_type_uncached(&mut self, ty: &Ty) -> DIType<'ctx> {
        match ty {
            Ty::Unit | Ty::Never | Ty::Param(_) | Ty::Const(_) | Ty::Error | Ty::Chars => {
                self.debug_unspecified(&Self::debug_name(ty))
            }
            Ty::SliceChunks(..) | Ty::SliceWindows(..) => self.debug_unspecified(&Self::debug_name(ty)),
            Ty::Bool => self.debug_basic("bool", 8, DW_ATE_BOOLEAN),
            Ty::Char => self.debug_basic("char", 32, DW_ATE_UTF),
            Ty::Int(i) => {
                let encoding = if i.signed() { DW_ATE_SIGNED } else { DW_ATE_UNSIGNED };
                self.debug_basic(i.name(), u64::from(i.bits()), encoding)
            }
            Ty::Float(f) => {
                let bits = match f {
                    burdin_tir::FloatTy::F32 => 32,
                    burdin_tir::FloatTy::F64 => 64,
                };
                let name = Self::debug_name(ty);
                self.debug_basic(&name, bits, DW_ATE_FLOAT)
            }
            Ty::Ref { inner, .. } | Ty::CPtr(inner) | Ty::Heap(inner, _) => {
                let pointee = self.debug_type(inner);
                let db = self.debug.as_ref().expect("debug info is on");
                db.builder
                    .create_pointer_type(&format!("{ty:?}"), pointee, 64, 64, inkwell::AddressSpace::default())
                    .as_type()
            }
            Ty::Shared(inner) => {
                let pointee = self.debug_type(inner);
                let db = self.debug.as_ref().expect("debug info is on");
                db.builder
                    .create_pointer_type(&format!("{ty:?}"), pointee, 64, 64, inkwell::AddressSpace::default())
                    .as_type()
            }
            Ty::Str | Ty::Slice(_) => {
                let name = Self::debug_name(ty);
                self.debug_words(&name, &[("ptr", 8), ("len", 8)])
            }
            Ty::String | Ty::List(_, burdin_tir::Alloc::Global) | Ty::Map(_, _, burdin_tir::Alloc::Global) => {
                let name = Self::debug_name(ty);
                self.debug_words(&name, &[("ptr", 8), ("len", 8), ("cap", 8)])
            }
            // A container with a stored allocator appends an arbitrary type after the buffer words.
            Ty::List(..) | Ty::Map(..) => self.debug_unspecified(&Self::debug_name(ty)),
            Ty::Fn { .. } => {
                let name = Self::debug_name(ty);
                self.debug_words(&name, &[("code", 8), ("env", 8), ("drop", 8)])
            }
            Ty::Any(_) => {
                let name = Self::debug_name(ty);
                self.debug_words(&name, &[("data", 8), ("vtable", 8)])
            }
            Ty::CString => {
                let char_ty = self.debug_basic("u8", 8, DW_ATE_UNSIGNED);
                let db = self.debug.as_ref().expect("debug info is on");
                db.builder.create_pointer_type("CString", char_ty, 64, 64, inkwell::AddressSpace::default()).as_type()
            }
            Ty::Array(inner, len) => {
                let inner = self.debug_type(inner);
                let (size, align) = self.debug_sizes(ty);
                let db = self.debug.as_ref().expect("debug info is on");
                db.builder
                    .create_array_type(inner, size * 8, align * 8, std::slice::from_ref(&(0..*len as i64)))
                    .as_type()
            }
            Ty::Tuple(fields) => {
                let members: Vec<(String, Ty)> =
                    fields.iter().enumerate().map(|(i, ty)| (i.to_string(), ty.clone())).collect();
                let name = Self::debug_name(ty);
                self.debug_product(&name, &members)
            }
            Ty::Struct(adt) => {
                let members: Vec<(String, Ty)> = self.program.structs[adt.id]
                    .fields
                    .iter()
                    .map(|field| (field.name.clone(), field.ty.clone()))
                    .collect();
                let name = adt.name.to_string();
                self.debug_product(&name, &members)
            }
            Ty::Enum(adt) => self.debug_unspecified(&adt.name),
            Ty::Range(inner, _) => {
                let members = vec![("lo".to_string(), (**inner).clone()), ("hi".to_string(), (**inner).clone())];
                let name = Self::debug_name(ty);
                self.debug_product(&name, &members)
            }
            Ty::RangeIter(inner, _) => {
                let members = vec![
                    ("lo".to_string(), (**inner).clone()),
                    ("hi".to_string(), (**inner).clone()),
                    ("done".to_string(), Ty::Bool),
                ];
                let name = Self::debug_name(ty);
                self.debug_product(&name, &members)
            }
            Ty::IterMut(..) => {
                let name = Self::debug_name(ty);
                self.debug_words(&name, &[("ptr", 8), ("len", 8)])
            }
            Ty::ListIntoIter(..) | Ty::MapIter(..) | Ty::MapIterMut(..) | Ty::MapIntoIter(..) => {
                let name = Self::debug_name(ty);
                self.debug_words(&name, &[("ptr", 8), ("len", 8), ("cap", 8), ("index", 8)])
            }
            Ty::ArrayIntoIter(..) | Ty::MapEntry(..) => self.debug_unspecified(&Self::debug_name(ty)),
            Ty::Option(inner) => {
                let members = vec![("is_some".to_string(), Ty::Bool), ("value".to_string(), (**inner).clone())];
                let name = Self::debug_name(ty);
                self.debug_product(&name, &members)
            }
            Ty::Result(ok, err) => {
                let members = vec![
                    ("is_ok".to_string(), Ty::Bool),
                    ("ok".to_string(), (**ok).clone()),
                    ("err".to_string(), (**err).clone()),
                ];
                let name = Self::debug_name(ty);
                self.debug_product(&name, &members)
            }
        }
    }

    fn debug_basic(&self, name: &str, size_bits: u64, encoding: u32) -> DIType<'ctx> {
        let db = self.debug.as_ref().expect("debug info is on");
        match db.builder.create_basic_type(name, size_bits, encoding, DIFlagsConstants::ZERO) {
            Ok(basic) => basic.as_type(),
            Err(_) => self.debug_unspecified(name),
        }
    }

    /// A struct of pointer-sized words, for the built-in view and container layouts.
    fn debug_words(&mut self, name: &str, words: &[(&str, u64)]) -> DIType<'ctx> {
        let scope = self.debug_scope();
        let file = self.debug_file_current();
        let word = self.debug_basic("usize", 64, DW_ATE_UNSIGNED);
        let mut members = Vec::new();
        for (i, (member_name, _)) in words.iter().enumerate() {
            let db = self.debug.as_ref().expect("debug info is on");
            let member = db.builder.create_member_type(
                scope,
                member_name,
                file,
                0,
                64,
                64,
                64 * i as u64,
                DIFlagsConstants::ZERO,
                word,
            );
            members.push(member.as_type());
        }
        let total = 64 * words.len() as u64;
        let db = self.debug.as_ref().expect("debug info is on");
        db.builder
            .create_struct_type(scope, name, file, 0, total, 8, DIFlagsConstants::ZERO, None, &members, 0, None, "")
            .as_type()
    }

    /// A struct with one member per `(name, type)` pair, laid out field by field.
    fn debug_product(&mut self, name: &str, fields: &[(String, Ty)]) -> DIType<'ctx> {
        let scope = self.debug_scope();
        let file = self.debug_file_current();
        let mut members = Vec::new();
        let mut offset = 0u64;
        let mut struct_align = 1u32;
        for (field_name, field_ty) in fields {
            let ty = self.debug_type(field_ty);
            let (size, align) = self.debug_sizes(field_ty);
            let align = align.max(1);
            struct_align = struct_align.max(align);
            offset = offset.div_ceil(u64::from(align)) * u64::from(align);
            let member = {
                let db = self.debug.as_ref().expect("debug info is on");
                db.builder.create_member_type(
                    scope,
                    field_name,
                    file,
                    0,
                    size * 8,
                    align.max(1) * 8,
                    offset * 8,
                    DIFlagsConstants::ZERO,
                    ty,
                )
            };
            members.push(member.as_type());
            offset += size;
        }
        let total = offset.div_ceil(u64::from(struct_align)) * u64::from(struct_align);
        let db = self.debug.as_ref().expect("debug info is on");
        db.builder
            .create_struct_type(
                scope,
                name,
                file,
                0,
                total * 8,
                struct_align * 8,
                DIFlagsConstants::ZERO,
                None,
                &members,
                0,
                None,
                "",
            )
            .as_type()
    }

    fn debug_unspecified(&self, name: &str) -> DIType<'ctx> {
        // LLVM's `DIBuilderCreateUnspecifiedType` is not wrapped by inkwell; a zero-sized basic type is the
        // equivalent, and debuggers show it by name.
        self.debug_basic(name, 0, DW_ATE_UNSIGNED)
    }

    fn debug_scope(&self) -> DIScope<'ctx> {
        self.debug
            .as_ref()
            .and_then(|db| db.scope)
            .map(|scope| scope.as_debug_info_scope())
            .unwrap_or_else(|| self.debug.as_ref().expect("debug info is on").unit.as_debug_info_scope())
    }

    fn debug_file_current(&self) -> DIFile<'ctx> {
        self.debug
            .as_ref()
            .and_then(|db| db.file)
            .unwrap_or_else(|| self.debug.as_ref().expect("debug info is on").unit.get_file())
    }

    fn debug_sizes(&self, ty: &Ty) -> (u64, u32) {
        use inkwell::types::BasicTypeEnum;
        let Some(t) = self.llvm_ty(ty) else { return (0, 1) };
        match t {
            BasicTypeEnum::ArrayType(t) => {
                (self.target_data.get_store_size(&t), self.target_data.get_abi_alignment(&t))
            }
            BasicTypeEnum::FloatType(t) => {
                (self.target_data.get_store_size(&t), self.target_data.get_abi_alignment(&t))
            }
            BasicTypeEnum::IntType(t) => (self.target_data.get_store_size(&t), self.target_data.get_abi_alignment(&t)),
            BasicTypeEnum::PointerType(t) => {
                (self.target_data.get_store_size(&t), self.target_data.get_abi_alignment(&t))
            }
            BasicTypeEnum::StructType(t) => {
                (self.target_data.get_store_size(&t), self.target_data.get_abi_alignment(&t))
            }
            BasicTypeEnum::VectorType(t) => {
                (self.target_data.get_store_size(&t), self.target_data.get_abi_alignment(&t))
            }
            BasicTypeEnum::ScalableVectorType(_) => (0, 1),
        }
    }

    /// A readable name for a `Ty` that has no source spelling.
    fn debug_name(ty: &Ty) -> String {
        match ty {
            Ty::Unit => "()".to_string(),
            Ty::Str => "str".to_string(),
            Ty::String => "String".to_string(),
            Ty::Slice(inner) => format!("[{}]", Self::debug_name(inner)),
            Ty::Array(inner, n) => format!("[{}; {}]", Self::debug_name(inner), n),
            Ty::Option(inner) => format!("Option<{}>", Self::debug_name(inner)),
            Ty::Result(ok, err) => format!("Result<{}, {}>", Self::debug_name(ok), Self::debug_name(err)),
            Ty::List(inner, _) => format!("List<{}>", Self::debug_name(inner)),
            Ty::Map(k, v, _) => format!("Map<{}, {}>", Self::debug_name(k), Self::debug_name(v)),
            Ty::MapEntry(k, v, _) => {
                format!("MapEntry<{}, {}>", Self::debug_name(k), Self::debug_name(v))
            }
            Ty::Heap(inner, _) => format!("Heap<{}>", Self::debug_name(inner)),
            Ty::CPtr(inner) => format!("CPtr<{}>", Self::debug_name(inner)),
            Ty::Fn { .. } | Ty::Any(_) => ty.to_string(),
            Ty::Range(inner, false) => format!("Range<{}>", Self::debug_name(inner)),
            Ty::Range(inner, true) => format!("RangeInclusive<{}>", Self::debug_name(inner)),
            Ty::Tuple(fields) => {
                format!("({})", fields.iter().map(Self::debug_name).collect::<Vec<_>>().join(", "))
            }
            other => format!("{other:?}"),
        }
    }
}

/// Sets the builder's current source location from `span`; a no-op without `-g`.
impl<'ctx, 'a> Codegen<'ctx, 'a> {
    pub(super) fn debug_at(&mut self, span: Span) {
        self.current_span = Some(span);
        if self.debug.is_none() {
            return;
        }
        let Some(location) = self.debug.as_mut().and_then(|db| db.location(self.cx, self.sources, span)) else {
            return;
        };
        self.builder.set_current_debug_location(location);
    }

    /// Finishes the module's debug metadata. Must run before verification or object emission.
    pub(crate) fn finish_debug(&mut self) {
        if let Some(db) = self.debug.take() {
            db.builder.finalize();
        }
    }
}
