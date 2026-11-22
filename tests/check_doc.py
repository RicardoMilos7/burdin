#!/usr/bin/env python3
"""Checks `burdin doc` (issues #234, #467, #468, #469, #656, #657, #658). Needs a built driver.
Usage: tests/check_doc.py [--update]

BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin is used. `--update` rewrites
the golden files under tests/doc/expect/ from the current output (review the diff before committing it).

What it checks (spec/doc.md has the rules, DC1-DC22, and names the checks that cover each):
  * the command line: usage errors exit with status 2, a package that does not compile is not documented;
  * the site of the fixture package tests/doc/geometry: the exact set of files, every page against its golden file, and the
    integrity of the whole site (every `href` and `src` leads to a file, every `#fragment` to an `id`, nothing leaves
    the site);
  * the type page (issue #658): operators written as `a + b`, derived and implemented interfaces, extension methods with
    their origin and the `use` they need, and the methods of a specific generic instance;
  * private items (issue #469): left out by default, shown and marked with `--private`, and re-exports from `pub use`;
  * intra-doc links (issue #468): the diagnostics (file, line, code) are exactly the `//~` markers of the fixtures under
    tests/doc/links -- `D0001` for a link that names nothing and `D0002` for a link to a `priv` item -- and
    `--deny-warnings` turns them into a failure;
  * the search index (issue #467) parses and names the items, their members, and the modules;
  * the JSON output (issue #656): the golden file, the schema version, determinism, and the same facts as the site;
  * the coverage report and the `missing_docs` lint (issue #657): the golden report, and the lint and the report list
    the same undocumented items;
  * markup in a doc comment stays text; the output directory is never an unrelated directory, and a second run removes
    the pages of items that are gone.
"""

import difflib
import json
import re
import shutil
import sys
from pathlib import Path

import harness

problems = []
checks = 0

DOC = harness.TESTS / "doc"
EXPECT = DOC / "expect"
UPDATE = "--update" in sys.argv[1:]
COMMON = ["--no-color", "--diagnostic-format=short"]
MARKER = re.compile(r"//~ ((?:D[0-9]{4}[ ]*)+)")
DIAGNOSTIC = re.compile(r"^(.+?):([0-9]+):([0-9]+): (warning|error)\[(D[0-9]{4})\]")

burdin = harness.require_burdin(
    2, "error: compiler driver not found. Build it first or set BURDIN=/path/to/burdin"
)


def add_problem(message: str) -> None:
    problems.append(message)


def check(condition: bool, message: str) -> bool:
    global checks
    checks += 1
    if not condition:
        add_problem(message)
    return condition


def doc(args, cwd=None):
    """Runs `burdin doc`; stdout, stderr, and the exit status."""
    result = harness.run([burdin, "doc", *args], cwd=cwd)
    out = harness.text(harness.normalize(result.stdout))
    err = harness.text(harness.normalize(result.stderr))
    return out, err, result.returncode


def diff(expected: str, actual: str, label: str) -> str:
    lines = difflib.unified_diff(
        expected.splitlines(), actual.splitlines(), f"expected {label}", f"actual {label}", lineterm="", n=2
    )
    return "\n".join(list(lines)[:40])


def golden(path: Path, actual: str, label: str) -> None:
    """Compares `actual` with the golden file (or writes it with --update)."""
    global checks
    checks += 1
    if UPDATE:
        path.parent.mkdir(parents=True, exist_ok=True)
        harness.write_text(path, actual)
        return
    if not path.is_file():
        add_problem(f"{label}: missing golden file {path.relative_to(harness.ROOT).as_posix()} (run with --update)")
        return
    expected = harness.read_text(path)
    if expected != actual:
        add_problem(f"{label}: differs from {path.relative_to(harness.ROOT).as_posix()}\n{diff(expected, actual, label)}")


def site_files(site: Path):
    return sorted(
        p.relative_to(site).as_posix() for p in site.rglob("*") if p.is_file() and p.name != ".burdin-doc"
    )


