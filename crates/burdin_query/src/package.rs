//! Packages: `burdin.toml`, every `.bd` file under `src/` as a module, and path, git, and official dependencies.
//!
//! A single `.bd` file is a package with one module (its path is empty). A directory is a package described by
//! its `burdin.toml`; its entry point is `src/main.bd`, and `[dependencies]` are loaded recursively from their
//! `path`, from the checkout of the commit `burdin.lock` pins for a `git` dependency, or from the compiler's own
//! `packages/` folder for an official one (`name = true`; `spec/packages.md`). The loader never fetches: `burdin_pkg` resolves and checks out git dependencies before a build. Module paths are
//! relative to `src/` (`geometry/shapes.bd` is module `geometry.shapes`).

use crate::lockfile::{self, GitRef, Lockfile};
use crate::workspace;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// One source file of a package.
pub struct ModuleFile {
    /// The path inside `src/`, without the `.bd` suffix: `["geometry", "shapes"]`. Empty for a one-file target.
    pub path: Vec<String>,
    pub file: PathBuf,
    /// The source text of an embedded module (the standard library); `None` for a file on disk.
    pub text: Option<&'static str>,
}

/// One package of the compilation.
pub struct PackageFiles {
    /// The name in the package's own `[package]` section.
    pub name: String,
    /// The package directory, for a package on disk. `None` for a one-file target and for the embedded standard
    /// library, which have no manifest to load as a workspace member.
    pub dir: Option<PathBuf>,
    pub modules: Vec<ModuleFile>,
    /// `(name used by `use`, package index)`.
    pub deps: Vec<(String, usize)>,
    /// The dependency package names whose `build.bd` this package's manifest allows (`[build] allow`). Only the
    /// entry package's list is consulted; a package's own script is trusted (`spec/build-scripts.md`, BS2).
    pub build_allow: Vec<String>,
    /// The module other packages reach as `use <package>`.
    pub root_module: Option<usize>,
    /// The `[lint]` table of the manifest: `(lint name, level, line)` in file order (`spec/lint.md`, LN3). The
    /// linter checks the names and levels, so a manifest from a newer compiler still loads.
    pub lint: Vec<(String, String, usize)>,
    /// The `[rust]` and `[rust.crates]` tables: Rust crates this package binds through generated C-ABI shims
    /// (`spec/rust-interop.md`, RI7).
    pub rust: RustConfig,
    /// `[package] edition`: the edition the package's source is written against (`spec/fix.md`, FX10).
    pub edition: burdin_sema::Edition,
}

/// A package's Rust interop configuration. The manifest parser only reads it; the driver validates it and reports
/// E0264, so a manifest from a newer compiler still loads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RustConfig {
    /// `[rust] toolchain`: the exact Rust version the shims are built and verified with.
    pub toolchain: Option<String>,
    /// `[rust.crates]`: `(module name, path relative to the manifest)` in file order.
    pub crates: Vec<(String, String)>,
}

pub struct Workspace {
    pub packages: Vec<PackageFiles>,
    pub entry_package: usize,
    pub entry_module: usize,
    /// The name for the object file and the executable.
    pub stem: String,
}

/// Loads the target: a `.bd` file, or a directory containing `burdin.toml`. A package directory must have a
/// `src/main.bd` entry point, because `build` and `run` need one.
pub fn load(target: &Path) -> Result<Workspace, String> {
    load_impl(target, true)
}

/// A package of in-memory files: `paths` name the files as `geometry/shapes.bd` (module `geometry.shapes`). The entry
/// module is `main.bd`, or the first file when there is none. The embedded standard library is attached.
pub fn in_memory<'a>(paths: impl IntoIterator<Item = &'a str>) -> Workspace {
    let modules: Vec<ModuleFile> = paths
        .into_iter()
        .map(|path| ModuleFile {
            path: path.trim_end_matches(".bd").split('/').map(str::to_string).collect(),
            file: PathBuf::from(path),
            text: None,
        })
        .collect();
    let entry_module = modules.iter().position(|m| m.path == ["main"]).unwrap_or(0);
    let package = PackageFiles {
        name: "app".to_string(),
        dir: None,
        modules,
        deps: Vec::new(),
        build_allow: Vec::new(),
        root_module: Some(entry_module),
        lint: Vec::new(),
        rust: Default::default(),
        edition: Default::default(),
    };
    let mut packages = vec![package];
    attach_std(&mut packages);
    Workspace { packages, entry_package: 0, entry_module, stem: "app".to_string() }
}

/// The user's Burdin home, where tools keep state that outlives one run (`spec/scripts.md` SC6 and
/// `spec/repl.md` RP11): `BURDIN_HOME` when it is set, otherwise the platform's directory for such data.
pub fn home_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("BURDIN_HOME") {
        return Some(PathBuf::from(home));
    }
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join("burdin"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support/burdin"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
            .map(|base| base.join("burdin"))
    }
}

/// A source path as diagnostics and recorded allocation sites spell it. On Windows, `fs::canonicalize` returns a
/// verbatim path (`\?\C:\...`); the prefix is an implementation detail, so it is removed (`\?\UNC\server\share`
/// becomes `\server\share`). Every other platform keeps the path as it is.
pub fn source_path_text(path: &Path) -> String {
    let text = path.display().to_string();
    if cfg!(windows) { strip_verbatim_prefix(&text) } else { text }
}

/// Removes the extended-length prefix Windows uses for canonical paths.
pub fn strip_verbatim_prefix(text: &str) -> String {
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) => rest.to_string(),
        None => text.to_string(),
    }
}

/// Loads the target for `burdin test`, which does not need a `main` function: a library package's tests live in
/// its own modules. The entry module falls back to `src/<package>.bd` or `src/lib.bd`, then the first module.
pub fn load_for_tests(target: &Path) -> Result<Workspace, String> {
    load_impl(target, false)
}

/// Loads the packages `dirs` as the build's roots, for a command that selects members of a workspace
/// (`spec/workspaces.md`, WS6-WS11). The first is the entry package. A dependency that two roots reach is one package,
/// and the lockfile is the workspace's (WS14). `dirs` are canonical package directories.
pub fn load_members(dirs: &[PathBuf]) -> Result<Workspace, String> {
    let Some(first) = dirs.first() else {
        return Err("no package is selected".to_string());
    };
    let mut loader = Loader::new(read_lock(&workspace::lock_dir(first)?)?);
    let mut roots = Vec::with_capacity(dirs.len());
    for dir in dirs {
        roots.push(loader.load_package(dir)?);
    }
    let entry_package = roots[0];
    let entry = &loader.packages[entry_package];
    let entry_module = entry.modules.iter().position(|m| m.path == ["main"]).or(entry.root_module).unwrap_or(0);
    let stem = entry.name.clone();
    let mut packages = loader.packages;
    attach_std(&mut packages);
    Ok(Workspace { packages, entry_package, entry_module, stem })
}

/// The lockfile in `dir`, when there is one: a directory without a manifest has none.
fn read_lock(dir: &Path) -> Result<Option<Lockfile>, String> {
    if dir.join(workspace::MANIFEST).is_file() { lockfile::read(dir) } else { Ok(None) }
}

/// The manifest fragment of a script (`spec/scripts.md`, SC3): the dependency entries of the leading `//!` doc
/// block. Paths are kept as written, relative to the script's directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptManifest {
    pub deps: Vec<(String, String)>,
}

