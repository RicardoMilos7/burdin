//! `burdin new <name>` and `burdin init`: create a package with a `burdin.toml`, a root module, and a `.gitignore` for
//! the build output. The root module is `src/main.bd` (a program that prints a greeting) or, with `--lib`,
//! `src/lib.bd` (a library with one tested function: `burdin test` runs it, and other packages reach it as
//! `use <name>`).
//!
//! `new` creates the package directory and refuses any path that already exists. `init` writes into an existing
//! directory, which may hold other files, and refuses only when a file it would create is already there. Both refuse
//! before anything is written, and a failed write removes what that run created.

use crate::package::is_ident;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// `main.bd`: the program `burdin run` prints a greeting for.
const MAIN: &str = "fn main() {\n    println(\"hello, world\")\n}\n";

/// `lib.bd`: the root module of a library. Other packages reach it as `use <name>`, and `burdin test` runs its
/// tests.
const LIB: &str = r#"// The root module of the library: other packages reach it as `use <name>`, and `burdin test` runs the
// tests below.

/// Adds two numbers.
fn add(a: i32, b: i32): i32 {
    a + b
}

test "add works" {
    assert_eq(add(2, 3), 5)?
}
"#;

/// The two package shapes: a program with `src/main.bd` (the default) and a library with `src/lib.bd`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Bin,
    Lib,
}

impl Kind {
    /// The root module, in `dir`.
    fn root_path(self, dir: &Path) -> PathBuf {
        match self {
            Kind::Bin => dir.join("src").join("main.bd"),
            Kind::Lib => dir.join("src").join("lib.bd"),
        }
    }

    fn source(self) -> &'static str {
        match self {
            Kind::Bin => MAIN,
            Kind::Lib => LIB,
        }
    }

    /// What the command reports: `package`, or `library package` for `--lib`.
    fn noun(self) -> &'static str {
        match self {
            Kind::Bin => "package",
            Kind::Lib => "library package",
        }
    }

    /// The `.gitignore`: the build output under `target/`, and for a program the executable `burdin build .` leaves
    /// in the package directory (`name`, or `name.exe` on Windows). A library builds no executable.
    fn gitignore(self, name: &str) -> String {
        match self {
            Kind::Bin => format!("/target/\n/{name}\n/{name}.exe\n"),
            Kind::Lib => "/target/\n".to_string(),
        }
    }
}

/// Splits the arguments of `new` and `init` into the package kind (a program unless `--lib`) and the positional
/// arguments. `--lib` and `--bin` together are a usage error.
fn parse_args(args: &[String]) -> Result<(Kind, Vec<&str>), String> {
    let mut kind: Option<Kind> = None;
    let mut positional = Vec::new();
    for arg in args {
        let this = match arg.as_str() {
            "--lib" => Kind::Lib,
            "--bin" => Kind::Bin,
            option if option.starts_with('-') => return Err(format!("unknown option `{option}`")),
            _ => {
                positional.push(arg.as_str());
                continue;
            }
        };
        if kind.is_some_and(|previous| previous != this) {
            return Err("`--lib` and `--bin` cannot be used together".to_string());
        }
        kind = Some(this);
    }
    Ok((kind.unwrap_or(Kind::Bin), positional))
}

