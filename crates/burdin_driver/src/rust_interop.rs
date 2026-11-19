//! Rust interop: the tools behind `burdin bindgen --rust` and the `[rust.crates]` build step (`spec/rust-interop.md`).
//!
//! The generator itself is the `burdin_rustgen` crate, which turns rustdoc's JSON into text. This file runs the Rust
//! tools around it: it checks that the toolchain is the pinned one, asks `cargo doc` for the crate's JSON, writes the
//! shim crate, builds it as a static library, and reads the system libraries that library needs from `rustc`.
//! Everything here is a plain function over paths and strings, so the build step (`rust_build.rs`) and the command
//! share it.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A Rust crate on disk.
pub(crate) struct CrateInfo {
    /// The crate's directory, absolute.
    pub(crate) dir: PathBuf,
    /// Its `Cargo.toml`.
    pub(crate) manifest: PathBuf,
    /// The `[package] name`.
    pub(crate) package: String,
}

/// Finds a crate from a directory or a `Cargo.toml` path.
pub(crate) fn locate(path: &Path) -> Result<CrateInfo, String> {
    let manifest = if path.is_dir() { path.join("Cargo.toml") } else { path.to_path_buf() };
    let text = std::fs::read_to_string(&manifest)
        .map_err(|_| format!("`{}` is not a Rust crate: there is no readable Cargo.toml there", path.display()))?;
    let package = package_name(&text)
        .ok_or_else(|| format!("{} has no `[package] name`; name a crate, not a workspace", manifest.display()))?;
    let manifest = manifest.canonicalize().map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
    let manifest = plain_path(&manifest);
    let dir = manifest.parent().expect("a manifest path has a parent").to_path_buf();
    Ok(CrateInfo { dir, manifest, package })
}

