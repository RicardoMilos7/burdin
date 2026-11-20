//! `burdin watch [check|build|test|run] <target>` and `--watch` (spec/watch.md): run a command, wait for the files it
//! reads to change, and run it again.
//!
//! The watcher polls file modification times instead of using operating-system notifications, so it needs no
//! dependency and behaves the same on every platform. Each run is the same `burdin` executable started as a child
//! process with the command's own arguments, so a watched command prints exactly what the plain command prints and
//! uses the same caches. `run` builds the program with `burdin build` and keeps the program as a child that the next
//! change kills.

use crate::TempDir;
use crate::package::{self, ModuleFile};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus};
use std::time::{Duration, SystemTime};

/// How often the files are looked at.
const POLL: Duration = Duration::from_millis(50);
/// How long the files must stay unchanged before a rerun starts (the debounce).
const QUIET: Duration = Duration::from_millis(150);
/// The commands `burdin watch` accepts.
const COMMANDS: [&str; 4] = ["check", "build", "test", "run"];
/// The screen is cleared with these ANSI sequences: erase the screen and the scrollback, and go home.
const CLEAR: &str = "\x1b[2J\x1b[3J\x1b[H";

/// `burdin watch [check|build|test|run] <target> [OPTIONS]`. Without a command it watches `check`.
pub(crate) fn run_watch(rest: &[String]) -> ExitCode {
    let (command, args) = match rest.first().map(String::as_str) {
        Some(first) if COMMANDS.contains(&first) => (first, &rest[1..]),
        _ => ("check", rest),
    };
    watch_command(command, args, true)
}

/// Whether `rest` (the arguments of `command`) ask for watching: `--watch` or `--no-clear` before any `--`.
pub(crate) fn asks_for_watch(rest: &[String]) -> bool {
    rest.iter().take_while(|a| *a != "--").any(|a| a == "--watch" || a == "--no-clear")
}

/// `burdin <command> --watch ...`: `--no-clear` alone, without `--watch`, is a usage error.
pub(crate) fn run_flagged(command: &str, rest: &[String]) -> ExitCode {
    let (_, no_clear, watch) = strip_watch_flags(rest);
    if no_clear && !watch {
        return crate::cli::usage_error(command, "`--no-clear` needs `--watch`");
    }
    watch_command(command, rest, false)
}

fn watch_command(command: &str, rest: &[String], standalone: bool) -> ExitCode {
    let (args, no_clear, _) = strip_watch_flags(rest);
    match Plan::new(command, args, no_clear) {
        Ok(plan) => plan.watch(),
        Err(message) => crate::cli::usage_error(if standalone { "watch" } else { command }, &message),
    }
}

/// `rest` without `--watch` and `--no-clear` (before any `--`), and whether each was present.
fn strip_watch_flags(rest: &[String]) -> (Vec<String>, bool, bool) {
    let mut out = Vec::new();
    let (mut no_clear, mut watch) = (false, false);
    let mut options = true;
    for arg in rest {
        if options && arg == "--" {
            options = false;
        }
        if options && arg == "--no-clear" {
            no_clear = true;
        } else if options && arg == "--watch" {
            watch = true;
        } else {
            out.push(arg.clone());
        }
    }
    (out, no_clear, watch)
}

/// Which words of `args` are positional: the target first, then the filters. An option that takes a value consumes
/// the next word. Everything after `--` is the program's.
fn positionals(command: &str, args: &[String]) -> Vec<usize> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            break;
        }
        if arg.starts_with('-') && arg.len() > 1 {
            if !arg.contains('=') && crate::cli::option_takes_value(command, arg) {
                i += 1;
            }
        } else {
            found.push(i);
        }
        i += 1;
    }
    found
}

/// The value of `option` in `args`, as `--option value` or `--option=value`.
fn option_value<'a>(args: &'a [String], option: &str) -> Option<&'a str> {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let mut i = 0;
    while i < end {
        if args[i] == option {
            return args.get(i + 1).map(String::as_str);
        }
        if let Some(value) = args[i].strip_prefix(option).and_then(|r| r.strip_prefix('=')) {
            return Some(value);
        }
        i += 1;
    }
    None
}

