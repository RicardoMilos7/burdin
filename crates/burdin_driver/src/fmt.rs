//! `burdin fmt`: the formatter's command line (`spec/fmt.md`, FM12). It formats files in place, checks them for CI, or
//! filters standard input to standard output for editors. The formatting itself is `burdin_fmt`; this module finds
//! the files, tells the formatter how to group `use` declarations, and reports.

use crate::package;
use crate::{
    ColorChoice, DiagnosticOptions, flag_without_value, option_value, parse_color, parse_format, parse_number,
};
use crate::{render_options, source_path_text};
use burdin_diag::SourceMap;
use burdin_fmt::{FormatError, Options, UseGroups};
use std::collections::HashMap;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub(crate) const FMT_USAGE: &str = "\
usage:
  burdin fmt [<file.bd | directory>]... [--check] [diagnostics]
  burdin fmt --stdin [--stdin-filepath <path>] [--range <lo>:<hi>] [--check] [diagnostics]

fmt options:
  --check                    change nothing; list the files that are not formatted and exit with status 1
  --stdin                    read one source from standard input and write the formatted source to standard output
  --stdin-filepath <path>    the path the standard input stands for: names it in diagnostics and finds the package
                             whose dependencies group the `use` declarations
  --range <lo>:<hi>          with --stdin, format only the items that overlap these byte offsets";

/// What the command line asked for.
#[derive(Debug, Default)]
struct FmtArgs {
    paths: Vec<PathBuf>,
    check: bool,
    stdin: bool,
    stdin_filepath: Option<PathBuf>,
    range: Option<(usize, usize)>,
    diagnostics: DiagnosticOptions,
}

fn parse_fmt_args(argv: &[String]) -> Result<FmtArgs, String> {
    let mut args = FmtArgs::default();
    let mut it = argv.iter();
    while let Some(raw) = it.next() {
        let (a, inline) = match raw.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            _ => (raw.as_str(), None),
        };
        match a {
            "--check" => args.check = flag_without_value(a, inline)?,
            "--stdin" => args.stdin = flag_without_value(a, inline)?,
            "--stdin-filepath" => args.stdin_filepath = Some(option_value(&mut it, inline, a, "a path")?.into()),
            "--range" => {
                let value = option_value(&mut it, inline, a, "a byte range such as 10:200")?;
                args.range = Some(parse_range(&value)?);
            }
            "--diagnostic-format" => {
                args.diagnostics.format = parse_format(&option_value(&mut it, inline, a, "a value")?)?
            }
            "--color" => args.diagnostics.color = parse_color(&option_value(&mut it, inline, a, "a value")?)?,
            "--no-color" => {
                flag_without_value(a, inline)?;
                args.diagnostics.color = ColorChoice::Never;
            }
            "--width" => {
                let value = option_value(&mut it, inline, a, "a number")?;
                args.diagnostics.width = Some(parse_number(a, &value)?);
            }
            "--error-limit" => {
                let value = option_value(&mut it, inline, a, "a number")?;
                let limit = parse_number(a, &value)?;
                args.diagnostics.error_limit = (limit > 0).then_some(limit);
            }
            _ if a.starts_with('-') && a != "-" => return Err(format!("unknown option `{a}`")),
            _ => args.paths.push(PathBuf::from(a)),
        }
    }
    if args.stdin {
        if !args.paths.is_empty() {
            return Err("`--stdin` formats standard input; it does not take files".to_string());
        }
    } else {
        if args.stdin_filepath.is_some() {
            return Err("`--stdin-filepath` is only valid with `--stdin`".to_string());
        }
        if args.range.is_some() {
            return Err("`--range` is only valid with `--stdin`".to_string());
        }
        if args.paths.is_empty() {
            args.paths.push(PathBuf::from("."));
        }
    }
    Ok(args)
}

/// `lo:hi`, two byte offsets with `lo <= hi`.
fn parse_range(value: &str) -> Result<(usize, usize), String> {
    let bad = || format!("`--range` needs two byte offsets such as `10:200`, got `{value}`");
    let (lo, hi) = value.split_once(':').ok_or_else(bad)?;
    let (lo, hi): (usize, usize) = (lo.trim().parse().map_err(|_| bad())?, hi.trim().parse().map_err(|_| bad())?);
    if lo > hi {
        return Err(format!("`--range` must not end before it starts, got `{value}`"));
    }
    Ok((lo, hi))
}