def page(site: Path, relative: str) -> str:
    return harness.read_text(site / relative)


# --- The command line ---------------------------------------------------------------------------------------------

GEOMETRY = DOC / "geometry"

for label, args in [
    ("no target", []),
    ("an unknown option", [str(GEOMETRY), "--frobnicate"]),
    ("a second target", [str(GEOMETRY), str(GEOMETRY)]),
    ("an unknown format", [str(GEOMETRY), "--output-format", "pdf"]),
    ("--coverage with -o", [str(GEOMETRY), "--coverage", "-o", "x"]),
    ("--output-format without a value", [str(GEOMETRY), "--output-format"]),
    ("--std with a target", ["--std", str(GEOMETRY)]),
    ("--std with -o and --coverage", ["--std", "--coverage", "-o", "x"]),
]:
    out, err, status = doc(args)
    check(status == 2, f"cli: {label}: expected exit status 2, got {status}: {err.strip()[:300]}")
    check("usage:" in err and "burdin doc" in err, f"cli: {label}: the usage text is missing: {err.strip()[:300]}")

with harness.temporary_dir("burdin-doc-errors-") as work:
    out, err, status = doc([str(DOC / "errors" / "type_error.bd"), "-o", str(work / "site"), *COMMON])
    check(status == 1, f"errors: a package that does not compile: expected exit status 1, got {status}")
    check("could not check" in err and "error[E" in err, f"errors: the compile error is not reported: {err.strip()[:300]}")
    check(not (work / "site").exists(), "errors: a package that does not compile has no site")

# --- The site of the fixture package ------------------------------------------------------------------------------

EXPECTED_FILES = [
    "collections/index.html",
    "collections/struct.Stack.html",
    "extras/index.html",
    "index.html",
    "lib/const.PI.html",
    "lib/fn.distance_squared.html",
    "lib/fn.undocumented.html",
    "lib/index.html",
    "search-index.js",
    "shapes/error.GeometryError.html",
    "shapes/index.html",
    "shapes/interface.Shape.html",
    "shapes/struct.Circle.html",
    "shapes/struct.Point.html",
    "static/doc.css",
    "static/search.js",
]
GOLDEN_SUFFIXES = (".html", ".js")


def docs_of(text: str) -> str:
    """The doc comment of a page: the first `<div class="docs">` and its body."""
    start = text.find('<div class="docs">')
    if start < 0:
        return ""
    return text[start:text.find("</div>", start)]


def check_site_integrity(site: Path, label: str, allow_external: bool = False, ignore=()) -> None:
    """Every link in the site leads to a file and an anchor that exist, and nothing leaves the site."""
    ids = {}
    for html in site.rglob("*.html"):
        ids[html.resolve()] = set(re.findall(r'id="([^"]+)"', harness.read_text(html)))
    for html in sorted(site.rglob("*.html")):
        text = harness.read_text(html)
        for attribute, value in re.findall(r'\b(href|src)="([^"]*)"', text):
            if value in ignore:
                continue
            where = f"{label}: {html.relative_to(site).as_posix()}: {attribute}=\"{value}\""
            if re.match(r"^[a-z][a-z0-9+.-]*:", value):
                check(allow_external and value.startswith(("http://", "https://")), f"{where} leaves the site")
                continue
            path, _, fragment = value.partition("#")
            target = (html.parent / path).resolve() if path else html.resolve()
            if not check(target.is_file(), f"{where} leads to a missing file"):
                continue
            if fragment and target.suffix == ".html":
                check(fragment in ids.get(target, set()), f"{where} leads to a missing anchor")


