//! Visiting every span of a syntax tree. The query library moves the spans of the items after an edit when it
//! reparses one function body (`burdin_query`, `spec/query.md`, QL6), and a test checks that this walk reaches every
//! span the parser produces, so a new node type that forgets it fails there.

use crate::ast::*;
use burdin_diag::Span;

/// Calls `f` on every span of `file`, in no particular order.
pub fn map_spans(file: &mut File, f: &mut dyn FnMut(&mut Span)) {
    let mut m = Mapper { f };
    for item in &mut file.items {
        m.item(item);
    }
    for u in &mut file.uses {
        m.use_decl(u);
    }
    for t in &mut file.tests {
        (m.f)(&mut t.name_span);
        for a in &mut t.attrs {
            m.ident(&mut a.name);
        }
        m.fn_decl(&mut t.func);
        m.fn_decl(&mut t.body_func);
        (m.f)(&mut t.span);
    }
    file.bodies.iter_mut().for_each(|s| (m.f)(s));
}

struct Mapper<'a> {
    f: &'a mut dyn FnMut(&mut Span),
}

impl Mapper<'_> {
    fn ident(&mut self, i: &mut Ident) {
        (self.f)(&mut i.span);
    }

    fn idents(&mut self, is: &mut [Ident]) {
        is.iter_mut().for_each(|i| self.ident(i));
    }

    fn item(&mut self, item: &mut Item) {
        match item {
            Item::Fn(d) => self.fn_decl(d),
            Item::Extension(e) => {
                self.ty(&mut e.recv);
                self.fn_decl(&mut e.func);
            }
            Item::Const(c) => {
                self.ident(&mut c.name);
                self.ty(&mut c.ty);
                self.expr(&mut c.value);
                (self.f)(&mut c.span);
            }
            Item::Struct(s) => {
                self.ident(&mut s.name);
                self.generics(&mut s.generics);
                s.derives.iter_mut().for_each(|t| self.ty(t));
                s.fields.iter_mut().for_each(|fd| self.field_decl(fd));
                (self.f)(&mut s.span);
            }
            Item::Enum(e) => {
                self.ident(&mut e.name);
                self.generics(&mut e.generics);
                e.derives.iter_mut().for_each(|t| self.ty(t));
                e.variants.iter_mut().for_each(|v| self.variant(v));
                (self.f)(&mut e.span);
            }
            Item::ErrorEnum(e) => {
                self.ident(&mut e.name);
                self.generics(&mut e.generics);
                e.derives.iter_mut().for_each(|t| self.ty(t));
                for v in &mut e.variants {
                    self.ident(&mut v.name);
                    self.variant_kind(&mut v.kind);
                    if let Some(s) = &mut v.from {
                        (self.f)(s);
                    }
                    self.parts(&mut v.message);
                    (self.f)(&mut v.message_span);
                }
                (self.f)(&mut e.span);
            }
            Item::TypeAlias(a) => {
                self.ident(&mut a.name);
                self.generics(&mut a.generics);
                self.ty(&mut a.ty);
                (self.f)(&mut a.span);
            }
            Item::Impl(b) => {
                self.ty(&mut b.ty);
                if let Some(i) = &mut b.interface {
                    self.ty(i);
                }
                self.wheres(&mut b.where_clause);
                b.methods.iter_mut().for_each(|m| self.fn_decl(m));
                (self.f)(&mut b.span);
            }
            Item::Interface(i) => {
                self.ident(&mut i.name);
                self.generics(&mut i.generics);
                i.requires.iter_mut().for_each(|t| self.ty(t));
                i.methods.iter_mut().for_each(|m| self.fn_decl(&mut m.decl));
                (self.f)(&mut i.span);
            }
            Item::Extern(b) => {
                (self.f)(&mut b.abi_span);
                b.decls.iter_mut().for_each(|d| self.fn_decl(d));
                (self.f)(&mut b.span);
            }
            Item::ExternFn(e) => {
                (self.f)(&mut e.abi_span);
                self.fn_decl(&mut e.decl);
            }
            Item::Derive(d) => {
                self.ident(&mut d.name);
                d.methods.iter_mut().for_each(|m| self.fn_decl(m));
                (self.f)(&mut d.span);
            }
        }
    }

    fn use_decl(&mut self, u: &mut UseDecl) {
        self.idents(&mut u.path);
        if let Some(a) = &mut u.alias {
            self.ident(a);
        }
        self.idents(&mut u.items);
        (self.f)(&mut u.span);
    }

    fn field_decl(&mut self, fd: &mut FieldDecl) {
        self.ident(&mut fd.name);
        self.ty(&mut fd.ty);
    }

    fn variant(&mut self, v: &mut Variant) {
        self.ident(&mut v.name);
        self.variant_kind(&mut v.kind);
    }

    fn variant_kind(&mut self, k: &mut VariantKind) {
        match k {
            VariantKind::Unit => {}
            VariantKind::Tuple(ts) => ts.iter_mut().for_each(|t| self.ty(t)),
            VariantKind::Struct(fs) => fs.iter_mut().for_each(|fd| self.field_decl(fd)),
        }
    }

    fn generics(&mut self, gs: &mut [GenericParam]) {
        for g in gs {
            self.ident(&mut g.name);
            g.bounds.iter_mut().for_each(|t| self.ty(t));
            if let Some(t) = &mut g.const_ty {
                self.ty(t);
            }
            if let Some(t) = &mut g.default {
                self.ty(t);
            }
        }
    }

    fn wheres(&mut self, ws: &mut [WherePred]) {
        for w in ws {
            self.ty(&mut w.ty);
            w.bounds.iter_mut().for_each(|t| self.ty(t));
        }
    }

    fn fn_decl(&mut self, d: &mut FnDecl) {
        self.ident(&mut d.name);
        self.generics(&mut d.generics);
        for p in &mut d.params {
            match &mut p.kind {
                ParamKind::SelfParam => {}
                ParamKind::Named { name, ty } => {
                    self.ident(name);
                    self.ty(ty);
                }
            }
            if let Some(e) = &mut p.default {
                self.expr(e);
            }
            if let Some(s) = &mut p.comptime {
                (self.f)(s);
            }
            (self.f)(&mut p.span);
        }
        if let Some(t) = &mut d.ret {
            self.ty(t);
        }
        self.wheres(&mut d.where_clause);
        self.block(&mut d.body);
        (self.f)(&mut d.span);
    }

    fn ty(&mut self, t: &mut Type) {
        match &mut t.kind {
            TypeKind::Path(segs) => {
                for s in segs {
                    self.ident(&mut s.name);
                    s.args.iter_mut().for_each(|a| self.ty(a));
                }
            }
            TypeKind::Any(inner) | TypeKind::Impl(inner) | TypeKind::Slice(inner) => self.ty(inner),
            TypeKind::Fn { params, ret, .. } => {
                params.iter_mut().for_each(|p| self.ty(p));
                if let Some(r) = ret {
                    self.ty(r);
                }
            }
            TypeKind::SelfType | TypeKind::Unit | TypeKind::Never => {}
            TypeKind::Tuple(ts) => ts.iter_mut().for_each(|x| self.ty(x)),
            TypeKind::Array(elem, len) => {
                self.ty(elem);
                self.expr(len);
            }
            TypeKind::Const(e) => self.expr(e),
            TypeKind::Ref { ty, .. } => self.ty(ty),
        }
        (self.f)(&mut t.span);
    }

    fn block(&mut self, b: &mut Block) {
        for s in &mut b.stmts {
            match s {
                Stmt::Let { pattern, ty, init, else_block, span } => {
                    self.pat(pattern);
                    if let Some(t) = ty {
                        self.ty(t);
                    }
                    self.expr(init);
                    if let Some(e) = else_block {
                        self.block(e);
                    }
                    (self.f)(span);
                }
                Stmt::Expr(e) => self.expr(e),
            }
        }
        (self.f)(&mut b.span);
    }

    fn pat(&mut self, p: &mut Pattern) {
        match &mut p.kind {
            PatternKind::Wild | PatternKind::Lit(_) | PatternKind::Range(..) => {}
            PatternKind::Binding { name, .. } => self.ident(name),
            PatternKind::Tuple(ps) | PatternKind::Or(ps) => ps.iter_mut().for_each(|x| self.pat(x)),
            PatternKind::Path(path) => self.idents(path),
            PatternKind::TupleStruct(path, ps) => {
                self.idents(path);
                ps.iter_mut().for_each(|x| self.pat(x));
            }
            PatternKind::Struct(path, fields) => {
                self.idents(path);
                for fp in fields {
                    self.ident(&mut fp.name);
                    if let Some(x) = &mut fp.pattern {
                        self.pat(x);
                    }
                }
            }
            PatternKind::At(name, inner) => {
                self.ident(name);
                self.pat(inner);
            }
        }
        (self.f)(&mut p.span);
    }

    fn parts(&mut self, parts: &mut [InterpPart]) {
        for part in parts {
            if let InterpPart::Hole(h) = part {
                if let HoleArg::Expr(e) = &mut h.arg {
                    self.expr(e);
                }
                (self.f)(&mut h.spec.span);
                (self.f)(&mut h.span);
            }
        }
    }

    fn expr(&mut self, e: &mut Expr) {
        match &mut e.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Char(_)
            | ExprKind::Str(_)
            | ExprKind::Bool(_)
            | ExprKind::Unit
            | ExprKind::SelfValue
            | ExprKind::Break
            | ExprKind::Continue => {}
            ExprKind::Interp(parts) => self.parts(parts),
            ExprKind::Name { name, args } => {
                self.ident(name);
                args.iter_mut().for_each(|a| self.ty(a));
            }
            ExprKind::Tuple(es) | ExprKind::Array(es) => es.iter_mut().for_each(|x| self.expr(x)),
            ExprKind::ArrayRepeat(a, b)
            | ExprKind::Assign(a, b)
            | ExprKind::Binary(_, a, b)
            | ExprKind::OrElse(a, b)
            | ExprKind::DynField(a, b)
            | ExprKind::Index(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Block(b) | ExprKind::UnsafeBlock(b) | ExprKind::Loop(b) => self.block(b),
            ExprKind::If { cond, then_block, else_expr } => {
                self.expr(cond);
                self.block(then_block);
                if let Some(x) = else_expr {
                    self.expr(x);
                }
            }
            ExprKind::While { cond, body } => {
                self.expr(cond);
                self.block(body);
            }
            ExprKind::For { pattern, iter, body, .. } => {
                self.pat(pattern);
                self.expr(iter);
                self.block(body);
            }
            ExprKind::Return(x) => {
                if let Some(x) = x {
                    self.expr(x);
                }
            }
            ExprKind::Unary(_, x) | ExprKind::Try(x) | ExprKind::Comptime(x) => self.expr(x),
            ExprKind::Cast(x, t) => {
                self.expr(x);
                self.ty(t);
            }
            ExprKind::Range { lo, hi, .. } => {
                self.expr(lo);
                self.expr(hi);
            }
            ExprKind::Field(x, name) => {
                self.expr(x);
                self.ident(name);
            }
            ExprKind::TypeArgs(x, ts) => {
                self.expr(x);
                ts.iter_mut().for_each(|t| self.ty(t));
            }
            ExprKind::TupleField(x, _, s) => {
                self.expr(x);
                (self.f)(s);
            }
            ExprKind::Call(callee, args) => {
                self.expr(callee);
                for a in args {
                    if let Some(n) = &mut a.name {
                        self.ident(n);
                    }
                    if let Some(s) = &mut a.mut_span {
                        (self.f)(s);
                    }
                    self.expr(&mut a.expr);
                }
            }
            ExprKind::Match(scrutinee, arms) => {
                self.expr(scrutinee);
                for arm in arms {
                    if let Some(p) = &mut arm.pattern {
                        self.pat(p);
                    }
                    if let Some(g) = &mut arm.guard {
                        self.expr(g);
                    }
                    self.expr(&mut arm.body);
                    (self.f)(&mut arm.span);
                }
            }
            ExprKind::Closure { params, ret, body } => {
                for p in params {
                    self.ident(&mut p.name);
                    if let Some(t) = &mut p.ty {
                        self.ty(t);
                    }
                }
                if let Some(t) = ret {
                    self.ty(t);
                }
                self.expr(body);
            }
            ExprKind::StructLit(path, fields) => {
                self.idents(path);
                for fi in fields {
                    self.ident(&mut fi.name);
                    if let Some(v) = &mut fi.value {
                        self.expr(v);
                    }
                }
            }
            ExprKind::IfLet { pattern, scrutinee, then_block, else_expr } => {
                self.pat(pattern);
                self.expr(scrutinee);
                self.block(then_block);
                if let Some(x) = else_expr {
                    self.expr(x);
                }
            }
            ExprKind::ComptimeAssert(a, b) => {
                self.expr(a);
                if let Some(b) = b {
                    self.expr(b);
                }
            }
        }
        (self.f)(&mut e.span);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burdin_diag::{Diagnostics, FileId};
    use std::path::{Path, PathBuf};

    fn bd_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                bd_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "bd") {
                out.push(path);
            }
        }
    }

    fn parse(text: &str) -> Option<String> {
        let mut d = Diagnostics::default();
        let ast = crate::parse(FileId(0), text, &mut d);
        (!d.has_errors()).then(|| format!("{ast:#?}"))
    }

    /// The standard library and every conformance test that parses cover every construct the grammar has, so a
    /// span the walk forgets shows up as a difference. A script is skipped: its shebang is only valid at byte 0
    /// (`spec/scripts.md`, SC1), so no padded copy of it parses.
    #[test]
    fn the_walk_reaches_every_span_the_parser_produces() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        bd_files(&root.join("std"), &mut files);
        bd_files(&root.join("tests"), &mut files);
        let mut checked = 0;
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            if text.starts_with("#!") {
                continue;
            }
            let mut d = Diagnostics::default();
            let mut ast = crate::parse(FileId(0), &text, &mut d);
            if d.has_errors() {
                continue;
            }
            map_spans(&mut ast, &mut |span: &mut Span| {
                span.lo += 5;
                span.hi += 5;
            });
            // The same text with five characters in front has the same tree, every span 5 further on.
            let shifted = parse(&format!("     {text}")).expect("padding keeps a file valid");
            assert_eq!(format!("{ast:#?}"), shifted, "{}", path.display());
            checked += 1;
        }
        assert!(checked > 200, "only {checked} files were checked");
    }
}
