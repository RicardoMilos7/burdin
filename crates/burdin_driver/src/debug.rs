//! `burdin debug <target> [-- args]` (`spec/debug.md`, DBG9-DBG10): compiles the target with `-g` into a
//! temporary directory and starts a debugger on it, with the Burdin pretty-printers loaded.
//!
//! The debugger is interactive and owns the terminal; the command passes its exit status through. Nothing about
//! `burdin build` changes: this command only picks the profile, the `-g` flag, and where the executable goes.

use crate::{
    ColorChoice, CompileOptions, DiagnosticOptions, TempDir, USAGE, compile, jobs_or_default, parse_color,
    parse_format, parse_number,
};
use burdin_diag::DiagnosticFormat;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// The lldb formatters, embedded at compile time (`spec/debug.md`, DBG7): lldb has no embedded-script section, so
/// a session writes the file beside it and imports it. The gdb printers need no such step — `-g` embeds them in
/// the binary itself (DBG6).
const LLDB_FORMATTERS: &str = include_str!("../../../debuggers/lldb/burdin_formatters.py");

/// `burdin debug`'s command line.
#[derive(Debug)]
pub(crate) struct Args {
    pub(crate) target: PathBuf,
    /// `--debugger <name | path>`: `gdb`, `lldb`, `cdb`, or a program to run instead (which is what the tests do).
    pub(crate) debugger: Option<String>,
    /// `--test <name | index>`: debug one test block.
    pub(crate) test: Option<String>,
    /// `--break-on-panic`: start with a breakpoint on `burdin_panic_begin` (DBG1).
    pub(crate) break_on_panic: bool,
    /// Everything after `--`, passed to the program.
    pub(crate) program_args: Vec<String>,
    pub(crate) release: bool,
    pub(crate) jobs: Option<usize>,
    pub(crate) diagnostics: DiagnosticOptions,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            target: PathBuf::new(),
            debugger: None,
            test: None,
            break_on_panic: false,
            program_args: Vec::new(),
            release: false,
            jobs: None,
            diagnostics: DiagnosticOptions {
                format: DiagnosticFormat::Human,
                color: ColorChoice::Auto,
                width: None,
                error_limit: None,
            },
        }
    }
}

pub(crate) fn parse(args: &[String]) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut rest = args.iter();
    let mut target: Option<PathBuf> = None;
    while let Some(arg) = rest.next() {
        if arg == "--" {
            parsed.program_args = rest.cloned().collect();
            break;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value)),
            _ => (arg.as_str(), None),
        };
        match name {
            "--debugger" => {
                let value = match inline {
                    Some(value) => value.to_string(),
                    None => rest.next().cloned().ok_or("`--debugger` needs a debugger name or path")?,
                };
                parsed.debugger = Some(value);
            }
            "--test" => {
                let value = match inline {
                    Some(value) => value.to_string(),
                    None => rest.next().cloned().ok_or("`--test` needs a test name or index")?,
                };
                parsed.test = Some(value);
            }
            "--break-on-panic" => {
                if inline.is_some() {
                    return Err("`--break-on-panic` takes no value".to_string());
                }
                parsed.break_on_panic = true;
            }
            "--release" => {
                if inline.is_some() {
                    return Err("`--release` takes no value".to_string());
                }
                parsed.release = true;
            }
            "-j" | "--jobs" => {
                let value = match inline {
                    Some(value) => value.to_string(),
                    None => rest.next().cloned().ok_or("`-j` needs a number of jobs")?,
                };
                let jobs = parse_number("-j", &value)?;
                if jobs == 0 {
                    return Err("`-j` must be at least 1".to_string());
                }
                parsed.jobs = Some(jobs);
            }
            "--diagnostic-format" => {
                let value = match inline {
                    Some(value) => value.to_string(),
                    None => rest.next().cloned().ok_or("`--diagnostic-format` needs a format")?,
                };
                parsed.diagnostics.format = parse_format(&value)?;
            }
            "--color" => {
                let value = match inline {
                    Some(value) => value.to_string(),
                    None => rest.next().cloned().ok_or("`--color` needs a choice")?,
                };
                parsed.diagnostics.color = parse_color(&value)?;
            }
            "--no-color" => parsed.diagnostics.color = ColorChoice::Never,
            "--width" => {
                let value = match inline {
                    Some(value) => value.to_string(),
                    None => rest.next().cloned().ok_or("`--width` needs a column count")?,
                };
                let width = parse_number("--width", &value)?;
                parsed.diagnostics.width = Some(width);
            }
            "--error-limit" => {
                let value = match inline {
                    Some(value) => value.to_string(),
                    None => rest.next().cloned().ok_or("`--error-limit` needs a number")?,
                };
                parsed.diagnostics.error_limit = Some(parse_number("--error-limit", &value)?);
            }
            other if other.starts_with('-') => return Err(format!("unknown argument `{other}` for `burdin debug`")),
            other => {
                if target.is_some() {
                    return Err(format!(
                        "`burdin debug` takes one target, and `{other}` is a second one (program arguments go \
                         after `--`)"
                    ));
                }
                target = Some(PathBuf::from(other));
            }
        }
    }
    parsed.target = target.ok_or("`burdin debug` needs a target: a `.bd` file or a package directory")?;
    Ok(parsed)
}

