//! `burdin fuzz <target> <name>` (`spec/fuzz.md`): builds one `fuzz` target with libFuzzer's coverage and
//! AddressSanitizer, links libFuzzer's `main`, and runs it on the target's corpus. Crashes are saved where
//! `burdin test --fuzz-replay` finds them.

use crate::{Args, CompileOptions, EntryMode, TempDir, compile};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// The directories of one fuzz target, next to the package's `burdin.toml` or the single file (FZ4).
pub(crate) struct FuzzDirs {
    /// `fuzz/corpus/<name>/`: the inputs libFuzzer keeps because they reached new code.
    pub(crate) corpus: PathBuf,
    /// `fuzz/artifacts/<name>/`: the inputs that crashed the target.
    pub(crate) artifacts: PathBuf,
}

/// Where the fuzz directories of a target live: the entry package's directory, or the directory of a single-file
/// target.
pub(crate) fn fuzz_root(target: &Path, package_dir: Option<&Path>) -> PathBuf {
    match package_dir {
        Some(dir) => dir.to_path_buf(),
        None => target.parent().filter(|dir| !dir.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf(),
    }
}

pub(crate) fn dirs(root: &Path, name: &str) -> FuzzDirs {
    let fuzz = root.join("fuzz");
    FuzzDirs { corpus: fuzz.join("corpus").join(name), artifacts: fuzz.join("artifacts").join(name) }
}

pub(crate) fn run(args: &Args) -> ExitCode {
    let name = args.fuzz_name.as_deref().expect("parse_args requires a name");
    let sanitize = burdin_codegen::Sanitizers { fuzzer: true, address: true, ..Default::default() };
    if let Err(message) = crate::sanitize::check_platform(&sanitize, std::env::consts::OS, std::env::consts::ARCH) {
        eprintln!("error: {message}");
        return ExitCode::from(2);
    }
    if let Err(code) = crate::sync_dependencies(&args.target, args.locked, args.offline) {
        return code;
    }
    let workspace = match crate::package::load_selected(&args.target, EntryMode::Fuzz, &args.features) {
        Ok(workspace) => workspace,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    };
    let root = fuzz_root(&args.target, workspace.packages[workspace.entry_package].dir.as_deref());
    let dirs = dirs(&root, name);
    let minimize = match &args.minimize {
        Some(input) if !input.is_file() => {
            eprintln!("error: cannot read the input to minimize, {}", input.display());
            return ExitCode::from(2);
        }
        other => other.clone(),
    };

    let binaries = match TempDir::new() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("error: cannot create a temporary directory: {e}");
            return ExitCode::from(2);
        }
    };
    let options = CompileOptions {
        release: args.release,
        debug: args.debug,
        strip: false,
        lto: false,
        pgo_generate: false,
        pgo_use: None,
        emit_ir: None,
        emit: None,
        link_libs: &args.link_libs,
        mode: EntryMode::Fuzz,
        diagnostics: &args.diagnostics,
        comptime_steps: args.comptime_steps,
        emit_deps: None,
        report_clean: false,
        run_build_scripts: true,
        locked: args.locked,
        features: args.features.clone(),
        offline: args.offline,
        jobs: crate::jobs_or_default(args.jobs),
        sanitize,
        fuzz_target: Some(name),
    };
    let exe = binaries.path.join(format!("fuzz-{name}")).with_extension(std::env::consts::EXE_EXTENSION);
    if let Err(code) = compile(&args.target, &binaries.path, Some(&exe), &options) {
        return code;
    }
    for dir in [&dirs.corpus, &dirs.artifacts] {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("error: cannot create {}: {e}", dir.display());
            return ExitCode::from(2);
        }
    }

    let mut command = Command::new(&exe);
    match &minimize {
        // Crash minimization: libFuzzer runs the input, then keeps trying smaller variants that still crash, for
        // the time limit (60 seconds unless `--max-time` says otherwise).
        Some(input) => {
            let file = input.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            let out = dirs.artifacts.join(format!("minimized-{file}"));
            command
                .arg("-minimize_crash=1")
                .arg(format!("-max_total_time={}", args.max_time.unwrap_or(60)))
                .arg(format!("-exact_artifact_path={}", out.display()))
                .args(&args.program_args)
                .arg(input);
            eprintln!("note: minimizing {} into {}", input.display(), out.display());
        }
        None => {
            command.arg(format!("-artifact_prefix={}{}", dirs.artifacts.display(), std::path::MAIN_SEPARATOR));
            if let Some(seconds) = args.max_time {
                command.arg(format!("-max_total_time={seconds}"));
            }
            command.args(&args.program_args).arg(&dirs.corpus);
        }
    }
    let status = match command.status() {
        Ok(status) => status,
        Err(e) => {
            eprintln!("error: cannot run the fuzz target {}: {e}", exe.display());
            return ExitCode::from(2);
        }
    };
    if minimize.is_none() && !status.success() {
        eprintln!(
            "note: the input that crashed `{name}` is in {}; `burdin test --fuzz-replay` runs it again",
            dirs.artifacts.display()
        );
    }
    match status.code() {
        Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        // Killed by a signal (an interrupted session, or a crash libFuzzer did not catch).
        None => ExitCode::from(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fuzz_directories_sit_next_to_the_package_or_the_file() {
        let root = fuzz_root(Path::new("pkg/src/main.bd"), Some(Path::new("pkg")));
        let found = dirs(&root, "parse");
        assert_eq!(found.corpus, Path::new("pkg").join("fuzz").join("corpus").join("parse"));
        assert_eq!(found.artifacts, Path::new("pkg").join("fuzz").join("artifacts").join("parse"));
        assert_eq!(fuzz_root(Path::new("dir/one.bd"), None), Path::new("dir"));
        assert_eq!(fuzz_root(Path::new("one.bd"), None), Path::new("."));
    }
}
