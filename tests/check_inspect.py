#!/usr/bin/env python3
"""Checks the inspection commands (stage 3, issue #595): `burdin expand`, `--emit`, `burdin asm`, `burdin bloat`, and
`burdin check --print-instances`. Needs a built driver.
Usage: tests/check_inspect.py [--bless]

BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin is used.

What it checks (spec/inspect.md has the rules, INS1-INS19):
  * `burdin expand` (INS1-INS6): the golden expansion of fixtures from m17_arguments, m19_operators, m20_comptime, and
    m21_reflect and of tests/inspect/expand_mix.bd (`--bless` rewrites the goldens from the driver's output; review
    the diff before committing it); `--item`; `--instances`; a package target; the failure and usage paths;
  * the contract that makes expansion trustworthy: the expansion of every conformance program that builds in those four
    folders goes back through `burdin check` with no diagnostics, and a program that runs prints the same thing
    after expansion as before;
  * `--emit` (INS7-INS8): each kind writes its file with the right format, a bad kind or `--intel` without `--emit asm`
    is a usage error;
  * `burdin asm` (INS9-INS11): the function list, an instance, the LLVM IR form, the Intel dialect, and every refusal;
  * `burdin check --print-instances` (INS12-INS15): the human report and the JSON report against goldens;
  * `burdin bloat` (INS16-INS19): the sections of the report, its JSON form, `--top`, and the failure paths.
"""

import json
import os
import platform
import re
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import harness

problems = []
checks = 0

INSPECT = harness.TESTS / "inspect"
GOLDEN = INSPECT / "golden"
BLESS = "--bless" in sys.argv[1:]
COMMON = ["--no-color", "--diagnostic-format=short"]

# Conformance programs whose expansion is a golden file: (folder under tests/, name).
EXPAND_GOLDENS = [
    ("m17_arguments", "default_values"),
    ("m17_arguments", "named_arguments"),
    ("m19_operators", "arith_ops"),
    ("m19_operators", "unary_ops"),
    ("m19_operators", "vector_add_index"),
    ("m20_comptime", "comptime_expr"),
    ("m20_comptime", "comptime_for_unroll"),
    ("m20_comptime", "comptime_if_dead_branch"),
    ("m20_comptime", "const_values"),
    ("m21_reflect", "derive_describe"),
    ("m21_reflect", "reflect_members"),
    ("m21_reflect", "computed_field_access"),
]
# The folders whose every program is expanded, checked again, and compared by what it prints.
EXPAND_FOLDERS = ["m17_arguments", "m19_operators", "m20_comptime", "m21_reflect"]


def add_problem(message: str) -> None:
    problems.append(message)


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        add_problem(message)


burdin = harness.require_burdin(
    2, "error: compiler driver not found. Build it first or set BURDIN=/path/to/burdin"
)


def driver(args, cwd=INSPECT, merge=False, timeout=600):
    """Runs `burdin <args>`; the standard output, the standard error, and the exit status (carriage returns removed)."""
    result = harness.run([burdin, *args], cwd=cwd, merge=merge, timeout=timeout)
    out = harness.text(harness.normalize(result.stdout))
    err = "" if merge else harness.text(harness.normalize(result.stderr))
    return out, err, result.returncode


def golden(name: str, actual: str, label: str) -> None:
    """Compares `actual` with tests/inspect/golden/<name>; with --bless writes it instead."""
    path = GOLDEN / name
    if BLESS:
        path.parent.mkdir(parents=True, exist_ok=True)
        harness.write_text(path, actual)
        return
    if not path.is_file():
        add_problem(f"{label}: missing golden file {path.relative_to(harness.TESTS)} (run with --bless)")
        return
    expected = harness.read_text(path)
    check(actual == expected, f"{label}: output differs from {path.relative_to(harness.TESTS)}\n--- expected\n{expected}\n--- actual\n{actual}")


# --------------------------------------------------------------------------------------------------------------------
# burdin expand


def expand_target(folder: str, name: str) -> Path:
    return harness.TESTS / folder / f"{name}.bd"


