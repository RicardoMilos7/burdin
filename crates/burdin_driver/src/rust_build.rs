//! The build step behind `[rust.crates]`: a package binds Rust crates through generated C-ABI shims, built with the
//! package (`spec/rust-interop.md`, RI7-RI9).
//!
//! For each crate the manifest names, the step checks the pinned toolchain, generates the Burdin module and the shim
//! crate (when the crate's sources changed), builds the shim as a static library with `cargo`, adds the module to
//! the package like a generated module (BS4), and hands the driver the static library and the system libraries it
//! needs to link. Like a build script it runs native code (the crate's own `build.rs` and procedural macros), so a
//! dependency's crates need the same `[build] allow` grant as a dependency's script (BS2).

use crate::build::{Failure, hash_bytes, merge_generated};
use crate::package::PackageFiles;
use crate::rust_interop::{self, CrateInfo};
use burdin_diag::Code;
use std::path::{Path, PathBuf};

/// The format version of a stored key; a change invalidates every stored key.
const KEY_VERSION: &str = "burdin-rust 1";

/// What the step adds to the build.
#[derive(Default)]
pub(crate) struct Outcome {
    /// The files whose change should rebuild the program (`--emit-deps`).
    pub(crate) inputs: Vec<String>,
    /// The shim archives and the system libraries they need, in link order.
    pub(crate) link_libs: Vec<String>,
}

/// Whether the package binds any Rust crate.
pub(crate) fn has_crates(package: &PackageFiles) -> bool {
    !package.rust.crates.is_empty()
}

fn fail(message: impl Into<String>) -> Failure {
    Failure::new(Code::RUST_INTEROP, message)
}

/// Generates, builds, and registers the package's Rust crates.
pub(crate) fn run_package(package: &mut PackageFiles, profile: &str, fingerprint: &str) -> Result<Outcome, Failure> {
    let dir = package.dir.clone().expect("a package with Rust crates has a directory");
    let toolchain = match package.rust.toolchain.clone() {
        Some(version) if rust_interop::is_exact_version(&version) => version,
        Some(version) => {
            return Err(fail(format!("`[rust] toolchain = \"{version}\"` is not an exact version"))
                .with_note("pin the Rust version exactly, for example `toolchain = \"1.100.0\"`"));
        }
        None => {
            return Err(fail(format!(
                "package `{}` binds Rust crates but does not pin a Rust toolchain",
                package.name
            ))
            .with_note(
                "add `[rust]` with `toolchain = \"1.100.0\"` (an exact version) to the package's `burdin.toml`",
            ));
        }
    };
    let out = dir.join("target").join("burdin").join(profile);
    let rust_src = out.join("rust-src");
    if let Err(e) = std::fs::remove_dir_all(&rust_src)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        return Err(Failure::new(Code::BUILD_OUTPUT, format!("cannot clear {}: {e}", rust_src.display())));
    }
    std::fs::create_dir_all(&rust_src)
        .map_err(|e| Failure::new(Code::BUILD_OUTPUT, format!("cannot create {}: {e}", rust_src.display())))?;

    let mut outcome = Outcome::default();
    // Every crate is found before any tool runs, so a typo in the manifest is reported without waiting for cargo.
    let mut found = Vec::new();
    for (name, relative) in &package.rust.crates {
        let info = rust_interop::locate(&dir.join(relative))
            .map_err(|message| fail(format!("`[rust.crates]` names `{name}` at `{relative}`: {message}")))?;
        found.push((name.clone(), info));
    }
    rust_interop::verify_toolchain(Some(&toolchain)).map_err(fail)?;
    for (name, info) in &found {
        let work = out.join("rust").join(name);
        let built = bind_crate(name, info, &work, &toolchain, profile, fingerprint)?;
        std::fs::write(rust_src.join(format!("{name}.bd")), &built.module)
            .map_err(|e| Failure::new(Code::BUILD_OUTPUT, format!("cannot write the module for `{name}`: {e}")))?;
        for input in built.inputs {
            if !outcome.inputs.contains(&input) {
                outcome.inputs.push(input);
            }
        }
        for lib in std::iter::once(built.archive).chain(built.native_libs) {
            if !outcome.link_libs.contains(&lib) {
                outcome.link_libs.push(lib);
            }
        }
    }
    merge_generated(package, &rust_src)?;
    Ok(outcome)
}

struct Bound {
    module: String,
    inputs: Vec<String>,
    archive: String,
    native_libs: Vec<String>,
}

