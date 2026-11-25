#!/usr/bin/env python3
"""Checks cross-compilation (`--target <triple>`, spec/cross-compilation.md, issue #282).

Needs a built driver. BURDIN selects it, like tests/run_tests.py. No cross toolchain is needed: the compiler driver
that links is replaced by a stand-in that records its command line, so the checks cover what the compiler produces
and asks the linker for, which is all of the compiler's part of a cross build.

What it checks:
  * XC1: the six supported triples are accepted; an unknown one, `--target` on `run`/`test`/`bench`, and `--target`
    with `--sanitize` are usage errors that say why;
  * XC3: for each of the six targets, the object file `--emit obj` writes is in the target's format and for the
    target's architecture (ELF `e_machine`, Mach-O `cputype`, COFF `Machine`), and `--emit asm` is written;
  * XC2: `target.os()` and `target.arch()` report the target in a compile-time `if`, in the LLVM IR the build emits, and in
    `burdin check --target`, whatever the host is;
  * XC5: the runtime's system calls and structure layouts are the target's (`lstat` on Linux, `lstat$INODE64` on
    Intel macOS, `_stat64` on Windows, and the arm64 Linux `struct stat` offsets);
  * XC7: the executable's default name carries the target's extension (`.exe` for Windows);
  * XC8: the link command: `BURDIN_CC_<TRIPLE>` before `BURDIN_CC`, `--target=`, the sysroot, lld, no `-lm` on Windows,
    the clang-cl form, and the error that names the variables when no cross linker exists;
  * XC9: a cross build is not served from the artifact cache, and the host's own triple builds and runs natively.
"""

import os
import platform
import re
import struct
import sys
from pathlib import Path

import harness

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        add_problem(message)


burdin = harness.require_burdin()

TARGETS = [
    # triple, format, machine code (ELF e_machine, Mach-O cputype, COFF Machine), os, arch
    ("x86_64-unknown-linux-gnu", "elf", 62, "Linux", "X86_64"),
    ("aarch64-unknown-linux-gnu", "elf", 183, "Linux", "Aarch64"),
    ("x86_64-apple-darwin", "macho", 0x01000007, "MacOs", "X86_64"),
    ("aarch64-apple-darwin", "macho", 0x0100000C, "MacOs", "Aarch64"),
    ("x86_64-pc-windows-msvc", "coff", 0x8664, "Windows", "X86_64"),
    ("aarch64-pc-windows-msvc", "coff", 0xAA64, "Windows", "Aarch64"),
]


def host_triple() -> str:
    machine = platform.machine().lower()
    arch = "aarch64" if machine in ("arm64", "aarch64") else "x86_64"
    if harness.PLATFORM == "linux":
        return f"{arch}-unknown-linux-gnu"
    if harness.PLATFORM == "macos":
        return f"{arch}-apple-darwin"
    return f"{arch}-pc-windows-msvc"


HOST = host_triple()
WINDOWS_HOST = harness.PLATFORM == "windows"


def variable(prefix: str, triple: str) -> str:
    return f"{prefix}_{triple.upper().replace('-', '_')}"


STUB_PY = """\
import os, sys
args = sys.argv[1:]
with open(os.environ["STUB_RECORD"], "a", newline="\\n") as record:
    record.write("\\n".join(args) + "\\n--\\n")
out = None
for i, arg in enumerate(args):
    if arg == "-o" and i + 1 < len(args):
        out = args[i + 1]
    elif arg.startswith("/Fe:"):
        out = arg[4:]
if out:
    open(out, "wb").close()
"""


def make_stub(directory: Path, name: str) -> Path:
    """A compiler driver that records its argument vector (to `$STUB_RECORD`) and creates the output file."""
    directory.mkdir(parents=True, exist_ok=True)
    script = directory / "stub_cc.py"
    harness.write_text(script, STUB_PY)
    if WINDOWS_HOST:
        wrapper = directory / f"{name}.cmd"
        harness.write_text(wrapper, f'@"{sys.executable}" "{script}" %*\r\n')
    else:
        wrapper = directory / name
        harness.write_text(wrapper, f'#!/bin/sh\nexec "{sys.executable}" "{script}" "$@"\n')
        wrapper.chmod(0o755)
    return wrapper