/// The debugger a session starts: `--debugger`'s choice resolved to a program, or the platform's default.
fn debugger_program(choice: Option<&str>) -> Result<String, String> {
    match choice {
        Some("gdb") | Some("lldb") | Some("cdb") => Ok(choice.expect("matched").to_string()),
        Some(program) => Ok(program.to_string()),
        None if cfg!(target_os = "macos") => Ok("lldb".to_string()),
        None if cfg!(windows) => Ok("cdb".to_string()),
        None => {
            if on_path("gdb") {
                Ok("gdb".to_string())
            } else if on_path("lldb") {
                Ok("lldb".to_string())
            } else {
                Err("no debugger found: install gdb or lldb, or name one with `--debugger`".to_string())
            }
        }
    }
}

fn on_path(program: &str) -> bool {
    let paths = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(program);
        candidate.is_file() || candidate.with_extension("exe").is_file()
    })
}

/// The test a `--test` value names, resolved like the runner's filters: `--test <index>` is the test at that
/// position of the compiled list, and a name is the first test whose full path (`module.name`) contains it
/// (`spec/debug.md`, DBG10; `spec/test-runner.md`).
fn resolve_test(tests: &[burdin_tir::TestEntry], query: &str) -> Result<usize, String> {
    let full = |test: &burdin_tir::TestEntry| {
        if test.module.is_empty() { test.name.clone() } else { format!("{}.{}", test.module, test.name) }
    };
    if let Ok(index) = query.parse::<usize>() {
        return if index < tests.len() {
            Ok(index)
        } else {
            Err(format!("`--test {index}` is past the last test: this target has {} test(s)", tests.len()))
        };
    }
    tests
        .iter()
        .position(|test| full(test).contains(query))
        .ok_or_else(|| format!("no test path contains `{query}` (run `burdin test --list` to see them)"))
}

/// The debugger's argument list: the commands that load the printers and set the panic breakpoint, then the
/// program and its arguments.
fn debugger_command(
    program: &str,
    exe: &Path,
    formatters: Option<&Path>,
    args: &Args,
    program_args: &[String],
) -> Command {
    let mut command = Command::new(program);
    match command_name(program) {
        "lldb" => {
            if let Some(formatters) = formatters {
                command.arg("-o").arg(format!("command script import {}", formatters.display()));
            }
            if args.break_on_panic {
                command.arg("-o").arg("breakpoint set --name burdin_panic_begin");
            }
            command.arg("--").arg(exe);
            command.args(program_args);
        }
        "cdb" => {
            if args.break_on_panic {
                command.arg("-c").arg("bp burdin_panic_begin; g");
            }
            command.arg(exe);
            command.args(program_args);
        }
        _ => {
            // gdb, and anything else that speaks its command line. `auto-load safe-path /` is what lets gdb read
            // the `.debug_gdb_scripts` section the printers are embedded in (DBG6).
            command.arg("-q");
            command.arg("-iex").arg("set auto-load safe-path /");
            if args.break_on_panic {
                command.arg("-ex").arg("break burdin_panic_begin");
            }
            command.arg("--args").arg(exe);
            command.args(program_args);
        }
    }
    command
}

/// The debugger's name without a path or an `.exe`, to pick its command line.
fn command_name(program: &str) -> &str {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let name = name.strip_suffix(".exe").unwrap_or(name);
    if name.starts_with("lldb") {
        "lldb"
    } else if name.starts_with("cdb") {
        "cdb"
    } else {
        "gdb"
    }
}

