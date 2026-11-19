#!/usr/bin/env python3
"""Black-box tests for Rust interop: `burdin bindgen --rust` and `[rust.crates]` (spec/rust-interop.md, RI1-RI11).

The tool reads rustdoc's JSON, which needs the pinned Rust toolchain (the one in rust-toolchain.toml), so these tests
need `cargo` and `rustc` on `PATH`; every CI job installs them.

  * the fixture crate in tests/rust/geometry generates the module and the shim in tests/rust/expected byte for byte,
    with the notes the module's header lists on standard error;
  * `--rustdoc-json` reads a file instead of running cargo and generates the same output, and a file that is not
    rustdoc JSON, or is another format version, is refused with exit status 1;
  * the generated shim builds as a static library, and its layout checks are real: a wrong size in the shim stops
    `cargo`, and a wrong size in the Burdin module is a failed `comptime assert`;
  * a package with `[rust.crates]` builds and runs: the first build generates (and warns), the second reuses the
    generated files, a changed crate source generates again, `--emit-deps` lists the crate's sources, and a release
    build builds the shim in release;
  * a crate that does not compile is E0264, with cargo's own message;
  * usage errors, a missing crate, and a toolchain that is not the pinned one fail with the documented exit codes.
"""

import os
import re
import shutil
import stat
import sys
from pathlib import Path

import harness

RUST = harness.TESTS / "rust"
CRATE = RUST / "geometry"
EXPECTED = RUST / "expected"
PACKAGE = harness.TESTS / "m22_build" / "rust_shim"

problems = []
checks = 0


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        problems.append(message)


def output(result) -> str:
    return harness.text(result.stdout) + harness.text(result.stderr)


def pinned_toolchain() -> str:
    text = harness.read_text(harness.ROOT / "rust-toolchain.toml")
    match = re.search(r'^channel\s*=\s*"([^"]+)"', text, re.MULTILINE)
    return match.group(1) if match else ""


burdin = harness.require_burdin()
TOOLCHAIN = pinned_toolchain()

if not (harness.which("cargo") and harness.which("rustc")):
    print("error: `cargo` and `rustc` are needed on PATH", file=sys.stderr)
    sys.exit(1)

# One environment for every Rust tool: the pinned toolchain, the way the driver runs them.
RUST_ENV = dict(os.environ, RUSTUP_TOOLCHAIN=TOOLCHAIN, CARGO_TERM_COLOR="never")


def bindgen(*args, cwd=RUST, env=None):
    return harness.run([burdin, "bindgen", *args], cwd=cwd, env=env)


def warnings_of(result) -> list:
    return [line[len("warning: "):] for line in harness.text(result.stderr).splitlines() if line.startswith("warning: ")]


def notes_of(module_text: str) -> list:
    return [line[len("// bindgen: "):] for line in module_text.splitlines() if line.startswith("// bindgen: ")]


def copy_tree(source: Path, target: Path) -> None:
    shutil.copytree(source, target, ignore=shutil.ignore_patterns("target"))


def rustdoc_json(crate: Path, work: Path) -> Path:
    """rustdoc's JSON for a crate, made the way the driver makes it."""
    env = dict(RUST_ENV, RUSTC_BOOTSTRAP="1", RUSTDOCFLAGS="-Z unstable-options --output-format json")
    target = work / "doc-target"
    result = harness.run(
        ["cargo", "doc", "--no-deps", "--lib", "--manifest-path", crate / "Cargo.toml", "--target-dir", target],
        env=env,
    )
    check(result.returncode == 0, f"cargo doc failed: {output(result).strip()}")
    produced = list((target / "doc").glob("*.json"))
    check(len(produced) == 1, f"cargo doc wrote {len(produced)} JSON files")
    return produced[0] if produced else Path("missing.json")


