//! Dependency resolution and `burdin.lock` (`spec/packages.md`, PKG5-PKG10).
//!
//! `resolve` walks the graph from the entry package, or from every member of a manifest-level workspace, because the
//! lockfile at its root covers them all (`spec/workspaces.md`, WS14): a path dependency is read where it is, and a git dependency is
//! pinned to the commit `burdin.lock` already records for its repository and reference, or resolved now when the
//! lockfile has none. Each git package is checked out once per commit and its files are hashed; a checksum that
//! disagrees with the lockfile is an error, because the files are no longer the ones the lockfile pinned. An
//! official package is read from the compiler's `packages/` folder and hashed too, but its files belong to the
//! compiler, so a different checksum is a difference between the lockfile and the graph, not a corrupt checkout.
//!
//! `sync` runs before a build: it compares the resolved graph with `burdin.lock` and writes the file when they
//! differ, or reports the difference and fails under `--locked`.

use crate::git;
use crate::sha256::Sha256;
use burdin_query::lockfile::{self, GitRef, GitSource, LockedPackage, Lockfile};
use burdin_query::package::{self, DepSource, source_path_text};
use burdin_query::workspace;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// The resolved graph of one entry package.
#[derive(Debug)]
pub struct Resolution {
    /// What `burdin.lock` should say.
    pub lock: Lockfile,
    /// Whether any package came from git or is official: a graph of path packages alone has nothing to pin, so it
    /// needs no lockfile until one exists (PKG6).
    pub has_pinned: bool,
}

/// What `sync` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Synced {
    /// `burdin.lock` already matched the graph.
    UpToDate,
    /// `burdin.lock` was created or rewritten.
    Wrote,
    /// The graph has only path packages and there was no lockfile, so none was written.
    NotNeeded,
}

/// Resolves the graph of the package in `dir` against its lockfile and writes the lockfile when it changed. With
/// `locked`, a lockfile that would change is an error that names each difference, and nothing is written (PKG8).
pub fn sync(dir: &Path, locked: bool) -> Result<Synced, String> {
    // The lockfile is the workspace root's when the package is a member (WS14), and its own otherwise.
    let lock_dir = workspace::lock_dir(dir)?;
    let existing = lockfile::read(&lock_dir)?;
    let resolution = resolve(dir, None, existing.as_ref(), locked)?;
    if existing.as_ref() == Some(&resolution.lock) {
        return Ok(Synced::UpToDate);
    }
    if existing.is_none() && !resolution.has_pinned {
        return Ok(Synced::NotNeeded);
    }
    if locked {
        return Err(stale_message(&differences(existing.as_ref(), &resolution.lock)));
    }
    write(&lock_dir, &resolution.lock)?;
    Ok(Synced::Wrote)
}

/// Writes `burdin.lock` in `dir`.
pub fn write(dir: &Path, lock: &Lockfile) -> Result<(), String> {
    let path = dir.join(lockfile::FILE_NAME);
    std::fs::write(&path, lock.render()).map_err(|e| format!("cannot write {}: {e}", source_path_text(&path)))
}

/// The `--locked` failure: the lockfile and the manifests disagree.
fn stale_message(reasons: &[String]) -> String {
    let mut message = String::from("burdin.lock is out of date with burdin.toml, and --locked forbids updating it:");
    for reason in reasons {
        message.push_str("\n  - ");
        message.push_str(reason);
    }
    message.push_str("\nrun the command without --locked to update burdin.lock");
    message
}

