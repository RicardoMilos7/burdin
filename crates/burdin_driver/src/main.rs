//! The `burdin` command: `build`, `run`, and `test`. The contract is documented in `README.md` at the repository
//! root (`The driver contract`) and summarized at the top of tests/run-tests.sh.

mod package;

use burdin_diag::{Diagnostics, SourceMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage:
  burdin build <file.bd | package directory> [--release] [-g] [--strip] [--lto]
               [--pgo-generate | --pgo-use <profile>]
               [-o <output>] [--emit-ir <file.ll>] [--link-lib <name>]...
  burdin run <file.bd | package directory> [--release] [-g] [--link-lib <name>]... [-- <program arguments>...]
  burdin test <file.bd | package directory> [--release]
  burdin --version

environment:
  BURDIN_CC            C compiler driver used to link against libc (default: cc; cl on Windows)
  BURDIN_COMMIT        overrides the git commit printed by `--version`
  BURDIN_JOBS          parser threads (default: the machine's parallelism; 1 is sequential)
  LLVM_SYS_211_PREFIX  LLVM 21 installation; its bin/llvm-config resolves `--link-lib LLVM-C`";

/// The build metadata `burdin --version` prints, in a fixed order: the version, the host target, the LLVM library
/// the compiler is linked against, the git commit, and the profile.
fn version_text() -> String {
    format!(
        "burdin {}\ntarget: {}\nllvm: {}\ncommit: {}\nprofile: {}",
        env!("CARGO_PKG_VERSION"),
        env!("BURDIN_TARGET"),
        burdin_codegen::llvm_version(),
        env!("BURDIN_GIT_COMMIT"),
        env!("BURDIN_PROFILE"),
    )
}

struct Args {
    command: String,
    target: PathBuf,
    release: bool,
    /// Emit source-level debug information (DWARF in the object and the executable).
    debug: bool,
    /// Pass the linker's strip option (Unix `-s`; on MSVC, drop the debug directory).
    strip: bool,
    /// Emit LLVM bitcode and link with `-flto`; needs an LLVM compiler driver as BURDIN_CC.
    lto: bool,
    /// Instrument the program for profile-guided optimization.
    pgo_generate: bool,
    /// Use a profile (`*.profraw` or `*.profdata`) for profile-guided optimization.
    pgo_use: Option<PathBuf>,
    output: Option<PathBuf>,
    emit_ir: Option<PathBuf>,
    link_libs: Vec<String>,
    program_args: Vec<String>,
}

fn parse_args(args: Vec<String>) -> Result<Args, String> {
    let mut it = args.into_iter();
    let command = it.next().ok_or("missing command")?;
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
    };
    let mut target = None;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--release" => args.release = true,
            "-g" | "--debug-info" => args.debug = true,
            "--strip" => args.strip = true,
            "--lto" => args.lto = true,
            "--pgo-generate" => args.pgo_generate = true,
            "--pgo-use" => args.pgo_use = Some(it.next().ok_or("`--pgo-use` needs a profile path")?.into()),
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
    if args.pgo_generate && args.pgo_use.is_some() {
        return Err("`--pgo-generate` and `--pgo-use` are different builds; choose one".to_string());
    }
    if args.lto && (args.pgo_generate || args.pgo_use.is_some()) {
        return Err("`--lto` cannot be combined with profile-guided optimization".to_string());
    }
    args.target = target.ok_or("missing target file")?;
    Ok(args)
}

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if matches!(raw.as_slice(), [flag] if flag == "--version" || flag == "-V") {
        println!("{}", version_text());
        return ExitCode::SUCCESS;
    }
    let args = match parse_args(raw) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("error: {msg}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match args.command.as_str() {
        "build" => match compile(&args, Path::new("."), args.output.as_deref(), false) {
            Ok(_) => ExitCode::SUCCESS,
            Err(code) => code,
        },
        "run" => run(&args),
        _ => run_tests(&args),
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

/// `burdin test`: builds the package in test mode and runs each `test` block in its own process, so a panic only
/// fails the test that aborted. The output format is the one `tests/run-tests.sh` reads.
fn run_tests(args: &Args) -> ExitCode {
    let dir = match TempDir::new() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot create a temporary directory: {e}");
            return ExitCode::from(2);
        }
    };
    let compiled = match compile(args, &dir.path, None, true) {
        Ok(compiled) => compiled,
        Err(code) => return code,
    };
    let mut passed = 0usize;
    let mut failed = 0usize;
    for (index, name) in compiled.tests.iter().enumerate() {
        let status = match Command::new(&compiled.exe).arg(index.to_string()).status() {
            Ok(status) => status,
            Err(e) => {
                eprintln!("error: cannot run {}: {e}", compiled.exe.display());
                return ExitCode::from(2);
            }
        };
        if status.success() {
            passed += 1;
        } else {
            failed += 1;
            println!("FAIL {name}");
        }
    }
    println!("test result: {passed} passed, {failed} failed");
    if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) }
}

fn run(args: &Args) -> ExitCode {
    let dir = match TempDir::new() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot create a temporary directory: {e}");
            return ExitCode::from(2);
        }
    };
    let exe = match compile(args, &dir.path, None, false) {
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

/// What a successful compilation produced: the executable and the names of its `test` blocks (empty for a normal
/// build).
struct Compiled {
    exe: PathBuf,
    tests: Vec<String>,
}

/// Compiles `args.target` into an executable: `output` when given, otherwise `<output_dir>/<package name>`. On
/// failure the diagnostics are already printed. With `test_mode`, the package's `test` blocks are compiled instead
/// of its `main`.
fn compile(args: &Args, output_dir: &Path, output: Option<&Path>, test_mode: bool) -> Result<Compiled, ExitCode> {
    let target = &args.target;
    let loaded = match package::load(target) {
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
        eprint!("{}", diags.render(&sources));
        let n = diags.len();
        eprintln!("error: could not compile {} ({n} error{})", target.display(), if n == 1 { "" } else { "s" });
        return Err(ExitCode::from(1));
    }
    let program = program.expect("no errors means checking produced a program");
    let tests: Vec<String> = program.tests.iter().map(|t| t.name.clone()).collect();

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
    let extension = if args.lto {
        "bc"
    } else if cfg!(windows) {
        "obj"
    } else {
        "o"
    };
    let object = dir.path.join(format!("program.{extension}"));
    let pgo_ir = args.pgo_use.as_ref().map(|_| dir.path.join("program.pgo.ll"));
    let opts = burdin_codegen::Options {
        release: args.release,
        emit_ir: args.emit_ir.clone(),
        pgo_ir,
        debug: args.debug,
        bitcode: args.lto,
        instrument: args.pgo_generate,
    };
    burdin_codegen::emit_object(&program, &sources, &loaded.stem, &object, &opts).map_err(fail)?;
    let object = match &args.pgo_use {
        Some(profile) => {
            let ir = dir.path.join("program.pgo.ll");
            pgo_object(&dir.path, &ir, profile, args.release).map_err(fail)?;
            dir.path.join(if cfg!(windows) { "program.pgo.obj" } else { "program.pgo.o" })
        }
        None => object,
    };
    let link_opts = LinkOptions {
        export_dynamic: cfg!(target_os = "linux"),
        strip: args.strip,
        lto: args.lto,
        pgo_generate: args.pgo_generate,
    };
    link(&object, &output, &args.link_libs, &link_opts).map_err(fail)?;
    Ok(Compiled { exe: output, tests })
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
}