/// What to watch and how to run it.
struct Plan {
    command: String,
    /// The command's arguments, without `--watch` and `--no-clear`.
    args: Vec<String>,
    no_clear: bool,
    /// The target as written, and the directory of the package (`None` for a one-file target).
    target: PathBuf,
    package_dir: Option<PathBuf>,
    /// The files the command itself writes, which never trigger a rerun.
    ignore: Vec<PathBuf>,
}

impl Plan {
    fn new(command: &str, args: Vec<String>, no_clear: bool) -> Result<Plan, String> {
        let words = positionals(command, &args);
        let Some(&at) = words.first() else {
            return Err(format!("`watch {command}` needs a file or package directory to watch"));
        };
        let target = PathBuf::from(&args[at]);
        let package_dir = if target.is_dir() {
            if !target.join("burdin.toml").is_file() {
                return Err(format!("`{}` is a directory without a burdin.toml", target.display()));
            }
            Some(absolute(&target))
        } else if target.is_file() {
            None
        } else {
            return Err(format!("`{}` is not a file or a package directory", target.display()));
        };
        let mut ignore = Vec::new();
        for option in ["-o", "--emit-ir", "--emit-deps"] {
            if let Some(value) = option_value(&args, option) {
                ignore.push(absolute(Path::new(value)));
            }
        }
        // `burdin build` writes the executable into the working directory, named after the package or the file.
        let stem = match &package_dir {
            Some(dir) => package::parse_manifest(&dir.join("burdin.toml")).map(|m| m.name).unwrap_or_default(),
            None => target.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        };
        if !stem.is_empty() {
            ignore.push(absolute(Path::new(&stem)));
            ignore.push(absolute(Path::new(&format!("{stem}.exe"))));
        }
        Ok(Plan { command: command.to_string(), args, no_clear, target, package_dir, ignore })
    }

    /// The directories and files watched: the package directory and every path dependency's, or the one file.
    fn roots(&self) -> Vec<PathBuf> {
        let Some(dir) = &self.package_dir else {
            return vec![absolute(&self.target)];
        };
        let mut roots = vec![dir.clone()];
        let mut pending = vec![dir.clone()];
        while let Some(current) = pending.pop() {
            let Ok(manifest) = package::parse_manifest(&current.join("burdin.toml")) else { continue };
            // Every declared path dependency is watched, whatever the build selects: a feature may turn it on.
            for dep in manifest.deps.into_iter().chain(manifest.dev_deps) {
                if let package::DepSource::Path(path) = dep.source {
                    let dep = absolute(&current.join(path));
                    if dep.is_dir() && !roots.iter().any(|r| dep.starts_with(r) || r.starts_with(&dep)) {
                        roots.push(dep.clone());
                        pending.push(dep);
                    }
                }
            }
        }
        roots
    }

    fn snapshot(&self, roots: &[PathBuf], extra: &[PathBuf]) -> Snapshot {
        let mut files = BTreeMap::new();
        for root in roots {
            if root.is_dir() {
                walk(root, true, &self.ignore, &mut files);
            } else {
                stamp(root, &mut files);
            }
        }
        for path in extra {
            if self.ignore.contains(path) {
                continue;
            }
            // A build script may declare a directory input (`spec/build-scripts.md`, BS5); every file under it is
            // watched, like a file input.
            if path.is_dir() {
                walk(path, false, &self.ignore, &mut files);
            } else {
                stamp(path, &mut files);
            }
        }
        files
    }

