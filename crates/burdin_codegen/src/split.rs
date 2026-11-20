//! Parallel code generation (issue #334): the finished module is split into codegen units, and each unit is
//! optimized and compiled to its own object file on its own thread.
//!
//! The typed tree is lowered to one LLVM module first, on one thread; that part is a small share of a build. The
//! expensive part is LLVM's optimizer and instruction selection, and those work on one module at a time, so the
//! module is cut into units the way LLVM's own `SplitModule` does it:
//!
//! 1. Unreferenced internal definitions are already gone (`emit_object` runs `globaldce` on every module), so a
//!    unit does not keep what the whole program never used.
//! 2. Every internal or private definition another unit could refer to is made external with hidden visibility and
//!    renamed `burdin.cgu.<name>`, so it is defined once in the linked program and cannot collide with a library
//!    symbol. A private `unnamed_addr` constant (a string literal, a vtable) has no identity, so every unit keeps its
//!    own copy instead, and the units that do not use it drop it.
//! 3. Each function with a body is owned by exactly one unit. The units are contiguous runs of the module's
//!    functions in module order, balanced by instruction count; global variables, the module's inline assembly,
//!    and the appending globals (`llvm.global_ctors`) belong to the first unit.
//! 4. Each unit starts from a copy of the whole module (written once as bitcode, read once per unit) and turns what
//!    it does not own into declarations. In a release build a small function owned elsewhere stays visible as an
//!    `available_externally` definition until the optimizer has had the chance to inline it, so splitting costs
//!    little run-time speed.
//!
//! The number of units depends only on the program, never on the number of threads, so `-j 1` and `-j 16` write the
//! same objects and link the same executable. The objects are returned in unit order.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use inkwell::GlobalVisibility;
use inkwell::context::Context;
use inkwell::memory_buffer::MemoryBuffer;
use inkwell::module::{Linkage, Module};
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::FileType;
use inkwell::values::FunctionValue;

use crate::OptLevel;

/// About this many instructions make one unit: below it, a second thread costs more (reading the bitcode, and a
/// second copy of the runtime's constants) than it saves. Measured on a generated 16-module package of 65,000
/// instructions, where 8 units built fastest in both profiles.
pub const UNIT_INSTRUCTIONS: usize = 8_000;
/// The most units a program is split into, however large it is.
pub const MAX_UNITS: usize = 16;
/// A function this small (in instructions) owned by another unit stays inlinable in a release build.
const IMPORT_INSTRUCTIONS: usize = 40;
/// LLVM's passes recurse over the IR; a unit's thread gets the stack the compiler thread has.
const UNIT_STACK: usize = 256 << 20;

/// The number of units for a module of `instructions` instructions in `functions` defined functions. `requested`
/// (from `BURDIN_CODEGEN_UNITS`) overrides the size rule, still at most one unit per function.
pub fn unit_count(instructions: usize, functions: usize, requested: Option<usize>) -> usize {
    let wanted = match requested {
        Some(n) => n,
        None => instructions / UNIT_INSTRUCTIONS,
    };
    wanted.clamp(1, MAX_UNITS.max(requested.unwrap_or(0))).min(functions.max(1))
}

/// Assigns each function (by its weight, in module order) to one of `units` contiguous runs of roughly equal total
/// weight. Every unit gets at least one function when there are enough functions.
pub fn assign(weights: &[usize], units: usize) -> Vec<usize> {
    let units = units.clamp(1, weights.len().max(1));
    let total: usize = weights.iter().map(|w| w.max(&1)).sum();
    let mut owners = Vec::with_capacity(weights.len());
    let mut unit = 0;
    let mut seen = 0;
    for (index, weight) in weights.iter().enumerate() {
        // Move to the next unit once this one has its share (and is not empty), or when every later unit still
        // needs a function and only that many are left.
        let remaining_functions = weights.len() - index;
        let later_units = units - unit - 1;
        let has_share = seen * units >= (unit + 1) * total;
        if later_units > 0 && owners.last() == Some(&unit) && (has_share || remaining_functions <= later_units) {
            unit += 1;
        }
        owners.push(unit);
        seen += weight.max(&1);
    }
    owners
}

/// The instructions in `function`'s body.
fn instruction_count(function: FunctionValue<'_>) -> usize {
    function.get_basic_blocks().into_iter().map(|block| block.get_instructions().count()).sum()
}

/// What every unit needs to know about the split: who owns each function, and which functions are small enough to
/// import.
struct Plan {
    units: usize,
    owners: HashMap<Vec<u8>, usize>,
    small: HashSet<Vec<u8>>,
}

