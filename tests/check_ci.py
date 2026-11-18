#!/usr/bin/env python3
"""Checks that the CI workflow still covers everything the project requires.
It does not compile anything. Usage: tests/check_ci.py

This guards the invariants behind the CI jobs (issues #11, #12, #16, #17, #18, #47):
  * the linux, macos, and windows jobs, and every required step, are present;
  * Linux installs LLVM 21 from apt.llvm.org; macOS installs Homebrew's llvm@21 and points
    LLVM_SYS_211_PREFIX and PATH at it; Windows loads the MSVC environment, installs the official LLVM 21
    archive, and puts its DLL directory on PATH;
  * macOS runs on macos-latest (Apple Silicon) and Windows on windows-latest; every job checks the host target;
  * every job passes every milestone folder under tests/ (m01..., and the stage 2 folders s2_...) to the conformance
    runner in both the debug and release profiles; the release step uses --release, and the runner skips tests
    marked debug-only;
  * every job repeats the channel, shared-state, and I/O stress tests in both profiles (`--repeat`), because a lost
    wake-up or a race shows up in one run in many (issues #209, #220);
  * the Windows job also runs the suite with BURDIN_CC=cl;
  * the scripts CI runs exist, are executable, and use LF endings;
  * the Rust toolchain is pinned to an exact version with clippy and rustfmt, and CI installs and verifies
    that pin;
  * every job caches the cargo registry and target/ with a key built from the pinned toolchain and
    Cargo.lock, and caches LLVM 21: the installed tree on Linux, the Homebrew bottle downloads on macOS;
  * the compiler benchmarks exist and CI runs their smoke budgets;
  * every job runs the workspace unit tests and the front-end unit-test guard;
  * every job runs the checks for debug information, the code-generation optimizations, the build flags and
    metadata, and parallel parsing;
  * every job runs the advisory benchmark regression check, keeps its baseline in a cache scoped to the
    runner, the pinned toolchain, and Cargo.lock, and records a new baseline on a push to main;
  * the workflow runs the lexer/parser fuzzer: the committed corpus replays through `cargo test --workspace`
    in every job, a scheduled `fuzz` job runs the longer session through tests/fuzz.py, and it uploads the
    failing inputs so they can be committed with the fix;
  * a conformance test still exercises --link-lib LLVM-C;
  * the documentation check (which reads the driver contract in README.md) runs in every job;
  * every job runs the diagnostics presentation check and the error-code reference check;
  * every job runs the black-box check of the test runner, of the formatter, and of `burdin bindgen`;
  * every job runs the dependency and license audit after `cargo build`, with the license metadata
    verification required;
  * every job runs the platform support check, and docs/platform-support.md exists and is linked;
  * every job runs the release-pipeline check.
"""

import re
import sys

import harness

WORKFLOW = harness.ROOT / ".github" / "workflows" / "ci.yml"

problems = []


def add_problem(message: str) -> None:
    problems.append(message)


if not WORKFLOW.is_file():
    print(f"missing {WORKFLOW}", file=sys.stderr)
    sys.exit(1)
text = harness.read_text(WORKFLOW)

# The Rust toolchain is pinned so fmt and clippy are reproducible (issue #19).
toolchain_file = harness.ROOT / "rust-toolchain.toml"
if not toolchain_file.is_file():
    add_problem("rust-toolchain.toml: missing (the Rust toolchain is not pinned)")
else:
    toolchain = harness.read_text(toolchain_file)
    match = re.search(r'^channel = "([0-9][0-9.]*)"$', toolchain, re.MULTILINE)
    channel = match.group(1) if match else ""
    if not channel:
        add_problem(
            "rust-toolchain.toml: no channel (an exact x.y.z version is required, not stable/nightly)"
        )
    elif not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", channel):
        add_problem(f"rust-toolchain.toml: channel '{channel}' is not an x.y.z version")
    for component in ("clippy", "rustfmt"):
        if f'"{component}"' not in toolchain:
            add_problem(f"rust-toolchain.toml: components do not include {component}")

if re.search(r"rustup (toolchain install|default)[^#]*stable", text):
    add_problem("ci.yml: installs a floating 'stable' toolchain instead of the pinned one")
