//! Versions, toolchain names, and the six release targets (`spec/toolchain.md` TC1, TC2).

use std::cmp::Ordering;
use std::fmt;

/// A release version: `MAJOR.MINOR.PATCH` with an optional prerelease such as `nightly.20261118`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// The prerelease after `-`, empty for a stable version.
    pub pre: String,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, pre),
            None => (text, ""),
        };
        let mut parts = core.split('.');
        let mut number = || -> Option<u64> {
            let part = parts.next()?;
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            part.parse().ok()
        };
        let (major, minor, patch) = (number()?, number()?, number()?);
        if parts.next().is_some() {
            return None;
        }
        if text.contains('-') {
            let valid = !pre.is_empty()
                && pre
                    .split('.')
                    .all(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
            if !valid {
                return None;
            }
        }
        Some(Version { major, minor, patch, pre: pre.to_string() })
    }

    pub fn is_stable(&self) -> bool {
        self.pre.is_empty()
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre)?;
        }
        Ok(())
    }
}

impl Ord for Version {
    /// SemVer precedence: the numbers, then a prerelease before its release, then the prerelease identifiers (numeric
    /// ones compared as numbers and before alphanumeric ones).
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => compare_pre(&self.pre, &other.pre),
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn compare_pre(a: &str, b: &str) -> Ordering {
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

/// The two release channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Nightly,
}

impl Channel {
    pub fn name(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Nightly => "nightly",
        }
    }
}

/// A toolchain name (TC1): a channel, which follows the channel's latest release, or an exact version, which never
/// changes. The name is also the toolchain's directory under `<home>/toolchains`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolchainName {
    Channel(Channel),
    Exact(Version),
}

impl ToolchainName {
    pub fn parse(text: &str) -> Result<ToolchainName, String> {
        match text {
            "stable" => Ok(ToolchainName::Channel(Channel::Stable)),
            "nightly" => Ok(ToolchainName::Channel(Channel::Nightly)),
            _ => Version::parse(text).map(ToolchainName::Exact).ok_or_else(|| {
                format!(
                    "`{text}` is not a toolchain: use `stable`, `nightly`, or an exact version such as `0.4.1` or \
                     `0.5.0-nightly.20261118`"
                )
            }),
        }
    }
}

impl fmt::Display for ToolchainName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolchainName::Channel(channel) => f.write_str(channel.name()),
            ToolchainName::Exact(version) => write!(f, "{version}"),
        }
    }
}

/// The six release targets (`docs/releasing.md`).
pub const TARGETS: [&str; 6] = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
];

/// The release target this manager was built for, when it is one of the six.
pub fn host_target() -> Option<&'static str> {
    let target = match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => TARGETS[0],
        ("aarch64", "linux") => TARGETS[1],
        ("x86_64", "macos") => TARGETS[2],
        ("aarch64", "macos") => TARGETS[3],
        ("x86_64", "windows") => TARGETS[4],
        ("aarch64", "windows") => TARGETS[5],
        _ => return None,
    };
    Some(target)
}

/// The executable suffix on the host: `.exe` on Windows.
pub const EXE: &str = std::env::consts::EXE_SUFFIX;

/// This manager's own version: the `BURDIN_VERSION` it was built with (as for the compiler), else the workspace's.
pub fn own_version() -> &'static str {
    option_env!("BURDIN_VERSION").filter(|v| !v.is_empty()).unwrap_or(env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn parses_and_prints_versions() {
        assert_eq!(v("0.4.1").to_string(), "0.4.1");
        assert_eq!(v("0.5.0-nightly.20261118").pre, "nightly.20261118");
        for bad in ["", "1", "1.2", "1.2.3.4", "1.2.x", "1.2.3-", "1.2.3-a..b", "v1.2.3", "1.2.3+build"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn orders_versions_by_semver() {
        assert!(v("0.4.1") > v("0.4.0"));
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("0.5.0-nightly.20261118") < v("0.5.0"));
        assert!(v("0.5.0-nightly.20261118") > v("0.4.9"));
        assert!(v("0.5.0-nightly.20261119") > v("0.5.0-nightly.20261118"));
        assert!(v("1.0.0-2") < v("1.0.0-10"));
        assert!(v("1.0.0-2") < v("1.0.0-alpha"));
    }

    #[test]
    fn parses_toolchain_names() {
        assert_eq!(ToolchainName::parse("stable").unwrap(), ToolchainName::Channel(Channel::Stable));
        assert_eq!(ToolchainName::parse("nightly").unwrap().to_string(), "nightly");
        assert_eq!(ToolchainName::parse("0.4.1").unwrap().to_string(), "0.4.1");
        assert!(ToolchainName::parse("beta").is_err());
        assert!(ToolchainName::parse("../x").is_err());
    }
}
