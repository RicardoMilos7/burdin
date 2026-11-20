//! Where the manager keeps its state (`spec/toolchain.md` TC3): every toolchain, the settings, the downloads, and the
//! receipt of what was created outside the home, all under `<burdin home>/toolchains`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::toml::{self, Value};
use crate::version::{ToolchainName, Version};

/// The file in each toolchain directory that records what is installed there. Its presence is also what marks the
/// directory as one the manager created.
pub const RECORD_FILE: &str = ".installed.toml";
/// The directory in each toolchain that lists every component's files, one `<component>.files` per component.
pub const COMPONENT_LISTS: &str = ".components";

pub fn io_error(action: &str, path: &Path, error: std::io::Error) -> String {
    format!("cannot {action} {}: {error}", path.display())
}

/// The Burdin home: `BURDIN_HOME` when it is set, otherwise the platform's directory for user state. This is the same
/// rule as `burdin_query::package::home_dir` (`spec/repl.md` RP14); it is repeated here so the proxy does not link
/// the compiler's front end.
pub fn burdin_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("BURDIN_HOME").filter(|home| !home.is_empty()) {
        return Some(PathBuf::from(home));
    }
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join("burdin"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support/burdin"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
            .map(|base| base.join("burdin"))
    }
}

/// The manager's directories and files.
pub struct Home {
    pub root: PathBuf,
}

impl Home {
    pub fn locate() -> Result<Home, String> {
        let root = burdin_home().ok_or("cannot find the Burdin home directory: set BURDIN_HOME")?;
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir().map_err(|e| format!("cannot read the current directory: {e}"))?.join(root)
        };
        Ok(Home { root })
    }

    pub fn toolchains(&self) -> PathBuf {
        self.root.join("toolchains")
    }

    pub fn toolchain_dir(&self, name: &ToolchainName) -> PathBuf {
        self.toolchains().join(name.to_string())
    }

    pub fn settings(&self) -> PathBuf {
        self.toolchains().join("settings.toml")
    }

    pub fn receipt(&self) -> PathBuf {
        self.toolchains().join("receipt")
    }

    pub fn downloads(&self) -> PathBuf {
        self.toolchains().join("downloads")
    }

    /// The default toolchain from the settings, if one is set.
    pub fn default_toolchain(&self) -> Result<Option<ToolchainName>, String> {
        let path = self.settings();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_error("read", &path, e)),
        };
        let entries = toml::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        for entry in entries {
            if entry.table.is_empty()
                && entry.key == "default"
                && let Value::Str(name) = entry.value
            {
                return ToolchainName::parse(&name).map(Some).map_err(|e| format!("{}: {e}", path.display()));
            }
        }
        Ok(None)
    }

    pub fn set_default(&self, name: Option<&ToolchainName>) -> Result<(), String> {
        let path = self.settings();
        match name {
            Some(name) => {
                let text = format!(
                    "# Written by `burdin toolchain`; change it with `burdin toolchain default`.\ndefault = {}\n",
                    toml::quote(&name.to_string())
                );
                write_file(&path, &text)
            }
            None => match fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(io_error("remove", &path, e)),
            },
        }
    }

    /// The record of an installed toolchain, or `None` when it is not installed.
    pub fn record(&self, name: &ToolchainName) -> Option<Record> {
        Record::read(&self.toolchain_dir(name)).ok()
    }

    /// Every installed toolchain, sorted by name.
    pub fn installed(&self) -> Vec<(ToolchainName, Record)> {
        let mut found = Vec::new();
        let Ok(entries) = fs::read_dir(self.toolchains()) else { return found };
        for entry in entries.flatten() {
            let Some(text) = entry.file_name().to_str().map(str::to_string) else { continue };
            let Ok(name) = ToolchainName::parse(&text) else { continue };
            if let Ok(record) = Record::read(&entry.path()) {
                found.push((name, record));
            }
        }
        found.sort_by_key(|(name, _)| name.to_string());
        found
    }
}

/// What is installed in one toolchain directory (`.installed.toml`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub version: Version,
    pub target: String,
    pub components: Vec<String>,
}

impl Record {
    pub fn read(dir: &Path) -> Result<Record, String> {
        let path = dir.join(RECORD_FILE);
        let text = fs::read_to_string(&path).map_err(|e| io_error("read", &path, e))?;
        let entries = toml::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut version = None;
        let mut target = None;
        let mut components = Vec::new();
        for entry in entries {
            match (entry.key.as_str(), entry.value) {
                ("version", Value::Str(text)) => version = Version::parse(&text),
                ("target", Value::Str(text)) => target = Some(text),
                ("components", Value::List(list)) => components = list,
                _ => {}
            }
        }
        match (version, target) {
            (Some(version), Some(target)) => Ok(Record { version, target, components }),
            _ => Err(format!("{}: the record has no version or target", path.display())),
        }
    }

