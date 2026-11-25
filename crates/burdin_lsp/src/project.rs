//! The packages a session analyzes (`spec/lsp.md`, LS4): one query database per package root, the analysis of each,
//! and the syntax index of each file. A file is *owned* by the package whose `burdin.toml` is the nearest one above
//! it (or by itself, for a file outside every package); its diagnostics come from its owner. A dependency's files are
//! also part of every package that depends on it, so an edit reaches every database that reads the file.

use crate::builtins;
use crate::index::FileIndex;
use burdin_query::EntryMode;
use burdin_query::package;
use burdin_query::{Analysis, Cancelled, Db, ModuleInfo, Options, PackageInfo, Parsed};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// A stretch of a file, as the server passes it around: the path as the database spells it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Loc {
    pub path: String,
    pub lo: u32,
    pub hi: u32,
}

impl Loc {
    pub fn new(path: &str, lo: u32, hi: u32) -> Loc {
        Loc { path: path.to_string(), lo, hi }
    }

    pub fn contains(&self, other: &Loc) -> bool {
        self.path == other.path && self.lo <= other.lo && other.hi <= self.hi
    }
}

impl From<&burdin_query::Location> for Loc {
    fn from(l: &burdin_query::Location) -> Loc {
        Loc { path: l.path.clone(), lo: l.lo, hi: l.hi }
    }
}

/// The text of each file of a package, by path, as an analysis saw them.
pub type Texts = HashMap<String, Rc<str>>;

/// One package root and its database.
pub struct Package {
    /// The package directory, or the file itself for a file outside every package.
    pub root: PathBuf,
    pub name: String,
    pub db: Db,
    /// The latest analysis, `None` before the first.
    pub analysis: Option<Analysis>,
    /// The latest analysis that produced a program, with the text of every file at that time (completion and
    /// signature help read it while the current text has a syntax error).
    pub last_good: Option<(Rc<Analysis>, Texts)>,
    /// The inputs changed since the latest analysis.
    pub dirty: bool,
    pub modules: Vec<ModuleInfo>,
    pub packages: Vec<PackageInfo>,
    /// The `[lint]` table of the entry package's manifest.
    pub lint: Vec<(String, String, usize)>,
    /// The files of the entry package whose diagnostics were published non-empty last time.
    pub published: Vec<String>,
    pub analyses: u64,
    /// The linter's findings for one analysis revision.
    pub lint_cache: Option<(u64, Vec<burdin_diag::Diagnostic>)>,
    /// The texts the latest analysis with a program was computed on.
    good_texts: Texts,
    indexes: HashMap<String, (Rc<Parsed>, Rc<FileIndex>)>,
    /// The index of the built-ins' file (`builtins.rs`), made on first use.
    builtins: Option<Rc<FileIndex>>,
}

impl Package {
    /// Loads a package directory or a single file. `texts` are the open documents, by path, which replace the text
    /// on disk.
    pub fn load(root: &Path, texts: &HashMap<String, String>) -> Result<Package, String> {
        // The open documents are the texts an editor shows, so the loader finds the official packages they import
        // even before the file is saved (`spec/packages.md`, PKG15).
        let workspace = package::load_for_tests_with_texts(root, texts)?;
        let mut overrides: HashMap<PathBuf, String> = HashMap::new();
        for p in &workspace.packages {
            for m in &p.modules {
                if let Some(text) = texts.get(&package::source_path_text(&m.file)) {
                    overrides.insert(m.file.clone(), text.clone());
                }
            }
        }
        let name = workspace.packages[workspace.entry_package].name.clone();
        let lint = workspace.packages[workspace.entry_package].lint.clone();
        let db = Db::from_workspace(&workspace, &overrides, Options { mode: EntryMode::Test, ..Options::default() })?;
        Ok(Package::new(root, name, db, lint))
    }

    fn new(root: &Path, name: String, db: Db, lint: Vec<(String, String, usize)>) -> Package {
        let modules = db.modules();
        let packages = db.packages();
        Package {
            root: root.to_path_buf(),
            name,
            db,
            analysis: None,
            last_good: None,
            dirty: true,
            modules,
            packages,
            lint,
            published: Vec::new(),
            analyses: 0,
            lint_cache: None,
            good_texts: HashMap::new(),
            indexes: HashMap::new(),
            builtins: None,
        }
    }

    pub fn has_file(&self, path: &str) -> bool {
        self.db.file_id(path).is_some()
    }

    /// The files of the entry package (the ones this package publishes diagnostics for), without the modules a
    /// build script generated under `target`.
    pub fn own_files(&self) -> Vec<String> {
        let entry = self.packages.iter().position(|p| p.is_entry).unwrap_or(0);
        let generated = self.packages[entry].dir.as_ref().map(|d| package::source_path_text(&d.join("target")));
        self.modules
            .iter()
            .filter(|m| m.package == entry && !generated.as_ref().is_some_and(|g| m.path.starts_with(g.as_str())))
            .map(|m| m.path.clone())
            .collect()
    }

