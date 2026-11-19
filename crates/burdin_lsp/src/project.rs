//! The packages a session analyzes (`spec/lsp.md`, LS4): one query database per package root, the analysis of each,
//! and the syntax index of each file. A file is *owned* by the package whose `burdin.toml` is the nearest one above
//! it (or by itself, for a file outside every package); its diagnostics come from its owner. A dependency's files are
//! also part of every package that depends on it, so an edit reaches every database that reads the file.

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
}

impl Package {
    /// Loads a package directory or a single file. `texts` are the open documents, by path, which replace the text
    /// on disk.
    pub fn load(root: &Path, texts: &HashMap<String, String>) -> Result<Package, String> {
        let workspace = package::load_for_tests(root)?;
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

    /// The analysis with a program to answer from: the latest when it has one, else the last good one, with the
    /// texts it was computed on (`None` for the latest, whose texts are current).
    pub fn program_analysis(&self) -> Option<(&Analysis, Option<&Texts>)> {
        match &self.analysis {
            Some(a) if a.program.is_some() => Some((a, None)),
            _ => self.last_good.as_ref().map(|(a, texts)| (&**a, Some(texts))),
        }
    }

    /// The analysis of the current texts with a program, if the latest one is current.
    pub fn current(&self) -> Option<&Analysis> {
        self.analysis.as_ref().filter(|a| a.program.is_some() && !self.dirty)
    }

    pub fn text(&self, path: &str) -> Option<&str> {
        self.db.file_text(path)
    }

    /// The syntax index of a file at its current text.
    pub fn index(&mut self, path: &str) -> Option<Rc<FileIndex>> {
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
        }
        out
    }
}

/// The path of a file as the databases spell it: canonical when it exists, without Windows' verbatim prefix.
pub fn path_key(path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()));
    package::source_path_text(&path)
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
    if dir.join("burdin.toml").is_file() {
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

/// The file on disk that shows `<std>/name.bd`, written when it is missing or out of date.
pub fn std_file(path: &str, text: &str) -> Option<PathBuf> {
    let name = path.strip_prefix("<std>/")?;
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

/// The `<std>/name.bd` path a file written by [`std_file`] stands for.
pub fn std_path_of(path: &Path) -> Option<String> {
    let dir = std_dir();
    let dir = dir.canonicalize().unwrap_or(dir);
    let parent = path.parent()?;
    let parent = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
    if parent != dir {
        return None;
    }
    Some(format!("<std>/{}", path.file_name()?.to_str()?))
}
