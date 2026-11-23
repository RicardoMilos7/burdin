#!/usr/bin/env python3
"""Checks the `http` official package (stage 3, issues #260, #328, #329, #330; spec/http-client.md).

Needs a built driver (BURDIN selects it, like tests/run_tests.py). BURDIN_PACKAGES points at a copy of this
repository's `packages/` folder, so nothing touches an installed compiler.

What it checks:
  * every fixture under `tests/http` against its `.expected` golden. Each fixture is a text file of one-line cases and
    its mode is its folder: `url` (parsing, HC1), `join` (reference resolution, HC2), `headers` (HC4), `request`
    (the request head, HC5 and HC6), `head` (the response head and the framing it implies, HC7 and HC8), and `body`
    (the Content-Length, chunked, and close-delimited decoders over every read size, HC8 to HC10);
  * the conformance program `tests/s3_http/client`, which runs the client against a scripted server in the same
    process (requests, redirects, timeouts, retries, and connection reuse, HC11 to HC16): its output is compared with
    the `//>` lines of its `src/main.bd`, the way tests/run_tests.py does;
  * the conformance program `tests/s3_http/tls`, the same over TLS with the test certificates (`https`, HC17); `http`
    imports `tls`, so building any program that uses `http` needs OpenSSL 3 to link (spec/tls.md, TLS7);
  * `use http` loads the package with no manifest entry, and `burdin.lock` pins it
    with `source = "official"` and the PKG7 checksum of its files, and pins the `tls` package it depends on too;
  * `--locked` accepts the pin, rejects a changed package by name, and a plain build re-pins it.
"""

import hashlib
import os
import re
import shutil
import sys
from pathlib import Path

import harness

problems = []
checks = 0


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        problems.append(message)


burdin = harness.require_burdin()

FIXTURES = harness.TESTS / "http"
PACKAGES = harness.ROOT / "packages"
MANIFEST = '[package]\nname = "httpcheck"\nversion = "0.1.0"\n'

