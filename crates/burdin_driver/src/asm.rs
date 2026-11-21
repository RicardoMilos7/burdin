//! `burdin asm`: one function's assembly or LLVM IR, with the Burdin source interleaved (`spec/inspect.md`,
//! INS9-INS11). The program is built the way `burdin build --release` builds it (with debug information so the line
//! table is there), the whole program's assembly is written to a temporary directory, and the function is cut out.

use crate::emit::{Emit, EmitKind};
use crate::profile::Profile;
use crate::{
    CompileOptions, DiagnosticOptions, EntryMode, FrontEnd, SourceOverrides, TempDir, emit_program, flag_without_value,
    front_end, jobs_or_default, option_value, parse_diagnostic_option, parse_jobs, print_warnings, report_errors,
};
use burdin_codegen::AsmSyntax;
use burdin_inspect::instances::{generic_name, is_instance};
use burdin_tir::Program;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, Default)]
struct AsmArgs {
    target: PathBuf,
    function: Option<String>,
    debug_profile: bool,
    syntax: AsmSyntax,
    llvm: bool,
    no_source: bool,
    comptime_steps: Option<u64>,
    jobs: Option<usize>,
    diagnostics: DiagnosticOptions,
}

fn parse(argv: &[String]) -> Result<AsmArgs, String> {
    let mut args = AsmArgs::default();
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
            "--llvm" => args.llvm = flag_without_value(a, inline)?,
            "--no-source" => args.no_source = flag_without_value(a, inline)?,
            "--intel" => {
                flag_without_value(a, inline)?;
                args.syntax = AsmSyntax::Intel;
            }
            "--att" => {
                flag_without_value(a, inline)?;
                args.syntax = AsmSyntax::Att;
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
            _ if args.function.is_none() => args.function = Some(a.to_string()),
            _ => return Err(format!("unexpected argument `{a}`")),
        }
    }
    if args.llvm && args.syntax != AsmSyntax::Default {
        return Err("`--intel` and `--att` choose an assembly dialect, which `--llvm` does not print".to_string());
    }
    args.target = target.ok_or("missing target file")?;
    Ok(args)
}

/// Why a function name did not select one function.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Selection {
    /// The index into `program.functions`.
    One(usize),
    /// A generic function named without its type arguments: the instances to choose from.
    Instances(Vec<String>),
    /// The name matches several functions.
    Ambiguous(Vec<String>),
    /// The function exists, but nothing calls it, so no code was generated.
    NoCode(String),
    Unknown,
}

/// Finds the function `query` names among the functions of `program`. `reached` says which functions have code.
/// The match is on the whole name or on a trailing `.`-separated part of it (`util.helper` for `pkg.util.helper`),
/// and a name without type arguments that is the generic part of several instances selects none of them.
pub(crate) fn select(program: &Program, reached: &[bool], query: &str) -> Selection {
    let query = query.trim().replace("::", ".");
    let matches = |name: &str, wanted: &str| name == wanted || name.ends_with(&format!(".{wanted}"));
    let named = |f: &burdin_tir::Function| !f.symbolic;
    let exact: Vec<usize> = (0..program.functions.len())
        .filter(|&i| named(&program.functions[i]) && matches(&program.functions[i].name, &query))
        .collect();
    // An exact name beats a suffix match: `main` is the program's `main`, not `util.main`.
    let exact_whole: Vec<usize> = exact.iter().copied().filter(|&i| program.functions[i].name == query).collect();
    let chosen = if exact_whole.is_empty() { exact } else { exact_whole };
    match chosen.as_slice() {
        [one] if reached[*one] => return Selection::One(*one),
        [one] => return Selection::NoCode(program.functions[*one].name.clone()),
        [] => {}
        many => {
            let mut names: Vec<String> = many.iter().map(|&i| program.functions[i].name.clone()).collect();
            names.sort();
            return Selection::Ambiguous(names);
        }
    }
    if !query.contains('<') {
        let mut instances: Vec<String> = program
            .functions
            .iter()
            .enumerate()
            .filter(|(i, f)| reached[*i] && named(f) && is_instance(&f.name) && matches(&generic_name(&f.name), &query))
            .map(|(_, f)| f.name.clone())
            .collect();
        if !instances.is_empty() {
            instances.sort();
            return Selection::Instances(instances);
        }
    }
    Selection::Unknown
}

