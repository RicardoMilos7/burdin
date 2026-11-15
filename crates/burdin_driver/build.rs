//! Build metadata for `burdin --version`: the version, the release channel, the target triple, the profile, and the
//! git commit the binary was built from.
//!
//! `BURDIN_COMMIT` overrides the commit (useful in CI archives without a `.git` directory). `BURDIN_VERSION` overrides
//! the version the workspace manifest declares (nightly builds append a prerelease suffix). `BURDIN_CHANNEL` names the
//! channel (`dev`, `nightly`, or `stable`); local builds default to `dev`.

use std::process::Command;

fn main() {
    for name in ["BURDIN_COMMIT", "BURDIN_VERSION", "BURDIN_CHANNEL"] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=BURDIN_TARGET={target}");
    println!("cargo:rustc-env=BURDIN_PROFILE={profile}");
    let commit = std::env::var("BURDIN_COMMIT").ok().filter(|c| !c.is_empty()).unwrap_or_else(git_commit);
    println!("cargo:rustc-env=BURDIN_GIT_COMMIT={commit}");
    let version = env_value("BURDIN_VERSION")
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION"));
    if !valid_version(&version) {
        panic!(
            "BURDIN_VERSION=`{version}` is not a MAJOR.MINOR.PATCH version (a -prerelease or +build suffix is allowed)"
        );
    }
    let channel = env_value("BURDIN_CHANNEL").unwrap_or_else(|| "dev".to_string());
    if !matches!(channel.as_str(), "dev" | "nightly" | "stable") {
        panic!("BURDIN_CHANNEL=`{channel}` is not dev, nightly, or stable");
    }
    println!("cargo:rustc-env=BURDIN_VERSION={version}");
    println!("cargo:rustc-env=BURDIN_CHANNEL={channel}");
}

/// A non-empty environment variable, or `None` when it is unset or empty.
fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// `MAJOR.MINOR.PATCH`, optionally followed by a SemVer prerelease or build suffix. The value is embedded in the
/// binary and compared against release tags, so a typo must fail the build instead of shipping.
fn valid_version(version: &str) -> bool {
    let core = version.split(['-', '+']).next().unwrap_or("");
    let parts: Vec<&str> = core.split('.').collect();
    let numeric =
        parts.len() == 3 && parts.iter().all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    numeric
        && !version.ends_with(['.', '-', '+'])
        && version.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

/// The short hash of `HEAD`, or `unknown` outside a git checkout.
fn git_commit() -> String {
    let output = Command::new("git").args(["rev-parse", "--short=12", "HEAD"]).output();
    match output {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if text.is_empty() { "unknown".to_string() } else { text }
        }
        _ => "unknown".to_string(),
    }
}
