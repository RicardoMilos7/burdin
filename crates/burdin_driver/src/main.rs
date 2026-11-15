//! The `burdin` command: `build`, `run`, and `test`. The contract is documented in `README.md` at the repository
//! root (`The driver contract`) and summarized at the top of tests/run-tests.sh.

mod package;
mod test_runner;

use burdin_diag::{DiagnosticFormat, Diagnostics, RenderOptions, SourceMap};
use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub(crate) const USAGE: &str = "\
usage:
  burdin build <file.bd | package directory> [--release] [-g] [--strip] [--lto]
               [--pgo-generate | --pgo-use <profile>] [-o <output>] [--emit-ir <file.ll>]
               [--link-lib <name>]... [diagnostics]
  burdin run <file.bd | package directory> [--release] [-g] [--link-lib <name>]... [diagnostics]
             [-- <program arguments>...]
  burdin test <file.bd | package directory> [--release] [-g] [-j <n>] [--verbose] [-p <package>]...
              [--workspace] [--filter <text>] [--ignored | --include-ignored] [--timeout <ms>]
              [diagnostics]
  burdin explain [<error code>]
  burdin --version

test options:
  -j, --jobs <n>       tests to run at once (default: the number of CPUs)
  -p <package>         run the tests of one package of the build; may repeat
  --workspace          run the tests of the entry package and every path dependency
  --filter <text>      run only tests whose name contains text
  --ignored            run only tests marked [ignore]
  --include-ignored    run ignored tests too
  --timeout <ms>       per-test timeout (default: 60000; 0 disables it)
  -v, --verbose        print the output of passing tests too

diagnostics:
  --diagnostic-format <human|short|json>  human (default) colors and wraps; short prints one plain line per
                                          diagnostic; json prints one JSON object per line
  --color <auto|always|never>             color (default auto: only when stderr is a terminal)
  --no-color                              same as --color never
  --width <columns>                       wrap human diagnostics at this width (0 disables wrapping)
  --error-limit <n>                       print at most n errors after sorting (0 prints all)

