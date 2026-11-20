//! `burdin-toolchain`: the toolchain manager and proxy. Installed as `burdin` and `burdin-lsp` (`spec/toolchain.md`).

fn main() {
    let mut args = std::env::args_os();
    let argv0 = args.next().unwrap_or_default();
    // `exit` rather than `ExitCode`, so a Windows status above 255 from the proxied tool reaches the caller intact.
    std::process::exit(burdin_toolchain::proxy::main(&argv0, args.collect()));
}