    /// The files of every package except the embedded standard library.
    pub fn user_files(&self) -> Vec<String> {
        self.modules.iter().filter(|m| !self.packages[m.package].embedded).map(|m| m.path.clone()).collect()
    }

    pub fn module(&self, path: &str) -> Option<&ModuleInfo> {
        self.modules.iter().find(|m| m.path == path)
    }

    /// Sets a file's text. Returns whether it changed.
    pub fn set_text(&mut self, path: &str, text: &str) -> bool {
        if self.has_file(path) && self.db.set_file(path, text) {
            self.dirty = true;
            return true;
        }
        false
    }

    /// Whether a current text of a package in the database imports an official package the database's layout does
    /// not have. A database opens with a fixed package set, so the server reloads when this is true: a new `use json`
    /// in an open file must load `json` (`spec/packages.md`, PKG15). A dependency's open file counts too.
    pub fn missing_official_import(&self) -> bool {
        for (index, package) in self.packages.iter().enumerate().filter(|(_, p)| !p.embedded) {
            let mut wanted = std::collections::BTreeSet::new();
            let mut local = std::collections::BTreeSet::new();
            for module in self.modules.iter().filter(|m| m.package == index) {
                if let Some(first) = module.module.first() {
                    local.insert(first.as_str());
                }
                if let Some(text) = self.db.file_text(&module.path) {
                    wanted.extend(package::official_uses(text));
                }
            }
            let missing = wanted.into_iter().any(|name| {
                name != package.name
                    && !local.contains(name.as_str())
                    && !package.deps.iter().any(|(dep, _)| dep == &name)
            });
            if missing {
                return true;
            }
        }
        false
    }

    /// Analyzes the package if its inputs changed since the last analysis.
    pub fn analyze(&mut self) -> Result<&Analysis, Cancelled> {
        if self.dirty || self.analysis.is_none() {
            let analysis = self.db.analyze()?;
            self.analyses += 1;
            self.dirty = false;
            if analysis.program.is_some() {
                // The good analysis is kept separately only while the latest one has no program.
                self.last_good = None;
                self.good_texts = self.snapshot_texts();
            } else if let Some(previous) = self.analysis.take().filter(|a| a.program.is_some()) {
                self.last_good = Some((Rc::new(previous), std::mem::take(&mut self.good_texts)));
            }
            self.analysis = Some(analysis);
        }
        Ok(self.analysis.as_ref().expect("analyzed above"))
    }

    /// The current text of every file outside the standard library.
    fn snapshot_texts(&self) -> Texts {
        self.user_files()
            .into_iter()
            .filter_map(|p| {
                let text = self.db.file_text(&p)?;
                Some((p, Rc::from(text)))
            })
            .collect()
    }

    /// The analysis to answer from: the latest when it has a typed program (a partial one beside a syntax error is
    /// current for every statement the parser kept), else the last good one, with the texts it was computed on
    /// (`None` for the latest, whose texts are current).
    pub fn program_analysis(&self) -> Option<(&Analysis, Option<&Texts>)> {
        match &self.analysis {
            Some(a) if a.has_types() => Some((a, None)),
            _ => self.last_good.as_ref().map(|(a, texts)| (&**a, Some(texts))),
        }
    }

    /// The last good analysis, when the latest one is partial: it still knows what a statement the parser dropped
    /// (the one being typed) said, for the answers that map across the edit.
    pub fn older_analysis(&self) -> Option<(&Analysis, &Texts)> {
        match (&self.analysis, &self.last_good) {
            (Some(a), Some((old, texts))) if a.is_partial() => Some((&**old, texts)),
            _ => None,
        }
    }

    /// The analysis of the current texts with a program, if the latest one is current.
    pub fn current(&self) -> Option<&Analysis> {
        self.analysis.as_ref().filter(|a| a.program.is_some() && !self.dirty)
    }

    /// The analysis of the current texts that answers position queries (types, resolution, locals): like
    /// [`Package::current`], but a partial program beside a syntax error counts (`spec/lsp.md`, LS21).
    pub fn current_typed(&self) -> Option<&Analysis> {
        self.analysis.as_ref().filter(|a| a.has_types() && !self.dirty)
    }

    pub fn text(&self, path: &str) -> Option<&str> {
        if path == builtins::PATH {
            return Some(&builtins::get().text);
        }
        self.db.file_text(path)
    }

