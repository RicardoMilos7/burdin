//! The one applier (`spec/fix.md`, FX2): edits to one file's text, suggestions that apply all or none, the inline
//! diff the human diagnostics show, and the unified diff `burdin fix`, `burdin lint --fix`, `burdin ssr`, and
//! `burdin refactor` preview.

use crate::{Edit, SourceMap, Span};

/// One file's edits as byte ranges: `(lo, hi, replacement)`.
pub type TextEdit = (u32, u32, String);

/// Whether two edits of one file cannot both apply: their ranges overlap, or both insert at one offset with
/// different text (the order of the two insertions would be a guess). Equal edits are the same edit.
fn clash(a: &TextEdit, b: &TextEdit) -> bool {
    if a == b {
        return false;
    }
    if a.0 == a.1 && b.0 == b.1 {
        return a.0 == b.0;
    }
    // An insertion at the boundary of a replacement does not overlap it; one strictly inside does.
    a.0 < b.1 && b.0 < a.1
}

/// What [`apply_groups`] did.
#[derive(Debug, PartialEq, Eq)]
pub struct GroupsApplied {
    pub text: String,
    /// The indices of the groups that applied, in the order they were given.
    pub applied: Vec<usize>,
    /// The indices of the groups left out because an edit of theirs clashes with an edit of a group that applied
    /// first, or is out of range.
    pub conflicts: Vec<usize>,
}

/// Applies groups of edits to `text`, each group all or none (a suggestion's edits are one group). Groups are
/// taken in the order given; a group with an edit that clashes with an edit already taken, or that is out of range
/// or splits a character, is left out whole and reported. An edit equal to one already taken is the same edit, so
/// two diagnostics that offer one fix apply it once.
pub fn apply_groups(text: &str, groups: &[Vec<TextEdit>]) -> GroupsApplied {
    let mut taken: Vec<TextEdit> = Vec::new();
    let mut applied = Vec::new();
    let mut conflicts = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        let valid = group.iter().all(|(lo, hi, _)| {
            let (l, h) = (*lo as usize, *hi as usize);
            l <= h && h <= text.len() && text.is_char_boundary(l) && text.is_char_boundary(h)
        });
        let inner = group.iter().enumerate().any(|(i, a)| group[i + 1..].iter().any(|b| clash(a, b)));
        if !valid || inner || group.iter().any(|e| taken.iter().any(|t| clash(e, t))) {
            conflicts.push(index);
            continue;
        }
        for edit in group {
            if !taken.contains(edit) {
                taken.push(edit.clone());
            }
        }
        applied.push(index);
    }
    let (out, rest) = apply_edits(text, &taken);
    debug_assert!(rest.is_empty(), "the edits taken never clash");
    GroupsApplied { text: out, applied, conflicts }
}

/// The lines of the inline diff a human diagnostic shows under a suggestion: `('-', line)` for each source line the
/// edits touch and `('+', line)` for what those lines become. Lines equal on both sides at the start and the end are
/// left out, so an inserted line shows as one `+`. The lines are trimmed of the indentation they share. Empty when
/// the edits are in more than one file or cannot apply.
pub fn inline_diff(sources: &SourceMap, edits: &[Edit]) -> Vec<(char, String)> {
    let Some(first) = edits.first() else { return Vec::new() };
    if edits.iter().any(|e| e.span.file != first.span.file) || first.span.file.0 as usize >= sources.len() {
        return Vec::new();
    }
    let text = &sources.get(first.span.file).text;
    let lo = edits.iter().map(|e| e.span.lo as usize).min().unwrap_or(0);
    let hi = edits.iter().map(|e| e.span.hi as usize).max().unwrap_or(0);
    if hi > text.len() || !text.is_char_boundary(lo) || !text.is_char_boundary(hi) {
        return Vec::new();
    }
    let start = text[..lo].rfind('\n').map_or(0, |n| n + 1);
    let end = text[hi..].find('\n').map_or(text.len(), |n| hi + n);
    let old = &text[start..end];
    let shifted: Vec<TextEdit> =
        edits.iter().map(|e| (e.span.lo - start as u32, e.span.hi - start as u32, e.replacement.clone())).collect();
    let (new, conflicts) = apply_edits(old, &shifted);
    if !conflicts.is_empty() {
        return Vec::new();
    }
    let mut a: Vec<&str> = old.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let mut b: Vec<&str> = new.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    while a.len() > 1 && b.len() > 1 && a[0] == b[0] {
        a.remove(0);
        b.remove(0);
    }
    while a.len() > 1 && b.len() > 1 && a.last() == b.last() {
        a.pop();
        b.pop();
    }
    // An insertion of whole lines leaves one equal line on one side; drop it so only the new lines show.
    if a.len() == 1 && b.len() > 1 {
        if b.last() == Some(&a[0]) {
            b.pop();
            a.clear();
        } else if b.first() == Some(&a[0]) {
            b.remove(0);
            a.clear();
        }
    }
    let indent = a
        .iter()
        .chain(b.iter())
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let cut = |l: &str| l.get(indent..).unwrap_or(l.trim_start()).to_string();
    let mut out: Vec<(char, String)> = a.iter().map(|l| ('-', cut(l))).collect();
    out.extend(b.iter().map(|l| ('+', cut(l))));
    out
}

