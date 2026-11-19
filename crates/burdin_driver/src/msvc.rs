//! Finding the MSVC libraries when the shell has not loaded the Visual Studio environment (issue #826).
//!
//! A Developer Command Prompt sets `LIB` to the MSVC and Windows SDK library directories, and `link.exe` reads it to
//! find `libcmt.lib`, `libucrt.lib`, and `kernel32.lib`. An ordinary terminal has no `LIB`, so the driver finds the
//! installation itself, as `rustc` does: Visual Studio through `VCINSTALLDIR` or `vswhere.exe`, the MSVC tools version
//! it names as the default, and the newest Windows SDK. It then passes their `lib\<arch>` directories to the linker as
//! `/LIBPATH:`, and uses that installation's `cl.exe` when the user did not name a linker.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What discovery reads from the machine. `from_env` fills it from the environment and `vswhere.exe`; a test fills it
/// with a fake installation.
pub(crate) struct Probe {
    /// `VCINSTALLDIR`: the `VC` directory of a Visual Studio installation (set by a partly loaded environment).
    pub vc_install_dir: Option<PathBuf>,
    /// The Visual Studio installations `vswhere.exe` reports, newest first.
    pub vs_install_dirs: Vec<PathBuf>,
    /// `WindowsSdkDir`, or the default `Windows Kits\10` directory.
    pub sdk_dir: Option<PathBuf>,
    /// `WindowsSDKVersion`, when the environment names one.
    pub sdk_version: Option<String>,
    /// The library architecture: `x64` or `arm64`.
    pub arch: &'static str,
}

/// The MSVC toolchain discovery found.
#[derive(Debug, PartialEq)]
pub(crate) struct Toolchain {
    /// The installation's `cl.exe` for the host and the target architecture.
    pub cl: PathBuf,
    /// The MSVC CRT directory, then the SDK's `ucrt` and `um` directories.
    pub lib_dirs: Vec<PathBuf>,
}

impl Toolchain {
    /// The `/LIBPATH:` arguments for `link.exe`. They follow `/link` on the `cl` command line.
    pub fn libpath_args(&self) -> Vec<String> {
        self.lib_dirs.iter().map(|dir| format!("/LIBPATH:{}", dir.display())).collect()
    }
}

/// The library architecture of the programs this driver builds: the host's.
pub(crate) fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" }
}

/// Whether the environment already names the libraries (`LIB` is set and not empty), so `link.exe` needs no help.
pub(crate) fn lib_env_is_set() -> bool {
    std::env::var_os("LIB").is_some_and(|value| !value.is_empty())
}

impl Probe {
    /// Reads the environment and asks `vswhere.exe` for the Visual Studio installations.
    pub fn from_env() -> Probe {
        let env_dir = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
        let program_files = env_dir("ProgramFiles(x86)").unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
        let sdk_dir = env_dir("WindowsSdkDir").or_else(|| Some(program_files.join("Windows Kits").join("10")));
        let sdk_version = std::env::var("WindowsSDKVersion")
            .ok()
            .map(|version| version.trim_end_matches(['\\', '/']).to_string())
            .filter(|version| !version.is_empty());
        Probe {
            vc_install_dir: env_dir("VCINSTALLDIR"),
            vs_install_dirs: vswhere(&program_files),
            sdk_dir,
            sdk_version,
            arch: host_arch(),
        }
    }
}

