#!/usr/bin/env python3
"""Checks `burdin clean`, `burdin package`, `burdin vendor`, and `--offline` (stage 3, issue #350; spec/packages.md).

Needs a built driver (BURDIN selects it, like tests/run_tests.py) and `git` on PATH. The git repository is a local
one reached through a `file://` URL, and BURDIN_HOME points into a temporary directory, so nothing touches the network
or the user's git cache.

What it checks:
  * `burdin clean` removes `target/` and reports what it removed or that there was nothing to remove; `--cache` also
    removes the compiled-script and git caches under BURDIN_HOME but never `vendor/`; a directory that is not a package
    and an unknown option fail (PKG19);
  * `burdin package` writes `target/package/<name>-<version>.tar` with exactly the selected files, sorted, with the
    documented header fields, and the same checksum for the same tree (PKG20-PKG22); `include` and `exclude` select
    files (PKG21); a package without a version, a bad pattern, and a symbolic link fail with their messages (PKG20-21);
  * `burdin vendor` copies a git dependency whose files match the lockfile, lists it, refuses a `vendor/<name>` the
    list does not name without removing anything, and says there is nothing to vendor for a path-only graph (PKG23);
  * `--offline` builds after the git cache and the repository are deleted, from `vendor/`; without the copy, and for a
    dependency the lockfile does not record, it fails with the documented messages and fetches nothing (PKG24).
"""

import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
from pathlib import Path

import harness

problems = []
checks = 0


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        problems.append(message)


burdin = harness.require_burdin()
if harness.which("git") is None:
    print("check_package_tools.py needs `git` on PATH", file=sys.stderr)
    sys.exit(1)


def remove_tree(path: Path) -> None:
    """Removes a tree, including git's read-only object files on Windows."""

    def make_writable_and_retry(function, target, _):
        os.chmod(target, stat.S_IWRITE)
        function(target)

    shutil.rmtree(path, onerror=make_writable_and_retry)


def write_package(directory: Path, name: str, version=None, extra_manifest: str = "") -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    version_line = f'version = "{version}"\n' if version is not None else ""
    harness.write_text(directory / "burdin.toml", f'[package]\nname = "{name}"\n{version_line}{extra_manifest}')
    (directory / "src").mkdir(exist_ok=True)
    harness.write_text(directory / "src" / "main.bd", "fn main() {\n    println(\"hi\")\n}\n")
    return directory


def tar_members(archive: Path):
    with tarfile.open(archive) as tar:
        return [(m.name, m.mode, m.mtime, m.uid, m.gid, m.uname, m.gname) for m in tar.getmembers()]


