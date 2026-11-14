//! The state of one run of the checker: what is declared, which generic items exist, and which functions are
//! waiting to be checked. The phases that fill and use it are in `declare`, `impls`, `instances`, `resolve`, and
//! `bodies`.

use crate::consts::check_const_cycles;
use crate::derives::validate_derives;
use crate::generics::Bound;
use crate::globals::{Global, Globals};
use crate::validate::check_impl_requirements;
use burdin_diag::{Code, Diagnostics, FileId, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, FuncId, Signature, Ty};

/// A function to check: a top-level function or a method, with the type `Self` stands for.
pub(crate) struct FnItem<'a> {
    pub(crate) decl: &'a ast::FnDecl,
    pub(crate) name: String,
    pub(crate) self_ty: Option<Ty>,
    /// What the type parameters stand for in this function (concrete types, or `Ty::Param` when symbolic).
    pub(crate) subst: Vec<(String, Ty)>,
    /// The bounds of the type parameters in scope, for the symbolic check.
    pub(crate) bounds: Vec<(String, Vec<Bound>)>,
    /// Checked against bounds only: it has no code, and its types may contain `Ty::Param`.
    pub(crate) symbolic: bool,
    /// Whether the body is checked. A symbolic function that exists only to be found by name is not.
    pub(crate) check_body: bool,
}

/// The generic items of the file, by index.
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
    /// The generic type it is for.
    pub(crate) template: usize,
    /// The names the block gives the type's arguments.
    pub(crate) params: Vec<String>,
    pub(crate) wheres: Vec<(String, Vec<Bound>)>,
    pub(crate) iface: Option<usize>,
}

/// The declaration behind every struct and enum, with what its type parameters stand for.
#[derive(Default)]
pub(crate) struct AdtSrc<'t> {
    pub(crate) structs: Vec<(&'t ast::StructDecl, Vec<(String, Ty)>)>,
    pub(crate) enums: Vec<(&'t ast::EnumDecl, Vec<(String, Ty)>)>,
}

/// Everything the checker knows while it works on one file.
pub(crate) struct Sema<'t> {
    pub(crate) globals: Globals,
    pub(crate) tpl: Templates<'t>,
    pub(crate) src: AdtSrc<'t>,
    pub(crate) fn_items: Vec<FnItem<'t>>,
    pub(crate) const_decls: Vec<&'t ast::ConstDecl>,
    pub(crate) alias_decls: Vec<&'t ast::TypeAliasDecl>,
    /// Every `Drop::drop` method and where it was declared, to check its signature once all are known.
    pub(crate) drop_checks: Vec<(FuncId, Span)>,
}

impl<'t> Sema<'t> {
    pub(crate) fn new() -> Self {
        Sema {
            globals: Globals::default(),
            tpl: Templates::default(),
            src: AdtSrc::default(),
            fn_items: Vec::new(),
            const_decls: Vec::new(),
            alias_decls: Vec::new(),
            drop_checks: Vec::new(),
        }
    }

    /// Adds a function to check and gives it a placeholder signature until its types are resolved.
    pub(crate) fn push_fn(&mut self, item: FnItem<'t>) -> FuncId {
        let id = FuncId(self.fn_items.len());
        self.globals.sigs.push(Signature::placeholder());
        self.fn_items.push(item);
        id
    }
}

impl<'a> FnItem<'a> {
    /// A plain function or method: no type parameters, checked as written.
    pub(crate) fn plain(decl: &'a ast::FnDecl, name: String, self_ty: Option<Ty>) -> FnItem<'a> {
        FnItem { decl, name, self_ty, subst: Vec::new(), bounds: Vec::new(), symbolic: false, check_body: true }
    }
}

impl<'t> Sema<'t> {
    /// Signatures and bodies, until no pass asks for an instance that does not exist yet. Each pass starts from
    /// clean diagnostics, so only the last pass's errors are reported.
    pub(crate) fn settle(&mut self, file: FileId, diags: &mut Diagnostics) -> (Vec<tir::Function>, Vec<tir::Const>) {
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
        file: FileId,
        functions: Vec<tir::Function>,
        consts: Vec<tir::Const>,
        diags: &mut Diagnostics,
    ) -> tir::Program {
        let main = self.find_main(file, diags);
        check_const_cycles(&consts, &self.const_decls, diags);
        let mut checks = Diagnostics::default();
        self.check_drop_signatures(&mut checks);
        check_impl_requirements(&self.globals, &mut checks);
        validate_derives(&self.globals, &mut checks);
        diags.absorb(checks);
        let Globals { structs, enums, sigs, .. } = self.globals;
        tir::Program { functions, consts, structs, enums, signatures: sigs, main }
    }

    fn find_main(&self, file: FileId, diags: &mut Diagnostics) -> Option<FuncId> {
        match self.globals.names.get("main") {
            Some(&Global::Func(id)) => {
                let f = self.fn_items[id.0].decl;
                if !f.params.is_empty() || self.globals.sigs[id.0].ret != Ty::Unit {
                    diags.error(Code::TYPE_MISMATCH, f.name.span, "`main` must take no parameters and return nothing");
                }
                Some(id)
            }
            _ => {
                diags.error(Code::UNKNOWN_NAME, Span::new(file, 0, 0), "the program has no `main` function");
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
