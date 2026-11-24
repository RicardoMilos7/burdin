//! The names a piece of syntax mentions: every name an expression, a pattern, or a type uses, without resolving it.
//! `unused_imports` and `dead_code` ask whether a name appears anywhere that could use it. Asking by name (instead
//! of resolving) never reports a use as missing: a name that is shadowed or spelled the same by something else only
//! makes the lints quieter, which is the direction this linter errs in.

use crate::walk::{Visit, walk_expr, walk_pattern, walk_type};
use burdin_syntax::ast::*;
use std::collections::HashSet;

#[derive(Default)]
pub struct Mentions {
    pub names: HashSet<String>,
}

impl Mentions {
    fn add(&mut self, name: &str) {
        self.names.insert(name.to_string());
    }

    fn path(&mut self, path: &[Ident]) {
        path.iter().for_each(|i| self.add(&i.name));
    }

    pub fn function(&mut self, f: &FnDecl) {
        self.generics(&f.generics);
        for p in &f.params {
            if let ParamKind::Named { ty, .. } = &p.kind {
                self.ty(ty);
            }
            if let Some(d) = &p.default {
                self.expr(d);
            }
        }
        if let Some(r) = &f.ret {
            self.ty(r);
        }
        self.where_clause(&f.where_clause);
        self.block(&f.body);
    }

    fn generics(&mut self, generics: &[GenericParam]) {
        for g in generics {
            g.bounds.iter().for_each(|b| self.ty(b));
            if let Some(t) = &g.const_ty {
                self.ty(t);
            }
            if let Some(t) = &g.default {
                self.ty(t);
            }
        }
    }

    fn where_clause(&mut self, preds: &[WherePred]) {
        for p in preds {
            self.ty(&p.ty);
            p.bounds.iter().for_each(|b| self.ty(b));
        }
    }

    fn fields(&mut self, fields: &[FieldDecl]) {
        fields.iter().for_each(|f| self.ty(&f.ty));
    }

    /// Everything an item mentions: its signature types, derives, and bodies.
    pub fn item(&mut self, item: &Item) {
        match item {
            Item::Fn(f) => self.function(f),
            Item::Extension(x) => {
                self.ty(&x.recv);
                self.function(&x.func);
            }
            Item::ExternFn(x) => self.function(&x.decl),
            Item::Const(c) => {
                self.ty(&c.ty);
                self.expr(&c.value);
            }
            Item::Struct(s) => {
                self.generics(&s.generics);
                s.derives.iter().for_each(|d| self.ty(d));
                self.fields(&s.fields);
            }
            Item::Enum(e) => {
                self.generics(&e.generics);
                e.derives.iter().for_each(|d| self.ty(d));
                for v in &e.variants {
                    match &v.kind {
                        VariantKind::Unit => {}
                        VariantKind::Tuple(ts) => ts.iter().for_each(|t| self.ty(t)),
                        VariantKind::Struct(fs) => self.fields(fs),
                    }
                }
            }
            Item::ErrorEnum(e) => {
                e.derives.iter().for_each(|d| self.ty(d));
            }
            Item::TypeAlias(t) => {
                self.generics(&t.generics);
                self.ty(&t.ty);
            }
            Item::Impl(b) => {
                self.ty(&b.ty);
                if let Some(i) = &b.interface {
                    self.ty(i);
                }
                self.where_clause(&b.where_clause);
                b.methods.iter().for_each(|m| self.function(m));
            }
            Item::Interface(i) => {
                self.generics(&i.generics);
                i.requires.iter().for_each(|r| self.ty(r));
                i.methods.iter().for_each(|m| self.function(&m.decl));
            }
            Item::Extern(b) => b.decls.iter().for_each(|d| self.function(d)),
            Item::Derive(d) => d.methods.iter().for_each(|m| self.function(m)),
        }
    }
}

impl Visit for Mentions {
    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Name { name, .. } => self.add(&name.name),
            ExprKind::Field(_, name) => self.add(&name.name),
            ExprKind::StructLit(path, fields, _) => {
                self.path(path);
                for f in fields {
                    if f.value.is_none() {
                        self.add(&f.name.name);
                    }
                }
            }
            _ => {}
        }
        walk_expr(self, e);
    }

    fn pattern(&mut self, p: &Pattern) {
        match &p.kind {
            PatternKind::Binding { name, .. } => self.add(&name.name),
            PatternKind::Path(path) | PatternKind::TupleStruct(path, _) | PatternKind::Struct(path, _, _) => {
                self.path(path);
            }
            _ => {}
        }
        walk_pattern(self, p);
    }

    fn ty(&mut self, t: &Type) {
        if let TypeKind::Path(segments) = &t.kind {
            for s in segments {
                self.add(&s.name.name);
            }
        }
        walk_type(self, t);
    }
}
