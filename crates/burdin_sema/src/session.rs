//! The state of one run of the checker: the modules of the program, what is declared, which generic items exist,
//! and which functions are waiting to be checked. The phases that fill and use it are in `declare`, `imports`,
//! `impls`, `instances`, `resolve`, and `bodies`.

use crate::consts::check_const_cycles;
use crate::derives::validate_derives;
use crate::generics::Bound;
use crate::globals::{Global, Globals, ModuleInfo, PackageInfo};
use crate::validate::check_impl_requirements;
use burdin_diag::{Code, Diagnostics, FileId, Span};
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
}

/// The generic items of the program, by index.
#[derive(Default)]
pub(crate) struct Templates<'t> {
    pub(crate) structs: Vec<&'t ast::StructDecl>,
    pub(crate) enums: Vec<&'t ast::EnumDecl>,
    pub(crate) fns: Vec<&'t ast::FnDecl>,
    pub(crate) ifaces: Vec<&'t ast::InterfaceDecl>,
    pub(crate) impls: Vec<ImplTemplate<'t>>,
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
    pub(crate) wheres: Vec<(String, Vec<Bound>)>,
    pub(crate) iface: Option<usize>,
}

/// A struct or enum declaration, with what its type parameters stand for and the module it was written in.
pub(crate) struct AdtSrcEntry<'t, T> {
    pub(crate) decl: &'t T,
    pub(crate) subst: Vec<(String, Ty)>,
    pub(crate) module: usize,
}

/// The declaration behind every struct and enum, with what its type parameters stand for.
#[derive(Default)]
pub(crate) struct AdtSrc<'t> {
    pub(crate) structs: Vec<AdtSrcEntry<'t, ast::StructDecl>>,
    pub(crate) enums: Vec<AdtSrcEntry<'t, ast::EnumDecl>>,
}

/// A constant or type alias, with the module whose names it uses.
pub(crate) struct NamedSrc<'t, T> {
    pub(crate) decl: &'t T,
    pub(crate) module: usize,
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
    /// Generic functions whose bounds are resolved once imports are known.
    pub(crate) generic_fn_bounds: Vec<(usize, &'t ast::FnDecl, usize)>,
    /// Every `Drop::drop` method and where it was declared, to check its signature once all are known.
    pub(crate) drop_checks: Vec<(FuncId, Span)>,
}

impl<'t> Sema<'t> {
    /// Builds the module and package tables from the parsed program. `entry_package` and `entry_module` select the
    /// file whose `main` is the program's entry point (an index into that package's modules).
    pub(crate) fn new(packages: &'t [crate::Package<'t>], entry_package: usize, entry_module: usize) -> Self {
        let mut globals = Globals::default();
        let mut module_asts = Vec::new();
        let mut first_module = Vec::new();
        for (package_id, package) in packages.iter().enumerate() {
            first_module.push(globals.modules.len());
            globals.packages.push(PackageInfo {
                name: package.name.clone(),
                is_root: package.is_root,
                deps: package.deps.iter().cloned().collect(),
                root_module: None,
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
                    value_priv: Default::default(),
                    type_priv: Default::default(),
                    iface_priv: Default::default(),
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
            generic_fn_bounds: Vec::new(),
            drop_checks: Vec::new(),
        }
    }

    /// Adds a function to check and gives it a placeholder signature until its types are resolved.
    pub(crate) fn push_fn(&mut self, item: FnItem<'t>) -> FuncId {
        let id = FuncId(self.fn_items.len());
        self.globals.sigs.push(Signature::placeholder());
        self.globals.fn_unsafe.push(item.is_unsafe);
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
        }
    }
}

impl<'t> Sema<'t> {
    /// Signatures and bodies, until no pass asks for an instance that does not exist yet. Each pass starts from
    /// clean diagnostics, so only the last pass's errors are reported.
    pub(crate) fn settle(&mut self, diags: &mut Diagnostics) -> (Vec<tir::Function>, Vec<tir::Const>) {
        let file = self.entry_file();
        let mut sig_diags;
        let mut body_diags;
        let checked = loop {
            loop {
                sig_diags = Diagnostics::default();
                self.resolve_all(&mut sig_diags);
                if !self.drain(file, &mut sig_diags) {
                    break;
                }
            }
            body_diags = Diagnostics::default();
            let checked = self.check_bodies(&mut body_diags);
            if !self.drain(file, &mut body_diags) {
                break checked;
            }
        };
        diags.absorb(sig_diags);
        diags.absorb(body_diags);
        checked
    }

    /// The declaration-level checks that need every body done, and the finished program.
    pub(crate) fn finish(
        self,
        functions: Vec<tir::Function>,
        consts: Vec<tir::Const>,
        diags: &mut Diagnostics,
    ) -> tir::Program {
        let main = self.find_main(diags);
        check_const_cycles(&consts, &self.const_decls.iter().map(|c| c.decl).collect::<Vec<_>>(), diags);
        let mut checks = Diagnostics::default();
        self.check_drop_signatures(&mut checks);
        check_impl_requirements(&self.globals, &mut checks);
        validate_derives(&self.globals, &mut checks);
        diags.absorb(checks);
        let Globals { structs, enums, sigs, externs, .. } = self.globals;
        let externs = externs
            .into_iter()
            .map(|e| tir::ExternFn {
                symbol: e.symbol,
                params: e.sig.params.into_iter().map(|p| p.ty).collect(),
                ret: e.sig.ret,
            })
            .collect();
        tir::Program { functions, consts, structs, enums, signatures: sigs, externs, main }
    }

    fn find_main(&self, diags: &mut Diagnostics) -> Option<FuncId> {
        let module = &self.globals.modules[self.entry];
        match module.values.get("main") {
            Some(&Global::Func(id)) => {
                let f = self.fn_items[id.0].decl;
                if !f.params.is_empty() || self.globals.sigs[id.0].ret != Ty::Unit {
                    diags.error(Code::TYPE_MISMATCH, f.name.span, "`main` must take no parameters and return nothing");
                }
                Some(id)
            }
            _ => {
                diags.error(Code::UNKNOWN_NAME, Span::new(module.file, 0, 0), "the program has no `main` function");
                None
            }
        }
    }

    /// `drop` must be `fn drop(mut self)` and return nothing.
    fn check_drop_signatures(&self, diags: &mut Diagnostics) {
        for &(id, span) in &self.drop_checks {
            let sig = &self.globals.sigs[id.0];
            if !(sig.has_self && sig.params.len() == 1 && sig.params[0].by_ref && sig.ret == Ty::Unit) {
                diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    "`drop` must be declared as `fn drop(mut self)` and return nothing",
                );
            }
        }
    }
}
