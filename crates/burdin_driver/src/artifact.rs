//! The artifact cache of a package's target directory (`spec/target.md`, TG1-TG10).
//!
//! `build`, `run`, and `test` of a package directory keep the executable they link under
//! `<package>/target/burdin/<profile>/<kind>/`, next to a fingerprint file that records every input the program was
//! built from and the hash of the executable. A later build whose inputs match, and whose executable still has the
//! recorded hash, copies that executable instead of running code generation and the linker again. The front end always
//! runs first, so diagnostics, build scripts, and compile-time evaluation happen exactly as they do without a cache.

use crate::build::compiler_fingerprint;
use crate::{CompileOptions, EntryMode, package::Workspace};
use burdin_diag::SourceMap;
use std::path::{Path, PathBuf};

/// The first line of every fingerprint file (TG3). A change to the format changes this line, so every older
/// fingerprint mismatches and its artifact is built again.
const FORMAT: &str = "burdin-artifact-v2";

/// The directory kind of a cacheable build, or `None` when the build is not cached (TG2). Only what `build`, `run`, and
/// `test` produce for a package directory is cached. A debug build writes sidecar files beside its executable (the PDB,
/// the Natvis file, the dSYM bundle), which a cached copy would leave behind. A build that writes `--emit` artifacts or
/// uses a PGO profile has outputs of its own. A bench or fuzz build is not a program the user runs again.
pub(crate) fn cached_kind(
    mode: EntryMode,
    debug: bool,
    emits: bool,
    pgo_use: bool,
    package: bool,
) -> Option<&'static str> {
    if debug || emits || pgo_use || !package {
        return None;
    }
    match mode {
        EntryMode::Build => Some("bin"),
        EntryMode::Test => Some("test"),
        EntryMode::Bench | EntryMode::Fuzz => None,
    }
}

/// One cacheable build: where its executable is kept and the fingerprint it must match.
pub(crate) struct Plan {
    /// The cached executable, `<dir>/<name><EXE_SUFFIX>`.
    pub(crate) exe: PathBuf,
    dir: PathBuf,
    /// Where the codegen units' objects are kept (TG11): `<profile>/objects/<kind>/`, beside the executable's directory.
    pub(crate) objects: PathBuf,
    name: String,
    /// Every input the program is built from, one per line, in a fixed order (TG3). A fingerprint file holds exactly
    /// this text followed by one `artifact` line.
    body: String,
}

impl Plan {
    /// A plan that keeps an executable named `name` in `dir`, with the given fingerprint body.
    pub(crate) fn new(dir: PathBuf, name: &str, body: String) -> Plan {
        let exe = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        let objects = match dir.parent() {
            Some(profile) => profile.join("objects").join(dir.file_name().unwrap_or_default()),
            None => dir.join("objects"),
        };
        Plan { exe, dir, objects, name: name.to_string(), body }
    }

    /// The plan for a build of the entry package, or `None` when the build is not cached (TG2). The inputs are the
    /// checked program's: the files the front end read, its build inputs, and the Rust shim libraries it links.
    pub(crate) fn for_build(
        loaded: &Workspace,
        files: &[(PathBuf, bool)],
        sources: &SourceMap,
        build_inputs: &[String],
        rust_libs: &[String],
        options: &CompileOptions,
    ) -> Option<Plan> {
        // A build for another platform is not cached: its executable cannot be told from the host's by the
        // fingerprint's inputs, and the object cache (keyed by the triple) already saves the codegen
        // (`spec/cross-compilation.md`, XC9).
        if burdin_tir::target::is_cross() {
            return None;
        }
        let package_dir = loaded.packages.get(loaded.entry_package)?.dir.as_deref();
        let emits = options.emit.is_some() || options.emit_ir.is_some();
        let kind = cached_kind(
            options.mode,
            options.debug || options.profile.debug,
            emits,
            options.pgo_use.is_some(),
            package_dir.is_some(),
        )?;
        let package_dir = package_dir?.canonicalize().ok()?;
        let dir =
            burdin_query::workspace::target_dir(&package_dir).join("burdin").join(&options.profile.dir).join(kind);
        let body = fingerprint_body(loaded, files, sources, build_inputs, rust_libs, options, &package_dir, kind);
        Some(Plan::new(dir, &loaded.stem, body))
    }

