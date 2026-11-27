//! The database: the inputs (file texts and the package layout), the memoized queries over them, and the revision
//! counters that show what each analysis did (`spec/query.md`).
//!
//! The queries, from the inputs up:
//!
//! | query | key | value |
//! | --- | --- | --- |
//! | `parse(file)` | the file's text | the syntax tree and its syntax diagnostics (a body edit reparses the body) |
//! | `type_check(item)` | the item's text, the declarations, the registry chain | the typed function and its diagnostics |
//! | `check_memory(function)` | the typed function, the signatures it reads | the move and borrow diagnostics |
//! | `diagnostics()` | every file | all diagnostics, in the order the batch compiler gives them |
//!
//! `type_check` lives in `burdin_sema` (`CheckCache`) because it shares the checker's private state; this crate
//! owns everything around it.

use crate::follow_on::{self, declared_names};
use crate::package::{self, Workspace};
use crate::reparse;
use crate::stats::{Counters, RevisionStats};
use burdin_diag::{Diagnostic, Diagnostics, FileId, Severity, SourceMap};
use burdin_sema::{CheckCache, Edit};
use burdin_syntax::ast;
use burdin_tir::Program;
use std::collections::{BTreeSet, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// What a run of the compiler front end is for.
#[derive(Clone, Debug)]
pub struct Options {
    /// What the entry point of this build is: the program's `main`, its `test` items (`burdin test`), or its
    /// `bench` items (`burdin bench`). A test or bench build does not require `main`.
    pub mode: burdin_sema::EntryMode,
    /// Compile for release: no error trace frames. The generated code's settings come from the profile
    /// (`spec/profiles.md`); this is the one front-end decision that is still spelled `release`.
    pub release: bool,
    /// Integer overflow panics in constant evaluation (`spec/profiles.md`).
    pub overflow_checks: bool,
    /// `debug_assert*` runs in constant evaluation (`spec/profiles.md`).
    pub debug_assertions: bool,
    /// The step budget of each compile-time evaluation; `None` is the default (`spec/const-eval.md`, CE8).
    pub const_steps: Option<u64>,
    /// Keep checked bodies between analyses. A language server or an editor wants this; a one-shot batch build
    /// (`burdin build`) turns it off, because it will never ask a second question.
    pub no_cache: bool,
    /// The most threads the front end may use (`-j`); 0 means `BURDIN_JOBS`, then the machine's parallelism.
    pub jobs: usize,
}

impl Default for Options {
    /// A development build: trace frames, overflow checks, and debug assertions.
    fn default() -> Options {
        Options {
            mode: burdin_sema::EntryMode::default(),
            release: false,
            overflow_checks: true,
            debug_assertions: true,
            const_steps: None,
            no_cache: false,
            jobs: 0,
        }
    }
}

/// The analysis was cancelled before it finished (see [`CancelHandle`]). The database is intact; analyze again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the analysis was cancelled")
    }
}

/// A handle another thread uses to cancel the analysis in progress, typically because the inputs changed. The
/// database polls it between files, between function bodies, and between compiler stages; the analysis then
/// returns [`Cancelled`] and the flag is cleared, so the next analysis starts normally.
#[derive(Clone, Debug, Default)]
pub struct CancelHandle(Arc<AtomicBool>);

