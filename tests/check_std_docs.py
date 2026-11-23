#!/usr/bin/env python3
"""Checks the documentation of the standard library (issue #938). Needs a built driver.
Usage: tests/check_std_docs.py [--write-baseline] [--no-run] [--jobs N]

BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin is used.

The public API is read from `burdin doc --std --output-format json`, the same document `burdin doc --std` renders. It
holds the public items of every module, their methods, and the module's extension methods: the same set
`burdin doc --std --coverage` counts. For each entry the check requires:

  * documentation: a doc comment. Always required; there is no baseline for it.
  * an example: at least one Burdin code block in the doc comment (spec/doc.md, DC6). Entries that still lack one are
    listed in tests/std_docs/no_example.txt, one `module | signature` per line. The check fails on a new entry without
    an example, and on a listed entry that now has one (or no longer exists), so the list only shrinks. The list is
    written with `--write-baseline`, never by hand.
  * every example runs: each Burdin block in the documentation of the standard library, and of its modules, is built as
    a program and run with the same attributes a doc test takes (spec/test-runner.md, "Doc-test attributes"): `ignore`
    skips it, `no_run` only checks it, `compile_fail [CODE]` must fail to compile (with that code), and `should_panic`
    must end in a panic. An example without `fn main` is wrapped in one; its `use` lines are hoisted above it, and
    its hidden `# ` lines are compiled (DC6). Identical examples are built once.

`--no-run` skips the examples (the documentation and the baseline are still checked); CI runs everything.
"""

import concurrent.futures
import json
import os
import re
import shutil
import sys
import tempfile
from pathlib import Path

import harness

problems = []
checks = 0

BASELINE = harness.TESTS / "std_docs" / "no_example.txt"
WRITE_BASELINE = "--write-baseline" in sys.argv[1:]
NO_RUN = "--no-run" in sys.argv[1:]
COMMON = ["--no-color", "--diagnostic-format=short"]
TIMEOUT = 120

burdin = harness.require_burdin(
    2, "error: compiler driver not found. Build it first or set BURDIN=/path/to/burdin"
)


def jobs_value() -> int:
    args = sys.argv[1:]
    for i, a in enumerate(args):
        if a == "--jobs" and i + 1 < len(args):
            return max(1, int(args[i + 1]))
    return max(1, min(8, os.cpu_count() or 1))


def check(condition: bool, message: str) -> bool:
    global checks
    checks += 1
    if not condition:
        problems.append(message)
    return condition


# --- Reading the API -----------------------------------------------------------------------------------------------------


class Entry:
    """A documented or undocumented public API entry: an item, a method, or an extension method."""

    def __init__(self, module: str, signature: str, docs, where: str, label: str):
        self.module = module
        self.signature = signature
        self.docs = docs
        self.where = where
        self.label = label

    @property
    def key(self) -> str:
        return f"{self.module} | {self.signature}"


def read_api(json_path: Path):
    data = json.loads(json_path.read_text(encoding="utf-8"))
    entries = []
    module_docs = []
    for module in data["modules"]:
        name = module["name"]
        if module.get("docs"):
            module_docs.append((name, module["file"], module["docs"]))
        for item in module["items"]:
            where = f"{module['file']}:{item['span']['line']}"
            entries.append(Entry(name, item["signature"], item["docs"], where, f"{item['kind']} {item['path']}"))
            for method in item["methods"]:
                where = f"{module['file']}:{method['span']['line']}"
                entries.append(Entry(name, method["signature"], method["docs"], where, f"{method['kind']} {method['display']}"))
        for ext in module["extensions"]:
            where = f"{module['file']}:{ext['span']['line']}"
            entries.append(Entry(name, ext["signature"], ext["docs"], where, f"extension method {ext['display']}"))
    return entries, module_docs


# --- Examples ------------------------------------------------------------------------------------------------------------

FENCE_OPEN = re.compile(r"^\s*```(.*)$")
MAIN = re.compile(r"^\s*fn\s+main\s*\(")


