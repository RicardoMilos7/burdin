#!/usr/bin/env python3
"""Checks diagnostics presentation and tooling (issues #434-#436, #642-#644). Needs a built driver.
Usage: tests/check_diagnostics.py
       BURDIN_UPDATE_GOLDEN=1 tests/check_diagnostics.py   rewrite the golden files in tests/diagnostics/

BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin is used.

What it checks:
  * default output is plain when stderr is not a terminal, and --color=always/--no-color/NO_COLOR behave as
    documented (an explicit --color always wins over NO_COLOR; the last color flag wins);
  * --width (and COLUMNS) wraps the human format, with no line over the width, and --width 0 does not wrap;
  * --diagnostic-format=short is one plain line per diagnostic, and --diagnostic-format=json is one JSON
    object per diagnostic plus a summary object;
  * a diagnostic's note and suggestion reach the human output (`= note:`, `= help:`) and the JSON output;
  * --error-limit prints the first N errors after sorting and reports how many were hidden;
  * the output is byte-for-byte deterministic between runs;
  * the flags parse as `--option value` and `--option=value`, and bad values exit 2 with a usage error;
  * the JSON format is version 1 (issue #642): every line the driver prints for every compile-fail conformance
    test validates against docs/diagnostic.schema.json, the schema rejects mutated objects, the multi-diagnostic
    fixture and the composite diagnostic match their golden files, columns count characters, bytes, and UTF-16
    code units, and `rendered` is the human text;
  * the SARIF format (issue #643) is one SARIF 2.1.0 log (a subset schema in tests/diagnostics), with the whole
    error-code registry and every lint as rules, results with related locations and fixes, a golden result list,
    an empty log for a clean `build` and `lint`, and nothing else on the stream; `fmt` refuses it;
  * every format links a diagnostic to the error reference (issue #644): the `url` of JSON, `helpUri` of SARIF,
    and an OSC 8 hyperlink in colored human output when hyperlinks are forced on (never without color).
"""

import json
import os
import re
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import harness
import jsonschema_lite
import run_tests

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


def check() -> None:
    global checks
    checks += 1


burdin = harness.require_burdin(
    2, "error: compiler driver not found. Build it first or set BURDIN=/path/to/burdin"
)

FIXTURES = harness.TESTS / "diagnostics"
SCHEMA = json.loads(harness.read_text(harness.ROOT / "docs" / "diagnostic.schema.json"))
SARIF_SCHEMA = json.loads(harness.read_text(FIXTURES / "sarif-subset.schema.json"))
UPDATE_GOLDEN = bool(os.environ.get("BURDIN_UPDATE_GOLDEN"))
# The stable root of the hosted reference (issue #644); `tests/check_error_docs.py` pins it too.
URL_BASE = "https://ricardomilos7.github.io/burdin"