    fn watch(&self) -> ExitCode {
        let Ok(exe) = std::env::current_exe() else {
            eprintln!("error: cannot find the burdin executable to rerun");
            return ExitCode::from(2);
        };
        let scratch = match TempDir::new() {
            Ok(dir) => dir,
            Err(e) => {
                eprintln!("error: cannot create a temporary directory: {e}");
                return ExitCode::from(2);
            }
        };
        let roots = self.roots();
        let mut extra: Vec<PathBuf> = Vec::new();
        let mut previous = self.snapshot(&roots, &extra);
        let mut changed: Option<Vec<PathBuf>> = None;
        let mut program: Option<Child> = None;
        loop {
            // Edits made while a command runs are seen at the next look, because the baseline is taken first.
            let baseline = previous.clone();
            if !self.no_clear {
                print!("{CLEAR}");
                let _ = std::io::stdout().flush();
            }
            if self.command == "run" {
                self.run_program(&exe, &scratch.path, &mut extra, &mut program);
            } else {
                self.run_command(&exe, &scratch.path, &mut extra, changed.as_deref());
            }
            eprintln!("watch: waiting for changes (Ctrl-C to stop)");
            let mut reported_exit = program.is_none();
            let mut current: Snapshot;
            // Wait for a change, then for the files to settle.
            loop {
                std::thread::sleep(POLL);
                if !reported_exit
                    && let Some(child) = program.as_mut()
                    && let Ok(Some(status)) = child.try_wait()
                {
                    eprintln!("watch: the program exited with {}", describe(status));
                    reported_exit = true;
                }
                let now = self.snapshot(&roots, &extra);
                if now != baseline {
                    current = now;
                    break;
                }
            }
            loop {
                std::thread::sleep(QUIET);
                let now = self.snapshot(&roots, &extra);
                if now == current {
                    break;
                }
                current = now;
            }
            changed = Some(difference(&baseline, &current));
            previous = current;
            if let Some(mut child) = program.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            eprintln!("watch: {} changed, rerunning", describe_changes(changed.as_deref().unwrap_or(&[])));
        }
    }

    /// Runs `check`, `build`, or `test` once; a test run after a change runs the changed modules' tests first.
    fn run_command(&self, exe: &Path, scratch: &Path, extra: &mut Vec<PathBuf>, changed: Option<&[PathBuf]>) -> bool {
        let mut args = self.args.clone();
        let deps_file = scratch.join("deps.txt");
        let wants_deps = self.command == "build" && option_value(&args, "--emit-deps").is_none();
        if wants_deps {
            args.push("--emit-deps".into());
            args.push(deps_file.display().to_string());
        }
        if self.command == "test"
            && let Some(first) = changed.and_then(|c| self.changed_module_filters(c))
        {
            eprintln!("watch: running the tests of {} first", first.join(", "));
            let mut phase = args.clone();
            phase.extend(first);
            if !self.child(exe, "test", &phase) {
                return false;
            }
        }
        let ok = self.child(exe, &self.command, &args);
        if wants_deps {
            self.read_deps(&deps_file, extra);
        }
        ok
    }

    /// Builds the program, then starts it and leaves it running in `program`.
    fn run_program(&self, exe: &Path, scratch: &Path, extra: &mut Vec<PathBuf>, program: &mut Option<Child>) -> bool {
        let split = self.args.iter().position(|a| a == "--").unwrap_or(self.args.len());
        let mut build: Vec<String> = self.args[..split].to_vec();
        let program_args: Vec<OsString> = self.args.iter().skip(split + 1).map(OsString::from).collect();
        let output = scratch.join(format!("program{}", std::env::consts::EXE_SUFFIX));
        let deps_file = scratch.join("deps.txt");
        build.extend(["-o".to_string(), output.display().to_string()]);
        if option_value(&build, "--emit-deps").is_none() {
            build.extend(["--emit-deps".to_string(), deps_file.display().to_string()]);
        }
        let _ = std::fs::remove_file(&output);
        let built = self.child(exe, "build", &build);
        self.read_deps(&deps_file, extra);
        if !built {
            return false;
        }
        match Command::new(&output).args(&program_args).spawn() {
            Ok(child) => {
                *program = Some(child);
                true
            }
            Err(e) => {
                eprintln!("error: cannot run {}: {e}", output.display());
                false
            }
        }
    }

    /// Runs `burdin <command> <args>` to the end and reports how it finished.
    fn child(&self, exe: &Path, command: &str, args: &[String]) -> bool {
        match Command::new(exe).arg(command).args(args).status() {
            Ok(status) => {
                if status.success() {
                    eprintln!("watch: {command} finished");
                } else {
                    eprintln!("watch: {command} failed with {}", describe(status));
                }
                status.success()
            }
            Err(e) => {
                eprintln!("error: cannot run {}: {e}", exe.display());
                false
            }
        }
    }