environment:
  BURDIN_CC            C compiler driver used to link against libc (default: cc; cl on Windows)
  BURDIN_COMMIT        overrides the git commit printed by `--version`
  BURDIN_JOBS          parser threads (default: the machine's parallelism; 1 is sequential)
  LLVM_SYS_211_PREFIX  LLVM 21 installation; its bin/llvm-config resolves `--link-lib LLVM-C`
  COLUMNS              terminal width used when --width is not given
  NO_COLOR             disables color unless --color=always or --color=never says otherwise";

const EXPLAIN_USAGE: &str = "\
usage:
  burdin explain           list the error codes
  burdin explain <code>    print the reference entry for an error code (for example E0003)";

/// The user-facing error-code reference, embedded so `burdin explain` cannot disagree with the page that
/// `tests/check-error-docs.sh` checks (issue #437).
const ERROR_DOCS: &str = include_str!("../../../docs/error-codes.md");

#[derive(Debug)]
pub(crate) struct Args {
    pub(crate) command: String,
    pub(crate) target: PathBuf,
    pub(crate) release: bool,
    /// Emit source-level debug information (DWARF in the object and the executable).
    pub(crate) debug: bool,
    /// Pass the linker's strip option (Unix `-s`; with MSVC, optimize for size).
    pub(crate) strip: bool,
    /// Emit LLVM bitcode and link with `-flto`; needs an LLVM compiler driver.
    pub(crate) lto: bool,
    /// Instrument the program for profile-guided optimization.
    pub(crate) pgo_generate: bool,
    /// Use a profile (`*.profraw` or `*.profdata`) for profile-guided optimization.
    pub(crate) pgo_use: Option<PathBuf>,
    pub(crate) output: Option<PathBuf>,
    pub(crate) emit_ir: Option<PathBuf>,
    pub(crate) link_libs: Vec<String>,
    pub(crate) program_args: Vec<String>,
    pub(crate) diagnostics: DiagnosticOptions,
    // `burdin test` only.
    pub(crate) jobs: Option<usize>,
    pub(crate) verbose: bool,
    pub(crate) packages: Vec<String>,
    pub(crate) workspace: bool,
    pub(crate) ignored_only: bool,
    pub(crate) include_ignored: bool,
    pub(crate) filter: Option<String>,
    /// Milliseconds; `Some(0)` disables the timeout. `None` keeps the default.
    pub(crate) timeout_ms: Option<u64>,
}

#[derive(Debug, Default)]
pub(crate) struct DiagnosticOptions {
    format: DiagnosticFormat,
    color: ColorChoice,
    /// `None` uses `COLUMNS`, then 80; `Some(0)` disables wrapping.
    width: Option<usize>,
    /// `None` prints every diagnostic; `Some(0)` also means every diagnostic.
    error_limit: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

/// The build metadata `burdin --version` prints, in a fixed order: the version, the host target, the LLVM library
/// the compiler is linked against, the git commit, and the profile.
fn version_text() -> String {
    format!(
        "burdin {}
target: {}
llvm: {}
commit: {}
profile: {}",
        env!("CARGO_PKG_VERSION"),
        env!("BURDIN_TARGET"),
        burdin_codegen::llvm_version(),
        env!("BURDIN_GIT_COMMIT"),
        env!("BURDIN_PROFILE"),
    )
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut it = argv.iter();
    let command = it.next().ok_or("missing command")?.clone();
    if !matches!(command.as_str(), "build" | "run" | "test") {
        return Err(format!("unknown command `{command}`"));
    }
    let mut args = Args {
        command,
        target: PathBuf::new(),
        release: false,
        debug: false,
        strip: false,
        lto: false,
        pgo_generate: false,
        pgo_use: None,
        output: None,
        emit_ir: None,
        link_libs: Vec::new(),
        program_args: Vec::new(),
        diagnostics: DiagnosticOptions::default(),
        jobs: None,
        verbose: false,
        packages: Vec::new(),
        workspace: false,
        ignored_only: false,
        include_ignored: false,
        filter: None,
        timeout_ms: None,
    };
    let mut target = None;
    while let Some(raw) = it.next() {
        // Long options accept `--option value` and `--option=value`.
        let (a, inline) = match raw.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            _ => (raw.as_str(), None),
        };
        match a {
            "--release" => args.release = flag_without_value(a, inline)?,
            "-g" | "--debug-info" => args.debug = flag_without_value(a, inline)?,
            "--strip" => args.strip = flag_without_value(a, inline)?,
            "--lto" => args.lto = flag_without_value(a, inline)?,
            "--pgo-generate" => args.pgo_generate = flag_without_value(a, inline)?,
            "--pgo-use" => args.pgo_use = Some(option_value(&mut it, inline, a, "a profile path")?.into()),
            "-o" => args.output = Some(option_value(&mut it, inline, a, "a path")?.into()),
            "--emit-ir" => args.emit_ir = Some(option_value(&mut it, inline, a, "a path")?.into()),
            "--link-lib" => args.link_libs.push(option_value(&mut it, inline, a, "a name")?),
            "--diagnostic-format" => {
                args.diagnostics.format = parse_format(&option_value(&mut it, inline, a, "a value")?)?;
            }
            "--color" => {
                args.diagnostics.color = parse_color(&option_value(&mut it, inline, a, "a value")?)?;
            }
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
            "-j" | "--jobs" => {
                let value = option_value(&mut it, inline, a, "a number of tests")?;
                let n: usize = value.parse().map_err(|_| format!("`-j` needs a number of tests, got `{value}`"))?;
                if n == 0 {
                    return Err("`-j` must be at least 1".to_string());
                }
                args.jobs = Some(n);
            }
            "-v" | "--verbose" => args.verbose = flag_without_value(a, inline)?,
            "-p" => args.packages.push(option_value(&mut it, inline, a, "a package name")?),
            "--workspace" => args.workspace = flag_without_value(a, inline)?,
            "--ignored" => args.ignored_only = flag_without_value(a, inline)?,
            "--include-ignored" => args.include_ignored = flag_without_value(a, inline)?,
            "--filter" => args.filter = Some(option_value(&mut it, inline, a, "a text")?),
            "--timeout" => {
                let value = option_value(&mut it, inline, a, "a number of milliseconds")?;
                let ms: u64 =
                    value.parse().map_err(|_| format!("`--timeout` needs a number of milliseconds, got `{value}`"))?;
                args.timeout_ms = Some(ms);
            }
            "--" => {
                args.program_args.extend(it.by_ref().cloned());
            }
            _ if a.starts_with('-') => return Err(format!("unknown option `{a}`")),
            _ if target.is_none() => target = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument `{a}`")),
        }
    }
    if args.pgo_generate && args.pgo_use.is_some() {
        return Err("`--pgo-generate` and `--pgo-use` are different builds; choose one".to_string());
    }
    if args.lto && (args.pgo_generate || args.pgo_use.is_some()) {
        return Err("`--lto` cannot be combined with profile-guided optimization".to_string());
    }
    args.target = target.ok_or("missing target file")?;
    if args.command != "test" {
        if args.jobs.is_some() {
            return Err("`-j` is only valid for `burdin test`".to_string());
        }
        if args.verbose {
            return Err("`--verbose` is only valid for `burdin test`".to_string());
        }
        if !args.packages.is_empty() || args.workspace {
            return Err("`-p` and `--workspace` are only valid for `burdin test`".to_string());
        }
        if args.ignored_only || args.include_ignored {
            return Err("`--ignored` and `--include-ignored` are only valid for `burdin test`".to_string());
        }
        if args.filter.is_some() {
            return Err("`--filter` is only valid for `burdin test`".to_string());
        }
        if args.timeout_ms.is_some() {
            return Err("`--timeout` is only valid for `burdin test`".to_string());
        }
    }
    if args.ignored_only && args.include_ignored {
        return Err("`--ignored` and `--include-ignored` cannot be used together".to_string());
    }
    if args.workspace && !args.packages.is_empty() {
        return Err("`-p` and `--workspace` cannot be used together".to_string());
    }
    Ok(args)
}

/// The value of an option: the part after `=` when written inline, otherwise the next argument.
fn option_value(
    it: &mut std::slice::Iter<'_, String>,
    inline: Option<&str>,
    option: &str,
    what: &str,
) -> Result<String, String> {
    match inline {
        Some(value) => Ok(value.to_string()),
        None => it.next().cloned().ok_or_else(|| format!("`{option}` needs {what}")),
    }
}

/// A flag that takes no value; `--release=yes` is a usage error rather than being ignored.
fn flag_without_value(option: &str, inline: Option<&str>) -> Result<bool, String> {
    match inline {
        Some(_) => Err(format!("`{option}` takes no value")),
        None => Ok(true),
    }
}

fn parse_format(value: &str) -> Result<DiagnosticFormat, String> {
    match value {
        "human" => Ok(DiagnosticFormat::Human),
        "short" => Ok(DiagnosticFormat::Short),
        "json" => Ok(DiagnosticFormat::Json),
        _ => Err(format!("unknown diagnostic format `{value}` (expected human, short, or json)")),
    }
}

fn parse_color(value: &str) -> Result<ColorChoice, String> {
    match value {
        "auto" => Ok(ColorChoice::Auto),
        "always" => Ok(ColorChoice::Always),
        "never" => Ok(ColorChoice::Never),
        _ => Err(format!("unknown color choice `{value}` (expected auto, always, or never)")),
    }
}

fn parse_number(option: &str, value: &str) -> Result<usize, String> {
    value.parse::<usize>().map_err(|_| format!("`{option}` needs a non-negative number, got `{value}`"))
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if matches!(argv.as_slice(), [flag] if flag == "--version" || flag == "-V") {
        println!("{}", version_text());
        return ExitCode::SUCCESS;
    }
    if argv.first().map(String::as_str) == Some("explain") {
        return explain(&argv[1..]);
    }
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("error: {msg}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let options = CompileOptions {
        release: args.release,
        debug: args.debug,
        strip: args.strip,
        lto: args.lto,
        pgo_generate: args.pgo_generate,
        pgo_use: args.pgo_use.as_deref(),
        emit_ir: args.emit_ir.as_deref(),
        link_libs: &args.link_libs,
        test_mode: false,
        diagnostics: &args.diagnostics,
    };
    match args.command.as_str() {
        "build" => match compile(&args.target, Path::new("."), args.output.as_deref(), &options) {
            Ok(_) => ExitCode::SUCCESS,
            Err(code) => code,
        },
        "run" => run(&args, &options),
        _ => test_runner::run(&args),
    }
}

/// Parses every module, in parallel when there is more than one and more than one worker. A worker parses a
/// contiguous chunk and keeps its own diagnostics; the caller merges them in file order, so the diagnostics of a
/// parallel run are identical to a sequential one.
fn parse_files(
    files: &[(burdin_diag::FileId, std::path::PathBuf, String)],
) -> Result<(Vec<(burdin_diag::FileId, burdin_syntax::ast::File)>, Diagnostics), String> {
    let jobs = parse_jobs(files.len());
    if jobs <= 1 {
        let mut diags = Diagnostics::default();
        let parsed =
            files.iter().map(|(file, _, text)| (*file, burdin_syntax::parse(*file, text, &mut diags))).collect();
        return Ok((parsed, diags));
    }
    let chunk = files.len().div_ceil(jobs);
    let mut results: Vec<Option<((burdin_diag::FileId, burdin_syntax::ast::File), Diagnostics)>> =
        (0..files.len()).map(|_| None).collect();
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for chunk in files.chunks(chunk) {
            let len = chunk.len();
            handles.push((
                len,
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|(file, _, text)| {
                            let mut diags = Diagnostics::default();
                            let ast = burdin_syntax::parse(*file, text, &mut diags);
                            ((*file, ast), diags)
                        })
                        .collect::<Vec<_>>()
                }),
            ));
        }
        let mut start = 0usize;
        for (len, handle) in handles {
            let parsed = handle.join().map_err(|_| "internal error: a parser thread panicked".to_string())?;
            for (offset, item) in parsed.into_iter().enumerate() {
                results[start + offset] = Some(item);
            }
            start += len;
        }
        Ok::<(), String>(())
    })?;
    let mut diags = Diagnostics::default();
    let mut parsed = Vec::with_capacity(files.len());
    for (item, file_diags) in results.into_iter().map(|r| r.expect("every file parsed")) {
        parsed.push(item);
        diags.absorb(file_diags);
    }
    Ok((parsed, diags))
}

