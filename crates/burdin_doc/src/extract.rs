//! Builds the documented API from the syntax trees (`spec/doc.md`, DC7-DC16): the items of every module, the members
//! that other blocks add to a type, the `pub use` re-exports, and the resolution of every intra-doc link.

use crate::markdown;
use crate::model::*;
use crate::resolve::{Cursor, Index, binds_of, declaration};
use crate::signature::{self, collapse, type_text, where_text};
use crate::{BROKEN_LINK, HIDDEN_LINK, Input, ModuleInput};
use burdin_diag::{Diagnostics, FileId, Span};
use burdin_syntax::ast::{self, ExtensionDecl, FnDecl, Ident, Type, TypeKind};
use burdin_syntax::docs::{self, DocComment};

struct Ctx<'a> {
    input: &'a Input<'a>,
    index: &'a Index,
}

impl Ctx<'_> {
    fn text(&self, file: FileId) -> &str {
        &self.input.sources.get(file).text
    }

    fn line(&self, span: Span) -> u32 {
        self.input.sources.get(span.file).line_col(span.lo).0
    }

    fn doc(&self, comment: Option<DocComment>) -> Option<Doc> {
        comment.map(|c| Doc { text: c.text, lines: c.lines, links: Vec::new() })
    }

    fn keep(&self, private: bool) -> bool {
        self.input.private || !private
    }
}

pub(crate) fn build(input: &Input) -> (Api, Diagnostics) {
    let entry = &input.packages[input.entry];
    let index = Index::new(input);
    let cx = Ctx { input, index: &index };
    let mut modules: Vec<Module> = entry.modules.iter().map(|m| extract_module(&cx, m)).collect();
    for (mi, m) in entry.modules.iter().enumerate() {
        attach(&cx, &mut modules, mi, m);
    }
    for (mi, m) in entry.modules.iter().enumerate() {
        modules[mi].reexports = reexports(&cx, mi, m);
    }
    // `reexported_by` is the same information seen from the other side.
    let mut by: Vec<(Vec<String>, String, String)> = Vec::new();
    for m in &modules {
        for r in &m.reexports {
            if let Resolution::Item { module, name, .. } = &r.target {
                by.push((module.clone(), name.clone(), m.display(entry.name)));
            }
        }
    }
    for (module, name, via) in by {
        if let Some(item) =
            modules.iter_mut().find(|m| m.path == module).and_then(|m| m.items.iter_mut().find(|i| i.name == name))
        {
            item.reexported_by.push(via);
        }
    }
    let mut api = Api { package: entry.name.to_string(), private: input.private, modules };
    let mut diags = Diagnostics::default();
    resolve_links(&cx, &mut api, &mut diags);
    api.modules.sort_by(|a, b| a.path.cmp(&b.path));
    (api, diags)
}

fn extract_module(cx: &Ctx, m: &ModuleInput) -> Module {
    let text = cx.text(m.file);
    let mut items = Vec::new();
    let mut extensions = Vec::new();
    for item in &m.ast.items {
        match item {
            ast::Item::Extern(block) => {
                for d in &block.decls {
                    if cx.keep(block.is_priv || d.is_priv) && !docs::is_internal_name(&d.name.name) {
                        let sig = format!("extern \"{}\" {}", block.abi, signature::fn_head(text, d));
                        items.push(simple_item(
                            cx,
                            text,
                            Kind::ExternFunction,
                            &d.name,
                            d.span,
                            sig,
                            block.is_priv || d.is_priv,
                        ));
                    }
                }
            }
            ast::Item::Extension(x) => {
                if cx.keep(x.func.is_priv) {
                    extensions.push(extension_member(cx, text, m, x));
                }
            }
            other => {
                if let Some(i) = item_of(cx, text, other) {
                    items.push(i);
                }
            }
        }
    }
    Module {
        path: m.path.to_vec(),
        file: m.file,
        file_path: m.file_path.clone(),
        docs: cx.doc(docs::module_docs(m.file, text)),
        items,
        extensions,
        reexports: Vec::new(),
        is_root: m.is_root,
    }
}