def check_expand_goldens() -> None:
    for folder, name in EXPAND_GOLDENS:
        out, err, code = driver(["expand", f"../{folder}/{name}.bd"])
        label = f"expand {folder}/{name}"
        check(code == 0 and err == "", f"{label}: exit {code}, stderr: {err}")
        golden(f"expand/{folder}__{name}.golden", out, label)
    out, err, code = driver(["expand", "expand_mix.bd"])
    check(code == 0 and err == "", f"expand expand_mix.bd: exit {code}, stderr: {err}")
    golden("expand/expand_mix.golden", out, "expand expand_mix.bd")
    # The rewrites the fixture exists to show (INS2, INS3, INS5).
    for needle, why in [
        ("a.add(b)", "an operator is the interface method call (INS3)"),
        ('greet("world", 5)', "named and default arguments are listed in parameter order (INS3)"),
        ("(64 as usize)", "a `comptime` expression is its value (INS2)"),
        ("n * 2", "a `comptime if` is the branch it chose (INS2)"),
        ('"hi".shout()', "an extension call stays a method call: extensions are not values (INS3)"),
        ("derives (Clone, Eq)", "a built-in derive stays (INS5)"),
    ]:
        check(needle in out, f"expand expand_mix.bd: {why}: `{needle}` missing")
    for needle in ["comptime SIZE", "comptime if", "a + b", "greet(times: 5)"]:
        check(needle not in out, f"expand expand_mix.bd: `{needle}` should be expanded away")


def check_expand_reparse() -> None:
    """The expansion of every conformance program that builds goes back through `burdin check`, and a program that runs
    prints the same after expansion (INS6)."""
    cases = []
    for folder in EXPAND_FOLDERS:
        for path in sorted((harness.TESTS / folder).glob("*.bd")):
            text = harness.read_text(path)
            first = text.split("\n", 1)[0].strip()
            # `embed` reads a file next to the source, which the expansion's copy would not have.
            if first in ("//@ run", "//@ run-abort") and "embed(" not in text:
                cases.append((folder, path, first == "//@ run"))
    check(len(cases) >= 50, f"expand: expected at least 50 programs to re-check, found {len(cases)}")

    def one(case):
        folder, path, compare = case
        label = f"{folder}/{path.stem}"
        with harness.temporary_dir("burdin-expand-") as tmp:
            out, err, code = driver(["expand", str(path)], cwd=tmp)
            if code != 0:
                return [f"expand {label}: exit {code}: {err.strip()[:300]}"]
            expanded = Path(tmp) / f"{path.stem}.bd"
            harness.write_text(expanded, out)
            _, chk_err, chk_code = driver(["check", *COMMON, expanded.name], cwd=tmp)
            if chk_code != 0 or chk_err.strip():
                return [f"expand {label}: the expansion does not check: {chk_err.strip()[:400]}"]
            found = []
            if compare:
                original = Path(tmp) / f"original_{path.stem}.bd"
                harness.write_text(original, harness.read_text(path))
                a, _, a_code = driver(["run", original.name], cwd=tmp)
                b, _, b_code = driver(["run", expanded.name], cwd=tmp)
                if (a, a_code) != (b, b_code):
                    found.append(f"expand {label}: the expansion prints something else (exit {a_code} vs {b_code})")
            return found

    with ThreadPoolExecutor(max_workers=os.cpu_count() or 2) as pool:
        for found in pool.map(one, cases):
            for message in found:
                add_problem(message)
    global checks
    checks += len(cases)


def check_expand_item_and_instances() -> None:
    out, err, code = driver(["expand", "expand_mix.bd", "--item", "scaled"])
    check(code == 0 and err == "", f"expand --item scaled: exit {code}, stderr: {err}")
    check(out.startswith("fn scaled(n: i32): i32 {") and "n * 2" in out and "fn main" not in out,
          f"expand --item scaled: only that function, expanded:\n{out}")
    out, _, code = driver(["expand", "../m21_reflect/derive_describe.bd", "--item", "Point.describe"])
    check(code == 0 and out.startswith("fn describe(self): String {") and 'String.from("Point")' in out and "self.x" in out,
          f"expand --item Point.describe: the method a derive generated, with its loop unrolled:\n{out}")
    out, err, code = driver(["expand", "expand_mix.bd", "--item", "nosuch"])
    check(code == 1 and out == "" and "no item named `nosuch`" in err and "scaled" in err,
          f"expand --item nosuch: exit 1 and the list of items, got exit {code}: {err}")
    out, err, code = driver(["expand", "expand_mix.bd", "--item", "scaled", "--instances"])
    check(code == 2 and "cannot be combined" in err, f"expand --item with --instances is a usage error: {code} {err[:200]}")

    base, _, _ = driver(["expand", "generics.bd"])
    out, err, code = driver(["expand", "generics.bd", "--instances"])
    check(code == 0 and err == "" and base in out and out.index(base) < out.index("// instance"), f"expand --instances keeps the expansion first: exit {code}")
    for name in ["largest<i32>", "largest<i64>", "twice<i32>", "twice<i64>"]:
        check(f"// instance {name}\n" in out, f"expand --instances: `{name}` is missing")
    out, _, code = driver(["expand", "expand_mix.bd", "--no-format"])
    check(code == 0 and "a.add(b)" in out, "expand --no-format prints the expansion unformatted")