/// How many parser threads to use for `modules` files. `BURDIN_JOBS` overrides the machine's parallelism, and 1
/// keeps the run sequential (useful for comparing output and for measuring).
fn parse_jobs(modules: usize) -> usize {
    let available = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let requested = std::env::var("BURDIN_JOBS").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    job_count(modules, available, requested)
}

/// The number of workers for a run: at most one per file, never zero.
fn job_count(modules: usize, available: usize, requested: usize) -> usize {
    if modules <= 1 {
        return 1;
    }
    let workers = if requested == 0 { available } else { requested };
    workers.max(1).min(modules)
}

/// `burdin explain`: prints the reference entry for one code, or lists the codes. The text comes from the same
/// page `tests/check-error-docs.sh` compiles examples from (issue #437).
fn explain(args: &[String]) -> ExitCode {
    match args {
        [] => {
            print!("{}", code_list());
            ExitCode::SUCCESS
        }
        [code] => match explain_section(code) {
            Some(section) => {
                println!("{section}");
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("error: unknown error code `{code}`\n{EXPLAIN_USAGE}");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("error: `explain` takes one error code\n{EXPLAIN_USAGE}");
            ExitCode::from(2)
        }
    }
}

/// The section of the error reference for `code`, without the HTML comments the check uses to delimit examples.
fn explain_section(code: &str) -> Option<String> {
    let code = code.to_ascii_uppercase();
    let heading = format!("## {code}:");
    let start = ERROR_DOCS.find(&heading)?;
    let rest = &ERROR_DOCS[start..];
    let end = rest[3..].find("\n## ").map_or(rest.len(), |i| 3 + i);
    Some(
        rest[..end]
            .trim_end()
            .lines()
            .filter(|line| !line.trim_start().starts_with("<!--"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Every heading in the reference, one per line, as `E0001: Syntax error`.
fn code_list() -> String {
    let mut out = String::new();
    for line in ERROR_DOCS.lines() {
        if heading_code(line).is_some() {
            out.push_str(line.trim_start_matches("## "));
            out.push('\n');
        }
    }
    out
}

/// The code a reference heading names, if the line is one.
fn heading_code(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("## ")?;
    let bytes = rest.as_bytes();
    if bytes.len() >= 6 && bytes[0] == b'E' && bytes[1..5].iter().all(u8::is_ascii_digit) && bytes[5] == b':' {
        Some(&rest[..5])
    } else {
        None
    }
}

fn run(args: &Args, options: &CompileOptions) -> ExitCode {
    let dir = match TempDir::new() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot create a temporary directory: {e}");
            return ExitCode::from(2);
        }
    };
    let exe = match compile(&args.target, &dir.path, None, options) {
        Ok(compiled) => compiled.exe,
        Err(code) => return code,
    };
    let status = match Command::new(&exe).args(&args.program_args).status() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot run {}: {e}", exe.display());
            return ExitCode::from(2);
        }
    };
    ExitCode::from(exit_code_of(status))
}

fn exit_code_of(status: std::process::ExitStatus) -> u8 {
    if let Some(code) = status.code() {
        // Windows exit codes are 32-bit; keep a failure a failure when truncating.
        let low = (code & 0xff) as u8;
        return if code != 0 && low == 0 { 1 } else { low };
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128u8.wrapping_add(signal as u8);
        }
    }
    1
}

/// The options of one compilation.
pub(crate) struct CompileOptions<'a> {
    pub(crate) release: bool,
    /// Emit source-level debug information.
    pub(crate) debug: bool,
    /// Strip the executable.
    pub(crate) strip: bool,
    /// Emit bitcode and link with `-flto`.
    pub(crate) lto: bool,
    /// Instrument for PGO.
    pub(crate) pgo_generate: bool,
    /// Use this profile for PGO.
    pub(crate) pgo_use: Option<&'a Path>,
    pub(crate) emit_ir: Option<&'a Path>,
    pub(crate) link_libs: &'a [String],
    /// `burdin test`: declare and check the package's `test` blocks, and emit the test dispatcher instead of
    /// `main`.
    pub(crate) test_mode: bool,
    /// How diagnostics are rendered (format, color, width, and limit).
    pub(crate) diagnostics: &'a DiagnosticOptions,
}

/// What a successful compilation produced: the executable and its `test` blocks (empty for a normal build).
pub(crate) struct Compiled {
    pub(crate) exe: PathBuf,
    pub(crate) tests: Vec<burdin_tir::TestEntry>,
}

