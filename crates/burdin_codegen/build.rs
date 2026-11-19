//! Links the compiler against LLVM.
//!
//! `inkwell` is built with `no-llvm-linking`, so `llvm-sys` leaves the link mode to this script. By default every
//! platform links the shared library, which keeps development builds fast:
//!
//! * **Windows/MSVC:** the static LLVM archives in the official Windows release are built with the static CRT;
//!   linking them into the Rust binary (which uses the dynamic CRT) makes the compiler crash at run time. `llvm-sys`
//!   also refuses its own dynamic mode on MSVC, so the `LLVM-C.lib` import library that ships beside `LLVM-C.dll`
//!   is used directly. `LLVM-C.dll` must be on `PATH` when the compiler runs.
//! * **Elsewhere:** the shared library named by `llvm-config --link-shared --libs`, as `prefer-dynamic` did.
//!
//! **Static linking.** Setting `BURDIN_STATIC_LLVM=1` links the LLVM static archives into the compiler instead, so the
//! binary needs no LLVM at run time (the release builds do this; docs/releasing.md). That is not done here: this crate
//! sits before `inkwell` and `llvm-sys` on the link line, so a library declared here would be searched before the
//! code that uses it. `crates/burdin_driver/build.rs` passes the archives as link arguments of the `burdin` binary,
//! which come last (the shared helpers are in `llvm_link.rs`). In that mode this script links nothing.
//!
//! The installation comes from `LLVM_SYS_211_PREFIX` (the variable `llvm-sys` itself reads) or from `llvm-config` on
//! `PATH`.

include!("llvm_link.rs");

fn main() {
    println!("cargo:rerun-if-env-changed=LLVM_SYS_211_PREFIX");
    println!("cargo:rerun-if-env-changed=BURDIN_STATIC_LLVM");
    println!("cargo:rerun-if-changed=llvm_link.rs");
    if static_llvm_requested() {
        return;
    }
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let prefix = llvm_prefix();
    if target_os == "windows" && target_env == "msvc" {
        link_msvc(&prefix);
    } else {
        link_unix(&prefix);
    }
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
