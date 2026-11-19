//! `burdin bindgen --rust`: a Rust crate's public API to Burdin declarations and a C-ABI shim crate
//! (`spec/rust-interop.md`).
//!
//! The input is the crate's rustdoc JSON ([`rustdoc`]), which the driver produces with the pinned toolchain. The
//! tool reads it ([`api`]), decides what can cross the C ABI, and prints two things ([`render`]): a Burdin module that
//! declares the shim's functions with their layout checks, and the Rust source of the shim, a static library that
//! depends on the crate and exports one `extern "C"` function per item. Nothing here runs `cargo` or reads a file:
//! the driver does, so this crate is plain text in, plain text out.

mod api;
mod render;
pub mod rustdoc;

pub use api::{Api, Class, snake};

/// What to name things in the generated shim crate.
pub struct Options {
    /// The path from the shim crate's directory to the Rust crate, as `Cargo.toml` should spell it.
    pub dependency_path: String,
    /// The Rust crate's package name (the `[package] name` of its `Cargo.toml`).
    pub package: String,
    /// The toolchain version the shim is pinned to.
    pub toolchain: String,
}

/// The generated files, as text.
pub struct Output {
    /// The Burdin module.
    pub module: String,
    /// The shim crate's `src/lib.rs`.
    pub shim_source: String,
    /// The shim crate's `Cargo.toml`.
    pub shim_manifest: String,
    /// The shim crate's `rust-toolchain.toml`.
    pub shim_toolchain: String,
    /// The shim package's name, which names its static library.
    pub shim_package: String,
    /// What was skipped or changed, one line each.
    pub notes: Vec<String>,
}

/// Reads rustdoc's JSON and generates the module and the shim.
pub fn generate(rustdoc_json: &str, options: &Options) -> Result<Output, String> {
    let krate = rustdoc::Krate::parse(rustdoc_json)?;
    let api = api::build(&krate)?;
    let shim_package = format!("{}_burdin_shim", snake(&krate.name));
    Ok(Output {
        module: render::burdin_module(&api, krate.format_version),
        shim_source: render::shim_source(&api),
        shim_manifest: render::shim_manifest(&shim_package, &krate.name, &options.package, &options.dependency_path),
        shim_toolchain: render::shim_toolchain(&options.toolchain),
        shim_package,
        notes: api.notes,
    })
}