/// Compiles `target` into an executable: `output` when given, otherwise `<output_dir>/<package name>`. On failure
/// the diagnostics are already printed. With `test_mode`, the package's `test` blocks are compiled instead of its
/// `main`.
pub(crate) fn compile(
    target: &Path,
    output_dir: &Path,
    output: Option<&Path>,
    options: &CompileOptions,
) -> Result<Compiled, ExitCode> {
    let test_mode = options.test_mode;
    let loaded = match if test_mode { package::load_for_tests(target) } else { package::load(target) } {
        Ok(loaded) => loaded,
        Err(message) => {
            eprintln!("error: {message}");
            return Err(ExitCode::from(1));
        }
    };

    // File ids are assigned in module order and kept through parsing; the source map is built from the same texts
    // afterwards, so the ids it hands out match and no source text is copied.
    let mut files: Vec<(burdin_diag::FileId, PathBuf, String)> = Vec::new();
    for package in &loaded.packages {
        for module in &package.modules {
            let text = match module.text {
                Some(text) => text.to_string(),
                None => match std::fs::read_to_string(&module.file) {
                    Ok(text) => text,
                    Err(e) => {
                        eprintln!("error: cannot read {}: {e}", module.file.display());
                        return Err(ExitCode::from(1));
                    }
                },
            };
            let file = burdin_diag::FileId(files.len() as u32);
            files.push((file, module.file.clone(), text));
        }
    }
    // Parsing is per file and independent, so the modules are parsed in parallel. Diagnostics from each file are
    // merged in file order, which keeps the output identical to a sequential run (issue #432).
    let (parsed, mut diags) = match parse_files(&files) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("error: {message}");
            return Err(ExitCode::from(1));
        }
    };
    let mut sources = SourceMap::default();
    for (file, path, text) in files {
        let id = sources.add(path.display().to_string(), text);
        debug_assert_eq!(id, file, "the source map must keep the ids assigned before parsing");
    }
    // Checking a tree with syntax errors would mostly report follow-on errors.
    let program = (!diags.has_errors()).then(|| {
        let mut module_files = parsed.iter();
        let packages: Vec<burdin_sema::Package> = loaded
            .packages
            .iter()
            .enumerate()
            .map(|(id, package)| burdin_sema::Package {
                name: package.name.clone(),
                is_root: id == loaded.entry_package,
                deps: package.deps.clone(),
                root_module: package.root_module,
                modules: package
                    .modules
                    .iter()
                    .map(|module| {
                        let (file, ast) = module_files.next().expect("one parsed file per module");
                        burdin_sema::Module { file: *file, path: module.path.clone(), ast }
                    })
                    .collect(),
            })
            .collect();
        let mut program =
            burdin_sema::check(&packages, loaded.entry_package, loaded.entry_module, test_mode, &mut diags);
        // The memory checks run even when type checking failed, to report every error in the program.
        let mut memory = Diagnostics::default();
        burdin_check::check(&mut program, &mut memory);
        diags.absorb(memory);
        program
    });
    if diags.has_errors() {
        let options = render_options(options.diagnostics, std::io::stderr().is_terminal());
        let rendered = diags.render_with(&sources, &options);
        eprint!("{}", rendered.text);
        let total = rendered.total;
        let suffix = if total == 1 { "" } else { "s" };
        match options.format {
            DiagnosticFormat::Json => {
                eprint!("{}", summary_json(total, rendered.shown, options.error_limit));
            }
            _ => {
                if rendered.shown < total {
                    let hidden = total - rendered.shown;
                    let plural = if hidden == 1 { "" } else { "s" };
                    let limit = options.error_limit.unwrap_or(0);
                    eprintln!("note: {hidden} more error{plural} not shown (--error-limit {limit})");
                }
                eprintln!("error: could not compile {} ({total} error{suffix})", target.display());
                if options.format == DiagnosticFormat::Human {
                    eprintln!("note: for an explanation of an error code, run `burdin explain <code>`");
                }
            }
        }
        return Err(ExitCode::from(1));
    }
    let program = program.expect("no errors means checking produced a program");

    let fail = |msg: String| {
        eprintln!("error: {msg}");
        ExitCode::from(2)
    };
    let output = match output {
        Some(path) => path.to_path_buf(),
        None => output_dir.join(&loaded.stem).with_extension(std::env::consts::EXE_EXTENSION),
    };
    // `burdin build app` from the package's parent directory would otherwise ask the linker to replace the package
    // directory with a file.
    if output.is_dir() {
        return Err(fail(format!("the output path `{}` is a directory; pass `-o <file>`", output.display())));
    }
    let dir = TempDir::new().map_err(|e| fail(format!("cannot create a temporary directory: {e}")))?;
    let extension = if options.lto {
        "bc"
    } else if cfg!(windows) {
        "obj"
    } else {
        "o"
    };
    let object = dir.path.join(format!("program.{extension}"));
    let pgo_ir = options.pgo_use.map(|_| dir.path.join("program.pgo.ll"));
    let opts = burdin_codegen::Options {
        release: options.release,
        emit_ir: options.emit_ir.map(Path::to_path_buf),
        pgo_ir,
        debug: options.debug,
        bitcode: options.lto,
        instrument: options.pgo_generate,
    };
    burdin_codegen::emit_object(&program, &sources, &loaded.stem, &object, &opts).map_err(fail)?;
    let object = match options.pgo_use {
        Some(profile) => {
            let ir = dir.path.join("program.pgo.ll");
            pgo_object(&dir.path, &ir, profile, options.release).map_err(fail)?;
            dir.path.join(if cfg!(windows) { "program.pgo.obj" } else { "program.pgo.o" })
        }
        None => object,
    };
    let link_opts = LinkOptions {
        export_dynamic: cfg!(target_os = "linux"),
        debug: options.debug,
        strip: options.strip,
        lto: options.lto,
        pgo_generate: options.pgo_generate,
    };
    link(&object, &output, options.link_libs, &link_opts).map_err(fail)?;
    Ok(Compiled { exe: output, tests: program.tests })
}

/// The rendering choices for this run: the format from the command line, color from the choice, the environment,
/// and whether stderr is a terminal, and the width from `--width`, then `COLUMNS`, then 80.
fn render_options(diagnostics: &DiagnosticOptions, stderr_is_terminal: bool) -> RenderOptions {
    let columns = std::env::var("COLUMNS").ok();
    RenderOptions {
        format: diagnostics.format,
        color: color_enabled(diagnostics.color, no_color_requested(), stderr_is_terminal),
        width: terminal_width(diagnostics.width, columns.as_deref()),
        error_limit: diagnostics.error_limit,
    }
}

/// `--color=always` and `--color=never` are explicit and win over `NO_COLOR`; `auto` colors only a terminal
/// whose environment does not ask for no color.
fn color_enabled(choice: ColorChoice, no_color_env: bool, stderr_is_terminal: bool) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => stderr_is_terminal && !no_color_env,
    }
}

/// The no-color.org convention: `NO_COLOR` disables color when it is present and not empty.
fn no_color_requested() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty())
}

/// The width in characters: `--width` when given, then a positive `COLUMNS`, then 80. Zero disables wrapping.
fn terminal_width(explicit: Option<usize>, columns: Option<&str>) -> usize {
    if let Some(width) = explicit {
        return width;
    }
    columns.and_then(|value| value.trim().parse::<usize>().ok()).filter(|width| *width > 0).unwrap_or(80)
}

/// The final line of `--diagnostic-format=json`: one object with the counts, so a JSON consumer never has to
/// parse the human summary.
fn summary_json(errors: usize, shown: usize, limit: Option<usize>) -> String {
    let limit = limit.map_or_else(|| "null".to_string(), |limit| limit.to_string());
    format!("{{\"type\":\"summary\",\"errors\":{errors},\"shown\":{shown},\"limit\":{limit}}}\n")
}

