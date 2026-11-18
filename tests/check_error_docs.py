#!/usr/bin/env python3
"""Checks the user-facing error-code reference (issue #437). Needs a built driver.
Usage: tests/check_error_docs.py

BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin is used.

What it checks:
  * docs/error-codes.md has exactly one section for every code in the tests/README.md registry, no extra
    codes, and no duplicate headings;
  * every section has a "How to fix" paragraph and at least one checked example;
  * every `burdin` snippet on the page is inside an `error-example` or a `warning-example`, and the page has no
    unchecked snippets;
  * every example is a complete program (or package) that the driver compiles and rejects with exactly the
    `//~ E0000` diagnostics marked in it, all of the section's code; a `warning-example` compiles successfully
    and emits exactly the marked warnings; an example marked `mode=export` is rejected by `burdin bindgen
    --export` instead, because that check runs after compilation;
  * the page is linked from README.md and from tests/README.md;
  * every diagnostic links to its entry (issue #644): the URL scheme is pinned here
    (`<root>/errors/<code>`, ending in the code), the page documents it, every JSON diagnostic of every example
    carries exactly that URL, and `burdin explain <code>` prints it;
  * `burdin explain <code>` prints every section, `burdin explain` lists every code, and an unknown code
    exits 2.
"""

import difflib
import json
import re
import sys
from pathlib import Path

import harness

problems = []
checks = 0
examples = 0

# The stable root of the hosted error reference (issue #644). A diagnostic's link is `<URL_BASE>/errors/<code>`; a
# change to the root or the scheme breaks every link ever printed, so it is pinned here and changed deliberately.
URL_BASE = "https://ricardomilos7.github.io/burdin"

DOCS = harness.ROOT / "docs" / "error-codes.md"
REGISTRY = harness.TESTS / "README.md"


def add_problem(message: str) -> None:
    problems.append(message)


if not DOCS.is_file():
    print(f"missing {DOCS}", file=sys.stderr)
    sys.exit(1)

burdin = harness.require_burdin(
    2, "error: compiler driver not found. Build it first or set BURDIN=/path/to/burdin"
)

# A CRLF checkout must not break the line-by-line parser; read_text removes the carriage returns.
docs_text = harness.read_text(DOCS)
registry_text = harness.read_text(REGISTRY)
docs_lines = docs_text.split("\n")

# --- Registry and page coverage ---------------------------------------------

registry_codes = sorted(
    {match.group(1) for line in registry_text.split("\n") for match in [re.match(r"^\| (E[0-9]{4}) \|", line)] if match}
)
doc_code_list = [
    match.group(1)
    for line in docs_lines
    for match in [re.match(r"^## (E[0-9]{4}):", line)]
    if match
]
doc_codes = sorted(set(doc_code_list))
checks += 1
if registry_codes != doc_codes:
    add_problem(
        "docs/error-codes.md and tests/README.md disagree about the codes: "
        f"registry [{' '.join(registry_codes)}], page [{' '.join(doc_codes)}]"
    )
duplicates = sorted({code for code in doc_code_list if doc_code_list.count(code) > 1})
if duplicates:
    add_problem(f"docs/error-codes.md: duplicate sections: {' '.join(duplicates)}")


def section_of(code: str) -> str:
    section = []
    inside = False
    for line in docs_lines:
        if line.startswith(f"## {code}:"):
            inside = True
            section.append(line)
            continue
        if inside and line.startswith("## "):
            break
        if inside:
            section.append(line)
    return "\n".join(section)


for code in registry_codes:
    checks += 1
    section = section_of(code)
    if not section:
        add_problem(f"docs/error-codes.md: no section for {code}")
        continue
    if "**How to fix:**" not in section:
        add_problem(f"docs/error-codes.md: {code} has no '**How to fix:**' paragraph")
    if f"error-example: {code}" not in section and f"warning-example: {code}" not in section:
        add_problem(f"docs/error-codes.md: {code} has no checked example")

checks += 1
if f"{URL_BASE}/errors/E" not in docs_text:
    add_problem(f"docs/error-codes.md: does not document the link scheme {URL_BASE}/errors/<code>")

for index in (harness.ROOT / "README.md", REGISTRY):
    checks += 1
    if "docs/error-codes.md" not in harness.read_text(index):
        add_problem(f"{index.relative_to(harness.ROOT).as_posix()}: does not link docs/error-codes.md")

# --- Example extraction ------------------------------------------------------


