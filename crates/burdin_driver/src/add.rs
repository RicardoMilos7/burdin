//! `burdin add`: the command line of `burdin_pkg::add` (`spec/packages.md`, PKG11-PKG14).

use burdin_pkg::add::{Request, Source};
use burdin_query::lockfile::GitRef;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub(crate) const ADD_USAGE: &str = "\
usage:
  burdin add [<name>] --path <dir> [--manifest-path <path>]
  burdin add [<name>] --git <url> [--branch <branch> | --tag <tag> | --rev <commit>] [--manifest-path <path>]

  <name>                  the name `use` reaches the dependency by (default: the dependency's package name)
  --path <dir>            a package directory, relative to the working directory
  --git <url>             a git repository whose root is a package; --branch, --tag, or --rev picks the commit
                          (default: the repository's default branch), and burdin.lock pins it
  --manifest-path <path>  the burdin.toml to edit, or its directory (default: the nearest burdin.toml at or above
                          the working directory)";

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
    let args = match parse(argv) {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{ADD_USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("error: {message}\n{ADD_USAGE}");
            return ExitCode::from(2);
        }
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
        _ => unreachable!("parse requires exactly one source"),
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

/// The arguments, or `None` for `--help`.
fn parse(argv: &[String]) -> Result<Option<AddArgs>, String> {
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
        (None, None) => {
            return Err(match &args.name {
                Some(name) => format!(
                    "there is no package registry yet, so `{name}` cannot be looked up by name: give `--path <dir>` \
                     or `--git <url>`"
                ),
                None => "`add` needs a dependency: `--path <dir>` or `--git <url>`".to_string(),
            });
        }
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

    #[test]
    fn the_forms_parse() {
        let args = parse(&argv(&["json", "--git", "https://x/json.git", "--tag=v1"])).unwrap().unwrap();
        assert_eq!(args.name.as_deref(), Some("json"));
        assert_eq!(args.git.as_deref(), Some("https://x/json.git"));
        assert_eq!(args.references, [GitRef::Tag("v1".into())]);
        let args = parse(&argv(&["--path", "../lib", "--manifest-path", "app"])).unwrap().unwrap();
        assert_eq!(args.path, Some(PathBuf::from("../lib")));
        assert_eq!(args.manifest, Some(PathBuf::from("app")));
        assert!(parse(&argv(&["--help"])).unwrap().is_none());
    }

    #[test]
    fn usage_errors() {
        assert!(parse(&argv(&["json"])).unwrap_err().contains("no package registry"));
        assert!(parse(&argv(&[])).unwrap_err().contains("needs a dependency"));
        assert!(parse(&argv(&["--path", "a", "--git", "b"])).unwrap_err().contains("not both"));
        assert!(parse(&argv(&["--git", "b", "--tag", "1", "--rev", "2"])).unwrap_err().contains("at most one"));
        assert!(parse(&argv(&["--path", "a", "--branch", "main"])).unwrap_err().contains("only to `--git`"));
        assert!(parse(&argv(&["--path"])).unwrap_err().contains("needs a directory"));
        assert!(parse(&argv(&["a", "b", "--path", "x"])).unwrap_err().contains("unexpected argument"));
        assert!(parse(&argv(&["--bogus"])).unwrap_err().contains("unknown option"));
    }
}