/// Reads a script's manifest fragment: the `[dependencies]` section of its leading `//!` doc block. `None` when
/// the block has no such section, so the file's doc comment is just documentation. A malformed fragment is an
/// error naming the script's line.
pub fn script_manifest(text: &str) -> Result<Option<ScriptManifest>, String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut at = 0usize;
    // A shebang line comes before the doc block.
    if lines.first().is_some_and(|line| line.starts_with("#!")) {
        at = 1;
    }
    // The leading doc block: blank lines, then the run of `//!` lines.
    while at < lines.len() && lines[at].trim().is_empty() {
        at += 1;
    }
    let mut block: Vec<(usize, &str)> = Vec::new();
    while at < lines.len() {
        let line = lines[at];
        let Some(rest) = line.strip_prefix("//!") else { break };
        block.push((at + 1, rest.strip_prefix(' ').unwrap_or(rest)));
        at += 1;
    }
    let Some(start) = block.iter().position(|(_, content)| content.trim() == "[dependencies]") else {
        return Ok(None);
    };
    let mut deps: Vec<(String, String)> = Vec::new();
    for (line_number, content) in &block[start + 1..] {
        let line = strip_comment(content).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            return Err(format!(
                "{line_number}: a script's manifest declares only `[dependencies]`;                  a script has no other manifest sections"
            ));
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("{line_number}: expected a dependency as `name = {{ path = \"..\" }}`"));
        };
        let (key, value) = (key.trim(), value.trim());
        if !is_ident(key) {
            return Err(format!("{line_number}: `{key}` is not a valid dependency name"));
        }
        if deps.iter().any(|(name, _)| name == key) {
            return Err(format!("{line_number}: dependency `{key}` is declared twice"));
        }
        let dep_path = match parse_dependency(value).map_err(|e| format!("{line_number}: {e}"))? {
            DepSource::Path(path) => path,
            DepSource::Git { .. } => {
                return Err(format!(
                    "{line_number}: a script's dependency must be a `path`; a script has no lockfile to pin a git \
                     dependency"
                ));
            }
            DepSource::Official => {
                return Err(format!(
                    "{line_number}: a script's dependency must be a `path`; a script has no lockfile to pin an \
                     official package"
                ));
            }
        };
        deps.push((key.to_string(), dep_path));
    }
    Ok(Some(ScriptManifest { deps }))
}

fn load_impl(target: &Path, require_entry: bool) -> Result<Workspace, String> {
    if target.is_dir() {
        let dir = target.canonicalize().map_err(|e| format!("cannot open {}: {e}", target.display()))?;
        // The lockfile of the workspace the package belongs to (or its own, standalone) says which commit each git
        // dependency builds with, anywhere in the graph (WS14).
        let lock = read_lock(&workspace::lock_dir(&dir)?)?;
        let mut loader = Loader::new(lock);
        let root = loader.load_package(&dir)?;
        let package = &loader.packages[root];
        let main = package.modules.iter().position(|m| m.path == ["main"]);
        let entry_module = match main {
            Some(module) => module,
            None if require_entry => {
                return Err(format!(
                    "package `{}` has no entry point: expected {}",
                    package.name,
                    source_path_text(&dir.join("src/main.bd"))
                ));
            }
            // A library package tested on its own starts from the module other packages import, or any module.
            None => package.root_module.unwrap_or(0),
        };
        let stem = package.name.clone();
        let mut packages = loader.packages;
        attach_std(&mut packages);
        Ok(Workspace { packages, entry_package: root, entry_module, stem })
    } else if target.is_file() {
        let stem = target.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "a".into());
        let path = source_path_text(target);
        let text = std::fs::read_to_string(target).map_err(|e| format!("cannot read {path}: {e}"))?;
        let manifest = script_manifest(&text).map_err(|e| format!("{path}:{e}"))?;
        let package = PackageFiles {
            name: stem.clone(),
            dir: None,
            modules: vec![ModuleFile { path: Vec::new(), file: target.to_path_buf(), text: None }],
            deps: Vec::new(),
            build_allow: Vec::new(),
            root_module: Some(0),
            lint: Vec::new(),
            rust: Default::default(),
            edition: Default::default(),
        };
        let mut packages = vec![package];
        if let Some(manifest) = manifest {
            let base = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            let mut loader = Loader::new(None);
            for (name, relative) in manifest.deps {
                let dep_dir =
                    base.join(&relative).canonicalize().map_err(|e| format!("{path}: dependency `{name}`: {e}"))?;
                let id = loader.load_package(&dep_dir).map_err(|e| format!("{path}: {e}"))?;
                packages[0].deps.push((name, id + 1));
            }
            if loader.packages.iter().any(|p| p.name == stem) {
                return Err(format!("{path}: a dependency is also named `{stem}`, the script's own package name"));
            }
            // The loader numbers its packages from zero; the script's package takes index zero.
            for package in &mut loader.packages {
                for (_, id) in &mut package.deps {
                    *id += 1;
                }
            }
            packages.extend(loader.packages);
        }
        attach_std(&mut packages);
        Ok(Workspace { packages, entry_package: 0, entry_module: 0, stem })
    } else {
        Err(format!("cannot open {}: no such file or directory", target.display()))
    }
}

/// The standard library, embedded in the driver so that every program has it without a manifest entry.
/// A project that declares its own `std` package keeps it instead.
fn std_package() -> PackageFiles {
    let sources: [(&str, &'static str); 41] = [
        ("std", include_str!("../../../std/src/std.bd")),
        ("alloc", include_str!("../../../std/src/alloc.bd")),
        ("io", include_str!("../../../std/src/io.bd")),
        ("fs", include_str!("../../../std/src/fs.bd")),
        ("env", include_str!("../../../std/src/env.bd")),
        ("process", include_str!("../../../std/src/process.bd")),
        ("mem", include_str!("../../../std/src/mem.bd")),
        ("iter", include_str!("../../../std/src/iter.bd")),
        ("fmt", include_str!("../../../std/src/fmt.bd")),
        ("ops", include_str!("../../../std/src/ops.bd")),
        ("target", include_str!("../../../std/src/target.bd")),
        ("cmp", include_str!("../../../std/src/cmp.bd")),
        ("hash", include_str!("../../../std/src/hash.bd")),
        ("option", include_str!("../../../std/src/option.bd")),
        ("set", include_str!("../../../std/src/set.bd")),
        ("deque", include_str!("../../../std/src/deque.bd")),
        ("heap", include_str!("../../../std/src/heap.bd")),
        ("unicode", include_str!("../../../std/src/unicode.bd")),
        ("text", include_str!("../../../std/src/text.bd")),
        ("sort", include_str!("../../../std/src/sort.bd")),
        ("mapops", include_str!("../../../std/src/mapops.bd")),
        ("encoding", include_str!("../../../std/src/encoding.bd")),
        ("float", include_str!("../../../std/src/float.bd")),
        ("bytes", include_str!("../../../std/src/bytes.bd")),
        ("lazy", include_str!("../../../std/src/lazy.bd")),
        ("path", include_str!("../../../std/src/path.bd")),
        ("rw", include_str!("../../../std/src/rw.bd")),
        ("int", include_str!("../../../std/src/int.bd")),
        ("list", include_str!("../../../std/src/list.bd")),
        ("task", include_str!("../../../std/src/task.bd")),
        ("channel", include_str!("../../../std/src/channel.bd")),
        ("marker", include_str!("../../../std/src/marker.bd")),
        ("atomic", include_str!("../../../std/src/atomic.bd")),
        ("sync", include_str!("../../../std/src/sync.bd")),
        ("thread", include_str!("../../../std/src/thread.bd")),
        ("reactor", include_str!("../../../std/src/reactor.bd")),
        ("net", include_str!("../../../std/src/net.bd")),
        ("time", include_str!("../../../std/src/time.bd")),
        ("random", include_str!("../../../std/src/random.bd")),
        ("slice", include_str!("../../../std/src/slice.bd")),
        ("serialize", include_str!("../../../std/src/serialize.bd")),
    ];
    let modules: Vec<ModuleFile> = sources
        .iter()
        .map(|(name, text)| ModuleFile {
            path: vec![name.to_string()],
            file: PathBuf::from(format!("<std>/{name}.bd")),
            text: Some(text),
        })
        .collect();
    let root_module = modules.iter().position(|m| m.path == ["std"]);
    PackageFiles {
        name: "std".to_string(),
        dir: None,
        modules,
        deps: Vec::new(),
        build_allow: Vec::new(),
        root_module,
        lint: Vec::new(),
        rust: Default::default(),
        edition: Default::default(),
    }
}

/// The names of the standard library's modules, which a `use` declaration may start with (`burdin fmt` groups them).
pub fn std_module_names() -> Vec<String> {
    std_package().modules.iter().filter_map(|m| m.path.first().cloned()).collect()
}

/// The directory of the nearest `burdin.toml` above `file`, if any.
pub fn manifest_dir(file: &Path) -> Option<PathBuf> {
    let absolute = std::path::absolute(file).ok()?;
    absolute.ancestors().skip(1).find(|dir| dir.join("burdin.toml").is_file()).map(Path::to_path_buf)
}

/// The names a package's manifest gives its dependencies; empty when the manifest does not read.
pub fn dependency_names(dir: &Path) -> Vec<String> {
    match parse_manifest(&dir.join("burdin.toml")) {
        Ok(manifest) => manifest.deps.into_iter().map(|(name, _)| name).collect(),
        Err(_) => Vec::new(),
    }
}

/// Adds the embedded standard library as a dependency of every package, unless a package already uses the
/// name `std` (then that package is the standard library).
fn attach_std(packages: &mut Vec<PackageFiles>) {
    if packages.iter().any(|p| p.name == "std") {
        return;
    }
    let std_id = packages.len();
    for p in packages.iter_mut() {
        // A package that already declares a `std` dependency keeps that one.
        if !p.deps.iter().any(|(name, _)| name == "std") {
            p.deps.push(("std".to_string(), std_id));
        }
    }
    packages.push(std_package());
}

/// The environment variable that names the directory of the official packages (`spec/packages.md`, PKG16).
pub const PACKAGES_ENV: &str = "BURDIN_PACKAGES";

/// The places the official packages may be, in the order they are tried (PKG16). With `BURDIN_PACKAGES` set that is
/// the only one.
pub fn official_roots() -> Vec<PathBuf> {
    if let Some(dir) = std::env::var_os(PACKAGES_ENV).filter(|dir| !dir.is_empty()) {
        return vec![PathBuf::from(dir)];
    }
    let mut roots = Vec::new();
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        roots.push(exe_dir.join("..").join("packages"));
        roots.push(exe_dir.join("packages"));
    }
    // A debug build finds the checkout it was built from. A release build embeds no build path, so it is
    // reproducible: it finds the packages beside its executable.
    #[cfg(debug_assertions)]
    roots.push(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packages")));
    roots
}

/// The first of [`official_roots`] that is a directory.
pub fn official_root() -> Option<PathBuf> {
    official_roots().into_iter().find(|root| root.is_dir())
}

/// The names of the official packages in `root`, sorted: its subdirectories that hold a `burdin.toml`.
fn official_names_in(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("burdin.toml").is_file())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| is_ident(name))
        .collect();
    names.sort();
    names
}

