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
mod cache;
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

/// The optimization level a build asks for (`spec/profiles.md`): LLVM's four. There is no `s` or `z` yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OptLevel {
    /// No optimization pipeline.
    #[default]
    None,
    O1,
    O2,
    O3,
}

impl OptLevel {
    /// The level a profile's `opt-level` names, or `None` for anything else.
    pub fn from_number(number: i64) -> Option<OptLevel> {
        match number {
            0 => Some(OptLevel::None),
            1 => Some(OptLevel::O1),
            2 => Some(OptLevel::O2),
            3 => Some(OptLevel::O3),
            _ => None,
        }
    }

    /// The level as a number, the way the profile cache key spells it.
    pub fn number(self) -> u8 {
        match self {
            OptLevel::None => 0,
            OptLevel::O1 => 1,
            OptLevel::O2 => 2,
            OptLevel::O3 => 3,
        }
    }

    /// The LLVM pass pipeline for the level. Level 0 is named too, because the always-on cleanup runs there as
    /// well, so the IR the backend sees is the same shape in every profile.
    pub fn pipeline(self) -> String {
        format!("default<O{}>", self.number())
    }

    /// Whether anything optimizes.
    pub fn optimized(self) -> bool {
        self != OptLevel::None
    }
}

/// A package's own overflow and debug-assertion settings (`[profile.<name>.package.<package>]`). The first entry
/// whose directory is a prefix of a function's source file applies; a file no entry matches uses the profile's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageFlags {
    pub dir: PathBuf,
    pub overflow_checks: bool,
    pub debug_assertions: bool,
}

pub use cache::ObjectCache;

pub struct Options {
    /// The optimization level (`spec/profiles.md`).
    pub opt_level: OptLevel,
    /// Keep `debug_assert*` (`spec/stdlib.md`).
    pub debug_assertions: bool,
    /// Panic on integer overflow (`spec/memory-rules.md`).
    pub overflow_checks: bool,
    /// Per-package overrides of the two flags above (`--profile`, `spec/profiles.md`, BP7).
    pub package_flags: Vec<PackageFlags>,
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
    /// instrumentation, the `--pgo-use` IR, and every inspection output below always use one unit.
    pub units: Option<usize>,
    /// Also write the LLVM IR the backend sees, after the profile's optimization pipeline, as text
    /// (`--emit=llvm-ir-opt`, `burdin asm --llvm`).
    pub emit_opt_ir: Option<PathBuf>,
    /// Also write the optimized module as LLVM bitcode (`--emit=llvm-bc`).
    pub emit_bitcode: Option<PathBuf>,
    /// Also write the program's assembly (`--emit=asm`, `burdin asm`). With debug information the text carries
    /// the `.loc` line table, which `burdin asm` uses to interleave source lines.
    pub emit_asm: Option<PathBuf>,
    /// The assembly dialect of `emit_asm`. Only x86 has two; asking for either on another processor is an error.
    pub asm_syntax: AsmSyntax,
    /// The sanitizers to instrument for (`--sanitize`, `spec/sanitizers.md`) and libFuzzer's coverage
    /// (`burdin fuzz`, `spec/fuzz.md`). An instrumented program is always one codegen unit.
    pub sanitize: Sanitizers,
    /// Keep each codegen unit's object under a hash of its module and reuse it when a later build produces the same
    /// module (`spec/target.md`, TG11-TG16). Ignored by every build that always uses one unit for its own reasons
    /// (bitcode, instrumentation, and the inspection outputs).
    pub object_cache: Option<ObjectCache>,
    /// Build a library rather than a program (`spec/library.md`, LB5 and LB6): no entry point, the static initializer
    /// runs when the object is loaded, every function that is not an exported `extern "C" fn` is local, and the
    /// plain allocator is used, as in a program.
    pub library: bool,
}

impl Default for Options {
    /// The `dev` profile: no optimization, debug assertions and overflow checks on, nothing else.
    fn default() -> Options {
        Options {
            opt_level: OptLevel::None,
            debug_assertions: true,
            overflow_checks: true,
            package_flags: Vec::new(),
            library: false,
            emit_ir: None,
            pgo_ir: None,
            debug: false,
            bitcode: false,
            instrument: false,
            jobs: 0,
            units: None,
            emit_opt_ir: None,
            emit_bitcode: None,
            emit_asm: None,
            asm_syntax: AsmSyntax::Default,
            sanitize: Sanitizers::default(),
            object_cache: None,
        }
    }
}