pub(crate) fn run(argv: &[String]) -> ExitCode {
    let args = match parse_fmt_args(argv) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("error: {message}\n{FMT_USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.stdin { run_stdin(&args) } else { run_files(&args) }
}

/// The `use` grouping for a file: the standard library's module names, and the dependencies of the package the file is
/// in (the nearest `burdin.toml` above it).
struct Grouping {
    std_modules: Vec<String>,
    manifests: HashMap<PathBuf, Vec<String>>,
}

impl Grouping {
    fn new() -> Grouping {
        Grouping { std_modules: package::std_module_names(), manifests: HashMap::new() }
    }

    fn options_for(&mut self, file: &Path) -> Options {
        let dependencies = match package::manifest_dir(file) {
            Some(dir) => self.manifests.entry(dir.clone()).or_insert_with(|| package::dependency_names(&dir)).clone(),
            None => Vec::new(),
        };
        Options { groups: UseGroups { std_modules: self.std_modules.clone(), dependencies } }
    }
}

/// What formatting one source came to.
enum Outcome {
    Formatted(String),
    /// The source has syntax errors; the diagnostics are already printed.
    Rejected,
    /// The formatter failed on a valid source; the message is already printed.
    Failed,
}

fn format_one(name: &str, text: &str, options: &Options, range: Option<(usize, usize)>, args: &FmtArgs) -> Outcome {
    let result = match range {
        Some((lo, hi)) => burdin_fmt::format_range(text, lo, hi, options),
        None => burdin_fmt::format_source(text, options),
    };
    match result {
        Ok(out) => Outcome::Formatted(out),
        Err(FormatError::Syntax(diags)) => {
            let mut sources = SourceMap::default();
            sources.add(name.to_string(), text.to_string());
            let render = render_options(&args.diagnostics, std::io::stderr().is_terminal());
            eprint!("{}", diags.render_with(&sources, &render).text);
            eprintln!("error: {name} was not formatted because it has syntax errors");
            Outcome::Rejected
        }
        Err(FormatError::Internal(message)) => {
            eprintln!("error: the formatter could not format {name}, which is left as it is: {message}");
            eprintln!("this is a bug in `burdin fmt`; please report it with the file");
            Outcome::Failed
        }
    }
}

fn run_stdin(args: &FmtArgs) -> ExitCode {
    let mut bytes = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut bytes) {
        eprintln!("error: cannot read standard input: {e}");
        return ExitCode::from(2);
    }
    let Ok(text) = String::from_utf8(bytes) else {
        eprintln!("error: standard input is not valid UTF-8");
        return ExitCode::from(2);
    };
    let path = args.stdin_filepath.clone().unwrap_or_else(|| PathBuf::from("<stdin>"));
    let options = Grouping::new().options_for(&path);
    let name = source_path_text(&path);
    match format_one(&name, &text, &options, args.range, args) {
        Outcome::Formatted(out) if args.check => {
            if out == text {
                ExitCode::SUCCESS
            } else {
                println!("{name}");
                ExitCode::from(1)
            }
        }
        Outcome::Formatted(out) => {
            let mut stdout = std::io::stdout().lock();
            if stdout.write_all(out.as_bytes()).and_then(|_| stdout.flush()).is_err() {
                eprintln!("error: cannot write to standard output");
                return ExitCode::from(2);
            }
            ExitCode::SUCCESS
        }
        Outcome::Rejected | Outcome::Failed => ExitCode::from(2),
    }
}

fn run_files(args: &FmtArgs) -> ExitCode {
    let mut files: Vec<PathBuf> = Vec::new();
    for path in &args.paths {
        if let Err(message) = collect(path, &mut files) {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    }
    let mut seen = std::collections::HashSet::new();
    files.retain(|f| seen.insert(f.canonicalize().unwrap_or_else(|_| f.clone())));
    let mut grouping = Grouping::new();
    let (mut differing, mut failed) = (0usize, 0usize);
    for file in &files {
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("error: cannot read {}: {e}", file.display());
                failed += 1;
                continue;
            }
        };
        let options = grouping.options_for(file);
        let name = file.display().to_string();
        match format_one(&name, &text, &options, None, args) {
            Outcome::Formatted(out) if out == text => {}
            Outcome::Formatted(out) => {
                differing += 1;
                if !args.check
                    && let Err(e) = std::fs::write(file, out)
                {
                    eprintln!("error: cannot write {}: {e}", file.display());
                    failed += 1;
                    continue;
                }
                println!("{name}");
            }
            Outcome::Rejected | Outcome::Failed => failed += 1,
        }
    }
    if failed > 0 {
        return ExitCode::from(2);
    }
    if args.check && differing > 0 {
        eprintln!(
            "{differing} file{} would be reformatted; run `burdin fmt` to fix",
            if differing == 1 { "" } else { "s" }
        );
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

/// The `.bd` files a command-line path stands for: the file itself, or every `.bd` file under a directory, skipping
/// hidden directories and `target` (build output).
fn collect(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_file() {
        out.push(path.to_path_buf());
        return Ok(());
    }
    if !path.is_dir() {
        return Err(format!("cannot open {}: no such file or directory", path.display()));
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for entry in entries {
        let name = entry.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if entry.is_dir() {
            if !name.starts_with('.') && name != "target" {
                collect(&entry, out)?;
            }
        } else if entry.extension().is_some_and(|e| e == "bd") {
            out.push(entry);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<FmtArgs, String> {
        parse_fmt_args(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn no_path_means_the_current_directory() {
        assert_eq!(parse(&[]).unwrap().paths, vec![PathBuf::from(".")]);
    }

    #[test]
    fn stdin_options_need_stdin() {
        assert!(parse(&["a.bd", "--range", "1:2"]).unwrap_err().contains("only valid with `--stdin`"));
        assert!(parse(&["--stdin", "a.bd"]).unwrap_err().contains("does not take files"));
        let args = parse(&["--stdin", "--range=3:9", "--stdin-filepath", "x.bd"]).unwrap();
        assert_eq!(args.range, Some((3, 9)));
    }

    #[test]
    fn a_range_needs_two_offsets_in_order() {
        assert!(parse_range("5").is_err());
        assert!(parse_range("9:3").is_err());
        assert_eq!(parse_range("0:10").unwrap(), (0, 10));
    }
}
