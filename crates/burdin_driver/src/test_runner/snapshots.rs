//! Snapshot assertions (`spec/test-runner.md`, "Snapshot tests").
//!
//! A test process does not compare anything: each `assert_snapshot` call appends a record (its location, whether it
//! has an inline snapshot, and the formatted text) to the file `BURDIN_SNAPSHOTS` names. After every test has run,
//! the runner reads the records of each test in declaration order and compares them with what it keeps: a
//! `snapshots/<module>__<test>[-N].snap` file next to the test's source file, or the `@"..."` literal of the call
//! itself. A missing or different snapshot fails the test and leaves a `.snap.new` beside the file; `--bless`
//! writes the new text instead (rewriting an inline literal in the source), and `--review` asks about each one.

use super::{Outcome, Planned, Run};
use burdin_syntax::token::TokenKind;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// What the runner does with a snapshot that is missing or differs (`--bless`, `--review`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SnapshotMode {
    /// Fail the test and write a `.snap.new` (the default).
    #[default]
    Check,
    /// Accept every new snapshot.
    Bless,
    /// Show each difference and ask whether to accept it.
    Review,
}

/// One `assert_snapshot` call a test process reached: where the call is and the text it recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Record {
    pub(super) file: String,
    pub(super) line: u32,
    pub(super) col: u32,
    pub(super) inline: bool,
    pub(super) text: String,
}

/// The records of one test process, in the order its calls ran. A truncated record (the process was killed while
/// writing it) ends the list.
pub(super) fn parse_records(bytes: &[u8]) -> Vec<Record> {
    let mut records = Vec::new();
    let mut rest = bytes;
    while let Some(newline) = rest.iter().position(|&b| b == b'\n') {
        let header = String::from_utf8_lossy(&rest[..newline]).into_owned();
        rest = &rest[newline + 1..];
        let mut words = header.splitn(4, ' ');
        let (Some("snapshot"), Some(kind), Some(len), Some(location)) =
            (words.next(), words.next(), words.next(), words.next())
        else {
            break;
        };
        let Ok(len) = len.parse::<usize>() else { break };
        if rest.len() < len + 1 {
            break;
        }
        let text = String::from_utf8_lossy(&rest[..len]).into_owned();
        rest = &rest[len + 1..];
        let mut parts = location.rsplitn(3, ':');
        let (Some(col), Some(line), Some(file)) = (parts.next(), parts.next(), parts.next()) else { break };
        let (Ok(line), Ok(col)) = (line.parse(), col.parse()) else { break };
        records.push(Record { file: file.to_string(), line, col, inline: kind == "1", text });
    }
    records
}

/// The name of a test's `nth` file snapshot (counting from 1): `<module>__<test>.snap`, then `-2`, `-3`, ... The
/// module is the dotted module path, or the file's stem for a package's entry file and a one-file target; every
/// character of the test name other than a letter, a digit, `_`, and `-` becomes `_`.
pub(super) fn file_name(module: &str, file: &str, test: &str, nth: usize) -> String {
    let module = if module.is_empty() {
        Path::new(file).file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default()
    } else {
        module.to_string()
    };
    let test: String =
        test.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    let suffix = if nth > 1 { format!("-{nth}") } else { String::new() };
    format!("{module}__{test}{suffix}.snap")
}

/// The text of a snapshot file: a header between `---` lines (the test, the call's location, and its source text)
/// and the snapshot, followed by one newline. Only what follows the header is compared.
pub(super) fn render_file(test: &str, source: &str, expression: &str, text: &str) -> String {
    let expression = expression.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("---\ntest: {test}\nsource: {source}\nexpression: {expression}\n---\n{text}\n")
}

/// The snapshot a file holds: what follows its header, without the final newline. `None` for a file that does not
/// start with a header.
pub(super) fn file_body(content: &str) -> Option<String> {
    let content = content.replace("\r\n", "\n");
    let after = content.strip_prefix("---\n")?;
    let end = after.find("\n---\n")?;
    let body = &after[end + 5..];
    Some(body.strip_suffix('\n').unwrap_or(body).to_string())
}

