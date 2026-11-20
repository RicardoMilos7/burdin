//! Git dependencies: the cache under the Burdin home, resolving a reference to a commit, and checking a commit out
//! (`spec/packages.md`, PKG3-PKG4).
//!
//! ```text
//! <home>/git/db/<key>/                 a bare clone of the repository, shared by every commit of it
//! <home>/git/checkouts/<key>/<commit>/ the files of one commit, without a `.git` directory
//! ```
//!
//! Every command runs the `git` executable with line-ending conversion off, so a checkout has the committed bytes on
//! every platform and its checksum is the same everywhere.

use burdin_query::lockfile::{GitRef, git_cache_dir, git_cache_key, git_checkout_dir};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Runs `git` with `args` and returns its standard output, trimmed. `index` sets `GIT_INDEX_FILE`.
fn git(args: &[&OsStr], index: Option<&Path>) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .args(["-c", "core.autocrlf=false", "-c", "core.eol=lf", "-c", "core.symlinks=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = command.output().map_err(|e| format!("cannot run `git`, which git dependencies need: {e}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let words: Vec<String> = args.iter().take(4).map(|a| a.to_string_lossy().to_string()).collect();
    Err(format!("`git {}` failed: {}", words.join(" "), stderr.trim()))
}

fn os(text: &str) -> &OsStr {
    OsStr::new(text)
}

/// The bare clone of `url`, and whether it was just cloned (so it is already up to date).
fn ensure_db(url: &str) -> Result<(PathBuf, bool), String> {
    let dbs = git_cache_dir()?.join("db");
    let db = dbs.join(git_cache_key(url));
    if db.is_dir() {
        return Ok((db, false));
    }
    std::fs::create_dir_all(&dbs).map_err(|e| format!("cannot create {}: {e}", dbs.display()))?;
    // Clone beside the final directory and rename, so an interrupted clone never looks like a finished one.
    let tmp = dbs.join(format!(".tmp-{}-{}", git_cache_key(url), std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    git(&[os("clone"), os("--bare"), os("--quiet"), os(url), tmp.as_os_str()], None)
        .map_err(|e| format!("cannot clone {url}: {e}"))?;
    if std::fs::rename(&tmp, &db).is_err() {
        // Another process cloned it first; theirs is as good as ours.
        let _ = std::fs::remove_dir_all(&tmp);
        if !db.is_dir() {
            return Err(format!("cannot move the clone of {url} to {}", db.display()));
        }
    }
    Ok((db, true))
}

/// Brings every branch and tag of the bare clone up to date with the repository.
fn fetch(db: &Path, url: &str) -> Result<(), String> {
    git(
        &[
            os("--git-dir"),
            db.as_os_str(),
            os("fetch"),
            os("--quiet"),
            os("--force"),
            os("--tags"),
            os(url),
            os("+refs/heads/*:refs/heads/*"),
        ],
        None,
    )
    .map(|_| ())
    .map_err(|e| format!("cannot fetch {url}: {e}"))
}

/// The commit `spec` names in the bare clone, or `None`.
fn rev_parse(db: &Path, spec: &str) -> Option<String> {
    let spec = format!("{spec}^{{commit}}");
    git(&[os("--git-dir"), db.as_os_str(), os("rev-parse"), os("--verify"), os("--quiet"), os(&spec)], None)
        .ok()
        .filter(|commit| commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Resolves `reference` in the repository at `url` to a full commit id, fetching first so a branch or the default
/// branch is the repository's current one.
pub fn resolve(url: &str, reference: &GitRef) -> Result<String, String> {
    let (db, fresh) = ensure_db(url)?;
    let spec = match reference {
        GitRef::DefaultBranch => "HEAD".to_string(),
        GitRef::Branch(branch) => format!("refs/heads/{branch}"),
        GitRef::Tag(tag) => format!("refs/tags/{tag}"),
        GitRef::Rev(rev) => rev.clone(),
    };
    // A commit the clone already has cannot change, so it needs no fetch.
    if let GitRef::Rev(_) = reference
        && let Some(commit) = rev_parse(&db, &spec)
    {
        return Ok(commit);
    }
    if !fresh {
        fetch(&db, url)?;
    }
    rev_parse(&db, &spec).ok_or_else(|| format!("{url} has no {}", reference.describe()))
}

/// The directory holding the files of `url` at `commit`, checked out now if it is not there yet.
pub fn checkout(url: &str, commit: &str) -> Result<PathBuf, String> {
    let target = git_checkout_dir(url, commit)?;
    if target.is_dir() {
        return Ok(target);
    }
    let (db, fresh) = ensure_db(url)?;
    if rev_parse(&db, commit).is_none() {
        if !fresh {
            fetch(&db, url)?;
        }
        if rev_parse(&db, commit).is_none() {
            return Err(format!("{url} has no commit {commit}"));
        }
    }
    let parent = target.parent().ok_or("a checkout directory has no parent")?.to_path_buf();
    std::fs::create_dir_all(&parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    let tmp = parent.join(format!(".tmp-{commit}-{}", std::process::id()));
    let index = parent.join(format!(".index-{commit}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir(&tmp).map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
    // A private index and the bare clone as the repository: the files land in `tmp` with no `.git` beside them.
    let read = git(
        &[
            os("--git-dir"),
            db.as_os_str(),
            os("--work-tree"),
            tmp.as_os_str(),
            os("read-tree"),
            os("--reset"),
            os("-u"),
            os(commit),
        ],
        Some(&index),
    );
    let _ = std::fs::remove_file(&index);
    if let Err(e) = read {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("cannot check out {url} at {commit}: {e}"));
    }
    if std::fs::rename(&tmp, &target).is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
        if !target.is_dir() {
            return Err(format!("cannot move the checkout of {url} to {}", target.display()));
        }
    }
    Ok(target)
}

/// The directory holding the files of `url` at `commit` in the cache, restored from `vendored` (a copy `burdin
/// vendor` made) when the cache has no checkout of them. Nothing is fetched, so this is the offline counterpart of
/// [`checkout`] (PKG24). The caller checks the restored files against the lockfile's checksum.
pub fn restore(url: &str, commit: &str, vendored: &Path) -> Result<PathBuf, String> {
    let target = git_checkout_dir(url, commit)?;
    if target.is_dir() {
        return Ok(target);
    }
    let parent = target.parent().ok_or("a checkout directory has no parent")?.to_path_buf();
    std::fs::create_dir_all(&parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    let tmp = parent.join(format!(".tmp-restore-{commit}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    if let Err(e) = crate::vendor::copy_tree(vendored, &tmp) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("cannot restore {url} at {commit} from {}: {e}", vendored.display()));
    }
    if std::fs::rename(&tmp, &target).is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
        if !target.is_dir() {
            return Err(format!("cannot move the restored checkout of {url} to {}", target.display()));
        }
    }
    Ok(target)
}
