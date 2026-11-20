//! The toolchain manager and the `burdin` proxy (`spec/toolchain.md`).
//!
//! One executable plays both parts. Installed as `burdin` (and `burdin-lsp`), it selects a toolchain for each command
//! (`+toolchain`, `BURDIN_TOOLCHAIN`, `burdin-toolchain.toml`, or the default) and runs that toolchain's own binary.
//! `burdin toolchain ...` is handled here instead: installing, updating, listing, selecting, and removing toolchains,
//! their components and targets, and the manager itself.

pub mod cli;
pub mod dist;
pub mod home;
pub mod install;
pub mod proxy;
pub mod select;
pub mod selfcmd;
pub mod toml;
pub mod version;

/// A failed command: its message and exit status (1 for a failure, 2 for a usage error).
#[derive(Debug)]
pub struct Failure {
    pub message: String,
    pub status: i32,
}

impl Failure {
    pub fn usage(message: impl Into<String>) -> Failure {
        Failure { message: message.into(), status: 2 }
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Failure {
        Failure { message, status: 1 }
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Failure {
        Failure { message: message.to_string(), status: 1 }
    }
}

pub type Outcome = Result<i32, Failure>;