if "rust-toolchain.toml" not in harness.read_text(harness.ROOT / "CLAUDE.md"):
    add_problem("CLAUDE.md: does not document the rust-toolchain.toml pin")


def require_command(job: str, block: str, command: str) -> None:
    if command not in block:
        add_problem(f"ci.yml: {job} job: missing step: {command}")


def profile_conformance(block: str, profile: str) -> str:
    # The BURDIN_CC step runs a subset, so it is never the full-suite step for either profile, and neither is
    # the repeated stress step, which runs a few tests many times.
    steps = [
        line
        for line in block.split("\n")
        if "tests/run_tests.py" in line and "BURDIN_CC" not in line and "--repeat" not in line
    ]
    if profile == "release":
        matching = [line for line in steps if "--release" in line]
    else:
        matching = [line for line in steps if "--release" not in line]
    return matching[-1] if matching else ""


def check_conformance(job: str, block: str, profile: str) -> None:
    label = "release-profile" if profile == "release" else "debug-profile"
    conformance = profile_conformance(block, profile)
    if not conformance:
        add_problem(f"ci.yml: {job} job: no {label} conformance step (tests/run_tests.py)")
        return
    for directory in sorted(harness.TESTS.glob("m[0-9]*_*")):
        if not directory.is_dir():
            continue
        name = directory.name.split("_", 1)[0]
        if not re.search(rf"(^| ){re.escape(name)}( |$)", conformance):
            add_problem(f"ci.yml: {job} job: {label} conformance step omits {name} ({directory.name})")
    for directory in sorted(harness.TESTS.glob("s[0-9]_*")):
        if directory.is_dir() and not re.search(rf"(^| ){re.escape(directory.name)}( |$)", conformance):
            add_problem(f"ci.yml: {job} job: {label} conformance step omits {directory.name}")
    for word in conformance.split():
        if re.match(r"m[0-9]*", word) and word.startswith("m"):
            if not any(harness.TESTS.glob(word + "_*")):
                add_problem(
                    f"ci.yml: {job} job: {label} conformance step names missing milestone {word}"
                )


def check_stress_repeat(job: str, block: str) -> None:
    """Every job repeats the channel, shared-state, and I/O stress tests in the debug and the release profile."""
    repeated = [line for line in block.split("\n") if "tests/run_tests.py" in line and "--repeat" in line]
    for label, wanted in (("debug-profile", False), ("release-profile", True)):
        for folder, what in (
            ("s2_channels/stress", "channel"),
            ("s2_shared/stress", "shared-state"),
            ("s2_io/stress", "I/O"),
        ):
            matching = [line for line in repeated if ("--release" in line) == wanted and folder in line]
            if not matching:
                add_problem(f"ci.yml: {job} job: no {label} repeated {what} stress step (run_tests.py --repeat)")


def cache_steps(block: str) -> str:
    """Only the steps that use actions/cache."""
    groups = []
    current = None
    for line in block.split("\n"):
        if line.startswith("      - "):
            if current is not None:
                groups.append(current)
            current = line + "\n"
        elif current is not None:
            current += line + "\n"
    if current is not None:
        groups.append(current)
    return "\n".join(group for group in groups if "actions/cache@" in group)


def check_caches(job: str, block: str) -> None:
    caches = cache_steps(block)
    if not caches:
        add_problem(f"ci.yml: {job} job: no actions/cache step")
        return
    if "uses: actions/cache@v4" not in caches:
        add_problem(f"ci.yml: {job} job: caches do not use actions/cache@v4")
    if "~/.cargo/registry" not in caches:
        add_problem(f"ci.yml: {job} job: cargo cache omits ~/.cargo/registry")
    if not re.search(r"^[ \t]+target$", caches, re.MULTILINE):
        add_problem(f"ci.yml: {job} job: cargo cache omits target/")
    if "hashFiles('rust-toolchain.toml', 'Cargo.lock')" not in caches:
        add_problem(
            f"ci.yml: {job} job: cargo cache key is not built from rust-toolchain.toml and Cargo.lock"
        )
    if not re.search(r"key:.*llvm-21", caches):
        add_problem(f"ci.yml: {job} job: no LLVM 21 cache key")
    if job == "linux":
        if "~/.cache/burdin/llvm-21" not in caches:
            add_problem("ci.yml: linux job: LLVM cache does not use a runner-writable path")
        if "cache=~/.cache/burdin/llvm-21" not in block:
            add_problem("ci.yml: linux job: install step does not use the cached LLVM path")
        if "steps.cache-llvm.outputs.cache-hit" not in block:
            add_problem("ci.yml: linux job: LLVM install ignores the cache hit")
        if "readlink -f" not in block:
            add_problem("ci.yml: linux job: LLVM cache does not follow libLLVM symlinks outside the tree")
    elif job == "macos":
        if "~/Library/Caches/Homebrew" not in caches:
            add_problem("ci.yml: macos job: LLVM bottle downloads are not cached")


