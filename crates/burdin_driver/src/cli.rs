//! The `burdin` command tree, declared with clap: every command, its one-line summary, its options, and its notes.
//!
//! clap renders the help: `burdin` (or `burdin --help`) lists the commands, and `burdin <command> --help` (or
//! `burdin help <command>`) prints everything about one command. clap also rejects an unknown command and suggests
//! the closest one. The options of each command are still read by that command's own parser (`parse_args` in
//! `main.rs`, `check::parse`, ...), because their messages are part of the tested contract (`spec/check.md`, CK8,
//! and the usage-error checks of the `tests/check_*.py` scripts). This tree is the documentation of those parsers:
//! a new option is added to both.

use clap::{Arg, ArgAction, Command};
use std::process::ExitCode;

/// What the command line asks for before any command parses its options.
pub(crate) enum Route {
    /// Help, the version, or an unknown command was handled; exit with this status.
    Exit(ExitCode),
    /// Run this command with the arguments after its name.
    Command(String),
}

/// Decides what `argv` (without the program name) asks for: help, the version, or a command.
pub(crate) fn route(argv: &[String]) -> Route {
    let Some(first) = argv.first().map(String::as_str) else {
        print!("{}", command().render_help());
        return Route::Exit(ExitCode::SUCCESS);
    };
    match first {
        "-V" | "--version" if argv.len() == 1 => {
            println!("{}", crate::version_text());
            Route::Exit(ExitCode::SUCCESS)
        }
        // `burdin --help build` is `burdin help build`.
        "help" | "-h" | "--help" => help(&argv[1..]),
        flag @ ("-V" | "--version") => {
            let error = command().error(
                clap::error::ErrorKind::UnknownArgument,
                format!("`{flag}` takes no arguments, got `{}`", argv[1]),
            );
            eprint!("{}", error.render());
            Route::Exit(ExitCode::from(2))
        }
        name if is_command(name) => {
            // `-h` or `--help` anywhere before `--` asks for the command's help; after `--` it is the program's.
            let options = argv[1..].iter().take_while(|a| *a != "--");
            if options.into_iter().any(|a| a == "-h" || a == "--help") {
                print_help(name);
                return Route::Exit(ExitCode::SUCCESS);
            }
            Route::Command(name.to_string())
        }
        word => Route::Exit(unknown(word)),
    }
}

/// `burdin help [<command>]`.
fn help(rest: &[String]) -> Route {
    match rest {
        [] => {
            print!("{}", command().render_help());
            Route::Exit(ExitCode::SUCCESS)
        }
        [name] if is_command(name) => {
            print_help(name);
            Route::Exit(ExitCode::SUCCESS)
        }
        [name] if name == "help" => help(&[]),
        [name] => Route::Exit(unknown(name)),
        [_, extra, ..] => {
            let error = command()
                .error(clap::error::ErrorKind::UnknownArgument, format!("`help` takes one command, got `{extra}`"));
            eprint!("{}", error.render());
            Route::Exit(ExitCode::from(2))
        }
    }
}

/// Reports a first word that is not a command (or an option clap does not know), with clap's suggestion of the
/// closest command.
fn unknown(word: &str) -> ExitCode {
    let error = match command().try_get_matches_from(["burdin", word]) {
        Err(error) => error,
        Ok(_) => command().error(clap::error::ErrorKind::InvalidSubcommand, format!("unknown command `{word}`")),
    };
    eprint!("{}", error.render());
    ExitCode::from(2)
}

fn is_command(name: &str) -> bool {
    name != "help" && command().get_subcommands().any(|c| c.get_name() == name)
}

/// Whether `flag` (`--name` or `-n`) of command `name` takes a value; unknown flags take none.
pub(crate) fn option_takes_value(name: &str, flag: &str) -> bool {
    let command = subcommand(name);
    let arg = if let Some(long) = flag.strip_prefix("--") {
        command.get_arguments().find(|a| a.get_long() == Some(long))
    } else {
        let mut chars = flag.chars().skip(1);
        match (chars.next(), chars.next()) {
            (Some(short), None) => command.get_arguments().find(|a| a.get_short() == Some(short)),
            _ => None,
        }
    };
    arg.is_some_and(|a| a.get_action().takes_values())
}

/// The built subcommand `name` of the tree: its usage and help name it `burdin <name>`.
fn subcommand(name: &str) -> Command {
    let mut root = command();
    root.build();
    root.find_subcommand(name).cloned().unwrap_or_else(|| panic!("`{name}` is not a command"))
}

/// Prints everything about one command: its description, usage, arguments, options, and notes.
pub(crate) fn print_help(name: &str) {
    print!("{}", subcommand(name).render_help());
}

/// A usage error of command `name`: the message, the command's usage, and where its help is. Exit status 2.
pub(crate) fn usage_error(name: &str, message: &str) -> ExitCode {
    eprintln!("{}", usage_error_text(name, message));
    ExitCode::from(2)
}