# The program the fixtures drive. It reads a fixture's path from the command line (so a non-ASCII fixture never
# depends on how a platform spells an argument). The first mode set drives the serialization mapping with a
# configuration type, the second drives the DOM.
PROGRAM = r"""use http
use http.{BodyReader, Framing, Headers, Method, Request, Url}

// ----- helpers --------------------------------------------------------------------------------------------------

/// The text of a fixture line with `\r`, `\n`, `\t`, and `\\` turned into the characters they name.
fn unescape(text: str): String {
    let mut out = String.new()
    let mut pending = false
    for c in text.chars() {
        if pending {
            pending = false
            if c == 'r' {
                out.push('\r')
            } else if c == 'n' {
                out.push('\n')
            } else if c == 't' {
                out.push('\t')
            } else {
                out.push(c)
            }
        } else if c == '\\' {
            pending = true
        } else {
            out.push(c)
        }
    }
    out
}

fn opt(value: Option<String>): String {
    match value {
        Some(text) => format("{:?}", text),
        None => "none".to_string(),
    }
}

fn show_url(url: Url) {
    println(
        "  scheme={} host={} port={} path={} query={} authority={} target={} origin={}",
        url.scheme,
        url.host,
        url.port,
        url.path,
        opt(url.query.clone()),
        url.authority(),
        url.target(),
        url.origin(),
    )
    println("  text={}", url.to_text())
}

fn show_error(e: http.HttpError) {
    println("  error: {}", e)
}

// ----- one mode per kind of fixture -----------------------------------------------------------------------------

fn url_case(line: str) {
    match Url.parse(line) {
        Ok(url) => show_url(url),
        Err(e) => show_error(e),
    }
}

/// `base | reference`
fn join_case(line: str) {
    let parts = line.split(" | ")
    match Url.parse(parts[0].as_str()) {
        Ok(base) => match base.join(parts[1].as_str()) {
            Ok(url) => println("  {}", url.to_text()),
            Err(e) => show_error(e),
        },
        Err(e) => show_error(e),
    }
}

/// One header operation per line, on one `Headers` that lives through the whole file.
fn headers_case(mut headers: Headers, line: str) {
    let parts = line.split(" ")
    let op = parts[0].as_str()
    let rest = match line.find(' ') {
        Some(at) => line[at + 1..line.len()].to_string(),
        None => String.new(),
    }
    if op == "add" || op == "set" {
        let colon = rest.find(':').unwrap()
        let name = unescape(rest.as_str()[0..colon].trim())
        let value = unescape(rest.as_str()[colon + 1..rest.len()].trim_start())
        let result = if op == "add" { headers.add(name.as_str(), value.as_str()) } else { headers.set(name.as_str(), value.as_str()) }
        match result {
            Ok(_) => println("  ok len={}", headers.len()),
            Err(e) => show_error(e),
        }
    } else if op == "get" {
        println("  {}", opt(headers.get(rest.as_str())))
    } else if op == "all" {
        println("  {:?}", headers.get_all(rest.as_str()))
    } else if op == "contains" {
        println("  {}", headers.contains(rest.as_str()))
    } else if op == "remove" {
        println("  removed {}", headers.remove(rest.as_str()))
    } else if op == "token" {
        let two = rest.split(" ")
        println("  {}", headers.has_token(two[0].as_str(), two[1].as_str()))
    } else if op == "wire" {
        println("  {:?}", headers.to_wire())
    } else {
        println("  unknown operation")
    }
}

/// `METHOD url | Name: value ; Name: value | body`; prints the request head as the client sends it, twice: as is, and
/// with `Connection: close`.
fn request_case(line: str) {
    let parts = line.split(" | ")
    let first = parts[0].split(" ")
    let method = match Method.parse(first[0].as_str()) {
        Ok(m) => m,
        Err(e) => {
            show_error(e)
            return
        }
    }
    let mut request = match Request.new(method, first[1].as_str()) {
        Ok(r) => r,
        Err(e) => {
            show_error(e)
            return
        }
    }
    if parts.len() > 1 && !parts[1].is_empty() {
        for field in parts[1].split(" ; ") {
            let colon = field.find(':').unwrap()
            request = request.header(field.as_str()[0..colon].trim(), unescape(field.as_str()[colon + 1..field.len()].trim()).as_str())
        }
    }
    if parts.len() > 2 {
        request = request.body_text(unescape(parts[2].as_str()).as_str())
    }
    println("  {:?}", request.to_wire_head("burdin-http/0.1", false))
    println("  {:?}", request.to_wire_head("burdin-http/0.1", true))
}

/// A response head, with `\r\n` written out: the parsed status line and fields, then the framing for `GET` and `HEAD`.
fn head_case(line: str) {
    let text = unescape(line)
    let bytes = text.to_utf8()
    let mut buffer = List<u8>.new()
    buffer.extend_from_slice(bytes[0..bytes.len()])
    match http.find_head_end(buffer) {
        Some(end) => println("  head ends at {} of {}", end, bytes.len()),
        None => println("  head is not complete"),
    }
    match http.parse_response_head(bytes[0..bytes.len()]) {
        Ok(head) => {
            println("  HTTP/1.{} {} {:?} fields={}", head.minor, head.status, head.reason, head.headers.len())
            for n in 0..head.headers.len() {
                println("    {}: {}", head.headers.name_at(n), head.headers.value_at(n))
            }
            println("  keeps alive: {}", http.keeps_alive(head.minor, head.headers.clone()))
            show_framing("GET", http.framing_of(Method.Get, head.status, head.headers.clone()))
            show_framing("HEAD", http.framing_of(Method.Head, head.status, head.headers.clone()))
        }
        Err(e) => show_error(e),
    }
}

fn show_framing(method: str, framing: Result<Framing, http.HttpError>) {
    match framing {
        Ok(Framing.None) => println("  framing for {}: no body", method),
        Ok(Framing.Length(n)) => println("  framing for {}: {} bytes", method, n),
        Ok(Framing.Chunked) => println("  framing for {}: chunked", method),
        Ok(Framing.UntilClose) => println("  framing for {}: until close", method),
        Err(e) => println("  framing for {}: error {}", method, e),
    }
}

/// An in-memory source that hands out at most `step` bytes per read, so the decoder sees every split of its input.
struct Mem {
    data: List<u8>
    at: usize
    step: usize
}

Mem: Read {
    fn read(mut self, mut buf: List<u8>, max: usize): Result<usize, IoError> {
        let mut count = self.data.len() - self.at
        if count > self.step {
            count = self.step
        }
        if count > max {
            count = max
        }
        for i in 0..count {
            buf.push(self.data[self.at + i])
        }
        self.at = self.at + count
        Ok(count)
    }
}

/// `framing | step | bytes`: framing is `chunked`, `until-close`, or `length N`; step is how many bytes a read gives at
/// most; bytes are the encoded body.
fn body_case(line: str) {
    let parts = line.split(" | ")
    let spec = parts[0].as_str()
    let framing = if spec == "chunked" {
        Framing.Chunked
    } else if spec == "until-close" {
        Framing.UntilClose
    } else {
        Framing.Length(u64.parse(spec[7..spec.len()]).unwrap())
    }
    let step = usize.parse(parts[1].as_str()).unwrap()
    let text = unescape(parts[2].as_str())
    let encoded = text.to_utf8()
    let mut data = List<u8>.new()
    data.extend_from_slice(encoded[0..encoded.len()])
    let mem = Mem { data: data, at: 0, step: step }
    let mut reader = BodyReader.new(mem, framing, List<u8>.new())
    let mut out = List<u8>.new()
    let mut failure = String.new()
    loop {
        match reader.read_body(mut out, 7) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                failure = e.to_string()
                break
            }
        }
    }
    println("  body={:?}", String.from_utf8_lossy(out[0..out.len()]))
    if failure.is_empty() {
        println("  complete={} clean={} trailers={}", reader.is_complete(), reader.is_clean(), reader.trailers().len())
        let trailers = reader.trailers()
        for n in 0..trailers.len() {
            println("    {}: {}", trailers.name_at(n), trailers.value_at(n))
        }
    } else {
        println("  error: {}", failure)
    }
}

fn main() {
    let args = env.args()
    let mode = args[1].clone()
    let text = read_to_string(args[2].as_str()).unwrap()
    let mut headers = Headers.new()
    for line in text.lines() {
        if line.is_empty() || line.starts_with("#") {
            continue
        }
        println("> {}", line)
        if mode.as_str() == "url" {
            url_case(line)
        } else if mode.as_str() == "join" {
            join_case(line)
        } else if mode.as_str() == "headers" {
            headers_case(mut headers, line)
        } else if mode.as_str() == "request" {
            request_case(line)
        } else if mode.as_str() == "head" {
            head_case(line)
        } else {
            body_case(line)
        }
    }
}
"""
# Each mode reads one fixture file of one-line cases and prints each case followed by its result.
MODES = ("url", "join", "headers", "request", "head", "body")
MINIMUM = {"url": 1, "join": 1, "headers": 1, "request": 1, "head": 1, "body": 1}