def check_bench_regression(job: str, block: str) -> None:
    if "actions/cache/restore@v4" not in block:
        add_problem(f"ci.yml: {job} job: no step restores the benchmark baseline")
    if "actions/cache/save@v4" not in block:
        add_problem(f"ci.yml: {job} job: no step saves the benchmark baseline")
    if "bench-${{ runner.os }}-${{ runner.arch }}-" not in block:
        add_problem(f"ci.yml: {job} job: the benchmark baseline cache key does not name the runner")
    if "hashFiles('rust-toolchain.toml', 'Cargo.lock')" not in block:
        add_problem(
            f"ci.yml: {job} job: the benchmark baseline is not scoped to the toolchain and Cargo.lock"
        )
    if "tests/bench_regression.py" not in block:
        add_problem(f"ci.yml: {job} job: does not run tests/bench_regression.py")
    if "continue-on-error: true" not in block:
        add_problem(f"ci.yml: {job} job: the benchmark regression step is not advisory")
    if "BURDIN_BENCH_UPDATE" not in block:
        add_problem(f"ci.yml: {job} job: a main-branch push does not record the benchmark baseline")
    if "github.ref == 'refs/heads/main'" not in block:
        add_problem(f"ci.yml: {job} job: the benchmark baseline is saved outside main")


def check_fuzz(block: str) -> None:
    if not re.search(r"^    if:.*(schedule|workflow_dispatch)", block, re.MULTILINE):
        add_problem("ci.yml: fuzz job: not limited to the schedule or a manual run")
    if "cargo test -p burdin_fuzz" not in block:
        add_problem("ci.yml: fuzz job: does not replay the corpus with cargo test -p burdin_fuzz")
    if "tests/fuzz.py" not in block:
        add_problem("ci.yml: fuzz job: does not run tests/fuzz.py")
    if "-p burdin_fmt --test fuzz" not in block:
        add_problem("ci.yml: fuzz job: does not fuzz the formatter with cargo test -p burdin_fmt --test fuzz")
    if "github.run_id" not in block:
        add_problem("ci.yml: fuzz job: uses the same generator seed every run (no github.run_id)")
    if "actions/upload-artifact@v4" not in block:
        add_problem("ci.yml: fuzz job: does not upload the found inputs")
    if "fuzz-out/crashes" not in block:
        add_problem("ci.yml: fuzz job: does not upload the directory that holds found inputs")


def check_deps_audit(job: str, block: str) -> None:
    build_line = next(
        (number for number, line in enumerate(block.split("\n"), 1) if "run: cargo build" in line),
        None,
    )
    deps_line = next(
        (number for number, line in enumerate(block.split("\n"), 1) if "tests/check_deps.py" in line),
        None,
    )
    if build_line is not None and deps_line is not None and deps_line < build_line:
        add_problem(
            f"ci.yml: {job} job: the dependency audit runs before cargo build "
            "(cargo metadata needs the registry)"
        )
    if "BURDIN_DEPS_REQUIRE_METADATA: 1" not in block:
        add_problem(
            f"ci.yml: {job} job: the dependency audit does not require cargo metadata "
            "(BURDIN_DEPS_REQUIRE_METADATA=1)"
        )