impl CancelHandle {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Withdraws a cancellation that arrived after the analysis it was meant for had finished, so it does not
    /// cancel the next one. A language server calls it when the request the handle was given for is answered.
    pub fn reset(&self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

/// How much the database may remember. After every analysis the least recently used results beyond the budget are
/// forgotten; the next analysis checks them again if it needs them.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    /// The most checked bodies to keep. A body is a typed function and its diagnostics.
    pub bodies: usize,
}

impl Default for Budget {
    fn default() -> Budget {
        // The standard library alone is a few thousand bodies once its generic instances are counted.
        Budget { bodies: 40_000 }
    }
}

/// A 128-bit fingerprint of a text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TextFp(u64, u64);

pub(crate) fn text_fp(text: &str) -> TextFp {
    let mut a = DefaultHasher::new();
    text.hash(&mut a);
    let mut b = DefaultHasher::new();
    b.write_u64(0x5bd1_e995);
    text.hash(&mut b);
    TextFp(a.finish(), b.finish())
}

/// The syntax of one file at one text.
pub struct Parsed {
    pub(crate) fp: TextFp,
    pub text: Rc<str>,
    pub ast: ast::File,
    pub diags: Vec<Diagnostic>,
}

struct FileSlot {
    /// The path diagnostics show, and the key of [`Db::set_file`].
    path: String,
    text: Rc<str>,
    fp: TextFp,
    /// The text the memoized check results were last moved to.
    analyzed: Rc<str>,
    parsed: Option<Rc<Parsed>>,
}

struct LayoutModule {
    path: Vec<String>,
    file: FileId,
}

struct LayoutPackage {
    name: String,
    dir: Option<PathBuf>,
    deps: Vec<(String, usize)>,
    root_module: Option<usize>,
    /// The official packages the package does not declare, for the help on an unresolved `use` (PKG18).
    official: Vec<String>,
    edition: burdin_sema::Edition,
    modules: Vec<LayoutModule>,
}

struct Layout {
    packages: Vec<LayoutPackage>,
    entry_package: usize,
    entry_module: usize,
}

/// The query database: file texts in, diagnostics and typed programs out, with everything unchanged between two
/// analyses reused.
pub struct Db {
    options: Options,
    files: Vec<FileSlot>,
    by_path: HashMap<String, usize>,
    /// The source map the syntax trees' spans point into. Ids are file slots.
    sources: SourceMap,
    layout: Layout,
    pub(crate) check_cache: CheckCache,
    pub(crate) memory: crate::memory::MemoryCache,
    cancel: CancelHandle,
    budget: Budget,
    counters: Counters,
    last: RevisionStats,
    revision: u64,
}

/// The result of one analysis.
pub struct Analysis {
    /// Every diagnostic of parsing, checking, and compile-time evaluation, in the order the batch compiler reports
    /// them.
    pub diagnostics: Diagnostics,
    /// The checked program. `None` when a file has syntax errors: its tree is partial, so the program is not handed
    /// out, but the other modules are still checked and their diagnostics are reported (`spec/check.md`, CK4).
    pub program: Option<Program>,
    /// The program checked against the partial trees of the files with syntax errors, when there are any (and
    /// `None` otherwise). Every item and statement the parser kept is typed in it; a statement it could not read is
    /// missing. It answers the position queries of [`Analysis`] (`type_at`, `resolve_at`, ...) beside a syntax
    /// error, and is never handed out as a program (`spec/query.md`, QL10).
    pub(crate) partial: Option<Program>,
    /// The revision this analysis is of.
    pub revision: u64,
    /// The path of every file of the program, in module order (the ids the diagnostics' spans carry).
    pub(crate) files: Vec<String>,
}

impl Db {
    /// Opens a package directory or a single `.bd` file from disk, with the embedded standard library.
    pub fn open(target: &Path, options: Options) -> Result<Db, String> {
        let workspace = package::load_selected(target, options.mode, &package::FeatureRequest::default())?;
        Db::from_workspace(&workspace, &HashMap::new(), options)
    }

    /// A program held in memory: `files` are `(path inside src, text)` pairs of one package, the entry point
    /// being `main.bd` (or the only file). The embedded standard library is attached.
    pub fn from_files(files: &[(&str, &str)], options: Options) -> Result<Db, String> {
        let workspace = package::in_memory(files.iter().copied())?;
        let texts: HashMap<PathBuf, String> =
            files.iter().map(|(path, text)| (PathBuf::from(path), (*text).to_string())).collect();
        Db::from_workspace(&workspace, &texts, options)
    }