pub(crate) fn run(argv: &[String]) -> ExitCode {
    let args = match parse(argv) {
        Ok(a) => a,
        Err(message) => return crate::cli::usage_error("asm", &message),
    };
    let kind = if args.llvm { EmitKind::LlvmIrOpt } else { EmitKind::Asm };
    let emit = Emit { kinds: vec![kind], asm_syntax: args.syntax, skip_link: true };
    let options = CompileOptions {
        profile: if args.debug_profile { Profile::dev() } else { Profile::release() },
        // The line table is what the source lines come from; the IR is clearer without the metadata.
        debug: !args.llvm,
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
        features: Default::default(),
        offline: false,
        jobs: jobs_or_default(args.jobs),
        sanitize: Default::default(),
        fuzz_target: None,
    };
    let FrontEnd { loaded, files, sources, program, diags, rust_libs, .. } =
        match front_end(&args.target, &options, &SourceOverrides::new()) {
            Ok(front) => front,
            Err(code) => return code,
        };
    if diags.has_errors() {
        report_errors("compile", &args.target, &diags, &sources, &args.diagnostics, None);
        return ExitCode::from(1);
    }
    print_warnings(&diags, &sources, &args.diagnostics, false);
    let program = program.expect("no errors means checking produced a program");
    let reached = burdin_inspect::reach::functions(&program);
    let is_std = |file: burdin_diag::FileId| files.get(file.0 as usize).is_some_and(|(_, embedded)| *embedded);

    let Some(query) = args.function.as_deref() else {
        // No function: the functions that have code, the program's own first.
        let mut names: Vec<(bool, &str)> = (0..program.functions.len())
            .filter(|&i| reached[i] && !program.functions[i].symbolic)
            .map(|i| (is_std(program.functions[i].span.file), program.functions[i].name.as_str()))
            .collect();
        names.sort();
        for (std, name) in names {
            println!("{name}{}", if std { "  [std]" } else { "" });
        }
        return ExitCode::SUCCESS;
    };
    let index = match select(&program, &reached, query) {
        Selection::One(index) => index,
        Selection::Instances(names) => {
            eprintln!("error: `{query}` is generic; name one of its instances:");
            for name in names {
                eprintln!("  {name}");
            }
            return ExitCode::from(2);
        }
        Selection::Ambiguous(names) => {
            eprintln!("error: `{query}` names more than one function; use the full name:");
            for name in names {
                eprintln!("  {name}");
            }
            return ExitCode::from(2);
        }
        Selection::NoCode(name) => {
            eprintln!("error: `{name}` has no code: nothing the program runs calls it");
            return ExitCode::from(2);
        }
        Selection::Unknown => {
            eprintln!("error: no function named `{query}`; run `burdin asm {}` to list them", args.target.display());
            return ExitCode::from(2);
        }
    };
    let function = &program.functions[index];
    let symbol = if function.exported { function.name.clone() } else { format!("burdin.{}", function.name) };

    let dir = match TempDir::new() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("error: cannot create a temporary directory: {e}");
            return ExitCode::from(2);
        }
    };
    let output = dir.path.join("program");
    if let Err(code) = emit_program(&program, &sources, &loaded.stem, &output, &options, &rust_libs, &[]) {
        return code;
    }
    let artifact = emit.path(&output, kind);
    let text = match std::fs::read_to_string(&artifact) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", artifact.display());
            return ExitCode::from(2);
        }
    };
    let cut = if args.llvm {
        burdin_inspect::asm::function_ir(&text, &symbol)
    } else {
        let lookup = |path: &str| source_text(&sources, path);
        let comment = if cfg!(target_arch = "aarch64") { "//" } else { "#" };
        if args.no_source {
            burdin_inspect::asm::render_asm(&text, &symbol, &|_| None, comment)
                .map(|text| strip_source_comments(&text, comment))
        } else {
            burdin_inspect::asm::render_asm(&text, &symbol, &lookup, comment)
        }
    };
    match cut {
        Some(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("error: the backend printed no definition of `{symbol}` (it may have been removed as unused)");
            ExitCode::from(2)
        }
    }
}

/// The text of the source file the debug information names by `path`: the same path the compiler loaded it under,
/// with `./` and the platform's separator normalized.
fn source_text(sources: &burdin_diag::SourceMap, path: &str) -> Option<String> {
    let normalize = |p: &str| p.replace('\\', "/").trim_start_matches("./").to_string();
    let wanted = normalize(path);
    sources.iter().find(|(_, file)| normalize(&file.path) == wanted).map(|(_, file)| file.text.clone())
}

/// Removes the comment lines `render_asm` adds for source locations (`--no-source`).
fn strip_source_comments(text: &str, comment: &str) -> String {
    let marker = format!("    {comment} ");
    let mut out = String::new();
    for line in text.lines() {
        if line.starts_with(&marker) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn the_command_line_takes_a_target_then_a_function() {
        let args = parse(&argv(&["app", "largest<i32>", "--intel", "--debug"])).unwrap();
        assert_eq!(args.target, PathBuf::from("app"));
        assert_eq!(args.function.as_deref(), Some("largest<i32>"));
        assert_eq!(args.syntax, AsmSyntax::Intel);
        assert!(args.debug_profile && !args.llvm);
        assert!(parse(&argv(&[])).unwrap_err().contains("missing target"));
        assert!(parse(&argv(&["a", "f", "g"])).unwrap_err().contains("unexpected argument `g`"));
        assert!(parse(&argv(&["a", "--llvm", "--att"])).unwrap_err().contains("dialect"));
        assert!(parse(&argv(&["a", "--emit-ir", "x"])).unwrap_err().contains("unknown option"));
    }

    #[test]
    fn source_comments_can_be_stripped() {
        let text = "f:\n    # a.bd:1: let x = 1\n\tmov eax, 1\n";
        assert_eq!(strip_source_comments(text, "#"), "f:\n\tmov eax, 1\n");
    }
}
