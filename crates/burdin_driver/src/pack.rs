//! `burdin package`: the command line of `burdin_pkg::archive::package` (`spec/packages.md`, PKG20-PKG22).

use std::process::ExitCode;

/// `burdin package [<dir>]`. Exit status 1 when the package cannot be packed (no `version`, a symbolic link among
/// the files, a path too long for the archive).
pub(crate) fn run(argv: &[String]) -> ExitCode {
    let (dir, _) = match crate::cli::parse_dir_args("package", argv, &[]) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            crate::cli::print_help("package");
            return ExitCode::SUCCESS;
        }
        Err(code) => return code,
    };
    if !dir.join("burdin.toml").is_file() {
        eprintln!("error: {} is not a package: it has no burdin.toml", burdin_query::package::source_path_text(&dir));
        return ExitCode::from(1);
    }
    match burdin_pkg::archive::package(&dir) {
        Ok(packaged) => {
            let noun = if packaged.files == 1 { "file" } else { "files" };
            println!(
                "Packaged {} {noun} into {} ({})",
                packaged.files,
                burdin_query::package::source_path_text(&packaged.path),
                packaged.checksum
            );
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(1)
        }
    }
}