jobs = {job: harness.workflow_job_block(text, job) for job in ("linux", "macos", "windows", "fuzz")}
linux, macos, windows, fuzz = jobs["linux"], jobs["macos"], jobs["windows"], jobs["fuzz"]
for job, block in jobs.items():
    if not block.strip():
        add_problem(f"ci.yml: no {job} job")

if not any(line == "    runs-on: ubuntu-24.04" for line in linux.split("\n")):
    add_problem("ci.yml: linux does not run on ubuntu-24.04")
if not any(line == "    runs-on: macos-latest" for line in macos.split("\n")):
    add_problem("ci.yml: macos does not run on macos-latest")
if not any(line == "    runs-on: windows-latest" for line in windows.split("\n")):
    add_problem("ci.yml: windows does not run on windows-latest")
for trigger, message in (
    ("  push:", "ci.yml: does not run on push"),
    ("  pull_request:", "ci.yml: does not run on pull_request"),
    ("  schedule:", "ci.yml: does not run on a schedule (continuous fuzzing)"),
    ("  workflow_dispatch:", "ci.yml: does not allow a manual fuzz run"),
):
    if not any(line == trigger for line in text.split("\n")):
        add_problem(message)
if not re.search(r"^    - cron: '[^']+'", text, re.MULTILINE):
    add_problem("ci.yml: the schedule has no cron entry")

# Linux: LLVM 21 from apt.llvm.org.
if "LLVM_SYS_211_PREFIX: /usr/lib/llvm-21" not in linux:
    add_problem("ci.yml: linux job: LLVM_SYS_211_PREFIX is not set")
if "llvm.sh 21" not in linux:
    add_problem("ci.yml: linux job: LLVM 21 is not installed from apt.llvm.org")
if "llvm-21-dev" not in linux:
    add_problem("ci.yml: linux job: llvm-21-dev is not installed")
if "libpolly-21-dev" not in linux:
    add_problem("ci.yml: linux job: libpolly-21-dev is not installed")

# macOS: Homebrew's keg-only LLVM 21, exported for inkwell and for the driver's llvm-config lookup.
if "brew install llvm@21" not in macos:
    add_problem("ci.yml: macos job: llvm@21 is not installed")
if "LLVM_SYS_211_PREFIX=$(brew --prefix llvm@21)" not in macos:
    add_problem("ci.yml: macos job: LLVM_SYS_211_PREFIX is not set from Homebrew")
if "$(brew --prefix llvm@21)/bin" not in macos:
    add_problem("ci.yml: macos job: llvm-config is not added to PATH")

# Windows: the MSVC environment, the official LLVM 21 archive, and its DLL directory on PATH.
if "ilammy/msvc-dev-cmd" not in windows:
    add_problem("ci.yml: windows job: does not load the MSVC environment (cl would not be on PATH)")
if "LLVM_SYS_211_PREFIX: C:/llvm" not in windows:
    add_problem("ci.yml: windows job: LLVM_SYS_211_PREFIX is not set")
if "llvmorg-21.1.8/clang+llvm-21.1.8-x86_64-pc-windows-msvc.tar.xz" not in windows:
    add_problem("ci.yml: windows job: does not install LLVM 21 from the official archive")
if "C:/llvm/lib/LLVM-C.lib" not in windows:
    add_problem("ci.yml: windows job: never checks that the LLVM C API import library exists")
if "C:\\llvm\\bin" not in windows:
    add_problem("ci.yml: windows job: does not put LLVM's DLL directory on PATH")
if "BURDIN_CC=cl" not in windows:
    add_problem("ci.yml: windows job: does not exercise the BURDIN_CC=cl path")
# Git for Windows ships a coreutils `link.exe`; rustc must be pointed at MSVC's linker explicitly.
if "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER" not in windows:
    add_problem("ci.yml: windows job: does not pin the MSVC linker (Git's link.exe can shadow it)")

