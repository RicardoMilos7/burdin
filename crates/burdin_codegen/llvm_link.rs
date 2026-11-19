// Shared by the build scripts of `burdin_codegen` and `burdin_driver` (`include!`d, so it has no inner attributes):
// where LLVM is installed, how to ask `llvm-config`, and the linker arguments of a static LLVM link.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The LLVM installation directory, from `LLVM_SYS_211_PREFIX` or from the first `llvm-config` on `PATH`.
fn llvm_prefix() -> PathBuf {
    if let Some(prefix) = std::env::var_os("LLVM_SYS_211_PREFIX") {
        return PathBuf::from(prefix);
    }
    let libdir = llvm_config_names()
        .find_map(|name| run(&PathBuf::from(&name), &["--libdir"]).map(PathBuf::from))
        .unwrap_or_else(|| panic!("{}", not_found()));
    libdir.parent().map(Path::to_path_buf).unwrap_or(libdir)
}

fn not_found() -> String {
    "LLVM 21 was not found: set LLVM_SYS_211_PREFIX to its installation directory, or put `llvm-config` on PATH"
        .to_string()
}

/// The names `llvm-config` is installed under, in the order to try them.
fn llvm_config_names() -> impl Iterator<Item = String> {
    ["llvm-config-21", "llvm-config", "llvm-config21", "llvm-config-21.1", "llvm-config21.1"]
        .into_iter()
        .flat_map(|name| [exe(name), name.to_string()])
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

fn run(command: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new(command).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Whether `BURDIN_STATIC_LLVM` asks for the static link.
fn static_llvm_requested() -> bool {
    std::env::var("BURDIN_STATIC_LLVM").is_ok_and(|value| !value.is_empty() && value != "0")
}

/// The linker arguments of a static LLVM link, in link order: every LLVM archive by path, the system libraries
/// LLVM needs, and the C++ runtime. They go on the final link line, after every Rust crate, because a linker only
/// takes archive members for symbols it has already seen: a native library declared by `burdin_codegen` lands before
/// `inkwell` and `llvm-sys`, which call into LLVM, and is not searched for their symbols.
#[allow(dead_code)]
fn static_link_args(prefix: &Path, target_os: &str, target_env: &str) -> Vec<String> {
    if target_os == "windows" && target_env == "msvc" {
        static_link_args_msvc(prefix)
    } else {
        static_link_args_unix(prefix, target_os)
    }
}

/// MSVC: every `LLVM*.lib` in the installation. The archives are built with the static CRT, so the compiler must
/// be too; this refuses the mismatch instead of producing a binary that crashes.
#[allow(dead_code)]
fn static_link_args_msvc(prefix: &Path) -> Vec<String> {
    let features = std::env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();
    if !features.split(',').any(|feature| feature == "crt-static") {
        panic!(
            "BURDIN_STATIC_LLVM on MSVC needs the static CRT: the LLVM archives are built with /MT. \
             Build with RUSTFLAGS=\"-C target-feature=+crt-static\""
        );
    }
    let libdir = prefix.join("lib");
    let mut names: Vec<String> = std::fs::read_dir(&libdir)
        .unwrap_or_else(|error| panic!("cannot read `{}`: {error}", libdir.display()))
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with("LLVM") && name.ends_with(".lib") && name != "LLVM-C.lib")
        .collect();
    if names.is_empty() {
        panic!("`{}` has no LLVM static libraries", libdir.display());
    }
    names.sort();
    let mut args: Vec<String> = names.iter().map(|name| libdir.join(name).display().to_string()).collect();
    // What LLVM's own code calls into on Windows.
    for system in ["ws2_32", "shell32", "ole32", "uuid", "advapi32", "ntdll", "version", "psapi"] {
        args.push(format!("{system}.lib"));
    }
    args
}

/// Linux and macOS: the archives `llvm-config --link-static --libs` names, then its system libraries.
#[allow(dead_code)]
fn static_link_args_unix(prefix: &Path, target_os: &str) -> Vec<String> {
    let config = prefix.join("bin").join(exe("llvm-config"));
    let config = if config.is_file() { config } else { PathBuf::from("llvm-config") };
    let libdir = run(&config, &["--libdir"]).unwrap_or_else(|| panic!("{}", not_found()));
    let libs = run(&config, &["--link-static", "--libs"]).unwrap_or_else(|| {
        panic!("`{} --link-static --libs` failed: LLVM 21 must include its static libraries", config.display())
    });
    let mut args = Vec::new();
    for word in libs.split_whitespace() {
        let path = if let Some(name) = word.strip_prefix("-l") {
            Path::new(&libdir).join(format!("lib{name}.a"))
        } else if word.ends_with(".a") {
            PathBuf::from(word)
        } else {
            continue; // -L<dir> and the like
        };
        if !path.is_file() {
            panic!("`{}` names `{word}`, but `{}` does not exist", config.display(), path.display());
        }
        args.push(path.display().to_string());
    }
    if args.is_empty() {
        panic!("`{} --link-static --libs` named no libraries: {libs}", config.display());
    }
    if let Some(system) = run(&config, &["--link-static", "--system-libs"]) {
        for word in system.split_whitespace() {
            if word == "-lzstd" && target_os == "macos" {
                args.push(macos_zstd_archive());
            } else if word.starts_with("-l") || Path::new(word).is_absolute() {
                args.push(word.to_string());
            }
        }
    }
    args.push(if target_os == "macos" { "-lc++" } else { "-lstdc++" }.to_string());
    args
}

/// macOS: Homebrew's `zstd` lives outside the compiler's default search path, and a dynamic link would make the
/// binary depend on a Homebrew path that other machines lack, so link its static archive.
#[allow(dead_code)]
fn macos_zstd_archive() -> String {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(prefix) = run(Path::new("brew"), &["--prefix", "zstd"]) {
        dirs.push(Path::new(&prefix).join("lib"));
    }
    dirs.push(PathBuf::from("/opt/homebrew/lib"));
    dirs.push(PathBuf::from("/usr/local/lib"));
    match dirs.iter().map(|dir| dir.join("libzstd.a")).find(|archive| archive.is_file()) {
        Some(archive) => archive.display().to_string(),
        None => panic!("cannot find libzstd.a for the static LLVM link: `brew install zstd`"),
    }
}
