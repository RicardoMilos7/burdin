#!/usr/bin/env python3
"""Checks that every `Code::UNSUPPORTED` site is assigned to a milestone (issue #84).

`E9000` is the temporary "not implemented in this compiler yet" code. It must never mark a construct whose
milestone is done, so every site carries a `// e9000: <id>` marker and every id has a row in
`spec/e9000-audit.md` naming the construct, the owning milestone, and the issue that removes it.

The check does not compile anything. Usage: tests/check_e9000.py

It fails when:
  * a `Code::UNSUPPORTED` or `.unsupported(...)` line has no marker, or a marker has no site;
  * two sites share an id, or a row and the source disagree about an id or its file;
  * a row has no construct, an unknown milestone, or no issue;
  * a row names a milestone that `.github/workflows/ci.yml` already runs (that milestone is done), or every
    stage 1 milestone is done;
  * `E9000` appears in the registry in tests/README.md (it is temporary, not a user-facing code);
  * the audit is not linked from the plan and the handoff.
"""

import re
import sys

import harness

AUDIT = harness.ROOT / "spec" / "e9000-audit.md"
REGISTRY = harness.TESTS / "README.md"
WORKFLOW = harness.ROOT / ".github" / "workflows" / "ci.yml"
SITE_RE = re.compile(r"Code::UNSUPPORTED\b|\.unsupported\(")
MARKER_RE = re.compile(r"//\s*e9000:\s*([A-Za-z0-9_-]+)")
ISSUE_RE = re.compile(r"#[0-9]+")
STAGE_ONE = [f"m{number}" for number in range(12, 25)]

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


def rel(path) -> str:
    return str(path.relative_to(harness.ROOT)).replace("\\", "/")


# --- Source sites -----------------------------------------------------------------------------

sites = {}
for path in sorted((harness.ROOT / "crates").rglob("*.rs")):
    text = harness.read_text(path).splitlines()
    for number, line in enumerate(text, 1):
        if line.lstrip().startswith("//"):
            continue
        has_site = bool(SITE_RE.search(line))
        marker = MARKER_RE.search(line)
        if has_site and marker is None:
            add_problem(f"{rel(path)}:{number}: E9000 site has no `// e9000: <id>` marker")
        elif marker is not None and not has_site:
            add_problem(f"{rel(path)}:{number}: `// e9000: {marker.group(1)}` is not on an E9000 site")
        if has_site and marker is not None:
            ident = marker.group(1)
            if ident in sites:
                add_problem(f"{rel(path)}:{number}: duplicate e9000 id `{ident}` (already at {sites[ident]})")
            sites[ident] = f"{rel(path)}:{number}"

# --- Audit table ------------------------------------------------------------------------------

if not AUDIT.is_file():
    print(f"missing {rel(AUDIT)}", file=sys.stderr)
    sys.exit(1)
audit_text = harness.read_text(AUDIT)
match = re.search(r"^## Sites\s*$", audit_text, re.MULTILINE)
if match is None:
    add_problem("spec/e9000-audit.md: missing the `## Sites` section")
    rows_text = ""
else:
    rest = audit_text[match.end():]
    end = re.search(r"^## ", rest, re.MULTILINE)
    rows_text = rest[: end.start()] if end else rest

rows = {}
for line in rows_text.splitlines():
    line = line.strip()
    if not line.startswith("|"):
        continue
    cells = [cell.strip() for cell in line.strip("|").split("|")]
    if len(cells) != 5:
        add_problem(f"spec/e9000-audit.md: table row has {len(cells)} columns, expected 5: {line}")
        continue
    ident = cells[0].strip("`")
    if ident in ("ID", "") or set(ident) <= {"-"}:
        continue
    if ident in rows:
        add_problem(f"spec/e9000-audit.md: duplicate row for `{ident}`")
        continue
    rows[ident] = {"file": cells[1].strip("`"), "construct": cells[2], "owner": cells[3], "issue": cells[4]}

checks += 1
for ident in sorted(set(sites) - set(rows)):
    add_problem(f"spec/e9000-audit.md: no row for site `{ident}` ({sites[ident]})")
for ident in sorted(set(rows) - set(sites)):
    add_problem(f"spec/e9000-audit.md: row `{ident}` has no E9000 site (remove the row)")

for ident, row in sorted(rows.items()):
    checks += 1
    if not row["construct"]:
        add_problem(f"spec/e9000-audit.md: `{ident}` has no construct")
    if row["owner"] == "helper":
        if row["issue"] != "-":
            add_problem(f"spec/e9000-audit.md: `{ident}` is the helper and must use `-` as its issue")
    else:
        if not ISSUE_RE.fullmatch(row["issue"]):
            add_problem(f"spec/e9000-audit.md: `{ident}` has no issue link (got `{row['issue']}`)")
        if row["owner"] == "Stage 1":
            pass
        elif re.fullmatch(r"m[0-9]{2}", row["owner"]):
            if not 12 <= int(row["owner"][1:]) <= 24:
                add_problem(f"spec/e9000-audit.md: `{ident}` names {row['owner']}, which is not a stage 1 milestone")
        else:
            add_problem(f"spec/e9000-audit.md: `{ident}` has unknown owner `{row['owner']}`")
    if ident in sites:
        source_file = sites[ident].rsplit(":", 1)[0]
        if row["file"] != source_file:
            add_problem(
                f"spec/e9000-audit.md: `{ident}` says `{row['file']}`, but the site is in `{source_file}`"
            )

# --- Finished milestones ----------------------------------------------------------------------

workflow = harness.read_text(WORKFLOW)
finished = set()
for line in workflow.splitlines():
    if line.lstrip().startswith("#") or "run_tests.py" not in line:
        continue
    finished.update(re.findall(r"\bm(?:1[2-9]|2[0-4])\b", line))
if finished:
    for ident, row in sorted(rows.items()):
        if row["owner"] in finished:
            add_problem(
                f"spec/e9000-audit.md: `{ident}` is owned by {row['owner']}, which CI already runs; "
                "remove the site and the row"
            )
stage_one_done = all(milestone in finished for milestone in STAGE_ONE)
if stage_one_done:
    for ident, row in sorted(rows.items()):
        if row["owner"] == "Stage 1":
            add_problem(
                f"spec/e9000-audit.md: `{ident}` is owned by Stage 1, which is done; remove the site and the row"
            )

# --- The registry and the cross-references -----------------------------------------------------

registry = harness.read_text(REGISTRY)
registry_section = registry.split("## Error codes", 1)
if len(registry_section) == 2:
    registry_table = registry_section[1].split("### Codes without a compile-fail test", 1)[0]
    checks += 1
    if re.search(r"^\| E9000 \|", registry_table, re.MULTILINE):
        add_problem("tests/README.md: E9000 is in the registry; it is temporary and must stay out")

for relative in ("spec/stage1-subset.md", "HANDOFF.md", "CLAUDE.md", "tests/README.md"):
    path = harness.ROOT / relative
    checks += 1
    if not path.is_file() or "e9000-audit.md" not in harness.read_text(path):
        add_problem(f"{relative}: does not link spec/e9000-audit.md")

# --- Result -----------------------------------------------------------------------------------

by_owner = {}
for row in rows.values():
    by_owner[row["owner"]] = by_owner.get(row["owner"], 0) + 1
summary = ", ".join(f"{owner} {count}" for owner, count in sorted(by_owner.items()))
print(f"Checked {len(sites)} E9000 sites in {len(rows)} audit rows ({summary}).")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
