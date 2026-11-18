#!/usr/bin/env python3
"""Black-box checks of `burdin fmt` (issues #229, #460, #461, #462, #463, #659, #660; spec/fmt.md).

Each fixture in tests/fmt is an `<name>.input` source and the `<name>.golden` text `burdin fmt` must turn it into.
For every fixture this script checks, through the command line only:

  * `--stdin` gives the golden text and nothing else on standard output (FM12);
  * the golden text is a fixed point, from standard input and from a file (idempotence, FM1);
  * `burdin fmt <file>` rewrites the file to the golden text, lists it, and a second run neither lists nor
    touches it;
  * `--check` changes nothing, lists the file, and exits 1 for the input and 0 for the golden text;
  * the same source with Windows line endings comes back with Windows line endings (FM1).

It also checks directories and packages (dependency grouping from burdin.toml, `target` and hidden directories
skipped), syntax errors (diagnostics, exit status 2, files untouched, other files still formatted), the exit
statuses for usage errors, `--range`, and a set of tricky sources: comments in awkward places and the shapes of
input a formatter commonly gets wrong.

Needs a built driver. BURDIN selects it, like tests/run_tests.py.
"""

import os
import sys
import time
from pathlib import Path

import harness

ROOT = Path(__file__).resolve().parent
FIXTURES = ROOT / "fmt"

problems = []
checks = 0
burdin = harness.require_burdin()


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        problems.append(message)


def fmt(*args, input=None, cwd=None):
    return harness.run([burdin, "fmt", *args], input=input, cwd=cwd)


def stdout_text(result) -> str:
    return result.stdout.decode("utf-8", "replace")


def stderr_text(result) -> str:
    return result.stderr.decode("utf-8", "replace")


def first_difference(expected: str, actual: str) -> str:
    expected_lines, actual_lines = expected.split("\n"), actual.split("\n")
    for index, (a, b) in enumerate(zip(expected_lines, actual_lines), start=1):
        if a != b:
            return f"line {index}: expected {a!r}, got {b!r}"
    return f"expected {len(expected_lines)} lines, got {len(actual_lines)}"


def format_stdin(source: str, *extra):
    return fmt("--stdin", *extra, input=source.encode("utf-8"))


# --- The fixtures -------------------------------------------------------------------------------------------------

inputs = sorted(FIXTURES.glob("*.input"))
check(len(inputs) >= 8, f"tests/fmt has only {len(inputs)} fixtures")

with harness.temporary_dir("burdin-fmt-") as work:
    for input_path in inputs:
        name = input_path.stem
        golden_path = input_path.with_suffix(".golden")
        if not golden_path.is_file():
            problems.append(f"{name}: there is no {golden_path.name}")
            continue
        source = input_path.read_bytes().decode("utf-8")
        golden = golden_path.read_bytes().decode("utf-8")
        check("\r" not in source and "\r" not in golden, f"{name}: fixtures use LF line endings")
        check(golden.endswith("\n") and not golden.endswith("\n\n"), f"{name}: the golden text ends with one newline")

        result = format_stdin(source)
        got = stdout_text(result)
        check(result.returncode == 0, f"{name}: --stdin exited {result.returncode}: {stderr_text(result)[:200]}")
        check(got == golden, f"{name}: --stdin output differs from {golden_path.name}: {first_difference(golden, got)}")
        check(stderr_text(result) == "", f"{name}: --stdin wrote to standard error: {stderr_text(result)[:200]}")

        again = stdout_text(format_stdin(golden))
        check(again == golden, f"{name}: formatting is not idempotent: {first_difference(golden, again)}")

        # In place: the file is rewritten and listed, and a second run leaves it alone.
        target = work / f"{name}.bd"
        target.write_bytes(source.encode("utf-8"))
        result = fmt(str(target))
        check(result.returncode == 0, f"{name}: `burdin fmt <file>` exited {result.returncode}")
        check(stdout_text(result).strip() == str(target), f"{name}: the changed file is listed: {stdout_text(result)!r}")
        check(target.read_bytes().decode("utf-8") == golden, f"{name}: the file was not rewritten to the golden text")
        before = target.stat().st_mtime_ns
        time.sleep(0.02)
        result = fmt(str(target))
        check(result.returncode == 0 and stdout_text(result) == "", f"{name}: a second run lists {stdout_text(result)!r}")
        check(target.stat().st_mtime_ns == before, f"{name}: a formatted file was written again")

        # --check: nothing is written; the input is listed and fails, the golden text passes.
        pending = work / f"{name}.pending.bd"
        pending.write_bytes(source.encode("utf-8"))
        result = fmt("--check", str(pending))
        if source == golden:
            check(result.returncode == 0, f"{name}: --check on formatted source exited {result.returncode}")
        else:
            check(result.returncode == 1, f"{name}: --check on unformatted source exited {result.returncode}")
            check(stdout_text(result).strip() == str(pending), f"{name}: --check lists {stdout_text(result)!r}")
        check(pending.read_bytes().decode("utf-8") == source, f"{name}: --check wrote to the file")
        result = fmt("--check", str(target))
        check(result.returncode == 0 and stdout_text(result) == "", f"{name}: --check on the golden text: {stdout_text(result)!r}")
        result = fmt("--stdin", "--check", input=source.encode("utf-8"))
        check(result.returncode == (0 if source == golden else 1), f"{name}: --stdin --check exited {result.returncode}")

        # Windows line endings are kept.
        crlf = format_stdin(source.replace("\n", "\r\n"))
        check(
            stdout_text(crlf) == golden.replace("\n", "\r\n"),
            f"{name}: a CRLF source does not come back as CRLF: {first_difference(golden.replace(chr(10), chr(13) + chr(10)), stdout_text(crlf))}",
        )

