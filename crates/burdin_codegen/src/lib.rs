//! LLVM code generation from the typed tree.
//!
//! Representation: `str` is `{ ptr, usize }`; `bool` is `i1`; `()` has no value. Functions are internal and named
//! `burdin.<name>`. The entry point `burdin_entry` calls the program's `main`; the C runtime (runtime/rt.c) defines
//! the real `main` and the output functions.

use burdin_sema::tir::{self, Builtin, FuncId, Program, Ty};
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, IntType, StructType};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, FunctionValue, PointerValue};
use inkwell::{AddressSpace, OptimizationLevel};
use std::path::Path;

pub struct Options {
    pub release: bool,
    /// Also write the LLVM IR as text to this path.
    pub emit_ir: Option<std::path::PathBuf>,
}

/// Compiles the program to a native object file for the host.
pub fn emit_object(program: &Program, module_name: &str, object: &Path, opts: &Options) -> Result<(), String> {
    Target::initialize_native(&InitializationConfig::default())?;
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(|e| e.to_string())?;
    let level = if opts.release { OptimizationLevel::Default } else { OptimizationLevel::None };
    let machine = target
        .create_target_machine(&triple, "generic", "", level, RelocMode::PIC, CodeModel::Default)
        .ok_or_else(|| format!("LLVM cannot create a target machine for {}", triple.as_str().to_string_lossy()))?;

    let context = Context::create();
    let module = context.create_module(module_name);
    module.set_triple(&triple);
    module.set_data_layout(&machine.get_target_data().get_data_layout());

    let mut cg = Codegen::new(&context, &module, &machine);
    cg.program(program)?;

    if let Some(path) = &opts.emit_ir {
        module.print_to_file(path).map_err(|e| e.to_string())?;
    }
    module.verify().map_err(|e| format!("LLVM module verification failed:\n{}", e.to_string_lossy()))?;
    if opts.release {
        module
            .run_passes("default<O2>", &machine, inkwell::passes::PassBuilderOptions::create())
            .map_err(|e| e.to_string_lossy().to_string())?;
    }
    machine.write_to_file(&module, FileType::Object, object).map_err(|e| e.to_string_lossy().to_string())
}

struct Codegen<'ctx, 'm> {
    cx: &'ctx Context,
    module: &'m Module<'ctx>,
    builder: Builder<'ctx>,
    str_ty: StructType<'ctx>,
    usize_ty: IntType<'ctx>,
    functions: Vec<FunctionValue<'ctx>>,
    rt_print: FunctionValue<'ctx>,
    /// Storage of the locals of the function being generated (`None` for locals without a value).
    slots: Vec<Option<PointerValue<'ctx>>>,
    string_count: usize,
}

type GenResult<T> = Result<T, String>;

fn llvm_err(e: inkwell::builder::BuilderError) -> String {
    format!("LLVM builder error: {e}")
}

impl<'ctx, 'm> Codegen<'ctx, 'm> {
    fn new(cx: &'ctx Context, module: &'m Module<'ctx>, machine: &TargetMachine) -> Self {
        let ptr = cx.ptr_type(AddressSpace::default());
        let usize_ty = cx.ptr_sized_int_type(&machine.get_target_data(), None);
        let str_ty = cx.struct_type(&[ptr.into(), usize_ty.into()], false);
        // void burdin_rt_print(int32_t fd, const uint8_t *ptr, size_t len, int32_t newline)
        let print_ty =
            cx.void_type().fn_type(&[cx.i32_type().into(), ptr.into(), usize_ty.into(), cx.i32_type().into()], false);
        let rt_print = module.add_function("burdin_rt_print", print_ty, Some(Linkage::External));
        Codegen {
            cx,
            module,
            builder: cx.create_builder(),
            str_ty,
            usize_ty,
            functions: Vec::new(),
            rt_print,
            slots: Vec::new(),
            string_count: 0,
        }
    }