def first_difference(got, wanted) -> str:
    for index in range(max(len(got), len(wanted))):
        a = got[index] if index < len(got) else "(missing)"
        b = wanted[index] if index < len(wanted) else "(missing)"
        if a != b:
            return f"line {index + 1} is {a!r}, expected {b!r}"
    return "no difference"


def tree_checksum(directory: Path) -> str:
    """PKG7: `sha256:` and the SHA-256 of every file's path, a NUL, its length, a NUL, and its bytes, by path."""
    files = []
    for path in directory.rglob("*"):
        relative = path.relative_to(directory).as_posix()
        if path.is_file() and relative.split("/")[0] not in (".git", "target"):
            files.append((relative, path))
    digest = hashlib.sha256()
    for relative, path in sorted(files):
        data = path.read_bytes()
        digest.update(relative.encode() + b"\0" + str(len(data)).encode() + b"\0" + data)
    return "sha256:" + digest.hexdigest()


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    harness.write_text(path, content)


def snapshot(directory: Path):
    def read(name):
        path = directory / name
        return harness.read_bytes(path) if path.exists() else None

    return read("burdin.toml"), read("burdin.lock")


manifest_text = harness.read_text(PACKAGES / "http" / "burdin.toml")
check(re.search(r'^name = "http"$', manifest_text, re.M) is not None, "packages/http does not declare `name = \"http\"`")
check(re.search(r'^version = "\d+\.\d+\.\d+"$', manifest_text, re.M) is not None, "packages/http has no version")
check((PACKAGES / "http" / "src" / "http.bd").is_file(), "packages/http has no src/http.bd")
readme = PACKAGES / "http" / "README.md"
check(readme.is_file() and "use http" in harness.read_text(readme), "packages/http/README.md is missing or does not show `use http`")

