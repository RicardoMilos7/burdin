//! Named build profiles: the settings a build compiles with (`spec/profiles.md`, issue #339).
//!
//! Four profiles are built in: `dev` (the default of `build`, `run`, `check`, `fuzz`, and `debug`), `release`
//! (`--release`, `--profile release`), `test` (the default of `burdin test`, inheriting `dev`), and `bench` (the
//! default of `burdin bench`, inheriting `release`). A package may customize any of them in its `burdin.toml`, and
//! may define another profile that inherits one of them:
//!
//! ```toml
//! [profile.release]
//! opt-level = 3
//!
//! [profile.fast]
//! inherits = "release"
//! overflow-checks = true
//!
//! [profile.release.package.crypto]
//! overflow-checks = true
//! ```
//!
//! Only the entry package's tables are read; a dependency's `[profile...]` tables are ignored, as in Cargo. The
//! driver resolves the selected profile once, before the build, and every later stage reads the resolved value.

use crate::cli;
use crate::package;
use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;

pub(crate) use burdin_codegen::OptLevel;

/// How a profile ends a panic. Burdin panics end the process (the runtime prints the message and exits), so
/// `abort` is the only strategy there is; `unwind` is rejected rather than silently accepted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PanicStrategy {
    #[default]
    Abort,
}

/// Per-package overrides of a profile (`[profile.<name>.package.<package>]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PackageOverride {
    pub(crate) package: String,
    pub(crate) overflow_checks: Option<bool>,
    pub(crate) debug_assertions: Option<bool>,
}

/// A resolved profile: every setting a build reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Profile {
    /// The profile's name, as `--profile` spells it: `dev`, `release`, `test`, `bench`, or a custom name.
    pub(crate) name: String,
    /// The directory component under `target/burdin/`: `debug` for `dev` and `test`, `release` for `release` and
    /// `bench`, and the profile's own name for a custom one.
    pub(crate) dir: String,
    pub(crate) opt_level: OptLevel,
    /// Emit source-level debug information (`-g`) for this profile.
    pub(crate) debug: bool,
    /// Keep `debug_assert*` and the error trace frames.
    pub(crate) debug_assertions: bool,
    /// Panic on integer overflow (and on `MIN / -1`).
    pub(crate) overflow_checks: bool,
    pub(crate) panic: PanicStrategy,
    /// The per-package overrides, with the selected profile's values already filled in.
    pub(crate) packages: Vec<PackageOverride>,
}

impl Default for Profile {
    fn default() -> Profile {
        builtin("dev").expect("dev is a built-in profile")
    }
}

impl Profile {
    /// The `dev` profile: no optimization and no debug information; debug assertions and overflow checks on.
    pub(crate) fn dev() -> Profile {
        Profile::default()
    }

    /// The `release` profile: `-O2`, no debug info or assertions, and wrapping arithmetic.
    pub(crate) fn release() -> Profile {
        builtin("release").expect("release is a built-in profile")
    }

    /// Replaces the optimization level with a `-O` option's (`spec/profiles.md`, BP4). The overflow and assertion
    /// checks stay as the profile sets them: the level and the checks are independent (BP5).
    pub(crate) fn with_opt_level(mut self, level: Option<OptLevel>) -> Profile {
        if let Some(level) = level {
            self.opt_level = level;
        }
        self
    }

    /// The settings that change the generated code, as the build-script cache key spells them. Two profiles with
    /// the same settings share the key (so `dev` and `test` do not rerun a script for no reason).
    pub(crate) fn key(&self) -> String {
        format!(
            "opt={};debug={};debug-assertions={};overflow-checks={};panic={}",
            self.opt_level.number(),
            self.debug,
            self.debug_assertions,
            self.overflow_checks,
            match self.panic {
                PanicStrategy::Abort => "abort",
            }
        )
    }

    /// The overflow and debug-assertion settings that apply to `package`, falling back to the profile's.
    pub(crate) fn package_flags(&self, package: &str) -> (bool, bool) {
        match self.packages.iter().find(|over| over.package == package) {
            Some(over) => (
                over.overflow_checks.unwrap_or(self.overflow_checks),
                over.debug_assertions.unwrap_or(self.debug_assertions),
            ),
            None => (self.overflow_checks, self.debug_assertions),
        }
    }
}

/// A profile that cannot be resolved. A name the package does not define is a usage error; a malformed table is a
/// manifest error and is reported at its line.
#[derive(Debug)]
pub(crate) enum Failure {
    Usage(String),
    Manifest(String),
}

