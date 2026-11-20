//! Packages: `burdin.toml`, every `.bd` file under `src/` as a module, and path, git, and official dependencies.
//!
//! A single `.bd` file is a package with one module (its path is empty). A directory is a package described by
//! its `burdin.toml`; its entry point is `src/main.bd`, and `[dependencies]` are loaded recursively from their
//! `path`, from the checkout of the commit `burdin.lock` pins for a `git` dependency, or from the compiler's own
//! `packages/` folder for an official one (`name = true`; `spec/packages.md`). The loader never fetches: `burdin_pkg` resolves and checks out git dependencies before a build. Module paths are
//! relative to `src/` (`geometry/shapes.bd` is module `geometry.shapes`).
//!
//! Which dependencies a build loads is decided here: features, optional dependencies, dev-dependencies of the entry
//! package in a test build, and `[target.'cfg(...)']` tables (`spec/packages.md`, PKG25-PKG33). The loader activates
//! packages from the entry package outward, until the features of every package are settled (`Loader::activate`).

use crate::cfg::{self, Cfg, Host};
use crate::features::{self, Item};
use crate::lockfile::{self, GitRef, Lockfile};
use burdin_sema::EntryMode;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// One source file of a package.
pub struct ModuleFile {
    /// The path inside `src/`, without the `.bd` suffix: `["geometry", "shapes"]`. Empty for a one-file target.
    pub path: Vec<String>,
    pub file: PathBuf,
    /// The source text of an embedded module (the standard library) or of a generated one; `None` for a file on
    /// disk.
    pub text: Option<Cow<'static, str>>,
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

/// What a command asks for beyond the manifests: the feature flags of the command line (PKG29, PKG31).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeatureRequest {
    /// `--features`: the items to enable for the entry package, each as in PKG28. A list may be split in several.
    pub items: Vec<String>,
    /// `--no-default-features`: the entry package does not enable its `default` feature.
    pub no_default_features: bool,
    /// `--all-features`: the entry package enables every declared feature and every optional dependency.
    pub all_features: bool,
}

impl FeatureRequest {
    /// Whether the request is what a command with no feature flags makes: the defaults.
    pub fn is_default(&self) -> bool {
        *self == FeatureRequest::default()
    }
}

/// Loads the target for `mode`: a `.bd` file, or a directory containing `burdin.toml`. A package directory must have a
/// `src/main.bd` entry point when `mode` wants `main` (`EntryMode::wants_main`), because `build` and `run` need one.
/// The entry package's dev-dependencies are loaded for every other mode (PKG26), and `request` selects its features
/// (PKG29). The features of a one-file target are not selectable: a request other than the default is an error.
pub fn load_selected(target: &Path, mode: EntryMode, request: &FeatureRequest) -> Result<Workspace, String> {
    load_impl(target, mode.wants_main(), mode != EntryMode::Build, request)
}