DIAGNOSTIC = re.compile(rb"^(?:error|warning)\[[EL][0-9]{4}\]:")
SHORT_LINE = re.compile(r"^[^:]+:[0-9]+:[0-9]+: error\[E[0-9]{4}\]: ")
with harness.temporary_dir("burdin-diagnostics-") as work:
    harness.write_text(
        work / "two_errors.bd",
        'fn main() {\n    let a: i32 = "x"\n    let b: i32 = "y"\n}\n',
    )
    harness.write_text(
        work / "long_message.bd",
        "fn main() {\n    this_identifier_is_far_too_long_to_fit_on_one_terminal_line(1)\n}\n",
    )
    harness.write_text(
        work / "suggestion.bd",
        "struct Counter {\n"
        "    n: i32\n"
        "}\n"
        "\n"
        "fn bump(mut c: Counter) {\n"
        "    c.n = c.n + 1\n"
        "}\n"
        "\n"
        "fn main() {\n"
        "    let mut c = Counter { n: 0 }\n"
        "    bump(c)\n"
        "}\n",
    )
    harness.write_text(
        work / "note.bd",
        "struct Thing {\n"
        "    id: i32\n"
        "}\n"
        "\n"
        "fn take(own t: Thing) {}\n"
        "\n"
        "fn main() {\n"
        "    let t = Thing { id: 1 }\n"
        "    take(t)\n"
        "    let u = t\n"
        "}\n",
    )
    harness.write_text(work / "run_bad.bd", 'fn main() {\n    let n: i32 = "x"\n}\n')

    status = 0
    stdout = ""
    stderr = ""
    stderr_raw = b""

    def execute(args, env=None) -> None:
        global status, stdout, stderr, stderr_raw
        result = harness.run(args, cwd=work, env=env)
        status = result.returncode
        stdout = harness.text(result.stdout).rstrip("\n")
        stderr_raw = result.stderr
        stderr = harness.text(result.stderr).rstrip("\n")

    def drive(*args, env=None) -> None:
        execute([burdin, *args], env=env)

    def expect_status(description: str, expected: int) -> None:
        check()
        if status != expected:
            add_problem(f"{description}: exit status {status}, expected {expected}")

    def expect_contains(description: str, needle: str, haystack: str) -> None:
        check()
        if needle not in haystack:
            add_problem(f"{description}: output does not contain: {needle}")

    def stderr_lines():
        lines = harness.text(stderr_raw).split("\n")
        if lines and lines[-1] == "":
            lines.pop()
        return lines

    def diagnostic_line_list():
        lines = stderr_lines()
        end = next(
            (index for index, line in enumerate(lines) if line.startswith("error: could not compile")),
            None,
        )
        if end is not None:
            return lines[:end]
        return lines[:-1] if lines else []

    def diagnostic_lines() -> str:
        return "\n".join(diagnostic_line_list())

    def expect_no_color(description: str) -> None:
        check()
        if b"\x1b[" in stderr_raw:
            add_problem(f"{description}: output contains ANSI color")

    def expect_color(description: str) -> None:
        check()
        if b"\x1b[" not in stderr_raw:
            add_problem(f"{description}: output has no ANSI color")

    # --- Default output and color (issue #434) -----------------------------------

    drive("build", "two_errors.bd")
    expect_status("default output", 1)
    expect_no_color("piped stderr")
    check()
    count = sum(1 for line in stderr_lines() if DIAGNOSTIC.search(line.encode()))
    if count != 2:
        add_problem(f"default output: {count} diagnostic lines, expected 2")
    expect_contains("default output", "error: could not compile two_errors.bd (2 errors)", stderr)
    expect_contains("default output", "burdin explain", stderr)

    drive("build", "two_errors.bd", "--color=always")
    expect_status("--color=always", 1)
    expect_color("--color=always")

    drive("build", "two_errors.bd", "--color", "never")
    expect_status("--color never", 1)
    expect_no_color("--color never")

    drive("build", "two_errors.bd", "--no-color")
    expect_status("--no-color", 1)
    expect_no_color("--no-color")

    drive("build", "two_errors.bd", "--color=always", "--no-color")
    expect_no_color("the last color flag wins (no-color last)")

    drive("build", "two_errors.bd", "--no-color", "--color=always")
    expect_color("the last color flag wins (always last)")

    drive("build", "two_errors.bd", "--color=always", env={**os.environ, "NO_COLOR": "1"})
    expect_color("NO_COLOR yields to an explicit --color=always")

    drive("build", "two_errors.bd", env={**os.environ, "NO_COLOR": "1"})
    expect_no_color("NO_COLOR with the default color choice")

    # --- Wrapping (issue #434) ---------------------------------------------------

    drive("build", "long_message.bd", "--width", "60")
    expect_status("--width", 1)
    check()
    if any(len(line) > 60 for line in diagnostic_line_list()):
        add_problem("--width 60: a diagnostic line is wider than 60 characters")
    check()
    if not any(line.startswith("    ") for line in stderr_lines()):
        add_problem("--width 60: no continuation line is indented")
    check()
    # Wrapping only breaks at whitespace or inside a word; removing the newlines and indentation reconstructs the
    # message, including the long identifier.
    joined = "".join(
        line[4:] if line.startswith("    ") else line for line in diagnostic_line_list()
    )
    if "this_identifier_is_far_too_long_to_fit_on_one_terminal_line" not in joined:
        add_problem("--width 60: the wrapped message does not contain the identifier")

    drive("build", "long_message.bd", "--width", "0")
    check()
    if "this_identifier_is_far_too_long_to_fit_on_one_terminal_line" not in diagnostic_line_list()[0]:
        add_problem(f"--width 0: the header message was wrapped: {diagnostic_line_list()[0]!r}")

    drive("build", "long_message.bd", env={**os.environ, "COLUMNS": "60"})
    check()
    if any(len(line) > 60 for line in diagnostic_line_list()):
        add_problem("COLUMNS=60: a diagnostic line is wider than 60 characters")

    # --- Short and JSON formats (issues #434, #435) ------------------------------

    drive("build", "suggestion.bd", "--diagnostic-format=short")
    expect_status("--diagnostic-format=short", 1)
    expect_no_color("short format ignores color")
    check()
    if sum(1 for line in stderr_lines() if SHORT_LINE.match(line)) != 1:
        add_problem("short format: not one diagnostic line")
    check()
    if "= help:" in stderr:
        add_problem("short format: contains a help line")
    check()
    for line in stderr_lines():
        if "error[" in line and not SHORT_LINE.match(line):
            add_problem("short format: a diagnostic line does not match path:LINE:COL: error[CODE]: message")
            break

    drive("build", "two_errors.bd", "--diagnostic-format=json")
    expect_status("--diagnostic-format=json", 1)
    check()
    if any(not re.fullmatch(r"\{.*\}", line) for line in stderr_lines()):
        add_problem("json format: a line is not a JSON object")
    check()
    if sum(1 for line in stderr_lines() if '"type":"diagnostic"' in line) != 2:
        add_problem("json format: expected two diagnostics")
    expect_contains("json format", '"code":"E0003"', stderr)
    expect_contains(
        "json format", '"span":{"file":"two_errors.bd","line":2,"column":18', stderr
    )
    expect_contains("json format", '"type":"summary","version":1,"errors":2,"shown":2,"limit":null', stderr)

    drive("build", "two_errors.bd", "--diagnostic-format=json", "--color=always")
    expect_no_color("json format ignores color")

    drive("build", "suggestion.bd", "--diagnostic-format=json")
    expect_status("json suggestion", 1)
    expect_contains(
        "json suggestion", '"suggestions":[{"message":"add `mut` before this argument"', stderr
    )
    expect_contains("json suggestion", '"replacement":"mut "', stderr)

    drive("build", "suggestion.bd", "--no-color")
    expect_contains("human suggestion", "help: add `mut` before this argument", stderr)

    drive("build", "note.bd", "--diagnostic-format=json")
    expect_status("json note", 1)
    # The move note is part of the value's path: its declaration, then the move itself (issue #593).
    expect_contains(
        "json note", '"message":"value moved here","span":{"file":"note.bd"', stderr
    )
    expect_contains("json note", '"message":"`t` is declared here"', stderr)

    drive("build", "note.bd", "--no-color")
    expect_contains("human note", "--> note.bd:", stderr)
    expect_contains("human note", "value moved here", stderr)

    # --- Error limit and ordering (issue #436) -----------------------------------

    drive("build", "two_errors.bd", "--error-limit", "1")
    expect_status("--error-limit 1", 1)
    check()
    if sum(1 for line in stderr_lines() if DIAGNOSTIC.search(line.encode())) != 1:
        add_problem("--error-limit 1: more than one diagnostic line")
    expect_contains("--error-limit 1", "note: 1 more error not shown (--error-limit 1)", stderr)
    expect_contains("--error-limit 1", "error: could not compile two_errors.bd (2 errors)", stderr)

    drive("build", "two_errors.bd", "--error-limit", "0")
    check()
    if sum(1 for line in stderr_lines() if DIAGNOSTIC.search(line.encode())) != 2:
        add_problem("--error-limit 0: did not print every diagnostic")
    check()
    if "not shown" in stderr:
        add_problem("--error-limit 0: reported hidden errors")

    drive("build", "two_errors.bd", "--error-limit", "5")
    check()
    if "not shown" in stderr:
        add_problem("--error-limit 5: reported hidden errors")

    drive("build", "two_errors.bd", "--diagnostic-format=short")
    first = stderr_raw
    drive("build", "two_errors.bd", "--diagnostic-format=short")
    check()
    if first != stderr_raw:
        add_problem("determinism: two runs of the same input differ")

    # The diagnostics are sorted by position even though the checker may report them in another order.
    drive("build", "two_errors.bd", "--diagnostic-format=short")
    check()
    numbers = ""
    for line in stderr_lines():
        match = re.search(r":([0-9]+):[0-9]+: error\[E[0-9]{4}\]", line)
        if match:
            numbers += match.group(1) + " "
    if numbers != "2 3 ":
        add_problem(f"ordering: diagnostic lines are {numbers}, expected 2 3")

    # --- Flag syntax and usage errors -------------------------------------------

    drive("build", "two_errors.bd", "--diagnostic-format", "xml")
    expect_status("unknown diagnostic format", 2)
    expect_contains("unknown diagnostic format", "unknown diagnostic format", stderr)

    drive("build", "two_errors.bd", "--color", "sparkles")
    expect_status("unknown color choice", 2)

    drive("build", "two_errors.bd", "--width", "wide")
    expect_status("bad width", 2)

    drive("build", "two_errors.bd", "--error-limit", "-1")
    expect_status("bad error limit", 2)

    drive("build", "two_errors.bd", "--release=yes")
    expect_status("a value on --release", 2)

    # `run` accepts the same options and still exits 1 on a compilation failure.
    drive("run", "run_bad.bd", "--no-color", "--error-limit", "1")
    expect_status("run with diagnostic options", 1)
    expect_no_color("run with diagnostic options")
    expect_contains(
        "run with diagnostic options",
        "error: could not compile run_bad.bd (1 error)",
        harness.text(stderr_raw),
    )

    # --- JSON format version 1 (issue #642) -----------------------------------------

    def json_objects(description: str, text_: str, ignore_foreign: bool = False):
        """The objects of a JSON-lines stream; a line that is not JSON is a problem."""
        objects = []
        for number, line in enumerate(text_.split("\n"), 1):
            # A line that is not an object is the output of something the build ran (a build script prints its own).
            if not line or (ignore_foreign and not line.startswith("{")):
                continue
            check()
            try:
                objects.append(json.loads(line))
            except ValueError as error:
                add_problem(f"{description}: line {number} is not JSON ({error}): {line[:120]}")
        return objects

    def validate_objects(description: str, objects) -> None:
        for obj in objects:
            check()
            for problem in jsonschema_lite.validate(obj, SCHEMA):
                add_problem(f"{description}: {problem}")
            if obj.get("type") == "diagnostic":
                check_diagnostic_invariants(description, obj)

    def locations_of(obj):
        """Every location object inside a diagnostic, with its path for messages."""
        found = [("span", obj["span"])]
        for index, label in enumerate(obj["labels"]):
            found.append((f"labels[{index}].span", label["span"]))
        for index, note in enumerate(obj["notes"]):
            if note["span"] is not None:
                found.append((f"notes[{index}].span", note["span"]))
        for index, related in enumerate(obj["related"]):
            found.append((f"related[{index}].span", related["span"]))
        for index, suggestion in enumerate(obj["suggestions"]):
            for edit_index, edit in enumerate(suggestion["edits"]):
                found.append((f"suggestions[{index}].edits[{edit_index}].span", edit["span"]))
        for index, child in enumerate(obj["children"]):
            found.append((f"children[{index}].span", child["span"]))
        return found

    def check_diagnostic_invariants(description: str, obj) -> None:
        """What the schema cannot say: the numbers agree with each other and with the human text."""
        check()
        prefix = f"{obj['severity']}[{obj['code']}]: "
        location = f"--> {obj['span']['file']}:{obj['span']['line']}:{obj['span']['column']}"
        if not obj["rendered"].startswith(prefix) or location not in obj["rendered"]:
            add_problem(
                f"{description}: rendered text of {obj['code']} does not start with {prefix!r} and contain {location!r}"
            )
        for where, span in locations_of(obj):
            check()
            if span["byte_end"] < span["byte_start"] or (span["end_line"], span["end_column"]) < (
                span["line"],
                span["column"],
            ):
                add_problem(f"{description}: {obj['code']} {where} ends before it starts")
            if span["utf8_column"] < span["column"] or span["utf16_column"] < span["column"]:
                add_problem(f"{description}: {obj['code']} {where} counts fewer bytes or UTF-16 units than characters")
        expected_url = None
        if obj["code"] != "E9000":
            kind = "errors" if obj["code"].startswith("E") else "lints"
            expected_url = f"{URL_BASE}/{kind}/{obj['code']}"
        if obj["url"] != expected_url:
            add_problem(f"{description}: {obj['code']} has url {obj['url']!r}, expected {expected_url!r}")

    def fixture(*args, env=None):
        result = harness.run([burdin, *args], cwd=FIXTURES, env=env)
        return result.returncode, harness.text(harness.normalize(result.stderr)), harness.text(
            harness.normalize(result.stdout)
        )

    def golden(description: str, name: str, actual: str) -> None:
        path = FIXTURES / name
        check()
        if UPDATE_GOLDEN:
            path.write_bytes(actual.encode("utf-8"))
            return
        if not path.is_file():
            add_problem(f"{description}: missing golden file tests/diagnostics/{name} (BURDIN_UPDATE_GOLDEN=1 writes it)")
        elif harness.read_text(path) != actual:
            add_problem(
                f"{description}: output differs from tests/diagnostics/{name}; if the change is intended, update "
                "docs/diagnostics.md and the schema, then run with BURDIN_UPDATE_GOLDEN=1"
            )

    # The fixture has a multi-span diagnostic (a note that points at the move), a suggestion (the missing `mut`),
    # and a column after non-ASCII text.
    code, err, _ = fixture("build", "multi.bd", "--diagnostic-format=json")
    check()
    if code != 1:
        add_problem(f"multi.bd: exit status {code}, expected 1")
    golden("json golden for multi.bd", "multi.golden.json", err)
    multi = json_objects("multi.bd json", err)
    validate_objects("multi.bd json", multi)
    check()
    kinds = [obj["type"] for obj in multi]
    if kinds != ["diagnostic", "diagnostic", "diagnostic", "summary"]:
        add_problem(f"multi.bd json: object types are {kinds}")
    else:
        moved, mut_marker, mismatch, _summary = multi
        check()
        if not (moved["code"] == "E0100" and len(moved["notes"]) >= 2 and len(moved["related"]) >= 2):
            add_problem("multi.bd json: the move error has no note with a span")
        check()
        if not (
            mut_marker["code"] == "E0114"
            and mut_marker["suggestions"][0]["applicability"] == "machine"
            and mut_marker["suggestions"][0]["edits"][0]["replacement"] == "mut "
        ):
            add_problem("multi.bd json: the missing `mut` marker has no machine fix")
        span = mismatch["span"]
        check()
        # `"añ𝄞"` is 1 + 1 + 2 + 4 + 1 characters wide but 9 bytes and 6 UTF-16 units wide.
        if (span["end_column"] - span["column"], span["byte_end"] - span["byte_start"]) != (5, 9) or (
            span["end_utf8_column"] - span["utf8_column"],
            span["end_utf16_column"] - span["utf16_column"],
        ) != (9, 6):
            add_problem(f"multi.bd json: the string's columns are wrong: {span}")

    # The composite diagnostic (a unit test in burdin_diag builds it) uses every field of the format.
    composite = json_objects("composite golden", harness.read_text(FIXTURES / "composite.golden.json"))
    validate_objects("composite golden", composite)
    check()
    if len(composite) != 1 or not all(
        composite[0][field]
        for field in ("labels", "notes", "help", "related", "children", "suggestions")
    ):
        add_problem("composite golden: it must use labels, notes, help, related, children, and suggestions")

    # The schema is not vacuous: each mutation of a valid object must be rejected.
    valid = json.loads(json.dumps(multi[0]))
    mutations = {
        "a missing field": lambda o: o.pop("url"),
        "an extra field": lambda o: o.update(surprise=1),
        "another version": lambda o: o.update(version=2),
        "a lowercase code": lambda o: o.update(code="e0100"),
        "a bad severity": lambda o: o.update(severity="fatal"),
        "a span without UTF-16 columns": lambda o: o["span"].pop("utf16_column"),
        "a zero line": lambda o: o["span"].update(line=0),
        "a note without a message": lambda o: o["notes"][0].pop("message"),
        "a suggestion without edits": lambda o: o["suggestions"].append({"message": "x", "applicability": "manual", "edits": []}),
        "a url outside the reference": lambda o: o.update(url="https://example.test/E0100"),
    }
    for name, mutate in mutations.items():
        broken = json.loads(json.dumps(valid))
        mutate(broken)
        check()
        if not jsonschema_lite.validate(broken, SCHEMA):
            add_problem(f"schema: {name} was accepted")
    for name, stream in {"a summary without a limit": {"type": "summary", "version": 1, "errors": 1, "shown": 1}}.items():
        check()
        if not jsonschema_lite.validate(stream, SCHEMA):
            add_problem(f"schema: {name} was accepted")

    # Every compile-fail conformance test: the JSON stream validates against the schema (issue #642).
    cases = []
    seen = set()
    for path in run_tests.discover(harness.TESTS):
        lines = run_tests.normalize(path.read_bytes()).split(b"\n")
        directives = [line[4:] for line in lines if line.startswith(b"//@ ")]
        kind = run_tests.kind_of(directives)
        if kind not in ("compile-fail", "test-compile-fail"):
            continue
        target = run_tests.target_of(path, harness.TESTS)
        mode = "test" if kind == "test-compile-fail" else "build"
        if (target, mode) not in seen:
            seen.add((target, mode))
            cases.append((target, mode, path.relative_to(harness.TESTS).as_posix()))

    def conformance_json(case):
        target, mode, rel = case
        with harness.temporary_dir("burdin-diagnostics-json-") as scratch:
            result = harness.run(
                [burdin, mode, str(target), "--diagnostic-format=json"], cwd=scratch, timeout=60
            )
        return rel, result.returncode, harness.text(harness.normalize(result.stderr))

    validated = 0
    with ThreadPoolExecutor(max_workers=min(8, os.cpu_count() or 1)) as pool:
        for rel, returncode, stream in pool.map(conformance_json, cases):
            objects = json_objects(rel, stream, ignore_foreign=True)
            diagnostics = [obj for obj in objects if obj.get("type") == "diagnostic"]
            check()
            if returncode == 0 or not diagnostics:
                add_problem(f"{rel}: expected a failing compilation with JSON diagnostics (exit {returncode})")
                continue
            validate_objects(rel, objects)
            validated += len(diagnostics)
    print(f"Validated {validated} JSON diagnostics from {len(cases)} compile-fail tests against the schema")

    # --- SARIF format (issue #643) ----------------------------------------------------

    def sarif_log(description: str, text_: str):
        """The one SARIF log on the stream, validated; None when the stream is not exactly one JSON document."""
        check()
        try:
            log = json.loads(text_)
        except ValueError as error:
            add_problem(f"{description}: the stream is not one JSON document ({error}): {text_[:200]!r}")
            return None
        for problem in jsonschema_lite.validate(log, SARIF_SCHEMA):
            add_problem(f"{description}: {problem}")
        check()
        if len(log.get("runs", [])) != 1:
            add_problem(f"{description}: expected exactly one run")
            return None
        rules = log["runs"][0]["tool"]["driver"]["rules"]
        for index, result in enumerate(log["runs"][0]["results"]):
            check()
            if "ruleIndex" in result and rules[result["ruleIndex"]]["id"] != result["ruleId"]:
                add_problem(f"{description}: result {index} has a ruleIndex that points at another rule")
            if not any(rule["id"] == result["ruleId"] for rule in rules):
                add_problem(f"{description}: result {index} names a rule the run does not define")
        return log

    code, err, _ = fixture("build", "multi.bd", "--diagnostic-format=sarif")
    check()
    if code != 1:
        add_problem(f"sarif build failure: exit status {code}, expected 1")
    log = sarif_log("sarif multi.bd", err)
    if log is not None:
        run = log["runs"][0]
        driver = run["tool"]["driver"]
        # The golden keeps the results; the version and the rules change with the build and the registry, so the
        # structural checks below cover them.
        normalized = json.loads(json.dumps(log))
        normalized["runs"][0]["tool"]["driver"]["version"] = "VERSION"
        normalized["runs"][0]["tool"]["driver"]["rules"] = "RULES"
        for result in normalized["runs"][0]["results"]:
            result.pop("ruleIndex", None)
        golden("sarif golden for multi.bd", "multi.golden.sarif", json.dumps(normalized, indent=2, ensure_ascii=False) + "\n")
        check()
        if run["invocations"][0]["executionSuccessful"] is not False:
            add_problem("sarif multi.bd: a failed build must say executionSuccessful false")
        results = run["results"]
        check()
        if [r["ruleId"] for r in results] != ["E0100", "E0114", "E0003"]:
            add_problem(f"sarif multi.bd: results are {[r['ruleId'] for r in results]}")
        else:
            check()
            if len(results[0].get("relatedLocations", [])) < 2:
                add_problem("sarif multi.bd: the move error's note is not a related location")
            fix_text = results[1].get("fixes", [{}])[0]
            replacement = fix_text.get("artifactChanges", [{}])[0].get("replacements", [{}])[0]
            check()
            if replacement.get("insertedContent", {}).get("text") != "mut " or replacement.get("deletedRegion", {}).get("byteLength") != 0:
                add_problem(f"sarif multi.bd: the `mut` fix is not an insertion of `mut `: {fix_text}")
            region = results[2]["locations"][0]["physicalLocation"]["region"]
            check()
            if (region["startColumn"], region["endColumn"], region["byteLength"]) != (18, 23, 9):
                add_problem(f"sarif multi.bd: the string region is wrong: {region}")
        # Every code of the registry and of the lint reference is a rule, with a link to its entry.
        registry = sorted(
            {m.group(1) for line in harness.read_text(harness.TESTS / "README.md").split("\n")
             for m in [re.match(r"^\| (E[0-9]{4}) \|", line)] if m}
        )
        rule_ids = [rule["id"] for rule in driver["rules"]]
        check()
        missing = [code_ for code_ in registry if code_ not in rule_ids]
        if missing:
            add_problem(f"sarif rules: the registry codes {' '.join(missing)} are not rules")
        check()
        if len(set(rule_ids)) != len(rule_ids):
            add_problem("sarif rules: a rule id repeats")
        for rule in driver["rules"]:
            check()
            expected = f"{URL_BASE}/{'errors' if rule['id'].startswith('E') else 'lints'}/{rule['id']}"
            if rule.get("helpUri") != expected:
                add_problem(f"sarif rules: {rule['id']} has helpUri {rule.get('helpUri')!r}, expected {expected}")
        listing = harness.text(harness.run([burdin, "lint", "--list"], cwd=FIXTURES).stdout)
        lint_codes = re.findall(r"^(L[0-9]{4})\b", listing, re.MULTILINE)
        check()
        if not lint_codes or any(code_ not in rule_ids for code_ in lint_codes):
            add_problem(f"sarif rules: the lints {lint_codes} are not all rules")
        check()
        if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+.*", driver["version"]):
            add_problem(f"sarif tool version is {driver['version']!r}")

    # The error limit keeps the log valid and records what it left out.
    code, err, _ = fixture("build", "multi.bd", "--diagnostic-format=sarif", "--error-limit", "1")
    log = sarif_log("sarif --error-limit", err)
    check()
    if log is not None and (len(log["runs"][0]["results"]) != 1 or log["runs"][0].get("properties") != {"diagnosticsNotShown": 2}):
        add_problem("sarif --error-limit 1: the log does not show one result and two hidden")

    # Nothing else is on the stream, whatever the failure: no summary line, no hint, no color.
    drive("build", "two_errors.bd", "--diagnostic-format=sarif", "--color=always")
    expect_status("sarif with --color=always", 1)
    expect_no_color("sarif ignores color")
    sarif_log("sarif two_errors.bd", stderr)

    harness.write_text(work / "clean.bd", "fn main() {}\n")
    harness.write_text(
        work / "warning.bd",
        "struct Gadget { n: i32 }\n\nGadget {\n    fn label(self): String {\n        String.from(\"method\")\n    }\n}\n\n"
        "fn Gadget.label(self): String {\n    String.from(\"extension\")\n}\n\nfn main() {\n    let g = Gadget { n: 1 }\n    println(g.label())\n}\n",
    )
    drive("build", "clean.bd", "-o", str(work / "clean-exe"), "--diagnostic-format=sarif")
    expect_status("sarif clean build", 0)
    log = sarif_log("sarif clean build", stderr)
    check()
    if log is not None and (log["runs"][0]["results"] != [] or log["runs"][0]["invocations"][0]["executionSuccessful"] is not True):
        add_problem("sarif clean build: expected an empty, successful log")
    drive("build", "warning.bd", "-o", str(work / "warning-exe"), "--diagnostic-format=sarif")
    expect_status("sarif warning-only build", 0)
    log = sarif_log("sarif warning-only build", stderr)
    check()
    if log is not None and [(r["ruleId"], r["level"]) for r in log["runs"][0]["results"]] != [("E0215", "warning")]:
        add_problem("sarif warning-only build: expected one E0215 warning")
    check()
    if log is not None and log["runs"][0]["invocations"][0]["executionSuccessful"] is not True:
        add_problem("sarif warning-only build: a warning does not fail the build")
    # `run` keeps its own streams: a clean program prints no log.
    drive("run", "clean.bd", "--diagnostic-format=sarif")
    expect_status("sarif clean run", 0)
    check()
    if stderr != "":
        add_problem(f"sarif clean run: stderr is not empty: {stderr[:200]!r}")

    # `burdin lint` writes a log for a clean package, for findings that warn, and for findings that deny.
    harness.write_text(work / "lint_clean.bd", "fn main() {}\n")
    harness.write_text(work / "lint_unused.bd", "fn main() {\n    let unused = 1\n}\n")
    drive("lint", "lint_clean.bd", "--diagnostic-format=sarif")
    expect_status("sarif lint, clean", 0)
    log = sarif_log("sarif lint, clean", stderr)
    check()
    if log is not None and log["runs"][0]["results"] != []:
        add_problem("sarif lint, clean: expected no results")
    drive("lint", "lint_unused.bd", "--diagnostic-format=sarif")
    expect_status("sarif lint, a warning", 0)
    log = sarif_log("sarif lint, a warning", stderr)
    if log is not None:
        results = log["runs"][0]["results"]
        check()
        if [(r["ruleId"], r["level"]) for r in results] != [("L0007", "warning")]:
            add_problem(f"sarif lint, a warning: results are {[(r['ruleId'], r['level']) for r in results]}")
        elif "fixes" not in results[0] or results[0]["properties"].get("fixIsMachineApplicable") != [True]:
            add_problem("sarif lint, a warning: the safe fix is not a machine-applicable fix")
    drive("lint", "lint_unused.bd", "-D", "unused_variables", "--diagnostic-format=sarif")
    expect_status("sarif lint, a denied lint", 1)
    log = sarif_log("sarif lint, a denied lint", stderr)
    check()
    if log is not None and (
        [r["level"] for r in log["runs"][0]["results"]] != ["error"]
        or log["runs"][0]["invocations"][0]["executionSuccessful"] is not False
    ):
        add_problem("sarif lint, a denied lint: expected one error and an unsuccessful invocation")

    # The JSON format validates for lint too (the lint summary is part of the schema).
    drive("lint", "lint_unused.bd", "--diagnostic-format=json")
    objects = json_objects("lint json", stderr)
    validate_objects("lint json", objects)
    check()
    if [obj.get("type") for obj in objects] != ["diagnostic", "lint-summary"]:
        add_problem(f"lint json: object types are {[obj.get('type') for obj in objects]}")

    # `fmt` reports file by file, so it cannot write one log.
    drive("fmt", "--check", "clean.bd", "--diagnostic-format=sarif")
    expect_status("fmt refuses sarif", 2)
    expect_contains("fmt refuses sarif", "does not support `--diagnostic-format sarif`", stderr)

    # --- Links to the error reference (issue #644) -------------------------------------

    drive("explain", "E0003")
    check()
    if f"Online: {URL_BASE}/errors/E0003" not in stdout:
        add_problem("burdin explain E0003 does not print the link to the reference")

    link = re.compile(r"\x1b\]8;;([^\x1b]*)\x1b\\")
    drive("build", "two_errors.bd", "--color=always", env={**os.environ, "FORCE_HYPERLINK": "1"})
    check()
    opening = [url for url in link.findall(harness.text(stderr_raw)) if url]
    if opening != [f"{URL_BASE}/errors/E0003"] * 2:
        add_problem(f"hyperlinks: expected two links to E0003, found {opening}")
    check()
    if harness.text(stderr_raw).count("\x1b]8;;\x1b\\") != 2:
        add_problem("hyperlinks: each link must be closed")
    linked = harness.text(stderr_raw)
    drive("build", "two_errors.bd", "--no-color")
    plain = stderr
    check()
    stripped = re.sub(r"\x1b\[[0-9;]*m", "", re.sub(r"\x1b\]8;;[^\x1b]*\x1b\\", "", linked))
    if stripped.rstrip("\n") != plain.rstrip("\n"):
        add_problem("hyperlinks: removing the escape sequences does not give the plain text")
    for description, args, env in (
        ("hyperlinks forced off", ["--color=always"], {"FORCE_HYPERLINK": "0"}),
        ("no color, hyperlinks forced on", ["--no-color"], {"FORCE_HYPERLINK": "1"}),
        ("json, hyperlinks forced on", ["--color=always", "--diagnostic-format=json"], {"FORCE_HYPERLINK": "1"}),
        ("short, hyperlinks forced on", ["--color=always", "--diagnostic-format=short"], {"FORCE_HYPERLINK": "1"}),
    ):
        drive("build", "two_errors.bd", *args, env={**os.environ, **env})
        check()
        if "\x1b]8" in harness.text(stderr_raw):
            add_problem(f"{description}: the output has a hyperlink")
    # The short format is exactly one line per diagnostic, so it has no link; the human footer names `burdin explain`.
    drive("build", "two_errors.bd", "--diagnostic-format=short")
    check()
    if "http" in stderr:
        add_problem("short format: contains a URL")

    # --- Human snippet golden test (issues #287, #1046) ---------------------------------
    human_golden = harness.TESTS / "diagnostics" / "human.golden.txt"
    human_res = harness.run([burdin, "check", "tests/diagnostics/multi.bd", "--no-color", "--width", "80"], cwd=harness.ROOT, merge=False)
    human_actual = harness.text(harness.normalize(human_res.stderr)).replace("\\", "/")
    check()
    if os.environ.get("BURDIN_UPDATE_GOLDEN") == "1":
        harness.write_text(human_golden, human_actual)
    elif not human_golden.is_file():
        add_problem(f"missing {human_golden}")
    else:
        human_expected = harness.read_text(human_golden)
        if human_actual != human_expected:
            diff = "\n".join(difflib.unified_diff(human_expected.splitlines(), human_actual.splitlines(), fromfile="expected", tofile="actual", lineterm=""))
            add_problem(f"human.golden.txt differs:\n{diff}")

print(f"Checked {checks} diagnostics behaviors from {work}")
if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