/// An `assert_snapshot` call found in its source file: the call's text and, for an inline snapshot, the literal's
/// byte range (from the `@` to the end of the string), its text, and the indentation of the call's line.
#[derive(Debug)]
pub(super) struct Call {
    pub(super) expression: String,
    pub(super) inline: Option<Inline>,
}

#[derive(Debug)]
pub(super) struct Inline {
    pub(super) lo: usize,
    pub(super) hi: usize,
    pub(super) literal: String,
    pub(super) indent: String,
}

/// Finds the call that starts at `line`:`col` (1-based; the column counts characters) in `source`.
pub(super) fn find_call(source: &str, line: u32, col: u32) -> Option<Call> {
    let line_start =
        if line <= 1 { 0 } else { source.match_indices('\n').nth(line as usize - 2).map(|(i, _)| i + 1)? };
    let line_text = source[line_start..].split('\n').next().unwrap_or("");
    let offset = line_start + line_text.chars().take(col.saturating_sub(1) as usize).map(char::len_utf8).sum::<usize>();
    let indent: String = line_text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    let mut diags = burdin_diag::Diagnostics::default();
    let lexed = burdin_syntax::lexer::lex_range(burdin_diag::FileId(0), source, offset, source.len(), &mut diags);
    let tokens = &lexed.tokens;
    let mut depth = 0usize;
    let mut inline = None;
    for (index, token) in tokens.iter().enumerate() {
        match &token.kind {
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    let expression = source[offset..token.span.hi as usize].to_string();
                    return Some(Call { expression, inline });
                }
            }
            TokenKind::At if depth == 1 && inline.is_none() => {
                if let Some(next) = tokens.get(index + 1)
                    && let TokenKind::Str(literal) = &next.kind
                {
                    inline = Some(Inline {
                        lo: token.span.lo as usize,
                        hi: next.span.hi as usize,
                        literal: literal.clone(),
                        indent: indent.clone(),
                    });
                }
            }
            TokenKind::Eof => return None,
            _ => {}
        }
    }
    None
}

/// The snapshot an inline literal holds. A literal that starts with a line break is in block form: the break is
/// dropped, the last line (only spaces before the closing quote) is dropped with the break before it, and its
/// spaces are the indentation every other line starts with, which is removed. Any other literal is the snapshot
/// exactly.
pub(super) fn inline_text(literal: &str) -> String {
    let Some(block) = literal.strip_prefix('\n') else { return literal.to_string() };
    let (body, closing) = match block.rfind('\n') {
        Some(i) => (&block[..i], &block[i + 1..]),
        None => ("", block),
    };
    if !closing.chars().all(|c| c == ' ' || c == '\t') {
        // Not the block form's closing line: the text is everything after the first break.
        return block.to_string();
    }
    if block.rfind('\n').is_none() {
        return String::new();
    }
    body.split('\n').map(|line| line.strip_prefix(closing).unwrap_or(line.trim_start())).collect::<Vec<_>>().join("\n")
}

/// The literal `--bless` writes for `text` in a call indented by `indent`: `@"text"` when a plain one-line string
/// holds it, `@r#"text"#` when it has quotes, braces, or backslashes, and the block form of `inline_text` for more
/// than one line.
pub(super) fn render_inline(text: &str, indent: &str) -> String {
    let plain = !text.contains(['"', '\\', '{', '}', '\n', '\r', '\t']);
    if plain {
        return format!("@\"{text}\"");
    }
    // Enough hashes that no `"` followed by them occurs in the text.
    let mut hashes = 1;
    while text.contains(&format!("\"{}", "#".repeat(hashes))) {
        hashes += 1;
    }
    let hashes = "#".repeat(hashes);
    if !text.contains('\n') {
        return format!("@r{hashes}\"{text}\"{hashes}");
    }
    let inner = format!("{indent}    ");
    let mut out = format!("@r{hashes}\"\n");
    for line in text.split('\n') {
        if !line.is_empty() {
            out.push_str(&inner);
            out.push_str(line);
        }
        out.push('\n');
    }
    out.push_str(&inner);
    out.push('"');
    out.push_str(&hashes);
    out
}

/// One snapshot of the run that does not match: where it is kept, the stored text (`None` when there is none
/// yet), and the new text.
struct Mismatch {
    plan_index: usize,
    target: Target,
    old: Option<String>,
    new: String,
    /// The snapshot file's whole text for the new value; unused for an inline snapshot.
    rendered: String,
}

