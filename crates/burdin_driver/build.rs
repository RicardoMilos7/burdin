//! Build metadata for `burdin --version`: the target triple, the profile, and the git commit the binary was built
//! from. `BURDIN_COMMIT` overrides the commit (useful in CI archives without a `.git` directory).

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=BURDIN_COMMIT");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=BURDIN_TARGET={target}");
    println!("cargo:rustc-env=BURDIN_PROFILE={profile}");
    let commit = std::env::var("BURDIN_COMMIT").ok().filter(|c| !c.is_empty()).unwrap_or_else(git_commit);
    println!("cargo:rustc-env=BURDIN_GIT_COMMIT={commit}");
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