with harness.temporary_dir("burdin-rust-") as work:
    # --- The golden outputs ---------------------------------------------------------------------------------------
    check(TOOLCHAIN != "", "rust-toolchain.toml has no `channel`")
    generated = work / "generated"
    module = generated / "geometry.bd"
    shim = generated / "shim"
    result = bindgen("--rust", str(CRATE), "--out", str(module), "--shim", str(shim), "--rust-toolchain", TOOLCHAIN)
    check(result.returncode == 0, f"bindgen --rust exited {result.returncode}: {output(result).strip()}")
    if module.is_file():
        actual_module = harness.read_text(module)
        check(actual_module == harness.read_text(EXPECTED / "geometry.bd"), "geometry.bd does not match tests/rust/expected/geometry.bd")
        check(
            harness.read_text(shim / "src" / "lib.rs") == harness.read_text(EXPECTED / "geometry_shim.rs"),
            "the shim does not match tests/rust/expected/geometry_shim.rs",
        )
        check(
            harness.read_text(shim / "rust-toolchain.toml").replace(TOOLCHAIN, "1.100.0")
            == harness.read_text(EXPECTED / "rust-toolchain.toml"),
            "the shim's rust-toolchain.toml does not pin the toolchain",
        )
        manifest = harness.read_text(shim / "Cargo.toml")
        check('crate-type = ["staticlib"]' in manifest, "the shim is not a static library")
        check('name = "geometry_burdin_shim"' in manifest, "the shim package has the wrong name")
        check(re.search(r'^geometry = \{ path = "[^"]+" \}$', manifest, re.MULTILINE) is not None, "the shim does not depend on the crate by path")
        check(warnings_of(result) == notes_of(actual_module), "the warnings on stderr are not the notes in the module")
        check(len(notes_of(actual_module)) == 6, f"expected six notes, saw {len(notes_of(actual_module))}")
    else:
        problems.append("bindgen --rust wrote no module")

    # The module and the shim do not depend on the machine: a second run in another directory is identical.
    again = work / "again"
    result = bindgen("--rust", str(CRATE), "--out", str(again / "geometry.bd"), "--shim", str(again / "shim"), "--rust-toolchain", TOOLCHAIN)
    check(result.returncode == 0, f"a second run exited {result.returncode}")
    if (again / "geometry.bd").is_file() and module.is_file():
        check(harness.read_bytes(again / "geometry.bd") == harness.read_bytes(module), "a second run wrote another module")
        check(
            harness.read_bytes(again / "shim" / "src" / "lib.rs") == harness.read_bytes(shim / "src" / "lib.rs"),
            "a second run wrote another shim",
        )

    # --- A JSON file instead of cargo -------------------------------------------------------------------------------
    json_file = rustdoc_json(CRATE, work)
    from_json = work / "from-json"
    result = bindgen(
        "--rust", str(CRATE), "--out", str(from_json / "geometry.bd"), "--shim", str(from_json / "shim"),
        "--rustdoc-json", str(json_file),
    )
    check(result.returncode == 0, f"--rustdoc-json exited {result.returncode}: {output(result).strip()}")
    if (from_json / "geometry.bd").is_file() and module.is_file():
        check(harness.read_bytes(from_json / "geometry.bd") == harness.read_bytes(module), "--rustdoc-json wrote another module")

    other_format = work / "other-format.json"
    harness.write_text(other_format, re.sub(r'"format_version":\s*\d+', '"format_version": 99', harness.read_text(json_file), count=1))
    result = bindgen(
        "--rust", str(CRATE), "--out", str(work / "x.bd"), "--shim", str(work / "x-shim"), "--rustdoc-json", str(other_format),
    )
    check(result.returncode == 1, f"another format version exited {result.returncode}, expected 1")
    check("format version 99" in output(result), "another format version is not named")
    check(not (work / "x.bd").exists(), "another format version still wrote a module")

    garbage = work / "garbage.json"
    harness.write_text(garbage, "this is not json")
    result = bindgen("--rust", str(CRATE), "--out", str(work / "y.bd"), "--shim", str(work / "y-shim"), "--rustdoc-json", str(garbage))
    check(result.returncode == 1, f"a file that is not JSON exited {result.returncode}, expected 1")
    check("not valid JSON" in output(result), "a file that is not JSON is not reported")

    result = bindgen("--rust", str(CRATE), "--out", str(work / "z.bd"), "--shim", str(work / "z-shim"), "--rustdoc-json", str(work / "nowhere.json"))
    check(result.returncode == 2, f"a missing JSON file exited {result.returncode}, expected 2")

    # --- The shim builds, and its layout checks are real ------------------------------------------------------------
    result = harness.run(["cargo", "build", "--manifest-path", shim / "Cargo.toml"], env=RUST_ENV)
    check(result.returncode == 0, f"the generated shim did not build: {output(result).strip()}")

    if (shim / "src" / "lib.rs").is_file():
        broken = work / "broken-shim"
        copy_tree(shim, broken)
        lib = broken / "src" / "lib.rs"
        text = harness.read_text(lib)
        check("size_of::<::geometry::Segment>() == 40" in text, "the shim has no size check for `Segment`")
        harness.write_text(lib, text.replace("size_of::<::geometry::Segment>() == 40", "size_of::<::geometry::Segment>() == 48"))
        # The copy depends on the crate by a path relative to the original shim directory.
        manifest = harness.read_text(broken / "Cargo.toml")
        harness.write_text(
            broken / "Cargo.toml",
            re.sub(r'path = "[^"]+"', 'path = "%s"' % CRATE.as_posix(), manifest),
        )
        result = harness.run(["cargo", "build", "--manifest-path", broken / "Cargo.toml"], env=RUST_ENV)
        check(result.returncode != 0, "a wrong size in the shim did not stop the build")
        check("evaluation panicked" in output(result), "the failed layout check is not reported as a failed evaluation")
        check("`Segment` must have size 40" in output(result), "the failed layout check does not name the type and the size")

    if module.is_file():
        wrong = work / "wrong-module"
        (wrong / "src").mkdir(parents=True)
        harness.write_text(wrong / "burdin.toml", '[package]\nname = "wrong"\nversion = "0.1.0"\n')
        text = harness.read_text(module)
        check("Segment.size() == 40" in text, "the module has no size check for `Segment`")
        harness.write_text(wrong / "src" / "geometry.bd", text.replace("Segment.size() == 40", "Segment.size() == 48"))
        harness.write_text(wrong / "src" / "main.bd", "use geometry.Segment\n\nfn main() {\n    println(\"{}\", Segment.size())\n}\n")
        result = harness.run([burdin, "build", str(wrong), "-o", str(wrong / "wrong")], cwd=wrong)
        check(result.returncode == 1, f"a wrong size in the module exited {result.returncode}, expected 1")
        check("E0223" in output(result) and "`Segment` must have size 40" in output(result), "the failed `comptime assert` is not reported")

    # --- Usage errors and a missing toolchain ---------------------------------------------------------------------------
    result = bindgen("--rust", str(CRATE), "--out", str(work / "a.bd"))
    check(result.returncode == 2 and "--shim" in output(result), "`--rust` without `--shim` is not a usage error naming `--shim`")

    result = bindgen("--rust", str(work / "no-such-crate"), "--out", str(work / "a.bd"), "--shim", str(work / "a-shim"))
    check(result.returncode == 2, f"a missing crate exited {result.returncode}, expected 2")
    check("Cargo.toml" in output(result), "a missing crate does not mention its Cargo.toml")

    result = bindgen("--rust", str(CRATE), "types.h", "--out", str(work / "a.bd"), "--shim", str(work / "a-shim"))
    check(result.returncode == 2, f"`--rust` with a header exited {result.returncode}, expected 2")

    result = bindgen("types.h", "--out", str(work / "a.bd"), "--shim", str(work / "a-shim"))
    check(result.returncode == 2 and "--rust" in output(result), "`--shim` without `--rust` is not a usage error")

    result = bindgen("--rust", str(CRATE), "--out", str(work / "a.bd"), "--shim", str(work / "a-shim"), "--rust-toolchain", "stable")
    check(result.returncode == 2, f"an inexact toolchain exited {result.returncode}, expected 2")

    if harness.PLATFORM != "windows":
        # A `rustc` that is not the pinned version: the proxy would honor RUSTUP_TOOLCHAIN, this one does not.
        fake = work / "fake-bin"
        fake.mkdir()
        script = fake / "rustc"
        harness.write_text(script, "#!/bin/sh\necho 'rustc 9.9.9 (fake 2026-01-01)'\n")
        script.chmod(script.stat().st_mode | stat.S_IEXEC)
        env = dict(os.environ, PATH=str(fake) + os.pathsep + os.environ.get("PATH", ""))
        result = bindgen("--rust", str(CRATE), "--out", str(work / "a.bd"), "--shim", str(work / "a-shim"), "--rust-toolchain", TOOLCHAIN, env=env)
        check(result.returncode == 2, f"a toolchain that is not the pinned one exited {result.returncode}, expected 2")
        check("9.9.9" in output(result) and TOOLCHAIN in output(result), "the toolchain mismatch does not name both versions")

    # --- Build integration ------------------------------------------------------------------------------------------
    app = work / "project" / "app"
    crate = work / "project" / "geometry"
    copy_tree(PACKAGE, app)
    copy_tree(CRATE, crate)
    manifest_text = harness.read_text(app / "burdin.toml").replace("../../rust/geometry", "../geometry")
    check('toolchain = "%s"' % TOOLCHAIN in manifest_text, "tests/m22_build/rust_shim does not pin the repository's toolchain")
    harness.write_text(app / "burdin.toml", manifest_text)
    program_text = harness.read_text(app / "src" / "main.bd")
    expected_lines = [line[len("//> "):] for line in program_text.splitlines() if line.startswith("//> ")]

    first = harness.run([burdin, "run", str(app)], cwd=app, env=RUST_ENV)
    check(first.returncode == 0, f"the first build exited {first.returncode}: {output(first).strip()}")
    check(harness.text(first.stdout).splitlines() == expected_lines, "the program printed other output than its `//>` lines")
    check(len(warnings_of(first)) == 6, f"the first build should warn about the six skipped items, saw {len(warnings_of(first))}")
    key = app / "target" / "burdin" / "debug" / "rust" / "geometry" / "rust.key"
    check(key.is_file(), "the build stored no key for the crate")
    check((app / "target" / "burdin" / "debug" / "rust-src" / "geometry.bd").is_file(), "the generated module is not under rust-src")

    second = harness.run([burdin, "run", str(app)], cwd=app, env=RUST_ENV)
    check(second.returncode == 0, f"the second build exited {second.returncode}: {output(second).strip()}")
    check(harness.text(second.stdout).splitlines() == expected_lines, "the second build printed other output")
    check(warnings_of(second) == [], "the second build generated again although nothing changed")

    # A changed crate source generates again, and the new function is callable.
    lib = crate / "src" / "lib.rs"
    harness.write_text(lib, harness.read_text(lib) + "\npub fn triple(value: i32) -> i32 {\n    value * 3\n}\n")
    harness.write_text(
        app / "src" / "main.bd",
        "fn main() {\n    println(\"{}\", unsafe { geometry.geometry_triple(7) })\n}\n",
    )
    third = harness.run([burdin, "run", str(app)], cwd=app, env=RUST_ENV)
    check(third.returncode == 0, f"the build after a source change exited {third.returncode}: {output(third).strip()}")
    check(harness.text(third.stdout).strip() == "21", f"the new Rust function printed {harness.text(third.stdout)!r}, expected 21")
    check(len(warnings_of(third)) == 6, f"the build after a source change should generate again (six notes, saw {len(warnings_of(third))})")

    # `--emit-deps` lists the crate's sources.
    deps = work / "deps.txt"
    built = harness.run([burdin, "build", str(app), "-o", str(app / "app-bin"), "--emit-deps", str(deps)], cwd=app, env=RUST_ENV)
    check(built.returncode == 0, f"build --emit-deps exited {built.returncode}: {output(built).strip()}")
    if deps.is_file():
        listed = harness.read_text(deps).splitlines()
        check(any(line.replace("\\", "/").endswith("geometry/src/lib.rs") for line in listed), "--emit-deps does not list the crate's lib.rs")
        check(any(line.replace("\\", "/").endswith("geometry/Cargo.toml") for line in listed), "--emit-deps does not list the crate's Cargo.toml")
    else:
        problems.append("--emit-deps wrote no file")

    # A release build builds the shim in release.
    release = harness.run([burdin, "build", str(app), "--release", "-o", str(app / "app-release")], cwd=app, env=RUST_ENV)
    check(release.returncode == 0, f"the release build exited {release.returncode}: {output(release).strip()}")
    archives = list((app / "target" / "burdin" / "release" / "rust" / "geometry" / "shim" / "target" / "release").glob("*geometry_burdin_shim*"))
    check(len(archives) > 0, "the release build left no shim library under target/release")

    # A crate that does not compile is E0264, with cargo's message.
    harness.write_text(lib, harness.read_text(lib) + "\npub fn broken( {\n")
    failed = harness.run([burdin, "build", str(app), "-o", str(app / "app-failed")], cwd=app, env=RUST_ENV)
    check(failed.returncode == 1, f"a crate that does not compile exited {failed.returncode}, expected 1")
    check("error[E0264]" in output(failed), "a crate that does not compile is not E0264")
    check("cargo doc" in output(failed), "the failure does not say which tool failed")

print(f"Checked Rust interop ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
