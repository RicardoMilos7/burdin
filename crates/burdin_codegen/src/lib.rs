//! LLVM code generation from the typed tree.
//!
//! Representation: integers are `iN` (`isize` and `usize` are pointer-sized), `char` is `i32`, `bool` is `i1`, `str`
//! is `{ ptr, usize }`, `String` is `{ ptr, usize len, usize cap }`, `Heap<T>` is a `malloc`ed `ptr` to a `T`, arrays
//! are LLVM arrays, and `Option<T>` is `{ i1 is_some, T }`. Tuples and structs are LLVM structs with one member per
//! field, in declaration order (a field without a value, such as `()`, is an empty struct, so member indices are
//! field indices). An enum is `{ i32 tag, [N x iA] payload }`, where the payload is sized and aligned for the largest
//! variant and is read through a pointer as the variant's own struct of fields. An `any I` box is
//! `{ ptr data, ptr vtable }`, where `data` is a `malloc`ed concrete value and `vtable` points at a private constant
//! `{ i64 type_id, ptr drop, ptr method... }` table (`spec/existentials.md`). `()` and `!` have no value.
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
mod split;
mod taskrt;

use burdin_diag::SourceMap;
use burdin_tir::Program;
use emit::Codegen;
use inkwell::OptimizationLevel;
use inkwell::attributes::{Attribute, AttributeLoc};
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine};
use std::path::{Path, PathBuf};

pub use split::{MAX_UNITS, UNIT_INSTRUCTIONS};

#[derive(Default)]
pub struct Options {
    pub release: bool,
    /// Also write the LLVM IR as text to this path.
    pub emit_ir: Option<std::path::PathBuf>,
    /// Write the unoptimized IR to this path for the driver's `--pgo-use` pipeline.
    pub pgo_ir: Option<std::path::PathBuf>,
    /// Emit source-level debug information (`-g`).
    pub debug: bool,
    /// Write LLVM bitcode instead of an object file, for link-time optimization.
    pub bitcode: bool,
    /// Add profile instrumentation (`instrprof`) so the program writes a profile when it runs.
    pub instrument: bool,
    /// How many threads compile codegen units at once; 0 uses the machine's parallelism (`-j`).
    pub jobs: usize,
    /// Split into exactly this many codegen units instead of choosing by size (`BURDIN_CODEGEN_UNITS`). Bitcode,
    /// instrumentation, and the `--pgo-use` IR always use one unit.
    pub units: Option<usize>,
}

/// The version of the LLVM library the compiler was linked against, as `major.minor.patch`.
pub fn llvm_version() -> String {
    let (mut major, mut minor, mut patch) = (0u32, 0u32, 0u32);
    // SAFETY: `LLVMGetVersion` only writes the three out-parameters.
    unsafe { inkwell::llvm_sys::core::LLVMGetVersion(&mut major, &mut minor, &mut patch) };
    format!("{major}.{minor}.{patch}")
}

/// The host target machine at the optimization level of the profile. PIC on every platform, including COFF: a
/// static relocation model makes LLVM address string constants with 32-bit absolute relocations
/// (`IMAGE_REL_AMD64_ADDR32`), which the MSVC linker rejects without `/LARGEADDRESSAWARE:NO`.
fn target_machine(release: bool) -> Result<TargetMachine, String> {
    Target::initialize_native(&InitializationConfig::default())?;
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(|e| e.to_string())?;
    let level = if release { OptimizationLevel::Default } else { OptimizationLevel::None };
    target
        .create_target_machine(&triple, "generic", "", level, RelocMode::PIC, CodeModel::Default)
        .ok_or_else(|| format!("LLVM cannot create a target machine for {}", triple.as_str().to_string_lossy()))
}

/// The number of threads `jobs` asks for: itself, or the machine's parallelism for 0.
pub fn resolve_jobs(jobs: usize) -> usize {
    if jobs > 0 { jobs } else { std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) }
}