fn simple_item(cx: &Ctx, text: &str, kind: Kind, name: &Ident, span: Span, signature: String, private: bool) -> Item {
    Item {
        kind,
        name: name.name.clone(),
        private,
        signature,
        docs: cx.doc(docs::doc_before(span.file, text, span.lo)),
        span: name.span,
        line: cx.line(name.span),
        members: Vec::new(),
        methods: Vec::new(),
        implementations: Vec::new(),
        operators: Vec::new(),
        extensions: Vec::new(),
        reexported_by: Vec::new(),
    }
}

fn member(cx: &Ctx, text: &str, kind: MemberKind, name: &Ident, at: u32, signature: String, private: bool) -> Member {
    Member {
        kind,
        name: name.name.clone(),
        display: name.name.clone(),
        signature,
        docs: cx.doc(docs::doc_before(name.span.file, text, at)),
        private,
        span: name.span,
        line: cx.line(name.span),
        origin: None,
        instance: None,
    }
}

fn item_of(cx: &Ctx, text: &str, item: &ast::Item) -> Option<Item> {
    let (name, kind, private) = declaration(item, text)?;
    if !cx.keep(private) {
        return None;
    }
    Some(match item {
        ast::Item::Fn(f) => simple_item(cx, text, kind, &f.name, f.span, signature::fn_head(text, f), private),
        ast::Item::ExternFn(x) => {
            let sig = format!("extern \"{}\" {}", x.abi, signature::fn_head(text, &x.decl));
            simple_item(cx, text, kind, &x.decl.name, x.decl.span, sig, private)
        }
        ast::Item::Struct(s) => {
            let mut i = simple_item(cx, text, kind, &s.name, s.span, signature::brace_head(text, s.span), private);
            for f in s.fields.iter().filter(|f| cx.keep(f.is_priv)) {
                i.members.push(member(
                    cx,
                    text,
                    MemberKind::Field,
                    &f.name,
                    f.name.span.lo,
                    signature::field_text(text, f),
                    f.is_priv,
                ));
            }
            i.implementations = derives(cx, &s.derives);
            i
        }
        ast::Item::Enum(e) => {
            let mut i = simple_item(cx, text, kind, &e.name, e.span, signature::brace_head(text, e.span), private);
            let error = kind == Kind::ErrorEnum;
            for v in &e.variants {
                i.members.push(member(
                    cx,
                    text,
                    MemberKind::Variant,
                    &v.name,
                    v.name.span.lo,
                    signature::variant_text(text, v, error),
                    false,
                ));
            }
            i.implementations = derives(cx, &e.derives);
            i
        }
        ast::Item::Interface(d) => {
            let mut i = simple_item(cx, text, kind, &d.name, d.span, signature::brace_head(text, d.span), private);
            for m in &d.methods {
                let k = if m.has_body { MemberKind::Provided } else { MemberKind::Required };
                i.methods.push(member(
                    cx,
                    text,
                    k,
                    &m.decl.name,
                    m.decl.span.lo,
                    signature::fn_head(text, &m.decl),
                    false,
                ));
            }
            i
        }
        ast::Item::TypeAlias(a) => {
            simple_item(cx, text, kind, &a.name, a.span, signature::whole(text, a.span), private)
        }
        ast::Item::Const(c) => simple_item(cx, text, kind, &c.name, c.span, signature::const_head(text, c), private),
        ast::Item::Derive(d) => {
            simple_item(cx, text, kind, &d.name, d.span, format!("derive {}", d.name.name), private)
        }
        _ => {
            let _ = name;
            return None;
        }
    })
}