def check_expand_package() -> None:
    package = INSPECT / "packages" / "expand_pkg"
    out, err, code = driver(["expand", "."], cwd=package)
    check(code == 0 and err == "", f"expand package: exit {code}, stderr: {err}")
    golden("expand/expand_pkg.golden", out.replace("\\", "/"), "expand package")
    check("// ==== src/main.bd ====" in out.replace("\\", "/") and "// ==== src/shapes.bd ====" in out.replace("\\", "/"),
          "expand package: each file starts with its header")
    out, err, code = driver(["expand", ".", "--item", "shapes.area"], cwd=package)
    check(code == 0 and out.startswith("fn area(h: i32, w: i32 = UNIT): i32 {") and "w * h" in out and "comptime" not in out,
          f"expand --item shapes.area: exit {code}\n{out}{err}")
    out, err, code = driver(["expand", ".", "--item", "expand_pkg.shapes.area"], cwd=package)
    check(code == 0 and "w * h" in out, "expand --item with the package-qualified name")
    with harness.temporary_dir("burdin-expand-pkg-") as tmp:
        # Each file of the package alone cannot be checked, so the expansion is checked as the package it is.
        import shutil

        shutil.copytree(package, Path(tmp) / "pkg")
        text, _, code = driver(["expand", "."], cwd=Path(tmp) / "pkg")
        parts = re.split(r"^// ==== (.+?) ====\n", text.replace("\\", "/"), flags=re.M)
        files = dict(zip(parts[1::2], parts[2::2]))
        for relative, content in files.items():
            harness.write_text(Path(tmp) / "pkg" / relative, content)
        _, chk_err, chk_code = driver(["check", *COMMON, "."], cwd=Path(tmp) / "pkg")
        check(chk_code == 0 and chk_err.strip() == "", f"expand package: the expanded package does not check: {chk_err}")


def check_expand_failures() -> None:
    out, err, code = driver(["expand", *COMMON, "expand_error.bd"])
    check(code == 1 and out == "" and "E0003" in err, f"expand of a program with an error: exit {code}, stdout {out!r}, stderr {err!r}")
    out, err, code = driver(["expand"])
    check(code == 2 and "missing target" in err, f"expand without a target: exit {code}: {err[:200]}")
    out, err, code = driver(["expand", "expand_mix.bd", "--bogus"])
    check(code == 2 and "unknown option `--bogus`" in err, f"expand --bogus: exit {code}: {err[:200]}")
    out, err, code = driver(["expand", "expand_mix.bd", "extra"])
    check(code == 2 and "unexpected argument" in err, f"expand with two targets: exit {code}: {err[:200]}")
    out, err, code = driver(["expand", "expand_mix.bd", "--item"])
    check(code == 2, f"expand --item without a value: exit {code}")
    out, err, code = driver(["expand", "--help"])
    check(code == 0 and "burdin expand" in out and "--item" in out and "--instances" in out, "expand --help prints the usage")
    out, err, code = driver(["expand", "no_such_file.bd"])
    check(code != 0 and out == "", f"expand of a missing file fails: exit {code}")


# --------------------------------------------------------------------------------------------------------------------
# check --print-instances


