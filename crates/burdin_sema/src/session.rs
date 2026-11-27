//! The state of one run of the checker: the modules of the program, what is declared, which generic items exist,
//! and which functions are waiting to be checked. The phases that fill and use it are in `declare`, `imports`,
//! `impls`, `instances`, `resolve`, and `bodies`.

use crate::cache::CheckCache;
use crate::consts::check_const_cycles;
use crate::derives::validate_derives;
use crate::generics::Bound;
use crate::globals::{Global, Globals, ModuleInfo, PackageInfo};
use crate::validate::check_public_signatures;
use burdin_diag::{Code, Diagnostics, FileId, SourceMap, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, FuncId, Signature, Ty};

/// A function to check: a top-level function or a method, with the type `Self` stands for.
pub(crate) struct FnItem<'a> {
    pub(crate) decl: &'a ast::FnDecl,
    pub(crate) name: String,
    /// The module whose names the body and signature resolve.
    pub(crate) module: usize,
    pub(crate) self_ty: Option<Ty>,
    /// What the type parameters stand for in this function (concrete types, or `Ty::Param` when symbolic).
    pub(crate) subst: Vec<(String, Ty)>,
    /// The bounds of the type parameters in scope, for the symbolic check.
    pub(crate) bounds: Vec<(String, Vec<Bound>)>,
    /// Checked against bounds only: it has no code, and its types may contain `Ty::Param`.
    pub(crate) symbolic: bool,
    /// Whether the body is checked. A symbolic function that exists only to be found by name is not.
    pub(crate) check_body: bool,
    /// An `unsafe fn`: calling it requires an `unsafe` context, and its body is one.
    pub(crate) is_unsafe: bool,
    /// An `extern "C" fn`: emitted with the C ABI under its plain symbol (issue #489).
    pub(crate) is_exported: bool,
}

/// The generic items of the program, by index.
#[derive(Default)]
pub(crate) struct Templates<'t> {
    pub(crate) structs: Vec<&'t ast::StructDecl>,
    pub(crate) enums: Vec<&'t ast::EnumDecl>,
    pub(crate) fns: Vec<&'t ast::FnDecl>,
    pub(crate) ifaces: Vec<&'t ast::InterfaceDecl>,
    pub(crate) impls: Vec<ImplTemplate<'t>>,
    /// Every generic method, by the template index [`crate::globals::Globals::method_templates`] stores.
    pub(crate) methods: Vec<MethodTemplate<'t>>,
}

/// A method with its own type parameters. Its identity function is checked with the method's parameters as
/// opaque types; each call instantiates it for the inferred arguments.
pub(crate) struct MethodTemplate<'t> {
    pub(crate) decl: &'t ast::FnDecl,
    /// The internal name (`List<i32>.map`), for instance names and diagnostics.
    pub(crate) name: String,
    pub(crate) module: usize,
    pub(crate) self_ty: Option<Ty>,
    /// What the enclosing impl or interface stands for, method parameters excluded.
    pub(crate) base_subst: Vec<(String, Ty)>,
    pub(crate) base_bounds: Vec<(String, Vec<Bound>)>,
    pub(crate) params: Vec<String>,
    pub(crate) bounds: Vec<Vec<Bound>>,
    pub(crate) is_unsafe: bool,
}

/// An impl block whose type is generic: its methods are instantiated with every instance of the type.
#[derive(Clone)]
pub(crate) struct ImplTemplate<'t> {
    pub(crate) decl: &'t ast::ImplBlock,
    pub(crate) module: usize,
    /// The generic type it is for.
    pub(crate) template: usize,
    /// The names the block gives the type's arguments.
    pub(crate) params: Vec<String>,
    /// The type's arguments as the block writes them (`[i32, B]` for `Pair<i32, B>`); the block applies to the
    /// instances these match.
    pub(crate) args: Vec<Ty>,
    pub(crate) wheres: Vec<(String, Vec<Bound>)>,
    pub(crate) iface: Option<usize>,
    /// The interface's type arguments, resolved with the impl's parameter names.
    pub(crate) iface_args: Vec<Ty>,
    /// `Type<T>: Drop { ... }`: the block's one method is the drop glue of every instance.
    pub(crate) is_drop: bool,
}

/// A struct or enum declaration, with what its type parameters stand for and the module it was written in.
pub(crate) struct AdtSrcEntry<'t, T> {
    pub(crate) decl: &'t T,
    pub(crate) subst: Vec<(String, Ty)>,
    /// The bounds each substituted type parameter keeps, so a field may write `T.Item` (`spec/associated-types.md`,
    /// AT3).
    pub(crate) bounds: Vec<(String, Vec<Bound>)>,
    pub(crate) module: usize,
}

