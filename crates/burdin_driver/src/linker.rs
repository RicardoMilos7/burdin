//! Choosing the linker (issue #335): a fast linker when one is installed, `BURDIN_LD` when it is set, and the C
//! compiler driver's own linker otherwise.
//!
//! The object files are linked through the system C compiler driver (`BURDIN_CC`, `cc`, or `cl`), which also supplies
//! the C runtime, the startup files, and the libraries. A fast linker takes the linker's part of that job over through
//! `-fuse-ld=<name>`: the driver still finds the C runtime, and the named linker does the work. The choice is:
//!
//!   1. `BURDIN_LD` when it is set: a linker name (`mold`, `lld`, `gold`, ...), the path of the linker executable
//!      (clang accepts a path; gcc does not), or `platform`/`default` to force the compiler driver's own linker;
//!   2. otherwise, on Linux, `mold` then `lld` when their `ld.mold`/`ld.lld` driver names are on `PATH` — the names
//!      both gcc and clang look for;
//!   3. otherwise the compiler driver's own linker.
//!
//! macOS and Windows keep their platform linkers: ld64 is already the native linker, and an MSVC-style compiler
//! driver starts `link.exe` and has no `-fuse-ld`. `--lto` also keeps the driver's linker, because LLVM bitcode needs
//! lld (not mold) and the installed lld may not be the LLVM version that produced the bitcode.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The linker a link command names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Linker {
    /// The C compiler driver's own linker: no `-fuse-ld`.
    Platform,
    /// `-fuse-ld=<value>`: a linker name (`lld`, `mold`) or, with clang, the path of the linker executable.
    Named(String),
}

impl Linker {
    /// The `-fuse-ld=` value, when a linker is named.
    pub(crate) fn fuse_ld(&self) -> Option<&str> {
        match self {
            Linker::Platform => None,
            Linker::Named(value) => Some(value),
        }
    }
}

/// Where the choice came from, so a failed link can say how to change it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// The compiler driver's own linker (the default, or `BURDIN_LD=platform`).
    Platform,
    /// `BURDIN_LD` named the linker.
    Environment,
    /// No `BURDIN_LD`, and an installed fast linker was found.
    Automatic,
}

/// The linker and where it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Choice {
    pub(crate) linker: Linker,
    pub(crate) origin: Origin,
}

impl Choice {
    fn platform() -> Choice {
        Choice { linker: Linker::Platform, origin: Origin::Platform }
    }

    fn from_environment(linker: Linker) -> Choice {
        Choice { linker, origin: Origin::Environment }
    }

    fn automatic(linker: Linker) -> Choice {
        Choice { linker, origin: Origin::Automatic }
    }
}

/// `BURDIN_LD` values that mean "let the compiler driver choose" on every platform.
fn is_platform_name(value: &str) -> bool {
    matches!(value.to_ascii_lowercase().as_str(), "platform" | "default")
}

/// The same, plus the names of the only linker an MSVC-style driver has.
fn is_msvc_platform_name(value: &str) -> bool {
    is_platform_name(value) || matches!(value.to_ascii_lowercase().as_str(), "link" | "link.exe" | "msvc")
}

/// Whether a `BURDIN_LD` value names mold, by name or by path.
fn is_mold(value: &str) -> bool {
    let name = value.rsplit(['/', '\\']).next().unwrap_or(value);
    name.trim_end_matches(".exe").eq_ignore_ascii_case("mold")
}

