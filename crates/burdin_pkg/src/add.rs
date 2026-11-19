//! `burdin add`: one dependency into `burdin.toml`, resolved, and `burdin.lock` brought up to date
//! (`spec/packages.md`, PKG11-PKG14).
//!
//! The command resolves the new dependency and the whole graph against the manifest it is about to write, and
//! writes nothing until that succeeded, so a failed `add` leaves both files as they were. The manifest is edited as
//! text: one line in `[dependencies]` is added or replaced and every other line, comment, and blank line stays.

use crate::git;
use crate::resolve;
use burdin_query::lockfile::{self, GitRef, quote};
use burdin_query::package::{self, is_ident, source_path_text};
use std::path::{Component, Path, PathBuf};

/// Where the new dependency comes from, as the command line gave it.
pub enum Source {
    /// A directory, relative to the working directory or absolute.
    Path(PathBuf),
    Git {
        url: String,
        reference: GitRef,
    },
}

pub struct Request {
    /// The directory of the `burdin.toml` to edit.
    pub manifest_dir: PathBuf,
    /// The name `use` reaches the dependency by; `None` takes the dependency's package name.
    pub name: Option<String>,
    pub source: Source,
}

/// What `add` did, for the command's report.
#[derive(Debug)]
pub struct Added {
    pub name: String,
    /// The manifest already had a dependency by this name, and its line was replaced.
    pub replaced: bool,
    /// The source as the report shows it: `path ../lib` or `git <url>, tag v1, commit 1a2b3c4d5e6f`.
    pub source: String,
}

pub fn add(request: &Request) -> Result<Added, String> {
    let manifest_dir = request
        .manifest_dir
        .canonicalize()
        .map_err(|e| format!("cannot open {}: {e}", request.manifest_dir.display()))?;
    let manifest_path = manifest_dir.join("burdin.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("cannot read {}: {e}", source_path_text(&manifest_path)))?;
    let manifest = package::parse_manifest_text(&text, &manifest_path)?;

    // The dependency itself: where it is, its package name, and the manifest entry that names it.
    let (dep_dir, value, shown, fresh_commit) = match &request.source {
        Source::Path(path) => {
            let dep_dir = path.canonicalize().map_err(|e| format!("cannot open {}: {e}", path.display()))?;
            let relative = relative_path(&manifest_dir, &dep_dir);
            let value = format!("{{ path = {} }}", quote(&relative));
            (dep_dir, value, format!("path {relative}"), None)
        }
        Source::Git { url, reference } => {
            let commit = git::resolve(url, reference)?;
            let dep_dir = git::checkout(url, &commit)?;
            let mut value = format!("{{ git = {}", quote(url));
            let mut shown = format!("git {url}");
            if let Some((key, ref_value)) = reference.key_value() {
                value.push_str(&format!(", {key} = {}", quote(ref_value)));
                shown.push_str(&format!(", {key} {ref_value}"));
            }
            value.push_str(" }");
            shown.push_str(&format!(", commit {}", &commit[..12.min(commit.len())]));
            (dep_dir, value, shown, Some(commit))
        }
    };
    let dep_manifest = dep_dir.join("burdin.toml");
    if !dep_manifest.is_file() {
        return Err(format!("{} is not a Burdin package: it has no burdin.toml", source_path_text(&dep_dir)));
    }
    let dep_name = package::parse_manifest(&dep_manifest)?.name;
    let name = request.name.clone().unwrap_or(dep_name);
    check_name(&name)?;
    if dep_dir == manifest_dir {
        return Err(format!("package `{}` cannot depend on itself", manifest.name));
    }

    let (new_text, replaced) = set_dependency(&text, &name, &value);
    // Resolve the whole graph against the new manifest before writing either file. Every commit the lockfile pins
    // stays, except the added dependency's: it builds the commit just resolved, so adding a branch again moves it
    // to the branch's head (PKG13).
    let mut existing = lockfile::read(&manifest_dir)?;
    if let (Source::Git { url, reference }, Some(lock), Some(commit)) =
        (&request.source, existing.as_mut(), &fresh_commit)
    {
        for package in &mut lock.packages {
            if let Some(git) = package.git.as_mut().filter(|g| &g.url == url && &g.reference == reference) {
                git.commit = commit.clone();
                package.checksum = None;
            }
        }
    }
    let resolution = resolve::resolve(&manifest_dir, Some(&new_text), existing.as_ref(), false)?;
    std::fs::write(&manifest_path, &new_text)
        .map_err(|e| format!("cannot write {}: {e}", source_path_text(&manifest_path)))?;
    resolve::write(&manifest_dir, &resolution.lock)?;
    Ok(Added { name, replaced, source: shown })
}

/// The `use` name of a dependency: an identifier that is not a reserved word and not `std`.
fn check_name(name: &str) -> Result<(), String> {
    if !is_ident(name) {
        return Err(format!(
            "`{name}` is not a valid dependency name: a name starts with a letter or `_` and has only letters, \
             digits, and `_`"
        ));
    }
    if burdin_syntax::token::TokenKind::keyword(name).is_some() {
        return Err(format!("`{name}` is a reserved word and cannot name a dependency"));
    }
    if name == "std" {
        return Err("`std` is the name of the standard library and cannot name a dependency".to_string());
    }
    Ok(())
}

/// `to` relative to `from`, both canonical, with `/` separators. When there is no relative path (another drive on
/// Windows), the absolute path.
fn relative_path(from: &Path, to: &Path) -> String {
    let from: Vec<Component> = from.components().collect();
    let to_parts: Vec<Component> = to.components().collect();
    if from.first() != to_parts.first() {
        return source_path_text(to).replace('\\', "/");
    }
    let common = from.iter().zip(&to_parts).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_string(); from.len() - common];
    parts.extend(to_parts[common..].iter().map(|c| c.as_os_str().to_string_lossy().to_string()));
    if parts.is_empty() { ".".to_string() } else { parts.join("/") }
}