    /// A loaded workspace. `overrides` replace the text of a file on disk (unsaved editor buffers, or the text a
    /// fixer wants checked before it writes it).
    pub fn from_workspace(
        workspace: &Workspace,
        overrides: &HashMap<PathBuf, String>,
        options: Options,
    ) -> Result<Db, String> {
        let check_cache = if options.no_cache { CheckCache::disabled() } else { CheckCache::new() };
        let mut db = Db {
            options,
            files: Vec::new(),
            by_path: HashMap::new(),
            sources: SourceMap::default(),
            layout: Layout { packages: Vec::new(), entry_package: workspace.entry_package, entry_module: 0 },
            check_cache,
            memory: crate::memory::MemoryCache::default(),
            cancel: CancelHandle::default(),
            budget: Budget::default(),
            counters: Counters::default(),
            last: RevisionStats::default(),
            revision: 0,
        };
        let official = package::official_names();
        for package in &workspace.packages {
            let mut modules = Vec::new();
            for module in &package.modules {
                let text = match (overrides.get(&module.file), &module.text) {
                    (Some(text), _) => text.clone(),
                    (None, Some(text)) => text.to_string(),
                    (None, None) => std::fs::read_to_string(&module.file)
                        .map_err(|e| format!("cannot read {}: {e}", module.file.display()))?,
                };
                let file = db.add_file(package::source_path_text(&module.file), text);
                modules.push(LayoutModule { path: module.path.clone(), file });
            }
            db.layout.packages.push(LayoutPackage {
                name: package.name.clone(),
                dir: package.dir.clone(),
                deps: package.deps.clone(),
                root_module: package.root_module,
                official: official
                    .iter()
                    .filter(|name| !package.deps.iter().any(|(dep, _)| dep == *name))
                    .cloned()
                    .collect(),
                edition: package.edition,
                modules,
            });
        }
        db.layout.entry_module = workspace.entry_module;
        Ok(db)
    }

    fn add_file(&mut self, path: String, text: String) -> FileId {
        let id = self.sources.add(path.clone(), text.clone());
        debug_assert_eq!(id.0 as usize, self.files.len());
        let text: Rc<str> = Rc::from(text);
        let fp = text_fp(&text);
        self.by_path.insert(path.clone(), self.files.len());
        self.files.push(FileSlot { path, analyzed: text.clone(), text, fp, parsed: None });
        id
    }

    /// Sets the text of a file of the program. Returns whether it changed. The path is the one diagnostics show.
    /// Panics when the program has no such file: the set of files is the package layout, which does not change.
    pub fn set_file(&mut self, path: &str, text: &str) -> bool {
        let Some(&slot) = self.by_path.get(path) else { panic!("`{path}` is not a file of this program") };
        let file = &mut self.files[slot];
        if &*file.text == text {
            return false;
        }
        file.text = Rc::from(text);
        file.fp = text_fp(text);
        self.sources.set_text(FileId(slot as u32), text.to_string());
        true
    }

    /// The current text of a file of the program.
    pub fn file_text(&self, path: &str) -> Option<&str> {
        self.by_path.get(path).map(|&slot| &*self.files[slot].text)
    }

    /// The path of every file of the program, in module order.
    pub fn file_paths(&self) -> Vec<&str> {
        self.files.iter().map(|f| f.path.as_str()).collect()
    }

    /// The id of a file of the program.
    pub fn file_id(&self, path: &str) -> Option<FileId> {
        self.by_path.get(path).map(|&slot| FileId(slot as u32))
    }

    /// The source map of the current texts: what diagnostics render against.
    pub fn sources(&self) -> &SourceMap {
        &self.sources
    }

    pub fn options(&self) -> &Options {
        &self.options
    }

    /// Every file of the program with its package and module path (`navigate::ModuleInfo`).
    pub fn modules(&self) -> Vec<crate::navigate::ModuleInfo> {
        let mut out = Vec::new();
        for (index, package) in self.layout.packages.iter().enumerate() {
            for module in &package.modules {
                out.push(crate::navigate::ModuleInfo {
                    path: self.files[module.file.0 as usize].path.clone(),
                    package: index,
                    module: module.path.clone(),
                });
            }
        }
        out
    }