/// The weights of the module's defined functions, in module order. Used to choose the unit count before the module
/// is changed.
pub(crate) fn measure(module: &Module<'_>) -> (usize, usize) {
    let mut instructions = 0;
    let mut functions = 0;
    for function in module.get_functions() {
        if function.count_basic_blocks() > 0 {
            instructions += instruction_count(function);
            functions += 1;
        }
    }
    (instructions, functions)
}

/// Prepares `module` for splitting into `units` units (steps 1-3 above) and returns the plan.
fn prepare(module: &Module<'_>, units: usize) -> Plan {
    let mut anonymous = 0usize;
    let mut rename = |name: &[u8]| -> String {
        if name.is_empty() {
            anonymous += 1;
            format!("burdin.cgu.anon.{anonymous}")
        } else {
            format!("burdin.cgu.{}", String::from_utf8_lossy(name))
        }
    };
    let mut defined = Vec::new();
    for function in module.get_functions() {
        if function.count_basic_blocks() == 0 {
            continue;
        }
        let global = function.as_global_value();
        if matches!(global.get_linkage(), Linkage::Internal | Linkage::Private) {
            global.set_name(&rename(global.get_name().to_bytes()));
            global.set_linkage(Linkage::External);
            global.set_visibility(GlobalVisibility::Hidden);
        }
        defined.push(function);
    }
    for global in module.get_globals() {
        if global.is_declaration() || global.get_linkage() == Linkage::Appending {
            continue;
        }
        let local = matches!(global.get_linkage(), Linkage::Internal | Linkage::Private);
        let copyable = local && global.is_constant() && global.has_unnamed_addr();
        if local && !copyable {
            global.set_name(&rename(global.get_name().to_bytes()));
            global.set_linkage(Linkage::External);
            global.set_visibility(GlobalVisibility::Hidden);
        }
    }
    let weights: Vec<usize> = defined.iter().map(|f| instruction_count(*f)).collect();
    let assigned = assign(&weights, units);
    let mut owners = HashMap::new();
    let mut small = HashSet::new();
    for ((function, unit), weight) in defined.iter().zip(&assigned).zip(&weights) {
        let name = function.get_name().to_bytes().to_vec();
        if *weight <= IMPORT_INSTRUCTIONS {
            small.insert(name.clone());
        }
        owners.insert(name, *unit);
    }
    let units = assigned.iter().copied().max().map_or(1, |last| last + 1);
    Plan { units, owners, small }
}

/// The path of unit `index`'s object: `object` itself for the first, `<stem>.<index>.<ext>` for the others.
pub(crate) fn unit_path(object: &Path, index: usize) -> PathBuf {
    if index == 0 {
        return object.to_path_buf();
    }
    let stem = object.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let name = match object.extension() {
        Some(ext) => format!("{stem}.{index}.{}", ext.to_string_lossy()),
        None => format!("{stem}.{index}"),
    };
    object.with_file_name(name)
}

/// What one codegen unit leaves behind: whether it compiled, and how long it took.
type UnitResult = (Result<(), String>, Duration);

/// Splits `module` into `units` units and writes one object per unit with `jobs` threads. Returns the objects in unit
/// order, and how long each unit took in the same order. On failure the error of the first failing unit (in unit
/// order) is returned.
pub(crate) fn emit_units(
    module: &Module<'_>,
    units: usize,
    jobs: usize,
    level: OptLevel,
    object: &Path,
) -> Result<(Vec<PathBuf>, Vec<Duration>), String> {
    let plan = prepare(module, units);
    let bitcode = module.write_bitcode_to_memory();
    let bitcode = bitcode.as_slice();
    let paths: Vec<PathBuf> = (0..plan.units).map(|index| unit_path(object, index)).collect();
    let results: Mutex<Vec<Option<UnitResult>>> = Mutex::new(vec![None; plan.units]);
    let next = AtomicUsize::new(0);
    let workers = jobs.clamp(1, plan.units);
    std::thread::scope(|scope| -> Result<(), String> {
        let mut handles = Vec::new();
        for worker in 0..workers {
            let handle = std::thread::Builder::new()
                .name(format!("burdin-cgu-{worker}"))
                .stack_size(UNIT_STACK)
                .spawn_scoped(scope, || {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        if index >= plan.units {
                            break;
                        }
                        let started = Instant::now();
                        let result = emit_unit(bitcode, &plan, index, level, &paths[index]);
                        let took = started.elapsed();
                        results.lock().expect("a unit thread panicked")[index] = Some((result, took));
                    }
                })
                .map_err(|e| format!("cannot start a code generation thread: {e}"))?;
            handles.push(handle);
        }
        for handle in handles {
            if handle.join().is_err() {
                return Err("a code generation thread panicked".to_string());
            }
        }
        Ok(())
    })?;
    let mut times = Vec::with_capacity(plan.units);
    for entry in results.into_inner().expect("a unit thread panicked") {
        let (result, took) = entry.unwrap_or((Err("a codegen unit was not compiled".to_string()), Duration::ZERO));
        result?;
        times.push(took);
    }
    Ok((paths, times))
}