/// What `--sanitize` and `burdin fuzz` instrument. `leak` adds no instrumentation (the runtime finds leaks from the
/// allocator alone), so only the linker sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sanitizers {
    pub address: bool,
    pub thread: bool,
    pub leak: bool,
    pub undefined: bool,
    /// libFuzzer's coverage feedback: inline 8-bit edge counters, the PC table, and comparison tracing.
    pub fuzzer: bool,
}

impl Sanitizers {
    /// Whether anything changes the generated code.
    pub fn instruments(&self) -> bool {
        self.address || self.thread || self.undefined || self.fuzzer
    }

    /// Whether the link needs a sanitizer runtime library. `undefined` is checks in the generated code that panic
    /// like any other run-time check, so it needs none.
    pub fn needs_runtime(&self) -> bool {
        self.address || self.thread || self.leak || self.fuzzer
    }

    /// The `-fsanitize=` list of the link, in a fixed order; empty when nothing is linked.
    pub fn link_list(&self) -> String {
        let mut names = Vec::new();
        if self.fuzzer {
            names.push("fuzzer");
        }
        if self.address {
            names.push("address");
        }
        if self.thread {
            names.push("thread");
        }
        if self.leak {
            names.push("leak");
        }
        names.join(",")
    }
}

/// The assembly dialect `--emit=asm` and `burdin asm` print.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AsmSyntax {
    /// The target's own default (AT&T on x86).
    #[default]
    Default,
    Intel,
    Att,
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
fn target_machine(level: OptLevel) -> Result<TargetMachine, String> {
    Target::initialize_native(&InitializationConfig::default())?;
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(|e| e.to_string())?;
    let level = match level {
        OptLevel::None => OptimizationLevel::None,
        OptLevel::O1 => OptimizationLevel::Less,
        OptLevel::O2 => OptimizationLevel::Default,
        OptLevel::O3 => OptimizationLevel::Aggressive,
    };
    target
        .create_target_machine(&triple, "generic", "", level, RelocMode::PIC, CodeModel::Default)
        .ok_or_else(|| format!("LLVM cannot create a target machine for {}", triple.as_str().to_string_lossy()))
}

/// The number of threads `jobs` asks for: itself, or the machine's parallelism for 0.
pub fn resolve_jobs(jobs: usize) -> usize {
    if jobs > 0 { jobs } else { std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) }
}

/// What [`emit_object_timed`] produced: the object files in link order, and how long each codegen unit took, in unit
/// order. An unsplit program has one unit, which is the whole module (`burdin build -v` prints these).
pub struct Emitted {
    pub objects: Vec<PathBuf>,
    pub unit_times: Vec<std::time::Duration>,
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
    emit_object_timed(program, sources, module_name, object, opts).map(|emitted| emitted.objects)
}

/// [`emit_object`], also reporting the time each codegen unit took.
pub fn emit_object_timed(
    program: &Program,
    sources: &SourceMap,
    module_name: &str,
    object: &Path,
    opts: &Options,
) -> Result<Emitted, String> {
    // The dialect is read when the target machine builds its assembly printer, so it is set first.
    set_asm_syntax(opts.asm_syntax)?;
    let machine = target_machine(opts.opt_level)?;
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
        emit::DebugInfo::new(&module, opts.opt_level.optimized(), first)
    });
    let mut cg = Codegen::new(
        &context,
        &module,
        &machine,
        program,
        sources,
        emit::ProfileFlags {
            debug_assertions: opts.debug_assertions,
            optimized: opts.opt_level.optimized(),
            overflow_checks: opts.overflow_checks,
            packages: opts.package_flags.clone(),
            library: opts.library,
        },
        debug,
    )?;
    cg.undefined_checks = opts.sanitize.undefined;
    cg.sanitized = opts.sanitize.needs_runtime();
    cg.program()?;
    cg.finish_debug();
    if opts.sanitize.address {
        runtime::define_asan_default_options(&context, &module)?;
    }
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
    let single = opts.bitcode
        || opts.sanitize.instruments()
        || opts.instrument
        || opts.pgo_ir.is_some()
        || opts.emit_opt_ir.is_some()
        || opts.emit_bitcode.is_some()
        || opts.emit_asm.is_some();
    let units = if single {
        1
    } else {
        let (instructions, functions) = split::measure(&module);
        split::unit_count(instructions, functions, opts.units)
    };
    if units > 1 {
        let (objects, unit_times) = split::emit_units(
            &module,
            units,
            resolve_jobs(opts.jobs),
            opts.opt_level,
            object,
            opts.object_cache.as_ref(),
        )?;
        return Ok(Emitted { objects, unit_times });
    }
    // One unit: the whole module is the unit, and its object is cached the same way (TG12).
    let cache = opts.object_cache.as_ref().filter(|_| !single);
    let cache_key = cache.map(|c| {
        let bitcode = module.write_bitcode_to_memory();
        c.key(bitcode.as_slice(), opts.opt_level.number(), &triple.as_str().to_string_lossy())
    });
    let cache_started = std::time::Instant::now();
    if let (Some(cache), Some(key)) = (cache, &cache_key)
        && cache.fetch(key, object)
    {
        cache.prune(&std::iter::once(key.clone()).collect());
        return Ok(Emitted { objects: vec![object.to_path_buf()], unit_times: vec![cache_started.elapsed()] });
    }
    let started = std::time::Instant::now();
    let objects = emit_single(&module, &machine, opts, object, cache, cache_key.as_ref())?;
    Ok(Emitted { objects, unit_times: vec![started.elapsed()] })
}

