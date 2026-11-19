#!/usr/bin/env python3
"""Checks the toolchain manager and the `burdin` proxy (stage 3, issues #624, #626, #628, #630, #631;
spec/toolchain.md).

Needs the built manager (`burdin-toolchain` under target/debug or target/release, or BURDIN_TOOLCHAIN_MANAGER),
`rustc` to build the fake compiler in tests/toolchain/fake_burdin.rs, and `curl` for the HTTP part. Every release is
a fixture made here: a distribution root (`channels/<channel>`, `v<version>/SHA256SUMS`, and the archives) whose
compiler is the fake, so nothing touches the network, the real Burdin home, or the user's PATH.

What it checks, with the exact output where the format is stable:
  * with nothing installed, the proxy and `show` say so (TC7);
  * `install`, `list`, `default`, `remove`, `which`, and the proxy running the selected toolchain's binary with its
    arguments and exit status (TC6, TC9);
  * `+toolchain`, BURDIN_TOOLCHAIN, burdin-toolchain.toml (found from a subdirectory), and the default, in that
    order, with `show` naming the one that applied; the file's toolchain and components installed on first use; bad
    files and names refused (TC7, TC8);
  * `update` moves a channel to its latest release and keeps its components; an exact version never changes; an
    update that fails verification leaves the old toolchain working and nothing behind (TC5, TC6);
  * downloads resume from a partial file, a partial file that does not verify is fetched again, and a release
    without the archive or without a SHA256SUMS entry is refused (TC5);
  * the same over HTTP through curl, with a range request for the resumed download (TC4, TC5);
  * components: list, add, remove (only their files), a release without the component, and the burdin-lsp proxy
    (TC10); targets: the host is installed, any other is refused until cross-compilation exists (TC11);
  * the installer's bootstrap (install.sh, or install.ps1 on Windows), `self show`, `self update` replacing the
    proxies, and `self uninstall` removing exactly what was created and nothing else (TC12-TC15).
"""

import http.server
import io
import os
import shutil
import sys
import tarfile
import threading
from pathlib import Path

import harness

problems = []
checks = 0


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        problems.append(message)


EXE = ".exe" if harness.PLATFORM == "windows" else ""


def find_manager():
    value = os.environ.get("BURDIN_TOOLCHAIN_MANAGER")
    if value:
        return Path(value).resolve()
    for profile in ("debug", "release"):
        candidate = harness.ROOT / "target" / profile / f"burdin-toolchain{EXE}"
        if candidate.is_file():
            return candidate
    return None


MANAGER = find_manager()
if MANAGER is None:
    print("No toolchain manager found: build it with `cargo build` or set BURDIN_TOOLCHAIN_MANAGER", file=sys.stderr)
    sys.exit(1)
if harness.which("rustc") is None:
    print("check_toolchain.py needs `rustc` on PATH to build the fake compiler", file=sys.stderr)
    sys.exit(1)

rustc_info = harness.text(harness.run(["rustc", "-vV"]).stdout)
HOST = next((line[6:].strip() for line in rustc_info.split("\n") if line.startswith("host: ")), "")
OTHER = "aarch64-unknown-linux-gnu" if HOST != "aarch64-unknown-linux-gnu" else "x86_64-unknown-linux-gnu"
COMPONENTS = ("debuggers", "docs", "lsp", "std-src")