/// The declaration behind every struct and enum, with what its type parameters stand for.
#[derive(Default)]
pub(crate) struct AdtSrc<'t> {
    /// `None` for the wrapper struct of an `impl Interface` return (`crate::opaque`), which has no declaration.
    pub(crate) structs: Vec<Option<AdtSrcEntry<'t, ast::StructDecl>>>,
    pub(crate) enums: Vec<AdtSrcEntry<'t, ast::EnumDecl>>,
}

/// An associated type binding of a generic impl whose `Self` type was not allocated when the impl was registered.
/// The binding is resolved again once the instance exists, so a binding may name `Self`
/// (`spec/associated-types.md`, AT2).
pub(crate) struct PendingAssoc<'t> {
    /// The index of the [`crate::generics::ImplInfo`] in `Globals::impls`.
    pub(crate) index: usize,
    pub(crate) decl: &'t ast::ImplBlock,
    /// The generic type the impl is for and the instance's arguments (the impl's pattern), for a generic impl.
    pub(crate) generic: Option<(usize, Vec<Ty>)>,
    /// The target type of a plain impl.
    pub(crate) self_ty: Option<Ty>,
    pub(crate) subst: Vec<(String, Ty)>,
    pub(crate) bounds: Vec<(String, Vec<Bound>)>,
    pub(crate) module: usize,
    /// The interface the impl is for and the arguments as written, resolved again with the bindings: an argument
    /// that names an instance of a generic type resolved to `Error` before the instance existed.
    pub(crate) iface: Option<(usize, Option<&'t ast::PathSegment>)>,
    /// The impl's [`crate::session::ImplTemplate`] in `tpl.impls`, whose interface arguments follow.
    pub(crate) template: Option<usize>,
}

/// A constant or type alias, with the module whose names it uses.
pub(crate) struct NamedSrc<'t, T> {
    pub(crate) decl: &'t T,
    pub(crate) module: usize,
}

/// One extension method declaration, parallel to [`Globals::extensions`].
pub(crate) struct ExtensionSrc<'t> {
    pub(crate) decl: &'t ast::ExtensionDecl,
    pub(crate) module: usize,
}

/// One `derive` declaration (`spec/reflection.md`, RFL8).
pub(crate) struct DeriveSrc<'t> {
    pub(crate) decl: &'t ast::DeriveDecl,
}

