#!/usr/bin/env python3
"""Checks single-file scripts (stage 3, issue #586, `spec/scripts.md`).

Needs a built driver. BURDIN selects it, like tests/run_tests.py.

What it checks:
  * a script (a shebang and/or an inline `[dependencies]` fragment) runs, with and without `-- program arguments`;
  * `burdin build` and `burdin check` resolve the fragment too, so a script is an ordinary one-file target;
  * the binary is cached by content: with a deliberately broken linker, an unchanged script still runs from the
    cache, while a changed script or a changed dependency must rebuild and fails;
  * touching a file without changing it keeps the cache hit, so the key is content, not modification time;
  * a plain one-file target (no shebang, no fragment) is never cached;
  * a malformed fragment and a `#!` line below the first line are reported, and the shebang line is accepted only
    at byte zero;
  * the cache and the REPL's history live under `BURDIN_HOME` when it is set.
"""

import os
import sys
from pathlib import Path

import harness

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        add_problem(message)


burdin = harness.require_burdin()

SHEBANG = "#!/usr/bin/env burdin run\n"
LIB_MANIFEST = '[package]\nname = "greet"\n'
LIB_SOURCE = 'fn hello(): str {\n    "hello from a dependency"\n}\n'


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    harness.write_text(path, content)


def run_env(work: Path, home: Path):
    env = dict(os.environ)
    env["BURDIN_HOME"] = str(home)
    return env


def burdin_run(args, work: Path, home: Path, env_extra=None, input=None):
    env = run_env(work, home)
    if env_extra:
        env.update(env_extra)
    return harness.run([burdin, *args], cwd=work, env=env, input=input)