/// Optimizes the whole module of an unsplit program and writes its one object file (and the artifacts that ask for
/// the optimized module).
fn emit_single(
    module: &Module<'_>,
    machine: &TargetMachine,
    opts: &Options,
    object: &Path,
    cache: Option<&crate::cache::ObjectCache>,
    cache_key: Option<&String>,
) -> Result<Vec<PathBuf>, String> {
    // The attributes go on before the optimization pipeline, as clang's front end puts them: some transformations
    // are not done to a function that will be instrumented.
    add_sanitizer_attributes(module, opts.sanitize);
    if opts.opt_level.optimized() {
        let pipeline = opts.opt_level.pipeline();
        module
            .run_passes(&pipeline, machine, inkwell::passes::PassBuilderOptions::create())
            .map_err(|e| e.to_string_lossy().to_string())?;
    }
    instrument_sanitizers(module, machine, opts.sanitize)?;
    if opts.instrument {
        // `pgo-instr-gen` adds the IR-level counters and `instrprof` lowers them to the profile data structures;
        // the result is what `pgo-instr-use` reads.
        module
            .run_passes("pgo-instr-gen,instrprof", machine, inkwell::passes::PassBuilderOptions::create())
            .map_err(|e| e.to_string_lossy().to_string())?;
    }
    if let Some(path) = &opts.emit_opt_ir {
        module.print_to_file(path).map_err(|e| e.to_string())?;
    }
    if let Some(path) = &opts.emit_bitcode
        && !module.write_bitcode_to_path(path)
    {
        return Err(format!("cannot write LLVM bitcode to {}", path.display()));
    }
    if let Some(path) = &opts.emit_asm {
        machine.write_to_file(module, FileType::Assembly, path).map_err(|e| e.to_string_lossy().to_string())?;
    }
    if opts.bitcode {
        // Link-time optimization consumes bitcode; the driver links it with an LLVM compiler driver.
        if !module.write_bitcode_to_path(object) {
            return Err(format!("cannot write LLVM bitcode to {}", object.display()));
        }
    } else {
        machine.write_to_file(module, FileType::Object, object).map_err(|e| e.to_string_lossy().to_string())?;
        if let (Some(cache), Some(key)) = (cache, cache_key) {
            cache.store(key, object);
            cache.prune(&std::iter::once(key.clone()).collect());
        }
    }
    Ok(vec![object.to_path_buf()])
}

/// Opts every definition in to AddressSanitizer and ThreadSanitizer, which only instrument functions that carry
/// `sanitize_address` or `sanitize_thread`.
fn add_sanitizer_attributes(module: &Module<'_>, sanitize: Sanitizers) {
    let context = module.get_context();
    let mut attributes = Vec::new();
    if sanitize.address {
        attributes.push(context.create_enum_attribute(Attribute::get_named_enum_kind_id("sanitize_address"), 0));
    }
    if sanitize.thread {
        attributes.push(context.create_enum_attribute(Attribute::get_named_enum_kind_id("sanitize_thread"), 0));
    }
    for function in module.get_functions() {
        if function.count_basic_blocks() > 0 {
            for attribute in &attributes {
                function.add_attribute(AttributeLoc::Function, *attribute);
            }
        }
    }
}

/// Adds the sanitizer instrumentation after the profile's optimization pipeline, where clang adds it too. The
/// coverage pass's options are LLVM command-line options, which are process-wide and set once.
fn instrument_sanitizers(module: &Module<'_>, machine: &TargetMachine, sanitize: Sanitizers) -> Result<(), String> {
    let mut passes = Vec::new();
    if sanitize.fuzzer {
        set_coverage_options();
        passes.push("sancov-module");
    }
    if sanitize.address {
        passes.push("asan");
    }
    if sanitize.thread {
        passes.push("tsan-module,function(tsan)");
    }
    if passes.is_empty() {
        return Ok(());
    }
    if sanitize.address || sanitize.thread {
        remove_zero_sized_accesses(module, machine);
    }
    module
        .run_passes(&passes.join(","), machine, inkwell::passes::PassBuilderOptions::create())
        .map_err(|e| e.to_string_lossy().to_string())
}