class Example:
    def __init__(self, text: str, attrs: list, line: int, where: str, owner: str):
        self.text = text
        self.attrs = attrs
        self.line = line
        self.where = where
        self.owner = owner


def blocks(doc: str, where: str, owner: str):
    """The Burdin code blocks of a doc comment (DC6): untagged, or tagged `burdin` or `bd`, with their attributes."""
    lines = doc.split("\n")
    out = []
    i = 0
    while i < len(lines):
        m = FENCE_OPEN.match(lines[i])
        if not m:
            i += 1
            continue
        info = m.group(1).strip()
        words = [w for w in re.split(r"[ ,]+", info) if w]
        start = i
        i += 1
        body = []
        while i < len(lines) and lines[i].strip() != "```":
            body.append(lines[i])
            i += 1
        i += 1  # the closing fence
        language = words[0] if words else ""
        if language not in ("", "burdin", "bd"):
            continue
        attrs = words[1:] if language else words
        out.append(Example(hidden_lines_kept("\n".join(body)), attrs, start + 1, where, owner))
    return out


def hidden_lines_kept(body: str) -> str:
    """The code a doc test compiles (DC6): a `# ` line, or a line that is only `#`, loses its marker and keeps its
    indentation."""
    kept = []
    for line in body.split("\n"):
        stripped = line.lstrip()
        if stripped == "#":
            kept.append("")
        elif stripped.startswith("# "):
            kept.append(line[: len(line) - len(stripped)] + stripped[2:])
        else:
            kept.append(line)
    return "\n".join(kept)


# A declaration starts with a keyword, or is an impl header such as `Bag<T>: Index<T> {` (a type, a colon, a body).
DECLARATION = re.compile(r"^((use|fn|struct|enum|error|interface|const|type|impl|priv|pub|extern|derive|test)\b|\w+(<[^>]*>)?: .*\{\s*$)")


def program(text: str) -> str:
    """The whole program: the example as written when it declares `fn main`; otherwise its top-level declarations
    (a line at column 0 that starts one, through its closing brace) hoisted above a generated `fn main` that holds the
    rest."""
    if any(MAIN.match(line) for line in text.split("\n")):
        return text + "\n"
    decls, rest = [], []
    lines = text.split("\n")
    i = 0
    while i < len(lines):
        if DECLARATION.match(lines[i]):
            depth = 0
            seen_brace = False
            j = i
            while True:
                depth += lines[j].count("{") - lines[j].count("}")
                seen_brace = seen_brace or "{" in lines[j]
                if depth <= 0 and (seen_brace or j == i) or j + 1 >= len(lines):
                    break
                j += 1
            decls.extend(lines[i : j + 1])
            i = j + 1
        else:
            rest.append(lines[i])
            i += 1
    return "\n".join(decls) + "\nfn main() {\n" + "\n".join(rest) + "\n}\n"


def run_example(example: Example):
    """Builds and runs one example; returns the problem text, or None when it passes."""
    if "ignore" in example.attrs:
        return None
    attrs = example.attrs
    code = None
    for i, word in enumerate(attrs):
        if word == "compile_fail" and i + 1 < len(attrs) and re.fullmatch(r"E[0-9]{4}", attrs[i + 1]):
            code = attrs[i + 1]
    work = Path(tempfile.mkdtemp(prefix="burdin-std-doc-"))
    try:
        source = work / "example.bd"
        source.write_bytes(program(example.text).encode("utf-8"))
        if "compile_fail" in attrs or "no_run" in attrs:
            command = [burdin, "check", *COMMON, str(source)]
        else:
            command = [burdin, "run", *COMMON, str(source)]
        result = harness.run(command, cwd=work, timeout=TIMEOUT)
        stderr = harness.text(harness.normalize(result.stderr))
        if "compile_fail" in attrs:
            if result.returncode == 0:
                return "compiles, but it is marked compile_fail"
            if code is not None and f"[{code}]" not in stderr:
                return f"fails, but not with {code}: {stderr.strip()[:300]}"
            return None
        if "no_run" in attrs or "compile_fail" in attrs:
            if result.returncode != 0:
                return f"does not compile: {stderr.strip()[:400]}"
            return None
        if "should_panic" in attrs:
            if result.returncode == 0:
                return "runs to the end, but it is marked should_panic"
            if "error[" in stderr:
                return f"does not compile: {stderr.strip()[:400]}"
            return None
        if result.returncode != 0:
            return f"fails (status {result.returncode}): {stderr.strip()[:400]}"
        return None
    except Exception as error:  # a timeout or a missing driver: a failure of this example, not a crash of the check
        return f"could not run: {error}"
    finally:
        shutil.rmtree(work, ignore_errors=True)


