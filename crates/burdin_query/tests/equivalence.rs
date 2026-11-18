//! The answer of an incremental analysis is the answer of a from-scratch one (`spec/query.md`, QL4): after every
//! edit of a long sequence, the diagnostics and the typed program of a database that has been edited all along are
//! identical to those of a fresh database given the same text.
//!
//! The sequences are generated from the conformance programs, with a fixed seed: deleting and duplicating lines,
//! inserting lines and comments (which renumbers every line below), swapping operators and literals, and adding
//! functions. Most steps produce errors, which is the point: a diagnostic cached for an unchanged function must move
//! with it, and an error must disappear when its cause does.
//!
//! `BURDIN_EQUIV_FILES` and `BURDIN_EQUIV_STEPS` scale the run (default 12 programs of 5 edits; CI runs more in
//! release).

use burdin_query::{Analysis, Db, Options};
use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};

/// Hashes what is written to it, so a typed program too large to print can be compared.
struct HashWriter(DefaultHasher);

impl std::fmt::Write for HashWriter {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.write(s.as_bytes());
        Ok(())
    }
}

/// Everything an analysis answers, as one comparable value: the diagnostics rendered, and a digest of the program.
fn answer(db: &Db, analysis: &Analysis) -> (String, Option<u64>) {
    let digest = analysis.program.as_ref().map(|program| {
        let mut writer = HashWriter(DefaultHasher::new());
        write!(writer, "{program:?}").unwrap();
        writer.0.finish()
    });
    (analysis.diagnostics.render(db.sources()), digest)
}

fn fresh_answer(text: &str, options: &Options) -> (String, Option<u64>) {
    let mut db = Db::from_files(&[("main.bd", text)], options.clone()).unwrap();
    let analysis = db.analyze().unwrap();
    answer(&db, &analysis)
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// One random edit of `text`, described for the failure message.
fn mutate(text: &str, rng: &mut Rng, serial: usize) -> (String, String) {
    let lines: Vec<&str> = text.lines().collect();
    let join = |lines: &[String]| {
        let mut out = lines.join("\n");
        out.push('\n');
        out
    };
    let owned: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    match rng.below(7) {
        0 if owned.len() > 1 => {
            let i = rng.below(owned.len());
            let mut v = owned.clone();
            v.remove(i);
            (join(&v), format!("delete line {}", i + 1))
        }
        1 if !owned.is_empty() => {
            let i = rng.below(owned.len());
            let mut v = owned.clone();
            v.insert(i, owned[i].clone());
            (join(&v), format!("duplicate line {}", i + 1))
        }
        2 => {
            let i = rng.below(owned.len() + 1);
            let mut v = owned.clone();
            v.insert(i, String::new());
            (join(&v), format!("insert a blank line at {}", i + 1))
        }
        3 => {
            let swaps =
                [(" + ", " - "), (" - ", " + "), (" < ", " > "), (" == ", " != "), ("true", "false"), (" * ", " + ")];
            let (from, to) = swaps[rng.below(swaps.len())];
            match text.find(from) {
                Some(_) => (text.replacen(from, to, 1), format!("replace `{from}` with `{to}`")),
                None => (text.to_string(), "no change".to_string()),
            }
        }
        4 => {
            // Change the first number written on a random line.
            let start = rng.below(owned.len().max(1));
            for i in (start..owned.len()).chain(0..start) {
                let line = &owned[i];
                if let Some(at) = line.find(|c: char| c.is_ascii_digit()) {
                    let end = line[at..].find(|c: char| !c.is_ascii_digit()).map_or(line.len(), |e| at + e);
                    let mut v = owned.clone();
                    v[i] = format!("{}{}{}", &line[..at], 7 + serial % 3, &line[end..]);
                    return (join(&v), format!("change the number on line {}", i + 1));
                }
            }
            (text.to_string(), "no change".to_string())
        }
        5 => (format!("{text}\nfn zz_added_{serial}(): i32 {{\n    {serial}\n}}\n"), "append a function".to_string()),
        _ => {
            // A comment on its own line after a line that opens a block: inside some body, or a type.
            let candidates: Vec<usize> = (0..owned.len()).filter(|&i| owned[i].trim_end().ends_with('{')).collect();
            if candidates.is_empty() {
                return (text.to_string(), "no change".to_string());
            }
            let i = candidates[rng.below(candidates.len())];
            let mut v = owned.clone();
            v.insert(i + 1, "    // edited".to_string());
            (join(&v), format!("insert a comment after line {}", i + 1))
        }
    }
}

/// The conformance programs that are one file, spread over every milestone.
fn corpus(count: usize) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('m')))
        .collect();
    dirs.sort();
    let mut files = Vec::new();
    for dir in &dirs {
        let mut here: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "bd"))
            .collect();
        here.sort();
        files.push(here);
    }
    // Round-robin over the milestones so every kind of program is in the sample.
    let mut out = Vec::new();
    let mut round = 0;
    while out.len() < count && files.iter().any(|f| round < f.len()) {
        for f in &files {
            if round < f.len() && out.len() < count {
                // Skip the middle of a long list: take every third program of a milestone in later rounds.
                out.push(f[(round * 3) % f.len()].clone());
            }
        }
        round += 1;
    }
    out.dedup();
    out
}

fn scaled(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[test]
fn an_edited_database_answers_like_a_fresh_one() {
    let files = scaled("BURDIN_EQUIV_FILES", 12);
    let steps = scaled("BURDIN_EQUIV_STEPS", 5);
    let options = Options::default();
    for (n, path) in corpus(files).iter().enumerate() {
        let original = std::fs::read_to_string(path).unwrap();
        let mut db = Db::from_files(&[("main.bd", &original)], options.clone()).unwrap();
        let first = db.analyze().unwrap();
        assert_eq!(answer(&db, &first), fresh_answer(&original, &options), "{}: the first analysis", path.display());
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ (n as u64 + 1).wrapping_mul(0xD1B5_4A32_D192_ED03));
        let mut text = original.clone();
        let mut history = Vec::new();
        for step in 0..steps {
            let (next, what) = mutate(&text, &mut rng, step);
            text = next;
            history.push(what.clone());
            db.set_file("main.bd", &text);
            let analysis = db.analyze().unwrap();
            let got = answer(&db, &analysis);
            let want = fresh_answer(&text, &options);
            assert_eq!(got, want, "{}: after {history:?}\n--- text ---\n{text}", path.display());
        }
        // Back to the original text: the answer is the first one again.
        db.set_file("main.bd", &original);
        let analysis = db.analyze().unwrap();
        assert_eq!(answer(&db, &analysis), fresh_answer(&original, &options), "{}: restored", path.display());
    }
}