/// Compiles unit `index` from the whole module's `bitcode` into `path`.
fn emit_unit(bitcode: &[u8], plan: &Plan, index: usize, level: OptLevel, path: &Path) -> Result<(), String> {
    let context = Context::create();
    let buffer = MemoryBuffer::create_from_memory_range(bitcode, "burdin-unit");
    let module = Module::parse_bitcode_from_buffer(&buffer, &context).map_err(|e| e.to_string())?;
    let machine = crate::target_machine(level)?;
    let mut imports = Vec::new();
    for function in module.get_functions() {
        if function.count_basic_blocks() == 0 {
            continue;
        }
        let name = function.get_name().to_bytes();
        let owner = plan.owners.get(name).copied().unwrap_or(0);
        if owner == index {
            continue;
        }
        if level.optimized() && plan.small.contains(name) {
            imports.push(function);
        } else {
            function.as_global_value().set_linkage(Linkage::AvailableExternally);
        }
    }
    let mut appending = Vec::new();
    for global in module.get_globals() {
        if global.is_declaration() || index == 0 {
            continue;
        }
        match global.get_linkage() {
            Linkage::Appending => appending.push(global),
            Linkage::External => global.set_linkage(Linkage::AvailableExternally),
            _ => {}
        }
    }
    for global in appending {
        // SAFETY: an appending global (`llvm.global_ctors`, `llvm.used`) has no uses; the first unit keeps it.
        unsafe { global.delete() };
    }
    if index != 0 {
        module.set_inline_assembly("");
    }
    // What another unit defines becomes a declaration, and what this unit no longer uses goes.
    module
        .run_passes("elim-avail-extern,globaldce", &machine, PassBuilderOptions::create())
        .map_err(|e| e.to_string_lossy().to_string())?;
    if level.optimized() {
        for function in imports {
            function.as_global_value().set_linkage(Linkage::AvailableExternally);
        }
    }
    module
        .verify()
        .map_err(|e| format!("LLVM verification of codegen unit {index} failed:\n{}", e.to_string_lossy()))?;
    if level.optimized() {
        // The imports this unit never calls go before the optimizer spends time on them.
        let pipeline = format!("globaldce,{}", level.pipeline());
        module
            .run_passes(&pipeline, &machine, PassBuilderOptions::create())
            .map_err(|e| e.to_string_lossy().to_string())?;
    }
    machine.write_to_file(&module, FileType::Object, path).map_err(|e| e.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_programs_stay_in_one_unit() {
        assert_eq!(unit_count(0, 0, None), 1);
        assert_eq!(unit_count(UNIT_INSTRUCTIONS - 1, 500, None), 1);
        assert_eq!(unit_count(UNIT_INSTRUCTIONS * 3, 500, None), 3);
        assert_eq!(unit_count(UNIT_INSTRUCTIONS * 100, 500, None), MAX_UNITS);
    }

    #[test]
    fn a_request_overrides_the_size_rule_but_not_the_function_count() {
        assert_eq!(unit_count(10, 500, Some(4)), 4);
        assert_eq!(unit_count(10, 3, Some(8)), 3);
        assert_eq!(unit_count(10, 500, Some(1)), 1);
        assert_eq!(unit_count(10, 500, Some(32)), 32);
    }

    #[test]
    fn units_are_contiguous_balanced_and_never_empty() {
        let owners = assign(&[10, 10, 10, 10], 2);
        assert_eq!(owners, vec![0, 0, 1, 1]);
        let owners = assign(&[100, 1, 1, 1], 4);
        assert_eq!(owners, vec![0, 1, 2, 3]);
        let owners = assign(&[1; 10], 3);
        assert!(owners.windows(2).all(|w| w[1] == w[0] || w[1] == w[0] + 1), "{owners:?}");
        assert_eq!(*owners.last().unwrap(), 2);
        assert_eq!(assign(&[5, 5], 8), vec![0, 1]);
        assert_eq!(assign(&[], 4), Vec::<usize>::new());
    }
}
