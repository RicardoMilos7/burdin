//! `burdin vendor`: the command line of `burdin_pkg::vendor::vendor` (`spec/packages.md`, PKG23).

use std::process::ExitCode;

/// `burdin vendor [<dir>]`. Exit status 1 when the graph does not resolve or a copy cannot be made.
pub(crate) fn run(argv: &[String]) -> ExitCode {
    let (dir, _) = match crate::cli::parse_dir_args("vendor", argv, &[]) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            crate::cli::print_help("vendor");
            return ExitCode::SUCCESS;
        }
        Err(code) => return code,
    };
    if !dir.join("burdin.toml").is_file() {
        eprintln!("error: {} is not a package: it has no burdin.toml", burdin_query::package::source_path_text(&dir));
        return ExitCode::from(1);
    }
    match burdin_pkg::vendor::vendor(&dir) {
        Ok(vendored) if vendored.names.is_empty() => {
            println!("No git dependencies to vendor");
            ExitCode::SUCCESS
        }
        Ok(vendored) => {
            let noun = if vendored.names.len() == 1 { "package" } else { "packages" };
            println!(
                "Vendored {} {noun} into {}",
                vendored.names.len(),
                burdin_query::package::source_path_text(&vendored.dir)
            );
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(1)
        }
    }
}
