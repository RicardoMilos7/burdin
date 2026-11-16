//! Checking the bodies of constants and functions.

use crate::fncx::{check_const, check_function};
use crate::session::Sema;
use burdin_diag::Diagnostics;
use burdin_tir as tir;
use std::cell::RefCell;
use std::rc::Rc;

impl Sema<'_> {
    /// Checks the constants and the body of every function, and collects the closure bodies of the functions
    /// that have code. A symbolic body's closures are checked for diagnostics and dropped.
    pub(crate) fn check_bodies(
        &self,
        diags: &mut Diagnostics,
    ) -> (Vec<tir::Function>, Vec<tir::Const>, Vec<tir::Closure>) {
        let closures = Rc::new(RefCell::new(Vec::new()));
        let consts = self
            .const_decls
            .iter()
            .enumerate()
            .map(|(i, src)| {
                check_const(
                    &self.globals,
                    src.decl,
                    src.module,
                    &self.globals.const_tys[i],
                    closures.clone(),
                    self.sources,
                    self.traces,
                    diags,
                )
            })
            .collect();
        let functions = self
            .fn_items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                // A symbolic function has no code, so its closures must not reach the program.
                let sink = if item.symbolic { Rc::new(RefCell::new(Vec::new())) } else { closures.clone() };
                check_function(&self.globals, item, &self.globals.sigs[i], sink, self.sources, self.traces, diags)
            })
            .collect();
        let closures = match Rc::try_unwrap(closures) {
            Ok(cell) => cell.into_inner(),
            // Every `FnCx` that received a clone has been dropped by now.
            Err(_) => unreachable!("every closure sink was dropped"),
        };
        (functions, consts, closures)
    }
}
