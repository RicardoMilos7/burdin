//! Build scripts: `build.bd` runs before compilation and may generate modules (`spec/build-scripts.md`).
//!
//! A package's own script is trusted. A dependency's script runs only when the entry package's manifest names it in
//! `[build] allow` (BS2). The script is an ordinary Burdin program, compiled in the build's profile and run with the
//! package directory and output directory as arguments (BS3). Generated `.bd` files under `<out>/src/` become
//! modules of the package (BS4), and the script runs again only when its own source, the manifest, or one of its
//! declared inputs changed, keyed per package and profile (BS5).

use crate::package::{self, PackageFiles, Workspace};
use crate::{CompileOptions, EntryMode};
use burdin_diag::Code;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The name of a package's build script, next to its `burdin.toml`.
const BUILD_SCRIPT: &str = "build.bd";
/// Where the script writes the list of files it read besides its own source.
const INPUTS_FILE: &str = "inputs";
/// The format version of a cache key; a change invalidates every stored key.
const KEY_VERSION: &str = "burdin-build 1";

/// Why the build stopped before compilation. The driver reports it at the entry package's first source line, so a
/// `compile-fail` test can name the code (`spec/build-scripts.md`, BS6).
#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) code: Code,
    pub(crate) message: String,
    pub(crate) note: Option<String>,
}

impl Failure {
    pub(crate) fn new(code: Code, message: impl Into<String>) -> Failure {
        Failure { code, message: message.into(), note: None }
    }

    pub(crate) fn with_note(mut self, note: impl Into<String>) -> Failure {
        self.note = Some(note.into());
        self
    }
}

/// What running the build steps added to the build.
#[derive(Default)]
pub(crate) struct Outcome {
    /// Every input the scripts declared (including the files their `embed` calls read) and every source of a bound
    /// Rust crate, in first-seen order.
    pub(crate) inputs: Vec<String>,
    /// The static libraries and system libraries of the bound Rust crates' shims, which the program links.
    pub(crate) link_libs: Vec<String>,
}

/// Binds the Rust crates of the loaded packages (`spec/rust-interop.md`), runs their build scripts, then adds each
/// package's generated modules.
pub(crate) fn run(
    workspace: &mut Workspace,
    profile: crate::profile::Profile,
    options: &CompileOptions,
) -> Result<Outcome, Failure> {
    check_grants(workspace)?;
    let fingerprint = compiler_fingerprint();
    let mut outcome = Outcome::default();
    // Dependencies first, like a package manager building a graph: a package's generated modules are ready before
    // the package that depends on it is loaded. The steps are independent either way.
    for index in (0..workspace.packages.len()).rev() {
        let package = &mut workspace.packages[index];
        if crate::rust_build::has_crates(package) {
            let bound = crate::rust_build::run_package(package, &profile.dir, &fingerprint)?;
            for input in bound.inputs {
                if !outcome.inputs.contains(&input) {
                    outcome.inputs.push(input);
                }
            }
            for lib in bound.link_libs {
                if !outcome.link_libs.contains(&lib) {
                    outcome.link_libs.push(lib);
                }
            }
        }
        if !has_script(package) {
            continue;
        }
        let declared = run_package(package, &profile, &fingerprint, options)?;
        for input in declared {
            if !outcome.inputs.contains(&input) {
                outcome.inputs.push(input);
            }
        }
    }
    Ok(outcome)
}

/// Whether the package has a `build.bd` next to its manifest. A one-file target and the embedded standard
/// library have no package directory and therefore no script.
fn has_script(package: &PackageFiles) -> bool {
    package.dir.as_ref().is_some_and(|dir| dir.join(BUILD_SCRIPT).is_file())
}

