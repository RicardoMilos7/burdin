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
    /// A type alias with parameters: an index into [`Globals::generic_aliases`]. It is expanded where it is used.
    GenericAlias(usize),
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
    /// Extension methods visible in this module, by method name. Several entries under one name apply to
    /// different receiver types (`spec/extensions.md`).
    pub(crate) extensions: HashMap<String, Vec<usize>>,
    /// Package-private declarations, for E0150 when another package imports them.
    pub(crate) value_priv: HashMap<String, bool>,
    pub(crate) type_priv: HashMap<String, bool>,
    pub(crate) iface_priv: HashMap<String, bool>,
    /// The extension names this module declares, with the `priv` flag. An imported extension is in
    /// `extensions` but not here: like every other item, an import is not re-exported.
    pub(crate) extension_priv: HashMap<String, bool>,
    /// The derives visible in this module, by name: its own `derive` declarations and the ones it imported
    /// (`spec/reflection.md`, RFL8). A value indexes [`crate::session::Sema::derive_srcs`].
    pub(crate) derives: HashMap<String, usize>,
    /// The derive names this module declares, with the `priv` flag.
    pub(crate) derive_priv: HashMap<String, bool>,
    /// Names this module re-exports with `pub use`, in each namespace, and the module aliases it re-exports.
    pub(crate) pub_values: std::collections::HashSet<String>,
    pub(crate) pub_types: std::collections::HashSet<String>,
    pub(crate) pub_ifaces: std::collections::HashSet<String>,
    pub(crate) pub_modules: std::collections::HashSet<String>,
    pub(crate) pub_extensions: std::collections::HashSet<String>,
    pub(crate) pub_derives: std::collections::HashSet<String>,
}

/// One extension method declaration: `fn Type.name(self, ...)`.
#[derive(Clone)]
pub(crate) struct ExtensionInfo {
    /// The method name after the receiver type.
    pub(crate) name: String,
    /// The receiver type as written, with the declaration's type parameters as `Ty::Param`.
    pub(crate) recv: Ty,
    /// The module that declares the extension, whose names its receiver and body use.
    pub(crate) module: usize,
    /// Every type parameter of the declaration: the receiver's, then the method's own.
    pub(crate) params: Vec<String>,
    /// The parameters that occur in `recv`, in order. A call binds all of them from the receiver.
    pub(crate) recv_params: Vec<String>,
    /// The bounds of each parameter in `params`.
    pub(crate) bounds: Vec<Vec<crate::generics::Bound>>,
    /// The [`Globals::generic_fns`] template when the declaration has type parameters, and therefore is
    /// instantiated per call like a generic function.
    pub(crate) generic: Option<usize>,
    /// The one function of a non-generic extension, or the identity function of a generic one. `None` when the
    /// declaration is invalid and was reported.
    pub(crate) func: Option<FuncId>,
    /// The method name, where diagnostics point.
    pub(crate) span: burdin_diag::Span,
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
    /// A C variadic function: a call may pass extra C-ABI arguments.
    pub(crate) vararg: bool,
}

/// One `Type: From<Source>` block: the `from` method `?` calls to convert a `Source` error.
pub(crate) struct FromImpl {
    pub(crate) target: Ty,
    pub(crate) source: Ty,
    pub(crate) func: FuncId,
}