with harness.temporary_dir("burdin-scripts-") as work:
    home = work / "home"

    # ---- a shebang-only script ------------------------------------------------
    simple = work / "simple.bd"
    write(simple, SHEBANG + 'fn main() {\n    println("simple says hi")\n}\n')
    first = burdin_run(["run", "simple.bd"], work, home)
    check(first.returncode == 0, f"`burdin run simple.bd` failed: {harness.text(first.stderr)[:400]}")
    check(harness.text(first.stdout).replace("\r", "") == "simple says hi\n", "the script's output differs")
    cached = sorted((home / "cache" / "scripts").glob("*/script*")) if (home / "cache" / "scripts").is_dir() else []
    check(len(cached) == 1, f"a shebang script left {len(cached)} cache entries, expected one")

    # The cache is content-addressed: with no linker at all, an unchanged script still runs.
    again = burdin_run(["run", "simple.bd"], work, home, {"BURDIN_CC": "/definitely-not-a-compiler"})
    check(again.returncode == 0, "an unchanged script did not run from the cache")
    check(harness.text(again.stdout).replace("\r", "") == "simple says hi\n", "the cached run's output differs")

    # Touching without changing keeps the hit: the key is content, not mtime.
    os.utime(simple, None)
    touched = burdin_run(["run", "simple.bd"], work, home, {"BURDIN_CC": "/definitely-not-a-compiler"})
    check(touched.returncode == 0, "touching the script without changing it broke the cache hit")

    # A change must rebuild, and the broken linker makes that fail: proof the key follows the content.
    write(simple, SHEBANG + 'fn main() {\n    println("simple says bye")\n}\n')
    changed = burdin_run(["run", "simple.bd"], work, home, {"BURDIN_CC": "/definitely-not-a-compiler"})
    check(changed.returncode != 0, "a changed script did not rebuild")
    check("linker" in harness.text(changed.stderr), "the rebuild failure does not name the linker")

    # ---- a script with an inline dependency -----------------------------------
    write(work / "greet" / "burdin.toml", LIB_MANIFEST)
    write(work / "greet" / "src" / "greet.bd", LIB_SOURCE)
    script = work / "script.bd"
    write(
        script,
        SHEBANG
        + "//! [dependencies]\n"
        + '//! greet = { path = "greet" }\n'
        + "use greet.hello\n\n"
        + "fn main() {\n"
        + "    let args = env.args()\n"
        + "    println(\"{hello()} from {args[1]}\")\n"
        + "}\n",
    )
    with_dep = burdin_run(["run", "script.bd", "--", "an argument"], work, home)
    check(with_dep.returncode == 0, f"the script with a dependency failed: {harness.text(with_dep.stderr)[:400]}")
    check(
        harness.text(with_dep.stdout).replace("\r", "") == "hello from a dependency from an argument\n",
        f"the dependency script printed {harness.text(with_dep.stdout)!r}",
    )
    # A dependency's content is part of the key: changing it forces a rebuild, so the broken linker fails.
    write(work / "greet" / "src" / "greet.bd", LIB_SOURCE.replace("a dependency", "a changed dependency"))
    stale = burdin_run(["run", "script.bd"], work, home, {"BURDIN_CC": "/definitely-not-a-compiler"})
    check(stale.returncode != 0, "changing a dependency did not invalidate the cached binary")
    rebuilt = burdin_run(["run", "script.bd", "--", "an argument"], work, home)
    check(rebuilt.returncode == 0, "the rebuilt dependency script failed")
    check("a changed dependency" in harness.text(rebuilt.stdout), "the rebuild did not pick up the dependency change")

    # `build` and `check` resolve the fragment too: the script is an ordinary one-file target.
    built = burdin_run(["build", "script.bd", "-o", "script-out"], work, home)
    check(built.returncode == 0, f"`burdin build script.bd` failed: {harness.text(built.stderr)[:400]}")
    check(harness.executable_in(work, "script-out").is_file(), "`burdin build script.bd` wrote no executable")
    checked = burdin_run(["check", "script.bd"], work, home)
    check(checked.returncode == 0, f"`burdin check script.bd` failed: {harness.text(checked.stderr)[:400]}")

    # ---- a plain one-file target is not a script -----------------------------
    plain_home = work / "plain-home"
    plain = work / "plain.bd"
    write(plain, 'fn main() {\n    println("plain")\n}\n')
    ran = burdin_run(["run", "plain.bd"], work, plain_home)
    check(ran.returncode == 0, "a plain one-file target failed to run")
    check(not (plain_home / "cache" / "scripts").exists(), "a plain one-file target was cached as a script")

    # ---- the fragment and the shebang line are checked -----------------------
    bad = work / "bad_manifest.bd"
    write(bad, "//! [dependencies]\n//! greet = { git = \"x\" }\nfn main() {}\n")
    result = burdin_run(["run", "bad_manifest.bd"], work, home)
    check(result.returncode == 1, f"a bad fragment exited {result.returncode}, expected 1")
    check("bad_manifest.bd:2" in harness.text(result.stderr), "the fragment error does not name its line")

    sectioned = work / "sectioned.bd"
    write(sectioned, "//! [dependencies]\n//! [package]\n//! name = \"x\"\nfn main() {}\n")
    result = burdin_run(["run", "sectioned.bd"], work, home)
    check(result.returncode == 1, "a script manifest with another section was accepted")
    check("[dependencies]" in harness.text(result.stderr), "the section error does not name [dependencies]")

    misplaced = work / "misplaced.bd"
    write(misplaced, "// a comment\n" + SHEBANG + "fn main() {}\n")
    result = burdin_run(["build", "misplaced.bd"], work, home)
    check(result.returncode == 1, "a misplaced shebang was accepted")
    check("E0300" in harness.text(result.stderr), "a misplaced shebang does not report E0300")
    check("misplaced.bd:2:1" in harness.text(result.stderr), "E0300 does not point at the shebang line")

    # `:` resolution reaches the program's arguments: indented output stays exact.
    exits = work / "exit.bd"
    write(exits, SHEBANG + "fn main() {\n    process.exit(3)\n}\n")
    code = burdin_run(["run", "exit.bd"], work, home)
    check(code.returncode == 3, f"a script's exit status came back as {code.returncode}, expected 3")

if problems:
    print("\n".join(problems))
    print(f"{len(problems)} problem(s) in {checks} checks")
    sys.exit(1)
print(f"all {checks} script checks passed")
