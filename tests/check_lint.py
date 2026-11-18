#!/usr/bin/env python3
"""Checks `burdin lint` (issues #230, #231, #232, #233, #464, #465, #466, #661). Needs a built driver.
Usage: tests/check_lint.py

BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin is used.

What it checks (spec/lint.md has the rules, LN1-LN32, and names the fixtures that cover each):
  * docs/lints.md has exactly one section for every lint `burdin lint --list` prints, each with a "How to fix"
    paragraph and a checked example, and `burdin lint --explain` prints every section;
  * every example in the page is a complete program (or package) that the linter checks and reports with exactly
    the `//~ L0000` diagnostics marked in it, all of the section's code;
  * every fixture under tests/lint/ (a `.bd` file, or a package directory with a burdin.toml) is linted and the
    diagnostics (file, line, code) are exactly the `//~` markers in its sources, with the exit status its `//% exit:`
    directive says (default 0). A fixture without markers proves that nothing is reported (the negative cases);
  * every fixture under tests/lint/fix/ is fixed: `--fix --dry-run` prints the diff and changes nothing, `--fix`
    writes exactly the golden `<name>.fixed.bd`, and a second `--fix` finds nothing to do. A package fixture under
    tests/lint/fix_packages/ keeps its golden files in an `expect/` folder;
  * a write that fails half way changes no file (all or nothing);
  * the command line: `--list`, `--explain`, option errors and exit statuses, levels, the manifest `[lint]` table,
    and the machine-readable output.

Fixture directives are comment lines at the top of a `.bd` file, or lines of an `expect.txt` next to a package's
burdin.toml:
    //% args: -D unused_variables        extra command line arguments
    //% exit: 1                          the expected exit status
    //% stderr: some text                text the output must contain
"""

import difflib
import os
import re
import shlex
import shutil
import sys
from pathlib import Path

import harness

problems = []
checks = 0
examples = 0
fixtures = 0

DOCS = harness.ROOT / "docs" / "lints.md"
LINT = harness.TESTS / "lint"
DIAGNOSTIC = re.compile(r"^(.+?):([0-9]+):([0-9]+): (warning|error)\[(L[0-9]{4})\]")
MARKER = re.compile(r"//~ ((?:L[0-9]{4}[ ]*)+)")
COMMON = ["--no-color", "--diagnostic-format=short"]


def add_problem(message: str) -> None:
    problems.append(message)


def normalize_path(path: str) -> str:
    path = path.replace("\\", "/")
    return path[2:] if path.startswith("./") else path


burdin = harness.require_burdin(
    2, "error: compiler driver not found. Build it first or set BURDIN=/path/to/burdin"
)


def lint(args, cwd):
    """Runs `burdin lint`; the merged output with carriage returns removed, and the exit status."""
    result = harness.run([burdin, "lint", *args], cwd=cwd, merge=True)
    return harness.text(harness.normalize(result.stdout)), result.returncode


def relative_to(path: str, directory: Path) -> str:
    """`path` as the diagnostics print it (a package target prints absolute paths), relative to `directory`."""
    if os.path.isabs(path):
        for base in (directory, directory.resolve()):
            try:
                return Path(path).relative_to(base).as_posix()
            except ValueError:
                continue
    return normalize_path(path)


def diagnostics_of(output: str, directory: Path):
    found = []
    for line in output.split("\n"):
        match = DIAGNOSTIC.match(line)
        if match:
            found.append((relative_to(match.group(1), directory), int(match.group(2)), match.group(5)))
    return sorted(found)


def markers_of(directory: Path, files):
    expected = []
    for name in files:
        text = harness.read_text(directory / name)
        for number, line in enumerate(text.split("\n"), 1):
            match = MARKER.search(line)
            if match:
                for code in match.group(1).split():
                    expected.append((normalize_path(name), number, code))
    return sorted(expected)


def directives_of(text: str, prefix: str):
    """The `key: value` pairs of the directive lines, in order."""
    found = []
    for line in text.split("\n"):
        if line.startswith(prefix):
            key, _, value = line[len(prefix):].partition(":")
            found.append((key.strip(), value.strip()))
    return found


