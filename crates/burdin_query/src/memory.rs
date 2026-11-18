//! The move and borrow checks of the typed program (`burdin_check`), through a memo.

use crate::stats::RevisionStats;
use burdin_diag::{Diagnostics, SourceMap};
use burdin_sema::Edit;
use burdin_tir::Program;

#[derive(Default)]
pub(crate) struct MemoryCache {}

impl MemoryCache {
    pub(crate) fn apply_edits(&mut self, _edits: &[Edit], _sources: &SourceMap) {}

    pub(crate) fn clear(&mut self) {}

    pub(crate) fn check(&mut self, program: &mut Program, diags: &mut Diagnostics, _stats: &mut RevisionStats) {
        burdin_check::check(program, diags);
    }
}