/// Runs `--pgo-use`: merges the profile, applies `pgo-instr-use` with `opt`, and compiles the result with `llc`.
/// `opt` is used because the LLVM C API does not expose the pass builder's profile options.
fn pgo_object(dir: &Path, ir: &Path, profile: &Path, release: bool) -> Result<(), String> {
    if !profile.is_file() {
        return Err(format!("the profile `{}` does not exist", profile.display()));
    }
    let profdata_tool = llvm_tool("llvm-profdata")
        .ok_or("`--pgo-use` needs `llvm-profdata`; install the LLVM 21 tools or put them on PATH")?;
    let profdata = dir.join("merged.profdata");
    run_tool(&profdata_tool, &["merge".into(), "-o".into(), profdata.clone().into(), profile.into()])?;
    let opt = llvm_tool("opt").ok_or("`--pgo-use` needs `opt`; install the LLVM 21 tools or put them on PATH")?;
    let llc = llvm_tool("llc").ok_or("`--pgo-use` needs `llc`; install the LLVM 21 tools or put them on PATH")?;
    let pipeline = if release { "pgo-instr-use,default<O2>" } else { "pgo-instr-use" };
    let bitcode = dir.join("program.pgo.bc");
    run_tool(
        &opt,
        &[
            format!("-passes={pipeline}").into(),
            format!("-pgo-test-profile-file={}", profdata.display()).into(),
            ir.into(),
            "-o".into(),
            bitcode.clone().into(),
        ],
    )?;
    let object = dir.join(if cfg!(windows) { "program.pgo.obj" } else { "program.pgo.o" });
    run_tool(
        &llc,
        &["-relocation-model=pic".into(), "-filetype=obj".into(), bitcode.into(), "-o".into(), object.into()],
    )
}

