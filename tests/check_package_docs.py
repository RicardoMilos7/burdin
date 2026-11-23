#!/usr/bin/env python3
"""Checks the documentation of every official package (issue #939). Needs a built driver.
Usage: tests/check_package_docs.py [package ...]

BURDIN selects the driver, like tests/run_tests.py. BURDIN_PACKAGES is set to this repository's `packages/` folder
unless the environment already names one, so the checks run against the packages in this checkout.

For each official package (every folder of `packages/` with a `burdin.toml`, or only the named ones) it checks:

  * undocumented: every public item (a top-level item that is not `priv`, and every public method) has a doc comment.
    These are the items `burdin doc --coverage` counts, so the zero here is the coverage report's zero. An internal
    helper that is not part of the API is `priv`, which takes it out of the count;
  * examples: every public struct, enum, error enum, interface, function, and method has at least one code block in
    its doc comment (spec/doc.md DC6). Extern functions are exempt: a binding to a C symbol has no example that runs
    without the library it names, and the module that declares it documents what it calls;
  * links: no intra-doc link is broken (`status` `broken` in `burdin doc --output-format json`);
  * doc tests: `burdin test --doc` passes, so every documented example compiles and runs (spec/test-runner.md);
  * reference page: `docs/packages/<name>.md` exists and names the package's version and its `use` line.

The checks print one line per problem and exit with status 1 when there is any.
"""

import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

import harness

ROOT = harness.ROOT
PACKAGES = ROOT / "packages"
DOCS = ROOT / "docs" / "packages"

problems = []
checks = 0

# Kinds of top-level items that must have an example (extern functions are exempt, see the module doc).
EXAMPLE_KINDS = {"struct", "enum", "error enum", "interface", "function"}
SKIP_KINDS = {"module"}

burdin = harness.require_burdin(
    2, "error: compiler driver not found. Build it first or set BURDIN=/path/to/burdin"
)


def add_problem(message: str) -> None:
    problems.append(message)


def official_packages(names):
    found = []
    for folder in sorted(PACKAGES.iterdir()):
        if (folder / "burdin.toml").is_file():
            found.append(folder)
    if names:
        wanted = set(names)
        found = [folder for folder in found if folder.name in wanted]
        missing = wanted - {folder.name for folder in found}
        for name in sorted(missing):
            add_problem(f"{name}: no official package of that name in packages/")
    return found


def package_version(folder: Path) -> str:
    text = harness.read_text(folder / "burdin.toml")
    match = re.search(r'^version\s*=\s*"([^"]+)"', text, re.MULTILINE)
    return match.group(1) if match else ""


def run_driver(args, env):
    return subprocess.run([burdin, *args], env=env, capture_output=True, text=True)


def public_items(doc):
    """Yields (label, item) for every public top-level item and public method of the documented package."""
    for module in doc["modules"]:
        for item in module["items"]:
            if item.get("private") or item["kind"] in SKIP_KINDS:
                continue
            yield module["name"] + "." + item["name"], item
            for method in item.get("methods") or []:
                if method.get("private") or method.get("kind") != "method" or not method.get("span"):
                    continue
                yield module["name"] + "." + item["name"] + "." + method["name"], method


def check_package(folder: Path, env) -> None:
    global checks
    name = folder.name
    with tempfile.TemporaryDirectory(prefix="burdin-docs-") as scratch:
        out = Path(scratch) / f"{name}.json"
        result = run_driver(["doc", str(folder), "--output-format", "json", "-o", str(out)], env)
        if result.returncode != 0:
            add_problem(f"{name}: `burdin doc` failed with status {result.returncode}: {result.stderr.strip()}")
            return
        doc = json.loads(out.read_text(encoding="utf-8"))

    undocumented = []
    no_example = []
    broken = []
    count = 0
    for label, item in public_items(doc):
        count += 1
        checks += 1
        docs = item.get("docs") or ""
        if not docs.strip():
            undocumented.append(f"{label} ({item.get('span', {}).get('file', '?')}:{item.get('span', {}).get('line', '?')})")
        elif item["kind"] in EXAMPLE_KINDS or item["kind"] == "method":
            if "```" not in docs:
                no_example.append(label)
        for link in item.get("links") or []:
            if link.get("status") == "broken":
                broken.append(f"{label}: [{link.get('text')}] -> {link.get('target')}")
    for module in doc["modules"]:
        for link in module.get("links") or []:
            if link.get("status") == "broken":
                broken.append(f"{module['name']}: [{link.get('text')}] -> {link.get('target')}")

    for label in undocumented:
        add_problem(f"{name}: undocumented public item {label}")
    for label in no_example:
        add_problem(f"{name}: public item {label} has no example")
    for line in broken:
        add_problem(f"{name}: broken link in {line}")

    version = package_version(folder)
    page = DOCS / f"{name}.md"
    checks += 1
    if not page.is_file():
        add_problem(f"{name}: no reference page at docs/packages/{name}.md")
    else:
        text = harness.read_text(page)
        checks += 1
        if f"`{version}`" not in text and f"version {version}" not in text:
            add_problem(f"{name}: docs/packages/{name}.md does not name version {version}")
        checks += 1
        if f"use {name}" not in text:
            add_problem(f"{name}: docs/packages/{name}.md does not show `use {name}`")

    print(f"{name}: {count} public items, {len(undocumented)} undocumented, {len(no_example)} without an example, "
          f"{len(broken)} broken links")

    # Doc tests: every documented example compiles and runs.
    result = subprocess.run([burdin, "test", "--doc"], cwd=folder, env=env, capture_output=True, text=True)
    checks += 1
    if result.returncode != 0:
        summary = [line for line in (result.stdout + result.stderr).splitlines() if line.startswith(("FAIL", "----"))]
        add_problem(f"{name}: doc tests failed (`burdin test --doc`): " + "; ".join(summary[:10]))
    else:
        print(f"{name}: doc tests pass")


def main() -> int:
    names = sys.argv[1:]
    env = dict(os.environ)
    env.setdefault("BURDIN_PACKAGES", str(PACKAGES))
    for folder in official_packages(names):
        check_package(folder, env)
    if problems:
        print()
        for problem in problems:
            print("error: " + problem)
    print(f"\n{checks} checks, {len(problems)} problems")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
