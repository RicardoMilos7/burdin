//! `dead_code` (`spec/lint.md`, LN19): a `priv` item that nothing in its package uses. `priv` items are visible only
//! inside the package, so a name that appears nowhere else in the package really is unused; an item without `priv` is part
//! of the package's interface and is never reported.
//!
//! "Used" is by name, across every file and every `test` block of the package: an item is used when its name appears in
//! a unit of code other than its own. A unit is an item, a method, or an `impl` header. The own type of a method is
//! excluded too, so a type that only its own methods mention is dead. The check is conservative: a name that two
//! items share counts as used.

use crate::mentions::Mentions;
use crate::walk::item_span;
use crate::{Ctx, Finding, Lint};
use burdin_diag::Span;
use burdin_syntax::ast::*;
use std::collections::HashSet;

struct Unit {
    /// Names whose mentions in this unit do not count as uses: the unit's own name and the type it belongs to.
    owners: Vec<String>,
    names: HashSet<String>,
}

struct Candidate {
    name: String,
    span: Span,
    what: &'static str,
}

pub fn check(cx: &Ctx, out: &mut Vec<Finding>) {
    let mut units: Vec<Unit> = Vec::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    for source in &cx.input.files {
        for item in &source.ast.items {
            if cx.is_generated(item_span(item)) {
                continue;
            }
            match item {
                Item::Impl(b) => {
                    let type_name = path_name(&b.ty);
                    let mut header = Mentions::default();
                    header_mentions(b, &mut header);
                    units.push(Unit { owners: type_name.iter().cloned().collect(), names: header.names });
                    for m in &b.methods {
                        method_unit(m, &type_name, &mut units, &mut candidates, "method");
                    }
                }
                Item::Fn(f) => method_unit(f, &None, &mut units, &mut candidates, "function"),
                Item::Extension(x) => {
                    let recv = path_name(&x.recv);
                    let mut m = Mentions::default();
                    m.item(item);
                    let owners = recv.into_iter().chain([x.func.name.name.clone()]).collect();
                    units.push(Unit { owners, names: m.names });
                    if x.func.is_priv {
                        candidates.push(candidate(&x.func.name, "extension method", units.len() - 1));
                    }
                }
                _ => {
                    let mut m = Mentions::default();
                    m.item(item);
                    let owner = declared(item);
                    units.push(Unit { owners: owner.iter().map(|(n, ..)| n.name.clone()).collect(), names: m.names });
                    if let Some((name, what, true)) = owner {
                        candidates.push(candidate(name, what, units.len() - 1));
                    }
                }
            }
        }
        for t in &source.ast.tests {
            let mut m = Mentions::default();
            m.function(&t.body_func);
            units.push(Unit { owners: Vec::new(), names: m.names });
        }
        for u in &source.ast.uses {
            let names = u.path.iter().chain(&u.items).chain(u.alias.iter()).map(|i| i.name.clone()).collect();
            units.push(Unit { owners: Vec::new(), names });
        }
    }
    for c in candidates {
        if c.name == "main" || c.name.starts_with('_') {
            continue;
        }
        // A name the source does not spell (the `comptime assert 1` constants) belongs to generated code.
        if cx.slice(c.span) != c.name {
            continue;
        }
        let used = units.iter().any(|u| !u.owners.contains(&c.name) && u.names.contains(&c.name));
        if !used {
            out.push(
                Finding::new(Lint::DeadCode, c.span, format!("{} `{}` is never used", c.what, c.name))
                    .note("remove it, or write `// lint: allow(dead_code)` above it if it is kept on purpose"),
            );
        }
    }
}

fn candidate(name: &Ident, what: &'static str, _unit: usize) -> Candidate {
    Candidate { name: name.name.clone(), span: name.span, what }
}

fn method_unit(
    f: &FnDecl,
    owner_type: &Option<String>,
    units: &mut Vec<Unit>,
    cands: &mut Vec<Candidate>,
    what: &'static str,
) {
    let mut m = Mentions::default();
    m.function(f);
    let owners = owner_type.iter().cloned().chain([f.name.name.clone()]).collect();
    units.push(Unit { owners, names: m.names });
    if f.is_priv {
        cands.push(candidate(&f.name, what, units.len() - 1));
    }
}

fn header_mentions(b: &ImplBlock, m: &mut Mentions) {
    use crate::walk::Visit;
    m.ty(&b.ty);
    if let Some(i) = &b.interface {
        m.ty(i);
    }
    for p in &b.where_clause {
        m.ty(&p.ty);
        p.bounds.iter().for_each(|t| m.ty(t));
    }
}

/// The name an item declares, what to call it, and whether it is `priv`.
fn declared(item: &Item) -> Option<(&Ident, &'static str, bool)> {
    match item {
        Item::Const(c) => Some((&c.name, "constant", c.is_priv)),
        Item::Struct(s) => Some((&s.name, "struct", s.is_priv)),
        Item::Enum(e) => Some((&e.name, "enum", e.is_priv)),
        Item::TypeAlias(t) => Some((&t.name, "type alias", t.is_priv)),
        Item::Interface(i) => Some((&i.name, "interface", i.is_priv)),
        Item::Derive(d) => Some((&d.name, "derive", d.is_priv)),
        _ => None,
    }
}

fn path_name(ty: &Type) -> Option<String> {
    match &ty.kind {
        TypeKind::Path(segments) => segments.last().map(|s| s.name.name.clone()),
        _ => None,
    }
}