# --- The checks ----------------------------------------------------------------------------------------------------------

with harness.temporary_dir("burdin-std-docs-") as work:
    api_json = work / "std.json"
    result = harness.run([burdin, "doc", "--std", "--output-format", "json", "-o", str(api_json), *COMMON])
    if result.returncode != 0:
        print(harness.text(harness.normalize(result.stderr)))
        print(f"error: `burdin doc --std` failed with status {result.returncode}")
        sys.exit(1)
    entries, module_docs = read_api(api_json)

# A. Every entry is documented.
undocumented = [e for e in entries if not e.docs]
for e in undocumented:
    check(False, f"undocumented: {e.label} `{e.signature}` at {e.where}")
check(len(entries) > 0, "the API has no entries: `burdin doc --std` reported nothing")

# A2. Compiler-internal names are not documented (spec/doc.md, DC7): the `__` helpers that the compiler's desugarings
# call stay public but stay off the site.
internal = [e for e in entries if re.search(r"\b__\w", e.signature)]
for e in internal:
    check(False, f"compiler-internal name documented: {e.label} `{e.signature}` at {e.where} (spec/doc.md, DC7)")

# B. Every entry has an example, except the baseline's.
keys_without_example = sorted({e.key for e in entries if e.docs and not blocks(e.docs, e.where, e.label)})
keys_with_example = {e.key for e in entries if e.docs and blocks(e.docs, e.where, e.label)}
baseline = []
if BASELINE.exists():
    baseline = [line for line in BASELINE.read_text(encoding="utf-8").split("\n") if line.strip()]

if WRITE_BASELINE:
    BASELINE.parent.mkdir(parents=True, exist_ok=True)
    harness.write_text(BASELINE, "".join(f"{key}\n" for key in keys_without_example))
    print(f"wrote {len(keys_without_example)} entries to {BASELINE.relative_to(harness.ROOT)}")
else:
    listed = set(baseline)
    all_keys = {e.key for e in entries}
    by_key = {}
    for e in entries:
        by_key.setdefault(e.key, e)
    for key in keys_without_example:
        if key not in listed:
            e = by_key[key]
            check(False, f"no example: {e.label} `{e.signature}` at {e.where}")
    for key in baseline:
        if key in keys_with_example:
            check(False, f"stale baseline entry, now has an example: {key}")
        elif key not in all_keys:
            check(False, f"stale baseline entry, no such entry: {key}")

# C. Every example runs. Identical blocks are built once; the first place that holds one names it.
examples = []
for e in entries:
    if e.docs:
        examples.extend(blocks(e.docs, e.where, f"`{e.signature}`"))
for name, file, docs in module_docs:
    examples.extend(blocks(docs, file, f"module `{name}`"))

unique = {}
for example in examples:
    unique.setdefault(program(example.text), example)
print(f"{len(examples)} examples ({len(unique)} distinct) in the standard library documentation")

if not NO_RUN:
    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs_value()) as pool:
        futures = {pool.submit(run_example, ex): (src, ex) for src, ex in unique.items()}
        for future in concurrent.futures.as_completed(futures):
            _, example = futures[future]
            failure = future.result()
            check(failure is None, f"example for {example.owner} at {example.where}:{example.line}: {failure}")

print(f"Checked {checks} items.")
if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
