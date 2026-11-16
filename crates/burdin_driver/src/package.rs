//! Packages: `burdin.toml`, every `.bd` file under `src/` as a module, and path dependencies.
//!
//! A single `.bd` file is a package with one module (its path is empty). A directory is a package described by
//! its `burdin.toml`; its entry point is `src/main.bd`, and `[dependencies]` are loaded recursively from their
//! `path`. Module paths are relative to `src/` (`geometry/shapes.bd` is module `geometry.shapes`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

/// Loads the target for `burdin test`, which does not need a `main` function: a library package's tests live in
/// its own modules. The entry module falls back to `src/<package>.bd` or `src/lib.bd`, then the first module.
pub fn load_for_tests(target: &Path) -> Result<Workspace, String> {
    load_impl(target, false)
}

fn load_impl(target: &Path, require_entry: bool) -> Result<Workspace, String> {
    if target.is_dir() {
        let dir = target.canonicalize().map_err(|e| format!("cannot open {}: {e}", target.display()))?;
        let mut loader =
            Loader { packages: Vec::new(), loaded: HashMap::new(), names: HashMap::new(), loading: Vec::new() };
        let root = loader.load_package(&dir)?;
        let package = &loader.packages[root];
        let main = package.modules.iter().position(|m| m.path == ["main"]);
        let entry_module = match main {
            Some(module) => module,
            None if require_entry => {
                return Err(format!(
                    "package `{}` has no entry point: expected {}",
                    package.name,
                    dir.join("src/main.bd").display()
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
        let package = PackageFiles {
            name: stem.clone(),
            dir: None,
            modules: vec![ModuleFile { path: Vec::new(), file: target.to_path_buf(), text: None }],
            deps: Vec::new(),
            build_allow: Vec::new(),
            root_module: Some(0),
        };
        let mut packages = vec![package];
        attach_std(&mut packages);
        Ok(Workspace { packages, entry_package: 0, entry_module: 0, stem })
    } else {
        Err(format!("cannot open {}: no such file or directory", target.display()))
    }
}

/// The standard library, embedded in the driver so that every program has it without a manifest entry.
/// A project that declares its own `std` package keeps it instead.
fn std_package() -> PackageFiles {
    let sources: [(&str, &'static str); 24] = [
        ("std", include_str!("../../../std/src/std.bd")),
        ("alloc", include_str!("../../../std/src/alloc.bd")),
        ("io", include_str!("../../../std/src/io.bd")),
        ("env", include_str!("../../../std/src/env.bd")),
        ("process", include_str!("../../../std/src/process.bd")),
        ("mem", include_str!("../../../std/src/mem.bd")),
        ("iter", include_str!("../../../std/src/iter.bd")),
        ("fmt", include_str!("../../../std/src/fmt.bd")),
        ("ops", include_str!("../../../std/src/ops.bd")),
        ("target", include_str!("../../../std/src/target.bd")),
        ("cmp", include_str!("../../../std/src/cmp.bd")),
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
        ("int", include_str!("../../../std/src/int.bd")),
        ("list", include_str!("../../../std/src/list.bd")),
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
    PackageFiles { name: "std".to_string(), dir: None, modules, deps: Vec::new(), build_allow: Vec::new(), root_module }
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

struct Loader {
    packages: Vec<PackageFiles>,
    /// Canonical directory of each loaded package, for a diamond dependency to reuse its package.
    loaded: HashMap<PathBuf, usize>,
    /// The directory each package name came from: a name must identify exactly one package, because it is part of
    /// every qualified item name.
    names: HashMap<String, PathBuf>,
    /// The packages being loaded, to report a dependency cycle.
    loading: Vec<PathBuf>,
}

impl Loader {
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
        });
        self.loaded.insert(dir.to_path_buf(), id);
        self.loading.push(dir.to_path_buf());
        let mut deps = Vec::new();
        let mut seen = Vec::new();
        for (name, relative) in &manifest.deps {
            if seen.contains(name) {
                return Err(format!("`{}`: dependency `{name}` is declared twice", manifest_path.display()));
            }
            seen.push(name.clone());
            let dep_dir = dir.join(relative);
            let dep_dir = dep_dir
                .canonicalize()
                .map_err(|e| format!("`{}`: dependency `{name}`: {e}", manifest_path.display()))?;
            let dep_id = self.load_package(&dep_dir)?;
            deps.push((name.clone(), dep_id));
        }
        self.loading.pop();
        self.packages[id].deps = deps;
        Ok(id)
    }
}

struct Manifest {
    name: String,
    /// `(dependency name, path relative to the manifest)`.
    deps: Vec<(String, String)>,
    /// The dependency names whose build scripts the manifest grants (`[build] allow`).
    build_allow: Vec<String>,
}

/// The minimal `burdin.toml` this stage needs: `[package] name = "..."`, `[dependencies]` entries with a
/// `path`, and `[build] allow = [...]` grants. Unknown sections and keys are ignored, so a newer manifest does
/// not break an older compiler.
fn parse_manifest(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_manifest_text(&text, path)
}

fn parse_manifest_text(text: &str, path: &Path) -> Result<Manifest, String> {
    let mut section = String::new();
    let mut name: Option<String> = None;
    let mut deps: Vec<(String, String)> = Vec::new();
    let mut build_allow: Vec<String> = Vec::new();
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
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("{}: expected `key = value`", where_()));
        };
        let (key, value) = (key.trim(), value.trim());
        match section.as_str() {
            "package" => {
                if key == "name" {
                    name = Some(parse_string(value).map_err(|e| format!("{}: {e}", where_()))?);
                }
            }
            "dependencies" => {
                if !is_ident(key) {
                    return Err(format!("{}: `{key}` is not a valid dependency name", where_()));
                }
                let dep_path = parse_dependency(value).map_err(|e| format!("{}: {e}", where_()))?;
                deps.push((key.to_string(), dep_path));
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
            _ => {}
        }
    }
    let Some(name) = name else {
        return Err(format!("{}: the `[package]` section needs a `name`", path.display()));
    };
    if !is_ident(&name) {
        return Err(format!("{}: `{name}` is not a valid package name", path.display()));
    }
    Ok(Manifest { name, deps, build_allow })
}

/// A `["a", "b"]` array of quoted strings, also accepting a single `"a"`.
fn parse_string_array(value: &str) -> Result<Vec<String>, String> {
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
fn parse_string(value: &str) -> Result<String, String> {
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

/// `{ path = "../lib" }`, or a bare `"../lib"` as a shorthand.
fn parse_dependency(value: &str) -> Result<String, String> {
    if value.starts_with('"') {
        return parse_string(value);
    }
    if !(value.starts_with('{') && value.ends_with('}')) {
        return Err("a dependency must be `{ path = \"...\" }`".to_string());
    }
    let inner = &value[1..value.len() - 1];
    for part in split_table(inner) {
        let Some((key, value)) = part.split_once('=') else { continue };
        if key.trim() == "path" {
            return parse_string(value.trim());
        }
    }
    Err("a dependency needs a `path`".to_string())
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
fn strip_comment(line: &str) -> &str {
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
        return Err(format!("package {} has no `src` directory", dir.display()));
    }
    let modules = discover_modules_under(&src)?;
    if modules.is_empty() {
        return Err(format!("package {} has no `.bd` files under `src`", dir.display()));
    }
    Ok(modules)
}

/// Every `.bd` file under one source root, as modules. Used for a package's `src/` and for the generated modules
/// under its build output directory (`spec/build-scripts.md`, BS4).
pub(crate) fn discover_modules_under(src: &Path) -> Result<Vec<ModuleFile>, String> {
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
fn is_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    (first.is_ascii_alphabetic() || first == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

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
            m.deps,
            vec![("lib".to_string(), "../lib".to_string()), ("other".to_string(), "../other".to_string())]
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
        assert!(manifest("[package]\nname = \"app\"\n[dependencies]\nlib = { git = \"x\" }\n").is_err());
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
