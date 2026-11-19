// A stand-in for a released `burdin` (and `burdin-lsp`) in the fixture releases of tests/check_toolchain.py. It
// prints which binary ran, the version in the `VERSION` file beside it, its arguments, and the toolchain the proxy
// selected, so the check can see exactly what the proxy ran. `--exit N` makes it exit with status N.

use std::env;
use std::fs;

fn main() {
    let exe = env::current_exe().expect("current_exe");
    let tool = exe.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let version = exe
        .parent()
        .and_then(|dir| fs::read_to_string(dir.join("VERSION")).ok())
        .map(|text| text.trim().to_string())
        .unwrap_or_default();
    let args: Vec<String> = env::args().skip(1).collect();
    let selected = env::var("BURDIN_TOOLCHAIN").unwrap_or_default();
    let dir_matches = env::var_os("BURDIN_TOOLCHAIN_DIR")
        .and_then(|dir| fs::canonicalize(dir).ok())
        .zip(exe.parent().and_then(|dir| fs::canonicalize(dir).ok()))
        .is_some_and(|(a, b)| a == b);
    println!("fake {tool} {version} [{}] toolchain={selected} dir={dir_matches}", args.join(" "));
    if args.first().map(String::as_str) == Some("--exit") {
        let code = args.get(1).and_then(|n| n.parse().ok()).unwrap_or(1);
        std::process::exit(code);
    }
}
