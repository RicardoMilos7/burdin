//! Links the compiler against LLVM.
//!
//! `inkwell` is built with `no-llvm-linking`, so `llvm-sys` leaves the link mode to this script. Both platforms link
//! the shared library:
//!
//! * **Windows/MSVC:** the static LLVM archives in the official Windows release are built with the static CRT;
//!   linking them into the Rust binary (which uses the dynamic CRT) makes the compiler crash at run time. `llvm-sys`
//!   also refuses its own dynamic mode on MSVC, so the `LLVM-C.lib` import library that ships beside `LLVM-C.dll`
//!   is used directly. `LLVM-C.dll` must be on `PATH` when the compiler runs.
//! * **Elsewhere:** the shared library named by `llvm-config --link-shared --libs`, as `prefer-dynamic` did.
//!
//! The installation comes from `LLVM_SYS_211_PREFIX` (the variable `llvm-sys` itself reads) or from `llvm-config` on
//! `PATH`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=LLVM_SYS_211_PREFIX");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let prefix = llvm_prefix();
    if target_os == "windows" && target_env == "msvc" {
        link_msvc(&prefix);
    } else {
        link_unix(&prefix);
    }
}

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

fn link_msvc(prefix: &Path) {
    let libdir = prefix.join("lib");
    let import_lib = libdir.join("LLVM-C.lib");
    if !import_lib.is_file() {
        panic!(
            "cannot find `{}`: set LLVM_SYS_211_PREFIX to an LLVM 21 installation that includes the LLVM C API",
            import_lib.display()
        );
    }
    println!("cargo:rustc-link-search=native={}", libdir.display());
    println!("cargo:rustc-link-lib=dylib=LLVM-C");
}

fn link_unix(prefix: &Path) {
    let config = prefix.join("bin").join(exe("llvm-config"));
    let config = if config.is_file() { config } else { PathBuf::from("llvm-config") };
    let libdir = run(&config, &["--libdir"]).unwrap_or_else(|| panic!("{}", not_found()));
    let libs = run(&config, &["--link-shared", "--libs"]).unwrap_or_else(|| {
        panic!("`{} --link-shared --libs` failed: LLVM 21 must include its shared library (libLLVM)", config.display())
    });
    println!("cargo:rustc-link-search=native={libdir}");
    for word in libs.split_whitespace() {
        if let Some(name) = word.strip_prefix("-l") {
            println!("cargo:rustc-link-lib=dylib={name}");
        } else if let Some(dir) = word.strip_prefix("-L") {
            println!("cargo:rustc-link-search=native={dir}");
        }
    }
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