def check_example(directory: Path, code: str, target: str, mode: str, warning: bool, files, contents) -> None:
    # Materialize the captured sources before looking for the target.
    for path in files:
        destination = directory / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        harness.write_text(destination, "".join(line + "\n" for line in contents.get(path, [])))

    global checks, examples
    if not files:
        add_problem(f"example {code}: no source files")
        return
    if not target:
        if len(files) != 1:
            add_problem(f"example {code}: a multi-file example needs target=<path>")
            return
        target = files[0]
    if not (directory / target).exists():
        add_problem(f"example {code}: target {target} is not one of the example files")
        return

    if mode == "export":
        command = [burdin, "bindgen", "--export", target, "--out", str(directory / "exports.h")]
        # `--out` takes a value, so the JSON run repeats the whole command rather than replacing its last argument.
        json_command = [burdin, "bindgen", "--export", target, "--out", str(directory / "exports.json.h"),
                        "--diagnostic-format=json"]
    else:
        command = [burdin, "test" if mode == "test" else "build", target, "--no-color", "--diagnostic-format=short"]
        json_command = command[:-1] + ["--diagnostic-format=json"]
    result = harness.run(command, cwd=directory, merge=True)
    output = harness.text(harness.normalize(result.stdout))
    if warning:
        # A warning is printed, but compilation succeeds: that is what makes it a warning.
        if result.returncode != 0:
            add_problem(f"warning example {code}: compilation failed, but a warning example must compile")
            return
    elif result.returncode == 0:
        add_problem(f"example {code}: compiled successfully, but the section expects {code}")
        return

    expected = []
    for path in files:
        source = directory / path
        if not source.is_file():
            continue
        for number, line in enumerate(harness.read_text(source).split("\n"), 1):
            matches = list(re.finditer(r"//~ (E[0-9]{4})", line))
            if matches:
                expected.append(f"{path} {number} {matches[-1].group(1)}")
    expected.sort()
    if not expected:
        add_problem(f"example {code}: no //~ marker")
        return
    for entry in expected:
        path, number, marker = entry.rsplit(" ", 2)
        if marker != code:
            add_problem(f"example {code}: {path}:{number} marks {marker} instead of {code}")

    actual = []
    severity = "warning" if warning else "error"
    diagnostic = re.compile(r":[0-9]+:[0-9]+: " + severity + r"\[E[0-9]{4}\]")
    for line in output.split("\n"):
        if not diagnostic.search(line):
            continue
        actual_file = re.sub(r":[0-9]+:[0-9]+: " + severity + r"\[E[0-9]{4}\].*$", "", line)
        line_match = re.search(r".*:([0-9]+):[0-9]+: " + severity + r"\[E[0-9]{4}\]", line)
        marker_match = re.search(severity + r"\[(E[0-9]{4})\]", line)
        if not line_match or not marker_match:
            continue
        line_number = line_match.group(1)
        marker = marker_match.group(1)
        actual_file = actual_file.replace("\\", "/")
        if actual_file.startswith("./"):
            actual_file = actual_file[2:]
        matched = ""
        for candidate in files:
            if actual_file == candidate or actual_file.endswith("/" + candidate):
                matched = candidate
                break
        if not matched:
            add_problem(f"example {code}: diagnostic outside the example files: {line}")
            continue
        actual.append(f"{matched} {line_number} {marker}")
    actual.sort()
    if expected != actual:
        diff = list(
            difflib.unified_diff(expected, actual, fromfile="expected", tofile="actual", lineterm="")
        )
        detail = ";".join(diff) + (";" if diff else "")
        add_problem(f"example {code}: diagnostics differ from the //~ markers: {detail}")
    # The same example in the JSON format: every diagnostic carries the stable link to this section's code.
    json_result = harness.run(json_command, cwd=directory, merge=True)
    linked = 0
    for line in harness.text(harness.normalize(json_result.stdout)).split("\n"):
        if not line.startswith("{"):
            continue
        try:
            diagnostic_object = json.loads(line)
        except ValueError:
            add_problem(f"example {code}: a JSON line does not parse: {line[:100]}")
            continue
        if diagnostic_object.get("type") != "diagnostic":
            continue
        linked += 1
        checks += 1
        expected_url = f"{URL_BASE}/errors/{code}"
        if diagnostic_object.get("url") != expected_url:
            add_problem(f"example {code}: a diagnostic has url {diagnostic_object.get('url')!r}, expected {expected_url!r}")
    if linked == 0:
        add_problem(f"example {code}: the JSON format printed no diagnostic")
    examples += 1


