//! `burdin add --git` and `sync` against a local repository, end to end without the driver: the cache layout, the
//! pinned commit, the checksum, `--locked`, and the loader reading the checkout the lockfile pins
//! (`spec/packages.md`, PKG3-PKG8). One test, because it points `BURDIN_HOME` at its own directory.

use burdin_pkg::add::{Request, Source, add};
use burdin_pkg::resolve::{Synced, sync};
use burdin_query::lockfile::{self, GitRef};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The tests set `BURDIN_HOME`, which is process-wide, so they take turns.
static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
fn a_git_dependency_is_added_pinned_checked_and_loaded() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("skipped: no git on PATH");
        return;
    }
    let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let work: PathBuf = std::env::temp_dir().join(format!("burdin-pkg-git-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).unwrap();
    // SAFETY: the tests take `ENV` in turn, so no other thread reads the variable while it is set.
    unsafe { std::env::set_var("BURDIN_HOME", work.join("home")) };

    let repo = work.join("colors");
    package(&repo, "colors", "fn shade(): i32 {\n    1\n}\n");
    git(&repo, &["init", "--quiet"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "one"]);
    git(&repo, &["tag", "v1"]);
    let v1 = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("src").join("colors.bd"), "fn shade(): i32 {\n    2\n}\n").unwrap();
    git(&repo, &["commit", "--quiet", "-am", "two"]);
    let url = file_url(&repo);

    let app = work.join("app");
    package(&app, "app", "fn main() {}\n");
    fs::rename(app.join("src").join("app.bd"), app.join("src").join("main.bd")).unwrap();
    let request = Request {
        manifest_dir: app.clone(),
        name: None,
        source: Source::Git { url: url.clone(), reference: GitRef::Tag("v1".into()) },
    };
    let added = add(&request).unwrap();
    assert_eq!(added.name, "colors");
    assert_eq!(added.source, format!("git {url}, tag v1, commit {}", &v1[..12]));
    assert!(
        fs::read_to_string(app.join("burdin.toml"))
            .unwrap()
            .contains(&format!("colors = {{ git = \"{url}\", tag = \"v1\" }}"))
    );

    let lock = lockfile::read(&app).unwrap().expect("a lockfile");
    let colors = lock.package("colors").unwrap();
    let source = colors.git.as_ref().unwrap();
    assert_eq!(source.commit, v1);
    assert_eq!(source.reference, GitRef::Tag("v1".into()));
    let checkout = lockfile::git_checkout_dir(&url, &v1).unwrap();
    assert!(checkout.join("burdin.toml").is_file(), "no checkout at {}", checkout.display());
    assert!(!checkout.join(".git").exists());
    assert_eq!(colors.checksum.as_deref(), Some(burdin_pkg::resolve::checksum(&checkout).unwrap().as_str()));
    assert_eq!(fs::read_to_string(checkout.join("src").join("colors.bd")).unwrap(), "fn shade(): i32 {\n    1\n}\n");

    // The lockfile matches, so `--locked` passes, and the loader reads the pinned checkout.
    assert_eq!(sync(&app, true).unwrap(), Synced::UpToDate);
    let workspace = burdin_query::package::load(&app).unwrap();
    let loaded = workspace.packages.iter().find(|p| p.name == "colors").unwrap();
    assert_eq!(loaded.dir.as_deref().map(|d| d.canonicalize().unwrap()), Some(checkout.canonicalize().unwrap()));

    // A changed checkout fails the checksum.
    fs::write(checkout.join("src").join("colors.bd"), "changed\n").unwrap();
    let error = sync(&app, false).unwrap_err();
    assert!(error.contains("do not match the checksum"), "{error}");
    fs::remove_dir_all(&checkout).unwrap();
    assert_eq!(sync(&app, true).unwrap(), Synced::UpToDate, "deleting the checkout fetches it again");

    let _ = fs::remove_dir_all(&work);
}

#[test]
fn a_workspace_locks_every_member() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("skipped: no git on PATH");
        return;
    }
    let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let work: PathBuf = std::env::temp_dir().join(format!("burdin-pkg-workspace-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).unwrap();
    // SAFETY: the tests take `ENV` in turn, so no other thread reads the variable while it is set.
    unsafe { std::env::set_var("BURDIN_HOME", work.join("home")) };

    let repo = work.join("colors");
    package(&repo, "colors", "fn shade(): i32 {\n    1\n}\n");
    git(&repo, &["init", "--quiet"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "one"]);
    let url = file_url(&repo);

    // A virtual root with two members: `app` reaches the git dependency, `lib` depends on `app`.
    let root = work.join("ws");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("burdin.toml"), "[workspace]\nmembers = [\"app\", \"lib\"]\n").unwrap();
    let app = root.join("app");
    package(&app, "app", "fn main() {}\n");
    fs::write(
        app.join("burdin.toml"),
        format!("[package]\nname = \"app\"\nversion = \"1.0.0\"\n\n[dependencies]\ncolors = {{ git = \"{url}\" }}\n"),
    )
    .unwrap();
    let lib = root.join("lib");
    package(&lib, "lib", "fn plus(): i32 {\n    1\n}\n");
    fs::write(
        lib.join("burdin.toml"),
        "[package]\nname = \"lib\"\nversion = \"1.0.0\"\n\n[dependencies]\napp = { path = \"../app\" }\n",
    )
    .unwrap();

    // A command in a member's directory writes one lockfile, at the workspace root, with every member in it.
    sync(&lib, false).unwrap();
    assert!(root.join("burdin.lock").is_file(), "no lockfile at the workspace root");
    assert!(!lib.join("burdin.lock").exists(), "a member wrote its own lockfile");
    let lock = lockfile::read(&root).unwrap().expect("a lockfile");
    for name in ["app", "colors", "lib"] {
        assert!(lock.package(name).is_some(), "burdin.lock has no `{name}`");
    }
    assert_eq!(sync(&root, true).unwrap(), Synced::UpToDate);

    let _ = fs::remove_dir_all(&work);
}
