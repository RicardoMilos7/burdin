//! `burdin doc`: documentation generated from `///` comments (`spec/doc.md`). The command compiles the package the way
//! `burdin lint` does (so a package that does not compile has no documentation), builds the documented API from the
//! syntax trees with `burdin_doc`, and writes a static HTML site, a JSON document, or a coverage report.

use crate::{
    CompileOptions, DiagnosticOptions, EntryMode, FrontEnd, SourceOverrides, flag_without_value, front_end,
    jobs_or_default, option_value, parse_color, parse_format, parse_jobs, parse_number, render_options,
};
use burdin_diag::{DiagnosticFormat, Severity};
use std::collections::BTreeSet;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The marker file that says a directory holds a site `burdin doc` wrote, and which files it wrote.
const MARKER: &str = ".burdin-doc";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Html,
    Json,
}

struct DocArgs {
    target: PathBuf,
    out: Option<PathBuf>,
    format: Format,
    private: bool,
    coverage: bool,
    deny_warnings: bool,
    diagnostics: DiagnosticOptions,
    comptime_steps: Option<u64>,
    /// `-j`: the most parser threads.
    jobs: Option<usize>,
}

fn parse(argv: &[String]) -> Result<DocArgs, String> {
    let mut args = DocArgs {
        target: PathBuf::new(),
        out: None,
        format: Format::Html,
        private: false,
        coverage: false,
        deny_warnings: false,
        diagnostics: DiagnosticOptions::default(),
        comptime_steps: None,
        jobs: None,
    };
    let mut target = None;
    let mut it = argv.iter();
    while let Some(raw) = it.next() {
        let (a, inline) = match raw.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            // `-j4` is `-j 4`, as in cargo and make.
            _ if raw.len() > 2 && raw.starts_with("-j") => ("-j", Some(&raw[2..])),
            _ => (raw.as_str(), None),
        };
        match a {
            "-o" | "--out" => args.out = Some(PathBuf::from(option_value(&mut it, inline, a, "a path")?)),
            "--output-format" => {
                args.format = match option_value(&mut it, inline, a, "html or json")?.as_str() {
                    "html" => Format::Html,
                    "json" => Format::Json,
                    other => return Err(format!("unknown output format `{other}` (expected html or json)")),
                };
            }
            "-j" | "--jobs" => args.jobs = Some(parse_jobs(&mut it, inline, a)?),
            "--private" => args.private = flag_without_value(a, inline)?,
            "--coverage" => args.coverage = flag_without_value(a, inline)?,
            "--deny-warnings" => args.deny_warnings = flag_without_value(a, inline)?,
            "--diagnostic-format" => {
                args.diagnostics.format = parse_format(&option_value(&mut it, inline, a, "a value")?)?;
            }
            "--color" => args.diagnostics.color = parse_color(&option_value(&mut it, inline, a, "a value")?)?,
            "--no-color" => {
                flag_without_value(a, inline)?;
                args.diagnostics.color = crate::ColorChoice::Never;
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
    if args.coverage && args.out.is_some() {
        return Err("`--coverage` prints a report and does not take `-o`".to_string());
    }
    Ok(args)
}

pub(crate) fn run(argv: &[String]) -> ExitCode {
    let args = match parse(argv) {
        Ok(a) => a,
        Err(message) => return crate::cli::usage_error("doc", &message),
    };
    let options = CompileOptions {
        release: false,
        debug: false,
        strip: false,
        lto: false,
        pgo_generate: false,
        pgo_use: None,
        emit_ir: None,
        emit: None,
        link_libs: &[],
        // A library package has no `main`, and the `test` blocks are not part of the documented interface.
        mode: EntryMode::Test,
        diagnostics: &args.diagnostics,
        comptime_steps: args.comptime_steps,
        emit_deps: None,
        report_clean: false,
        run_build_scripts: true,
        locked: false,
        features: Default::default(),
        jobs: jobs_or_default(args.jobs),
        sanitize: Default::default(),
        fuzz_target: None,
    };
    let front = match front_end(&args.target, &options, &SourceOverrides::new()) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let render = render_options(&args.diagnostics, std::io::stderr().is_terminal());
    if front.diags.has_errors() {
        let rendered = front.diags.render_with(&front.sources, &render);
        eprint!("{}", rendered.text);
        let errors = front.diags.iter().filter(|d| d.severity == Severity::Error).count();
        let s = if errors == 1 { "" } else { "s" };
        eprintln!("error: could not check {} ({errors} error{s}); fix them before documenting", args.target.display());
        return ExitCode::from(1);
    }
    let packages = packages_of(&front);
    let input = burdin_doc::Input {
        sources: &front.sources,
        packages,
        entry: front.loaded.entry_package,
        private: args.private,
    };
    if args.coverage {
        let (rows, missing) = burdin_doc::coverage(&input);
        print!("{}", burdin_doc::render_coverage(input.packages[input.entry].name, &rows, &missing));
        return ExitCode::SUCCESS;
    }
    let (api, diags) = burdin_doc::build(&input);
    // The compiler's warnings are not repeated here: they are `burdin build`'s to report.
    let rendered = diags.render_with(&front.sources, &render);
    eprint!("{}", rendered.text);
    let warnings = diags.iter().count();
    if render.format == DiagnosticFormat::Json {
        eprintln!("{{\"type\":\"doc-summary\",\"warnings\":{warnings}}}");
    }
    match args.format {
        Format::Json => {
            let text = burdin_doc::json::render(&api, &front.sources);
            match &args.out {
                Some(path) => {
                    if let Err(e) = std::fs::write(path, text) {
                        eprintln!("error: cannot write {}: {e}", path.display());
                        return ExitCode::from(2);
                    }
                    eprintln!("doc: wrote {}", path.display());
                }
                None => print!("{text}"),
            }
        }
        Format::Html => {
            let out = args.out.clone().unwrap_or_else(|| default_out(&args.target));
            let pages = burdin_doc::html::render(&api);
            match write_site(&out, &pages) {
                Ok(()) => eprintln!("doc: wrote {} files to {}", pages.len(), out.display()),
                Err(message) => {
                    eprintln!("error: {message}");
                    return ExitCode::from(2);
                }
            }
        }
    }
    if warnings > 0 && args.deny_warnings {
        eprintln!(
            "error: {warnings} warning{} in doc comments (--deny-warnings)",
            if warnings == 1 { "" } else { "s" }
        );
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

/// `<package>/target/doc` for a package directory, `target/doc` beside the file for a single file.
fn default_out(target: &Path) -> PathBuf {
    let base =
        if target.is_dir() { target.to_path_buf() } else { target.parent().map(Path::to_path_buf).unwrap_or_default() };
    base.join("target").join("doc")
}

/// The loaded packages in the form `burdin_doc` reads. The entry package's modules leave out what a build script
/// generated under its `target` directory; the file paths are relative to the package directory.
fn packages_of(front: &FrontEnd) -> Vec<burdin_doc::PackageInput<'_>> {
    let mut out = Vec::new();
    let mut first = 0usize;
    for (index, package) in front.loaded.packages.iter().enumerate() {
        let is_entry = index == front.loaded.entry_package;
        let generated = package.dir.as_ref().map(|d| d.join("target"));
        let mut modules = Vec::new();
        for (m, module) in package.modules.iter().enumerate() {
            let skip =
                module.text.is_some() && is_entry || generated.as_ref().is_some_and(|g| module.file.starts_with(g));
            if skip {
                continue;
            }
            let (file, ast) = &front.parsed[first + m];
            let relative = match &package.dir {
                Some(dir) => module.file.strip_prefix(dir).unwrap_or(&module.file).to_path_buf(),
                None => module.file.file_name().map(PathBuf::from).unwrap_or_else(|| module.file.clone()),
            };
            let is_root = match package.root_module {
                Some(root) => root == m,
                None => is_entry && m == front.loaded.entry_module,
            };
            modules.push(burdin_doc::ModuleInput {
                file: *file,
                path: &module.path,
                ast,
                file_path: crate::source_path_text(&relative).replace('\\', "/"),
                is_root,
            });
        }
        out.push(burdin_doc::PackageInput {
            name: &package.name,
            deps: &package.deps,
            modules,
            is_std: !package.modules.is_empty() && package.modules.iter().all(|m| m.text.is_some()),
        });
        first += package.modules.len();
    }
    out
}

/// Writes the site into `out`: every page, then removes the files an earlier run wrote that this one did not. A
/// directory that already holds other files is refused, so `-o .` cannot overwrite a project.
fn write_site(out: &Path, pages: &[burdin_doc::html::Page]) -> Result<(), String> {
    let marker = out.join(MARKER);
    let previous: BTreeSet<String> = match std::fs::read_to_string(&marker) {
        Ok(text) => text.lines().map(str::to_string).collect(),
        Err(_) => {
            let occupied = std::fs::read_dir(out).map(|mut d| d.next().is_some()).unwrap_or(false);
            if occupied {
                return Err(format!(
                    "{} is not empty and was not written by `burdin doc`; choose another directory with -o",
                    out.display()
                ));
            }
            BTreeSet::new()
        }
    };
    let mut written = BTreeSet::new();
    for page in pages {
        let path = out.join(&page.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, &page.text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        written.insert(page.path.clone());
    }
    for stale in previous.difference(&written) {
        let path = out.join(stale);
        let _ = std::fs::remove_file(&path);
        // Remove the directories that are now empty, up to the output directory.
        let mut dir = path.parent();
        while let Some(d) = dir {
            if d == out || std::fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }
    }
    let listing: Vec<&str> = written.iter().map(String::as_str).collect();
    std::fs::write(&marker, listing.join("\n") + "\n")
        .map_err(|e| format!("cannot write {}: {e}", marker.display()))?;
    Ok(())
}