/// Chooses the linker.
///
/// `msvc_style` is the compiler driver's command-line family; `lto` is `--lto`; `automatic` says whether a fast
/// linker may be picked without being asked for (it is, on Linux); `environment` is `BURDIN_LD`; `find` locates a
/// program on `PATH`.
pub(crate) fn choose(
    msvc_style: bool,
    lto: bool,
    automatic: bool,
    environment: Option<&str>,
    find: impl Fn(&str) -> Option<PathBuf>,
) -> Result<Choice, String> {
    let environment = environment.map(str::trim).filter(|value| !value.is_empty());
    if msvc_style {
        return match environment {
            Some(value) if !is_msvc_platform_name(value) => Err(format!(
                "BURDIN_LD=`{value}` is not supported here: this build links with `link.exe` through an MSVC-style \
                 compiler driver, which has no `-fuse-ld`; unset BURDIN_LD"
            )),
            _ => Ok(Choice::platform()),
        };
    }
    if let Some(value) = environment {
        if is_platform_name(value) {
            return Ok(Choice::platform());
        }
        if lto && is_mold(value) {
            return Err("`--lto` links LLVM bitcode, which mold cannot read; use `BURDIN_LD=lld` or unset BURDIN_LD"
                .to_string());
        }
        return Ok(Choice::from_environment(Linker::Named(value.to_string())));
    }
    if !automatic || lto {
        return Ok(Choice::platform());
    }
    // The names, not paths: gcc and clang turn `-fuse-ld=<name>` into `ld.<name>` on `PATH`, and that is what the
    // packages install (mold ships `ld.mold`; lld's `lld` package ships `ld.lld`).
    for (name, driver) in [("mold", "ld.mold"), ("lld", "ld.lld")] {
        if find(driver).is_some() {
            return Ok(Choice::automatic(Linker::Named(name.to_string())));
        }
    }
    Ok(Choice::platform())
}

/// The note a failed link adds when something other than the platform linker was chosen, or empty text.
pub(crate) fn failure_note(choice: &Choice) -> String {
    match (choice.origin, choice.linker.fuse_ld()) {
        (Origin::Environment, Some(value)) => format!(
            "\nnote: BURDIN_LD selected the `{value}` linker; unset BURDIN_LD to use the compiler's default linker"
        ),
        (Origin::Automatic, Some(value)) => format!(
            "\nnote: the `{value}` linker was selected automatically because `ld.{value}` is on PATH; \
             set BURDIN_LD=platform to use the compiler's default linker"
        ),
        _ => String::new(),
    }
}

/// Finds `name` on `PATH`. On Windows an executable's `.exe` is implied, as `Command::new` implies it.
pub(crate) fn find_on_path(name: &str) -> Option<PathBuf> {
    find_in_path(std::env::var_os("PATH"), name)
}

fn find_in_path(path: Option<OsString>, name: &str) -> Option<PathBuf> {
    for dir in std::env::split_paths(&path?) {
        let candidate = dir.join(name);
        if is_program(&candidate) {
            return Some(candidate);
        }
        if !std::env::consts::EXE_EXTENSION.is_empty() {
            // Appended, not `with_extension`: a name like `ld.lld` must not become `ld.exe`.
            let with_extension = dir.join(format!("{name}.{}", std::env::consts::EXE_EXTENSION));
            if is_program(&with_extension) {
                return Some(with_extension);
            }
        }
    }
    None
}

