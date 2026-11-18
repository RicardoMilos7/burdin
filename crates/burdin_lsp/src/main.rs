//! `burdin-lsp`: the language server without the rest of the driver (`spec/lsp.md`). Editors run `burdin lsp`; this
//! binary exists so the server can be built and tested without LLVM.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!("burdin-lsp {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if let Some(other) = args.iter().find(|a| *a != "--stdio") {
        eprintln!("burdin-lsp: unknown argument `{other}` (the server speaks LSP on standard input and output)");
        std::process::exit(2);
    }
    std::process::exit(burdin_lsp::serve_stdio());
}