def describe(items) -> str:
    return "; ".join(f"{path}:{line} {code}" for path, line, code in items) or "(none)"


def compare(label: str, expected, actual, output: str) -> bool:
    if expected == actual:
        return True
    diff = difflib.unified_diff(
        [f"{p}:{n} {c}" for p, n, c in expected],
        [f"{p}:{n} {c}" for p, n, c in actual],
        fromfile="expected",
        tofile="actual",
        lineterm="",
    )
    add_problem(f"{label}: diagnostics differ from the //~ markers: {';'.join(diff)}\n    output: {output.strip()[:600]}")
    return False


# --- The documentation page ---------------------------------------------------------------------------------------


def lint_table():
    """The `(code, name)` pairs of `burdin lint --list`."""
    output, status = lint(["--list"], harness.ROOT)
    rows = []
    for line in output.split("\n"):
        match = re.match(r"^(L[0-9]{4})  (\S+)", line)
        if match:
            rows.append((match.group(1), match.group(2)))
    return rows, status


def section_of(lines, code: str) -> str:
    section = []
    inside = False
    for line in lines:
        if line.startswith(f"## {code}:"):
            inside = True
        elif inside and line.startswith("## "):
            break
        if inside:
            section.append(line)
    return "\n".join(section)


def check_example(directory: Path, code: str, target: str, files, contents) -> None:
    global checks, examples
    for path in files:
        destination = directory / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        harness.write_text(destination, "".join(line + "\n" for line in contents.get(path, [])))
    if not target:
        if len(files) != 1:
            add_problem(f"example {code}: a multi-file example needs target=<path>")
            return
        target = files[0]
    # `none` examples are clean under every lint; the others turn on only the section's lint.
    if code == "none":
        args = [target, "--deny-warnings", *COMMON]
    elif code == "L0000":
        args = [target, *COMMON]
    else:
        name = LINT_NAMES.get(code)
        if name is None:
            add_problem(f"example {code}: not a lint code")
            return
        args = [target, "-A", "all", "-W", name, *COMMON]
    output, status = lint(args, directory)
    sources = [path for path in files if path.endswith(".bd")]
    expected = markers_of(directory, sources)
    for path, number, marker in expected:
        if marker != code:
            add_problem(f"example {code}: {path}:{number} marks {marker} instead of {code}")
    if code == "none":
        if expected:
            add_problem("example none: has //~ markers")
        if status != 0 or diagnostics_of(output, directory):
            add_problem(f"example none: expected no lint findings, got: {output.strip()[:600]}")
    elif not expected:
        add_problem(f"example {code}: no //~ marker")
    else:
        compare(f"example {code}", expected, diagnostics_of(output, directory), output)
    examples += 1
    checks += 1


if not DOCS.is_file():
    print(f"missing {DOCS}", file=sys.stderr)
    sys.exit(1)

table, list_status = lint_table()
LINT_NAMES = {code: name for code, name in table}
checks += 1
if list_status != 0 or len(table) < 2:
    add_problem(f"burdin lint --list failed (status {list_status})")

docs_lines = harness.read_text(DOCS).split("\n")
doc_codes = [m.group(1) for line in docs_lines for m in [re.match(r"^## (L[0-9]{4}):", line)] if m]
checks += 1
if sorted(doc_codes) != sorted(LINT_NAMES):
    add_problem(
        f"docs/lints.md and `burdin lint --list` disagree about the lints: list [{' '.join(sorted(LINT_NAMES))}], "
        f"page [{' '.join(sorted(doc_codes))}]"
    )
for code in sorted(set(doc_codes)):
    if doc_codes.count(code) > 1:
        add_problem(f"docs/lints.md: duplicate section {code}")
for line in docs_lines:
    match = re.match(r"^## (L[0-9]{4}): (\S+)$", line)
    if match and LINT_NAMES.get(match.group(1)) not in (None, match.group(2)):
        add_problem(f"docs/lints.md: {match.group(1)} is `{LINT_NAMES[match.group(1)]}`, not `{match.group(2)}`")