/// Each way the lockfile `old` differs from the resolved `new`, one line each.
pub fn differences(old: Option<&Lockfile>, new: &Lockfile) -> Vec<String> {
    let Some(old) = old else {
        return vec!["there is no burdin.lock".to_string()];
    };
    let mut reasons = Vec::new();
    for package in &new.packages {
        let Some(locked) = old.package(&package.name) else {
            reasons.push(format!("`{}` is a dependency but is not in burdin.lock", package.name));
            continue;
        };
        if locked.version != package.version {
            let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "none".to_string());
            reasons.push(format!(
                "`{}` is version {} but burdin.lock records {}",
                package.name,
                show(&package.version),
                show(&locked.version)
            ));
        }
        if source_of(locked) != source_of(package) {
            reasons.push(format!(
                "`{}` comes from {} but burdin.lock records {}",
                package.name,
                describe_source(package),
                describe_source(locked)
            ));
        } else if let (Some(a), Some(b)) = (&locked.git, &package.git)
            && a.commit != b.commit
        {
            reasons
                .push(format!("`{}` resolves to commit {} but burdin.lock pins {}", package.name, b.commit, a.commit));
        } else if package.official && locked.checksum != package.checksum {
            let show = |checksum: &Option<String>| checksum.clone().unwrap_or_else(|| "none".to_string());
            reasons.push(format!(
                "`{}` is an official package whose files are not the ones burdin.lock pinned: checksum {} but \
                 burdin.lock records {}",
                package.name,
                show(&package.checksum),
                show(&locked.checksum)
            ));
        }
        if locked.dependencies != package.dependencies {
            reasons.push(format!(
                "`{}` depends on [{}] but burdin.lock records [{}]",
                package.name,
                package.dependencies.join(", "),
                locked.dependencies.join(", ")
            ));
        }
    }
    for locked in &old.packages {
        if new.package(&locked.name).is_none() {
            reasons.push(format!("`{}` is in burdin.lock but is no longer a dependency", locked.name));
        }
    }
    reasons
}

/// What decides whether two lockfile entries come from the same place: the git repository and reference, and whether
/// the package is official.
fn source_of(package: &LockedPackage) -> (Option<(&str, &GitRef)>, bool) {
    (package.git.as_ref().map(|git| (git.url.as_str(), &git.reference)), package.official)
}

fn describe_source(package: &LockedPackage) -> String {
    match &package.git {
        Some(git) => format!("{} ({})", git.url, git.reference.describe()),
        None if package.official => "the official packages".to_string(),
        None => "a path".to_string(),
    }
}

/// Where a visited package came from, with the checksum of the files a git or an official package was read from.
#[derive(Clone, Debug)]
enum Origin {
    Path,
    Git(GitSource, String),
    Official(String),
}

impl Origin {
    /// Whether two ways of reaching one directory are the same source.
    fn same_source(&self, other: &Origin) -> bool {
        match (self, other) {
            (Origin::Path, Origin::Path) | (Origin::Official(_), Origin::Official(_)) => true,
            (Origin::Git(a, _), Origin::Git(b, _)) => a.url == b.url && a.reference == b.reference,
            _ => false,
        }
    }

    fn describe(&self) -> String {
        match self {
            Origin::Path => "a path".to_string(),
            Origin::Git(git, _) => format!("{} ({})", git.url, git.reference.describe()),
            Origin::Official(_) => "the official packages".to_string(),
        }
    }
}

/// Resolves the graph of the package in `dir`. `root_manifest` stands in for the text of its `burdin.toml` (what
/// `burdin add` is about to write). `existing` pins git dependencies; with `locked`, a git dependency it does not
/// record is an error instead of a fresh resolution.
pub fn resolve(
    dir: &Path,
    root_manifest: Option<&str>,
    existing: Option<&Lockfile>,
    locked: bool,
) -> Result<Resolution, String> {
    let dir = dir.canonicalize().map_err(|e| format!("cannot open {}: {e}", dir.display()))?;
    // The lockfile of a workspace covers every member, whichever one the command names (WS14), so every member is a
    // root of the graph. `root_manifest` replaces the manifest of `dir`, wherever the graph reaches it.
    let roots: Vec<PathBuf> = match workspace::layout_of(&dir)? {
        Some(layout) => layout.members.into_iter().map(|member| member.dir).collect(),
        None => vec![dir.clone()],
    };
    let mut walker = Walker {
        existing,
        locked,
        names: HashMap::new(),
        dirs: HashMap::new(),
        packages: BTreeMap::new(),
        git: HashMap::new(),
        official: HashMap::new(),
        visiting: Vec::new(),
        edited: root_manifest.map(|text| (dir.clone(), text.to_string())),
    };
    for root in &roots {
        walker.visit(root, None, Origin::Path)?;
    }
    let has_pinned = walker.packages.values().any(|p| p.git.is_some() || p.official);
    Ok(Resolution { lock: Lockfile { packages: walker.packages.into_values().collect() }, has_pinned })
}

