//! Workspaces (`spec/workspaces.md`): a root manifest with a `[workspace]` section, the packages it lists, and the root a
//! package directory belongs to. Membership reads directories and manifests only and never loads a package graph, so a
//! member's manifest can inherit from its root without a cycle (WS2).

use crate::package::{self, Document, WorkspaceSection, parse_document};
use std::path::{Path, PathBuf};

/// The manifest every package and workspace root has.
pub const MANIFEST: &str = "burdin.toml";

/// A member of a workspace: its package's name and its canonical directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub dir: PathBuf,
}

/// A workspace as its root manifest describes it (WS1-WS5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The canonical directory of the root manifest.
    pub root: PathBuf,
    /// The members: the root package first when the root has one, then the listed packages in order.
    pub members: Vec<Member>,
    /// The index in `members` of the root package, when the root has one.
    pub root_package: Option<usize>,
    /// The indices in `members` that a command selects when it selects nothing (WS5), in member order.
    pub default_members: Vec<usize>,
}

impl Layout {
    /// The index in `members` of the member whose directory is `dir` (canonical).
    pub fn member_at(&self, dir: &Path) -> Option<usize> {
        self.members.iter().position(|member| member.dir == dir)
    }
}

/// The workspace root that the package in `dir` belongs to (WS2): `dir` itself when its manifest has a `[workspace]`
/// section, otherwise the nearest ancestor with one that lists `dir` as a member. `None` for a standalone package, and
/// for a directory that does not exist.
pub fn root_of(dir: &Path) -> Result<Option<PathBuf>, String> {
    let Ok(dir) = dir.canonicalize() else {
        return Ok(None);
    };
    if read_document(&dir)?.is_some_and(|doc| doc.workspace.is_some()) {
        return Ok(Some(dir));
    }
    for ancestor in dir.ancestors().skip(1) {
        let Some(doc) = read_document(ancestor)? else {
            continue;
        };
        let Some(section) = doc.workspace else {
            continue;
        };
        let members = member_dirs(ancestor, doc.package_seen, &section)?;
        return Ok(members.contains(&dir).then(|| ancestor.to_path_buf()));
    }
    Ok(None)
}

/// The directory the lockfile of the package in `dir` lives in: the workspace root's, or `dir` itself when the package
/// is standalone (WS14).
pub fn lock_dir(dir: &Path) -> Result<PathBuf, String> {
    Ok(root_of(dir)?.unwrap_or_else(|| dir.to_path_buf()))
}

/// The target directory of the package in `dir` (WS19): the workspace root's `target` for a member, `dir`'s own for a
/// standalone package. A workspace that cannot be read is treated as standalone; the loader reports its error.
pub fn target_dir(dir: &Path) -> PathBuf {
    match root_of(dir) {
        Ok(Some(root)) => root.join("target"),
        _ => dir.join("target"),
    }
}

/// Where the package `name` in `dir` keeps its own build state for `profile` (the build script's output and key, the
/// Rust shims): `<package>/target/burdin/<profile>` for a standalone package, and
/// `<root>/target/burdin/<profile>/packages/<name>` for a member, so members sharing one target directory never
/// overwrite each other's state (WS19).
pub fn package_out_dir(dir: &Path, name: &str, profile: &str) -> PathBuf {
    let out = target_dir(dir).join("burdin").join(profile);
    match root_of(dir) {
        Ok(Some(_)) => out.join("packages").join(name),
        _ => out,
    }
}

/// The layout of the workspace the package in `dir` belongs to, or `None` for a standalone package.
pub fn layout_of(dir: &Path) -> Result<Option<Layout>, String> {
    match root_of(dir)? {
        Some(root) => layout(&root).map(Some),
        None => Ok(None),
    }
}

