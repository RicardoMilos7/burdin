#!/usr/bin/env python3
"""Checks the `math` official package (stage 3, issue #268; spec/math.md).

Needs a built driver (BURDIN selects it, like tests/run_tests.py). BURDIN_PACKAGES points at a copy of this
repository's `packages/` folder, so nothing touches an installed compiler.

What it checks:
  * the manifest, the module files, and the README of `packages/math`;
  * the fixtures under `tests/math` are the ones `tests/math/regenerate.py` writes (`--check`), and every fixture file
    is one this script knows the mode of;
  * every known-answer case, through the runner in `tests/math/runner`: each line of each fixture file prints `name: ok`,
    and the runner ends with `failures=0`. The fixtures hold the exact rational answers, the integer number theory, the
    statistics, the vectors, and the fixed-size and heap-backed matrices (MA3-MA8);
  * the runner notices a wrong vector: a fixture with one flipped digit must report a failure;
  * `use math` loads the package with no manifest entry, and `burdin.lock` pins it
    with `source = "official"` and the PKG7 checksum of its files;
  * `--locked` accepts the pin, rejects a changed package by name, and a plain build re-pins it.

The conformance cases of the package (`tests/s3_math`) run under tests/run_tests.py, not here.
"""

import hashlib
import os
import re
import shutil
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

FIXTURES = harness.TESTS / "math"
RUNNER = FIXTURES / "runner"
PACKAGES = harness.ROOT / "packages"
PACKAGE = PACKAGES / "math"

# fixture file -> (runner mode, the fewest cases it must hold)
KNOWN_ANSWERS = {
    "num.kat": ("num", 2000),
    "scalar.kat": ("float", 15),
    "gamma.kat": ("float", 40),
    "stats.kat": ("stats", 100),
    "vector.kat": ("vector", 50),
    "matrix.kat": ("fixed", 40),
    "linalg.kat": ("dyn", 80),
}
MODULES = ["math.bd", "num.bd", "special.bd", "stats.bd", "vector.bd", "matrix.bd", "dyn_matrix.bd"]
MANIFEST = '[package]\nname = "mathkat"\nversion = "0.1.0"\n'


def tree_checksum(directory: Path) -> str:
    """PKG7: `sha256:` and the SHA-256 of every file's path, a NUL, its length, a NUL, and its bytes, by path."""
    files = []
    for path in directory.rglob("*"):
        relative = path.relative_to(directory).as_posix()
        if path.is_file() and relative.split("/")[0] not in (".git", "target"):
            files.append((relative, path))
    digest = hashlib.sha256()
    for relative, path in sorted(files):
        data = path.read_bytes()
        digest.update(relative.encode() + b"\0" + str(len(data)).encode() + b"\0" + data)
    return "sha256:" + digest.hexdigest()


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    harness.write_text(path, content)


def snapshot(directory: Path):
    def read(name):
        path = directory / name
        return harness.read_bytes(path) if path.exists() else None

    return read("burdin.toml"), read("burdin.lock")


manifest_text = harness.read_text(PACKAGE / "burdin.toml")
check(re.search(r'^name = "math"$', manifest_text, re.M) is not None, "packages/math does not declare `name = \"math\"`")
check(re.search(r'^version = "\d+\.\d+\.\d+"$', manifest_text, re.M) is not None, "packages/math has no version")
for module in MODULES:
    check((PACKAGE / "src" / module).is_file(), f"packages/math has no src/{module}")
readme = PACKAGE / "README.md"
check(readme.is_file() and "use math" in harness.read_text(readme), "packages/math/README.md is missing or does not show `use math`")
check(
    not any(path.suffix == ".bd" and "f32" in harness.read_text(path) for path in (PACKAGE / "src").glob("*.bd")),
    "packages/math mentions f32, which spec/math.md (MA1) leaves out",
)

# The fixtures are the ones the generator writes, and every fixture file is accounted for.
generator = FIXTURES / "regenerate.py"
regenerated = harness.run([sys.executable, str(generator), "--check"], cwd=harness.ROOT)
check(
    regenerated.returncode == 0,
    f"tests/math fixtures are out of date: {harness.text(harness.normalize(regenerated.stdout)).strip()[:300]}",
)
on_disk = {path.name for path in FIXTURES.glob("*.kat")}
check(on_disk == set(KNOWN_ANSWERS), f"tests/math fixtures and this script disagree: only on disk {sorted(on_disk - set(KNOWN_ANSWERS))}, only here {sorted(set(KNOWN_ANSWERS) - on_disk)}")