/// The names of the official packages this compiler has, sorted; empty when it has none.
pub fn official_names() -> Vec<String> {
    official_root().map(|root| official_names_in(&root)).unwrap_or_default()
}

/// The directory of the official package `name`, or the error that explains why there is none (`name = true` for a
/// name that is not official, or no packages directory at all).
pub fn official_dir(name: &str) -> Result<PathBuf, String> {
    let Some(root) = official_root() else {
        let looked: Vec<String> = official_roots().iter().map(|root| source_path_text(root)).collect();
        return Err(format!(
            "it is not an official package: this burdin found no packages directory (looked in {}; set \
             {PACKAGES_ENV} to the directory that holds the official packages); for any other package give \
             `{{ path = \"...\" }}` or `{{ git = \"...\" }}`",
            looked.join(", ")
        ));
    };
    official_dir_in(&root, name)
}

fn official_dir_in(root: &Path, name: &str) -> Result<PathBuf, String> {
    let names = official_names_in(root);
    if names.iter().any(|official| official == name) {
        return Ok(root.join(name));
    }
    let known = if names.is_empty() {
        "this burdin has no official packages".to_string()
    } else {
        format!("the official packages are: {}", names.join(", "))
    };
    Err(format!(
        "it is not an official package; {known}; for any other package give `{{ path = \"...\" }}` or \
         `{{ git = \"...\" }}`"
    ))
}

struct Loader {
    packages: Vec<PackageFiles>,
    /// Canonical directory of each loaded package, for a diamond dependency to reuse its package.
    loaded: HashMap<PathBuf, usize>,
    /// The directory each package name came from: a name must identify exactly one package, because it is part of
    /// every qualified item name.
    names: HashMap<String, PathBuf>,
    /// The packages being loaded, to report a dependency cycle.
    loading: Vec<PathBuf>,
    /// The entry package's `burdin.lock`, which pins every git dependency to a commit.
    lock: Option<Lockfile>,
}

impl Loader {
    fn new(lock: Option<Lockfile>) -> Loader {
        Loader { packages: Vec::new(), loaded: HashMap::new(), names: HashMap::new(), loading: Vec::new(), lock }
    }

    /// The directory a dependency's package is in: its path, or the checkout of the commit the lockfile pins.
    fn dependency_dir(
        &self,
        dir: &Path,
        manifest_path: &Path,
        name: &str,
        source: &DepSource,
    ) -> Result<PathBuf, String> {
        match source {
            DepSource::Path(relative) => dir
                .join(relative)
                .canonicalize()
                .map_err(|e| format!("`{}`: dependency `{name}`: {e}", manifest_path.display())),
            DepSource::Official => official_dir(name)
                .and_then(|dir| dir.canonicalize().map_err(|e| format!("cannot open {}: {e}", dir.display())))
                .map_err(|e| format!("`{}`: dependency `{name} = true`: {e}", manifest_path.display())),
            DepSource::Git { url, reference } => {
                let locked = self.lock.as_ref().and_then(|lock| lock.git_package(url, reference));
                let Some(git) = locked.and_then(|p| p.git.as_ref()) else {
                    return Err(format!(
                        "`{}`: git dependency `{name}` ({url}, {}) is not in burdin.lock; run `burdin build` to \
                         resolve it",
                        manifest_path.display(),
                        reference.describe()
                    ));
                };
                let checkout = lockfile::git_checkout_dir(url, &git.commit)?;
                if !checkout.is_dir() {
                    return Err(format!(
                        "`{}`: git dependency `{name}` ({url} at {}) is not fetched; run `burdin build` to fetch it",
                        manifest_path.display(),
                        git.commit
                    ));
                }
                checkout.canonicalize().map_err(|e| format!("`{}`: dependency `{name}`: {e}", manifest_path.display()))
            }
        }
    }

    fn load_package(&mut self, dir: &Path) -> Result<usize, String> {
        if self.loading.contains(&dir.to_path_buf()) {
            let mut chain: Vec<String> = self.loading.iter().map(|p| p.display().to_string()).collect();
            chain.push(dir.display().to_string());
            return Err(format!("dependency cycle: {}", chain.join(" -> ")));
        }
        if let Some(&id) = self.loaded.get(dir) {
            return Ok(id);
        }
        let manifest_path = dir.join("burdin.toml");
        let manifest = parse_manifest(&manifest_path)?;
        if let Some(previous) = self.names.get(&manifest.name) {
            return Err(format!(
                "package `{}` is provided by both {} and {}; package names must be unique",
                manifest.name,
                previous.display(),
                dir.display()
            ));
        }
        self.names.insert(manifest.name.clone(), dir.to_path_buf());
        let modules = discover_modules(dir)?;
        // Other packages reach this one as `use <package>`: prefer `src/<package>.bd`, then the conventional
        // `src/lib.bd`.
        let root_module = modules
            .iter()
            .position(|m| m.path.len() == 1 && m.path[0] == manifest.name)
            .or_else(|| modules.iter().position(|m| m.path.len() == 1 && m.path[0] == "lib"));
        let id = self.packages.len();
        self.packages.push(PackageFiles {
            name: manifest.name,
            dir: Some(dir.to_path_buf()),
            modules,
            deps: Vec::new(),
            build_allow: manifest.build_allow,
            root_module,
            lint: manifest.lint,
            rust: manifest.rust,
            edition: manifest.edition,
        });
        self.loaded.insert(dir.to_path_buf(), id);
        self.loading.push(dir.to_path_buf());
        let mut deps = Vec::new();
        let mut seen = Vec::new();
        for (name, source) in &manifest.deps {
            if seen.contains(name) {
                return Err(format!("`{}`: dependency `{name}` is declared twice", manifest_path.display()));
            }
            seen.push(name.clone());
            let dep_dir = self.dependency_dir(dir, &manifest_path, name, source)?;
            let dep_id = self.load_package(&dep_dir)?;
            deps.push((name.clone(), dep_id));
        }
        self.loading.pop();
        self.packages[id].deps = deps;
        Ok(id)
    }
}

/// Where a dependency comes from (`spec/packages.md`, PKG1-PKG2, PKG15).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DepSource {
    /// A directory, relative to the manifest that names it.
    Path(String),
    /// A git repository and the reference to build; `burdin.lock` pins it to a commit.
    Git { url: String, reference: GitRef },
    /// The official package of the dependency's own name, bundled with the compiler (`name = true`).
    Official,
}

#[derive(Debug)]
pub struct Manifest {
    pub name: String,
    /// `[package] version`, when the manifest gives one or inherits it (WS15).
    pub version: Option<String>,
    /// `[package] license`, when the manifest gives one or inherits it (WS15).
    pub license: Option<String>,
    /// `[package] authors`, when the manifest gives them or inherits them (WS15).
    pub authors: Vec<String>,
    /// `(dependency name, where it comes from)`, in file order.
    pub deps: Vec<(String, DepSource)>,
    /// The dependency names whose build scripts the manifest grants (`[build] allow`).
    pub build_allow: Vec<String>,
    /// The `[lint]` entries: `(name, level, line)`.
    pub lint: Vec<(String, String, usize)>,
    pub rust: RustConfig,
    /// `[package] edition`; the default edition when the manifest names none.
    pub edition: burdin_sema::Edition,
}