/// One crate: generate when its sources changed, build always (cargo knows when the shim is fresh).
fn bind_crate(
    name: &str,
    info: &CrateInfo,
    work: &Path,
    toolchain: &str,
    profile: &str,
    fingerprint: &str,
) -> Result<Bound, Failure> {
    let shim_dir = work.join("shim");
    let module_path = work.join("bindings.bd");
    let key_path = work.join("rust.key");
    let sources = crate_sources(&info.dir)
        .map_err(|e| fail(format!("cannot read the sources of the Rust crate `{name}`: {e}")))?;
    let mut key = format!("{KEY_VERSION}\n{fingerprint}\ntoolchain {toolchain}\nprofile {profile}\nname {name}\n");
    key.push_str(&format!("package {}\ndir {}\n", info.package, info.dir.display()));
    for (relative, path) in &sources {
        let bytes = std::fs::read(path).map_err(|e| fail(format!("cannot read {}: {e}", path.display())))?;
        key.push_str(&format!("file {:016x} {relative}\n", hash_bytes(&bytes)));
    }
    let fresh = std::fs::read_to_string(&key_path).is_ok_and(|stored| stored == key)
        && module_path.is_file()
        && shim_dir.join("src").join("lib.rs").is_file();
    let module = if fresh {
        std::fs::read_to_string(&module_path)
            .map_err(|e| fail(format!("cannot read {}: {e}", module_path.display())))?
    } else {
        let _ = std::fs::remove_file(&key_path);
        let json = rust_interop::rustdoc_json(info, Some(toolchain))
            .map_err(|message| fail(format!("cannot read the API of the Rust crate `{name}`: {message}")))?;
        let generated = rust_interop::generate(info, toolchain, &json, &shim_dir)
            .map_err(|message| fail(format!("cannot bind the Rust crate `{name}`: {message}")))?;
        for note in &generated.notes {
            eprintln!("warning: {note}");
        }
        std::fs::create_dir_all(work).map_err(|e| fail(format!("cannot create {}: {e}", work.display())))?;
        rust_interop::write_shim(&shim_dir, &generated).map_err(fail)?;
        std::fs::write(&module_path, &generated.module)
            .map_err(|e| fail(format!("cannot write {}: {e}", module_path.display())))?;
        generated.module
    };
    let shim_package = shim_package_name(&shim_dir)
        .ok_or_else(|| fail(format!("the shim crate in {} has no package name", shim_dir.display())))?;
    let built = rust_interop::build_shim(&shim_dir, &shim_package, profile == "release", toolchain)
        .map_err(|message| fail(format!("the shim for the Rust crate `{name}` did not build: {message}")))?;
    // The key is written once the shim built, so a failed build is generated again next time.
    if !fresh {
        std::fs::write(&key_path, key).map_err(|e| fail(format!("cannot write {}: {e}", key_path.display())))?;
    }
    let inputs = sources.iter().map(|(_, path)| path.display().to_string()).collect();
    Ok(Bound { module, inputs, archive: built.archive.display().to_string(), native_libs: built.native_libs })
}

/// The `[package] name` of the generated shim crate, which names its static library.
fn shim_package_name(shim_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(shim_dir.join("Cargo.toml")).ok()?;
    let line = text.lines().find_map(|line| line.trim().strip_prefix("name = "))?;
    Some(line.trim().trim_matches('"').to_string())
}

/// The files a crate's API can depend on: its manifest, its lockfile, its build script, and every `.rs` file, as
/// `(path relative to the crate, absolute path)` in sorted order. `target` and hidden directories are skipped.
fn crate_sources(dir: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut files = Vec::new();
    walk(dir, dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn walk(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if path.is_dir() {
            if name != "target" && !name.starts_with('.') {
                walk(root, &path, files)?;
            }
        } else if name.ends_with(".rs") || name == "Cargo.toml" || name == "Cargo.lock" {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let shown: Vec<String> =
                relative.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            files.push((shown.join("/"), path));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_sources_skip_target_and_hidden_directories() {
        let base = std::env::temp_dir().join(format!("burdin-rust-sources-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for dir in ["src", "target/debug", ".git", "src/nested"] {
            std::fs::create_dir_all(base.join(dir)).unwrap();
        }
        for file in ["Cargo.toml", "src/lib.rs", "src/nested/a.rs", "target/debug/x.rs", ".git/y.rs", "README.md"] {
            std::fs::write(base.join(file), "").unwrap();
        }
        let sources = crate_sources(&base).unwrap();
        let names: Vec<&str> = sources.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["Cargo.toml", "src/lib.rs", "src/nested/a.rs"]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_shim_package_is_read_from_its_manifest() {
        let base = std::env::temp_dir().join(format!("burdin-rust-shim-name-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("Cargo.toml"), "[package]\nname = \"geometry_burdin_shim\"\n").unwrap();
        assert_eq!(shim_package_name(&base).as_deref(), Some("geometry_burdin_shim"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
