#!/usr/bin/env python3
"""Audits the Rust dependencies of the Burdin compiler (issue #449).
It needs no compiler, but re-verifies the license fields through `cargo metadata` when the registry
cache is populated (set BURDIN_DEPS_REQUIRE_METADATA=1 to make that verification mandatory; CI does).

What it checks:
  * Cargo.lock exists, is tracked by git, and pins every external crate by name, exact version,
    crates.io source, and checksum;
  * every workspace crate is under crates/ and every crates/ directory is in the lockfile;
  * tests/dependencies.txt names every external crate exactly (name and version), with a license and
    a reason, and nothing that is not in Cargo.lock;
  * every license is permissive: each identifier in the expression is on the policy allowlist in
    tests/dependencies.txt (plus the exact term `Apache-2.0 WITH LLVM-exception`);
  * every direct external dependency in Cargo.toml or a crate manifest requires an exact `=x.y.z`
    version, and that version is the one Cargo.lock resolves;
  * when `cargo metadata --locked --offline` succeeds, each crate's license field from its own
    metadata matches the reviewed inventory, the package count matches, and `--locked` proves the
    lockfile is current with the manifests.
"""

import json
import os
import re
import shutil
import sys

import harness

LOCK = harness.ROOT / "Cargo.lock"
AUDIT = harness.TESTS / "dependencies.txt"

# Permissive licenses the project accepts. Every identifier in an expression must be on this list, so
# an OR with a copyleft option is rejected instead of being accepted through the permissive branch.
ALLOWED_LICENSES = {
    "MIT",
    "MIT-0",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "0BSD",
    "Zlib",
    "BSL-1.0",
    "Unlicense",
    "CC0-1.0",
    "Unicode-3.0",
    "Unicode-DFS-2016",
}
REGISTRY_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


if not LOCK.is_file():
    print(f"missing {LOCK}", file=sys.stderr)
    sys.exit(1)
if not AUDIT.is_file():
    print(f"missing {AUDIT}", file=sys.stderr)
    sys.exit(1)

# --- The lockfile ------------------------------------------------------------

if harness.run(["git", "-C", harness.ROOT, "rev-parse", "--git-dir"]).returncode == 0:
    checks += 1
    tracked = harness.run(["git", "-C", harness.ROOT, "ls-files", "--error-unmatch", "Cargo.lock"])
    if tracked.returncode != 0:
        add_problem("Cargo.lock: not tracked by git (the pin must be committed)")

packages = []
current = {}
for line in harness.read_text(LOCK).split("\n"):
    if line == "[[package]]":
        if current:
            packages.append(current)
        current = {}
        continue
    for field in ("name", "version", "source", "checksum"):
        match = re.match(rf'^{field} = "(.*)"$', line)
        if match:
            current[field] = match.group(1)
            break
if current:
    packages.append(current)

if not packages:
    add_problem("Cargo.lock: no packages (is it empty or malformed?)")

external = []
workspace = []
for package in packages:
    name = package.get("name", "")
    if not name:
        continue
    if not package.get("source"):
        workspace.append(name)
        continue
    checks += 1
    source = package.get("source", "")
    if source != REGISTRY_SOURCE:
        add_problem(f"Cargo.lock: {name} {package.get('version', '')} comes from {source}, not crates.io")
    if not package.get("checksum"):
        add_problem(f"Cargo.lock: {name} {package.get('version', '')} has no checksum (it is not pinned)")
    external.append(package)

# Every workspace package is a crate directory, and every crate directory is in the lockfile.
for name in workspace:
    if not (harness.ROOT / "crates" / name).is_dir():
        add_problem(f"Cargo.lock: workspace package {name} has no crates/{name} directory")
for directory in sorted(path for path in (harness.ROOT / "crates").glob("*") if path.is_dir()):
    checks += 1
    if directory.name not in workspace:
        add_problem(
            f"crates/{directory.name}: not a workspace package in Cargo.lock "
            "(run cargo build after adding it)"
        )

# --- The reviewed inventory --------------------------------------------------


def licenses_allowed(expression: str) -> bool:
    if expression.count("(") != expression.count(")"):
        return False
    # Only the LLVM exception is accepted as a WITH term, and only on Apache-2.0.
    if "WITH" in expression:
        if "Apache-2.0 WITH LLVM-exception" in expression:
            expression = expression.replace("Apache-2.0 WITH LLVM-exception", "Apache-2.0")
        else:
            return False
    expression = expression.replace("(", "").replace(")", "")
    # Drop the AND/OR operators; everything left must be a license identifier.
    for _ in range(4):
        updated = re.sub(r"\s+(AND|OR)\s+", " ", expression)
        if updated == expression:
            break
        expression = updated
    if "AND" in expression or "OR" in expression:
        return False
    expression = re.sub(r"\s+", " ", expression).strip()
    if not expression:
        return False
    return all(token in ALLOWED_LICENSES for token in expression.split(" "))

audit_rows = []
raw_keys = []
for line in harness.read_text(AUDIT).split("\n"):
    if not line or line.startswith("#"):
        continue
    fields = line.split("\t", 3)
    while len(fields) < 4:
        fields.append("")
    name, version, license_text, role = fields
    raw_keys.append((name, version))
    checks += 1
    if not name or not version or not license_text or not role:
        add_problem(
            f"tests/dependencies.txt: '{name}' needs four tab-separated fields (name, version, license, why)"
        )
        continue
    if not re.fullmatch(r"[A-Za-z0-9_-]+", name):
        add_problem(f"tests/dependencies.txt: invalid crate name '{name}'")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        add_problem(f"tests/dependencies.txt: {name}: '{version}' is not an exact x.y.z version")
    if not licenses_allowed(license_text):
        add_problem(
            f"tests/dependencies.txt: {name}: license '{license_text}' is not in the permissive policy"
        )
    audit_rows.append((name, version, license_text))