    /// Adds the files a build read besides the sources (`--emit-deps`) to the watched set.
    fn read_deps(&self, file: &Path, extra: &mut Vec<PathBuf>) {
        let Ok(text) = std::fs::read_to_string(file) else { return };
        extra.clear();
        for line in text.lines().filter(|l| !l.is_empty()) {
            let path = Path::new(line);
            let resolved = match (&self.package_dir, path.is_relative()) {
                (Some(dir), true) if dir.join(path).exists() => dir.join(path),
                _ => absolute(path),
            };
            if resolved.is_file() || resolved.is_dir() {
                extra.push(resolved);
            }
        }
    }

    /// The test filters (`<module>.`) of the changed modules and their dependents, or `None` when the run should
    /// not be split: a one-file target, a test filter on the command line, a change that is not a source of the
    /// entry package, or one that reaches the entry module (its tests have no module prefix to filter by).
    fn changed_module_filters(&self, changed: &[PathBuf]) -> Option<Vec<String>> {
        let dir = self.package_dir.as_ref()?;
        let selecting = ["--filter", "--exact", "--shard", "--failed", "--list", "--doc", "--ignored", "--workspace"];
        if self.args.iter().any(|a| selecting.iter().any(|s| a == s || a.starts_with(&format!("{s}="))))
            || positionals("test", &self.args).len() > 1
        {
            return None;
        }
        let src = dir.join("src");
        let modules = package::discover_modules_under(&src).ok()?;
        let mut seeds = Vec::new();
        for path in changed {
            let relative = path.strip_prefix(&src).ok()?;
            if relative.extension().and_then(|e| e.to_str()) != Some("bd") {
                return None;
            }
            let module = modules.iter().find(|m| absolute(&m.file) == *path)?;
            seeds.push(module.path.clone());
        }
        let affected = dependents(&modules, &seeds);
        if affected.iter().any(|m| m == &["main".to_string()]) {
            return None;
        }
        let mut filters: Vec<String> = affected.iter().map(|m| format!("{}.", m.join("."))).collect();
        filters.sort();
        (!filters.is_empty()).then_some(filters)
    }
}

/// The modules in `seeds` and every module that imports one of them, directly or through another.
fn dependents(modules: &[ModuleFile], seeds: &[Vec<String>]) -> Vec<Vec<String>> {
    let uses: Vec<(Vec<String>, Vec<Vec<String>>)> = modules
        .iter()
        .map(|m| {
            let text = std::fs::read_to_string(&m.file).unwrap_or_default();
            (m.path.clone(), imported_paths(&text))
        })
        .collect();
    let mut affected: BTreeSet<Vec<String>> = seeds.iter().cloned().collect();
    loop {
        let before = affected.len();
        for (module, imports) in &uses {
            if imports.iter().any(|import| affected.iter().any(|a| import.starts_with(a))) {
                affected.insert(module.clone());
            }
        }
        if affected.len() == before {
            return affected.into_iter().collect();
        }
    }
}

/// The dotted paths of the `use` declarations of `text`, one segment list each (`use a.b.{c}` is `["a", "b"]`).
fn imported_paths(text: &str) -> Vec<Vec<String>> {
    let mut paths = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        let line = line.strip_prefix("pub ").unwrap_or(line);
        let Some(rest) = line.strip_prefix("use ") else { continue };
        let path: String =
            rest.trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.').collect();
        let segments: Vec<String> = path.split('.').filter(|s| !s.is_empty()).map(str::to_string).collect();
        if !segments.is_empty() {
            paths.push(segments);
        }
    }
    paths
}

type Snapshot = BTreeMap<PathBuf, (Option<SystemTime>, u64)>;

fn stamp(path: &Path, files: &mut Snapshot) {
    if let Ok(meta) = std::fs::metadata(path) {
        files.insert(path.to_path_buf(), (meta.modified().ok(), meta.len()));
    }
}

