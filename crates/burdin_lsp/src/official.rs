//! The official packages' public API, read from their sources (`spec/lsp.md`, LS12; `spec/packages.md`, PKG15).
//!
//! The official packages are part of the standard library's surface: a file reaches one with `use json`, with no
//! `[dependencies]` entry and without the package joining the build until something imports it. Completion may
//! therefore ask about a package the package's graph does not have, and it must see the whole public API the compiler
//! would load, `pub use` re-exports included. This index parses every official package once per session; a path or git
//! dependency is not indexed, because it is reachable only through the manifest the package already declares.

use crate::index::{FileIndex, Item, Kind};
use burdin_diag::{Diagnostics, FileId};
use burdin_query::package;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::rc::Rc;

/// The public API of every official package this compiler ships.
pub struct OfficialIndex {
    packages: BTreeMap<String, OfficialPackage>,
}

/// One official package's root API.
pub struct OfficialPackage {
    /// The items the package root exports: its own public top-level items, then the ones its `pub use` declarations
    /// re-export, transitively, in declaration order. A name is listed once.
    pub items: Vec<Item>,
    /// The module names the root re-exports (`pub use hash`) and their aliases, for completion after `crypto.`.
    pub modules: Vec<String>,
    /// The package's root file, spelled as the databases spell it (`src/<name>.bd` or `src/lib.bd`).
    root_file: String,
}

impl OfficialIndex {
    /// Indexes every official package. A package that cannot be read is left out: completion still answers for the
    /// others, and a build reports the error.
    pub fn load() -> OfficialIndex {
        let mut packages = BTreeMap::new();
        for name in package::official_names() {
            let Ok(dir) = package::official_dir(&name) else { continue };
            if let Some(package) = index_package(&name, &dir) {
                packages.insert(name, package);
            }
        }
        OfficialIndex { packages }
    }

    pub fn get(&self, name: &str) -> Option<&OfficialPackage> {
        self.packages.get(name)
    }

    pub fn packages(&self) -> impl Iterator<Item = (&str, &OfficialPackage)> {
        self.packages.iter().map(|(name, package)| (name.as_str(), package))
    }

    /// Whether `file` is `name`'s root module: the file `use <name>` resolves to. A local module or another dependency
    /// that happens to share the name is not.
    pub fn is_root(&self, name: &str, file: &str) -> bool {
        self.packages.get(name).is_some_and(|package| package.root_file == file)
    }
}

fn index_package(name: &str, dir: &Path) -> Option<OfficialPackage> {
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let modules = package::discover_modules_under(&dir.join("src")).ok()?;
    let mut files: BTreeMap<Vec<String>, std::path::PathBuf> = BTreeMap::new();
    for module in modules {
        files.insert(module.path, module.file);
    }
    // Other packages reach this one with `use <package>`: `src/<package>.bd`, then `src/lib.bd` (PKG2).
    let root = files
        .keys()
        .find(|path| path.len() == 1 && (path[0] == name || path[0] == "lib"))
        .cloned()
        .unwrap_or_else(|| vec![name.to_string()]);
    let root_file = package::source_path_text(files.get(&root)?);
    let mut indexes: BTreeMap<Vec<String>, Rc<FileIndex>> = BTreeMap::new();
    for (path, file) in &files {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        let mut diagnostics = Diagnostics::default();
        let ast = burdin_syntax::parse(FileId(0), &text, &mut diagnostics);
        indexes.insert(path.clone(), Rc::new(FileIndex::new(&ast, &text)));
    }
    let mut builder = Builder { indexes: &indexes, items: Vec::new(), modules: Vec::new(), seen: BTreeSet::new() };
    builder.root(&root);
    let Builder { items, modules, .. } = builder;
    Some(OfficialPackage { items, modules, root_file })
}