audit_keys = raw_keys
lock_keys = [(package["name"], package.get("version", "")) for package in external]

# The same crate at two versions needs two reviewed lines; sort alone would not catch that.
seen = set()
for key in sorted(audit_keys):
    if key in seen:
        add_problem(f"tests/dependencies.txt: duplicate entry: {key[0]} {key[1]}")
    seen.add(key)

for key in sorted(set(audit_keys) - set(lock_keys)):
    add_problem(f"tests/dependencies.txt: {key[0]} {key[1]} is not in Cargo.lock")
for key in sorted(set(lock_keys) - set(audit_keys)):
    add_problem(f"Cargo.lock: {key[0]} {key[1]} is not reviewed in tests/dependencies.txt")

# --- Exact pins in the manifests --------------------------------------------

# One entry per external dependency declaration: file, name, version requirement.
pins = []
manifests = [harness.ROOT / "Cargo.toml"] + sorted((harness.ROOT / "crates").glob("*/Cargo.toml"))
for manifest in manifests:
    section = ""
    dependency = ""
    lines = harness.read_text(manifest).split("\n")
    for line_number, line in enumerate(lines):
        if line_number == 0:
            section = ""
            dependency = ""
            continue
        if line.lstrip().startswith("#"):
            continue
        if line.lstrip().startswith("["):
            header = line.strip().lstrip("[").rstrip("]")
            section = header
            dependency = ""
            continue
        if not re.match(r"^(dependencies|dev-dependencies|build-dependencies|workspace\.dependencies)$", section) and not re.search(
            r"\.(dependencies|dev-dependencies|build-dependencies)$", section
        ):
            continue
        if dependency:
            match = re.search(r'version\s*=\s*"[^"]*"', line)
            if match:
                requirement = re.sub(r'^version\s*=\s*"', "", match.group(0))
                requirement = re.sub(r'"$', "", requirement)
                pins.append((manifest, dependency, requirement))
            if "}" in line:
                dependency = ""
            continue
        match = re.match(r'^([A-Za-z0-9_-]+)\s*=\s*"', line)
        if match:
            requirement_match = re.search(r'"[^"]*"', line)
            requirement = requirement_match.group(0)[1:-1] if requirement_match else ""
            pins.append((manifest, match.group(1), requirement))
            dependency = ""
            continue
        match = re.match(r"^([A-Za-z0-9_-]+)\s*=\s*\{", line)
        if match:
            dependency = match.group(1)
            version_match = re.search(r'version\s*=\s*"[^"]*"', line)
            if version_match:
                requirement = re.sub(r'^version\s*=\s*"', "", version_match.group(0))
                requirement = re.sub(r'"$', "", requirement)
                pins.append((manifest, dependency, requirement))
            if "}" in line:
                dependency = ""

if not pins:
    add_problem("Cargo.toml: no external dependency was found to pin (the manifest parser may be out of date)")

for manifest, dependency, requirement in pins:
    checks += 1
    relative = manifest.relative_to(harness.ROOT).as_posix()
    if not requirement.startswith("="):
        add_problem(f"{relative}: {dependency}: '{requirement}' is not an exact pin (write '=x.y.z')")
        continue
    version = requirement[1:]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        add_problem(f"{relative}: {dependency}: '{requirement}' is not an exact =x.y.z pin")
        continue
    if (dependency, version) not in set(lock_keys):
        add_problem(
            f"{relative}: {dependency} is pinned to {version}, which is not the version in Cargo.lock"
        )

# --- The crates' own metadata (optional, mandatory in CI) --------------------

metadata_note = ""
cargo = shutil.which("cargo")
if cargo is None:
    metadata_note = "cargo is not on PATH"
else:
    metadata = harness.run(
        [cargo, "metadata", "--locked", "--offline", "--format-version", "1"], cwd=harness.ROOT
    )
    if metadata.returncode == 0:
        try:
            data = json.loads(harness.text(metadata.stdout))
            metadata_licenses = sorted(
                f"{package['name']}@{package['version']}@{package['license']}"
                for package in data["packages"]
                if package.get("license")
            )
            audit_licenses = sorted(f"{name}@{version}@{license_text}" for name, version, license_text in audit_rows)
            for entry in sorted(set(audit_licenses) ^ set(metadata_licenses)):
                add_problem(f"tests/dependencies.txt: license does not match the crate metadata: {entry}")
            if len(metadata_licenses) != len(external):
                add_problem(
                    f"cargo metadata: {len(metadata_licenses)} licensed packages, but Cargo.lock has "
                    f"{len(external)} external crates (is the metadata parser out of date?)"
                )
        except (ValueError, KeyError, TypeError):
            metadata_note = "cargo metadata --locked --offline produced unexpected output"
    else:
        first_line = harness.text(metadata.stderr).split("\n")[0]
        metadata_note = "cargo metadata --locked --offline failed" + (
            f": {first_line}" if first_line else ""
        )

if metadata_note:
    if os.environ.get("BURDIN_DEPS_REQUIRE_METADATA") == "1":
        add_problem(f"{metadata_note} (BURDIN_DEPS_REQUIRE_METADATA=1)")
    else:
        print(
            f"note: {metadata_note}; the license fields were not re-verified against the crates",
            file=sys.stderr,
        )

print(
    f"Checked {checks} dependency records and pins in Cargo.lock, the manifests, and tests/dependencies.txt"
)
if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