/// The level a `-O` option names, `0` to `3` (`spec/profiles.md`, BP4). `-O2` and `-O 2` name the same level.
pub(crate) fn parse_opt_level(value: &str) -> Result<OptLevel, String> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .and_then(OptLevel::from_number)
        .ok_or_else(|| format!("`-O` needs an optimization level from 0 to 3, got `{}`", value.trim()))
}

/// Records a `-O` option's level in `slot`. A second `-O` that names another level is a usage error, as
/// `--release` with another `--profile` is (BP4); the same level twice is accepted.
pub(crate) fn set_opt_level(slot: &mut Option<OptLevel>, level: OptLevel) -> Result<(), String> {
    match *slot {
        Some(previous) if previous != level => {
            Err(format!("`-O{}` conflicts with `-O{}`: give one optimization level", level.number(), previous.number()))
        }
        _ => {
            *slot = Some(level);
            Ok(())
        }
    }
}

/// Resolves the profile a command builds with, printing and returning the exit status when it cannot.
///
/// `requested` is `--profile` (or `--release`, which is `--profile release`); `default_name` is the command's
/// default (`dev`, `test`, or `bench`); `opt_level` is the `-O` option, which replaces the profile's level.
pub(crate) fn resolve_for_command(
    command: &str,
    target: &Path,
    requested: Option<&str>,
    default_name: &str,
    opt_level: Option<OptLevel>,
) -> Result<Profile, ExitCode> {
    match resolve(target, requested, default_name) {
        Ok(profile) => Ok(profile.with_opt_level(opt_level)),
        Err(Failure::Usage(message)) => Err(cli::usage_error(command, &message)),
        Err(Failure::Manifest(message)) => {
            eprintln!("error: {message}");
            Err(ExitCode::from(1))
        }
    }
}

/// Resolves a profile from `target`'s `burdin.toml` (when it has one) and the built-in defaults.
pub(crate) fn resolve(target: &Path, requested: Option<&str>, default_name: &str) -> Result<Profile, Failure> {
    let (path, entries) = manifest_entries(target)?;
    let selected = requested.unwrap_or(default_name);
    let mut defs: HashMap<String, Vec<&package::ProfileEntry>> = HashMap::new();
    let mut defined: Vec<String> = Vec::new();
    for entry in &entries {
        let (name, _) = section_parts(&entry.section, entry.line, &path)?;
        match defs.entry(name) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                defined.push(slot.key().clone());
                slot.insert(vec![entry]);
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => slot.get_mut().push(entry),
        }
    }
    if !is_builtin(selected) && !defs.contains_key(selected) {
        let mut known: Vec<&str> = BUILT_INS.to_vec();
        known.extend(defined.iter().map(String::as_str).filter(|name| !is_builtin(name)));
        known.sort_unstable();
        return Err(Failure::Usage(format!(
            "unknown profile `{selected}` (the profiles are {}; `[profile.{selected}]` in burdin.toml defines \
             another)",
            known.join(", ")
        )));
    }
    let mut cache: HashMap<String, Profile> = HashMap::new();
    let mut stack: Vec<String> = Vec::new();
    // Every table the manifest writes is validated, not only the selected one, so a typo in an unused profile is
    // reported instead of waiting for the build that selects it. The order is the file's, so the first error is
    // deterministic.
    for name in &defined {
        resolve_named(name, &defs, &path, &mut cache, &mut stack)?;
    }
    resolve_named(selected, &defs, &path, &mut cache, &mut stack)
}

/// The built-in profile names.
const BUILT_INS: [&str; 4] = ["dev", "release", "test", "bench"];

fn is_builtin(name: &str) -> bool {
    BUILT_INS.contains(&name)
}

/// A built-in profile with its factory settings, before the package's table customizes it.
fn builtin(name: &str) -> Option<Profile> {
    let (opt_level, debug, debug_assertions, overflow_checks) = match name {
        // `debug` is off in `dev` too: `-g` remains how a build asks for debug information, and a profile that
        // wants it sets `debug = true` (`spec/profiles.md`, BP1).
        "dev" | "test" => (OptLevel::None, false, true, true),
        "release" | "bench" => (OptLevel::O2, false, false, false),
        _ => return None,
    };
    Some(Profile {
        name: name.to_string(),
        dir: dir_for(name).to_string(),
        opt_level,
        debug,
        debug_assertions,
        overflow_checks,
        panic: PanicStrategy::Abort,
        packages: Vec::new(),
    })
}

