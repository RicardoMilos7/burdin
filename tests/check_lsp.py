#!/usr/bin/env python3
"""Checks `burdin lsp` from the driver and the editor files (stage 3, issue #242; spec/lsp.md).

The JSON-RPC integration tests of the server are `cargo test -p burdin_lsp`; they start the crate's own `burdin-lsp`
binary, which cannot run tests because it has no back end. This script covers what needs the real driver and what is
not Rust.

Needs a built driver for the second part. BURDIN selects it, like tests/run_tests.py.

What it checks:
  * the editor files (LS20): the VS Code manifest, grammar, and language configuration parse; the extension starts
    `burdin lsp`; the settings it declares are exactly the ones spec/lsp.md (LS5) lists; the grammar's keywords are
    exactly the lexer's (crates/burdin_syntax/src/token.rs), the reserved word included;
  * `burdin lsp` (LS1, LS2): it initializes, publishes the diagnostics of an opened file with an error, and exits 0
    after shutdown and exit; an unknown argument is a usage error (exit status 2);
  * the test lenses (LS19): the lens above a passing test runs it through `workspace/executeCommand` with the real
    `burdin test`, the answer counts it as passed, and the next lenses show `✓ passed`; the lens of a file runs every
    test, a failing one included, and the failing test's lens shows `✗ failed` with the reason.
"""

import json
import queue
import re
import shutil
import subprocess
import sys
import threading
from pathlib import Path

import harness

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


def check(condition: bool, message: str) -> bool:
    global checks
    checks += 1
    if not condition:
        add_problem(message)
    return condition


ROOT = harness.ROOT
EDITOR = ROOT / "editors" / "vscode"
SETTINGS = [
    "inlayHints.types",
    "inlayHints.parameterNames",
    "diagnostics.lints",
    "diagnostics.debounceMs",
    "formatting.enabled",
    "codeLens.enabled",
    "path",
]


def load_json(path: Path):
    try:
        return json.loads(harness.read_text(path))
    except (OSError, ValueError) as e:
        add_problem(f"{path.relative_to(ROOT).as_posix()}: {e}")
        return None


def lexer_keywords() -> set:
    source = harness.read_text(ROOT / "crates" / "burdin_syntax" / "src" / "token.rs")
    body = source[source.index("pub fn keyword(") :]
    body = body[: body.index("_ => return None")]
    return set(re.findall(r'"([A-Za-z]+)" =>', body))


def grammar_keywords(grammar) -> set:
    words = set()
    for pattern in grammar.get("repository", {}).get("keywords", {}).get("patterns", []):
        match = re.fullmatch(r"\\b\(([A-Za-z|]+)\)\\b", pattern.get("match", ""))
        if check(match is not None, f"grammar: keyword pattern {pattern.get('match')!r} is not `\\b(a|b)\\b`"):
            words.update(match.group(1).split("|"))
    return words


def check_editor_files() -> None:
    manifest = load_json(EDITOR / "package.json")
    grammar = load_json(EDITOR / "syntaxes" / "burdin.tmLanguage.json")
    configuration = load_json(EDITOR / "language-configuration.json")
    extension = harness.read_text(EDITOR / "extension.js")
    check(configuration is not None, "language-configuration.json does not parse")
    if manifest is not None:
        declared = set(manifest["contributes"]["configuration"]["properties"])
        expected = {f"burdin.{s}" for s in SETTINGS}
        check(declared == expected, f"package.json settings {sorted(declared)} are not LS5's {sorted(expected)}")
        languages = manifest["contributes"]["languages"]
        check(any(".bd" in l.get("extensions", []) for l in languages), "package.json: no language for `.bd`")
        grammars = manifest["contributes"]["grammars"]
        check(
            any((EDITOR / g["path"]).is_file() and g["scopeName"] == "source.burdin" for g in grammars),
            "package.json: the grammar is missing",
        )
        commands = {c["command"] for c in manifest["contributes"]["commands"]}
        check({"burdin.run", "burdin.debug"} <= commands, f"package.json: the client commands are missing: {commands}")
        dependency = manifest.get("dependencies", {}).get("vscode-languageclient", "")
        check(re.fullmatch(r"\d+\.\d+\.\d+", dependency) is not None, f"vscode-languageclient is not pinned: {dependency!r}")
    check('args: ["lsp"]' in extension, "extension.js does not start `burdin lsp`")
    for command in ("burdin.run", "burdin.debug"):
        check(f'registerCommand("{command}"' in extension, f"extension.js does not implement `{command}`")
    spec = harness.read_text(ROOT / "spec" / "lsp.md")
    for setting in SETTINGS:
        check(f"| `{setting}` |" in spec, f"spec/lsp.md (LS5) does not list `{setting}`")
    if grammar is not None:
        lexer, ours = lexer_keywords(), grammar_keywords(grammar)
        check(len(lexer) > 30, f"could not read the lexer's keywords from token.rs ({len(lexer)})")
        check(ours == lexer, f"grammar keywords differ from the lexer's: missing {sorted(lexer - ours)}, extra {sorted(ours - lexer)}")