/// The layout of the workspace whose root manifest is in `root` (WS1-WS5, WS17). Every member is read, so its name and
/// its inheritance are checked, and a broken member is an error here.
pub fn layout(root: &Path) -> Result<Layout, String> {
    let root = root.canonicalize().map_err(|e| format!("cannot open {}: {e}", root.display()))?;
    let manifest = root.join(MANIFEST);
    let Some(doc) = read_document(&root)? else {
        return Err(format!("{}: there is no manifest to read the workspace from", manifest.display()));
    };
    let Some(section) = doc.workspace.clone() else {
        return Err(format!("{}: the manifest has no `[workspace]` section", manifest.display()));
    };
    let dirs = member_dirs(&root, doc.package_seen, &section)?;
    if dirs.is_empty() {
        return Err(format!("{}: the workspace has no members; list packages in `members`", manifest.display()));
    }
    let mut members: Vec<Member> = Vec::new();
    let mut root_package = None;
    for dir in dirs {
        let member_manifest = dir.join(MANIFEST);
        let member_doc = read_document(&dir)?.unwrap_or_default();
        if dir != root {
            if !member_doc.package_seen {
                return Err(format!(
                    "{}: `{}` is listed as a member but has no `[package]` section; a member is a package",
                    manifest.display(),
                    dir.display()
                ));
            }
            if member_doc.workspace.is_some() {
                return Err(format!(
                    "{}: a workspace member cannot have its own `[workspace]` section, so it cannot be a workspace \
                     root (WS11)",
                    member_manifest.display()
                ));
            }
        }
        let parsed = package::parse_manifest(&member_manifest)?;
        if let Some(other) = members.iter().find(|member| member.name == parsed.name) {
            return Err(format!(
                "packages `{}` and `{}` share the name `{}`: a workspace's package names are unique (WS4)",
                other.dir.display(),
                dir.display(),
                parsed.name
            ));
        }
        if dir == root {
            root_package = Some(members.len());
        }
        members.push(Member { name: parsed.name, dir });
    }
    let default_members = if section.default_members.is_empty() {
        match root_package {
            Some(index) => vec![index],
            None => (0..members.len()).collect(),
        }
    } else {
        let mut picked: Vec<usize> = Vec::new();
        for (entry, line) in &section.default_members {
            for dir in expand(&root, entry, *line)? {
                let Some(index) = members.iter().position(|member| member.dir == dir) else {
                    return Err(format!(
                        "{}:{line}: default member `{entry}` is not a member of the workspace",
                        manifest.display()
                    ));
                };
                if !picked.contains(&index) {
                    picked.push(index);
                }
            }
        }
        picked.sort_unstable();
        picked
    };
    Ok(Layout { root, members, root_package, default_members })
}

