//! `--sanitize` (`spec/sanitizers.md`): which sanitizers a build asks for, whether this platform has them, and the
//! LLVM compiler driver that links their runtime libraries. `burdin fuzz` links libFuzzer the same way.

use burdin_codegen::Sanitizers;
use std::path::PathBuf;
use std::process::Command;

/// Parses the value of `--sanitize`: a comma-separated list of `address`, `undefined`, `thread`, and `leak`, each at
/// most once. ThreadSanitizer keeps its own shadow memory, so it combines with neither `address` nor `leak`
/// (SAN1).
pub(crate) fn parse(value: &str) -> Result<Sanitizers, String> {
    let mut sanitizers = Sanitizers::default();
    for name in value.split(',') {
        let slot = match name.trim() {
            "address" => &mut sanitizers.address,
            "undefined" => &mut sanitizers.undefined,
            "thread" => &mut sanitizers.thread,
            "leak" => &mut sanitizers.leak,
            "" => return Err("`--sanitize` needs a list such as `address` or `address,undefined`".to_string()),
            other => {
                return Err(format!(
                    "unknown sanitizer `{other}`; `--sanitize` takes `address`, `undefined`, `thread`, and `leak`"
                ));
            }
        };
        if *slot {
            return Err(format!("`{}` is named twice in `--sanitize`", name.trim()));
        }
        *slot = true;
    }
    if sanitizers.thread && (sanitizers.address || sanitizers.leak) {
        return Err(
            "`thread` cannot be combined with `address` or `leak`: ThreadSanitizer has its own runtime".to_string()
        );
    }
    Ok(sanitizers)
}

/// Whether this platform supports every sanitizer asked for (SAN2): Linux has all four and the fuzzer; macOS and
/// Windows have `undefined` only, which is the checks in the generated code and needs no runtime library. A sanitizer
/// runtime that cannot start is worse than no sanitizer, and Apple's runtime does not: its search for shadow memory
/// never finishes on the macOS runners, so every instrumented program hangs before `main` (SAN2, and the known
/// limitations in `docs/platform-support.md`). `os` and `arch` are `std::env::consts`'s spellings.
pub(crate) fn check_platform(sanitizers: &Sanitizers, os: &str, arch: &str) -> Result<(), String> {
    let wanted = [
        ("address", sanitizers.address),
        ("thread", sanitizers.thread),
        ("leak", sanitizers.leak),
        ("undefined", sanitizers.undefined),
        ("fuzzer", sanitizers.fuzzer),
    ];
    let supported_arch = matches!(arch, "x86_64" | "aarch64");
    for (name, on) in wanted {
        if !on {
            continue;
        }
        let supported = match (os, name) {
            (_, "undefined") => true,
            ("linux", _) => supported_arch,
            _ => false,
        };
        if !supported {
            let what =
                if name == "fuzzer" { "`burdin fuzz` (libFuzzer)".to_string() } else { format!("`--sanitize={name}`") };
            return Err(format!(
                "{what} is not supported on {os}-{arch}; see the sanitizer table in docs/platform-support.md"
            ));
        }
    }
    Ok(())
}

/// The compiler driver that links a sanitizer runtime: `BURDIN_CC` when it names clang, otherwise `clang-21` or
/// `clang` from `LLVM_SYS_211_PREFIX/bin` or `PATH`. The runtimes are LLVM's (compiler-rt), so another C compiler
/// would link a mismatched or missing runtime (SAN3).
pub(crate) fn linker() -> Result<String, String> {
    if let Ok(cc) = std::env::var("BURDIN_CC") {
        let name = cc.rsplit(['/', '\\']).next().unwrap_or(&cc).to_ascii_lowercase();
        if name.starts_with("clang") && !name.starts_with("clang-cl") {
            return Ok(cc);
        }
        return Err(format!(
            "the sanitizer runtimes are linked by clang, and BURDIN_CC is `{cc}`; set BURDIN_CC to clang or unset it"
        ));
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(prefix) = std::env::var_os("LLVM_SYS_211_PREFIX") {
        let bin = PathBuf::from(prefix).join("bin");
        candidates.push(bin.join("clang-21"));
        candidates.push(bin.join("clang"));
    }
    candidates.push(PathBuf::from("clang-21"));
    candidates.push(PathBuf::from("clang"));
    candidates
        .into_iter()
        .find(|candidate| Command::new(candidate).arg("--version").output().is_ok_and(|out| out.status.success()))
        .map(|path| path.display().to_string())
        .ok_or_else(|| {
            "the sanitizer runtimes are linked by clang, and neither `clang-21` nor `clang` was found; install \
             clang 21 (with compiler-rt) or set BURDIN_CC to it"
                .to_string()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_parse_and_conflicts_are_rejected() {
        let both = parse("address,undefined").unwrap();
        assert!(both.address && both.undefined && !both.thread && !both.leak);
        assert!(parse("thread").unwrap().thread);
        assert!(parse("leak, address").unwrap().leak);
        assert!(parse("memory").unwrap_err().contains("unknown sanitizer `memory`"));
        assert!(parse("address,address").unwrap_err().contains("named twice"));
        assert!(parse("thread,address").unwrap_err().contains("cannot be combined"));
        assert!(parse("thread,leak").unwrap_err().contains("cannot be combined"));
        assert!(parse("").is_err());
    }

    #[test]
    fn the_platform_table_matches_the_documentation() {
        let all = |name: &str| parse(name).unwrap();
        for name in ["address", "thread", "leak", "undefined"] {
            assert!(check_platform(&all(name), "linux", "x86_64").is_ok(), "{name}");
            assert!(check_platform(&all(name), "linux", "aarch64").is_ok(), "{name}");
        }
        assert!(check_platform(&all("undefined"), "macos", "aarch64").is_ok());
        for name in ["address", "thread", "leak"] {
            let error = check_platform(&all(name), "macos", "aarch64").unwrap_err();
            assert!(error.contains(&format!("`--sanitize={name}` is not supported on macos-aarch64")), "{error}");
        }
        assert!(check_platform(&all("undefined"), "windows", "x86_64").is_ok());
        for name in ["address", "thread", "leak"] {
            let error = check_platform(&all(name), "windows", "x86_64").unwrap_err();
            assert!(error.contains(&format!("`--sanitize={name}` is not supported on windows-x86_64")), "{error}");
        }
        let fuzz = Sanitizers { fuzzer: true, ..Sanitizers::default() };
        assert!(check_platform(&fuzz, "linux", "x86_64").is_ok());
        assert!(check_platform(&fuzz, "windows", "x86_64").unwrap_err().contains("`burdin fuzz` (libFuzzer)"));
        assert!(check_platform(&fuzz, "macos", "aarch64").unwrap_err().contains("`burdin fuzz` (libFuzzer)"));
    }

    #[test]
    fn the_link_list_names_what_needs_a_runtime() {
        let s = Sanitizers { address: true, undefined: true, fuzzer: true, ..Sanitizers::default() };
        assert_eq!(s.link_list(), "fuzzer,address");
        assert!(s.instruments() && s.needs_runtime());
        let undefined = Sanitizers { undefined: true, ..Sanitizers::default() };
        assert!(undefined.instruments() && !undefined.needs_runtime());
        let leak = Sanitizers { leak: true, ..Sanitizers::default() };
        assert!(!leak.instruments() && leak.needs_runtime());
    }
}