/// A `[package]` value that a manifest sets itself, or inherits from its workspace (`spec/workspaces.md`, WS15). Each
/// keeps its line, so an error can name it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Slot<T> {
    Set(T, usize),
    Inherit(usize),
}

/// The `[package]` values of one manifest that a workspace can supply (WS15).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Fields {
    pub(crate) version: Option<Slot<String>>,
    pub(crate) edition: Option<Slot<String>>,
    pub(crate) license: Option<Slot<String>>,
    pub(crate) authors: Option<Slot<Vec<String>>>,
}

/// A dependency's value in `[dependencies]`: a source, or `workspace = true` (WS16).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DepValue {
    Source(DepSource),
    Inherit,
}

/// The `[workspace]` section: its entries with their lines (WS3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceSection {
    pub(crate) members: Vec<(String, usize)>,
    pub(crate) exclude: Vec<(String, usize)>,
    pub(crate) default_members: Vec<(String, usize)>,
}

/// One `burdin.toml`, read line by line before any inheritance is resolved (`spec/workspaces.md`). Only the sections
/// and keys the compiler reads are kept; every other line is ignored.
#[derive(Debug, Default)]
pub(crate) struct Document {
    /// Whether the manifest has a `[package]` section.
    pub(crate) package_seen: bool,
    /// `name` in `[package]`, with its line.
    name: Option<(String, usize)>,
    pub(crate) fields: Fields,
    /// `[dependencies]` in file order: name, value, line.
    deps: Vec<(String, DepValue, usize)>,
    /// `Some` when the manifest has a `[workspace]` section.
    pub(crate) workspace: Option<WorkspaceSection>,
    /// `[workspace.package]`: the values a member may inherit.
    workspace_fields: Fields,
    /// `[workspace.dependencies]` in file order: name, source, line.
    workspace_deps: Vec<(String, DepSource, usize)>,
    build_allow: Vec<String>,
    lint: Vec<(String, String, usize)>,
    rust: RustConfig,
}

/// Whether a `[package]` value is inherited from the workspace.
fn is_inherited<T>(slot: &Option<Slot<T>>) -> bool {
    matches!(slot, Some(Slot::Inherit(_)))
}

impl Document {
    /// Whether any value or dependency of the package is inherited from its workspace.
    fn inherits(&self) -> bool {
        is_inherited(&self.fields.version)
            || is_inherited(&self.fields.edition)
            || is_inherited(&self.fields.license)
            || is_inherited(&self.fields.authors)
            || self.deps.iter().any(|(_, value, _)| *value == DepValue::Inherit)
    }
}

/// The `burdin.toml` of a package, with the sections the compiler reads (`spec/packages.md`, `spec/workspaces.md`).
/// Unknown sections and keys are ignored, so a manifest from a newer compiler still loads. Reads the manifest at `path`.
pub fn parse_manifest(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_manifest_text(&text, path)
}

/// Parses a package's manifest. A value or dependency it inherits (`key.workspace = true`) is taken from the workspace
/// its directory belongs to (WS15-WS16); a workspace root without a `[package]` is not a package, and says so.
pub fn parse_manifest_text(text: &str, path: &Path) -> Result<Manifest, String> {
    let doc = parse_document(text, path)?;
    if !doc.package_seen && doc.workspace.is_some() {
        return Err(format!(
            "{}: a workspace root without a `[package]` section is not a package; build, run, test, bench, doc, and \
             fmt select its members with `-p` or `--workspace`, and the other commands take a member's directory",
            path.display()
        ));
    }
    let name = match &doc.name {
        Some((name, _)) if doc.package_seen => name.clone(),
        _ => return Err(format!("{}: the `[package]` section needs a `name`", path.display())),
    };
    if !is_ident(&name) {
        return Err(format!("{}: `{name}` is not a valid package name", path.display()));
    }
    let dir = manifest_dir_of(path);
    // The workspace this package inherits from: its own manifest when it is a root, else the root above it. It is read
    // only when something is inherited, so an ordinary package never looks above its directory.
    let above: Option<(Document, PathBuf)> = if doc.workspace.is_none() && doc.inherits() {
        match workspace::root_of(&dir)? {
            Some(root_dir) => Some((read_root(&root_dir)?, root_dir)),
            None => None,
        }
    } else {
        None
    };
    let root: Option<(&Document, &Path)> = if doc.workspace.is_some() {
        Some((&doc, dir.as_path()))
    } else {
        above.as_ref().map(|(root_doc, root_dir)| (root_doc, root_dir.as_path()))
    };

    let version = inherited_value(&doc.fields.version, "version", path, root, |f| &f.version)?;
    let license = inherited_value(&doc.fields.license, "license", path, root, |f| &f.license)?;
    let authors = inherited_value(&doc.fields.authors, "authors", path, root, |f| &f.authors)?.unwrap_or_default();
    let edition = match inherited_value(&doc.fields.edition, "edition", path, root, |f| &f.edition)? {
        Some(text) => burdin_sema::Edition::parse(&text)
            .ok_or_else(|| format!("{}: {}", path.display(), unknown_edition(&text)))?,
        None => burdin_sema::Edition::default(),
    };
    let mut deps = Vec::with_capacity(doc.deps.len());
    for (dep, value, line) in &doc.deps {
        match value {
            DepValue::Source(source) => deps.push((dep.clone(), source.clone())),
            DepValue::Inherit => {
                let Some((root_doc, root_dir)) = root else {
                    return Err(format!(
                        "{}:{line}: dependency `{dep}` inherits from the workspace, but no workspace root above this \
                         package lists it as a member",
                        path.display()
                    ));
                };
                let Some((_, source, _)) = root_doc.workspace_deps.iter().find(|(declared, _, _)| declared == dep)
                else {
                    return Err(format!(
                        "{}:{line}: dependency `{dep}` inherits from the workspace, but `{}` declares no \
                         `[workspace.dependencies] {dep}`",
                        path.display(),
                        root_dir.join(workspace::MANIFEST).display()
                    ));
                };
                deps.push((dep.clone(), rebase(source, root_dir, &dir)));
            }
        }
    }
    Ok(Manifest {
        name,
        version,
        license,
        authors,
        deps,
        build_allow: doc.build_allow.clone(),
        lint: doc.lint.clone(),
        rust: doc.rust.clone(),
        edition,
    })
}

/// Whether the manifest of `dir` is a workspace root with no `[package]` (WS1): such a directory holds members, not a
/// package, so a tool that loads packages skips it.
pub fn is_virtual_root(dir: &Path) -> bool {
    let path = dir.join(workspace::MANIFEST);
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_document(&text, &path).is_ok_and(|doc| doc.workspace.is_some() && !doc.package_seen),
        Err(_) => false,
    }
}

/// Whether the manifest `text` takes its `edition` from the workspace (WS15). `burdin fix --migrate` moves a package's
/// own edition, so it refuses such a package.
pub fn edition_is_inherited(text: &str, path: &Path) -> Result<bool, String> {
    Ok(matches!(parse_document(text, path)?.fields.edition, Some(Slot::Inherit(_))))
}

/// The directory of a manifest, canonical when it exists.
fn manifest_dir_of(path: &Path) -> PathBuf {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf())
}

/// The document of the workspace root in `dir`.
fn read_root(dir: &Path) -> Result<Document, String> {
    let path = dir.join(workspace::MANIFEST);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_document(&text, &path)
}

/// The value of a `[package]` field: the one the manifest sets, or the one its workspace root sets under
/// `[workspace.package]` (WS15). `None` when neither sets it.
fn inherited_value<T: Clone>(
    slot: &Option<Slot<T>>,
    key: &str,
    path: &Path,
    root: Option<(&Document, &Path)>,
    from_root: fn(&Fields) -> &Option<Slot<T>>,
) -> Result<Option<T>, String> {
    match slot {
        None => Ok(None),
        Some(Slot::Set(value, _)) => Ok(Some(value.clone())),
        Some(Slot::Inherit(line)) => {
            let Some((root_doc, root_dir)) = root else {
                return Err(format!(
                    "{}:{line}: `{key}` inherits from the workspace, but no workspace root above this package lists it \
                     as a member",
                    path.display()
                ));
            };
            match from_root(&root_doc.workspace_fields) {
                Some(Slot::Set(value, _)) => Ok(Some(value.clone())),
                _ => Err(format!(
                    "{}:{line}: `{key}` inherits from the workspace, but `{}` does not set `[workspace.package] {key}`",
                    path.display(),
                    root_dir.join(workspace::MANIFEST).display()
                )),
            }
        }
    }
}

