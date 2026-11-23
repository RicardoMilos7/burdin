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
        "help" | "h" | "-h" | "--help" => help(&argv[1..]),
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
            Route::Command(canonical(name))
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
    name != "help"
        && command().get_subcommands().any(|c| c.get_name() == name || c.get_all_aliases().any(|a| a == name))
}

/// The name of the command that `name` is, whether it is the name or an alias: `r` is `run`.
pub(crate) fn canonical(name: &str) -> String {
    command()
        .get_subcommands()
        .find(|c| c.get_name() == name || c.get_all_aliases().any(|a| a == name))
        .map_or_else(|| name.to_string(), |c| c.get_name().to_string())
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

/// Commands whose first positional argument is the target. Left out, the target is the working directory.
const TARGET_FIRST: &[&str] = &["build", "run", "check", "lint", "fix", "doc", "debug", "expand", "bloat"];

/// Commands whose first positional argument is a target or the name that follows it: a test or bench filter, an
/// assembly function, a fuzz target's name. The word is the target when it names a path; otherwise it is the name.
const TARGET_OR_NAME: &[&str] = &["test", "bench", "asm", "fuzz"];

/// The commands `burdin watch` runs, the first word of its arguments when one is given.
const WATCHED: &[&str] = &["check", "build", "test", "run"];

/// `argv` (the command's name first, canonical) with the default target added where the command line leaves the
/// target out: `burdin run` is `burdin run .`, so a package runs from its own directory (`spec/build-tool.md`).
pub(crate) fn with_default_target(mut argv: Vec<String>) -> Vec<String> {
    let Some(name) = argv.first().cloned() else { return argv };
    if let Some(index) = default_target_index(&name, &argv[1..]) {
        argv.insert(index + 1, ".".to_string());
    }
    argv
}

/// Where the default target goes among `rest` (the arguments after the command's name): the index it is inserted
/// before, or `None` when the command line has its target already.
fn default_target_index(name: &str, rest: &[String]) -> Option<usize> {
    // `burdin doc --std` documents the standard library the compiler carries and takes no target (`spec/doc.md`, DC24).
    if name == "doc" && rest.iter().any(|a| a == "--std") {
        return None;
    }
    let positions = positionals(name, rest);
    if TARGET_FIRST.contains(&name) {
        return positions.is_empty().then_some(0);
    }
    if TARGET_OR_NAME.contains(&name) {
        return match positions.first() {
            None => Some(0),
            Some(&first) if is_path(&rest[first]) => None,
            Some(&first) => Some(first),
        };
    }
    match name {
        // `burdin ssr <pattern> <target>` and `burdin refactor <refactoring> <target>`: one word is the first.
        "ssr" | "refactor" => match positions.as_slice() {
            [_] => Some(positions[0] + 1),
            _ => None,
        },
        // `burdin watch [check|build|test|run] <target> [options]`: the options after the word belong to the
        // command it runs, so they are read as that command's.
        "watch" => {
            let word = rest.first().filter(|w| WATCHED.contains(&w.as_str()));
            let watched = word.map_or("check", String::as_str);
            let positions = positionals(watched, rest);
            let targets = if word.is_some() { positions.get(1..).unwrap_or(&[]) } else { &positions[..] };
            targets.is_empty().then_some(usize::from(word.is_some()))
        }
        _ => None,
    }
}

/// The indices in `rest` of its positional arguments for `name`: the words that are neither options nor the values
/// of options that take one. The arguments after `--` belong to the program and are not counted.
fn positionals(name: &str, rest: &[String]) -> Vec<usize> {
    let mut found = Vec::new();
    let mut is_value = false;
    for (index, arg) in rest.iter().enumerate() {
        if std::mem::take(&mut is_value) {
            continue;
        }
        if arg == "--" {
            break;
        }
        if arg.len() > 1 && arg.starts_with('-') {
            // `--name value` takes the next word; `--name=value` and `-j4` carry their value.
            is_value = !arg.contains('=') && option_takes_value(name, arg);
        } else {
            found.push(index);
        }
    }
    found
}

/// Whether a word names a path rather than a name: a `.bd` file, a path with a separator, `.`, or an existing file
/// or directory. Test filters, bench filters, function names, and fuzz target names are not paths.
fn is_path(word: &str) -> bool {
    word.ends_with(".bd")
        || word.contains('/')
        || word.contains('\\')
        || word == "."
        || word == ".."
        || std::path::Path::new(word).exists()
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

/// The arguments of a command that takes one optional package directory and the flags in `flags` (`clean`,
/// `package`, `vendor`): the directory (the working directory when none is given) and the flags given, or `None`
/// for `--help`. An unknown option, or a second directory, is a usage error (exit status 2).
pub(crate) fn parse_dir_args(
    name: &str,
    argv: &[String],
    flags: &[&str],
) -> Result<Option<(std::path::PathBuf, Vec<String>)>, ExitCode> {
    let mut dir: Option<std::path::PathBuf> = None;
    let mut given = Vec::new();
    for arg in argv {
        if arg == "-h" || arg == "--help" {
            return Ok(None);
        }
        if flags.contains(&arg.as_str()) {
            given.push(arg.clone());
        } else if arg.starts_with('-') {
            return Err(usage_error(name, &format!("unknown option `{arg}` for `burdin {name}`")));
        } else if dir.is_some() {
            return Err(usage_error(
                name,
                &format!("`burdin {name}` takes one package directory, got a second: `{arg}`"),
            ));
        } else {
            dir = Some(std::path::PathBuf::from(arg));
        }
    }
    Ok(Some((dir.unwrap_or_else(|| std::path::PathBuf::from(".")), given)))
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
  BURDIN_LD             The linker: mold, lld, a path, or platform (default: mold/lld if installed on Linux;
                        sanitizer builds keep the compiler's linker)
  BURDIN_JOBS           The default of -j (default: the machine's parallelism; 1 is sequential)
  BURDIN_HOME           Where tools keep state, including the git dependency cache under git/
  BURDIN_CODEGEN_UNITS  Split code generation into exactly this many units (default: by program size)
  LLVM_SYS_211_PREFIX   The LLVM 21 installation; its bin/llvm-config resolves `--link-lib LLVM-C`
  COLUMNS               The terminal width used when --width is not given
  FORCE_HYPERLINK       1 links error codes to the error reference in colored output, 0 never does
  NO_COLOR              Disables color unless --color says otherwise";

const DIAGNOSTICS: &str = "Diagnostic Options";
const OUTPUT: &str = "Output Options";
const COMPILATION: &str = "Compilation Options";
const SELECTION: &str = "Selection Options";

/// The short name of each command, shown in help and accepted everywhere the name is: `burdin r` is `burdin run`.
const ALIASES: &[(&str, &str)] = &[
    ("build", "b"),
    ("run", "r"),
    ("test", "t"),
    ("check", "c"),
    ("new", "n"),
    ("init", "i"),
    ("add", "a"),
    ("fmt", "f"),
    ("lint", "l"),
    ("fix", "fx"),
    ("doc", "d"),
    ("bench", "bn"),
    ("watch", "w"),
    ("repl", "rp"),
    ("debug", "dbg"),
    ("fuzz", "fz"),
    ("clean", "cl"),
    ("ssr", "s"),
    ("refactor", "rf"),
    ("expand", "xp"),
    ("asm", "as"),
    ("bloat", "bl"),
    ("explain", "ex"),
    ("package", "pk"),
    ("vendor", "v"),
    ("bindgen", "bg"),
    ("help", "h"),
];

/// The whole command tree.
pub(crate) fn command() -> Command {
    let root = Command::new("burdin")
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
        // The order is the help's order: the commands used every day first, the specialized ones last (BT1).
        .subcommands([
            // Every day: write, build, run, and check a program.
            build(),
            run(),
            test(),
            check(),
            // Start and manage a package.
            new(),
            init(),
            add(),
            // Keep the code tidy.
            fmt(),
            lint(),
            fix(),
            doc(),
            // Measure, iterate, and explore.
            bench(),
            watch(),
            repl(),
            debug(),
            fuzz(),
            clean(),
            // Refactor and inspect.
            ssr(),
            refactor(),
            expand(),
            asm(),
            bloat(),
            explain(),
            // Packaging, bindings, and editor tooling.
            package(),
            vendor(),
            bindgen(),
            lsp(),
            dap(),
            // About the tool itself.
            Command::new("commands").about("List every command, alphabetically").override_usage("burdin commands"),
            Command::new("help").about("Print the help of a command").override_usage("burdin help [<command>]"),
        ]);
    ALIASES.iter().fold(root, |root, &(name, alias)| root.mut_subcommand(name, |c| c.visible_alias(alias)))
}

/// `burdin commands`: every command with its summary, alphabetically (the root help is ordered by use instead).
pub(crate) fn commands(rest: &[String]) -> ExitCode {
    if let Some(extra) = rest.first() {
        return usage_error("commands", &format!("`burdin commands` takes no arguments, got `{extra}`"));
    }
    let mut root = command();
    root.build();
    let mut all: Vec<(String, String)> = root
        .get_subcommands()
        .map(|c| (c.get_name().to_string(), c.get_about().map(|a| a.to_string()).unwrap_or_default()))
        .collect();
    all.sort();
    let width = all.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    for (name, about) in all {
        println!("{name:<width$}  {about}");
    }
    ExitCode::SUCCESS
}

// --- Shared arguments ---------------------------------------------------------------------------------------------

fn target() -> Arg {
    Arg::new("target").value_name("file.bd | package directory").help("The source file or package directory")
}

/// A target that is the working directory when none is given: `burdin run` is `burdin run .` (`default_target`).
fn target_or_here() -> Arg {
    target().default_value(".").help("The source file or package directory (default: the working directory)")
}

fn flag(id: &'static str, help: &'static str) -> Arg {
    Arg::new(id).long(id).action(ArgAction::SetTrue).help(help)
}

fn option(id: &'static str, value: &'static str, help: &'static str) -> Arg {
    Arg::new(id).long(id).value_name(value).help(help)
}

/// `-q` and `-v` of `build`, `run`, and `check`: what the command prints besides its results (`spec/build-tool.md`).
fn verbosity() -> [Arg; 2] {
    [
        flag("quiet", "Print only errors: no warnings and no notes (the results are unchanged)")
            .short('q')
            .help_heading(OUTPUT),
        flag("verbose", "Print each phase with its time, and each codegen unit of build and run, to standard error")
            .short('v')
            .help_heading(OUTPUT),
    ]
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

fn profile_option() -> Arg {
    option(
        "profile",
        "name",
        "Build with this profile: dev, release, test, bench, or one burdin.toml defines (spec/profiles.md)",
    )
}

/// `-O<n>`: the optimization level of one invocation, written `-O2` or `-O 2` (`spec/profiles.md`, BP4).
fn opt_level() -> Arg {
    Arg::new("opt-level")
        .short('O')
        .value_name("n")
        .help("Optimize at level n, from 0 to 3, instead of the profile's opt-level (-O2 is -O 2)")
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

/// `--features`: the features the package enables (`spec/packages.md`, PKG29). It may repeat.
fn features_option() -> Arg {
    option(
        "features",
        "list",
        "Enable these features of the package, separated by commas or spaces; may repeat (spec/packages.md)",
    )
    .action(ArgAction::Append)
}

fn no_default_features() -> Arg {
    flag("no-default-features", "Do not enable the package's default feature (spec/packages.md, PKG29)")
}

fn all_features() -> Arg {
    flag("all-features", "Enable every feature and optional dependency of the package (spec/packages.md, PKG29)")
}

fn offline() -> Arg {
    flag("offline", "Fetch no git package: read it from the git cache or vendor/ (spec/packages.md)")
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
            .override_usage(
                "burdin build [file.bd | package directory | workspace root] [OPTIONS]
       \n                 burdin build <workspace root> -p <member>... | --workspace [--exclude <member>]... [OPTIONS]",
            )
            .arg(target_or_here())
            .arg(Arg::new("output").short('o').value_name("output").help("Where to write the executable"))
            .args(member_selection())
            .args([
                release("Build with optimizations"),
                profile_option(),
                opt_level(),
                debug_info(),
                locked(),
                features_option(),
                no_default_features(),
                all_features(),
                offline(),
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
            .args(verbosity())
            .after_help(ENVIRONMENT),
    )
}

fn run() -> Command {
    with_diagnostics(
        Command::new("run")
            .about("Build a program and run it")
            .override_usage(
                "burdin run [file.bd | package directory] [-p <member>] [OPTIONS] [-- <program arguments>...]",
            )
            .arg(target_or_here())
            .args(member_selection())
            .arg(
                Arg::new("program-args")
                    .value_name("program arguments")
                    .num_args(0..)
                    .last(true)
                    .help("Arguments passed to the program"),
            )
            .args([
                release("Build with optimizations"),
                profile_option(),
                opt_level(),
                debug_info(),
                locked(),
                features_option(),
                no_default_features(),
                all_features(),
                offline(),
                link_lib(),
                sanitize(),
                compile_jobs(),
                comptime_steps(),
            ])
            .args(watch_options())
            .args(verbosity())
            .after_help(
                "A single file that starts with a shebang or an inline manifest runs as a script, from a cache \
                 (spec/scripts.md). The exit status is the program's.\n\n"
                    .to_string()
                    + ENVIRONMENT,
            ),
    )
}

/// The selection options `test` and `bench` share.
/// The flags that pick members of a workspace for the commands that build packages (`spec/workspaces.md`, WS6).
fn member_selection() -> [Arg; 3] {
    [
        Arg::new("package")
            .short('p')
            .long("package")
            .value_name("package")
            .action(ArgAction::Append)
            .help("Select this member of the workspace; may repeat"),
        flag("workspace", "Select every member of the workspace, except those --exclude names"),
        option("exclude", "member", "With --workspace, leave out this member; may repeat").action(ArgAction::Append),
    ]
    .map(|arg| arg.help_heading(SELECTION))
}

fn selection(noun: &'static str) -> [Arg; 6] {
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
        Arg::new("package").short('p').long("package").value_name("package").action(ArgAction::Append).help(packages),
        flag("workspace", workspace),
        option("exclude", "member", "With --workspace, leave out this member; may repeat").action(ArgAction::Append),
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
            .override_usage("burdin test [file.bd | package directory] [<filter>...] [OPTIONS]")
            .arg(target_or_here())
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
                    profile_option(),
                    opt_level(),
                    debug_info(),
                    locked(),
                    features_option(),
                    no_default_features(),
                    all_features(),
                    offline(),
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
                    profile_option(),
                    opt_level(),
                    debug_info(),
                    locked(),
                    features_option(),
                    no_default_features(),
                    all_features(),
                    offline(),
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
            .override_usage("burdin bench [file.bd | package directory] [<filter>...] [OPTIONS]")
            .arg(target_or_here())
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
                    release("Build with optimizations (the same as --profile release)"),
                    profile_option(),
                    opt_level(),
                    debug_info(),
                    locked(),
                    features_option(),
                    no_default_features(),
                    all_features(),
                    offline(),
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
            .override_usage("burdin check [file.bd | package directory] [OPTIONS]")
            .arg(target_or_here())
            .args([
                release("Check the way `burdin build --release` does"),
                profile_option(),
                opt_level(),
                flag("tests", "Also check the `test` blocks, and do not require a `main` (a library package)"),
                flag("print-instances", "Print every generic instance the program created and why (spec/inspect.md)"),
                locked(),
                features_option(),
                no_default_features(),
                all_features(),
                offline(),
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
            .args(verbosity())
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
                 burdin fmt -p <member>... | --workspace [--exclude <member>]... [OPTIONS]\n       \
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
            .args(member_selection())
            .after_help("There is one style and nothing to configure. The behavior is spec/fmt.md."),
    )
}

fn lint() -> Command {
    with_diagnostics(
        Command::new("lint")
            .about("Report common mistakes and style issues, and fix them")
            .override_usage(
                "burdin lint [file.bd | package directory] [OPTIONS]\n       \
                 burdin lint --list\n       \
                 burdin lint --explain <lint>",
            )
            .arg(target_or_here())
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
            .override_usage("burdin fix [file.bd | package directory] [OPTIONS]")
            .arg(target_or_here())
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
            .override_usage("burdin ssr '<pattern> [==>> <replacement>]' [file.bd | package directory] [OPTIONS]")
            .arg(Arg::new("pattern").value_name("pattern").required(true).help(
                "The pattern, and after `==>>` what to put in its place. `$name` is any expression; a type may be \
                 constrained after `where`: `$a.unwrap_or($b) where $a: Option<_>`. Without a replacement the matches \
                 are listed",
            ))
            .arg(target_or_here())
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
            .arg(target_or_here())
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
            .override_usage(
                "burdin doc [file.bd | package directory | workspace root] [OPTIONS]
       \n                 burdin doc <workspace root> -p <member>... | --workspace [--exclude <member>]... [OPTIONS]
       \n                 burdin doc --std [OPTIONS]",
            )
            .arg(target_or_here())
            .args(member_selection())
            .args([
                Arg::new("out").short('o').long("out").value_name("dir | file").help(
                    "Where to write: a directory for html (default: <package>/target/doc, or target/doc/std with \
                     --std), a file for json (default: standard output)",
                ),
                option("output-format", "html|json", "A static site (default: html) or the documented API as JSON"),
                flag("std", "Document the standard library the compiler carries, instead of a target")
                    .conflicts_with("target"),
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
        .override_usage("burdin new <name> [--lib | --bin]")
        .arg(Arg::new("name").value_name("name").required(true).help("The package directory to create"))
        .args(kind_flags())
        .after_help(
            "Creates <name>/burdin.toml, <name>/src/main.bd (or <name>/src/lib.bd with --lib), and <name>/.gitignore. \
             It refuses a path that already exists.",
        )
}

fn init() -> Command {
    Command::new("init")
        .about("Create a package in an existing directory")
        .override_usage("burdin init [<dir>] [--lib | --bin]")
        .arg(
            Arg::new("dir")
                .value_name("dir")
                .help("The existing directory to write the package into (default: the working directory)"),
        )
        .args(kind_flags())
        .after_help(
            "Writes burdin.toml, src/main.bd (or src/lib.bd with --lib), and .gitignore into <dir>, and leaves its \
             other files alone. It refuses, before writing anything, when one of those files already exists.",
        )
}

/// `--lib` and `--bin`, the two package shapes. A package is a program unless `--lib` is given.
fn kind_flags() -> [Arg; 2] {
    [
        flag("lib", "Create a library: src/lib.bd, which `burdin test` runs and other packages `use`"),
        flag("bin", "Create a program: src/main.bd, which `burdin run` runs (the default)"),
    ]
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
             the commit its branch, tag, or rev names now. An official package (one bundled with the compiler) is not \
             declared: `use <name>` loads it. On failure neither file changes. The behavior is spec/packages.md.",
        )
}

fn clean() -> Command {
    Command::new("clean")
        .about("Remove a package's build output, and with --cache its caches")
        .override_usage("burdin clean [<dir>] [--cache]")
        .args([
            package_dir(),
            flag("cache", "Also remove the caches under the Burdin home: compiled scripts and the git cache"),
        ])
        .after_help(
            "Removes <dir>/target, which holds everything a build writes. It never removes vendor/, which an offline \
             build needs. The behavior is spec/packages.md (PKG19).",
        )
}

fn package() -> Command {
    Command::new("package")
        .about("Write the package's reproducible archive to target/package")
        .override_usage("burdin package [<dir>]")
        .arg(package_dir())
        .after_help(
            "Writes target/package/<name>-<version>.tar and prints its checksum. The files are chosen by [package] \
             include and exclude, and the archive is the same for the same tree (spec/packages.md, PKG20-PKG22).",
        )
}

fn vendor() -> Command {
    Command::new("vendor")
        .about("Copy the git dependencies into vendor/ for offline builds")
        .override_usage("burdin vendor [<dir>]")
        .arg(package_dir())
        .after_help(
            "Resolves the graph, writing burdin.lock as a build does, then copies each git package into \
             vendor/<name>. A build with --offline then reads them without the network (spec/packages.md, PKG23-PKG24).",
        )
}

fn package_dir() -> Arg {
    Arg::new("dir").value_name("dir").help("The package directory (default: the working directory)")
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
            .override_usage("burdin debug [file.bd | package directory] [OPTIONS] [-- <program arguments>...]")
            .arg(target_or_here())
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
                profile_option(),
                opt_level(),
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
            .override_usage("burdin expand [file.bd | package directory] [OPTIONS]")
            .arg(target_or_here())
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
            .override_usage("burdin asm [file.bd | package directory] [<function>] [OPTIONS]")
            .arg(target_or_here())
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
            .override_usage("burdin bloat [file.bd | package directory] [OPTIONS]")
            .arg(target_or_here())
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
        .override_usage("burdin watch [check|build|test|run] [file.bd | package directory] [OPTIONS]")
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
        for name in [
            "build", "run", "test", "bench", "check", "fmt", "lint", "doc", "new", "init", "add", "clean", "package",
            "vendor", "explain", "watch", "lsp",
        ] {
            assert!(help.contains(&format!("  {name} ")), "`{name}` is missing from:\n{help}");
        }
        // The list stays short: the options of a command are only in its own help.
        assert!(!help.contains("--release"), "the command list shows options:\n{help}");
    }

    #[test]
    fn a_usage_error_names_the_command_and_its_help() {
        let text = usage_error_text("check", "unknown option `--bogus`");
        assert!(text.starts_with("error: unknown option `--bogus`\n\nusage: burdin check [file.bd"), "{text}");
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

    fn words(w: &[&str]) -> Vec<String> {
        w.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn every_alias_is_a_short_name_of_its_command() {
        let mut seen = std::collections::HashSet::new();
        for &(name, alias) in ALIASES {
            // `help` is routed by name before the command table is consulted, so `is_command` leaves it out.
            if name != "help" {
                assert!(is_command(name), "`{name}` is not a command");
            }
            assert!(is_command(alias) || alias == "h", "`{alias}` is not accepted");
            assert_eq!(canonical(alias), name, "`{alias}` should be `{name}`");
            assert!(seen.insert(alias), "`{alias}` is used twice");
            assert!(command().find_subcommand(alias).is_none_or(|c| c.get_name() == name), "`{alias}` shadows a name");
        }
        assert_eq!(canonical("run"), "run");
        assert_eq!(canonical("zz"), "zz");
    }

    #[test]
    fn an_alias_routes_to_its_canonical_command() {
        assert!(matches!(route(&words(&["r", "app"])), Route::Command(name) if name == "run"));
        assert!(matches!(route(&words(&["t"])), Route::Command(name) if name == "test"));
        assert!(matches!(route(&words(&["h"])), Route::Exit(_)));
    }

    #[test]
    fn an_omitted_target_is_the_working_directory() {
        let default = |w: &[&str]| with_default_target(words(w));
        assert_eq!(default(&["run"]), words(&["run", "."]));
        assert_eq!(default(&["run", "--release"]), words(&["run", ".", "--release"]));
        assert_eq!(default(&["run", "--profile", "dev"]), words(&["run", ".", "--profile", "dev"]));
        assert_eq!(default(&["run", "--profile=dev"]), words(&["run", ".", "--profile=dev"]));
        assert_eq!(default(&["run", "--", "x"]), words(&["run", ".", "--", "x"]));
        assert_eq!(default(&["run", "app"]), words(&["run", "app"]));
        assert_eq!(default(&["run", ".", "--release"]), words(&["run", ".", "--release"]));
        assert_eq!(default(&["build", "-j", "2"]), words(&["build", ".", "-j", "2"]));
        assert_eq!(default(&["check"]), words(&["check", "."]));
        assert_eq!(default(&["debug", "-g"]), words(&["debug", ".", "-g"]));
        assert_eq!(default(&["fix"]), words(&["fix", "."]));
        assert_eq!(default(&["expand", "--item", "f"]), words(&["expand", ".", "--item", "f"]));
    }

    #[test]
    fn a_word_after_test_asm_or_fuzz_is_a_name_unless_it_is_a_path() {
        let default = |w: &[&str]| with_default_target(words(w));
        assert_eq!(default(&["test"]), words(&["test", "."]));
        assert_eq!(default(&["test", "parse"]), words(&["test", ".", "parse"]));
        assert_eq!(default(&["test", "parse", "lex"]), words(&["test", ".", "parse", "lex"]));
        assert_eq!(default(&["test", "app.bd", "parse"]), words(&["test", "app.bd", "parse"]));
        assert_eq!(default(&["test", "sub/app", "parse"]), words(&["test", "sub/app", "parse"]));
        // `--filter` takes its value, so `x` is not a positional word; `y` is the first, and it is a name.
        assert_eq!(default(&["test", "--filter", "x", "y"]), words(&["test", "--filter", "x", ".", "y"]));
        assert_eq!(default(&["test", "--filter", "x", "app.bd"]), words(&["test", "--filter", "x", "app.bd"]));
        assert_eq!(default(&["bench", "--list"]), words(&["bench", ".", "--list"]));
        assert_eq!(default(&["asm", "main"]), words(&["asm", ".", "main"]));
        assert_eq!(default(&["asm", "app.bd", "main"]), words(&["asm", "app.bd", "main"]));
        assert_eq!(default(&["fuzz", "parse"]), words(&["fuzz", ".", "parse"]));
        assert_eq!(default(&["fuzz", "app.bd", "parse"]), words(&["fuzz", "app.bd", "parse"]));
    }

    #[test]
    fn ssr_and_refactor_take_the_target_after_their_first_word() {
        let default = |w: &[&str]| with_default_target(words(w));
        assert_eq!(default(&["ssr", "$a.unwrap()"]), words(&["ssr", "$a.unwrap()", "."]));
        assert_eq!(default(&["ssr", "$a", "app.bd"]), words(&["ssr", "$a", "app.bd"]));
        assert_eq!(
            default(&["refactor", "extract-variable", "app.bd:1:2"]),
            words(&["refactor", "extract-variable", "app.bd:1:2"])
        );
        assert_eq!(default(&["refactor", "inline-variable"]), words(&["refactor", "inline-variable", "."]));
    }

    #[test]
    fn watch_reads_the_options_of_the_command_it_runs() {
        let default = |w: &[&str]| with_default_target(words(w));
        assert_eq!(default(&["watch"]), words(&["watch", "."]));
        assert_eq!(default(&["watch", "--no-clear"]), words(&["watch", ".", "--no-clear"]));
        assert_eq!(default(&["watch", "run"]), words(&["watch", "run", "."]));
        assert_eq!(default(&["watch", "run", "app.bd"]), words(&["watch", "run", "app.bd"]));
        assert_eq!(default(&["watch", "run", "--profile", "dev"]), words(&["watch", "run", ".", "--profile", "dev"]));
        assert_eq!(default(&["watch", "test", "--filter", "x"]), words(&["watch", "test", ".", "--filter", "x"]));
    }

    #[test]
    fn a_command_without_a_target_is_left_alone() {
        assert_eq!(with_default_target(words(&["new", "app"])), words(&["new", "app"]));
        assert_eq!(with_default_target(words(&["fmt"])), words(&["fmt"]));
        assert_eq!(with_default_target(words(&["fuzz"])), words(&["fuzz", "."]));
        // `doc --std` takes no target; plain `doc` still defaults to the working directory.
        assert_eq!(with_default_target(words(&["doc", "--std"])), words(&["doc", "--std"]));
        assert_eq!(with_default_target(words(&["doc", "--std", "-o", "out"])), words(&["doc", "--std", "-o", "out"]));
        assert_eq!(with_default_target(words(&["doc"])), words(&["doc", "."]));
    }
}
