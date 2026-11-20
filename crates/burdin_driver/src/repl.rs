//! `burdin repl`: the interactive session (`spec/repl.md`). The command parses its options, builds the two
//! closures that need the LLVM back end (`:compile` and `:ir`), and runs the REPL crate; everything else —
//! checking, evaluating, printing, the meta-commands — is the crate's, over the query library and the comptime
//! interpreter.

use crate::{
    CompileOptions, DiagnosticOptions, TempDir, compile, flag_without_value, jobs_or_default, option_value,
    parse_diagnostic_option, parse_jobs, render_options,
};
use std::io::IsTerminal as _;
use std::path::Path;
use std::process::{Command, ExitCode};

#[derive(Debug, Default)]
struct ReplArgs {
    release: bool,
    comptime_steps: Option<u64>,
    jobs: Option<usize>,
    diagnostics: DiagnosticOptions,
}

fn parse(argv: &[String]) -> Result<ReplArgs, String> {
    let mut args = ReplArgs::default();
    let mut it = argv.iter();
    while let Some(raw) = it.next() {
        let (a, inline) = match raw.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            _ if raw.len() > 2 && raw.starts_with("-j") => ("-j", Some(&raw[2..])),
            _ => (raw.as_str(), None),
        };
        if parse_diagnostic_option(&mut it, inline, a, &mut args.diagnostics)? {
            continue;
        }
        match a {
            "--release" => args.release = flag_without_value(a, inline)?,
            "-j" | "--jobs" => args.jobs = Some(parse_jobs(&mut it, inline, a)?),
            "--comptime-steps" => {
                let value = option_value(&mut it, inline, a, "a number of steps")?;
                let steps: u64 = value
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("`--comptime-steps` needs a positive number of steps, got `{value}`"))?;
                args.comptime_steps = Some(steps);
            }
            _ if a.starts_with('-') => return Err(format!("unknown option `{a}`")),
            _ => return Err(format!("unexpected argument `{a}`: `burdin repl` reads its session from standard input")),
        }
    }
    Ok(args)
}

/// The options every native build of the session uses (`:compile`, `:ir`).
fn session_options<'a>(args: &'a ReplArgs, jobs: usize, emit_ir: Option<&'a Path>) -> CompileOptions<'a> {
    CompileOptions {
        release: args.release,
        debug: false,
        strip: false,
        lto: false,
        pgo_generate: false,
        pgo_use: None,
        emit_ir,
        emit: None,
        link_libs: &[],
        // The session's `test` items are checked by the interpreter's analysis; a native build is an ordinary
        // program, so they are stripped as in every build.
        mode: crate::EntryMode::Build,
        diagnostics: &args.diagnostics,
        comptime_steps: args.comptime_steps,
        emit_deps: None,
        report_clean: false,
        // A one-file target has no package directory, so there is no build script to run.
        run_build_scripts: false,
        locked: false,
        features: Default::default(),
        jobs,
        sanitize: Default::default(),
        fuzz_target: None,
    }
}

pub(crate) fn run(argv: &[String]) -> ExitCode {
    let args = match parse(argv) {
        Ok(a) => a,
        Err(message) => return crate::cli::usage_error("repl", &message),
    };
    let jobs = jobs_or_default(args.jobs);
    let native = burdin_repl::Native {
        // `:compile`: the session text is a one-file program, built with the ordinary pipeline and run as a
        // child process, its output inherited (RP10).
        compile: Box::new(|text| {
            let Ok(dir) = TempDir::new() else {
                eprintln!("error: cannot create a temporary directory");
                return Err(());
            };
            let session = dir.path.join("session.bd");
            if let Err(e) = std::fs::write(&session, text) {
                eprintln!("error: cannot write {}: {e}", session.display());
                return Err(());
            }
            let options = session_options(&args, jobs, None);
            match compile(&session, &dir.path, None, &options) {
                Ok(built) => match Command::new(&built.exe).status() {
                    Ok(status) => Ok(exit_code_of(status)),
                    Err(e) => {
                        eprintln!("error: cannot run {}: {e}", built.exe.display());
                        Err(())
                    }
                },
                Err(_) => Err(()),
            }
        }),
        // `:ir <fn>`: the same build with `--emit-ir`, and the named function's definition out of the module.
        ir: Box::new(|text, name| session_ir(text, name, &args, jobs)),
    };
    let mut config = burdin_repl::Config {
        release: args.release,
        steps: args.comptime_steps,
        jobs,
        render: render_options(&args.diagnostics, std::io::stderr().is_terminal()),
        native,
    };
    let code = burdin_repl::run(&mut config);
    ExitCode::from(code as u8)
}

fn exit_code_of(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        // Windows exit codes are 32-bit; keep a failure a failure when truncating.
        let low = code & 0xff;
        return if code != 0 && low == 0 { 1 } else { low };
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128i32.wrapping_add(signal);
        }
    }
    1
}

/// The LLVM IR of the function `name` in the session's program: the `define` block the back end emitted, or an
/// error naming what is wrong. Every function the session declares under that name matches, so a method
/// (`Rect.new`) and its instantiations all print.
fn session_ir(text: &str, name: &str, args: &ReplArgs, jobs: usize) -> Result<String, String> {
    let dir = TempDir::new().map_err(|e| format!("cannot create a temporary directory: {e}"))?;
    let session = dir.path.join("session.bd");
    std::fs::write(&session, text).map_err(|e| format!("cannot write {}: {e}", session.display()))?;
    let ir = dir.path.join("session.ll");
    let options = session_options(args, jobs, Some(&ir));
    compile(&session, &dir.path, Some(&dir.path.join("session.exe")), &options)
        .map_err(|_| "the session did not compile; the errors are above".to_string())?;
    let module = std::fs::read_to_string(&ir).map_err(|e| format!("cannot read {}: {e}", ir.display()))?;
    let lines: Vec<&str> = module.lines().collect();
    let mut out = String::new();
    let mut at = 0;
    while at < lines.len() {
        let defined = lines[at].starts_with("define ");
        let named = defined && lines[at].contains(&format!("@burdin.{name}("));
        if !named {
            at += 1;
            continue;
        }
        out.push_str(lines[at]);
        out.push('\n');
        at += 1;
        while at < lines.len() {
            out.push_str(lines[at]);
            out.push('\n');
            let closed = lines[at] == "}";
            at += 1;
            if closed {
                break;
            }
        }
    }
    if out.is_empty() {
        return Err(format!(
            "the session declares no function `{name}`; :ir takes a name the session declares, as in `:ir main`"
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn the_repl_takes_its_own_options_only() {
        let args = parse(&argv(&["--release", "--comptime-steps=50"])).unwrap();
        assert!(args.release);
        assert_eq!(args.comptime_steps, Some(50));
        assert_eq!(parse(&argv(&["-j", "3"])).unwrap().jobs, Some(3));
    }

    #[test]
    fn a_repl_reads_its_session_from_standard_input() {
        assert!(parse(&argv(&["app"])).unwrap_err().contains("standard input"));
        assert!(parse(&argv(&["--bogus"])).unwrap_err().contains("unknown option `--bogus`"));
        assert!(parse(&argv(&["--release=yes"])).unwrap_err().contains("takes no value"));
        assert!(parse(&argv(&["--comptime-steps", "0"])).unwrap_err().contains("positive number"));
        assert!(parse(&argv(&["--diagnostic-format", "xml"])).unwrap_err().contains("unknown diagnostic format"));
    }
}
