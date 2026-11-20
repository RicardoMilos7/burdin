//! Black-box tests for `build.bd` through the command line (`spec/build-scripts.md`, BS1-BS8, issue #362): the
//! success path, the stdout/stderr split, the documented arguments and environment, the content-keyed rerun rules
//! (declared inputs including directories, the script's `embed` files, generated outputs, the script, the manifest,
//! and the profile), the dependency grant, and every failure path with its code and exit status.
//!
//! Each test writes fixture packages into a fresh temporary directory and runs the compiled `burdin` binary as a
//! user would, so it covers exit statuses, routing, and caches rather than the driver's internals. The conformance
//! tests in `tests/m22_build/` cover each diagnostic line by line; this file is the driver/CLI evidence the issue's
//! verification asks for.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// A temporary directory that removes itself, so a failed assertion leaves no fixture behind.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let path =
            std::env::temp_dir().join(format!("burdin-build-cli-{name}-{}-{unique}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&path).expect("a temporary directory is created");
        TempDir(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("a fixture directory is created");
    }
    std::fs::write(path, text).expect("a fixture file is written");
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn manifest(name: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n")
}

/// Puts a text inside a Burdin string literal: backslashes, quotes, newlines, and the braces of an interpolated
/// string escaped the way the language spells them.
fn literal(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('{', "{{").replace('}', "}}")
}

/// A build script that prints `marker`, then writes `source` as the module `<out>/src/<module>`.
fn generating_script(marker: &str, module: &str, source: &str) -> String {
    format!(
        r#"fn main() {{
    println("{marker}")
    let args = env.args()
    let out = args[2]
    let source = "{literal_source}"
    match write_file("{{out}}/src/{module}", source) {{
        Ok(_) => {{}}
        Err(_) => panic("cannot write the generated module"),
    }}
}}
"#,
        literal_source = literal(source)
    )
}

/// Writes an `app` package with a `main.bd` and, when given, a `build.bd`.
fn package(dir: &Path, _name: &str, manifest: &str, script: Option<&str>, main: &str) {
    write(&dir.join("burdin.toml"), manifest);
    write(&dir.join("src/main.bd"), main);
    if let Some(script) = script {
        write(&dir.join("build.bd"), script);
    }
}