/// The interfaces the `: A, B` list after a struct or enum name names.
fn derives(cx: &Ctx, list: &[Type]) -> Vec<Implementation> {
    list.iter()
        .map(|t| Implementation {
            interface: type_text(cx.text(t.span.file), t),
            target: None,
            derived: true,
            instance: None,
            where_clause: String::new(),
            members: Vec::new(),
        })
        .collect()
}

fn extension_member(cx: &Ctx, text: &str, m: &ModuleInput, x: &ExtensionDecl) -> Member {
    let mut mem = member(
        cx,
        text,
        MemberKind::Extension,
        &x.func.name,
        x.func.span.lo,
        signature::fn_head(text, &x.func),
        x.func.is_priv,
    );
    let pkg = cx.input.packages[cx.input.entry].name;
    let shown = if m.path.is_empty() || m.is_root { pkg.to_string() } else { m.path.join(".") };
    let import = (!m.path.is_empty()).then(|| format!("use {shown}.{}", x.func.name.name));
    mem.origin = Some(Origin { module: shown, import });
    mem
}

/// The declaration a receiver or impl type names, when it is one of the package's.
fn receiver(cx: &Ctx, mi: usize, ty: &Type) -> Option<(usize, String)> {
    let TypeKind::Path(segments) = &ty.kind else { return None };
    let names: Vec<String> = segments.iter().map(|s| s.name.name.clone()).collect();
    match cx.index.resolve(Some(mi), &names)? {
        (Cursor::Item { module, name }, used) if used == names.len() => Some((module, name)),
        _ => None,
    }
}

/// The instance an impl block is for, when its type arguments are not just the block's own parameters.
fn instance(cx: &Ctx, mi: usize, text: &str, ty: &Type) -> Option<String> {
    let TypeKind::Path(segments) = &ty.kind else { return None };
    let args = &segments.last()?.args;
    let concrete = args.iter().any(|a| match &a.kind {
        TypeKind::Path(segs) if segs.len() == 1 && segs[0].args.is_empty() => {
            cx.index.resolve(Some(mi), &[segs[0].name.name.clone()]).is_some()
        }
        _ => true,
    });
    concrete.then(|| type_text(text, ty))
}

fn find_item<'a>(modules: &'a mut [Module], index: &Index, module: usize, name: &str) -> Option<&'a mut Item> {
    let path = &index.modules[module].path;
    modules.iter_mut().find(|m| &m.path == path)?.items.iter_mut().find(|i| i.name == name)
}

/// Adds the members that impl blocks and extension methods give to the types they are for.
fn attach(cx: &Ctx, modules: &mut [Module], mi: usize, m: &ModuleInput) {
    let text = cx.text(m.file);
    for item in &m.ast.items {
        match item {
            ast::Item::Impl(b) if !(docs::is_generated(text, b.span) && b.span == b.ty.span) => {
                let Some((tm, tname)) = receiver(cx, mi, &b.ty) else { continue };
                let inst = instance(cx, mi, text, &b.ty);
                let where_clause = where_text(text, &b.where_clause);
                let methods: Vec<&FnDecl> =
                    b.methods.iter().filter(|f| cx.keep(f.is_priv) && !docs::is_internal_name(&f.name.name)).collect();
                let make = |f: &FnDecl, kind: MemberKind| {
                    let mut mem = member(cx, text, kind, &f.name, f.span.lo, signature::fn_head(text, f), f.is_priv);
                    mem.instance = inst.clone();
                    mem
                };
                let Some(target) = find_item(modules, cx.index, tm, &tname) else { continue };
                match &b.interface {
                    None => target.methods.extend(methods.iter().map(|f| make(f, MemberKind::Method))),
                    Some(iface) => {
                        let iname = docs::type_name(iface).unwrap_or("").to_string();
                        let mut members = Vec::new();
                        for f in &methods {
                            match signature::operator(&iname, &f.name.name) {
                                Some(op) => {
                                    let mut mem = make(f, MemberKind::Operator);
                                    mem.display = op.to_string();
                                    target.operators.push(mem);
                                }
                                None => members.push(make(f, MemberKind::Method)),
                            }
                        }
                        let derived = false;
                        target.implementations.push(Implementation {
                            interface: type_text(text, iface),
                            target: None,
                            derived,
                            instance: inst,
                            where_clause,
                            members,
                        });
                    }
                }
            }
            ast::Item::Extension(x) if cx.keep(x.func.is_priv) && !docs::is_internal_name(&x.func.name.name) => {
                let Some((tm, tname)) = receiver(cx, mi, &x.recv) else { continue };
                let mem = extension_member(cx, text, m, x);
                if let Some(target) = find_item(modules, cx.index, tm, &tname) {
                    target.extensions.push(mem);
                }
            }
            _ => {}
        }
    }
    // The interface of an impl block is linked to its declaration when the package declares it.
    for item in &m.ast.items {
        if let ast::Item::Impl(b) = item
            && let (Some(iface), Some((tm, tname))) = (&b.interface, receiver(cx, mi, &b.ty))
        {
            let resolved = resolve_type(cx, mi, iface);
            if let Some(target) = find_item(modules, cx.index, tm, &tname) {
                let wanted = type_text(text, iface);
                if let Some(imp) =
                    target.implementations.iter_mut().find(|i| i.interface == wanted && i.target.is_none())
                {
                    imp.target = resolved;
                }
            }
        }
    }
}

