#!/usr/bin/env python3
"""Checks the linker choice (stage 3, issue #335).

Needs a built driver. BURDIN selects it, like tests/run_tests.py.

What it checks:
  * with `BURDIN_LD` unset, a Linux build names `mold` or `lld` when `ld.mold` or `ld.lld` is on `PATH`, and leaves
    the choice to the C compiler driver (no `-fuse-ld`) otherwise; macOS and Windows keep the platform linker;
  * `BURDIN_LD` names the linker (`mold`, `lld`, a path) and wins over the automatic choice; `BURDIN_LD=platform`
    forces the compiler driver's own linker; an empty value is the same as unset;
  * `BURDIN_LD` naming an unavailable linker fails, and the error says that `BURDIN_LD` chose it;
  * mold with `--lto` is refused before anything is linked, because mold cannot read LLVM bitcode;
  * a sanitizer build keeps the compiler driver's linker: `BURDIN_LD` naming another one is a usage error (the
    runtimes start up from module constructors in `.ctors`, which lld leaves where libc never runs them), and an
    installed `ld.mold` is not picked up;
  * an MSVC-style compiler driver (`cl`) has no `-fuse-ld`, so any other `BURDIN_LD` value is a usage error;
  * the program builds and runs in every case, and with a real fast linker installed its build time is compared
    with the platform linker's (reported, not asserted: CI machines vary).

The C compiler driver is replaced by a wrapper script that records its argument vector and then runs the real
compiler, so `-fuse-ld=` is observed rather than guessed. The fast linkers are stand-in scripts that run the real
`ld`, which proves the driver asked for them without needing mold or lld installed.
"""

import os
import shutil
import statistics
import sys
import time
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

HELLO = 'fn main() {\n    println("linker ok")\n}\n'
EXPECTED = "linker ok\n"


def compiler():
    """An `cc`-style compiler: the one `BURDIN_CC` names, or the first of cc, clang, and gcc on `PATH`."""
    named = os.environ.get("BURDIN_CC")
    if named:
        return named
    for name in ("cc", "clang", "gcc"):
        found = shutil.which(name)
        if found:
            return found
    return None


def script(path: Path, text: str) -> None:
    harness.write_text(path, text)
    path.chmod(0o755)


def environment(extra: dict, path: str = None) -> dict:
    """The check's environment: no inherited `BURDIN_LD`/`BURDIN_CC`, and the given settings."""
    env = dict(os.environ)
    for key in ("BURDIN_LD", "BURDIN_CC"):
        env.pop(key, None)
    env.update(extra)
    if path is not None:
        env["PATH"] = path
    return env


def build(args, env):
    return harness.run([burdin, *args], cwd=work, env=env)


def recorded_arguments(log: Path, expected, label: str) -> None:
    """The wrapper's argument log carries `expected` as a whole argument (`None`: no `-fuse-ld` at all)."""
    lines = harness.read_text(log).splitlines() if log.is_file() else []
    fuses = [line for line in lines if line.startswith("-fuse-ld=")]
    if expected is None:
        check(not fuses, f"{label}: unexpected `-fuse-ld` in the compiler's arguments: {fuses}")
    else:
        check(fuses == [expected], f"{label}: expected `{expected}`, the compiler got {fuses}")
        check(lines.index(expected) == 0, f"{label}: `{expected}` is not the first argument: {lines[:3]}")


def builds_and_runs(args, env, label: str) -> None:
    exe = work / ("hello.exe" if harness.PLATFORM == "windows" else "hello")
    if exe.exists():
        exe.unlink()
    result = build(args, env)
    check(result.returncode == 0, f"{label}: the build failed ({result.returncode}): {harness.text(result.stderr)[:500]}")
    if result.returncode != 0:
        return
    check(exe.is_file(), f"{label}: the build wrote no executable")
    if not exe.is_file():
        return
    ran = harness.run([exe], cwd=work, env=env)
    check(ran.returncode == 0, f"{label}: the program exited {ran.returncode}")
    check(harness.text(ran.stdout).replace("\r", "") == EXPECTED, f"{label}: the program's output differs")


