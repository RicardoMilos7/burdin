//! The `burdin` command: `build`, `run`, and `test`. The contract is documented in `README.md` at the repository
//! root (`The driver contract`) and summarized at the top of tests/run-tests.sh.

mod package;
mod test_runner;

use burdin_diag::{Diagnostics, SourceMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub(crate) const USAGE: &str = "\
usage:
  burdin build <file.bd | package directory> [--release] [-o <output>] [--emit-ir <file.ll>] [--link-lib <name>]...
  burdin run <file.bd | package directory> [--release] [--link-lib <name>]... [-- <program arguments>...]
  burdin test <file.bd | package directory> [--release] [-j <n>] [--verbose] [-p <package>]... [--workspace]
              [--filter <text>] [--ignored | --include-ignored] [--timeout <ms>]

test options:
  -j, --jobs <n>       tests to run at once (default: the number of CPUs)
  -p <package>         run the tests of one package of the build; may repeat
  --workspace          run the tests of the entry package and every path dependency
  --filter <text>      run only tests whose name contains text
  --ignored            run only tests marked [ignore]
  --include-ignored    run ignored tests too
  --timeout <ms>       per-test timeout (default: 60000; 0 disables it)
  -v, --verbose        print the output of passing tests too

environment:
  BURDIN_CC            C compiler driver used to link against libc (default: cc; cl on Windows)
  LLVM_SYS_211_PREFIX  LLVM 21 installation; its bin/llvm-config resolves `--link-lib LLVM-C`";

#[derive(Debug)]
pub(crate) struct Args {
    pub(crate) command: String,
    pub(crate) target: PathBuf,
    pub(crate) release: bool,
    pub(crate) output: Option<PathBuf>,
    pub(crate) emit_ir: Option<PathBuf>,
    pub(crate) link_libs: Vec<String>,
    pub(crate) program_args: Vec<String>,
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

fn parse_args() -> Result<Args, String> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from<I: Iterator<Item = String>>(it: I) -> Result<Args, String> {
    let mut it = it;
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
    while let Some(a) = it.next() {
        match a.as_str() {
            "--release" => args.release = true,
            "-o" => args.output = Some(it.next().ok_or("`-o` needs a path")?.into()),
            "--emit-ir" => args.emit_ir = Some(it.next().ok_or("`--emit-ir` needs a path")?.into()),
            "--link-lib" => args.link_libs.push(it.next().ok_or("`--link-lib` needs a name")?),
            "-j" | "--jobs" => {
                let value = it.next().ok_or("`-j` needs a number")?;
                let n: usize = value.parse().map_err(|_| format!("`-j` needs a number of tests, got `{value}`"))?;
                if n == 0 {
                    return Err("`-j` must be at least 1".to_string());
                }
                args.jobs = Some(n);
            }
            "-v" | "--verbose" => args.verbose = true,
            "-p" => args.packages.push(it.next().ok_or("`-p` needs a package name")?),
            "--workspace" => args.workspace = true,
            "--ignored" => args.ignored_only = true,
            "--include-ignored" => args.include_ignored = true,
            "--filter" => args.filter = Some(it.next().ok_or("`--filter` needs a text")?),
            "--timeout" => {
                let value = it.next().ok_or("`--timeout` needs a number of milliseconds")?;
                let ms: u64 =
                    value.parse().map_err(|_| format!("`--timeout` needs a number of milliseconds, got `{value}`"))?;
                args.timeout_ms = Some(ms);
            }
            "--" => {
                args.program_args.extend(it.by_ref());
            }
            _ if a.starts_with('-') => return Err(format!("unknown option `{a}`")),
            _ if target.is_none() => target = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument `{a}`")),
        }
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

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("error: {msg}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let options = CompileOptions {
        release: args.release,
        emit_ir: args.emit_ir.as_deref(),
        link_libs: &args.link_libs,
        test_mode: false,
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
    pub(crate) emit_ir: Option<&'a Path>,
    pub(crate) link_libs: &'a [String],
    /// `burdin test`: declare and check the package's `test` blocks, and emit the test dispatcher instead of
    /// `main`.
    pub(crate) test_mode: bool,
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

    let mut sources = SourceMap::default();
    let mut parsed: Vec<(burdin_diag::FileId, burdin_syntax::ast::File)> = Vec::new();
    let mut diags = Diagnostics::default();
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
        let mut program =
            burdin_sema::check(&packages, loaded.entry_package, loaded.entry_module, test_mode, &mut diags);
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
    let opts = burdin_codegen::Options { release: options.release, emit_ir: options.emit_ir.map(Path::to_path_buf) };
    burdin_codegen::emit_object(&program, &sources, &loaded.stem, &object, &opts).map_err(fail)?;
    link(&object, &output, options.link_libs).map_err(fail)?;
    Ok(Compiled { exe: output, tests: program.tests })
}

/// Links the object file (which defines `main` and the runtime) with libc into the executable `output`.
fn link(object: &Path, output: &Path, libs: &[String]) -> Result<(), String> {
    let cc = std::env::var("BURDIN_CC").unwrap_or_else(|_| if cfg!(windows) { "cl".into() } else { "cc".into() });
    let mut cmd = Command::new(&cc);
    cmd.args(link_args(&cc, object, output, libs));
    let out = cmd.output().map_err(|e| linker_error(&cc, e))?;
    if !out.status.success() {
        return Err(format!(
            "linking failed ({cc}):\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
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
fn link_args(cc: &str, object: &Path, output: &Path, libs: &[String]) -> Vec<OsString> {
    if is_msvc_style(cc) {
        // `LLVM-C.lib` lives in the LLVM installation, which is not in the linker's `LIB` path. `LLVM_SYS_211_PREFIX`
        // and `llvm-config` are how the rest of the build finds that installation.
        let llvm_libdir = libs.iter().any(|l| l == "LLVM-C").then(resolve_llvm_c_libdir).flatten();
        msvc_link_args(object, output, libs, llvm_libdir.as_deref())
    } else {
        unix_link_args(object, output, libs)
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
    ];
    if let Some(dir) = llvm_libdir {
        args.push(format!("/LIBPATH:{}", dir.display()).into());
    }
    for lib in libs {
        args.push(format!("{lib}.lib").into());
    }
    args
}

fn unix_link_args(object: &Path, output: &Path, libs: &[String]) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["-o".into(), output.into(), object.into()];
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
    fn msvc_link_args_name_the_static_crt_and_map_libraries_to_lib_files() {
        let args =
            as_strings(msvc_link_args(Path::new("program.obj"), Path::new("out.exe"), &["LLVM-C".to_string()], None));
        assert_eq!(args[0], "/nologo");
        assert!(args.contains(&"/link".to_string()));
        assert!(args.contains(&"libcmt.lib".to_string()));
        assert!(args.contains(&"libucrt.lib".to_string()));
        assert!(args.contains(&"libvcruntime.lib".to_string()));
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
        let args = as_strings(unix_link_args(Path::new("program.o"), Path::new("out"), &["m".to_string()]));
        assert_eq!(args, ["-o", "out", "program.o", "-lm"]);
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
        parse_args_from(words.iter().map(|word| word.to_string()))
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
}