/// What a type written in the module names, when the package declares it.
fn resolve_type(cx: &Ctx, mi: usize, ty: &Type) -> Option<Resolution> {
    let TypeKind::Path(segments) = &ty.kind else { return None };
    let names: Vec<String> = segments.iter().map(|s| s.name.name.clone()).collect();
    match cx.index.resolve(Some(mi), &names)? {
        (Cursor::Item { module, name }, used) if used == names.len() => {
            let private = cx.index.modules[module].decls.get(&name).is_some_and(|d| d.private);
            (cx.keep(private)).then(|| Resolution::Item {
                module: cx.index.modules[module].path.clone(),
                name,
                member: None,
            })
        }
        _ => None,
    }
}

fn reexports(cx: &Ctx, mi: usize, m: &ModuleInput) -> Vec<Reexport> {
    let mut out = Vec::new();
    for u in m.ast.uses.iter().filter(|u| u.is_pub) {
        let mut binds = Vec::new();
        binds_of(u, &mut binds);
        for b in binds {
            let target = match cx.index.resolve(None, &b.path) {
                Some((Cursor::Item { module, name }, used)) if used == b.path.len() => {
                    Resolution::Item { module: cx.index.modules[module].path.clone(), name, member: None }
                }
                Some((Cursor::Module(path), used)) if used == b.path.len() && cx.index.module_at(&path).is_some() => {
                    Resolution::Module(path)
                }
                _ => Resolution::External,
            };
            let _ = mi;
            out.push(Reexport { name: b.name, path: b.path.join("."), target, span: u.span, line: cx.line(u.span) });
        }
    }
    out
}

// ----- links ----------------------------------------------------------------------------------------------------

/// Resolves the intra-doc links of every doc in the API and reports the broken ones.
fn resolve_links(cx: &Ctx, api: &mut Api, diags: &mut Diagnostics) {
    let snapshot = api.clone();
    for (mi, module) in api.modules.iter_mut().enumerate() {
        // `api.modules` is still in the order of the index here: sorting happens afterwards.
        let docs = std::iter::once((&mut module.docs, true))
            .chain(module.extensions.iter_mut().map(|e| (&mut e.docs, true)))
            .collect::<Vec<_>>();
        for (doc, report) in docs {
            resolve_doc(cx, &snapshot, mi, doc, report, diags);
        }
        for item in &mut module.items {
            resolve_doc(cx, &snapshot, mi, &mut item.docs, true, diags);
            for mem in item.members.iter_mut().chain(&mut item.methods) {
                resolve_doc(cx, &snapshot, mi, &mut mem.docs, true, diags);
            }
            for mem in item.operators.iter_mut() {
                resolve_doc(cx, &snapshot, mi, &mut mem.docs, true, diags);
            }
            // The same declaration is shown on the module page and the type's page: report it once.
            for mem in item.extensions.iter_mut() {
                resolve_doc(cx, &snapshot, mi, &mut mem.docs, false, diags);
            }
            for imp in &mut item.implementations {
                for mem in imp.members.iter_mut() {
                    resolve_doc(cx, &snapshot, mi, &mut mem.docs, true, diags);
                }
            }
        }
    }
}