/// The `(lo, hi, replacement)` edits of `edits` that fall in `span`'s file.
pub fn text_edits(edits: &[Edit], file: crate::FileId) -> Vec<TextEdit> {
    edits.iter().filter(|e| e.span.file == file).map(|e| (e.span.lo, e.span.hi, e.replacement.clone())).collect()
}

/// An edit of a span.
pub fn edit(span: Span, replacement: impl Into<String>) -> Edit {
    Edit { span, replacement: replacement.into() }
}

/// Why an edit was not applied.
#[derive(Debug, PartialEq, Eq)]
pub struct EditConflict {
    pub lo: u32,
    pub hi: u32,
}

/// Applies the edits `(lo, hi, replacement)` to `text`. An edit that is identical to one already taken is the same
/// edit and applies once (two diagnostics can offer one fix). An edit that overlaps an earlier one is left out and
/// returned, so the caller can run again after the first has landed. Offsets that are out of range or fall inside a
/// character are conflicts too.
pub fn apply_edits(text: &str, edits: &[(u32, u32, String)]) -> (String, Vec<EditConflict>) {
    let mut sorted: Vec<&(u32, u32, String)> = edits.iter().collect();
    sorted.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    sorted.dedup();
    let mut out = String::with_capacity(text.len());
    let mut conflicts = Vec::new();
    let mut cursor = 0usize;
    for (lo, hi, replacement) in sorted {
        let (l, h) = (*lo as usize, *hi as usize);
        if l < cursor || l > h || h > text.len() || !text.is_char_boundary(l) || !text.is_char_boundary(h) {
            conflicts.push(EditConflict { lo: *lo, hi: *hi });
            continue;
        }
        out.push_str(&text[cursor..l]);
        out.push_str(replacement);
        cursor = h;
    }
    out.push_str(&text[cursor..]);
    (out, conflicts)
}

#[derive(Clone, Copy)]
enum Op {
    Equal,
    Delete,
    Insert,
}

/// Myers' shortest edit script between two line lists, as one operation per line (an `Equal` and a `Delete`
/// consume a line of `a`; an `Equal` and an `Insert` consume a line of `b`).
fn edit_script(a: &[&str], b: &[&str]) -> Vec<Op> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = n + m;
    let offset = max;
    let mut v = vec![0isize; (2 * max + 2) as usize];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    'search: for d in 0..=max {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let idx = |k: isize| (k + offset) as usize;
            let mut x =
                if k == -d || (k != d && v[idx(k - 1)] < v[idx(k + 1)]) { v[idx(k + 1)] } else { v[idx(k - 1)] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx(k)] = x;
            if x >= n && y >= m {
                break 'search;
            }
            k += 2;
        }
    }
    // Walk the trace backwards from (n, m).
    let mut ops: Vec<Op> = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..trace.len() as isize).rev() {
        let v = &trace[d as usize];
        let idx = |k: isize| (k + offset) as usize;
        let k = x - y;
        let prev_k = if k == -d || (k != d && v[idx(k - 1)] < v[idx(k + 1)]) { k + 1 } else { k - 1 };
        let prev_x = v[idx(prev_k)];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            ops.push(Op::Equal);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            if x == prev_x {
                ops.push(Op::Insert);
            } else {
                ops.push(Op::Delete);
            }
        }
        x = prev_x;
        y = prev_y;
    }
    ops.reverse();
    ops
}

