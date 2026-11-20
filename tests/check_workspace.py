#!/usr/bin/env python3
"""Checks workspaces: members, default members, excludes, the selection flags, and inherited fields (stage 3, issues
#342 and #343; spec/workspaces.md).

Needs a built driver (BURDIN selects it, like tests/run_tests.py) and `git` on PATH for the lockfile check. Every
workspace is written into a temporary directory, the git dependency is a local repository reached through a `file://`
URL, and BURDIN_HOME points into the same directory, so nothing touches the network or the user's git cache.

What it checks:
  * a virtual root with `default-members` and `exclude`: `build` and `run` take the default members, a member's own
    directory selects that member, and `exclude` keeps a package out of the workspace (WS2, WS5, WS9, WS10);
  * the selection flags for build, run, test, doc, and fmt, and each usage error (exit status 2): an unknown name,
    `-p` with `--workspace`, `--exclude` without `--workspace`, `-o` for several packages, `fmt` with paths, and `run`
    of several packages (WS6, WS8-WS13);
  * `build -p` of a package without `src/main.bd` fails, and the same package named by `--workspace` is skipped with
    its note (WS9);
  * `test` runs the selected members in one report, and `doc` writes one site per member under its own `target/doc`
    (WS11, WS12);
  * a package outside any workspace takes `-p` naming itself, and another name is a usage error (WS7);
  * a virtual root is not a package: `check` rejects it, and `check` on a member works (WS1, WS8);
  * inherited fields and dependencies: `version`, `edition`, `license`, `authors`, and `dep.workspace = true` build, and
    an inherited key the root does not set is an error that names `[workspace.package]` (WS15, WS16);
  * `burdin fix --migrate` refuses a member whose edition is inherited (WS15);
  * one lockfile for the whole workspace, at the root: a member's build writes `burdin.lock` beside the root manifest
    and none beside the member, and `--locked` passes until the lockfile is missing (WS14).
"""

import os
import shutil
import subprocess
import sys
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
    print("check_workspace.py needs `git` on PATH", file=sys.stderr)
    sys.exit(1)

EXE_SUFFIXES = ("", ".exe")


def out(result) -> str:
    return harness.text(result.stdout).replace("\r", "")


def err(result) -> str:
    return harness.text(result.stderr).replace("\r", "")


def built(package_dir: Path, stem: str, profile: str = "debug") -> bool:
    """Whether `burdin build` wrote the executable `stem` under the package's own target directory."""
    folder = package_dir / "target" / "burdin" / profile
    return any((folder / (stem + suffix)).is_file() for suffix in EXE_SUFFIXES)


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    harness.write_text(path, text)


MAIN = "fn main() {\n    println(\"NAME\")\n}\n"
KIT_LIB = "fn greeting(): i32 {\n    42\n}\n\ntest \"greeting is 42\" {\n    assert_eq(greeting(), 42)?\n}\n"
CLI_MAIN = "use kit.greeting\n\nfn main() {\n    println(greeting().to_string())\n}\n"
INHERIT = "version.workspace = true\nedition.workspace = true\nlicense.workspace = true\nauthors.workspace = true\n"
ROOT_PACKAGE = "[workspace.package]\nversion = \"0.3.0\"\nedition = \"2026\"\nlicense = \"MIT\"\nauthors = [\"ann\"]\n"


def virtual_workspace(base: Path, remote_url=None) -> Path:
    """A virtual root with two entry packages (`cli`, `tool`), a library (`kit`), and an excluded package (`old`)."""
    dependency = f'shout = {{ git = "{remote_url}", tag = "v1" }}\n' if remote_url else ""
    write(
        base / "burdin.toml",
        "[workspace]\nmembers = [\"apps/*\", \"libs/kit\"]\ndefault-members = [\"apps/cli\"]\n"
        "exclude = [\"apps/old\"]\n\n" + ROOT_PACKAGE + "\n[workspace.dependencies]\n"
        "kit = { path = \"libs/kit\" }\n" + dependency,
    )
    write(
        base / "apps" / "cli" / "burdin.toml",
        "[package]\nname = \"cli\"\n" + INHERIT + "\n[dependencies]\nkit.workspace = true\n"
        + ("shout.workspace = true\n" if remote_url else ""),
    )
    write(base / "apps" / "cli" / "src" / "main.bd", CLI_MAIN)
    write(base / "apps" / "tool" / "burdin.toml", "[package]\nname = \"tool\"\n" + INHERIT)
    write(base / "apps" / "tool" / "src" / "main.bd", MAIN.replace("NAME", "tool"))
    write(base / "apps" / "old" / "burdin.toml", "[package]\nname = \"old\"\n")
    write(base / "apps" / "old" / "src" / "main.bd", MAIN.replace("NAME", "old"))
    write(base / "libs" / "kit" / "burdin.toml", "[package]\nname = \"kit\"\n" + INHERIT)
    write(base / "libs" / "kit" / "src" / "kit.bd", KIT_LIB)
    return base