/// Applies the trust rule (BS2): the entry package's own script is trusted; every other loaded package with a
/// script must be named in the entry package's `[build] allow` list.
fn check_grants(workspace: &Workspace) -> Result<(), Failure> {
    let entry = &workspace.packages[workspace.entry_package];
    for (index, package) in workspace.packages.iter().enumerate() {
        let has_crates = crate::rust_build::has_crates(package);
        if index == workspace.entry_package || !(has_script(package) || has_crates) {
            continue;
        }
        if !entry.build_allow.contains(&package.name) {
            let what = if has_script(package) { "build script" } else { "Rust crates" };
            return Err(Failure::new(
                Code::BUILD_CONFIG,
                format!("the {what} of dependency `{}` is not allowed by this package's manifest", package.name),
            )
            .with_note(format!(
                "add `{}` to `[build] allow = [\"...\"]` in the package's `burdin.toml`",
                package.name
            )));
        }
    }
    for name in &entry.build_allow {
        // Naming the entry package itself is harmless: its own script is trusted anyway.
        if name == &entry.name {
            continue;
        }
        let known = workspace
            .packages
            .iter()
            .enumerate()
            .any(|(index, package)| index != workspace.entry_package && &package.name == name);
        if !known {
            return Err(Failure::new(
                Code::BUILD_CONFIG,
                format!("`[build] allow` names `{name}`, but no dependency of that name is in this build"),
            ));
        }
    }
    Ok(())
}

/// Runs (or reuses) one package's script and merges its generated modules.
fn run_package(
    package: &mut PackageFiles,
    profile: &crate::profile::Profile,
    fingerprint: &str,
    options: &CompileOptions,
) -> Result<Vec<String>, Failure> {
    let dir = package.dir.clone().expect("a build script belongs to a package with a directory");
    let script = dir.join(BUILD_SCRIPT);
    let out = burdin_query::workspace::package_out_dir(&dir, &package.name, &profile.dir);
    let out_src = out.join("src");
    let key_path = out.join("build.key");
    let script_bytes = std::fs::read(&script)
        .map_err(|e| Failure::new(Code::BUILD_SCRIPT, format!("cannot read {}: {e}", script.display())))?;
    let manifest_bytes = std::fs::read(dir.join("burdin.toml")).unwrap_or_default();
    // The key names the settings, not the profile: `dev` and `test` share their output when they share settings
    // (`spec/profiles.md`, BP6).
    let header = key_header(fingerprint, &profile.key(), &script_bytes, &manifest_bytes);
    let inputs = match cached_inputs(&key_path, &header, &dir, &out_src) {
        Some(inputs) => inputs,
        None => run_script(package, &dir, &out, &out_src, &script, profile, options, &header)?,
    };
    merge_generated(package, &out_src)?;
    Ok(inputs)
}

/// The fixed lines of a cache key: the format, the compiler, the profile, and the two files that always matter.
fn key_header(fingerprint: &str, profile: &str, script: &[u8], manifest: &[u8]) -> Vec<String> {
    vec![
        KEY_VERSION.to_string(),
        format!("compiler {fingerprint}"),
        format!("profile {profile}"),
        format!("script {:016x}", hash_bytes(script)),
        format!("manifest {:016x}", hash_bytes(manifest)),
    ]
}

/// The inputs of a cached run, when the key and every recorded input and output still match. `None` means the
/// script must run again (BS5).
fn cached_inputs(key_path: &Path, header: &[String], package_dir: &Path, out_src: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(key_path).ok()?;
    let mut lines = text.lines();
    for expected in header {
        if lines.next()? != expected {
            return None;
        }
    }
    let mut inputs = Vec::new();
    for line in lines {
        if let Some(rest) = line.strip_prefix("input ") {
            let (hash, path) = rest.split_once(' ')?;
            let full = resolve(package_dir, path);
            if format!("{:016x}", hash_path(&full)?) != hash {
                return None;
            }
            inputs.push(full.display().to_string());
        } else {
            let rest = line.strip_prefix("output ")?;
            let (hash, path) = rest.split_once(' ')?;
            let full = out_src.join(path);
            if format!("{:016x}", hash_file(&full)?) != hash {
                return None;
            }
        }
    }
    Some(inputs)
}

