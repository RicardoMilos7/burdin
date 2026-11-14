//! LLVM code generation from the typed tree.
//!
//! Representation: integers are `iN` (`isize` and `usize` are pointer-sized), `char` is `i32`, `bool` is `i1`, `str`
//! is `{ ptr, usize }`, `String` is `{ ptr, usize len, usize cap }`, `Heap<T>` is a `malloc`ed `ptr` to a `T`, arrays
//! are LLVM arrays, and `Option<T>` is `{ i1 is_some, T }`. Tuples and structs are LLVM structs with one member per
//! field, in declaration order (a field without a value, such as `()`, is an empty struct, so member indices are
//! field indices). An enum is `{ i32 tag, [N x iA] payload }`, where the payload is sized and aligned for the largest
//! variant and is read through a pointer as the variant's own struct of fields. `()` and `!` have no value.
//!
//! **Ownership.** Values are owned by exactly one place: a local, a parameter, a field, or a temporary. A move reads
//! the value and leaves the source uninitialized. The move checker records every place a function reads in a move
//! context (`tir::Function::moved`); code generation gives each such place a runtime `i1` drop flag in a stack slot, set
//! when the place is initialized and cleared by a move. Every drop site — the end of a scope, an overwritten assignment
//! (DR4), the end of a statement for temporaries (DR5), and every early exit (DR6) — checks the flag of each tracked
//! path and drops a partially moved aggregate field by field (DR7). Writing a place sets its flags again.
//!
//! Drop and clone glue live in internal functions (`burdin.drop.N`, `burdin.clone.N`). A type that contains itself
//! through a `Heap` gets its glue declared before a body is generated, so recursion terminates. `Heap.new` allocates,
//! moving the value out of a `Heap` frees the allocation (MV6), and a `String` frees its buffer from its drop glue.
//!
//! A `mut self` receiver is passed by address: the callee's `self` local uses the caller's place as its storage.
//! Functions are internal and named `burdin.<name>`. The module also defines the process entry point `main`, which
//! calls the program's `main`, and the runtime (output, string allocation and conversions, heap allocation, and the
//! panic path), generated as IR that calls libc (see `runtime.rs`).
//!
//! Every function starts with an `entry` block that holds only stack slots and their initialization, and branches to
//! the body. After a terminator (`return`, `break`, a panic, ...) code generation continues in a fresh block without
//! predecessors, so the rest of the tree can be generated without special cases.
mod emit;
mod runtime;

use burdin_diag::SourceMap;
use burdin_tir::Program;
use emit::Codegen;
use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine};
use std::path::Path;

pub struct Options {
    pub release: bool,
    /// Also write the LLVM IR as text to this path.
    pub emit_ir: Option<std::path::PathBuf>,
}

/// Compiles the program to a native object file for the host. `sources` gives panic messages their file and line.
pub fn emit_object(
    program: &Program,
    sources: &SourceMap,
    module_name: &str,
    object: &Path,
    opts: &Options,
) -> Result<(), String> {
    Target::initialize_native(&InitializationConfig::default())?;
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(|e| e.to_string())?;
    let level = if opts.release { OptimizationLevel::Default } else { OptimizationLevel::None };
    // PIC on every platform, including COFF: a static relocation model makes LLVM address string constants with
    // 32-bit absolute relocations (`IMAGE_REL_AMD64_ADDR32`), which the MSVC linker rejects without
    // `/LARGEADDRESSAWARE:NO`.
    let machine = target
        .create_target_machine(&triple, "generic", "", level, RelocMode::PIC, CodeModel::Default)
        .ok_or_else(|| format!("LLVM cannot create a target machine for {}", triple.as_str().to_string_lossy()))?;

    let context = Context::create();
    let module = context.create_module(module_name);
    module.set_triple(&triple);
    module.set_data_layout(&machine.get_target_data().get_data_layout());

    let mut cg = Codegen::new(&context, &module, &machine, program, sources, opts.release)?;
    cg.program()?;

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