    /// The LLVM type of a value of `ty`, or `None` for types without a runtime value.
    fn llvm_ty(&self, ty: Ty) -> Option<BasicTypeEnum<'ctx>> {
        match ty {
            Ty::Unit | Ty::Never | Ty::Error => None,
            Ty::Bool => Some(self.cx.bool_type().into()),
            Ty::Str => Some(self.str_ty.into()),
        }
    }

    fn program(&mut self, program: &Program) -> GenResult<()> {
        for f in &program.functions {
            let params: Vec<BasicMetadataTypeEnum> =
                f.params.iter().filter_map(|p| self.llvm_ty(f.locals[p.0].ty)).map(|t| t.into()).collect();
            let fn_ty = match self.llvm_ty(f.ret) {
                Some(t) => t.fn_type(&params, false),
                None => self.cx.void_type().fn_type(&params, false),
            };
            let value = self.module.add_function(&format!("burdin.{}", f.name), fn_ty, Some(Linkage::Internal));
            self.functions.push(value);
        }
        for (i, f) in program.functions.iter().enumerate() {
            self.function(self.functions[i], f)?;
        }

        let entry = self.module.add_function("burdin_entry", self.cx.void_type().fn_type(&[], false), None);
        let bb = self.cx.append_basic_block(entry, "entry");
        self.builder.position_at_end(bb);
        self.builder.build_call(self.functions[program.main.0], &[], "").map_err(llvm_err)?;
        self.builder.build_return(None).map_err(llvm_err)?;
        Ok(())
    }

    fn function(&mut self, value: FunctionValue<'ctx>, f: &tir::Function) -> GenResult<()> {
        let entry = self.cx.append_basic_block(value, "entry");
        self.builder.position_at_end(entry);
        self.slots.clear();
        for local in &f.locals {
            let slot = match self.llvm_ty(local.ty) {
                Some(t) => Some(self.builder.build_alloca(t, &local.name).map_err(llvm_err)?),
                None => None,
            };
            self.slots.push(slot);
        }
        let mut arg_index = 0;
        for p in &f.params {
            if let Some(slot) = self.slots[p.0] {
                let arg = value.get_nth_param(arg_index).expect("parameter count matches the signature");
                self.builder.build_store(slot, arg).map_err(llvm_err)?;
                arg_index += 1;
            }
        }
        let result = self.block(&f.body)?;
        match (result, self.llvm_ty(f.ret)) {
            (Some(v), Some(_)) => self.builder.build_return(Some(&v)).map_err(llvm_err)?,
            _ => self.builder.build_return(None).map_err(llvm_err)?,
        };
        Ok(())
    }

    fn block(&mut self, b: &tir::Block) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        for s in &b.stmts {
            match s {
                tir::Stmt::Let(local, init) => {
                    let v = self.expr(init)?;
                    if let (Some(slot), Some(v)) = (self.slots[local.0], v) {
                        self.builder.build_store(slot, v).map_err(llvm_err)?;
                    }
                }
                tir::Stmt::Expr(e) => {
                    self.expr(e)?;
                }
            }
        }
        match &b.tail {
            Some(e) => self.expr(e),
            None => Ok(None),
        }
    }

    fn expr(&mut self, e: &tir::Expr) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        match &e.kind {
            tir::ExprKind::Unit => Ok(None),
            tir::ExprKind::Bool(b) => Ok(Some(self.cx.bool_type().const_int(u64::from(*b), false).into())),
            tir::ExprKind::Str(s) => Ok(Some(self.str_literal(s.as_bytes()).into())),
            tir::ExprKind::Local(id) => match (self.slots[id.0], self.llvm_ty(e.ty)) {
                (Some(slot), Some(t)) => Ok(Some(self.builder.build_load(t, slot, "").map_err(llvm_err)?)),
                _ => Ok(None),
            },
            tir::ExprKind::Block(b) => self.block(b),
            tir::ExprKind::Call(f, args) => self.call(*f, args, e.ty),
            tir::ExprKind::Builtin(b, args) => {
                let s = self.expr(&args[0])?.expect("a str argument has a value").into_struct_value();
                let ptr = self.builder.build_extract_value(s, 0, "ptr").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(s, 1, "len").map_err(llvm_err)?;
                let (fd, newline) = match b {
                    Builtin::Print => (1, 0),
                    Builtin::Println => (1, 1),
                    Builtin::Eprint => (2, 0),
                    Builtin::Eprintln => (2, 1),
                };
                let i32_ty = self.cx.i32_type();
                let args: [BasicMetadataValueEnum; 4] = [
                    i32_ty.const_int(fd, false).into(),
                    ptr.into(),
                    len.into(),
                    i32_ty.const_int(newline, false).into(),
                ];
                self.builder.build_call(self.rt_print, &args, "").map_err(llvm_err)?;
                Ok(None)
            }
        }
    }

    fn call(&mut self, f: FuncId, args: &[tir::Expr], ret: Ty) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let mut values: Vec<BasicMetadataValueEnum> = Vec::new();
        for a in args {
            if let Some(v) = self.expr(a)? {
                values.push(v.into());
            }
        }
        let call = self.builder.build_call(self.functions[f.0], &values, "").map_err(llvm_err)?;
        if self.llvm_ty(ret).is_none() {
            return Ok(None);
        }
        Ok(call.try_as_basic_value().basic())
    }

    /// A `str` constant pointing to a private global holding the bytes (no terminating NUL).
    fn str_literal(&mut self, bytes: &[u8]) -> inkwell::values::StructValue<'ctx> {
        let data = self.cx.const_string(bytes, false);
        let global = self.module.add_global(data.get_type(), None, &format!("str.{}", self.string_count));
        self.string_count += 1;
        global.set_initializer(&data);
        global.set_constant(true);
        global.set_linkage(Linkage::Private);
        global.set_unnamed_addr(true);
        let len = self.usize_ty.const_int(bytes.len() as u64, false);
        self.str_ty.const_named_struct(&[global.as_pointer_value().into(), len.into()])
    }
}