    /// Whether the cached executable was built from exactly this plan's inputs and is still intact (TG3, TG7): the
    /// fingerprint file holds this body and an `artifact` line with the executable's SHA-256.
    pub(crate) fn is_fresh(&self) -> bool {
        let Ok(text) = std::fs::read_to_string(self.fingerprint_path()) else {
            return false;
        };
        let Some(rest) = text.strip_prefix(self.body.as_str()) else {
            return false;
        };
        let Some(recorded) = rest.strip_prefix("artifact ").and_then(|hash| hash.strip_suffix('\n')) else {
            return false;
        };
        match std::fs::read(&self.exe) {
            Ok(bytes) => recorded == burdin_sha256::hex(&bytes),
            Err(_) => false,
        }
    }

    /// Where the linker writes a build before it is published (TG5), in the cache directory, which is created here. The
    /// process id keeps two builds apart. An error means the cache cannot be written at all.
    pub(crate) fn staging(&self) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(&self.dir)?;
        Ok(self.dir.join(format!("{}.staging-{}{}", self.name, std::process::id(), std::env::consts::EXE_SUFFIX)))
    }

    /// Publishes the staged executable as the cached one and records its fingerprint (TG5). The old fingerprint goes
    /// first, so it never vouches for the new executable. An error means the staged file was not moved: the caller
    /// uses it directly. A fingerprint that cannot be written is not an error; the next build simply links again.
    pub(crate) fn publish(&self, staged: &Path) -> std::io::Result<()> {
        let _ = std::fs::remove_file(self.fingerprint_path());
        std::fs::rename(staged, &self.exe)?;
        let Ok(bytes) = std::fs::read(&self.exe) else {
            return Ok(());
        };
        let text = format!("{}artifact {}\n", self.body, burdin_sha256::hex(&bytes));
        let temp = self.dir.join(format!("{}.fingerprint.tmp-{}", self.name, std::process::id()));
        let written = std::fs::write(&temp, text).and_then(|()| std::fs::rename(&temp, self.fingerprint_path()));
        if written.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        Ok(())
    }

    fn fingerprint_path(&self) -> PathBuf {
        self.dir.join(format!("{}.fingerprint", self.name))
    }
}

/// The fingerprint body of one build (TG3): the format, the compiler's identity, the host, the profile and kind, the
/// executable's name, the flags that change code or linking, then every input in a fixed order. Each file is hashed
/// and named relative to the entry package when it is inside it.
#[allow(clippy::too_many_arguments)]
fn fingerprint_body(
    loaded: &Workspace,
    files: &[(PathBuf, bool)],
    sources: &SourceMap,
    build_inputs: &[String],
    rust_libs: &[String],
    options: &CompileOptions,
    package_dir: &Path,
    kind: &str,
) -> String {
    let mut lines = vec![
        FORMAT.to_string(),
        format!("compiler {}", compiler_fingerprint()),
        format!("host {} {}", std::env::consts::OS, std::env::consts::ARCH),
        format!("profile {}", options.profile.dir),
        format!("kind {kind}"),
        format!("name {}", loaded.stem),
        format!("settings {}", options.profile.key()),
        format!(
            "options strip={} lto={} pgo-generate={} sanitize={}",
            options.strip,
            options.lto,
            options.pgo_generate,
            sanitizer_names(options.sanitize)
        ),
    ];
    for over in &options.profile.packages {
        lines.push(format!(
            "package {} overflow-checks={:?} debug-assertions={:?}",
            over.package, over.overflow_checks, over.debug_assertions
        ));
    }
    lines.extend(options.link_libs.iter().map(|lib| format!("link-lib {lib}")));
    for package in &loaded.packages {
        if let Some(dir) = &package.dir {
            let manifest = dir.join("burdin.toml");
            lines.push(format!("manifest {} {}", digest(&manifest), relative(package_dir, &manifest)));
        }
    }
    for (id, file) in sources.iter() {
        // The embedded standard library has no file on disk; the compiler's identity covers it. Every other
        // in-memory module is hashed like a file it was read from: the generated `features` module changes with the
        // features a build enables, and that must change the fingerprint (`spec/packages.md`, PKG32).
        if files.get(id.0 as usize).is_some_and(|(path, embedded)| *embedded && path.starts_with("<std>")) {
            continue;
        }
        let hash = burdin_sha256::hex(file.text.as_bytes());
        lines.push(format!("source {hash} {}", relative(package_dir, Path::new(&file.path))));
    }
    for input in build_inputs {
        let path = Path::new(input);
        lines.push(format!("input {} {}", digest(path), relative(package_dir, path)));
    }
    for lib in rust_libs {
        // A system library is a name, not a file: it is recorded as missing and its name is the input.
        lines.push(format!("rust-lib {} {lib}", digest(Path::new(lib))));
    }
    let mut body = lines.join("\n");
    body.push('\n');
    body
}