/// The directory under `target/burdin/` a profile writes to. `dev` and `test` share `debug`, `release` and
/// `bench` share `release`, and a custom profile gets its own name, as Cargo does.
fn dir_for(name: &str) -> &str {
    match name {
        "dev" | "test" => "debug",
        "release" | "bench" => "release",
        _ => name,
    }
}

/// The manifest path (for diagnostics) and its profile entries. A single-file target has no manifest.
fn manifest_entries(target: &Path) -> Result<(String, Vec<package::ProfileEntry>), Failure> {
    let manifest = target.join("burdin.toml");
    if !manifest.is_file() {
        return Ok((manifest.display().to_string(), Vec::new()));
    }
    // A workspace root declares profiles for its members and has no `[package]`, so only the profile tables are read.
    match package::manifest_profiles(&manifest) {
        Ok(profiles) => Ok((manifest.display().to_string(), profiles)),
        Err(message) => Err(Failure::Manifest(message)),
    }
}

/// `profile.<name>` or `profile.<name>.package.<package>`: the profile name and the package an override names.
fn section_parts(section: &str, line: usize, path: &str) -> Result<(String, Option<String>), Failure> {
    let parts: Vec<&str> = section.split('.').collect();
    let bad = || {
        Failure::Manifest(format!(
            "{path}:{line}: `[{section}]` is not a profile section (`[profile.<name>]` or `[profile.<name>.package.<package>]`)"
        ))
    };
    match parts.as_slice() {
        ["profile", name] => {
            check_name(name, line, path, section)?;
            Ok((name.to_string(), None))
        }
        ["profile", name, "package", package] => {
            check_name(name, line, path, section)?;
            if !package.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                || !package.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return Err(Failure::Manifest(format!(
                    "{path}:{line}: `{package}` is not a package name in `[{section}]`"
                )));
            }
            Ok((name.to_string(), Some(package.to_string())))
        }
        _ => Err(bad()),
    }
}

fn check_name(name: &str, line: usize, path: &str, section: &str) -> Result<(), Failure> {
    let valid = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if valid {
        Ok(())
    } else {
        Err(Failure::Manifest(format!("{path}:{line}: `{name}` is not a profile name in `[{section}]`")))
    }
}

/// Resolves one profile, inheriting its parent when it is custom, and caches the result.
fn resolve_named(
    name: &str,
    defs: &HashMap<String, Vec<&package::ProfileEntry>>,
    path: &str,
    cache: &mut HashMap<String, Profile>,
    stack: &mut Vec<String>,
) -> Result<Profile, Failure> {
    if let Some(profile) = cache.get(name) {
        return Ok(profile.clone());
    }
    if stack.iter().any(|open| open == name) {
        let chain: Vec<&str> = stack.iter().map(String::as_str).chain([name]).collect();
        return Err(Failure::Manifest(format!("{path}: a profile inheritance cycle: {}", chain.join(" -> "))));
    }
    let entries = defs.get(name).cloned().unwrap_or_default();
    let mut profile = match builtin(name) {
        Some(profile) => {
            if entries.iter().any(|entry| entry.key == "inherits") {
                let entry = entries.iter().find(|entry| entry.key == "inherits").expect("just found");
                return Err(Failure::Manifest(format!(
                    "{path}:{}: `{name}` is a built-in profile; it already has its parent and cannot inherit \
                     another",
                    entry.line
                )));
            }
            profile
        }
        None => {
            if !defs.contains_key(name) {
                return Err(Failure::Usage(format!("unknown profile `{name}`")));
            }
            let inherits = entries.iter().find(|entry| entry.key == "inherits");
            let Some(inherits) = inherits else {
                return Err(Failure::Manifest(format!(
                    "{path}: the profile `{name}` needs `inherits = \"dev\"` (or another profile)"
                )));
            };
            let parent = parse_string(&inherits.value)
                .map_err(|e| Failure::Manifest(format!("{path}:{}: {e}", inherits.line)))?;
            if !is_builtin(&parent) && !defs.contains_key(&parent) {
                return Err(Failure::Manifest(format!(
                    "{path}:{}: `{name}` inherits `{parent}`, which is not a profile",
                    inherits.line
                )));
            }
            stack.push(name.to_string());
            let mut profile = resolve_named(&parent, defs, path, cache, stack)?;
            stack.pop();
            profile.name = name.to_string();
            profile.dir = dir_for(name).to_string();
            profile
        }
    };
    for entry in &entries {
        if entry.key == "inherits" {
            if is_builtin(name) {
                // Reported above, before any other entry is read.
                unreachable!("a built-in profile's inherits was rejected");
            }
            continue;
        }
        let (_, package) = section_parts(&entry.section, entry.line, path)?;
        apply(&mut profile, entry, package.as_deref(), path)?;
    }
    cache.insert(name.to_string(), profile.clone());
    Ok(profile)
}