/// The standard library's operator interfaces (`spec/operators.md`, OP2). The compiler resolves an operator on a
/// user type to one of these; it never looks the name up in the program, so a module that declares its own `Add`
/// does not change what `+` means.
#[derive(Clone, Copy)]
pub(crate) struct OpIfaces {
    pub(crate) add: usize,
    pub(crate) sub: usize,
    pub(crate) mul: usize,
    pub(crate) div: usize,
    pub(crate) rem: usize,
    pub(crate) bitand: usize,
    pub(crate) bitor: usize,
    pub(crate) bitxor: usize,
    pub(crate) neg: usize,
    pub(crate) not: usize,
    pub(crate) partial_eq: usize,
    pub(crate) partial_ord: usize,
    pub(crate) index: usize,
    pub(crate) index_mut: usize,
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
    /// Whether each struct or enum (an instance included) is package-private.
    pub(crate) adt_priv: HashMap<(bool, usize), bool>,
    /// Every extension method declaration of the program, in declaration order.
    pub(crate) extensions: Vec<ExtensionInfo>,
    /// Methods by the type they belong to (`(false, struct)` or `(true, enum)`) and name.
    pub(crate) methods: HashMap<((bool, usize), String), FuncId>,
    /// Whether each method is package-private.
    pub(crate) method_priv: HashMap<((bool, usize), String), bool>,
    /// Where each method was declared, for signature diagnostics.
    pub(crate) method_spans: HashMap<((bool, usize), String), burdin_diag::Span>,
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
    /// The signatures of `Fn(...)` bounds, by the index a [`crate::generics::Bound::Fn`] carries.
    pub(crate) fn_bounds: RefCell<Vec<tir::FnSig>>,
    /// The type aliases that have parameters (`crate::aliases`) and how deeply their expansions nest right now.
    pub(crate) generic_aliases: Vec<crate::aliases::GenericAlias>,
    pub(crate) alias_depth: std::cell::Cell<usize>,
    /// The const expressions over const parameters that an array length or a const argument stands for while a
    /// generic body is checked, by the index `SYM_LEN`/`SYM_CONST` plus it (`crate::consteval`).
    pub(crate) sym_exprs: RefCell<Vec<crate::consteval::ConstExpr>>,
    /// The initializer of each constant and the module it is written in, by constant index: an array length may
    /// name a constant.
    pub(crate) const_inits: Vec<(usize, &'static burdin_syntax::ast::Expr)>,
    /// The files `embed` read, in the order they were first read: inputs of the build (`spec/comptime.md`, CT15).
    pub(crate) embedded: RefCell<Vec<String>>,
    /// The methods of `Type: From<..>` blocks: their one parameter is taken by ownership even though the
    /// declaration does not write `own`, because a conversion consumes its input.
    pub(crate) from_methods: Vec<FuncId>,
    /// Instances asked for while resolving types and checking calls, to be allocated before the next pass.
    pub(crate) requests: RefCell<Vec<Request>>,
    /// Every module of the embedded `std` package, where the compiler finds the iterator protocol's names.
    pub(crate) std_modules: Vec<usize>,
    /// The `std.Iterator<T>` and `std.Iterable<T>` interfaces, when the standard library declares them.
    pub(crate) iterator_iface: Option<usize>,
    pub(crate) iterable_iface: Option<usize>,
    /// The `std.FromIterator<T>` interface: what `Iterator.collect` builds its target through (`spec/iterators.md`, IT16).
    pub(crate) from_iterator_iface: Option<usize>,
    /// The `std.DoubleEndedIterator<T>` interface: an iterator that can also take items from its back (IT14).
    pub(crate) double_ended_iface: Option<usize>,
    /// The `std.Display` and `std.Debug` interfaces: what `{}` and `{:?}` need (spec/format.md).
    pub(crate) display_iface: Option<usize>,
    pub(crate) debug_iface: Option<usize>,
    /// The standard library's operator interfaces (`spec/operators.md`, OP2), when it declares them.
    pub(crate) op_ifaces: Option<OpIfaces>,
    /// The standard library's `Allocator` interface and its default allocator type `Global`
    /// (`spec/allocators.md`, AL2-AL3). A standard library that does not declare them leaves the built-in
    /// containers on the C heap only.
    pub(crate) allocator_iface: Option<usize>,
    pub(crate) global_alloc_ty: Option<Ty>,
    /// The `std.Error` interface (the marker that makes a type an error type) and the `std.Error` struct (the
    /// dynamic error value). Both are `None` when the standard library does not declare them (`spec/errors.md`,
    /// ERR4).
    pub(crate) error_iface: Option<usize>,
    pub(crate) error_ty: Option<Ty>,
    /// The standard library's dynamic-error constructors (`spec/errors.md`, ERR4 and ERR7).
    pub(crate) error_plain: Option<FuncId>,
    pub(crate) error_at: Option<FuncId>,
    pub(crate) error_frame: Option<FuncId>,
    pub(crate) error_context: Option<FuncId>,
    /// The typed `assert` and `assert_eq` of a `burdin test` build (`spec/errors.md`, ERR9).
    pub(crate) test_assert: Option<FuncId>,
    pub(crate) test_assert_eq: Option<usize>,
    /// The standard library's error types for its own primitives (`spec/errors.md`, ERR10-ERR11).
    pub(crate) io_error_kind_ty: Option<Ty>,
    pub(crate) parse_int_error_ty: Option<Ty>,
    /// The generic method template each registered generic method stands for.
    pub(crate) method_templates: HashMap<FuncId, crate::generics::GenericMethod>,
    /// The instance of each generic method for its method-level type arguments.
    pub(crate) method_instances: HashMap<(usize, Vec<Ty>), FuncId>,
    /// The lazily created default-method templates of interface methods for built-in receiver types.
    pub(crate) builtin_method_templates: HashMap<(usize, usize, Ty, Vec<Ty>), crate::generics::GenericMethod>,
    /// The `any Interface` instances, vtables, and type identities (`any.rs`).
    pub(crate) any: RefCell<crate::any::AnyState>,
    /// The concrete allocator types a container in the program stores (`spec/allocators.md`, AL4), collected as
    /// types are resolved; `finish` turns them into `Program::allocator_fns`.
    pub(crate) allocator_requests: RefCell<Vec<Ty>>,
}

pub(crate) fn adt_key(ty: &Ty) -> Option<(bool, usize)> {
    match ty {
        Ty::Struct(a) => Some((false, a.id)),
        Ty::Enum(a) => Some((true, a.id)),
        _ => None,
    }
}

impl Globals {
    /// The allocator a container stores (`spec/allocators.md`, AL3). `Global` and the absence of a standard
    /// library's default allocator both mean the default: no value is stored.
    pub(crate) fn alloc_of(&self, ty: Ty) -> burdin_tir::Alloc {
        match &self.global_alloc_ty {
            Some(default) if *default == ty => burdin_tir::Alloc::Global,
            _ => {
                if !self.ty_has_param(&ty) {
                    let mut requests = self.allocator_requests.borrow_mut();
                    if !requests.contains(&ty) {
                        requests.push(ty.clone());
                    }
                }
                burdin_tir::Alloc::Value(Box::new(ty))
            }
        }
    }

