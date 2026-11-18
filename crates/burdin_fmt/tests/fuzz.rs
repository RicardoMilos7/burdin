//! The formatter under fuzzing (issue #462). Every input the lexer/parser fuzz corpus and a deterministic stream of
//! mutations of it produce, together with the formatter's own fixtures, goes through `format_source`. An input that
//! does not parse is left alone by the formatter (that is the parser's business, and `burdin_fuzz` covers it); an
//! input that parses must format without an internal error (the output is the same program with the same comments)
//! and the output must be a fixed point.
//!
//! `cargo test --workspace` runs this in every CI job. The seed is fixed, so every run and every platform checks the
//! same inputs; a failure prints the input.

use burdin_fmt::{FormatError, Options, format_source};
use burdin_fuzz::{CORPUS, Seed, Session, load_corpus, load_seeds};
use std::path::{Path, PathBuf};

const MUTATION_SEED: u64 = 0x666d_7420_2026_0229;
const MUTATIONS: u64 = 20_000;

fn seeds() -> Vec<Seed> {
    let mut seeds = load_corpus(Path::new(CORPUS)).expect("the committed corpus must load");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    // The language's own conformance sources are the richest real programs the repository has.
    let extra = load_seeds(&[root.join("tests").join("m02_basics"), root.join("tests").join("m13_iterators")])
        .expect("the conformance sources must load");
    seeds.extend(extra);
    for entry in std::fs::read_dir(root.join("tests").join("fmt")).expect("the fixtures directory") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "input" || e == "golden") {
            let text = std::fs::read_to_string(&path).expect("a readable fixture");
            seeds.push(Seed { name: path.display().to_string(), text });
        }
    }
    seeds
}

/// Formats `text` twice. `Ok(true)` when it formatted, `Ok(false)` when it did not parse, `Err` for a bug.
fn round_trip(text: &str) -> Result<bool, String> {
    let options = Options::default();
    let once = match format_source(text, &options) {
        Ok(out) => out,
        Err(FormatError::Syntax(_)) => return Ok(false),
        Err(FormatError::Internal(message)) => return Err(format!("internal error: {message}")),
    };
    match format_source(&once, &options) {
        Ok(twice) if twice == once => Ok(true),
        Ok(twice) => Err(format!("not idempotent\n--- first ---\n{once}\n--- second ---\n{twice}")),
        Err(e) => Err(format!("the output does not format again ({e})\n--- output ---\n{once}")),
    }
}

#[test]
fn every_seed_and_mutation_that_parses_formats_to_a_fixed_point() {
    // A longer session: `BURDIN_FMT_FUZZ_SEED` and `BURDIN_FMT_FUZZ_MUTATIONS` choose another stream of inputs.
    let number = |name: &str, default: u64| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    let mut session = Session::new(
        seeds(),
        number("BURDIN_FMT_FUZZ_SEED", MUTATION_SEED),
        number("BURDIN_FMT_FUZZ_MUTATIONS", MUTATIONS),
    );
    let (mut formatted, mut rejected) = (0u64, 0u64);
    while let Some((text, label)) = session.next_input() {
        match round_trip(&text) {
            Ok(true) => formatted += 1,
            Ok(false) => rejected += 1,
            Err(message) => panic!("{label}: {message}\n  input: {text:?}"),
        }
    }
    println!("{formatted} inputs formatted, {rejected} did not parse");
    assert!(formatted > 1_000, "the mutations should reach the formatter often enough to prove something");
}

#[test]
fn nothing_the_parser_accepts_is_lost_when_the_source_is_cut_at_every_token() {
    // Cutting a program after each of its lines gives many different, mostly broken, prefixes; those that parse are
    // formatted like any other source.
    let source = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("fmt")
            .join("comments.input"),
    )
    .expect("the comments fixture");
    let lines: Vec<&str> = source.lines().collect();
    for end in 0..=lines.len() {
        let prefix = lines[..end].join("\n");
        if let Err(message) = round_trip(&prefix) {
            panic!("a prefix of {end} lines: {message}\n  input: {prefix:?}");
        }
    }
}