with harness.temporary_dir("burdin-linker-") as work:
    harness.write_text(work / "hello.bd", HELLO)

    if harness.PLATFORM == "windows":
        # --- An MSVC-style driver keeps link.exe ------------------------------------------------------------
        builds_and_runs(["build", "hello.bd"], environment({}), "windows default")
        builds_and_runs(["build", "hello.bd"], environment({"BURDIN_LD": "platform"}), "BURDIN_LD=platform")
        builds_and_runs(["build", "hello.bd"], environment({"BURDIN_LD": "link"}), "BURDIN_LD=link")
        builds_and_runs(["build", "hello.bd"], environment({"BURDIN_LD": ""}), "BURDIN_LD empty")
        for value in ("mold", "lld", "/usr/bin/ld.lld"):
            result = build(["build", "hello.bd"], environment({"BURDIN_LD": value}))
            text = harness.text(result.stderr)
            check(result.returncode == 2, f"BURDIN_LD={value} with an MSVC driver exited {result.returncode}, expected 2")
            check("BURDIN_LD" in text and value in text, f"BURDIN_LD={value} is not named in the error: {text[:400]}")
            check("link.exe" in text, f"BURDIN_LD={value} does not say link.exe is the MSVC linker: {text[:400]}")
    else:
        # --- The Unix command line, observed through a wrapper around the compiler driver ------------------
        real_cc = compiler()
        real_ld = shutil.which("ld")
        real_as = shutil.which("as")
        if real_cc and real_ld and real_as:
            bin_dir = work / "bin"
            bin_dir.mkdir()
            for tool in (real_ld, real_as):
                (bin_dir / Path(tool).name).symlink_to(tool)
            # A fast linker is what `-fuse-ld=` names; a stand-in runs the real `ld` so the link still succeeds.
            linker_log = work / "linkers.txt"
            # `ld64.lld` is the name Apple's clang looks for when `-fuse-ld=lld` names a Mach-O linker.
            for name in ("ld.mold", "ld.lld", "ld64.lld"):
                script(
                    bin_dir / name,
                    f'#!/bin/sh\nprintf "%s\\n" "{name}" >> "{linker_log}"\nexec "{real_ld}" "$@"\n',
                )
            wrapper = work / "cc-wrapper"
            cc_log = work / "cc-arguments.txt"
            script(wrapper, f'#!/bin/sh\nprintf "%s\\n" "$@" >> "{cc_log}"\nexec "{real_cc}" "$@"\n')
            # macOS keeps the platform linker (ld64), so only Linux picks one up without being asked. The
            # stand-ins still test the explicit `BURDIN_LD` on both.
            stock = [str(bin_dir)] if harness.PLATFORM == "linux" else [str(bin_dir), "/usr/bin", "/bin"]
            host_path = os.pathsep.join(stock)

            def unix_build(extra, expected, label):
                if cc_log.is_file():
                    cc_log.unlink()
                if linker_log.is_file():
                    linker_log.unlink()
                env = environment({"BURDIN_CC": str(wrapper), **extra}, path=host_path)
                builds_and_runs(["build", "hello.bd"], env, label)
                recorded_arguments(cc_log, expected, label)
                return harness.read_text(linker_log) if linker_log.is_file() else ""

            # No fast linker on PATH: the compiler driver's own linker.
            for name in ("ld.mold", "ld.lld"):
                (bin_dir / name).unlink()
            unix_build({}, None, "no fast linker")
            # mold is preferred to lld, and the stand-in really runs.
            for name in ("ld.mold", "ld.lld"):
                script(bin_dir / name, f'#!/bin/sh\nprintf "%s\\n" "{name}" >> "{linker_log}"\nexec "{real_ld}" "$@"\n')
            ran = unix_build({}, "-fuse-ld=mold" if harness.PLATFORM == "linux" else None, "both installed")
            if harness.PLATFORM == "linux":
                check("ld.mold" in ran, "the automatic choice did not run ld.mold")
            # With only lld installed, lld is chosen.
            (bin_dir / "ld.mold").unlink()
            unix_build({}, "-fuse-ld=lld" if harness.PLATFORM == "linux" else None, "lld installed")
            # A chosen-but-broken linker fails the build, and the error says how to go back to the default.
            if harness.PLATFORM == "linux":
                script(bin_dir / "ld.mold", "#!/bin/sh\nexit 1\n")
                broken = build(["build", "hello.bd"], environment({"BURDIN_CC": str(wrapper)}, path=host_path))
                text = harness.text(broken.stderr)
                check(broken.returncode == 2, f"a broken automatic linker exited {broken.returncode}, expected 2")
                check(
                    "`mold` linker was selected automatically" in text and "BURDIN_LD=platform" in text,
                    f"a broken automatic linker does not say how to change it: {text[:500]}",
                )
            # `BURDIN_LD` wins over the automatic choice, and `platform` turns it off. mold's name is only tried
            # where the compiler driver is known to accept it; the choice itself is tested on Linux.
            script(bin_dir / "ld.mold", f'#!/bin/sh\nprintf "%s\\n" "ld.mold" >> "{linker_log}"\nexec "{real_ld}" "$@"\n')
            unix_build({"BURDIN_LD": "lld"}, "-fuse-ld=lld", "BURDIN_LD=lld")
            if harness.PLATFORM == "linux":
                unix_build({"BURDIN_LD": "mold"}, "-fuse-ld=mold", "BURDIN_LD=mold")
            unix_build({"BURDIN_LD": "platform"}, None, "BURDIN_LD=platform")
            unix_build({"BURDIN_LD": "default"}, None, "BURDIN_LD=default")
            # A named linker that is not there fails, and the error names BURDIN_LD.
            result = build(
                ["build", "hello.bd"],
                environment({"BURDIN_CC": str(wrapper), "BURDIN_LD": "burdin-no-such-linker"}, path=host_path),
            )
            text = harness.text(result.stderr)
            check(result.returncode == 2, f"a missing BURDIN_LD linker exited {result.returncode}, expected 2")
            check("linking failed" in text, f"a missing BURDIN_LD linker is not a link failure: {text[:400]}")
            check(
                "BURDIN_LD selected the `burdin-no-such-linker` linker" in text,
                f"the link failure does not name BURDIN_LD: {text[:600]}",
            )
        else:
            print("skipping the compiler-wrapper checks: no cc, ld, and as found", file=sys.stderr)

        # --- `--lto` refuses mold before compiling ---------------------------------------------------------
        result = build(["build", "hello.bd", "--lto"], environment({"BURDIN_LD": "mold"}))
        text = harness.text(result.stderr)
        check(result.returncode == 2, f"`--lto` with BURDIN_LD=mold exited {result.returncode}, expected 2")
        check("mold" in text and "--lto" in text, f"the mold refusal does not explain itself: {text[:400]}")

        # --- A sanitizer build keeps the compiler driver's linker ------------------------------------------
        if harness.PLATFORM == "linux":
            # Naming a fast linker there is a usage error: the runtimes start up from module constructors in
            # `.ctors`, and lld keeps those out of `.init_array`, so the sanitizer would never initialize.
            result = build(["build", "hello.bd", "--sanitize=address"], environment({"BURDIN_LD": "lld"}))
            text = harness.text(result.stderr)
            check(
                result.returncode == 2,
                f"BURDIN_LD=lld with --sanitize=address exited {result.returncode}, expected 2",
            )
            check("BURDIN_LD" in text and "lld" in text, f"the sanitizer refusal does not name BURDIN_LD: {text[:400]}")
            check(".ctors" in text, f"the sanitizer refusal does not explain itself: {text[:500]}")
            # `platform` is the same as unset, so the build goes ahead (it needs clang, which the checks below guard).
            result = build(["build", "hello.bd", "--sanitize=address"], environment({"BURDIN_LD": "platform"}))
            check("BURDIN_LD" not in harness.text(result.stderr), "BURDIN_LD=platform was refused for a sanitizer build")
            # An installed fast linker is not picked up: the build still works with an `ld.mold` that would fail it.
            if shutil.which("clang-21") or shutil.which("clang"):
                only_mold = work / "mold-only"
                only_mold.mkdir(exist_ok=True)
                script(only_mold / "ld.mold", "#!/bin/sh\nexit 1\n")
                path = os.pathsep.join([str(only_mold), os.environ.get("PATH", "")])
                result = build(["build", "hello.bd", "--sanitize=address"], environment({}, path=path))
                check(
                    result.returncode == 0,
                    f"a sanitizer build picked up ld.mold: {harness.text(result.stderr)[:500]}",
                )

    # --- A real fast linker, when this machine has one: build, run, and time it ----------------------------
    if harness.PLATFORM == "linux":
        installed = [name for name in ("mold", "lld") if shutil.which(f"ld.{name}")]
        if installed:
            times = {}
            for choice in ["platform", installed[0]]:
                durations = []
                for _ in range(3):
                    started = time.monotonic()
                    result = build(["build", "hello.bd", "-o", f"timed-{choice}"], environment({"BURDIN_LD": choice}))
                    durations.append(time.monotonic() - started)
                    check(result.returncode == 0, f"the timed build with {choice} failed: {harness.text(result.stderr)[:300]}")
                times[choice] = statistics.median(durations)
            # Informational: a CI machine's timing is not a threshold. The closing comment reports the suite's.
            summary = ", ".join(f"{name}: {seconds * 1000:.0f} ms" for name, seconds in times.items())
            print(f"build time, median of 3 (hello world): {summary}")
        else:
            print("no lld or mold installed; the fast-linker build and timing were skipped", file=sys.stderr)

if problems:
    print("\n".join(problems))
    print(f"{len(problems)} problem(s) in {checks} checks")
    sys.exit(1)
print(f"all {checks} linker checks passed")
