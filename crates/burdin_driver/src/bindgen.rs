//! `burdin bindgen`: C headers to Burdin declarations, Burdin exports to a C header (`spec/bindgen.md`), and a Rust
//! crate to a C-ABI shim plus Burdin declarations (`spec/rust-interop.md`).
//!
//! `burdin bindgen <header> --out <file>` reads a C header through libclang (the `burdin_bindgen` crate) and writes a
//! Burdin module of raw declarations. `burdin bindgen --export <target> --out <header.h>` goes the other way
//! (`export.rs`), and `burdin bindgen --rust <crate> --out <file> --shim <dir>` reads a Rust crate's rustdoc JSON
//! (`rust_interop.rs`). This file is the command line: the options, the exit codes, and the files written.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::DiagnosticOptions;

const USAGE: &str =
    "usage: burdin bindgen <header> --out <file> [-I <dir>]... [-D <name[=value]>]... [--clang-arg <arg>]...
                     [--wrappers <file.c>] [--wrapper-include <text>] [diagnostics]
       burdin bindgen --export <target> --out <header.h> [--guard <NAME>] [diagnostics]
       burdin bindgen --rust <crate> --out <file> --shim <dir> [--rust-toolchain <version>] [--rustdoc-json <file>]";

/// What the command line asked for.
struct Request {
    header: Option<String>,
    export: Option<String>,
    /// `--rust`: a Rust crate directory or `Cargo.toml`.
    rust: Option<String>,
    /// `--shim`: where the shim crate is written.
    shim: Option<String>,
    rust_toolchain: Option<String>,
    rustdoc_json: Option<String>,
    out: Option<String>,
    guard: Option<String>,
    clang_args: Vec<String>,
    wrappers: Option<String>,
    wrapper_include: Option<String>,
    /// How `--export` renders its diagnostics; it is the same group every other command takes.
    diagnostics: DiagnosticOptions,
}

pub(crate) fn run(args: &[String]) -> ExitCode {
    let request = match parse(args) {
        Ok(Some(request)) => request,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => return usage_error(&message),
    };
    let Some(out) = request.out.clone() else {
        return usage_error("missing `--out <file>`");
    };
    if let Some(krate) = &request.rust {
        if request.header.is_some() || request.export.is_some() {
            return usage_error("`--rust` takes a crate, not a header or `--export`");
        }
        if !request.clang_args.is_empty() || request.wrappers.is_some() || request.wrapper_include.is_some() {
            return usage_error("`--rust` takes no C compiler options and no wrapper options");
        }
        let Some(shim) = request.shim.clone() else {
            return usage_error("`--rust` needs `--shim <dir>`, where the shim crate is written");
        };
        return rust(&request, krate, &out, &shim);
    }
    if request.shim.is_some() || request.rust_toolchain.is_some() || request.rustdoc_json.is_some() {
        return usage_error("`--shim`, `--rust-toolchain`, and `--rustdoc-json` only apply to `--rust`");
    }
    if let Some(target) = &request.export {
        if request.header.is_some() {
            return usage_error("`--export` takes a package or file, not a header");
        }
        if !request.clang_args.is_empty() || request.wrappers.is_some() || request.wrapper_include.is_some() {
            return usage_error("`--export` takes no C compiler options and no wrapper options");
        }
        return crate::export::run(Path::new(target), Path::new(&out), request.guard.as_deref(), &request.diagnostics);
    }
    if request.guard.is_some() {
        return usage_error("`--guard` only applies to `--export`");
    }
    let Some(header) = request.header.clone() else {
        return usage_error("missing the header path");
    };
    import(&request, &header, &out)
}