with harness.temporary_dir("burdin-doc-site-") as work:
    site = work / "site"
    out, err, status = doc([str(GEOMETRY), "-o", str(site), *COMMON])
    check(status == 0, f"site: expected exit status 0, got {status}: {err.strip()[:400]}")
    check("warning" not in err and "error" not in err, f"site: the fixture has no diagnostics: {err.strip()[:400]}")
    check(f"doc: wrote {len(EXPECTED_FILES)} files to" in err, f"site: unexpected summary: {err.strip()[:300]}")
    check(out == "", f"site: nothing goes to standard output, got {out[:100]!r}")
    files = site_files(site)
    check(files == EXPECTED_FILES, f"site: the files are not the expected ones:\n{diff(chr(10).join(EXPECTED_FILES), chr(10).join(files), 'files')}")
    for relative in files:
        if relative.endswith(GOLDEN_SUFFIXES) and not relative.startswith("static/"):
            golden(EXPECT / "geometry" / relative, page(site, relative), f"site/{relative}")
    # The assets are compiled into the binary and copied; they must be there and be what the pages name.
    check(page(site, "static/doc.css").lstrip().startswith("/*"), "site: static/doc.css is not the stylesheet")
    check("BURDIN_DOC_INDEX" in page(site, "static/search.js"), "site: static/search.js is not the search script")
    check_site_integrity(site, "site")
    for relative in files:
        if relative.endswith(".html"):
            text = page(site, relative)
            check("<script src=\"http" not in text and "<link rel=\"stylesheet\" href=\"http" not in text,
                  f"site: {relative}: loads something from another server")
            check(text.startswith("<!DOCTYPE html>") and "<title>" in text, f"site: {relative}: not a complete page")

    # Pages under a directory reach the assets with the right number of `../`.
    check('href="../static/doc.css"' in page(site, "shapes/struct.Point.html"), "site: nested page: stylesheet path")
    check('href="static/doc.css"' in page(site, "index.html"), "site: root page: stylesheet path")

    # --- The type page (issue #658) ---
    point = page(site, "shapes/struct.Point.html")
    check('<code class="operator">a + b</code>' in point, "type page: the Add impl is shown as `a + b`")
    check('<code class="operator">-a</code>' in point, "type page: the Neg impl is shown as `-a`")
    check("Add.add" not in point, "type page: an operator is not shown as `Add.add`")
    for derived in ("Copy", "Eq", "Debug"):
        check(f'id="impl.{derived}"' in point and "badge\">derived" in point, f"type page: derived {derived} is listed")
    check('id="impl.Add"' in point and 'id="impl.Neg"' in point, "type page: implemented interfaces are listed")
    check('id="extension.doubled"' in point, "type page: the extension method is listed")
    check("import it with <code>use shapes.doubled</code>" in point, "type page: the extension names the `use` it needs")
    check("declared in module <code>shapes</code>" in point, "type page: the extension names its module")
    check('id="field.x"' in point and 'id="field.tag"' not in point, "type page: the private field is hidden")
    circle = page(site, "shapes/struct.Circle.html")
    check('id="method.new"' in circle and 'id="method.secret"' not in circle, "type page: the private method is hidden")
    check('href="../shapes/interface.Shape.html"' in circle, "type page: an implemented interface of the package links to its page")
    check('id="method.area"' in circle, "type page: the methods of an implemented interface are listed")
    check('id="extension.diameter"' in circle and "use extras.diameter" in circle, "type page: an extension from another module")
    stack = page(site, "collections/struct.Stack.html")
    check("badge\">for <code>Stack&lt;i32&gt;</code>" in stack, "type page: the method of a specific instance is marked")
    check(stack.count("badge\">for <code>") == 1, "type page: only the specific instance's methods are marked")
    shape = page(site, "shapes/interface.Shape.html")
    check("Required methods" in shape and "Provided methods" in shape, "interface page: required and provided methods")
    error = page(site, "shapes/error.GeometryError.html")
    check("NegativeRadius(f64) &quot;radius {0} must not be negative&quot;" in error, "error page: the variant shows its message")
    extras = page(site, "extras/index.html")
    check("fn String.shout(self): String" in extras, "module page: an extension on a type of the standard library")
    # --- Re-exports and the root module (issue #469) ---
    root = page(site, "lib/index.html")
    check("pub use <a href=\"../shapes/struct.Circle.html\">shapes.Circle</a>" in root, "re-exports: `pub use` of an item links to it")
    check("pub use <a href=\"../shapes/index.html\">shapes</a>" in root, "re-exports: `pub use` of a module links to it")
    check("Re-exported by <code>geometry</code>" in circle, "re-exports: the item says who re-exports it")
    check("<h1>Package <span class=\"name\">geometry</span></h1>" in page(site, "index.html"), "overview: the package name")
    # --- Links in doc comments (issue #468) ---
    overview = page(site, "index.html")
    check('<a href="shapes/struct.Circle.html">Circle</a>' in overview, "links: [Type] through a `pub use`")
    check('<a href="shapes/interface.Shape.html">shapes.Shape</a>' in overview, "links: [module.Type]")
    check('<a href="shapes/struct.Circle.html#method.area"><code>Circle.area</code></a>' in overview, "links: [`Type.method`]")
    distance = page(site, "lib/fn.distance_squared.html")
    check('href="../shapes/error.GeometryError.html#variant.NegativeRadius"' in distance, "links: [Type.Variant]")
    check('href="../shapes/struct.Circle.html#method.new"' in distance, "links: [Type.method]")
    check("<pre><code class=\"language-burdin\">" in distance, "docs: an untagged code block is Burdin")
    check("<h3 id=\"doc-errors\">Errors</h3>" in distance, "docs: a `# Errors` heading")

    # --- The search index (issue #467) ---
    index_text = page(site, "search-index.js")
    prefix = "window.BURDIN_DOC_INDEX = "
    ok = check(index_text.startswith(prefix) and index_text.rstrip().endswith("];"), "search: the index is a script that sets one global")
    if ok:
        entries = json.loads(index_text[len(prefix):].rstrip().rstrip(";"))
        paths = {e["p"]: e for e in entries}
        for wanted, kind in [
            ("shapes.Circle", "struct"),
            ("shapes.Circle.area", "method"),
            ("shapes.Point.x", "field"),
            ("shapes.GeometryError.NegativeRadius", "variant"),
            ("shapes.Point.add", "operator"),
            ("shapes.Point.doubled", "extension method"),
            ("collections", "module"),
            ("geometry.PI", "constant"),
        ]:
            check(paths.get(wanted, {}).get("k") == kind, f"search: {wanted} is not indexed as a {kind}")
        check(paths.get("shapes.Circle", {}).get("u") == "shapes/struct.Circle.html", "search: the url of an item")
        check(paths.get("shapes.Circle.new", {}).get("u") == "shapes/struct.Circle.html#method.new", "search: the url of a member")
        check(all(set(e) == {"n", "p", "k", "u", "d"} for e in entries), "search: every entry has the same fields")

