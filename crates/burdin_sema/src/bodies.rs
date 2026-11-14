//! Checking the bodies of constants and functions.

use crate::fncx::{check_const, check_function};
use crate::session::Sema;
use burdin_diag::Diagnostics;
use burdin_tir as tir;

impl Sema<'_> {
    /// Checks the constants and the body of every function.
    pub(crate) fn check_bodies(&self, diags: &mut Diagnostics) -> (Vec<tir::Function>, Vec<tir::Const>) {
        let consts = self
            .const_decls
            .iter()
            .enumerate()
            .map(|(i, decl)| check_const(&self.globals, decl, &self.globals.const_tys[i], diags))
            .collect();
        let functions = self
            .fn_items
            .iter()
            .enumerate()
            .map(|(i, item)| check_function(&self.globals, item, &self.globals.sigs[i], diags))
            .collect();
        (functions, consts)
    }
}