/// A dependency's source as the member reads it. A path in `[workspace.dependencies]` is relative to the workspace root,
/// so it is rewritten relative to the member's directory (WS16); a git source is the same everywhere.
fn rebase(source: &DepSource, root_dir: &Path, member_dir: &Path) -> DepSource {
    match source {
        DepSource::Path(relative) if root_dir != member_dir => {
            DepSource::Path(relative_between(member_dir, &lexical_join(root_dir, relative)))
        }
        other => other.clone(),
    }
}

/// `base` joined with a relative path that may use `..`, normalized without touching the disk.
fn lexical_join(base: &Path, relative: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    for part in Path::new(relative).components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => out.push(name),
            _ => {}
        }
    }
    out
}

/// `to` as a path relative to `from`, with `/` separators. Both are absolute and normalized (WS16).
fn relative_between(from: &Path, to: &Path) -> String {
    let from: Vec<Component> = from.components().collect();
    let to: Vec<Component> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_string(); from.len() - common];
    parts.extend(to[common..].iter().map(|part| part.as_os_str().to_string_lossy().into_owned()));
    if parts.is_empty() { ".".to_string() } else { parts.join("/") }
}

/// The error for an edition this compiler does not know.
fn unknown_edition(text: &str) -> String {
    let known: Vec<&str> = burdin_sema::Edition::ALL.iter().map(|e| e.as_str()).collect();
    format!("unknown edition `{text}` (this compiler knows {})", known.join(" and "))
}

/// Reads a manifest line by line: the sections and keys the compiler reads, with their lines. Every error names the
/// line (`spec/packages.md`, `spec/workspaces.md`).
pub(crate) fn parse_document(text: &str, path: &Path) -> Result<Document, String> {
    let mut doc = Document::default();
    let mut section = String::new();
    for (index, raw) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let at = format!("{}:{line_number}", path.display());
        if line.starts_with('[') {
            if !line.ends_with(']') {
                return Err(format!("{at}: a section header must end with `]`"));
            }
            section = line[1..line.len() - 1].trim().to_string();
            match section.as_str() {
                "package" => doc.package_seen = true,
                "workspace" => {
                    doc.workspace.get_or_insert_with(WorkspaceSection::default);
                }
                _ => {}
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("{at}: expected `key = value`"));
        };
        let (key, value) = (key.trim(), value.trim());
        // `key.workspace = true` inherits the key from the workspace (WS15-WS16).
        let (key, inherits) = match key.strip_suffix(".workspace") {
            Some(base) => (base.trim(), true),
            None => (key, false),
        };
        if inherits {
            if value != "true" {
                return Err(format!("{at}: `{key}.workspace` must be `true`"));
            }
            match section.as_str() {
                "package" | "dependencies" => {}
                "workspace.package" | "workspace.dependencies" => {
                    return Err(format!("{at}: `[{section}]` cannot inherit from the workspace it defines"));
                }
                _ => {
                    return Err(format!(
                        "{at}: `{key}.workspace` is not inherited here: only `[package]` values and `[dependencies]` \
                         take `workspace = true`"
                    ));
                }
            }
        }
        match section.as_str() {
            "package" if key == "name" => {
                if inherits {
                    return Err(format!("{at}: `name` cannot be inherited from the workspace"));
                }
                let name = parse_string(value).map_err(|e| format!("{at}: {e}"))?;
                doc.name = Some((name, line_number));
            }
            "package" => set_package_field(&mut doc.fields, key, value, inherits, line_number, &at)?,
            "dependencies" => {
                if !is_ident(key) {
                    return Err(format!("{at}: `{key}` is not a valid dependency name"));
                }
                if value == "false" {
                    return Err(format!(
                        "{at}: `{key} = false` means nothing: remove the line (a dependency you do not use needs no \
                         entry)"
                    ));
                }
                let dep = if inherits {
                    DepValue::Inherit
                } else {
                    parse_dependency_value(value).map_err(|e| format!("{at}: {e}"))?
                };
                doc.deps.push((key.to_string(), dep, line_number));
            }
            "lint" => {
                let level = parse_string(value).map_err(|e| format!("{at}: {e}"))?;
                doc.lint.push((key.to_string(), level, line_number));
            }
            "rust" if key == "toolchain" => {
                doc.rust.toolchain = Some(parse_string(value).map_err(|e| format!("{at}: {e}"))?);
            }
            "rust.crates" => {
                if !is_ident(key) {
                    return Err(format!("{at}: `{key}` is not a valid module name for a Rust crate"));
                }
                let crate_path = match parse_dependency(value).map_err(|e| format!("{at}: {e}"))? {
                    DepSource::Path(crate_path) => crate_path,
                    DepSource::Git { .. } | DepSource::Official => {
                        return Err(format!("{at}: a Rust crate must be a `path`"));
                    }
                };
                doc.rust.crates.push((key.to_string(), crate_path));
            }
            "build" if key == "allow" => {
                let names = parse_string_array(value).map_err(|e| format!("{at}: {e}"))?;
                for allowed in names {
                    if !is_ident(&allowed) {
                        return Err(format!("{at}: `{allowed}` is not a valid package name in `[build] allow`"));
                    }
                    if !doc.build_allow.contains(&allowed) {
                        doc.build_allow.push(allowed);
                    }
                }
            }
            "workspace" => {
                let entries: Vec<(String, usize)> = parse_string_array(value)
                    .map_err(|e| format!("{at}: {e}"))?
                    .into_iter()
                    .map(|entry| (entry, line_number))
                    .collect();
                let section = doc.workspace.get_or_insert_with(WorkspaceSection::default);
                match key {
                    "members" => section.members = entries,
                    "exclude" => section.exclude = entries,
                    "default-members" => section.default_members = entries,
                    _ => {}
                }
            }
            "workspace.package" => set_package_field(&mut doc.workspace_fields, key, value, false, line_number, &at)?,
            "workspace.dependencies" => {
                if !is_ident(key) {
                    return Err(format!("{at}: `{key}` is not a valid dependency name"));
                }
                if value == "false" {
                    return Err(format!("{at}: `{key} = false` means nothing: remove the line"));
                }
                let source = parse_dependency(value).map_err(|e| format!("{at}: {e}"))?;
                doc.workspace_deps.push((key.to_string(), source, line_number));
            }
            _ => {}
        }
    }
    let virtual_root = doc.workspace.is_some() && !doc.package_seen;
    if let Some((_, _, line)) = doc.deps.first().filter(|_| virtual_root) {
        return Err(format!(
            "{}:{line}: a workspace root without a `[package]` section cannot declare `[dependencies]`: a virtual \
             workspace has no package of its own",
            path.display()
        ));
    }
    Ok(doc)
}

/// Sets one `[package]` value: `version`, `edition`, `license`, or `authors` (WS15). A key that is not one of those is
/// left alone, unless it is inherited, which only those four can be.
fn set_package_field(
    fields: &mut Fields,
    key: &str,
    value: &str,
    inherits: bool,
    line: usize,
    at: &str,
) -> Result<(), String> {
    match key {
        "version" => put(&mut fields.version, key, text_slot(value, inherits, line, at)?, at)?,
        "license" => put(&mut fields.license, key, text_slot(value, inherits, line, at)?, at)?,
        "edition" => {
            let slot = text_slot(value, inherits, line, at)?;
            if let Slot::Set(text, _) = &slot
                && burdin_sema::Edition::parse(text).is_none()
            {
                return Err(format!("{at}: {}", unknown_edition(text)));
            }
            put(&mut fields.edition, key, slot, at)?;
        }
        "authors" => {
            let slot = if inherits || workspace_table(value).map_err(|e| format!("{at}: {e}"))?.is_some() {
                Slot::Inherit(line)
            } else {
                Slot::Set(parse_string_array(value).map_err(|e| format!("{at}: {e}"))?, line)
            };
            put(&mut fields.authors, key, slot, at)?;
        }
        _ if inherits => {
            return Err(format!(
                "{at}: `{key}` cannot be inherited from the workspace: only version, edition, license, and authors can be"
            ));
        }
        _ => {}
    }
    Ok(())
}