# --- Private items --------------------------------------------------------------------------------------------------

with harness.temporary_dir("burdin-doc-private-") as work:
    site = work / "site"
    out, err, status = doc([str(GEOMETRY), "-o", str(site), "--private", *COMMON])
    check(status == 0, f"private: expected exit status 0, got {status}: {err.strip()[:400]}")
    files = site_files(site)
    check("lib/fn.helper.html" in files, "private: --private documents the `priv` function")
    helper = page(site, "lib/fn.helper.html") if "lib/fn.helper.html" in files else ""
    check("badge\">private" in helper, "private: a `priv` item is marked")
    point = page(site, "shapes/struct.Point.html")
    check('id="field.tag"' in point and "badge\">private" in point, "private: --private shows the private field, marked")
    check('id="method.secret"' in page(site, "shapes/struct.Circle.html"), "private: --private shows the private method")
    check("private" in page(site, "lib/index.html"), "private: the module page marks the private item")
    check_site_integrity(site, "private site")

# --- Intra-doc links ------------------------------------------------------------------------------------------------


def markers_of(path: Path):
    found = []
    for number, line in enumerate(harness.read_text(path).split("\n"), 1):
        match = MARKER.search(line)
        if match:
            for code in match.group(1).split():
                found.append((path.name, number, code))
    return sorted(found)