for code in LINT_NAMES:
    checks += 1
    section = section_of(docs_lines, code)
    if not section:
        add_problem(f"docs/lints.md: no section for {code}")
        continue
    if "**How to fix:**" not in section:
        add_problem(f"docs/lints.md: {code} has no '**How to fix:**' paragraph")
    if f"lint-example: {code}" not in section:
        add_problem(f"docs/lints.md: {code} has no checked example")

for index in (harness.ROOT / "README.md", harness.TESTS / "README.md"):
    checks += 1
    if "docs/lints.md" not in harness.read_text(index):
        add_problem(f"{index.relative_to(harness.ROOT).as_posix()}: does not link docs/lints.md")

with harness.temporary_dir("burdin-lint-docs-") as work:
    state = "idle"
    code = ""
    target = ""
    directory = work
    path = ""
    files = []
    contents = {}
    count = 0
    for line in docs_lines:
        if state == "idle":
            if line.startswith("<!-- lint-example:"):
                attrs = line[len("<!-- lint-example:"):].replace("-->", "").split()
                code = attrs[0] if attrs else ""
                target = ""
                for attribute in attrs[1:]:
                    if attribute.startswith("target="):
                        target = attribute[len("target="):]
                    else:
                        add_problem(f"docs/lints.md: unknown example attribute: {attribute}")
                count += 1
                directory = work / f"example-{count}"
                directory.mkdir(parents=True, exist_ok=True)
                files, contents, path = [], {}, ""
                state = "await_fence"
            elif line.startswith("```burdin"):
                add_problem(f"docs/lints.md: a burdin block outside an example: {line}")
        elif state == "await_fence":
            if line == "```burdin":
                state = "in_code"
            else:
                add_problem(f"docs/lints.md: example {code} is not followed by a ```burdin fence")
                state = "idle"
        elif state == "in_code":
            if line == "```":
                state = "await_end"
            elif line.startswith("// --- "):
                path = line[len("// --- "):]
                if path.endswith(" ---"):
                    path = path[:-4]
                if not path or path.startswith("/") or ".." in path:
                    add_problem(f"docs/lints.md: example {code} has an invalid path: {path}")
                    path = ""
                else:
                    files.append(path)
                    contents[path] = []
            else:
                if not path:
                    path = "example.bd"
                    files.append(path)
                    contents[path] = []
                contents.setdefault(path, []).append(line)
        elif state == "await_end":
            if line == "<!-- /lint-example -->":
                check_example(directory, code, target, files, contents)
                state = "idle"
            else:
                add_problem(f"docs/lints.md: unexpected line after the example fence: {line}")
    if state != "idle":
        add_problem("docs/lints.md: unterminated example")

for code, name in LINT_NAMES.items():
    for spelling in (code, name, code.lower()):
        checks += 1
        output, status = lint(["--explain", spelling], harness.ROOT)
        if status != 0:
            add_problem(f"burdin lint --explain {spelling} failed: {output.strip()}")
        elif f"## {code}: {name}" not in output or f"//~ {code}" not in output and code != "L0000":
            add_problem(f"burdin lint --explain {spelling} does not print the section of {code}")

# --- Fixtures -----------------------------------------------------------------------------------------------------


def check_fixture(label: str, directory: Path, target: str, sources, header: str) -> None:
    global checks, fixtures
    checks += 1
    fixtures += 1
    notes = directives_of(header, "//% ")
    args, exit_code, texts = [], 0, []
    for key, value in notes:
        if key == "args":
            args = shlex.split(value)
        elif key == "exit":
            exit_code = int(value)
        elif key == "stderr":
            texts.append(value)
    output, status = lint([target, *args, *COMMON], directory)
    if status != exit_code:
        add_problem(f"{label}: exit status {status}, expected {exit_code}\n    output: {output.strip()[:600]}")
    for text in texts:
        if text not in output:
            add_problem(f"{label}: output does not contain `{text}`: {output.strip()[:600]}")
    if exit_code == 2:
        return
    expected = markers_of(directory, sources)
    compare(label, expected, diagnostics_of(output, directory), output)


