//! `burdin vendor` and `--offline` against a local repository, end to end without the driver (`spec/packages.md`,
//! PKG23-PKG24): the copy and its list, the refusal to overwrite what burdin did not write, a build that reads the
//! vendored copy after the repository and the git cache are gone, and the failures that name what is missing. One
//! test, because it points `BURDIN_HOME` at its own directory.

use burdin_pkg::add::{Request, Source, add};
use burdin_pkg::resolve::{Mode, checksum, sync_with};
use burdin_pkg::vendor::{LIST_FILE, vendor};
use burdin_query::lockfile::{self, GitRef};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(["-c", "core.autocrlf=false", "-c", "user.name=burdin", "-c", "user.email=burdin@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn package(dir: &Path, name: &str, body: &str) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("burdin.toml"), format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n")).unwrap();
    fs::write(dir.join("src").join(format!("{name}.bd")), body).unwrap();
}

/// A `file://` URL for a local directory, as `git clone` accepts it on every platform.
fn file_url(path: &Path) -> String {
    let text = burdin_query::package::source_path_text(&path.canonicalize().unwrap()).replace('\\', "/");
    if text.starts_with('/') { format!("file://{text}") } else { format!("file:///{text}") }
}

#[test]
fn vendored_git_packages_build_offline_without_the_repository() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("skipped: no git on PATH");
        return;
    }
    let work: PathBuf = std::env::temp_dir().join(format!("burdin-pkg-vendor-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).unwrap();
    // SAFETY: this test binary has one test, so no other thread reads the environment.
    unsafe { std::env::set_var("BURDIN_HOME", work.join("home")) };
    let cache = work.join("home").join("git");

    let repo = work.join("colors");
    package(&repo, "colors", "fn shade(): i32 {\n    1\n}\n");
    git(&repo, &["init", "--quiet"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "one"]);
    git(&repo, &["tag", "v1"]);
    let commit = git(&repo, &["rev-parse", "HEAD"]);
    let url = file_url(&repo);

    let app = work.join("app");
    package(&app, "app", "fn main() {}\n");
    let request = Request {
        manifest_dir: app.clone(),
        name: None,
        source: Source::Git { url: url.clone(), reference: GitRef::Tag("v1".into()) },
    };
    add(&request).unwrap();
    let lock = lockfile::read(&app).unwrap().expect("a lockfile");
    let pinned = lock.package("colors").unwrap().checksum.clone().expect("a checksum");

    // The copy: every git package, in full, listed by name, with the lockfile's checksum (PKG23).
    let vendored = vendor(&app).unwrap();
    assert_eq!(vendored.names, ["colors"]);
    let copy = app.join("vendor").join("colors");
    assert_eq!(fs::read_to_string(copy.join("src").join("colors.bd")).unwrap(), "fn shade(): i32 {\n    1\n}\n");
    assert!(!copy.join(".git").exists(), "the copy has no git metadata");
    assert_eq!(checksum(&copy).unwrap(), pinned);
    let list = fs::read_to_string(app.join("vendor").join(LIST_FILE)).unwrap();
    assert!(list.lines().any(|line| line == "colors"), "{list}");
    // Running it again writes the same set.
    assert_eq!(vendor(&app).unwrap().names, ["colors"]);

    // A `vendor/colors` that the list does not name is burdin's to leave alone: refused, and nothing is removed.
    fs::write(copy.join("mine.txt"), "not burdin's").unwrap();
    fs::remove_file(app.join("vendor").join(LIST_FILE)).unwrap();
    let error = vendor(&app).unwrap_err();
    assert!(error.contains("was not written by `burdin vendor`"), "{error}");
    assert!(copy.join("mine.txt").is_file(), "the refused run removed a file");
    fs::remove_dir_all(&copy).unwrap();
    vendor(&app).unwrap();

    // Offline with the cache and the repository gone: the vendored copy is restored, and the build's graph is the
    // lockfile's (PKG24).
    fs::remove_dir_all(&repo).unwrap();
    fs::remove_dir_all(&cache).unwrap();
    let offline = Mode { locked: false, offline: true };
    sync_with(&app, offline).unwrap();
    let restored = lockfile::git_checkout_dir(&url, &commit).unwrap();
    assert_eq!(
        fs::read_to_string(restored.join("src").join("colors.bd")).unwrap(),
        "fn shade(): i32 {\n    1\n}\n",
        "the restored checkout"
    );
    assert_eq!(checksum(&restored).unwrap(), pinned);

    // Without the cache and without a copy, offline says what is missing and fetches nothing.
    fs::remove_dir_all(&cache).unwrap();
    fs::remove_dir_all(app.join("vendor")).unwrap();
    let error = sync_with(&app, offline).unwrap_err();
    assert!(error.contains("is not in the git cache") && error.contains("--offline fetches nothing"), "{error}");
    assert!(error.contains("run `burdin vendor` with network access first"), "{error}");

    // A git dependency the lockfile does not record cannot be resolved offline.
    fs::remove_file(app.join("burdin.lock")).unwrap();
    let error = sync_with(&app, offline).unwrap_err();
    assert!(error.contains("is not in burdin.lock") && error.contains("--offline does not resolve"), "{error}");

    // A graph of path packages has no git package to copy: vendoring writes no directory.
    let lib = work.join("lib");
    package(&lib, "lib", "fn f() {}\n");
    let plain = work.join("plain");
    package(&plain, "plain", "fn main() {}\n");
    fs::write(
        plain.join("burdin.toml"),
        "[package]\nname = \"plain\"\nversion = \"1.0.0\"\n[dependencies]\nlib = { path = \"../lib\" }\n",
    )
    .unwrap();
    let none = vendor(&plain).unwrap();
    assert!(none.names.is_empty());
    assert!(!plain.join("vendor").exists());
}