/// Compiles and runs the script, reads its declared inputs, and stores the new cache key.
#[allow(clippy::too_many_arguments)]
fn run_script(
    package: &PackageFiles,
    dir: &Path,
    out: &Path,
    out_src: &Path,
    script: &Path,
    profile: &crate::profile::Profile,
    options: &CompileOptions,
    header: &[String],
) -> Result<Vec<String>, Failure> {
    // Generated sources are the script's output, so a stale module cannot survive a rerun.
    if let Err(e) = std::fs::remove_dir_all(out_src)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        return Err(Failure::new(Code::BUILD_SCRIPT, format!("cannot clear {}: {e}", out_src.display())));
    }
    std::fs::create_dir_all(out_src)
        .map_err(|e| Failure::new(Code::BUILD_SCRIPT, format!("cannot create {}: {e}", out_src.display())))?;
    let _ = std::fs::remove_file(out.join(INPUTS_FILE));

    let exe = out.join("build-script").with_extension(std::env::consts::EXE_EXTENSION);
    let nested = CompileOptions {
        profile: profile.clone(),
        debug: false,
        strip: false,
        lto: false,
        pgo_generate: false,
        pgo_use: None,
        emit_ir: None,
        emit: None,
        link_libs: &[],
        mode: EntryMode::Build,
        diagnostics: options.diagnostics,
        comptime_steps: options.comptime_steps,
        emit_deps: None,
        report_clean: false,
        run_build_scripts: false,
        locked: false,
        features: Default::default(),
        offline: false,
        jobs: options.jobs,
        sanitize: Default::default(),
        fuzz_target: None,
        verbose: false,
    };
    let compiled = crate::compile(script, out, Some(&exe), &nested).map_err(|_| {
        Failure::new(Code::BUILD_SCRIPT, format!("the build script of package `{}` did not compile", package.name))
    })?;

    let status = run_program(&exe, dir, out, &profile.dir).map_err(|e| {
        Failure::new(Code::BUILD_SCRIPT, format!("cannot run the build script of `{}`: {e}", package.name))
            .with_note(format!("the script is {}", script.display()))
    })?;
    if !status.success() {
        return Err(Failure::new(
            Code::BUILD_SCRIPT,
            format!("the build script of package `{}` failed ({})", package.name, describe_status(&status)),
        ));
    }

    let mut inputs: Vec<(String, PathBuf)> = Vec::new();
    for input in read_declared_inputs(out, dir)? {
        let shown = input.display().to_string();
        if !inputs.iter().any(|(seen, _)| seen == &shown) {
            inputs.push((shown, input));
        }
    }
    for input in &compiled.build_inputs {
        let path = PathBuf::from(input);
        if !inputs.iter().any(|(seen, _)| seen == input) {
            inputs.push((input.clone(), path));
        }
    }
    let inputs: Vec<String> = inputs.into_iter().map(|(shown, _)| shown).collect();

    let outputs = generated_files(out_src)?;
    let mut key = String::new();
    for line in header {
        key.push_str(line);
        key.push('\n');
    }
    for input in &inputs {
        let path = resolve(dir, input);
        let hash = hash_path(&path).ok_or_else(|| {
            Failure::new(
                Code::BUILD_SCRIPT,
                format!(
                    "the build script of `{}` declared an input that cannot be read: {}",
                    package.name,
                    path.display()
                ),
            )
        })?;
        key.push_str(&format!("input {hash:016x} {input}\n"));
    }
    for (relative, full) in &outputs {
        let hash = hash_file(full).ok_or_else(|| {
            Failure::new(Code::BUILD_SCRIPT, format!("cannot read generated file {}", full.display()))
        })?;
        key.push_str(&format!("output {hash:016x} {relative}\n"));
    }
    std::fs::write(key_path(out), key)
        .map_err(|e| Failure::new(Code::BUILD_SCRIPT, format!("cannot write the build cache key: {e}")))?;
    Ok(inputs)
}

fn key_path(out: &Path) -> PathBuf {
    out.join("build.key")
}

