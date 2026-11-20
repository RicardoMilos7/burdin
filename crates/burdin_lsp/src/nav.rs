//! Navigation (`spec/lsp.md`, LS8-LS10): what the name under the cursor is, where it is declared, every place that
//! names it, renaming it, implementations, and the call and type hierarchies.
//!
//! A name is resolved in this order: a declaration written at the position; a local's binding; the analysis's answer
//! for an expression (a local, a parameter, a function or method, a constant); a member after a `.` (of a module, of a
//! type, or of a value whose type the analysis knows); a name in a `use` declaration; a field name in a struct
//! literal; and finally a type, function, constant, or variant of that name visible from the file.

use crate::edit::Answering;
use crate::index::{FileIndex, Item, Kind, Tk};
use crate::json::Json;
use crate::project::{Loc, Package};
use crate::server::{Reply, ReqError, Server};
use crate::text::LineIndex;
use burdin_query::{DefinitionKind, LocalInfo};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymKind {
    Local,
    Parameter,
    Module,
    Item(Kind),
}

/// What a name refers to.
#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub kind: SymKind,
    /// The name at its declaration (the whole file, empty, for a module).
    pub def: Loc,
    /// The whole declaration.
    pub span: Loc,
    /// The type of a local or parameter.
    pub ty: Option<String>,
    /// The declaration as written (`fn add(a: i32, b: i32): i32`).
    pub detail: Option<String>,
    pub docs: Option<String>,
    /// The type a method, field, or variant belongs to.
    pub owner: Option<String>,
    /// The interface the `impl` block of a method implements.
    pub interface: Option<String>,
}

impl Symbol {
    fn of_item(path: &str, item: &Item) -> Symbol {
        Symbol {
            name: item.name.clone(),
            kind: SymKind::Item(item.kind),
            def: Loc::new(path, item.name_span.0, item.name_span.1),
            span: Loc::new(path, item.span.0, item.span.1),
            ty: None,
            detail: Some(item.detail.clone()),
            docs: item.docs.clone(),
            owner: item.owner.clone(),
            interface: item.interface.clone(),
        }
    }

    fn of_local(local: &LocalInfo) -> Symbol {
        let def = Loc::from(&local.at);
        Symbol {
            name: local.name.clone(),
            kind: if local.is_parameter { SymKind::Parameter } else { SymKind::Local },
            def: def.clone(),
            span: def,
            ty: Some(local.ty.clone()),
            detail: None,
            docs: None,
            owner: None,
            interface: None,
        }
    }

    fn module(path: &str, name: &str) -> Symbol {
        let def = Loc::new(path, 0, 0);
        Symbol {
            name: name.to_string(),
            kind: SymKind::Module,
            def: def.clone(),
            span: def,
            ty: None,
            detail: Some(format!("module {name}")),
            docs: None,
            owner: None,
            interface: None,
        }
    }

    pub fn is_std(&self) -> bool {
        self.def.path.starts_with("<std>/")
    }
}

/// Answers name questions about one package, with the per-file data it needs computed once.
pub struct Resolver<'a> {
    pub p: &'a mut Package,
    locals: HashMap<String, Rc<Vec<LocalInfo>>>,
    all: Option<Rc<Vec<(String, Item)>>>,
}