/// Everything the checker knows while it works on the program.
pub(crate) struct Sema<'t> {
    pub(crate) globals: Globals,
    /// The syntax tree of every module, parallel to [`Globals::modules`].
    pub(crate) module_asts: Vec<&'t ast::File>,
    /// The module that holds the program's `main`.
    pub(crate) entry: usize,
    pub(crate) tpl: Templates<'t>,
    pub(crate) src: AdtSrc<'t>,
    pub(crate) fn_items: Vec<FnItem<'t>>,
    pub(crate) const_decls: Vec<NamedSrc<'t, ast::ConstDecl>>,
    pub(crate) alias_decls: Vec<NamedSrc<'t, ast::TypeAliasDecl>>,
    /// The `extern` declarations whose signatures are resolved once types and imports are known.
    pub(crate) extern_srcs: Vec<crate::declare::ExternSrc<'t>>,
    /// The extension method declarations, parallel to [`Globals::extensions`].
    pub(crate) extension_srcs: Vec<ExtensionSrc<'t>>,
    /// The `derive` declarations, by the index [`crate::globals::ModuleInfo::derives`] stores.
    pub(crate) derive_srcs: Vec<DeriveSrc<'t>>,
    /// The `extern "C" fn` definitions, for their C-ABI signature checks.
    pub(crate) export_srcs: Vec<(FuncId, &'t ast::ExternFnDecl)>,
    /// Generic functions whose bounds are resolved once imports are known.
    pub(crate) generic_fn_bounds: Vec<(usize, &'t ast::FnDecl, usize)>,
    /// Generic impls whose associated bindings name `Self`: resolved again after the instance exists.
    pub(crate) pending_assoc: Vec<PendingAssoc<'t>>,
    /// Every `Drop::drop` method and where it was declared, to check its signature once all are known.
    pub(crate) drop_checks: Vec<(FuncId, Span)>,
    /// Declare and check the entry package's `test` or `bench` items: what `burdin test` and `burdin bench` build.
    pub(crate) mode: crate::EntryMode,
    /// The `test` blocks that were declared, in declaration order.
    pub(crate) tests: Vec<tir::TestEntry>,
    /// The `bench` blocks that were declared, in declaration order (`spec/bench.md`).
    pub(crate) benches: Vec<tir::BenchEntry>,
    /// The `fuzz` targets that were declared, in declaration order (`spec/fuzz.md`).
    pub(crate) fuzzes: Vec<tir::FuzzEntry>,
    /// Record the source locations of dynamic-error conversions (a debug build; `spec/errors.md`, ERR7).
    pub(crate) traces: bool,
    /// Whether the generic-instance limit was already reported, so the passes after it do not repeat the error.
    pub(crate) instance_limit_reported: bool,
    /// Set while impl blocks are registered, when one names a generic type whose instance does not exist yet.
    pub(crate) impl_needs_instance: bool,
    /// Where the paths, lines, and columns of trace frames come from.
    pub(crate) sources: &'t SourceMap,
}

impl<'t> Sema<'t> {
    /// Builds the module and package tables from the parsed program. `entry_package` and `entry_module` select the
    /// file whose `main` is the program's entry point (an index into that package's modules).
    pub(crate) fn new(
        packages: &'t [crate::Package<'t>],
        entry_package: usize,
        entry_module: usize,
        mode: crate::EntryMode,
        traces: bool,
        sources: &'t SourceMap,
    ) -> Self {
        let mut globals = Globals::default();
        let mut module_asts = Vec::new();
        let mut first_module = Vec::new();
        for (package_id, package) in packages.iter().enumerate() {
            first_module.push(globals.modules.len());
            globals.packages.push(PackageInfo {
                name: package.name.clone(),
                is_root: package.is_root,
                edition: package.edition,
                deps: package.deps.iter().cloned().collect(),
                root_module: None,
                official: package.official.clone(),
            });
            for module in &package.modules {
                let id = globals.modules.len();
                globals.module_index.insert((package_id, module.path.join(".")), id);
                globals.modules.push(ModuleInfo {
                    package: package_id,
                    path: module.path.clone(),
                    file: module.file,
                    values: Default::default(),
                    types: Default::default(),
                    ifaces: Default::default(),
                    modules: Default::default(),
                    extensions: Default::default(),
                    derives: Default::default(),
                    derive_priv: Default::default(),
                    pub_derives: Default::default(),
                    value_priv: Default::default(),
                    type_priv: Default::default(),
                    iface_priv: Default::default(),
                    extension_priv: Default::default(),
                    pub_values: Default::default(),
                    pub_types: Default::default(),
                    pub_ifaces: Default::default(),
                    pub_modules: Default::default(),
                    pub_extensions: Default::default(),
                    module_imports: Default::default(),
                });
                module_asts.push(module.ast);
            }
        }
        for (package_id, package) in packages.iter().enumerate() {
            globals.packages[package_id].root_module =
                package.root_module.map(|module| first_module[package_id] + module);
        }
        let entry = first_module[entry_package] + entry_module;
        Sema {
            globals,
            module_asts,
            entry,
            tpl: Templates::default(),
            src: AdtSrc::default(),
            fn_items: Vec::new(),
            const_decls: Vec::new(),
            alias_decls: Vec::new(),
            extern_srcs: Vec::new(),
            extension_srcs: Vec::new(),
            derive_srcs: Vec::new(),
            export_srcs: Vec::new(),
            generic_fn_bounds: Vec::new(),
            pending_assoc: Vec::new(),
            drop_checks: Vec::new(),
            mode,
            tests: Vec::new(),
            benches: Vec::new(),
            fuzzes: Vec::new(),
            traces,
            instance_limit_reported: false,
            impl_needs_instance: false,
            sources,
        }
    }

    /// Adds a function to check and gives it a placeholder signature until its types are resolved.
    pub(crate) fn push_fn(&mut self, item: FnItem<'t>) -> FuncId {
        let id = FuncId(self.fn_items.len());
        self.globals.sigs.push(Signature::placeholder());
        self.globals.fn_unsafe.push(item.is_unsafe);
        self.globals.fn_spans.push(item.decl.name.span);
        self.globals.param_spans.push(item.decl.params.iter().map(|p| p.span).collect());
        self.globals.ret_spans.push(item.decl.ret.as_ref().map(|r| r.span));
        self.fn_items.push(item);
        id
    }

    /// The file diagnostics with no better location point at.
    pub(crate) fn entry_file(&self) -> FileId {
        self.globals.modules[self.entry].file
    }
}