    /// The packages of the program, in the order [`crate::navigate::ModuleInfo::package`] indexes them.
    pub fn packages(&self) -> Vec<crate::navigate::PackageInfo> {
        self.layout
            .packages
            .iter()
            .enumerate()
            .map(|(index, package)| crate::navigate::PackageInfo {
                name: package.name.clone(),
                dir: package.dir.clone(),
                deps: package.deps.clone(),
                root_file: package
                    .root_module
                    .and_then(|m| package.modules.get(m))
                    .map(|m| self.files[m.file.0 as usize].path.clone()),
                is_entry: index == self.layout.entry_package,
                embedded: package
                    .modules
                    .first()
                    .is_some_and(|m| self.files[m.file.0 as usize].path.starts_with("<std>")),
            })
            .collect()
    }

    /// A handle for cancelling the analysis in progress from another thread.
    pub fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
    }

    pub fn set_budget(&mut self, budget: Budget) {
        self.budget = budget;
    }

    /// What the last analysis did, query by query.
    pub fn stats(&self) -> &RevisionStats {
        &self.last
    }

    /// What every analysis so far did, summed.
    pub fn counters(&self) -> &Counters {
        &self.counters
    }

    /// How many checked bodies the database remembers.
    pub fn remembered_bodies(&self) -> usize {
        self.check_cache.len()
    }

    /// Forgets every memoized result. The inputs stay.
    pub fn forget(&mut self) {
        self.check_cache.clear();
        self.memory.clear();
        for file in &mut self.files {
            file.parsed = None;
        }
    }

    /// Takes the source map and the parsed files out of the database, consuming it. A batch tool that has its
    /// answer (the command line driver) keeps the trees for its later stages; the memo is dropped with the rest.
    pub fn into_parts(mut self) -> (SourceMap, Vec<(FileId, ast::File)>) {
        let trees = self
            .files
            .iter_mut()
            .enumerate()
            .map(|(slot, file)| {
                let parsed = file.parsed.take().expect("every file is parsed before into_parts");
                // The database is the only owner at this point, so the tree moves rather than copying.
                let ast = match Rc::try_unwrap(parsed) {
                    Ok(parsed) => parsed.ast,
                    Err(parsed) => parsed.ast.clone(),
                };
                (FileId(slot as u32), ast)
            })
            .collect();
        (self.sources, trees)
    }

    fn poll(&self) -> Result<(), Cancelled> {
        if self.cancel.is_cancelled() { Err(Cancelled) } else { Ok(()) }
    }

    /// The syntax of `slot`, from the memo when its text is the one that was parsed. A change inside one function
    /// body reparses only that body (`reparse`); any other change reparses the file.
    fn parse(&mut self, slot: usize) -> Rc<Parsed> {
        let file = &self.files[slot];
        if let Some(parsed) = &file.parsed
            && parsed.fp == file.fp
        {
            self.last.files_reused += 1;
            return parsed.clone();
        }
        let id = FileId(slot as u32);
        let text = file.text.clone();
        let path = file.path.clone();
        let fp = file.fp;
        let old = file.parsed.clone();
        if let Some(old) = old
            && let Some(ast) = reparse::reparse_body(id, &old, &text)
        {
            self.counters.body_reparses += 1;
            self.last.bodies_reparsed.push(path);
            let parsed = Rc::new(Parsed { fp, text, ast, diags: Vec::new() });
            self.files[slot].parsed = Some(parsed.clone());
            return parsed;
        }
        let mut diags = Diagnostics::default();
        let ast = burdin_syntax::parse(id, &text, &mut diags);
        self.counters.parses += 1;
        self.last.files_parsed.push(path);
        let parsed = Rc::new(Parsed { fp, text, ast, diags: diags.into_vec() });
        self.files[slot].parsed = Some(parsed.clone());
        parsed
    }