# --- Directories and packages ---------------------------------------------------------------------------------------

UNFORMATTED = "fn  main( ){let x=1\n}\n"
FORMATTED = "fn main() {\n    let x = 1\n}\n"

with harness.temporary_dir("burdin-fmt-dir-") as work:
    package = work / "app"
    (package / "src" / "deep").mkdir(parents=True)
    (package / "target").mkdir()
    (package / ".hidden").mkdir()
    (package / "burdin.toml").write_bytes(b'[package]\nname = "app"\n\n[dependencies]\njson = { path = "../json" }\n')
    (work / "json" / "src").mkdir(parents=True)
    (work / "json" / "burdin.toml").write_bytes(b'[package]\nname = "json"\n')
    (work / "json" / "src" / "json.bd").write_bytes(b"pub fn parse() {}\n")
    (package / "src" / "main.bd").write_bytes(
        b"use shapes\nuse json\nuse io\n\nfn main() {}\n"
    )
    (package / "src" / "shapes.bd").write_bytes(UNFORMATTED.encode())
    (package / "src" / "deep" / "inner.bd").write_bytes(UNFORMATTED.encode())
    (package / "target" / "skipped.bd").write_bytes(UNFORMATTED.encode())
    (package / ".hidden" / "skipped.bd").write_bytes(UNFORMATTED.encode())
    (package / "src" / "notes.txt").write_bytes(b"not a source")

    result = fmt("--check", str(package))
    listed = stdout_text(result).split("\n")
    check(result.returncode == 1, f"package --check exited {result.returncode}")
    check(
        listed[:3] == [str(package / "src" / "deep" / "inner.bd"), str(package / "src" / "main.bd"), str(package / "src" / "shapes.bd")],
        f"package --check lists the files in sorted order, skipping target and hidden directories: {listed!r}",
    )
    check("1 file" not in stderr_text(result) and "3 files would be reformatted" in stderr_text(result), f"the --check summary: {stderr_text(result)!r}")

    result = fmt(str(package))
    check(result.returncode == 0, f"package fmt exited {result.returncode}")
    check(
        (package / "src" / "main.bd").read_bytes().decode() == "use io\n\nuse json\n\nuse shapes\n\nfn main() {}\n",
        "the package's use declarations are grouped: standard library, dependencies from burdin.toml, the package",
    )
    check((package / "src" / "shapes.bd").read_bytes().decode() == FORMATTED, "a package file is formatted")
    check((package / "target" / "skipped.bd").read_bytes().decode() == UNFORMATTED, "`target` is skipped")
    check((package / ".hidden" / "skipped.bd").read_bytes().decode() == UNFORMATTED, "a hidden directory is skipped")
    check(fmt("--check", str(package)).returncode == 0, "a formatted package passes --check")

    # No path: the current directory.
    (work / "loose").mkdir()
    (work / "loose" / "a.bd").write_bytes(UNFORMATTED.encode())
    result = fmt("--check", cwd=work / "loose")
    check(result.returncode == 1 and "a.bd" in stdout_text(result), f"no path means the current directory: {stdout_text(result)!r}")

    # --stdin-filepath tells which package the source is in.
    result = format_stdin("use shapes\nuse json\nuse io\n", "--stdin-filepath", str(package / "src" / "main.bd"))
    check(stdout_text(result) == "use io\n\nuse json\n\nuse shapes\n", f"--stdin-filepath groups by its package: {stdout_text(result)!r}")
    result = format_stdin("use shapes\nuse json\nuse io\n")
    check(stdout_text(result) == "use io\n\nuse json\nuse shapes\n", f"without a package every non-std name is the package's: {stdout_text(result)!r}")

