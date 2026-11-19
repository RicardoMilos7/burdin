//! Script mode for `burdin run` (`spec/scripts.md`, SC4-SC7): a one-file target whose first line is a shebang or
//! whose leading doc block declares dependencies runs from a content-addressed cache, so the second run of an
//! unchanged script starts instantly. The cache lives under the Burdin home; a cache that cannot be written is
//! skipped, never fatal.

use crate::{Args, CompileOptions, TempDir, compile};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// The binary's name inside one cache entry. It does not depend on the script's name, so the key alone decides
/// where a script's build lives.
const BINARY: &str = "script";

/// Runs the target as a script when it is one. `None` when it is not a script, so `burdin run` takes its usual path.
pub(crate) fn run_script(args: &Args, options: &CompileOptions) -> Option<ExitCode> {
    let target = &args.target;
    if !target.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(target).ok()?;
    if !is_script(&text) {
        return None;
    }
    Some(run_cached(target, &text, args, options))
}

/// Whether a one-file target is a script: its first line is `#!`, or its leading doc block declares dependencies
/// (SC3). A plain one-file target is unchanged and never cached.
pub(crate) fn is_script(text: &str) -> bool {
    text.starts_with("#!") || matches!(burdin_query::package::script_manifest(text), Ok(Some(_)))
}

fn run_cached(target: &Path, text: &str, args: &Args, options: &CompileOptions) -> ExitCode {
    let key = match content_key(target, text, options) {
        Ok(key) => key,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    };
    let Some(cache) = burdin_query::package::home_dir().map(|home| home.join("cache").join("scripts").join(&key))
    else {
        return build_uncached(target, args, options);
    };
    let exe = cache.join(format!("{BINARY}{}", std::env::consts::EXE_EXTENSION));
    if exe.is_file() {
        return run_exe(&exe, args);
    }
    // Build into a staging directory, then publish the finished binary with one file rename, so a crash mid-build
    // never leaves a half-written binary where a later run would trust it. The file is renamed, not the staging
    // directory: Windows refuses to rename a directory while anything inside it is open, and a freshly linked
    // executable is often held for a moment by a virus scanner.
    let staging = cache.with_extension(format!("tmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    if let Err(e) = std::fs::create_dir_all(&staging) {
        // An unwritable home is not a broken script: fall back to a build that is not cached.
        let _ = e;
        return build_uncached(target, args, options);
    }
    let staged_exe = staging.join(format!("{BINARY}{}", std::env::consts::EXE_EXTENSION));
    let code = build_and_run(target, &staging, &staged_exe, args, options);
    // The linker writes the binary only when the build succeeded; the program's own exit status is its own.
    if staged_exe.is_file() {
        let _ = publish(&staged_exe, &cache, &exe);
    }
    let _ = std::fs::remove_dir_all(&staging);
    code
}

/// Moves a built binary to its cache entry. The rename is retried briefly, and falls back to a copy that is itself
/// renamed into place, so the entry either holds a whole binary or no binary at all (SC6).
fn publish(staged: &Path, cache: &Path, exe: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(cache)?;
    let mut last = None;
    for attempt in 0..5u64 {
        match std::fs::rename(staged, exe) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(50 * (attempt + 1)));
    }
    let partial = exe.with_extension(format!("part-{}", std::process::id()));
    let copied = std::fs::copy(staged, &partial).and_then(|_| std::fs::rename(&partial, exe));
    if copied.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    copied.map_err(|_| last.expect("the loop ran"))
}

/// Builds without a cache, for a Burdin home that cannot hold one.
fn build_uncached(target: &Path, args: &Args, options: &CompileOptions) -> ExitCode {
    match TempDir::new() {
        Ok(dir) => {
            let exe = dir.path.join(format!("{BINARY}{}", std::env::consts::EXE_EXTENSION));
            build_and_run(target, &dir.path, &exe, args, options)
        }
        Err(e) => {
            eprintln!("error: cannot create a temporary directory: {e}");
            ExitCode::from(1)
        }
    }
}

/// The script's content key (SC5): its text, every package of its dependency closure (manifests and sources),
/// the compiler's identity, and the flags that change what a build produces.
fn content_key(target: &Path, text: &str, options: &CompileOptions) -> Result<String, String> {
    let mut hasher = DefaultHasher::new();
    "burdin-script-v1".hash(&mut hasher);
    env!("BURDIN_VERSION").hash(&mut hasher);
    env!("BURDIN_GIT_COMMIT").hash(&mut hasher);
    burdin_codegen::llvm_version().hash(&mut hasher);
    text.hash(&mut hasher);
    options.release.hash(&mut hasher);
    options.debug.hash(&mut hasher);
    options.strip.hash(&mut hasher);
    options.lto.hash(&mut hasher);
    options.pgo_generate.hash(&mut hasher);
    options.pgo_use.hash(&mut hasher);
    options.link_libs.hash(&mut hasher);
    // The dependency closure: the manifests and every source file of every package the load pulled in. The
    // embedded standard library has no directory, and the compiler's identity above covers it.
    let workspace = burdin_query::package::load(target)?;
    for package in &workspace.packages {
        let Some(dir) = &package.dir else { continue };
        package.name.hash(&mut hasher);
        hash_tree(dir, dir, &mut hasher)?;
    }
    // A second, independently seeded hash widens the key to 128 bits, like the query library's text fingerprints.
    let a = hasher.finish();
    let mut second = DefaultHasher::new();
    0x6a09_e667_f3bc_c909u64.hash(&mut second);
    (a, text).hash(&mut second);
    Ok(format!("{a:016x}{:016x}", second.finish()))
}

/// Hashes every file under `dir` with its path relative to `root`, in sorted order so the key is deterministic.
fn hash_tree(root: &Path, dir: &Path, hasher: &mut DefaultHasher) -> Result<(), String> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    entries.sort();
    for path in entries {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if name.starts_with('.') || name == "target" {
            continue;
        }
        if path.is_dir() {
            hash_tree(root, &path, hasher)?;
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path).display().to_string();
        relative.hash(hasher);
        let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        bytes.hash(hasher);
    }
    Ok(())
}

/// Builds `target` to `exe` and runs it, passing its exit status through.
fn build_and_run(target: &Path, dir: &Path, exe: &Path, args: &Args, options: &CompileOptions) -> ExitCode {
    match compile(target, dir, Some(exe), options) {
        Ok(_) => run_exe(exe, args),
        Err(code) => code,
    }
}

fn run_exe(exe: &Path, args: &Args) -> ExitCode {
    match Command::new(exe).args(&args.program_args).status() {
        Ok(status) => ExitCode::from(crate::exit_code_of(status)),
        Err(e) => {
            eprintln!("error: cannot run {}: {e}", exe.display());
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_told_apart_from_plain_files() {
        assert!(is_script("#!/usr/bin/env burdin run\nfn main() {}\n"));
        assert!(is_script("//! [dependencies]\n//! lib = { path = \"../lib\" }\nfn main() {}\n"));
        assert!(!is_script("fn main() {}\n"));
        assert!(!is_script("//! Just documentation.\nfn main() {}\n"));
        assert!(!is_script("\n#!/usr/bin/env burdin run\nfn main() {}\n"), "the shebang is the first line only");
    }
}