    /// Parses every file whose memo is out of date, in parallel when there is more than one and more than one
    /// worker (`Options::jobs`, then `BURDIN_JOBS`, then the machine's parallelism; 1 keeps it sequential). The
    /// workers take the next unparsed file from a shared counter, so one long file does not hold up a fixed share
    /// of the others. The diagnostics come back in file order, so a parallel run is identical to a sequential one.
    fn parse_all(&mut self) -> Result<Vec<Rc<Parsed>>, Cancelled> {
        // Files never parsed (a cold start) are independent and parsed together; a file that changed since it was
        // parsed is one editor edit, handled one at a time by `parse`, which tries the body-only reparse first.
        let stale: Vec<usize> = (0..self.files.len()).filter(|&slot| self.files[slot].parsed.is_none()).collect();
        let jobs = job_count(stale.len(), self.options.jobs);
        if jobs > 1 {
            let texts: Vec<(FileId, String)> =
                stale.iter().map(|&slot| (FileId(slot as u32), self.files[slot].text.to_string())).collect();
            let next = std::sync::atomic::AtomicUsize::new(0);
            let slots: Vec<std::sync::Mutex<Option<(ast::File, Diagnostics)>>> =
                texts.iter().map(|_| std::sync::Mutex::new(None)).collect();
            std::thread::scope(|scope| {
                for _ in 0..jobs {
                    scope.spawn(|| {
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some((id, text)) = texts.get(index) else { break };
                            let mut diags = Diagnostics::default();
                            let ast = burdin_syntax::parse(*id, text, &mut diags);
                            *slots[index].lock().expect("a parser thread panicked") = Some((ast, diags));
                        }
                    });
                }
            });
            let results = slots
                .into_iter()
                .map(|slot| slot.into_inner().expect("a parser thread panicked").expect("every file is parsed"));
            for (&slot, (ast, diags)) in stale.iter().zip(results) {
                let file = &self.files[slot];
                let parsed = Rc::new(Parsed { fp: file.fp, text: file.text.clone(), ast, diags: diags.into_vec() });
                self.counters.parses += 1;
                self.last.files_parsed.push(file.path.clone());
                self.files[slot].parsed = Some(parsed);
            }
        }
        let mut out = Vec::with_capacity(self.files.len());
        for slot in 0..self.files.len() {
            self.poll()?;
            out.push(self.parse(slot));
        }
        Ok(out)
    }

    /// The syntax tree of a file, parsed if its text changed since it was last parsed.
    pub fn syntax(&mut self, path: &str) -> Option<Rc<Parsed>> {
        let slot = *self.by_path.get(path)?;
        Some(self.parse(slot))
    }

    /// The edits that turn the texts the check results were last moved to into the current texts, one per file.
    fn pending_edits(&mut self) -> Vec<Edit> {
        let mut edits = Vec::new();
        for (slot, file) in self.files.iter_mut().enumerate() {
            if Rc::ptr_eq(&file.analyzed, &file.text) || *file.analyzed == *file.text {
                continue;
            }
            edits.push(diff_edit(FileId(slot as u32), &file.analyzed, &file.text));
            file.analyzed = file.text.clone();
        }
        edits
    }

    /// Analyzes the program as it is now: parses what changed, checks what the changes can affect, and returns every
    /// diagnostic plus the typed program. Everything an earlier analysis computed that the changes cannot affect is
    /// reused; the answer is always what a from-scratch run gives.
    pub fn analyze(&mut self) -> Result<Analysis, Cancelled> {
        self.revision += 1;
        self.counters.analyses += 1;
        self.last = RevisionStats::default();
        let result = self.analyze_inner();
        if result.is_err() {
            // The canceller has made its point; the next analysis is a new request.
            self.cancel.0.store(false, Ordering::Relaxed);
        }
        result
    }

    fn analyze_inner(&mut self) -> Result<Analysis, Cancelled> {
        self.poll()?;
        // Move what the cache remembers to the new texts first, so it is consistent whatever happens next.
        let edits = self.pending_edits();
        self.check_cache.apply_edits(&edits, &self.sources);
        self.memory.apply_edits(&edits, &self.sources);

        let parsed = self.parse_all()?;
        let mut diags = Diagnostics::default();
        for p in &parsed {
            let mut file_diags = Diagnostics::default();
            for d in &p.diags {
                file_diags.push(d.clone());
            }
            diags.absorb(file_diags);
        }
        // A file with syntax errors is not type checked, but the other modules are, against the declarations its
        // tree kept (`spec/check.md`, CK4). Everything checked from here on comes after the syntax errors, and the
        // follow-on errors among it are dropped once the checks have run.
        let syntax_errors = diags.has_errors();
        let checked_from = diags.len();
        let broken: Vec<FileId> = parsed
            .iter()
            .enumerate()
            .filter(|(_, p)| p.diags.iter().any(|d| d.severity == Severity::Error))
            .map(|(slot, _)| FileId(slot as u32))
            .collect();

        // The checks of a file with syntax errors are not reported, and neither are the follow-on errors of the
        // names such a file declares (`follow_on`, CK4). The syntax errors themselves are kept.
        let names: BTreeSet<String> = broken
            .iter()
            .flat_map(|&file| {
                let tree = &parsed[file.0 as usize];
                declared_names(&tree.text, file, &tree.ast.unparsed)
            })
            .collect();

        // A program with syntax errors is checked without the cache. Its answers belong to a partial tree, so they
        // must not replace the answers of the last checked program, which the next edit can still reuse.
        let cache_wanted = !self.options.no_cache;
        self.check_cache.set_enabled(cache_wanted && !syntax_errors);

        let packages = sema_packages(&self.layout, &parsed);
        self.check_cache.set_cancel(Some(self.cancel.0.clone()));
        // A debug build records error trace frames; `--release` records none (`spec/errors.md`, ERR7).
        let checked = burdin_sema::check_cached(
            &packages,
            self.layout.entry_package,
            self.layout.entry_module,
            self.options.mode,
            !self.options.release,
            &self.sources,
            &mut diags,
            &mut self.check_cache,
        );
        self.check_cache.set_enabled(cache_wanted);
        let Some(mut program) = checked else { return Err(Cancelled) };
        let stats = self.check_cache.stats();
        self.counters.bodies_checked += stats.checked.len();
        self.counters.bodies_reused += stats.reused;
        self.last.bodies_checked = stats.distinct_checked();
        self.last.body_checks = stats.checked.len();
        self.last.bodies_reused = stats.reused;
        self.last.passes = stats.passes;
        self.poll()?;

        // The memory checks run even when type checking failed, to report every error in the program.
        let mut memory = Diagnostics::default();
        self.memory.check(&mut program, &mut memory, &mut self.last);
        self.counters.memory_checked += self.last.memory_checked.len();
        self.counters.memory_reused += self.last.memory_reused;
        diags.absorb(memory);
        self.poll()?;
        let mut diags = follow_on::retain_checked(diags, checked_from, &broken, &names, &self.sources);

        // Compile-time evaluation runs only on a program every check accepted (`spec/const-eval.md`, CE1). With
        // syntax errors there is no such program: it is never evaluated, and it is not returned (its tree is
        // partial).
        if !diags.has_errors() {
            let options = self.const_options();
            burdin_interp::evaluate(&mut program, options, &mut diags);
        }
        drop(packages);
        self.check_cache.evict(self.budget.bodies);
        let (program, partial) = if syntax_errors { (None, Some(program)) } else { (Some(program), None) };
        Ok(Analysis { diagnostics: diags, program, partial, revision: self.revision, files: self.paths() })
    }

    fn const_options(&self) -> burdin_interp::Options {
        burdin_interp::Options {
            overflow_checks: self.options.overflow_checks,
            debug_assertions: self.options.debug_assertions,
            steps: self.options.const_steps.unwrap_or(burdin_interp::DEFAULT_STEPS),
        }
    }

    /// The path of every file, in module order.
    fn paths(&self) -> Vec<String> {
        self.files.iter().map(|f| f.path.clone()).collect()
    }
}