fn is_program(path: &Path) -> bool {
    path.metadata().is_ok_and(|metadata| {
        if !metadata.is_file() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            true
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(names: &'static [&'static str]) -> impl Fn(&str) -> Option<PathBuf> {
        move |name| names.contains(&name).then(|| PathBuf::from(format!("/usr/bin/{name}")))
    }

    fn choose_auto(names: &'static [&'static str]) -> Choice {
        choose(false, false, true, None, found(names)).unwrap()
    }

    #[test]
    fn without_burdin_ld_mold_is_preferred_then_lld() {
        assert_eq!(choose_auto(&["ld.mold", "ld.lld"]).linker, Linker::Named("mold".into()));
        assert_eq!(choose_auto(&["ld.lld"]).linker, Linker::Named("lld".into()));
        assert_eq!(choose_auto(&[]).linker, Linker::Platform);
        assert_eq!(choose_auto(&["ld.mold"]).origin, Origin::Automatic);
    }

    #[test]
    fn an_existing_linker_is_not_picked_without_automatic_selection() {
        let choice = choose(false, false, false, None, found(&["ld.mold", "ld.lld"])).unwrap();
        assert_eq!(choice.linker, Linker::Platform);
        // macOS and Windows keep the platform linker; `BURDIN_LD` can still name one.
        let forced = choose(false, false, false, Some("lld"), found(&[])).unwrap();
        assert_eq!(forced.linker, Linker::Named("lld".into()));
        assert_eq!(forced.origin, Origin::Environment);
    }

    #[test]
    fn burdin_ld_wins_over_the_automatic_choice() {
        let choice = choose(false, false, true, Some("lld"), found(&["ld.mold", "ld.lld"])).unwrap();
        assert_eq!(choice.linker, Linker::Named("lld".into()));
        assert_eq!(choice.origin, Origin::Environment);
    }

    #[test]
    fn platform_names_force_the_compiler_drivers_linker() {
        for value in ["platform", "default", "PLATFORM"] {
            let choice = choose(false, false, true, Some(value), found(&["ld.mold"])).unwrap();
            assert_eq!(choice.linker, Linker::Platform, "{value}");
            assert_eq!(choice.origin, Origin::Platform, "{value}");
        }
        // An empty value is the same as unset: the automatic choice still applies.
        assert_eq!(
            choose(false, false, true, Some("  "), found(&["ld.lld"])).unwrap().linker,
            Linker::Named("lld".into())
        );
    }

    #[test]
    fn lto_keeps_the_drivers_linker_and_refuses_mold() {
        assert_eq!(choose(false, true, true, None, found(&["ld.mold", "ld.lld"])).unwrap().linker, Linker::Platform);
        let error = choose(false, true, true, Some("mold"), found(&[])).unwrap_err();
        assert!(error.contains("`--lto`") && error.contains("mold"), "{error}");
        // lld does link LLVM bitcode, so asking for it is allowed.
        assert_eq!(choose(false, true, true, Some("lld"), found(&[])).unwrap().linker, Linker::Named("lld".into()));
    }

    #[test]
    fn a_path_to_mold_is_recognized_for_lto() {
        let error = choose(false, true, false, Some("/usr/bin/mold"), found(&[])).unwrap_err();
        assert!(error.contains("mold"), "{error}");
        let choice = choose(false, false, false, Some("/opt/mold/bin/mold"), found(&[])).unwrap();
        assert_eq!(choice.linker, Linker::Named("/opt/mold/bin/mold".into()));
    }

    #[test]
    fn an_msvc_style_driver_only_has_the_platform_linker() {
        assert_eq!(choose(true, false, true, None, found(&["ld.lld"])).unwrap().linker, Linker::Platform);
        for value in ["platform", "link", "LINK.EXE", "msvc"] {
            assert_eq!(choose(true, false, true, Some(value), found(&[])).unwrap().linker, Linker::Platform, "{value}");
        }
        for value in ["lld", "mold", "/usr/bin/ld.lld"] {
            let error = choose(true, false, true, Some(value), found(&[])).unwrap_err();
            assert!(error.contains("BURDIN_LD") && error.contains(value), "{value}: {error}");
            assert!(error.contains("link.exe"), "{value}: {error}");
        }
    }

    #[test]
    fn a_failed_link_says_how_to_change_the_choice() {
        let note = failure_note(&choose(false, false, true, Some("gold"), found(&[])).unwrap());
        assert!(note.contains("BURDIN_LD") && note.contains("gold"), "{note}");
        let note = failure_note(&choose_auto(&["ld.mold"]));
        assert!(note.contains("automatic") && note.contains("BURDIN_LD=platform"), "{note}");
        let note = failure_note(&choose(false, false, true, None, found(&[])).unwrap());
        assert!(note.is_empty(), "{note}");
    }

    #[test]
    fn a_linker_driver_name_is_found_on_the_path() {
        let dir =
            std::env::temp_dir().join(format!("burdin-path-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // On Windows the extension is appended, so a dotted name keeps its own dot (`ld.lld.exe`).
        let name = if std::env::consts::EXE_EXTENSION.is_empty() {
            "ld.probe".to_string()
        } else {
            format!("ld.probe.{}", std::env::consts::EXE_EXTENSION)
        };
        let program = dir.join(&name);
        std::fs::write(&program, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = Some(dir.clone().into_os_string());
        assert_eq!(find_in_path(path.clone(), "ld.probe"), Some(program.clone()));
        assert_eq!(find_in_path(path, "ld.missing"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
