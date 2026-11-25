//! The platform the build is for (`spec/cross-compilation.md`): file names and link choices that follow `--target`
//! instead of the machine the compiler runs on. Without `--target` the answers are the host's, so a native build
//! behaves as it always has.

use burdin_tir::target::{self, Os, Target};

/// Whether the program is built for Windows.
pub(crate) fn windows() -> bool {
    target::current().os == Os::Windows
}

/// Whether the program is built for macOS.
pub(crate) fn macos() -> bool {
    target::current().os == Os::MacOs
}

/// Whether the program is built for Linux.
pub(crate) fn linux() -> bool {
    target::current().os == Os::Linux
}

/// Whether `--target` names a platform other than the host: the link then needs a cross toolchain.
pub(crate) fn is_cross() -> bool {
    target::is_cross()
}

/// The extension of an executable of the target: `exe` for Windows, none elsewhere.
pub(crate) fn exe_extension() -> &'static str {
    if windows() { "exe" } else { "" }
}

/// The name of the environment variable that names the cross linker (`BURDIN_CC_<TRIPLE>`) or the sysroot
/// (`BURDIN_SYSROOT_<TRIPLE>`) of the target being built for.
pub(crate) fn variable(prefix: &str) -> String {
    variable_for(target::current(), prefix)
}

/// `prefix` and the triple of `target` in upper case, every `-` an `_`.
fn variable_for(target: Target, prefix: &str) -> String {
    format!("{prefix}_{}", target.triple().to_ascii_uppercase().replace('-', "_"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_names_follow_the_triple() {
        let arm = Target::parse("aarch64-unknown-linux-gnu").unwrap();
        assert_eq!(variable_for(arm, "BURDIN_CC"), "BURDIN_CC_AARCH64_UNKNOWN_LINUX_GNU");
        let windows = Target::parse("x86_64-pc-windows-msvc").unwrap();
        assert_eq!(variable_for(windows, "BURDIN_SYSROOT"), "BURDIN_SYSROOT_X86_64_PC_WINDOWS_MSVC");
    }
}