/// Applies one entry of a profile table, validating its key and value.
fn apply(
    profile: &mut Profile,
    entry: &package::ProfileEntry,
    package: Option<&str>,
    path: &str,
) -> Result<(), Failure> {
    let where_ = || format!("{path}:{}", entry.line);
    if let Some(package) = package {
        // The key decides first, so `opt-level = 3` under a package gets the explanation rather than a type error.
        let value = match entry.key.as_str() {
            "overflow-checks" | "debug-assertions" => {
                parse_bool(&entry.value).map_err(|e| Failure::Manifest(format!("{}: {e}", where_())))?
            }
            // The program is linked as one binary, so these are properties of the whole build, not of a package
            // in it (`spec/profiles.md`, BP7).
            "opt-level" | "debug" | "panic" => {
                return Err(Failure::Manifest(format!(
                    "{}: `{}` is a setting of the whole build; `[{}]` can override only `overflow-checks` and \
                     `debug-assertions`",
                    where_(),
                    entry.key,
                    entry.section
                )));
            }
            other => {
                return Err(Failure::Manifest(format!(
                    "{}: `{other}` is not a profile setting (`overflow-checks` and `debug-assertions` apply to one \
                     package)",
                    where_()
                )));
            }
        };
        let over = match profile.packages.iter_mut().find(|over| over.package == package) {
            Some(over) => over,
            None => {
                profile.packages.push(PackageOverride {
                    package: package.to_string(),
                    overflow_checks: None,
                    debug_assertions: None,
                });
                profile.packages.last_mut().expect("just pushed")
            }
        };
        match entry.key.as_str() {
            "overflow-checks" => over.overflow_checks = Some(value),
            "debug-assertions" => over.debug_assertions = Some(value),
            _ => unreachable!("the key was checked above"),
        }
        return Ok(());
    }
    match entry.key.as_str() {
        "inherits" => {}
        "opt-level" => {
            let number: i64 = entry
                .value
                .trim()
                .parse()
                .map_err(|_| Failure::Manifest(format!("{}: `opt-level` needs a number from 0 to 3", where_())))?;
            profile.opt_level = OptLevel::from_number(number).ok_or_else(|| {
                Failure::Manifest(format!("{}: `opt-level` is 0, 1, 2, or 3; got `{number}`", where_()))
            })?;
        }
        "debug" => {
            profile.debug = parse_bool(&entry.value).map_err(|e| Failure::Manifest(format!("{}: {e}", where_())))?
        }
        "debug-assertions" => {
            profile.debug_assertions =
                parse_bool(&entry.value).map_err(|e| Failure::Manifest(format!("{}: {e}", where_())))?
        }
        "overflow-checks" => {
            profile.overflow_checks =
                parse_bool(&entry.value).map_err(|e| Failure::Manifest(format!("{}: {e}", where_())))?
        }
        "panic" => {
            let value = parse_string(&entry.value).map_err(|e| Failure::Manifest(format!("{}: {e}", where_())))?;
            if value != "abort" {
                return Err(Failure::Manifest(format!(
                    "{}: `panic = \"{value}\"` is not supported: Burdin panics end the process, so the only \
                     strategy is `abort`",
                    where_()
                )));
            }
            profile.panic = PanicStrategy::Abort;
        }
        other => {
            return Err(Failure::Manifest(format!(
                "{}: `{other}` is not a profile setting (`opt-level`, `debug`, `debug-assertions`, \
                 `overflow-checks`, `panic`)",
                where_()
            )));
        }
    }
    Ok(())
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("expected `true` or `false`, got `{other}`")),
    }
}