def environment(extra: dict, drop=()) -> dict:
    env = dict(os.environ)
    for key in list(env):
        if key.startswith(("BURDIN_CC", "BURDIN_SYSROOT", "BURDIN_AR")) or key == "BURDIN_LD":
            env.pop(key)
    for key in drop:
        env.pop(key, None)
    env.update(extra)
    return env


def invocations(record: Path) -> list:
    """The recorded compiler command lines, each a list of arguments."""
    if not record.exists():
        return []
    blocks = harness.read_text(record).split("--\n")
    return [block.splitlines() for block in blocks if block.strip()]


def machine_of(path: Path, kind: str) -> int:
    data = path.read_bytes()
    if kind == "elf":
        return struct.unpack_from("<H", data, 18)[0] if data[:4] == b"\x7fELF" else -1
    if kind == "macho":
        return struct.unpack_from("<I", data, 4)[0] if data[:4] == b"\xcf\xfa\xed\xfe" else -1
    return struct.unpack_from("<H", data, 0)[0]


HELLO = 'fn main() {\n    println("hello")\n}\n'

PLATFORM_MARKS = """\
fn main() {
    if target.os() == target.Os.Linux {
        println("OS_LINUX")
    } else if target.os() == target.Os.MacOs {
        println("OS_MACOS")
    } else {
        println("OS_WINDOWS")
    }
    if target.arch() == target.Arch.Aarch64 {
        println("ARCH_AARCH64")
    } else {
        println("ARCH_X86_64")
    }
}
"""

METADATA = """\
fn main() {
    let result = metadata("x")
    println(result.is_ok().to_string())
}
"""

