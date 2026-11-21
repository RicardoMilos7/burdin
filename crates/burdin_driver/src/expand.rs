//! `burdin expand`: the program's source after comptime evaluation, derives, default and named arguments, and
//! operators have been expanded (`spec/inspect.md`, INS1-INS6). The expansion itself is `burdin_inspect::expand`; this
//! file is the command line and the formatting of its result.

use crate::{
    CompileOptions, DiagnosticOptions, EntryMode, FrontEnd, SourceOverrides, flag_without_value, front_end,
    jobs_or_default, option_value, parse_diagnostic_option, parse_jobs, print_warnings, report_errors,
};
use burdin_inspect::expand::{Options, SourceFile};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, Default)]
struct ExpandArgs {
    target: PathBuf,
    item: Option<String>,
    instances: bool,
    no_format: bool,
    comptime_steps: Option<u64>,
    jobs: Option<usize>,
    diagnostics: DiagnosticOptions,
}

fn parse(argv: &[String]) -> Result<ExpandArgs, String> {
    let mut args = ExpandArgs::default();
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
            "--item" => args.item = Some(option_value(&mut it, inline, a, "an item path")?),
            "--instances" => args.instances = flag_without_value(a, inline)?,
            "--no-format" => args.no_format = flag_without_value(a, inline)?,
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
    if args.item.is_some() && args.instances {
        return Err("`--item` and `--instances` cannot be combined".to_string());
    }
    Ok(args)
}

pub(crate) fn run(argv: &[String]) -> ExitCode {
    let args = match parse(argv) {
        Ok(a) => a,
        Err(message) => return crate::cli::usage_error("expand", &message),
    };
    let options = CompileOptions {
        profile: crate::profile::Profile::dev(),
        debug: false,
        strip: false,
        lto: false,
        pgo_generate: false,
        pgo_use: None,
        emit_ir: None,
        emit: None,
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
    let FrontEnd { loaded, files, parsed, sources, program, diags, .. } =
        match front_end(&args.target, &options, &SourceOverrides::new()) {
            Ok(front) => front,
            Err(code) => return code,
        };
    if diags.has_errors() {
        report_errors("expand", &args.target, &diags, &sources, &args.diagnostics, None);
        return ExitCode::from(1);
    }
    print_warnings(&diags, &sources, &args.diagnostics, false);
    let program = program.expect("no errors means checking produced a program");

    // The package and module of each file, numbered the way the front end numbers its files.
    let mut owners: Vec<(String, String)> = Vec::new();
    for package in &loaded.packages {
        for module in &package.modules {
            owners.push((package.name.clone(), module.path.join(".")));
        }
    }
    let sources_files: Vec<SourceFile> = parsed
        .iter()
        .filter(|(id, _)| !files.get(id.0 as usize).is_some_and(|(_, embedded)| *embedded))
        .map(|(id, ast)| {
            let file = sources.get(*id);
            let (package, module) = owners.get(id.0 as usize).map_or(("", ""), |(p, m)| (p.as_str(), m.as_str()));
            SourceFile { id: *id, path: &file.path, text: &file.text, ast, package, module }
        })
        .collect();

    let expand_options = Options { item: args.item.clone(), instances: args.instances };
    let expanded = match burdin_inspect::expand::expand(&program, &sources_files, &expand_options) {
        Ok(expanded) => expanded,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    };
    let several = expanded.len() > 1;
    for file in &expanded {
        let text = if args.no_format { file.text.clone() } else { format_text(&file.text) };
        if several {
            println!("// ==== {} ====", display_path(&args.target, &file.path));
        }
        print!("{text}");
        if !text.ends_with('\n') {
            println!();
        }
    }
    ExitCode::SUCCESS
}

/// The path of a source file as the header names it: relative to the package directory (or the directory of the
/// target file), with `/` separators, so the output does not depend on where the package is.
fn display_path(target: &std::path::Path, path: &str) -> String {
    let root = if target.is_dir() { Some(target.to_path_buf()) } else { target.parent().map(|p| p.to_path_buf()) };
    let relative = root
        .and_then(|root| std::fs::canonicalize(root).ok())
        .zip(std::fs::canonicalize(path).ok())
        .and_then(|(root, file)| file.strip_prefix(&root).ok().map(|p| p.to_path_buf()));
    relative.as_deref().unwrap_or(std::path::Path::new(path)).to_string_lossy().replace('\\', "/")
}

/// The text through the formatter. When the formatter refuses (the text does not parse as one file, as with an
/// item on its own) the text is returned as it is.
fn format_text(text: &str) -> String {
    match burdin_fmt::format_source(text, &burdin_fmt::Options::default()) {
        Ok(formatted) => formatted,
        Err(_) => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn the_command_line_has_a_target_an_item_and_instances() {
        let args = parse(&argv(&["app", "--item", "shapes.area", "--no-format"])).unwrap();
        assert_eq!(args.item.as_deref(), Some("shapes.area"));
        assert!(args.no_format && !args.instances);
        assert!(parse(&argv(&["app", "--instances"])).unwrap().instances);
        assert!(parse(&argv(&["app", "--item=a", "--instances"])).unwrap_err().contains("cannot be combined"));
        assert!(parse(&argv(&[])).unwrap_err().contains("missing target"));
        assert!(parse(&argv(&["a", "b"])).unwrap_err().contains("unexpected"));
        assert!(parse(&argv(&["a", "--item"])).is_err());
    }
}