with harness.temporary_dir("burdin-math-") as work:
    env = dict(os.environ)
    env["BURDIN_HOME"] = str(work / "home")
    official = work / "packages"
    shutil.copytree(PACKAGES, official)
    env["BURDIN_PACKAGES"] = str(official)

    def run_in(directory, *arguments, environment=None):
        return harness.run([burdin, *arguments], cwd=directory, env=environment or env)

    def out(result):
        return harness.text(harness.normalize(result.stdout))

    def err(result):
        return harness.text(harness.normalize(result.stderr))

    app = work / "app"
    write(app / "burdin.toml", MANIFEST)
    write(app / "src" / "main.bd", harness.read_text(RUNNER / "src" / "main.bd"))
    built = run_in(app, "build", ".")
    check(built.returncode == 0, f"the math package or its runner did not build: {err(built)[:800]}")
    binary = harness.executable_in(app, "mathkat")
    check(binary.is_file(), "`burdin build .` did not produce the runner")

    if binary.is_file():
        total = 0
        for file_name, (mode, minimum) in sorted(KNOWN_ANSWERS.items()):
            fixture = FIXTURES / file_name
            lines = [line for line in harness.read_text(fixture).splitlines() if line.strip() and not line.startswith("#")]
            check(len(lines) >= minimum, f"tests/math/{file_name} has too few cases ({len(lines)}, want at least {minimum})")
            result = harness.run([binary, mode, str(fixture)], cwd=app, env=env)
            printed = harness.text(harness.normalize(result.stdout))
            check(result.returncode == 0, f"{file_name} ended with status {result.returncode}: {err(result)[:300]}")
            report = printed.strip().splitlines()
            check(
                bool(report) and re.fullmatch(r"cases=\d+ failures=0", report[-1]) is not None,
                f"{file_name}: {[l for l in report if 'FAIL' in l][:5] or report[-1:]}",
            )
            passed = {line[: -len(": ok")] for line in report if line.endswith(": ok")}
            for line in lines:
                name = line.split("|")[0]
                check(name in passed, f"{file_name}: case `{name}` did not pass")
            total += len(lines)
        check(total >= 2400, f"only {total} known-answer cases in all")

        # The runner must notice a wrong answer: one flipped digit in a rational result has to be reported.
        wrong = work / "wrong.kat"
        original = harness.read_text(FIXTURES / "num.kat").splitlines()
        line = next(l for l in original if l.startswith("gcd_12_18|"))
        head, _, expected = line.rpartition("|")
        write(wrong, f"{head}|{int(expected) + 1}\n")
        broken = harness.run([binary, "num", str(wrong)], cwd=app, env=env)
        check("FAIL" in out(broken) and "failures=0" not in out(broken), "the runner did not report a wrong gcd")
        wrong_float = work / "wrong_float.kat"
        gamma_line = next(l for l in harness.read_text(FIXTURES / "gamma.kat").splitlines() if l.startswith("gamma_5.0|"))
        head, _, expected = gamma_line.rpartition("|")
        write(wrong_float, f"{head}|{float(expected) * 1.001!r}\n")
        broken_float = harness.run([binary, "float", str(wrong_float)], cwd=app, env=env)
        check("FAIL" in out(broken_float), "the runner did not report a gamma that is 0.1% off")

    # The lockfile pins the package's files with the official source (PKG15, PKG17).
    lock = harness.read_text(app / "burdin.lock")
    check('source = "official"' in lock, "burdin.lock does not record the official source")
    checksum = tree_checksum(official / "math")
    check(f'checksum = "{checksum}"' in lock, "burdin.lock does not pin the math package's files")
    locked = run_in(app, "build", ".", "--locked")
    check(locked.returncode == 0, f"`--locked` rejected the fresh lockfile: {err(locked)[:300]}")

    # A changed package is a difference the lockfile reports by name, and a plain build re-pins it (PKG17).
    before = snapshot(app)
    write(official / "math" / "src" / "math.bd", harness.read_text(official / "math" / "src" / "math.bd") + "\n// changed\n")
    changed = run_in(app, "build", ".", "--locked")
    check(changed.returncode == 1, f"`--locked` accepted a changed math package (status {changed.returncode})")
    check(
        "`math` is an official package whose files are not the ones burdin.lock pinned" in err(changed),
        f"`--locked` did not name the changed package: {err(changed)[:400]}",
    )
    check(snapshot(app) == before, "`--locked` changed burdin.toml or burdin.lock")
    repinned = run_in(app, "build", ".")
    check(repinned.returncode == 0, f"a plain build did not re-pin the changed package: {err(repinned)[:300]}")
    check(f'checksum = "{tree_checksum(official / "math")}"' in harness.read_text(app / "burdin.lock"), "the re-pin kept the old checksum")
    locked_again = run_in(app, "build", ".", "--locked")
    check(locked_again.returncode == 0, f"`--locked` rejected the re-pinned lockfile: {err(locked_again)[:300]}")

print(f"Checked the math package ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
