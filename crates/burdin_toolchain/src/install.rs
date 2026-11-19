//! Installing, replacing, and removing toolchains and their components (`spec/toolchain.md` TC5, TC6, TC10).
//!
//! Everything is assembled in a staging directory beside the toolchains and moved into place only after it is
//! complete and verified, so a failed download, a checksum mismatch, or a bad archive leaves an installed toolchain
//! exactly as it was.

use std::fs;
use std::path::{Path, PathBuf};

use crate::dist::{self, Source};
use crate::home::{COMPONENT_LISTS, Home, RECORD_FILE, Record, io_error};
use crate::version::{EXE, ToolchainName, Version};

/// The optional components (TC10), with what each one holds.
pub const COMPONENTS: [(&str, &str); 4] = [
    ("debuggers", "the gdb and lldb printers and the Windows Natvis file"),
    ("docs", "the offline documentation (the docs/ and spec/ pages)"),
    ("lsp", "burdin-lsp, the language server without the LLVM back end"),
    ("std-src", "the standard library's sources, for go-to-definition"),
];

pub fn check_component(name: &str) -> Result<(), String> {
    if COMPONENTS.iter().any(|(known, _)| *known == name) {
        Ok(())
    } else {
        let known: Vec<&str> = COMPONENTS.iter().map(|(name, _)| *name).collect();
        Err(format!("unknown component `{name}` (the components are {})", known.join(", ")))
    }
}

/// What every operation needs: the home, the release source, and the host target.
pub struct Context {
    pub home: Home,
    pub source: Source,
    pub target: &'static str,
}

impl Context {
    fn staging(&self, what: &str) -> PathBuf {
        self.home.toolchains().join(format!(".staging-{what}-{}", std::process::id()))
    }

    /// The version a toolchain name stands for now: a channel's latest release, or the exact version.
    pub fn resolve(&self, name: &ToolchainName) -> Result<Version, String> {
        match name {
            ToolchainName::Channel(channel) => self.source.latest(*channel),
            ToolchainName::Exact(version) => Ok(version.clone()),
        }
    }

    /// Downloads and verifies one archive of `version`; `missing` is the error when the release does not list it.
    fn fetch(&self, version: &Version, file: &str, missing: String) -> Result<PathBuf, String> {
        let expected = self.source.expected_sha256(version, file)?.ok_or(missing)?;
        dist::fetch_verified(&self.source, version, file, &expected, &self.home.downloads())
    }

    /// Installs (or replaces) the toolchain `name` at `version` with `components`.
    pub fn install(&self, name: &ToolchainName, version: &Version, components: &[String]) -> Result<(), String> {
        let archive = format!("burdin-{version}-{}.tar.gz", self.target);
        let file = self.fetch(version, &archive, format!("release {version} has no archive for {}", self.target))?;
        let staging = self.staging(&name.to_string());
        let result = (|| {
            let _ = fs::remove_dir_all(&staging);
            dist::extract(&file, &staging)?;
            let root = staging.join(format!("burdin-{version}-{}", self.target));
            if !root.join(format!("burdin{EXE}")).is_file() {
                return Err(format!(
                    "the archive {archive} does not contain burdin-{version}-{}/burdin{EXE}",
                    self.target
                ));
            }
            Record { version: version.clone(), target: self.target.to_string(), components: Vec::new() }
                .write(&root)?;
            for component in components {
                self.add_component_into(&root, version, component)?;
            }
            self.place(&root, &self.home.toolchain_dir(name))
        })();
        let _ = fs::remove_dir_all(&staging);
        let _ = fs::remove_file(&file);
        result
    }

    /// Installs the toolchain `name` from an unpacked, already verified release directory (the installers'
    /// bootstrap, TC13).
    pub fn install_from_dir(&self, name: &ToolchainName, version: &Version, dir: &Path) -> Result<(), String> {
        if !dir.join(format!("burdin{EXE}")).is_file() {
            return Err(format!("{} does not contain burdin{EXE}", dir.display()));
        }
        let staging = self.staging(&name.to_string());
        let result = (|| {
            let _ = fs::remove_dir_all(&staging);
            let root = staging.join("toolchain");
            copy_tree(dir, &root)?;
            Record { version: version.clone(), target: self.target.to_string(), components: Vec::new() }
                .write(&root)?;
            self.place(&root, &self.home.toolchain_dir(name))
        })();
        let _ = fs::remove_dir_all(&staging);
        result
    }