/// A `"..."` string with the `\"` and `\\` escapes.
fn parse_string(value: &str) -> Result<String, String> {
    let value = value.trim();
    if !(value.starts_with('"') && value.ends_with('"') && value.len() >= 2) {
        return Err(format!("expected a quoted string, got `{value}`"));
    }
    let mut out = String::new();
    let mut chars = value[1..value.len() - 1].chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => return Err(format!("unknown escape `\\{other}`")),
                None => return Err("a string ends with a backslash".to_string()),
            },
            other => out.push(other),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(section: &str, key: &str, value: &str, line: usize) -> package::ProfileEntry {
        package::ProfileEntry { section: section.to_string(), key: key.to_string(), value: value.to_string(), line }
    }

    #[test]
    fn the_builtins_have_the_documented_settings() {
        let dev = Profile::dev();
        assert_eq!(
            (dev.opt_level, dev.debug, dev.debug_assertions, dev.overflow_checks),
            (OptLevel::None, false, true, true)
        );
        let release = Profile::release();
        assert_eq!(
            (release.opt_level, release.debug, release.debug_assertions, release.overflow_checks),
            (OptLevel::O2, false, false, false)
        );
        assert_eq!(dev.key(), builtin("test").unwrap().key());
        assert_eq!(release.key(), builtin("bench").unwrap().key());
        assert_eq!((dev.dir.as_str(), release.dir.as_str()), ("debug", "release"));
    }

    #[test]
    fn a_custom_profile_inherits_and_overrides() {
        let entries = vec![
            entry("profile.fast", "inherits", "\"release\"", 2),
            entry("profile.fast", "overflow-checks", "true", 3),
            entry("profile.fast.package.crypto", "debug-assertions", "true", 6),
        ];
        let mut defs: HashMap<String, Vec<&package::ProfileEntry>> = HashMap::new();
        for entry in &entries {
            let name = entry.section.split('.').nth(1).unwrap();
            defs.entry(name.to_string()).or_default().push(entry);
        }
        let mut cache = HashMap::new();
        let profile = resolve_named("fast", &defs, "burdin.toml", &mut cache, &mut Vec::new()).unwrap();
        assert_eq!(
            (
                profile.name.as_str(),
                profile.dir.as_str(),
                profile.opt_level,
                profile.overflow_checks,
                profile.debug_assertions
            ),
            ("fast", "fast", OptLevel::O2, true, false)
        );
        assert_eq!(profile.package_flags("crypto"), (true, true));
        assert_eq!(profile.package_flags("other"), (true, false));
    }

    #[test]
    fn an_inheritance_cycle_is_reported() {
        let entries = vec![entry("profile.a", "inherits", "\"b\"", 1), entry("profile.b", "inherits", "\"a\"", 2)];
        let mut defs: HashMap<String, Vec<&package::ProfileEntry>> = HashMap::new();
        for entry in &entries {
            let name = entry.section.split('.').nth(1).unwrap();
            defs.entry(name.to_string()).or_default().push(entry);
        }
        let mut cache = HashMap::new();
        let error = resolve_named("a", &defs, "burdin.toml", &mut cache, &mut Vec::new()).unwrap_err();
        match error {
            Failure::Manifest(message) => assert!(message.contains("cycle"), "{message}"),
            Failure::Usage(message) => panic!("a cycle is not a usage error: {message}"),
        }
    }

    #[test]
    fn opt_levels_are_zero_to_three() {
        assert_eq!(OptLevel::from_number(0), Some(OptLevel::None));
        assert_eq!(OptLevel::from_number(3), Some(OptLevel::O3));
        assert_eq!(OptLevel::from_number(4), None);
        assert_eq!(OptLevel::O3.pipeline(), "default<O3>");
        assert!(!OptLevel::None.optimized());
    }

    #[test]
    fn a_dash_o_option_names_a_level_and_replaces_only_the_level() {
        assert_eq!(parse_opt_level("2"), Ok(OptLevel::O2));
        assert_eq!(parse_opt_level(" 0 "), Ok(OptLevel::None));
        assert!(parse_opt_level("4").unwrap_err().contains("needs an optimization level"));
        assert!(parse_opt_level("x").unwrap_err().contains("got `x`"));

        let mut slot = None;
        set_opt_level(&mut slot, OptLevel::O1).unwrap();
        set_opt_level(&mut slot, OptLevel::O1).unwrap();
        let conflict = set_opt_level(&mut slot, OptLevel::O2).unwrap_err();
        assert_eq!(conflict, "`-O2` conflicts with `-O1`: give one optimization level");

        // The level changes; the checks and debug information stay as the profile sets them (BP5).
        let dev = Profile::dev().with_opt_level(Some(OptLevel::O3));
        assert_eq!(
            (dev.opt_level, dev.debug, dev.debug_assertions, dev.overflow_checks),
            (OptLevel::O3, false, true, true)
        );
        let release = Profile::release().with_opt_level(Some(OptLevel::None));
        assert_eq!(
            (release.opt_level, release.debug, release.debug_assertions, release.overflow_checks),
            (OptLevel::None, false, false, false)
        );
        assert_eq!(Profile::release().with_opt_level(None), Profile::release());
    }
}
