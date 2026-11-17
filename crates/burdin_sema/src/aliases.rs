//! Generic type aliases (`type Pair<T> = (T, T)`): an alias with parameters is a template that is expanded where it
//! is used, with the written arguments in place of its parameters. The alias is not a type of its own: `Pair<i32>`
//! is the tuple `(i32, i32)`, in every position a type can be written.

use crate::generics::Bound;
use crate::globals::Globals;
use burdin_diag::{Code, Diagnostics, Span};
use burdin_syntax::ast;
use burdin_tir::Ty;

/// How many generic aliases may expand inside one another before the expansion is called endless.
const MAX_EXPANSION_DEPTH: usize = 32;

/// One generic alias declaration.
pub(crate) struct GenericAlias {
    pub(crate) module: usize,
    /// The declaration, kept for as long as the compilation runs: every use expands it again.
    pub(crate) decl: &'static ast::TypeAliasDecl,
}

impl Globals {
    /// `Pair<i32>` for the generic alias `index`: its definition with the arguments in place of its parameters.
    // The arguments are the whole resolution context, as in `generic_adt_type`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn generic_alias_type(
        &self,
        index: usize,
        span: Span,
        args: &[ast::Type],
        self_ty: Option<&Ty>,
        subst: &[(String, Ty)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> Ty {
        let alias = &self.generic_aliases[index];
        let decl = alias.decl;
        if args.len() != decl.generics.len() {
            diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!(
                    "`{}` takes {} type argument(s), but {} were given",
                    decl.name.name,
                    decl.generics.len(),
                    args.len()
                ),
            );
            return Ty::Error;
        }
        if self.alias_depth.get() >= MAX_EXPANSION_DEPTH {
            diags.error(Code::CYCLIC_DECLARATION, span, format!("type alias `{}` expands without end", decl.name.name));
            return Ty::Error;
        }
        // The written arguments, resolved where they are written.
        let mut env: Vec<(String, Ty)> = Vec::new();
        for (param, arg) in decl.generics.iter().zip(args) {
            let ty = match &param.const_ty {
                Some(declared) => {
                    let declared = self.resolve_type_in(declared, None, &[], alias.module, diags);
                    self.eval_const_arg(arg, &declared, subst, module, diags)
                }
                None => self.resolve_type_in(arg, self_ty, subst, module, diags),
            };
            env.push((param.name.name.clone(), ty));
        }
        if env.iter().any(|(_, t)| *t == Ty::Error) {
            return Ty::Error;
        }
        // The bounds on the parameters hold for these arguments, as for a generic type (E0141).
        let own: Vec<(String, Ty)> =
            decl.generics.iter().map(|g| (g.name.name.clone(), crate::generics::param_ty(&g.name.name))).collect();
        for (param, (_, arg)) in decl.generics.iter().zip(&env) {
            if param.const_ty.is_some() || self.ty_has_param(arg) {
                continue;
            }
            let bounds: Vec<Bound> =
                param.bounds.iter().filter_map(|b| self.resolve_bound(b, &own, alias.module, diags)).collect();
            for bound in &bounds {
                if !self.satisfies_with(arg, bound, &[], &env) {
                    let (code, message) =
                        self.bound_unmet(arg, bound, &[], &format!("`{}` of `{}`", param.name.name, decl.name.name));
                    diags.error(code, span, message);
                }
            }
        }
        self.alias_depth.set(self.alias_depth.get() + 1);
        let ty = self.resolve_type_in(&decl.ty, None, &env, alias.module, diags);
        self.alias_depth.set(self.alias_depth.get() - 1);
        ty
    }
}