fn usage_error_text(name: &str, message: &str) -> String {
    let usage = subcommand(name).render_usage().to_string();
    let usage = usage.trim_end().strip_prefix("Usage:").map(|rest| format!("usage:{rest}")).unwrap_or(usage);
    format!("error: {message}\n\n{usage}\n\nFor more information, try `burdin {name} --help`.")
}

const ROOT_AFTER_HELP: &str = "See `burdin help <command>` for more information on a specific command.";

const ENVIRONMENT: &str = "\
Environment:
  BURDIN_CC             The C compiler driver used to link against libc (default: cc; cl on Windows)
  BURDIN_JOBS           The default of -j (default: the machine's parallelism; 1 is sequential)
  BURDIN_HOME           Where tools keep state, including the git dependency cache under git/
  BURDIN_CODEGEN_UNITS  Split code generation into exactly this many units (default: by program size)
  LLVM_SYS_211_PREFIX   The LLVM 21 installation; its bin/llvm-config resolves `--link-lib LLVM-C`
  COLUMNS               The terminal width used when --width is not given
  FORCE_HYPERLINK       1 links error codes to the error reference in colored output, 0 never does
  NO_COLOR              Disables color unless --color says otherwise";

const DIAGNOSTICS: &str = "Diagnostic Options";
const COMPILATION: &str = "Compilation Options";
const SELECTION: &str = "Selection Options";

/// The whole command tree.
pub(crate) fn command() -> Command {
    Command::new("burdin")
        .about("The Burdin compiler and toolchain")
        .override_usage("burdin [OPTIONS] [COMMAND]")
        .disable_version_flag(true)
        .disable_help_subcommand(true)
        .arg(
            Arg::new("version")
                .short('V')
                .long("version")
                .action(ArgAction::SetTrue)
                .help("Print version info and exit"),
        )
        .after_help(ROOT_AFTER_HELP)
        .subcommands([
            build(),
            run(),
            test(),
            bench(),
            fuzz(),
            check(),
            fmt(),
            lint(),
            fix(),
            ssr(),
            refactor(),
            doc(),
            new(),
            add(),
            repl(),
            watch(),
            debug(),
            expand(),
            asm(),
            bloat(),
            explain(),
            bindgen(),
            lsp(),
            dap(),
            Command::new("help").about("Print the help of a command").override_usage("burdin help [<command>]"),
        ])
}

// --- Shared arguments ---------------------------------------------------------------------------------------------

fn target() -> Arg {
    Arg::new("target").value_name("file.bd | package directory").help("The source file or package directory")
}

fn flag(id: &'static str, help: &'static str) -> Arg {
    Arg::new(id).long(id).action(ArgAction::SetTrue).help(help)
}

fn option(id: &'static str, value: &'static str, help: &'static str) -> Arg {
    Arg::new(id).long(id).value_name(value).help(help)
}

/// The options of `build`, `run`, `test`, and `check` that rerun the command when a file changes.
fn watch_options() -> [Arg; 2] {
    [
        flag("watch", "Rerun the command whenever a source, the manifest, or a build input changes (spec/watch.md)")
            .help_heading("Watch Options"),
        flag("no-clear", "With --watch: do not clear the screen before each run").help_heading("Watch Options"),
    ]
}

fn jobs(help: &'static str) -> Arg {
    Arg::new("jobs").short('j').long("jobs").value_name("n").help(help)
}

fn compile_jobs() -> Arg {
    jobs("Compile on at most n threads (default: BURDIN_JOBS, then the number of CPUs)")
}

fn comptime_steps() -> Arg {
    option("comptime-steps", "n", "The step budget of each compile-time evaluation (default: 10000000)")
}

fn release(help: &'static str) -> Arg {
    flag("release", help)
}

fn debug_info() -> Arg {
    Arg::new("debug-info")
        .short('g')
        .long("debug-info")
        .action(ArgAction::SetTrue)
        .help("Emit source-level debug information")
}

fn locked() -> Arg {
    flag("locked", "Fail instead of writing burdin.lock when it does not match the manifests (spec/packages.md)")
}

fn link_lib() -> Arg {
    option("link-lib", "name", "Link against a native library; may repeat").action(ArgAction::Append)
}

fn sanitize() -> Arg {
    option(
        "sanitize",
        "list",
        "Build with sanitizers: address, undefined, thread, leak, comma-separated (spec/sanitizers.md)",
    )
}

/// The diagnostics options every compiling command takes, under their own heading.
fn diagnostics() -> [Arg; 5] {
    [
        option(
            "diagnostic-format",
            "human|short|json|sarif",
            "How to print diagnostics (default: human; docs/diagnostics.md)",
        ),
        option("color", "auto|always|never", "When to color diagnostics (default auto: when stderr is a terminal)"),
        flag("no-color", "The same as --color never"),
        option("width", "columns", "Wrap human diagnostics at this width (0 disables wrapping)"),
        option("error-limit", "n", "Print at most n errors after sorting (0 prints all)"),
    ]
    .map(|arg| arg.help_heading(DIAGNOSTICS))
}

fn with_diagnostics(command: Command) -> Command {
    command.args(diagnostics())
}

// --- Commands -----------------------------------------------------------------------------------------------------

fn build() -> Command {
    with_diagnostics(
        Command::new("build")
            .about("Compile a file or package into an executable")
            .override_usage("burdin build <file.bd | package directory> [OPTIONS]")
            .arg(target().required(true))
            .arg(Arg::new("output").short('o').value_name("output").help("Where to write the executable"))
            .args([
                release("Build with optimizations"),
                debug_info(),
                locked(),
                flag("strip", "Strip the executable (with MSVC, optimize for size)"),
                flag("lto", "Emit bitcode and link with -flto (needs an LLVM compiler driver)"),
                flag("pgo-generate", "Instrument the program for profile-guided optimization"),
                option("pgo-use", "profile", "Optimize with this profile (*.profraw or *.profdata)"),
                flag(
                    "tests",
                    "Compile the package's `test` blocks and their dispatcher instead of `main`, without running them",
                ),
                link_lib(),
                sanitize(),
                compile_jobs(),
                comptime_steps(),
            ])
            .args(
                [
                    option(
                        "emit",
                        "kinds",
                        "Also write these artifacts next to the output, comma-separated: tir, llvm-ir, llvm-ir-opt, \
                         llvm-bc, asm, obj, deps (spec/inspect.md)",
                    ),
                    flag("intel", "With --emit asm: the Intel x86 dialect"),
                    flag("att", "With --emit asm: the AT&T x86 dialect (the default on x86)"),
                    option("emit-ir", "file.ll", "Write the generated LLVM IR to this file"),
                    option("emit-deps", "file", "Write the files the build read besides the sources (`embed`)"),
                ]
                .map(|arg| arg.help_heading("Output Options")),
            )
            .args(watch_options())
            .after_help(ENVIRONMENT),
    )
}

fn run() -> Command {
    with_diagnostics(
        Command::new("run")
            .about("Build a program and run it")
            .override_usage("burdin run <file.bd | package directory> [OPTIONS] [-- <program arguments>...]")
            .arg(target().required(true))
            .arg(
                Arg::new("program-args")
                    .value_name("program arguments")
                    .num_args(0..)
                    .last(true)
                    .help("Arguments passed to the program"),
            )
            .args([
                release("Build with optimizations"),
                debug_info(),
                locked(),
                link_lib(),
                sanitize(),
                compile_jobs(),
                comptime_steps(),
            ])
            .args(watch_options())
            .after_help(
                "A single file that starts with a shebang or an inline manifest runs as a script, from a cache \
                 (spec/scripts.md). The exit status is the program's.\n\n"
                    .to_string()
                    + ENVIRONMENT,
            ),
    )
}

/// The selection options `test` and `bench` share.
fn selection(noun: &'static str) -> [Arg; 5] {
    let (packages, workspace, skip) = match noun {
        "tests" => (
            "Run the tests of one package of the build; may repeat",
            "Run the tests of the entry package and every path dependency",
            "Leave out tests whose full path contains this pattern; may repeat",
        ),
        _ => (
            "Measure the benches of one package of the build; may repeat",
            "Measure the entry package and every path dependency",
            "Leave out benches whose full path contains this pattern; may repeat",
        ),
    };
    [
        Arg::new("package").short('p').value_name("package").action(ArgAction::Append).help(packages),
        flag("workspace", workspace),
        option("filter", "text", "The same as a positional filter").action(ArgAction::Append),
        flag("exact", "Filters and --skip patterns must equal the full path instead of being contained in it"),
        option("skip", "pattern", skip).action(ArgAction::Append),
    ]
    .map(|arg| arg.help_heading(SELECTION))
}

fn filters(help: &'static str) -> Arg {
    Arg::new("filters").value_name("filter").num_args(0..).help(help)
}

fn test() -> Command {
    with_diagnostics(
        Command::new("test")
            .about("Build and run the `test` blocks of a file or package")
            .override_usage("burdin test <file.bd | package directory> [<filter>...] [OPTIONS]")
            .arg(target().required(true))
            .arg(filters("Run only tests whose full path (`module.test name`) contains any filter"))
            .args(selection("tests"))
            .args(
                [
                    flag("ignored", "Run only tests marked [ignore]"),
                    flag("include-ignored", "Run ignored tests too"),
                    flag("failed", "Run only the tests that failed in the previous run"),
                    option("shard", "i/n", "Run the i-th of n deterministic slices of the selected tests"),
                    flag("doc", "Run only the doc tests: the Burdin code blocks of doc comments"),
                    flag("fuzz-replay", "Also run each fuzz target on every input saved under fuzz/artifacts/<name>/"),
                    flag(
                        "interpret",
                        "Run the tests in the interpreter, with strict memory checks, instead of compiling them",
                    ),
                ]
                .map(|arg| arg.help_heading(SELECTION)),
            )
            .args([
                flag("list", "Print the selected tests and their attributes without running them"),
                option("format", "human|json", "A report (default: human) or one JSON event per line"),
                option("junit", "file", "Also write a JUnit XML report of the run"),
                option("timeout", "ms", "The per-test timeout (default: 60000; 0 disables it)"),
                option("slow-threshold", "ms", "List tests that ran at least this long (default: 5000; 0 disables it)"),
                Arg::new("verbose")
                    .short('v')
                    .long("verbose")
                    .action(ArgAction::SetTrue)
                    .help("Print the output of passing tests too"),
            ])
            .args(
                [
                    flag("bless", "Accept every new or changed snapshot, rewriting files and inline snapshots"),
                    flag("review", "Show each new or changed snapshot and ask whether to accept it"),
                ]
                .map(|arg| arg.help_heading("Snapshot Options")),
            )
            .args(
                [
                    release("Build the tests with optimizations"),
                    debug_info(),
                    locked(),
                    sanitize(),
                    jobs("Run at most n tests at once, and compile on at most n threads (default: the number of CPUs)"),
                    comptime_steps(),
                ]
                .map(|arg| arg.help_heading(COMPILATION)),
            )
            .args(watch_options())
            .after_help("The behavior is spec/test-runner.md.\n\n".to_string() + ENVIRONMENT),
    )
}

fn fuzz() -> Command {
    with_diagnostics(
        Command::new("fuzz")
            .about("Build a `fuzz` target with libFuzzer and AddressSanitizer, and fuzz it")
            .override_usage("burdin fuzz <file.bd | package directory> <name> [OPTIONS] [-- <libFuzzer options>...]")
            .arg(target().required(true))
            .arg(Arg::new("name").value_name("name").required(true).help("The fuzz target to run"))
            .args([
                option("max-time", "seconds", "Stop after this many seconds without a crash (default: no limit)"),
                option(
                    "minimize",
                    "file",
                    "Shrink this crashing input instead of fuzzing; the result is saved beside the crashes",
                ),
                Arg::new("libfuzzer-args")
                    .value_name("libFuzzer options")
                    .num_args(0..)
                    .last(true)
                    .help("Options passed to libFuzzer, such as -jobs=4 or -dict=file"),
            ])
            .args(
                [
                    release("Build the target with optimizations"),
                    debug_info(),
                    locked(),
                    compile_jobs(),
                    comptime_steps(),
                ]
                .map(|arg| arg.help_heading(COMPILATION)),
            )
            .after_help(
                "The corpus is fuzz/corpus/<name>/ and crashes are saved to fuzz/artifacts/<name>/, next to the \
                 package's burdin.toml (or the file). `burdin test --fuzz-replay` runs the saved crashes. Linux and \
                 macOS only. The behavior is spec/fuzz.md.\n\n"
                    .to_string()
                    + ENVIRONMENT,
            ),
    )
}

fn bench() -> Command {
    with_diagnostics(
        Command::new("bench")
            .about("Build and measure the `bench` blocks of a file or package")
            .override_usage("burdin bench <file.bd | package directory> [<filter>...] [OPTIONS]")
            .arg(target().required(true))
            .arg(filters("Measure only benches whose full path contains any filter"))
            .args(selection("benches"))
            .args([
                flag("list", "Print the selected benches without measuring them"),
                option("format", "human|json", "A report (default: human) or one JSON event per line"),
                option("timeout", "ms", "The per-bench timeout (default: 600000; 0 disables it)"),
                option("warmup", "ms", "How long the warmup of each bench runs (default: 100)"),
                option("sample-count", "n", "How many samples each bench takes (default: 20)"),
                option("sample-time", "ms", "The target duration of one sample (default: 5)"),
                Arg::new("verbose")
                    .short('v')
                    .long("verbose")
                    .action(ArgAction::SetTrue)
                    .help("Print the captured output of passing benches too"),
            ])
            .args(
                [
                    option("save-baseline", "name", "Store the measurements under target/burdin/bench/<name>.json"),
                    option("baseline", "name", "Compare each measurement with the named baseline"),
                    option(
                        "fail-on-regression",
                        "percent",
                        "With --baseline, fail when a bench is this much slower, beyond noise",
                    ),
                    option(
                        "profile-time",
                        "seconds",
                        "Run one selected bench in a loop for this long without measuring, for a profiler",
                    ),
                ]
                .map(|arg| arg.help_heading("Baseline Options")),
            )
            .args(
                [
                    release("Accepted for symmetry: benches always build with optimizations"),
                    debug_info(),
                    locked(),
                    jobs("Compile on at most n threads; benches are measured one at a time"),
                    comptime_steps(),
                ]
                .map(|arg| arg.help_heading(COMPILATION)),
            )
            .after_help("The behavior is spec/bench.md.\n\n".to_string() + ENVIRONMENT),
    )
}

fn check() -> Command {
    with_diagnostics(
        Command::new("check")
            .about("Type-check a program and run the memory checks, without generating code")
            .override_usage("burdin check <file.bd | package directory> [OPTIONS]")
            .arg(target().required(true))
            .args([
                release("Check the way `burdin build --release` does"),
                flag("tests", "Also check the `test` blocks, and do not require a `main` (a library package)"),
                flag("print-instances", "Print every generic instance the program created and why (spec/inspect.md)"),
                locked(),
                option(
                    "format",
                    "fmt",
                    "With --print-instances: human (default) or json; with --explain-ownership: text (default) or json",
                ),
                jobs("Parse on at most n threads (default: BURDIN_JOBS, then the number of CPUs)"),
                comptime_steps(),
            ])
            .args(
                [
                    flag(
                        "explain-ownership",
                        "Print each function's source annotated with its moves, copies, borrows, drops, drop flags, \
                         argument conventions, and derived clones",
                    ),
                    option("fn", "name", "With --explain-ownership: only this function (or qualified name)"),
                    flag("explain", "Print the path that leads to each memory error, its rule ID, and the fix"),
                ]
                .map(|arg| arg.help_heading("Ownership Options")),
            )
            .args(watch_options())
            .after_help(
                "Nothing is printed when the program has no errors and no warnings. The exit status is 0 without \
                 errors, 1 with errors, and 2 for a usage error. With --print-instances the report goes to standard \
                 output. The behavior is spec/check.md.",
            ),
    )
}

fn fmt() -> Command {
    with_diagnostics(
        Command::new("fmt")
            .about("Format source files")
            .override_usage(
                "burdin fmt [<file.bd | directory>]... [OPTIONS]\n       \
                 burdin fmt --stdin [--stdin-filepath <path>] [--range <lo>:<hi>] [OPTIONS]",
            )
            .arg(
                Arg::new("paths")
                    .value_name("file.bd | directory")
                    .num_args(0..)
                    .help("The files and directories to format (default: the package in the current directory)"),
            )
            .args([
                flag("check", "Change nothing; list the files that are not formatted and exit with status 1"),
                flag("stdin", "Format standard input to standard output"),
                option(
                    "stdin-filepath",
                    "path",
                    "The path standard input stands for: names it in diagnostics and finds its package",
                ),
                option("range", "lo>:<hi", "With --stdin, format only the items that overlap these byte offsets"),
                jobs("Format at most n files at once (default: BURDIN_JOBS, then the number of CPUs)"),
            ])
            .after_help("There is one style and nothing to configure. The behavior is spec/fmt.md."),
    )
}

fn lint() -> Command {
    with_diagnostics(
        Command::new("lint")
            .about("Report common mistakes and style issues, and fix them")
            .override_usage(
                "burdin lint <file.bd | package directory> [OPTIONS]\n       \
                 burdin lint --list\n       \
                 burdin lint --explain <lint>",
            )
            .arg(target())
            .args(
                [
                    Arg::new("allow")
                        .short('A')
                        .long("allow")
                        .value_name("lint")
                        .action(ArgAction::Append)
                        .help("Do not run the lint (`all` names every lint)"),
                    Arg::new("warn")
                        .short('W')
                        .long("warn")
                        .value_name("lint")
                        .action(ArgAction::Append)
                        .help("Report the lint as a warning"),
                    Arg::new("deny")
                        .short('D')
                        .long("deny")
                        .value_name("lint")
                        .action(ArgAction::Append)
                        .help("Report the lint as an error (`warnings` raises every lint that warns)"),
                    flag("deny-warnings", "The same as -D warnings"),
                ]
                .map(|arg| arg.help_heading("Level Options")),
            )
            .args([
                flag("fix", "Apply the safe fixes, check that the package still compiles, and write the files"),
                flag("dry-run", "With --fix: print the diff and write nothing"),
                flag("list", "Print every lint with its code and default level"),
                option("explain", "lint", "Print the reference entry for a lint (a name or a code)"),
                jobs("Parse on at most n threads (default: BURDIN_JOBS, then the number of CPUs)"),
                comptime_steps(),
            ])
            .after_help(
                "Levels also come from the [lint] table of burdin.toml (name = \"allow\" | \"warn\" | \"deny\"); \
                 the command line wins. A comment `// lint: allow(name)` suppresses a lint at one item, statement, \
                 or line. The lint reference is docs/lints.md.",
            ),
    )
}

fn fix() -> Command {
    with_diagnostics(
        Command::new("fix")
            .about("Apply the machine-applicable fixes of the compiler and the linter across a package")
            .override_usage("burdin fix <file.bd | package directory> [OPTIONS]")
            .arg(target().required(true))
            .args([
                flag("dry-run", "Print the changes as a unified diff and write nothing"),
                flag("allow-dirty", "Write even when the version-control working tree has uncommitted changes"),
                flag("migrate", "Also move the package to the next edition, after checking it compiles there"),
                jobs("Parse on at most n threads (default: BURDIN_JOBS, then the number of CPUs)"),
                comptime_steps(),
            ])
            .after_help(
                "Fixes apply round after round until nothing changes, conflicting fixes are skipped and reported,                  and a round that would add errors is undone. The touched files are formatted. The behavior is                  spec/fix.md.",
            ),
    )
}

fn ssr() -> Command {
    with_diagnostics(
        Command::new("ssr")
            .about("Structural search and replace: match a pattern written as Burdin code across a package")
            .override_usage("burdin ssr '<pattern> [==>> <replacement>]' <file.bd | package directory> [OPTIONS]")
            .arg(Arg::new("pattern").value_name("pattern").required(true).help(
                "The pattern, and after `==>>` what to put in its place. `$name` is any expression; a type may be \
                 constrained after `where`: `$a.unwrap_or($b) where $a: Option<_>`. Without a replacement the matches \
                 are listed",
            ))
            .arg(target().required(true))
            .args([
                flag("dry-run", "Print the changes as a unified diff and write nothing"),
                flag("allow-dirty", "Write even when the version-control working tree has uncommitted changes"),
                jobs("Parse on at most n threads (default: BURDIN_JOBS, then the number of CPUs)"),
            ])
            .after_help(
                "A match is a whole expression, so nothing inside a string or a comment changes. Names match by what \
                 they resolve to, so an import or an alias does not hide a match. The result is checked before it is \
                 written: a replacement that would not compile is skipped and reported. The behavior is \
                 spec/refactor.md.",
            ),
    )
}

fn refactor() -> Command {
    with_diagnostics(
        Command::new("refactor")
            .about("Refactor code: extract, inline, change a signature, or move an item")
            .override_usage(
                "burdin refactor <refactoring> <file>:<line>:<col>[-<line>:<col>] [OPTIONS]
       \
                 burdin refactor extract-variable <range> --name <name>
       \
                 burdin refactor extract-function <range> --name <name>
       \
                 burdin refactor inline-variable <file>:<line>:<col>
       \
                 burdin refactor inline-function <file>:<line>:<col>
       \
                 burdin refactor change-signature <file>:<line>:<col> [--reorder ...] [--add ...] [--remove ...] \
                 [--rename ...]
       \
                 burdin refactor move-item <file>:<line>:<col> --to <module>",
            )
            .arg(Arg::new("refactoring").value_name("refactoring").required(true).help(
                "extract-variable, extract-function, inline-variable, inline-function, change-signature, or move-item",
            ))
            .arg(target().required(true))
            .args([
                option("name", "name", "The name of the local or function an extraction creates"),
                option("to", "module", "The module a moved item goes to (`move-item`)"),
                option("reorder", "a,b", "The parameters of a changed signature, in their new order"),
                option("add", "name: type = default", "A parameter to add, with a default value"),
                option("remove", "name", "A parameter to remove"),
                option("rename", "old=new", "A parameter to rename"),
                flag("dry-run", "Print the changes as a unified diff and write nothing"),
                flag("allow-dirty", "Write even when the version-control working tree has uncommitted changes"),
                jobs("Parse on at most n threads (default: BURDIN_JOBS, then the number of CPUs)"),
            ])
            .after_help(
                "Every refactoring checks its result before it is written: the package must still compile, with no \
                 new error, and every local must still be moved exactly as often. When the result would change what \
                 the program does, the refactoring refuses and says why. The behavior is spec/refactor.md.",
            ),
    )
}

fn doc() -> Command {
    with_diagnostics(
        Command::new("doc")
            .about("Generate the documentation of a package")
            .override_usage("burdin doc <file.bd | package directory> [OPTIONS]")
            .arg(target().required(true))
            .args([
                Arg::new("out").short('o').long("out").value_name("dir | file").help(
                    "Where to write: a directory for html (default: <package>/target/doc), a file for json \
                     (default: standard output)",
                ),
                option("output-format", "html|json", "A static site (default: html) or the documented API as JSON"),
                flag("private", "Document `priv` items, fields, and methods too"),
                flag("coverage", "Print how much of the public interface is documented, per module"),
                flag("deny-warnings", "Exit with status 1 when a doc comment has a broken link"),
                jobs("Parse on at most n threads (default: BURDIN_JOBS, then the number of CPUs)"),
                comptime_steps(),
            ])
            .after_help("The behavior is spec/doc.md."),
    )
}

fn new() -> Command {
    Command::new("new")
        .about("Create a new package")
        .override_usage("burdin new <name>")
        .arg(Arg::new("name").value_name("name").required(true).help("The package directory to create"))
        .after_help("Creates <name>/burdin.toml, <name>/src/main.bd, and <name>/.gitignore.")
}

fn add() -> Command {
    Command::new("add")
        .about("Add a path or git dependency to burdin.toml")
        .override_usage(
            "burdin add [<name>] --path <dir> [--manifest-path <path>]\n       \
             burdin add [<name>] --git <url> [--branch <branch> | --tag <tag> | --rev <commit>] [--manifest-path <path>]",
        )
        .arg(
            Arg::new("name")
                .value_name("name")
                .help("The name `use` reaches the dependency by (default: the dependency's package name)"),
        )
        .args([
            option("path", "dir", "A package directory, relative to the working directory"),
            option("git", "url", "A git repository whose root is a package"),
            option("branch", "branch", "With --git: the branch to pin (default: the repository's default branch)"),
            option("tag", "tag", "With --git: the tag to pin"),
            option("rev", "commit", "With --git: the commit to pin"),
            option(
                "manifest-path",
                "path",
                "The burdin.toml to edit, or its directory (default: the nearest one at or above the working directory)",
            ),
        ])
        .after_help(
            "The dependency and the whole graph are resolved and burdin.lock is written; a git dependency is pinned to \
             the commit its branch, tag, or rev names now. On failure neither file changes. The behavior is \
             spec/packages.md.",
        )
}

fn repl() -> Command {
    with_diagnostics(
        Command::new("repl")
            .about("Start an interactive session")
            .override_usage("burdin repl [OPTIONS]")
            .args([release("Evaluate the way a release build does"), compile_jobs(), comptime_steps()])
            .after_help(
                "The session is one growing program: inputs are items, statements, or expressions, each checked \
                 by the real front end and evaluated by the comptime interpreter; an expression's value prints with \
                 Debug. The meta-commands (:help lists them) include :type, :ownership, :explain, :load, :ir, \
                 :compile, and :reset. Standard input that is not a terminal disables interactive editing, so a \
                 scripted session is deterministic. The behavior is spec/repl.md.",
            ),
    )
}

fn debug() -> Command {
    with_diagnostics(
        Command::new("debug")
            .about("Build a program with debug info and start a debugger on it")
            .override_usage("burdin debug <file.bd | package directory> [OPTIONS] [-- <program arguments>...]")
            .arg(target().required(true))
            .arg(
                Arg::new("program-args")
                    .value_name("program arguments")
                    .num_args(0..)
                    .last(true)
                    .help("Arguments passed to the program"),
            )
            .args([
                option("debugger", "name | path", "gdb, lldb, cdb, or a program to start instead"),
                option("test", "name | index", "Debug one `test` block, resolved like `burdin test`'s filters"),
                flag("break-on-panic", "Start with a breakpoint on the panic hook, before the panic prints"),
                release("Debug an optimized build"),
                compile_jobs(),
            ])
            .after_help(
                "The target builds with -g in a temporary directory and starts under the platform debugger (lldb on \
                 macOS, gdb or lldb on Linux, cdb on Windows) with the Burdin pretty-printers loaded. The behavior \
                 is spec/debug.md.",
            ),
    )
}

fn expand() -> Command {
    with_diagnostics(
        Command::new("expand")
            .about("Print the source after comptime, derives, default arguments, and operators")
            .override_usage("burdin expand <file.bd | package directory> [OPTIONS]")
            .arg(target().required(true))
            .args([
                option(
                    "item",
                    "path",
                    "Print only this function, type, constant, or method (`area`, `Circle.area`, `shapes.area`)",
                ),
                flag("instances", "After the source, print each monomorphized instance of a generic function"),
                flag("no-format", "Print the expansion as built, without running the formatter over it"),
                compile_jobs(),
                comptime_steps(),
            ])
            .after_help(
                "The output is Burdin source: `burdin check` accepts it wherever it accepted the input. The behavior \
                 is spec/inspect.md.",
            ),
    )
}

fn asm() -> Command {
    with_diagnostics(
        Command::new("asm")
            .about("Print the assembly or LLVM IR of one function, with its source")
            .override_usage("burdin asm <file.bd | package directory> [<function>] [OPTIONS]")
            .arg(target().required(true))
            .arg(Arg::new("function").value_name("function").help(
                "`module.name`, `Type.method`, or an instance such as `largest<i32>` (`::` is accepted for `.`); \
                 a generic function without type arguments lists its instances, and no function lists every \
                 function that has code",
            ))
            .args([
                flag("debug", "Build the debug profile (the default is release)"),
                flag("intel", "The Intel x86 dialect"),
                flag("att", "The AT&T x86 dialect (the default on x86)"),
                flag("llvm", "Print the function's optimized LLVM IR instead of assembly"),
                flag("no-source", "Leave out the interleaved source lines"),
                compile_jobs(),
                comptime_steps(),
            ])
            .after_help("The behavior is spec/inspect.md."),
    )
}

fn bloat() -> Command {
    with_diagnostics(
        Command::new("bloat")
            .about("Report the size of each function, module, package, and generic in the executable")
            .override_usage("burdin bloat <file.bd | package directory> [OPTIONS]")
            .arg(target().required(true))
            .args([
                flag("debug", "Measure the debug profile (the default is release)"),
                option("top", "n", "List the n largest functions and generics (default: 20)"),
                option("format", "human|json", "A table (default) or one JSON object"),
                compile_jobs(),
                comptime_steps(),
            ])
            .after_help("The behavior is spec/inspect.md."),
    )
}

fn explain() -> Command {
    Command::new("explain")
        .about("Print the reference entry for an error code")
        .override_usage("burdin explain [<error code>]")
        .arg(
            Arg::new("code")
                .value_name("error code")
                .help("The code to explain, for example E0003; without it, every code is listed"),
        )
}

fn bindgen() -> Command {
    with_diagnostics(
        Command::new("bindgen")
            .about("Generate bindings: a module from a C header or Rust crate, or a C header for a package")
            .override_usage(
                "burdin bindgen <header> --out <file> [OPTIONS]\n       \
                 burdin bindgen --export <target> --out <header.h> [--guard <NAME>]\n       \
                 burdin bindgen --rust <crate> --out <file> --shim <dir> [OPTIONS]",
            )
            .arg(Arg::new("header").value_name("header").help("The C header to bind"))
            .arg(option("out", "file", "Where to write the module (or, with --export, the header)"))
            .args(
                [
                    Arg::new("include")
                        .short('I')
                        .value_name("dir")
                        .action(ArgAction::Append)
                        .help("Add an include directory; may repeat"),
                    Arg::new("define")
                        .short('D')
                        .value_name("name[=value]")
                        .action(ArgAction::Append)
                        .help("Define a preprocessor macro; may repeat"),
                    option("clang-arg", "arg", "Pass an argument to clang; may repeat").action(ArgAction::Append),
                    option("wrappers", "file.c", "Write C wrappers for the functions that need them to this file"),
                    option("wrapper-include", "text", "The #include line the wrapper file starts with"),
                ]
                .map(|arg| arg.help_heading("C Header Options")),
            )
            .args(
                [
                    option("export", "target", "Write a C header for a package's exported functions and types"),
                    option("guard", "NAME", "With --export: the include guard of the header"),
                ]
                .map(|arg| arg.help_heading("Export Options")),
            )
            .args(
                [
                    option("rust", "crate", "Bind a Rust crate (a directory or its Cargo.toml)"),
                    option("shim", "dir", "With --rust: where the C-ABI shim crate is written"),
                    option("rust-toolchain", "version", "With --rust: the exact toolchain, such as 1.100.0"),
                    option("rustdoc-json", "file", "With --rust: read this rustdoc JSON instead of running rustdoc"),
                ]
                .map(|arg| arg.help_heading("Rust Options")),
            )
            .after_help("The behavior is spec/bindgen.md and spec/rust-interop.md."),
    )
}

fn watch() -> Command {
    Command::new("watch")
        .about("Rerun check, build, test, or run whenever the sources change")
        .override_usage("burdin watch [check|build|test|run] <file.bd | package directory> [OPTIONS]")
        .arg(
            Arg::new("args")
                .value_name("command and options")
                .num_args(0..)
                .allow_hyphen_values(true)
                .trailing_var_arg(true)
                .help("The command (default: check), its target, and its options, as for the command itself"),
        )
        .arg(flag("no-clear", "Do not clear the screen before each run"))
        .after_help(
            "Watches the package's sources, burdin.toml, path dependencies, and the files build scripts read, \
             ignoring the target directory and version-control metadata. `run` restarts the program; `test` runs \
             the tests of changed modules and their dependents first. The behavior is spec/watch.md.",
        )
}

fn lsp() -> Command {
    Command::new("lsp")
        .about("Run the language server on standard input and output")
        .override_usage("burdin lsp [--stdio]")
        .arg(flag("stdio", "Accepted for editors; standard input and output is the only transport"))
        .after_help("The behavior is spec/lsp.md.")
}

fn dap() -> Command {
    Command::new("dap")
        .about("Run the debug adapter on standard input and output")
        .override_usage("burdin dap [--stdio]")
        .arg(flag("stdio", "Accepted for editors; standard input and output is the only transport"))
        .after_help("The adapter forwards to lldb-dap. The behavior is spec/debug.md.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_is_valid() {
        command().debug_assert();
    }

    #[test]
    fn every_command_has_a_summary_and_a_usage() {
        let mut root = command();
        root.build();
        for sub in root.get_subcommands() {
            assert!(sub.get_about().is_some(), "`{}` has no summary", sub.get_name());
            let usage = sub.clone().render_usage().to_string();
            assert!(usage.contains(&format!("burdin {}", sub.get_name())), "`{}` usage: {usage}", sub.get_name());
        }
    }

    #[test]
    fn the_command_list_names_every_command() {
        let help = command().render_help().to_string();
        for name in
            ["build", "run", "test", "bench", "check", "fmt", "lint", "doc", "new", "add", "explain", "watch", "lsp"]
        {
            assert!(help.contains(&format!("  {name} ")), "`{name}` is missing from:\n{help}");
        }
        // The list stays short: the options of a command are only in its own help.
        assert!(!help.contains("--release"), "the command list shows options:\n{help}");
    }

    #[test]
    fn a_usage_error_names_the_command_and_its_help() {
        let text = usage_error_text("check", "unknown option `--bogus`");
        assert!(text.starts_with("error: unknown option `--bogus`\n\nusage: burdin check <file.bd"), "{text}");
        assert!(text.ends_with("try `burdin check --help`."), "{text}");
    }

    #[test]
    fn help_after_the_separator_belongs_to_the_program() {
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(matches!(route(&words(&["run", "a.bd", "--", "--help"])), Route::Command(name) if name == "run"));
        assert!(is_command("check"));
        assert!(!is_command("help"));
        assert!(!is_command("chek"));
    }
}