/// `burdin bindgen --rust <crate>`: rustdoc's JSON for the crate (run here, or read from `--rustdoc-json`), then the
/// Burdin module and the shim crate. Exit 1 when the crate or its JSON cannot be read, 2 for a usage or I/O error.
fn rust(request: &Request, krate: &str, out: &str, shim: &str) -> ExitCode {
    let info = match crate::rust_interop::locate(Path::new(krate)) {
        Ok(info) => info,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };
    let pinned = request.rust_toolchain.as_deref();
    if let Some(version) = pinned
        && !crate::rust_interop::is_exact_version(version)
    {
        return usage_error("`--rust-toolchain` takes an exact version such as `1.100.0`");
    }
    let toolchain = match crate::rust_interop::verify_toolchain(pinned) {
        Ok(version) => version,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };
    let json = match &request.rustdoc_json {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("error: cannot read {path}: {e}");
                return ExitCode::from(2);
            }
        },
        None => match crate::rust_interop::rustdoc_json(&info, pinned) {
            Ok(text) => text,
            Err(message) => {
                eprintln!("error: {message}");
                return ExitCode::from(1);
            }
        },
    };
    let generated = match crate::rust_interop::generate(&info, &toolchain, &json, Path::new(shim)) {
        Ok(generated) => generated,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    };
    for note in &generated.notes {
        eprintln!("warning: {note}");
    }
    if let Err(code) = write_file(out, &generated.module) {
        return code;
    }
    if let Err(message) = crate::rust_interop::write_shim(Path::new(shim), &generated) {
        eprintln!("error: {message}");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

fn import(request: &Request, header: &str, out: &str) -> ExitCode {
    if let Err(e) = std::fs::metadata(header) {
        eprintln!("error: cannot read {header}: {e}");
        return ExitCode::from(2);
    }
    let wrapper_include = request.wrapper_include.clone().unwrap_or_else(|| header.replace('\\', "/"));
    let options = burdin_bindgen::Options {
        header: PathBuf::from(header),
        clang_args: request.clang_args.clone(),
        wrappers: request.wrappers.as_ref().map(|_| wrapper_include),
    };
    let generated = match burdin_bindgen::generate(&options) {
        Ok(generated) => generated,
        Err(burdin_bindgen::Error::Load(message)) | Err(burdin_bindgen::Error::Clang(message)) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
        Err(burdin_bindgen::Error::Parse(errors)) => {
            for error in &errors {
                eprintln!("error: {error}");
            }
            let plural = if errors.len() == 1 { "" } else { "s" };
            eprintln!("error: could not parse {header} ({} error{plural})", errors.len());
            return ExitCode::from(1);
        }
    };
    for warning in &generated.warnings {
        eprintln!("warning: {warning}");
    }
    if let Err(code) = write_file(out, &generated.module) {
        return code;
    }
    if let (Some(path), Some(text)) = (&request.wrappers, &generated.wrappers)
        && let Err(code) = write_file(path, text)
    {
        return code;
    }
    ExitCode::SUCCESS
}

/// Writes `text` to `path`, creating its directory. Exit code 2 on failure, like every I/O error of the command.
pub(crate) fn write_file(path: &str, text: &str) -> Result<(), ExitCode> {
    if let Some(parent) = Path::new(path).parent().filter(|parent| !parent.as_os_str().is_empty())
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("error: cannot create {}: {e}", parent.display());
        return Err(ExitCode::from(2));
    }
    if let Err(e) = std::fs::write(path, text) {
        eprintln!("error: cannot write {path}: {e}");
        return Err(ExitCode::from(2));
    }
    Ok(())
}

