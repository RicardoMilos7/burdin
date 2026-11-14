//! The `burdin` command: `build`, `run`, and `test` (the contract is at the top of tests/run-tests.sh).

mod package;

use burdin_diag::{Diagnostics, SourceMap};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage:
  burdin build <file.bd | package directory> [--release] [-o <output>] [--emit-ir <file.ll>] [--link-lib <name>]...
  burdin run <file.bd | package directory> [--release] [--link-lib <name>]... [-- <program arguments>...]
  burdin test <file.bd | package directory> [--release]

environment:
  BURDIN_CC   C compiler driver used to link against libc (default: cc; cl on Windows)";

struct Args {
    command: String,
    target: PathBuf,
    release: bool,
    output: Option<PathBuf>,
    emit_ir: Option<PathBuf>,
    link_libs: Vec<String>,
    program_args: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let command = it.next().ok_or("missing command")?;
    if !matches!(command.as_str(), "build" | "run" | "test") {
        return Err(format!("unknown command `{command}`"));
    }
    let mut args = Args {
        command,
        target: PathBuf::new(),
        release: false,
        output: None,
        emit_ir: None,
        link_libs: Vec::new(),
        program_args: Vec::new(),
    };
    let mut target = None;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--release" => args.release = true,
            "-o" => args.output = Some(it.next().ok_or("`-o` needs a path")?.into()),
            "--emit-ir" => args.emit_ir = Some(it.next().ok_or("`--emit-ir` needs a path")?.into()),
            "--link-lib" => args.link_libs.push(it.next().ok_or("`--link-lib` needs a name")?),
            "--" => {
                args.program_args.extend(it.by_ref());
            }
            _ if a.starts_with('-') => return Err(format!("unknown option `{a}`")),
            _ if target.is_none() => target = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument `{a}`")),
        }
    }
    args.target = target.ok_or("missing target file")?;
    Ok(args)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("error: {msg}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match args.command.as_str() {
        "build" => match compile(&args, Path::new("."), args.output.as_deref()) {
            Ok(_) => ExitCode::SUCCESS,
            Err(code) => code,
        },
        "run" => run(&args),
        _ => {
            eprintln!("error: `burdin test` is not implemented yet (milestone 10)");
            ExitCode::from(2)
        }
    }
}

fn run(args: &Args) -> ExitCode {
    let dir = match TempDir::new() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot create a temporary directory: {e}");
            return ExitCode::from(2);
        }
    };
    let exe = match compile(args, &dir.path, None) {
        Ok(exe) => exe,
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

/// Compiles `args.target` into an executable: `output` when given, otherwise `<output_dir>/<package name>`. On
/// failure the diagnostics are already printed. Returns the executable's path.
fn compile(args: &Args, output_dir: &Path, output: Option<&Path>) -> Result<PathBuf, ExitCode> {
    let target = &args.target;
    let loaded = match package::load(target) {
        Ok(loaded) => loaded,
        Err(message) => {
            eprintln!("error: {message}");
            return Err(ExitCode::from(1));
        }
    };

    let mut sources = SourceMap::default();
    let mut parsed: Vec<(burdin_diag::FileId, burdin_syntax::ast::File)> = Vec::new();
    let mut diags = Diagnostics::default();
    for package in &loaded.packages {
        for module in &package.modules {
            let text = match std::fs::read_to_string(&module.file) {
                Ok(text) => text,
                Err(e) => {
                    eprintln!("error: cannot read {}: {e}", module.file.display());
                    return Err(ExitCode::from(1));
                }
            };
            let file = sources.add(module.file.display().to_string(), text);
            let ast = burdin_syntax::parse(file, &sources.get(file).text, &mut diags);
            parsed.push((file, ast));
        }
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
        let mut program = burdin_sema::check(&packages, loaded.entry_package, loaded.entry_module, &mut diags);
        // The memory checks run even when type checking failed, to report every error in the program.
        let mut memory = Diagnostics::default();
        burdin_check::check(&mut program, &mut memory);
        diags.absorb(memory);
        program
    });
    if diags.has_errors() {
        eprint!("{}", diags.render(&sources));
        let n = diags.len();
        eprintln!("error: could not compile {} ({n} error{})", target.display(), if n == 1 { "" } else { "s" });
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
    let object = dir.path.join(if cfg!(windows) { "program.obj" } else { "program.o" });
    let opts = burdin_codegen::Options { release: args.release, emit_ir: args.emit_ir.clone() };
    burdin_codegen::emit_object(&program, &sources, &loaded.stem, &object, &opts).map_err(fail)?;
    link(&object, &output, &args.link_libs).map_err(fail)?;
    Ok(output)
}

/// Links the object file (which defines `main` and the runtime) with libc into the executable `output`.
fn link(object: &Path, output: &Path, libs: &[String]) -> Result<(), String> {
    let cc = std::env::var("BURDIN_CC").unwrap_or_else(|_| if cfg!(windows) { "cl".into() } else { "cc".into() });
    let mut cmd = Command::new(&cc);
    let msvc_style = cc.ends_with("cl") || cc.ends_with("cl.exe");
    if msvc_style {
        // MSVC: cl only calls link.exe. The object file names no C runtime, so name the static one.
        cmd.arg("/nologo").arg(object).arg(format!("/Fe:{}", output.display()));
        cmd.arg("/link").arg("libcmt.lib").arg("libucrt.lib").arg("libvcruntime.lib");
        for l in libs {
            cmd.arg(format!("{l}.lib"));
        }
    } else {
        cmd.arg("-o").arg(output).arg(object);
        for l in libs {
            cmd.arg(format!("-l{l}"));
        }
    }
    let out = cmd.output().map_err(|e| format!("cannot run the linker `{cc}`: {e} (set BURDIN_CC)"))?;
    if !out.status.success() {
        return Err(format!(
            "linking failed ({cc}):\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// A uniquely named temporary directory, removed when dropped.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> std::io::Result<TempDir> {
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        for attempt in 0..100u32 {
            let name = format!("burdin-{}-{nanos}-{attempt}", std::process::id());
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
