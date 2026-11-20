//! `burdin toolchain self ...` (`spec/toolchain.md` TC12-TC15): the manager itself. `show` prints where every piece
//! lives, `update` replaces the proxies with a newer release's manager, and `uninstall` removes exactly what the
//! installer and the manager created. `install` and `record` are the installers' bootstrap interface.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::dist;
use crate::home::{RECORD_FILE, ReceiptEntry, io_error, read_receipt, record_in_receipt};
use crate::install::Context;
use crate::proxy::context;
use crate::version::{Channel, EXE, TARGETS, ToolchainName, Version, own_version};
use crate::{Failure, Outcome};

/// The two names the proxy is installed under.
const PROXIES: [&str; 2] = ["burdin", "burdin-lsp"];

pub fn run(args: &[String]) -> Outcome {
    let Some(action) = args.first() else {
        return Err(Failure::usage("`burdin toolchain self` needs show, update, or uninstall"));
    };
    let rest = &args[1..];
    match action.as_str() {
        "show" => no_arguments("show", rest).and_then(|_| show()),
        "update" => no_arguments("update", rest).and_then(|_| update()),
        "uninstall" => match rest {
            [] => uninstall(false),
            [yes] if yes == "--yes" || yes == "-y" => uninstall(true),
            [other, ..] => Err(Failure::usage(format!("unknown argument `{other}` for `self uninstall`"))),
        },
        "install" => bootstrap(rest),
        "record" => record(rest),
        other => Err(Failure::usage(format!("unknown command `self {other}` (show, update, or uninstall)"))),
    }
}

fn no_arguments(command: &str, rest: &[String]) -> Result<(), Failure> {
    match rest.first() {
        Some(extra) => Err(Failure::usage(format!("unexpected argument `{extra}` for `self {command}`"))),
        None => Ok(()),
    }
}

/// The value after `option` in `args`.
fn option_value(args: &[String], option: &str) -> Result<Option<String>, Failure> {
    match args.iter().position(|arg| arg == option) {
        None => Ok(None),
        Some(index) => {
            args.get(index + 1).cloned().map(Some).ok_or_else(|| Failure::usage(format!("{option} needs a value")))
        }
    }
}

fn absolute(path: &Path) -> Result<PathBuf, Failure> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(crate::proxy::current_dir()?.join(path))
}

/// `self install --from <dir> --bin-dir <dir> [--name <toolchain>] [--default]`: the installers' bootstrap (TC13).
/// `<dir>` is an unpacked, verified release archive named `burdin-<version>-<target>`; it becomes the toolchain
/// `<name>` (the version when no name is given), the default with `--default` or when none is set, and this
/// executable is copied into `<bin-dir>` as the `burdin` and `burdin-lsp` proxies, which the receipt records.
fn bootstrap(args: &[String]) -> Outcome {
    for (index, arg) in args.iter().enumerate() {
        let is_value = index > 0 && matches!(args[index - 1].as_str(), "--from" | "--bin-dir" | "--name");
        if !is_value && !matches!(arg.as_str(), "--from" | "--bin-dir" | "--name" | "--default") {
            return Err(Failure::usage(format!("unknown argument `{arg}` for `self install`")));
        }
    }
    let from = option_value(args, "--from")?.ok_or_else(|| Failure::usage("`self install` needs --from <dir>"))?;
    let bin = option_value(args, "--bin-dir")?.ok_or_else(|| Failure::usage("`self install` needs --bin-dir <dir>"))?;
    let from = absolute(Path::new(&from))?;
    let bin = absolute(Path::new(&bin))?;
    let ctx = context()?;
    let dir_name = from.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    let (version, target) = parse_release_dir(&dir_name).ok_or_else(|| {
        Failure::from(format!("{} is not an unpacked release (burdin-<version>-<target>)", from.display()))
    })?;
    if target != ctx.target {
        return Err(Failure::from(format!("the release is for {target}, but this platform is {}", ctx.target)));
    }
    let name = match option_value(args, "--name")? {
        Some(name) => ToolchainName::parse(&name).map_err(Failure::usage)?,
        None => ToolchainName::Exact(version.clone()),
    };
    ctx.install_from_dir(&name, &version, &from)?;
    println!("installed toolchain {name} {version} in {}", ctx.home.toolchain_dir(&name).display());
    let make_default = args.iter().any(|arg| arg == "--default");
    if make_default || ctx.home.default_toolchain()?.is_none() {
        ctx.home.set_default(Some(&name))?;
        println!("default toolchain: {name}");
    }
    let me = std::env::current_exe().map_err(|e| Failure::from(format!("cannot find this executable: {e}")))?;
    fs::create_dir_all(&bin).map_err(|e| io_error("create", &bin, e))?;
    for proxy in PROXIES {
        let path = bin.join(format!("{proxy}{EXE}"));
        install_proxy(&me, &path)?;
        record_in_receipt(&ctx.home, ReceiptEntry::Proxy(path.clone()))?;
        println!("installed the proxy {}", path.display());
    }
    Ok(0)
}