class Server:
    """`burdin lsp` as a child process, with a reader thread."""

    def __init__(self, burdin: str, cwd: Path):
        self.process = subprocess.Popen(
            [burdin, "lsp"], cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL
        )
        self.messages = queue.Queue()
        self.kept = []
        self.next_id = 0
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self) -> None:
        out = self.process.stdout
        while True:
            length = None
            while True:
                line = out.readline()
                if not line:
                    self.messages.put(None)
                    return
                line = line.strip()
                if not line:
                    break
                name, _, value = line.partition(b":")
                if name.strip().lower() == b"content-length":
                    length = int(value)
            self.messages.put(json.loads(out.read(length or 0).decode("utf-8")))

    def send(self, message: dict) -> None:
        body = json.dumps(dict(message, jsonrpc="2.0")).encode("utf-8")
        self.process.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        self.process.stdin.flush()

    def receive(self, timeout: float = 120):
        message = self.messages.get(timeout=timeout)
        if message is None:
            raise EOFError("the server closed its output")
        if "method" in message and "id" in message:
            # A request from the server (progress tokens, watcher registration): accept it.
            self.send({"id": message["id"], "result": None})
        return message

    def request(self, method: str, params) -> dict:
        self.next_id += 1
        ident = self.next_id
        self.send({"id": ident, "method": method, "params": params})
        while True:
            message = self.receive()
            if "method" not in message and message.get("id") == ident:
                return message
            self.kept.append(message)

    def notify(self, method: str, params) -> None:
        self.send({"method": method, "params": params})

    def wait_for(self, method: str, predicate) -> dict:
        while True:
            for i, message in enumerate(self.kept):
                if message.get("method") == method and predicate(message.get("params")):
                    return self.kept.pop(i)["params"]
            self.kept.append(self.receive())

    def exit_code(self) -> int:
        self.request("shutdown", None)
        self.notify("exit", None)
        self.process.stdin.close()
        return self.process.wait(timeout=60)


def uri(path: Path) -> str:
    return path.resolve().as_uri()


def lens_titles(lenses) -> list:
    return [lens["command"]["title"] for lens in lenses]


def execute(server: Server, lens: dict) -> dict:
    command = lens["command"]
    return server.request("workspace/executeCommand", {"command": command["command"], "arguments": command["arguments"]})


def check_server(burdin: str) -> None:
    usage = harness.run([burdin, "lsp", "--bogus"])
    check(usage.returncode == 2, f"`burdin lsp --bogus` exited {usage.returncode}, expected 2")

    fixture = ROOT / "crates" / "burdin_lsp" / "tests" / "fixtures" / "geometry"
    with harness.temporary_dir("burdin-lsp-") as work:
        package = work / "geometry"
        shutil.copytree(fixture, package)
        main = package / "src" / "main.bd"
        text = harness.read_text(main) + '\ntest "fails" {\n    assert_eq(total(2, 2), 5)?\n}\n'
        harness.write_text(main, text)

        server = Server(burdin, work)
        initialized = server.request(
            "initialize",
            {
                "processId": None,
                "capabilities": {"window": {"workDoneProgress": True}},
                "workspaceFolders": [{"uri": uri(package), "name": "geometry"}],
                "initializationOptions": {"path": burdin, "diagnostics": {"debounceMs": 10}},
            },
        )
        check("result" in initialized, f"initialize failed: {initialized}")
        server.notify("initialized", {})
        broken = text.replace('let label = "área"', 'let label: i32 = "área"')
        document = {"uri": uri(main), "languageId": "burdin", "version": 1, "text": broken}
        server.notify("textDocument/didOpen", {"textDocument": document})
        published = server.wait_for("textDocument/publishDiagnostics", lambda p: p["uri"] == uri(main))
        codes = [d.get("code") for d in published["diagnostics"]]
        check(codes == ["E0003"], f"the opened file's diagnostics are {codes}, expected ['E0003']")
        server.notify(
            "textDocument/didChange",
            {"textDocument": {"uri": uri(main), "version": 2}, "contentChanges": [{"text": text}]},
        )
        server.wait_for(
            "textDocument/publishDiagnostics", lambda p: p["uri"] == uri(main) and p.get("version") == 2
        )

        lenses = server.request("textDocument/codeLens", {"textDocument": {"uri": uri(main)}}).get("result") or []
        titles = lens_titles(lenses)
        check(titles[:1] == ["▶ Run 3 tests in file"], f"the lenses are {titles}")
        one = next((l for l in lenses if l["command"]["command"] == "burdin.runTest"), None)
        if check(one is not None, "no `burdin.runTest` lens"):
            argv = one["command"]["arguments"][0]["argv"]
            check(argv[0] == burdin and argv[1] == "test" and argv[3:] == ["--exact", "total adds"], f"the lens runs {argv}")
            ran = execute(server, one)
            result = ran.get("result") or {}
            check(
                result.get("passed") == 1 and result.get("failed") == 0,
                f"running `total adds` answered {ran}",
            )
            check(
                [r.get("path") for r in result.get("results", [])] == ["total adds"],
                f"the run's results are {result.get('results')}",
            )
            lenses = server.request("textDocument/codeLens", {"textDocument": {"uri": uri(main)}})["result"]
            check(any(t.startswith("✓ passed (") for t in lens_titles(lenses)), f"no result lens after the run: {lens_titles(lenses)}")

        everything = execute(server, lenses[0])
        result = everything.get("result") or {}
        check(result.get("passed") == 2 and result.get("failed") == 1, f"running the file's tests answered {everything}")
        lenses = server.request("textDocument/codeLens", {"textDocument": {"uri": uri(main)}})["result"]
        failed = [t for t in lens_titles(lenses) if t.startswith("✗ failed")]
        check(len(failed) == 1, f"the failing test has no `✗ failed` lens: {lens_titles(lenses)}")
        code = server.exit_code()
        check(code == 0, f"`burdin lsp` exited {code} after shutdown and exit, expected 0")


check_editor_files()
check_server(harness.require_burdin())

print(f"Checked `burdin lsp` and the editor files ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