    /// The syntax index of a file at its current text.
    pub fn index(&mut self, path: &str) -> Option<Rc<FileIndex>> {
        if path == builtins::PATH {
            return Some(self.builtins.get_or_insert_with(|| Rc::new(builtins::get().index.clone())).clone());
        }
        let parsed = self.db.syntax(path)?;
        if let Some((p, index)) = self.indexes.get(path)
            && Rc::ptr_eq(p, &parsed)
        {
            return Some(index.clone());
        }
        let index = Rc::new(FileIndex::new(&parsed.ast, &parsed.text));
        self.indexes.insert(path.to_string(), (parsed, index.clone()));
        Some(index)
    }

    /// The parsed tree of a file at its current text.
    pub fn parsed(&mut self, path: &str) -> Option<Rc<Parsed>> {
        self.db.syntax(path)
    }

    /// The package a file belongs to (an index into [`Package::packages`]).
    pub fn package_of(&self, path: &str) -> Option<usize> {
        self.module(path).map(|m| m.package)
    }

    /// The file of module `segments` reached from a file of package `from`: a module of the same package, or a
    /// module of a dependency named by the first segment (the dependency's root module when nothing follows).
    pub fn module_file(&self, from: usize, segments: &[String]) -> Option<String> {
        if segments.is_empty() {
            return None;
        }
        if let Some(m) = self.modules.iter().find(|m| m.package == from && m.module == segments) {
            return Some(m.path.clone());
        }
        let (dep, rest) = segments.split_first()?;
        let &(_, target) = self.packages.get(from)?.deps.iter().find(|(name, _)| name == dep)?;
        if rest.is_empty() {
            return self.packages[target].root_file.clone();
        }
        self.modules.iter().find(|m| m.package == target && m.module == rest).map(|m| m.path.clone())
    }

    /// Every module file reachable from package `from` with the path that names it: its own modules and its
    /// dependencies' (as `dep.module`, and `dep` for a root module).
    pub fn visible_modules(&self, from: usize) -> Vec<(Vec<String>, String)> {
        let mut out: Vec<(Vec<String>, String)> = Vec::new();
        for m in self.modules.iter().filter(|m| m.package == from && !m.module.is_empty()) {
            out.push((m.module.clone(), m.path.clone()));
        }
        if let Some(p) = self.packages.get(from) {
            for (name, target) in &p.deps {
                if let Some(root) = &self.packages[*target].root_file {
                    out.push((vec![name.clone()], root.clone()));
                }
                // The dependency's root module is the dependency's own name, not a module below it.
                let root = self.packages[*target].root_file.as_deref();
                for m in self.modules.iter().filter(|m| m.package == *target && Some(m.path.as_str()) != root) {
                    let mut path = vec![name.clone()];
                    path.extend(m.module.iter().cloned());
                    if !out.iter().any(|(p, _)| *p == path) {
                        out.push((path, m.path.clone()));
                    }
                }
            }
            // An official package is available to `use` without a manifest entry, so completion offers every one
            // that is not already loaded and does not collide with a module of this package (PKG15).
            let package_name = self.packages[from].name.clone();
            let deps = self.packages[from].deps.clone();
            for name in package::official_names() {
                let taken = name == package_name
                    || deps.iter().any(|(dep, _)| dep == &name)
                    || self.modules.iter().any(|m| m.package == from && m.module.first() == Some(&name))
                    || out.iter().any(|(path, _)| path.len() == 1 && path[0] == name);
                if !taken {
                    out.push((vec![name], String::new()));
                }
            }
        }
        out
    }
}

/// The path of a file as the databases spell it: canonical when it exists, without Windows' verbatim prefix.
pub fn path_key(path: &Path) -> String {
    package::source_path_text(&canonical_or_nearest(path))
}

/// The canonical path, or, for a path that does not exist yet (the target of a rename), the canonical form of its
/// nearest existing ancestor with the missing names appended, so it spells the same as the paths of the files beside it.
fn canonical_or_nearest(path: &Path) -> PathBuf {
    if let Ok(found) = path.canonicalize() {
        return found;
    }
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut missing = Vec::new();
    let mut ancestor = path.as_path();
    while let (Some(parent), Some(name)) = (ancestor.parent(), ancestor.file_name()) {
        missing.push(name.to_os_string());
        if let Ok(found) = parent.canonicalize() {
            return missing.iter().rev().fold(found, |dir, name| dir.join(name));
        }
        ancestor = parent;
    }
    path
}

/// The root a file belongs to: the nearest directory above it with a `burdin.toml`, or the file itself.
pub fn root_of(path: &Path) -> PathBuf {
    match package::manifest_dir(path) {
        Some(dir) => dir.canonicalize().unwrap_or(dir),
        None => path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
    }
}

