//! Which toolchain a command uses (`spec/toolchain.md` TC7-TC9): `+toolchain` on the command line, then
//! `BURDIN_TOOLCHAIN`, then the nearest `burdin-toolchain.toml`, then the default.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::home::{Home, io_error};
use crate::install::check_component;
use crate::toml::{self, Value};
use crate::version::{TARGETS, ToolchainName};

pub const FILE_NAME: &str = "burdin-toolchain.toml";

/// A project's pinned toolchain (TC8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolchainFile {
    pub path: PathBuf,
    pub channel: ToolchainName,
    pub components: Vec<String>,
    pub targets: Vec<String>,
}

impl ToolchainFile {
    pub fn read(path: &Path) -> Result<ToolchainFile, String> {
        let text = fs::read_to_string(path).map_err(|e| io_error("read", path, e))?;
        ToolchainFile::parse(path, &text)
    }

    pub fn parse(path: &Path, text: &str) -> Result<ToolchainFile, String> {
        let at = |message: String| format!("{}: {message}", path.display());
        let entries = toml::parse(text).map_err(at)?;
        let mut channel = None;
        let mut components = Vec::new();
        let mut targets = Vec::new();
        for entry in entries {
            let line = entry.line;
            if entry.table != "toolchain" {
                let place = if entry.table.is_empty() {
                    "outside a table".to_string()
                } else {
                    format!("in [{}]", entry.table)
                };
                return Err(at(format!("line {line}: `{}` {place}; the file has only a [toolchain] table", entry.key)));
            }
            match (entry.key.as_str(), entry.value) {
                ("channel", Value::Str(name)) => {
                    channel = Some(ToolchainName::parse(&name).map_err(|e| at(format!("line {line}: {e}")))?)
                }
                ("components", Value::List(list)) => {
                    for component in &list {
                        check_component(component).map_err(|e| at(format!("line {line}: {e}")))?;
                    }
                    components = list;
                }
                ("targets", Value::List(list)) => {
                    for target in &list {
                        if !TARGETS.contains(&target.as_str()) {
                            return Err(at(format!("line {line}: `{target}` is not one of the six release targets")));
                        }
                    }
                    targets = list;
                }
                ("channel", _) => return Err(at(format!("line {line}: `channel` must be a string"))),
                ("components" | "targets", _) => {
                    return Err(at(format!("line {line}: `{}` must be an array of strings", entry.key)));
                }
                (key, _) => {
                    return Err(at(format!(
                        "line {line}: unknown key `{key}` (the keys are channel, components, and targets)"
                    )));
                }
            }
        }
        let channel = channel.ok_or_else(|| at("the [toolchain] table has no `channel`".to_string()))?;
        Ok(ToolchainFile { path: path.to_path_buf(), channel, components, targets })
    }
}

/// The nearest `burdin-toolchain.toml` in `start` or a directory above it.
pub fn find_file(start: &Path) -> Option<PathBuf> {
    start.ancestors().map(|dir| dir.join(FILE_NAME)).find(|path| path.is_file())
}

/// Why a toolchain was selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    CommandLine,
    Environment,
    File(ToolchainFile),
    Default,
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reason::CommandLine => f.write_str("the +toolchain argument on the command line"),
            Reason::Environment => f.write_str("the BURDIN_TOOLCHAIN environment variable"),
            Reason::File(file) => write!(f, "{}", file.path.display()),
            Reason::Default => f.write_str("the default toolchain"),
        }
    }
}

/// The selected toolchain and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub name: ToolchainName,
    pub reason: Reason,
}

/// Selects the toolchain for a command run in `cwd` (TC7). `None` when nothing selects one.
pub fn select(home: &Home, command_line: Option<&str>, cwd: &Path) -> Result<Option<Selection>, String> {
    if let Some(text) = command_line {
        let name = ToolchainName::parse(text).map_err(|e| format!("+{text}: {e}"))?;
        return Ok(Some(Selection { name, reason: Reason::CommandLine }));
    }
    if let Ok(text) = std::env::var("BURDIN_TOOLCHAIN")
        && !text.is_empty()
    {
        let name = ToolchainName::parse(&text).map_err(|e| format!("BURDIN_TOOLCHAIN: {e}"))?;
        return Ok(Some(Selection { name, reason: Reason::Environment }));
    }
    if let Some(path) = find_file(cwd) {
        let file = ToolchainFile::read(&path)?;
        return Ok(Some(Selection { name: file.channel.clone(), reason: Reason::File(file) }));
    }
    Ok(home.default_toolchain()?.map(|name| Selection { name, reason: Reason::Default }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<ToolchainFile, String> {
        ToolchainFile::parse(Path::new("p/burdin-toolchain.toml"), text)
    }

    #[test]
    fn reads_a_toolchain_file() {
        let file = parse(
            "[toolchain]\nchannel = \"0.4.1\"\ncomponents = [\"lsp\", \"std-src\"]\ntargets = [\"x86_64-unknown-linux-gnu\"]\n",
        )
        .unwrap();
        assert_eq!(file.channel.to_string(), "0.4.1");
        assert_eq!(file.components, ["lsp", "std-src"]);
        assert_eq!(file.targets, ["x86_64-unknown-linux-gnu"]);
    }

    #[test]
    fn rejects_bad_toolchain_files() {
        let error = |text: &str| parse(text).unwrap_err();
        assert_eq!(error("[toolchain]\n"), "p/burdin-toolchain.toml: the [toolchain] table has no `channel`");
        assert!(
            error("[toolchain]\nchannel = \"beta\"\n")
                .starts_with("p/burdin-toolchain.toml: line 2: `beta` is not a toolchain")
        );
        assert!(
            error("[toolchain]\nchannel = \"stable\"\ncomponents = [\"gui\"]\n")
                .contains("line 3: unknown component `gui`")
        );
        assert!(
            error("[toolchain]\nchannel = \"stable\"\ntargets = [\"riscv64gc-unknown-linux-gnu\"]\n")
                .contains("line 3: `riscv64gc-unknown-linux-gnu` is not one of")
        );
        assert!(
            error("[toolchain]\nchannel = \"stable\"\nprofile = \"minimal\"\n")
                .contains("line 3: unknown key `profile`")
        );
        assert!(error("channel = \"stable\"\n").contains("line 1: `channel` outside a table"));
    }
}