/// Runs the script with the package directory as its working directory and as its first argument, and the output
/// directory as its second (BS3). The driver's own directory goes first on `PATH`, so the script can invoke
/// `burdin bindgen` (BS7).
fn run_program(exe: &Path, dir: &Path, out: &Path, profile: &str) -> std::io::Result<std::process::ExitStatus> {
    // The loader canonicalizes package directories, which on Windows gives an extended-length `\\?\` path. That
    // prefix disables slash normalization, so a script that joins `{out}/src/x.bd` with a forward slash would fail
    // to create the file. The script sees the ordinary spelling; the driver keeps the canonical one.
    let script_dir = plain_path(dir);
    let script_out = plain_path(out);
    let mut command = Command::new(exe);
    command.current_dir(dir);
    command.arg(&script_dir).arg(&script_out);
    command.env("BURDIN_MANIFEST_DIR", &script_dir);
    command.env("BURDIN_OUT_DIR", &script_out);
    command.env("BURDIN_PROFILE", profile);
    if let Ok(current) = std::env::current_exe() {
        command.env("BURDIN", &current);
        if let Some(parent) = current.parent() {
            let mut paths: Vec<PathBuf> = vec![plain_path(parent)];
            if let Some(existing) = std::env::var_os("PATH") {
                paths.extend(std::env::split_paths(&existing));
            }
            if let Ok(joined) = std::env::join_paths(paths) {
                command.env("PATH", joined);
            }
        }
    }
    // The script's output belongs on the compiler's stderr: stdout is reserved for the compiled program, which
    // `burdin run` passes through unchanged.
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = command.output()?;
    eprint!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    Ok(output.status)
}

/// A path as a program argument or a `PATH` entry: the Windows extended-length prefix is removed because it makes
/// `/` an ordinary character and breaks a script's forward-slash joins.
fn plain_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.as_os_str().to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest.to_string());
        }
    }
    path.to_path_buf()
}

/// A status as the message should show it, without the platform's exit-code conventions.
fn describe_status(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit code {code}"),
        None => "terminated by a signal".to_string(),
    }
}

/// The files and directories the script listed in `<out>/inputs`, resolved against the package directory. Each must
/// exist: a declared input that cannot be read would make every later build rerun. A directory stands for every file
/// under it, the way a `rerun-if-changed` on a directory does in other build tools (BS5).
fn read_declared_inputs(out: &Path, dir: &Path) -> Result<Vec<PathBuf>, Failure> {
    let text = match std::fs::read_to_string(out.join(INPUTS_FILE)) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(Failure::new(Code::BUILD_SCRIPT, format!("cannot read the build script's input list: {e}")));
        }
    };
    let mut inputs = Vec::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let path = resolve(dir, line);
        if !path.is_file() && !path.is_dir() {
            return Err(Failure::new(
                Code::BUILD_SCRIPT,
                format!("the build script declared an input that does not exist: {}", path.display()),
            ));
        }
        inputs.push(path);
    }
    Ok(inputs)
}

/// A path from a cache key or an input list: relative paths are relative to the package directory.
fn resolve(package_dir: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() { path.to_path_buf() } else { package_dir.join(path) }
}

/// Every generated file under `<out>/src`, as `(path relative to src with `/`, full path)`, sorted.
fn generated_files(out_src: &Path) -> Result<Vec<(String, PathBuf)>, Failure> {
    let mut files = Vec::new();
    if out_src.is_dir() {
        walk_files(out_src, out_src, &mut files)
            .map_err(|e| Failure::new(Code::BUILD_OUTPUT, format!("cannot read {}: {e}", out_src.display())))?;
    }
    files.sort();
    Ok(files)
}

fn walk_files(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_files(root, &path, files)?;
        } else if path.is_file() {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let mut shown = String::new();
            for component in relative.components() {
                if !shown.is_empty() {
                    shown.push('/');
                }
                shown.push_str(&component.as_os_str().to_string_lossy());
            }
            files.push((shown, path));
        }
    }
    Ok(())
}