def diagnostics_of(err: str):
    found = []
    for line in err.split("\n"):
        match = DIAGNOSTIC.match(line)
        if match:
            found.append((Path(match.group(1)).name, int(match.group(2)), match.group(5)))
    return sorted(found)


for fixture, extra in [("broken.bd", []), ("private.bd", [])]:
    path = DOC / "links" / fixture
    with harness.temporary_dir("burdin-doc-links-") as work:
        out, err, status = doc([str(path), "-o", str(work / "site"), *extra, *COMMON])
        check(status == 0, f"links: {fixture}: warnings do not fail the run: got {status}: {err.strip()[:300]}")
        expected, actual = markers_of(path), diagnostics_of(err)
        check(expected == actual, f"links: {fixture}: diagnostics differ from the //~ markers\n  expected {expected}\n  actual   {actual}")
        out, err, status = doc([str(path), "-o", str(work / "site2"), "--deny-warnings", *extra, *COMMON])
        check(status == 1, f"links: {fixture}: --deny-warnings: expected exit status 1, got {status}")
        check("(--deny-warnings)" in err, f"links: {fixture}: --deny-warnings does not say why it failed: {err.strip()[:300]}")
        check((work / "site2" / "index.html").is_file(), f"links: {fixture}: the site is written even when --deny-warnings fails")
        check_site_integrity(work / "site", f"links {fixture}", allow_external=True, ignore=("docs/guide.md",))

with harness.temporary_dir("burdin-doc-links-") as work:
    site = work / "site"
    doc([str(DOC / "links" / "broken.bd"), "-o", str(site), *COMMON])
    text = page(site, "index.html")
    check(text.count('class="broken-link"') == 1, f"links: the module doc marks its one broken link, got {text.count('class=\"broken-link\"')}")
    broken = sum(
        harness.read_text(p).count('class="broken-link"') for p in site.rglob("*.html")
    )
    check(broken == 6, f"links: the site marks the six broken links, got {broken}")
    check('<a href="struct.Target.html">Target</a>' in text, "links: the module doc links [Target]")
    good = docs_of(page(site, "fn.good.html"))
    check(good.count('<a href="struct.Target.html') == 6, f"links: fn.good: six links to Target, got {good.count('<a href=\"struct.Target.html')}")
    check('href="struct.Target.html#field.value"' in good, "links: a link to a field")
    check('<span class="external-link">i32</span>' in good, "links: a primitive type is not a link")
    check('<span class="external-link">List</span>' in good, "links: a standard library type is not a link")
    not_links = docs_of(page(site, "fn.not_links.html"))
    check("[ ] [1] [a b] <code>[Missing]</code> [Missing]" in not_links, "links: brackets that are not links stay text")
    check('<a href="docs/guide.md">real file</a>' in not_links, "links: a file address is an ordinary link")
    check('<a href="https://example.org">https://example.org</a>' in not_links, "links: an autolink")
    private_site = work / "private"
    doc([str(DOC / "links" / "private.bd"), "-o", str(private_site), "--private", *COMMON])
    check('href="struct.Secret.html"' in page(private_site, "fn.visible.html"), "links: --private makes a link to a priv item work")

# --- The JSON output --------------------------------------------------------------------------------------------------