/// The `name` of a `Cargo.toml`'s `[package]` table.
fn package_name(text: &str) -> Option<String> {
    let mut in_package = false;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "name"
        {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// Windows paths from `canonicalize` carry a `\\?\` prefix that cargo and relative-path arithmetic do not want.
pub(crate) fn plain_path(path: &Path) -> PathBuf {
    let text = path.display().to_string();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

fn tool(name: &str, toolchain: Option<&str>) -> Command {
    let mut command = Command::new(name);
    if let Some(version) = toolchain {
        // The rustup proxies read this; it wins over a `rust-toolchain.toml` in the crate's directory.
        command.env("RUSTUP_TOOLCHAIN", version);
    }
    command.env("CARGO_TERM_COLOR", "never");
    command
}

/// The version `rustc` reports, with the toolchain pinned when one is given: `rustc 1.100.0 (hash date)` is `1.100.0`.
pub(crate) fn rustc_version(toolchain: Option<&str>) -> Result<String, String> {
    let output = tool("rustc", toolchain).arg("--version").output().map_err(|e| {
        format!("cannot run `rustc`: {e}; install Rust with rustup (https://rustup.rs) to bind a Rust crate")
    })?;
    if !output.status.success() {
        let text = String::from_utf8_lossy(&output.stderr);
        return Err(match toolchain {
            Some(version) => format!(
                "cannot run the pinned Rust toolchain `{version}`: {}\nhelp: install it with `rustup toolchain install {version}`",
                text.trim()
            ),
            None => format!("`rustc --version` failed: {}", text.trim()),
        });
    }
    parse_rustc_version(&String::from_utf8_lossy(&output.stdout))
}

fn parse_rustc_version(text: &str) -> Result<String, String> {
    text.split_whitespace()
        .nth(1)
        .filter(|version| version.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(str::to_string)
        .ok_or_else(|| format!("cannot read a version from `{}`", text.trim()))
}

/// Whether `text` is an exact `major.minor.patch` version.
pub(crate) fn is_exact_version(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 3 && parts.iter().all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// Checks that the toolchain the tools will run is the pinned one, and returns its version. With no pin it is
/// whatever `rustc` is active.
pub(crate) fn verify_toolchain(pinned: Option<&str>) -> Result<String, String> {
    let active = rustc_version(pinned)?;
    match pinned {
        Some(version) if version != active => Err(format!(
            "the Rust toolchain is `{active}`, but `{version}` is pinned; install it with `rustup toolchain install {version}`"
        )),
        _ => Ok(active),
    }
}

/// Runs `cargo doc` for the crate and returns rustdoc's JSON. The JSON output is a nightly feature; the stable
/// toolchain reads it when `RUSTC_BOOTSTRAP` allows it, which is why the version is pinned (RI2).
pub(crate) fn rustdoc_json(info: &CrateInfo, toolchain: Option<&str>) -> Result<String, String> {
    let scratch = crate::TempDir::new().map_err(|e| format!("cannot create a temporary directory: {e}"))?;
    let output = tool("cargo", toolchain)
        .args(["doc", "--no-deps", "--lib", "--manifest-path"])
        .arg(&info.manifest)
        .arg("--target-dir")
        .arg(&scratch.path)
        .env("RUSTC_BOOTSTRAP", "1")
        .env("RUSTDOCFLAGS", "-Z unstable-options --output-format json")
        .output()
        .map_err(|e| format!("cannot run `cargo`: {e}; install Rust with rustup (https://rustup.rs)"))?;
    if !output.status.success() {
        return Err(format!(
            "`cargo doc` failed for {}:\n{}",
            info.manifest.display(),
            String::from_utf8_lossy(&output.stderr).trim_end()
        ));
    }
    let doc = scratch.path.join("doc");
    let json: Vec<PathBuf> = std::fs::read_dir(&doc)
        .map_err(|e| format!("`cargo doc` wrote no output in {}: {e}", doc.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    match json.as_slice() {
        [one] => std::fs::read_to_string(one).map_err(|e| format!("cannot read {}: {e}", one.display())),
        [] => Err(format!(
            "`cargo doc` wrote no rustdoc JSON for {}; does it have a library target?",
            info.manifest.display()
        )),
        _ => Err(format!("`cargo doc` wrote several JSON files for {}", info.manifest.display())),
    }
}

/// Generates the module and the shim for a crate from its rustdoc JSON.
pub(crate) fn generate(
    info: &CrateInfo,
    toolchain: &str,
    json: &str,
    shim_dir: &Path,
) -> Result<burdin_rustgen::Output, String> {
    let dependency_path = relative_path(shim_dir, &info.dir);
    let options =
        burdin_rustgen::Options { dependency_path, package: info.package.clone(), toolchain: toolchain.to_string() };
    burdin_rustgen::generate(json, &options)
}

/// Writes the shim crate's files into `shim_dir`, replacing the three generated ones and leaving its `target/`.
pub(crate) fn write_shim(shim_dir: &Path, output: &burdin_rustgen::Output) -> Result<(), String> {
    let write = |relative: &str, text: &str| -> Result<(), String> {
        let path = shim_dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        // Rewriting an unchanged file would make cargo rebuild the shim for nothing.
        if std::fs::read_to_string(&path).is_ok_and(|old| old == text) {
            return Ok(());
        }
        std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
    };
    write("Cargo.toml", &output.shim_manifest)?;
    write("rust-toolchain.toml", &output.shim_toolchain)?;
    write("src/lib.rs", &output.shim_source)
}

/// `to` as seen from the directory `from`, with `/` separators and `..` where needed, because `Cargo.toml` takes a
/// path relative to the manifest. Two paths with no common root (different drives) give the absolute path.
pub(crate) fn relative_path(from: &Path, to: &Path) -> String {
    let from = absolute(from);
    let to = absolute(to);
    let (from_parts, to_parts): (Vec<_>, Vec<_>) = (from.components().collect(), to.components().collect());
    let common = from_parts.iter().zip(&to_parts).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return to.display().to_string().replace('\\', "/");
    }
    let mut parts: Vec<String> = vec!["..".to_string(); from_parts.len() - common];
    parts.extend(to_parts[common..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()));
    if parts.is_empty() { ".".to_string() } else { parts.join("/") }
}

fn absolute(path: &Path) -> PathBuf {
    let joined =
        if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(path) };
    // Resolve what exists; the rest (a shim directory not made yet) is kept as written.
    let mut existing = joined.clone();
    let mut tail = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => tail.push(name.to_os_string()),
            None => break,
        }
        if !existing.pop() {
            break;
        }
    }
    let mut resolved = existing.canonicalize().map(|p| plain_path(&p)).unwrap_or(existing);
    for name in tail.into_iter().rev() {
        resolved.push(name);
    }
    resolved
}

/// A built shim: its static library and the system libraries that library needs at the final link.
pub(crate) struct BuiltShim {
    pub(crate) archive: PathBuf,
    pub(crate) native_libs: Vec<String>,
}

/// Builds the shim crate as a static library with the pinned toolchain.
pub(crate) fn build_shim(
    shim_dir: &Path,
    shim_package: &str,
    release: bool,
    toolchain: &str,
) -> Result<BuiltShim, String> {
    let target = shim_dir.join("target");
    let mut command = tool("cargo", Some(toolchain));
    command
        .args(["rustc", "--lib", "--manifest-path"])
        .arg(shim_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target);
    if release {
        command.arg("--release");
    }
    command.args(["--", "--print", "native-static-libs"]);
    if cfg!(all(windows, target_env = "msvc")) {
        // Burdin programs link the static C runtime; a Rust library built for the dynamic one would not link.
        command.env("RUSTFLAGS", "-C target-feature=+crt-static");
    }
    let output = command.output().map_err(|e| format!("cannot run `cargo`: {e}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!("building the shim crate in {} failed:\n{}", shim_dir.display(), stderr.trim_end()));
    }
    let profile = if release { "release" } else { "debug" };
    let archive = [format!("lib{shim_package}.a"), format!("{shim_package}.lib")]
        .into_iter()
        .map(|name| target.join(profile).join(name))
        .find(|path| path.is_file())
        .ok_or_else(|| {
            format!("building the shim crate left no static library in {}", target.join(profile).display())
        })?;
    // The package directory can be a verbatim path (`\\?\C:\...`) on Windows, which `link.exe` does not read: it
    // reports the file as `\\name.lib`. The linker is given the plain spelling.
    let archive = plain_path(&archive);
    // When cargo finds the library up to date it does not run rustc, so the note is missing; the last run's answer
    // is kept beside the library.
    let stored = shim_dir.join("native-libs.txt");
    let native_libs = match parse_native_libs(&stderr) {
        Some(libs) => {
            let _ = std::fs::write(&stored, libs.join("\n"));
            libs
        }
        None => match std::fs::read_to_string(&stored) {
            Ok(text) => text.lines().map(str::to_string).filter(|line| !line.is_empty()).collect(),
            Err(_) => default_native_libs(),
        },
    };
    Ok(BuiltShim { archive, native_libs })
}

/// The library names in `rustc --print native-static-libs`' note: `-lgcc_s -lc` is `gcc_s`, `c`; on MSVC the names
/// end in `.lib`. A macOS framework is dropped: the C runtime Burdin links needs none of them.
fn parse_native_libs(stderr: &str) -> Option<Vec<String>> {
    let line = stderr.lines().find(|line| line.contains("native-static-libs:"))?;
    let list = line.split("native-static-libs:").nth(1)?;
    let mut libs: Vec<String> = Vec::new();
    let mut tokens = list.split_whitespace();
    while let Some(token) = tokens.next() {
        let name = if token == "-framework" {
            tokens.next();
            continue;
        } else if let Some(name) = token.strip_prefix("-l") {
            name.rsplit('=').next().unwrap_or(name).to_string()
        } else if let Some(name) = token.strip_suffix(".lib") {
            name.to_string()
        } else {
            continue;
        };
        if !name.is_empty() && !libs.contains(&name) {
            libs.push(name);
        }
    }
    Some(libs)
}

fn default_native_libs() -> Vec<String> {
    let names: &[&str] = if cfg!(windows) {
        &["kernel32", "ntdll", "userenv", "ws2_32", "dbghelp"]
    } else if cfg!(target_os = "macos") {
        &["System", "c", "m"]
    } else {
        &["gcc_s", "util", "rt", "pthread", "m", "dl", "c"]
    };
    names.iter().map(|name| name.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_package_name_is_read_from_the_package_table() {
        let text = "[workspace]\nname = \"no\"\n[package] # c\nname = \"geo-metry\" # the crate\nversion = \"0.1.0\"\n";
        assert_eq!(package_name(text).as_deref(), Some("geo-metry"));
        assert_eq!(package_name("[workspace]\nmembers = []\n"), None);
    }

    #[test]
    fn a_rustc_version_line_is_read() {
        assert_eq!(parse_rustc_version("rustc 1.100.0 (0ef1bc386 2026-11-09)\n").unwrap(), "1.100.0");
        assert!(parse_rustc_version("error").is_err());
    }

    #[test]
    fn a_pinned_version_must_be_exact() {
        assert!(is_exact_version("1.100.0"));
        assert!(!is_exact_version("1.100"));
        assert!(!is_exact_version("stable"));
        assert!(!is_exact_version("1.100.0-nightly"));
        assert!(!is_exact_version("1..0"));
    }

    #[test]
    fn a_verbatim_prefix_is_removed_but_a_unc_share_is_kept() {
        assert_eq!(plain_path(Path::new(r"\\?\C:\work\shim.lib")), PathBuf::from(r"C:\work\shim.lib"));
        assert_eq!(
            plain_path(Path::new(r"\\?\UNC\host\share\shim.lib")),
            PathBuf::from(r"\\?\UNC\host\share\shim.lib")
        );
        assert_eq!(plain_path(Path::new("/tmp/shim.a")), PathBuf::from("/tmp/shim.a"));
    }

    #[test]
    fn native_libraries_are_read_from_rustcs_note() {
        let linux = "note: native-static-libs: -lgcc_s -lutil -lrt -lpthread -lm -ldl -lc\n";
        assert_eq!(parse_native_libs(linux).unwrap(), ["gcc_s", "util", "rt", "pthread", "m", "dl", "c"]);
        let mac = "note: native-static-libs: -framework Foundation -lSystem -lc -lm\n";
        assert_eq!(parse_native_libs(mac).unwrap(), ["System", "c", "m"]);
        let msvc = "note: native-static-libs: kernel32.lib ntdll.lib userenv.lib ws2_32.lib dbghelp.lib msvcrt.lib\n";
        assert_eq!(parse_native_libs(msvc).unwrap(), ["kernel32", "ntdll", "userenv", "ws2_32", "dbghelp", "msvcrt"]);
        assert!(parse_native_libs("Finished `dev` profile\n").is_none());
    }

    #[test]
    fn a_relative_path_climbs_and_descends() {
        let base = std::env::temp_dir().join("burdin-relative-path-test");
        let crate_dir = base.join("crates").join("geometry");
        let shim_dir = base.join("app").join("target").join("shim");
        std::fs::create_dir_all(&crate_dir).unwrap();
        assert_eq!(relative_path(&shim_dir, &crate_dir), "../../../crates/geometry");
        assert_eq!(relative_path(&crate_dir, &crate_dir), ".");
        let _ = std::fs::remove_dir_all(&base);
    }
}