if not LINT.is_dir():
    add_problem(f"missing {LINT}")
else:
    skipped = {"fix", "fix_packages"}
    for entry in sorted(LINT.rglob("*")):
        relative = entry.relative_to(LINT)
        if relative.parts[0] in skipped:
            continue
        label = f"tests/lint/{relative.as_posix()}"
        if entry.is_file() and entry.suffix == ".bd" and len(relative.parts) >= 2:
            # A file of a package directory is checked with its package.
            if any((LINT / Path(*relative.parts[:depth]) / "burdin.toml").is_file() for depth in range(1, len(relative.parts))):
                continue
            check_fixture(label, entry.parent, entry.name, [entry.name], harness.read_text(entry))
        elif entry.is_dir() and (entry / "burdin.toml").is_file():
            sources = sorted(p.relative_to(entry).as_posix() for p in entry.rglob("*.bd"))
            expect = entry / "expect.txt"
            # The lines of expect.txt are directives without the `//% ` a `.bd` header starts them with.
            header = "".join(f"//% {line}\n" for line in harness.read_text(expect).split("\n") if line) if expect.is_file() else ""
            check_fixture(label, entry, ".", sources, header)

# --- --fix ---------------------------------------------------------------------------------------------------------


def hunks(old: str, new: str, path: str) -> str:
    return "".join(difflib.unified_diff(old.splitlines(True), new.splitlines(True), f"a/{path}", f"b/{path}", n=3))


def check_fix(label: str, work: Path, target: str, expected_files, originals) -> None:
    """`--fix --dry-run`, `--fix`, and `--fix` again, in `work`. `expected_files` maps a path to its golden text."""
    global checks, fixtures
    checks += 1
    fixtures += 1
    dry = harness.run([burdin, "lint", target, "--fix", "--dry-run", *COMMON], cwd=work)
    dry_out = harness.text(harness.normalize(dry.stdout))
    dry_err = harness.text(harness.normalize(dry.stderr))
    if dry.returncode not in (0, 1):
        add_problem(f"{label}: --fix --dry-run exited with status {dry.returncode}: {dry_err.strip()[:600]}")
    for path, old in originals.items():
        if harness.read_text(work / path) != old:
            add_problem(f"{label}: --dry-run changed {path}")
    for path, golden in expected_files.items():
        wanted = hunks(originals[path], golden, path)
        if wanted not in dry_out.replace("\r", ""):
            add_problem(f"{label}: the --dry-run diff for {path} is not the expected one:\n--- wanted\n{wanted}--- got\n{dry_out}")
    if "would apply" not in dry_err:
        add_problem(f"{label}: --dry-run did not say what it would apply: {dry_err.strip()[:300]}")
    real = harness.run([burdin, "lint", target, "--fix", *COMMON], cwd=work)
    real_err = harness.text(harness.normalize(real.stderr))
    if real.returncode not in (0, 1):
        add_problem(f"{label}: --fix exited with status {real.returncode}: {real_err.strip()[:600]}")
    if "applied" not in real_err:
        add_problem(f"{label}: --fix did not say what it applied: {real_err.strip()[:300]}")
    for path, golden in expected_files.items():
        got = harness.read_text(work / path)
        if got != golden:
            diff = "".join(difflib.unified_diff(golden.splitlines(True), got.splitlines(True), "expected", "actual"))
            add_problem(f"{label}: {path} after --fix differs from the golden file:\n{diff}")
    for path, old in originals.items():
        if path not in expected_files and harness.read_text(work / path) != old:
            add_problem(f"{label}: --fix changed {path}, which has no golden file")
    again = harness.run([burdin, "lint", target, "--fix", *COMMON], cwd=work)
    again_err = harness.text(harness.normalize(again.stderr))
    if "nothing to fix" not in again_err:
        add_problem(f"{label}: a second --fix still found something to do: {again_err.strip()[:600]}")