# --- Syntax errors and the other failures ------------------------------------------------------------------------

with harness.temporary_dir("burdin-fmt-err-") as work:
    broken = work / "broken.bd"
    good = work / "good.bd"
    broken.write_bytes(b"fn main( {\n")
    good.write_bytes(UNFORMATTED.encode())
    result = fmt(str(broken), str(good), "--no-color")
    check(result.returncode == 2, f"a syntax error exits 2, got {result.returncode}")
    check("error[E0001]" in stderr_text(result), f"a syntax error is reported with the compiler's diagnostic: {stderr_text(result)[:200]!r}")
    check("syntax errors" in stderr_text(result), "the error says the file was not formatted")
    check(broken.read_bytes() == b"fn main( {\n", "a file with a syntax error is untouched")
    check(good.read_bytes().decode() == FORMATTED, "the other files are still formatted")
    check(stdout_text(result).strip() == str(good), f"only the changed file is listed: {stdout_text(result)!r}")

    result = format_stdin("fn main( {\n", "--no-color")
    check(result.returncode == 2 and stdout_text(result) == "", "--stdin with a syntax error exits 2 and prints no source")
    check("error[E0001]" in stderr_text(result) and "<stdin>" in stderr_text(result), "the error names <stdin>")
    result = format_stdin("fn main( {\n", "--stdin-filepath", "named.bd", "--diagnostic-format", "short")
    check("named.bd:" in stderr_text(result), f"--stdin-filepath names the source: {stderr_text(result)[:200]!r}")

    for args, what in [
        (["--range", "0:1", str(good)], "--range without --stdin"),
        (["--stdin-filepath", "x.bd", str(good)], "--stdin-filepath without --stdin"),
        (["--bogus"], "an unknown option"),
        ([str(work / "missing.bd")], "a missing path"),
        (["--stdin", str(good)], "--stdin with a file"),
        (["--stdin", "--range", "5"], "a malformed range"),
        (["--stdin", "--range", "9:3"], "a backwards range"),
    ]:
        result = fmt(*args, input=b"")
        check(result.returncode == 2, f"{what} exits 2, got {result.returncode}")
        check(stdout_text(result) == "", f"{what} prints nothing to standard output")

    check(fmt("--stdin", input=b"\xff\xfe").returncode == 2, "invalid UTF-8 on standard input exits 2")

# --- Ranges ---------------------------------------------------------------------------------------------------------

RANGE_SOURCE = "fn  a( ){1}\n\nfn  b( ){2}\n\nfn  c( ){3}\n"
start = RANGE_SOURCE.index("fn  b")
result = format_stdin(RANGE_SOURCE, "--range", f"{start}:{start + 3}")
check(
    stdout_text(result) == "fn  a( ){1}\n\nfn b() {\n    2\n}\n\nfn  c( ){3}\n",
    f"--range formats only the items it touches: {stdout_text(result)!r}",
)
check(format_stdin(stdout_text(result), "--range", f"{start}:{start + 3}").stdout == result.stdout, "a range is idempotent")
result = format_stdin(RANGE_SOURCE, "--range", "0:0")
check(stdout_text(result).startswith("fn a() {\n    1\n}\n\nfn  b( ){2}"), f"an empty range touches the item at its offset: {stdout_text(result)!r}")
result = format_stdin(RANGE_SOURCE, "--range", f"{len(RANGE_SOURCE) + 50}:{len(RANGE_SOURCE) + 60}")
check(stdout_text(result) == RANGE_SOURCE, "a range past the end changes nothing")
result = format_stdin("use b\nuse a\n\nfn  f( ){1}\n", "--range", "0:3")
check(stdout_text(result) == "use b\nuse a\n\nfn  f( ){1}\n", f"a range does not sort `use` declarations: {stdout_text(result)!r}")

# --- Tricky sources: the result must be stable, keep every comment, and keep the program -------------------------------