/// Loads the target: a `.bd` file, or a directory containing `burdin.toml`. A package directory must have a
/// `src/main.bd` entry point, because `build` and `run` need one.
pub fn load(target: &Path) -> Result<Workspace, String> {
    load_selected(target, EntryMode::Build, &FeatureRequest::default())
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
    load_selected(target, EntryMode::Test, &FeatureRequest::default())
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

fn load_impl(target: &Path, require_entry: bool, dev: bool, request: &FeatureRequest) -> Result<Workspace, String> {
    if target.is_dir() {
        let dir = target.canonicalize().map_err(|e| format!("cannot open {}: {e}", target.display()))?;
        // The entry package's lockfile says which commit each git dependency builds with, anywhere in the graph.
        let lock = if dir.join("burdin.toml").is_file() { lockfile::read(&dir)? } else { None };
        let mut loader = Loader::new(lock);
        let root = loader.add_root(&dir)?;
        loader.activate(root, dev, request)?;
        let packages = loader.into_packages(None)?;
        let package = &packages[root];
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
        let mut packages = packages;
        attach_std(&mut packages);
        Ok(Workspace { packages, entry_package: root, entry_module, stem })
    } else if target.is_file() {
        if !request.is_default() {
            return Err(format!(
                "{}: a one-file target has no features: the feature flags need a package with a burdin.toml",
                source_path_text(target)
            ));
        }
        let stem = target.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "a".into());
        let path = source_path_text(target);
        let text = std::fs::read_to_string(target).map_err(|e| format!("cannot read {path}: {e}"))?;
        let manifest = script_manifest(&text).map_err(|e| format!("{path}:{e}"))?;
        let base = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let script = ModuleFile { path: Vec::new(), file: target.to_path_buf(), text: None };
        let Some(manifest) = manifest else {
            let mut packages = vec![PackageFiles {
                name: stem.clone(),
                dir: None,
                modules: vec![script],
                deps: Vec::new(),
                build_allow: Vec::new(),
                root_module: Some(0),
                lint: Vec::new(),
                rust: Default::default(),
                edition: Default::default(),
            }];
            attach_std(&mut packages);
            return Ok(Workspace { packages, entry_package: 0, entry_module: 0, stem });
        };
        // The script's own name is its package name; a dependency with the same name would be ambiguous.
        if manifest.deps.iter().any(|(name, _)| *name == stem) {
            return Err(format!("{path}: a dependency is also named `{stem}`, the script's own package name"));
        }
        // A script's dependencies are paths relative to the script, and it has no features: the script is the root
        // of the graph, with a manifest of its own that names the dependencies and nothing else.
        let script_manifest = Manifest {
            name: stem.clone(),
            version: None,
            deps: manifest
                .deps
                .into_iter()
                .map(|(name, relative)| Dependency {
                    name,
                    source: DepSource::Path(relative),
                    optional: false,
                    default_features: true,
                    features: Vec::new(),
                    target: None,
                })
                .collect(),
            dev_deps: Vec::new(),
            features: Vec::new(),
            features_section: false,
            build_allow: Vec::new(),
            lint: Vec::new(),
            rust: Default::default(),
            edition: Default::default(),
            profiles: Vec::new(),
            include: None,
            exclude: Vec::new(),
        };
        let mut loader = Loader::new(None);
        let root = loader.add_node(base.to_path_buf(), target.to_path_buf(), script_manifest)?;
        loader.activate(root, false, request)?;
        let mut packages = loader.into_packages(Some(script))?;
        attach_std(&mut packages);
        Ok(Workspace { packages, entry_package: root, entry_module: 0, stem })
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
            text: Some(Cow::Borrowed(text)),
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

/// The names a package's manifest declares as dependencies, in every table (normal, dev, and target-specific), whether
/// or not a given build loads them; empty when the manifest does not read.
pub fn dependency_names(dir: &Path) -> Vec<String> {
    match parse_manifest(&dir.join("burdin.toml")) {
        Ok(manifest) => manifest.deps.iter().chain(&manifest.dev_deps).map(|dep| dep.name.clone()).collect(),
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

/// The loader's state for one package of the build. The index of a node is its package's index in the workspace.
struct Node {
    /// The canonical directory of the package.
    dir: PathBuf,
    manifest_path: PathBuf,
    manifest: Manifest,
    state: Activation,
    /// The dependencies this package loads in this build: `(name used by use, node)`, in declaration order.
    edges: Vec<(String, usize)>,
}

/// What a build enables in one package (PKG28-PKG30). Each set only grows while the loader settles the graph.
#[derive(Default)]
struct Activation {
    /// Whether the build loads this package at all.
    active: bool,
    /// The items enabled here: feature names, `dep:x`, and `x/feature` (PKG28). An item of a feature is added here
    /// when the feature is enabled.
    enabled: BTreeSet<String>,
    /// The optional dependencies this package enables: named by `dep:x`, by `x/feature`, or by the implicit feature
    /// `x`.
    optional_on: BTreeSet<String>,
    /// The features `x/feature` requests of each dependency, keyed by the dependency's name (PKG28, PKG30).
    forwarded: BTreeMap<String, BTreeSet<String>>,
}

struct Loader {
    /// Every package the build loads so far. A package is loaded when a dependency of an active package names it.
    nodes: Vec<Node>,
    /// The node of each loaded directory, for a diamond dependency to reuse its package.
    by_dir: HashMap<PathBuf, usize>,
    /// The directory each package name came from: a name must identify exactly one package, because it is part of
    /// every qualified item name.
    names: HashMap<String, PathBuf>,
    /// The entry package's `burdin.lock`, which pins every git dependency to a commit.
    lock: Option<Lockfile>,
}

impl Loader {
    fn new(lock: Option<Lockfile>) -> Loader {
        Loader { nodes: Vec::new(), by_dir: HashMap::new(), names: HashMap::new(), lock }
    }

    /// Adds the entry package in `dir` (canonical), read from its `burdin.toml`.
    fn add_root(&mut self, dir: &Path) -> Result<usize, String> {
        let manifest_path = dir.join("burdin.toml");
        let manifest = parse_manifest(&manifest_path)?;
        self.add_node(dir.to_path_buf(), manifest_path, manifest)
    }

    /// Adds a package whose manifest is already read. Its name must be unique in the build (PKG1).
    fn add_node(&mut self, dir: PathBuf, manifest_path: PathBuf, manifest: Manifest) -> Result<usize, String> {
        if let Some(previous) = self.names.get(&manifest.name)
            && *previous != dir
        {
            return Err(format!(
                "package `{}` is provided by both {} and {}; package names must be unique",
                manifest.name,
                previous.display(),
                dir.display()
            ));
        }
        self.names.insert(manifest.name.clone(), dir.clone());
        let id = self.nodes.len();
        self.by_dir.insert(dir.clone(), id);
        self.nodes.push(Node { dir, manifest_path, manifest, state: Activation::default(), edges: Vec::new() });
        Ok(id)
    }

    /// The node a dependency of `parent` names: a package already loaded, or a new one read from its manifest.
    fn dependency(&mut self, parent: usize, dep: &Dependency) -> Result<usize, String> {
        let dir =
            self.dependency_dir(&self.nodes[parent].dir, &self.nodes[parent].manifest_path, &dep.name, &dep.source)?;
        if let Some(&id) = self.by_dir.get(&dir) {
            return Ok(id);
        }
        let manifest_path = dir.join("burdin.toml");
        let manifest = parse_manifest(&manifest_path)?;
        self.add_node(dir, manifest_path, manifest)
    }

    /// Enables what the command asks for in the entry package `root` and settles the graph (PKG29-PKG30).
    ///
    /// Each round expands every active package's enabled items and activates the dependencies those items and the
    /// package's tables select. Enabling a feature can enable an optional dependency and request features of another
    /// package, so the rounds repeat until nothing changes. Every set only grows, so the loop ends. A dependency of
    /// a package that is not active is never loaded, and a request for an inactive dependency is dropped.
    fn activate(&mut self, root: usize, dev: bool, request: &FeatureRequest) -> Result<(), String> {
        let Node { manifest, state, .. } = &mut self.nodes[root];
        state.active = true;
        if request.all_features {
            state.enabled.extend(manifest.features.iter().map(|feature| feature.name.clone()));
            state.optional_on.extend(manifest.optional_deps().map(str::to_string));
        } else if !request.no_default_features && manifest.feature("default").is_some() {
            state.enabled.insert("default".to_string());
        }
        state.enabled.extend(request.items.iter().cloned());
        loop {
            let mut changed = false;
            let mut i = 0;
            while i < self.nodes.len() {
                if self.nodes[i].state.active {
                    changed |= self.expand(i)?;
                    changed |= self.activate_dependencies(i, dev && i == root)?;
                }
                i += 1;
            }
            if !changed {
                break;
            }
        }
        self.check_cycles(root)
    }

    /// Expands the enabled items of package `i` (PKG28). Returns whether anything new was enabled.
    fn expand(&mut self, i: usize) -> Result<bool, String> {
        let items: Vec<String> = self.nodes[i].state.enabled.iter().cloned().collect();
        let mut changed = false;
        for item in &items {
            changed |= self.expand_item(i, item)?;
        }
        Ok(changed)
    }

    fn expand_item(&mut self, i: usize, text: &str) -> Result<bool, String> {
        let node = &mut self.nodes[i];
        let package = node.manifest.name.clone();
        let state = &mut node.state;
        Ok(match features::parse_item(text) {
            Item::Name(name) => {
                if let Some(feature) = node.manifest.feature(name) {
                    let mut changed = false;
                    for item in &feature.items {
                        changed |= state.enabled.insert(item.clone());
                    }
                    changed
                } else if node.manifest.optional_deps().any(|dep| dep == name) {
                    state.optional_on.insert(name.to_string())
                } else {
                    return Err(format!("package `{package}` has no feature `{name}`"));
                }
            }
            Item::Dep(dep) => state.optional_on.insert(dep.to_string()),
            Item::Forward { dep, feature } => {
                let mut changed = state.optional_on.insert(dep.to_string());
                changed |= state.forwarded.entry(dep.to_string()).or_default().insert(feature.to_string());
                changed
            }
        })
    }

    /// Activates the dependencies of package `i` that its tables and enabled features select, loads them, and passes
    /// on the features they request (PKG25-PKG27, PKG30). Returns whether anything changed.
    fn activate_dependencies(&mut self, i: usize, dev: bool) -> Result<bool, String> {
        let host = Host::current();
        let deps: Vec<Dependency> = {
            let manifest = &self.nodes[i].manifest;
            let mut deps: Vec<Dependency> = manifest.deps.clone();
            if dev {
                deps.extend(manifest.dev_deps.iter().cloned());
            }
            deps
        };
        let mut changed = false;
        let mut edges = Vec::new();
        for dep in deps {
            if dep.target.as_ref().is_some_and(|cfg| !cfg.matches(host)) {
                continue;
            }
            if dep.optional && !self.nodes[i].state.optional_on.contains(&dep.name) {
                continue;
            }
            let id = self.dependency(i, &dep)?;
            if !self.nodes[id].state.active {
                self.nodes[id].state.active = true;
                changed = true;
            }
            let mut requested: Vec<String> = dep.features.clone();
            if let Some(forwarded) = self.nodes[i].state.forwarded.get(&dep.name) {
                requested.extend(forwarded.iter().cloned());
            }
            let child = &mut self.nodes[id];
            for item in requested {
                changed |= child.state.enabled.insert(item);
            }
            if dep.default_features && child.manifest.feature("default").is_some() {
                changed |= child.state.enabled.insert("default".to_string());
            }
            edges.push((dep.name.clone(), id));
        }
        self.nodes[i].edges = edges;
        Ok(changed)
    }

    /// A dependency cycle among the loaded packages is an error, reported with its chain of directories.
    fn check_cycles(&self, root: usize) -> Result<(), String> {
        fn visit(loader: &Loader, node: usize, stack: &mut Vec<usize>, done: &mut Vec<bool>) -> Result<(), String> {
            if let Some(at) = stack.iter().position(|&n| n == node) {
                let mut chain: Vec<String> =
                    stack[at..].iter().map(|&n| loader.nodes[n].dir.display().to_string()).collect();
                chain.push(loader.nodes[node].dir.display().to_string());
                return Err(format!("dependency cycle: {}", chain.join(" -> ")));
            }
            if done[node] {
                return Ok(());
            }
            stack.push(node);
            for (_, child) in &loader.nodes[node].edges {
                visit(loader, *child, stack, done)?;
            }
            stack.pop();
            done[node] = true;
            Ok(())
        }
        let mut done = vec![false; self.nodes.len()];
        visit(self, root, &mut Vec::new(), &mut done)
    }

    /// The packages of the build, in node order (the entry package first). A script's package is `script` (its one
    /// file, and no manifest), so its modules are not read from a directory.
    fn into_packages(self, script: Option<ModuleFile>) -> Result<Vec<PackageFiles>, String> {
        let mut script = script;
        let mut packages = Vec::with_capacity(self.nodes.len());
        for (id, node) in self.nodes.iter().enumerate() {
            let manifest = &node.manifest;
            // A script is the root of its build: its one file is its only module, and it has no directory.
            let script_file = if id == 0 { script.take() } else { None };
            let is_script = script_file.is_some();
            let mut modules = match script_file {
                Some(file) => vec![file],
                None => discover_modules(&node.dir)?,
            };
            // Other packages reach this one as `use <package>`: prefer `src/<package>.bd`, then `src/lib.bd`.
            let root_module = if is_script {
                Some(0)
            } else {
                modules
                    .iter()
                    .position(|m| m.path.len() == 1 && m.path[0] == manifest.name)
                    .or_else(|| modules.iter().position(|m| m.path.len() == 1 && m.path[0] == "lib"))
            };
            if node.generates_features() {
                if modules.iter().any(|m| m.path == ["features"]) {
                    return Err(format!(
                        "package `{}` has a module `features`, the name of its generated features module: rename it",
                        manifest.name
                    ));
                }
                let entries = node.feature_constants();
                modules.push(ModuleFile {
                    path: vec!["features".to_string()],
                    file: PathBuf::from(format!("<features of {}>/features.bd", manifest.name)),
                    text: Some(Cow::Owned(features::module_text(&entries))),
                });
            }
            packages.push(PackageFiles {
                name: manifest.name.clone(),
                dir: if is_script { None } else { Some(node.dir.clone()) },
                modules,
                deps: node.edges.clone(),
                build_allow: manifest.build_allow.clone(),
                root_module,
                lint: manifest.lint.clone(),
                rust: manifest.rust.clone(),
                edition: manifest.edition,
            });
        }
        Ok(packages)
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

/// One entry of a dependency table: `[dependencies]`, `[dev-dependencies]`, or a `[target.'cfg(...)']` table of either
/// (`spec/packages.md`, PKG25-PKG27).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    /// The name `use` reaches the package by.
    pub name: String,
    pub source: DepSource,
    /// `optional = true`: loaded only when a feature enables it (PKG28).
    pub optional: bool,
    /// `default-features`: whether the dependency's `default` feature is requested (PKG25).
    pub default_features: bool,
    /// `features = [...]`: the features the dependency is asked to enable (PKG25).
    pub features: Vec<String>,
    /// The predicate of a `[target.'cfg(...)']` table; `None` applies to every platform (PKG27).
    pub target: Option<Cfg>,
}

/// One entry of the `[features]` table: its name and the items enabling it turns on (PKG28).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Feature {
    pub name: String,
    pub items: Vec<String>,
    /// The manifest line, for the errors about the feature's items.
    pub line: usize,
}

#[derive(Debug)]
pub struct Manifest {
    pub name: String,
    /// `[package] version`, when the manifest gives one.
    pub version: Option<String>,
    /// `[dependencies]` and the `[target.'cfg(...)'.dependencies]` tables, in file order (PKG25).
    pub deps: Vec<Dependency>,
    /// `[dev-dependencies]` and the target tables of them, in file order (PKG26).
    pub dev_deps: Vec<Dependency>,
    /// The `[features]` entries, in file order (PKG28).
    pub features: Vec<Feature>,
    /// Whether the manifest has a `[features]` section, even an empty one: it gives the package its `features` module
    /// (PKG32).
    pub features_section: bool,
    /// The dependency names whose build scripts the manifest grants (`[build] allow`).
    pub build_allow: Vec<String>,
    /// The `[lint]` entries: `(name, level, line)`.
    pub lint: Vec<(String, String, usize)>,
    pub rust: RustConfig,
    /// `[package] edition`; the default edition when the manifest names none.
    pub edition: burdin_sema::Edition,
    /// The `[profile...]` entries, in file order (`spec/profiles.md`). The parser records them without checking
    /// them: only the entry package's tables are read, and the driver validates the selected profile, so a
    /// dependency's table (which is ignored) cannot fail a build.
    pub profiles: Vec<ProfileEntry>,
    /// `[package] include`: the file patterns `burdin package` selects, when the manifest gives any
    /// (`spec/packages.md`, PKG21).
    pub include: Option<Vec<String>>,
    /// `[package] exclude`: the file patterns `burdin package` leaves out (PKG21).
    pub exclude: Vec<String>,
}

impl Manifest {
    /// The feature named `name`, if the `[features]` table declares it.
    pub fn feature(&self, name: &str) -> Option<&Feature> {
        self.features.iter().find(|feature| feature.name == name)
    }

    /// The names of the optional dependencies (every target's, PKG25), in file order.
    pub fn optional_deps(&self) -> impl Iterator<Item = &str> {
        self.deps.iter().filter(|dep| dep.optional).map(|dep| dep.name.as_str())
    }
}

impl Node {
    /// Whether the package gets a generated `features` module: it has a `[features]` table or an optional dependency
    /// (PKG32).
    fn generates_features(&self) -> bool {
        self.manifest.features_section || self.manifest.optional_deps().next().is_some()
    }

    /// The constants of the generated module: each declared feature and each optional dependency, enabled or not
    /// (PKG32).
    fn feature_constants(&self) -> Vec<(String, bool)> {
        let mut entries: Vec<(String, bool)> = self
            .manifest
            .features
            .iter()
            .map(|feature| (feature.name.clone(), self.state.enabled.contains(&feature.name)))
            .collect();
        entries.extend(
            self.manifest.optional_deps().map(|name| (name.to_string(), self.state.optional_on.contains(name))),
        );
        entries
    }
}

/// One `key = value` line of a `[profile...]` table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileEntry {
    /// The section as written, without the brackets: `profile.dev` or `profile.fast.package.low`.
    pub section: String,
    pub key: String,
    /// The raw right-hand side, still quoted when it is a string.
    pub value: String,
    /// The line number, counted from 1.
    pub line: usize,
}

/// The minimal `burdin.toml` this stage needs: `[package] name = "..."` and `version`, `[dependencies]` entries
/// with a `path`, a `git` repository, or `true`, and `[build] allow = [...]` grants. Unknown sections and keys are ignored, so a newer manifest does
/// not break an older compiler.
pub fn parse_manifest(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_manifest_text(&text, path)
}

pub fn parse_manifest_text(text: &str, path: &Path) -> Result<Manifest, String> {
    let mut section = String::new();
    let mut name: Option<String> = None;
    let mut version: Option<String> = None;
    let mut edition = burdin_sema::Edition::default();
    let mut deps: Vec<Dependency> = Vec::new();
    let mut dev_deps: Vec<Dependency> = Vec::new();
    let mut features: Vec<Feature> = Vec::new();
    let mut features_section = false;
    let mut build_allow: Vec<String> = Vec::new();
    let mut lint: Vec<(String, String, usize)> = Vec::new();
    let mut rust = RustConfig::default();
    // Each dependency name with the line it was declared on: a name is declared once per manifest (PKG25).
    let mut declared: Vec<(String, usize)> = Vec::new();
    let mut profiles: Vec<ProfileEntry> = Vec::new();
    let mut include: Option<Vec<String>> = None;
    let mut exclude: Vec<String> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let where_ = || format!("{}:{}", path.display(), index + 1);
        if line.starts_with('[') {
            if !line.ends_with(']') {
                return Err(format!("{}: a section header must end with `]`", where_()));
            }
            section = line[1..line.len() - 1].trim().to_string();
            if section == "features" {
                features_section = true;
            }
            if let Some(table) = dependency_table(&section) {
                table.map_err(|e| format!("{}: {e}", where_()))?;
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("{}: expected `key = value`", where_()));
        };
        let (key, value) = (key.trim(), value.trim());
        if let Some(table) = dependency_table(&section) {
            let (target, dev) = table.map_err(|e| format!("{}: {e}", where_()))?;
            if !is_ident(key) {
                return Err(format!("{}: `{key}` is not a valid dependency name", where_()));
            }
            if value == "false" {
                return Err(format!(
                    "{}: `{key} = false` means nothing: remove the line (a dependency you do not use needs no entry)",
                    where_()
                ));
            }
            let spec = parse_dependency_spec(value).map_err(|e| format!("{}: {e}", where_()))?;
            if spec.optional && dev {
                return Err(format!(
                    "{}: `{key}` is a dev-dependency, and a dev-dependency cannot be optional",
                    where_()
                ));
            }
            if let Some(feature) = spec.features.iter().find(|feature| !features::is_feature_name(feature)) {
                return Err(format!("{}: `{feature}` is not a feature name, in the features of `{key}`", where_()));
            }
            if let Some((_, first)) = declared.iter().find(|(name, _)| name == key) {
                return Err(format!("{}: dependency `{key}` is declared twice (first on line {first})", where_()));
            }
            declared.push((key.to_string(), index + 1));
            let dependency = Dependency {
                name: key.to_string(),
                source: spec.source,
                optional: spec.optional,
                default_features: spec.default_features,
                features: spec.features,
                target,
            };
            if dev {
                dev_deps.push(dependency);
            } else {
                deps.push(dependency);
            }
            continue;
        }
        match section.as_str() {
            "package" => {
                if key == "name" {
                    name = Some(parse_string(value).map_err(|e| format!("{}: {e}", where_()))?);
                } else if key == "version" {
                    version = Some(parse_string(value).map_err(|e| format!("{}: {e}", where_()))?);
                } else if key == "edition" {
                    let text = parse_string(value).map_err(|e| format!("{}: {e}", where_()))?;
                    edition = burdin_sema::Edition::parse(&text).ok_or_else(|| {
                        let known: Vec<&str> = burdin_sema::Edition::ALL.iter().map(|e| e.as_str()).collect();
                        format!("{}: unknown edition `{text}` (this compiler knows {})", where_(), known.join(" and "))
                    })?;
                } else if key == "include" || key == "exclude" {
                    let patterns = parse_string_array(value).map_err(|e| format!("{}: `{key}`: {e}", where_()))?;
                    if let Some(bad) = patterns.iter().find(|p| p.is_empty() || p.starts_with('/')) {
                        return Err(format!(
                            "{}: `{key}` pattern `{bad}` must be a path relative to the package",
                            where_()
                        ));
                    }
                    if key == "include" {
                        include = Some(patterns);
                    } else {
                        exclude = patterns;
                    }
                }
            }
            "features" => {
                if !features::is_feature_name(key) {
                    return Err(format!(
                        "{}: `{key}` is not a feature name: a feature is an identifier that is not a keyword",
                        where_()
                    ));
                }
                if features.iter().any(|feature| feature.name == key) {
                    return Err(format!("{}: feature `{key}` is declared twice", where_()));
                }
                let items = parse_string_array(value).map_err(|e| format!("{}: {e}", where_()))?;
                features.push(Feature { name: key.to_string(), items, line: index + 1 });
            }
            "lint" => {
                let level = parse_string(value).map_err(|e| format!("{}: {e}", where_()))?;
                lint.push((key.to_string(), level, index + 1));
            }
            "rust" if key == "toolchain" => {
                rust.toolchain = Some(parse_string(value).map_err(|e| format!("{}: {e}", where_()))?);
            }
            "rust.crates" => {
                if !is_ident(key) {
                    return Err(format!("{}: `{key}` is not a valid module name for a Rust crate", where_()));
                }
                let path = match parse_dependency(value).map_err(|e| format!("{}: {e}", where_()))? {
                    DepSource::Path(path) => path,
                    DepSource::Git { .. } | DepSource::Official => {
                        return Err(format!("{}: a Rust crate must be a `path`", where_()));
                    }
                };
                rust.crates.push((key.to_string(), path));
            }
            "build" if key == "allow" => {
                let names = parse_string_array(value).map_err(|e| format!("{}: {e}", where_()))?;
                for allowed in names {
                    if !is_ident(&allowed) {
                        return Err(format!(
                            "{}: `{allowed}` is not a valid package name in `[build] allow`",
                            where_()
                        ));
                    }
                    if !build_allow.contains(&allowed) {
                        build_allow.push(allowed);
                    }
                }
            }
            // `[profile.<name>]` and `[profile.<name>.package.<package>]` (`spec/profiles.md`). The entries are
            // collected verbatim; the driver validates only the selected profile of the entry package.
            section if section.starts_with("profile.") => profiles.push(ProfileEntry {
                section: section.to_string(),
                key: key.to_string(),
                value: value.to_string(),
                line: index + 1,
            }),
            _ => {}
        }
    }
    let Some(name) = name else {
        return Err(format!("{}: the `[package]` section needs a `name`", path.display()));
    };
    if !is_ident(&name) {
        return Err(format!("{}: `{name}` is not a valid package name", path.display()));
    }
    let manifest = Manifest {
        name,
        version,
        deps,
        dev_deps,
        features,
        features_section,
        build_allow,
        lint,
        rust,
        edition,
        profiles,
        include,
        exclude,
    };
    check_features(&manifest, path)?;
    Ok(manifest)
}

/// The predicate and kind of a dependency table header: `Some(Ok((predicate, dev)))` for `[dependencies]`,
/// `[dev-dependencies]`, and the `[target.'cfg(...)']` tables of either; `None` for every other section.
fn dependency_table(section: &str) -> Option<Result<(Option<Cfg>, bool), String>> {
    match section {
        "dependencies" => Some(Ok((None, false))),
        "dev-dependencies" => Some(Ok((None, true))),
        _ if section.starts_with("target.") => Some(target_table(section)),
        _ => None,
    }
}

/// `target.'cfg(...)'.dependencies` or `target.'cfg(...)'.dev-dependencies`, with either quote mark (PKG27). The
/// result is the predicate and whether the table holds dev-dependencies.
fn target_table(section: &str) -> Result<(Option<Cfg>, bool), String> {
    let usage = format!(
        "a target table is `[target.'cfg(...)'.dependencies]` or `[target.'cfg(...)'.dev-dependencies]`, found `[{section}]`"
    );
    let rest = section.strip_prefix("target.").unwrap_or(section);
    let Some(quote) = rest.chars().next().filter(|c| *c == '\'' || *c == '"') else {
        return Err(usage);
    };
    let body = &rest[1..];
    let Some(end) = body.find(quote) else {
        return Err(format!("an unterminated predicate in `[{section}]`"));
    };
    let predicate = cfg::parse_cfg(&body[..end])?;
    match &body[end + 1..] {
        ".dependencies" => Ok((Some(predicate), false)),
        ".dev-dependencies" => Ok((Some(predicate), true)),
        _ => Err(usage),
    }
}

/// The checks of the `[features]` table that need the whole manifest (PKG28): an item names a feature or an optional
/// dependency, `dep:x` names an optional one, `x/feature` names a dependency that is not a dev-dependency, and a
/// feature does not share a name with an optional dependency.
fn check_features(manifest: &Manifest, path: &Path) -> Result<(), String> {
    let optional = |name: &str| manifest.optional_deps().any(|dep| dep == name);
    for feature in &manifest.features {
        let at = |message: String| format!("{}:{}: {message}", path.display(), feature.line);
        if optional(&feature.name) {
            return Err(at(format!(
                "feature `{}` has the name of an optional dependency, and the two share one constant: rename one",
                feature.name
            )));
        }
        for item in &feature.items {
            let problem = match features::parse_item(item) {
                features::Item::Name(name) => {
                    if manifest.feature(name).is_none() && !optional(name) {
                        Some(format!(
                            "feature `{}` enables `{item}`, which is not a feature or an optional dependency",
                            feature.name
                        ))
                    } else {
                        None
                    }
                }
                features::Item::Dep(dep) => {
                    if optional(dep) {
                        None
                    } else {
                        Some(format!(
                            "feature `{}` enables `{item}`, but `{dep}` is not an optional dependency",
                            feature.name
                        ))
                    }
                }
                features::Item::Forward { dep, feature: name } => {
                    if manifest.dev_deps.iter().any(|d| d.name == dep) {
                        Some(format!(
                            "feature `{}` enables `{item}`, but `{dep}` is a dev-dependency, which no feature can enable",
                            feature.name
                        ))
                    } else if !manifest.deps.iter().any(|d| d.name == dep) {
                        Some(format!("feature `{}` enables `{item}`, but `{dep}` is not a dependency", feature.name))
                    } else if !features::is_feature_name(name) {
                        Some(format!("`{name}` in `{item}` is not a feature name"))
                    } else {
                        None
                    }
                }
            };
            if let Some(message) = problem {
                return Err(at(message));
            }
        }
    }
    Ok(())
}

/// A `true` or `false` value of a dependency table.
fn parse_bool(value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("expected `true` or `false`, found `{value}`")),
    }
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

/// What a dependency value says: its source and the keys that shape the dependency (PKG25).
pub(crate) struct DependencySpec {
    pub(crate) source: DepSource,
    pub(crate) optional: bool,
    pub(crate) default_features: bool,
    pub(crate) features: Vec<String>,
}

/// `{ path = "../lib" }` (or a bare `"../lib"` as a shorthand), `{ git = "<url>" }` with at most one of `branch`,
/// `tag`, and `rev`, or `true` for the official package of that name (`spec/packages.md`, PKG1-PKG2, PKG15), with the
/// keys of PKG25: `optional`, `default-features`, and `features`. Other keys are ignored, so a newer manifest loads.
fn parse_dependency_spec(value: &str) -> Result<DependencySpec, String> {
    let plain =
        |source: DepSource| DependencySpec { source, optional: false, default_features: true, features: Vec::new() };
    if value.starts_with('"') {
        return parse_string(value).map(|path| plain(DepSource::Path(path)));
    }
    if value == "true" {
        return Ok(plain(DepSource::Official));
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
    let mut optional = false;
    let mut default_features = true;
    let mut features = Vec::new();
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
            "optional" => optional = parse_bool(value)?,
            "default-features" => default_features = parse_bool(value)?,
            "features" => features = parse_string_array(value)?,
            _ => {}
        }
    }
    if references.len() > 1 {
        return Err("a git dependency takes at most one of `branch`, `tag`, and `rev`".to_string());
    }
    let source = match (path, git) {
        (Some(_), Some(_)) => return Err("a dependency takes a `path` or a `git` repository, not both".to_string()),
        (Some(_), None) if !references.is_empty() => {
            return Err("`branch`, `tag`, and `rev` apply only to a `git` dependency".to_string());
        }
        (Some(path), None) => DepSource::Path(path),
        (None, Some(url)) => {
            if url.trim().is_empty() {
                return Err("a `git` dependency needs a repository URL".to_string());
            }
            DepSource::Git { url, reference: references.pop().unwrap_or(GitRef::DefaultBranch) }
        }
        (None, None) if !references.is_empty() => {
            return Err("`branch`, `tag`, and `rev` apply only to a `git` dependency".to_string());
        }
        (None, None) if has_version => {
            return Err("a dependency needs a `path` or a `git` repository: there is no package registry yet, so a \
                        `version` alone cannot be resolved"
                .to_string());
        }
        (None, None) => return Err("a dependency needs a `path` or a `git` repository".to_string()),
    };
    Ok(DependencySpec { source, optional, default_features, features })
}

