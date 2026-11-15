//! Deterministic fuzzing for the Burdin lexer and parser (issues #288 and #447).
//!
//! The fuzzer deliberately has no dependencies and runs on the pinned stable toolchain. A run is a
//! deterministic function of the corpus and the seed, so a finding is reproduced by printing the seed and
//! the input. The committed corpus under `corpus/syntax/` holds hand-written edge cases and every input
//! that once found a panic; `tests/corpus.rs` replays it and runs a short mutation pass during
//! `cargo test --workspace`, which is what fails when a panic is reintroduced. The `burdin-fuzz-syntax`
//! binary runs longer sessions locally and in the scheduled CI job and writes every new failing input to
//! a directory so it can be committed with its fix.
//!
//! The fuzzable input is a source text, so every input is a valid UTF-8 `&str`, exactly what the driver
//! hands the lexer after `read_to_string`. One [`Session`] first replays the corpus and then mutates it
//! with [`Rng`] and [`mutate`].

use std::any::Any;
use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::Instant;

use burdin_diag::{Diagnostics, FileId};

/// The committed corpus: edge cases plus the inputs that found a panic.
pub const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/corpus/syntax");

/// The largest input a mutation may produce, in characters. Real sources are larger, but a fuzz input
/// only has to reach a parser state, and the cap keeps an iteration cheap and a run bounded.
pub const MAX_INPUT: usize = 4096;

/// One source text in the corpus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seed {
    /// The path or file name, used in labels and reports.
    pub name: String,
    pub text: String,
}

/// A panic caught while lexing and parsing one input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// The input that caused the panic, ready to be saved as a corpus file.
    pub input: String,
    /// The panic message, or the reason the session gave up (a stall).
    pub message: String,
}

impl Failure {
    /// The input as a Rust string literal, for a report.
    pub fn escaped(&self) -> String {
        format!("{:?}", self.input)
    }
}

/// Lexes and parses `text`, turning a panic into a [`Failure`].
pub fn check(text: &str) -> Result<(), Failure> {
    check_with(text, |input| {
        let mut diags = Diagnostics::default();
        let _ = burdin_syntax::parse(FileId(0), input, &mut diags);
    })
}

/// Runs `run` on `text`, returning a [`Failure`] when it panics. Exposed so a test can prove that the
/// harness reports a deliberately broken parser.
pub fn check_with<F>(text: &str, run: F) -> Result<(), Failure>
where
    F: FnOnce(&str),
{
    match panic::catch_unwind(AssertUnwindSafe(|| run(text))) {
        Ok(()) => Ok(()),
        Err(payload) => Err(Failure { input: text.to_string(), message: describe_panic(&*payload) }),
    }
}

fn describe_panic(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "a panic whose payload is not text".to_string()
    }
}

/// Loads every corpus file under `dir`, recursively, sorted by path. A missing directory or an empty
/// corpus is an error: a replay that silently checks nothing would prove nothing.
pub fn load_corpus(dir: &Path) -> Result<Vec<Seed>, String> {
    let seeds = load_seeds(&[dir.to_path_buf()])?;
    if seeds.is_empty() {
        return Err(format!("the corpus {} has no .bd files", dir.display()));
    }
    Ok(seeds)
}

/// Loads the `.bd` files at `paths` (files or directories, recursively). Directories are walked in path
/// order and files are read in path order, so the result is deterministic. Seeds with the same text are
/// kept once: a directory tree can hold the same program twice, and parsing it twice adds nothing.
pub fn load_seeds(paths: &[PathBuf]) -> Result<Vec<Seed>, String> {
    let mut files = Vec::new();
    for path in paths {
        collect_files(path, &mut files)?;
    }
    files.sort();
    let mut seeds = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for path in files {
        let text = fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if seen.insert(text.clone()) {
            let name = path.display().to_string();
            seeds.push(Seed { name, text });
        }
    }
    Ok(seeds)
}

fn collect_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_file() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    let entries = fs::read_dir(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| format!("cannot read {}: {e}", path.display()))?.path();
        if path.is_dir() {
            collect_files(&path, files)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("bd") {
            files.push(path);
        }
    }
    Ok(())
}