/// The installations with the C++ build tools, newest first. `vswhere.exe` ships with the Visual Studio installer at
/// a fixed path; when it is missing there is no installation it could report.
fn vswhere(program_files: &Path) -> Vec<PathBuf> {
    let exe = program_files.join("Microsoft Visual Studio").join("Installer").join("vswhere.exe");
    let Ok(out) = Command::new(exe)
        .args(["-products", "*", "-sort", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64"])
        .args(["-property", "installationPath"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Finds `cl.exe` and the library directories, or says what is missing and how to install it.
pub(crate) fn discover(probe: &Probe) -> Result<Toolchain, String> {
    let arch = probe.arch;
    let vc_dirs = probe.vc_install_dir.iter().cloned().chain(probe.vs_install_dirs.iter().map(|dir| dir.join("VC")));
    let mut msvc = None;
    for vc in vc_dirs {
        if let Some(tools) = msvc_tools(&vc, arch) {
            msvc = Some(tools);
            break;
        }
    }
    let Some(tools) = msvc else {
        return Err(missing("no Visual Studio installation with the MSVC C++ build tools was found".to_string()));
    };
    let cl = tools.join("bin").join(format!("Host{arch}")).join(arch).join("cl.exe");
    let crt = tools.join("lib").join(arch);
    let Some(sdk_lib) = sdk_lib_dir(probe) else {
        let place = probe.sdk_dir.as_ref().map(|dir| format!(" in {}", dir.display())).unwrap_or_default();
        return Err(missing(format!("no Windows SDK with `ucrt\\{arch}\\libucrt.lib` was found{place}")));
    };
    Ok(Toolchain { cl, lib_dirs: vec![crt, sdk_lib.join("ucrt").join(arch), sdk_lib.join("um").join(arch)] })
}

/// The error for a missing toolchain: what is missing, then how to get it.
fn missing(what: String) -> String {
    format!(
        "cannot find the MSVC libraries: {what}, and `LIB` is not set\n\
         note: install Build Tools for Visual Studio (https://visualstudio.microsoft.com/visual-cpp-build-tools/) with \
         the \"Desktop development with C++\" workload, which includes the MSVC build tools and a Windows SDK, or run \
         from a Visual Studio Developer Command Prompt"
    )
}

/// `VC\Tools\MSVC\<version>` for the default tools version the installation names, or else the newest version that
/// has the static CRT for `arch`.
fn msvc_tools(vc: &Path, arch: &str) -> Option<PathBuf> {
    let root = vc.join("Tools").join("MSVC");
    let has_crt = |dir: &Path| dir.join("lib").join(arch).join("libcmt.lib").is_file();
    let default = vc.join("Auxiliary").join("Build").join("Microsoft.VCToolsVersion.default.txt");
    if let Ok(version) = std::fs::read_to_string(default) {
        let dir = root.join(version.trim());
        if has_crt(&dir) {
            return Some(dir);
        }
    }
    newest_version(&root, |dir| has_crt(dir))
}

/// `Windows Kits\10\Lib\<version>` for the version the environment names, or else the newest version that has the
/// UCRT for the architecture.
fn sdk_lib_dir(probe: &Probe) -> Option<PathBuf> {
    let lib = probe.sdk_dir.as_ref()?.join("Lib");
    let has_ucrt = |dir: &Path| dir.join("ucrt").join(probe.arch).join("libucrt.lib").is_file();
    if let Some(version) = &probe.sdk_version {
        let dir = lib.join(version);
        if has_ucrt(&dir) {
            return Some(dir);
        }
    }
    newest_version(&lib, |dir| has_ucrt(dir))
}

/// The subdirectory of `root` with the highest dotted version number that passes `usable`.
fn newest_version(root: &Path, usable: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let name = entry.file_name();
        let Some(version) = name.to_str().and_then(parse_version) else { continue };
        let dir = entry.path();
        if usable(&dir) && best.as_ref().is_none_or(|(newest, _)| version > *newest) {
            best = Some((version, dir));
        }
    }
    best.map(|(_, dir)| dir)
}

/// `14.44.35207` as `[14, 44, 35207]`; anything that is not dotted numbers is not a version directory.
fn parse_version(name: &str) -> Option<Vec<u64>> {
    name.split('.').map(|part| part.parse().ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake Visual Studio and Windows SDK layout in a temporary directory.
    struct Layout(PathBuf);

    impl Layout {
        fn new(name: &str) -> Layout {
            let dir = std::env::temp_dir().join(format!("burdin-msvc-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Layout(dir)
        }

        /// `relative` under the layout, with the platform's separators.
        fn path(&self, relative: &str) -> PathBuf {
            relative.split('/').fold(self.0.clone(), |path, part| path.join(part))
        }

        fn touch(&self, relative: &str) {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }

        fn probe(&self, arch: &'static str) -> Probe {
            Probe {
                vc_install_dir: None,
                vs_install_dirs: vec![self.0.join("VS")],
                sdk_dir: Some(self.0.join("Kits")),
                sdk_version: None,
                arch,
            }
        }
    }

    impl Drop for Layout {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_default_tools_version_and_the_newest_sdk_give_the_library_paths() {
        let layout = Layout::new("default");
        layout.touch("VS/VC/Tools/MSVC/14.29.30133/lib/x64/libcmt.lib");
        layout.touch("VS/VC/Tools/MSVC/14.44.35207/lib/x64/libcmt.lib");
        layout.touch("VS/VC/Tools/MSVC/14.50.1/lib/x64/libcmt.lib");
        std::fs::create_dir_all(layout.0.join("VS/VC/Auxiliary/Build")).unwrap();
        std::fs::write(layout.0.join("VS/VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt"), "14.44.35207\r\n")
            .unwrap();
        layout.touch("Kits/Lib/10.0.22621.0/ucrt/x64/libucrt.lib");
        layout.touch("Kits/Lib/10.0.26100.0/ucrt/x64/libucrt.lib");
        layout.touch("Kits/Lib/10.0.9999.0/um/x64/kernel32.lib");

        let toolchain = discover(&layout.probe("x64")).unwrap();
        // Built one component at a time: the arguments are strings, so the separators must be the platform's.
        let msvc = layout.path("VS/VC/Tools/MSVC/14.44.35207");
        let sdk = layout.path("Kits/Lib/10.0.26100.0");
        assert_eq!(toolchain.cl, msvc.join("bin").join("Hostx64").join("x64").join("cl.exe"));
        assert_eq!(
            toolchain.libpath_args(),
            [
                format!("/LIBPATH:{}", msvc.join("lib").join("x64").display()),
                format!("/LIBPATH:{}", sdk.join("ucrt").join("x64").display()),
                format!("/LIBPATH:{}", sdk.join("um").join("x64").display()),
            ]
        );
    }

    #[test]
    fn without_a_default_file_the_newest_tools_for_the_architecture_win() {
        let layout = Layout::new("arm64");
        layout.touch("VS/VC/Tools/MSVC/14.38.1/lib/arm64/libcmt.lib");
        layout.touch("VS/VC/Tools/MSVC/14.40.2/lib/arm64/libcmt.lib");
        layout.touch("VS/VC/Tools/MSVC/14.41.3/lib/x64/libcmt.lib");
        layout.touch("Kits/Lib/10.0.26100.0/ucrt/arm64/libucrt.lib");

        let toolchain = discover(&layout.probe("arm64")).unwrap();
        let msvc = layout.0.join("VS/VC/Tools/MSVC/14.40.2");
        assert_eq!(toolchain.cl, msvc.join("bin").join("Hostarm64").join("arm64").join("cl.exe"));
        assert_eq!(toolchain.lib_dirs[0], msvc.join("lib").join("arm64"));
        assert_eq!(toolchain.lib_dirs[1], layout.0.join("Kits/Lib/10.0.26100.0").join("ucrt").join("arm64"));
    }

    #[test]
    fn the_environment_names_the_installation_and_the_sdk_version() {
        let layout = Layout::new("env");
        layout.touch("Other/VC/Tools/MSVC/14.30.1/lib/x64/libcmt.lib");
        layout.touch("VS/VC/Tools/MSVC/14.44.35207/lib/x64/libcmt.lib");
        layout.touch("Kits/Lib/10.0.19041.0/ucrt/x64/libucrt.lib");
        layout.touch("Kits/Lib/10.0.26100.0/ucrt/x64/libucrt.lib");
        let mut probe = layout.probe("x64");
        probe.vc_install_dir = Some(layout.0.join("Other/VC"));
        probe.sdk_version = Some("10.0.19041.0".to_string());

        let toolchain = discover(&probe).unwrap();
        assert_eq!(toolchain.lib_dirs[0], layout.0.join("Other/VC/Tools/MSVC/14.30.1").join("lib").join("x64"));
        assert_eq!(toolchain.lib_dirs[1], layout.0.join("Kits/Lib/10.0.19041.0").join("ucrt").join("x64"));
    }

    #[test]
    fn a_missing_visual_studio_is_reported_with_how_to_install_it() {
        let layout = Layout::new("no-vs");
        layout.touch("Kits/Lib/10.0.26100.0/ucrt/x64/libucrt.lib");
        let error = discover(&layout.probe("x64")).unwrap_err();
        assert!(error.contains("no Visual Studio installation with the MSVC C++ build tools"), "{error}");
        assert!(error.contains("`LIB` is not set"), "{error}");
        assert!(error.contains("Build Tools for Visual Studio"), "{error}");
        assert!(error.contains("Desktop development with C++"), "{error}");
        assert!(error.contains("Developer Command Prompt"), "{error}");
    }

    #[test]
    fn a_missing_windows_sdk_is_reported_with_how_to_install_it() {
        let layout = Layout::new("no-sdk");
        layout.touch("VS/VC/Tools/MSVC/14.44.35207/lib/x64/libcmt.lib");
        layout.touch("Kits/Lib/10.0.26100.0/ucrt/arm64/libucrt.lib");
        let error = discover(&layout.probe("x64")).unwrap_err();
        assert!(error.contains("no Windows SDK with `ucrt\\x64\\libucrt.lib`"), "{error}");
        assert!(error.contains("Build Tools for Visual Studio"), "{error}");
    }

    #[test]
    fn version_directories_are_compared_numerically() {
        assert!(parse_version("14.100.1") > parse_version("14.44.35207"));
        assert_eq!(parse_version("10.0.26100.0"), Some(vec![10, 0, 26100, 0]));
        assert_eq!(parse_version("wdf"), None);
    }
}