/// The packages the checker reads: the layout over the parsed trees.
fn sema_packages<'a>(layout: &Layout, parsed: &'a [Rc<Parsed>]) -> Vec<burdin_sema::Package<'a>> {
    layout
        .packages
        .iter()
        .enumerate()
        .map(|(id, package)| burdin_sema::Package {
            name: package.name.clone(),
            is_root: id == layout.entry_package,
            edition: package.edition,
            deps: package.deps.clone(),
            root_module: package.root_module,
            official: package.official.clone(),
            modules: package
                .modules
                .iter()
                .map(|module| burdin_sema::Module {
                    file: module.file,
                    path: module.path.clone(),
                    ast: &parsed[module.file.0 as usize].ast,
                })
                .collect(),
        })
        .collect()
}

/// How many workers to parse `files` files with: at most one per file, never zero. `jobs` (from `-j`) wins, then
/// `BURDIN_JOBS` overrides the machine's parallelism, and 1 keeps the run sequential (useful for comparing output
/// and for measuring).
fn job_count(files: usize, jobs: usize) -> usize {
    let available = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let requested = if jobs > 0 { jobs } else { env_jobs().unwrap_or(0) };
    workers(files, available, requested)
}

/// `BURDIN_JOBS`, when it is a number.
pub fn env_jobs() -> Option<usize> {
    std::env::var("BURDIN_JOBS").ok().and_then(|v| v.trim().parse::<usize>().ok())
}