/// Adds `<out>/src/**.bd` to the package's modules under their own paths (BS4). A generated module that repeats a
/// source module's path is an error, not an override.
pub(crate) fn merge_generated(package: &mut PackageFiles, out_src: &Path) -> Result<(), Failure> {
    if !out_src.is_dir() {
        return Ok(());
    }
    let generated = package::discover_modules_under(out_src)
        .map_err(|message| Failure::new(Code::BUILD_OUTPUT, format!("package `{}`: {message}", package.name)))?;
    for module in &generated {
        if package.modules.iter().any(|existing| existing.path == module.path) {
            let name = module.path.join(".");
            return Err(Failure::new(
                Code::BUILD_OUTPUT,
                format!(
                    "the build script of `{}` generated the module `{name}`, which the package also declares",
                    package.name
                ),
            )
            .with_note("generated modules may add new module names, but they cannot replace a source module"));
        }
    }
    package.modules.extend(generated);
    // Generated modules keep their place at the end: the entry module index was computed from the source modules,
    // and sorting here would move `main` out from under it.
    if package.root_module.is_none() {
        package.root_module = package
            .modules
            .iter()
            .position(|m| m.path.len() == 1 && m.path[0] == package.name)
            .or_else(|| package.modules.iter().position(|m| m.path.len() == 1 && m.path[0] == "lib"));
    }
    Ok(())
}

/// A stable description of the compiler binary, so a rebuilt compiler reruns the scripts. The version and commit
/// make the result meaningful across checkouts; the executable's length and modification time make it change
/// within one commit too.
pub(crate) fn compiler_fingerprint() -> String {
    let (length, modified) = std::env::current_exe()
        .ok()
        .and_then(|exe| std::fs::metadata(exe).ok())
        .map(|meta| {
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            (meta.len(), modified)
        })
        .unwrap_or((0, 0));
    format!(
        "{} {} llvm {} exe {} {}",
        env!("BURDIN_VERSION"),
        env!("BURDIN_GIT_COMMIT"),
        burdin_codegen::llvm_version(),
        length,
        modified
    )
}

fn hash_file(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    Some(hash_bytes(&bytes))
}