/// Deletes the loads and stores of zero-sized types (an empty struct, a unit, a zero-length array) that the code
/// generator emits for values with no bytes. They touch no memory, but the sanitizers check the byte range
/// `[addr, addr + size - 1]`, which for a size of 0 is the byte before the object: AddressSanitizer reports a heap
/// overflow there for every boxed empty struct and then crashes describing it. A deleted load is replaced by
/// `undef`, which is every value of a type with no bytes.
fn remove_zero_sized_accesses(module: &Module<'_>, machine: &TargetMachine) {
    use inkwell::llvm_sys::core::{LLVMGetUndef, LLVMReplaceAllUsesWith, LLVMTypeOf};
    use inkwell::types::{AnyType, BasicType};
    use inkwell::values::{AsValueRef, InstructionOpcode};
    let data = machine.get_target_data();
    let zero_sized = |ty: inkwell::types::AnyTypeEnum<'_>| -> bool {
        inkwell::types::BasicTypeEnum::try_from(ty).is_ok_and(|ty| ty.is_sized() && data.get_store_size(&ty) == 0)
    };
    let mut dead = Vec::new();
    for function in module.get_functions() {
        for block in function.get_basic_blocks() {
            let mut next = block.get_first_instruction();
            while let Some(instruction) = next {
                next = instruction.get_next_instruction();
                let accessed = match instruction.get_opcode() {
                    InstructionOpcode::Load => Some(instruction.get_type()),
                    InstructionOpcode::Store => instruction
                        .get_operand(0)
                        .and_then(|operand| operand.value())
                        .map(|value| value.get_type().as_any_type_enum()),
                    _ => None,
                };
                if accessed.is_some_and(zero_sized) {
                    dead.push(instruction);
                }
            }
        }
    }
    for instruction in dead {
        if instruction.get_opcode() == InstructionOpcode::Load {
            let value = instruction.as_value_ref();
            // SAFETY: `value` is a live load whose type is a sized first-class type, so its `undef` exists and has
            // the same type as every use.
            unsafe { LLVMReplaceAllUsesWith(value, LLVMGetUndef(LLVMTypeOf(value))) };
        }
        instruction.erase_from_basic_block();
    }
}

/// The coverage libFuzzer reads:`-fsanitize=fuzzer`'s choice of inline 8-bit edge counters, the PC table, and
/// comparison tracing, set once per process through LLVM's command-line options.
fn set_coverage_options() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let flags = [
            "burdin",
            "-sanitizer-coverage-level=3",
            "-sanitizer-coverage-inline-8bit-counters",
            "-sanitizer-coverage-pc-table",
            "-sanitizer-coverage-trace-compares",
        ];
        let owned: Vec<std::ffi::CString> =
            flags.iter().map(|flag| std::ffi::CString::new(*flag).expect("no NUL in a literal")).collect();
        let argv: Vec<*const std::ffi::c_char> = owned.iter().map(|flag| flag.as_ptr()).collect();
        // SAFETY: `argv` holds NUL-terminated strings that outlive the call, and the overview is a null pointer.
        unsafe {
            inkwell::llvm_sys::support::LLVMParseCommandLineOptions(argv.len() as i32, argv.as_ptr(), std::ptr::null())
        };
    });
}

/// Selects the x86 assembly dialect through LLVM's own command-line option, which the x86 assembly printer reads
/// when the target machine is created. The option is process-wide, so it is set once, before the machine exists.
fn set_asm_syntax(syntax: AsmSyntax) -> Result<(), String> {
    let flag = match syntax {
        AsmSyntax::Default => return Ok(()),
        AsmSyntax::Intel => "-x86-asm-syntax=intel",
        AsmSyntax::Att => "-x86-asm-syntax=att",
    };
    if cfg!(not(target_arch = "x86_64")) {
        return Err("`--intel` and `--att` choose between the two x86 assembly dialects; this processor has only one"
            .to_string());
    }
    let program = std::ffi::CString::new("burdin").expect("no NUL in a literal");
    let option = std::ffi::CString::new(flag).expect("no NUL in a literal");
    let argv = [program.as_ptr(), option.as_ptr()];
    // SAFETY: `argv` holds two NUL-terminated strings that outlive the call, and the overview is a null pointer,
    // which LLVM accepts.
    unsafe { inkwell::llvm_sys::support::LLVMParseCommandLineOptions(2, argv.as_ptr(), std::ptr::null()) };
    Ok(())
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