required_steps = [
    "tests/check_tests.py",
    "tests/check_e9000.py",
    "tests/check_unit_tests.py",
    "tests/check_ci.py",
    "tests/check_deps.py",
    "tests/check_platform.py",
    "tests/check_docs.py",
    "tests/check_debug_info.py",
    "tests/check_codegen_opt.py",
    "tests/check_build_flags.py",
    "tests/check_parallel.py",
    "tests/check_bindgen.py",
    "tests/check_diagnostics.py",
    "tests/check_error_docs.py",
    "tests/check_test_runner.py",
    "tests/check_fmt.py",
    "tests/check_lint.py",
    "tests/check_doc.py",
    "tests/check_check.py",
    "tests/check_platform_support.py",
    "tests/check_release.py",
    "tests/bench_regression.py",
    "rustup toolchain install",
    "rustc --version",
    "cargo fmt --all --check",
    "cargo clippy --all-targets -- -D warnings",
    "cargo test --workspace",
    "cargo build",
    "cargo bench -p burdin_bench -- --smoke",
]
for job in ("linux", "macos", "windows"):
    block = jobs[job]
    for command in required_steps:
        require_command(job, block, command)
    check_conformance(job, block, "debug")
    check_conformance(job, block, "release")
    check_stress_repeat(job, block)
    check_caches(job, block)
    check_bench_regression(job, block)
    check_deps_audit(job, block)

check_fuzz(fuzz)

# The fuzz corpus is committed and replayed by `cargo test --workspace`, which is the check that fails
# when a panic is reintroduced (issue #447). Losing the corpus or the replay test would silently disable it.
fuzz_crate = harness.ROOT / "crates" / "burdin_fuzz"
for relative in ("Cargo.toml", "tests/corpus.rs", "src/bin/syntax.rs"):
    if not (fuzz_crate / relative).is_file():
        add_problem(f"crates/burdin_fuzz/{relative}: missing")
corpus = fuzz_crate / "corpus" / "syntax"
if not corpus.is_dir():
    add_problem("crates/burdin_fuzz/corpus/syntax: missing")
elif not any(corpus.glob("*.bd")):
    add_problem("crates/burdin_fuzz/corpus/syntax: no .bd corpus files")

# The release run (issue #47) is only meaningful when the runner selects the profile and skips
# debug-only tests, and the suite must contain one so that skip path stays exercised.
runner = harness.TESTS / "run_tests.py"
if runner.is_file():
    runner_text = harness.read_text(runner)
    if "--release" not in runner_text:
        add_problem("tests/run_tests.py: no --release option")
    if "debug-only" not in runner_text:
        add_problem("tests/run_tests.py: the release profile does not skip debug-only tests")
    if not any(
        b"//@ debug-only" in path.read_bytes()
        for path in harness.TESTS.rglob("*.bd")
        if path.is_file()
    ):
        add_problem("tests/: no test is marked //@ debug-only, so the release profile never exercises the skip")

# The compiler benchmarks (issue #41): CI runs the smoke budgets, so the bench target, the crate that
# generates its inputs, and the `--smoke` mode of the harness must all stay in place.
bench_manifest = harness.ROOT / "crates" / "burdin_bench" / "Cargo.toml"
bench_target = harness.ROOT / "crates" / "burdin_bench" / "benches" / "compiler.rs"
bench_lib = harness.ROOT / "crates" / "burdin_bench" / "src" / "lib.rs"
for path in (bench_manifest, bench_target, bench_lib):
    if not path.is_file():
        add_problem(f"{path.relative_to(harness.ROOT).as_posix()}: missing")
if bench_manifest.is_file() and 'name = "compiler"' not in harness.read_text(bench_manifest):
    add_problem("crates/burdin_bench/Cargo.toml: no 'compiler' benchmark target")
if bench_target.is_file() and '"--smoke"' not in harness.read_text(bench_target):
    add_problem("crates/burdin_bench/benches/compiler.rs: no --smoke mode")

# Benchmark regression tracking (issue #446): the harness must keep the modes the script uses to record
# and compare a baseline, and the script must keep running that comparison and updating the baseline.
bench_script = harness.TESTS / "bench_regression.py"
if not bench_script.is_file():
    add_problem("tests/bench_regression.py: missing")
if bench_target.is_file():
    bench_target_text = harness.read_text(bench_target)
    for mode in ('"--save"', '"--baseline"', '"--threshold"'):
        if mode not in bench_target_text:
            add_problem(f"crates/burdin_bench/benches/compiler.rs: no {mode} mode")