impl<'a> Resolver<'a> {
    pub fn new(p: &'a mut Package) -> Resolver<'a> {
        Resolver { p, locals: HashMap::new(), all: None }
    }

    pub fn index(&mut self, path: &str) -> Option<Rc<FileIndex>> {
        self.p.index(path)
    }

    fn locals(&mut self, path: &str) -> Rc<Vec<LocalInfo>> {
        if let Some(l) = self.locals.get(path) {
            return l.clone();
        }
        let list = Rc::new(self.p.current().map(|a| a.locals(path)).unwrap_or_default());
        self.locals.insert(path.to_string(), list.clone());
        list
    }

    /// Every item of every file of the package, the standard library's and the built-ins' included.
    pub fn all_items(&mut self) -> Rc<Vec<(String, Item)>> {
        if let Some(all) = &self.all {
            return all.clone();
        }
        let mut paths: Vec<String> = self.p.modules.iter().map(|m| m.path.clone()).collect();
        paths.push(crate::builtins::PATH.to_string());
        let mut out = Vec::new();
        for path in paths {
            if let Some(ix) = self.p.index(&path) {
                out.extend(ix.items.iter().map(|i| (path.clone(), i.clone())));
            }
        }
        let all = Rc::new(out);
        self.all = Some(all.clone());
        all
    }

    /// How close a declaration in `candidate` is to code in `from`: the same file, the same package, a
    /// dependency, the standard library.
    fn rank(&self, from: &str, candidate: &str) -> u8 {
        if from == candidate {
            0
        } else if candidate.starts_with("<std>/") {
            3
        } else if self.p.package_of(from) == self.p.package_of(candidate) {
            1
        } else {
            2
        }
    }

    /// The best item named `name` that `pred` accepts, as seen from `from`.
    fn find(&mut self, from: &str, name: &str, pred: impl Fn(&Item) -> bool) -> Option<Symbol> {
        let all = self.all_items();
        let imported = self.imported_files(from, name);
        all.iter()
            .filter(|(_, i)| i.name == name && pred(i))
            .min_by_key(|(path, i)| {
                let rank = if imported.contains(path) { 0 } else { self.rank(from, path) };
                (rank, i.parent.is_some(), path.clone(), i.span.0)
            })
            .map(|(path, i)| Symbol::of_item(path, i))
    }

    /// The files a `use` in `from` imports `name` from (`use m.{name}`, `use m.name`).
    fn imported_files(&mut self, from: &str, name: &str) -> Vec<String> {
        let Some(ix) = self.p.index(from) else { return Vec::new() };
        let Some(package) = self.p.package_of(from) else { return Vec::new() };
        let mut out = Vec::new();
        for u in &ix.uses {
            let path: Vec<String> = u.path.iter().map(|(s, _)| s.clone()).collect();
            if u.items.iter().any(|(s, _)| s == name) {
                if let Some(f) = self.p.module_file(package, &path) {
                    out.push(f);
                }
            } else if u.items.is_empty()
                && path.last().is_some_and(|s| s == name)
                && u.alias.is_none()
                && let Some(f) = self.p.module_file(package, &path[..path.len() - 1])
            {
                out.push(f);
            }
        }
        out
    }

    /// The module file a name stands for in `from`: an alias or an imported module, a module of the package, or a
    /// dependency.
    fn module_named(&mut self, from: &str, name: &str) -> Option<String> {
        let ix = self.p.index(from)?;
        let package = self.p.package_of(from)?;
        for u in &ix.uses {
            let path: Vec<String> = u.path.iter().map(|(s, _)| s.clone()).collect();
            if u.alias.as_ref().is_some_and(|(a, _)| a == name) {
                return self.p.module_file(package, &path);
            }
            if u.items.is_empty()
                && u.alias.is_none()
                && path.last().is_some_and(|s| s == name)
                && let Some(f) = self.p.module_file(package, &path)
            {
                return Some(f);
            }
            if u.items.iter().any(|(s, _)| s == name) {
                let mut full = path.clone();
                full.push(name.to_string());
                if let Some(f) = self.p.module_file(package, &full) {
                    return Some(f);
                }
            }
        }
        std_module(self.p, package, name)
    }

    /// A member (field, variant, or method) of a type.
    fn member(&mut self, from: &str, owner: &str, name: &str) -> Option<Symbol> {
        let owner = base_type(owner);
        self.find(from, name, |i| i.owner.as_deref() == Some(owner.as_str()) && i.kind != Kind::Impl)
    }

    /// A member of a value of type `ty`: a member of the type, or, when `ty` is a generic parameter of the function
    /// around `at`, a method one of its bounds declares (`s.serialize_unit` with `S: Serializer`).
    fn member_of(&mut self, from: &str, ix: &FileIndex, at: u32, ty: &str, name: &str) -> Option<Symbol> {
        if let Some(bounds) = ix.bounds_of(at, &base_type(ty)) {
            return self.find(from, name, |i| {
                i.kind == Kind::InterfaceMethod && i.owner.as_ref().is_some_and(|owner| bounds.contains(owner))
            });
        }
        self.member(from, ty, name)
    }

    /// The declaration item an analysis definition (a whole function or constant declaration) stands for.
    fn item_at_definition(&mut self, def: &Loc) -> Option<Symbol> {
        let ix = self.p.index(&def.path)?;
        ix.items
            .iter()
            .filter(|i| i.kind != Kind::Impl && i.span.0 <= def.lo && i.name_span.1 <= def.hi.max(i.name_span.1))
            .filter(|i| def.lo <= i.name_span.0 && i.name_span.1 <= def.hi)
            .min_by_key(|i| i.span.1 - i.span.0)
            .map(|i| Symbol::of_item(&def.path, i))
    }

    /// What the name at a byte offset of a file is.
    pub fn symbol_at(&mut self, path: &str, offset: u32) -> Option<Symbol> {
        let ix = self.p.index(path)?;
        let ti = ix.token_at(offset)?;
        let ti = if ix.tokens[ti].ident().is_some() {
            ti
        } else if ti > 0 && ix.tokens[ti - 1].hi == offset && ix.tokens[ti - 1].ident().is_some() {
            ti - 1
        } else {
            return None;
        };
        let tok = ix.tokens[ti].clone();
        let name = tok.ident()?.to_string();

        // A declaration written here.
        if let Some(n) = ix.item_named_at(tok.lo) {
            return Some(Symbol::of_item(path, &ix.items[n]));
        }
        // A local's binding.
        let locals = self.locals(path);
        if let Some(local) = locals.iter().find(|l| l.at.lo <= tok.lo && tok.lo < l.at.hi) {
            return Some(Symbol::of_local(local));
        }
        // A name inside a `use` declaration.
        if let Some(u) = ix.uses.iter().find(|u| u.span.0 <= tok.lo && tok.hi <= u.span.1).cloned() {
            return self.use_name(path, &u, &tok, &name);
        }
        let prev_dot = ti >= 2 && ix.tokens[ti - 1].kind == Tk::Dot;
        // The analysis's answer for an expression.
        if let Some(analysis) = self.p.current()
            && let Some(r) = analysis.resolve_at(path, tok.lo)
            && r.name == name
        {
            match r.kind {
                DefinitionKind::Local | DefinitionKind::Parameter => {
                    if let Some(def) = &r.definition {
                        let def = Loc::from(def);
                        let local = locals.iter().find(|l| Loc::from(&l.at) == def).cloned();
                        return Some(match local {
                            Some(l) => Symbol::of_local(&l),
                            None => Symbol {
                                name,
                                kind: if r.kind == DefinitionKind::Local { SymKind::Local } else { SymKind::Parameter },
                                def: def.clone(),
                                span: def,
                                ty: Some(r.ty),
                                detail: None,
                                docs: None,
                                owner: None,
                                interface: None,
                            },
                        });
                    }
                }
                DefinitionKind::Function | DefinitionKind::Constant => {
                    if let Some(def) = &r.definition
                        && let Some(s) = self.item_at_definition(&Loc::from(def))
                    {
                        return Some(s);
                    }
                }
            }
        }
        // A member after a `.`.
        if prev_dot {
            let receiver = ix.tokens[ti - 2].clone();
            match &receiver.kind {
                Tk::Ident(r) => {
                    if let Some(file) = self.module_named(path, r)
                        && !self.is_value(path, &receiver)
                    {
                        return self
                            .find(&file, &name, |i| i.parent.is_none() && i.kind != Kind::Impl)
                            .filter(|s| s.def.path == file);
                    }
                    if !self.is_value(path, &receiver)
                        && let Some(owner) = self.find(path, r, |i| i.kind.is_type())
                        && let Some(m) = self.member(path, &owner.name, &name)
                    {
                        return Some(m);
                    }
                }
                // `self` and `Self` name the type of the method (or impl block) around them, while the body may not parse.
                Tk::SelfValue | Tk::SelfType => {
                    if let Some(owner) = ix.self_owner(receiver.lo)
                        && let Some(m) = self.member(path, &owner, &name)
                    {
                        return Some(m);
                    }
                }
                _ => {}
            }
            if let Some(ty) = self.local_type(path, &receiver) {
                return self.member_of(path, &ix, receiver.lo, &ty, &name);
            }
            if let Some(ty) = self.p.current().and_then(|a| a.type_at(path, receiver.lo)) {
                // The innermost expression at the receiver's start may be longer than the receiver (`a.b` at `a` is
                // `a`); only one that ends at the dot is the receiver.
                if ty.at.hi == receiver.hi || ty.at.hi == ix.tokens[ti - 1].lo {
                    return self.member_of(path, &ix, receiver.lo, &ty.ty, &name);
                }
            }
            // A parameter of the function, read from its signature when no analysis answers (`s: S` in `fn f(mut s: S)`).
            if let Tk::Ident(r) = &receiver.kind
                && let Some(ty) = ix.parameter_type(receiver.lo, r)
            {
                return self.member_of(path, &ix, receiver.lo, &ty, &name);
            }
            return None;
        }
        // A field name in a struct literal or pattern (`Rect { w: 1 }`).
        if ix.tokens.get(ti + 1).is_some_and(|t| t.kind == Tk::Colon)
            && let Some(owner) = literal_owner(&ix, ti)
            && let Some(s) = self.member(path, &owner, &name)
        {
            return Some(s);
        }
        // A type, function, constant, or variant visible from here.
        self.find(path, &name, |i| {
            i.kind.is_type() || matches!(i.kind, Kind::Function | Kind::Const | Kind::ExternFn | Kind::Derive)
        })
        .or_else(|| self.find(path, &name, |i| i.kind == Kind::Variant))
    }

    /// Whether the identifier is a local or parameter in scope (so `x.f` is a member of a value, not of a module).
    fn is_value(&mut self, path: &str, tok: &crate::index::Tok) -> bool {
        self.local_type(path, tok).is_some()
    }

    /// The type of the local or parameter an identifier names, when it names one. The analysis answers for a name
    /// its typed tree holds. Otherwise the local declared last before it gives the type (see
    /// [`Answering::local_before`]): a method named without a call (`sq.area`) leaves its receiver out of the typed
    /// tree, and a call still being written (`sq.area(`) makes the text unparseable, so the last good analysis is
    /// the one that knows `sq`.
    fn local_type(&mut self, path: &str, tok: &crate::index::Tok) -> Option<String> {
        let name = tok.ident()?;
        if let Some(analysis) = self.p.current()
            && let Some(r) = analysis.resolve_at(path, tok.lo)
            && r.name == name
            && matches!(r.kind, DefinitionKind::Local | DefinitionKind::Parameter)
        {
            return Some(r.ty);
        }
        Answering::of(&*self.p, path)?.local_before(path, name, tok.lo)
    }

    fn use_name(&mut self, path: &str, u: &crate::index::Use, tok: &crate::index::Tok, name: &str) -> Option<Symbol> {
        let package = self.p.package_of(path)?;
        let segments: Vec<String> = u.path.iter().map(|(s, _)| s.clone()).collect();
        if let Some(pos) = u.path.iter().position(|(_, span)| span.0 == tok.lo) {
            // A segment of the path: a module, or the item a path ends with.
            let prefix = &segments[..=pos];
            if let Some(file) = self.p.module_file(package, prefix) {
                return Some(Symbol::module(&file, name));
            }
            let file = self.p.module_file(package, &segments[..pos])?;
            return self
                .find(&file, name, |i| i.parent.is_none() && i.kind != Kind::Impl)
                .filter(|s| s.def.path == file);
        }
        if u.items.iter().any(|(_, span)| span.0 == tok.lo) {
            let mut full = segments.clone();
            full.push(name.to_string());
            if let Some(file) = self.p.module_file(package, &full) {
                return Some(Symbol::module(&file, name));
            }
            let file = self.p.module_file(package, &segments)?;
            return self
                .find(&file, name, |i| i.parent.is_none() && i.kind != Kind::Impl)
                .filter(|s| s.def.path == file);
        }
        if u.alias.as_ref().is_some_and(|(_, span)| span.0 == tok.lo) {
            let file = self.p.module_file(package, &segments)?;
            return Some(Symbol::module(&file, name));
        }
        None
    }
}

/// A module of the package or of a dependency named `name`, or else the standard-library module the prelude binds to
/// that name (`env`, `fs`).
pub fn std_module(p: &Package, package: usize, name: &str) -> Option<String> {
    p.module_file(package, &[name.to_string()])
        .or_else(|| p.module_file(package, &["std".to_string(), name.to_string()]))
}

/// `List<i32>` -> `List`; `&mut Rect` -> `Rect`.
pub fn base_type(ty: &str) -> String {
    let t = ty.trim().trim_start_matches('&').trim_start();
    let t = t.strip_prefix("mut ").unwrap_or(t);
    t.split(['<', '[', '(']).next().unwrap_or(t).trim().to_string()
}

/// The type a struct literal or pattern that holds token `ti` names: the identifier before its unmatched `{`.
pub fn literal_owner(ix: &FileIndex, ti: usize) -> Option<String> {
    let mut depth = 0i32;
    let mut i = ti;
    while i > 0 {
        i -= 1;
        match ix.tokens[i].kind {
            Tk::RBrace | Tk::RParen | Tk::RBracket => depth += 1,
            Tk::LParen | Tk::LBracket if depth > 0 => depth -= 1,
            Tk::LParen | Tk::LBracket => return None,
            Tk::LBrace if depth > 0 => depth -= 1,
            Tk::LBrace => {
                let before = ix.tokens.get(i.checked_sub(1)?)?;
                let name = before.ident()?;
                return name.starts_with(|c: char| c.is_ascii_uppercase()).then(|| name.to_string());
            }
            _ => {}
        }
    }
    None
}

/// Whether token `ti` is a field written in shorthand (`Rect { w, h }`): the type it belongs to.
pub fn shorthand_owner(ix: &FileIndex, ti: usize) -> Option<String> {
    let prev = ix.tokens.get(ti.checked_sub(1)?)?;
    let next = ix.tokens.get(ti + 1)?;
    if !matches!(prev.kind, Tk::LBrace | Tk::Comma) || !matches!(next.kind, Tk::Comma | Tk::RBrace) {
        return None;
    }
    literal_owner(ix, ti)
}

// ---- requests ---------------------------------------------------------------------------------------------------

/// The symbol at a request's position, from the document's owning package.
fn symbol_for(server: &mut Server, params: &Json) -> Result<Option<(usize, String, Symbol)>, ReqError> {
    let (i, path) = server.document(params, true)?;
    let offset = server.offset(i, &path, params.get("position"))?;
    let symbol = Resolver::new(&mut server.packages[i]).symbol_at(&path, offset);
    Ok(symbol.map(|s| (i, path, s)))
}

pub fn definition(server: &mut Server, params: &Json) -> Reply {
    let Some((_, _, symbol)) = symbol_for(server, params)? else { return Ok(Json::Null) };
    Ok(server.location(&symbol.def))
}

/// Every place that names the symbol, its declaration included. Locals are searched in their file; anything else in
/// every file (outside the standard library) of every package that reads the declaring file.
pub fn references_of(server: &mut Server, symbol: &Symbol) -> Result<Vec<Loc>, ReqError> {
    let mut by_file: BTreeMap<String, usize> = BTreeMap::new();
    let local = matches!(symbol.kind, SymKind::Local | SymKind::Parameter);
    for i in server.packages_with(&symbol.def.path) {
        let files = if local { vec![symbol.def.path.clone()] } else { server.packages[i].user_files() };
        for f in files {
            by_file.entry(f).or_insert(i);
        }
    }
    if symbol.is_std() {
        by_file.remove(&symbol.def.path);
    }
    let mut out: Vec<Loc> = Vec::new();
    if symbol.kind != SymKind::Module && !symbol.def.path.is_empty() {
        out.push(symbol.def.clone());
    }
    let mut analyzed: Vec<usize> = Vec::new();
    for (file, i) in by_file {
        if !analyzed.contains(&i) {
            server.analyzed(i)?;
            analyzed.push(i);
        }
        let mut r = Resolver::new(&mut server.packages[i]);
        let Some(ix) = r.index(&file) else { continue };
        for t in &ix.tokens {
            if t.ident() != Some(symbol.name.as_str()) {
                continue;
            }
            if let Some(s) = r.symbol_at(&file, t.lo)
                && s.def == symbol.def
            {
                out.push(Loc::new(&file, t.lo, t.hi));
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

pub fn references(server: &mut Server, params: &Json) -> Reply {
    let Some((_, _, symbol)) = symbol_for(server, params)? else { return Ok(Json::Null) };
    let include = params.at(&["context", "includeDeclaration"]).as_bool().unwrap_or(true);
    let locs = references_of(server, &symbol)?;
    Ok(Json::Arr(locs.iter().filter(|l| include || **l != symbol.def).map(|l| server.location(l)).collect()))
}

pub fn highlight(server: &mut Server, params: &Json) -> Reply {
    let Some((_, path, symbol)) = symbol_for(server, params)? else { return Ok(Json::Null) };
    let locs = references_of(server, &symbol)?;
    let text = server.text_of(&path).unwrap_or_default();
    Ok(Json::Arr(
        locs.iter()
            .filter(|l| l.path == path)
            .map(|l| {
                let write = *l == symbol.def || is_assigned(&text, l.hi);
                Json::obj().with("range", server.range_of(l)).with("kind", if write { 3 } else { 2 })
            })
            .collect(),
    ))
}

/// Whether the name ending at `end` is the target of an assignment: `=` or a compound assignment follows it.
fn is_assigned(text: &str, end: u32) -> bool {
    let rest = text.get(end as usize..).unwrap_or("").trim_start_matches([' ', '\t']);
    if rest.starts_with("==") || rest.starts_with("=>") {
        return false;
    }
    if rest.starts_with('=') {
        return true;
    }
    ["+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>="].iter().any(|op| rest.starts_with(op))
}

fn renameable(symbol: &Symbol) -> Result<(), ReqError> {
    if symbol.is_std() {
        return Err(ReqError::failed(format!("`{}` is declared in the standard library", symbol.name)));
    }
    match symbol.kind {
        SymKind::Module => Err(ReqError::failed("renaming a module renames its file; do that in the file system")),
        SymKind::Item(Kind::Test) => Err(ReqError::failed("a test's name is a string; edit it in place")),
        _ => Ok(()),
    }
}

pub fn prepare_rename(server: &mut Server, params: &Json) -> Reply {
    let (i, path) = server.document(params, true)?;
    let offset = server.offset(i, &path, params.get("position"))?;
    let mut r = Resolver::new(&mut server.packages[i]);
    let Some(symbol) = r.symbol_at(&path, offset) else { return Ok(Json::Null) };
    renameable(&symbol)?;
    let ix = r.index(&path).ok_or_else(|| ReqError::failed("the file has no syntax tree"))?;
    let tok = ix.ident_at(offset).ok_or_else(|| ReqError::failed("there is no name here"))?;
    let loc = Loc::new(&path, tok.lo, tok.hi);
    Ok(Json::obj().with("range", server.range_of(&loc)).with("placeholder", symbol.name.as_str()))
}

pub fn rename(server: &mut Server, params: &Json) -> Reply {
    let new_name = params.get("newName").as_str().ok_or_else(|| ReqError::params("missing newName"))?.to_string();
    if !crate::index::is_identifier(&new_name) {
        return Err(ReqError::params(format!("`{new_name}` is not a valid name")));
    }
    let Some((_, _, symbol)) = symbol_for(server, params)? else {
        return Err(ReqError::failed("there is no name to rename here"));
    };
    renameable(&symbol)?;
    let mut edits: BTreeMap<String, Vec<(Loc, String)>> = BTreeMap::new();
    for loc in references_of(server, &symbol)? {
        // A field written in shorthand keeps its value: `Rect { w }` -> `Rect { width: w }`; a local used in
        // shorthand keeps the field: `Rect { w }` -> `Rect { w: width }`.
        let i = server.packages_with(&loc.path).first().copied();
        let short = i.and_then(|i| {
            let ix = server.packages[i].index(&loc.path)?;
            let ti = ix.tokens.iter().position(|t| t.lo == loc.lo)?;
            shorthand_owner(&ix, ti)
        });
        let replacement = match (&short, symbol.kind) {
            (Some(_), SymKind::Item(Kind::Field)) => format!("{new_name}: {}", symbol.name),
            (Some(_), SymKind::Local | SymKind::Parameter) if loc != symbol.def => {
                format!("{}: {new_name}", symbol.name)
            }
            _ => new_name.clone(),
        };
        edits.entry(loc.path.clone()).or_default().push((loc, replacement));
    }
    // A field's shorthand sites resolve to the local they read, so they are found separately.
    if symbol.kind == SymKind::Item(Kind::Field)
        && let Some(owner) = &symbol.owner
    {
        for i in server.packages_with(&symbol.def.path) {
            for file in server.packages[i].user_files() {
                let Some(ix) = server.packages[i].index(&file) else { continue };
                for (ti, t) in ix.tokens.iter().enumerate() {
                    if t.ident() == Some(symbol.name.as_str()) && shorthand_owner(&ix, ti).as_deref() == Some(owner) {
                        let loc = Loc::new(&file, t.lo, t.hi);
                        let list = edits.entry(file.clone()).or_default();
                        if !list.iter().any(|(l, _)| *l == loc) {
                            list.push((loc, format!("{new_name}: {}", symbol.name)));
                        }
                    }
                }
            }
        }
    }
    let mut changes = Json::obj();
    for (path, list) in edits {
        let text = server.text_of(&path).unwrap_or_default();
        let index = LineIndex::new(&text, server.encoding);
        let items: Vec<Json> = list
            .iter()
            .map(|(l, new)| Json::obj().with("range", index.range(l.lo, l.hi)).with("newText", new.as_str()))
            .collect();
        changes.set(&server.uri_of(&path), Json::Arr(items));
    }
    Ok(Json::obj().with("changes", changes))
}

/// Go to implementation (LS10): from an interface or one of its methods to the implementations, from an
/// implementing method back to the interface's declaration, and from a type to its `impl` blocks.
pub fn implementation(server: &mut Server, params: &Json) -> Reply {
    let Some((i, _, symbol)) = symbol_for(server, params)? else { return Ok(Json::Null) };
    let mut r = Resolver::new(&mut server.packages[i]);
    let all = r.all_items();
    let mut out: Vec<Loc> = Vec::new();
    match symbol.kind {
        SymKind::Item(Kind::InterfaceMethod) => {
            let iface = symbol.owner.clone().unwrap_or_default();
            for (path, item) in all.iter() {
                if item.kind == Kind::Method && item.name == symbol.name && item.interface.as_deref() == Some(&iface) {
                    out.push(Loc::new(path, item.name_span.0, item.name_span.1));
                }
            }
        }
        SymKind::Item(Kind::Interface) => {
            for (path, item) in all.iter() {
                if item.kind == Kind::Impl && item.interface.as_deref() == Some(&symbol.name) {
                    out.push(Loc::new(path, item.name_span.0, item.name_span.1));
                }
            }
        }
        SymKind::Item(Kind::Method) if symbol.interface.is_some() => {
            let iface = symbol.interface.clone().unwrap_or_default();
            for (path, item) in all.iter() {
                if item.kind == Kind::InterfaceMethod
                    && item.name == symbol.name
                    && item.owner.as_deref() == Some(&iface)
                {
                    out.push(Loc::new(path, item.name_span.0, item.name_span.1));
                }
            }
        }
        SymKind::Item(Kind::Struct | Kind::Enum) => {
            for (path, item) in all.iter() {
                if item.kind == Kind::Impl && item.owner.as_deref() == Some(&symbol.name) {
                    out.push(Loc::new(path, item.name_span.0, item.name_span.1));
                }
            }
        }
        _ => return Ok(Json::Null),
    }
    out.sort();
    out.dedup();
    Ok(Json::Arr(out.iter().map(|l| server.location(l)).collect()))
}

// ---- call hierarchy ---------------------------------------------------------------------------------------------

fn hierarchy_item(server: &Server, symbol: &Symbol, kind: u32) -> Json {
    Json::obj()
        .with("name", symbol.name.as_str())
        .with("kind", kind)
        .with("detail", symbol.detail.clone().unwrap_or_default())
        .with("uri", server.uri_of(&symbol.span.path))
        .with("range", server.range_of(&symbol.span))
        .with("selectionRange", server.range_of(&symbol.def))
        .with(
            "data",
            Json::obj().with("path", symbol.def.path.as_str()).with("lo", symbol.def.lo).with("hi", symbol.def.hi),
        )
}

fn symbol_kind_number(kind: Kind) -> u32 {
    match kind {
        Kind::Function | Kind::ExternFn | Kind::Test => 12,
        Kind::Method | Kind::InterfaceMethod => 6,
        Kind::Struct => 23,
        Kind::Enum => 10,
        Kind::Variant => 22,
        Kind::Field => 8,
        Kind::Interface => 11,
        Kind::Const => 14,
        Kind::Alias => 26,
        Kind::Impl => 19,
        Kind::Derive => 11,
    }
}

pub fn symbol_kind(kind: Kind) -> u32 {
    symbol_kind_number(kind)
}

pub fn prepare_call_hierarchy(server: &mut Server, params: &Json) -> Reply {
    let Some((_, _, symbol)) = symbol_for(server, params)? else { return Ok(Json::Null) };
    let SymKind::Item(kind) = symbol.kind else { return Ok(Json::Null) };
    if !kind.is_function() && kind != Kind::Test {
        return Ok(Json::Null);
    }
    Ok(Json::Arr(vec![hierarchy_item(server, &symbol, symbol_kind_number(kind))]))
}

/// The symbol a hierarchy item stands for, from its `data`.
fn item_symbol(server: &mut Server, item: &Json) -> Result<Option<(usize, Symbol)>, ReqError> {
    let data = item.get("data");
    let path = data.get("path").as_str().ok_or_else(|| ReqError::params("missing item data"))?.to_string();
    let lo = data.get("lo").as_u32().unwrap_or(0);
    let Some(i) = server.owner(&path).or_else(|| server.packages_with(&path).first().copied()) else {
        return Ok(None);
    };
    server.analyzed(i)?;
    let symbol = Resolver::new(&mut server.packages[i]).symbol_at(&path, lo);
    Ok(symbol.map(|s| (i, s)))
}

/// The function item whose declaration starts at `decl`: the caller or callee of a call.
fn function_symbol(r: &mut Resolver, decl: &Loc) -> Option<Symbol> {
    let ix = r.index(&decl.path)?;
    ix.items
        .iter()
        .filter(|i| {
            (i.kind.is_function() || i.kind == Kind::Test)
                && i.span.0 <= decl.lo
                && decl.lo <= i.name_span.0
                && i.name_span.1 <= decl.hi
        })
        .min_by_key(|i| i.span.1 - i.span.0)
        .map(|i| Symbol::of_item(&decl.path, i))
        .or_else(|| {
            ix.items
                .iter()
                .filter(|i| i.kind == Kind::Test && i.span.0 <= decl.lo && decl.hi <= i.span.1)
                .min_by_key(|i| i.span.1 - i.span.0)
                .map(|i| Symbol::of_item(&decl.path, i))
        })
}

/// The name of the callee inside a call expression: the last occurrence of the name before the argument list
/// (`a.b.area()` names `area`, `Rect.new(1)` names `new`). `None` for a call the source does not write by name,
/// such as the formatting calls of an interpolated string or an overloaded operator.
fn callee_name_loc(r: &mut Resolver, at: &Loc, name: &str) -> Option<Loc> {
    let ix = r.index(&at.path)?;
    let paren = ix.tokens.iter().find(|p| p.lo >= at.lo && p.kind == Tk::LParen).map_or(at.hi, |p| p.lo);
    let t = ix.tokens.iter().rfind(|t| at.lo <= t.lo && t.hi <= paren && t.ident() == Some(name))?;
    Some(Loc::new(&at.path, t.lo, t.hi))
}

pub fn incoming_calls(server: &mut Server, params: &Json) -> Reply {
    let Some((_, target)) = item_symbol(server, params.get("item"))? else { return Ok(Json::Null) };
    // Grouped by caller declaration.
    let mut groups: BTreeMap<Loc, (Symbol, Vec<Loc>)> = BTreeMap::new();
    for i in server.packages_with(&target.def.path) {
        server.analyzed(i)?;
        let calls = server.packages[i].current().map(|a| a.calls()).unwrap_or_default();
        let mut r = Resolver::new(&mut server.packages[i]);
        for call in calls {
            let Some(callee) = &call.callee_location else { continue };
            if !Loc::from(callee).contains(&target.def) {
                continue;
            }
            let Some(caller) = call.caller_location.as_ref().map(Loc::from) else { continue };
            let Some(symbol) = function_symbol(&mut r, &caller) else { continue };
            let Some(at) = callee_name_loc(&mut r, &Loc::from(&call.at), &call.callee) else { continue };
            let entry = groups.entry(symbol.def.clone()).or_insert_with(|| (symbol, Vec::new()));
            if !entry.1.contains(&at) {
                entry.1.push(at);
            }
        }
    }
    let out: Vec<Json> = groups
        .values()
        .map(|(symbol, ranges)| {
            let kind = match symbol.kind {
                SymKind::Item(k) => symbol_kind_number(k),
                _ => 12,
            };
            Json::obj()
                .with("from", hierarchy_item(server, symbol, kind))
                .with("fromRanges", Json::Arr(ranges.iter().map(|l| server.range_of(l)).collect()))
        })
        .collect();
    Ok(Json::Arr(out))
}

pub fn outgoing_calls(server: &mut Server, params: &Json) -> Reply {
    let Some((i, source)) = item_symbol(server, params.get("item"))? else { return Ok(Json::Null) };
    let calls = server.packages[i].current().map(|a| a.calls()).unwrap_or_default();
    let mut groups: BTreeMap<Loc, (Symbol, Vec<Loc>)> = BTreeMap::new();
    let mut r = Resolver::new(&mut server.packages[i]);
    for call in calls {
        let Some(caller) = call.caller_location.as_ref().map(Loc::from) else { continue };
        if !caller.contains(&source.def) {
            continue;
        }
        let Some(callee) = call.callee_location.as_ref().map(Loc::from) else { continue };
        let Some(symbol) = function_symbol(&mut r, &callee) else { continue };
        let Some(at) = callee_name_loc(&mut r, &Loc::from(&call.at), &call.callee) else { continue };
        let entry = groups.entry(symbol.def.clone()).or_insert_with(|| (symbol, Vec::new()));
        if !entry.1.contains(&at) {
            entry.1.push(at);
        }
    }
    let out: Vec<Json> = groups
        .values()
        .map(|(symbol, ranges)| {
            let kind = match symbol.kind {
                SymKind::Item(k) => symbol_kind_number(k),
                _ => 12,
            };
            Json::obj()
                .with("to", hierarchy_item(server, symbol, kind))
                .with("fromRanges", Json::Arr(ranges.iter().map(|l| server.range_of(l)).collect()))
        })
        .collect();
    Ok(Json::Arr(out))
}

// ---- type hierarchy ---------------------------------------------------------------------------------------------

pub fn prepare_type_hierarchy(server: &mut Server, params: &Json) -> Reply {
    let Some((_, _, symbol)) = symbol_for(server, params)? else { return Ok(Json::Null) };
    let SymKind::Item(kind) = symbol.kind else { return Ok(Json::Null) };
    if !kind.is_type() {
        return Ok(Json::Null);
    }
    Ok(Json::Arr(vec![hierarchy_item(server, &symbol, symbol_kind_number(kind))]))
}

/// Supertypes: the interfaces a type implements, or an interface requires. Subtypes: the types that implement an
/// interface, and the interfaces that require it.
pub fn type_hierarchy(server: &mut Server, params: &Json, up: bool) -> Reply {
    let Some((i, symbol)) = item_symbol(server, params.get("item"))? else { return Ok(Json::Null) };
    let mut r = Resolver::new(&mut server.packages[i]);
    let all = r.all_items();
    let from = symbol.def.path.clone();
    let mut names: Vec<String> = Vec::new();
    let mut found: Vec<Symbol> = Vec::new();
    for (path, item) in all.iter() {
        if up {
            if item.kind == Kind::Impl
                && item.owner.as_deref() == Some(&symbol.name)
                && let Some(iface) = &item.interface
            {
                names.push(iface.clone());
            }
            if item.kind == Kind::Interface && Loc::new(path, item.name_span.0, item.name_span.1) == symbol.def {
                names.extend(item.requires.iter().cloned());
            }
        } else {
            if item.kind == Kind::Impl
                && item.interface.as_deref() == Some(&symbol.name)
                && let Some(owner) = &item.owner
            {
                names.push(owner.clone());
            }
            if item.kind == Kind::Interface && item.requires.contains(&symbol.name) {
                found.push(Symbol::of_item(path, item));
            }
        }
    }
    names.sort();
    names.dedup();
    for name in names {
        if let Some(s) = r.find(&from, &name, |i| i.kind.is_type()) {
            found.push(s);
        }
    }
    found.sort_by(|a, b| a.def.cmp(&b.def));
    found.dedup_by(|a, b| a.def == b.def);
    Ok(Json::Arr(
        found
            .iter()
            .map(|s| {
                let kind = match s.kind {
                    SymKind::Item(k) => symbol_kind_number(k),
                    _ => 5,
                };
                hierarchy_item(server, s, kind)
            })
            .collect(),
    ))
}