with harness.temporary_dir("burdin-doc-json-") as work:
    out, err, status = doc([str(GEOMETRY), "--output-format", "json", *COMMON])
    check(status == 0, f"json: expected exit status 0, got {status}: {err.strip()[:300]}")
    golden(EXPECT / "geometry.json", out, "json")
    try:
        data = json.loads(out)
    except ValueError as e:
        add_problem(f"json: the output is not valid JSON: {e}")
        data = None
    if data is not None:
        check(data["format_version"] == 1 and data["package"] == "geometry" and data["private_items"] is False, "json: header")
        modules = {m["name"]: m for m in data["modules"]}
        check(sorted(modules) == ["collections", "extras", "geometry", "shapes"], f"json: the modules are {sorted(modules)}")
        shapes = {i["name"]: i for i in modules["shapes"]["items"]}
        point = shapes["Point"]
        check(point["path"] == "shapes.Point" and point["kind"] == "struct", "json: item path and kind")
        check(point["signature"] == "struct Point: Copy, Eq, Debug", "json: item signature")
        check(point["span"] == {"file": "src/shapes.bd", "line": 6, "column": 8, "end_line": 6, "end_column": 13}, f"json: item span {point['span']}")
        check([f["name"] for f in point["fields"]] == ["x", "y"], "json: the private field is left out")
        check([o["display"] for o in point["operators"]] == ["a + b", "-a"], "json: operators")
        check([e["name"] for e in point["extensions"]] == ["doubled"], "json: extensions")
        check(point["extensions"][0]["origin"] == {"module": "shapes", "import": "use shapes.doubled"}, "json: extension origin")
        check([i["interface"] for i in point["implementations"]] == ["Copy", "Eq", "Debug", "Add", "Neg"], "json: implementations")
        check(point["reexported_by"] == ["geometry"], "json: reexported_by")
        error = shapes["GeometryError"]
        check(error["kind"] == "error enum" and len(error["variants"]) == 2, "json: error enum and variants")
        stack = {i["name"]: i for i in modules["collections"]["items"]}["Stack"]
        check([(m["name"], m["instance"]) for m in stack["methods"]] == [("new", None), ("push", None), ("total", "Stack<i32>")], "json: generic instance methods")
        root_module = modules["geometry"]
        check([r["path"] for r in root_module["reexports"]] == ["shapes.Circle", "shapes.GeometryError", "shapes.Point", "shapes.Shape", "collections.Stack", "shapes"], "json: re-exports")
        check({r["status"] for r in root_module["reexports"]} == {"item", "module"}, "json: re-export targets")
        check(root_module["root"] is True and modules["shapes"]["root"] is False, "json: the root module")
        links = modules["geometry"]["links"]
        check(
            [(l["target"], l["status"]) for l in links]
            == [("Circle", "item"), ("shapes.Shape", "item"), ("GeometryError", "item"), ("Circle.area", "item"), ("Stack", "item")],
            f"json: module links {links}",
        )
        check(links[3]["path"] == "shapes.Circle.area", "json: a resolved link's path")
    again, _, _ = doc([str(GEOMETRY), "--output-format", "json", *COMMON])
    check(again == out, "json: two runs give the same text")
    target = work / "api.json"
    stdout, err, status = doc([str(GEOMETRY), "--output-format", "json", "-o", str(target), *COMMON])
    check(status == 0 and stdout == "" and harness.read_text(target) == out, "json: -o writes the same text to a file")
    out_private, _, _ = doc([str(GEOMETRY), "--output-format", "json", "--private", *COMMON])
    private_data = json.loads(out_private)
    check(private_data["private_items"] is True, "json: --private is recorded")
    names = {i["name"] for m in private_data["modules"] for i in m["items"]}
    check("helper" in names, "json: --private includes the private function")
    check("helper" not in {i["name"] for m in data["modules"] for i in m["items"]}, "json: the private function is left out by default")

# --- Coverage and the lint --------------------------------------------------------------------------------------------

out, err, status = doc([str(GEOMETRY), "--coverage", *COMMON])
check(status == 0, f"coverage: expected exit status 0, got {status}: {err.strip()[:300]}")
golden(EXPECT / "coverage.txt", out, "coverage")
coverage_missing = sorted(
    (path.split("/")[-1], int(line)) for path, line in re.findall(r"^  (\S+?):([0-9]+)  ", out, re.M)
)
lint = harness.run([burdin, "lint", str(GEOMETRY), "-A", "all", "-W", "missing_docs", *COMMON], merge=True)
lint_text = harness.text(harness.normalize(lint.stdout))
lint_missing = sorted(
    (Path(m.group(1)).name, int(m.group(2)))
    for m in re.finditer(r"^(.+?):([0-9]+):[0-9]+: warning\[L0017\]", lint_text, re.M)
)
check(lint_missing != [] and lint_missing == coverage_missing, f"coverage: the lint and the report list different items\n  lint     {lint_missing}\n  coverage {coverage_missing}")
out, err, status = doc([str(DOC / "links" / "broken.bd"), "--coverage", *COMMON])
check("main" not in out, "coverage: `main` is not a public item to document")
check("broken  7/7 (100%)" in out, f"coverage: a one-file target's module is the package: {out.strip()[:300]}")