with harness.temporary_dir("burdin-cross-") as work:
    stubs = work / "stubs"
    stub = make_stub(stubs, "cross-cc")
    record = work / "record.txt"
    base_env = {"STUB_RECORD": str(record)}
    harness.write_text(work / "hello.bd", HELLO)
    harness.write_text(work / "marks.bd", PLATFORM_MARKS)
    harness.write_text(work / "meta.bd", METADATA)

    def build(source: str, triple: str, *flags: str, env=None, cwd=None):
        return harness.run(
            [burdin, "build", source, "--target", triple, *flags],
            cwd=cwd or work,
            env=env if env is not None else environment({**base_env, "BURDIN_CC": str(stub)}),
        )

    # --- XC1: what is accepted and what is refused -------------------------------
    unknown = harness.run([burdin, "build", "hello.bd", "--target", "riscv64-unknown-linux-gnu"], cwd=work)
    err = harness.text(unknown.stderr)
    check(unknown.returncode == 2, f"an unknown triple: expected a usage error (2), got {unknown.returncode}")
    check("`riscv64-unknown-linux-gnu` is not a target this compiler supports" in err, f"unknown triple message: {err}")
    check("aarch64-apple-darwin" in err, "the unknown-triple message does not list the supported targets")

    for command in ("run", "test", "bench"):
        refused = harness.run([burdin, command, "hello.bd", "--target", HOST], cwd=work)
        check(refused.returncode == 2, f"`{command} --target`: expected a usage error, got {refused.returncode}")
        check(
            "only valid for `burdin build` and `burdin check`" in harness.text(refused.stderr),
            f"`{command} --target`: {harness.text(refused.stderr)}",
        )

    sanitized = harness.run(
        [burdin, "build", "hello.bd", "--target", "aarch64-unknown-linux-gnu", "--sanitize", "undefined"], cwd=work
    )
    check(sanitized.returncode == 2, "`--target` with `--sanitize` is a usage error")

    missing = harness.run([burdin, "build", "hello.bd", "--target"], cwd=work)
    check(missing.returncode == 2, "`--target` without a value is a usage error")

    # --- XC3: the object of each target ---------------------------------------------
    for triple, kind, machine, _, _ in TARGETS:
        out = work / f"hello-{triple}"
        record.unlink(missing_ok=True)
        built = build("hello.bd", triple, "--emit", "obj,asm", "-o", str(out))
        check(built.returncode == 0, f"{triple}: build failed: {harness.text(built.stderr)}")
        suffix = ".obj" if kind == "coff" else ".o"
        obj = Path(str(out) + suffix)
        check(obj.exists(), f"{triple}: --emit obj wrote no {obj.name}")
        if obj.exists():
            check(
                machine_of(obj, kind) == machine,
                f"{triple}: the object is not {kind} for machine {machine:#x} (found {machine_of(obj, kind):#x})",
            )
        check(Path(str(out) + ".s").exists(), f"{triple}: --emit asm wrote no assembly")

    # --- XC2: the target facts follow the target ------------------------------------
    for triple, _, _, os_name, arch_name in TARGETS:
        out = work / f"marks-{triple}"
        built = build("marks.bd", triple, "--emit", "llvm-ir", "-o", str(out))
        check(built.returncode == 0, f"{triple}: marks build failed: {harness.text(built.stderr)}")
        ir_path = Path(str(out) + ".ll")
        ir = harness.read_text(ir_path) if ir_path.exists() else ""
        wanted = f"OS_{os_name.upper()}"
        for other in ("OS_LINUX", "OS_MACOS", "OS_WINDOWS"):
            check((other in ir) == (other == wanted), f"{triple}: the IR {'lacks' if other == wanted else 'has'} {other}")
        wanted_arch = "ARCH_AARCH64" if arch_name == "Aarch64" else "ARCH_X86_64"
        check(wanted_arch in ir, f"{triple}: the IR lacks {wanted_arch}")
        check(("ARCH_X86_64" in ir) != ("ARCH_AARCH64" in ir), f"{triple}: both architectures' branches were kept")
        check(f'target triple = "{triple}"' in ir, f"{triple}: the module's triple is not the target's")

    # `burdin check --target`: the constant check holds only for the named target.
    harness.write_text(
        work / "assert_windows.bd",
        "const WINDOWS_ONLY: () = if target.os() == target.Os.Windows { } else { panic(\"not windows\") }\n\nfn main() {\n    println(\"ok\")\n}\n",
    )
    windows_check = harness.run(
        [burdin, "check", "assert_windows.bd", "--target", "x86_64-pc-windows-msvc"], cwd=work
    )
    check(windows_check.returncode == 0, f"check --target windows: {harness.text(windows_check.stderr)}")
    linux_check = harness.run([burdin, "check", "assert_windows.bd", "--target", "aarch64-unknown-linux-gnu"], cwd=work)
    check(linux_check.returncode != 0, "check --target linux: the Windows-only const passed")
    bad_check = harness.run([burdin, "check", "hello.bd", "--target", "nope"], cwd=work)
    check(bad_check.returncode == 2, "check --target with an unknown triple is a usage error")

    # --- XC5: the runtime is the target's ---------------------------------------------
    def declared(triple: str) -> str:
        out = work / f"meta-{triple}"
        built = build("meta.bd", triple, "--emit", "llvm-ir", "-o", str(out))
        check(built.returncode == 0, f"{triple}: metadata build failed: {harness.text(built.stderr)}")
        path = Path(str(out) + ".ll")
        return harness.read_text(path) if path.exists() else ""

    linux_ir = declared("aarch64-unknown-linux-gnu")
    check("@lstat(" in linux_ir and "@_stat64" not in linux_ir, "linux: the metadata call is not `lstat`")
    intel_mac = declared("x86_64-apple-darwin")
    check('@"lstat$INODE64"' in intel_mac, "x86-64 macOS: the metadata call is not `lstat$INODE64`")
    arm_mac = declared("aarch64-apple-darwin")
    check("@lstat(" in arm_mac and "INODE64" not in arm_mac, "arm64 macOS: the metadata call is not plain `lstat`")
    windows_ir = declared("x86_64-pc-windows-msvc")
    check("@_stat64(" in windows_ir, "windows: the metadata call is not `_stat64`")
    x86_linux = declared("x86_64-unknown-linux-gnu")

    def stat_words(ir: str) -> str:
        # The size of the `struct stat` buffer, in 8-byte words: `%stat.buf = alloca [N x i64]`.
        match = re.search(r"%stat\.buf = alloca \[(\d+) x i64\]", ir)
        return match.group(1) if match else "?"

    check(stat_words(linux_ir) == "16", f"arm64 linux: `struct stat` is {stat_words(linux_ir)} words, not 16")
    check(stat_words(x86_linux) == "18", f"x86-64 linux: `struct stat` is {stat_words(x86_linux)} words, not 18")
    check(stat_words(intel_mac) == "18", f"macOS: `struct stat` is {stat_words(intel_mac)} words, not 18")
    check(stat_words(windows_ir) == "7", f"windows: `struct _stat64` is {stat_words(windows_ir)} words, not 7")

    # --- XC7, XC8: the link ---------------------------------------------------------
    sysroot = work / "sysroot"
    for triple, _, _, os_name, _ in TARGETS:
        record.unlink(missing_ok=True)
        for leftover in work.glob("hello.exe"):
            leftover.unlink()
        (work / "hello").unlink(missing_ok=True)
        env = environment(
            {**base_env, variable("BURDIN_CC", triple): str(stub), "BURDIN_CC": "/definitely-not-a-compiler"}
        )
        if triple != HOST:
            built = build("hello.bd", triple, env={**env, variable("BURDIN_SYSROOT", triple): str(sysroot)})
        else:
            continue
        check(built.returncode == 0, f"{triple}: link failed: {harness.text(built.stderr)}")
        calls = invocations(record)
        check(len(calls) == 1, f"{triple}: expected one compiler invocation, saw {len(calls)}")
        if len(calls) != 1:
            continue
        args = calls[0]
        check(f"--target={triple}" in args, f"{triple}: the link does not pass --target: {args}")
        check(f"--sysroot={sysroot}" in args, f"{triple}: the link does not pass the sysroot: {args}")
        check("-fuse-ld=lld" in args, f"{triple}: the link does not use lld: {args}")
        if os_name == "Windows":
            check("-lm" not in args, f"{triple}: a Windows link names -lm")
            check("-lkernel32" in args and "-lws2_32" in args, f"{triple}: a Windows link lacks the system libraries")
            check((work / "hello.exe").exists(), f"{triple}: the executable is not named hello.exe")
            check(any(a.endswith("hello.exe") for a in args), f"{triple}: the output is not hello.exe: {args}")
        else:
            check("-lm" in args, f"{triple}: the link does not name -lm: {args}")
            check((work / "hello").exists(), f"{triple}: the executable is not named hello")

    # `BURDIN_CC` is used when no per-triple variable is set.
    record.unlink(missing_ok=True)
    generic = build("hello.bd", "aarch64-unknown-linux-gnu" if HOST != "aarch64-unknown-linux-gnu" else "x86_64-apple-darwin")
    check(generic.returncode == 0, f"BURDIN_CC: {harness.text(generic.stderr)}")
    check(len(invocations(record)) == 1, "BURDIN_CC was not used for a cross link")

    # A different linker by `BURDIN_LD`, and the sysroot by the generic variable.
    record.unlink(missing_ok=True)
    cross_triple = "aarch64-unknown-linux-gnu" if HOST != "aarch64-unknown-linux-gnu" else "x86_64-unknown-linux-gnu"
    env = environment({**base_env, "BURDIN_CC": str(stub), "BURDIN_LD": "mold", "BURDIN_SYSROOT": str(sysroot)})
    other = build("hello.bd", cross_triple, env=env)
    calls = invocations(record)
    check(other.returncode == 0 and len(calls) == 1, f"BURDIN_LD: {harness.text(other.stderr)}")
    if calls:
        check("-fuse-ld=mold" in calls[0], f"BURDIN_LD=mold was not passed: {calls[0]}")
        check(f"--sysroot={sysroot}" in calls[0], "BURDIN_SYSROOT was not passed")

    # The clang-cl form for a Windows target (the stand-in must be named like the driver).
    if not WINDOWS_HOST:
        clang_cl = make_stub(work / "clang-cl-stub", "clang-cl")
        record.unlink(missing_ok=True)
        env = environment({**base_env, "BURDIN_CC": str(clang_cl), "BURDIN_SYSROOT": str(sysroot)})
        built = build("hello.bd", "aarch64-pc-windows-msvc", env=env)
        calls = invocations(record)
        check(built.returncode == 0 and len(calls) == 1, f"clang-cl: {harness.text(built.stderr)}")
        if calls:
            args = calls[0]
            check("--target=aarch64-pc-windows-msvc" in args, f"clang-cl: no --target: {args}")
            check("/winsysroot" in args and str(sysroot) in args, f"clang-cl: no /winsysroot: {args}")
            check(any(a.startswith("/Fe:") for a in args), f"clang-cl: the MSVC link arguments are missing: {args}")
            check(not any(a.startswith("-fuse-ld") for a in args), "clang-cl: a -fuse-ld flag was passed")

    # No cross linker anywhere: the error names the variables, and `--emit obj` still has its object. (not on a Windows host, where an empty PATH also hides LLVM's DLL from the driver itself)
    if not WINDOWS_HOST:
        # A target that is not the host's, whatever the host is.
        ABSENT = "aarch64-apple-darwin" if HOST != "aarch64-apple-darwin" else "x86_64-apple-darwin"
        empty = work / "empty-path"
        empty.mkdir(exist_ok=True)
        env = environment({"PATH": str(empty)})
        lonely = harness.run(
            [burdin, "build", "hello.bd", "--target", ABSENT, "--emit", "obj", "-o", "lonely"],
            cwd=work,
            env=env,
        )
        err = harness.text(lonely.stderr)
        check(lonely.returncode != 0, "a cross link without a linker succeeded")
        check(variable("BURDIN_CC", ABSENT) in err and variable("BURDIN_SYSROOT", ABSENT) in err, f"no-linker error: {err}")
        check((work / "lonely.o").exists(), "the object of a build whose link failed was not kept")

    # A failing cross linker reports its output and the sysroot hint.
    failing = work / "failing"
    harness.write_text(failing, "#!/bin/sh\necho 'cross link boom' >&2\nexit 1\n")
    failing.chmod(0o755)
    if not WINDOWS_HOST:
        env = environment({"BURDIN_CC": str(failing)})
        broken = build("hello.bd", "aarch64-unknown-linux-gnu", env=env)
        err = harness.text(broken.stderr)
        check(broken.returncode != 0 and "cross link boom" in err, f"failing cross linker: {err}")
        check("BURDIN_SYSROOT_AARCH64_UNKNOWN_LINUX_GNU" in err, "the failed cross link does not name the sysroot variable")

    # --- XC9: the host's own triple is a native build, and a cross build is not cached ----
    native = work / "native"
    native_env = environment({})
    built = harness.run([burdin, "build", "hello.bd", "--target", HOST, "-o", str(native)], cwd=work, env=native_env)
    check(built.returncode == 0, f"--target {HOST}: {harness.text(built.stderr)}")
    ran = harness.run([harness.executable_in(work, "native")], cwd=work)
    check(harness.text(ran.stdout).strip() == "hello", f"--target {HOST}: the program printed {harness.text(ran.stdout)!r}")

    package = work / "pkg"
    (package / "src").mkdir(parents=True)
    harness.write_text(package / "burdin.toml", '[package]\nname = "pkg"\nversion = "0.1.0"\n')
    harness.write_text(package / "src" / "main.bd", HELLO)
    cache_env = environment({**base_env, "BURDIN_CC": str(stub)})
    cross = harness.run(
        [burdin, "build", str(package), "--target", "aarch64-unknown-linux-gnu", "-o", str(work / "pkg-out")],
        cwd=work,
        env=cache_env,
    )
    check(cross.returncode == 0, f"package cross build: {harness.text(cross.stderr)}")
    check(not (package / "target" / "burdin" / "dev" / "bin").exists(), "a cross build wrote to the artifact cache")

if problems:
    harness.report_problems(problems)
    print(f"{len(problems)} problem(s) in {checks} checks")
    sys.exit(1)
print(f"all {checks} cross-compilation checks passed")