impl<'a> FnItem<'a> {
    /// A plain function or method: no type parameters, checked as written.
    pub(crate) fn plain(decl: &'a ast::FnDecl, name: String, module: usize, self_ty: Option<Ty>) -> FnItem<'a> {
        FnItem {
            decl,
            name,
            module,
            self_ty,
            subst: Vec::new(),
            bounds: Vec::new(),
            symbolic: false,
            check_body: true,
            is_unsafe: decl.is_unsafe,
            is_exported: false,
        }
    }
}

impl<'t> Sema<'t> {
    /// Signatures and bodies, until no pass asks for an instance that does not exist yet. Each pass starts from
    /// clean diagnostics, so only the last pass's errors are reported.
    pub(crate) fn settle(
        &mut self,
        diags: &mut Diagnostics,
        cache: &mut CheckCache,
    ) -> Option<(Vec<tir::Function>, Vec<tir::Const>, Vec<tir::Closure>)> {
        let file = self.entry_file();
        let mut sig_diags;
        let mut body_diags;
        // What allocating an instance reports (a duplicate method, a bound that fails) is reported once, by the
        // pass that allocated it: the passes after it start from clean diagnostics.
        let mut alloc_diags = Diagnostics::default();
        let checked = loop {
            loop {
                sig_diags = Diagnostics::default();
                self.resolve_all(&mut sig_diags);
                if !self.drain(file, &mut alloc_diags) {
                    break;
                }
            }
            body_diags = Diagnostics::default();
            // The concrete types of `impl Interface` returns come first: a body that calls one is checked against it.
            self.resolve_opaque_returns(&mut body_diags);
            let checked = self.check_bodies(&mut body_diags, cache)?;
            // Impl signatures and requirements may substitute generic instances, so they run inside the loop:
            // what they request is allocated by the drain below and checked again on the next pass.
            crate::validate::check_impl_requirements(&self.globals, &mut body_diags);
            crate::validate::check_impl_signatures(&self.globals, &mut body_diags);
            if !self.drain(file, &mut alloc_diags) {
                break checked;
            }
        };
        diags.absorb(alloc_diags);
        // At the instance limit the instances that were never allocated are `Error` types, and the errors the
        // passes found follow from that; the limit is the one error worth reporting.
        if !self.instance_limit_reported {
            diags.absorb(sig_diags);
            diags.absorb(body_diags);
        }
        Some(checked)
    }

    /// The declaration-level checks that need every body done, and the finished program.
    pub(crate) fn finish(
        self,
        functions: Vec<tir::Function>,
        consts: Vec<tir::Const>,
        closures: Vec<tir::Closure>,
        diags: &mut Diagnostics,
    ) -> tir::Program {
        // A test or bench build replaces the program's entry point with its dispatcher: the user's `main` is not
        // run, and a package whose items all live in other files needs none.
        let main = if self.mode.wants_main() { self.find_main(diags) } else { None };
        check_const_cycles(&consts, &self.const_decls.iter().map(|c| c.decl).collect::<Vec<_>>(), diags);
        let mut checks = Diagnostics::default();
        self.check_drop_signatures(&mut checks);
        self.check_export_signatures(&mut checks);
        check_public_signatures(&self.globals, &self.fn_items, &mut checks);
        validate_derives(&self.globals, &mut checks);
        diags.absorb(checks);
        let (any_ifaces, any_vtables, any_type_ids) = self.globals.any_program_ifaces();
        let allocator_fns = self.globals.allocator_fns();
        let comparison_methods = self.globals.comparison_methods();
        let global_alloc_ty = self.globals.global_alloc_ty.clone();
        let Globals { mut structs, mut enums, sigs, externs, embedded, .. } = self.globals;
        for (key, eq, lt) in comparison_methods {
            let derives = if key.0 { &mut enums[key.1].derives } else { &mut structs[key.1].derives };
            derives.eq_method = eq;
            derives.lt_method = lt;
        }
        let externs = externs
            .into_iter()
            .map(|e| tir::ExternFn {
                symbol: e.symbol,
                params: e.sig.params.into_iter().map(|p| p.ty).collect(),
                ret: e.sig.ret,
                vararg: e.vararg,
                blocking: e.blocking,
            })
            .collect();
        tir::Program {
            functions,
            closures,
            consts,
            structs,
            enums,
            signatures: sigs,
            externs,
            main,
            tests: self.tests,
            benches: self.benches,
            fuzzes: self.fuzzes,
            fuzz_target: None,
            static_init: None,
            any_ifaces,
            any_vtables,
            any_type_ids,
            build_inputs: embedded.into_inner(),
            allocator_fns,
            global_alloc_ty,
        }
    }