def check_print_instances() -> None:
    out, err, code = driver(["check", *COMMON, "generics.bd", "--print-instances"])
    check(code == 0 and err == "", f"check --print-instances: exit {code}, stderr: {err}")
    # The count of instances nothing reaches follows the standard library, so it is not part of the golden.
    stable = re.sub(r"^not listed: .*\n", "not listed: <n> function instances and <n> type instances that no emitted code reaches\n", out, flags=re.M)
    golden("print_instances.golden", stable, "check --print-instances")
    for needle in ["fn twice: 2 instances", "fn largest: 2 instances", "type Pair: 2 instances", "largest<i64>", "required at generics.bd:"]:
        check(needle in out, f"check --print-instances: `{needle}` missing")
    out, err, code = driver(["check", *COMMON, "generics.bd", "--print-instances", "--format", "json"])
    check(code == 0, f"check --print-instances --format json: exit {code}")
    try:
        report = json.loads(out)
    except ValueError as error:
        add_problem(f"check --print-instances --format json: not JSON: {error}")
        return
    report["unreachable_function_instances"] = 0
    report["unreachable_type_instances"] = 0
    text = json.dumps(report, indent=2) + "\n"
    golden("print_instances.json.golden", text, "check --print-instances --format json")
    check(report.get("version") == 1 and report.get("function_instances") == 4 and report.get("type_instances") == 2,
          f"the JSON report counts: {report.get('function_instances')} / {report.get('type_instances')}")
    # Without the flag nothing is printed, and the format needs it.
    out, err, code = driver(["check", *COMMON, "generics.bd"])
    check(code == 0 and out == "" and err == "", "check without --print-instances prints nothing")
    out, err, code = driver(["check", "generics.bd", "--format", "json"])
    check(code == 2 and "--print-instances" in err, f"check --format without --print-instances: exit {code}: {err[:200]}")
    out, err, code = driver(["check", *COMMON, "expand_error.bd", "--print-instances"])
    check(code == 1 and out == "" and "E0003" in err, f"check --print-instances on a program with an error: exit {code}, stdout {out!r}")


# --------------------------------------------------------------------------------------------------------------------
# --emit


def object_magic_ok(data: bytes) -> bool:
    if harness.PLATFORM == "linux":
        return data[:4] == b"\x7fELF"
    if harness.PLATFORM == "macos":
        return data[:4] in (b"\xcf\xfa\xed\xfe", b"\xca\xfe\xba\xbe")
    return data[:2] in (b"\x64\x86", b"\x64\xaa", b"\x4c\x01")


def check_emit() -> None:
    with harness.temporary_dir("burdin-emit-") as tmp:
        tmp = Path(tmp)
        output = tmp / "app"
        kinds = "tir,llvm-ir,llvm-ir-opt,llvm-bc,asm,obj,deps"
        out, err, code = driver(["build", "generics.bd", "-o", str(output), "--emit", kinds])
        check(code == 0, f"build --emit {kinds}: exit {code}: {err[:300]}")
        suffix = ".obj" if harness.PLATFORM == "windows" else ".o"
        names = {"tir": ".tir", "llvm-ir": ".ll", "llvm-ir-opt": ".opt.ll", "llvm-bc": ".bc", "asm": ".s", "obj": suffix, "deps": ".deps"}
        files = {kind: tmp / f"app{ext}" for kind, ext in names.items()}
        for kind, path in files.items():
            check(path.is_file(), f"--emit {kind}: {path.name} was not written")
        executable = harness.executable_in(tmp, "app")
        check(executable.is_file(), "--emit does not replace the executable")
        if files["tir"].is_file():
            tir = harness.read_text(files["tir"])
            check("fn largest<i32>" in tir and "fn main" in tir and "fn twice<i64>" in tir, "--emit tir lists the program's own functions")
        if files["llvm-ir"].is_file():
            ir = harness.read_text(files["llvm-ir"])
            check("define" in ir and "largest<i32>" in ir, "--emit llvm-ir holds the function definitions")
        if files["llvm-ir-opt"].is_file():
            check("define" in harness.read_text(files["llvm-ir-opt"]), "--emit llvm-ir-opt holds the optimized definitions")
        if files["llvm-bc"].is_file():
            check(harness.read_bytes(files["llvm-bc"])[:4] == b"BC\xc0\xde", "--emit llvm-bc is LLVM bitcode")
        if files["asm"].is_file():
            check("largest<i32>" in harness.read_text(files["asm"]), "--emit asm holds the function labels")
        if files["obj"].is_file():
            check(object_magic_ok(harness.read_bytes(files["obj"])), "--emit obj is an object file of this platform")
        if files["deps"].is_file():
            check(harness.read_text(files["deps"]).strip() == "", "--emit deps lists no files for a program that embeds none")
        # One kind only writes one file next to the output.
        other = tmp / "one"
        out, err, code = driver(["build", "generics.bd", "-o", str(other), "--emit", "asm"])
        check(code == 0 and (tmp / "one.s").is_file() and not (tmp / "one.ll").exists(), "--emit asm writes only the assembly")
        if platform.machine().lower() in ("x86_64", "amd64"):
            for flag, marker in [("--intel", "%"), ("--att", "%")]:
                path = tmp / f"d{flag[2:]}"
                out, err, code = driver(["build", "generics.bd", "-o", str(path), "--emit", "asm", flag])
                text = harness.read_text(Path(f"{path}.s")) if Path(f"{path}.s").is_file() else ""
                body = text.split('"burdin.largest<i32>":')[-1].split(".cfi_endproc")[0] if text else ""
                if flag == "--intel":
                    check(code == 0 and body and "%" not in body, f"--emit asm --intel prints Intel syntax, got:\n{body[:300]}")
                else:
                    check(code == 0 and "%" in body, f"--emit asm --att prints AT&T syntax, got:\n{body[:300]}")
        out, err, code = driver(["build", "generics.bd", "-o", str(tmp / "x"), "--emit", "bogus"])
        check(code == 2 and "unknown `--emit` kind `bogus`" in err and "llvm-ir-opt" in err, f"--emit bogus: exit {code}: {err[:200]}")
        out, err, code = driver(["build", "generics.bd", "-o", str(tmp / "x"), "--intel"])
        check(code == 2 and "only apply to `--emit asm`" in err, f"--intel without --emit asm: exit {code}: {err[:200]}")
        out, err, code = driver(["run", "generics.bd", "--emit", "asm"])
        check(code == 2 and "only valid for `burdin build`" in err, f"run --emit: exit {code}: {err[:200]}")
        out, err, code = driver(["build", "expand_error.bd", "-o", str(tmp / "y"), "--emit", "tir", *COMMON])
        check(code == 1 and not (tmp / "y.tir").exists(), f"--emit tir of a program with an error writes nothing: exit {code}")