FIX = LINT / "fix"
if FIX.is_dir():
    for before in sorted(FIX.glob("*.bd")):
        if before.name.endswith(".fixed.bd"):
            continue
        golden = before.with_name(before.stem + ".fixed.bd")
        if not golden.is_file():
            add_problem(f"tests/lint/fix/{before.name}: no golden file {golden.name}")
            continue
        with harness.temporary_dir("burdin-lint-fix-") as work:
            old = harness.read_text(before)
            harness.write_text(work / before.name, old)
            check_fix(f"tests/lint/fix/{before.name}", work, before.name, {before.name: harness.read_text(golden)}, {before.name: old})

FIX_PACKAGES = LINT / "fix_packages"
if FIX_PACKAGES.is_dir():
    for package in sorted(p for p in FIX_PACKAGES.iterdir() if (p / "burdin.toml").is_file()):
        with harness.temporary_dir("burdin-lint-fixpkg-") as work:
            originals, expected = {}, {}
            for source in package.rglob("*"):
                relative = source.relative_to(package)
                if source.is_dir() or relative.parts[0] == "expect":
                    continue
                originals[relative.as_posix()] = harness.read_text(source)
                destination = work / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                harness.write_text(destination, originals[relative.as_posix()])
            expect_dir = package / "expect"
            if expect_dir.is_dir():
                for golden in expect_dir.rglob("*"):
                    if golden.is_file():
                        expected[golden.relative_to(expect_dir).as_posix()] = harness.read_text(golden)
            check_fix(f"tests/lint/fix_packages/{package.name}", work, ".", expected, originals)

# --- A write that fails changes nothing ----------------------------------------------------------------------------

# The linter writes `<file>.burdin-fix` beside each file and renames it over the file. A read-only folder makes the write
# of `src/sub/zz.bd` fail after `src/main.bd` has been replaced, so main.bd must get its old text back. Permissions are
# not enforced for the superuser or on Windows, so the check is skipped there.
if os.name == "posix" and os.geteuid() != 0:
    checks += 1
    with harness.temporary_dir("burdin-lint-atomic-") as work:
        (work / "src" / "sub").mkdir(parents=True)
        harness.write_text(work / "burdin.toml", '[package]\nname = "atomic"\nversion = "0.1.0"\n')
        main_text = 'use sub.zz.{used, extra}\n\nfn main() {\n    println("{}", used())\n}\n'
        zz_text = "fn used(): i32 { 1 }\nfn extra(): i32 { 2 }\n\nfn other() {\n    let unused = 3\n}\n"
        harness.write_text(work / "src" / "main.bd", main_text)
        harness.write_text(work / "src" / "sub" / "zz.bd", zz_text)
        before = harness.run([burdin, "lint", ".", "--fix", "--dry-run", *COMMON], cwd=work, merge=True)
        if b"would apply 2 edits to 2 files" not in before.stdout:
            add_problem(f"all-or-nothing: the setup does not produce a fix in each file: {harness.text(before.stdout).strip()[:600]}")
        (work / "src" / "sub").chmod(0o555)
        try:
            result = harness.run([burdin, "lint", ".", "--fix", *COMMON], cwd=work)
            err = harness.text(harness.normalize(result.stderr))
            if result.returncode != 2:
                add_problem(f"all-or-nothing: expected exit status 2 from a failed write, got {result.returncode}: {err.strip()[:600]}")
            if "no files were changed" not in err:
                add_problem(f"all-or-nothing: the failed write did not say that no file was changed: {err.strip()[:600]}")
            if harness.read_text(work / "src" / "main.bd") != main_text or harness.read_text(work / "src" / "sub" / "zz.bd") != zz_text:
                add_problem("all-or-nothing: a file changed although the write failed")
        finally:
            (work / "src" / "sub").chmod(0o755)

# --- The command line ----------------------------------------------------------------------------------------------