/// A unified diff (three lines of context) of `old` against `new`, with `path` in both headers. Empty when the texts
/// are equal.
pub fn unified_diff(path: &str, old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let a: Vec<&str> = old.split_inclusive('\n').collect();
    let b: Vec<&str> = new.split_inclusive('\n').collect();
    // Lines are compared with their terminators, so a missing final newline shows as a change of that line.
    let ops = edit_script(&a, &b);
    // One entry per line of the script: the op and the 0-based line in `a` and `b` it stands at.
    let mut lines: Vec<(Op, usize, usize)> = Vec::with_capacity(ops.len());
    let (mut i, mut j) = (0, 0);
    for op in ops {
        lines.push((op, i, j));
        match op {
            Op::Equal => {
                i += 1;
                j += 1;
            }
            Op::Delete => i += 1,
            Op::Insert => j += 1,
        }
    }
    const CONTEXT: usize = 3;
    let changed: Vec<usize> =
        lines.iter().enumerate().filter(|(_, (op, ..))| !matches!(op, Op::Equal)).map(|(n, _)| n).collect();
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    let mut n = 0;
    while n < changed.len() {
        // A hunk runs from `CONTEXT` lines before its first change to `CONTEXT` lines after its last, and absorbs the
        // next change when the gap between them is small enough to share context.
        let mut last = changed[n];
        let first = changed[n];
        n += 1;
        while n < changed.len() && changed[n] - last <= 2 * CONTEXT {
            last = changed[n];
            n += 1;
        }
        let start = first.saturating_sub(CONTEXT);
        let end = (last + CONTEXT + 1).min(lines.len());
        let (mut a_count, mut b_count) = (0, 0);
        let mut body = String::new();
        for &(op, ai, bi) in &lines[start..end] {
            match op {
                Op::Equal => {
                    a_count += 1;
                    b_count += 1;
                    push_line(&mut body, ' ', a[ai]);
                }
                Op::Delete => {
                    a_count += 1;
                    push_line(&mut body, '-', a[ai]);
                }
                Op::Insert => {
                    b_count += 1;
                    push_line(&mut body, '+', b[bi]);
                }
            }
        }
        let (a_start, b_start) = (lines[start].1 + 1, lines[start].2 + 1);
        let a_start = if a_count == 0 { a_start - 1 } else { a_start };
        let b_start = if b_count == 0 { b_start - 1 } else { b_start };
        out.push_str(&format!("@@ -{a_start},{a_count} +{b_start},{b_count} @@\n{body}"));
    }
    out
}

fn push_line(out: &mut String, sign: char, line: &str) {
    out.push(sign);
    out.push_str(line.trim_end_matches('\n'));
    out.push('\n');
    if !line.ends_with('\n') {
        out.push_str("\\ No newline at end of file\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_apply_once_and_conflicts_are_returned() {
        let text = "let a = 1\nlet b = 2\n";
        let edits = vec![(4, 5, "x".to_string()), (4, 5, "x".to_string()), (4, 9, "overlap".to_string())];
        let (out, conflicts) = apply_edits(text, &edits);
        assert_eq!(out, "let x = 1\nlet b = 2\n");
        assert_eq!(conflicts, vec![EditConflict { lo: 4, hi: 9 }]);
        let (out, conflicts) = apply_edits(text, &[(0, 0, "// hi\n".to_string()), (20, 20, "end".to_string())]);
        assert_eq!(out, "// hi\nlet a = 1\nlet b = 2\nend");
        assert!(conflicts.is_empty());
        let (_, conflicts) = apply_edits(text, &[(5, 99, String::new())]);
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn diff_shows_changed_lines_with_context() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
        let new = "a\nb\nc\nD\ne\nf\ng\nh\ni\nj\n";
        let diff = unified_diff("f.bd", old, new);
        assert_eq!(diff, "--- a/f.bd\n+++ b/f.bd\n@@ -1,7 +1,7 @@\n a\n b\n c\n-d\n+D\n e\n f\n g\n");
        assert_eq!(unified_diff("f.bd", old, old), "");
    }

    #[test]
    fn distant_changes_make_separate_hunks() {
        let old: String = (1..=20).map(|n| format!("{n}\n")).collect();
        let new = old.replacen("2\n", "two\n", 1).replacen("19\n", "nineteen\n", 1);
        let diff = unified_diff("f.bd", &old, &new);
        assert_eq!(diff.matches("@@ -").count(), 2, "{diff}");
        assert!(diff.contains("-2\n+two\n") && diff.contains("-19\n+nineteen\n"), "{diff}");
    }

    #[test]
    fn a_removed_line_and_a_missing_final_newline() {
        let diff = unified_diff("f.bd", "a\nb\n", "a\n");
        assert_eq!(diff, "--- a/f.bd\n+++ b/f.bd\n@@ -1,2 +1,1 @@\n a\n-b\n");
        let diff = unified_diff("f.bd", "a\nb", "a\nb\n");
        assert!(diff.contains("\\ No newline at end of file"), "{diff}");
    }
}
