#!/usr/bin/env python3
"""Builds, smoke-tests, packages, and checksums one Burdin compiler release archive (issues #452-#456).

Usage: tests/build_release.py [options]
  --target <triple>       target triple for the archive name (default: the host triple)
  --version <x.y.z>       version (default: the workspace version in Cargo.toml)
  --channel <channel>     dev, nightly, or stable (default: BURDIN_CHANNEL, else dev)
  --out <dir>             output directory (default: dist/)
  --binary <path>         package this binary instead of building; its --version must match
  --llvm-dll <path>       Windows: bundle this LLVM-C.dll beside burdin.exe (default for a build: LLVM-C.dll in
                          LLVM_SYS_211_PREFIX/bin, else on PATH; --binary bundles one only when this is given)
  --manager <path>        with --binary: bundle this toolchain manager as burdin-toolchain (a build bundles its own)
  --lsp <path>            with --binary: package this burdin-lsp as the lsp component (a build packages its own)
  --components            with --binary: also package the std-src, docs, and debuggers components (a build always does)
  --skip-smoke           do not run build/run/test with the binary
  --verify-reproducible   build twice from two copies and require identical archives
  --print-version         print the workspace version and exit
  -h, --help              show this help

The archive is `burdin-<version>-<target>.tar.gz`; its root directory is `burdin-<version>-<target>/` and holds the
compiler, the toolchain manager `burdin-toolchain` (spec/toolchain.md), README.md,
docs/error-codes.md, docs/lints.md, and packages/ (the official packages, which the compiler finds beside its own
executable: spec/packages.md, PKG16); a Windows archive also holds
LLVM-C.dll beside burdin.exe, because that build links LLVM as a shared library. Linux and macOS builds link LLVM
statically (BURDIN_STATIC_LLVM=1, set by this script) and the script checks that the binary needs no LLVM shared
library. BURDIN_STATIC_LLVM=1 in the environment on Windows links LLVM statically there too, with the static CRT, and
the archive then has no DLL; BURDIN_STATIC_LLVM=0 on Linux or macOS links the shared library. Next to it the script writes
`<archive>.sha256` and `SHA256SUMS.<target>`, both in the `sha256sum -c` format. The release workflow aggregates the
per-target files into one `SHA256SUMS` (docs/releasing.md).

Each optional component (spec/toolchain.md TC10) is its own archive, `burdin-<component>-<version>-<target>.tar.gz`
with the root `burdin-<component>-<version>-<target>/`, whose files the toolchain manager copies into a toolchain:
`lsp` (burdin-lsp), `std-src` (lib/std/src), `docs` (share/doc/docs and share/doc/spec), and `debuggers`
(debuggers/). Their checksums go into the same `SHA256SUMS.<target>`, and each gets a `.sha256` file.

Reproducibility: builds set CARGO_INCREMENTAL=0, remap the checkout and target paths to /burdin, and use the git
commit time as SOURCE_DATE_EPOCH; archives sort their entries, pin every mtime to SOURCE_DATE_EPOCH, use uid/gid 0,
and are compressed without a timestamp. `--verify-reproducible` proves the result byte for byte on the host.

Environment: BURDIN_VERSION and BURDIN_CHANNEL are read through when the flags are not given. BURDIN_COMMIT
overrides the commit embedded in the binary (the release workflow checks out with a .git directory, so normally it
is derived from git).
"""

import gzip
import hashlib
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

import harness

USAGE = __doc__.split("The archive is")[0].rstrip()
ROOT = harness.ROOT
SUPPORTED_TARGETS = {
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
}