    /// Moves the finished `new` directory to `dest`, replacing what is there only once the move can succeed.
    fn place(&self, new: &Path, dest: &Path) -> Result<(), String> {
        let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let old = self.home.toolchains().join(format!(".old-{name}-{}", std::process::id()));
        if dest.exists() {
            let _ = fs::remove_dir_all(&old);
            fs::rename(dest, &old).map_err(|e| io_error("replace", dest, e))?;
        }
        if let Err(e) = fs::rename(new, dest) {
            if old.exists() {
                let _ = fs::rename(&old, dest);
            }
            return Err(io_error("install", dest, e));
        }
        let _ = fs::remove_dir_all(&old);
        Ok(())
    }

    /// Adds `component` of `version` to the toolchain directory `dir` and records its files.
    pub fn add_component_into(&self, dir: &Path, version: &Version, component: &str) -> Result<(), String> {
        check_component(component)?;
        let target = self.target;
        let archive = format!("burdin-{component}-{version}-{target}.tar.gz");
        let file =
            self.fetch(version, &archive, format!("release {version} has no {component} component for {target}"))?;
        let staging = self.staging(&format!("component-{component}"));
        let result = (|| {
            let _ = fs::remove_dir_all(&staging);
            dist::extract(&file, &staging)?;
            let root = staging.join(format!("burdin-{component}-{version}-{target}"));
            if !root.is_dir() {
                return Err(format!("the archive {archive} does not contain burdin-{component}-{version}-{target}/"));
            }
            let files = files_under(&root)?;
            for relative in &files {
                let to = dir.join(relative);
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent).map_err(|e| io_error("create", parent, e))?;
                }
                fs::copy(root.join(relative), &to).map_err(|e| io_error("write", &to, e))?;
            }
            let list: Vec<String> = files.iter().map(|path| slash_path(path)).collect();
            let lists = dir.join(COMPONENT_LISTS);
            fs::create_dir_all(&lists).map_err(|e| io_error("create", &lists, e))?;
            crate::home::write_file(&lists.join(format!("{component}.files")), &(list.join("\n") + "\n"))?;
            let mut record = Record::read(dir)?;
            if !record.components.iter().any(|c| c == component) {
                record.components.push(component.to_string());
                record.components.sort();
            }
            record.write(dir)
        })();
        let _ = fs::remove_dir_all(&staging);
        let _ = fs::remove_file(&file);
        result
    }

    /// Removes `component` from the toolchain directory `dir`: exactly the files its list names.
    pub fn remove_component(&self, dir: &Path, component: &str) -> Result<(), String> {
        check_component(component)?;
        let mut record = Record::read(dir)?;
        if !record.components.iter().any(|c| c == component) {
            return Err(format!("component {component} is not installed"));
        }
        let list_path = dir.join(COMPONENT_LISTS).join(format!("{component}.files"));
        let list = fs::read_to_string(&list_path).map_err(|e| io_error("read", &list_path, e))?;
        for relative in list.lines().filter(|line| !line.is_empty()) {
            if relative.split('/').any(|part| part == ".." || part.is_empty()) || relative == RECORD_FILE {
                continue;
            }
            let path = dir.join(relative);
            match fs::remove_file(&path) {
                Ok(()) => prune_empty_parents(&path, dir),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_error("remove", &path, e)),
            }
        }
        fs::remove_file(&list_path).map_err(|e| io_error("remove", &list_path, e))?;
        prune_empty_parents(&list_path, dir);
        record.components.retain(|c| c != component);
        record.write(dir)
    }
}

/// Removes the empty directories above `path`, stopping at `top`.
fn prune_empty_parents(path: &Path, top: &Path) {
    let mut current = path.parent();
    while let Some(dir) = current {
        if dir == top || !dir.starts_with(top) || fs::remove_dir(dir).is_err() {
            break;
        }
        current = dir.parent();
    }
}

fn slash_path(path: &Path) -> String {
    path.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

/// Every file under `root`, relative to it, sorted.
fn files_under(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let dir = root.join(&relative);
        for entry in fs::read_dir(&dir).map_err(|e| io_error("read", &dir, e))? {
            let entry = entry.map_err(|e| io_error("read", &dir, e))?;
            let path = relative.join(entry.file_name());
            if entry.file_type().map_err(|e| io_error("read", &entry.path(), e))?.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Copies the directory `from` to the new directory `to`.
pub fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| io_error("create", to, e))?;
    for relative in files_under(from)? {
        let target = to.join(&relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| io_error("create", parent, e))?;
        }
        fs::copy(from.join(&relative), &target).map_err(|e| io_error("write", &target, e))?;
    }
    Ok(())
}