# --------------------------------------------------------------------------------------------------------------------
# burdin asm


def check_asm() -> None:
    out, err, code = driver(["asm", "generics.bd"])
    lines = out.split("\n")
    check(code == 0 and err == "", f"asm (the list): exit {code}: {err[:200]}")
    own = [line for line in lines if line and "[std]" not in line]
    check(own[:1] == ["largest<i32>"] or "largest<i32>" in own, f"asm (the list): the program's own functions come first: {own[:8]}")
    for name in ["largest<i32>", "largest<i64>", "twice<i32>", "twice<i64>", "main"]:
        check(name in own, f"asm (the list): `{name}` is missing")
    check(any(line.endswith("[std]") for line in lines), "asm (the list): standard library functions are marked `[std]`")
    out, err, code = driver(["asm", "generics.bd", "largest"])
    check(code == 2 and "`largest` is generic" in err and "largest<i32>" in err and "largest<i64>" in err,
          f"asm of a generic without type arguments lists the instances: exit {code}: {err}")
    out, err, code = driver(["asm", "generics.bd", "largest<i32>"])
    check(code == 0 and err == "", f"asm largest<i32>: exit {code}: {err[:300]}")
    check('largest<i32>' in out.split("\n")[0] or "largest<i32>" in out, "asm largest<i32>: the function's label")
    check(re.search(r"\bret", out) is not None, f"asm largest<i32>: a return instruction:\n{out[:400]}")
    check("generics.bd" in out and "if x > y" in out, f"asm largest<i32>: the Burdin source lines are interleaved:\n{out[:600]}")
    out, _, code = driver(["asm", "generics.bd", "largest<i32>", "--no-source"])
    check(code == 0 and "if x > y" not in out and re.search(r"\bret", out) is not None, "asm --no-source leaves the source out")
    out, _, code = driver(["asm", "generics.bd", "largest<i32>", "--llvm"])
    check(code == 0 and out.lstrip().startswith("define") and "largest<i32>" in out, f"asm --llvm prints the function's IR:\n{out[:300]}")
    out, _, code = driver(["asm", "generics.bd", "main", "--debug"])
    check(code == 0 and "main" in out.split("\n")[0], "asm --debug builds the debug profile")
    if platform.machine().lower() in ("x86_64", "amd64"):
        intel, _, code_i = driver(["asm", "generics.bd", "largest<i32>", "--intel", "--no-source"])
        att, _, code_a = driver(["asm", "generics.bd", "largest<i32>", "--att", "--no-source"])
        check(code_i == 0 and code_a == 0 and "%" not in intel and "%" in att, f"asm --intel and --att:\n{intel[:200]}\n{att[:200]}")
    out, err, code = driver(["asm", "generics.bd", "nosuch"])
    check(code == 2 and "no function named `nosuch`" in err, f"asm of an unknown function: exit {code}: {err[:200]}")
    out, err, code = driver(["asm", "expand_error.bd", "main", *COMMON])
    check(code == 1 and out == "" and "E0003" in err, f"asm of a program with an error: exit {code}")
    out, err, code = driver(["asm"])
    check(code == 2 and "missing target" in err, f"asm without a target: exit {code}")
    out, err, code = driver(["asm", "generics.bd", "main", "--bogus"])
    check(code == 2 and "unknown option" in err, f"asm --bogus: exit {code}")
    out, err, code = driver(["asm", "generics.bd", "main", "extra"])
    check(code == 2 and "unexpected argument" in err, f"asm with two functions: exit {code}")
    out, err, code = driver(["asm", "--help"])
    check(code == 0 and "burdin asm" in out, "asm --help prints the usage")


