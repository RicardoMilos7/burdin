//! `burdin add`: the command line of `burdin_pkg::add` (`spec/packages.md`, PKG11-PKG14).

use burdin_pkg::add::{Request, Source};
use burdin_query::lockfile::GitRef;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Debug, Default)]
struct AddArgs {
    name: Option<String>,
    path: Option<PathBuf>,
    git: Option<String>,
    references: Vec<GitRef>,
    manifest: Option<PathBuf>,
}

/// `burdin add`. Exit status 2 for a usage error, 1 when the dependency cannot be added; on failure neither
/// `burdin.toml` nor `burdin.lock` changes.
pub(crate) fn run(argv: &[String]) -> ExitCode {
    let official = burdin_query::package::official_names();
    let args = match parse(argv, &official) {
        Ok(Some(args)) => args,
        Ok(None) => {
            crate::cli::print_help("add");
            return ExitCode::SUCCESS;
        }
        Err(message) => return crate::cli::usage_error("add", &message),
    };
    let manifest_dir = match manifest_dir(args.manifest.as_deref()) {
        Ok(dir) => dir,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    };
    let source = match (args.path, args.git) {
        (Some(path), None) => Source::Path(path),
        (None, Some(url)) => {
            Source::Git { url, reference: args.references.into_iter().next().unwrap_or(GitRef::DefaultBranch) }
        }
        // Neither: `parse` checked that the name is an official package.
        (None, None) => Source::Official,
        (Some(_), Some(_)) => unreachable!("parse refuses both sources"),
    };
    let request = Request { manifest_dir, name: args.name, source };
    match burdin_pkg::add::add(&request) {
        Ok(added) => {
            let verb = if added.replaced { "Replaced" } else { "Added" };
            println!("{verb} dependency `{}` ({})", added.name, added.source);
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(1)
        }
    }
}

/// The arguments, or `None` for `--help`. `official` is the names of the official packages: a name given with
/// neither `--path` nor `--git` must be one of them.
fn parse(argv: &[String], official: &[String]) -> Result<Option<AddArgs>, String> {
    let mut args = AddArgs::default();
    let mut it = argv.iter();
    while let Some(raw) = it.next() {
        let (flag, inline) = match raw.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_string())),
            _ => (raw.as_str(), None),
        };
        let mut value = |what: &str| -> Result<String, String> {
            match inline.clone() {
                Some(value) => Ok(value),
                None => it.next().cloned().ok_or_else(|| format!("`{flag}` needs {what}")),
            }
        };
        match flag {
            "-h" | "--help" => return Ok(None),
            "--path" => args.path = Some(PathBuf::from(value("a directory")?)),
            "--git" => args.git = Some(value("a repository URL")?),
            "--branch" => args.references.push(GitRef::Branch(value("a branch name")?)),
            "--tag" => args.references.push(GitRef::Tag(value("a tag name")?)),
            "--rev" => args.references.push(GitRef::Rev(value("a commit")?)),
            "--manifest-path" => args.manifest = Some(PathBuf::from(value("a path")?)),
            option if option.starts_with('-') => return Err(format!("unknown option `{option}`")),
            _ if args.name.is_none() => args.name = Some(raw.clone()),
            _ => return Err(format!("unexpected argument `{raw}`")),
        }
    }
    match (&args.path, &args.git) {
        (Some(_), Some(_)) => return Err("give `--path` or `--git`, not both".to_string()),
        (None, None) => match &args.name {
            Some(name) if official.contains(name) => {}
            Some(name) => {
                let known = if official.is_empty() {
                    "this burdin has no official packages".to_string()
                } else {
                    format!("the official packages are: {}", official.join(", "))
                };
                return Err(format!(
                    "there is no package registry yet, so `{name}` cannot be looked up by name ({known}): give \
                     `--path <dir>` or `--git <url>`"
                ));
            }
            None => {
                return Err("`add` needs a dependency: `--path <dir>`, `--git <url>`, or the name of an official \
                            package"
                    .to_string());
            }
        },
        _ => {}
    }
    if args.references.len() > 1 {
        return Err("give at most one of `--branch`, `--tag`, and `--rev`".to_string());
    }
    if !args.references.is_empty() && args.git.is_none() {
        return Err("`--branch`, `--tag`, and `--rev` apply only to `--git`".to_string());
    }
    Ok(Some(args))
}