/// The package directories under a workspace folder: the folder itself when it has a `burdin.toml`, and every
/// directory below it that has one, skipping `target`, hidden directories, and `node_modules`.
pub fn discover(folder: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(folder, 0, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    // A virtual workspace root has no package of its own (WS18): its members are found below it.
    if dir.join("burdin.toml").is_file() && !burdin_query::package::is_virtual_root(dir) {
        out.push(dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()));
    }
    if depth >= 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            !name.starts_with('.') && name != "target" && name != "node_modules"
        })
        .collect();
    dirs.sort();
    for d in dirs {
        walk(&d, depth + 1, out);
    }
}

/// Where the embedded standard library's files are written so an editor can open them: `<temp>/burdin-lsp-std-<v>`.
pub fn std_dir() -> PathBuf {
    std::env::temp_dir().join(format!("burdin-lsp-std-{}", env!("CARGO_PKG_VERSION")))
}

/// The installed standard-library sources of the toolchain this server runs from: `<toolchain>/lib/std/src`, which
/// the `std-src` component installs. The toolchain proxy names the toolchain in `BURDIN_TOOLCHAIN_DIR`
/// (`spec/toolchain.md` TC10).
pub fn toolchain_std_dir() -> Option<PathBuf> {
    toolchain_std_dir_in(std::env::var_os("BURDIN_TOOLCHAIN_DIR").map(PathBuf::from).as_deref())
}

fn toolchain_std_dir_in(toolchain: Option<&Path>) -> Option<PathBuf> {
    toolchain.map(|dir| dir.join("lib").join("std").join("src")).filter(|dir| dir.is_dir())
}

/// The file on disk that shows `<std>/name.bd`: the toolchain's installed source when it has exactly this text,
/// otherwise a file under [`std_dir`], written when it is missing or out of date.
pub fn std_file(path: &str, text: &str) -> Option<PathBuf> {
    let name = path.strip_prefix("<std>/")?;
    if let Some(installed) = toolchain_std_dir().map(|dir| dir.join(name))
        && std::fs::read_to_string(&installed).ok().as_deref() == Some(text)
    {
        return Some(installed);
    }
    let dir = std_dir();
    let file = dir.join(name);
    if std::fs::read_to_string(&file).ok().as_deref() != Some(text) {
        std::fs::create_dir_all(&dir).ok()?;
        let tmp = dir.join(format!(".{name}.{}", std::process::id()));
        std::fs::write(&tmp, text).ok()?;
        std::fs::rename(&tmp, &file).ok()?;
    }
    Some(file)
}

/// The `<std>/name.bd` path a file returned by [`std_file`] stands for.
pub fn std_path_of(path: &Path) -> Option<String> {
    std_path_in(path, &[Some(std_dir()), toolchain_std_dir()])
}

fn std_path_in(path: &Path, dirs: &[Option<PathBuf>]) -> Option<String> {
    let parent = path.parent()?;
    let parent = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
    let known = dirs.iter().flatten().any(|dir| dir.canonicalize().unwrap_or_else(|_| dir.clone()) == parent);
    if !known {
        return None;
    }
    Some(format!("<std>/{}", path.file_name()?.to_str()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_discovery_skips_a_virtual_root() {
        // WS18: a folder is one package per member, and a virtual root is not a package.
        let folder = std::env::temp_dir().join(format!("burdin-lsp-discovery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        for (dir, manifest) in [("app", "[package]\nname = \"app\"\n"), ("lib", "[package]\nname = \"lib\"\n")] {
            std::fs::create_dir_all(folder.join(dir).join("src")).unwrap();
            std::fs::write(folder.join(dir).join("burdin.toml"), manifest).unwrap();
        }
        std::fs::write(folder.join("burdin.toml"), "[workspace]\nmembers = [\"app\", \"lib\"]\n").unwrap();
        let found = discover(&folder);
        let root = folder.canonicalize().unwrap();
        assert_eq!(found, vec![root.join("app"), root.join("lib")]);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn a_toolchain_with_std_src_supplies_the_std_files() {
        let toolchain = std::env::temp_dir().join(format!("burdin-lsp-toolchain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&toolchain);
        assert_eq!(toolchain_std_dir_in(Some(&toolchain)), None);
        assert_eq!(toolchain_std_dir_in(None), None);
        let src = toolchain.join("lib").join("std").join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("std.bd"), "// std\n").unwrap();
        assert_eq!(toolchain_std_dir_in(Some(&toolchain)), Some(src.clone()));
        let dirs = [Some(std_dir()), Some(src.clone())];
        assert_eq!(std_path_in(&src.join("std.bd"), &dirs).as_deref(), Some("<std>/std.bd"));
        assert_eq!(std_path_in(&toolchain.join("std.bd"), &dirs), None);
        std::fs::remove_dir_all(&toolchain).unwrap();
    }
}
