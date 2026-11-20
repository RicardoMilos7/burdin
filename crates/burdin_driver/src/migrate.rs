//! `burdin fix --migrate` (`spec/fix.md`, FX10): after the fixes of the deprecations the package's edition reports,
//! the package moves to the next edition. The new manifest is written only when the fixed package checks under the
//! new edition, and together with the fixed files, so a migration is one reviewable change.

use crate::lint::lint_once;
use crate::{CompileOptions, SourceOverrides, render_options};
use burdin_query::Edition;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// What a migration moves: the manifest, its text, and the editions.
pub(crate) struct Plan {
    pub(crate) manifest: PathBuf,
    pub(crate) text: String,
    pub(crate) from: Edition,
    pub(crate) to: Edition,
}

/// Why a migration could not be planned: a command-line mistake (exit status 2) or a package that cannot move
/// (exit status 1).
pub(crate) enum Refusal {
    Usage(String),
    Error(String),
}

/// Reads the package's manifest and decides the edition it moves to: the one after its own.
pub(crate) fn plan(target: &Path) -> Result<Plan, Refusal> {
    if !target.join("burdin.toml").is_file() {
        return Err(Refusal::Usage(format!(
            "`--migrate` needs a package directory with a burdin.toml, where the edition is recorded; {} is a \
             one-file target, which is always edition {}",
            crate::source_path_text(target),
            Edition::default().as_str()
        )));
    }
    let dir = target.canonicalize().map_err(|e| Refusal::Usage(format!("cannot open {}: {e}", target.display())))?;
    let manifest = dir.join("burdin.toml");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| Refusal::Usage(format!("cannot read {}: {e}", manifest.display())))?;
    // The edition a workspace sets is the workspace's to move, and `--migrate` cannot move it (WS15).
    if burdin_query::package::edition_is_inherited(&text, &manifest).map_err(Refusal::Usage)? {
        return Err(Refusal::Error(format!(
            "the edition of {} is inherited from its workspace (`edition.workspace = true`), and `--migrate` cannot \
             move an inherited edition; set the edition in the workspace's `[workspace.package]` instead",
            crate::source_path_text(&manifest)
        )));
    }
    let from = burdin_query::package::parse_manifest_text(&text, &manifest).map_err(Refusal::Usage)?.edition;
    let to = from.next().unwrap_or(from);
    Ok(Plan { manifest, text, from, to })
}

/// `text` with `[package] edition` set to `to`: the existing line's value is replaced, or a line is added after the
/// last key of the `[package]` section.
pub(crate) fn set_edition(text: &str, to: Edition) -> String {
    let line = format!("edition = \"{}\"", to.as_str());
    let mut out: Vec<String> = Vec::new();
    let mut section = String::new();
    let mut last_package_key: Option<usize> = None;
    let mut replaced = false;
    for raw in text.lines() {
        let trimmed = burdin_query::package::strip_comment(raw).trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed[1..trimmed.len() - 1].trim().to_string();
        } else if section == "package" && trimmed.contains('=') {
            let key = trimmed.split('=').next().unwrap_or("").trim();
            if key == "edition" {
                out.push(line.clone());
                replaced = true;
                last_package_key = Some(out.len() - 1);
                continue;
            }
            last_package_key = Some(out.len());
        }
        out.push(raw.to_string());
    }
    if !replaced {
        match last_package_key {
            Some(at) => out.insert(at + 1, line),
            None => {
                out.insert(0, line);
                out.insert(0, "[package]".to_string());
            }
        }
    }
    let mut result = out.join("\n");
    if text.ends_with('\n') || text.is_empty() {
        result.push('\n');
    }
    result
}

/// Checks the fixed package under the new edition and returns the manifest change, or `None` when the package is
/// already at the newest edition. On errors, they are printed and nothing is to be written.
pub(crate) fn finish(
    plan: &Plan,
    target: &Path,
    options: &CompileOptions,
    changes: &[(PathBuf, String, String)],
) -> Result<Option<(PathBuf, String, String)>, ExitCode> {
    if plan.to == plan.from {
        eprintln!("note: the package is already at edition {}, the newest this compiler knows", plan.from.as_str());
        return Ok(None);
    }
    let new_text = set_edition(&plan.text, plan.to);
    let mut overrides: SourceOverrides = changes.iter().map(|(path, _, new)| (path.clone(), new.clone())).collect();
    overrides.insert(plan.manifest.clone(), new_text.clone());
    let outcome = lint_once(target, options, &overrides, &[])?;
    if outcome.front.diags.has_errors() {
        let render = render_options(options.diagnostics, std::io::stderr().is_terminal());
        eprint!("{}", outcome.front.diags.render_with(&outcome.front.sources, &render).text);
        eprintln!(
            "error: the package does not compile under edition {} after the fixes; no files were changed",
            plan.to.as_str()
        );
        return Err(ExitCode::from(1));
    }
    Ok(Some((plan.manifest.clone(), plan.text.clone(), new_text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_edition_line_is_replaced_or_added_to_the_package_section() {
        let text = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n";
        assert_eq!(
            set_edition(text, Edition::E2026),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[dependencies]\n"
        );
        let text = "[package]\nname = \"app\"\nedition = \"2024\" # old\n";
        assert_eq!(set_edition(text, Edition::E2026), "[package]\nname = \"app\"\nedition = \"2026\"\n");
    }
}