/// Records every file under `dir`, leaving out hidden entries (version-control metadata), `target` at the top of a
/// watched directory, `burdin.lock` (the commands rewrite it), and the files in `ignore`.
fn walk(dir: &Path, top: bool, ignore: &[PathBuf], files: &mut Snapshot) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        if name.starts_with('.') || (top && name == "target") || name == "burdin.lock" || ignore.contains(&path) {
            continue;
        }
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => walk(&path, false, ignore, files),
            Ok(_) => stamp(&path, files),
            Err(_) => {}
        }
    }
}

/// The paths added, removed, or modified between two snapshots.
fn difference(before: &Snapshot, after: &Snapshot) -> Vec<PathBuf> {
    let mut paths: BTreeSet<&PathBuf> = BTreeSet::new();
    for (path, stamp) in after {
        if before.get(path) != Some(stamp) {
            paths.insert(path);
        }
    }
    for path in before.keys() {
        if !after.contains_key(path) {
            paths.insert(path);
        }
    }
    paths.into_iter().cloned().collect()
}

fn describe_changes(paths: &[PathBuf]) -> String {
    let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match paths {
        [] => "files".to_string(),
        [one] => name(one),
        [first, rest @ ..] => format!("{} and {} more", name(first), rest.len()),
    }
}

fn describe(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit status {code}"),
        None => "a signal".to_string(),
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(w: &[&str]) -> Vec<String> {
        w.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_watch_flags_are_removed_before_the_separator_only() {
        let (args, no_clear, watch) = strip_watch_flags(&words(&["a.bd", "--watch", "--no-clear", "--", "--watch"]));
        assert_eq!(args, words(&["a.bd", "--", "--watch"]));
        assert!(no_clear && watch);
        assert!(asks_for_watch(&words(&["a.bd", "--watch"])));
        assert!(!asks_for_watch(&words(&["a.bd", "--", "--watch"])));
    }

    #[test]
    fn positionals_skip_option_values_and_the_program_arguments() {
        let args = words(&["pkg", "-j", "2", "--release", "geometry", "--", "x"]);
        assert_eq!(positionals("test", &args), vec![0, 4]);
        assert_eq!(option_value(&words(&["a", "-o", "out"]), "-o"), Some("out"));
        assert_eq!(option_value(&words(&["a", "--emit-ir=x.ll"]), "--emit-ir"), Some("x.ll"));
        assert_eq!(option_value(&words(&["a", "--", "-o", "out"]), "-o"), None);
    }

    #[test]
    fn imports_are_read_as_module_paths() {
        let text = "use text.{shout}\npub use geometry.shapes.area\n  use std.fmt as f\nfn use_it() {}\n";
        assert_eq!(
            imported_paths(text),
            vec![words(&["text"]), words(&["geometry", "shapes", "area"]), words(&["std", "fmt"])]
        );
    }

    #[test]
    fn dependents_follow_imports_transitively() {
        let dir = std::env::temp_dir().join(format!("burdin-watch-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let module = |name: &str, text: &str| {
            let file = dir.join(format!("{name}.bd"));
            std::fs::write(&file, text).unwrap();
            ModuleFile { path: vec![name.to_string()], file, text: None }
        };
        let modules = [module("a", ""), module("b", "use a.f\n"), module("c", "use b.g\n"), module("d", "")];
        let affected = dependents(&modules, &[words(&["a"])]);
        assert_eq!(affected, vec![words(&["a"]), words(&["b"]), words(&["c"])]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_snapshot_skips_target_hidden_entries_and_ignored_files() {
        let dir = std::env::temp_dir().join(format!("burdin-watch-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        for file in ["burdin.toml", "burdin.lock", "src/main.bd", "target/x", ".git/y", "app"] {
            std::fs::write(dir.join(file), "x").unwrap();
        }
        let mut files = Snapshot::new();
        walk(&dir, true, &[dir.join("app")], &mut files);
        let names: Vec<String> =
            files.keys().map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(names, vec!["burdin.toml".to_string(), "src/main.bd".to_string()]);
        let mut changed = files.clone();
        changed.insert(dir.join("src/new.bd"), (None, 1));
        assert_eq!(difference(&files, &changed), vec![dir.join("src/new.bd")]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
