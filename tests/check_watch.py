#!/usr/bin/env python3
"""Checks `burdin watch` and `--watch` (stage 3, issue #632, `spec/watch.md`).

Needs a built driver. BURDIN selects it, like tests/run_tests.py.

What it checks:
  * `burdin watch check <package>` runs once, then reruns when a source is edited to introduce an error and again
    when it is fixed, printing the error and the success of each run;
  * a file written under `target/` or `.git/`, `.git/`, and `burdin.lock` do not start a run;
  * the screen is cleared before each run unless `--no-clear`;
  * `burdin build --watch` rebuilds without being triggered by its own executable, and a changed file inside a build
    script's declared directory input rebuilds too;
  * `burdin test --watch` reruns the tests of the changed module and its dependents before the whole suite;
  * `burdin watch run` restarts the program after a change and prints its new output, with the feature flags it was
    given (`spec/packages.md`, PKG25);
  * the usage errors: `--no-clear` without `--watch`, a missing target, and an unknown directory.
"""

import os
import subprocess
import sys
import threading
import time
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
CLEAR = "\x1b[2J"
WAITING = "watch: waiting for changes"
TIMEOUT = 60.0


class Watch:
    """A running `burdin watch`: its merged output is collected on a thread."""

    def __init__(self, args, cwd: Path):
        self.chunks = []
        self.lock = threading.Lock()
        self.process = subprocess.Popen(
            [burdin, *args], cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL
        )
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self) -> None:
        while True:
            data = self.process.stdout.read1(4096)
            if not data:
                return
            with self.lock:
                self.chunks.append(data)

    def output(self) -> str:
        with self.lock:
            return harness.text(b"".join(self.chunks)).replace("\r", "")

    def waiting(self) -> int:
        return self.output().count(WAITING)

    def wait_until(self, condition, timeout: float = TIMEOUT) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if condition(self.output()):
                return True
            time.sleep(0.05)
        return False

    def wait_runs(self, runs: int, timeout: float = TIMEOUT) -> bool:
        return self.wait_until(lambda text: text.count(WAITING) >= runs, timeout)

    def stop(self) -> str:
        self.process.kill()
        self.process.wait()
        self.reader.join(timeout=5)
        return self.output()


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    harness.write_text(path, content)
    # Keep a later edit from landing in the same modification-time tick.
    time.sleep(0.25)


def make_package(root: Path, name: str, files: dict) -> Path:
    package = root / name
    write(package / "burdin.toml", f'[package]\nname = "{name}"\nversion = "0.1.0"\n')
    for relative, content in files.items():
        write(package / "src" / relative, content)
    return package


def since(text: str, marker: str, start: int = 0) -> str:
    at = text.find(marker, start)
    return "" if at < 0 else text[at:]


GOOD = 'fn main() {\n    println("one")\n}\n'
BAD = 'fn main() {\n    let n: String = 1\n    println(n)\n}\n'

