//! The `[lib]` table (`spec/library.md`): what a build of a library writes, and the C header of its exports.
//!
//! The package loader keeps the table's entries as written (`burdin_query::package`); this module checks them and
//! reports E0265, so a bad table stops the build before a program's source is checked as one. A static or shared
//! library is written by `compile` (`main.rs`): its objects are archived or linked as a shared library, and
//! `c-header` is written by the C-header export (`export.rs`).

use std::path::Path;

use burdin_diag::Code;
use burdin_query::package::{PackageFiles, lib_is_not_source, parse_string};

use crate::build::Failure;

/// The artifact a `[lib]` table asks a build to write (`source` writes none, so it is not a `Kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Static,
    Shared,
}

/// A checked `[lib]` table of a static or shared library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Library {
    pub(crate) kind: Kind,
    /// `c-header`, as written: a path relative to the package directory.
    pub(crate) c_header: Option<String>,
}

/// What a `[lib]` table declares, before its keys are checked against the package.
enum Declared {
    Source,
    Static,
    Shared,
}

fn fail(line: usize, message: impl std::fmt::Display) -> Failure {
    Failure::new(Code::LIBRARY_CONFIG, format!("burdin.toml:{line}: {message}"))
}

/// Whether `path` names a place inside the package: a relative path with no `..` part and no drive or root.
fn is_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with(['/', '\\'])
        && !path.contains(':')
        && !path.split(['/', '\\']).any(|part| part == "..")
}

/// Checks the `[lib]` entries of the entry package: `Ok(None)` for a program or a `source` library, `Ok(Some(..))` for
/// a static or shared library, and E0265 for every other table or a library with a `src/main.bd` (`spec/library.md`,
/// LB1 and LB4). `has_main` says whether the package has a `src/main.bd`.
pub(crate) fn table(lib: &[(String, String, usize)], has_main: bool) -> Result<Option<Library>, Failure> {
    let mut declared: Option<(Declared, usize)> = None;
    let mut header: Option<(String, usize)> = None;
    for (key, value, line) in lib {
        let line = *line;
        match key.as_str() {
            "kind" => {
                let text = parse_string(value)
                    .map_err(|_| fail(line, "`kind` must be a string: \"source\", \"static\", or \"shared\""))?;
                let found = match text.as_str() {
                    "source" => Declared::Source,
                    "static" => Declared::Static,
                    "shared" => Declared::Shared,
                    other => {
                        return Err(fail(
                            line,
                            format!(
                                "`kind = \"{other}\"` is not a library kind; the kinds are \"source\", \"static\", and \"shared\""
                            ),
                        ));
                    }
                };
                declared = Some((found, line));
            }
            "c-header" => {
                let path = parse_string(value).map_err(|_| fail(line, "`c-header` must be a string: a path"))?;
                if !is_relative(&path) {
                    return Err(fail(
                        line,
                        format!(
                            "`c-header = \"{path}\"` must be a relative path inside the package, such as \"include/name.h\""
                        ),
                    ));
                }
                header = Some((path, line));
            }
            other => {
                return Err(fail(line, format!("`{other}` is not a `[lib]` key; the keys are `kind` and `c-header`")));
            }
        }
    }
    let (kind, kind_line) = match declared {
        None | Some((Declared::Source, _)) => {
            if let Some((_, line)) = header {
                return Err(fail(
                    line,
                    "`c-header` needs a `kind` of \"static\" or \"shared\"; a `source` library writes no C surface",
                ));
            }
            return Ok(None);
        }
        Some((Declared::Static, line)) => (Kind::Static, line),
        Some((Declared::Shared, line)) => (Kind::Shared, line),
    };
    if has_main {
        return Err(fail(
            kind_line,
            "a static or shared library has no entry point, so its `src/main.bd` must go; set `kind = \"source\"` for a program",
        ));
    }
    Ok(Some(Library { kind, c_header: header.map(|(path, _)| path) }))
}

/// The entry package's checked `[lib]` table (see [`table`]).
pub(crate) fn validate(package: &PackageFiles) -> Result<Option<Library>, Failure> {
    table(&package.lib, package.modules.iter().any(|module| module.path == ["main"]))
}

/// Whether the package in `dir` has a `[lib]` table that makes it a library: a static or shared one, or an invalid one
/// the driver reports. Such a package is checked, built, and tested as a library (`spec/library.md`, LB2 and LB3); a
/// workspace build builds it without a `src/main.bd` (LB10).
pub(crate) fn dir_is_library(dir: &Path) -> bool {
    burdin_query::package::parse_manifest(&dir.join("burdin.toml"))
        .is_ok_and(|manifest| lib_is_not_source(&manifest.lib))
}