/// Reads the arguments. `Ok(None)` means `--help` was printed.
fn parse(args: &[String]) -> Result<Option<Request>, String> {
    let mut request = Request {
        header: None,
        export: None,
        rust: None,
        shim: None,
        rust_toolchain: None,
        rustdoc_json: None,
        out: None,
        guard: None,
        clang_args: Vec::new(),
        wrappers: None,
        wrapper_include: None,
        diagnostics: DiagnosticOptions::default(),
    };
    let mut iterator = args.iter();
    while let Some(raw) = iterator.next() {
        let (option, inline) = match raw.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_string())),
            _ => (raw.as_str(), None),
        };
        // The `[diagnostics]` group is the one `build`, `run`, `test`, `check`, `lint`, and `doc` take, parsed by the
        // same function so the two can never disagree.
        if crate::parse_diagnostic_option(&mut iterator, inline.as_deref(), option, &mut request.diagnostics)? {
            continue;
        }
        let mut value = |name: &str| -> Result<String, String> {
            match &inline {
                Some(value) => Ok(value.clone()),
                None => iterator.next().cloned().ok_or_else(|| format!("`{name}` needs a value")),
            }
        };
        match option {
            "--out" | "-o" => request.out = Some(value("--out")?),
            "--export" => request.export = Some(value("--export")?),
            "--rust" => request.rust = Some(value("--rust")?),
            "--shim" => request.shim = Some(value("--shim")?),
            "--rust-toolchain" => request.rust_toolchain = Some(value("--rust-toolchain")?),
            "--rustdoc-json" => request.rustdoc_json = Some(value("--rustdoc-json")?),
            "--guard" => request.guard = Some(value("--guard")?),
            "--wrappers" => request.wrappers = Some(value("--wrappers")?),
            "--wrapper-include" => request.wrapper_include = Some(value("--wrapper-include")?),
            "--clang-arg" => request.clang_args.push(value("--clang-arg")?),
            "-I" => request.clang_args.push(format!("-I{}", value("-I")?)),
            "-D" => request.clang_args.push(format!("-D{}", value("-D")?)),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            _ if option.starts_with("-I") || option.starts_with("-D") => request.clang_args.push(raw.clone()),
            _ if option.starts_with('-') => return Err(format!("unknown option `{option}`")),
            _ if request.header.is_none() => request.header = Some(raw.clone()),
            _ => return Err(format!("unexpected argument `{option}`")),
        }
    }
    Ok(Some(request))
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("error: {message}\n{USAGE}");
    ExitCode::from(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_header_and_an_output_are_read() {
        let request = parse(&args(&["a.h", "--out", "a.bd"])).unwrap().unwrap();
        assert_eq!(request.header.as_deref(), Some("a.h"));
        assert_eq!(request.out.as_deref(), Some("a.bd"));
    }

    #[test]
    fn c_compiler_options_are_collected_in_order() {
        let request = parse(&args(&["a.h", "-I", "inc", "-Dx=1", "-I=ignored", "--clang-arg=-std=c99"])).unwrap();
        let request = request.unwrap();
        assert_eq!(request.clang_args, ["-Iinc", "-Dx=1", "-I=ignored", "-std=c99"]);
    }

    #[test]
    fn the_rust_options_are_read() {
        let request = parse(&args(&[
            "--rust",
            "crates/geometry",
            "--out",
            "geometry.bd",
            "--shim=shim",
            "--rust-toolchain",
            "1.100.0",
            "--rustdoc-json",
            "doc.json",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(request.rust.as_deref(), Some("crates/geometry"));
        assert_eq!(request.shim.as_deref(), Some("shim"));
        assert_eq!(request.rust_toolchain.as_deref(), Some("1.100.0"));
        assert_eq!(request.rustdoc_json.as_deref(), Some("doc.json"));
    }

    #[test]
    fn export_and_wrapper_options_are_read() {
        let request = parse(&args(&["--export", "pkg", "--out", "pkg.h", "--guard=PKG_H"])).unwrap().unwrap();
        assert_eq!(request.export.as_deref(), Some("pkg"));
        assert_eq!(request.guard.as_deref(), Some("PKG_H"));
        let request = parse(&args(&["a.h", "--wrappers", "w.c", "--wrapper-include", "a.h"])).unwrap().unwrap();
        assert_eq!(request.wrappers.as_deref(), Some("w.c"));
        assert_eq!(request.wrapper_include.as_deref(), Some("a.h"));
    }

    #[test]
    fn the_diagnostic_options_are_the_same_group_as_every_other_command() {
        let request = parse(&args(&["--export", "pkg", "--out", "pkg.h", "--diagnostic-format=json"])).unwrap();
        let request = request.unwrap();
        assert_eq!(request.diagnostics.format, crate::DiagnosticFormat::Json);
        let request = parse(&args(&["--export", "pkg", "--out", "pkg.h", "--no-color", "--width", "100"])).unwrap();
        let request = request.unwrap();
        assert_eq!(request.diagnostics.color, crate::ColorChoice::Never);
        assert_eq!(request.diagnostics.width, Some(100));
        assert!(parse(&args(&["--export", "pkg", "--out", "pkg.h", "--diagnostic-format=xml"])).is_err());
    }

    #[test]
    fn bad_command_lines_are_usage_errors() {
        assert!(parse(&args(&["a.h", "--bogus"])).is_err());
        assert!(parse(&args(&["a.h", "b.h"])).is_err());
        assert!(parse(&args(&["a.h", "--out"])).is_err());
        assert!(parse(&args(&["--help"])).unwrap().is_none());
    }
}