SAMPLE = "fn main() {\n    let unused = 5\n    println(\"hello\")\n}\n"
with harness.temporary_dir("burdin-lint-cli-") as work:
    harness.write_text(work / "sample.bd", SAMPLE)

    def expect_status(label: str, args, status: int, text: str = "") -> str:
        global checks
        checks += 1
        output, got = lint(args, work)
        if got != status:
            add_problem(f"{label}: exit status {got}, expected {status}: {output.strip()[:400]}")
        if text and text not in output:
            add_problem(f"{label}: output does not contain `{text}`: {output.strip()[:400]}")
        return output

    expect_status("default levels warn", ["sample.bd", *COMMON], 0, "warning[L0007]")
    expect_status("summary line", ["sample.bd", *COMMON], 0, "lint: 1 warning, 0 errors")
    expect_status("-D by name", ["sample.bd", "-D", "unused_variables", *COMMON], 1, "error[L0007]")
    expect_status("--deny by code", ["sample.bd", "--deny", "L0007", *COMMON], 1, "error[L0007]")
    expect_status("--deny-warnings", ["sample.bd", "--deny-warnings", *COMMON], 1, "error[L0007]")
    expect_status("-A silences", ["sample.bd", "-A", "unused_variables", *COMMON], 0, "no warnings in")
    expect_status("-A all", ["sample.bd", "-A", "all", *COMMON], 0, "no warnings in")
    expect_status("a later flag wins", ["sample.bd", "-A", "unused_variables", "-D", "unused_variables", *COMMON], 1)
    expect_status("-W undoes -D", ["sample.bd", "-D", "unused_variables", "-W", "unused_variables", *COMMON], 0)
    expect_status("unknown lint", ["sample.bd", "-D", "no_such_lint", *COMMON], 2, "unknown lint")
    expect_status("unknown option", ["sample.bd", "--frobnicate"], 2, "usage")
    expect_status("--dry-run needs --fix", ["sample.bd", "--dry-run"], 2, "--fix")
    expect_status("a missing target", [], 2)
    expect_status("a missing file", ["nope.bd", *COMMON], 1, "cannot open")
    expect_status("--explain an unknown lint", ["--explain", "nope"], 2, "unknown lint")

    broken = work / "broken.bd"
    harness.write_text(broken, "fn main() {\n    let x: i32 = \"a\"\n}\n")
    expect_status("compile errors stop the linter", ["broken.bd", *COMMON], 1, "E0")

    checks += 1
    output, status = lint(["sample.bd", "--diagnostic-format=json"], work)
    if '"code":"L0007"' not in output or '"applicability":"machine"' not in output or '"type":"lint-summary"' not in output:
        add_problem(f"json output lacks the code, the applicability, or the summary: {output.strip()[:600]}")

    # The manifest table.
    package = work / "pkg"
    (package / "src").mkdir(parents=True)
    harness.write_text(package / "src" / "main.bd", SAMPLE)

    def manifest(body: str) -> None:
        harness.write_text(package / "burdin.toml", f'[package]\nname = "pkg"\nversion = "0.1.0"\n{body}')

    manifest("")
    expect_status("a package without [lint]", ["pkg", *COMMON], 0, "warning[L0007]")
    manifest('\n[lint]\nunused_variables = "deny"\n')
    expect_status("[lint] deny", ["pkg", *COMMON], 1, "error[L0007]")
    expect_status("the command line beats the manifest", ["pkg", "-W", "unused_variables", *COMMON], 0, "warning[L0007]")
    manifest('\n[lint]\nunused_variables = "allow"\n')
    expect_status("[lint] allow", ["pkg", *COMMON], 0, "no warnings in")
    manifest('\n[lint]\nno_such_lint = "deny"\n')
    expect_status("[lint] unknown lint", ["pkg", *COMMON], 2, "burdin.toml:6: unknown lint `no_such_lint`")
    manifest('\n[lint]\nunused_variables = "loud"\n')
    expect_status("[lint] unknown level", ["pkg", *COMMON], 2, "burdin.toml:6")

print(f"Checked {checks} items: {examples} documented examples and {fixtures} fixtures")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