    fn find_main(&self, diags: &mut Diagnostics) -> Option<FuncId> {
        let module = &self.globals.modules[self.entry];
        match module.values.get("main") {
            Some(&Global::Func(id)) => {
                let f = self.fn_items[id.0].decl;
                // `fn main(): Result<...>` is the other accepted form (`spec/errors.md`, ERR8), but only when the
                // `Result` is written directly. An alias declared in this file is renamed by the parser
                // (`burdin_syntax::error_item`), so `main` here is the unit wrapper and passes; an alias of a `Result`
                // imported from another module is not renamed, so it is still E0003 (`spec/type-aliases.md`, AL7).
                let result_main = f.ret.as_ref().is_some_and(|ty| {
                    matches!(&ty.kind, ast::TypeKind::Path(segs)
                        if segs.len() == 1 && segs[0].name.name == "Result")
                });
                let ret = &self.globals.sigs[id.0].ret;
                let returns_ok = *ret == Ty::Unit
                    || (result_main && (self.globals.result_parts(ret).is_some() || matches!(ret, Ty::Error)));
                if !f.params.is_empty() || !returns_ok {
                    diags.error(
                        Code::TYPE_MISMATCH,
                        f.name.span,
                        "`main` must take no parameters and return nothing, or return a `Result<...>` written directly",
                    );
                }
                Some(id)
            }
            _ => {
                diags.error(Code::UNKNOWN_NAME, Span::new(module.file, 0, 0), "the program has no `main` function");
                None
            }
        }
    }

    /// An `extern "C" fn` must use the C ABI: ABI "C", no generics or receiver, `own` parameters only (they are
    /// the by-value ones), no `...`, and only C-ABI types (issues #489 and #490).
    fn check_export_signatures(&self, diags: &mut Diagnostics) {
        for &(func, e) in &self.export_srcs {
            let decl = &e.decl;
            if e.abi != "C" {
                diags.error(Code::C_ABI, e.abi_span, format!("the `{}` ABI cannot be exported; only \"C\"", e.abi));
            }
            if !decl.generics.is_empty() || !decl.where_clause.is_empty() {
                diags.error(Code::C_ABI, decl.name.span, "an exported function cannot be generic");
            }
            if decl.is_vararg {
                diags.error(Code::C_ABI, decl.name.span, "an exported function cannot take variadic arguments");
            }
            for p in &decl.params {
                match &p.kind {
                    ast::ParamKind::SelfParam => {
                        diags.error(Code::C_ABI, p.span, "an exported function cannot take `self`");
                    }
                    ast::ParamKind::Named { .. } if p.convention != ast::Convention::Own => {
                        diags.error(
                            Code::C_ABI,
                            p.span,
                            "an exported function parameter is passed by value in C; declare it `own`",
                        );
                    }
                    ast::ParamKind::Named { .. } => {}
                }
            }
            if let Some(sig) = self.globals.sigs.get(func.0) {
                let bad = sig
                    .params
                    .iter()
                    .map(|p| &p.ty)
                    .chain(std::iter::once(&sig.ret))
                    .find(|ty| **ty != Ty::Unit && **ty != Ty::Error && !crate::resolve::is_c_abi_value(ty));
                if let Some(ty) = bad {
                    diags.error(
                        Code::C_ABI,
                        decl.name.span,
                        crate::resolve::c_abi_rejection(ty, "the C ABI of an exported function"),
                    );
                }
            }
        }
    }

    /// `drop` must be `fn drop(mut self)` and return nothing. `mut self` is the only receiver that is
    /// passed by address and borrowed exclusively; a read-only `self` is not mutable enough, and `own self`
    /// would move the value that is being dropped.
    fn check_drop_signatures(&self, diags: &mut Diagnostics) {
        for &(id, span) in &self.drop_checks {
            let sig = &self.globals.sigs[id.0];
            let receiver = sig.params.first();
            let mut_self = receiver.is_some_and(|p| p.by_ref && p.exclusive);
            if !(sig.has_self && sig.params.len() == 1 && mut_self && sig.ret == Ty::Unit) {
                diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    "`drop` must be declared as `fn drop(mut self)` and return nothing",
                );
            }
        }
    }
}