/// A text `[package]` value, or the inheritance of one.
fn text_slot(value: &str, inherits: bool, line: usize, at: &str) -> Result<Slot<String>, String> {
    if inherits || workspace_table(value).map_err(|e| format!("{at}: {e}"))?.is_some() {
        return Ok(Slot::Inherit(line));
    }
    Ok(Slot::Set(parse_string(value).map_err(|e| format!("{at}: {e}"))?, line))
}

/// Stores a `[package]` value. A key both set and inherited is an error that names the other line (WS15); a second
/// value of the same kind replaces the first, as a repeated key always has.
fn put<T>(target: &mut Option<Slot<T>>, key: &str, slot: Slot<T>, at: &str) -> Result<(), String> {
    if let Some(previous) = target.as_ref() {
        let other = match (previous, &slot) {
            (Slot::Set(_, line), Slot::Inherit(_)) | (Slot::Inherit(line), Slot::Set(_, _)) => Some(*line),
            _ => None,
        };
        if let Some(other) = other {
            return Err(format!(
                "{at}: `{key}` is both set and inherited from the workspace (line {other}); give it one or the other"
            ));
        }
    }
    *target = Some(slot);
    Ok(())
}

/// `Some(())` when `value` is the inline table `{ workspace = true }`, the spelling of `key.workspace = true`; `None`
/// when `value` is not a table with a `workspace` key. Any other table with that key is an error (WS15-WS16).
fn workspace_table(value: &str) -> Result<Option<()>, String> {
    if !(value.starts_with('{') && value.ends_with('}')) {
        return Ok(None);
    }
    let parts = split_table(&value[1..value.len() - 1]);
    let has_workspace = parts.iter().any(|part| part.split_once('=').is_some_and(|(key, _)| key.trim() == "workspace"));
    if !has_workspace {
        return Ok(None);
    }
    let [part] = parts.as_slice() else {
        return Err("`workspace = true` takes no other keys: the value comes from the workspace".to_string());
    };
    let flag = part.split_once('=').map_or("", |(_, flag)| flag.trim());
    if flag != "true" {
        return Err("`workspace` must be `true`".to_string());
    }
    Ok(Some(()))
}

/// A `["a", "b"]` array of quoted strings, also accepting a single `"a"`.
pub(crate) fn parse_string_array(value: &str) -> Result<Vec<String>, String> {
    if value.starts_with('"') {
        return Ok(vec![parse_string(value)?]);
    }
    if !(value.starts_with('[') && value.ends_with(']')) {
        return Err("`allow` must be an array of quoted package names, for example [\"lib\"]".to_string());
    }
    let mut names = Vec::new();
    for part in split_table(&value[1..value.len() - 1]) {
        names.push(parse_string(part.trim())?);
    }
    Ok(names)
}

/// A `"..."` string, with `\"`, `\\`, `\n`, and `\t` escapes.
pub(crate) fn parse_string(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'"' || bytes[bytes.len() - 1] != b'"' {
        return Err("expected a quoted string".to_string());
    }
    let inner = &value[1..value.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => return Err(format!("unknown escape `\\{other}`")),
            None => return Err("a string ends with a backslash".to_string()),
        }
    }
    Ok(out)
}

/// `{ path = "../lib" }` (or a bare `"../lib"` as a shorthand), `{ git = "<url>" }` with at most one of `branch`,
/// `tag`, and `rev`, or `true` for the official package of that name (`spec/packages.md`, PKG1-PKG2, PKG15). Other
/// keys are ignored, so a newer manifest loads.
fn parse_dependency(value: &str) -> Result<DepSource, String> {
    if workspace_table(value)?.is_some() {
        return Err("`{ workspace = true }` takes its source from `[workspace.dependencies]`, so it is valid only in \
                    `[dependencies]`"
            .to_string());
    }
    parse_source(value)
}

/// A `[dependencies]` value: a source, or `{ workspace = true }`, which takes the source the workspace declares (WS16).
fn parse_dependency_value(value: &str) -> Result<DepValue, String> {
    match workspace_table(value)? {
        Some(()) => Ok(DepValue::Inherit),
        None => parse_source(value).map(DepValue::Source),
    }
}

/// The source of a dependency that is written out (not inherited): see the forms in the doc comment above
/// [`parse_dependency`].
fn parse_source(value: &str) -> Result<DepSource, String> {
    if value.starts_with('"') {
        return parse_string(value).map(DepSource::Path);
    }
    if value == "true" {
        return Ok(DepSource::Official);
    }
    if !(value.starts_with('{') && value.ends_with('}')) {
        return Err("a dependency must be `{ path = \"...\" }`, `{ git = \"...\" }`, or `true` for an official \
                    package"
            .to_string());
    }
    let inner = &value[1..value.len() - 1];
    let mut path = None;
    let mut git = None;
    let mut references: Vec<GitRef> = Vec::new();
    let mut has_version = false;
    for part in split_table(inner) {
        let Some((key, value)) = part.split_once('=') else {
            return Err(format!("expected `key = value` in the dependency table, found `{}`", part.trim()));
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "path" => path = Some(parse_string(value)?),
            "git" => git = Some(parse_string(value)?),
            "branch" => references.push(GitRef::Branch(parse_string(value)?)),
            "tag" => references.push(GitRef::Tag(parse_string(value)?)),
            "rev" => references.push(GitRef::Rev(parse_string(value)?)),
            "version" => has_version = true,
            _ => {}
        }
    }
    if references.len() > 1 {
        return Err("a git dependency takes at most one of `branch`, `tag`, and `rev`".to_string());
    }
    match (path, git) {
        (Some(_), Some(_)) => Err("a dependency takes a `path` or a `git` repository, not both".to_string()),
        (Some(_), None) if !references.is_empty() => {
            Err("`branch`, `tag`, and `rev` apply only to a `git` dependency".to_string())
        }
        (Some(path), None) => Ok(DepSource::Path(path)),
        (None, Some(url)) => {
            if url.trim().is_empty() {
                return Err("a `git` dependency needs a repository URL".to_string());
            }
            Ok(DepSource::Git { url, reference: references.pop().unwrap_or(GitRef::DefaultBranch) })
        }
        (None, None) if !references.is_empty() => {
            Err("`branch`, `tag`, and `rev` apply only to a `git` dependency".to_string())
        }
        (None, None) if has_version => Err("a dependency needs a `path` or a `git` repository: there is no \
                                            package registry yet, so a `version` alone cannot be resolved"
            .to_string()),
        (None, None) => Err("a dependency needs a `path` or a `git` repository".to_string()),
    }
}

/// Splits the inside of an inline table on commas that are not inside a string.
fn split_table(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if c == ',' {
            parts.push(&text[start..i]);
            start = i + 1;
        }
    }
    parts.push(&text[start..]);
    parts.into_iter().filter(|p| !p.trim().is_empty()).collect()
}

/// The part of a line before a `#` that is not inside a string.
pub fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if c == '#' {
            return &line[..i];
        }
    }
    line
}

/// Every `.bd` file under `src/`, as modules. Directories and file stems must be identifiers, so every module has
/// a name that source code can write.
fn discover_modules(dir: &Path) -> Result<Vec<ModuleFile>, String> {
    let src = dir.join("src");
    if !src.is_dir() {
        return Err(format!("package {} has no `src` directory", source_path_text(dir)));
    }
    let modules = discover_modules_under(&src)?;
    if modules.is_empty() {
        return Err(format!("package {} has no `.bd` files under `src`", source_path_text(dir)));
    }
    Ok(modules)
}

/// Every `.bd` file under one source root, as modules. Used for a package's `src/` and for the generated modules
/// under its build output directory (`spec/build-scripts.md`, BS4).
pub fn discover_modules_under(src: &Path) -> Result<Vec<ModuleFile>, String> {
    let mut modules = Vec::new();
    walk(src, &mut Vec::new(), &mut modules)?;
    modules.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(modules)
}

fn walk(dir: &Path, prefix: &mut Vec<String>, modules: &mut Vec<ModuleFile>) -> Result<(), String> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .map(|e| e.map(|e| e.path()).map_err(|e| format!("cannot read {}: {e}", dir.display())))
        .collect::<Result<_, _>>()?;
    entries.sort();
    for path in entries {
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => return Err(format!("{}: a file name is not valid UTF-8", path.display())),
        };
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            if !is_ident(&name) {
                return Err(format!("{}: a folder name must be a Burdin identifier", path.display()));
            }
            prefix.push(name);
            walk(&path, prefix, modules)?;
            prefix.pop();
        } else if let Some(stem) = name.strip_suffix(".bd") {
            if !is_ident(stem) {
                return Err(format!("{}: a module name must be a Burdin identifier", path.display()));
            }
            prefix.push(stem.to_string());
            modules.push(ModuleFile { path: prefix.clone(), file: path, text: None });
            prefix.pop();
        }
    }
    Ok(())
}