/// Compiles the program to native object files for the host: `object` itself, and for a program large enough to
/// be split into codegen units (`split.rs`), `<stem>.1.<ext>`, `<stem>.2.<ext>`, ... next to it, compiled on up to
/// `opts.jobs` threads. Returns every object written, in link order. `sources` gives panic messages their file and
/// line. With `opts.bitcode` the single output is LLVM bitcode.
pub fn emit_object(
    program: &Program,
    sources: &SourceMap,
    module_name: &str,
    object: &Path,
    opts: &Options,
) -> Result<Vec<PathBuf>, String> {
    let machine = target_machine(opts.release)?;
    let triple = TargetMachine::get_default_triple();

    let context = Context::create();
    let module = context.create_module(module_name);
    module.set_triple(&triple);
    module.set_data_layout(&machine.get_target_data().get_data_layout());

    let debug = opts.debug.then(|| {
        let first = sources.first().map(|(_, source)| match source.path.rfind(['/', '\\']) {
            Some(i) => (source.path[i + 1..].to_string(), source.path[..i].to_string()),
            None => (source.path.clone(), ".".to_string()),
        });
        emit::DebugInfo::new(&module, opts.release, first)
    });
    let mut cg = Codegen::new(&context, &module, &machine, program, sources, opts.release, debug)?;
    cg.program()?;
    cg.finish_debug();
    // Panic backtraces unwind through the generated frames: every definition needs an unwind table, in both
    // profiles, or `backtrace` stops at the first optimized function.
    add_unwind_tables(&module);

    if let Some(path) = &opts.emit_ir {
        module.print_to_file(path).map_err(|e| e.to_string())?;
    }
    if let Some(path) = &opts.pgo_ir {
        module.print_to_file(path).map_err(|e| e.to_string())?;
    }
    module.verify().map_err(|e| format!("LLVM module verification failed:\n{}", e.to_string_lossy()))?;
    // The runtime and the glue are generated whole, and most programs use a small part of them: what nothing
    // reaches goes before the backend spends time on it, in both profiles.
    module
        .run_passes("globaldce", &machine, inkwell::passes::PassBuilderOptions::create())
        .map_err(|e| e.to_string_lossy().to_string())?;
    let single = opts.bitcode || opts.instrument || opts.pgo_ir.is_some();
    let units = if single {
        1
    } else {
        let (instructions, functions) = split::measure(&module);
        split::unit_count(instructions, functions, opts.units)
    };
    if units > 1 {
        return split::emit_units(&module, units, resolve_jobs(opts.jobs), opts.release, object);
    }
    if opts.release {
        module
            .run_passes("default<O2>", &machine, inkwell::passes::PassBuilderOptions::create())
            .map_err(|e| e.to_string_lossy().to_string())?;
    }
    if opts.instrument {
        // `pgo-instr-gen` adds the IR-level counters and `instrprof` lowers them to the profile data structures;
        // the result is what `pgo-instr-use` reads.
        module
            .run_passes("pgo-instr-gen,instrprof", &machine, inkwell::passes::PassBuilderOptions::create())
            .map_err(|e| e.to_string_lossy().to_string())?;
    }
    if opts.bitcode {
        // Link-time optimization consumes bitcode; the driver links it with an LLVM compiler driver.
        if !module.write_bitcode_to_path(object) {
            return Err(format!("cannot write LLVM bitcode to {}", object.display()));
        }
    } else {
        machine.write_to_file(&module, FileType::Object, object).map_err(|e| e.to_string_lossy().to_string())?;
    }
    Ok(vec![object.to_path_buf()])
}

/// Marks every defined function `uwtable`: without it LLVM emits no `.eh_frame` entry for a generated function, and
/// a panic backtrace would stop at the first Burdin frame. On Darwin it also keeps the frame pointer, which
/// libSystem's `backtrace` needs; the target machine is not created with clang's debug-build frame-pointer policy.
fn add_unwind_tables(module: &Module<'_>) {
    let context = module.get_context();
    let uwtable = context.create_enum_attribute(Attribute::get_named_enum_kind_id("uwtable"), 1);
    let darwin = module.get_triple().as_str().to_string_lossy().contains("darwin");
    let frame_pointer = context.create_string_attribute("frame-pointer", "all");
    for function in module.get_functions() {
        if function.count_basic_blocks() > 0 {
            function.add_attribute(AttributeLoc::Function, uwtable);
            if darwin {
                function.add_attribute(AttributeLoc::Function, frame_pointer);
            }
        }
    }
}