/// `burdin new <name> [--lib | --bin]`. Exit status 2 for a usage error, an invalid name, an existing path, or an I/O
/// failure, the same as the other subcommands.
pub(crate) fn run(args: &[String]) -> ExitCode {
    let (kind, positional) = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(message) => return crate::cli::usage_error("new", &message),
    };
    let target = match positional.as_slice() {
        [target] => *target,
        [] => return crate::cli::usage_error("new", "`new` needs the name of the package to create"),
        [_, extra, ..] => return crate::cli::usage_error("new", &format!("unexpected argument `{extra}`")),
    };
    match create(Path::new(target), kind) {
        Ok(name) => {
            println!("Created {} `{name}` in {target}", kind.noun());
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

/// `burdin init [<dir>] [--lib | --bin]`: writes the package into the existing directory `dir` (the working directory
/// by default). Exit status 2 for a usage error, a directory that does not exist, a file it would create that is
/// already there, an invalid name, or an I/O failure.
pub(crate) fn run_init(args: &[String]) -> ExitCode {
    let (kind, positional) = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(message) => return crate::cli::usage_error("init", &message),
    };
    let target = match positional.as_slice() {
        [] => ".",
        [target] => *target,
        [_, extra, ..] => return crate::cli::usage_error("init", &format!("unexpected argument `{extra}`")),
    };
    match initialize(Path::new(target), kind) {
        Ok(name) => {
            println!("Initialized {} `{name}` in {target}", kind.noun());
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

/// Creates the package directory at `dir` and returns the package name, which is the last component of `dir`.
fn create(dir: &Path, kind: Kind) -> Result<String, String> {
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
    if let Err(message) = write_package(dir, &name, kind) {
        // The directory is the one just created, so removing it cannot lose anything else.
        let _ = std::fs::remove_dir_all(dir);
        return Err(message);
    }
    Ok(name)
}

/// Writes the package into the existing directory `dir` and returns the package name, which is the directory's real
/// name (so `burdin init ..` names the parent). Refuses, before writing anything, when `burdin.toml`, the root module,
/// or `.gitignore` already exists: other files in `dir` are left alone.
fn initialize(dir: &Path, kind: Kind) -> Result<String, String> {
    match std::fs::metadata(dir) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(format!("`{}` is not a directory", dir.display())),
        Err(_) => {
            return Err(format!(
                "`{}` does not exist; `burdin init` writes into an existing directory (`burdin new` creates one)",
                dir.display()
            ));
        }
    }
    let real = dir.canonicalize().map_err(|e| format!("cannot open {}: {e}", dir.display()))?;
    let name = package_name(&real)?;
    for path in [dir.join("burdin.toml"), kind.root_path(dir), dir.join(".gitignore")] {
        if std::fs::symlink_metadata(&path).is_ok() {
            return Err(format!("`{}` already exists; `burdin init` does not overwrite it", path.display()));
        }
    }
    write_package(dir, &name, kind)?;
    Ok(name)
}

/// Writes the package's files into the existing directory `dir`. A failed write removes the files and the `src`
/// directory this call created, and nothing else: a file is only removed if this call created it.
fn write_package(dir: &Path, name: &str, kind: Kind) -> Result<(), String> {
    let mut created = Vec::new();
    let result = write_files(dir, name, kind, &mut created);
    if result.is_err() {
        for path in created.iter().rev() {
            let _ = if path.is_dir() { std::fs::remove_dir(path) } else { std::fs::remove_file(path) };
        }
    }
    result
}

fn write_files(dir: &Path, name: &str, kind: Kind, created: &mut Vec<PathBuf>) -> Result<(), String> {
    let src = dir.join("src");
    if !src.is_dir() {
        std::fs::create_dir(&src).map_err(|e| format!("cannot create {}: {e}", src.display()))?;
        created.push(src);
    }
    let files = [
        (dir.join("burdin.toml"), manifest(name)),
        (kind.root_path(dir), kind.source().to_string()),
        (dir.join(".gitignore"), kind.gitignore(name)),
    ];
    for (path, text) in files {
        // `create_new`: a file that appeared since the checks is an error, never something to overwrite.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        created.push(path.clone());
        file.write_all(text.as_bytes()).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok(())
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

    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn creates_the_three_files() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("app");
        assert_eq!(create(&dir, Kind::Bin).unwrap(), "app");
        assert_eq!(std::fs::read_to_string(dir.join("src/main.bd")).unwrap(), MAIN);
        assert_eq!(std::fs::read_to_string(dir.join(".gitignore")).unwrap(), "/target/\n/app\n/app.exe\n");
        let manifest = std::fs::read_to_string(dir.join("burdin.toml")).unwrap();
        assert_eq!(manifest, "[package]\nname = \"app\"\nversion = \"0.1.0\"\n");
    }

    #[test]
    fn a_library_has_src_lib_bd_and_no_main() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("greet");
        assert_eq!(create(&dir, Kind::Lib).unwrap(), "greet");
        assert_eq!(std::fs::read_to_string(dir.join("src/lib.bd")).unwrap(), LIB);
        assert!(!dir.join("src/main.bd").exists());
        assert_eq!(std::fs::read_to_string(dir.join(".gitignore")).unwrap(), "/target/\n");
        assert!(dir.join("burdin.toml").is_file());
    }

    #[test]
    fn the_manifest_is_one_the_loader_accepts() {
        let text = manifest("my_app2");
        let parsed = crate::package::parse_manifest_text(&text, Path::new("burdin.toml")).unwrap();
        assert_eq!(parsed.name, "my_app2");
    }

    #[test]
    fn the_templates_parse_without_errors() {
        for text in [MAIN, LIB] {
            let mut diagnostics = burdin_diag::Diagnostics::default();
            let mut sources = burdin_diag::SourceMap::default();
            let file = sources.add("main.bd".to_string(), text.to_string());
            burdin_syntax::parse(file, text, &mut diagnostics);
            assert!(!diagnostics.has_errors(), "{text}");
        }
    }

    #[test]
    fn a_nested_path_creates_its_parents_and_names_the_package_after_the_last_component() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("tools").join("cli");
        assert_eq!(create(&dir, Kind::Bin).unwrap(), "cli");
        assert!(dir.join("burdin.toml").is_file());
    }

    #[test]
    fn an_existing_directory_is_not_overwritten() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("app");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.bd"), "mine").unwrap();
        let error = create(&dir, Kind::Bin).unwrap_err();
        assert!(error.contains("already exists"), "{error}");
        assert_eq!(std::fs::read_to_string(dir.join("src/main.bd")).unwrap(), "mine");
        assert!(!dir.join("burdin.toml").exists());
    }

    #[test]
    fn an_existing_empty_directory_and_an_existing_file_are_refused() {
        let scratch = Scratch::new();
        let empty = scratch.0.join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(create(&empty, Kind::Bin).unwrap_err().contains("already exists"));
        let file = scratch.0.join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(create(&file, Kind::Lib).unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "x");
    }

    #[test]
    fn invalid_names_are_rejected_before_anything_is_created() {
        let scratch = Scratch::new();
        for bad in ["my-app", "2fast", "a b", "fn", "std", "match"] {
            let dir = scratch.0.join(bad);
            assert!(create(&dir, Kind::Bin).is_err(), "{bad}");
            assert!(!dir.exists(), "{bad}");
        }
        assert!(create(Path::new(".."), Kind::Bin).is_err());
        assert!(create(Path::new(""), Kind::Lib).is_err());
    }

    #[test]
    fn the_kind_flags_choose_the_root_and_conflict() {
        assert_eq!(parse_args(&words(&["app"])).unwrap(), (Kind::Bin, vec!["app"]));
        assert_eq!(parse_args(&words(&["--lib", "app"])).unwrap(), (Kind::Lib, vec!["app"]));
        assert_eq!(parse_args(&words(&["app", "--bin"])).unwrap(), (Kind::Bin, vec!["app"]));
        assert_eq!(parse_args(&words(&["--lib", "--lib", "app"])).unwrap(), (Kind::Lib, vec!["app"]));
        let error = parse_args(&words(&["--lib", "--bin", "app"])).unwrap_err();
        assert_eq!(error, "`--lib` and `--bin` cannot be used together");
        assert_eq!(parse_args(&words(&["--bogus"])).unwrap_err(), "unknown option `--bogus`");
    }

    #[test]
    fn init_writes_into_an_existing_directory_and_keeps_its_other_files() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("app");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("README.md"), "notes\n").unwrap();
        std::fs::write(dir.join("src/helper.bd"), "fn helper() {}\n").unwrap();
        assert_eq!(initialize(&dir, Kind::Bin).unwrap(), "app");
        assert_eq!(std::fs::read_to_string(dir.join("README.md")).unwrap(), "notes\n");
        assert_eq!(std::fs::read_to_string(dir.join("src/helper.bd")).unwrap(), "fn helper() {}\n");
        assert_eq!(std::fs::read_to_string(dir.join("src/main.bd")).unwrap(), MAIN);
        assert_eq!(std::fs::read_to_string(dir.join(".gitignore")).unwrap(), "/target/\n/app\n/app.exe\n");
        assert!(dir.join("burdin.toml").is_file());
    }

    #[test]
    fn init_creates_src_when_it_is_missing_and_names_the_directory_it_is_given() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("outer").join("inner");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(initialize(&dir, Kind::Lib).unwrap(), "inner");
        assert_eq!(std::fs::read_to_string(dir.join("src/lib.bd")).unwrap(), LIB);
        // `..` names the parent by its real name, so the package here is `outer`.
        let elsewhere = scratch.0.join("outer").join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        assert_eq!(initialize(&elsewhere.join(".."), Kind::Bin).unwrap(), "outer");
        assert!(scratch.0.join("outer").join("burdin.toml").is_file());
    }

    #[test]
    fn init_refuses_each_file_it_would_create_and_writes_nothing() {
        let scratch = Scratch::new();
        for (index, (clash, kind)) in [
            ("burdin.toml", Kind::Bin),
            ("src/main.bd", Kind::Bin),
            ("src/lib.bd", Kind::Lib),
            (".gitignore", Kind::Lib),
        ]
        .into_iter()
        .enumerate()
        {
            // The directory is named `clash<n>`: a valid package name, so only the clash can be refused.
            let dir = scratch.0.join(format!("clash{index}"));
            std::fs::create_dir_all(dir.join("src")).unwrap();
            std::fs::write(dir.join(clash), "mine").unwrap();
            let error = initialize(&dir, kind).unwrap_err();
            assert!(error.contains("already exists") && error.contains(clash.rsplit('/').next().unwrap()), "{error}");
            assert_eq!(std::fs::read_to_string(dir.join(clash)).unwrap(), "mine");
            let mut left: Vec<String> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            left.sort();
            assert_eq!(left.len(), if clash.starts_with("src") { 1 } else { 2 }, "{clash}: {left:?}");
        }
    }

    #[test]
    fn a_program_and_a_library_root_do_not_clash() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("both");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.bd"), "fn f() {}\n").unwrap();
        assert_eq!(initialize(&dir, Kind::Bin).unwrap(), "both");
        assert!(dir.join("src/main.bd").is_file());
    }

    #[test]
    fn init_refuses_a_missing_directory_a_file_and_a_src_that_is_a_file_without_writing() {
        let scratch = Scratch::new();
        assert!(initialize(&scratch.0.join("missing"), Kind::Bin).unwrap_err().contains("does not exist"));
        let file = scratch.0.join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(initialize(&file, Kind::Bin).unwrap_err().contains("is not a directory"));
        let blocked = scratch.0.join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::write(blocked.join("src"), "not a directory").unwrap();
        assert!(initialize(&blocked, Kind::Bin).is_err());
        assert!(!blocked.join("burdin.toml").exists());
        assert!(!blocked.join(".gitignore").exists());
    }

    #[test]
    fn init_refuses_an_invalid_directory_name_before_writing() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("my-app");
        std::fs::create_dir(&dir).unwrap();
        assert!(initialize(&dir, Kind::Bin).unwrap_err().contains("not a valid package name"));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }
}