    /// The `Allocator` functions of every stored allocator type the program uses, for code generation.
    pub(crate) fn allocator_fns(&self) -> Vec<burdin_tir::AllocatorFns> {
        let mut out = Vec::new();
        for ty in self.allocator_requests.borrow().iter() {
            let Some(key) = adt_key(ty) else { continue };
            let method = |name: &str| self.methods.get(&(key, name.to_string())).copied();
            if let (Some(allocate), Some(reallocate), Some(deallocate)) =
                (method("allocate"), method("reallocate"), method("deallocate"))
            {
                out.push(burdin_tir::AllocatorFns { ty: ty.clone(), allocate, reallocate, deallocate });
            }
        }
        out
    }

    /// The allocator of a container after substituting type parameters. The result is canonicalized, so a
    /// parameter that becomes `Global` turns back into the default allocator.
    pub(crate) fn subst_alloc(&self, alloc: &burdin_tir::Alloc, env: &[(String, Ty)]) -> burdin_tir::Alloc {
        match alloc {
            burdin_tir::Alloc::Global => burdin_tir::Alloc::Global,
            burdin_tir::Alloc::Value(t) => self.alloc_of(self.subst_ty(t, env)),
        }
    }

    /// Whether `ty` implements `Allocator`, with the bounds of the type parameters in `env`. A type parameter
    /// needs the bound; concrete types need a visible impl block.
    pub(crate) fn is_allocator(&self, ty: &Ty, env: &[(String, Vec<crate::generics::Bound>)]) -> bool {
        let Some(iface) = self.allocator_iface else { return true };
        self.satisfies_with(ty, &crate::generics::Bound::User(iface, Vec::new()), env, &[])
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

    /// The value `name` if the module declares it or re-exports it with `pub use`: plain imports are private to
    /// their module.
    pub(crate) fn exported_value(&self, module: usize, name: &str) -> Option<Global> {
        let info = &self.modules[module];
        if !info.value_priv.contains_key(name) && !info.pub_values.contains(name) {
            return None;
        }
        self.value(module, name)
    }

    /// The type `name` if the module declares or re-exports it.
    pub(crate) fn exported_type(&self, module: usize, name: &str) -> Option<TypeName> {
        let info = &self.modules[module];
        if !info.type_priv.contains_key(name) && !info.pub_types.contains(name) {
            return None;
        }
        self.type_name(module, name)
    }

    /// The interface `name` if the module declares or re-exports it.
    pub(crate) fn exported_iface(&self, module: usize, name: &str) -> Option<usize> {
        let info = &self.modules[module];
        if !info.iface_priv.contains_key(name) && !info.pub_ifaces.contains(name) {
            return None;
        }
        self.iface_name(module, name)
    }

    /// The extensions named `name` if the module declares or re-exports them. A name may have several
    /// declarations, one per receiver type.
    pub(crate) fn exported_extension(&self, module: usize, name: &str) -> Option<Vec<usize>> {
        let info = &self.modules[module];
        if !info.extension_priv.contains_key(name) && !info.pub_extensions.contains(name) {
            return None;
        }
        info.extensions.get(name).cloned()
    }

    /// The derive `name` if the module declares or re-exports it.
    pub(crate) fn exported_derive(&self, module: usize, name: &str) -> Option<usize> {
        let info = &self.modules[module];
        if !info.derive_priv.contains_key(name) && !info.pub_derives.contains(name) {
            return None;
        }
        info.derives.get(name).copied()
    }

    /// The struct `name` of the standard library, as a type.
    pub(crate) fn std_struct_ty(&self, name: &str) -> Option<Ty> {
        self.std_modules.iter().find_map(|&module| match self.type_name(module, name)? {
            TypeName::Struct(i) => Some(Ty::Struct(self.struct_refs[i].clone())),
            _ => None,
        })
    }

    /// The struct or enum `name` of the standard library, as a type.
    pub(crate) fn std_adt_ty(&self, name: &str) -> Option<Ty> {
        self.std_modules.iter().find_map(|&module| match self.type_name(module, name)? {
            TypeName::Struct(i) => Some(Ty::Struct(self.struct_refs[i].clone())),
            TypeName::Enum(i) => Some(Ty::Enum(self.enum_refs[i].clone())),
            _ => None,
        })
    }

    /// Whether `ty` is the dynamic error value `std.Error` (`spec/errors.md`, ERR4).
    pub(crate) fn is_error_ty(&self, ty: &Ty) -> bool {
        self.error_ty.as_ref().is_some_and(|error| error == ty)
    }

    /// Whether `ty` is an error type: it implements the standard library's `Error` interface, which requires
    /// `Display` (`spec/errors.md`, ERR4).
    pub(crate) fn implements_error(&self, ty: &Ty, env: &crate::generics::Env) -> bool {
        match self.error_iface {
            Some(iface) => self.satisfies(ty, &crate::generics::Bound::User(iface, Vec::new()), env),
            None => false,
        }
    }

    /// The non-generic function `name` of the standard library (the formatting helpers are `priv`, so the
    /// compiler finds them here rather than through the prelude).
    pub(crate) fn std_func(&self, name: &str) -> Option<FuncId> {
        self.std_modules.iter().find_map(|&module| match self.value(module, name)? {
            Global::Func(f) => Some(f),
            _ => None,
        })
    }

    /// The generic function `name` of the standard library.
    pub(crate) fn std_value(&self, name: &str) -> Option<Global> {
        self.std_modules.iter().find_map(|&module| self.value(module, name))
    }

    /// The generic function `name` declared in the standard library, if any.
    pub(crate) fn std_generic(&self, name: &str) -> Option<usize> {
        self.std_modules.iter().find_map(|&module| match self.value(module, name)? {
            Global::Generic(g) => Some(g),
            _ => None,
        })
    }

    /// The generic function behind an iterator adapter that needs a particular kind of source (`it.flatten()`,
    /// `it.cloned()`, `it.copied()`, `it.cycle()`), looked up in the standard library (spec/iterators.md, IT13).
    pub(crate) fn iterator_adapter_fn(&self, name: &str) -> Option<usize> {
        if !matches!(name, "flatten" | "cloned" | "copied" | "cycle" | "rev") {
            return None;
        }
        self.std_generic(&format!("__iter_{name}"))
    }

    /// The generic function behind an eager `List` method (`xs.map(f)`), looked up in the standard library.
    pub(crate) fn eager_list_fn(&self, name: &str) -> Option<usize> {
        if !matches!(name, "map" | "filter" | "fold" | "any" | "all" | "find") {
            return None;
        }
        let name = format!("__list_{name}");
        self.std_modules.iter().find_map(|&module| match self.value(module, &name)? {
            Global::Generic(g) => Some(g),
            _ => None,
        })
    }

    /// The generic function in `std` behind an iterator consumer that needs a bound on the item type (`min`, `max`,
    /// `eq`, `cmp`), which an interface default method cannot state.
    pub(crate) fn iterator_item_fn(&self, name: &str) -> Option<usize> {
        if !matches!(name, "min" | "max" | "eq" | "cmp") {
            return None;
        }
        let name = format!("__iter_{name}");
        self.std_modules.iter().find_map(|&module| match self.value(module, &name)? {
            Global::Generic(g) => Some(g),
            _ => None,
        })
    }

    /// The first package-private struct or enum a type mentions, for the public-signature rule (E0150).
    pub(crate) fn private_adt(&self, ty: &Ty) -> Option<String> {
        match ty {
            Ty::Struct(a) => {
                if self.adt_priv.get(&(false, a.id)).copied().unwrap_or(false) {
                    Some(a.name.to_string())
                } else {
                    None
                }
            }
            Ty::Enum(a) => {
                if self.adt_priv.get(&(true, a.id)).copied().unwrap_or(false) {
                    Some(a.name.to_string())
                } else {
                    None
                }
            }
            Ty::Ref { inner, .. }
            | Ty::Slice(inner)
            | Ty::Array(inner, _)
            | Ty::Option(inner)
            | Ty::Heap(inner, _)
            | Ty::CPtr(inner)
            | Ty::Range(inner, _)
            | Ty::List(inner, _) => self.private_adt(inner),
            Ty::Result(t, e) => self.private_adt(t).or_else(|| self.private_adt(e)),
            Ty::Map(k, v, _) | Ty::MapEntry(k, v, _) => self.private_adt(k).or_else(|| self.private_adt(v)),
            Ty::Fn { sig, .. } => {
                sig.params.iter().find_map(|t| self.private_adt(t)).or_else(|| self.private_adt(&sig.ret))
            }
            Ty::Tuple(ts) => ts.iter().find_map(|t| self.private_adt(t)),
            _ => None,
        }
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
            let module_package = self.modules[module].package;
            if module_package != self.package_of(from) && path.len() > 1 {
                let name = &path[1];
                let target = *self.modules[module].modules.get(name)?;
                if !self.modules[module].pub_modules.contains(name) {
                    return None;
                }
                let mut full = self.modules[target].path.clone();
                full.extend_from_slice(&path[2..]);
                return self.find_module_in_package(module_package, &full);
            }
            let mut full = self.modules[module].path.clone();
            full.extend_from_slice(&path[1..]);
            return self.find_module_in_package(module_package, &full);
        }
        let package = self.package_of(from);
        if let Some(module) = self.find_module_in_package(package, path) {
            return Some(module);
        }
        let dep = *self.packages[package].deps.get(first)?;
        if path.len() == 1 {
            return self.packages[dep].root_module;
        }
        // A dependency's non-root modules are private: only the aliases its root module re-exports with
        // `pub use <module>` are reachable (issue #81).
        let root = self.packages[dep].root_module?;
        let name = &path[1];
        let target = *self.modules[root].modules.get(name)?;
        if !self.modules[root].pub_modules.contains(name) {
            return None;
        }
        let mut full = self.modules[target].path.clone();
        full.extend_from_slice(&path[2..]);
        self.find_module_in_package(dep, &full)
    }
}