/// `burdin-<version>-<target>` → the version and the target.
fn parse_release_dir(name: &str) -> Option<(Version, &'static str)> {
    let rest = name.strip_prefix("burdin-")?;
    TARGETS.iter().find_map(|target| {
        let version = rest.strip_suffix(target)?.strip_suffix('-')?;
        Some((Version::parse(version)?, *target))
    })
}

/// `self record --startup-line <file> <line>` or `self record --user-path <dir>`: the installers note a PATH change
/// they made, so `self uninstall` can undo exactly that change (TC14).
fn record(args: &[String]) -> Outcome {
    let entry = match args {
        [flag, file, line] if flag == "--startup-line" => {
            ReceiptEntry::StartupLine(absolute(Path::new(file))?, line.clone())
        }
        [flag, dir] if flag == "--user-path" => ReceiptEntry::UserPath(dir.clone()),
        _ => return Err(Failure::usage("usage: self record --startup-line <file> <line> | --user-path <dir>")),
    };
    let ctx = context()?;
    record_in_receipt(&ctx.home, entry)?;
    Ok(0)
}

/// Copies `source` to `dest` through a temporary file. On Windows the running executable cannot be overwritten, so
/// the old file is renamed to `<dest>.old` first; the next run of the proxy deletes it.
fn install_proxy(source: &Path, dest: &Path) -> Result<(), Failure> {
    if dest.exists() && same_file(source, dest) {
        return Ok(());
    }
    let tmp = suffixed(dest, ".new");
    fs::copy(source, &tmp).map_err(|e| io_error("write", &tmp, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).map_err(|e| io_error("write", &tmp, e))?;
    }
    if cfg!(windows) && dest.exists() {
        let old = suffixed(dest, ".old");
        let _ = fs::remove_file(&old);
        fs::rename(dest, &old).map_err(|e| io_error("replace", dest, e))?;
    }
    fs::rename(&tmp, dest).map_err(|e| Failure::from(io_error("install", dest, e)))
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut text = path.as_os_str().to_os_string();
    text.push(suffix);
    PathBuf::from(text)
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

fn show() -> Outcome {
    let ctx = context()?;
    let home = &ctx.home;
    println!("burdin toolchain {}", own_version());
    println!("home: {}", home.root.display());
    println!("toolchains: {}", home.toolchains().display());
    for (name, record) in home.installed() {
        println!("toolchain {name} {}: {}", record.version, home.toolchain_dir(&name).display());
    }
    println!("settings: {}", home.settings().display());
    println!("downloads: {}", home.downloads().display());
    println!("receipt: {}", home.receipt().display());
    let receipt = read_receipt(home)?;
    if !receipt.iter().any(|entry| matches!(entry, ReceiptEntry::Proxy(_))) {
        println!("proxy: none recorded (not installed by install.sh or install.ps1)");
    }
    for entry in &receipt {
        match entry {
            ReceiptEntry::Proxy(path) => println!("proxy: {}", path.display()),
            ReceiptEntry::StartupLine(file, line) => println!("startup line: {}: {line}", file.display()),
            ReceiptEntry::UserPath(dir) => println!("user PATH entry: {dir}"),
        }
    }
    Ok(0)
}

fn update() -> Outcome {
    let ctx = context()?;
    let proxies: Vec<PathBuf> = read_receipt(&ctx.home)?
        .into_iter()
        .filter_map(|entry| if let ReceiptEntry::Proxy(path) = entry { Some(path) } else { None })
        .collect();
    if proxies.is_empty() {
        return Err(Failure::from(format!(
            "{} records no proxy to update; install burdin with install.sh or install.ps1 first",
            ctx.home.receipt().display()
        )));
    }
    let own = Version::parse(own_version()).ok_or("this manager's own version is not a version")?;
    let latest = ctx.source.latest(Channel::Stable)?;
    if latest <= own {
        println!("burdin toolchain {own} is up to date (the latest stable release is {latest})");
        return Ok(0);
    }
    let manager = new_manager(&ctx, &latest)?;
    let result = (|| {
        for proxy in &proxies {
            install_proxy(&manager, proxy)?;
        }
        Ok::<(), Failure>(())
    })();
    if let Some(staging) = manager.parent().and_then(Path::parent) {
        let _ = fs::remove_dir_all(staging);
    }
    result?;
    println!("updated burdin toolchain {own} -> {latest}");
    Ok(0)
}

/// Downloads and verifies the release `version`'s archive and returns its unpacked `burdin-toolchain`.
fn new_manager(ctx: &Context, version: &Version) -> Result<PathBuf, Failure> {
    let archive = format!("burdin-{version}-{}.tar.gz", ctx.target);
    let expected = ctx
        .source
        .expected_sha256(version, &archive)?
        .ok_or_else(|| Failure::from(format!("release {version} has no archive for {}", ctx.target)))?;
    let file = dist::fetch_verified(&ctx.source, version, &archive, &expected, &ctx.home.downloads())?;
    let staging = ctx.home.toolchains().join(format!(".staging-self-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    let extracted = dist::extract(&file, &staging);
    let _ = fs::remove_file(&file);
    extracted?;
    let manager = staging.join(format!("burdin-{version}-{}", ctx.target)).join(format!("burdin-toolchain{EXE}"));
    if !manager.is_file() {
        let _ = fs::remove_dir_all(&staging);
        return Err(Failure::from(format!("release {version} does not contain burdin-toolchain{EXE}")));
    }
    Ok(manager)
}

/// What `self uninstall` does with one thing.
enum Step {
    RemoveFile(PathBuf),
    RemoveDir(PathBuf),
    RemoveLine(PathBuf, String),
    RemoveUserPath(String),
    Leave(PathBuf),
}

fn uninstall(yes: bool) -> Outcome {
    let ctx = context()?;
    let home = &ctx.home;
    let receipt = read_receipt(home)?;
    let mut steps = Vec::new();
    for entry in &receipt {
        match entry {
            ReceiptEntry::Proxy(path) => {
                for candidate in [path.clone(), suffixed(path, ".old"), suffixed(path, ".new")] {
                    if candidate.exists() {
                        steps.push(Step::RemoveFile(candidate));
                    }
                }
            }
            ReceiptEntry::StartupLine(file, line) => steps.push(Step::RemoveLine(file.clone(), line.clone())),
            ReceiptEntry::UserPath(dir) => steps.push(Step::RemoveUserPath(dir.clone())),
        }
    }
    let toolchains = home.toolchains();
    let mut entries: Vec<PathBuf> = match fs::read_dir(&toolchains) {
        Ok(read) => read.flatten().map(|entry| entry.path()).collect(),
        Err(_) => Vec::new(),
    };
    entries.sort();
    let mut receipt_file = None;
    for path in entries {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let ours_dir = path.is_dir()
            && (name == "downloads"
                || name.starts_with(".staging-")
                || name.starts_with(".old-")
                || path.join(RECORD_FILE).is_file());
        if ours_dir {
            steps.push(Step::RemoveDir(path));
        } else if name == "settings.toml" {
            steps.push(Step::RemoveFile(path));
        } else if name == "receipt" {
            receipt_file = Some(path);
        } else {
            steps.push(Step::Leave(path));
        }
    }
    if let Some(path) = receipt_file {
        steps.push(Step::RemoveFile(path));
    }

    if !yes {
        for step in &steps {
            match step {
                Step::RemoveFile(path) | Step::RemoveDir(path) => println!("would remove {}", path.display()),
                Step::RemoveLine(file, line) => println!("would remove the line `{line}` from {}", file.display()),
                Step::RemoveUserPath(dir) => println!("would remove {dir} from the user PATH"),
                Step::Leave(path) => println!("would leave {} (not created by burdin toolchain)", path.display()),
            }
        }
        return Err(Failure::from("nothing was removed; run `burdin toolchain self uninstall --yes` to remove these"));
    }

    let me = std::env::current_exe().ok();
    let mut failed = false;
    for step in &steps {
        let result = match step {
            Step::RemoveFile(path) => remove_file(path, me.as_deref()).map(|_| format!("removed {}", path.display())),
            Step::RemoveDir(path) => fs::remove_dir_all(path)
                .map_err(|e| io_error("remove", path, e))
                .map(|_| format!("removed {}", path.display())),
            Step::RemoveLine(file, line) => remove_line(file, line).map(|removed| {
                if removed {
                    format!("removed the line `{line}` from {}", file.display())
                } else {
                    format!("left {}: it no longer has the line `{line}`", file.display())
                }
            }),
            Step::RemoveUserPath(dir) => remove_user_path(dir).map(|removed| {
                if removed {
                    format!("removed {dir} from the user PATH")
                } else {
                    format!("left the user PATH: it no longer has {dir}")
                }
            }),
            Step::Leave(path) => Ok(format!("left {} (not created by burdin toolchain)", path.display())),
        };
        match result {
            Ok(message) => println!("{message}"),
            Err(message) => {
                eprintln!("error: {message}");
                failed = true;
            }
        }
    }
    // The directory goes too when nothing else is left in it.
    if fs::remove_dir(&toolchains).is_ok() {
        println!("removed {}", toolchains.display());
    }
    if failed {
        return Err(Failure::from("burdin toolchain was not removed completely"));
    }
    println!("uninstalled burdin toolchain");
    Ok(0)
}

/// Removes `path`. The running proxy cannot delete itself on Windows, so there a short-lived `cmd` deletes it once
/// this process has exited.
fn remove_file(path: &Path, me: Option<&Path>) -> Result<(), String> {
    if cfg!(windows) && me.is_some_and(|me| same_file(me, path)) {
        let script = format!("ping -n 3 127.0.0.1 >NUL & del /F /Q \"{}\"", path.display());
        return Command::new("cmd")
            .args(["/C", &script])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("cannot schedule the removal of {}: {e}", path.display()));
    }
    fs::remove_file(path).map_err(|e| io_error("remove", path, e))
}

/// Removes every line equal to `line` from `file`, with the blank line the installer wrote before it. Returns whether
/// anything was removed.
fn remove_line(file: &Path, line: &str) -> Result<bool, String> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(io_error("read", file, e)),
    };
    match without_line(&text, line) {
        Some(updated) => crate::home::write_file(file, &updated).map(|_| true),
        None => Ok(false),
    }
}