# --------------------------------------------------------------------------------------------------------------------
# burdin bloat


def check_bloat() -> None:
    out, err, code = driver(["bloat", "generics.bd", "--top", "3"])
    check(code == 0 and err == "", f"bloat: exit {code}: {err[:300]}")
    for heading in [
        "bloat: generics (release)",
        "executable ",
        "largest functions (top 3 of ",
        "size per package",
        "size per module",
        "size per generic function (instances added up)",
        "code that is not a source function",
    ]:
        check(heading in out, f"bloat: `{heading}` missing:\n{out[:800]}")
    section = out.split("largest functions")[1].split("size per package")[0]
    rows = [line for line in section.split("\n")[2:] if line.strip()]
    check(len(rows) == 3, f"bloat --top 3 lists three functions, got {len(rows)}")
    check(re.search(r"^\s+\d[\d,]*\s+[\d.]+\s+2\s+largest\b", out, flags=re.M) is not None, f"bloat: `largest` has two instances added up:\n{out}")
    check("twice" in out.split("size per generic function")[1], "bloat: `twice` is a generic function")
    out, err, code = driver(["bloat", "generics.bd", "--format", "json", "--top", "2"])
    check(code == 0, f"bloat --format json: exit {code}")
    try:
        report = json.loads(out)
    except ValueError as error:
        add_problem(f"bloat --format json: not JSON: {error}")
        return
    for key in ["version", "title", "executable", "code", "data", "functions", "largest", "packages", "modules", "generics"]:
        check(key in report, f"bloat --format json: `{key}` missing from {sorted(report)}")
    check(report.get("version") == 1 and report.get("title") == "generics (release)", "bloat --format json: version and title")
    check(len(report.get("largest", [])) == 2, "bloat --format json --top 2: two largest functions")
    sizes = [item["size"] for item in report.get("largest", [])]
    check(sizes == sorted(sizes, reverse=True), f"bloat: the largest functions are sorted: {sizes}")
    generics = {item["generic"]: item for item in report.get("generics", [])}
    check(generics.get("largest", {}).get("instances") == 2, f"bloat --format json: largest has two instances: {generics}")
    packages = {item["package"]: item for item in report.get("packages", [])}
    check("std" in packages and "generics" in packages, f"bloat --format json: the std and the program's own package: {sorted(packages)}")
    out, _, code = driver(["bloat", "generics.bd", "--debug"])
    check(code == 0 and "bloat: generics (debug)" in out, "bloat --debug measures the debug profile")
    out, err, code = driver(["bloat", "generics.bd", "--top", "0"])
    check(code == 2 and "positive" in err, f"bloat --top 0: exit {code}: {err[:200]}")
    out, err, code = driver(["bloat", "generics.bd", "--format", "xml"])
    check(code == 2 and "`human` or `json`" in err, f"bloat --format xml: exit {code}")
    out, err, code = driver(["bloat", "expand_error.bd", *COMMON])
    check(code == 1 and out == "" and "E0003" in err, f"bloat of a program with an error: exit {code}")
    out, err, code = driver(["bloat"])
    check(code == 2 and "missing target" in err, f"bloat without a target: exit {code}")
    out, err, code = driver(["bloat", "--help"])
    check(code == 0 and "burdin bloat" in out, "bloat --help prints the usage")


def main() -> int:
    check_expand_goldens()
    check_expand_reparse()
    check_expand_item_and_instances()
    check_expand_package()
    check_expand_failures()
    check_print_instances()
    check_emit()
    check_asm()
    check_bloat()
    if BLESS:
        print("goldens rewritten; review the diff before committing")
        return 0
    if problems:
        for problem in problems:
            print(f"FAIL: {problem}")
        print(f"\n{len(problems)} problem(s) in {checks} checks")
        return 1
    print(f"ok: {checks} checks")
    return 0


if __name__ == "__main__":
    sys.exit(main())