with harness.temporary_dir("burdin-watch-") as work:
    # --- check: introduce an error, fix it, and ignore files that are not inputs ------------------------------------
    pkg = make_package(work, "app", {"main.bd": GOOD})
    watcher = Watch(["watch", "check", str(pkg)], work)
    try:
        check(watcher.wait_runs(1), "the first run of `watch check` did not finish")
        first = watcher.output()
        check("watch: check finished" in first, f"the clean first run is not reported:\n{first}")
        check(first.startswith(CLEAR), "the screen is not cleared before the first run")

        write(pkg / "src" / "main.bd", BAD)
        check(watcher.wait_runs(2), "the broken edit did not start a run")
        second = since(watcher.output(), "watch: main.bd changed")
        check("watch: main.bd changed, rerunning" in second, f"the change is not reported:\n{second}")
        check("E0003" in second, f"the type error is not printed:\n{second}")
        check("watch: check failed with exit status 1" in second, f"the failed run is not reported:\n{second}")

        write(pkg / "src" / "main.bd", GOOD)
        check(watcher.wait_runs(3), "the fix did not start a run")
        third = since(watcher.output(), "watch: check failed")
        check("watch: check finished" in third, f"the fixed run is not reported:\n{third}")

        # Files the command never reads: the target directory, version-control metadata, and the lock file.
        runs = watcher.waiting()
        write(pkg / "target" / "burdin" / "noise.txt", "x")
        write(pkg / ".git" / "HEAD", "ref: refs/heads/main\n")
        write(pkg / "burdin.lock", "# lock\n")
        time.sleep(1.0)
        check(watcher.waiting() == runs, "a file under target/, .git/, or burdin.lock started a run")

        # A new source file is an input.
        write(pkg / "src" / "extra.bd", "fn extra() {}\n")
        check(watcher.wait_runs(runs + 1), "a new source file did not start a run")
        check(watcher.output().count(CLEAR) >= 4, "the screen is not cleared before each run")
    finally:
        watcher.stop()

    # --- --no-clear and the trailing-flag form ----------------------------------------------------------------------
    quiet = Watch(["check", str(pkg), "--watch", "--no-clear"], work)
    try:
        check(quiet.wait_runs(1), "`burdin check --watch --no-clear` did not finish a run")
        check(CLEAR not in quiet.output(), "`--no-clear` still cleared the screen")
    finally:
        quiet.stop()

    # --- build: the executable the build writes does not retrigger it ------------------------------------------------
    built = make_package(work, "built", {"main.bd": GOOD})
    builder = Watch(["build", ".", "--watch", "--no-clear"], built)
    try:
        check(builder.wait_runs(1), "`burdin build --watch` did not finish the first build")
        check("watch: build finished" in builder.output(), f"the build did not succeed:\n{builder.output()}")
        time.sleep(1.0)
        check(builder.waiting() == 1, "the build's own executable started another run")
        write(built / "src" / "main.bd", 'fn main() {\n    println("two")\n}\n')
        check(builder.wait_runs(2), "a source edit did not rebuild")
        exe = built / ("built" + (".exe" if os.name == "nt" else ""))
        ran = harness.run([str(exe)])
        check(harness.text(ran.stdout).strip() == "two", f"the rebuilt program printed {harness.text(ran.stdout)!r}")
    finally:
        builder.stop()

    # --- build: a build script's declared directory input is watched --------------------------------------------------
    scripted = make_package(work, "scripted", {"main.bd": GOOD})
    # The directory is outside the package, so only the script's declared input can make it watched.
    shared = work / "shared-include"
    write(shared / "value.h", "first\n")
    write(
        scripted / "build.bd",
        'fn main() {\n    println("the script ran")\n    let args = env.args()\n    let out = args[2]\n'
        '    match write_file("{out}/inputs", "../shared-include\\n") {\n        Ok(_) => {}\n'
        '        Err(_) => panic("cannot write the input list"),\n    }\n}\n',
    )
    script_watcher = Watch(["build", ".", "--watch", "--no-clear"], scripted)
    try:
        check(script_watcher.wait_runs(1), "`burdin build --watch` with a build script did not finish the first build")
        check("the script ran" in script_watcher.output(), "the first build did not run the build script")
        runs = script_watcher.waiting()
        write(shared / "value.h", "changed\n")
        check(
            script_watcher.wait_runs(runs + 1),
            "a changed file inside a build script's declared directory input did not rebuild",
        )
    finally:
        script_watcher.stop()

    # --- test: the changed module and its dependents run first -------------------------------------------------------
    testing = make_package(
        work,
        "suite",
        {
            "main.bd": "fn main() {}\n",
            "alpha.bd": 'fn one(): i32 {\n    1\n}\n\ntest "alpha one" {\n    assert_eq(one(), 1)?\n}\n',
            "beta.bd": 'use alpha.one\n\nfn two(): i32 {\n    one() + 1\n}\n\ntest "beta two" {\n    assert_eq(two(), 2)?\n}\n',
            "gamma.bd": 'fn three(): i32 {\n    3\n}\n\ntest "gamma three" {\n    assert_eq(three(), 3)?\n}\n',
        },
    )
    tester = Watch(["watch", "test", str(testing), "--no-clear"], work)
    try:
        check(tester.wait_runs(1), "the first `watch test` run did not finish")
        first_run = tester.output()
        check("test result: 3 passed, 0 failed" in first_run, f"the first run did not run every test:\n{first_run}")
        check("tests of" not in first_run, "the first run was split")
        write(testing / "src" / "alpha.bd", 'fn one(): i32 {\n    1\n}\n\ntest "alpha one" {\n    assert_eq(one(), 1)?\n}\n// edit\n')
        check(tester.wait_runs(2, 120), "the edit did not rerun the tests")
        check(tester.wait_until(lambda t: t.count("watch: test finished") >= 3), "the full suite did not run after the changed modules")
        rerun = since(tester.output(), "watch: alpha.bd changed")
        check("running the tests of alpha., beta. first" in rerun, f"the changed modules are not named:\n{rerun}")
        split = rerun.find("watch: test finished")
        before = rerun[:split]
        after = rerun[split:]
        check("2 passed, 0 failed, 1 filtered out" in before, f"the first phase is not the two modules:\n{before}")
        check("3 passed, 0 failed" in after, f"the second phase did not run the whole suite:\n{after}")
    finally:
        tester.stop()

    # --- run: the program restarts with the new source ---------------------------------------------------------------
    app = make_package(work, "runner", {"main.bd": 'fn main() {\n    println("first")\n}\n'})
    runner = Watch(["watch", "run", str(app), "--no-clear"], work)
    try:
        check(runner.wait_until(lambda t: "first" in t and "the program exited with exit status 0" in t), "`watch run` did not run the program")
        write(app / "src" / "main.bd", 'fn main() {\n    println("second")\n}\n')
        check(runner.wait_until(lambda t: "second" in t), f"the program was not restarted:\n{runner.output()}")
    finally:
        runner.stop()

    # Feature flags reach each run `watch` starts (spec/packages.md, PKG25).
    write(
        work / "featured" / "burdin.toml",
        '[package]\nname = "featured"\nversion = "0.1.0"\n\n[features]\ndefault = []\nextra = []\n',
    )
    write(
        work / "featured" / "src" / "main.bd",
        'use features.extra\n\nfn main() {\n    comptime if extra {\n        println("with extra")\n    } else '
        '{\n        println("base")\n    }\n}\n',
    )
    featured = Watch(["watch", "run", str(work / "featured"), "--features", "extra", "--no-clear"], work)
    try:
        check(
            featured.wait_until(lambda t: "with extra" in t and "the program exited with exit status 0" in t),
            f"`watch run --features extra` did not pass the flag to its run:\n{featured.output()}",
        )
    finally:
        featured.stop()

    # --- usage errors -----------------------------------------------------------------------------------------------
    flagless = harness.run([burdin, "check", str(pkg), "--no-clear"], cwd=work)
    check(flagless.returncode == 2, f"`--no-clear` without `--watch` exited {flagless.returncode}, expected 2")
    check("--watch" in harness.text(flagless.stderr), "the `--no-clear` error does not name `--watch`")
    missing = harness.run([burdin, "watch"], cwd=work)
    check(missing.returncode == 2, f"`burdin watch` exited {missing.returncode}, expected 2")
    absent = harness.run([burdin, "watch", "check", str(work / "nowhere")], cwd=work)
    check(absent.returncode == 2, f"`burdin watch check nowhere` exited {absent.returncode}, expected 2")
    helped = harness.run([burdin, "watch", "--help"], cwd=work)
    check(helped.returncode == 0, f"`burdin watch --help` exited {helped.returncode}, expected 0")
    check("burdin watch [check|build|test|run]" in harness.text(helped.stdout), "`burdin watch --help` lacks the usage")

print(f"Checked `burdin watch` ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
