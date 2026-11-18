//! A development aid: formats the files named on the command line to standard output, or checks them with `--check`; `--raw`
//! skips the safety checks, to see what the layout produced.
//! The real command is `burdin fmt`.

use burdin_fmt::{FormatError, Options, format_source};

fn main() {
    let mut check = false;
    let mut raw = false;
    let mut failed = false;
    for arg in std::env::args().skip(1) {
        if arg == "--raw" {
            raw = true;
            continue;
        }
        if arg == "--check" {
            check = true;
            continue;
        }
        let src = std::fs::read_to_string(&arg).expect("readable file");
        if raw {
            match burdin_fmt::format_unverified(&src, &Options::default()) {
                Ok(out) => print!("{out}"),
                Err(e) => eprintln!("{arg}: {e}"),
            }
            continue;
        }
        match format_source(&src, &Options::default()) {
            Ok(out) if check => {
                if out != src {
                    println!("would change: {arg}");
                    failed = true;
                }
            }
            Ok(out) => print!("{out}"),
            Err(FormatError::Syntax(_)) => eprintln!("{arg}: syntax errors"),
            Err(e) => {
                eprintln!("{arg}: {e}");
                failed = true;
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
}
