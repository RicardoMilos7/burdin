#!/usr/bin/env python3
"""Generates release notes from CHANGELOG.md and checks the links in them (issues #452, #456).

Usage: tests/release_notes.py [options]
  --version <x.y.z>      use the `## <x.y.z> ...` section
  --unreleased           use the `## Unreleased ...` section (the default; for a dry run)
  --changelog <file>     read another changelog (default: CHANGELOG.md)
  --check-links          fail when a relative link in the notes does not exist
  --output <file>        write the notes to a file instead of stdout
  -h, --help             show this help

The notes are the changelog section under a `# Burdin <version>` title. Cutting a stable release means moving the
Unreleased entries under a `## <version> — <date>` heading first; docs/releasing.md has the steps.
"""

import os
import re
import sys
from pathlib import Path

import harness

USAGE = "Usage:" + __doc__.split("Usage:", 1)[1].split("The notes are")[0].rstrip()


def die(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(1)


def manifest_version() -> str:
    for line in harness.read_text(harness.ROOT / "Cargo.toml").split("\n"):
        match = re.match(r'^version = "(.*)"$', line)
        if match:
            return match.group(1)
    return ""


version = ""
unreleased = False
changelog = harness.ROOT / "CHANGELOG.md"
check_links = False
output = None

index = 1
while index < len(sys.argv):
    argument = sys.argv[index]
    if argument == "--version":
        if index + 1 >= len(sys.argv):
            die("--version needs a value")
        version = sys.argv[index + 1]
        index += 2
    elif argument == "--unreleased":
        unreleased = True
        index += 1
    elif argument == "--changelog":
        if index + 1 >= len(sys.argv):
            die("--changelog needs a path")
        changelog = Path(sys.argv[index + 1])
        index += 2
    elif argument == "--check-links":
        check_links = True
        index += 1
    elif argument == "--output":
        if index + 1 >= len(sys.argv):
            die("--output needs a path")
        output = Path(sys.argv[index + 1])
        index += 2
    elif argument in ("-h", "--help"):
        print(USAGE)
        raise SystemExit(0)
    else:
        die(f"unknown option: {argument} (try --help)")

if not changelog.is_file():
    die(f"missing changelog: {changelog}")
if version and unreleased:
    die("--version and --unreleased are mutually exclusive")

if unreleased or not version:
    pattern = re.compile(r"^## Unreleased")
    title_version = manifest_version() or "unknown"
    title = f"# Burdin {title_version} (unreleased)"
else:
    pattern = re.compile(r"^## " + re.escape(version) + r"(\s|\(|$)")
    title = f"# Burdin {version}"

lines = harness.read_text(changelog).split("\n")
chunk = []
started = False
for line in lines:
    if pattern.search(line):
        started = True
        continue
    if started and line.startswith("## "):
        break
    if started:
        chunk.append(line)

body = ""
trimmed = [index for index, line in enumerate(chunk) if line.strip()]
if trimmed:
    body = "\n".join(chunk[trimmed[0]:trimmed[-1] + 1])

if not body:
    if unreleased or not version:
        die("CHANGELOG.md has no Unreleased section")
    die(
        f"CHANGELOG.md has no section for version {version}; "
        f"add '## {version} — <date>' before tagging"
    )

notes = f"{title}\n\n{body}"

if check_links:
    problems = 0
    targets = sorted(set(re.findall(r"\]\(([^)]*)\)", body)))
    for target in targets:
        if not target:
            continue
        if target.startswith(("http://", "https://", "mailto:", "tel:", "#")):
            continue
        path = target.split("#", 1)[0]
        if not path:
            continue
        path = path.replace("%20", " ")
        resolved = Path(path) if os.path.isabs(path) else harness.ROOT / path
        if not resolved.exists():
            print(f"broken link in the release notes: {target}", file=sys.stderr)
            problems += 1
    if problems:
        raise SystemExit(1)

if output is not None:
    try:
        output.write_bytes((notes + "\n").encode("utf-8"))
    except OSError:
        die(f"cannot write {output}")
else:
    print(notes)
