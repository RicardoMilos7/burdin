//! The package resolver (`spec/packages.md`): git dependencies, `burdin.lock`, and `burdin add`.
//!
//! ```text
//! resolve::sync(dir, locked)   before a build: resolve the graph, then write burdin.lock or report it stale
//! add::add(request)            `burdin add`: edit burdin.toml, resolve, and write both files
//! git::{resolve, checkout}     the git cache under the Burdin home
//! archive::package(dir)        `burdin package`: the package's reproducible archive (PKG20-PKG22)
//! vendor::vendor(dir)          `burdin vendor`: the git packages copied into vendor/ for --offline builds (PKG23)
//! ```
//!
//! The manifest parser and the lockfile format live in `burdin_query`, whose package loader reads a git
//! dependency from the checkout `burdin.lock` pins; this crate is the part that fetches and writes. It never links
//! LLVM, and it runs no tool but `git`.

pub mod add;
pub mod archive;
pub mod git;
pub mod resolve;
pub mod vendor;
pub use burdin_sha256 as sha256;