# --- The standard library (DC24) --------------------------------------------------------------------------------------

with harness.temporary_dir("burdin-doc-std-") as work:
    site = work / "site"
    out, err, status = doc(["--std", "-o", str(site), *COMMON])
    check(status == 0, f"std: expected exit status 0, got {status}: {err.strip()[:400]}")
    check("warning" not in err and "error" not in err, f"std: the standard library has no doc diagnostics: {err.strip()[:400]}")
    check(out == "", f"std: nothing goes to standard output, got {out[:100]!r}")
    files = site_files(site)
    check(f"doc: wrote {len(files)} files to" in err, f"std: unexpected summary: {err.strip()[:300]}")
    overview = page(site, "index.html")
    check("<h1>Package <span class=\"name\">std</span></h1>" in overview, "std: the overview names the package `std`")
    for module in ("iter", "fs", "io", "list", "deque"):
        check(f'href="{module}/index.html"' in overview, f"std: the overview lists the module `{module}`")
        check(f"{module}/index.html" in files, f"std: the module page {module}/index.html is written")
    check("deque/struct.Deque.html" in files and "iter/interface.Iterator.html" in files, "std: item pages are written")
    deque = page(site, "deque/struct.Deque.html")
    check("<h1>" in deque and "Deque" in deque and 'id="method.push_back"' in deque, "std: the Deque page lists its methods")
    iterator = page(site, "iter/interface.Iterator.html")
    check('id="method.peekable"' in iterator and "peekable(own self)" in iterator, "std: `peekable` is on the Iterator page")
    index = page(site, "search-index.js")
    check('"u":"fs/index.html"' in index and '"n":"peekable"' in index, "std: the search index names the modules and methods")
    check_site_integrity(site, "std site", allow_external=True)
    # No broken links in the standard library's own doc comments: a warning would fail the run.
    out, err, status = doc(["--std", "-o", str(work / "strict"), "--deny-warnings", *COMMON])
    check(status == 0, f"std: --deny-warnings passes: {status} {err.strip()[:300]}")

    # JSON: the same package, its files named as the standard library's own (`src/<module>.bd`).
    out, err, status = doc(["--std", "--output-format", "json", *COMMON])
    check(status == 0, f"std json: expected exit status 0, got {status}: {err.strip()[:300]}")
    try:
        data = json.loads(out)
    except ValueError as e:
        add_problem(f"std json: the output is not valid JSON: {e}")
        data = None
    if data is not None:
        check(data["package"] == "std" and data["format_version"] == 1, f"std json: header {data['package']}")
        modules = {m["name"]: m for m in data["modules"]}
        check({"iter", "fs", "io", "list", "deque", "std"} <= set(modules), f"std json: the modules are {sorted(modules)}")
        check(modules["iter"]["file"] == "src/iter.bd", f"std json: the module's file is {modules['iter']['file']}")
        check([m["name"] for m in data["modules"] if m["root"]] == ["std"], "std json: `std` is the root module")
        iterator = {i["name"]: i for i in modules["iter"]["items"]}["Iterator"]
        check("peekable" in [m["name"] for m in iterator["methods"]], "std json: `peekable` is a method of Iterator")
    # `--coverage` reports on the standard library too.
    out, err, status = doc(["--std", "--coverage", *COMMON])
    check(status == 0 and "documentation coverage of std" in out, f"std coverage: {status} {out.strip()[:200]} {err.strip()[:200]}")
    # With no `-o` the site goes to `target/doc/std` in the working directory.
    out, err, status = doc(["--std", *COMMON], cwd=work)
    check(status == 0 and (work / "target" / "doc" / "std" / "index.html").is_file(), f"std: default location: {status} {err.strip()[:200]}")