def die(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(1)


def say(message: str) -> None:
    print(message)


def manifest_version() -> str:
    for line in harness.read_text(ROOT / "Cargo.toml").split("\n"):
        match = re.match(r'^version = "(.*)"$', line)
        if match:
            return match.group(1)
    return ""


def host_target() -> str:
    result = harness.run(["rustc", "-vV"])
    for line in harness.text(result.stdout).split("\n"):
        if line.startswith("host: "):
            return line[len("host: "):].strip()
    return ""


def sha256_of(path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def exe_suffix(target: str) -> str:
    return ".exe" if target.endswith("-pc-windows-msvc") else ""


def archive_name(version: str, target: str) -> str:
    return f"burdin-{version}-{target}.tar.gz"


def binary_command(path) -> list:
    """A fake --binary may be a script; the real compiler is a native executable."""
    if str(path).endswith(".py"):
        return [sys.executable, str(path)]
    if harness.PLATFORM == "windows" and not str(path).lower().endswith(".exe"):
        return ["sh", str(path)]
    return [str(path)]


class Options:
    def __init__(self):
        self.target = ""
        self.version = ""
        self.channel = ""
        self.out = ROOT / "dist"
        self.binary = ""
        self.llvm_dll = ""
        self.manager = ""
        self.lsp = ""
        self.components = False
        self.skip_smoke = False
        self.reproducible = False


def parse_args(arguments) -> Options:
    options = Options()
    index = 0
    while index < len(arguments):
        argument = arguments[index]
        if argument == "--print-version":
            print(manifest_version())
            raise SystemExit(0)
        if argument in ("-h", "--help"):
            print(USAGE)
            raise SystemExit(0)
        switches = {"--skip-smoke": "skip_smoke", "--verify-reproducible": "reproducible", "--components": "components"}
        if argument in switches:
            setattr(options, switches[argument], True)
            index += 1
            continue
        flags = {
            "--target": ("target", "a triple"),
            "--version": ("version", "a value"),
            "--channel": ("channel", "a value"),
            "--out": ("out", "a directory"),
            "--binary": ("binary", "a path"),
            "--llvm-dll": ("llvm_dll", "a path"),
            "--manager": ("manager", "a path"),
            "--lsp": ("lsp", "a path"),
        }
        if argument not in flags:
            die(f"unknown option: {argument} (try --help)")
        attribute, description = flags[argument]
        if index + 1 >= len(arguments):
            die(f"{argument} needs {description}")
        value = arguments[index + 1]
        setattr(options, attribute, Path(value) if attribute == "out" else value)
        index += 2
    return options


def check_binary_version(binary, version: str, channel: str, target: str) -> None:
    result = harness.run([*binary_command(binary), "--version"], merge=True)
    output = harness.text(result.stdout).rstrip("\n")
    if result.returncode != 0:
        die(f"{binary} --version failed: {output}")
    lines = output.split("\n")
    for expected in (f"burdin {version}", f"channel: {channel}", f"target: {target}"):
        if expected not in lines:
            die(f"{binary} --version does not report {expected}: {output}")


def uses_static_llvm(target: str) -> bool:
    """Linux and macOS release builds link LLVM into the compiler. Windows ships LLVM-C.dll beside it unless
    BURDIN_STATIC_LLVM=1 asks for the static link there too (it needs the static CRT; see crates/burdin_driver/build.rs)."""
    setting = os.environ.get("BURDIN_STATIC_LLVM")
    if setting is not None:
        return setting not in ("", "0")
    return not target.endswith("-pc-windows-msvc")


def check_no_llvm_library(binary: Path) -> None:
    """A statically linked compiler has no LLVM shared library among its dependencies."""
    tool = ["otool", "-L"] if harness.PLATFORM == "macos" else ["ldd"]
    result = harness.run([*tool, binary], merge=True)
    listing = harness.text(result.stdout)
    if result.returncode != 0:
        die(f"{tool[0]} {binary} failed: {listing}")
    for line in listing.splitlines():
        if "libLLVM" in line or "LLVM-C" in line:
            die(f"{binary} still depends on an LLVM shared library: {line.strip()}")


def build_binary(source_root: Path, target_dir: Path, version: str, channel: str, target: str) -> Path:
    if not target_dir.is_absolute():
        target_dir = source_root / target_dir
    flags = (
        f"--remap-path-prefix={source_root}=/burdin "
        f"--remap-path-prefix={target_dir}=/burdin/target"
    )
    if target.endswith("-pc-windows-msvc") and uses_static_llvm(target):
        flags += " -C target-feature=+crt-static"
    if os.environ.get("RUSTFLAGS"):
        flags = os.environ["RUSTFLAGS"] + " " + flags
    print(f"building {version} ({channel}) for {target} from {source_root}", file=sys.stderr)
    env = {
        **os.environ,
        "CARGO_INCREMENTAL": "0",
        "RUSTFLAGS": flags,
        "CARGO_TARGET_DIR": str(target_dir),
        "BURDIN_VERSION": version,
        "BURDIN_CHANNEL": channel,
    }
    if uses_static_llvm(target):
        env["BURDIN_STATIC_LLVM"] = "1"
    # The compiler, the toolchain manager that becomes the `burdin` proxy, and the LLVM-free language server of the
    # lsp component (spec/toolchain.md).
    result = harness.run(
        ["cargo", "build", "--release", "--locked", "-p", "burdin_driver", "-p", "burdin_toolchain", "-p", "burdin_lsp"],
        cwd=source_root,
        env=env,
        merge=True,
    )
    sys.stderr.write(harness.text(result.stdout))
    if result.returncode != 0:
        die("cargo build failed")
    binary = target_dir / "release" / f"burdin{exe_suffix(target)}"
    if not binary.is_file():
        die(f"cargo did not produce {binary}")
    if uses_static_llvm(target) and not target.endswith("-pc-windows-msvc"):
        check_no_llvm_library(binary)
    for companion in companions(binary, target):
        if not companion.is_file():
            die(f"cargo did not produce {companion}")
    return binary


def companions(binary: Path, target: str):
    """The toolchain manager and burdin-lsp a build put beside the compiler."""
    suffix = exe_suffix(target)
    return binary.parent / f"burdin-toolchain{suffix}", binary.parent / f"burdin-lsp{suffix}"


def smoke_test_manager(manager: Path) -> None:
    print("smoke-testing the toolchain manager", file=sys.stderr)
    result = harness.run([manager, "toolchain", "--help"], merge=True)
    if result.returncode != 0 or "burdin [+<toolchain>] toolchain" not in harness.text(result.stdout):
        die(f"burdin-toolchain toolchain --help failed: {harness.text(result.stdout)}")


def smoke_test(binary: Path, work: Path) -> None:
    harness.write_text(work / "smoke_main.bd", 'fn main() {\n    println("release smoke")\n}\n')
    harness.write_text(work / "smoke_test.bd", 'test "release smoke" {\n    assert_eq(1 + 1, 2)\n}\n')
    print("smoke-testing build, run, and test", file=sys.stderr)
    built = harness.run([binary, "build", "smoke_main.bd", "-o", "smoke_program"], cwd=work)
    if built.returncode != 0:
        die("burdin build failed in the smoke test")
    program = harness.executable_in(work, "smoke_program")
    if not program.is_file():
        die("burdin build did not produce a program")
    ran = harness.run([program], cwd=work)
    if harness.text(ran.stdout).rstrip("\n") != "release smoke":
        die("the smoke program printed the wrong output")
    executed = harness.run([binary, "run", "smoke_main.bd"], cwd=work)
    if harness.text(executed.stdout).rstrip("\n") != "release smoke":
        die("burdin run printed the wrong output")
    tested = harness.run([binary, "test", "smoke_test.bd"], cwd=work, merge=True)
    log = harness.text(tested.stdout)
    if tested.returncode != 0:
        die(f"burdin test failed: {log}")
    if "test result: 1 passed, 0 failed" not in log:
        die(f"burdin test printed an unexpected summary: {log}")


def make_archive(stage: Path, root: str, archive: Path, epoch: int) -> None:
    plain = Path(str(stage) + ".tar")
    items = [stage / root]
    for directory, directories, files in os.walk(stage / root):
        directories.sort()
        files.sort()
        for name in directories:
            items.append(Path(directory) / name)
        for name in files:
            items.append(Path(directory) / name)
    items.sort(key=lambda path: path.relative_to(stage).as_posix())
    try:
        with tarfile.open(plain, "w", format=tarfile.USTAR_FORMAT) as tar:
            for item in items:
                info = tar.gettarinfo(str(item), arcname=item.relative_to(stage).as_posix())
                info.mtime = epoch
                info.uid = 0
                info.gid = 0
                info.uname = ""
                info.gname = ""
                if info.isfile():
                    with open(item, "rb") as source:
                        tar.addfile(info, source)
                else:
                    tar.addfile(info)
        with open(plain, "rb") as source, open(archive, "wb") as destination:
            with gzip.GzipFile(
                fileobj=destination, mode="wb", compresslevel=9, mtime=0, filename=""
            ) as compressed:
                shutil.copyfileobj(source, compressed)
    finally:
        plain.unlink(missing_ok=True)


def find_llvm_dll() -> str:
    """The LLVM-C.dll a Windows build links against: the LLVM prefix first, then PATH."""
    prefix = os.environ.get("LLVM_SYS_211_PREFIX", "")
    if prefix:
        candidate = Path(prefix) / "bin" / "LLVM-C.dll"
        if candidate.is_file():
            return str(candidate)
    return shutil.which("LLVM-C.dll") or ""


def component_files(component: str, target: str, lsp: str) -> dict:
    """The files of one component archive: each path inside a toolchain, mapped to its source file."""
    if component == "lsp":
        return {f"burdin-lsp{exe_suffix(target)}": Path(lsp)}
    trees = {
        "std-src": [(ROOT / "std" / "src", "lib/std/src")],
        "docs": [(ROOT / "docs", "share/doc/docs"), (ROOT / "spec", "share/doc/spec")],
        "debuggers": [(ROOT / "debuggers", "debuggers")],
    }[component]
    files = {}
    for source, prefix in trees:
        for path in sorted(source.rglob("*")):
            if path.is_file():
                files[f"{prefix}/{path.relative_to(source).as_posix()}"] = path
    return files


def package_component(component: str, files: dict, out: Path, version: str, target: str, epoch: int) -> Path:
    root = f"burdin-{component}-{version}-{target}"
    archive = out / f"{root}.tar.gz"
    stage = Path(tempfile.mkdtemp(prefix="burdin-release-component-"))
    try:
        for relative, source in files.items():
            destination = stage / root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)
            destination.chmod(0o755 if relative.startswith("burdin-lsp") else 0o644)
        make_archive(stage, root, archive, epoch)
    finally:
        shutil.rmtree(stage, ignore_errors=True)
    return archive


def package_binary(
    binary: Path,
    out: Path,
    version: str,
    target: str,
    epoch: int,
    llvm_dll: str = "",
    manager: str = "",
    lsp: str = "",
    components: bool = False,
) -> Path:
    root = f"burdin-{version}-{target}"
    name = archive_name(version, target)
    archive = out / name
    out.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix="burdin-release-stage-"))
    try:
        (stage / root / "docs").mkdir(parents=True)
        shutil.copyfile(binary, stage / root / f"burdin{exe_suffix(target)}")
        (stage / root / f"burdin{exe_suffix(target)}").chmod(0o755)
        if manager:
            shutil.copyfile(manager, stage / root / f"burdin-toolchain{exe_suffix(target)}")
            (stage / root / f"burdin-toolchain{exe_suffix(target)}").chmod(0o755)
        if llvm_dll:
            shutil.copyfile(llvm_dll, stage / root / "LLVM-C.dll")
        for source, destination in (
            (ROOT / "README.md", stage / root / "README.md"),
            (ROOT / "docs" / "error-codes.md", stage / root / "docs" / "error-codes.md"),
            (ROOT / "docs" / "lints.md", stage / root / "docs" / "lints.md"),
        ):
            shutil.copyfile(source, destination)
        # The official packages sit beside the compiler, where it looks for them (`<exe dir>/packages`). Build output
        # and a stray .git directory are not part of a package; the archive sorts what is copied.
        shutil.copytree(
            ROOT / "packages", stage / root / "packages", ignore=shutil.ignore_patterns("target", ".git"), symlinks=False
        )
        make_archive(stage, root, archive, epoch)
    finally:
        shutil.rmtree(stage, ignore_errors=True)
    if not archive.is_file() or archive.stat().st_size == 0:
        die(f"the archive is empty: {archive}")
    archives = [archive]
    selected = (["debuggers", "docs", "std-src"] if components else []) + (["lsp"] if lsp else [])
    for component in sorted(selected):
        files = component_files(component, target, lsp)
        archives.append(package_component(component, files, out, version, target, epoch))
    lines = []
    for packaged in sorted(archives, key=lambda path: path.name):
        digest = sha256_of(packaged)
        (out / f"{packaged.name}.sha256").write_bytes(f"{digest}  {packaged.name}\n".encode())
        lines.append(f"{digest}  {packaged.name}\n")
        print(f"archive: {packaged}", file=sys.stderr)
        print(f"sha256:  {digest}", file=sys.stderr)
    (out / f"SHA256SUMS.{target}").write_bytes("".join(lines).encode())
    return archive


