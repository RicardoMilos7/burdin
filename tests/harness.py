#!/usr/bin/env python3
"""Shared helpers for the Python checks and tools under tests/.

Every script is run as `python3 tests/<name>.py` (or `python tests/<name>.py` on Windows), so the tests directory
is on sys.path and `import harness` works on every platform.
"""

from __future__ import annotations

import contextlib
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

TESTS = Path(__file__).resolve().parent
ROOT = TESTS.parent


def platform_name() -> str:
    if sys.platform.startswith("linux"):
        return "linux"
    if sys.platform == "darwin":
        return "macos"
    if sys.platform.startswith("win"):
        return "windows"
    return "unknown"


PLATFORM = platform_name()

# The shell scripts emitted LF on every platform; keep the Python output byte-identical.
if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(newline="\n")
    sys.stderr.reconfigure(newline="\n")


def read_bytes(path) -> bytes:
    return Path(path).read_bytes()


def read_text(path, keep_cr: bool = False) -> str:
    data = read_bytes(path)
    if not keep_cr:
        data = data.replace(b"\r", b"")
    return data.decode("utf-8", "replace")


def normalize(data: bytes) -> bytes:
    """Match the shell tools' `tr -d '\\r'`."""
    return data.replace(b"\r", b"")


def text(data: bytes) -> str:
    return data.decode("utf-8", "replace")


def run(args, cwd=None, env=None, timeout=None, merge: bool = False, stream: bool = False):
    """Run a command with captured output by default, or inherited output with stream=True.

    With merge=True, stderr is redirected into the captured stdout, like the shell's `2>&1`.
    """
    command = [str(argument) for argument in args]
    if stream:
        stdout, stderr = None, None
    elif merge:
        stdout, stderr = subprocess.PIPE, subprocess.STDOUT
    else:
        stdout, stderr = subprocess.PIPE, subprocess.PIPE
    return subprocess.run(
        command,
        cwd=str(cwd) if cwd is not None else None,
        env=env,
        input=None,
        stdout=stdout,
        stderr=stderr,
        timeout=timeout,
        check=False,
    )


def find_burdin():
    value = os.environ.get("BURDIN")
    if value:
        return Path(value).resolve()
    for profile in ("debug", "release"):
        for name in ("burdin", "burdin.exe"):
            candidate = ROOT / "target" / profile / name
            if candidate.is_file() and os.access(candidate, os.X_OK):
                return candidate
    found = shutil.which("burdin")
    return Path(found) if found else None


def require_burdin(code: int = 1, message: str = "No driver found: build it with `cargo build` or set BURDIN") -> str:
    path = find_burdin()
    if path is None:
        print(message, file=sys.stderr)
        raise SystemExit(code)
    return str(path)


def which(tool: str):
    return shutil.which(tool)


def find_llvm_tool(name: str):
    """A usable tool, preferring the LLVM 21 spelling and LLVM_SYS_211_PREFIX/bin, like the shell checks."""
    prefix = os.environ.get("LLVM_SYS_211_PREFIX", "")
    candidates = []
    if prefix:
        candidates.append(str(Path(prefix) / "bin" / f"{name}-21"))
    candidates.append(f"{name}-21")
    if prefix:
        candidates.append(str(Path(prefix) / "bin" / name))
    candidates.append(name)
    for candidate in candidates:
        found = shutil.which(candidate)
        if found:
            return found
        found = shutil.which(candidate + ".exe")
        if found:
            return found
    return None


def executable_in(directory, stem: str) -> Path:
    """The executable a build produced: `stem`, or `stem.exe` on Windows."""
    for name in (stem, stem + ".exe"):
        candidate = Path(directory) / name
        if candidate.is_file():
            return candidate
    return Path(directory) / stem


def file_size(path) -> int:
    return Path(path).stat().st_size


@contextlib.contextmanager
def temporary_dir(prefix: str = "burdin-"):
    path = Path(tempfile.mkdtemp(prefix=prefix))
    try:
        yield path
    finally:
        shutil.rmtree(path, ignore_errors=True)


def write_text(path, content: str) -> None:
    Path(path).write_bytes(content.encode("utf-8"))


def report_problems(problems, blank_line: bool = True) -> int:
    """Print the common Problems list; return the exit code."""
    if not problems:
        return 0
    if blank_line:
        print()
    print("Problems:")
    for problem in problems:
        print(f"  {problem}")
    return 1


def workflow_job_block(workflow_text: str, job: str) -> str:
    """The lines of one job: from `  <job>:` to the next job at the same indentation."""
    lines = workflow_text.split("\n")
    start = None
    for index, line in enumerate(lines):
        if line == f"  {job}:":
            start = index + 1
            break
    if start is None:
        return ""
    for index in range(start, len(lines)):
        if re.fullmatch(r"  [A-Za-z0-9_-]+:", lines[index]):
            return "\n".join(lines[start:index])
    return "\n".join(lines[start:])


def count_lines_containing(data: bytes, needle: bytes) -> int:
    return sum(1 for line in data.split(b"\n") if needle in line)