/// The manifest `text` with `name = value` in `[dependencies]`: the line for `name` replaced when there is one,
/// otherwise a new line after the section's last entry, or a new section at the end. Every other line is kept, and
/// the file's line ending (`\n` or `\r\n`) is used. Returns the text and whether a line was replaced.
pub fn set_dependency(text: &str, name: &str, value: &str) -> (String, bool) {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let entry = format!("{name} = {value}{newline}");
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    let header = |line: &str| {
        let line = package_line(line);
        line.starts_with('[').then(|| line.trim_start_matches('[').trim_end_matches(']').trim().to_string())
    };
    let Some(start) = lines.iter().position(|line| header(line).as_deref() == Some("dependencies")) else {
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(newline);
        }
        if !out.trim().is_empty() {
            out.push_str(newline);
        }
        out.push_str(&format!("[dependencies]{newline}"));
        out.push_str(&entry);
        return (out, false);
    };
    let end = lines[start + 1..].iter().position(|line| header(line).is_some()).map_or(lines.len(), |i| start + 1 + i);
    for line in &mut lines[start + 1..end] {
        let key = package_line(line).split_once('=').map(|(key, _)| key.trim().to_string());
        if key.as_deref() == Some(name) {
            *line = entry;
            return (lines.concat(), true);
        }
    }
    // After the last entry of the section, so blank lines and comments before the next section stay where they are.
    let last_entry = (start + 1..end).rev().find(|&i| !package_line(&lines[i]).is_empty()).unwrap_or(start);
    if !lines[last_entry].ends_with('\n') {
        lines[last_entry].push_str(newline);
    }
    lines.insert(last_entry + 1, entry);
    (lines.concat(), false)
}

/// A manifest line without its comment and surrounding space.
fn package_line(line: &str) -> &str {
    package::strip_comment(line).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_section_is_appended() {
        let (text, replaced) = set_dependency("[package]\nname = \"app\"\n", "lib", "{ path = \"../lib\" }");
        assert_eq!(text, "[package]\nname = \"app\"\n\n[dependencies]\nlib = { path = \"../lib\" }\n");
        assert!(!replaced);
        let (text, _) = set_dependency("[package]\nname = \"app\"", "lib", "\"../lib\"");
        assert_eq!(text, "[package]\nname = \"app\"\n\n[dependencies]\nlib = \"../lib\"\n");
    }

    #[test]
    fn an_entry_goes_after_the_last_one_and_keeps_comments() {
        let before = "[package]\nname = \"app\"\n\n[dependencies]\n# mine\na = \"../a\" # first\n\n# lint\n[lint]\n";
        let (text, replaced) = set_dependency(before, "b", "\"../b\"");
        assert_eq!(
            text,
            "[package]\nname = \"app\"\n\n[dependencies]\n# mine\na = \"../a\" # first\nb = \"../b\"\n\n# lint\n[lint]\n"
        );
        assert!(!replaced);
    }

    #[test]
    fn an_existing_entry_is_replaced_in_place() {
        let before = "[dependencies]\na = \"../a\"\nb = \"../b\"\n[build]\n";
        let (text, replaced) = set_dependency(before, "a", "{ git = \"x\" }");
        assert_eq!(text, "[dependencies]\na = { git = \"x\" }\nb = \"../b\"\n[build]\n");
        assert!(replaced);
    }

    #[test]
    fn crlf_files_stay_crlf() {
        let (text, _) = set_dependency("[package]\r\nname = \"app\"\r\n[dependencies]\r\n", "a", "\"../a\"");
        assert_eq!(text, "[package]\r\nname = \"app\"\r\n[dependencies]\r\na = \"../a\"\r\n");
    }

    #[test]
    fn relative_paths_use_forward_slashes() {
        let base = std::env::temp_dir();
        assert_eq!(relative_path(&base.join("a").join("app"), &base.join("a").join("lib")), "../lib");
        assert_eq!(relative_path(&base.join("app"), &base.join("app").join("vendor").join("x")), "vendor/x");
        assert_eq!(relative_path(&base, &base), ".");
    }

    #[test]
    fn dependency_names_are_checked() {
        assert!(check_name("json").is_ok());
        assert!(check_name("my-lib").unwrap_err().contains("not a valid"));
        assert!(check_name("fn").unwrap_err().contains("reserved word"));
        assert!(check_name("std").unwrap_err().contains("standard library"));
    }
}
