//! Allocation: the default `Global` allocator and a container's stored allocator (`spec/allocators.md`).

use super::{Codegen, GenResult, llvm_err};
use burdin_diag::Span;
use burdin_tir::{Alloc, AllocatorFns, Ty};
use inkwell::values::{IntValue, PointerValue};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// `(site_ptr, site_len)` for the expression being generated: the source location the checking allocator
    /// records in a test build. A location outside an expression (an allocation made by drop or clone glue) is
    /// the runtime's own, and reported as `<runtime>`.
    pub(super) fn site(&mut self) -> (PointerValue<'ctx>, IntValue<'ctx>) {
        let text = match self.current_span {
            Some(span) => {
                let file = self.sources.get(span.file);
                let (line, column) = file.line_col(span.lo);
                format!("{}:{line}:{column}", file.path)
            }
            None => "<runtime>".to_string(),
        };
        let value = self.str_literal(text.as_bytes());
        let ptr = self.builder.build_extract_value(value, 0, "site").expect("a string has a pointer");
        let len = self.builder.build_extract_value(value, 1, "site.len").expect("a string has a length");
        (ptr.into_pointer_value(), len.into_int_value())
    }

    /// An allocation from the default allocator, recording `span` for a leak report.
    pub(super) fn global_alloc(&mut self, size: IntValue<'ctx>, span: Option<Span>) -> GenResult<PointerValue<'ctx>> {
        let saved = self.current_span;
        if span.is_some() {
            self.current_span = span;
        }
        let (site, len) = self.site();
        let call = self
            .builder
            .build_call(self.rt.alloc, &[size.into(), site.into(), len.into()], "alloc")
            .map_err(llvm_err)?;
        self.current_span = saved;
        Ok(call.try_as_basic_value().basic().expect("alloc returns a pointer").into_pointer_value())
    }

    /// A reallocation from the default allocator.
    pub(super) fn global_realloc(
        &mut self,
        ptr: PointerValue<'ctx>,
        size: IntValue<'ctx>,
        span: Option<Span>,
    ) -> GenResult<PointerValue<'ctx>> {
        let saved = self.current_span;
        if span.is_some() {
            self.current_span = span;
        }
        let (site, len) = self.site();
        let call = self
            .builder
            .build_call(self.rt.realloc, &[ptr.into(), size.into(), site.into(), len.into()], "realloc")
            .map_err(llvm_err)?;
        self.current_span = saved;
        Ok(call.try_as_basic_value().basic().expect("realloc returns a pointer").into_pointer_value())
    }

    /// Frees a default-allocator allocation.
    pub(super) fn global_dealloc(&mut self, ptr: PointerValue<'ctx>) -> GenResult<()> {
        self.builder.build_call(self.rt.free, &[ptr.into()], "").map_err(llvm_err)?;
        Ok(())
    }

    /// Whether `ty` is the standard library's default allocator, which a container stores as no value at all.
    pub(super) fn is_global_allocator(&self, ty: &burdin_tir::Ty) -> bool {
        self.program.global_alloc_ty.as_ref() == Some(ty)
    }

    /// The functions of a concrete allocator's `Allocator` implementation.
    fn allocator_fns(&self, ty: &Ty) -> GenResult<&AllocatorFns> {
        self.program
            .allocator_fns
            .iter()
            .find(|f| &f.ty == ty)
            .ok_or_else(|| format!("no `Allocator` implementation of `{ty}` is in the program"))
    }

    /// Calls `allocate` on the value at `addr`. Every argument is passed by address, as a Burdin convention.
    fn stored_alloc(
        &mut self,
        ty: &Ty,
        addr: PointerValue<'ctx>,
        size: IntValue<'ctx>,
        align: u64,
    ) -> GenResult<PointerValue<'ctx>> {
        let f = self.functions[self.allocator_fns(ty)?.allocate.0];
        let size = self.spill(Some(size.into()), &Ty::Int(burdin_tir::IntTy::Usize))?.expect("a spilled size");
        let align = self
            .spill(Some(self.usize_ty.const_int(align, false).into()), &Ty::Int(burdin_tir::IntTy::Usize))?
            .expect("a spilled alignment");
        let call = self.builder.build_call(f, &[addr.into(), size.into(), align.into()], "alloc").map_err(llvm_err)?;
        Ok(call.try_as_basic_value().basic().expect("allocate returns a pointer").into_pointer_value())
    }

    fn stored_realloc(
        &mut self,
        ty: &Ty,
        addr: PointerValue<'ctx>,
        ptr: PointerValue<'ctx>,
        old_size: IntValue<'ctx>,
        new_size: IntValue<'ctx>,
        align: u64,
    ) -> GenResult<PointerValue<'ctx>> {
        let f = self.functions[self.allocator_fns(ty)?.reallocate.0];
        let usize = Ty::Int(burdin_tir::IntTy::Usize);
        let ptr = self
            .spill(Some(ptr.into()), &Ty::CPtr(Box::new(Ty::Int(burdin_tir::IntTy::U8))))?
            .expect("a spilled pointer");
        let old_size = self.spill(Some(old_size.into()), &usize)?.expect("a spilled size");
        let new_size = self.spill(Some(new_size.into()), &usize)?.expect("a spilled size");
        let align =
            self.spill(Some(self.usize_ty.const_int(align, false).into()), &usize)?.expect("a spilled alignment");
        let call = self
            .builder
            .build_call(f, &[addr.into(), ptr.into(), old_size.into(), new_size.into(), align.into()], "realloc")
            .map_err(llvm_err)?;
        Ok(call.try_as_basic_value().basic().expect("reallocate returns a pointer").into_pointer_value())
    }

    fn stored_dealloc(
        &mut self,
        ty: &Ty,
        addr: PointerValue<'ctx>,
        ptr: PointerValue<'ctx>,
        size: IntValue<'ctx>,
        align: u64,
    ) -> GenResult<()> {
        let f = self.functions[self.allocator_fns(ty)?.deallocate.0];
        let usize = Ty::Int(burdin_tir::IntTy::Usize);
        let ptr = self
            .spill(Some(ptr.into()), &Ty::CPtr(Box::new(Ty::Int(burdin_tir::IntTy::U8))))?
            .expect("a spilled pointer");
        let size = self.spill(Some(size.into()), &usize)?.expect("a spilled size");
        let align =
            self.spill(Some(self.usize_ty.const_int(align, false).into()), &usize)?.expect("a spilled alignment");
        self.builder
            .build_call(f, &[addr.into(), ptr.into(), size.into(), align.into()], "dealloc")
            .map_err(llvm_err)?;
        Ok(())
    }

    /// Allocates `size` bytes for a container whose allocator is `alloc`, stored at `addr` when it has one.
    pub(super) fn alloc_in(
        &mut self,
        alloc: &Alloc,
        addr: Option<PointerValue<'ctx>>,
        size: IntValue<'ctx>,
        align: u64,
        span: Option<Span>,
    ) -> GenResult<PointerValue<'ctx>> {
        match alloc {
            Alloc::Global => self.global_alloc(size, span),
            Alloc::Value(ty) => {
                let addr = addr.ok_or_else(|| "a stored allocator has no address".to_string())?;
                self.stored_alloc(ty, addr, size, align)
            }
        }
    }

    /// Resizes a buffer allocated by [`Codegen::alloc_in`].
    #[allow(clippy::too_many_arguments)]
    pub(super) fn realloc_in(
        &mut self,
        alloc: &Alloc,
        addr: Option<PointerValue<'ctx>>,
        ptr: PointerValue<'ctx>,
        old_size: IntValue<'ctx>,
        new_size: IntValue<'ctx>,
        align: u64,
        span: Option<Span>,
    ) -> GenResult<PointerValue<'ctx>> {
        match alloc {
            Alloc::Global => self.global_realloc(ptr, new_size, span),
            Alloc::Value(ty) => {
                let addr = addr.ok_or_else(|| "a stored allocator has no address".to_string())?;
                self.stored_realloc(ty, addr, ptr, old_size, new_size, align)
            }
        }
    }

    /// Frees a buffer with the allocator that made it.
    pub(super) fn dealloc_in(
        &mut self,
        alloc: &Alloc,
        addr: Option<PointerValue<'ctx>>,
        ptr: PointerValue<'ctx>,
        size: IntValue<'ctx>,
        align: u64,
    ) -> GenResult<()> {
        match alloc {
            Alloc::Global => self.global_dealloc(ptr),
            Alloc::Value(ty) => {
                let addr = addr.ok_or_else(|| "a stored allocator has no address".to_string())?;
                self.stored_dealloc(ty, addr, ptr, size, align)
            }
        }
    }

    /// The cell pointer of a `Shared<T>` receiver, read from its place or spilled to a temporary.
    pub(super) fn shared_pointer(&mut self, recv: &'a burdin_tir::Expr) -> GenResult<PointerValue<'ctx>> {
        let ptr_ty = self.cx.ptr_type(inkwell::AddressSpace::default());
        let storage = match self.direct_place_ptr(recv)? {
            Some(p) => p,
            None => {
                let v = self.expr_mode(recv, super::Mode::Use)?;
                let slot = self.alloca(ptr_ty.into(), "shared")?;
                let v = self.need(v, &recv.ty);
                self.builder.build_store(slot, v).map_err(llvm_err)?;
                slot
            }
        };
        Ok(self.builder.build_load(ptr_ty, storage, "shared.ptr").map_err(llvm_err)?.into_pointer_value())
    }

    /// `ptr + offset` bytes.
    pub(super) fn offset_ptr(
        &mut self,
        ptr: PointerValue<'ctx>,
        offset: IntValue<'ctx>,
    ) -> GenResult<PointerValue<'ctx>> {
        unsafe { self.builder.build_in_bounds_gep(self.cx.i8_type(), ptr, &[offset], "") }.map_err(llvm_err)
    }

    /// The alignment of a value of `ty`, capped at 8 (every Burdin type's alignment is at most one machine word).
    pub(super) fn value_align(&self, ty: &Ty) -> u64 {
        self.llvm_ty(ty).map(|t| u64::from(self.target_data.get_abi_alignment(&t))).unwrap_or(8).clamp(1, 8)
    }
}