/// The directory of the manifest to edit: `--manifest-path` (a `burdin.toml` or its directory), or the nearest
/// directory at or above the working directory that has one.
fn manifest_dir(given: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(path) = given {
        let dir = if path.file_name().is_some_and(|n| n == "burdin.toml") {
            path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf()
        } else {
            path.to_path_buf()
        };
        if !dir.join("burdin.toml").is_file() {
            return Err(format!("{} has no burdin.toml", dir.display()));
        }
        return Ok(dir);
    }
    let cwd = std::env::current_dir().map_err(|e| format!("cannot read the working directory: {e}"))?;
    cwd.ancestors()
        .find(|dir| dir.join("burdin.toml").is_file())
        .map(Path::to_path_buf)
        .ok_or_else(|| "no burdin.toml in the working directory or above it; give `--manifest-path`".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// `parse` with no official packages, the case of a compiler whose `packages/` folder is empty.
    fn parse_plain(items: &[&str]) -> Result<Option<AddArgs>, String> {
        parse(&argv(items), &[])
    }

    #[test]
    fn the_forms_parse() {
        let args = parse_plain(&["json", "--git", "https://x/json.git", "--tag=v1"]).unwrap().unwrap();
        assert_eq!(args.name.as_deref(), Some("json"));
        assert_eq!(args.git.as_deref(), Some("https://x/json.git"));
        assert_eq!(args.references, [GitRef::Tag("v1".into())]);
        let args = parse_plain(&["--path", "../lib", "--manifest-path", "app"]).unwrap().unwrap();
        assert_eq!(args.path, Some(PathBuf::from("../lib")));
        assert_eq!(args.manifest, Some(PathBuf::from("app")));
        assert!(parse_plain(&["--help"]).unwrap().is_none());
    }

    #[test]
    fn a_bare_name_is_an_official_package_or_a_usage_error() {
        let official = ["json".to_string(), "toml".to_string()];
        let args = parse(&argv(&["json"]), &official).unwrap().unwrap();
        assert_eq!((args.name.as_deref(), args.path, args.git), (Some("json"), None, None));
        // Not official: the registry message names what is.
        let error = parse(&argv(&["yaml"]), &official).unwrap_err();
        assert!(
            error.contains("no package registry") && error.contains("the official packages are: json, toml"),
            "{error}"
        );
        assert!(parse_plain(&["json"]).unwrap_err().contains("has no official packages"));
        assert!(parse(&argv(&["json@1.0"]), &official).unwrap_err().contains("no package registry"));
        // An official name takes no git reference.
        assert!(parse(&argv(&["json", "--tag", "v1"]), &official).unwrap_err().contains("only to `--git`"));
        // With a source, the name is just the `use` name.
        assert!(parse(&argv(&["yaml", "--path", "../y"]), &official).is_ok());
    }

    #[test]
    fn usage_errors() {
        assert!(parse_plain(&["json"]).unwrap_err().contains("no package registry"));
        assert!(parse_plain(&[]).unwrap_err().contains("needs a dependency"));
        assert!(parse_plain(&["--path", "a", "--git", "b"]).unwrap_err().contains("not both"));
        assert!(parse_plain(&["--git", "b", "--tag", "1", "--rev", "2"]).unwrap_err().contains("at most one"));
        assert!(parse_plain(&["--path", "a", "--branch", "main"]).unwrap_err().contains("only to `--git`"));
        assert!(parse_plain(&["--path"]).unwrap_err().contains("needs a directory"));
        assert!(parse_plain(&["a", "b", "--path", "x"]).unwrap_err().contains("unexpected argument"));
        assert!(parse_plain(&["--bogus"]).unwrap_err().contains("unknown option"));
    }
}
