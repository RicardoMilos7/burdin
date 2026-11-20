//! `burdin clean`: removes a package's build output and, with `--cache`, the caches under the Burdin home
//! (`spec/packages.md`, PKG19).

use std::path::Path;
use std::process::ExitCode;

/// `burdin clean [<dir>] [--cache]`. Exit status 1 when `<dir>` is not a package or a path cannot be removed.
pub(crate) fn run(argv: &[String]) -> ExitCode {
    let (dir, flags) = match crate::cli::parse_dir_args("clean", argv, &["--cache"]) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            crate::cli::print_help("clean");
            return ExitCode::SUCCESS;
        }
        Err(code) => return code,
    };
    if !dir.join("burdin.toml").is_file() {
        eprintln!("error: {} is not a package: it has no burdin.toml", burdin_query::package::source_path_text(&dir));
        return ExitCode::from(1);
    }
    let mut targets = vec![burdin_query::workspace::target_dir(&dir)];
    if flags.iter().any(|f| f == "--cache") {
        let Some(home) = burdin_query::package::home_dir() else {
            eprintln!("error: cannot find the Burdin home directory for the caches: set BURDIN_HOME");
            return ExitCode::from(1);
        };
        targets.push(home.join("cache"));
        targets.push(home.join("git"));
    }
    for target in &targets {
        if let Err(message) = remove(target) {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    }
    ExitCode::SUCCESS
}

/// Removes `path` (a directory or a file) and reports what happened.
fn remove(path: &Path) -> Result<(), String> {
    let shown = burdin_query::package::source_path_text(path);
    if path.is_dir() {
        std::fs::remove_dir_all(path).map_err(|e| format!("cannot remove {shown}: {e}"))?;
        println!("Removed {shown}");
    } else if path.exists() {
        std::fs::remove_file(path).map_err(|e| format!("cannot remove {shown}: {e}"))?;
        println!("Removed {shown}");
    } else {
        println!("Nothing to remove: {shown} does not exist");
    }
    Ok(())
}