struct Walker<'a> {
    existing: Option<&'a Lockfile>,
    locked: bool,
    /// The package name and origin of each visited directory.
    dirs: HashMap<PathBuf, (String, Origin)>,
    /// The directory each package name came from: names are unique in a graph.
    names: HashMap<String, PathBuf>,
    packages: BTreeMap<String, LockedPackage>,
    /// Each repository and reference resolved so far: the commit, its checkout, and the checksum of its files.
    git: HashMap<(String, GitRef), (String, PathBuf, String)>,
    /// Each official package resolved so far: its directory and the checksum of its files.
    official: HashMap<String, (PathBuf, String)>,
    /// The directories being visited, to report a cycle.
    visiting: Vec<PathBuf>,
    /// The directory whose manifest is being edited (`burdin add`) and the text it is about to have.
    edited: Option<(PathBuf, String)>,
}

impl Walker<'_> {
    /// Visits the package in `dir` (canonical) and its dependencies; returns its package name.
    fn visit(&mut self, dir: &Path, manifest_text: Option<&str>, origin: Origin) -> Result<String, String> {
        if self.visiting.iter().any(|d| d == dir) {
            let mut chain: Vec<String> = self.visiting.iter().map(|p| source_path_text(p)).collect();
            chain.push(source_path_text(dir));
            return Err(format!("dependency cycle: {}", chain.join(" -> ")));
        }
        if let Some((name, seen_origin)) = self.dirs.get(dir) {
            if !seen_origin.same_source(&origin) {
                let (a, b) = (seen_origin.describe(), origin.describe());
                return Err(format!(
                    "package `{name}` is required as both {a} and {b}, which are the same files; name it one way"
                ));
            }
            return Ok(name.clone());
        }
        let manifest_path = dir.join("burdin.toml");
        let edited = self.edited.as_ref().filter(|(edited_dir, _)| edited_dir == dir).map(|(_, text)| text.as_str());
        let manifest = match edited.or(manifest_text) {
            Some(text) => package::parse_manifest_text(text, &manifest_path)?,
            None => package::parse_manifest(&manifest_path)?,
        };
        if let Some(previous) = self.names.get(&manifest.name) {
            return Err(format!(
                "package `{}` is provided by both {} and {}; package names must be unique",
                manifest.name,
                source_path_text(previous),
                source_path_text(dir)
            ));
        }
        self.names.insert(manifest.name.clone(), dir.to_path_buf());
        self.dirs.insert(dir.to_path_buf(), (manifest.name.clone(), origin.clone()));
        self.visiting.push(dir.to_path_buf());
        let mut dependencies = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        for (key, source) in &manifest.deps {
            if seen.contains(&key.as_str()) {
                return Err(format!("`{}`: dependency `{key}` is declared twice", source_path_text(&manifest_path)));
            }
            seen.push(key);
            let name = match source {
                DepSource::Path(relative) => {
                    let dep_dir = dir
                        .join(relative)
                        .canonicalize()
                        .map_err(|e| format!("`{}`: dependency `{key}`: {e}", manifest_path.display()))?;
                    self.visit(&dep_dir, None, Origin::Path)?
                }
                DepSource::Git { url, reference } => {
                    let (commit, dep_dir, checksum) = self.git_dependency(key, url, reference)?;
                    let source = GitSource { url: url.clone(), reference: reference.clone(), commit };
                    self.visit(&dep_dir, None, Origin::Git(source, checksum))?
                }
                DepSource::Official => {
                    let (dep_dir, checksum) = self
                        .official_dependency(key)
                        .map_err(|e| format!("`{}`: dependency `{key} = true`: {e}", manifest_path.display()))?;
                    self.visit(&dep_dir, None, Origin::Official(checksum))?
                }
            };
            dependencies.push(name);
        }
        self.visiting.pop();
        dependencies.sort();
        dependencies.dedup();
        let (git, official, checksum) = match origin {
            Origin::Path => (None, false, None),
            Origin::Git(source, checksum) => (Some(source), false, Some(checksum)),
            Origin::Official(checksum) => (None, true, Some(checksum)),
        };
        let name = manifest.name.clone();
        self.packages.insert(
            name.clone(),
            LockedPackage { name: name.clone(), version: manifest.version, git, official, checksum, dependencies },
        );
        Ok(name)
    }

    /// The directory and checksum of the official package `name` (PKG15-PKG17).
    fn official_dependency(&mut self, name: &str) -> Result<(PathBuf, String), String> {
        if let Some(found) = self.official.get(name) {
            return Ok(found.clone());
        }
        let dir = package::official_dir(name)?;
        let dir = dir.canonicalize().map_err(|e| format!("cannot open {}: {e}", source_path_text(&dir)))?;
        let result = (dir.clone(), checksum(&dir)?);
        self.official.insert(name.to_string(), result.clone());
        Ok(result)
    }

    /// The commit, checkout, and checksum of a git dependency: the lockfile's commit when it records this repository
    /// and reference, otherwise a fresh resolution (refused under `--locked`).
    fn git_dependency(
        &mut self,
        key: &str,
        url: &str,
        reference: &GitRef,
    ) -> Result<(String, PathBuf, String), String> {
        let memo = (url.to_string(), reference.clone());
        if let Some(found) = self.git.get(&memo) {
            return Ok(found.clone());
        }
        let pinned = self.existing.and_then(|lock| lock.git_package(url, reference));
        let result = match pinned {
            Some(package) => {
                let commit = package.git.as_ref().map(|g| g.commit.clone()).unwrap_or_default();
                let dir = git::checkout(url, &commit)?;
                let checksum = checksum(&dir)?;
                if let Some(expected) = &package.checksum
                    && *expected != checksum
                {
                    return Err(format!(
                        "the files of git dependency `{key}` ({url} at {commit}) do not match the checksum in \
                         burdin.lock: expected {expected}, found {checksum}; the checkout at {} was changed, so \
                         delete it to fetch it again",
                        source_path_text(&dir)
                    ));
                }
                (commit, dir, checksum)
            }
            None if self.locked => {
                return Err(stale_message(&[format!(
                    "git dependency `{key}` ({url}, {}) is not in burdin.lock",
                    reference.describe()
                )]));
            }
            None => {
                let commit = git::resolve(url, reference)?;
                let dir = git::checkout(url, &commit)?;
                let checksum = checksum(&dir)?;
                (commit, dir, checksum)
            }
        };
        self.git.insert(memo, result.clone());
        Ok(result)
    }
}

