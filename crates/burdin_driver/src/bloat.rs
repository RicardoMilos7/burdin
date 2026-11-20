//! `burdin bloat`: where a binary's size comes from (`spec/inspect.md`, INS16-INS19). The program is built the way
//! `burdin build --release` builds it, as one object file, and the symbol sizes are read from that object.

use crate::emit::{Emit, EmitKind, artifact_path};
use crate::{
    CompileOptions, DiagnosticOptions, EntryMode, FrontEnd, SourceOverrides, TempDir, emit_program, flag_without_value,
    front_end, jobs_or_default, option_value, parse_diagnostic_option, parse_jobs, print_warnings, report_errors,
};
use burdin_inspect::bloat::{Known, Owner, Table};
use burdin_inspect::instances::{generic_name, is_instance};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug)]
struct BloatArgs {
    target: PathBuf,
    debug_profile: bool,
    top: usize,
    json: bool,
    comptime_steps: Option<u64>,
    jobs: Option<usize>,
    diagnostics: DiagnosticOptions,
}

fn parse(argv: &[String]) -> Result<BloatArgs, String> {
    let mut args = BloatArgs {
        target: PathBuf::new(),
        debug_profile: false,
        top: 20,
        json: false,
        comptime_steps: None,
        jobs: None,
        diagnostics: DiagnosticOptions::default(),
    };
    let mut target = None;
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
            "--debug" => args.debug_profile = flag_without_value(a, inline)?,
            "--top" => {
                let value = option_value(&mut it, inline, a, "a number")?;
                args.top = value
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("`--top` needs a positive number, got `{value}`"))?;
            }
            "--format" => {
                args.json = match option_value(&mut it, inline, a, "a value")?.as_str() {
                    "human" => false,
                    "json" => true,
                    other => return Err(format!("`--format` takes `human` or `json`, got `{other}`")),
                };
            }
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
    let args = match parse(argv) {
        Ok(a) => a,
        Err(message) => return crate::cli::usage_error("bloat", &message),
    };
    // One codegen unit, so there is one object file to read.
    let emit = Emit { kinds: vec![EmitKind::Obj], ..Emit::default() };
    let options = CompileOptions {
        release: !args.debug_profile,
        debug: false,
        strip: false,
        lto: false,
        pgo_generate: false,
        pgo_use: None,
        emit_ir: None,
        emit: Some(&emit),
        link_libs: &[],
        mode: EntryMode::Build,
        diagnostics: &args.diagnostics,
        comptime_steps: args.comptime_steps,
        emit_deps: None,
        report_clean: false,
        run_build_scripts: true,
        locked: false,
        offline: false,
        jobs: jobs_or_default(args.jobs),
        sanitize: Default::default(),
        fuzz_target: None,
    };
    let FrontEnd { loaded, sources, program, diags, rust_libs, .. } =
        match front_end(&args.target, &options, &SourceOverrides::new()) {
            Ok(front) => front,
            Err(code) => return code,
        };
    if diags.has_errors() {
        report_errors("compile", &args.target, &diags, &sources, &args.diagnostics);
        return ExitCode::from(1);
    }
    print_warnings(&diags, &sources, &args.diagnostics, false);
    let program = program.expect("no errors means checking produced a program");

    let dir = match TempDir::new() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("error: cannot create a temporary directory: {e}");
            return ExitCode::from(2);
        }
    };
    let output = dir.path.join(&loaded.stem).with_extension(std::env::consts::EXE_EXTENSION);
    if let Err(code) = emit_program(&program, &sources, &loaded.stem, &output, &options, &rust_libs) {
        return code;
    }
    let object = artifact_path(&output, EmitKind::Obj);
    let symbols = match std::fs::read(&object)
        .map_err(|e| format!("cannot read {}: {e}", object.display()))
        .and_then(|bytes| burdin_inspect::object::read_symbols(&bytes))
    {
        Ok(symbols) => symbols,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };

    // The module of each source file, numbered the way the front end numbers its files: package by package, module
    // by module.
    let mut owners: Vec<Owner> = Vec::new();
    for package in &loaded.packages {
        for module in &package.modules {
            owners.push(Owner { package: package.name.clone(), module: module.path.join(".") });
        }
    }
    let owner_of = |file: burdin_diag::FileId| {
        owners.get(file.0 as usize).cloned().unwrap_or(Owner { package: "?".to_string(), module: String::new() })
    };
    let mut table = Table::default();
    for f in program.functions.iter().filter(|f| !f.symbolic) {
        let symbol = if f.exported { f.name.clone() } else { format!("burdin.{}", f.name) };
        let generic = is_instance(&f.name).then(|| generic_name(&f.name));
        table
            .known
            .insert(symbol, Known { name: f.name.clone(), owner: owner_of(f.span.file), generic, closure: false });
    }
    for closure in &program.closures {
        let owner = owner_of(closure.span.file);
        table.known.insert(
            format!("burdin.{}", closure.name),
            Known { name: closure.name.clone(), owner, generic: None, closure: true },
        );
    }

    let executable = std::fs::metadata(&output).ok().map(|m| m.len());
    let report = burdin_inspect::bloat::analyze(&symbols, &table, executable, args.top);
    let title = format!("{} ({})", loaded.stem, if args.debug_profile { "debug" } else { "release" });
    let text = if args.json {
        burdin_inspect::bloat::render_json(&report, &title)
    } else {
        burdin_inspect::bloat::render_human(&report, &title)
    };
    print!("{text}");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn the_command_line_has_a_target_a_limit_and_a_format() {
        let args = parse(&argv(&["app", "--top", "5", "--format=json", "--debug"])).unwrap();
        assert_eq!((args.top, args.json, args.debug_profile), (5, true, true));
        assert_eq!(parse(&argv(&["app"])).unwrap().top, 20);
        assert!(parse(&argv(&["app", "--top", "0"])).unwrap_err().contains("positive"));
        assert!(parse(&argv(&["app", "--format", "xml"])).unwrap_err().contains("human"));
        assert!(parse(&argv(&[])).unwrap_err().contains("missing target"));
        assert!(parse(&argv(&["a", "b"])).unwrap_err().contains("unexpected"));
    }
}
