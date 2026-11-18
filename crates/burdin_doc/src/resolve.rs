//! Names in the documented package: which module declares a name, what a `use` brings in, and how a path such as
//! `shapes.Circle` or `Circle.area` is followed (`spec/doc.md`, DC8).
//!
//! The rules are the language's own for a path written in code, applied to the package's modules: a name is looked
//! up in the module it is written in (its declarations, then its `use` names), then as a module of the package, then
//! as a name from outside the package (the standard library, a dependency, a primitive type). Only the package being
//! documented is followed in detail; anything that leaves it resolves to [`Cursor::External`].

use crate::model::Kind;
use crate::{Input, PackageInput};
use burdin_syntax::ast::{self, Item};
use burdin_syntax::docs;
use std::collections::{HashMap, HashSet};

/// What a declaration is, for name lookup.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Decl {
    pub private: bool,
}

/// A name a `use` declaration brings into a module.
#[derive(Clone, Debug)]
pub(crate) struct Bind {
    pub name: String,
    pub path: Vec<String>,
    /// Declared with `pub use`.
    pub public: bool,
}

pub(crate) struct ModIndex {
    pub path: Vec<String>,
    pub decls: HashMap<String, Decl>,
    pub binds: Vec<Bind>,
}

/// Where a path has led so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Cursor {
    /// A module of the package, by path. The path may be a directory with no file of its own.
    Module(Vec<String>),
    /// A declaration: the index of its module and its name.
    Item { module: usize, name: String },
    /// Outside the package.
    External,
}

pub(crate) struct Index {
    pub modules: Vec<ModIndex>,
    externals: HashSet<String>,
}

/// Names the compiler provides without any source: the primitive and C types, the built-in containers and enums, their
/// variants, and the output and panic functions. They resolve to [`Cursor::External`].
const PRIMITIVES: &[&str] = &[
    "i8",
    "i16",
    "i32",
    "i64",
    "isize",
    "u8",
    "u16",
    "u32",
    "u64",
    "usize",
    "f32",
    "f64",
    "bool",
    "char",
    "str",
    "String",
    "CString",
    "CChar",
    "CSize",
    "CVoid",
    "CPtr",
    "List",
    "Map",
    "Heap",
    "Option",
    "Result",
    "Range",
    "RangeInclusive",
    "Shared",
    "Some",
    "None",
    "Ok",
    "Err",
    "Self",
    "std",
    "print",
    "println",
    "eprint",
    "eprintln",
    "panic",
    "format",
];

/// The name, kind, and privacy of a declaration that other code can refer to.
pub(crate) fn declaration(item: &Item, text: &str) -> Option<(String, Kind, bool)> {
    let (name, kind, private) = match item {
        Item::Fn(f) => (&f.name.name, Kind::Function, f.is_priv),
        Item::Struct(s) => (&s.name.name, Kind::Struct, s.is_priv),
        Item::Enum(e) => {
            let kind = if docs::is_error_enum(text, e.span) { Kind::ErrorEnum } else { Kind::Enum };
            (&e.name.name, kind, e.is_priv)
        }
        Item::Interface(i) => (&i.name.name, Kind::Interface, i.is_priv),
        Item::TypeAlias(a) => (&a.name.name, Kind::TypeAlias, a.is_priv),
        Item::Const(c) => (&c.name.name, Kind::Constant, c.is_priv),
        Item::Derive(d) => (&d.name.name, Kind::Derive, d.is_priv),
        Item::ExternFn(x) => (&x.decl.name.name, Kind::ExternFunction, x.decl.is_priv),
        _ => return None,
    };
    if docs::is_internal_name(name) {
        return None;
    }
    Some((name.clone(), kind, private))
}

impl Index {
    pub fn new(input: &Input) -> Index {
        let entry = &input.packages[input.entry];
        let mut modules = Vec::new();
        for module in &entry.modules {
            let text = &input.sources.get(module.file).text;
            let mut decls = HashMap::new();
            for item in &module.ast.items {
                if let Some((name, kind, private)) = declaration(item, text) {
                    let _ = kind;
                    decls.insert(name, Decl { private });
                }
                if let Item::Extern(block) = item {
                    for d in &block.decls {
                        decls.insert(d.name.name.clone(), Decl { private: block.is_priv || d.is_priv });
                    }
                }
            }
            let mut binds = Vec::new();
            for u in &module.ast.uses {
                binds_of(u, &mut binds);
            }
            modules.push(ModIndex { path: module.path.to_vec(), decls, binds });
        }
        let mut externals: HashSet<String> = PRIMITIVES.iter().map(|s| s.to_string()).collect();
        for (i, p) in input.packages.iter().enumerate() {
            if i == input.entry {
                continue;
            }
            externals.insert(p.name.to_string());
            if p.is_std {
                externals.extend(std_names(p, input));
            }
        }
        for (name, _) in entry.deps {
            externals.insert(name.clone());
        }
        Index { modules, externals }
    }