enum Target {
    File(PathBuf),
    Inline { source: PathBuf, lo: usize, hi: usize, indent: String, line: u32 },
}

impl Target {
    fn describe(&self) -> String {
        match self {
            Target::File(path) => format!("snapshot {}", path.display()),
            Target::Inline { source, line, .. } => format!("inline snapshot at {}:{line}", source.display()),
        }
    }
}

/// What the snapshot pass did: the snapshot files a test of the run checked, and the snapshots it wrote.
#[derive(Default)]
pub(super) struct Settled {
    pub(super) touched: BTreeSet<PathBuf>,
    pub(super) updated: Vec<String>,
}

/// Compares the snapshots every test recorded with the stored ones and settles each difference by `mode`. A test
/// with a difference left unaccepted fails, with the reason in its note and the diff in its captured stderr.
pub(super) fn settle(
    tests: &[Planned],
    outcomes: &mut [Option<Outcome>],
    capture_dir: &Path,
    mode: SnapshotMode,
) -> Result<Settled, String> {
    let mut settled = Settled::default();
    let mut mismatches: Vec<Mismatch> = Vec::new();
    let mut sources: BTreeMap<PathBuf, Option<String>> = BTreeMap::new();
    for (plan_index, planned) in tests.iter().enumerate() {
        if !matches!(planned.case.run, Run::Block { .. }) || outcomes[plan_index].is_none() {
            continue;
        }
        let records = match std::fs::read(record_path(capture_dir, plan_index)) {
            Ok(bytes) => parse_records(&bytes),
            Err(_) => continue,
        };
        let mut nth = 0;
        for record in records {
            let source_path = PathBuf::from(&record.file);
            let source =
                sources.entry(source_path.clone()).or_insert_with(|| std::fs::read_to_string(&source_path).ok());
            let call = source.as_deref().and_then(|text| find_call(text, record.line, record.col));
            let expression = call.as_ref().map_or_else(|| "assert_snapshot(...)".to_string(), |c| c.expression.clone());
            let location = format!("{}:{}", record.file, record.line);
            // The header names the source by its file name: the snapshot directory sits next to it, and a
            // committed snapshot must not depend on the machine's paths.
            let source_name = Path::new(&record.file)
                .file_name()
                .map_or_else(|| record.file.clone(), |name| format!("{}:{}", name.to_string_lossy(), record.line));
            if record.inline {
                let Some(inline) = call.and_then(|call| call.inline) else {
                    return Err(format!("cannot find the inline snapshot of the call at {location}"));
                };
                let old = inline_text(&inline.literal);
                if old != record.text {
                    mismatches.push(Mismatch {
                        plan_index,
                        target: Target::Inline {
                            source: source_path,
                            lo: inline.lo,
                            hi: inline.hi,
                            indent: inline.indent,
                            line: record.line,
                        },
                        old: Some(old),
                        new: record.text,
                        rendered: String::new(),
                    });
                }
                continue;
            }
            nth += 1;
            let case = &planned.case;
            let dir = Path::new(&case.file).parent().unwrap_or(Path::new("")).join("snapshots");
            let path = dir.join(file_name(&case.module, &case.file, &case.name, nth));
            settled.touched.insert(path.clone());
            let old = std::fs::read_to_string(&path).ok().and_then(|content| file_body(&content));
            let rendered = render_file(&case.local, &source_name, &expression, &record.text);
            if old.as_deref() == Some(record.text.as_str()) {
                // A stale `.snap.new` from an earlier failure is no longer pending.
                let _ = std::fs::remove_file(new_path(&path));
                continue;
            }
            mismatches.push(Mismatch { plan_index, target: Target::File(path), old, new: record.text, rendered });
        }
    }

    let mut accept_all = mode == SnapshotMode::Bless;
    let mut quit = false;
    let mut inline_edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    for mismatch in &mismatches {
        let accept = if accept_all {
            true
        } else if mode == SnapshotMode::Review && !quit {
            match ask(mismatch, &tests[mismatch.plan_index].case.display)? {
                Answer::Yes => true,
                Answer::No => false,
                Answer::All => {
                    accept_all = true;
                    true
                }
                Answer::Quit => {
                    quit = true;
                    false
                }
            }
        } else {
            false
        };
        if accept {
            match &mismatch.target {
                Target::File(path) => {
                    if let Some(dir) = path.parent() {
                        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
                    }
                    std::fs::write(path, &mismatch.rendered)
                        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                    let _ = std::fs::remove_file(new_path(path));
                }
                Target::Inline { source, lo, hi, indent, .. } => {
                    inline_edits.entry(source.clone()).or_default().push((
                        *lo,
                        *hi,
                        render_inline(&mismatch.new, indent),
                    ));
                }
            }
            settled.updated.push(mismatch.target.describe());
            continue;
        }
        if let Target::File(path) = &mismatch.target {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            }
            let pending = new_path(path);
            std::fs::write(&pending, &mismatch.rendered)
                .map_err(|e| format!("cannot write {}: {e}", pending.display()))?;
        }
        let Some(outcome) = outcomes[mismatch.plan_index].as_mut() else { continue };
        let what = mismatch.target.describe();
        let reason = match &mismatch.old {
            None => format!("{what} is missing"),
            Some(_) => format!("{what} does not match"),
        };
        let diff = snapshot_diff(mismatch);
        let hint = match &mismatch.target {
            Target::File(path) => {
                format!("the new snapshot is in {}; run `burdin test --bless` to accept it\n", new_path(path).display())
            }
            Target::Inline { .. } => "run `burdin test --bless` to rewrite the inline snapshot\n".to_string(),
        };
        outcome.stderr.extend_from_slice(format!("{reason}:\n{diff}{hint}").as_bytes());
        outcome.passed = false;
        outcome.note = Some(match outcome.note.take() {
            Some(note) if !note.is_empty() => format!("{note}; {reason}"),
            _ => reason,
        });
    }
    for (source, mut edits) in inline_edits {
        let mut text =
            std::fs::read_to_string(&source).map_err(|e| format!("cannot read {}: {e}", source.display()))?;
        // From the end, so an earlier edit's offsets stay valid.
        edits.sort_by_key(|(at, ..)| std::cmp::Reverse(*at));
        for (lo, hi, replacement) in edits {
            text.replace_range(lo..hi, &replacement);
        }
        std::fs::write(&source, text).map_err(|e| format!("cannot write {}: {e}", source.display()))?;
    }
    Ok(settled)
}