fn resolve_doc(cx: &Ctx, api: &Api, mi: usize, doc: &mut Option<Doc>, report: bool, diags: &mut Diagnostics) {
    let Some(doc) = doc else { return };
    let blocks = markdown::parse(&doc.text);
    let mut links = Vec::new();
    for raw in markdown::intra_links(&blocks) {
        let resolution = resolve_target(cx, api, mi, &raw.target);
        if report {
            let at = doc.lines.get(raw.line).or(doc.lines.first()).copied();
            if let Some(at) = at {
                match resolution {
                    Resolution::Broken => {
                        diags
                            .warning(BROKEN_LINK, at, format!("unresolved link to `{}` in a doc comment", raw.target))
                            .note_without_span("write `\\[` to show a bracket that is not a link");
                    }
                    Resolution::Hidden => {
                        diags
                            .warning(
                                HIDDEN_LINK,
                                at,
                                format!("`{}` is `priv`, so the documentation has no page to link to", raw.target),
                            )
                            .note_without_span("`burdin doc --private` documents `priv` items");
                    }
                    _ => {}
                }
            }
        }
        links.push(Link { text: raw.text, target: raw.target, line: raw.line, resolution });
    }
    doc.links = links;
}

/// Follows an intra-doc path written in module `mi`: the declaration it names, then a member of it.
fn resolve_target(cx: &Ctx, api: &Api, mi: usize, target: &str) -> Resolution {
    let segments: Vec<String> = target.split('.').map(str::to_string).collect();
    let Some((cursor, used)) = cx.index.resolve(Some(mi), &segments) else { return Resolution::Broken };
    match cursor {
        Cursor::External => Resolution::External,
        Cursor::Module(path) => {
            if used == segments.len() && cx.index.module_at(&path).is_some() {
                Resolution::Module(path)
            } else {
                Resolution::Broken
            }
        }
        Cursor::Item { module, name } => {
            let decl = cx.index.modules[module].decls.get(&name);
            if decl.is_some_and(|d| d.private) && !api.private {
                return Resolution::Hidden;
            }
            let path = cx.index.modules[module].path.clone();
            match segments.len() - used {
                0 => Resolution::Item { module: path, name, member: None },
                1 => {
                    let wanted = &segments[used];
                    let Some(item) = api.item(&path, &name) else { return Resolution::External };
                    match find_member(item, wanted) {
                        Some(true) => Resolution::Item { module: path, name, member: Some(wanted.clone()) },
                        Some(false) => Resolution::Hidden,
                        None => Resolution::Broken,
                    }
                }
                _ => Resolution::Broken,
            }
        }
    }
}

/// Whether an item has a member of that name: `Some(true)` for one shown, `Some(false)` for one that exists but is
/// private and not documented, `None` for none.
fn find_member(item: &Item, name: &str) -> Option<bool> {
    let shown = item
        .members
        .iter()
        .chain(&item.methods)
        .chain(&item.extensions)
        .chain(&item.operators)
        .chain(item.implementations.iter().flat_map(|i| i.members.iter()))
        .any(|m| m.name == name);
    shown.then_some(true)
}

/// A free function for the coverage report and the lint: the line a span is on (1-based).
#[allow(dead_code)]
pub(crate) fn collapse_ws(s: &str) -> String {
    collapse(s)
}
