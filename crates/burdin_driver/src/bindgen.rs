//! `burdin bindgen`: C headers to Burdin declarations, and Burdin exports to a C header (`spec/bindgen.md`).
//!
//! `burdin bindgen <header> --out <file>` reads a C header through libclang (the `burdin_bindgen` crate) and writes a
//! Burdin module of raw declarations. `burdin bindgen --export <target> --out <header.h>` goes the other way
//! (`export.rs`). This file is the command line: the options, the exit codes, and the files written.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::{ColorChoice, DiagnosticOptions};

const USAGE: &str =
    "usage: burdin bindgen <header> --out <file> [-I <dir>]... [-D <name[=value]>]... [--clang-arg <arg>]...
                     [--wrappers <file.c>] [--wrapper-include <text>] [diagnostics]
       burdin bindgen --export <target> --out <header.h> [--guard <NAME>] [diagnostics]";

/// What the command line asked for.
struct Request {
    header: Option<String>,
    export: Option<String>,
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
        let mut value = |name: &str| -> Result<String, String> {
            match &inline {
                Some(value) => Ok(value.clone()),
                None => iterator.next().cloned().ok_or_else(|| format!("`{name}` needs a value")),
            }
        };
        match option {
            "--out" | "-o" => request.out = Some(value("--out")?),
            "--export" => request.export = Some(value("--export")?),
            "--guard" => request.guard = Some(value("--guard")?),
            "--wrappers" => request.wrappers = Some(value("--wrappers")?),
            "--wrapper-include" => request.wrapper_include = Some(value("--wrapper-include")?),
            "--clang-arg" => request.clang_args.push(value("--clang-arg")?),
            "--diagnostic-format" => request.diagnostics.format = crate::parse_format(&value("--diagnostic-format")?)?,
            "--color" => request.diagnostics.color = crate::parse_color(&value("--color")?)?,
            "--no-color" => request.diagnostics.color = ColorChoice::Never,
            "--width" => request.diagnostics.width = Some(crate::parse_number("--width", &value("--width")?)?),
            "--error-limit" => {
                let limit = crate::parse_number("--error-limit", &value("--error-limit")?)?;
                request.diagnostics.error_limit = (limit > 0).then_some(limit);
            }
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
        assert_eq!(request.diagnostics.color, ColorChoice::Never);
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