/// Resolves a module's exported API from its syntax index, following `pub use` re-exports.
struct Builder<'a> {
    indexes: &'a BTreeMap<Vec<String>, Rc<FileIndex>>,
    items: Vec<Item>,
    modules: Vec<String>,
    seen: BTreeSet<String>,
}

impl Builder<'_> {
    /// Adds the root module's own public items and everything it re-exports.
    fn root(&mut self, root: &[String]) {
        let Some(ix) = self.indexes.get(root) else { return };
        let (items, uses) = (ix.items.clone(), ix.uses.clone());
        for item in items.iter().filter(|item| item.parent.is_none()) {
            self.push(item);
        }
        for u in uses.iter().filter(|u| u.is_pub) {
            let path: Vec<String> = u.path.iter().map(|(segment, _)| segment.clone()).collect();
            if !u.items.is_empty() {
                for (name, _) in &u.items {
                    if let Some(item) = self.resolve(&path, name, &mut BTreeSet::new()) {
                        self.push(&item);
                    }
                }
            } else if let Some((alias, _)) = &u.alias {
                if self.indexes.contains_key(&path) && !self.modules.iter().any(|module| module == alias) {
                    self.modules.push(alias.clone());
                }
            } else if self.indexes.contains_key(&path) {
                if let Some(last) = path.last()
                    && !self.modules.iter().any(|module| module == last)
                {
                    self.modules.push(last.clone());
                }
            } else if let Some((name, module)) = path.split_last()
                && let Some(item) = self.resolve(module, name, &mut BTreeSet::new())
            {
                self.push(&item);
            }
        }
    }

    fn push(&mut self, item: &Item) {
        if item.is_priv
            || item.name.starts_with("__")
            || matches!(item.kind, Kind::Impl | Kind::Test | Kind::Method | Kind::Field | Kind::Variant)
        {
            return;
        }
        if self.seen.insert(item.name.clone()) {
            self.items.push(item.clone());
        }
    }

    /// The public item `name` a module exports, directly or through its own `pub use` declarations. `visiting` stops a
    /// cycle of re-exports from recursing forever.
    fn resolve(&self, module: &[String], name: &str, visiting: &mut BTreeSet<Vec<String>>) -> Option<Item> {
        if !visiting.insert(module.to_vec()) {
            return None;
        }
        let ix = self.indexes.get(module)?;
        if let Some(item) = ix.items.iter().find(|item| item.parent.is_none() && !item.is_priv && item.name == name) {
            return Some(item.clone());
        }
        for u in ix.uses.iter().filter(|u| u.is_pub) {
            let path: Vec<String> = u.path.iter().map(|(segment, _)| segment.clone()).collect();
            if u.items.iter().any(|(imported, _)| imported == name)
                && let Some(item) = self.resolve(&path, name, visiting)
            {
                return Some(item);
            }
            if u.items.is_empty()
                && u.alias.is_none()
                && let Some((last, parent)) = path.split_last()
                && last == name
                && let Some(item) = self.resolve(parent, name, visiting)
            {
                return Some(item);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_official_package_exports_its_reexported_api() {
        if package::official_names().is_empty() {
            return;
        }
        let index = OfficialIndex::load();
        let json = index.get("json").expect("json is an official package");
        for name in ["from_str", "to_string", "to_string_pretty"] {
            assert!(json.items.iter().any(|item| item.name == name), "json does not export `{name}`");
        }
        // A package whose root is only `pub use` declarations: the items and the modules are both reachable.
        let crypto = index.get("crypto").expect("crypto is an official package");
        for name in ["HashAlg", "Sha256", "CryptoError", "hmac"] {
            assert!(crypto.items.iter().any(|item| item.name == name), "crypto does not export `{name}`");
        }
        assert!(crypto.modules.iter().any(|module| module == "hash"), "{:?}", crypto.modules);
        let http = index.get("http").expect("http is an official package");
        assert!(http.items.iter().any(|item| item.name == "Client"));
        assert!(index.get("completion").is_none());
        assert!(index.is_root("json", &json.root_file));
    }
}