/// Runs the driver from `dir` (the working directory, where a build writes its executable).
fn burdin(dir: &Path, args: &[&str]) -> Output {
    std::fs::create_dir_all(dir).expect("the working directory is created");
    Command::new(env!("CARGO_BIN_EXE_burdin"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("the burdin binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn succeed(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed with {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        stdout(output),
        stderr(output)
    );
}

fn fail(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(1), "a build failure exits with status 1:\n{}", stderr(output));
    stderr(output)
}

fn contains(text: &str, needle: &str, what: &str) {
    assert!(text.contains(needle), "{what}: expected `{needle}` in:\n{text}");
}

fn path_text(path: &Path) -> &str {
    path.to_str().expect("the temporary path is UTF-8")
}

/// Sets a file's modification time forward, so a content-keyed cache must ignore it.
fn touch(path: &Path) {
    let later = SystemTime::now() + Duration::from_secs(120);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap_or_else(|e| panic!("cannot open {}: {e}", path.display()))
        .set_modified(later)
        .unwrap_or_else(|e| panic!("cannot set the modification time of {}: {e}", path.display()));
}

#[test]
fn the_script_runs_before_the_sources_and_its_output_goes_to_stderr() {
    let temp = TempDir::new("output");
    let app = temp.path().join("app");
    let run = temp.path().join("run");
    package(
        &app,
        "app",
        &manifest("app"),
        Some(&generating_script(
            "build script stdout",
            "generated.bd",
            "const ANSWER: i32 = 42\n\nfn triple(n: i32): i32 {\n    n * 3\n}\n",
        )),
        "use generated.{ANSWER, triple}\n\nfn main() {\n    println(\"{} {}\", ANSWER, triple(2))\n}\n",
    );

    let built = burdin(&run, &["build", path_text(&app)]);
    succeed(&built, "the build with a generated module");
    assert!(stdout(&built).is_empty(), "a build leaves stdout for the compiler's own artifacts:\n{}", stdout(&built));
    contains(&stderr(&built), "build script stdout", "the script's output");

    // The generated module is an ordinary module of the package: `main` imports `ANSWER` and `triple`.
    let exe = run.join("app").with_extension(std::env::consts::EXE_EXTENSION);
    let program = Command::new(&exe).output().expect("the built program runs");
    assert!(program.status.success(), "the program failed: {}", stderr(&program));
    assert_eq!(stdout(&program), "42 6\n", "the program uses the generated module");

    // `burdin run` leaves the program's stdout exact. The cache is already warm, so the script does not run again.
    let rerun = burdin(&run, &["run", path_text(&app)]);
    succeed(&rerun, "the run");
    assert_eq!(stdout(&rerun), "42 6\n", "`burdin run` stdout is the program's alone");
    assert!(!stderr(&rerun).contains("build script stdout"), "a cached script did not run again");
}

#[test]
fn the_cache_reruns_only_when_an_input_script_manifest_or_output_changes() {
    let temp = TempDir::new("cache");
    let app = temp.path().join("app");
    let run = temp.path().join("run");
    let script = "\
fn main() {
    println(\"the script ran\")
    let header = embed(\"header.h\")
    let args = env.args()
    let out = args[2]
    match write_file(\"{out}/inputs\", \"declared.txt\\ninclude\\n\") {
        Ok(_) => {}
        Err(_) => panic(\"cannot write the input list\"),
    }
    let source = \"const LEN: i32 = {header.len()}\\n\"
    match write_file(\"{out}/src/generated.bd\", source) {
        Ok(_) => {}
        Err(_) => panic(\"cannot write the generated module\"),
    }
}
";
    package(
        &app,
        "app",
        &manifest("app"),
        Some(script),
        "use generated.LEN\n\nfn main() {\n    println(\"{}\", LEN)\n}\n",
    );
    write(&app.join("declared.txt"), "first input\n");
    write(&app.join("include/point.h"), "struct Point { int x; };\n");
    write(&app.join("header.h"), "header\n");
    let output = app.join("target/burdin/debug");
    let generated = output.join("src/generated.bd");

    let deps = run.join("deps.txt");
    let first = burdin(&run, &["build", path_text(&app), "--emit-deps", path_text(&deps)]);
    succeed(&first, "the first build");
    contains(&stderr(&first), "the script ran", "the first build");
    let listed = read(&deps);
    for input in ["declared.txt", "include", "header.h"] {
        contains(&listed, input, "--emit-deps lists every declared input and the script's `embed` input");
    }

    let reused = burdin(&run, &["build", path_text(&app)]);
    succeed(&reused, "the cached build");
    assert!(!stderr(&reused).contains("the script ran"), "a valid cache reran the script:\n{}", stderr(&reused));

    // Content, not modification time, decides: touching every input does not rerun.
    touch(&app.join("declared.txt"));
    touch(&app.join("header.h"));
    touch(&app.join("include/point.h"));
    let touched = burdin(&run, &["build", path_text(&app)]);
    succeed(&touched, "the touched-input build");
    assert!(!stderr(&touched).contains("the script ran"), "touching an input reran the script");

    // A changed declared file, a changed `embed` file, and a changed file inside a declared directory each rerun.
    write(&app.join("declared.txt"), "a changed declared input\n");
    let declared = burdin(&run, &["build", path_text(&app)]);
    succeed(&declared, "the changed declared input");
    contains(&stderr(&declared), "the script ran", "a changed declared input");
    write(&app.join("header.h"), "a changed header\n");
    let embedded = burdin(&run, &["build", path_text(&app)]);
    succeed(&embedded, "the changed `embed` input");
    contains(&stderr(&embedded), "the script ran", "a changed `embed` input");
    write(&app.join("include/point.h"), "struct Point { int y; };\n");
    let inside = burdin(&run, &["build", path_text(&app)]);
    succeed(&inside, "the changed directory input");
    contains(&stderr(&inside), "the script ran", "a changed file inside a declared directory");
    write(&app.join("include/added.h"), "#define ADDED 1\n");
    let added = burdin(&run, &["build", path_text(&app)]);
    succeed(&added, "the added directory input");
    contains(&stderr(&added), "the script ran", "a file added to a declared directory");

    // The script and the manifest are inputs like any other; a comment is enough of a content change.
    write(&app.join("build.bd"), &format!("{script}// a changed script\n"));
    let script_changed = burdin(&run, &["build", path_text(&app)]);
    succeed(&script_changed, "the changed script");
    contains(&stderr(&script_changed), "the script ran", "a changed build.bd");
    write(&app.join("burdin.toml"), &format!("{}# a changed manifest\n", manifest("app")));
    let manifest_changed = burdin(&run, &["build", path_text(&app)]);
    succeed(&manifest_changed, "the changed manifest");
    contains(&stderr(&manifest_changed), "the script ran", "a changed burdin.toml");

    // A deleted or edited generated output is not fresh.
    std::fs::remove_file(&generated).expect("the generated module exists");
    let deleted = burdin(&run, &["build", path_text(&app)]);
    succeed(&deleted, "the deleted-output build");
    contains(&stderr(&deleted), "the script ran", "a deleted generated module");
    assert!(generated.is_file(), "the generated module was not recreated");
    write(&generated, "const LEN: i32 = 999\n");
    let edited = burdin(&run, &["build", path_text(&app)]);
    succeed(&edited, "the edited-output build");
    contains(&stderr(&edited), "the script ran", "an edited generated module");
    assert_eq!(read(&generated), "const LEN: i32 = 17\n", "the rerun restored the script's output");

    // The profile is part of the cache: a release build runs the script again.
    let release = burdin(&run, &["build", path_text(&app), "--release"]);
    succeed(&release, "the release build");
    contains(&stderr(&release), "the script ran", "the release profile reused the debug cache");
    assert!(app.join("target/burdin/release/src/generated.bd").is_file());
}

#[test]
fn the_script_gets_the_documented_arguments_and_environment() {
    let temp = TempDir::new("environment");
    let app = temp.path().join("app");
    let run = temp.path().join("run");
    let script = "\
fn require(condition: bool, message: str) {
    if !condition {
        panic(message)
    }
}

fn main() {
    let args = env.args()
    require(args.len() == 3, \"argv is the program, the package directory, and the output directory\")
    match env.get(\"BURDIN_MANIFEST_DIR\") {
        Some(dir) => require(args[1] == dir, \"argv[1] is BURDIN_MANIFEST_DIR\"),
        None => panic(\"BURDIN_MANIFEST_DIR is unset\"),
    }
    match env.get(\"BURDIN_OUT_DIR\") {
        Some(dir) => require(args[2] == dir, \"argv[2] is BURDIN_OUT_DIR\"),
        None => panic(\"BURDIN_OUT_DIR is unset\"),
    }
    match env.get(\"BURDIN_PROFILE\") {
        Some(profile) => require(
            profile.as_str() == \"debug\" || profile.as_str() == \"release\",
            \"BURDIN_PROFILE is the profile\",
        ),
        None => panic(\"BURDIN_PROFILE is unset\"),
    }
    require(env.get(\"BURDIN\").is_some(), \"BURDIN names the driver\")
    // A relative path lands in the package directory, which is the working directory.
    match write_file(\"script-cwd.txt\", \"written by the script\") {
        Ok(_) => {}
        Err(_) => panic(\"cannot write in the working directory\"),
    }
    let out = args[2]
    match write_file(\"{out}/src/from_env.bd\", \"const FROM_ENV: i32 = 1\\n\") {
        Ok(_) => {}
        Err(_) => panic(\"cannot write into the output directory\"),
    }
}
";
    // The generated module is not imported by `main`; the build proves the script passed its own checks.
    package(&app, "app", &manifest("app"), Some(script), "fn main() {}\n");
    let built = burdin(&run, &["build", path_text(&app)]);
    succeed(&built, "the environment build");
    assert_eq!(read(&app.join("script-cwd.txt")), "written by the script", "the working directory is the package");
    assert!(app.join("target/burdin/debug/src/from_env.bd").is_file(), "argv[2] is the output directory");

    let release = burdin(&run, &["build", path_text(&app), "--release"]);
    succeed(&release, "the release environment build");
    assert!(app.join("target/burdin/release/src/from_env.bd").is_file());
}

#[test]
fn a_dependency_script_needs_a_grant_and_is_trusted_when_tested_alone() {
    let temp = TempDir::new("grant");
    let app = temp.path().join("app");
    let lib = temp.path().join("lib");
    let run = temp.path().join("run");
    write(&lib.join("burdin.toml"), &manifest("lib"));
    write(&lib.join("src/lib.bd"), "pub use extra.VALUE\n");
    write(&lib.join("build.bd"), &generating_script("the lib script ran", "extra.bd", "const VALUE: i32 = 1\n"));
    write(&app.join("src/main.bd"), "fn main() {\n    println(\"{}\", lib.VALUE)\n}\n");
    let dependency = "[dependencies]\nlib = { path = \"../lib\" }\n";
    let un_granted = format!("{}{dependency}", manifest("app"));
    write(&app.join("burdin.toml"), &un_granted);

    let refused = burdin(&run, &["build", path_text(&app)]);
    let text = fail(&refused);
    contains(&text, "error[E0260]", "an un-granted dependency script");
    contains(&text, "not allowed", "the grant error names the rule");
    contains(&text, "`lib`", "the grant error names the dependency");
    contains(&text, "add `lib` to", "the note says how to grant the script");

    // A dependency selected with `-p` is the entry package for its own compilation, so its script is trusted.
    let alone = burdin(&run, &["test", path_text(&app), "-p", "lib"]);
    succeed(&alone, "`burdin test -p lib` without the entry package's grant");
    assert!(lib.join("target/burdin/debug/src/extra.bd").is_file(), "the dependency's script ran for `-p lib`");

    let granted = format!("{}{dependency}\n[build]\nallow = [\"lib\"]\n", manifest("app"));
    write(&app.join("burdin.toml"), &granted);
    // Clear the dependency's cache so this build really has to run its script.
    let _ = std::fs::remove_dir_all(lib.join("target"));
    let built = burdin(&run, &["build", path_text(&app)]);
    succeed(&built, "the granted dependency build");
    contains(&stderr(&built), "the lib script ran", "a granted dependency script runs");
    let program = Command::new(run.join("app").with_extension(std::env::consts::EXE_EXTENSION))
        .output()
        .expect("the built program runs");
    assert_eq!(stdout(&program), "1\n", "the generated dependency module is usable");

    // An `allow` entry that names no dependency in the build is a typo, and E0260 says so rather than silently
    // disabling the grant.
    let ghost = format!("{}{dependency}\n[build]\nallow = [\"lib\", \"ghost\"]\n", manifest("app"));
    write(&app.join("burdin.toml"), &ghost);
    let typo = burdin(&run, &["build", path_text(&app)]);
    let text = fail(&typo);
    contains(&text, "error[E0260]", "an `allow` entry that names no dependency");
    contains(&text, "`ghost`", "the error names the unknown entry");
}

#[test]
fn every_script_failure_is_reported_with_its_code() {
    let temp = TempDir::new("failures");
    let run = temp.path().join("run");

    // A script that ends non-zero: its own output is on stderr, and E0261 is at the entry package's first line.
    let exit = temp.path().join("exit");
    let script = "fn main() {\n    println(\"the script is running\")\n    process.exit(3)\n}\n";
    package(&exit, "exit", &manifest("exit"), Some(script), "fn main() {}\n");
    let failed = burdin(&run, &["build", path_text(&exit), "--diagnostic-format", "short"]);
    let text = fail(&failed);
    assert!(stdout(&failed).is_empty(), "a failing build leaves stdout empty:\n{}", stdout(&failed));
    contains(&text, "the script is running", "the failing script's output reaches stderr");
    contains(&text, "src/main.bd:1:1: error[E0261]", "E0261 is at the entry package's first source line");
    contains(&text, "exit code 3", "E0261 names the script's status");

    // A script that does not compile: its diagnostics come first, for `build.bd`, then E0261 for the package.
    let broken = temp.path().join("broken");
    let script = "fn main() {\n    let broken: i32 = \"not a number\"\n    println(broken)\n}\n";
    package(&broken, "broken", &manifest("broken"), Some(script), "fn main() {}\n");
    let failed = burdin(&run, &["build", path_text(&broken), "--diagnostic-format", "short"]);
    let text = fail(&failed);
    contains(&text, "build.bd", "the script's own diagnostics are reported first");
    contains(&text, "error[E0003]", "the script's compile error is reported");
    contains(&text, "error[E0261]", "a script that does not compile fails the build");
    contains(&text, "did not compile", "E0261 says the script did not compile");
    let script_error = text.find("build.bd").expect("the script's diagnostic is present");
    let package_error = text.find("error[E0261]").expect("the package diagnostic is present");
    assert!(script_error < package_error, "the script's diagnostics come before E0261:\n{text}");

    // A declared input that does not exist would rerun every build, so it is an error.
    let missing = temp.path().join("missing");
    let script = "\
fn main() {
    let args = env.args()
    let out = args[2]
    match write_file(\"{out}/inputs\", \"gone.txt\\n\") {
        Ok(_) => {}
        Err(_) => panic(\"cannot write the input list\"),
    }
}
";
    package(&missing, "missing", &manifest("missing"), Some(script), "fn main() {}\n");
    let failed = burdin(&run, &["build", path_text(&missing), "--diagnostic-format", "short"]);
    let text = fail(&failed);
    contains(&text, "error[E0261]", "a missing declared input fails the build");
    contains(&text, "does not exist", "the missing input is named as missing");

    // A generated module that repeats a source module is rejected rather than overriding it.
    let conflict = temp.path().join("conflict");
    let script = &generating_script("conflict", "main.bd", "fn main() {}\n");
    package(&conflict, "conflict", &manifest("conflict"), Some(script), "fn main() {}\n");
    let failed = burdin(&run, &["build", path_text(&conflict), "--diagnostic-format", "short"]);
    let text = fail(&failed);
    contains(&text, "error[E0262]", "a generated module that repeats a source module");
    contains(&text, "generated the module `main`", "the conflict names the module");

    // A generated path that is not an identifier cannot become a module name.
    let bad_name = temp.path().join("bad-name");
    let script = &generating_script("bad name", "bad-name.bd", "const X: i32 = 1\n");
    package(&bad_name, "bad_name", &manifest("bad_name"), Some(script), "fn main() {}\n");
    let failed = burdin(&run, &["build", path_text(&bad_name), "--diagnostic-format", "short"]);
    let text = fail(&failed);
    contains(&text, "error[E0262]", "a generated path that is not an identifier");
    contains(&text, "identifier", "the error says what a module name must be");
}

#[test]
fn generated_modules_register_under_their_paths_and_stale_ones_disappear() {
    let temp = TempDir::new("generated");
    let run = temp.path().join("run");

    // A nested generated module is `geometry.shapes`, like a file under the package's own `src/`.
    let nested = temp.path().join("nested");
    let script = "\
fn main() {
    let args = env.args()
    let out = args[2]
    match create_dir_all(\"{out}/src/geometry\") {
        Ok(_) => {}
        Err(_) => panic(\"cannot create the generated directory\"),
    }
    match write_file(\"{out}/src/geometry/shapes.bd\", \"const N: i32 = 5\\n\") {
        Ok(_) => {}
        Err(_) => panic(\"cannot write the generated module\"),
    }
}
";
    package(
        &nested,
        "nested",
        &manifest("nested"),
        Some(script),
        "use geometry.shapes.N\n\nfn main() {\n    println(\"{}\", N)\n}\n",
    );
    let built = burdin(&run, &["build", path_text(&nested)]);
    succeed(&built, "the nested generated module");
    let program = Command::new(run.join("nested").with_extension(std::env::consts::EXE_EXTENSION))
        .output()
        .expect("the built program runs");
    assert_eq!(stdout(&program), "5\n", "a nested generated module is imported by its dotted path");

    // The `src/` directory is cleared before a rerun, so a module the script stops producing does not linger.
    let stale = temp.path().join("stale");
    let first = "\
fn main() {
    let args = env.args()
    let out = args[2]
    match write_file(\"{out}/src/a.bd\", \"const A: i32 = 1\\n\") {
        Ok(_) => {}
        Err(_) => panic(\"cannot write a.bd\"),
    }
    match write_file(\"{out}/src/b.bd\", \"const B: i32 = 2\\n\") {
        Ok(_) => {}
        Err(_) => panic(\"cannot write b.bd\"),
    }
}
";
    package(&stale, "stale", &manifest("stale"), Some(first), "use b.B\n\nfn main() {\n    println(\"{}\", B)\n}\n");
    succeed(&burdin(&run, &["build", path_text(&stale)]), "the first stale-output build");
    let out_src = stale.join("target/burdin/debug/src");
    assert!(out_src.join("a.bd").is_file() && out_src.join("b.bd").is_file());
    let second = "\
fn main() {
    let args = env.args()
    let out = args[2]
    match write_file(\"{out}/src/b.bd\", \"const B: i32 = 2\\n\") {
        Ok(_) => {}
        Err(_) => panic(\"cannot write b.bd\"),
    }
}
";
    write(&stale.join("build.bd"), second);
    succeed(&burdin(&run, &["build", path_text(&stale)]), "the second stale-output build");
    assert!(!out_src.join("a.bd").exists(), "a generated module the script stopped writing must not survive");
    assert!(out_src.join("b.bd").is_file(), "the module the script still writes is present");
}
