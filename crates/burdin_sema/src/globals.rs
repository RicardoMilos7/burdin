//! What the checker knows about the program's declarations: modules, packages, names, types, signatures, and
//! generics.
//!
//! Every file is a module. Names are resolved in the module that writes them, so two modules may declare the same
//! simple name; internally every item is displayed with a qualified name (`math.add`, `lib.Gadget`) so that
//! instances and LLVM symbols stay unique. Imports are copied into the importing module's maps by `imports.rs`.

use crate::generics::{GenericAdt, GenericFn, ImplInfo, InterfaceInfo, Request};
use burdin_diag::FileId;
use burdin_tir::{self as tir, AdtRef, Builtin, ConstId, FuncId, Signature, Ty};
use std::cell::RefCell;
use std::collections::HashMap;

pub(crate) enum Callee {
    User(FuncId),
    Builtin(Builtin),
}

#[derive(Clone, Copy)]
pub(crate) enum Global {
    Func(FuncId),
    Const(ConstId),
    /// A generic function: an index into [`Globals::generic_fns`].
    Generic(usize),
    /// An `extern "C"` declaration: an index into [`Globals::externs`].
    Extern(usize),
}

#[derive(Clone, Copy)]
pub(crate) enum TypeName {
    Struct(usize),
    Enum(usize),
    Alias(usize),
    /// A generic struct or enum: an index into [`Globals::generic_adts`]. It is not a type until it has arguments.
    GenericAdt(usize),
}

/// One file of a package, and everything its items bind in its own namespace.
pub(crate) struct ModuleInfo {
    pub(crate) package: usize,
    /// The file's path inside the package's `src` (for example `["geometry", "shapes"]`). A single-file target
    /// has an empty path.
    pub(crate) path: Vec<String>,
    pub(crate) file: FileId,
    pub(crate) values: HashMap<String, Global>,
    pub(crate) types: HashMap<String, TypeName>,
    pub(crate) ifaces: HashMap<String, usize>,
    /// Modules imported with `use`, by the name they are used with.
    pub(crate) modules: HashMap<String, usize>,
    /// Package-private declarations, for E0150 when another package imports them.
    pub(crate) value_priv: HashMap<String, bool>,
    pub(crate) type_priv: HashMap<String, bool>,
    pub(crate) iface_priv: HashMap<String, bool>,
}

/// One package of the compilation: the package being built and its dependencies.
pub(crate) struct PackageInfo {
    pub(crate) name: String,
    pub(crate) is_root: bool,
    /// The packages this one may import from, by the name it uses for them.
    pub(crate) deps: HashMap<String, usize>,
    /// The module other packages reach as `use <package>` (the package's `src/<name>.bd` or `src/lib.bd`).
    pub(crate) root_module: Option<usize>,
}

/// One `extern "C"` function. Its parameters are always passed by value.
pub(crate) struct ExternInfo {
    pub(crate) symbol: String,
    pub(crate) sig: Signature,
}

/// One `Type: From<Source>` block: the `from` method `?` calls to convert a `Source` error.
pub(crate) struct FromImpl {
    pub(crate) target: Ty,
    pub(crate) source: Ty,
    pub(crate) func: FuncId,
}

#[derive(Default)]
pub(crate) struct Globals {
    pub(crate) modules: Vec<ModuleInfo>,
    pub(crate) packages: Vec<PackageInfo>,
    /// `(package, module path joined with '.')` to the module index.
    pub(crate) module_index: HashMap<(usize, String), usize>,
    pub(crate) aliases: Vec<Ty>,
    pub(crate) structs: Vec<tir::StructDef>,
    pub(crate) enums: Vec<tir::EnumDef>,
    pub(crate) struct_refs: Vec<AdtRef>,
    pub(crate) enum_refs: Vec<AdtRef>,
    /// The package that declares each struct or enum, for field visibility (E0150).
    pub(crate) struct_pkg: Vec<usize>,
    pub(crate) enum_pkg: Vec<usize>,
    /// Methods by the type they belong to (`(false, struct)` or `(true, enum)`) and name.
    pub(crate) methods: HashMap<((bool, usize), String), FuncId>,
    /// Whether each method is package-private.
    pub(crate) method_priv: HashMap<((bool, usize), String), bool>,
    /// The package of the impl block or interface that defines each method, for visibility.
    pub(crate) method_pkg: HashMap<((bool, usize), String), usize>,
    /// The `Drop::drop` method of a type that has a `Type: Drop` block.
    pub(crate) drop_methods: HashMap<(bool, usize), FuncId>,
    pub(crate) sigs: Vec<Signature>,
    pub(crate) const_tys: Vec<Ty>,
    /// Whether each function is `unsafe` (`unsafe fn`), by function index.
    pub(crate) fn_unsafe: Vec<bool>,
    // Interfaces and generics (see `generics.rs`).
    pub(crate) interfaces: Vec<InterfaceInfo>,
    pub(crate) generic_adts: Vec<GenericAdt>,
    pub(crate) generic_fns: Vec<GenericFn>,
    pub(crate) impls: Vec<ImplInfo>,
    /// The struct or enum for a generic type and its arguments.
    pub(crate) adt_instances: HashMap<(usize, Vec<Ty>), (bool, usize)>,
    /// The other way around: the generic type and arguments of an instance.
    pub(crate) adt_origin: HashMap<(bool, usize), (usize, Vec<Ty>)>,
    pub(crate) fn_instances: HashMap<(usize, Vec<Ty>), FuncId>,
    /// `extern "C"` declarations, by symbol (a symbol is declared once).
    pub(crate) externs: Vec<ExternInfo>,
    pub(crate) extern_symbols: HashMap<String, usize>,
    /// `Type: From<Source>` conversions, for `?`.
    pub(crate) from_impls: Vec<FromImpl>,
    /// The methods of `Type: From<..>` blocks: their one parameter is taken by ownership even though the
    /// declaration does not write `own`, because a conversion consumes its input.
    pub(crate) from_methods: Vec<FuncId>,
    /// Instances asked for while resolving types and checking calls, to be allocated before the next pass.
    pub(crate) requests: RefCell<Vec<Request>>,
}

