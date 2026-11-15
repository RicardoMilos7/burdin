//! The fast regression check for the lexer/parser fuzzer (issues #288 and #447).
//!
//! `cargo test --workspace` runs this in every CI job. It replays every committed corpus file and then
//! runs a deterministic mutation pass from a fixed seed, so a panic that was fixed stays fixed: the input
//! that found it lives under `corpus/syntax/` and is parsed again here. A failure prints the seed and the
//! label needed to reproduce it with the `burdin-fuzz-syntax` binary.
//!
//! The long session lives in `src/bin/syntax.rs` and runs in the scheduled `fuzz` CI job.

use std::path::Path;

use burdin_fuzz::{CORPUS, Session, check, check_with, load_corpus};

/// The seed for the fast mutation pass. Changing it changes which inputs run; it is fixed so every run
/// and every platform checks the same sequence.
const MUTATION_SEED: u64 = 0x6275_7264_696e_2026;

/// How many mutations the fast pass runs. Small enough for a unit-test step, large enough to reach the
/// parser states the corpus alone misses.
const MUTATIONS: u64 = 20_000;

fn corpus() -> Vec<burdin_fuzz::Seed> {
    load_corpus(Path::new(CORPUS)).expect("the committed corpus must load")
}

#[test]
fn the_committed_corpus_replays_without_a_panic() {
    for seed in corpus() {
        if let Err(failure) = check(&seed.text) {
            panic!(
                "the corpus seed {} panicked: {}\n  input: {}\n\
                 fix the panic; the input must stay in the corpus",
                seed.name,
                failure.message,
                failure.escaped()
            );
        }
    }
}

#[test]
fn deterministic_mutations_do_not_panic() {
    let mut session = Session::new(corpus(), MUTATION_SEED, MUTATIONS);
    let mut inputs = 0u64;
    while let Some((text, label)) = session.next_input() {
        inputs += 1;
        if let Err(failure) = check(&text) {
            panic!(
                "the lexer or parser panicked on {label}: {}\n  input: {}\n\
                 reproduce: cargo run -p burdin_fuzz --bin burdin-fuzz-syntax -- --seed {MUTATION_SEED} --iterations {MUTATIONS}\n\
                 save it as a corpus file: crates/burdin_fuzz/corpus/syntax/",
                failure.message,
                failure.escaped()
            );
        }
    }
    assert_eq!(inputs, session.scheduled_inputs());
}

/// Proves the check reports a panic instead of swallowing it; the same wrapper covers the parser.
#[test]
fn the_check_reports_a_deliberately_broken_parser() {
    let failure = check_with("fn main() {}", |_| panic!("deliberate fuzz fixture")).unwrap_err();
    assert_eq!(failure.message, "deliberate fuzz fixture");
    assert_eq!(failure.input, "fn main() {}");
}

#[test]
fn an_empty_corpus_is_an_error() {
    let dir = std::env::temp_dir().join(format!("burdin-fuzz-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let error = load_corpus(&dir).unwrap_err();
    assert!(error.contains("no .bd files"), "{error}");
    let _ = std::fs::remove_dir_all(&dir);
}