/// The worker count for a run, from the file count, the machine, and the request (0 is "whatever the machine
/// has").
fn workers(files: usize, available: usize, requested: usize) -> usize {
    if files <= 1 {
        return 1;
    }
    let workers = if requested == 0 { available } else { requested };
    workers.max(1).min(files)
}

/// The smallest single replacement that turns `old` into `new`: what is left after the common prefix and suffix.
pub(crate) fn diff_edit(file: FileId, old: &str, new: &str) -> Edit {
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let mut prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    // Do not split a multi-byte character: keep the prefix on a boundary of both texts.
    while prefix > 0 && !(old.is_char_boundary(prefix) && new.is_char_boundary(prefix)) {
        prefix -= 1;
    }
    let max_suffix = a.len().min(b.len()) - prefix;
    let mut suffix = a.iter().rev().zip(b.iter().rev()).take(max_suffix).take_while(|(x, y)| x == y).count();
    while suffix > 0 && !(old.is_char_boundary(a.len() - suffix) && new.is_char_boundary(b.len() - suffix)) {
        suffix -= 1;
    }
    let (lo, hi) = (prefix, a.len() - suffix);
    let new_hi = b.len() - suffix;
    let newlines = |text: &str| text.bytes().filter(|&c| c == b'\n').count();
    let end_line = new[..new_hi].bytes().filter(|&c| c == b'\n').count();
    Edit {
        file,
        lo: lo as u32,
        hi: hi as u32,
        new_len: (new_hi - prefix) as u32,
        lines_changed: newlines(&old[lo..hi]) != newlines(&new[prefix..new_hi]),
        end_line: end_line as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::workers;

    #[test]
    fn the_worker_count_respects_the_request_and_the_file_count() {
        assert_eq!(workers(0, 8, 0), 1);
        assert_eq!(workers(1, 8, 0), 1);
        assert_eq!(workers(4, 8, 0), 4);
        assert_eq!(workers(64, 8, 0), 8);
        assert_eq!(workers(64, 8, 1), 1, "BURDIN_JOBS=1 is sequential");
        assert_eq!(workers(64, 8, 3), 3);
        assert_eq!(workers(2, 8, 99), 2, "never more workers than files");
    }
}