# --- Markup stays text ------------------------------------------------------------------------------------------------

with harness.temporary_dir("burdin-doc-escape-") as work:
    site = work / "site"
    out, err, status = doc([str(DOC / "safety" / "escape.bd"), "-o", str(site), *COMMON])
    check(status == 0, f"escape: expected exit status 0, got {status}: {err.strip()[:300]}")
    text = page(site, "fn.hostile.html")
    check("&lt;script&gt;alert(1)&lt;/script&gt;" in text and "<script>alert" not in text, "escape: a script tag in a doc comment is text")
    check("javascript:" not in text.replace("&lt;", "<").split("<a ")[0] or 'href="javascript:' not in text, "escape: no javascript: address")
    check('href="javascript:' not in text, "escape: a javascript: link is dropped")
    check("<img" not in text, "escape: an HTML tag in a doc comment is text")
    check('href="https://example.org/a?b=1&amp;c=2"' in text, "escape: an ordinary link keeps its address, escaped")
    check("&lt;b&gt;not bold&lt;/b&gt; &amp; more" in text, "escape: code block text is escaped")
    check_site_integrity(site, "escape site", allow_external=True)

# --- The output directory ------------------------------------------------------------------------------------------------

with harness.temporary_dir("burdin-doc-output-") as work:
    # A directory with other files in it is not overwritten.
    unrelated = work / "unrelated"
    unrelated.mkdir()
    harness.write_text(unrelated / "index.html", "mine\n")
    out, err, status = doc([str(GEOMETRY), "-o", str(unrelated), *COMMON])
    check(status == 2 and "not empty" in err, f"output: a directory that holds other files is refused: {status} {err.strip()[:200]}")
    check(harness.read_text(unrelated / "index.html") == "mine\n", "output: the refused directory is unchanged")
    # An empty directory is fine, and a second run replaces the pages of items that are gone.
    package = work / "pkg"
    shutil.copytree(GEOMETRY, package)
    site = work / "site"
    site.mkdir()
    out, err, status = doc([str(package), "-o", str(site), *COMMON])
    check(status == 0, f"output: an empty directory is used: {status} {err.strip()[:200]}")
    check((site / "lib" / "fn.undocumented.html").is_file(), "output: first run")
    source = package / "src" / "lib.bd"
    harness.write_text(source, harness.read_text(source).replace("fn undocumented() {}\n", ""))
    out, err, status = doc([str(package), "-o", str(site), *COMMON])
    check(status == 0, f"output: second run: {status} {err.strip()[:200]}")
    check(not (site / "lib" / "fn.undocumented.html").exists(), "output: the page of a removed item is removed")
    check((site / "lib" / "fn.distance_squared.html").is_file(), "output: the other pages remain")
    check_site_integrity(site, "second run")
    # The default place is <package>/target/doc, and a single file puts it beside the file.
    out, err, status = doc([str(package), *COMMON])
    check(status == 0 and (package / "target" / "doc" / "index.html").is_file(), f"output: default location: {status} {err.strip()[:200]}")
    single = work / "single"
    single.mkdir()
    shutil.copy(DOC / "links" / "broken.bd", single / "broken.bd")
    out, err, status = doc([str(single / "broken.bd"), *COMMON])
    check(status == 0 and (single / "target" / "doc" / "index.html").is_file(), f"output: a single file's default location: {status} {err.strip()[:200]}")
    single_index = page(single / "target" / "doc", "index.html")
    check("<h1><span class=\"kind\">package</span>" in single_index, "output: a one-file target's page is the overview")

print(f"Checked {checks} items.")
if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