with harness.temporary_dir("burdin-toolchain-") as work:
    work = work.resolve()
    WORK_TEXTS = sorted({str(work), str(work).replace("\\", "/")}, key=len, reverse=True)

    def norm(data) -> str:
        text = harness.text(data).replace("\r", "")
        for prefix in WORK_TEXTS:
            text = text.replace(prefix, "<work>")
        return text.replace("\\", "/")

    # On Windows, rustc must be told which linker to use: Git's coreutils `link.exe` can come first on PATH, and CI
    # pins the MSVC one for cargo with CARGO_TARGET_<TRIPLE>_LINKER (the same trap the driver's build avoids).
    pinned_linker = os.environ.get(f"CARGO_TARGET_{HOST.upper().replace('-', '_')}_LINKER")
    linker_flags = ["-C", f"linker={pinned_linker}"] if pinned_linker else []
    built = harness.run(
        ["rustc", "--edition", "2021", *linker_flags, "-o", work / f"fake_burdin{EXE}",
         harness.TESTS / "toolchain" / "fake_burdin.rs"],
        merge=True,
    )
    if built.returncode != 0:
        print(f"cannot build the fake compiler:\n{harness.text(built.stdout)}", file=sys.stderr)
        sys.exit(1)
    FAKE = (work / f"fake_burdin{EXE}").read_bytes()
    MANAGER_BYTES = MANAGER.read_bytes()

    dist = work / "dist"

    def make_archive(path: Path, root: str, files: dict) -> None:
        with tarfile.open(path, "w:gz") as tar:
            directories = set()
            for relative in sorted(files):
                parts = relative.split("/")
                for depth in range(len(parts)):
                    directory = "/".join([root, *parts[:depth]])
                    if directory not in directories:
                        directories.add(directory)
                        info = tarfile.TarInfo(directory)
                        info.type = tarfile.DIRTYPE
                        info.mode = 0o755
                        tar.addfile(info)
                data = files[relative]
                info = tarfile.TarInfo(f"{root}/{relative}")
                info.size = len(data)
                executable = relative.split("/")[-1] in (f"burdin{EXE}", f"burdin-lsp{EXE}", f"burdin-toolchain{EXE}")
                info.mode = 0o755 if executable else 0o644
                tar.addfile(info, io.BytesIO(data))

    def sha256(path: Path) -> str:
        import hashlib

        return hashlib.sha256(path.read_bytes()).hexdigest()

    def make_release(version, components=COMPONENTS, manager=None, tamper=False, list_compiler=True):
        release = dist / f"v{version}"
        release.mkdir(parents=True, exist_ok=True)
        names = []
        compiler = f"burdin-{version}-{HOST}.tar.gz"
        make_archive(
            release / compiler,
            f"burdin-{version}-{HOST}",
            {
                f"burdin{EXE}": FAKE,
                "VERSION": f"{version}\n".encode(),
                "README.md": b"fixture release\n",
                f"burdin-toolchain{EXE}": manager if manager is not None else MANAGER_BYTES,
            },
        )
        if list_compiler:
            names.append(compiler)
        contents = {
            "lsp": {f"burdin-lsp{EXE}": FAKE},
            "std-src": {"lib/std/src/std.bd": f"// the standard library of {version}\n".encode()},
            "docs": {"share/doc/docs/releasing.md": b"# Releasing\n", "share/doc/spec/toolchain.md": b"# Toolchains\n"},
            "debuggers": {"debuggers/gdb/burdin_printers.py": b"# printers\n"},
        }
        for component in components:
            name = f"burdin-{component}-{version}-{HOST}.tar.gz"
            make_archive(release / name, f"burdin-{component}-{version}-{HOST}", contents[component])
            names.append(name)
        sums = "".join(f"{sha256(release / name)}  {name}\n" for name in sorted(names))
        (release / "SHA256SUMS").write_text(sums)
        (release / "VERSION").write_text(f"{version}\n")
        if tamper:
            with open(release / compiler, "ab") as archive:
                archive.write(b"tampered")

    def set_channel(channel, version):
        (dist / "channels").mkdir(parents=True, exist_ok=True)
        (dist / "channels" / channel).write_text(f"{version}\n")

    make_release("0.3.0", components=("std-src",))
    make_release("0.4.0")
    make_release("0.4.1")
    make_release("0.5.0-nightly.20261118")
    make_release("0.5.0-nightly.20261119", tamper=True)
    make_release("0.2.0", list_compiler=False)
    set_channel("stable", "0.4.0")
    set_channel("nightly", "0.5.0-nightly.20261118")

    base_env = {
        key: value
        for key, value in os.environ.items()
        if key not in ("BURDIN_TOOLCHAIN", "BURDIN_TOOLCHAIN_DIR", "BURDIN_DIST_ROOT", "BURDIN_HOME")
    }
    home = work / "home"

    def env_for(home_dir=home, root=dist, **extra):
        env = dict(base_env)
        env["BURDIN_HOME"] = str(home_dir)
        env["BURDIN_DIST_ROOT"] = str(root)
        env.update({key: str(value) for key, value in extra.items()})
        return env

    def run(args, cwd=work, exe=MANAGER, env=None):
        result = harness.run([exe, *args], cwd=cwd, env=env or env_for())
        return result.returncode, norm(result.stdout), norm(result.stderr)

    def expect(name, outcome, status=0, stdout=None, stderr_has=None):
        code, out, err = outcome
        check(code == status, f"{name}: exit status {code}, expected {status}\n    stdout: {out!r}\n    stderr: {err!r}")
        if stdout is not None:
            check(out == stdout, f"{name}: stdout\n    expected: {stdout!r}\n    actual:   {out!r}")
        for needle in [stderr_has] if isinstance(stderr_has, str) else (stderr_has or []):
            check(needle in err, f"{name}: stderr does not contain {needle!r}\n    stderr: {err!r}")

    def fake_line(tool, version, args, toolchain):
        return f"fake {tool} {version} [{' '.join(args)}] toolchain={toolchain} dir=true\n"

    toolchains = home / "toolchains"
    lsp_proxy = work / "bin" / f"burdin-lsp{EXE}"
    lsp_proxy.parent.mkdir()
    shutil.copyfile(MANAGER, lsp_proxy)
    lsp_proxy.chmod(0o755)

    # --- Nothing installed (TC7) ---------------------------------------------------------------------------------
    expect("proxy with nothing installed", run(["build"]), 1, "", "no toolchain is selected and no default is set")
    expect("list with nothing installed", run(["toolchain", "list"]), 0, "no toolchains are installed\n")
    expect(
        "show with nothing installed",
        run(["toolchain", "show"]),
        0,
        "active toolchain: none\nselected by: nothing (no +toolchain, BURDIN_TOOLCHAIN, burdin-toolchain.toml, or "
        "default toolchain)\n",
    )
    expect("no command", run(["toolchain"]), 2, "", "Usage: burdin [+<toolchain>] toolchain")
    expect("unknown command", run(["toolchain", "frobnicate"]), 2, "", "unknown command `toolchain frobnicate`")

    # --- Install, the proxy, list (TC6, TC9) ---------------------------------------------------------------------
    expect(
        "install stable",
        run(["toolchain", "install", "stable"]),
        0,
        "installed stable 0.4.0\ndefault toolchain: stable\n",
        f"info: downloading <work>/dist/v0.4.0/burdin-0.4.0-{HOST}.tar.gz",
    )
    check(not any((toolchains / "downloads").iterdir()), "install left a download behind")
    check(not list(toolchains.glob(".staging-*")), "install left a staging directory behind")
    expect("proxy runs the default", run(["build", "main.bd"]), 0, fake_line("burdin", "0.4.0", ["build", "main.bd"], "stable"))
    expect("proxy passes the exit status", run(["--exit", "7"]), 7)
    expect(
        "install two more",
        run(["toolchain", "install", "0.3.0", "nightly"]),
        0,
        "installed 0.3.0 0.3.0\ninstalled nightly 0.5.0-nightly.20261118\n",
    )
    expect("install again", run(["toolchain", "install", "stable"]), 0, "stable 0.4.0 is already installed\n")
    expect(
        "list",
        run(["toolchain", "list"]),
        0,
        "0.3.0 0.3.0\nnightly 0.5.0-nightly.20261118\nstable 0.4.0 (default)\n",
    )
    expect("install a bad name", run(["toolchain", "install", "beta"]), 2, "", "`beta` is not a toolchain")
    expect("install nothing", run(["toolchain", "install"]), 2, "", "needs at least one toolchain")

    # --- Selection and its precedence (TC7, TC8) -----------------------------------------------------------------
    expect("+toolchain", run(["+nightly", "run"]), 0, fake_line("burdin", "0.5.0-nightly.20261118", ["run"], "nightly"))
    expect(
        "BURDIN_TOOLCHAIN",
        run(["test"], env=env_for(BURDIN_TOOLCHAIN="0.3.0")),
        0,
        fake_line("burdin", "0.3.0", ["test"], "0.3.0"),
    )
    expect(
        "+toolchain before BURDIN_TOOLCHAIN",
        run(["+stable", "test"], env=env_for(BURDIN_TOOLCHAIN="0.3.0")),
        0,
        fake_line("burdin", "0.4.0", ["test"], "stable"),
    )
    expect(
        "+toolchain not installed",
        run(["+0.9.9", "build"]),
        1,
        "",
        "toolchain 0.9.9 is not installed; install it with `burdin toolchain install 0.9.9`",
    )
    expect("+toolchain not a name", run(["+beta", "build"]), 1, "", "+beta: `beta` is not a toolchain")
    expect("BURDIN_TOOLCHAIN not a name", run(["build"], env=env_for(BURDIN_TOOLCHAIN="beta")), 1, "", "BURDIN_TOOLCHAIN: `beta`")

    project = work / "proj"
    sub = project / "src" / "deep"
    sub.mkdir(parents=True)
    harness.write_text(project / "burdin-toolchain.toml", '[toolchain]\nchannel = "0.4.1"\ncomponents = ["lsp"]\n')
    expect(
        "show selects the file",
        run(["toolchain", "show"], cwd=sub),
        0,
        "active toolchain: 0.4.1\nselected by: <work>/proj/burdin-toolchain.toml\ninstalled: no (installed on first use)\n",
    )
    expect(
        "show selects BURDIN_TOOLCHAIN before the file",
        run(["toolchain", "show"], cwd=sub, env=env_for(BURDIN_TOOLCHAIN="nightly")),
        0,
        "active toolchain: nightly\nselected by: the BURDIN_TOOLCHAIN environment variable\n"
        "installed: 0.5.0-nightly.20261118\n",
    )
    expect(
        "show selects +toolchain first",
        run(["+0.3.0", "toolchain", "show"], cwd=sub, env=env_for(BURDIN_TOOLCHAIN="nightly")),
        0,
        "active toolchain: 0.3.0\nselected by: the +toolchain argument on the command line\ninstalled: 0.3.0\n",
    )
    expect(
        "show falls back to the default",
        run(["toolchain", "show"]),
        0,
        "active toolchain: stable\nselected by: the default toolchain\ninstalled: 0.4.0\n",
    )
    expect(
        "the file's toolchain is installed on first use",
        run(["check"], cwd=sub),
        0,
        fake_line("burdin", "0.4.1", ["check"], "0.4.1"),
        "info: installing 0.4.1 for <work>/proj/burdin-toolchain.toml",
    )
    expect(
        "the file's components are installed with it",
        run(["--stdio"], cwd=sub, exe=lsp_proxy),
        0,
        fake_line("burdin-lsp", "0.4.1", ["--stdio"], "0.4.1"),
    )
    expect("second use installs nothing", run(["check"], cwd=sub), 0, fake_line("burdin", "0.4.1", ["check"], "0.4.1"))
    harness.write_text(
        project / "burdin-toolchain.toml", '[toolchain]\nchannel = "0.4.1"\ncomponents = ["lsp", "std-src"]\n'
    )
    expect(
        "a component added to the file is installed on next use",
        run(["check"], cwd=sub),
        0,
        fake_line("burdin", "0.4.1", ["check"], "0.4.1"),
        "info: installing component std-src of 0.4.1 for <work>/proj/burdin-toolchain.toml",
    )
    check((toolchains / "0.4.1" / "lib" / "std" / "src" / "std.bd").is_file(), "std-src did not install lib/std/src")

    bad = work / "bad-file"
    bad.mkdir()
    harness.write_text(bad / "burdin-toolchain.toml", '[toolchain]\nchannel = "beta"\n')
    expect(
        "a bad toolchain file",
        run(["build"], cwd=bad),
        1,
        "",
        "<work>/bad-file/burdin-toolchain.toml: line 2: `beta` is not a toolchain",
    )
    harness.write_text(bad / "burdin-toolchain.toml", '[toolchain]\nchannel = "stable"\nprofile = "minimal"\n')
    expect("an unknown key", run(["toolchain", "show"], cwd=bad), 1, "", "line 3: unknown key `profile`")
    cross = work / "cross"
    cross.mkdir()
    harness.write_text(cross / "burdin-toolchain.toml", f'[toolchain]\nchannel = "stable"\ntargets = ["{OTHER}"]\n')
    expect(
        "a file naming another target",
        run(["build"], cwd=cross),
        1,
        "",
        f"cannot add the target {OTHER}: cross-compilation is not available yet (issue #282)",
    )

    # --- Update (TC5, TC6) ---------------------------------------------------------------------------------------
    expect("component add before update", run(["toolchain", "component", "add", "lsp"]), 0, "installed lsp in stable\n")
    set_channel("stable", "0.4.1")
    expect(
        "update",
        run(["toolchain", "update"]),
        0,
        "nightly: 0.5.0-nightly.20261118 is up to date\nstable: updated 0.4.0 -> 0.4.1\n",
    )
    expect("the updated toolchain runs", run(["x"]), 0, fake_line("burdin", "0.4.1", ["x"], "stable"))
    expect(
        "the update kept the components",
        run(["--stdio"], exe=lsp_proxy),
        0,
        fake_line("burdin-lsp", "0.4.1", ["--stdio"], "stable"),
    )
    expect("update again", run(["toolchain", "update", "stable"]), 0, "stable: 0.4.1 is up to date\n")
    expect(
        "update an exact version",
        run(["toolchain", "update", "0.3.0"]),
        0,
        "0.3.0: 0.3.0 is an exact version and never changes\n",
    )
    set_channel("nightly", "0.5.0-nightly.20261119")
    expect(
        "an update that fails verification",
        run(["toolchain", "update", "nightly"]),
        1,
        "",
        f"error: nightly: checksum mismatch for burdin-0.5.0-nightly.20261119-{HOST}.tar.gz",
    )
    expect(
        "the old toolchain survives a failed update",
        run(["+nightly", "x"]),
        0,
        fake_line("burdin", "0.5.0-nightly.20261118", ["x"], "nightly"),
    )
    check(not any((toolchains / "downloads").iterdir()), "a failed update left a download behind")
    check(not list(toolchains.glob(".staging-*")), "a failed update left a staging directory behind")
    set_channel("nightly", "0.5.0-nightly.20261118")

    # --- Resumed and refused downloads (TC5) ---------------------------------------------------------------------
    archive_030 = f"burdin-0.3.0-{HOST}.tar.gz"
    data_030 = (dist / "v0.3.0" / archive_030).read_bytes()
    expect("remove", run(["toolchain", "remove", "0.3.0"]), 0, "removed 0.3.0\n")
    (toolchains / "downloads" / f"{archive_030}.part").write_bytes(data_030[: len(data_030) // 2])
    expect(
        "a partial download is resumed",
        run(["toolchain", "install", "0.3.0"]),
        0,
        "installed 0.3.0 0.3.0\n",
        f"info: resuming the download of {archive_030}",
    )
    expect("remove again", run(["toolchain", "remove", "0.3.0"]), 0, "removed 0.3.0\n")
    corrupt = bytearray(data_030[: len(data_030) // 2])
    corrupt[10] ^= 0xFF
    (toolchains / "downloads" / f"{archive_030}.part").write_bytes(bytes(corrupt))
    expect(
        "a partial download that does not verify is fetched again",
        run(["toolchain", "install", "0.3.0"]),
        0,
        "installed 0.3.0 0.3.0\n",
        f"info: the resumed download of {archive_030} did not verify; downloading it again",
    )
    expect(
        "a release without the archive",
        run(["toolchain", "install", "0.2.0"]),
        1,
        "",
        f"release 0.2.0 has no archive for {HOST}",
    )
    expect("a release that does not exist", run(["toolchain", "install", "0.1.5"]), 1, "", "release 0.1.5: cannot read")
    check(not (toolchains / "0.2.0").exists() and not (toolchains / "0.1.5").exists(), "a refused install left a toolchain")

    # --- Default, remove, which (TC6) ----------------------------------------------------------------------------
    expect("set the default", run(["toolchain", "default", "nightly"]), 0, "default toolchain: nightly\n")
    expect("print the default", run(["toolchain", "default"]), 0, "nightly\n")
    expect(
        "default installs a missing toolchain",
        run(["toolchain", "default", "0.4.0"]),
        0,
        "installed 0.4.0 0.4.0\ndefault toolchain: 0.4.0\n",
    )
    expect(
        "removing the default",
        run(["toolchain", "remove", "0.4.0"]),
        0,
        "removed 0.4.0\n0.4.0 was the default toolchain; no default is set now\n",
    )
    expect("no default", run(["toolchain", "default"]), 1, "", "no default toolchain is set")
    expect("remove a missing toolchain", run(["toolchain", "remove", "0.4.0"]), 1, "", "toolchain 0.4.0 is not installed")
    expect("restore the default", run(["toolchain", "default", "stable"]), 0, "default toolchain: stable\n")
    expect("which", run(["toolchain", "which"]), 0, f"<work>/home/toolchains/stable/burdin{EXE}\n")
    expect(
        "which burdin-lsp",
        run(["+0.4.1", "toolchain", "which", "burdin-lsp"]),
        0,
        f"<work>/home/toolchains/0.4.1/burdin-lsp{EXE}\n",
    )
    expect("which another tool", run(["toolchain", "which", "cargo"]), 2, "", "`cargo` is not a toolchain binary")

    # --- Components (TC10) ---------------------------------------------------------------------------------------
    expect(
        "component list",
        run(["toolchain", "component", "list"]),
        0,
        "debuggers (available): the gdb and lldb printers and the Windows Natvis file\n"
        "docs (available): the offline documentation (the docs/ and spec/ pages)\n"
        "lsp (installed): burdin-lsp, the language server without the LLVM back end\n"
        "std-src (available): the standard library's sources, for go-to-definition\n",
    )
    expect(
        "component add",
        run(["toolchain", "component", "add", "docs", "debuggers"]),
        0,
        "installed docs in stable\ninstalled debuggers in stable\n",
    )
    stable_dir = toolchains / "stable"
    check((stable_dir / "share" / "doc" / "spec" / "toolchain.md").is_file(), "docs did not install share/doc")
    check((stable_dir / "debuggers" / "gdb" / "burdin_printers.py").is_file(), "debuggers did not install debuggers/")
    expect("component add again", run(["toolchain", "component", "add", "docs"]), 0, "docs is already installed in stable\n")
    expect("component remove", run(["toolchain", "component", "remove", "docs"]), 0, "removed docs from stable\n")
    check(not (stable_dir / "share").exists(), "component remove left share/ behind")
    check((stable_dir / "debuggers" / "gdb" / "burdin_printers.py").is_file(), "component remove removed another component")
    check((stable_dir / f"burdin{EXE}").is_file(), "component remove removed the compiler")
    expect("component unknown", run(["toolchain", "component", "add", "gui"]), 2, "", "unknown component `gui`")
    expect(
        "component missing from the release",
        run(["toolchain", "component", "add", "lsp", "--toolchain", "0.3.0"]),
        1,
        "",
        f"release 0.3.0 has no lsp component for {HOST}",
    )
    expect(
        "component remove not installed",
        run(["toolchain", "component", "remove", "lsp", "--toolchain", "0.3.0"]),
        1,
        "",
        "0.3.0: component lsp is not installed",
    )
    expect(
        "burdin-lsp without the component",
        run(["+0.3.0", "--stdio"], exe=lsp_proxy),
        1,
        "",
        "toolchain 0.3.0 has no burdin-lsp; add it with `burdin toolchain component add lsp`",
    )

    # --- Targets (TC11) ------------------------------------------------------------------------------------------
    targets = [
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ]
    expect(
        "target list",
        run(["toolchain", "target", "list"]),
        0,
        "".join(f"{t} ({'installed, host' if t == HOST else 'not available'})\n" for t in targets),
    )
    expect(
        "target add the host",
        run(["toolchain", "target", "add", HOST]),
        0,
        f"{HOST} is already installed in stable: it is the host target\n",
    )
    expect(
        "target add another",
        run(["toolchain", "target", "add", OTHER]),
        1,
        "",
        f"cannot add the target {OTHER}: cross-compilation is not available yet (issue #282)",
    )
    expect("target add unknown", run(["toolchain", "target", "add", "riscv64gc-unknown-linux-gnu"]), 2, "", "not one of the six")
    expect("target remove the host", run(["toolchain", "target", "remove", HOST]), 1, "", "is the host target of stable")

    # --- HTTP through curl, with a resumed download (TC4, TC5) ---------------------------------------------------
    if harness.which("curl") is None:
        check(False, "curl is not on PATH; the manager downloads through it")
    else:

        class RangeHandler(http.server.SimpleHTTPRequestHandler):
            ranges = []

            def __init__(self, *args, **kwargs):
                super().__init__(*args, directory=str(dist), **kwargs)

            def log_message(self, *args):
                pass

            def send_head(self):
                path = Path(self.translate_path(self.path))
                if not path.is_file():
                    self.send_error(404)
                    return None
                data = path.read_bytes()
                header = self.headers.get("Range", "")
                if header.startswith("bytes="):
                    start = int(header[6:].split("-")[0] or 0)
                    RangeHandler.ranges.append((self.path, start))
                    if start >= len(data):
                        self.send_response(416)
                        self.send_header("Content-Range", f"bytes */{len(data)}")
                        self.end_headers()
                        return None
                    body = data[start:]
                    self.send_response(206)
                    self.send_header("Content-Range", f"bytes {start}-{len(data) - 1}/{len(data)}")
                else:
                    body = data
                    self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                return io.BytesIO(body)

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), RangeHandler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            url = f"http://127.0.0.1:{server.server_address[1]}"
            http_home = work / "home-http"
            http_env = env_for(home_dir=http_home, root=url)
            expect(
                "install over HTTP",
                run(["toolchain", "install", "stable"], env=http_env),
                0,
                "installed stable 0.4.1\ndefault toolchain: stable\n",
            )
            nightly_archive = f"burdin-0.5.0-nightly.20261118-{HOST}.tar.gz"
            nightly_data = (dist / "v0.5.0-nightly.20261118" / nightly_archive).read_bytes()
            (http_home / "toolchains" / "downloads").mkdir(parents=True, exist_ok=True)
            prefix = len(nightly_data) // 3
            (http_home / "toolchains" / "downloads" / f"{nightly_archive}.part").write_bytes(nightly_data[:prefix])
            expect(
                "a resumed download over HTTP",
                run(["toolchain", "install", "nightly"], env=http_env),
                0,
                "installed nightly 0.5.0-nightly.20261118\n",
                f"info: resuming the download of {nightly_archive}",
            )
            check(
                (f"/v0.5.0-nightly.20261118/{nightly_archive}", prefix) in RangeHandler.ranges,
                f"the resumed download sent no range request from byte {prefix}: {RangeHandler.ranges}",
            )
            expect(
                "the HTTP toolchain runs",
                run(["+nightly", "a"], env=http_env),
                0,
                fake_line("burdin", "0.5.0-nightly.20261118", ["a"], "nightly"),
            )
            expect(
                "a missing release over HTTP",
                run(["toolchain", "install", "0.1.5"], env=http_env),
                1,
                "",
                "release 0.1.5: cannot fetch",
            )
        finally:
            server.shutdown()

    # --- The installer's bootstrap, self show, self update, self uninstall (TC12-TC15) ---------------------------
    own_version = run(["toolchain", "self", "show"])[1].split("\n")[0].removeprefix("burdin toolchain ")
    inst_home = work / "home-inst"
    inst_bin = work / "inst" / "bin"
    user_home = work / "userhome"
    user_home.mkdir()
    # install.sh writes the PATH line to the startup file of the user's shell: ~/.bash_profile on macOS, ~/.bashrc on
    # Linux (the installer's own startup_file rule).
    startup_name = ".bash_profile" if harness.PLATFORM == "macos" else ".bashrc"
    bashrc = user_home / startup_name
    harness.write_text(bashrc, "alias ll='ls -l'\n")
    inst_env = env_for(home_dir=inst_home)
    for key in ("BURDIN_VERSION", "BURDIN_CHANNEL", "BURDIN_INSTALL_DIR", "BURDIN_TARGET", "BURDIN_BASE_URL",
                "BURDIN_RELEASE_DIR", "BURDIN_NO_MODIFY_PATH", "XDG_CONFIG_HOME"):
        inst_env.pop(key, None)
    release_dir = dist / "v0.4.1"
    if harness.PLATFORM == "windows":
        powershell = harness.which("powershell") or harness.which("pwsh")
        installed = harness.run(
            [powershell, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File",
             harness.ROOT / "install.ps1", "-Version", "0.4.1", "-ReleaseDir", release_dir,
             "-InstallDir", inst_bin, "-NoModifyPath"],
            env=inst_env,
            merge=True,
        )
    else:
        installed = harness.run(
            ["sh", harness.ROOT / "install.sh"],
            env={**inst_env, "HOME": str(user_home), "SHELL": "/bin/bash", "BURDIN_RELEASE_DIR": str(release_dir),
                 "BURDIN_VERSION": "0.4.1", "BURDIN_INSTALL_DIR": str(inst_bin)},
            merge=True,
        )
    install_log = norm(installed.stdout)
    check(installed.returncode == 0, f"the installer failed with the manager in the release:\n{install_log}")
    check("installed toolchain 0.4.1 0.4.1" in install_log, f"the installer did not run the manager:\n{install_log}")
    check(not (inst_bin / "LLVM-C.dll").exists(), "the installer put a runtime library beside the proxy")
    proxy = inst_bin / f"burdin{EXE}"
    expect("the installed proxy", run(["--version"], exe=proxy, env=inst_env), 0,
           fake_line("burdin", "0.4.1", ["--version"], "0.4.1"))
    path_line = f'export PATH="{inst_bin}:$PATH"'
    startup = "" if harness.PLATFORM == "windows" else f"startup line: <work>/userhome/{startup_name}: {norm(path_line.encode())}\n"
    if harness.PLATFORM != "windows":
        check(path_line in harness.read_text(bashrc), "install.sh did not add the PATH line")
    expect(
        "self show",
        run(["toolchain", "self", "show"], exe=proxy, env=inst_env),
        0,
        f"burdin toolchain {own_version}\n"
        "home: <work>/home-inst\n"
        "toolchains: <work>/home-inst/toolchains\n"
        "toolchain 0.4.1 0.4.1: <work>/home-inst/toolchains/0.4.1\n"
        "settings: <work>/home-inst/toolchains/settings.toml\n"
        "downloads: <work>/home-inst/toolchains/downloads\n"
        "receipt: <work>/home-inst/toolchains/receipt\n"
        f"proxy: <work>/inst/bin/burdin{EXE}\n"
        f"proxy: <work>/inst/bin/burdin-lsp{EXE}\n" + startup,
    )
    check(
        "proxy: none recorded" in run(["toolchain", "self", "show"])[1],
        "self show does not say that no proxy is recorded",
    )
    expect(
        "self update without a receipt",
        run(["toolchain", "self", "update"]),
        1,
        "",
        "records no proxy to update",
    )
    set_channel("stable", own_version)
    expect(
        "self update when up to date",
        run(["toolchain", "self", "update"], exe=proxy, env=inst_env),
        0,
        f"burdin toolchain {own_version} is up to date (the latest stable release is {own_version})\n",
    )
    make_release("99.0.0", components=(), manager=FAKE)
    set_channel("stable", "99.0.0")
    expect(
        "self update",
        run(["toolchain", "self", "update"], exe=proxy, env=inst_env),
        0,
        f"updated burdin toolchain {own_version} -> 99.0.0\n",
    )
    check(proxy.read_bytes() == FAKE, "self update did not replace the burdin proxy")
    check((inst_bin / f"burdin-lsp{EXE}").read_bytes() == FAKE, "self update did not replace the burdin-lsp proxy")
    check(not list((inst_home / "toolchains").glob(".staging-*")), "self update left a staging directory behind")

    expect("self record usage", run(["toolchain", "self", "record", "--startup-line"], env=inst_env), 2)
    expect(
        "self install from a directory that is not a release",
        run(["toolchain", "self", "install", "--from", work / "proj", "--bin-dir", work / "nowhere"], env=inst_env),
        1,
        "",
        "is not an unpacked release",
    )

    # Things the manager did not create, which uninstall must leave alone.
    (inst_home / "git").mkdir(parents=True)
    harness.write_text(inst_home / "git" / "cache.txt", "a git checkout\n")
    harness.write_text(inst_home / "toolchains" / "notes.txt", "mine\n")
    harness.write_text(inst_bin / "other.txt", "mine\n")
    expect(
        "self uninstall without --yes",
        run(["toolchain", "self", "uninstall"], env=inst_env),
        1,
        stderr_has="nothing was removed",
    )
    check(proxy.is_file() and (inst_home / "toolchains" / "0.4.1").is_dir(), "self uninstall without --yes removed something")
    code, out, err = run(["toolchain", "self", "uninstall", "--yes"], env=inst_env)
    check(code == 0, f"self uninstall --yes: status {code}\n    stdout: {out!r}\n    stderr: {err!r}")
    for gone in (proxy, inst_bin / f"burdin-lsp{EXE}", inst_home / "toolchains" / "0.4.1",
                 inst_home / "toolchains" / "settings.toml", inst_home / "toolchains" / "receipt"):
        check(not gone.exists(), f"self uninstall left {gone}")
    for kept in (inst_home / "git" / "cache.txt", inst_home / "toolchains" / "notes.txt", inst_bin / "other.txt"):
        check(kept.is_file(), f"self uninstall removed {kept}, which it did not create")
    check(
        "left <work>/home-inst/toolchains/notes.txt (not created by burdin toolchain)" in out,
        f"self uninstall did not report the file it left: {out!r}",
    )
    if harness.PLATFORM != "windows":
        check(
            harness.read_text(bashrc) == "alias ll='ls -l'\n",
            f"self uninstall did not restore {startup_name}: {harness.read_text(bashrc)!r}",
        )
        check("removed the line" in out, f"self uninstall did not report the startup line: {out!r}")

    # A compiler run directly, not through the proxy, says where `toolchain` and `+toolchain` belong (TC9).
    driver = harness.find_burdin()
    if driver is not None and driver.stem == "burdin" and driver.parent.name in ("debug", "release"):
        for args in (["toolchain", "list"], ["+nightly", "build"]):
            result = harness.run([driver, *args], env=env_for())
            check(result.returncode == 2, f"burdin {' '.join(args)} run on the compiler: status {result.returncode}")
            check(
                "belongs to the toolchain manager" in harness.text(result.stderr),
                f"burdin {' '.join(args)} run on the compiler: {harness.text(result.stderr)!r}",
            )

print(f"Checked the toolchain manager ({checks} checks)")
if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