with harness.temporary_dir("burdin-http-") as work:
    env = dict(os.environ)
    env["BURDIN_HOME"] = str(work / "home")
    official = work / "packages"
    shutil.copytree(PACKAGES, official)
    env["BURDIN_PACKAGES"] = str(official)

    def run_in(directory, *arguments, environment=None):
        return harness.run([burdin, *arguments], cwd=directory, env=environment or env)

    def out(result):
        return harness.text(harness.normalize(result.stdout))

    def err(result):
        return harness.text(harness.normalize(result.stderr))

    app = work / "app"
    write(app / "burdin.toml", MANIFEST)
    write(app / "src" / "main.bd", PROGRAM)
    built = run_in(app, "build", ".")
    check(built.returncode == 0, f"the http package did not build: {err(built)[:400]}")
    binary = harness.executable_in(app, "httpcheck")
    check(binary.is_file(), "`burdin build .` did not produce the program")

    if binary.is_file():
        for mode in MODES:
            folder = mode
            fixtures = sorted((FIXTURES / folder).glob("*.txt"))
            check(len(fixtures) >= MINIMUM[folder], f"tests/http/{folder} has too few fixtures ({len(fixtures)})")
            for fixture in fixtures:
                expected = fixture.with_suffix(".expected")
                check(expected.is_file(), f"{folder}/{fixture.name} has no .expected file")
                if not expected.is_file():
                    continue
                result = harness.run([binary, mode, str(fixture)], cwd=app, env=env)
                actual = harness.normalize(result.stdout)
                wanted = harness.normalize(harness.read_bytes(expected))
                check(result.returncode == 0, f"{folder}/{fixture.name} ended with status {result.returncode}: {err(result)[:300]}")
                check(
                    actual == wanted,
                    f"{folder}/{fixture.name} printed {actual!r}, expected {wanted!r}",
                )

    # The conformance programs: the client against a scripted server in the same process, over plain TCP
    # (tests/s3_http/client) and over TLS (tests/s3_http/tls, which links OpenSSL through the `tls` package). Each is built
    # from a copy, so its `burdin.lock` and `target` stay out of the tree, and its output is the `//>` lines of its main
    # file.
    for folder, program, minimum in (("client", "http_conformance", 100), ("tls", "http_tls_conformance", 70)):
        conformance = work / f"conformance_{folder}"
        shutil.copytree(harness.TESTS / "s3_http" / folder, conformance)
        wanted_lines = [
            line[4:] if line.startswith("//> ") else line[3:]
            for line in harness.read_text(conformance / "src" / "main.bd").splitlines()
            if line.startswith("//>")
        ]
        check(len(wanted_lines) >= minimum, f"tests/s3_http/{folder} expects too few lines ({len(wanted_lines)})")
        conformance_built = run_in(conformance, "build", ".")
        check(conformance_built.returncode == 0, f"tests/s3_http/{folder} did not build: {err(conformance_built)[:400]}")
        conformance_binary = harness.executable_in(conformance, program)
        if conformance_binary.is_file():
            ran = harness.run([conformance_binary], cwd=conformance, env=env, timeout=180)
            check(ran.returncode == 0, f"tests/s3_http/{folder} ended with status {ran.returncode}: {err(ran)[:300]}")
            got_lines = out(ran).splitlines()
            check(got_lines == wanted_lines, f"tests/s3_http/{folder} printed other lines than its //> lines: " + first_difference(got_lines, wanted_lines))
        else:
            check(False, f"`burdin build .` did not produce the {folder} conformance program")

    # The README's https example must compile: every ```burdin block that starts with `// example: name` is
    # type-checked as a program of its own; `use http` and `use tls` load both packages with no manifest entry.
    examples = re.findall(r"```burdin\n(// example: (\w+)\n.*?)```", harness.read_text(readme), re.S)
    check(len(examples) >= 1, "packages/http/README.md has no checked example")
    for source, name in examples:
        example = work / f"example_{name}"
        write(example / "burdin.toml", f'[package]\nname = "example_{name}"\nversion = "0.1.0"\n')
        write(example / "src" / "main.bd", source)
        checked = run_in(example, "check", ".")
        check(checked.returncode == 0, f"the README's {name} example does not compile: {err(checked)[:500]}")

    # The lockfile pins the package's files with the official source (PKG15, PKG17).
    lock = harness.read_text(app / "burdin.lock")
    check('source = "official"' in lock, "burdin.lock does not record the official source")
    checksum = tree_checksum(official / "http")
    check(f'checksum = "{checksum}"' in lock, "burdin.lock does not pin the http package's files")
    # `http` imports `tls`, so an official package reaches another and both are pinned.
    check(f'checksum = "{tree_checksum(official / "tls")}"' in lock, "burdin.lock does not pin the tls package http depends on")
    check('dependencies = ["tls"]' in lock, "burdin.lock does not record that http depends on tls")
    locked = run_in(app, "build", ".", "--locked")
    check(locked.returncode == 0, f"`--locked` rejected the fresh lockfile: {err(locked)[:300]}")

    # A changed package is a difference the lockfile reports by name, and a plain build re-pins it (PKG17).
    before = snapshot(app)
    write(official / "http" / "src" / "http.bd", harness.read_text(official / "http" / "src" / "http.bd") + "\n// changed\n")
    changed = run_in(app, "build", ".", "--locked")
    check(changed.returncode == 1, f"`--locked` accepted a changed http package (status {changed.returncode})")
    check(
        "`http` is an official package whose files are not the ones burdin.lock pinned" in err(changed),
        f"`--locked` did not name the changed package: {err(changed)[:400]}",
    )
    check(snapshot(app) == before, "`--locked` changed burdin.toml or burdin.lock")
    repinned = run_in(app, "build", ".")
    check(repinned.returncode == 0, f"a plain build did not re-pin the changed package: {err(repinned)[:300]}")
    check(f'checksum = "{tree_checksum(official / "http")}"' in harness.read_text(app / "burdin.lock"), "the re-pin kept the old checksum")
    locked_again = run_in(app, "build", ".", "--locked")
    check(locked_again.returncode == 0, f"`--locked` rejected the re-pinned lockfile: {err(locked_again)[:300]}")

print(f"Checked the http package ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