def main(arguments) -> int:
    options = parse_args(arguments)

    target = options.target or host_target()
    if not target:
        die("cannot determine the host triple; pass --target")
    if target not in SUPPORTED_TARGETS:
        die(f"unsupported release target: {target} (see docs/releasing.md)")
    version = options.version or os.environ.get("BURDIN_VERSION", "") or manifest_version()
    if not version:
        die("cannot read the workspace version from Cargo.toml; pass --version")
    channel = options.channel or os.environ.get("BURDIN_CHANNEL", "") or "dev"
    if channel not in ("dev", "nightly", "stable"):
        die(f"the channel must be dev, nightly, or stable, not: {channel}")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?", version):
        die(f"not a MAJOR.MINOR.PATCH version: {version}")
    if options.reproducible and options.binary:
        die("--verify-reproducible builds the compiler; it cannot be combined with --binary")
    if options.llvm_dll and not target.endswith("-pc-windows-msvc"):
        die("--llvm-dll only applies to a Windows target")
    if options.llvm_dll and not Path(options.llvm_dll).is_file():
        die(f"--llvm-dll is not a file: {options.llvm_dll}")
    if options.binary and not os.access(options.binary, os.X_OK):
        die(f"the binary is missing or not executable: {options.binary}")
    if (options.manager or options.lsp or options.components) and not options.binary:
        die("--manager, --lsp, and --components apply to --binary; a build packages its own")
    for flag, path in (("--manager", options.manager), ("--lsp", options.lsp)):
        if path and not Path(path).is_file():
            die(f"{flag} is not a file: {path}")

    commit = os.environ.get("BURDIN_COMMIT", "")
    if not commit:
        resolved = harness.run(["git", "-C", ROOT, "rev-parse", "--short=12", "HEAD"])
        if resolved.returncode == 0:
            commit = harness.text(resolved.stdout).strip()
    if not commit:
        commit = "unknown"
    os.environ["BURDIN_COMMIT"] = commit

    epoch_text = os.environ.get("SOURCE_DATE_EPOCH", "")
    if not epoch_text:
        resolved = harness.run(["git", "-C", ROOT, "log", "-1", "--format=%ct"])
        if resolved.returncode == 0:
            epoch_text = harness.text(resolved.stdout).strip()
    if not epoch_text:
        epoch_text = "315532800"  # 1980-01-01, the zip epoch
    os.environ["SOURCE_DATE_EPOCH"] = epoch_text
    epoch = int(epoch_text)

    if options.binary:
        check_binary_version(options.binary, version, channel, target)
        package_binary(
            Path(options.binary), options.out, version, target, epoch, options.llvm_dll,
            options.manager, options.lsp, options.components,
        )
        return 0

    # A Windows build needs LLVM-C.dll at run time and the archive does not bundle LLVM any other way, so a build
    # without one would publish an archive whose compiler cannot start (docs/releasing.md).
    llvm_dll = options.llvm_dll
    if not llvm_dll and target.endswith("-pc-windows-msvc") and not uses_static_llvm(target):
        llvm_dll = find_llvm_dll()
        if not llvm_dll:
            die("LLVM-C.dll not found; set LLVM_SYS_211_PREFIX to the LLVM 21 install or pass --llvm-dll")

    if options.reproducible:
        work = Path(tempfile.mkdtemp(prefix="burdin-repro-"))
        try:
            copy = work / "source"
            shutil.copytree(
                ROOT,
                copy,
                ignore=shutil.ignore_patterns("target", "dist", ".git"),
                symlinks=True,
            )
            print("reproducibility run 1", file=sys.stderr)
            binary_a = build_binary(ROOT, work / "target-a", version, channel, target)
            check_binary_version(binary_a, version, channel, target)
            if not options.skip_smoke:
                smoke_test(binary_a, work)
            manager_a, lsp_a = companions(binary_a, target)
            archive_a = package_binary(
                binary_a, work / "out-a", version, target, epoch, llvm_dll, str(manager_a), str(lsp_a), True
            )
            print("reproducibility run 2 (different checkout path)", file=sys.stderr)
            binary_b = build_binary(copy, work / "target-b", version, channel, target)
            check_binary_version(binary_b, version, channel, target)
            if not options.skip_smoke:
                smoke_test(binary_b, work)
            manager_b, lsp_b = companions(binary_b, target)
            archive_b = package_binary(
                binary_b, work / "out-b", version, target, epoch, llvm_dll, str(manager_b), str(lsp_b), True
            )
            hash_a = sha256_of(archive_a)
            hash_b = sha256_of(archive_b)
            if hash_a != hash_b:
                die(f"archives differ: {hash_a} ({archive_a}) vs {hash_b} ({archive_b})")
            say(f"reproducible: {hash_a}")
            return 0
        finally:
            shutil.rmtree(work, ignore_errors=True)

    native = host_target()
    if target != native:
        die(f"--target {target} is not the host triple ({native}); release jobs use a native runner per target")
    target_dir = Path(os.environ.get("CARGO_TARGET_DIR") or ROOT / "target")
    binary = build_binary(ROOT, target_dir, version, channel, target)
    check_binary_version(binary, version, channel, target)
    manager, lsp = companions(binary, target)
    if not options.skip_smoke:
        work = Path(tempfile.mkdtemp(prefix="burdin-release-smoke-"))
        try:
            smoke_test(binary, work)
            smoke_test_manager(manager)
        finally:
            shutil.rmtree(work, ignore_errors=True)
    package_binary(binary, options.out, version, target, epoch, llvm_dll, str(manager), str(lsp), True)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