    pub fn module_at(&self, path: &[String]) -> Option<usize> {
        self.modules.iter().position(|m| m.path == path)
    }

    /// Whether `path` is a module's path or the start of one (a directory of modules).
    fn is_prefix(&self, path: &[String]) -> bool {
        self.modules.iter().any(|m| m.path.starts_with(path))
    }

    /// Follows `segments` as far as the package's own names go. Written in module `from` (an index), or from the
    /// package root when `from` is `None`. Returns where the path ended and how many segments that used; a path
    /// that stops short at an item continues with that item's members, which only the model knows.
    pub fn resolve(&self, from: Option<usize>, segments: &[String]) -> Option<(Cursor, usize)> {
        self.resolve_depth(from, segments, 0)
    }

    fn resolve_depth(&self, from: Option<usize>, segments: &[String], depth: usize) -> Option<(Cursor, usize)> {
        if depth > 16 || segments.is_empty() {
            return None;
        }
        let mut cursor = self.first(from, &segments[0], depth)?;
        let mut used = 1;
        while used < segments.len() {
            match cursor {
                Cursor::Item { .. } => break,
                Cursor::External => return Some((Cursor::External, segments.len())),
                Cursor::Module(_) => {
                    cursor = self.step(&cursor, &segments[used], depth)?;
                    used += 1;
                }
            }
        }
        Some((cursor, used))
    }

    fn first(&self, from: Option<usize>, name: &str, depth: usize) -> Option<Cursor> {
        if let Some(m) = from {
            if self.modules[m].decls.contains_key(name) {
                return Some(Cursor::Item { module: m, name: name.to_string() });
            }
            for bind in self.modules[m].binds.iter().filter(|b| b.name == name) {
                if let Some(c) = self.bound(bind, depth) {
                    return Some(c);
                }
            }
        }
        if self.is_prefix(&[name.to_string()]) {
            return Some(Cursor::Module(vec![name.to_string()]));
        }
        self.externals.contains(name).then_some(Cursor::External)
    }

    /// What a `use` name stands for.
    fn bound(&self, bind: &Bind, depth: usize) -> Option<Cursor> {
        let (cursor, used) = self.resolve_depth(None, &bind.path, depth + 1)?;
        (used == bind.path.len()).then_some(cursor)
    }

    fn step(&self, cursor: &Cursor, name: &str, depth: usize) -> Option<Cursor> {
        match cursor {
            Cursor::External => Some(Cursor::External),
            Cursor::Item { .. } => None,
            Cursor::Module(path) => {
                if let Some(m) = self.module_at(path) {
                    if self.modules[m].decls.contains_key(name) {
                        return Some(Cursor::Item { module: m, name: name.to_string() });
                    }
                    for bind in self.modules[m].binds.iter().filter(|b| b.public && b.name == name) {
                        if let Some(c) = self.bound(bind, depth) {
                            return Some(c);
                        }
                    }
                }
                let mut extended = path.clone();
                extended.push(name.to_string());
                self.is_prefix(&extended).then_some(Cursor::Module(extended))
            }
        }
    }
}

/// The names one `use` declaration binds.
pub(crate) fn binds_of(u: &ast::UseDecl, out: &mut Vec<Bind>) {
    let path: Vec<String> = u.path.iter().map(|i| i.name.clone()).collect();
    if u.items.is_empty() {
        if let Some(last) = path.last() {
            let name = u.alias.as_ref().map_or_else(|| last.clone(), |a| a.name.clone());
            out.push(Bind { name, path, public: u.is_pub });
        }
    } else {
        for item in &u.items {
            let mut full = path.clone();
            full.push(item.name.clone());
            out.push(Bind { name: item.name.clone(), path: full, public: u.is_pub });
        }
    }
}

/// The names the standard library declares at module level, and its module names: what a path can start with
/// without a `use`.
fn std_names(package: &PackageInput, input: &Input) -> Vec<String> {
    let mut names = Vec::new();
    for module in &package.modules {
        if let Some(head) = module.path.first() {
            names.push(head.clone());
        }
        let text = &input.sources.get(module.file).text;
        for item in &module.ast.items {
            if let Some((name, _, false)) = declaration(item, text) {
                names.push(name);
            }
        }
    }
    names
}