/// Runs `tool` with `args`, reporting its output when it fails.
fn run_tool(tool: &Path, args: &[OsString]) -> Result<(), String> {
    let out = Command::new(tool).args(args).output().map_err(|e| format!("cannot run {}: {e}", tool.display()))?;
    if !out.status.success() {
        return Err(format!(
            "{} failed:\n{}{}",
            tool.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// Finds an LLVM command-line tool: `$LLVM_SYS_211_PREFIX/bin/<name>[-21]` first, then the versioned and plain
/// names on `PATH`. A candidate counts only when it runs.
fn llvm_tool(name: &str) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(prefix) = std::env::var_os("LLVM_SYS_211_PREFIX") {
        let bin = PathBuf::from(prefix).join("bin");
        for candidate in [format!("{name}-21"), name.to_string(), format!("{name}-21.1")] {
            let path = bin.join(&candidate);
            candidates.push(path.with_extension(std::env::consts::EXE_EXTENSION));
            candidates.push(path);
        }
    }
    for candidate in [format!("{name}-21"), name.to_string()] {
        candidates.push(PathBuf::from(&candidate));
        if !std::env::consts::EXE_EXTENSION.is_empty() {
            candidates.push(PathBuf::from(format!("{candidate}.{}", std::env::consts::EXE_EXTENSION)));
        }
    }
    candidates.into_iter().find(|candidate| {
        Command::new(candidate).arg("--version").output().map(|out| out.status.success()).unwrap_or(false)
    })
}

/// The link-time choices that change the linker command line.
#[derive(Default)]
struct LinkOptions {
    /// Put the program's symbols in the dynamic symbol table (ELF), for panic backtraces.
    export_dynamic: bool,
    /// Keep source-level debug information (`-g`). On macOS, `link` also writes the `.dSYM` bundle.
    debug: bool,
    /// Strip the executable (`--strip`).
    strip: bool,
    /// The input is LLVM bitcode and the linker must run LTO (`--lto`).
    lto: bool,
    /// The program carries profile instrumentation, so the profile runtime must be linked (`--pgo-generate`).
    pgo_generate: bool,
}

/// Links the object file (which defines `main` and the runtime) with libc into the executable `output`.
fn link(object: &Path, output: &Path, libs: &[String], opts: &LinkOptions) -> Result<(), String> {
    let cc = std::env::var("BURDIN_CC").unwrap_or_else(|_| if cfg!(windows) { "cl".into() } else { "cc".into() });
    if is_msvc_style(&cc) && (opts.lto || opts.pgo_generate) {
        return Err(format!(
            "`--lto` and `--pgo-generate` need an LLVM compiler driver, and `{cc}` is not one; \
             set BURDIN_CC to clang or clang-cl"
        ));
    }
    let mut cmd = Command::new(&cc);
    cmd.args(link_args(&cc, object, output, libs, opts));
    let out = cmd.output().map_err(|e| linker_error(&cc, e))?;
    if !out.status.success() {
        let hint = if opts.lto || opts.pgo_generate {
            "\nnote: linking LLVM bitcode or the profile runtime needs an LLVM compiler driver; set BURDIN_CC=clang"
        } else {
            ""
        };
        return Err(format!(
            "linking failed ({cc}):\n{}{}{hint}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    if opts.debug && cfg!(target_os = "macos") {
        write_dsym(output)?;
    }
    Ok(())
}

/// ld64 keeps DWARF out of the executable and writes a debug map instead; `dsymutil` combines the object's DWARF
/// into `<output>.dSYM`, which is where lldb and other Darwin debuggers read it from.
fn write_dsym(output: &Path) -> Result<(), String> {
    let mut dsym = output.as_os_str().to_os_string();
    dsym.push(".dSYM");
    let dsym = PathBuf::from(dsym);
    let out = Command::new("dsymutil").arg(output).arg("-o").arg(&dsym).output().map_err(|e| {
        format!(
            "cannot run `dsymutil`: {e}; source-level debug information on macOS needs the Xcode command-line tools"
        )
    })?;
    if !out.status.success() {
        return Err(format!("`dsymutil` failed for `{}`:\n{}", output.display(), String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

/// The error for a linker that did not start. A missing `cl` is by far the most common case on Windows: the driver
/// falls back to `cl`, but it is only on `PATH` inside a Developer Command Prompt.
fn linker_error(cc: &str, error: std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        format!(
            "cannot find the linker `{cc}` on PATH; run from a Visual Studio Developer Command Prompt \
             (so `cl` is on PATH) or set BURDIN_CC"
        )
    } else {
        format!("cannot run the linker `{cc}`: {error} (set BURDIN_CC)")
    }
}

/// The linker command line. Split out so the MSVC branch, which cannot run on Linux CI, has unit tests.
fn link_args(cc: &str, object: &Path, output: &Path, libs: &[String], opts: &LinkOptions) -> Vec<OsString> {
    if is_msvc_style(cc) {
        // `LLVM-C.lib` lives in the LLVM installation, which is not in the linker's `LIB` path. `LLVM_SYS_211_PREFIX`
        // and `llvm-config` are how the rest of the build finds that installation.
        let llvm_libdir = libs.iter().any(|l| l == "LLVM-C").then(resolve_llvm_c_libdir).flatten();
        let mut args = msvc_link_args(object, output, libs, llvm_libdir.as_deref());
        if opts.strip {
            // Without `/DEBUG` link.exe writes no debug directory; `/OPT:REF` drops unreferenced sections.
            args.push("/OPT:REF".into());
            args.push("/OPT:ICF".into());
        }
        args
    } else {
        unix_link_args(object, output, libs, opts)
    }
}

/// `cl` and `cl.exe`, bare or as a path; `clang-cl` accepts the same flags. Both separators are recognized so the
/// check does not depend on the host `Path` semantics.
fn is_msvc_style(cc: &str) -> bool {
    let name = cc.rsplit(['/', '\\']).next().unwrap_or(cc);
    name.eq_ignore_ascii_case("cl")
        || name.eq_ignore_ascii_case("cl.exe")
        || name.eq_ignore_ascii_case("clang-cl")
        || name.eq_ignore_ascii_case("clang-cl.exe")
}

fn msvc_link_args(object: &Path, output: &Path, libs: &[String], llvm_libdir: Option<&Path>) -> Vec<OsString> {
    // MSVC: cl only calls link.exe. The object file names no C runtime, so name the static one.
    let mut args: Vec<OsString> = vec![
        "/nologo".into(),
        object.into(),
        format!("/Fe:{}", output.display()).into(),
        "/link".into(),
        "libcmt.lib".into(),
        "libucrt.lib".into(),
        "libvcruntime.lib".into(),
        // The runtime calls `CaptureStackBackTrace` for panic backtraces. An object emitted by LLVM carries no
        // default-library directives, so name kernel32 explicitly; `cl` does not add it for a bare object.
        "kernel32.lib".into(),
    ];
    if let Some(dir) = llvm_libdir {
        args.push(format!("/LIBPATH:{}", dir.display()).into());
    }
    for lib in libs {
        args.push(format!("{lib}.lib").into());
    }
    args
}

fn unix_link_args(object: &Path, output: &Path, libs: &[String], opts: &LinkOptions) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["-o".into(), output.into(), object.into()];
    if opts.debug {
        // ld64 keeps debug sections only when the link names -g; GNU ld ignores the flag.
        args.push("-g".into());
    }
    if opts.export_dynamic {
        args.push("-rdynamic".into());
    }
    if opts.lto {
        args.push("-flto".into());
    }
    if opts.pgo_generate {
        args.push("-fprofile-instr-generate".into());
    }
    if opts.strip {
        args.push("-s".into());
        if cfg!(target_os = "macos") {
            // `-s` removes the symbol table but Mach-O still exports every global function through the export
            // trie, and `nm` reports those names. A stripped executable needs to export only its entry point.
            args.push("-Wl,-exported_symbol,_main".into());
        }
    }
    for lib in libs {
        match resolve_llvm_c(lib) {
            // `LLVM-C` is the C API of the LLVM shared library. Ubuntu installs it as `libLLVM-21.so`, not as
            // `libLLVM-C.so`, so `llvm-config` says which directory and flag to use.
            Some((dir, flag)) => {
                args.push(format!("-L{}", dir.display()).into());
                args.push(format!("-Wl,-rpath,{}", dir.display()).into());
                args.push(flag.into());
            }
            None => args.push(format!("-l{lib}").into()),
        }
    }
    args
}

/// The directory holding `LLVM-C.lib` on Windows/MSVC, from `LLVM_SYS_211_PREFIX` or `llvm-config`.
fn resolve_llvm_c_libdir() -> Option<PathBuf> {
    if let Some(prefix) = std::env::var_os("LLVM_SYS_211_PREFIX") {
        let dir = PathBuf::from(prefix).join("lib");
        if dir.join("LLVM-C.lib").is_file() {
            return Some(dir);
        }
    }
    llvm_config().map(|(dir, _)| dir).filter(|dir| dir.join("LLVM-C.lib").is_file())
}

/// `--link-lib LLVM-C` on a Unix host: the directory to search and the flag that names the LLVM shared library
/// which exports the C API. Other names are left to the linker's own search path.
fn resolve_llvm_c(name: &str) -> Option<(PathBuf, String)> {
    if name != "LLVM-C" {
        return None;
    }
    let (dir, libs) = llvm_config()?;
    llvm_c_link(&dir, libs)
}

/// Picks the linker flag for the LLVM shared library in `dir`. Linux installs `libLLVM-C.so` or a versioned
/// `libLLVM-21.so`; Homebrew on macOS installs `libLLVM.dylib`. `libs` is what `llvm-config --link-shared --libs`
/// reported, used when the directory listing does not name a single library.
fn llvm_c_link(dir: &Path, libs: Option<String>) -> Option<(PathBuf, String)> {
    for stem in ["LLVM-C", "LLVM"] {
        for extension in ["so", "dylib"] {
            if dir.join(format!("lib{stem}.{extension}")).is_file() {
                return Some((dir.to_path_buf(), format!("-l{stem}")));
            }
        }
    }
    if let Some(flag) = libs {
        return Some((dir.to_path_buf(), flag));
    }
    // The versioned development symlink: `libLLVM-21.so` or `libLLVM-21.dylib` is `-lLLVM-21`.
    let mut found = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let file = entry.file_name().to_string_lossy().to_string();
        if let Some(stem) =
            file.strip_prefix("lib").and_then(|f| f.strip_suffix(".so").or_else(|| f.strip_suffix(".dylib")))
            && stem.starts_with("LLVM-")
        {
            found = Some(format!("-l{stem}"));
            break;
        }
    }
    found.map(|flag| (dir.to_path_buf(), flag))
}

/// The LLVM installation's library directory and the shared-library flag `llvm-config` reports for it. The tool is
/// looked up in `LLVM_SYS_211_PREFIX` (the variable that configures the `inkwell` build) and then on `PATH`, because
/// Homebrew's LLVM is keg-only and its `bin` directory is usually not on `PATH`.
fn llvm_config() -> Option<(PathBuf, Option<String>)> {
    let mut tools = Vec::new();
    if let Some(prefix) = std::env::var_os("LLVM_SYS_211_PREFIX") {
        tools.push(PathBuf::from(prefix).join("bin").join("llvm-config"));
    }
    tools.extend(
        ["llvm-config", "llvm-config-21", "llvm-config-20", "llvm-config-19", "llvm-config-18", "llvm-config-17"]
            .map(PathBuf::from),
    );
    for tool in tools {
        let Ok(out) = Command::new(&tool).arg("--libdir").output() else { continue };
        if !out.status.success() {
            continue;
        }
        let dir = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        if !dir.is_dir() {
            continue;
        }
        let libs = Command::new(&tool)
            .args(["--link-shared", "--libs"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| {
                let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
                (text.starts_with("-l") && !text.contains(' ')).then_some(text)
            });
        return Some((dir, libs));
    }
    None
}

/// A uniquely named temporary directory, removed when dropped. The driver and the parallel test runner create
/// these from several threads, so the name includes an atomic counter as well as the process id and the time.
pub(crate) struct TempDir {
    pub(crate) path: PathBuf,
}

impl TempDir {
    pub(crate) fn new() -> std::io::Result<TempDir> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        for attempt in 0..100u32 {
            let count = COUNTER.fetch_add(1, Ordering::Relaxed);
            let name = format!("burdin-{}-{nanos}-{count}-{attempt}", std::process::id());
            let path = std::env::temp_dir().join(name);
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(TempDir { path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::other("no free temporary directory name"))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn as_strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn job_count_respects_the_request_and_the_file_count() {
        assert_eq!(job_count(0, 8, 0), 1);
        assert_eq!(job_count(1, 8, 0), 1);
        assert_eq!(job_count(4, 8, 0), 4);
        assert_eq!(job_count(64, 8, 0), 8);
        assert_eq!(job_count(64, 8, 1), 1, "BURDIN_JOBS=1 is sequential");
        assert_eq!(job_count(64, 8, 3), 3);
        assert_eq!(job_count(2, 8, 99), 2, "never more workers than files");
    }

    #[test]
    fn version_metadata_has_the_documented_fields() {
        let text = version_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5, "{text}");
        assert!(lines[0].starts_with("burdin "), "{text}");
        for prefix in ["target: ", "llvm: ", "commit: ", "profile: "] {
            assert!(lines.iter().any(|line| line.starts_with(prefix)), "missing {prefix}: {text}");
        }
        let llvm = lines.iter().find(|line| line.starts_with("llvm: ")).unwrap();
        assert!(llvm["llvm: ".len()..].split('.').count() == 3, "{llvm}");
    }

    #[test]
    fn msvc_link_args_name_the_static_crt_and_map_libraries_to_lib_files() {
        let args =
            as_strings(msvc_link_args(Path::new("program.obj"), Path::new("out.exe"), &["LLVM-C".to_string()], None));
        assert_eq!(args[0], "/nologo");
        assert!(args.contains(&"/link".to_string()));
        assert!(args.contains(&"libcmt.lib".to_string()));
        assert!(args.contains(&"libucrt.lib".to_string()));
        assert!(args.contains(&"libvcruntime.lib".to_string()));
        assert!(args.contains(&"kernel32.lib".to_string()));
        assert!(args.contains(&"LLVM-C.lib".to_string()));
        assert!(!args.iter().any(|arg| arg.starts_with("/LIBPATH:")));
    }

    #[test]
    fn msvc_link_args_add_the_llvm_library_directory() {
        let args = as_strings(msvc_link_args(
            Path::new("program.obj"),
            Path::new("out.exe"),
            &["LLVM-C".to_string()],
            Some(Path::new(r"C:\llvm\lib")),
        ));
        assert!(args.contains(&r"/LIBPATH:C:\llvm\lib".to_string()));
    }

    #[test]
    fn msvc_linker_is_recognized_by_name_or_path() {
        for cc in ["cl", "cl.exe", "CL.EXE", r"C:\VS\bin\cl.exe", "clang-cl", "clang-cl.exe"] {
            assert!(is_msvc_style(cc), "{cc} should use the MSVC command line");
        }
        for cc in ["cc", "cc.exe", "gcc", "clang"] {
            assert!(!is_msvc_style(cc), "{cc} should not use the MSVC command line");
        }
    }

    /// An empty directory removed when dropped; each test gets its own.
    struct LibDir(PathBuf);

    impl LibDir {
        fn new(name: &str) -> LibDir {
            let path = std::env::temp_dir().join(format!("burdin-link-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).unwrap();
            LibDir(path)
        }

        fn touch(&self, name: &str) {
            fs::write(self.0.join(name), b"").unwrap();
        }
    }

    impl Drop for LibDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn unix_link_args_use_the_cc_command_line() {
        let args = as_strings(unix_link_args(
            Path::new("program.o"),
            Path::new("out"),
            &["m".to_string()],
            &LinkOptions::default(),
        ));
        assert_eq!(args, ["-o", "out", "program.o", "-lm"]);
    }

    #[test]
    fn unix_link_args_keep_debug_information() {
        let opts = LinkOptions { debug: true, ..Default::default() };
        let args = as_strings(unix_link_args(Path::new("program.o"), Path::new("out"), &[], &opts));
        assert!(args.contains(&"-g".to_string()), "{args:?}");
    }

    #[test]
    fn unix_link_args_export_symbols_for_backtraces() {
        let opts = LinkOptions { export_dynamic: true, ..Default::default() };
        let args = as_strings(unix_link_args(Path::new("program.o"), Path::new("out"), &[], &opts));
        assert!(args.contains(&"-rdynamic".to_string()), "{args:?}");
    }

    #[test]
    fn a_missing_linker_is_reported_with_a_fix() {
        let error = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
        let message = linker_error("cl", error);
        assert!(message.contains("cannot find the linker `cl` on PATH"), "{message}");
        assert!(message.contains("Developer Command Prompt"), "{message}");
        assert!(message.contains("BURDIN_CC"), "{message}");
    }

    #[test]
    fn llvm_c_prefers_the_c_api_library() {
        let dir = LibDir::new("c-api");
        dir.touch("libLLVM.so");
        dir.touch("libLLVM-C.so");
        let (found, flag) = llvm_c_link(&dir.0, None).unwrap();
        assert_eq!(found, dir.0);
        assert_eq!(flag, "-lLLVM-C");
    }

    #[test]
    fn llvm_c_finds_the_homebrew_dylib() {
        let dir = LibDir::new("homebrew");
        dir.touch("libLLVM.dylib");
        let (_, flag) = llvm_c_link(&dir.0, None).unwrap();
        assert_eq!(flag, "-lLLVM");
    }

    #[test]
    fn llvm_c_finds_a_versioned_dylib() {
        let dir = LibDir::new("versioned-dylib");
        dir.touch("libLLVM-21.dylib");
        let (_, flag) = llvm_c_link(&dir.0, None).unwrap();
        assert_eq!(flag, "-lLLVM-21");
    }

    #[test]
    fn llvm_c_falls_back_to_the_llvm_config_flag() {
        let dir = LibDir::new("llvm-config");
        let (_, flag) = llvm_c_link(&dir.0, Some("-lLLVM-21".to_string())).unwrap();
        assert_eq!(flag, "-lLLVM-21");
    }

    #[test]
    fn llvm_c_ignores_other_library_names() {
        let dir = LibDir::new("empty");
        assert!(llvm_c_link(&dir.0, None).is_none());
        assert!(resolve_llvm_c("m").is_none());
    }

    fn parse(words: &[&str]) -> Result<Args, String> {
        let argv: Vec<String> = words.iter().map(|word| word.to_string()).collect();
        parse_args(&argv)
    }

    #[test]
    fn test_options_are_parsed() {
        let args = parse(&[
            "test",
            "app",
            "-j",
            "3",
            "--verbose",
            "-p",
            "lib",
            "--filter",
            "add",
            "--timeout",
            "250",
            "--ignored",
        ])
        .unwrap();
        assert_eq!(args.command, "test");
        assert_eq!(args.target, PathBuf::from("app"));
        assert_eq!(args.jobs, Some(3));
        assert!(args.verbose);
        assert_eq!(args.packages, ["lib"]);
        assert_eq!(args.filter.as_deref(), Some("add"));
        assert_eq!(args.timeout_ms, Some(250));
        assert!(args.ignored_only);
    }

    #[test]
    fn test_options_are_rejected_for_other_commands() {
        assert!(parse(&["build", "app", "-j", "2"]).unwrap_err().contains("only valid for `burdin test`"));
        assert!(parse(&["run", "app", "--verbose"]).unwrap_err().contains("only valid for `burdin test`"));
        assert!(parse(&["build", "app", "--workspace"]).unwrap_err().contains("only valid for `burdin test`"));
    }

    #[test]
    fn conflicting_test_options_are_rejected() {
        assert!(parse(&["test", "app", "-j", "0"]).unwrap_err().contains("at least 1"));
        assert!(parse(&["test", "app", "-j", "many"]).unwrap_err().contains("needs a number"));
        assert!(
            parse(&["test", "app", "--ignored", "--include-ignored"]).unwrap_err().contains("cannot be used together")
        );
        assert!(parse(&["test", "app", "-p", "lib", "--workspace"]).unwrap_err().contains("cannot be used together"));
        assert!(parse(&["test", "app", "--timeout", "soon"]).unwrap_err().contains("milliseconds"));
    }

    #[test]
    fn color_choice_interacts_with_no_color_and_the_terminal() {
        assert!(color_enabled(ColorChoice::Always, true, false));
        assert!(!color_enabled(ColorChoice::Never, false, true));
        assert!(color_enabled(ColorChoice::Auto, false, true));
        assert!(!color_enabled(ColorChoice::Auto, true, true));
        assert!(!color_enabled(ColorChoice::Auto, false, false));
    }

    #[test]
    fn width_comes_from_the_flag_then_columns_then_the_default() {
        assert_eq!(terminal_width(Some(100), Some("40")), 100);
        assert_eq!(terminal_width(None, Some("40")), 40);
        assert_eq!(terminal_width(None, Some(" 100 ")), 100);
        assert_eq!(terminal_width(None, Some("0")), 80);
        assert_eq!(terminal_width(None, Some("wide")), 80);
        assert_eq!(terminal_width(None, None), 80);
        assert_eq!(terminal_width(Some(0), Some("40")), 0);
    }

    #[test]
    fn parse_args_reads_the_diagnostic_options() {
        let argv: Vec<String> = [
            "build",
            "app.bd",
            "--diagnostic-format",
            "json",
            "--color",
            "always",
            "--width",
            "100",
            "--error-limit",
            "3",
        ]
        .map(String::from)
        .to_vec();
        let args = parse_args(&argv).unwrap();
        assert_eq!(args.diagnostics.format, DiagnosticFormat::Json);
        assert_eq!(args.diagnostics.color, ColorChoice::Always);
        assert_eq!(args.diagnostics.width, Some(100));
        assert_eq!(args.diagnostics.error_limit, Some(3));
        let argv: Vec<String> = ["run", "app.bd", "--no-color", "--error-limit", "0"].map(String::from).to_vec();
        let args = parse_args(&argv).unwrap();
        assert_eq!(args.diagnostics.color, ColorChoice::Never);
        assert_eq!(args.diagnostics.error_limit, None);
    }

    #[test]
    fn parse_args_rejects_bad_diagnostic_values() {
        for argv in [
            ["build", "a.bd", "--diagnostic-format", "xml"],
            ["build", "a.bd", "--color", "sparkles"],
            ["build", "a.bd", "--width", "wide"],
            ["build", "a.bd", "--error-limit", "-1"],
        ] {
            let argv: Vec<String> = argv.map(String::from).to_vec();
            assert!(parse_args(&argv).is_err(), "{argv:?} should be rejected");
        }
    }

    #[test]
    fn the_last_color_flag_wins() {
        let argv: Vec<String> = ["build", "a.bd", "--color", "always", "--no-color"].map(String::from).to_vec();
        assert_eq!(parse_args(&argv).unwrap().diagnostics.color, ColorChoice::Never);
        let argv: Vec<String> = ["build", "a.bd", "--no-color", "--color", "always"].map(String::from).to_vec();
        assert_eq!(parse_args(&argv).unwrap().diagnostics.color, ColorChoice::Always);
    }

    fn is_code(text: &str) -> bool {
        let bytes = text.as_bytes();
        bytes.len() == 5 && bytes[0] == b'E' && bytes[1..].iter().all(u8::is_ascii_digit)
    }

    #[test]
    fn every_registry_code_has_a_checked_reference_section() {
        let registry = include_str!("../../../tests/README.md");
        let mut codes: Vec<&str> = registry
            .lines()
            .filter_map(|line| {
                let rest = line.strip_prefix("| ")?;
                let code = &rest[..rest.find(' ')?];
                is_code(code).then_some(code)
            })
            .collect();
        assert!(codes.len() > 20, "the registry has only {} codes", codes.len());
        codes.sort_unstable();
        codes.dedup();
        for code in &codes {
            let section = explain_section(code).unwrap_or_else(|| panic!("no reference section for {code}"));
            assert!(section.contains(&format!("## {code}:")), "{code} has no heading: {section}");
            assert!(section.contains("**How to fix:**"), "{code} has no fix");
            assert!(section.contains(&format!("//~ {code}")), "{code} prints no checked example");
            assert!(ERROR_DOCS.contains(&format!("error-example: {code}")), "{code} has no checked example");
        }
        let listing = code_list();
        let listed: Vec<&str> = listing.lines().filter_map(|line| line.split(':').next()).collect();
        assert_eq!(listed, codes, "the reference must list exactly the registry codes");
        assert!(explain_section("E9999").is_none());
        assert!(explain_section("e0003").is_some(), "codes are accepted in lower case");
    }
}