with harness.temporary_dir("burdin-error-docs-") as work:
    state = "idle"
    code = ""
    target = ""
    mode = "build"
    directory = None
    path = ""
    files = []
    contents = {}

    kind = "error"
    for line in docs_lines:
        if state == "idle":
            opener = next(
                (name for name in ("error-example:", "warning-example:") if line.startswith(f"<!-- {name}")),
                None,
            )
            if opener is not None:
                kind = opener.split("-")[0]
                attrs = line[len(f"<!-- {opener}"):]
                if attrs.endswith("-->"):
                    attrs = attrs[:-3]
                attrs = attrs.strip()
                code = attrs.split()[0] if attrs else ""
                target = ""
                mode = "build"
                rest = attrs[len(code):]
                for attribute in rest.split():
                    if attribute.startswith("target="):
                        target = attribute[len("target="):]
                    elif attribute in ("mode=test", "mode=export"):
                        mode = attribute[len("mode="):]
                    else:
                        add_problem(f"docs/error-codes.md: unknown example attribute: {attribute}")
                if not re.fullmatch(r"E[0-9]{4}", code):
                    add_problem(f"docs/error-codes.md: example without a code: {line}")
                checks += 1
                directory = work / f"example-{checks}"
                directory.mkdir(parents=True, exist_ok=True)
                files = []
                contents = {}
                path = ""
                state = "await_fence"
            elif line.startswith("```"):
                add_problem(f"docs/error-codes.md: fenced block outside an example: {line}")

        elif state == "await_fence":
            if line == "```burdin":
                state = "in_code"
            else:
                add_problem(
                    f"docs/error-codes.md: example {code} is not followed by a ```burdin fence"
                )
                state = "idle"

        elif state == "in_code":
            if line == "```":
                state = "await_end"
            elif line.startswith("// --- "):
                path = line[len("// --- "):]
                if path.endswith(" ---"):
                    path = path[:-4]
                if not path or path.startswith("/") or ".." in path:
                    add_problem(f"docs/error-codes.md: example {code} has an invalid path: {path}")
                    path = ""
                else:
                    if path in files:
                        add_problem(f"docs/error-codes.md: example {code} repeats the path {path}")
                    destination = directory / path
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    files.append(path)
                    contents[path] = []
                    harness.write_text(destination, "")
            else:
                if not path:
                    path = f"{code}.bd"
                    files.append(path)
                    contents[path] = []
                contents.setdefault(path, []).append(line)

        elif state == "await_end":
            if line == f"<!-- /{kind}-example -->":
                check_example(directory, code, target, mode, kind == "warning", files, contents)
                state = "idle"
            else:
                add_problem(
                    f"docs/error-codes.md: unexpected line after the example fence: {line}"
                )

    if state != "idle":
        add_problem("docs/error-codes.md: unterminated example")

# --- burdin explain ----------------------------------------------------------


def run_explain(*arguments):
    return harness.run([burdin, "explain", *arguments], cwd=harness.ROOT)


checks += 1
listing = run_explain()
if listing.returncode != 0:
    add_problem(f"burdin explain (list) failed: {harness.text(listing.stderr).rstrip(chr(10))}")
else:
    listing_out = harness.text(listing.stdout)
    for code in registry_codes:
        if f"{code}:" not in listing_out:
            add_problem(f"burdin explain (list) omits {code}")

for code in registry_codes:
    checks += 1
    explained = run_explain(code)
    if explained.returncode != 0:
        add_problem(f"burdin explain {code} failed: {harness.text(explained.stderr).rstrip(chr(10))}")
        continue
    explained_out = harness.text(explained.stdout)
    if f"## {code}:" not in explained_out:
        add_problem(f"burdin explain {code} does not print its heading")
    if f"//~ {code}" not in explained_out:
        add_problem(f"burdin explain {code} does not print its example")
    if f"Online: {URL_BASE}/errors/{code}" not in explained_out:
        add_problem(f"burdin explain {code} does not print its link {URL_BASE}/errors/{code}")

checks += 1
unknown = run_explain("E9999")
if unknown.returncode != 2:
    add_problem(f"burdin explain E9999: exit status {unknown.returncode}, expected 2")
elif "unknown error code" not in harness.text(unknown.stderr):
    add_problem("burdin explain E9999: does not report an unknown code")

checks += 1
extra = run_explain("E0003", "extra")
if extra.returncode != 2:
    add_problem(f"burdin explain with two arguments: exit status {extra.returncode}, expected 2")

print(f"Checked {checks} links, sections, and explanations; compiled {examples} examples")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