/// The difference between the stored and the new snapshot as a unified diff: only additions when there is no stored
/// snapshot yet.
fn snapshot_diff(mismatch: &Mismatch) -> String {
    let old = mismatch.old.as_ref().map(|old| format!("{old}\n")).unwrap_or_default();
    burdin_lint::unified_diff("snapshot", &old, &format!("{}\n", mismatch.new))
}

/// Where the test at `plan_index` records its snapshots, next to its captured output.
pub(super) fn record_path(capture_dir: &Path, plan_index: usize) -> PathBuf {
    capture_dir.join(format!("capture-{plan_index}.snap"))
}

/// `<path>.new`: where a snapshot that did not match is left for review.
fn new_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".new");
    PathBuf::from(name)
}

enum Answer {
    Yes,
    No,
    All,
    Quit,
}

/// `--review`: shows one difference on stdout and reads the answer from stdin. The end of the input declines this
/// and every later snapshot.
fn ask(mismatch: &Mismatch, test: &str) -> Result<Answer, String> {
    let diff = snapshot_diff(mismatch);
    let mut stdout = std::io::stdout().lock();
    let state = if mismatch.old.is_some() { "changed" } else { "new" };
    let _ = write!(
        stdout,
        "---- {state} {} (test \"{test}\") ----\n{diff}accept it? [y]es, [n]o, [a]ll, [q]uit: ",
        mismatch.target.describe()
    );
    let _ = stdout.flush();
    let mut line = String::new();
    loop {
        line.clear();
        let read = std::io::stdin().lock().read_line(&mut line).map_err(|e| format!("cannot read stdin: {e}"))?;
        if read == 0 {
            let _ = writeln!(stdout);
            return Ok(Answer::Quit);
        }
        match line.trim() {
            "y" | "yes" => return Ok(Answer::Yes),
            "n" | "no" => return Ok(Answer::No),
            "a" | "all" => return Ok(Answer::All),
            "q" | "quit" => return Ok(Answer::Quit),
            _ => {
                let _ = write!(stdout, "please answer y, n, a, or q: ");
                let _ = stdout.flush();
            }
        }
    }
}