/// The source of a dependency that takes only a path, as a script's inline manifest and `[rust.crates]` do (PKG2): the
/// keys of PKG25 belong to package dependencies, so they are errors here.
fn parse_dependency(value: &str) -> Result<DepSource, String> {
    let spec = parse_dependency_spec(value)?;
    if spec.optional || !spec.default_features || !spec.features.is_empty() {
        return Err(
            "`optional`, `default-features`, and `features` apply to a package's dependencies, not to a script's \
                    or a Rust crate's"
                .to_string(),
        );
    }
    Ok(spec.source)
}

/// Splits the inside of an inline table or an array on the commas that are not inside a string or a nested array or
/// table, so `features = ["a", "b"]` is one entry.
fn split_table(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_string = false;
    let mut escaped = false;
    let mut depth = 0usize;
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
        } else if c == '[' || c == '{' {
            depth += 1;
        } else if c == ']' || c == '}' {
            depth = depth.saturating_sub(1);
        } else if c == ',' && depth == 0 {
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
        let Err(error) = load_impl(&dir, true, false, &FeatureRequest::default()) else {
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

    #[test]
    fn dependency_forms() {
        let m =
            manifest("[package]\nname = \"app\"\n[dependencies]\nlib = { path = \"../lib\" }\nother = \"../other\"\n")
                .unwrap();
        assert_eq!(
            sources(&m.deps),
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
            sources(&m.deps),
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
            sources(&m.deps),
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

    /// The `(name, source)` pairs of dependencies, for comparing a manifest's sources.
    fn sources(deps: &[Dependency]) -> Vec<(String, DepSource)> {
        deps.iter().map(|dep| (dep.name.clone(), dep.source.clone())).collect()
    }

    /// Writes a package: its `burdin.toml` and one module per `(path under src, text)`.
    fn write_package(root: &Path, manifest: &str, modules: &[(&str, &str)]) {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("burdin.toml"), manifest).unwrap();
        for (file, text) in modules {
            fs::write(root.join("src").join(file), text).unwrap();
        }
    }

    #[test]
    fn dependency_tables_are_parsed() {
        let text = "[package]
name = \"app\"
[dependencies]
lib = { path = \"../lib\", features = [\"a\", \"b\"], default-features = false }
ui = { path = \"../ui\", optional = true }
[dev-dependencies]
testkit = \"../testkit\"
[target.'cfg(windows)'.dependencies]
winapi = { path = \"../winapi\" }
[target.\"cfg(unix)\".dev-dependencies]
bench = { path = \"../bench\", optional = false }
";
        let m = manifest(text).unwrap();
        let names: Vec<&str> = m.deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["lib", "ui", "winapi"]);
        let lib = &m.deps[0];
        assert!(!lib.optional && !lib.default_features);
        assert_eq!(lib.features, ["a", "b"]);
        assert!(m.deps[1].optional && m.deps[1].default_features && m.deps[1].target.is_none());
        assert_eq!(m.deps[2].target, Some(Cfg::Windows));
        let dev: Vec<&str> = m.dev_deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(dev, ["testkit", "bench"]);
        assert!(m.dev_deps[0].target.is_none());
        assert_eq!(m.dev_deps[1].target, Some(Cfg::Unix));
        assert_eq!(dependency_names_of_manifest(&m), ["lib", "ui", "winapi", "testkit", "bench"]);

        let dev_optional =
            manifest("[package]\nname = \"app\"\n[dev-dependencies]\nt = { path = \"../t\", optional = true }\n");
        assert!(dev_optional.unwrap_err().contains("cannot be optional"));
        let twice = manifest(
            "[package]\nname = \"app\"\n[dependencies]\nlib = { path = \"../a\" }\n[dev-dependencies]\nlib = { path = \"../b\" }\n",
        );
        assert!(twice.unwrap_err().contains("declared twice (first on line 4)"));
        let header = manifest("[package]\nname = \"app\"\n[target.x86_64.dependencies]\n");
        assert!(header.unwrap_err().contains("a target table is"));
        let predicate = manifest("[package]\nname = \"app\"\n[target.'cfg(wasm)'.dependencies]\n");
        assert!(predicate.unwrap_err().contains("unknown predicate `wasm`"));
        let flag = manifest(
            "[package]\nname = \"app\"\n[dependencies]\nlib = { path = \"../a\", default-features = maybe }\n",
        );
        assert!(flag.unwrap_err().contains("expected `true` or `false`"));
        let feature = manifest(
            "[package]\nname = \"app\"\n[dependencies]\nlib = { path = \"../a\", features = [\"not a name\"] }\n",
        );
        assert!(feature.unwrap_err().contains("is not a feature name"));
    }

    /// The names of every dependency a manifest declares, normal then dev.
    fn dependency_names_of_manifest(m: &Manifest) -> Vec<&str> {
        m.deps.iter().chain(&m.dev_deps).map(|d| d.name.as_str()).collect()
    }

    #[test]
    fn feature_tables_are_parsed_and_checked() {
        let text = "[package]
name = \"app\"
[dependencies]
json = { path = \"../json\", optional = true }
lib = { path = \"../lib\" }
[features]
default = [\"fast\"]
fast = [\"dep:json\", \"lib/extra\", \"helper\"]
helper = []
";
        let m = manifest(text).unwrap();
        assert!(m.features_section);
        let names: Vec<&str> = m.features.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["default", "fast", "helper"]);
        assert_eq!(m.features[1].items, ["dep:json", "lib/extra", "helper"]);
        assert_eq!(m.features[1].line, 8);
        assert!(manifest("[package]\nname = \"app\"\n[features]\n").unwrap().features_section);

        let item = |items: &str| {
            manifest(&format!(
                "[package]\nname = \"app\"\n[dependencies]\nlib = {{ path = \"../lib\" }}\nopt = {{ path = \"../o\", optional = true }}\n[dev-dependencies]\ntest = \"../t\"\n[features]\nf = [{items}]\n"
            ))
        };
        assert!(item("\"nope\"").unwrap_err().contains("which is not a feature or an optional dependency"));
        assert!(item("\"dep:lib\"").unwrap_err().contains("`lib` is not an optional dependency"));
        assert!(item("\"ghost/x\"").unwrap_err().contains("`ghost` is not a dependency"));
        assert!(item("\"test/x\"").unwrap_err().contains("is a dev-dependency"));
        assert!(item("\"lib/\"").unwrap_err().contains("is not a feature name"));
        assert!(item("\"opt\"").is_ok(), "an optional dependency is an implicit feature");
        let clash = manifest(
            "[package]\nname = \"app\"\n[dependencies]\nopt = { path = \"../o\", optional = true }\n[features]\nopt = []\n",
        );
        assert!(clash.unwrap_err().contains("has the name of an optional dependency"));
        let keyword = manifest("[package]\nname = \"app\"\n[features]\ntype = []\n");
        assert!(keyword.unwrap_err().contains("is not a feature name: a feature is an identifier"));
        let twice = manifest("[package]\nname = \"app\"\n[features]\na = []\na = []\n");
        assert!(twice.unwrap_err().contains("feature `a` is declared twice"));
    }

    #[test]
    fn target_and_dev_dependencies_are_not_read_by_a_build() {
        let temp = TempDir::new("tables");
        // The dev-dependency and the other platform's dependency name directories that do not exist: a build never
        // reads them, so it succeeds, while a test build reads the dev-dependency and fails.
        let app = temp.0.join("app");
        write_package(
            &app,
            "[package]\nname = \"app\"\n[dev-dependencies]\ntestkit = { path = \"../missing\" }\n\
             [target.'cfg(all(windows, unix))'.dependencies]\nghost = { path = \"../ghost\", features = [\"nope\"] }\n",
            &[("main.bd", "fn main() {}\n")],
        );
        let app = app.canonicalize().unwrap();
        let build = load_selected(&app, EntryMode::Build, &FeatureRequest::default());
        let names: Vec<String> = build.unwrap().packages.iter().map(|p| p.name.clone()).collect();
        assert_eq!(names, ["app", "std"]);
        let test = load_selected(&app, EntryMode::Test, &FeatureRequest::default());
        assert!(test.err().expect("a test build reads the dev-dependency").contains("testkit"));
    }

    #[test]
    fn features_unify_across_the_graph() {
        let temp = TempDir::new("features");
        let root = temp.0.clone();
        write_package(
            &root.join("app"),
            "[package]\nname = \"app\"\n[dependencies]\n\
             lib = { path = \"../lib\", features = [\"extra\"], default-features = false }\n\
             tool = { path = \"../tool\", optional = true }\n[features]\ntooling = [\"tool\"]\n",
            &[("main.bd", "fn main() {}\n")],
        );
        write_package(
            &root.join("lib"),
            "[package]\nname = \"lib\"\n[dependencies]\nhelper = { path = \"../helper\", optional = true }\n\
             [features]\ndefault = [\"plain\"]\nplain = []\nextra = [\"helper\", \"helper/deep\"]\n",
            &[("lib.bd", "pub fn f() {}\n")],
        );
        write_package(
            &root.join("helper"),
            "[package]\nname = \"helper\"\n[features]\ndeep = []\n",
            &[("helper.bd", "pub fn g() {}\n")],
        );
        write_package(&root.join("tool"), "[package]\nname = \"tool\"\n", &[("tool.bd", "pub fn t() {}\n")]);
        let app = root.join("app").canonicalize().unwrap();
        let loaded = |request: &FeatureRequest| {
            let workspace = load_selected(&app, EntryMode::Build, request).unwrap();
            let mut names: Vec<String> = workspace.packages.iter().map(|p| p.name.clone()).collect();
            names.sort();
            (names, workspace)
        };
        // `lib` asks for `extra` and not its default, so `helper` is loaded (an optional dependency `extra` enables)
        // and `tool`, which only `tooling` enables, is not.
        let (names, workspace) = loaded(&FeatureRequest::default());
        assert_eq!(names, ["app", "helper", "lib", "std"]);
        let module = |workspace: &Workspace, package: &str| {
            let index = workspace.packages.iter().position(|p| p.name == package).unwrap();
            let module = workspace.packages[index].modules.iter().find(|m| m.path == ["features"]).unwrap();
            module.text.as_deref().unwrap().to_string()
        };
        let lib_features = module(&workspace, "lib");
        assert!(lib_features.contains("const extra: bool = true\n"), "{lib_features}");
        assert!(lib_features.contains("const plain: bool = false\n"), "{lib_features}");
        assert!(lib_features.contains("const helper: bool = true\n"), "{lib_features}");
        assert!(module(&workspace, "helper").contains("const deep: bool = true\n"), "forwarded feature");
        // The entry package's `tooling` loads `tool`; `--all-features` enables every optional dependency too.
        let tooling = FeatureRequest { items: vec!["tooling".into()], ..Default::default() };
        assert_eq!(loaded(&tooling).0, ["app", "helper", "lib", "std", "tool"]);
        let all = FeatureRequest { all_features: true, ..Default::default() };
        assert_eq!(loaded(&all).0, ["app", "helper", "lib", "std", "tool"]);
        // A feature the entry package does not declare is an error that names it.
        let unknown = FeatureRequest { items: vec!["ghost".into()], ..Default::default() };
        assert!(
            load_selected(&app, EntryMode::Build, &unknown)
                .err()
                .expect("the load must fail")
                .contains("package `app` has no feature `ghost`")
        );
    }

    #[test]
    fn a_dependency_asks_for_a_feature_the_package_lacks() {
        let temp = TempDir::new("missing-feature");
        let root = temp.0.clone();
        write_package(
            &root.join("app"),
            "[package]\nname = \"app\"\n[dependencies]\nlib = { path = \"../lib\", features = [\"nope\"] }\n",
            &[("main.bd", "fn main() {}\n")],
        );
        write_package(&root.join("lib"), "[package]\nname = \"lib\"\n", &[("lib.bd", "pub fn f() {}\n")]);
        let app = root.join("app").canonicalize().unwrap();
        let error =
            load_selected(&app, EntryMode::Build, &FeatureRequest::default()).err().expect("the load must fail");
        assert!(error.contains("package `lib` has no feature `nope`"), "{error}");
    }

    #[test]
    fn the_features_module_is_generated_only_for_packages_with_features() {
        let temp = TempDir::new("features-module");
        let root = temp.0.clone();
        write_package(&root.join("plain"), "[package]\nname = \"plain\"\n", &[("main.bd", "fn main() {}\n")]);
        write_package(
            &root.join("app"),
            "[package]\nname = \"app\"\n[dependencies]\nplain = { path = \"../plain\" }\n[features]\n",
            &[("main.bd", "fn main() {}\n")],
        );
        let app = root.join("app").canonicalize().unwrap();
        let workspace = load_selected(&app, EntryMode::Build, &FeatureRequest::default()).unwrap();
        let has_module = |name: &str| {
            let index = workspace.packages.iter().position(|p| p.name == name).unwrap();
            workspace.packages[index].modules.iter().any(|m| m.path == ["features"])
        };
        assert!(has_module("app"), "an empty [features] table still generates the module");
        assert!(!has_module("plain"), "a package without features or optional dependencies has none");

        // A source module with the generated module's name is an error.
        write_package(
            &root.join("clash"),
            "[package]\nname = \"clash\"\n[features]\nfast = []\n",
            &[("main.bd", "fn main() {}\n"), ("features.bd", "const x: bool = true\n")],
        );
        let clash = root.join("clash").canonicalize().unwrap();
        let error =
            load_selected(&clash, EntryMode::Build, &FeatureRequest::default()).err().expect("the load must fail");
        assert!(error.contains("has a module `features`"), "{error}");
    }

    #[test]
    fn a_one_file_target_takes_no_feature_flags_and_a_cycle_is_an_error() {
        let temp = TempDir::new("one-file");
        let script = temp.0.join("tool.bd");
        fs::write(&script, "fn main() {}\n").unwrap();
        let flags = FeatureRequest { all_features: true, ..Default::default() };
        assert!(
            load_selected(&script, EntryMode::Build, &flags)
                .err()
                .expect("the load must fail")
                .contains("has no features")
        );
        assert!(load_selected(&script, EntryMode::Build, &FeatureRequest::default()).is_ok());

        let root = temp.0.join("cycle");
        write_package(
            &root.join("app"),
            "[package]\nname = \"app\"\n[dependencies]\nlib = { path = \"../lib\" }\n",
            &[("main.bd", "fn main() {}\n")],
        );
        write_package(
            &root.join("lib"),
            "[package]\nname = \"lib\"\n[dependencies]\napp = { path = \"../app\" }\n",
            &[("lib.bd", "pub fn f() {}\n")],
        );
        let app = root.join("app").canonicalize().unwrap();
        let error =
            load_selected(&app, EntryMode::Build, &FeatureRequest::default()).err().expect("the load must fail");
        assert!(error.contains("dependency cycle"), "{error}");
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