if bench_script.is_file():
    bench_text = harness.read_text(bench_script)
    for word in ("--save", "--baseline", "--threshold", "BURDIN_BENCH_UPDATE"):
        if word not in bench_text:
            add_problem(f"tests/bench_regression.py: does not use {word}")


def check_script(relative: str, *patterns: str) -> None:
    path = harness.ROOT / relative
    if not path.is_file():
        return  # reported by the existence loop below
    content = harness.read_text(path)
    for pattern in patterns:
        if pattern not in content:
            add_problem(f"{relative}: no longer contains: {pattern}")


# The new checks must keep testing what their issues require, so a rewritten script cannot silently weaken CI.
check_script(
    "tests/check_debug_info.py", "llvm-dwarfdump", "gdb", "-g", "DW_TAG_subprogram", "DW_TAG_variable"
)
check_script(
    "tests/check_codegen_opt.py", "alloca i1", "bounds.fail", "burdin.drop", "index out of bounds"
)
check_script(
    "tests/check_build_flags.py", "burdin --version", "--strip", "--lto", "--pgo-use", "147456"
)
check_script("tests/check_parallel.py", "BURDIN_JOBS", "unified_diff")
check_script("tests/check_fmt.py", "--check", "--stdin", "--range", "idempotent", "TRICKY", "E0001")
check_script(
    "tests/check_doc.py", "--output-format", "--private", "--coverage", "--deny-warnings", "D0001", "D0002", "missing_docs"
)
check_script(
    "tests/check_e9000.py", "e9000-audit.md", "Code::UNSUPPORTED", "// e9000:", "run_tests.py", "Stage 1"
)
check_script(
    "tests/check_platform_support.py",
    "docs/platform-support.md",
    "Untested",
    "check-docs",
    "len(ran.stdout)",
)

# The dependency and license audit (issue #449): the reviewed inventory must cover the direct dependency
# and its LLVM bindings, the policy must stay stated, the script must keep checking the lockfile, the pins,
# and the licenses through cargo metadata, and the review page must exist and be linked.
deps_inventory = harness.TESTS / "dependencies.txt"
if not deps_inventory.is_file():
    add_problem("tests/dependencies.txt: missing (the reviewed dependency inventory)")
check_script(
    "tests/check_deps.py",
    "Cargo.lock",
    "dependencies.txt",
    "cargo metadata",
    "BURDIN_DEPS_REQUIRE_METADATA",
    "checksum",
    "permissive",
    "exact",
)
if deps_inventory.is_file():
    inventory = harness.read_text(deps_inventory)
    for crate in ("inkwell", "llvm-sys"):
        if not re.search(rf"^{re.escape(crate)}[ \t]", inventory, re.MULTILINE):
            add_problem(f"tests/dependencies.txt: no {crate} entry")
    if "permissive" not in inventory.lower():
        add_problem("tests/dependencies.txt: the license policy is no longer stated")
if not (harness.ROOT / "docs" / "dependencies.md").is_file():
    add_problem("docs/dependencies.md: missing (the dependency and license review)")
elif "docs/dependencies.md" not in harness.read_text(harness.ROOT / "README.md"):
    add_problem("README.md: does not link docs/dependencies.md")

# The release pipeline (issues #452-#456): the check must keep covering the six targets, the release workflow,
# the archive and checksum scripts, both installers, and the version policy.
release_workflow = harness.ROOT / ".github" / "workflows" / "release.yml"
if not release_workflow.is_file():
    add_problem(".github/workflows/release.yml: missing")
else:
    release_text = harness.read_text(release_workflow)
    for target in (
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ):
        if target not in release_text:
            add_problem(f"release.yml: does not build {target}")
    if "if: needs.plan.outputs.publish == 'true'" not in release_text:
        add_problem("release.yml: the publish step is not gated")
check_script(
    "tests/check_release.py",
    "SHA256SUMS",
    "install.sh",
    "install.ps1",
    "release_notes.py",
    "x86_64-unknown-linux-gnu",
    "docs/releasing.md",
    "channel:",
    "smoke_test",
    "--verify-reproducible",
)
for relative in (
    "docs/releasing.md",
    "tests/build_release.py",
    "tests/release_notes.py",
    "install.sh",
    "install.ps1",
):
    if not (harness.ROOT / relative).is_file():
        add_problem(f"{relative}: missing")