/// The snapshot files of the `snapshots/` directories next to `sources` that no test of the run checked, sorted.
pub(super) fn stale(sources: &[PathBuf], touched: &BTreeSet<PathBuf>) -> Vec<PathBuf> {
    let dirs: BTreeSet<PathBuf> =
        sources.iter().filter_map(|file| file.parent().map(|dir| dir.join("snapshots"))).collect();
    let mut stale = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "snap") && !touched.contains(&path) {
                stale.push(path);
            }
        }
    }
    stale.sort();
    stale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_through_the_file_format() {
        let bytes = b"snapshot 0 5 src/main.bd:3:5\nhello\nsnapshot 1 3 C:\\a b\\x.bd:10:9\na\nb\nsnapshot 0 9 cut";
        let records = parse_records(bytes);
        assert_eq!(records.len(), 2);
        assert_eq!(
            records[0],
            Record { file: "src/main.bd".into(), line: 3, col: 5, inline: false, text: "hello".into() }
        );
        assert_eq!(records[1].file, "C:\\a b\\x.bd");
        assert_eq!((records[1].line, records[1].col, records[1].inline), (10, 9, true));
        assert_eq!(records[1].text, "a\nb");
    }

    #[test]
    fn file_names_use_the_module_and_the_test_name() {
        assert_eq!(file_name("", "src/main.bd", "renders a board", 1), "main__renders_a_board.snap");
        assert_eq!(file_name("geometry.shapes", "src/geometry/shapes.bd", "área", 2), "geometry.shapes___rea-2.snap");
        assert_eq!(file_name("", "one.bd", "x-y_z", 3), "one__x-y_z-3.snap");
    }

    #[test]
    fn file_bodies_skip_the_header_and_the_final_newline() {
        let text = render_file("points", "src/main.bd:4", "assert_snapshot(\n    p)", "Point { x: 1 }\n---\nend");
        assert!(text.contains("expression: assert_snapshot( p)\n"));
        assert_eq!(file_body(&text).as_deref(), Some("Point { x: 1 }\n---\nend"));
        assert_eq!(file_body(&text.replace('\n', "\r\n")).as_deref(), Some("Point { x: 1 }\n---\nend"));
        assert_eq!(file_body("no header"), None);
    }

    #[test]
    fn calls_and_inline_literals_are_found_from_their_location() {
        let source = "test \"t\" {\n    assert_snapshot(f(1, [2]), @r#\"a \"b\"\"#)\n    assert_snapshot(x)\n}\n";
        let call = find_call(source, 2, 5).unwrap();
        assert_eq!(call.expression, "assert_snapshot(f(1, [2]), @r#\"a \"b\"\"#)");
        let inline = call.inline.unwrap();
        assert_eq!(inline.literal, "a \"b\"");
        assert_eq!(&source[inline.lo..inline.hi], "@r#\"a \"b\"\"#");
        assert_eq!(inline.indent, "    ");
        let plain = find_call(source, 3, 5).unwrap();
        assert_eq!(plain.expression, "assert_snapshot(x)");
        assert!(plain.inline.is_none());
    }

    #[test]
    fn inline_literals_render_and_read_back_the_same_text() {
        for text in ["", "plain", "[1, 2]", "Point { x: 1 }", "quote \"#\" here", "two\nlines", "a\n\n  b\n", "\nx"] {
            let literal = render_inline(text, "        ");
            let inner = literal.strip_prefix('@').unwrap();
            let mut diags = burdin_diag::Diagnostics::default();
            let lexed = burdin_syntax::lexer::lex(burdin_diag::FileId(0), inner, &mut diags);
            let TokenKind::Str(read) = &lexed.tokens[0].kind else { panic!("{literal} is not one string") };
            assert_eq!(inline_text(read), text, "{literal}");
        }
        assert_eq!(render_inline("[]", ""), "@\"[]\"");
        assert_eq!(render_inline("a\nb", "  "), "@r#\"\n      a\n      b\n      \"#");
    }
}
