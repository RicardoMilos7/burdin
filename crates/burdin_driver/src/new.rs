//! `burdin new <name>`: creates a package directory with a `burdin.toml`, a `src/main.bd` that prints a greeting, and
//! a `.gitignore` for the build output. It never writes into a path that already exists, and it removes what it
//! created if a write fails, so a failed run leaves nothing behind.

use crate::package::is_ident;
use std::path::Path;
use std::process::ExitCode;

const NEW_USAGE: &str = "\
usage:
  burdin new <name>    create the package directory <name> with burdin.toml, src/main.bd, and .gitignore";

/// `main.bd`: the program `burdin run` prints a greeting for.
const MAIN: &str = "fn main() {\n    println(\"hello, world\")\n}\n";

/// `burdin new <name>`. Exit status 2 for a usage error, an invalid name, an existing path, or an I/O failure, the
/// same as the other subcommands.
pub(crate) fn run(args: &[String]) -> ExitCode {
    let mut target: Option<&str> = None;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{NEW_USAGE}");
                return ExitCode::SUCCESS;
            }
            option if option.starts_with('-') => return usage_error(&format!("unknown option `{option}`")),
            _ if target.is_none() => target = Some(arg),
            _ => return usage_error(&format!("unexpected argument `{arg}`")),
        }
    }
    let Some(target) = target else {
        return usage_error("`new` needs the name of the package to create");
    };
    match create(Path::new(target)) {
        Ok(name) => {
            println!("Created package `{name}` in {target}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("error: {message}\n{NEW_USAGE}");
    ExitCode::from(2)
}

/// Creates the package directory at `dir` and returns the package name, which is the last component of `dir`.
fn create(dir: &Path) -> Result<String, String> {
    let name = package_name(dir)?;
    // `symlink_metadata`: a dangling symbolic link is still something that is already there.
    if std::fs::symlink_metadata(dir).is_ok() {
        return Err(format!("`{}` already exists; `burdin new` does not overwrite it", dir.display()));
    }
    if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    // `create_dir` fails when the path appeared since the check above, so nothing is ever written into it.
    std::fs::create_dir(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    if let Err(message) = write_package(dir, &name) {
        // The directory is the one just created, so removing it cannot lose anything else.
        let _ = std::fs::remove_dir_all(dir);
        return Err(message);
    }
    Ok(name)
}

fn write_package(dir: &Path, name: &str) -> Result<(), String> {
    let src = dir.join("src");
    std::fs::create_dir(&src).map_err(|e| format!("cannot create {}: {e}", src.display()))?;
    let files = [
        (dir.join("burdin.toml"), manifest(name)),
        (src.join("main.bd"), MAIN.to_string()),
        (dir.join(".gitignore"), gitignore(name)),
    ];
    for (path, text) in files {
        std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok(())
}

/// The `.gitignore`: the executable `burdin build .` leaves in the package directory (`name`, or `name.exe` on
/// Windows) and the build-script output under `target/`.
fn gitignore(name: &str) -> String {
    format!("/target/\n/{name}\n/{name}.exe\n")
}

fn manifest(name: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n")
}

/// The package name for the directory `dir`: its last component, which other packages write in `use <name>`, so it
/// must be an identifier that is not a reserved word and does not shadow the standard library.
fn package_name(dir: &Path) -> Result<String, String> {
    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
        return Err(format!(
            "cannot take a package name from `{}`; give a directory name such as `app`",
            dir.display()
        ));
    };
    if !is_ident(name) {
        return Err(format!(
            "`{name}` is not a valid package name: a name starts with a letter or `_` and has only letters, digits, \
             and `_`"
        ));
    }
    if burdin_syntax::token::TokenKind::keyword(name).is_some() {
        return Err(format!("`{name}` is a reserved word and cannot name a package"));
    }
    if name == "std" {
        return Err("`std` is the name of the standard library and cannot name a package".to_string());
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fresh directory under the system temporary directory, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Scratch {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "burdin-new-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn creates_the_three_files() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("app");
        assert_eq!(create(&dir).unwrap(), "app");
        assert_eq!(std::fs::read_to_string(dir.join("src/main.bd")).unwrap(), MAIN);
        assert_eq!(std::fs::read_to_string(dir.join(".gitignore")).unwrap(), "/target/\n/app\n/app.exe\n");
        let manifest = std::fs::read_to_string(dir.join("burdin.toml")).unwrap();
        assert_eq!(manifest, "[package]\nname = \"app\"\nversion = \"0.1.0\"\n");
    }

    #[test]
    fn the_manifest_is_one_the_loader_accepts() {
        let text = manifest("my_app2");
        let parsed = crate::package::parse_manifest_text(&text, Path::new("burdin.toml")).unwrap();
        assert_eq!(parsed.name, "my_app2");
    }

    #[test]
    fn the_template_program_parses_without_errors() {
        let mut diagnostics = burdin_diag::Diagnostics::default();
        let mut sources = burdin_diag::SourceMap::default();
        let file = sources.add("main.bd".to_string(), MAIN.to_string());
        burdin_syntax::parse(file, MAIN, &mut diagnostics);
        assert!(!diagnostics.has_errors());
    }

    #[test]
    fn a_nested_path_creates_its_parents_and_names_the_package_after_the_last_component() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("tools").join("cli");
        assert_eq!(create(&dir).unwrap(), "cli");
        assert!(dir.join("burdin.toml").is_file());
    }

    #[test]
    fn an_existing_directory_is_not_overwritten() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("app");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.bd"), "mine").unwrap();
        let error = create(&dir).unwrap_err();
        assert!(error.contains("already exists"), "{error}");
        assert_eq!(std::fs::read_to_string(dir.join("src/main.bd")).unwrap(), "mine");
        assert!(!dir.join("burdin.toml").exists());
    }

    #[test]
    fn an_existing_empty_directory_and_an_existing_file_are_refused() {
        let scratch = Scratch::new();
        let empty = scratch.0.join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(create(&empty).unwrap_err().contains("already exists"));
        let file = scratch.0.join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(create(&file).unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "x");
    }

    #[test]
    fn invalid_names_are_rejected_before_anything_is_created() {
        let scratch = Scratch::new();
        for bad in ["my-app", "2fast", "a b", "fn", "std", "match"] {
            let dir = scratch.0.join(bad);
            assert!(create(&dir).is_err(), "{bad}");
            assert!(!dir.exists(), "{bad}");
        }
        assert!(create(Path::new("..")).is_err());
        assert!(create(Path::new("")).is_err());
    }
}