/// Writes a failing input under `dir` as `crash-<hash>.bd`, the raw text that must be committed as a
/// corpus file. The content hash makes the write idempotent and deduplicates the same input found from
/// different seeds. Returns the path written.
pub fn save_failure(dir: &Path, failure: &Failure) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(format!("crash-{:016x}.bd", fnv1a64(failure.input.as_bytes())));
    fs::write(&path, &failure.input).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

/// FNV-1a over bytes, the same hash the language uses for derived `Hash` values, used only to name a
/// saved input reproducibly.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// SplitMix64. The generator is part of the harness rather than a dependency so a finding is reproduced
/// on every platform with the same seed.
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `0..n`. `n` must not be zero.
    pub fn below(&mut self, n: usize) -> usize {
        debug_assert!(n > 0, "below(0)");
        (self.next_u64() % n as u64) as usize
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// The pieces the mutator inserts: keywords, punctuation, literals, comments, and unusual characters.
/// Invalid combinations are the point; the parser must report them, not crash.
const SNIPPETS: &[&str] = &[
    // Items, statements, and expressions.
    "fn",
    "struct",
    "enum",
    "interface",
    "impl",
    "type",
    "const",
    "use",
    "extern",
    "priv",
    "unsafe",
    "test",
    "match",
    "if",
    "else",
    "while",
    "for",
    "in",
    "loop",
    "return",
    "break",
    "continue",
    "let",
    "mut",
    "own",
    "self",
    "Self",
    "as",
    "where",
    "true",
    "false",
    "orelse",
    "any",
    "comptime",
    "derive",
    "requires",
    "derives",
    "ident",
    "T",
    "i32",
    "usize",
    "_",
    // Punctuation, including the tokens the parser has to split or look past.
    "(",
    ")",
    "[",
    "]",
    "{",
    "}",
    ",",
    ":",
    ";",
    ".",
    "@",
    "?",
    "=",
    "+",
    "-",
    "*",
    "/",
    "%",
    "&",
    "|",
    "^",
    "!",
    "<",
    ">",
    "..",
    "..=",
    "=>",
    "==",
    "!=",
    "<=",
    ">=",
    "<<",
    ">>",
    "&&",
    "||",
    ".{",
    "}.",
    ">>=",
    // Literals and the escapes around them.
    "\"",
    "'",
    "\\",
    "\"text\"",
    "'\n'",
    "'\\''",
    "'é'",
    "\"\\u{1F600}\"",
    "'ab'",
    "\"\\\\\"",
    "0",
    "1",
    "42",
    "0x",
    "0b102",
    "0o77",
    "1.0",
    "1.",
    ".5",
    "1e",
    "1e+",
    "1e-9",
    "999999999999999999999999999999999999999",
    // Trivia, line endings, and characters outside ASCII.
    " ",
    "\n",
    "\r",
    "\r\n",
    "\t",
    "//",
    "/*",
    "*/",
    "// comment\n",
    "/* comment */",
    "/**/",
    "/// doc\n",
    "é",
    "日",
    "🦀",
    "\u{0}",
    "\u{7f}",
    "\u{202e}",
    "\u{feff}",
];

/// The single characters the mutator inserts. Kept separate so one edit can change the shape of a token
/// without inserting a whole snippet.
const CHARS: &[char] = &[
    '(', ')', '[', ']', '{', '}', '<', '>', '.', ',', ':', ';', '"', '\'', '\\', '/', '*', '&', '|', '!', '=', '+',
    '-', '0', '1', '9', '_', ' ', '\n', '\r', '\t', 'é', '日', '🦀', '\u{0}',
];

/// Mutates `seed`, using `other` for the occasional crossover. The result is at most [`MAX_INPUT`]
/// characters and is a deterministic function of the argument order, the contents, and `rng`.
pub fn mutate(seed: &str, other: &str, rng: &mut Rng) -> String {
    let mut chars: Vec<char> = seed.chars().collect();
    let edits = 1 + rng.below(8);
    for _ in 0..edits {
        match rng.below(10) {
            0 => {
                let end = chars.len();
                insert(&mut chars, end, rng);
            }
            1 => delete(&mut chars, rng),
            2 => replace(&mut chars, rng),
            3 => duplicate(&mut chars, rng),
            4 => {
                let pos = rng.below(chars.len() + 1);
                chars.insert(pos, *rng.pick(CHARS));
            }
            5 => {
                let keep = rng.below(chars.len() + 1);
                chars.truncate(keep);
            }
            6 => splice(&mut chars, rng),
            7 => crossover(&mut chars, other, rng),
            8 => {
                let pos = rng.below(chars.len() + 1);
                let snippet: Vec<char> = rng.pick(SNIPPETS).chars().collect();
                chars.splice(pos..pos, snippet);
            }
            _ => {
                let pos = rng.below(chars.len() + 1);
                chars.insert(pos, *rng.pick(CHARS));
                insert(&mut chars, pos, rng);
            }
        }
        if chars.len() > MAX_INPUT {
            chars.truncate(MAX_INPUT);
        }
    }
    chars.into_iter().collect()
}

/// Inserts one snippet at or after `pos` (clamped to the input).
fn insert(chars: &mut Vec<char>, pos: usize, rng: &mut Rng) {
    let pos = pos.min(chars.len());
    let snippet: Vec<char> = rng.pick(SNIPPETS).chars().collect();
    chars.splice(pos..pos, snippet);
}

fn delete(chars: &mut Vec<char>, rng: &mut Rng) {
    if chars.is_empty() {
        insert(chars, 0, rng);
        return;
    }
    let start = rng.below(chars.len());
    let len = 1 + rng.below((chars.len() - start).min(16));
    chars.drain(start..start + len);
}

fn replace(chars: &mut Vec<char>, rng: &mut Rng) {
    if chars.is_empty() {
        insert(chars, 0, rng);
        return;
    }
    let start = rng.below(chars.len());
    let len = 1 + rng.below((chars.len() - start).min(8));
    let snippet: Vec<char> = rng.pick(SNIPPETS).chars().collect();
    chars.splice(start..start + len, snippet);
}

fn duplicate(chars: &mut Vec<char>, rng: &mut Rng) {
    if chars.is_empty() {
        insert(chars, 0, rng);
        return;
    }
    let start = rng.below(chars.len());
    let len = 1 + rng.below((chars.len() - start).min(64));
    let copy: Vec<char> = chars[start..start + len].to_vec();
    let at = rng.below(chars.len() + 1);
    chars.splice(at..at, copy);
}

/// Takes a prefix of the input and an unrelated chunk of it, and repeats the chunk after the prefix. This
/// moves whole constructs next to each other, which byte-level edits do not.
fn splice(chars: &mut Vec<char>, rng: &mut Rng) {
    if chars.is_empty() {
        insert(chars, 0, rng);
        return;
    }
    let keep = rng.below(chars.len() + 1);
    let start = rng.below(chars.len());
    let len = 1 + rng.below((chars.len() - start).min(64));
    let chunk: Vec<char> = chars[start..start + len].to_vec();
    chars.truncate(keep);
    let at = rng.below(chars.len() + 1);
    chars.splice(at..at, chunk);
}

fn crossover(chars: &mut Vec<char>, other: &str, rng: &mut Rng) {
    let other: Vec<char> = other.chars().collect();
    let left = rng.below(chars.len() + 1);
    let right = rng.below(other.len() + 1);
    chars.truncate(left);
    chars.extend_from_slice(&other[right..]);
}

/// One fuzzing session: every corpus seed once, then `iterations` mutations. The sequence is a
/// deterministic function of the seeds, `seed`, and `iterations`.
pub struct Session {
    seeds: Vec<Seed>,
    rng: Rng,
    iterations: u64,
    replay: usize,
    mutation: u64,
    deadline: Option<Instant>,
}

impl Session {
    pub fn new(seeds: Vec<Seed>, seed: u64, iterations: u64) -> Session {
        Session { seeds, rng: Rng::new(seed), iterations, replay: 0, mutation: 0, deadline: None }
    }

    /// Stops the session once `deadline` passes. `--seconds` uses this instead of a fixed mutation count.
    pub fn with_deadline(mut self, deadline: Option<Instant>) -> Session {
        self.deadline = deadline;
        self
    }

    /// The number of inputs the session can produce at most: the corpus, then the mutations. An empty
    /// corpus produces nothing, because there is nothing to mutate.
    pub fn scheduled_inputs(&self) -> u64 {
        if self.seeds.is_empty() {
            return 0;
        }
        (self.seeds.len() as u64).saturating_add(self.iterations)
    }

    /// The next input and a label naming where it came from.
    pub fn next_input(&mut self) -> Option<(String, String)> {
        if self.deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return None;
        }
        if self.replay < self.seeds.len() {
            let seed = &self.seeds[self.replay];
            self.replay += 1;
            return Some((seed.text.clone(), format!("corpus {}", seed.name)));
        }
        if self.seeds.is_empty() || self.mutation >= self.iterations {
            return None;
        }
        let a = self.rng.below(self.seeds.len());
        let b = self.rng.below(self.seeds.len());
        let text = mutate(&self.seeds[a].text, &self.seeds[b].text, &mut self.rng);
        let label = format!("mutation {} (corpus seeds {a} and {b})", self.mutation);
        self.mutation += 1;
        Some((text, label))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng() -> Rng {
        Rng::new(0x1234_5678_9abc_def0)
    }

    #[test]
    fn the_generator_is_splitmix64() {
        // The first values are fixed, so a change in the generator is a deliberate change of every run.
        let mut rng = Rng::new(0);
        assert_eq!(rng.next_u64(), 0xe220_a839_7b1d_cdaf);
        assert_eq!(rng.next_u64(), 0x6e78_9e6a_a1b9_65f4);
    }

    #[test]
    fn a_panic_is_a_failure() {
        let failure = check_with("boom", |_| panic!("the parser broke")).unwrap_err();
        assert_eq!(failure.input, "boom");
        assert_eq!(failure.message, "the parser broke");
        assert_eq!(failure.escaped(), "\"boom\"");
    }

    #[test]
    fn a_clean_run_is_not_a_failure() {
        assert!(check_with("", |_| {}).is_ok());
    }

    #[test]
    fn mutations_are_deterministic() {
        let first = mutate("fn main() {}", "struct S {}", &mut rng());
        let second = mutate("fn main() {}", "struct S {}", &mut rng());
        assert_eq!(first, second);
    }

    #[test]
    fn mutations_stay_within_the_cap() {
        let mut rng = rng();
        for _ in 0..100 {
            let long = "x".repeat(MAX_INPUT);
            assert!(mutate(&long, &long, &mut rng).chars().count() <= MAX_INPUT);
        }
    }

    #[test]
    fn a_failing_input_is_saved_under_a_content_hash() {
        let dir = std::env::temp_dir().join(format!("burdin-fuzz-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let failure = Failure { input: "fn (".to_string(), message: "boom".to_string() };
        let path = save_failure(&dir, &failure).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "fn (");
        assert!(path.file_name().unwrap().to_string_lossy().starts_with("crash-"));
        let again = save_failure(&dir, &failure).unwrap();
        assert_eq!(path, again, "the same input must map to the same file");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_session_replays_the_corpus_before_mutating() {
        let seeds = vec![
            Seed { name: "a".to_string(), text: "fn a() {}".to_string() },
            Seed { name: "b".to_string(), text: "fn b() {}".to_string() },
        ];
        let mut session = Session::new(seeds, 7, 2);
        assert_eq!(session.scheduled_inputs(), 4);
        assert_eq!(session.next_input().unwrap().0, "fn a() {}");
        assert_eq!(session.next_input().unwrap().0, "fn b() {}");
        assert!(session.next_input().is_some());
        assert!(session.next_input().is_some());
        assert!(session.next_input().is_none());
    }

    #[test]
    fn an_empty_session_produces_nothing() {
        let mut session = Session::new(Vec::new(), 1, 100);
        assert_eq!(session.scheduled_inputs(), 0);
        assert!(session.next_input().is_none());
    }

    #[test]
    fn the_corpus_directory_has_seeds() {
        let seeds = load_corpus(Path::new(CORPUS)).unwrap();
        assert!(seeds.len() >= 10, "the corpus has only {} seeds", seeds.len());
    }
}