/// A declared input's content hash: a file's bytes, or for a directory every file under it. A directory hash covers
/// each file's relative path and contents in sorted order, so adding, removing, renaming, or changing any file under
/// it reruns the script, while touching one does not.
fn hash_path(path: &Path) -> Option<u64> {
    if !path.is_dir() {
        return hash_file(path);
    }
    let mut files = Vec::new();
    walk_files(path, path, &mut files).ok()?;
    files.sort();
    let mut hash = fnv_update(FNV_OFFSET, b"directory");
    for (relative, full) in files {
        hash = fnv_update(hash, relative.as_bytes());
        hash = fnv_update(hash, &[0]);
        let bytes = std::fs::read(full).ok()?;
        hash = fnv_update(hash, &bytes);
        hash = fnv_update(hash, &[0xff]);
    }
    Some(hash)
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// FNV-1a, 64-bit. The key records content, not modification times, so a touched file does not rerun the script.
pub(crate) fn hash_bytes(bytes: &[u8]) -> u64 {
    fnv_update(FNV_OFFSET, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let path = std::env::temp_dir().join(format!(
                "burdin-build-{}-{}-{}",
                name,
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
            ));
            fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    #[test]
    fn the_key_holds_the_script_and_manifest_contents() {
        let dir = TempDir::new("header");
        let out_src = dir.path().join("target/debug/src");
        let key = dir.path().join("target/debug/build.key");
        let input = dir.path().join("header.h");
        write(&input, "int x;\n");
        write(&out_src.join("generated.bd"), "const X: i32 = 1\n");

        let header = key_header("compiler", "debug", b"script", b"manifest");
        let outputs = generated_files(&out_src).unwrap();
        let mut text = String::new();
        for line in &header {
            text.push_str(line);
            text.push('\n');
        }
        text.push_str(&format!("input {:016x} header.h\n", hash_file(&input).unwrap()));
        for (relative, full) in &outputs {
            text.push_str(&format!("output {:016x} {relative}\n", hash_file(full).unwrap()));
        }
        write(&key, &text);

        assert_eq!(cached_inputs(&key, &header, dir.path(), &out_src).unwrap(), vec![input.display().to_string()]);
    }

    #[test]
    fn a_changed_input_manifest_profile_or_output_invalidates_the_key() {
        let dir = TempDir::new("stale");
        let out_src = dir.path().join("target/debug/src");
        let key = dir.path().join("target/debug/build.key");
        let input = dir.path().join("header.h");
        write(&input, "int x;\n");
        write(&out_src.join("generated.bd"), "const X: i32 = 1\n");

        let header = key_header("compiler", "debug", b"script", b"manifest");
        let outputs = generated_files(&out_src).unwrap();
        let mut text = String::new();
        for line in &header {
            text.push_str(line);
            text.push('\n');
        }
        text.push_str(&format!("input {:016x} header.h\n", hash_file(&input).unwrap()));
        for (relative, full) in &outputs {
            text.push_str(&format!("output {:016x} {relative}\n", hash_file(full).unwrap()));
        }
        write(&key, &text);
        assert!(cached_inputs(&key, &header, dir.path(), &out_src).is_some());

        write(&input, "int y;\n");
        assert!(cached_inputs(&key, &header, dir.path(), &out_src).is_none(), "a changed input reruns");
        write(&input, "int x;\n");
        assert!(cached_inputs(&key, &header, dir.path(), &out_src).is_some());

        let release = key_header("compiler", "release", b"script", b"manifest");
        assert!(cached_inputs(&key, &release, dir.path(), &out_src).is_none(), "the profile is part of the key");

        fs::remove_file(out_src.join("generated.bd")).unwrap();
        assert!(cached_inputs(&key, &header, dir.path(), &out_src).is_none(), "a missing output reruns");

        write(&out_src.join("generated.bd"), "const X: i32 = 2\n");
        assert!(cached_inputs(&key, &header, dir.path(), &out_src).is_none(), "a changed output reruns");
    }

    #[test]
    fn the_script_is_always_part_of_the_key() {
        let dir = TempDir::new("script");
        let key = dir.path().join("key");
        let header = key_header("compiler", "debug", b"first", b"manifest");
        write(&key, &header.iter().map(|line| format!("{line}\n")).collect::<String>());
        assert!(cached_inputs(&key, &header, dir.path(), &dir.path().join("src")).is_some());
        let changed = key_header("compiler", "debug", b"second", b"manifest");
        assert!(cached_inputs(&key, &changed, dir.path(), &dir.path().join("src")).is_none());
    }

    #[test]
    fn declared_inputs_are_resolved_and_missing_ones_are_rejected() {
        let dir = TempDir::new("inputs");
        let out = dir.path().join("target/debug");
        write(&dir.path().join("include/point.h"), "struct Point { int x; };\n");
        write(&out.join(INPUTS_FILE), "# a comment\ninclude/point.h\n\n");
        assert_eq!(read_declared_inputs(&out, dir.path()).unwrap(), vec![dir.path().join("include/point.h")]);

        // A directory is an input too: it stands for every file under it.
        write(&out.join(INPUTS_FILE), "include\n");
        assert_eq!(read_declared_inputs(&out, dir.path()).unwrap(), vec![dir.path().join("include")]);

        write(&out.join(INPUTS_FILE), "include/missing.h\n");
        let failure = read_declared_inputs(&out, dir.path()).err().unwrap();
        assert_eq!(failure.code, Code::BUILD_SCRIPT);
        assert!(failure.message.contains("does not exist"));
    }

    #[test]
    fn a_directory_input_hashes_the_content_of_every_file_under_it() {
        let dir = TempDir::new("directory");
        let include = dir.path().join("include");
        write(&include.join("point.h"), "struct Point { int x; };\n");
        write(&include.join("nested/size.h"), "#define SIZE 4\n");
        let first = hash_path(&include).unwrap();

        // A touched file keeps the same content hash.
        let touched = fs::File::options().write(true).open(include.join("point.h")).unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
        touched.set_modified(later).unwrap();
        assert_eq!(hash_path(&include).unwrap(), first, "a touched file hashes the same");

        write(&include.join("point.h"), "struct Point { int y; };\n");
        let changed = hash_path(&include).unwrap();
        assert_ne!(changed, first, "a changed file under the directory changes the hash");
        write(&include.join("point.h"), "struct Point { int x; };\n");
        assert_eq!(hash_path(&include).unwrap(), first);

        write(&include.join("nested/added.h"), "");
        assert_ne!(hash_path(&include).unwrap(), first, "a new file changes the hash");
        fs::remove_file(include.join("nested/added.h")).unwrap();
        assert_eq!(hash_path(&include).unwrap(), first, "removing the file restores the hash");

        // A file and a directory of the same size do not collide, because the directory hash is marked.
        let lone = dir.path().join("lone.h");
        write(&lone, "");
        assert_ne!(hash_path(&lone).unwrap(), hash_path(&include).unwrap());
    }

    #[test]
    fn generated_modules_merge_and_conflicts_are_rejected() {
        let dir = TempDir::new("merge");
        let out_src = dir.path().join("src");
        write(&out_src.join("bindings.bd"), "const X: i32 = 1\n");
        let package_with = |modules: Vec<&str>| PackageFiles {
            name: "app".to_string(),
            dir: Some(dir.path().to_path_buf()),
            modules: modules
                .into_iter()
                .map(|name| package::ModuleFile {
                    path: vec![name.to_string()],
                    file: dir.path().join("src").join(format!("{name}.bd")),
                    text: None,
                })
                .collect(),
            deps: Vec::new(),
            build_allow: Vec::new(),
            lint: Vec::new(),
            rust: Default::default(),
            edition: Default::default(),
            root_module: None,
        };
        let mut package = package_with(vec!["main"]);
        merge_generated(&mut package, &out_src).unwrap();
        assert!(package.modules.iter().any(|m| m.path == ["bindings"]));

        let mut conflicting = package_with(vec!["main"]);
        write(&out_src.join("main.bd"), "fn main() {}\n");
        let failure = merge_generated(&mut conflicting, &out_src).err().unwrap();
        assert_eq!(failure.code, Code::BUILD_OUTPUT);
        assert!(failure.message.contains("generated the module `main`"), "{}", failure.message);
    }

    #[test]
    fn the_fingerprint_names_the_compiler_and_changes_with_the_binary() {
        let first = compiler_fingerprint();
        assert!(first.contains(env!("BURDIN_VERSION")));
        assert!(first.contains("llvm"));
    }

    fn package(name: &str, dir: Option<PathBuf>) -> PackageFiles {
        PackageFiles {
            name: name.to_string(),
            dir,
            modules: Vec::new(),
            deps: Vec::new(),
            build_allow: Vec::new(),
            lint: Vec::new(),
            rust: Default::default(),
            edition: Default::default(),
            root_module: None,
        }
    }

    #[test]
    fn grants_name_dependencies_and_ignore_the_entry_itself() {
        let mut entry = package("app", None);
        entry.build_allow = vec!["app".to_string(), "lib".to_string()];
        let workspace = Workspace {
            packages: vec![entry, package("lib", None)],
            entry_package: 0,
            entry_module: 0,
            stem: "app".to_string(),
        };
        assert!(check_grants(&workspace).is_ok(), "a dependency and the entry itself are both acceptable");

        let mut unknown = package("app", None);
        unknown.build_allow = vec!["ghost".to_string()];
        let workspace = Workspace {
            packages: vec![unknown, package("lib", None)],
            entry_package: 0,
            entry_module: 0,
            stem: "app".to_string(),
        };
        assert_eq!(check_grants(&workspace).err().unwrap().code, Code::BUILD_CONFIG);
    }
}
