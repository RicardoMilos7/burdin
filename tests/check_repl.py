#!/usr/bin/env python3
"""Checks `burdin repl` (stage 3, issues #580, #582, #584; `spec/repl.md`).

Needs a built driver. BURDIN selects it, like tests/run_tests.py.

A session is a fixture triple under tests/repl/: `<name>.session` holds the lines piped to `burdin repl`,
`<name>.out` the expected standard output, `<name>.err` the expected standard error, and `<name>.exit` the exit
status. Standard input is a pipe, so the REPL runs non-interactively and the transcript is deterministic (RP11).
`BURDIN_UPDATE_GOLDEN=1` rewrites the golden files. Any other file in tests/repl/ is copied next to the session
before it runs, and the session runs in a fresh temporary directory with its own BURDIN_HOME.

What the sessions cover: values printed with `Debug`, bindings and shadowing, a move across lines, type errors,
items defined then used, closures carried between inputs, panics and impure operations with the rollback that keeps
the session unchanged, drops on `:reset`, multi-line input decided by the parser, and the meta-commands
(`:type`, `:ownership`, `:explain`, `:load`, `:help`).

What is checked outside the goldens:
  * `:ir` prints a `define` block for the named function, and `:compile` builds the session and runs it (its output
    appears in addition to the interpreted one);
  * `use shout.loud` in a session loads the official package with no manifest (PKG15);
  * a session that ends inside an incomplete input reports it instead of hanging;
  * standard input that is not a terminal prints no prompt or banner.
"""

import os
import re
import shutil
import sys
from pathlib import Path

import harness

problems = []
checks = 0

UPDATE = bool(os.environ.get("BURDIN_UPDATE_GOLDEN"))

FIXTURES = harness.TESTS / "repl"


def add_problem(message: str) -> None:
    problems.append(message)


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        add_problem(message)


burdin = harness.require_burdin()


def normalize(data: bytes) -> str:
    return harness.text(harness.normalize(data))


def run_session(session: bytes, work: Path):
    env = dict(os.environ)
    env["BURDIN_HOME"] = str(work / "home")
    return harness.run([burdin, "repl"], cwd=work, env=env, input=session)


def golden(name: str, suffix: str) -> Path:
    return FIXTURES / f"{name}.{suffix}"


def check_fixture(path: Path) -> None:
    global checks
    name = path.stem
    session = harness.read_bytes(path)
    with harness.temporary_dir("burdin-repl-") as work:
        # A session may load files; everything beside it but the goldens is available to it.
        for extra in sorted(FIXTURES.iterdir()):
            if extra.is_file() and extra.suffix not in (".session", ".out", ".err", ".exit"):
                shutil.copy(extra, work / extra.name)
        result = run_session(session, work)
    out = normalize(result.stdout)
    err = normalize(result.stderr)
    status = str(result.returncode)
    if UPDATE:
        harness.write_text(golden(name, "out"), out)
        harness.write_text(golden(name, "err"), err)
        harness.write_text(golden(name, "exit"), status + "\n")
        return
    for suffix, actual in (("out", out), ("err", err), ("exit", status)):
        expected_path = golden(name, suffix)
        if not expected_path.is_file():
            add_problem(f"{name}: missing golden tests/repl/{name}.{suffix} (BURDIN_UPDATE_GOLDEN=1 writes it)")
            continue
        expected = harness.read_text(expected_path)
        if suffix == "exit":
            expected = expected.strip()
        if actual != expected:
            add_problem(f"{name}.{suffix} differs:\n--- expected\n{expected}\n--- actual\n{actual}")
    checks += 1
    # A piped session never shows a prompt or the banner (RP11).
    if "burdin> " in out or "Burdin REPL" in out:
        add_problem(f"{name}: a non-interactive session printed a prompt or the banner")


fixtures = sorted(FIXTURES.glob("*.session"))
check(bool(fixtures), "tests/repl has no .session fixtures")
for fixture in fixtures:
    check_fixture(fixture)

# ---- :ir and :compile ----------------------------------------------------------
with harness.temporary_dir("burdin-repl-ir-") as work:
    session = b"fn add(a: i32, b: i32): i32 { a + b }\nadd(1, 2)\n:ir add\n:compile\n"
    result = run_session(session, work)
    out = normalize(result.stdout)
    define = re.search(r"^define .*@burdin\.add\(.*\{$", out, re.MULTILINE)
    check(define is not None, f":ir printed no define block for `add`:\n{out}")
    check("  %" in out or "ret i32" in out, f":ir printed no body:\n{out}")
    # The interpreted call printed `3`; `:compile` builds the session and runs it, printing the same again.
    check(out.split("\n").count("3") == 2, f":compile did not run the session again:\n{out}")

# ---- an official package needs no manifest entry, even in a session -----------
with harness.temporary_dir("burdin-repl-official-") as work:
    env = dict(os.environ)
    env["BURDIN_HOME"] = str(work / "home")
    env["BURDIN_PACKAGES"] = str(harness.TESTS / "packages" / "official")
    result = harness.run([burdin, "repl"], cwd=work, env=env, input=b"use shout.loud\nloud()\n")
    out = normalize(result.stdout)
    check(
        result.returncode == 0 and out == "34\n",
        f"an official package in a session did not load: exit {result.returncode}, out {out!r}, "
        f"err {normalize(result.stderr)[:400]!r}",
    )

# ---- an incomplete input at the end of input is reported, not swallowed --------
with harness.temporary_dir("burdin-repl-eof-") as work:
    result = run_session(b"let x =\n", work)
    text = normalize(result.stderr)
    check(result.returncode == 1, f"a session ending on an incomplete input exited {result.returncode}")
    check("main.bd:" in text and "error[" in text, f"the incomplete input was not reported:\n{text}")

# ---- an unknown meta-command is an error --------------------------------------
with harness.temporary_dir("burdin-repl-meta-") as work:
    result = run_session(b":bogus\n", work)
    check(result.returncode == 1, "an unknown meta-command exited 0")
    check("unknown meta-command" in normalize(result.stderr), "an unknown meta-command is not named")
    result = run_session(b":help\n", work)
    check(result.returncode == 0, "`:help` exited non-zero")
    for word in (":type", ":ownership", ":explain", ":load", ":ir", ":compile", ":reset", ":quit"):
        check(word in normalize(result.stdout), f"`:help` does not list {word}")

if problems:
    print("\n".join(problems))
    print(f"{len(problems)} problem(s) in {checks} checks")
    sys.exit(1)
print(f"all {checks} REPL checks passed")
