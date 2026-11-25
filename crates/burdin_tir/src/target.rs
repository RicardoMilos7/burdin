//! The platform a program is compiled for (`spec/cross-compilation.md`, XC1-XC3).
//!
//! By default it is the machine the compiler runs on. `burdin build --target <triple>` names another one, and the
//! driver records it here once, before the front end starts: the compile-time target facts (`target.os()`,
//! `target.arch()`, the `cfg(...)` predicates of a manifest), the code generator's target machine, and the runtime's
//! system constants all read it from this module, so they cannot disagree about which platform is being built for.
//! Nothing here depends on LLVM.

use std::sync::atomic::{AtomicU8, Ordering};

/// An operating system a program can be built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
}

/// A processor architecture a program can be built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Arch {
    X86_64,
    Aarch64,
}

/// An operating system and an architecture: one of the six platforms the compiler supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    pub os: Os,
    pub arch: Arch,
}

impl Target {
    /// The six supported platforms, with the triple that names each.
    pub const SUPPORTED: [(Target, &'static str); 6] = [
        (Target { os: Os::Linux, arch: Arch::X86_64 }, "x86_64-unknown-linux-gnu"),
        (Target { os: Os::Linux, arch: Arch::Aarch64 }, "aarch64-unknown-linux-gnu"),
        (Target { os: Os::MacOs, arch: Arch::X86_64 }, "x86_64-apple-darwin"),
        (Target { os: Os::MacOs, arch: Arch::Aarch64 }, "aarch64-apple-darwin"),
        (Target { os: Os::Windows, arch: Arch::X86_64 }, "x86_64-pc-windows-msvc"),
        (Target { os: Os::Windows, arch: Arch::Aarch64 }, "aarch64-pc-windows-msvc"),
    ];

    /// The machine this compiler runs on.
    pub fn host() -> Target {
        let os = if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        };
        let arch = if cfg!(target_arch = "aarch64") { Arch::Aarch64 } else { Arch::X86_64 };
        Target { os, arch }
    }

    /// The platform a triple names: exactly one of the six `SUPPORTED` spellings.
    pub fn parse(triple: &str) -> Result<Target, String> {
        if let Some((target, _)) = Self::SUPPORTED.iter().find(|(_, name)| *name == triple) {
            return Ok(*target);
        }
        let list: Vec<&str> = Self::SUPPORTED.iter().map(|(_, name)| *name).collect();
        Err(format!("`{triple}` is not a target this compiler supports; the targets are {}", list.join(", ")))
    }

    /// The triple that names this platform.
    pub fn triple(self) -> &'static str {
        Self::SUPPORTED.iter().find(|(target, _)| *target == self).map_or("", |(_, name)| name)
    }

    /// `linux`, `macos`, or `windows`, as `target.os()` and `target_os = "..."` spell it.
    pub fn os_name(self) -> &'static str {
        match self.os {
            Os::Linux => "linux",
            Os::MacOs => "macos",
            Os::Windows => "windows",
        }
    }

    /// `x86_64` or `aarch64`, as `target.arch()` and `target_arch = "..."` spell it.
    pub fn arch_name(self) -> &'static str {
        match self.arch {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }

    fn index(self) -> u8 {
        Self::SUPPORTED.iter().position(|(target, _)| *target == self).map_or(0, |i| i as u8 + 1)
    }
}

/// 0 when no target was named (the host), otherwise the index into `Target::SUPPORTED` plus one.
static EXPLICIT: AtomicU8 = AtomicU8::new(0);

/// Records the platform to build for, or the host again with `None`. The driver calls this once per command, before
/// compiling; a build script is compiled for the host, so it resets the target around that compile.
pub fn set(target: Option<Target>) {
    EXPLICIT.store(target.map_or(0, Target::index), Ordering::SeqCst);
}

/// The platform named with `--target`, or `None` for the host.
pub fn explicit() -> Option<Target> {
    match EXPLICIT.load(Ordering::SeqCst) {
        0 => None,
        n => Some(Target::SUPPORTED[usize::from(n) - 1].0),
    }
}

/// The platform programs are compiled for now.
pub fn current() -> Target {
    explicit().unwrap_or_else(Target::host)
}

/// Whether the current target is not the machine the compiler runs on.
pub fn is_cross() -> bool {
    explicit().is_some_and(|target| target != Target::host())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_triple_round_trips() {
        for (target, triple) in Target::SUPPORTED {
            assert_eq!(Target::parse(triple), Ok(target));
            assert_eq!(target.triple(), triple);
        }
    }

    #[test]
    fn an_unknown_triple_lists_the_supported_ones() {
        let error = Target::parse("riscv64-unknown-linux-gnu").unwrap_err();
        assert!(error.contains("`riscv64-unknown-linux-gnu` is not a target"));
        assert!(error.contains("aarch64-apple-darwin"));
        assert!(Target::parse("x86_64-linux").is_err());
    }

    #[test]
    fn the_names_match_the_target_facts() {
        let linux = Target::parse("aarch64-unknown-linux-gnu").unwrap();
        assert_eq!((linux.os_name(), linux.arch_name()), ("linux", "aarch64"));
        let mac = Target::parse("x86_64-apple-darwin").unwrap();
        assert_eq!((mac.os_name(), mac.arch_name()), ("macos", "x86_64"));
        let windows = Target::parse("x86_64-pc-windows-msvc").unwrap();
        assert_eq!((windows.os_name(), windows.arch_name()), ("windows", "x86_64"));
    }
}
