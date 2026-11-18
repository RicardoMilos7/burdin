//! Checking the bodies of constants and functions, through the body cache (`crate::cache`).

use crate::cache::{CheckCache, Fingerprinter, ItemKey, Value, fp_of_str};
use crate::fncx::{check_const, check_function};
use crate::session::{FnItem, Sema};
use burdin_diag::{Diagnostics, Span};
use burdin_tir as tir;
use std::cell::RefCell;
use std::hash::Hash;
use std::rc::Rc;

impl Sema<'_> {
    /// Checks the constants and the body of every function, and collects the closure bodies of the functions
    /// that have code. A symbolic body's closures are checked for diagnostics and dropped. `None` means the run was
    /// cancelled (`CheckCache::set_cancel`) before every body was checked.
    pub(crate) fn check_bodies(
        &self,
        diags: &mut Diagnostics,
        cache: &mut CheckCache,
    ) -> Option<(Vec<tir::Function>, Vec<tir::Const>, Vec<tir::Closure>)> {
        let closures = Rc::new(RefCell::new(Vec::new()));
        let mut pass = cache.begin_pass(&self.globals, self.instance_fingerprint());
        let mut consts = Vec::with_capacity(self.const_decls.len());
        for (i, src) in self.const_decls.iter().enumerate() {
            if cache.cancelled() {
                return None;
            }
            let name = self.globals.qualified(src.module, &src.decl.name.name);
            let mut identity = Fingerprinter::new();
            ("const", src.module, &name).hash(&mut identity);
            let key = ItemKey {
                identity: identity.finish_fp(),
                text: self.text_fingerprint(src.decl.span),
                span: src.decl.span,
                name,
                symbolic: false,
            };
            let value = cache.run(&mut pass, &self.globals, &key, diags, &closures, |diags| {
                Value::Const(check_const(
                    &self.globals,
                    src.decl,
                    src.module,
                    &self.globals.const_tys[i],
                    closures.clone(),
                    self.sources,
                    self.traces,
                    diags,
                ))
            });
            let Value::Const(c) = value else { unreachable!("a constant produces a constant") };
            consts.push(c);
        }
        let mut functions = Vec::with_capacity(self.fn_items.len());
        for (i, item) in self.fn_items.iter().enumerate() {
            if cache.cancelled() {
                return None;
            }
            // A symbolic function has no code, so its closures must not reach the program.
            let sink = if item.symbolic { Rc::new(RefCell::new(Vec::new())) } else { closures.clone() };
            let live = |diags: &mut Diagnostics| {
                Value::Function(check_function(
                    &self.globals,
                    item,
                    &self.globals.sigs[i],
                    sink.clone(),
                    self.sources,
                    self.traces,
                    diags,
                ))
            };
            // A declaration that is only found by name has no body to check and nothing to remember.
            let value = if item.check_body {
                let key = self.item_key(item);
                cache.run(&mut pass, &self.globals, &key, diags, &sink, live)
            } else {
                live(diags)
            };
            let Value::Function(f) = value else { unreachable!("a function produces a function") };
            functions.push(f);
        }
        let closures = match Rc::try_unwrap(closures) {
            Ok(cell) => cell.into_inner(),
            // Every `FnCx` that received a clone has been dropped by now.
            Err(_) => unreachable!("every closure sink was dropped"),
        };
        Some((functions, consts, closures))
    }

    /// The fingerprint of every function, struct, and enum the program has so far, in allocation order. Generic
    /// instances are named by their type arguments, so this identifies the instances exactly.
    fn instance_fingerprint(&self) -> crate::cache::Fp {
        let mut h = Fingerprinter::new();
        self.fn_items.iter().for_each(|item| item.name.hash(&mut h));
        self.globals.structs.iter().for_each(|s| s.name.hash(&mut h));
        self.globals.enums.iter().for_each(|e| e.name.hash(&mut h));
        h.finish_fp()
    }

    /// The cache key of a function: the item, what its type parameters stand for, and the text of its declaration.
    fn item_key(&self, item: &FnItem<'_>) -> ItemKey {
        let mut identity = Fingerprinter::new();
        (
            "fn",
            &item.name,
            item.module,
            &item.self_ty,
            &item.subst,
            item.symbolic,
            item.check_body,
            item.is_unsafe,
            item.is_exported,
        )
            .hash(&mut identity);
        ItemKey {
            identity: identity.finish_fp(),
            text: self.text_fingerprint(item.decl.span),
            span: item.decl.span,
            name: item.name.clone(),
            symbolic: item.symbolic,
        }
    }

    /// The fingerprint of the source text a declaration spans. A generated declaration (a derive's `debug`) spans
    /// whatever its generator pointed at, and everything it is generated from is part of the declarations.
    fn text_fingerprint(&self, span: Span) -> crate::cache::Fp {
        let text = &self.sources.get(span.file).text;
        fp_of_str(text.get(span.lo as usize..span.hi as usize).unwrap_or(""))
    }
}