with harness.temporary_dir("burdin-workspace-") as work:
    env = dict(os.environ)
    env["BURDIN_HOME"] = str(work / "home")
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

    def fixture(name: str, remote_url=None) -> Path:
        return virtual_workspace(work / name, remote_url)

    # --- A virtual root: default members, excludes, and the member's own directory (WS2, WS5, WS9, WS10) ---
    ws = fixture("default")
    built_default = burdin_in(ws, "build", ".")
    check(built_default.returncode == 0, f"`build` of the virtual root failed: {err(built_default)[:400]}")
    check(built(ws / "apps" / "cli", "cli"), "the default build did not write cli under its target directory")
    check(not built(ws / "apps" / "tool", "tool"), "the default build built a package outside the default members")
    check(not (ws / "apps" / "old" / "target").exists(), "an excluded package was built")
    ran = burdin_in(ws, "run", ".")
    check(ran.returncode == 0 and out(ran) == "42\n", f"`run` of the virtual root: {out(ran)!r} {err(ran)[:300]}")

    ws = fixture("member-dir")
    tool = burdin_in(ws / "apps" / "tool", "build", ".")
    check(tool.returncode == 0, f"`build` in a member's directory failed: {err(tool)[:400]}")
    check(built(ws / "apps" / "tool", "tool"), "a build in a member's directory did not build that member")
    check(not (ws / "apps" / "cli" / "target").exists(), "a build in a member's directory built the default members too")

    # --- Selection flags for build (WS6, WS8, WS9) ---
    ws = fixture("build-flags")
    everything = burdin_in(ws, "build", ".", "--workspace")
    check(everything.returncode == 0, f"`build --workspace` failed: {err(everything)[:400]}")
    check("note: skipped kit: it has no src/main.bd" in err(everything), f"no skip note for kit: {err(everything)!r}")
    check(built(ws / "apps" / "cli", "cli") and built(ws / "apps" / "tool", "tool"), "`--workspace` did not build every entry")
    check(not built(ws / "libs" / "kit", "kit"), "a library was built as an executable")

    missing_entry = burdin_in(ws, "build", ".", "-p", "kit")
    check(missing_entry.returncode == 1 and "`kit` has no src/main.bd to build" in err(missing_entry),
          f"`build -p kit` of a library: exit {missing_entry.returncode}, {err(missing_entry)[:300]!r}")

    unknown = burdin_in(ws, "build", ".", "-p", "nope")
    check(unknown.returncode == 2 and "no member named `nope`" in err(unknown) and "`cli`" in err(unknown),
          f"`build -p nope`: exit {unknown.returncode}, {err(unknown)[:300]!r}")
    both = burdin_in(ws, "build", ".", "-p", "cli", "--workspace")
    check(both.returncode == 2 and "cannot be used together" in err(both), f"`-p` with `--workspace`: {err(both)[:300]!r}")
    no_workspace = burdin_in(ws, "build", ".", "--exclude", "cli")
    check(no_workspace.returncode == 2 and "needs `--workspace`" in err(no_workspace),
          f"`--exclude` without `--workspace`: {err(no_workspace)[:300]!r}")
    unknown_exclude = burdin_in(ws, "build", ".", "--workspace", "--exclude", "nope")
    check(unknown_exclude.returncode == 2 and "no member named `nope`" in err(unknown_exclude),
          f"`--exclude nope`: {err(unknown_exclude)[:300]!r}")
    several_one_file = burdin_in(ws, "build", ".", "--workspace", "-o", "one")
    check(several_one_file.returncode == 2 and "name one file" in err(several_one_file),
          f"`-o` with several packages: exit {several_one_file.returncode}, {err(several_one_file)[:300]!r}")
    one = burdin_in(ws, "build", ".", "-p", "cli", "-p", "cli", "-o", "one")
    check(one.returncode == 0 and (ws / "one").is_file(), f"`-o` with one package: {err(one)[:300]!r}")

    # --- run takes exactly one package (WS10) ---
    several_run = burdin_in(ws, "run", ".", "--workspace")
    check(several_run.returncode == 2 and "runs one program" in err(several_run),
          f"`run --workspace`: exit {several_run.returncode}, {err(several_run)[:300]!r}")
    run_library = burdin_in(ws, "run", ".", "-p", "kit")
    check(run_library.returncode == 1 and "has no src/main.bd to run" in err(run_library),
          f"`run -p kit`: exit {run_library.returncode}, {err(run_library)[:300]!r}")
    run_tool = burdin_in(ws, "run", ".", "-p", "tool")
    check(run_tool.returncode == 0 and out(run_tool) == "tool\n", f"`run -p tool`: {out(run_tool)!r}")

    # --- test runs the selected members in one report (WS11) ---
    tested = burdin_in(ws, "test", ".", "--workspace")
    check(tested.returncode == 0 and "1 passed" in out(tested), f"`test --workspace`: {out(tested)[-300:]!r} {err(tested)[:200]!r}")
    tested_one = burdin_in(ws, "test", ".", "-p", "kit")
    check(tested_one.returncode == 0 and "1 passed" in out(tested_one), f"`test -p kit`: {out(tested_one)[-300:]!r}")
    unknown_test = burdin_in(ws, "test", ".", "-p", "nope")
    check(unknown_test.returncode == 2 and "no member named `nope`" in err(unknown_test),
          f"`test -p nope`: exit {unknown_test.returncode}, {err(unknown_test)[:300]!r}")

    # --- doc writes one site per member (WS12) ---
    documented = burdin_in(ws, "doc", ".", "-p", "kit")
    check(documented.returncode == 0 and (ws / "libs" / "kit" / "target" / "doc").is_dir(),
          f"`doc -p kit` did not write the site under the member: {err(documented)[:300]!r}")
    check(not (ws / "apps" / "cli" / "target" / "doc").exists(), "`doc -p kit` documented another member")
    several_doc = burdin_in(ws, "doc", ".", "--workspace", "-o", "site")
    check(several_doc.returncode == 2 and "names one place" in err(several_doc),
          f"`doc --workspace -o`: exit {several_doc.returncode}, {err(several_doc)[:300]!r}")

    # --- fmt selects members from the working directory (WS13) ---
    ws = fixture("fmt")
    write(ws / "libs" / "kit" / "src" / "kit.bd", "fn greeting():i32{42}\n")
    dirty = burdin_in(ws, "fmt", "-p", "kit", "--check")
    check(dirty.returncode == 1 and "libs/kit/src/kit.bd" in out(dirty).replace("\\", "/"),
          f"`fmt -p kit --check` of an unformatted file: exit {dirty.returncode}, {out(dirty)[:300]!r}")
    formatted = burdin_in(ws, "fmt", "--workspace")
    check(formatted.returncode == 0, f"`fmt --workspace` failed: {err(formatted)[:300]}")
    clean = burdin_in(ws, "fmt", "--workspace", "--check")
    check(clean.returncode == 0, f"`fmt --workspace --check` after formatting: exit {clean.returncode}")
    with_paths = burdin_in(ws, "fmt", "-p", "kit", "libs")
    check(with_paths.returncode == 2 and "cannot be combined" in err(with_paths),
          f"`fmt -p kit libs`: exit {with_paths.returncode}, {err(with_paths)[:300]!r}")

    # --- A package outside any workspace (WS7) ---
    solo = work / "solo"
    write(solo / "burdin.toml", "[package]\nname = \"solo\"\nversion = \"0.1.0\"\n")
    write(solo / "src" / "main.bd", MAIN.replace("NAME", "solo"))
    own = burdin_in(solo, "build", ".", "-p", "solo")
    written = any((solo / ("solo" + suffix)).is_file() for suffix in EXE_SUFFIXES)
    check(own.returncode == 0 and written, f"`build -p solo` of a standalone package: {err(own)[:300]!r}")
    other = burdin_in(solo, "build", ".", "-p", "other")
    check(other.returncode == 2 and "the only package is `solo`" in err(other),
          f"`build -p other`: exit {other.returncode}, {err(other)[:300]!r}")
    no_members = burdin_in(solo, "build", ".", "--exclude", "solo")
    check(no_members.returncode == 2 and "needs `--workspace`" in err(no_members),
          f"`--exclude` without `--workspace`: exit {no_members.returncode}, {err(no_members)[:300]!r}")
    exclude_outside = burdin_in(solo, "build", ".", "--workspace", "--exclude", "solo")
    check(exclude_outside.returncode == 2 and "not one" in err(exclude_outside),
          f"`--exclude` outside a workspace: exit {exclude_outside.returncode}, {err(exclude_outside)[:300]!r}")

    # --- A virtual root is not a package (WS1, WS8) ---
    ws = fixture("virtual-check")
    rejected = burdin_in(ws, "check", ".")
    check(rejected.returncode == 1 and "not a package" in err(rejected) and "-p" in err(rejected),
          f"`check` of a virtual root: exit {rejected.returncode}, {err(rejected)[:300]!r}")
    member = burdin_in(ws / "apps" / "cli", "check", ".")
    check(member.returncode == 0, f"`check` of a member: exit {member.returncode}, {err(member)[:300]!r}")

    # --- Inherited fields and dependencies (WS15, WS16) ---
    ws = fixture("inherit-missing")
    write(ws / "burdin.toml", "[workspace]\nmembers = [\"apps/*\", \"libs/kit\"]\n\n[workspace.package]\nversion = \"0.3.0\"\n"
          "edition = \"2026\"\n\n[workspace.dependencies]\nkit = { path = \"libs/kit\" }\n")
    missing_license = burdin_in(ws / "apps" / "cli", "check", ".")
    check(missing_license.returncode == 1 and "[workspace.package] license" in err(missing_license),
          f"an inherited license the root does not set: exit {missing_license.returncode}, {err(missing_license)[:300]!r}")
    migrate = burdin_in(ws / "apps" / "tool", "fix", ".", "--migrate")
    check(migrate.returncode == 1 and "inherited" in err(migrate),
          f"`fix --migrate` of an inherited edition: exit {migrate.returncode}, {err(migrate)[:300]!r}")

    # --- One lockfile for the workspace, at the root (WS14) ---
    remote = work / "remote-shout"
    write(remote / "burdin.toml", "[package]\nname = \"shout\"\nversion = \"0.1.0\"\n")
    write(remote / "src" / "shout.bd", "fn shout(): i32 {\n    1\n}\n")
    git(remote, "init")
    git(remote, "add", ".")
    git(remote, "commit", "-m", "shout")
    git(remote, "tag", "v1")
    url = "file://" + remote.as_posix()
    ws = fixture("lockfile", remote_url=url)
    locked_build = burdin_in(ws, "build", ".", "-p", "cli")
    check(locked_build.returncode == 0, f"a workspace with a git dependency did not build: {err(locked_build)[:400]}")
    check((ws / "burdin.lock").is_file(), "the workspace lockfile is not beside the root manifest")
    check(not (ws / "apps" / "cli" / "burdin.lock").exists(), "a member wrote its own lockfile")
    check(harness.read_text(ws / "burdin.lock").count('name = "shout"') == 1, "the lockfile does not record the git dependency")
    still_locked = burdin_in(ws / "apps" / "cli", "build", ".", "--locked")
    check(still_locked.returncode == 0, f"`build --locked` from a member after a build: {err(still_locked)[:300]!r}")
    (ws / "burdin.lock").unlink()
    no_lock = burdin_in(ws, "build", ".", "-p", "cli", "--locked")
    check(no_lock.returncode == 1 and not (ws / "burdin.lock").exists(),
          f"`--locked` without a lockfile must fail and write nothing: exit {no_lock.returncode}")

print(f"Checked workspaces ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