/// `sha256:<hex>` of a package's files: every file under `dir` except the top-level `.git` and `target`
/// directories (build output), in byte order of their `/`-separated relative paths, each hashed as its path, a NUL,
/// its length in decimal, a NUL, and its bytes (PKG7).
pub fn checksum(dir: &Path) -> Result<String, String> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_files(dir, "", &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    for (relative, path) in files {
        let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", source_path_text(&path)))?;
        hasher.update(relative.as_bytes());
        hasher.update(&[0]);
        hasher.update(bytes.len().to_string().as_bytes());
        hasher.update(&[0]);
        hasher.update(&bytes);
    }
    Ok(format!("sha256:{}", hasher.finish_hex()))
}

fn collect_files(dir: &Path, prefix: &str, files: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", source_path_text(dir)))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", source_path_text(dir)))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if prefix.is_empty() && (name == ".git" || name == "target") {
            continue;
        }
        let relative = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, &relative, files)?;
        } else {
            files.push((relative, path));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "burdin-pkg-{tag}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn package(&self, name: &str, manifest: &str) -> PathBuf {
            let dir = self.0.join(name);
            fs::create_dir_all(dir.join("src")).unwrap();
            fs::write(dir.join("burdin.toml"), manifest).unwrap();
            let stem = name.rsplit('/').next().unwrap_or(name);
            fs::write(dir.join("src").join(format!("{stem}.bd")), "fn f() {}\n").unwrap();
            dir
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_path_graph_resolves_to_sorted_packages() {
        let s = Scratch::new("path");
        s.package("util", "[package]\nname = \"util\"\nversion = \"0.2.0\"\n");
        s.package("lib", "[package]\nname = \"lib\"\n[dependencies]\nutil = { path = \"../util\" }\n");
        let app = s.package(
            "app",
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[dependencies]\nlib = \"../lib\"\nu = \"../util\"\n",
        );
        let resolution = resolve(&app, None, None, false).unwrap();
        assert!(!resolution.has_pinned);
        let names: Vec<&str> = resolution.lock.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["app", "lib", "util"]);
        assert_eq!(resolution.lock.packages[0].dependencies, ["lib", "util"]);
        assert_eq!(resolution.lock.packages[2].version.as_deref(), Some("0.2.0"));
        // No git package and no lockfile: nothing to write.
        assert_eq!(sync(&app, false).unwrap(), Synced::NotNeeded);
        assert!(!app.join("burdin.lock").exists());
    }

    #[test]
    fn an_existing_lockfile_is_kept_current_and_locked_reports_why() {
        let s = Scratch::new("stale");
        s.package("lib", "[package]\nname = \"lib\"\nversion = \"0.1.0\"\n");
        let app = s.package("app", "[package]\nname = \"app\"\n[dependencies]\nlib = \"../lib\"\n");
        let lock = resolve(&app, None, None, false).unwrap().lock;
        write(&app, &lock).unwrap();
        assert_eq!(sync(&app, true).unwrap(), Synced::UpToDate);
        fs::write(s.0.join("lib").join("burdin.toml"), "[package]\nname = \"lib\"\nversion = \"0.2.0\"\n").unwrap();
        let error = sync(&app, true).unwrap_err();
        assert!(error.contains("out of date"), "{error}");
        assert!(error.contains("`lib` is version 0.2.0 but burdin.lock records 0.1.0"), "{error}");
        assert_eq!(fs::read_to_string(app.join("burdin.lock")).unwrap(), lock.render(), "--locked wrote the file");
        assert_eq!(sync(&app, false).unwrap(), Synced::Wrote);
        assert_eq!(sync(&app, true).unwrap(), Synced::UpToDate);
    }

    #[test]
    fn differences_name_each_change() {
        let package = |name: &str, deps: &[&str]| LockedPackage {
            name: name.into(),
            version: None,
            git: None,
            official: false,
            checksum: None,
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
        };
        let old = Lockfile { packages: vec![package("app", &["gone"]), package("gone", &[])] };
        let new = Lockfile { packages: vec![package("app", &["new"]), package("new", &[])] };
        assert_eq!(
            differences(Some(&old), &new),
            [
                "`app` depends on [new] but burdin.lock records [gone]",
                "`new` is a dependency but is not in burdin.lock",
                "`gone` is in burdin.lock but is no longer a dependency",
            ]
        );
        assert_eq!(differences(None, &new), ["there is no burdin.lock"]);

        // A package that moves from a path to the official packages, and one whose official files changed.
        let official = |name: &str, checksum: &str| LockedPackage {
            official: true,
            checksum: Some(checksum.to_string()),
            ..package(name, &[])
        };
        let old = Lockfile { packages: vec![official("json", "sha256:aa"), package("text", &[])] };
        let new = Lockfile { packages: vec![official("json", "sha256:bb"), official("text", "sha256:cc")] };
        assert_eq!(
            differences(Some(&old), &new),
            [
                "`json` is an official package whose files are not the ones burdin.lock pinned: checksum sha256:bb but \
                 burdin.lock records sha256:aa",
                "`text` comes from the official packages but burdin.lock records a path",
            ]
        );
    }

    #[test]
    fn an_official_package_is_pinned_by_its_files() {
        let s = Scratch::new("official");
        let json = s.package("official/json", "[package]\nname = \"json\"\nversion = \"0.1.0\"\n");
        // An official package may need another one.
        s.package("official/text", "[package]\nname = \"text\"\n");
        fs::write(
            json.join("burdin.toml"),
            "[package]\nname = \"json\"\nversion = \"0.1.0\"\n[dependencies]\ntext = true\n",
        )
        .unwrap();
        let app = s.package("app", "[package]\nname = \"app\"\n[dependencies]\njson = true\n");
        // SAFETY: this is the only test of this crate that sets the variable, and it sets it once.
        unsafe { std::env::set_var(package::PACKAGES_ENV, s.0.join("official")) };

        let resolution = resolve(&app, None, None, false).unwrap();
        assert!(resolution.has_pinned);
        let json_entry = resolution.lock.package("json").unwrap();
        assert!(json_entry.official && json_entry.git.is_none());
        assert_eq!(
            json_entry.checksum,
            Some(checksum(&s.0.join("official").join("json").canonicalize().unwrap()).unwrap())
        );
        assert_eq!(json_entry.dependencies, ["text"]);
        assert!(resolution.lock.package("text").unwrap().official);
        assert!(!resolution.lock.package("app").unwrap().official);

        // No lockfile and a pinned package: one is written, and then it is kept.
        assert!(sync(&app, true).unwrap_err().contains("there is no burdin.lock"));
        assert_eq!(sync(&app, false).unwrap(), Synced::Wrote);
        assert_eq!(sync(&app, true).unwrap(), Synced::UpToDate);

        // A different file in the official package is a difference, and `--locked` names it.
        fs::write(s.0.join("official").join("text").join("src").join("text.bd"), "fn g() {}\n").unwrap();
        let error = sync(&app, true).unwrap_err();
        assert!(
            error.contains("`text` is an official package whose files are not the ones burdin.lock pinned"),
            "{error}"
        );
        assert_eq!(sync(&app, false).unwrap(), Synced::Wrote);
        assert_eq!(sync(&app, true).unwrap(), Synced::UpToDate);

        // An unknown official name says what the official packages are.
        fs::write(app.join("burdin.toml"), "[package]\nname = \"app\"\n[dependencies]\nyaml = true\n").unwrap();
        let error = resolve(&app, None, None, false).unwrap_err();
        assert!(
            error.contains("dependency `yaml = true`") && error.contains("the official packages are: json, text"),
            "{error}"
        );

        // The same directory reached as an official package and as a path is refused.
        let both = "[package]\nname = \"app\"\n[dependencies]\njson = true\nj = \"../official/json\"\n";
        fs::write(app.join("burdin.toml"), both).unwrap();
        assert!(resolve(&app, None, None, false).unwrap_err().contains("the official packages and a path"));
        // SAFETY: as above.
        unsafe { std::env::remove_var(package::PACKAGES_ENV) };
    }

    #[test]
    fn errors_for_cycles_and_duplicate_names() {
        let s = Scratch::new("errors");
        s.package("a", "[package]\nname = \"a\"\n[dependencies]\nb = \"../b\"\n");
        s.package("b", "[package]\nname = \"b\"\n[dependencies]\na = \"../a\"\n");
        assert!(resolve(&s.0.join("a"), None, None, false).unwrap_err().contains("dependency cycle"));
        s.package("x", "[package]\nname = \"same\"\n");
        s.package("y", "[package]\nname = \"same\"\n");
        let app = s.package("app", "[package]\nname = \"app\"\n[dependencies]\nx = \"../x\"\ny = \"../y\"\n");
        assert!(resolve(&app, None, None, false).unwrap_err().contains("must be unique"));
    }

    #[test]
    fn checksums_cover_paths_and_contents_but_not_build_output() {
        let s = Scratch::new("sum");
        let dir = s.package("lib", "[package]\nname = \"lib\"\n");
        let first = checksum(&dir).unwrap();
        assert!(first.starts_with("sha256:") && first.len() == 7 + 64, "{first}");
        fs::create_dir_all(dir.join("target").join("burdin")).unwrap();
        fs::write(dir.join("target").join("burdin").join("out"), "x").unwrap();
        assert_eq!(checksum(&dir).unwrap(), first, "build output changed the checksum");
        fs::write(dir.join("src").join("lib.bd"), "fn g() {}\n").unwrap();
        assert_ne!(checksum(&dir).unwrap(), first);
    }
}