/// The sanitizers a build instruments and links, comma-separated, or `none` (`spec/sanitizers.md`).
fn sanitizer_names(sanitize: burdin_codegen::Sanitizers) -> String {
    let enabled: Vec<&str> = [
        (sanitize.address, "address"),
        (sanitize.thread, "thread"),
        (sanitize.leak, "leak"),
        (sanitize.undefined, "undefined"),
        (sanitize.fuzzer, "fuzzer"),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, name)| name)
    .collect();
    if enabled.is_empty() { "none".to_string() } else { enabled.join(",") }
}

/// The SHA-256 of a file's bytes, or `missing` when it cannot be read.
fn digest(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => burdin_sha256::hex(&bytes),
        Err(_) => "missing".to_string(),
    }
}

/// The path as the fingerprint names it: relative to the entry package when inside it, otherwise absolute. The file
/// is resolved first, so two spellings of one path name it the same way.
fn relative(package_dir: &Path, path: &Path) -> String {
    let full = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    match full.strip_prefix(package_dir) {
        Ok(inside) => inside.display().to_string(),
        Err(_) => full.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory that is removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let nanos =
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            let dir = std::env::temp_dir().join(format!("burdin-artifact-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("a scratch directory");
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_build_and_test_of_a_package_are_cached() {
        assert_eq!(cached_kind(EntryMode::Build, false, false, false, true), Some("bin"));
        assert_eq!(cached_kind(EntryMode::Test, false, false, false, true), Some("test"));
        assert_eq!(cached_kind(EntryMode::Bench, false, false, false, true), None, "bench builds are not cached");
        assert_eq!(cached_kind(EntryMode::Fuzz, false, false, false, true), None, "fuzz builds are not cached");
        assert_eq!(cached_kind(EntryMode::Build, true, false, false, true), None, "debug builds write sidecars");
        assert_eq!(cached_kind(EntryMode::Build, false, true, false, true), None, "--emit writes its own files");
        assert_eq!(cached_kind(EntryMode::Build, false, false, true, true), None, "a PGO use build reads a profile");
        assert_eq!(cached_kind(EntryMode::Build, false, false, false, false), None, "a one-file target has no target");
    }

    #[test]
    fn publishing_moves_the_staged_program_and_records_it() {
        let scratch = Scratch::new("publish");
        let plan = Plan::new(scratch.0.join("bin"), "app", "compiler x\n".to_string());
        assert!(!plan.is_fresh(), "nothing is cached before the first build");
        let staged = plan.staging().expect("the cache directory is created");
        std::fs::write(&staged, b"program bytes").unwrap();
        plan.publish(&staged).expect("the staged program moves into place");
        assert!(!staged.exists(), "publishing moves the staged file, it does not copy it");
        assert!(plan.exe.is_file());
        assert!(plan.is_fresh(), "the fingerprint describes the executable it was written with");
        let recorded = std::fs::read_to_string(plan.fingerprint_path()).unwrap();
        assert_eq!(recorded, format!("compiler x\nartifact {}\n", burdin_sha256::hex(b"program bytes")));
    }

    #[test]
    fn a_changed_executable_or_fingerprint_is_not_fresh() {
        let scratch = Scratch::new("fresh");
        let plan = Plan::new(scratch.0.join("bin"), "app", "compiler x\n".to_string());
        let staged = plan.staging().unwrap();
        std::fs::write(&staged, b"program bytes").unwrap();
        plan.publish(&staged).unwrap();

        std::fs::write(&plan.exe, b"truncated").unwrap();
        assert!(!plan.is_fresh(), "an executable whose hash changed must be built again");
        std::fs::write(&plan.exe, b"program bytes").unwrap();
        assert!(plan.is_fresh(), "restoring the bytes restores the hit");

        let other_inputs = Plan::new(scratch.0.join("bin"), "app", "compiler y\n".to_string());
        assert!(!other_inputs.is_fresh(), "a different body is a different build");

        std::fs::remove_file(plan.fingerprint_path()).unwrap();
        assert!(!plan.is_fresh(), "an executable without its fingerprint is not trusted");
    }

    #[test]
    fn a_failed_move_publishes_nothing() {
        let scratch = Scratch::new("failed-move");
        let plan = Plan::new(scratch.0.join("bin"), "app", "compiler x\n".to_string());
        assert!(plan.publish(&scratch.0.join("never-staged")).is_err());
        assert!(!plan.fingerprint_path().exists(), "no fingerprint is written for a program that was not moved");
        assert!(!plan.is_fresh());
    }
}