/// The file name of `kind`'s artifact for the package `stem`, on the host the compiler builds for (`spec/library.md`,
/// LB3): `NAME.lib` and `NAME.dll` on Windows, `libNAME.dylib` on macOS, `libNAME.so` elsewhere, and `libNAME.a` for a
/// static library on Unix.
pub(crate) fn file_name(kind: Kind, stem: &str) -> String {
    match (kind, cfg!(windows), cfg!(target_os = "macos")) {
        (Kind::Static, true, _) => format!("{stem}.lib"),
        (Kind::Static, false, _) => format!("lib{stem}.a"),
        (Kind::Shared, true, _) => format!("{stem}.dll"),
        (Kind::Shared, false, true) => format!("lib{stem}.dylib"),
        (Kind::Shared, false, false) => format!("lib{stem}.so"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(text: &[(&str, &str)]) -> Vec<(String, String, usize)> {
        text.iter().enumerate().map(|(i, (k, v))| (k.to_string(), v.to_string(), i + 6)).collect()
    }

    fn message(result: Result<Option<Library>, Failure>) -> String {
        match result {
            Ok(_) => panic!("expected an error"),
            Err(failure) => {
                assert_eq!(failure.code, Code::LIBRARY_CONFIG);
                failure.message
            }
        }
    }

    /// The checked table, or a panic naming the error.
    fn library(result: Result<Option<Library>, Failure>) -> Option<Library> {
        match result {
            Ok(lib) => lib,
            Err(failure) => panic!("the table is refused: {}", failure.message),
        }
    }

    #[test]
    fn a_program_or_a_source_library_has_no_artifact() {
        assert!(library(table(&[], false)).is_none());
        assert!(library(table(&entries(&[("kind", "\"source\"")]), false)).is_none());
        // A `source` library with a `main` is a program, which is fine.
        assert!(library(table(&entries(&[("kind", "\"source\"")]), true)).is_none());
    }

    #[test]
    fn a_static_or_shared_library_takes_its_header_and_kind() {
        let lib = library(table(&entries(&[("kind", "\"static\""), ("c-header", "\"include/a.h\"")]), false));
        assert_eq!(lib, Some(Library { kind: Kind::Static, c_header: Some("include/a.h".into()) }));
        let lib = library(table(&entries(&[("kind", "\"shared\"")]), false));
        assert_eq!(lib, Some(Library { kind: Kind::Shared, c_header: None }));
    }

    #[test]
    fn an_invalid_table_names_its_line() {
        assert_eq!(
            message(table(&entries(&[("kind", "\"static\""), ("colour", "\"red\"")]), false)),
            "burdin.toml:7: `colour` is not a `[lib]` key; the keys are `kind` and `c-header`"
        );
        assert!(
            message(table(&entries(&[("kind", "\"dylib\"")]), false))
                .contains("`kind = \"dylib\"` is not a library kind")
        );
        assert!(
            message(table(&entries(&[("kind", "1")]), false)).starts_with("burdin.toml:6: `kind` must be a string")
        );
        assert!(message(table(&entries(&[("c-header", "\"a.h\"")]), false)).contains("needs a `kind` of"));
    }

    #[test]
    fn a_header_must_be_a_relative_path_inside_the_package() {
        for bad in ["\"/abs.h\"", "\"../a.h\"", "\"include/../../a.h\"", "\"C:/a.h\"", "\"\""] {
            let result = table(&entries(&[("kind", "\"static\""), ("c-header", bad)]), false);
            assert!(message(result).contains("must be a relative path"), "{bad} is accepted");
        }
        assert!(is_relative("include/app.h"));
    }

    #[test]
    fn a_library_with_a_main_is_refused_at_its_kind_line() {
        assert_eq!(
            message(table(&entries(&[("kind", "\"shared\"")]), true)),
            "burdin.toml:6: a static or shared library has no entry point, so its `src/main.bd` must go; set `kind = \"source\"` for a program"
        );
    }

    #[test]
    fn artifact_names_follow_the_host() {
        let (windows, mac) = (cfg!(windows), cfg!(target_os = "macos"));
        let expected_static = if windows { "greet.lib" } else { "libgreet.a" };
        let expected_shared = if windows {
            "greet.dll"
        } else if mac {
            "libgreet.dylib"
        } else {
            "libgreet.so"
        };
        assert_eq!(file_name(Kind::Static, "greet"), expected_static);
        assert_eq!(file_name(Kind::Shared, "greet"), expected_shared);
    }
}
