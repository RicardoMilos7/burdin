#!/usr/bin/env python3
"""Checks the build-script behavior the conformance suite cannot observe in one run (milestone 22, issue #177).

Needs a built driver. BURDIN selects it, like tests/run_tests.py.

What it checks:
  * the script runs on the first build and its output goes to stderr, not stdout;
  * a second build reuses the cache and does not run it;
  * touching a declared input or an `embed` input does not rerun it, while changing either does;
  * deleting a generated module reruns it, and the debug and release profiles have separate caches;
  * a declared directory input is hashed by the contents of every file under it;
  * `--emit-deps` lists the declared input and the script's `embed` input;
  * a declared input that does not exist is E0261;
  * `burdin bindgen` exits 2 for a missing header and an unknown option.
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


def write_package(directory: Path) -> None:
    source = directory / "src"
    source.mkdir(parents=True)
    harness.write_text(
        directory / "burdin.toml",
        '[package]\nname = "app"\nversion = "0.1.0"\n',
    )
    harness.write_text(
        directory / "build.bd",
        """fn main() {
    println("build script ran")
    let args = env.args()
    let out = args[2]
    let header = embed("header.h")
    let source = "const N: i32 = {header.len()}\\n"
    match write_file("{out}/src/generated.bd", source) {
        Result.Ok(_) => {}
        Result.Err(_) => panic("cannot write the generated module"),
    }
    match write_file("{out}/inputs", "declared.txt\\n") {
        Result.Ok(_) => {}
        Result.Err(_) => panic("cannot write the input list"),
    }
}
""",
    )
    harness.write_text(
        source / "main.bd",
        "use generated.N\n\nfn main() {\n    println(\"{}\", N)\n}\n",
    )
    harness.write_text(directory / "header.h", "first header\n")
    harness.write_text(directory / "declared.txt", "first input\n")


def build(run_dir: Path, directory: Path, *options):
    return harness.run([burdin, "build", directory, *options], cwd=run_dir)


with harness.temporary_dir("burdin-build-scripts-") as work:
    package = work / "app"
    run_dir = work / "run"
    run_dir.mkdir()
    write_package(package)
    deps = run_dir / "deps.txt"
    out = package / "target" / "burdin" / "debug"

    first = build(run_dir, package, "--emit-deps", deps)
    check(first.returncode == 0, f"the first build failed: {harness.text(first.stderr)[:400]}")
    first_text = harness.text(first.stderr)
    check("build script ran" in first_text, "the first build did not run the build script")
    check("build script ran" not in harness.text(first.stdout), "the script's output reached stdout")
    check(deps.is_file(), "--emit-deps wrote no file")
    if deps.is_file():
        listed = harness.read_text(deps)
        check("declared.txt" in listed, "--emit-deps does not list the declared input")
        check("header.h" in listed, "--emit-deps does not list the script's embed input")

    second_deps = run_dir / "cached-deps.txt"
    second = build(run_dir, package, "--emit-deps", second_deps)
    check(second.returncode == 0, "the cached build failed")
    check("build script ran" not in harness.text(second.stderr), "the script reran from a valid cache")
    cached = harness.read_text(second_deps) if second_deps.is_file() else ""
    check(
        "declared.txt" in cached and "header.h" in cached,
        "a cached build did not report the recorded inputs to --emit-deps",
    )

    os.utime(package / "declared.txt")
    os.utime(package / "header.h")
    touched = build(run_dir, package)
    check(touched.returncode == 0, "the touched-input build failed")
    check("build script ran" not in harness.text(touched.stderr), "touching an input reran the script")

    harness.write_text(package / "declared.txt", "a changed declared input\n")
    changed = build(run_dir, package)
    check(changed.returncode == 0, "the changed-input build failed")
    check("build script ran" in harness.text(changed.stderr), "a changed declared input did not rerun the script")

    harness.write_text(package / "header.h", "a changed embedded header\n")
    embedded = build(run_dir, package)
    check(embedded.returncode == 0, "the changed-embed build failed")
    check("build script ran" in harness.text(embedded.stderr), "a changed embed input did not rerun the script")

    (out / "src" / "generated.bd").unlink()
    deleted = build(run_dir, package)
    check(deleted.returncode == 0, "the deleted-output build failed")
    check("build script ran" in harness.text(deleted.stderr), "a deleted generated module did not rerun the script")
    check((out / "src" / "generated.bd").is_file(), "the generated module was not recreated")

    release = build(run_dir, package, "--release")
    check(release.returncode == 0, "the release build failed")
    check("build script ran" in harness.text(release.stderr), "the release profile reused the debug cache")

    # A declared input may be a directory: every file under it decides, recursively.
    dirs = work / "dirs"
    write_package(dirs)
    harness.write_text(
        dirs / "build.bd",
        harness.read_text(dirs / "build.bd").replace(
            'write_file("{out}/inputs", "declared.txt\\n")',
            'write_file("{out}/inputs", "include\\n")',
        ),
    )
    (dirs / "include").mkdir()
    harness.write_text(dirs / "include" / "a.h", "first header\n")
    first_dir = build(run_dir, dirs)
    check(first_dir.returncode == 0, f"the directory-input build failed: {harness.text(first_dir.stderr)[:400]}")
    check("build script ran" in harness.text(first_dir.stderr), "the directory-input build did not run the script")
    os.utime(dirs / "include" / "a.h")
    touched_dir = build(run_dir, dirs)
    check(touched_dir.returncode == 0, "the touched directory-input build failed")
    check(
        "build script ran" not in harness.text(touched_dir.stderr),
        "touching a file inside a declared directory reran the script",
    )
    harness.write_text(dirs / "include" / "a.h", "a changed header\n")
    changed_dir = build(run_dir, dirs)
    check(changed_dir.returncode == 0, "the changed directory-input build failed")
    check(
        "build script ran" in harness.text(changed_dir.stderr),
        "changing a file inside a declared directory did not rerun the script",
    )
    harness.write_text(dirs / "include" / "b.h", "an added header\n")
    added_dir = build(run_dir, dirs)
    check(added_dir.returncode == 0, "the added directory-input build failed")
    check(
        "build script ran" in harness.text(added_dir.stderr),
        "adding a file to a declared directory did not rerun the script",
    )

    # A declared input that does not exist is E0261, not a cache that reruns forever.
    missing = work / "missing"
    write_package(missing)
    harness.write_text(missing / "build.bd", harness.read_text(missing / "build.bd").replace(
        'write_file("{out}/inputs", "declared.txt\\n")',
        'write_file("{out}/inputs", "gone.txt\\n")',
    ))
    failed = build(run_dir, missing)
    check(failed.returncode != 0, "a missing declared input did not fail the build")
    check("E0261" in harness.text(failed.stderr), "a missing declared input did not report E0261")
    check("does not exist" in harness.text(failed.stderr), "the missing input is not named")

    # The bindgen command line: a missing header and an unknown option are usage or I/O errors.
    missing_header = harness.run([burdin, "bindgen", "missing.h", "--out", run_dir / "out.bd"], cwd=run_dir)
    check(missing_header.returncode == 2, f"bindgen with a missing header exited {missing_header.returncode}, expected 2")
    check("cannot read" in harness.text(missing_header.stderr), "bindgen does not say it cannot read the header")
    unknown = harness.run([burdin, "bindgen", "x.h", "--bogus"], cwd=run_dir)
    check(unknown.returncode == 2, f"bindgen with an unknown option exited {unknown.returncode}, expected 2")

print(f"Checked build-script caching and inputs ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