with harness.temporary_dir("burdin-package-tools-") as work:
    env = dict(os.environ)
    home = work / "home"
    env["BURDIN_HOME"] = str(home)
    env.update(
        GIT_AUTHOR_NAME="burdin",
        GIT_AUTHOR_EMAIL="burdin@example.com",
        GIT_COMMITTER_NAME="burdin",
        GIT_COMMITTER_EMAIL="burdin@example.com",
        GIT_CONFIG_NOSYSTEM="1",
        GIT_CONFIG_GLOBAL=str(work / "gitconfig"),
    )
    harness.write_text(work / "gitconfig", "")

    def burdin_in(cwd, *arguments):
        return harness.run([burdin, *arguments], cwd=cwd, env=env)

    def git(cwd, *arguments):
        result = subprocess.run(
            ["git", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false",
             "-c", "core.autocrlf=false", *arguments],
            cwd=str(cwd), env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
        )
        if result.returncode != 0:
            raise SystemExit(f"git {' '.join(arguments)} failed: {harness.text(result.stderr)}")
        return harness.text(result.stdout).strip()

    def out(result):
        return harness.text(result.stdout).replace("\r", "")

    def err(result):
        return harness.text(result.stderr).replace("\r", "")

    # --- burdin clean -------------------------------------------------------------------------------------------
    clean_app = write_package(work / "clean-app", "cleanapp", "0.1.0")
    (clean_app / "target" / "burdin").mkdir(parents=True)
    harness.write_text(clean_app / "target" / "burdin" / "out", "x")
    cleaned = burdin_in(work, "clean", str(clean_app))
    check(cleaned.returncode == 0, f"`burdin clean` failed: {err(cleaned)[:300]}")
    check(out(cleaned).startswith("Removed ") and out(cleaned).rstrip().endswith("target"), f"clean printed {out(cleaned)!r}")
    check(not (clean_app / "target").exists(), "`burdin clean` left target/ behind")
    check((clean_app / "burdin.toml").is_file() and (clean_app / "src" / "main.bd").is_file(), "clean removed sources")
    again = burdin_in(clean_app, "clean")
    check(again.returncode == 0 and "Nothing to remove:" in out(again) and out(again).rstrip().endswith("does not exist"),
          f"a second clean printed {out(again)!r} (exit {again.returncode})")

    not_package = work / "not-a-package"
    not_package.mkdir()
    refused = burdin_in(not_package, "clean")
    check(refused.returncode == 1 and "is not a package: it has no burdin.toml" in err(refused),
          f"clean of a non-package exited {refused.returncode}: {err(refused)[:200]}")
    check(burdin_in(clean_app, "clean", "--bogus").returncode == 2, "an unknown option to `clean` is not a usage error")
    check("--cache" in out(burdin_in(clean_app, "clean", "--help")), "`burdin clean --help` does not list --cache")

    # `--cache` removes the caches under BURDIN_HOME and nothing else: vendor/ is kept.
    (home / "cache" / "scripts").mkdir(parents=True)
    harness.write_text(home / "cache" / "scripts" / "x", "x")
    (home / "git" / "db").mkdir(parents=True)
    harness.write_text(home / "git" / "db" / "y", "y")
    (clean_app / "vendor").mkdir()
    harness.write_text(clean_app / "vendor" / "keep.txt", "keep")
    with_cache = burdin_in(clean_app, "clean", "--cache")
    check(with_cache.returncode == 0, f"`burdin clean --cache` failed: {err(with_cache)[:300]}")
    check(not (home / "cache").exists() and not (home / "git").exists(), "`clean --cache` left a cache behind")
    check((clean_app / "vendor" / "keep.txt").is_file(), "`clean --cache` removed vendor/")

    # --- burdin package -----------------------------------------------------------------------------------------
    pkg = write_package(work / "pack", "packed", "0.1.0")
    harness.write_text(pkg / "README.md", "# packed\n")
    (pkg / "docs").mkdir()
    harness.write_text(pkg / "docs" / "guide.md", "guide\n")
    (pkg / ".git").mkdir()
    harness.write_text(pkg / ".git" / "HEAD", "ref: refs/heads/main\n")
    (pkg / "target" / "burdin").mkdir(parents=True)
    harness.write_text(pkg / "target" / "burdin" / "junk", "junk")
    (pkg / "vendor" / "dep").mkdir(parents=True)
    harness.write_text(pkg / "vendor" / "dep" / "burdin.toml", "[package]\nname = \"dep\"\n")

    packed = burdin_in(pkg, "package", ".")
    check(packed.returncode == 0, f"`burdin package` failed: {err(packed)[:300]}")
    match = re.fullmatch(
        r"Packaged 4 files into .*target[/\\]package[/\\]packed-0\.1\.0\.tar \(sha256:([0-9a-f]{64})\)\n",
        out(packed),
    )
    check(match is not None, f"`burdin package` printed {out(packed)!r}")
    archive = pkg / "target" / "package" / "packed-0.1.0.tar"
    check(archive.is_file(), "the archive was not written to target/package/packed-0.1.0.tar")
    members = tar_members(archive) if archive.is_file() else []
    names = [member[0] for member in members]
    check(
        names == ["README.md", "burdin.toml", "docs/guide.md", "src/main.bd"],
        f"the archive holds {names}: .git, target, and vendor are left out by default, and the rest is sorted",
    )
    check(
        all(mode == 0o644 and mtime == 0 and uid == 0 and gid == 0 and uname == "" and gname == ""
            for _, mode, mtime, uid, gid, uname, gname in members),
        f"an archive header is not the documented one: {members}",
    )

    # The same tree packs to the same bytes, whatever the modification times and the directory it is in.
    first = match.group(1) if match else ""
    for file in [pkg / "README.md", pkg / "burdin.toml", pkg / "src" / "main.bd"]:
        os.utime(file, (1_900_000_000, 1_900_000_000))
    copy = work / "pack-copy"
    shutil.copytree(pkg, copy, ignore=shutil.ignore_patterns("target", ".git", "vendor"))
    repacked = burdin_in(copy, "package", ".")
    repacked_sum = re.search(r"sha256:([0-9a-f]{64})", out(repacked))
    check(repacked.returncode == 0 and repacked_sum is not None and repacked_sum.group(1) == first,
          f"a copy of the tree with other times packed to a different checksum: {out(repacked)!r}")

    # `include` and `exclude` (PKG21).
    harness.write_text(pkg / "src" / "skip.bd", "fn skip() {}\n")
    harness.write_text(
        pkg / "burdin.toml",
        '[package]\nname = "packed"\nversion = "0.1.0"\ninclude = ["src/**", "README.md"]\nexclude = ["src/skip.bd"]\n',
    )
    selected = burdin_in(pkg, "package", ".")
    check(selected.returncode == 0, f"`burdin package` with include/exclude failed: {err(selected)[:300]}")
    check(
        [m[0] for m in tar_members(archive)] == ["README.md", "burdin.toml", "src/main.bd"],
        f"include and exclude selected {[m[0] for m in tar_members(archive)]}",
    )

    harness.write_text(pkg / "burdin.toml", '[package]\nname = "packed"\nversion = "0.1.0"\ninclude = ["/abs"]\n')
    bad_pattern = burdin_in(pkg, "package", ".")
    check(bad_pattern.returncode == 1 and "must be a path relative to the package" in err(bad_pattern),
          f"an absolute pattern exited {bad_pattern.returncode}: {err(bad_pattern)[:200]}")

    no_version = write_package(work / "no-version", "noversion")
    unversioned = burdin_in(no_version, "package", ".")
    check(unversioned.returncode == 1 and "needs a `version` in [package]" in err(unversioned),
          f"a package without a version exited {unversioned.returncode}: {err(unversioned)[:200]}")

    link_pkg = write_package(work / "link-pkg", "linked", "0.1.0")
    try:
        os.symlink(link_pkg / "src" / "main.bd", link_pkg / "alias.bd")
        linked = burdin_in(link_pkg, "package", ".")
        check(linked.returncode == 1 and "symbolic link" in err(linked),
              f"a symbolic link was packaged: exit {linked.returncode}, {err(linked)[:200]}")
    except (OSError, NotImplementedError):
        print("skipped: this host cannot create symbolic links")

    # --- burdin vendor and --offline ----------------------------------------------------------------------------
    repo = write_package(work / "colors-repo", "colors", "1.0.0")
    harness.write_text(repo / "src" / "colors.bd", "fn shade(): i32 {\n    1\n}\n")
    git(repo, "init", "--quiet")
    git(repo, "add", ".")
    git(repo, "commit", "--quiet", "-m", "one")
    git(repo, "tag", "v1")
    url = repo.resolve().as_uri()

    app = write_package(work / "app", "app", "0.1.0")
    harness.write_text(app / "src" / "main.bd", "use colors.shade\n\nfn main() {\n    println(shade().to_string())\n}\n")
    added = burdin_in(app, "add", "--git", url, "--tag", "v1")
    check(added.returncode == 0, f"`burdin add --git` failed: {err(added)[:300]}")

    vendored = burdin_in(app, "vendor", ".")
    check(vendored.returncode == 0 and re.fullmatch(r"Vendored 1 package into .*vendor\n", out(vendored)) is not None,
          f"`burdin vendor` printed {out(vendored)!r} (exit {vendored.returncode}): {err(vendored)[:300]}")
    copied = app / "vendor" / "colors"
    check((copied / "src" / "colors.bd").is_file() and not (copied / ".git").exists(), "the vendored copy is not the package")
    check(
        re.search(r"^colors$", harness.read_text(app / "vendor" / "burdin-vendor.txt"), re.MULTILINE) is not None,
        "vendor/burdin-vendor.txt does not list colors",
    )

    # A vendor/<name> that burdin did not write is refused, and nothing in it is removed.
    harness.write_text(copied / "mine.txt", "not burdin's")
    (app / "vendor" / "burdin-vendor.txt").unlink()
    refused_vendor = burdin_in(app, "vendor", ".")
    check(refused_vendor.returncode == 1 and "was not written by `burdin vendor`" in err(refused_vendor),
          f"vendor over an unlisted directory exited {refused_vendor.returncode}: {err(refused_vendor)[:200]}")
    check((copied / "mine.txt").is_file(), "the refused vendor run removed a file")
    remove_tree(copied)
    check(burdin_in(app, "vendor", ".").returncode == 0, "vendor did not run once the directory was moved aside")

    # The offline build, with the repository and the git cache gone (PKG24).
    remove_tree(repo)
    remove_tree(home / "git")
    offline = burdin_in(app, "run", ".", "--offline")
    check(offline.returncode == 0 and out(offline) == "1\n",
          f"`burdin run . --offline` after the cache and repository were deleted gave {offline.returncode}: "
          f"{out(offline)!r} {err(offline)[:400]}")
    check(any((home / "git").rglob("colors.bd")), "the offline build did not restore the checkout into the cache")
    checked = burdin_in(app, "check", ".", "--offline", "--locked")
    check(checked.returncode == 0, f"`burdin check . --offline --locked` failed: {err(checked)[:300]}")

    # Without the vendored copy and without the cache, the offline build fetches nothing and says why.
    remove_tree(home / "git")
    remove_tree(app / "vendor")
    no_copy = burdin_in(app, "run", ".", "--offline")
    check(no_copy.returncode == 1 and "--offline fetches nothing" in err(no_copy)
          and "run `burdin vendor` with network access first" in err(no_copy),
          f"an offline build without a copy exited {no_copy.returncode}: {err(no_copy)[:300]}")

    # A git dependency the lockfile does not record cannot be resolved offline.
    (app / "burdin.lock").unlink()
    unpinned = burdin_in(app, "build", ".", "--offline")
    check(unpinned.returncode == 1 and "--offline does not resolve a reference" in err(unpinned),
          f"an offline build of an unpinned dependency exited {unpinned.returncode}: {err(unpinned)[:300]}")

    # A path-only graph has no git package to copy.
    lib = write_package(work / "lib", "lib", "0.1.0")
    plain = write_package(work / "plain", "plain", "0.1.0")
    harness.write_text(plain / "burdin.toml", '[package]\nname = "plain"\nversion = "0.1.0"\n\n[dependencies]\nlib = "../lib"\n')
    nothing = burdin_in(plain, "vendor", ".")
    check(nothing.returncode == 0 and out(nothing) == "No git dependencies to vendor\n", f"vendor of a path-only graph printed {out(nothing)!r}")
    check(not (plain / "vendor").exists(), "vendor of a path-only graph created vendor/")
    check(lib.is_dir(), "lib was removed")

print(f"Checked clean, package, vendor, and --offline ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