TRICKY = {
    "empty file": "",
    "only comments": "// one\n\n\n// two\n/* three */\n",
    "only blank lines": "\n\n\n",
    "no final newline": "fn main() {}",
    "trailing white space": "fn main() {   \n    let x = 1   \n}   \n",
    "tabs": "fn main() {\n\tlet x = 1\n\tx\n}\n",
    "comment after open brace": "fn main() { // c\n    1\n}\n",
    "comment before closing brace": "fn main() {\n    1 // c\n    // d\n}\n",
    "comment between arguments": "fn main() {\n    f(1, // a\n      2, /* b */ 3)\n}\n",
    "comment before else": "fn main() {\n    if a {\n        1\n    } // c\n    else {\n        2\n    }\n}\n",
    "comment in empty struct": "struct S { // c\n}\n",
    "comment in empty fn": "fn f() {\n    // nothing\n}\n",
    "comment after arrow": "fn main() {\n    match x {\n        1 => // c\n            2,\n        else => 3\n    }\n}\n",
    "block comment between tokens": "fn /* a */ main /* b */ ( /* c */ ) /* d */ { /* e */ }\n",
    "multi-line block comment": "fn main() {\n    /* line one\n       line two */\n    1\n}\n",
    "nested generics": "fn f(x: Map<String, List<Option<i32>>>): Map<String, List<Option<i32>>> { x }\n",
    "shift and generics": "fn main() {\n    let a = 1 << 2 >> 3\n    let b = List<List<i32>>.new()\n}\n",
    "closure in call": "fn main() {\n    run(|a, b| a + b, |c| { c })\n}\n",
    "statement starting with a paren": "fn main() {\n    let a = f\n    (g)(a)\n}\n",
    "statement starting with ||": "fn main() {\n    let a = 1;\n    || a\n}\n",
    "match arms": "fn main() {\n    match x {\n        1 | 2 | 3 => a(),\n        n if n > 5 => { b() }\n        Some(v) => v,\n        else => 0,\n    }\n}\n",
    "long string": 'fn main() {\n    let s = "' + "x" * 150 + '"\n}\n',
    "string with braces": 'fn main() {\n    let s = "{a} // not a comment /* nor this */"\n}\n',
    "raw multi-line string": 'fn main() {\n    let s = "first\nsecond"\n}\n',
    "empty blocks": "fn f() {}\nstruct S {}\nenum E {}\nS {}\n",
    "doc comment on a variant": "enum E {\n    /// doc\n    A,\n    /// doc\n    B(i32),\n}\n",
    "very long chain": "fn main() {\n    let a = "
    + ".".join(["value"] + [f"method_number_{i}(argument_{i})" for i in range(12)])
    + "\n}\n",
    "deep nesting": "fn main() {\n    " + "f(" * 30 + "1" + ")" * 30 + "\n}\n",
    "many operators": "fn main() {\n    let a = "
    + " + ".join(f"term_number_{i}" for i in range(30))
    + "\n}\n",
    "use before comment": "use a\n// c\nuse b\n",
    "pub use only": "pub use b\npub use a\n",
    "parenthesized type with a comment": "fn main() {\n    let d: (// comment\n((((i32))))) = a\n}\n",
    "tuple field of an integer": "fn main() {\n    let a = 1.\n        2\n}\n",
    "comments after priv": "priv// a\n// b\n// c\nfn f() {}\n",
    "comment between = and value": "fn main() {\n    let total =\n        // note\n        b\n}\n",
    "two trailing line comments": "fn main() {\n    it.next( // one\n) // two\n}\n",
    "windows path in string": 'fn main() {\n    let s = "C:\\\\dir\\\\file"\n}\n',
}

for name, source in TRICKY.items():
    result = format_stdin(source)
    out = stdout_text(result)
    check(result.returncode == 0, f"tricky `{name}`: exit {result.returncode}: {stderr_text(result)[:200]}")
    if result.returncode != 0:
        continue
    again = format_stdin(out)
    check(stdout_text(again) == out, f"tricky `{name}`: not idempotent: {first_difference(out, stdout_text(again))}")
    check(not any(line != line.rstrip() for line in out.split("\n")), f"tricky `{name}`: trailing white space in the output")
    check(out == "" or (out.endswith("\n") and not out.endswith("\n\n")), f"tricky `{name}`: the output ends with one newline")
    check(all(len(line) <= 120 or '"' in line for line in out.split("\n")), f"tricky `{name}`: a line is wider than 120 columns")
    for comment in ("// one", "// two", "// note", "// a", "// b", "/* three */", "// c", "// d", "/* a */", "/* b */", "/* c */", "/* d */", "/* e */", "// a", "// nothing"):
        if comment in source:
            check(comment in out, f"tricky `{name}`: the comment {comment!r} was lost")
check(stdout_text(format_stdin("")) == "", "an empty file stays empty")
check(stdout_text(format_stdin("\n\n\n")) == "", "a file of blank lines becomes empty")

print(f"Checked burdin fmt ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