/// `[A-Za-z_][A-Za-z0-9_]*`, the identifiers the lexer accepts.
pub fn is_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    (first.is_ascii_alphabetic() || first == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A temporary package directory, removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let path = std::env::temp_dir().join(format!(
                "burdin-package-{}-{}-{}",
                name,
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
            ));
            fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    // Windows canonical paths carry the `\\?\` verbatim prefix, which is an implementation detail. A message
    // that names a path prints it the way diagnostics do (`source_path_text`), on every platform the same.
    #[test]
    fn the_entry_point_error_prints_the_path_without_the_verbatim_prefix() {
        let temp = TempDir::new("entry");
        fs::write(temp.0.join("burdin.toml"), "[package]\nname = \"app\"\n").unwrap();
        fs::create_dir_all(temp.0.join("src")).unwrap();
        fs::write(temp.0.join("src").join("lib.bd"), "fn nothing() {}\n").unwrap();
        let dir = temp.0.canonicalize().unwrap();
        let Err(error) = load_impl(&dir, true) else {
            panic!("a package without `src/main.bd` must not load");
        };
        let expected = source_path_text(&dir.join("src/main.bd"));
        assert!(error.contains(&expected), "the error does not name {expected}: {error}");
        assert!(!error.contains(r"\\?\"), "the error prints a verbatim path: {error}");
    }

    fn manifest(text: &str) -> Result<Manifest, String> {
        parse_manifest_text(text, Path::new("burdin.toml"))
    }

    #[test]
    fn minimal_manifest() {
        let m = manifest("[package]\nname = \"app\"\nversion = \"0.1.0\"\n").unwrap();
        assert_eq!(m.name, "app");
        assert!(m.deps.is_empty());
    }

    #[test]
    fn comments_and_unknown_keys_are_ignored() {
        let m = manifest("# a comment\n[package]\nname = \"app\" # trailing\nauthors = [\"x\"]\n[other]\nk = \"v\"\n")
            .unwrap();
        assert_eq!(m.name, "app");
    }

    /// Writes `files` (a path relative to the directory, and its text) under a fresh directory, canonical.
    fn workspace_files(name: &str, files: &[(&str, &str)]) -> (TempDir, PathBuf) {
        let temp = TempDir::new(name);
        for (path, text) in files {
            let file = temp.0.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, text).unwrap();
        }
        let dir = temp.0.canonicalize().unwrap();
        (temp, dir)
    }

    /// A root that inherits `version`, `edition`, `license`, and `authors`, and declares `lib` as a dependency.
    const ROOT: &str = "[workspace]\nmembers = [\"app\", \"lib\"]\n\n[workspace.package]\nversion = \"1.2.3\"\n\
        edition = \"2026\"\nlicense = \"MIT\"\nauthors = [\"ann\", \"bo\"]\n\n[workspace.dependencies]\nlib = { path = \"lib\" }\n";

    /// The error of loading the member `app` when its root is `root` and its own manifest is `member`.
    fn member_error(root: &str, member: &str) -> String {
        let (_temp, dir) = workspace_files(
            "member-error",
            &[("burdin.toml", root), ("app/burdin.toml", member), ("lib/burdin.toml", "[package]\nname = \"lib\"\n")],
        );
        match parse_manifest(&dir.join("app/burdin.toml")) {
            Ok(_) => panic!("the member loads, but it should be rejected: {member}"),
            Err(error) => error,
        }
    }

    #[test]
    fn package_fields_are_inherited() {
        let member = "[package]\nname = \"app\"\nversion.workspace = true\nedition.workspace = true\n\
            license = { workspace = true }\nauthors.workspace = true\n";
        let (_temp, dir) = workspace_files(
            "inherit",
            &[("burdin.toml", ROOT), ("app/burdin.toml", member), ("lib/burdin.toml", "[package]\nname = \"lib\"\n")],
        );
        let app = parse_manifest(&dir.join("app/burdin.toml")).unwrap();
        assert_eq!(app.version.as_deref(), Some("1.2.3"));
        assert_eq!(app.license.as_deref(), Some("MIT"));
        assert_eq!(app.authors, ["ann", "bo"]);
        assert_eq!(app.edition.as_str(), "2026");
        assert!(edition_is_inherited(member, Path::new("burdin.toml")).unwrap());
        assert!(
            !edition_is_inherited("[package]\nname = \"a\"\nedition = \"2026\"\n", Path::new("burdin.toml")).unwrap()
        );
    }

    #[test]
    fn conflicting_and_missing_inheritance_is_rejected() {
        // Set and inherited on two lines.
        let error = member_error(ROOT, "[package]\nname = \"app\"\nversion = \"0.1.0\"\nversion.workspace = true\n");
        assert!(error.contains("both set and inherited"), "{error}");
        // The workspace does not set the key.
        let error = member_error(
            "[workspace]\nmembers = [\"app\"]\n[workspace.package]\nversion = \"1.0.0\"\n",
            "[package]\nname = \"app\"\nlicense.workspace = true\n",
        );
        assert!(error.contains("[workspace.package] license"), "{error}");
        // `name` and other keys are never inherited, and `workspace = false` is not a value.
        let error = member_error(ROOT, "[package]\nname.workspace = true\n");
        assert!(error.contains("`name` cannot be inherited"), "{error}");
        let error = member_error(ROOT, "[package]\nname = \"app\"\npath.workspace = true\n");
        assert!(error.contains("cannot be inherited"), "{error}");
        assert!(!member_error(ROOT, "[package]\nname = \"app\"\nversion = { workspace = false }\n").is_empty());
    }

    #[test]
    fn dependencies_are_inherited_from_the_root() {
        let member = "[package]\nname = \"app\"\n\n[dependencies]\nlib.workspace = true\n";
        let (_temp, dir) = workspace_files(
            "inherit-deps",
            &[("burdin.toml", ROOT), ("app/burdin.toml", member), ("lib/burdin.toml", "[package]\nname = \"lib\"\n")],
        );
        let app = parse_manifest(&dir.join("app/burdin.toml")).unwrap();
        let [(name, DepSource::Path(path))] = app.deps.as_slice() else {
            panic!("the inherited dependency is one path dependency: {:?}", app.deps);
        };
        assert_eq!(name, "lib");
        // The workspace's `lib` is `<root>/lib`, and the member reads it from its own directory.
        assert_eq!(path.replace('\\', "/"), "../lib");
    }

    #[test]
    fn undeclared_inherited_dependencies_are_rejected() {
        let error = member_error(ROOT, "[package]\nname = \"app\"\n\n[dependencies]\nnope.workspace = true\n");
        assert!(error.contains("`nope`") && error.contains("[workspace.dependencies] nope"), "{error}");
    }

    #[test]
    fn a_virtual_root_takes_only_workspace_sections() {
        let virtual_root = "[workspace]\nmembers = [\"app\"]\n";
        let (_temp, dir) = workspace_files(
            "virtual",
            &[("burdin.toml", virtual_root), ("app/burdin.toml", "[package]\nname = \"app\"\n")],
        );
        assert!(is_virtual_root(&dir));
        assert!(!is_virtual_root(&dir.join("app")));
        let error = parse_manifest(&dir.join("burdin.toml")).unwrap_err();
        assert!(error.contains("not a package"), "{error}");
        // A virtual root has no package, so it cannot declare dependencies of its own.
        let with_deps = "[workspace]\nmembers = [\"app\"]\n\n[dependencies]\nx = \"y\"\n";
        let (_temp, dir) = workspace_files(
            "virtual-deps",
            &[("burdin.toml", with_deps), ("app/burdin.toml", "[package]\nname = \"app\"\n")],
        );
        let error = parse_manifest(&dir.join("burdin.toml")).unwrap_err();
        assert!(error.contains("cannot declare `[dependencies]`"), "{error}");
    }

    #[test]
    fn dependency_forms() {
        let m =
            manifest("[package]\nname = \"app\"\n[dependencies]\nlib = { path = \"../lib\" }\nother = \"../other\"\n")
                .unwrap();
        assert_eq!(
            m.deps,
            vec![
                ("lib".to_string(), DepSource::Path("../lib".to_string())),
                ("other".to_string(), DepSource::Path("../other".to_string()))
            ]
        );
    }

    #[test]
    fn git_dependency_forms() {
        let text = "[package]
name = \"app\"
version = \"0.3.1\"
[dependencies]
a = { git = \"https://example.com/a.git\" }
b = { git = \"https://example.com/b.git\", branch = \"main\" }
c = { git = \"https://example.com/c.git\", tag = \"v1\" }
d = { git = \"https://example.com/d.git\", rev = \"1a2b3c\" }
";
        let m = manifest(text).unwrap();
        assert_eq!(m.version.as_deref(), Some("0.3.1"));
        let git = |url: &str, reference: GitRef| DepSource::Git { url: url.to_string(), reference };
        assert_eq!(
            m.deps,
            vec![
                ("a".to_string(), git("https://example.com/a.git", GitRef::DefaultBranch)),
                ("b".to_string(), git("https://example.com/b.git", GitRef::Branch("main".into()))),
                ("c".to_string(), git("https://example.com/c.git", GitRef::Tag("v1".into()))),
                ("d".to_string(), git("https://example.com/d.git", GitRef::Rev("1a2b3c".into()))),
            ]
        );
    }

    #[test]
    fn official_dependency_forms() {
        let m = manifest("[package]\nname = \"app\"\n[dependencies]\njson = true\nlib = \"../lib\"\n").unwrap();
        assert_eq!(
            m.deps,
            vec![("json".to_string(), DepSource::Official), ("lib".to_string(), DepSource::Path("../lib".to_string()))]
        );
        let error = manifest("[package]\nname = \"app\"\n[dependencies]\njson = false\n").unwrap_err();
        assert!(error.contains("`json = false` means nothing"), "{error}");
        assert!(error.starts_with("burdin.toml:4:"), "{error}");
        // Only a dependency of a package takes `true`: a script and a Rust crate take paths.
        let script = "//! [dependencies]\n//! json = true\nfn main() {}\n";
        assert!(script_manifest(script).unwrap_err().contains("must be a `path`"));
        assert!(manifest("[package]\nname = \"app\"\n[rust.crates]\ngeo = true\n").unwrap_err().contains("`path`"));
        // Not a boolean spelling the manifest knows.
        assert!(manifest("[package]\nname = \"app\"\n[dependencies]\njson = yes\n").is_err());
    }

    #[test]
    fn official_roots_are_searched_in_order() {
        let temp = TempDir::new("official");
        let root = temp.0.join("packages");
        for name in ["toml", "json"] {
            fs::create_dir_all(root.join(name).join("src")).unwrap();
            fs::write(root.join(name).join("burdin.toml"), format!("[package]\nname = \"{name}\"\n")).unwrap();
        }
        // A directory without a manifest and a file are not packages.
        fs::create_dir_all(root.join("notes")).unwrap();
        fs::write(root.join("README.md"), "official packages\n").unwrap();
        assert_eq!(official_names_in(&root), ["json", "toml"]);
        assert_eq!(official_dir_in(&root, "json").unwrap(), root.join("json"));
        let error = official_dir_in(&root, "yaml").unwrap_err();
        assert!(error.contains("the official packages are: json, toml"), "{error}");
        assert!(error.contains("`{ path = \"...\" }`"), "{error}");
        let empty = temp.0.join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert!(official_dir_in(&empty, "json").unwrap_err().contains("has no official packages"));

        // The environment variable is the only place searched when it is set. Nothing else in this crate's tests
        // reads it, so setting it here cannot disturb them.
        // SAFETY: no other thread of these tests reads or writes the environment.
        unsafe { std::env::set_var(PACKAGES_ENV, &root) };
        assert_eq!(official_roots(), std::slice::from_ref(&root));
        assert_eq!(official_names(), ["json", "toml"]);
        assert_eq!(official_dir("toml").unwrap(), root.join("toml"));
        unsafe { std::env::set_var(PACKAGES_ENV, temp.0.join("missing")) };
        assert_eq!(official_names(), Vec::<String>::new());
        assert!(official_dir("json").unwrap_err().contains("found no packages directory"));
        unsafe { std::env::remove_var(PACKAGES_ENV) };
        let roots = official_roots();
        assert!(roots[0].ends_with("../packages") && roots[1].ends_with("packages"), "{roots:?}");
        #[cfg(debug_assertions)]
        assert!(roots.last().unwrap().ends_with("../../packages"), "{roots:?}");
    }

    #[test]
    fn invalid_dependency_tables_are_rejected() {
        let dep = |table: &str| manifest(&format!("[package]\nname = \"app\"\n[dependencies]\nlib = {table}\n"));
        assert!(dep("{ git = \"x\", tag = \"a\", branch = \"b\" }").unwrap_err().contains("at most one"));
        assert!(dep("{ path = \"../lib\", git = \"x\" }").unwrap_err().contains("not both"));
        assert!(dep("{ path = \"../lib\", tag = \"v1\" }").unwrap_err().contains("only to a `git`"));
        assert!(dep("{ version = \"1.0\" }").unwrap_err().contains("no package registry"));
        assert!(dep("{ git = \"\" }").unwrap_err().contains("repository URL"));
        assert!(dep("{ }").unwrap_err().contains("needs a `path` or a `git`"));
    }

    #[test]
    fn scripts_and_rust_crates_take_only_paths() {
        let script = "//! [dependencies]\n//! lib = { git = \"https://example.com/lib.git\" }\nfn main() {}\n";
        assert!(script_manifest(script).unwrap_err().contains("must be a `path`"));
        let rust = "[package]\nname = \"app\"\n[rust.crates]\ngeo = { git = \"x\" }\n";
        assert!(manifest(rust).unwrap_err().contains("must be a `path`"));
    }

    #[test]
    fn the_rust_tables_are_parsed() {
        let text = "[package]
name = \"app\"
[rust]
toolchain = \"1.100.0\"
[rust.crates]
geometry = { path = \"../geometry\" }
other = \"../other\"
";
        let m = manifest(text).unwrap();
        assert_eq!(m.rust.toolchain.as_deref(), Some("1.100.0"));
        assert_eq!(
            m.rust.crates,
            vec![("geometry".to_string(), "../geometry".to_string()), ("other".to_string(), "../other".to_string())]
        );
        assert!(
            manifest(
                "[package]
name = \"app\"
[rust.crates]
bad-name = \"x\"
"
            )
            .is_err()
        );
        assert_eq!(
            manifest(
                "[package]
name = \"app\"
"
            )
            .unwrap()
            .rust,
            RustConfig::default()
        );
    }

    #[test]
    fn build_allow_grants_are_parsed_and_deduplicated() {
        let m =
            manifest("[package]\nname = \"app\"\n[build]\nallow = [\"lib\", \"other\"]\nallow = [\"lib\"]\n").unwrap();
        assert_eq!(m.build_allow, vec!["lib".to_string(), "other".to_string()]);
        let single = manifest("[package]\nname = \"app\"\n[build]\nallow = \"lib\"\n").unwrap();
        assert_eq!(single.build_allow, vec!["lib".to_string()]);
        assert!(manifest("[package]\nname = \"app\"\n[build]\nallow = 3\n").is_err());
        assert!(manifest("[package]\nname = \"app\"\n[build]\nallow = [\"not an ident\"]\n").is_err());
    }

    #[test]
    fn malformed_manifests_are_rejected() {
        assert!(manifest("[package]\nversion = \"1\"\n").is_err(), "a package needs a name");
        assert!(manifest("[package]\nname = \"not an ident\"\n").is_err());
        assert!(manifest("[package]\nname = \"app\"\n[dependencies]\nlib = { other = \"x\" }\n").is_err());
        assert!(manifest("[package]\nname = \"app\"\n[dependencies]\nlib = { path = \"../lib\"\n").is_err());
        assert!(manifest("[package]\nname = \"unterminated\n").is_err());
    }

    #[test]
    fn identifiers_match_the_lexer() {
        for ok in ["a", "_x", "Http2", "net_tcp"] {
            assert!(is_ident(ok), "{ok}");
        }
        for bad in ["", "2x", "a-b", "a.b", "a b"] {
            assert!(!is_ident(bad), "{bad}");
        }
    }

    #[test]
    fn inline_tables_split_on_top_level_commas() {
        assert_eq!(split_table(" path = \"a,b\", version = \"1\" "), vec![" path = \"a,b\"", " version = \"1\" "]);
        assert!(strip_comment("name = \"a#b\" # c").contains("\"a#b\""));
    }
}