pub(crate) fn run(words: &[String]) -> ExitCode {
    let args = match parse(words) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("error: {message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let target = args.target.clone();
    let dir = match TempDir::new() {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("error: cannot create a temporary directory: {error}");
            return ExitCode::from(2);
        }
    };
    let stem = target.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "program".to_string());
    let exe = dir.path.join(stem).with_extension(std::env::consts::EXE_EXTENSION);
    let options = CompileOptions {
        release: args.release,
        debug: true,
        strip: false,
        lto: false,
        pgo_generate: false,
        pgo_use: None,
        emit_ir: None,
        emit: None,
        link_libs: &[],
        mode: if args.test.is_some() { crate::EntryMode::Test } else { crate::EntryMode::Build },
        diagnostics: &args.diagnostics,
        comptime_steps: None,
        emit_deps: None,
        report_clean: false,
        run_build_scripts: true,
        jobs: jobs_or_default(args.jobs),
    };
    let built = match compile(&target, &dir.path, Some(&exe), &options) {
        Ok(built) => built,
        Err(code) => return code,
    };
    let mut program_args = args.program_args.clone();
    if let Some(query) = &args.test {
        match resolve_test(&built.tests, query) {
            Ok(index) => program_args.insert(0, index.to_string()),
            Err(message) => {
                eprintln!("error: {message}");
                return ExitCode::from(2);
            }
        }
    }
    let program = match debugger_program(args.debugger.as_deref()) {
        Ok(program) => program,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };
    // lldb imports the formatters from a file written beside the session (DBG7); the others need nothing, and an
    // arbitrary `--debugger` program gets no import line either.
    let formatters = (command_name(&program) == "lldb").then(|| {
        let path = dir.path.join("burdin_formatters.py");
        std::fs::write(&path, LLDB_FORMATTERS).ok().map(|_| path)
    });
    let formatters = formatters.flatten();
    let status = debugger_command(&program, &built.exe, formatters.as_deref(), &args, &program_args).status();
    match status {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(error) => {
            eprintln!("error: cannot start the debugger `{program}`: {error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_words(words: &[&str]) -> Result<Args, String> {
        parse(&words.iter().map(|w| w.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn a_target_is_required() {
        assert!(parse_words(&[]).unwrap_err().contains("needs a target"));
    }

    #[test]
    fn program_arguments_follow_the_separator() {
        let args = parse_words(&["app", "--", "--flag", "x"]).unwrap();
        assert_eq!(args.target, PathBuf::from("app"));
        assert_eq!(args.program_args, ["--flag", "x"]);
    }

    #[test]
    fn a_second_target_is_a_usage_error() {
        let error = parse_words(&["app", "other"]).unwrap_err();
        assert!(error.contains("one target"), "{error}");
    }

    #[test]
    fn options_take_their_values_inline_or_next() {
        let args = parse_words(&["app", "--debugger=lldb", "--test", "loads"]).unwrap();
        assert_eq!(args.debugger.as_deref(), Some("lldb"));
        assert_eq!(args.test.as_deref(), Some("loads"));
        assert!(parse_words(&["app", "--debugger"]).unwrap_err().contains("needs a debugger"));
    }

    #[test]
    fn unknown_options_are_rejected() {
        assert!(parse_words(&["app", "--wat"]).unwrap_err().contains("unknown argument"));
    }

    #[test]
    fn the_platform_debugger_speaks_its_own_command_line() {
        let args = Args { break_on_panic: true, ..Args::default() };
        let exe = Path::new("app");
        let gdb = debugger_command("gdb", exe, None, &args, &["x".to_string()]);
        let gdb: Vec<_> = gdb.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert!(gdb.contains(&"break burdin_panic_begin".to_string()), "{gdb:?}");
        assert!(gdb.contains(&"set auto-load safe-path /".to_string()), "{gdb:?}");
        assert_eq!(&gdb[gdb.len() - 2..], ["app", "x"]);

        let lldb = debugger_command("lldb", exe, Some(Path::new("f.py")), &args, &[]);
        let lldb: Vec<_> = lldb.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert!(lldb.contains(&"command script import f.py".to_string()), "{lldb:?}");
        assert!(lldb.contains(&"breakpoint set --name burdin_panic_begin".to_string()), "{lldb:?}");

        let cdb = debugger_command("cdb", exe, None, &args, &[]);
        let cdb: Vec<_> = cdb.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert!(cdb.contains(&"bp burdin_panic_begin; g".to_string()), "{cdb:?}");
    }

    #[test]
    fn a_named_program_is_used_as_it_is() {
        assert_eq!(debugger_program(Some("lldb")).unwrap(), "lldb");
        assert_eq!(debugger_program(Some("./stub-debugger")).unwrap(), "./stub-debugger");
    }
}