/// `text` without the lines equal to `line` (and an empty line right before each), or `None` when it has none.
pub fn without_line(text: &str, line: &str) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut removed = false;
    for current in lines {
        if current.trim_end_matches('\r') == line {
            removed = true;
            if kept.last() == Some(&"") && kept.len() > 1 {
                kept.pop();
            }
            continue;
        }
        kept.push(current);
    }
    removed.then(|| kept.join("\n"))
}

/// The user `PATH` (raw, unexpanded) without the entries naming `dir`, or `None` when it has none.
pub fn without_path_entry(path: &str, dir: &str) -> Option<String> {
    let wanted = dir.trim_end_matches('\\');
    let entries: Vec<&str> = path.split(';').filter(|entry| !entry.is_empty()).collect();
    let kept: Vec<&str> =
        entries.iter().copied().filter(|entry| !entry.trim_end_matches('\\').eq_ignore_ascii_case(wanted)).collect();
    (kept.len() != entries.len()).then(|| kept.join(";"))
}

/// Removes `dir` from the user's `PATH` in `HKCU\Environment`, reading and writing the raw registry string the way
/// `install.ps1` does. Only Windows has a user `PATH`.
fn remove_user_path(dir: &str) -> Result<bool, String> {
    if !cfg!(windows) {
        return Err(format!("cannot remove {dir} from the user PATH: only Windows has one"));
    }
    const KEY: &str = "[Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)";
    let read = format!(
        "$k = {KEY}; [Console]::Out.Write([string]$k.GetValue('Path', '', \
         [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)); $k.Close()"
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &read])
        .output()
        .map_err(|e| format!("cannot run powershell to read the user PATH: {e}"))?;
    if !output.status.success() {
        return Err(format!("cannot read the user PATH: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let current = String::from_utf8_lossy(&output.stdout).trim_end_matches(['\r', '\n']).to_string();
    let Some(updated) = without_path_entry(&current, dir) else { return Ok(false) };
    let write = format!(
        "$k = {KEY}; $k.SetValue('Path', $env:BURDIN_UNINSTALL_PATH, \
         [Microsoft.Win32.RegistryValueKind]::ExpandString); $k.Close()"
    );
    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &write])
        .env("BURDIN_UNINSTALL_PATH", &updated)
        .status()
        .map_err(|e| format!("cannot run powershell to write the user PATH: {e}"))?;
    if !status.success() {
        return Err("cannot write the user PATH".into());
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_directories() {
        let (version, target) = parse_release_dir("burdin-0.5.0-nightly.20261118-aarch64-apple-darwin").unwrap();
        assert_eq!(version.to_string(), "0.5.0-nightly.20261118");
        assert_eq!(target, "aarch64-apple-darwin");
        assert!(parse_release_dir("burdin-0.4.1-riscv64gc-unknown-linux-gnu").is_none());
        assert!(parse_release_dir("burdin-lsp-0.4.1-x86_64-unknown-linux-gnu").is_none());
    }

    #[test]
    fn removes_the_startup_line_and_the_blank_line_before_it() {
        let line = "export PATH=\"/b:$PATH\"";
        let text = format!("alias ll='ls -l'\n\n{line}\n");
        assert_eq!(without_line(&text, line).unwrap(), "alias ll='ls -l'\n");
        assert_eq!(without_line("a\nb\n", line), None);
        assert_eq!(without_line(&format!("{line}\n"), line).unwrap(), "");
    }

    #[test]
    fn removes_a_user_path_entry() {
        let path = r"%USERPROFILE%\bin;C:\Users\u\AppData\Local\Programs\burdin\bin\;C:\Tools";
        assert_eq!(
            without_path_entry(path, r"c:\users\u\appdata\local\programs\burdin\bin").unwrap(),
            r"%USERPROFILE%\bin;C:\Tools"
        );
        assert_eq!(without_path_entry(path, r"C:\Other"), None);
    }
}