/// The directories a workspace lists, in order, without reading any member's manifest: the root package first when
/// there is one, then each `members` entry, minus the `exclude` entries (WS2-WS3).
fn member_dirs(root: &Path, has_package: bool, section: &WorkspaceSection) -> Result<Vec<PathBuf>, String> {
    let mut excluded: Vec<PathBuf> = Vec::new();
    for (entry, line) in &section.exclude {
        for dir in expand(root, entry, *line)? {
            if dir == root {
                return Err(format!(
                    "{}:{line}: `exclude` cannot name the workspace root",
                    root.join(MANIFEST).display()
                ));
            }
            excluded.push(dir);
        }
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if has_package {
        dirs.push(root.to_path_buf());
    }
    for (entry, line) in &section.members {
        for dir in expand(root, entry, *line)? {
            if !dirs.contains(&dir) && !excluded.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    Ok(dirs)
}

/// The package directories one `members`, `exclude`, or `default-members` entry names (WS3). An entry without `*` must
/// name a package directory. An entry with `*` matches the package directories whose names fit, and skips the others.
fn expand(root: &Path, entry: &str, line: usize) -> Result<Vec<PathBuf>, String> {
    let at = format!("{}:{line}", root.join(MANIFEST).display());
    let normalized = entry.trim().replace('\\', "/");
    if normalized.is_empty() {
        return Err(format!("{at}: a workspace path is empty"));
    }
    if normalized.starts_with('/') || normalized.contains(':') {
        return Err(format!("{at}: `{entry}` must be a path relative to the workspace root"));
    }
    if normalized.split('/').any(|part| part == "..") {
        return Err(format!("{at}: `{entry}` must not contain `..`: members are inside the workspace"));
    }
    let parts: Vec<&str> = normalized.split('/').filter(|part| !part.is_empty() && *part != ".").collect();
    let wildcard = parts.iter().any(|part| part.contains('*'));
    let mut current = vec![root.to_path_buf()];
    for part in &parts {
        let mut next = Vec::new();
        for base in &current {
            if part.contains('*') {
                for child in subdirectories(base)? {
                    let name = child.file_name().and_then(|name| name.to_str()).unwrap_or_default();
                    if !name.starts_with('.') && glob_matches(part, name) {
                        next.push(child);
                    }
                }
            } else if base.join(part).is_dir() {
                next.push(base.join(part));
            }
        }
        current = next;
    }
    if !wildcard {
        let Some(dir) = current.pop() else {
            return Err(format!("{at}: `{entry}` is not a directory in the workspace"));
        };
        let dir = dir.canonicalize().map_err(|e| format!("{at}: `{entry}`: {e}"))?;
        if !dir.join(MANIFEST).is_file() {
            return Err(format!("{at}: `{entry}` has no {MANIFEST}, so it is not a package"));
        }
        return Ok(vec![dir]);
    }
    let mut dirs = Vec::new();
    for dir in current {
        if dir.join(MANIFEST).is_file() {
            dirs.push(dir.canonicalize().map_err(|e| format!("{at}: {e}"))?);
        }
    }
    Ok(dirs)
}

/// Whether `name` matches `pattern`, where `*` matches any run of characters (WS3).
pub fn glob_matches(pattern: &str, name: &str) -> bool {
    let pieces: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (pieces[0], pieces[pieces.len() - 1]);
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    for piece in &pieces[1..pieces.len() - 1] {
        match rest.find(piece) {
            Some(at) => rest = &rest[at + piece.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// The subdirectories of `dir`, sorted by name.
fn subdirectories(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut out: Vec<PathBuf> =
        entries.filter_map(Result::ok).map(|entry| entry.path()).filter(|p| p.is_dir()).collect();
    out.sort();
    Ok(out)
}

/// The parsed manifest in `dir`, or `None` when the directory has none.
fn read_document(dir: &Path) -> Result<Option<Document>, String> {
    let path = dir.join(MANIFEST);
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_document(&text, &path).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A temporary directory, removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let path = std::env::temp_dir().join(format!(
                "burdin-workspace-{}-{}-{}",
                name,
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
            ));
            fs::create_dir_all(&path).unwrap();
            TempDir(path.canonicalize().unwrap())
        }

        fn write(&self, relative: &str, text: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn package(name: &str) -> String {
        format!("[package]\nname = \"{name}\"\n")
    }

    #[test]
    fn members_share_the_root_target_and_keep_their_own_state_apart() {
        let dir = TempDir::new("target");
        dir.write(
            "burdin.toml",
            "[workspace]
members = [\"apps/*\"]
",
        );
        dir.write("apps/a/burdin.toml", &package("a"));
        dir.write("apps/b/burdin.toml", &package("b"));
        dir.write("alone/burdin.toml", &package("alone"));
        let a = dir.0.join("apps").join("a");
        assert_eq!(target_dir(&a), dir.0.join("target"));
        assert_eq!(target_dir(&dir.0.join("apps").join("b")), dir.0.join("target"));
        assert_eq!(package_out_dir(&a, "a", "debug"), dir.0.join("target/burdin/debug/packages/a"));
        assert_ne!(package_out_dir(&a, "a", "debug"), package_out_dir(&dir.0.join("apps/b"), "b", "debug"));
        // A package outside the workspace keeps its own layout (TG1).
        let alone = dir.0.join("alone");
        assert_eq!(target_dir(&alone), alone.join("target"));
        assert_eq!(package_out_dir(&alone, "alone", "release"), alone.join("target/burdin/release"));
    }

    #[test]
    fn globs_match_one_path_component() {
        assert!(glob_matches("*", "anything"));
        assert!(glob_matches("core-*", "core-math"));
        assert!(glob_matches("*-lib", "net-lib"));
        assert!(glob_matches("a*b*c", "aXbYc"));
        assert!(!glob_matches("core-*", "math"));
        assert!(!glob_matches("a*a", "a"));
        assert!(glob_matches("a*a", "aa"));
        assert!(!glob_matches("a*b", "ba"));
    }

    #[test]
    fn membership_follows_the_nearest_root() {
        let temp = TempDir::new("membership");
        temp.write("burdin.toml", "[workspace]\nmembers = [\"app\", \"libs/*\"]\nexclude = [\"libs/old\"]\n");
        temp.write("app/burdin.toml", &package("app"));
        temp.write("libs/core/burdin.toml", &package("core"));
        temp.write("libs/old/burdin.toml", &package("old"));
        temp.write("outside/burdin.toml", &package("outside"));
        let root = temp.0.clone();
        assert_eq!(root_of(&root.join("app")).unwrap(), Some(root.clone()));
        assert_eq!(root_of(&root.join("libs/core")).unwrap(), Some(root.clone()));
        assert_eq!(root_of(&root).unwrap(), Some(root.clone()));
        // Excluded and unlisted packages are standalone: they keep their own lockfile.
        assert_eq!(root_of(&root.join("libs/old")).unwrap(), None);
        assert_eq!(root_of(&root.join("outside")).unwrap(), None);
        assert_eq!(lock_dir(&root.join("outside")).unwrap(), root.join("outside"));
        assert_eq!(lock_dir(&root.join("libs/core")).unwrap(), root);
        // A nested package with its own root is not a member of the outer one.
        temp.write("app/nested/burdin.toml", "[workspace]\nmembers = [\".\"]\n[package]\nname = \"nested\"\n");
        assert_eq!(root_of(&root.join("app/nested")).unwrap(), Some(root.join("app/nested")));
    }

    #[test]
    fn member_patterns_match_one_component() {
        let temp = TempDir::new("patterns");
        temp.write("burdin.toml", "[workspace]\nmembers = [\"libs/*\"]\n");
        temp.write("libs/core-a/burdin.toml", &package("core_a"));
        temp.write("libs/core-b/burdin.toml", &package("core_b"));
        temp.write("libs/notes/README.md", "not a package\n");
        temp.write("libs/deep/inner/burdin.toml", &package("inner"));
        let layout = layout(&temp.0).unwrap();
        let names: Vec<&str> = layout.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["core_a", "core_b"], "a glob skips directories without a manifest");
    }

    #[test]
    fn explicit_entries_must_be_packages() {
        let temp = TempDir::new("explicit");
        temp.write("burdin.toml", "[workspace]\nmembers = [\"missing\"]\n");
        let error = layout(&temp.0).unwrap_err();
        assert!(error.contains("`missing` is not a directory"), "{error}");
        temp.write("burdin.toml", "[workspace]\nmembers = [\"plain\"]\n");
        temp.write("plain/src/main.bd", "fn main() {}\n");
        let error = layout(&temp.0).unwrap_err();
        assert!(error.contains("`plain` has no burdin.toml"), "{error}");
        temp.write("burdin.toml", "[workspace]\nmembers = [\"../escape\"]\n");
        assert!(layout(&temp.0).unwrap_err().contains("must not contain `..`"));
        temp.write("burdin.toml", "[workspace]\nmembers = [\"/abs\"]\n");
        assert!(layout(&temp.0).unwrap_err().contains("relative to the workspace root"));
    }

    #[test]
    fn members_must_be_packages() {
        let temp = TempDir::new("kinds");
        temp.write("burdin.toml", "[workspace]\nmembers = [\"lib\"]\n");
        temp.write("lib/burdin.toml", "[workspace.package]\nversion = \"1\"\n");
        let error = layout(&temp.0).unwrap_err();
        assert!(error.contains("has no `[package]` section"), "{error}");
        temp.write("lib/burdin.toml", "[workspace]\nmembers = []\n[package]\nname = \"lib\"\n");
        let error = layout(&temp.0).unwrap_err();
        assert!(error.contains("cannot have its own `[workspace]`"), "{error}");
    }

    #[test]
    fn member_names_are_unique() {
        let temp = TempDir::new("names");
        temp.write("burdin.toml", "[workspace]\nmembers = [\"a\", \"b\"]\n");
        temp.write("a/burdin.toml", &package("same"));
        temp.write("b/burdin.toml", &package("same"));
        let error = layout(&temp.0).unwrap_err();
        assert!(error.contains("share the name `same`"), "{error}");
    }

    #[test]
    fn default_members_follow_the_rules() {
        let temp = TempDir::new("defaults");
        temp.write("burdin.toml", "[package]\nname = \"root\"\n[workspace]\nmembers = [\"a\", \"b\"]\n");
        temp.write("a/burdin.toml", &package("a"));
        temp.write("b/burdin.toml", &package("b"));
        let layout_of_root = layout(&temp.0).unwrap();
        assert_eq!(layout_of_root.root_package, Some(0), "the root package is the first member");
        assert_eq!(layout_of_root.default_members, [0], "a root package is the default when nothing names defaults");

        temp.write("burdin.toml", "[workspace]\nmembers = [\"a\", \"b\"]\n");
        assert_eq!(layout(&temp.0).unwrap().default_members, [0, 1], "every member when there is no root package");

        temp.write("burdin.toml", "[workspace]\nmembers = [\"a\", \"b\"]\ndefault-members = [\"b\"]\n");
        assert_eq!(layout(&temp.0).unwrap().default_members, [1]);

        temp.write("burdin.toml", "[workspace]\nmembers = [\"a\"]\ndefault-members = [\"b\"]\n");
        let error = layout(&temp.0).unwrap_err();
        assert!(error.contains("default member `b` is not a member"), "{error}");
    }

    #[test]
    fn exclude_removes_members_and_cannot_name_the_root() {
        let temp = TempDir::new("exclude");
        temp.write("burdin.toml", "[workspace]\nmembers = [\"a\", \"b\"]\nexclude = [\"b\"]\n");
        temp.write("a/burdin.toml", &package("a"));
        temp.write("b/burdin.toml", &package("b"));
        let names: Vec<String> = layout(&temp.0).unwrap().members.into_iter().map(|m| m.name).collect();
        assert_eq!(names, ["a"]);
        temp.write("burdin.toml", "[workspace]\nmembers = [\"a\"]\nexclude = [\".\"]\n");
        assert!(layout(&temp.0).unwrap_err().contains("cannot name the workspace root"));
    }

    #[test]
    fn a_workspace_without_members_is_an_error() {
        let temp = TempDir::new("empty");
        temp.write("burdin.toml", "[workspace]\n");
        assert!(layout(&temp.0).unwrap_err().contains("has no members"));
    }
}