# The m11 test is the one that proves --link-lib LLVM-C resolves in CI.
if not any(
    b"//@ link-lib: LLVM-C" in path.read_bytes()
    for path in (harness.TESTS / "m11_llvm").rglob("*.bd")
):
    add_problem("tests/m11_llvm: no test uses //@ link-lib: LLVM-C")

# The documentation check reads the driver contract from the root README (issue #42).
if not (harness.ROOT / "README.md").is_file():
    add_problem("README.md: missing (the driver contract lives there)")
elif "check-docs" not in harness.read_text(harness.ROOT / "README.md"):
    add_problem("README.md: does not mention tests/check_docs.py")

# The user-facing error-code reference (issue #437) must mirror the registry exactly and be in the docs index.
error_codes = harness.ROOT / "docs" / "error-codes.md"
if not error_codes.is_file():
    add_problem("docs/error-codes.md: missing (the user-facing error-code reference)")
else:
    if "docs/error-codes.md" not in harness.read_text(harness.ROOT / "README.md"):
        add_problem("README.md: does not link docs/error-codes.md")
    doc_codes = set(re.findall(r"^## (E[0-9]{4}):", harness.read_text(error_codes), re.MULTILINE))
    registry_codes = set(
        re.findall(r"^\| (E[0-9]{4}) \|", harness.read_text(harness.TESTS / "README.md"), re.MULTILINE)
    )
    if doc_codes != registry_codes:
        add_problem("docs/error-codes.md: codes do not match the tests/README.md registry")

# The platform support matrix (issue #457) must exist and be reachable from the documentation index and the
# release notes; the script that checks it is required in every job above.
platform_page = harness.ROOT / "docs" / "platform-support.md"
if not platform_page.is_file():
    add_problem("docs/platform-support.md: missing (the platform support matrix and known limitations)")
else:
    if "docs/platform-support.md" not in harness.read_text(harness.ROOT / "README.md"):
        add_problem("README.md: does not link docs/platform-support.md")
    if "platform-support.md" not in harness.read_text(harness.ROOT / "CHANGELOG.md"):
        add_problem("CHANGELOG.md: the release notes do not link docs/platform-support.md")

# Scripts CI invokes must exist, be executable, and have LF endings.
scripts = [
    "tests/check_tests.py",
    "tests/check_e9000.py",
    "tests/check_unit_tests.py",
    "tests/run_tests.py",
    "tests/check_ci.py",
    "tests/check_platform.py",
    "tests/check_platform_support.py",
    "tests/check_docs.py",
    "tests/check_deps.py",
    "tests/check_diagnostics.py",
    "tests/check_error_docs.py",
    "tests/check_test_runner.py",
    "tests/check_fmt.py",
    "tests/check_lint.py",
    "tests/check_doc.py",
    "tests/check_check.py",
    "tests/check_debug_info.py",
    "tests/check_codegen_opt.py",
    "tests/check_build_flags.py",
    "tests/check_parallel.py",
    "tests/check_bindgen.py",
    "tests/check_release.py",
    "tests/build_release.py",
    "tests/release_notes.py",
    "tests/bench_regression.py",
    "tests/fuzz.py",
    "tests/harness.py",
    "install.sh",
]
for relative in scripts:
    path = harness.ROOT / relative
    if not path.is_file():
        add_problem(f"{relative}: missing")
        continue
    listing = harness.run(["git", "-C", harness.ROOT, "ls-files", "-s", relative])
    fields = harness.text(listing.stdout).split()
    if listing.returncode == 0 and fields:
        mode = fields[0]
        if mode != "100755":
            add_problem(f"{relative}: git mode {mode}, expected 100755")
    elif not (path.stat().st_mode & 0o111):
        add_problem(f"{relative}: not executable")
    if b"\r" in path.read_bytes():
        add_problem(f"{relative}: CRLF line endings")

print("Checked .github/workflows/ci.yml against tests/")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
