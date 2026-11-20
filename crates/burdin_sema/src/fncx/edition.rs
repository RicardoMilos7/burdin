//! Edition gates (`spec/fix.md`, FX11): a form one edition deprecates and the next removes. In the edition that
//! deprecates it, a use is a warning (E0301) whose machine-applicable fix writes the replacement; in the edition that
//! removes it, a use is an error with the same fix. `burdin fix --migrate` applies the fixes and moves the package
//! to the next edition.

use super::{FnCx, error_expr};
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, FuncId};

/// A standard-library function an edition removes.
struct Removed {
    /// The name the call writes.
    name: &'static str,
    /// The first edition without it.
    removed_in: crate::Edition,
    /// What a call with no arguments becomes: the same behavior, spelled the way every edition accepts.
    replacement: &'static str,
}

/// Every removal, oldest first. Edition 2026 removes `unimplemented()`, which only differs from `todo()` in its
/// panic message; the replacement keeps that message.
const REMOVED: [Removed; 1] =
    [Removed { name: "unimplemented", removed_in: crate::Edition::E2026, replacement: "panic(\"not implemented\")" }];

impl FnCx<'_> {
    /// Checks a call of the standard-library function `f`, written as `id(...)`, against the editions. Returns the
    /// error expression when the package's edition no longer has the function; `None` lets the call be checked as
    /// usual (after a deprecation warning, when there is one).
    pub(super) fn edition_gate(
        &mut self,
        f: FuncId,
        id: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<tir::Expr> {
        let removed = REMOVED.iter().find(|r| r.name == id.name)?;
        if self.globals.std_func(removed.name) != Some(f) {
            return None;
        }
        let edition = self.globals.packages[self.globals.package_of(self.module)].edition;
        let gone = removed.removed_in.as_str();
        let fix = format!("replace the call with `{}`", removed.replacement);
        if edition >= removed.removed_in {
            let d = self.diags.error(
                Code::UNKNOWN_NAME,
                id.span,
                format!("unknown function `{}`: edition {gone} removed it", removed.name),
            );
            if args.is_empty() {
                d.suggest_safe(span, fix, removed.replacement);
            }
            for a in args {
                self.expr(&a.expr, None);
            }
            return Some(error_expr(span));
        }
        let d = self.diags.warning(
            Code::DEPRECATED,
            id.span,
            format!("`{}` is deprecated and edition {gone} removes it", removed.name),
        );
        d.note_without_span("`burdin fix --migrate` applies the fix and moves the package to the next edition");
        if args.is_empty() {
            d.suggest_safe(span, fix, removed.replacement);
        }
        None
    }
}
