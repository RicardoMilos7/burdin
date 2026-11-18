//! `burdin check`: type-check only (`spec/check.md`). The command runs the same front end as `burdin build` (build
//! scripts, parsing, name resolution and type checking, the memory checks, and compile-time evaluation) and stops
//! before code generation, so it needs no LLVM work and no linker. It prints the same diagnostics, in the same
//! formats, and exits 0 exactly when `burdin build` would get as far as code generation.

use crate::{
    CompileOptions, DiagnosticOptions, EntryMode, SourceOverrides, flag_without_value, front_end, jobs_or_default,
    option_value, parse_diagnostic_option, parse_jobs, print_warnings, report_errors,
};
use std::path::PathBuf;
use std::process::ExitCode;

pub(crate) const CHECK_USAGE: &str = "\
usage:
  burdin check <file.bd | package directory> [--release] [--tests] [-j <n>] [--comptime-steps <n>] [diagnostics]

check options:
  --release            check the way `burdin build --release` does (integer overflow in compile-time code, error traces)
  --tests              also check the package's `test` blocks, and do not require a `main` (a library package)
  -j, --jobs <n>       parse on at most n threads (default: BURDIN_JOBS, then the number of CPUs)
  --comptime-steps <n> the step budget of each compile-time evaluation (default: 10000000)

Nothing is printed when the package has no errors and no warnings; the exit status is 0 then and 1 otherwise.";

#[derive(Debug, Default)]
struct CheckArgs {
    target: PathBuf,
    release: bool,
    tests: bool,
    comptime_steps: Option<u64>,
    jobs: Option<usize>,
    diagnostics: DiagnosticOptions,
}

fn parse(argv: &[String]) -> Result<CheckArgs, String> {
    let mut args = CheckArgs::default();
    let mut target = None;
    let mut it = argv.iter();
    while let Some(raw) = it.next() {
        let (a, inline) = match raw.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            // `-j4` is `-j 4`, as in cargo and make.
            _ if raw.len() > 2 && raw.starts_with("-j") => ("-j", Some(&raw[2..])),
            _ => (raw.as_str(), None),
        };
        if parse_diagnostic_option(&mut it, inline, a, &mut args.diagnostics)? {
            continue;
        }
        match a {
            "--release" => args.release = flag_without_value(a, inline)?,
            "--tests" => args.tests = flag_without_value(a, inline)?,
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
            _ if target.is_none() => target = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument `{a}`")),
        }
    }
    args.target = target.ok_or("missing target file")?;
    Ok(args)
}

pub(crate) fn run(argv: &[String]) -> ExitCode {
    if matches!(argv, [flag] if flag == "-h" || flag == "--help") {
        println!("{CHECK_USAGE}");
        return ExitCode::SUCCESS;
    }
    let args = match parse(argv) {
        Ok(a) => a,
        Err(message) => {
            eprintln!("error: {message}\n{CHECK_USAGE}");
            return ExitCode::from(2);
        }
    };
    let options = CompileOptions {
        release: args.release,
        debug: false,
        strip: false,
        lto: false,
        pgo_generate: false,
        pgo_use: None,
        emit_ir: None,
        link_libs: &[],
        mode: if args.tests { EntryMode::Test } else { EntryMode::Build },
        diagnostics: &args.diagnostics,
        comptime_steps: args.comptime_steps,
        emit_deps: None,
        // A CI gate gets a log even when the package is clean, like `burdin build` (`spec/diagnostics.md`, DG11).
        report_clean: true,
        run_build_scripts: true,
        jobs: jobs_or_default(args.jobs),
    };
    let front = match front_end(&args.target, &options, &SourceOverrides::new()) {
        Ok(front) => front,
        Err(code) => return code,
    };
    if front.diags.has_errors() {
        report_errors("check", &args.target, &front.diags, &front.sources, &args.diagnostics);
        return ExitCode::from(1);
    }
    print_warnings(&front.diags, &front.sources, &args.diagnostics, options.report_clean);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn a_target_alone_checks_like_a_plain_build() {
        let args = parse(&argv(&["app"])).unwrap();
        assert_eq!(args.target, PathBuf::from("app"));
        assert!(!args.release && !args.tests);
        assert_eq!(args.comptime_steps, None);
    }

    #[test]
    fn options_and_diagnostics_flags_are_read_in_any_order() {
        let args = parse(&argv(&["--tests", "app", "--release", "--comptime-steps=50", "--error-limit", "3"])).unwrap();
        assert!(args.release && args.tests);
        assert_eq!(args.comptime_steps, Some(50));
        assert_eq!(args.diagnostics.error_limit, Some(3));
    }

    #[test]
    fn usage_errors() {
        assert!(parse(&argv(&[])).unwrap_err().contains("missing target"));
        assert!(parse(&argv(&["a", "b"])).unwrap_err().contains("unexpected argument `b`"));
        assert!(parse(&argv(&["a", "--bogus"])).unwrap_err().contains("unknown option `--bogus`"));
        assert!(parse(&argv(&["a", "--release=yes"])).unwrap_err().contains("takes no value"));
        assert!(parse(&argv(&["a", "--comptime-steps", "0"])).unwrap_err().contains("positive number"));
        assert!(parse(&argv(&["a", "--diagnostic-format", "xml"])).unwrap_err().contains("unknown diagnostic format"));
        // Options of the other commands are not accepted here.
        assert!(parse(&argv(&["a", "--emit-ir", "x.ll"])).unwrap_err().contains("unknown option"));
        assert!(parse(&argv(&["a", "-o", "x"])).unwrap_err().contains("unknown option"));
    }
}
