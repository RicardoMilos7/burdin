//! Applying fixes (`spec/lint.md`, LN28-LN31): edits to one file's text, and the unified diff the driver previews.

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