    pub fn write(&self, dir: &Path) -> Result<(), String> {
        let text = format!(
            "# Written by `burdin toolchain`: what is installed in this toolchain.\nversion = {}\ntarget = {}\ncomponents = {}\n",
            toml::quote(&self.version.to_string()),
            toml::quote(&self.target),
            toml::quote_list(&self.components)
        );
        write_file(&dir.join(RECORD_FILE), &text)
    }
}

/// One thing created outside `<home>/toolchains` (TC14): a proxy executable, a line appended to a shell startup file,
/// or an entry appended to the user's `PATH` on Windows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceiptEntry {
    Proxy(PathBuf),
    StartupLine(PathBuf, String),
    UserPath(String),
}

/// The receipt: one entry per line, its fields separated by tabs.
pub fn read_receipt(home: &Home) -> Result<Vec<ReceiptEntry>, String> {
    let path = home.receipt();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_error("read", &path, e)),
    };
    let mut entries = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        let entry = match fields.as_slice() {
            ["proxy", path] => ReceiptEntry::Proxy(PathBuf::from(path)),
            ["startup-line", file, text] => ReceiptEntry::StartupLine(PathBuf::from(file), text.to_string()),
            ["user-path", dir] => ReceiptEntry::UserPath(dir.to_string()),
            _ => return Err(format!("{}: line {}: not a receipt entry", path.display(), index + 1)),
        };
        entries.push(entry);
    }
    Ok(entries)
}

pub fn write_receipt(home: &Home, entries: &[ReceiptEntry]) -> Result<(), String> {
    let mut text = String::from(
        "# Written by the installer and `burdin toolchain`: what they created outside this directory.\n\
         # `burdin toolchain self uninstall` removes exactly these.\n",
    );
    for entry in entries {
        match entry {
            ReceiptEntry::Proxy(path) => text.push_str(&format!("proxy\t{}\n", path.display())),
            ReceiptEntry::StartupLine(file, line) => {
                text.push_str(&format!("startup-line\t{}\t{line}\n", file.display()))
            }
            ReceiptEntry::UserPath(dir) => text.push_str(&format!("user-path\t{dir}\n")),
        }
    }
    write_file(&home.receipt(), &text)
}

/// Adds `entry` to the receipt unless it is there already.
pub fn record_in_receipt(home: &Home, entry: ReceiptEntry) -> Result<(), String> {
    let text = match &entry {
        ReceiptEntry::Proxy(path) | ReceiptEntry::StartupLine(path, _) => path.display().to_string(),
        ReceiptEntry::UserPath(dir) => dir.clone(),
    };
    let line = match &entry {
        ReceiptEntry::StartupLine(_, line) => line.as_str(),
        _ => "",
    };
    if text.contains(['\t', '\n', '\r']) || line.contains(['\t', '\n', '\r']) {
        return Err("a receipt entry cannot contain a tab or a line break".into());
    }
    let mut entries = read_receipt(home)?;
    if !entries.contains(&entry) {
        entries.push(entry);
        write_receipt(home, &entries)?;
    }
    Ok(())
}

/// Writes `text` to `path` through a temporary file, so a reader never sees half of it.
pub fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| io_error("create", parent, e))?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, text).map_err(|e| io_error("write", &tmp, e))?;
    fs::rename(&tmp, path).map_err(|e| io_error("write", path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("burdin-toolchain-home-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn records_round_trip() {
        let dir = scratch("record");
        let record = Record {
            version: Version::parse("0.4.1").unwrap(),
            target: "x86_64-unknown-linux-gnu".into(),
            components: vec!["lsp".into()],
        };
        record.write(&dir).unwrap();
        assert_eq!(Record::read(&dir).unwrap(), record);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn receipt_round_trips_and_deduplicates() {
        let home = Home { root: scratch("receipt") };
        let proxy = ReceiptEntry::Proxy(PathBuf::from("/b/burdin"));
        record_in_receipt(&home, proxy.clone()).unwrap();
        record_in_receipt(&home, proxy.clone()).unwrap();
        let line = ReceiptEntry::StartupLine(PathBuf::from("/h/.bashrc"), "export PATH=\"/b:$PATH\"".into());
        record_in_receipt(&home, line.clone()).unwrap();
        record_in_receipt(&home, ReceiptEntry::UserPath(r"C:\b".into())).unwrap();
        assert_eq!(read_receipt(&home).unwrap(), vec![proxy, line, ReceiptEntry::UserPath(r"C:\b".into())]);
        assert!(record_in_receipt(&home, ReceiptEntry::UserPath("a\tb".into())).is_err());
        fs::remove_dir_all(&home.root).unwrap();
    }

    #[test]
    fn default_toolchain_round_trips() {
        let home = Home { root: scratch("settings") };
        assert_eq!(home.default_toolchain().unwrap(), None);
        let name = ToolchainName::parse("nightly").unwrap();
        home.set_default(Some(&name)).unwrap();
        assert_eq!(home.default_toolchain().unwrap(), Some(name));
        home.set_default(None).unwrap();
        assert_eq!(home.default_toolchain().unwrap(), None);
        fs::remove_dir_all(&home.root).unwrap();
    }
}