pub(crate) fn adt_key(ty: &Ty) -> Option<(bool, usize)> {
    match ty {
        Ty::Struct(a) => Some((false, a.id)),
        Ty::Enum(a) => Some((true, a.id)),
        _ => None,
    }
}

impl Globals {
    pub(crate) fn package_of(&self, module: usize) -> usize {
        self.modules[module].package
    }

    /// The value name `name` as seen from `module`, after imports.
    pub(crate) fn value(&self, module: usize, name: &str) -> Option<Global> {
        self.modules[module].values.get(name).copied()
    }

    /// The type name `name` as seen from `module`, after imports.
    pub(crate) fn type_name(&self, module: usize, name: &str) -> Option<TypeName> {
        self.modules[module].types.get(name).copied()
    }

    /// The interface name `name` as seen from `module`, after imports.
    pub(crate) fn iface_name(&self, module: usize, name: &str) -> Option<usize> {
        self.modules[module].ifaces.get(name).copied()
    }

    /// Whether a module alias `name` is in scope in `module`.
    pub(crate) fn module_alias(&self, module: usize, name: &str) -> Option<usize> {
        self.modules[module].modules.get(name).copied()
    }

    /// The value `name` if the module declares it: imports are private to their module and are not re-exported.
    pub(crate) fn exported_value(&self, module: usize, name: &str) -> Option<Global> {
        if !self.modules[module].value_priv.contains_key(name) {
            return None;
        }
        self.value(module, name)
    }

    /// The type `name` if the module declares it.
    pub(crate) fn exported_type(&self, module: usize, name: &str) -> Option<TypeName> {
        if !self.modules[module].type_priv.contains_key(name) {
            return None;
        }
        self.type_name(module, name)
    }

    /// The interface `name` if the module declares it.
    pub(crate) fn exported_iface(&self, module: usize, name: &str) -> Option<usize> {
        if !self.modules[module].iface_priv.contains_key(name) {
            return None;
        }
        self.iface_name(module, name)
    }

    /// Whether a value name is package-private where it is declared.
    pub(crate) fn value_is_private(&self, module: usize, name: &str) -> bool {
        self.modules[module].value_priv.get(name).copied().unwrap_or(false)
    }

    /// Whether a type name is package-private where it is declared.
    pub(crate) fn type_is_private(&self, module: usize, name: &str) -> bool {
        self.modules[module].type_priv.get(name).copied().unwrap_or(false)
    }

    /// How a module is written in qualified names and diagnostics.
    pub(crate) fn module_display(&self, module: usize) -> String {
        let info = &self.modules[module];
        let pkg = &self.packages[info.package];
        let path = info.path.join(".");
        if pkg.is_root {
            path
        } else if pkg.root_module == Some(module) || path.is_empty() {
            pkg.name.clone()
        } else {
            format!("{}.{path}", pkg.name)
        }
    }

    /// The internally unique name of an item: `add` in a single-file target, `math.add`, `lib.Gadget`.
    pub(crate) fn qualified(&self, module: usize, name: &str) -> String {
        match self.module_display(module) {
            s if s.is_empty() => name.to_string(),
            s => format!("{s}.{name}"),
        }
    }

    /// The user's `Type: From<Source>` conversion from `source` into `target`, if there is one.
    pub(crate) fn conversion(&self, source: &Ty, target: &Ty) -> Option<FuncId> {
        self.from_impls.iter().find(|f| &f.source == source && &f.target == target).map(|f| f.func)
    }

    /// The module of `package` whose path is exactly `path`.
    pub(crate) fn find_module_in_package(&self, package: usize, path: &[String]) -> Option<usize> {
        self.module_index.get(&(package, path.join("."))).copied()
    }

    /// Resolves a dotted module path from `from`, trying its own package first and then the package names it
    /// depends on. A module imported with `use` may start the path.
    pub(crate) fn resolve_module_path(&self, from: usize, path: &[String]) -> Option<usize> {
        let Some(first) = path.first() else { return self.find_module_in_package(self.package_of(from), path) };
        if let Some(module) = self.module_alias(from, first) {
            let mut full = self.modules[module].path.clone();
            full.extend_from_slice(&path[1..]);
            return self.find_module_in_package(self.modules[module].package, &full);
        }
        let package = self.package_of(from);
        if let Some(module) = self.find_module_in_package(package, path) {
            return Some(module);
        }
        let dep = *self.packages[package].deps.get(first)?;
        if path.len() == 1 {
            return self.packages[dep].root_module;
        }
        self.find_module_in_package(dep, &path[1..])
    }
}
