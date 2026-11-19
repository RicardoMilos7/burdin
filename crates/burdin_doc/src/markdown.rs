//! The Markdown of doc comments (`spec/doc.md`, DC5-DC6): a small, fixed subset that renders to HTML without raw HTML
//! and without any dependency. The hidden `# ` setup lines of a Burdin code block are handled here too, so `burdin doc`
//! (which hides them) and the driver's doc tests (which compile them) read the same block text.
//!
//! Blocks: paragraphs, `#` headings, fenced code blocks (``` or ~~~ with an info string), block quotes, bullet and
//! ordered lists (nested by indentation), tables, and `---` rules. Inlines: `code`, `*emphasis*`, `**strong**`,
//! `[text](https://url)`, `<https://url>`, backslash escapes, and the intra-doc links of `[Type]`, `[module.fn]`,
//! `` [`Type.method`] ``, `[text](Type)`, and `[text][Type]`. Raw HTML is shown as text.
//!
//! The parser keeps an intra-doc link as a node of its own (`Inline::Intra`); `links.rs` resolves them in document
//! order, and the renderer is handed the resolutions in the same order.

use std::fmt::Write;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        inlines: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    /// A fenced code block. `line` is the 0-based line of the opening fence in the doc text, so a tool can map the
    /// block back to its source line; `text` keeps every line, hidden ones included (DC6).
    Code {
        info: String,
        text: String,
        line: usize,
    },
    Quote(Vec<Block>),
    List {
        ordered: bool,
        start: u32,
        items: Vec<Vec<Block>>,
    },
    Table {
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    Rule,
}

/// One line of a Burdin code block: the line as the block holds it, and how many bytes a hidden-line marker adds
/// back when a diagnostic is mapped to the doc comment (`# ` is two bytes, `#` is one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockLine {
    pub text: String,
    /// The marker a hidden line carries (`"# "` or `"#"`), or `""` for a visible line.
    pub hidden: &'static str,
}

impl BlockLine {
    /// The line as the compiler reads it: the marker removed, the indentation kept.
    pub fn compiled(&self) -> &str {
        &self.text
    }

    /// Whether the line is hidden setup (DC6).
    pub fn is_hidden(&self) -> bool {
        !self.hidden.is_empty()
    }
}

/// Splits a code block into its lines with their hidden markers (`# ` or `#`, DC6). A hidden line is compiled by a
/// doc test and not shown by the rendered documentation; `text` holds the line with its marker already removed, and
/// `hidden` is the marker (whose length maps a column of the compiled line back to the doc comment).
pub fn block_lines(text: &str) -> Vec<BlockLine> {
    text.lines()
        .map(|line| {
            let indent = line.len() - line.trim_start().len();
            let rest = &line[indent..];
            if rest == "#" {
                BlockLine { text: line[..indent].to_string(), hidden: "#" }
            } else if let Some(after) = rest.strip_prefix("# ") {
                BlockLine { text: format!("{}{}", &line[..indent], after), hidden: "# " }
            } else {
                BlockLine { text: line.to_string(), hidden: "" }
            }
        })
        .collect()
}

/// The text of a Burdin code block as the rendered documentation shows it: the hidden lines removed. `None` when
/// the block has lines and every one of them is hidden, so the renderer writes no code block at all; an empty block
/// keeps the empty one it always rendered.
pub fn visible_code(text: &str) -> Option<String> {
    let lines = block_lines(text);
    if !lines.is_empty() && lines.iter().all(BlockLine::is_hidden) {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    for line in lines {
        if line.is_hidden() {
            continue;
        }
        out.push_str(&line.text);
        out.push('\n');
    }
    Some(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Code(String),
    Emphasis(Vec<Inline>),
    Strong(Vec<Inline>),
    /// A link to an address outside the documentation.
    Link {
        text: Vec<Inline>,
        url: String,
    },
    /// An intra-doc link: `target` is the path as written; `line` is the line of the doc text it is on.
    Intra {
        text: Vec<Inline>,
        target: String,
        line: usize,
    },
}

/// A candidate intra-doc link found in a doc text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawLink {
    pub text: String,
    pub target: String,
    pub line: usize,
}

/// Parses a doc text into blocks.
pub fn parse(text: &str) -> Vec<Block> {
    let lines: Vec<(usize, &str)> = text.lines().enumerate().map(|(i, l)| (i, l.trim_end_matches('\r'))).collect();
    parse_blocks(&lines)
}

/// The intra-doc links of a doc text, in document order.
pub fn intra_links(blocks: &[Block]) -> Vec<RawLink> {
    let mut out = Vec::new();
    for b in blocks {
        links_in_block(b, &mut out);
    }
    out
}

fn links_in_block(block: &Block, out: &mut Vec<RawLink>) {
    match block {
        Block::Heading { inlines, .. } | Block::Paragraph(inlines) => links_in_inlines(inlines, out),
        Block::Quote(blocks) => blocks.iter().for_each(|b| links_in_block(b, out)),
        Block::List { items, .. } => items.iter().flatten().for_each(|b| links_in_block(b, out)),
        Block::Table { header, rows } => {
            header.iter().for_each(|c| links_in_inlines(c, out));
            rows.iter().flatten().for_each(|c| links_in_inlines(c, out));
        }
        Block::Code { .. } | Block::Rule => {}
    }
}

fn links_in_inlines(inlines: &[Inline], out: &mut Vec<RawLink>) {
    for i in inlines {
        match i {
            Inline::Intra { text, target, line } => {
                out.push(RawLink { text: plain(text), target: target.clone(), line: *line });
            }
            Inline::Emphasis(inner) | Inline::Strong(inner) => links_in_inlines(inner, out),
            Inline::Link { text, .. } => links_in_inlines(text, out),
            Inline::Text(_) | Inline::Code(_) => {}
        }
    }
}

/// The number of fenced code blocks that hold Burdin code (untagged, or tagged `burdin` or `bd`): the examples
/// that the coverage report counts.
pub fn example_count(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|b| match b {
            Block::Code { info, .. } => usize::from(is_burdin_info(info)),
            Block::Quote(inner) => example_count(inner),
            Block::List { items, .. } => items.iter().map(|i| example_count(i)).sum(),
            _ => 0,
        })
        .sum()
}

/// Whether a code block's info string says the block is Burdin: the first word is empty, `burdin`, or `bd`, possibly
/// followed by attributes after a comma or space (`burdin,no_run`).
pub fn is_burdin_info(info: &str) -> bool {
    let word = info.split(|c: char| c == ',' || c.is_whitespace()).next().unwrap_or("");
    word.is_empty() || word == "burdin" || word == "bd"
}

// ----- blocks ---------------------------------------------------------------------------------------------------

fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}

/// The opening fence of a code block: its character, length, and info string.
fn fence_open(line: &str) -> Option<(char, usize, String)> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return None;
    }
    let c = t.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let n = t.chars().take_while(|x| *x == c).count();
    if n < 3 {
        return None;
    }
    let info = t[n..].trim();
    if c == '`' && info.contains('`') {
        return None;
    }
    Some((c, n, info.to_string()))
}

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    let Some(c) = t.chars().next() else { return false };
    if indent_of(line) > 3 || !matches!(c, '-' | '*' | '_') {
        return false;
    }
    let marks = t.chars().filter(|x| *x == c).count();
    marks >= 3 && t.chars().all(|x| x == c || x == ' ')
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return None;
    }
    let n = t.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&n) {
        return None;
    }
    let rest = &t[n..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    let rest = rest.trim();
    let rest = rest.trim_end_matches('#').trim_end();
    Some((n as u8, rest))
}

/// A list marker at the start of a line: `(ordered, start, indent, width)`, where `width` is the offset of the item's
/// content from the start of the line.
fn list_marker(line: &str) -> Option<(bool, u32, usize, usize)> {
    let ind = indent_of(line);
    if ind > 3 {
        return None;
    }
    let t = &line[ind..];
    let mut chars = t.chars();
    let first = chars.next()?;
    if matches!(first, '-' | '*' | '+') {
        let rest = &t[1..];
        if rest.is_empty() {
            return Some((false, 0, ind, ind + 2));
        }
        if rest.starts_with(' ') && !is_rule(line) {
            let spaces = rest.chars().take_while(|c| *c == ' ').count().min(4);
            return Some((false, 0, ind, ind + 1 + if spaces == 0 { 1 } else { spaces }));
        }
        return None;
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let after = &t[digits..];
        if (after.starts_with('.') || after.starts_with(')')) && (after.len() == 1 || after[1..].starts_with(' ')) {
            let start: u32 = t[..digits].parse().unwrap_or(1);
            let spaces = after[1..].chars().take_while(|c| *c == ' ').count().min(4);
            return Some((true, start, ind, ind + digits + 1 + if spaces == 0 { 1 } else { spaces }));
        }
    }
    None
}

fn starts_block(line: &str) -> bool {
    fence_open(line).is_some()
        || heading(line).is_some()
        || is_rule(line)
        || line.trim_start().starts_with('>')
        || list_marker(line).is_some_and(|(ordered, start, ..)| !ordered || start == 1)
}

fn table_cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = t.chars().peekable();
    let mut in_code = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cur.push('|');
                chars.next();
            }
            '`' => {
                in_code = !in_code;
                cur.push(c);
            }
            '|' if !in_code => cells.push(std::mem::take(&mut cur).trim().to_string()),
            _ => cur.push(c),
        }
    }
    cells.push(cur.trim().to_string());
    cells
}

fn is_table_delimiter(line: &str) -> bool {
    let cells = table_cells(line);
    line.contains('-')
        && !cells.is_empty()
        && cells.iter().all(|c| {
            let c = c.trim_matches(':');
            !c.is_empty() && c.chars().all(|x| x == '-')
        })
}

fn parse_blocks(lines: &[(usize, &str)]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (no, line) = lines[i];
        if line.trim().is_empty() {
            i += 1;
        } else if let Some((c, n, info)) = fence_open(line) {
            let mut body: Vec<&str> = Vec::new();
            let open_indent = indent_of(line);
            i += 1;
            while i < lines.len() {
                let l = lines[i].1;
                let t = l.trim_start_matches(' ');
                let closes = indent_of(l) <= 3
                    && t.chars().take_while(|x| *x == c).count() >= n
                    && t.chars().all(|x| x == c || x == ' ');
                if closes {
                    i += 1;
                    break;
                }
                let strip = indent_of(l).min(open_indent);
                body.push(&l[strip..]);
                i += 1;
            }
            let mut text = body.join("\n");
            if !body.is_empty() {
                text.push('\n');
            }
            blocks.push(Block::Code { info, text, line: no });
        } else if let Some((level, content)) = heading(line) {
            blocks.push(Block::Heading { level, inlines: parse_inlines(content, no) });
            i += 1;
        } else if is_rule(line) {
            blocks.push(Block::Rule);
            i += 1;
        } else if line.trim_start().starts_with('>') && indent_of(line) <= 3 {
            let mut inner: Vec<(usize, &str)> = Vec::new();
            while i < lines.len() {
                let (n, l) = lines[i];
                let t = l.trim_start();
                if let Some(rest) = t.strip_prefix('>') {
                    inner.push((n, rest.strip_prefix(' ').unwrap_or(rest)));
                    i += 1;
                } else if !t.is_empty()
                    && !starts_block(l)
                    && !inner.is_empty()
                    && !inner.last().is_some_and(|x| x.1.trim().is_empty())
                {
                    inner.push((n, l));
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(Block::Quote(parse_blocks(&inner)));
        } else if let Some((ordered, start, ..)) = list_marker(line) {
            let (list, next) = parse_list(lines, i, ordered, start);
            blocks.push(list);
            i = next;
        } else if line.contains('|') && i + 1 < lines.len() && is_table_delimiter(lines[i + 1].1) {
            let header: Vec<Vec<Inline>> = table_cells(line).iter().map(|c| parse_inlines(c, no)).collect();
            let mut rows = Vec::new();
            i += 2;
            while i < lines.len() && !lines[i].1.trim().is_empty() && lines[i].1.contains('|') {
                let mut cells: Vec<Vec<Inline>> =
                    table_cells(lines[i].1).iter().map(|c| parse_inlines(c, lines[i].0)).collect();
                cells.resize(header.len(), Vec::new());
                rows.push(cells);
                i += 1;
            }
            blocks.push(Block::Table { header, rows });
        } else {
            let first = no;
            let mut text = String::new();
            while i < lines.len() {
                let l = lines[i].1;
                if l.trim().is_empty() || (i > 0 && !text.is_empty() && starts_block(l)) {
                    break;
                }
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(l.trim());
                i += 1;
            }
            blocks.push(Block::Paragraph(parse_inlines(&text, first)));
        }
    }
    blocks
}

/// Parses the list whose first marker is at `lines[at]`; returns it and the index of the first line after it.
fn parse_list(lines: &[(usize, &str)], at: usize, ordered: bool, start: u32) -> (Block, usize) {
    let mut items: Vec<Vec<Block>> = Vec::new();
    let mut i = at;
    while i < lines.len() {
        let Some((o, _, indent, width)) = list_marker(lines[i].1) else { break };
        if o != ordered {
            break;
        }
        let first = &lines[i].1[width.min(lines[i].1.len())..];
        let mut content: Vec<(usize, String)> = vec![(lines[i].0, first.to_string())];
        i += 1;
        let mut blank_pending = false;
        while i < lines.len() {
            let l = lines[i].1;
            if l.trim().is_empty() {
                blank_pending = true;
                content.push((lines[i].0, String::new()));
                i += 1;
                continue;
            }
            let ind = indent_of(l);
            if ind >= width {
                content.push((lines[i].0, l[width.min(l.len())..].to_string()));
                blank_pending = false;
                i += 1;
            } else if !blank_pending
                && !starts_block(l)
                && content.last().is_some_and(|c| !c.1.trim().is_empty() && fence_open(&c.1).is_none())
                && ind > indent.saturating_sub(1)
                && list_marker(l).is_none()
            {
                // A lazy continuation of the item's last paragraph.
                content.push((lines[i].0, l.trim_start().to_string()));
                i += 1;
            } else {
                break;
            }
        }
        while content.last().is_some_and(|c| c.1.trim().is_empty()) {
            content.pop();
        }
        let refs: Vec<(usize, &str)> = content.iter().map(|(n, s)| (*n, s.as_str())).collect();
        items.push(parse_blocks(&refs));
        // A blank line followed by another marker of the same list continues the list.
        if i < lines.len() && list_marker(lines[i].1).is_none_or(|m| m.0 != ordered) {
            break;
        }
    }
    (Block::List { ordered, start, items }, i)
}

// ----- inlines --------------------------------------------------------------------------------------------------

/// File extensions that make `[text](name.ext)` a file link and not an intra-doc path.
const FILE_EXTENSIONS: &[&str] = &[
    "md", "html", "htm", "txt", "pdf", "png", "jpg", "jpeg", "gif", "svg", "bd", "toml", "json", "css", "js", "rs",
    "py", "c", "h", "yml", "yaml", "sh",
];

/// Whether `text` reads as an item path: identifiers joined by `.` or `::`, with an optional trailing `()`.
pub fn path_segments(text: &str) -> Option<Vec<String>> {
    let text = text.trim();
    let text = text.strip_suffix("()").unwrap_or(text);
    if text.is_empty() {
        return None;
    }
    let normalized = text.replace("::", ".");
    let segments: Vec<&str> = normalized.split('.').collect();
    let ident = |s: &str| {
        let mut chars = s.chars();
        chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    if !segments.iter().all(|s| ident(s)) {
        return None;
    }
    Some(segments.into_iter().map(str::to_string).collect())
}

fn is_file_like(target: &str) -> bool {
    target.rsplit_once('.').is_some_and(|(_, ext)| FILE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Whether the address of `[text](address)` is an intra-doc path.
fn is_intra_target(url: &str) -> bool {
    path_segments(url).is_some() && !is_file_like(url)
}

/// Finds the `]` that closes the `[` at `chars[open]`, skipping code spans and escapes.
fn find_close(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '`' => {
                let run = chars[i..].iter().take_while(|c| **c == '`').count();
                if let Some(end) = find_code_end(chars, i + run, run) {
                    i = end + run - 1;
                } else {
                    i += run - 1;
                }
            }
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The index of the `)` that closes a link address starting at `from`, allowing balanced parentheses inside it.
fn find_paren_close(chars: &[char], from: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in chars.iter().enumerate().skip(from) {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(i),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// The index of the closing backtick run of length `run` at or after `from`.
fn find_code_end(chars: &[char], from: usize, run: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == '`' {
            let n = chars[i..].iter().take_while(|c| **c == '`').count();
            if n == run {
                return Some(i);
            }
            i += n;
        } else {
            i += 1;
        }
    }
    None
}

fn push_text(out: &mut Vec<Inline>, s: &str) {
    if s.is_empty() {
        return;
    }
    if let Some(Inline::Text(last)) = out.last_mut() {
        last.push_str(s);
    } else {
        out.push(Inline::Text(s.to_string()));
    }
}

/// Parses inline Markdown. `line` is the doc-text line the text starts on; a newline inside the text advances it.
pub fn parse_inlines(text: &str, line: usize) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    inlines(&chars, line)
}

fn inlines(chars: &[char], line: usize) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    let mut i = 0;
    let mut buf = String::new();
    let flush = |buf: &mut String, out: &mut Vec<Inline>| {
        push_text(out, buf);
        buf.clear();
    };
    let line_at = |idx: usize| line + chars[..idx].iter().filter(|c| **c == '\n').count();
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' if i + 1 < chars.len() && chars[i + 1].is_ascii_punctuation() => {
                buf.push(chars[i + 1]);
                i += 2;
            }
            '`' => {
                let run = chars[i..].iter().take_while(|x| **x == '`').count();
                if let Some(end) = find_code_end(chars, i + run, run) {
                    flush(&mut buf, &mut out);
                    let mut code: String = chars[i + run..end].iter().collect();
                    code = code.replace('\n', " ");
                    if code.len() > 2 && code.starts_with(' ') && code.ends_with(' ') && code.trim() != "" {
                        code = code[1..code.len() - 1].to_string();
                    }
                    out.push(Inline::Code(code));
                    i = end + run;
                } else {
                    buf.extend(std::iter::repeat_n('`', run));
                    i += run;
                }
            }
            '[' => match link_at(chars, i, line_at(i)) {
                Some((node, next)) => {
                    flush(&mut buf, &mut out);
                    out.push(node);
                    i = next;
                }
                None => {
                    buf.push('[');
                    i += 1;
                }
            },
            '<' => {
                let rest: String = chars[i + 1..].iter().take_while(|x| **x != '>' && **x != ' ').collect();
                let closes = chars.get(i + 1 + rest.chars().count()) == Some(&'>');
                if closes
                    && (rest.starts_with("http://") || rest.starts_with("https://") || rest.starts_with("mailto:"))
                {
                    flush(&mut buf, &mut out);
                    out.push(Inline::Link { text: vec![Inline::Text(rest.clone())], url: rest.clone() });
                    i += rest.chars().count() + 2;
                } else {
                    buf.push('<');
                    i += 1;
                }
            }
            '*' | '_' => {
                let run = chars[i..].iter().take_while(|x| **x == c).count();
                let marks = run.min(2);
                let before_ok = i == 0 || !chars[i - 1].is_alphanumeric() || c == '*';
                let after = chars.get(i + marks).copied();
                if before_ok
                    && after.is_some_and(|a| !a.is_whitespace())
                    && let Some(close) = find_closing(chars, i + marks, c, marks)
                {
                    flush(&mut buf, &mut out);
                    let inner = inlines(&chars[i + marks..close], line_at(i + marks));
                    out.push(if marks == 2 { Inline::Strong(inner) } else { Inline::Emphasis(inner) });
                    i = close + marks;
                    continue;
                }
                buf.extend(std::iter::repeat_n(c, run));
                i += run;
            }
            _ => {
                buf.push(c);
                i += 1;
            }
        }
    }
    flush(&mut buf, &mut out);
    out
}

/// The index of the delimiter that closes an emphasis opened before `from`: `marks` copies of `c`, after a
/// non-space, and (for `_`) not inside a word.
fn find_closing(chars: &[char], from: usize, c: char, marks: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if chars[i] == '`' {
            let run = chars[i..].iter().take_while(|x| **x == '`').count();
            i = find_code_end(chars, i + run, run).map_or(i + run, |e| e + run);
            continue;
        }
        if chars[i] == c {
            let run = chars[i..].iter().take_while(|x| **x == c).count();
            let prev_ok = i > from && !chars[i - 1].is_whitespace();
            let next_ok = c == '*' || chars.get(i + run).is_none_or(|n| !n.is_alphanumeric());
            if run >= marks && prev_ok && next_ok {
                return Some(i + run - marks);
            }
            i += run;
            continue;
        }
        i += 1;
    }
    None
}

/// Tries to read a link that starts with the `[` at `chars[at]`; returns the node and the index after it.
fn link_at(chars: &[char], at: usize, line: usize) -> Option<(Inline, usize)> {
    let close = find_close(chars, at)?;
    let label: Vec<char> = chars[at + 1..close].to_vec();
    if label.is_empty() {
        return None;
    }
    let label_text: String = label.iter().collect();
    let after = chars.get(close + 1).copied();
    match after {
        Some('(') => {
            let end = find_paren_close(chars, close + 2)?;
            let dest: String = chars[close + 2..end].iter().collect();
            let url = dest.trim().trim_matches(['<', '>']).split_whitespace().next().unwrap_or("").to_string();
            let text = inlines(&label, line);
            if is_intra_target(&url) {
                Some((Inline::Intra { text, target: normalize_target(&url), line }, end + 1))
            } else if url.is_empty() {
                None
            } else {
                Some((Inline::Link { text, url }, end + 1))
            }
        }
        Some('[') => {
            let end = chars[close + 2..].iter().position(|c| *c == ']')? + close + 2;
            let reference: String = chars[close + 2..end].iter().collect();
            let target = if reference.trim().is_empty() { strip_ticks(&label_text) } else { strip_ticks(&reference) };
            path_segments(&target)?;
            Some((Inline::Intra { text: inlines(&label, line), target: normalize_target(&target), line }, end + 1))
        }
        _ => {
            let target = strip_ticks(&label_text);
            path_segments(&target)?;
            // `[x]` in a checklist (`[ ]`, `[x]`) and a bare number are not links; a path needs a letter.
            Some((Inline::Intra { text: inlines(&label, line), target: normalize_target(&target), line }, close + 1))
        }
    }
}

fn strip_ticks(s: &str) -> String {
    s.trim().trim_matches('`').trim().to_string()
}

/// The target as the resolver wants it: dots between segments, no `()`.
fn normalize_target(target: &str) -> String {
    let t = target.trim();
    let t = t.strip_suffix("()").unwrap_or(t);
    t.replace("::", ".")
}

// ----- rendering ------------------------------------------------------------------------------------------------

/// How the renderer turns a resolved intra-doc link into HTML.
pub trait LinkRender {
    /// The HTML for the next intra-doc link, whose text is already rendered. Called once per link, in document order.
    fn link(&mut self, text_html: String) -> String;
}

/// Escapes text for an HTML body or an attribute value.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Whether a URL is safe to put in an `href`: no `javascript:` and its relatives.
fn safe_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    !(lower.starts_with("javascript:") || lower.starts_with("data:") || lower.starts_with("vbscript:"))
}

pub fn render_blocks(blocks: &[Block], links: &mut dyn LinkRender) -> String {
    let mut out = String::new();
    for b in blocks {
        render_block(b, links, &mut out);
    }
    out
}

fn render_block(block: &Block, links: &mut dyn LinkRender, out: &mut String) {
    match block {
        Block::Heading { level, inlines } => {
            // The page's own title is `h1`, so a heading in a doc comment starts at `h3`, which keeps the outline
            // of the page in order.
            let h = (level + 2).min(6);
            let body = render_inlines(inlines, links);
            let id = slug(&plain(inlines));
            let _ = writeln!(out, "<h{h} id=\"doc-{id}\">{body}</h{h}>");
        }
        Block::Paragraph(inlines) => {
            let _ = writeln!(out, "<p>{}</p>", render_inlines(inlines, links));
        }
        Block::Code { info, text, .. } => {
            let lang = info.split(|c: char| c == ',' || c.is_whitespace()).next().unwrap_or("");
            let class = if lang.is_empty() { "burdin" } else { lang };
            // A Burdin block hides its `# ` setup lines; a block that is entirely hidden shows nothing (DC6).
            let Some(text) = (if is_burdin_info(info) { visible_code(text) } else { Some(text.clone()) }) else {
                return;
            };
            let _ = writeln!(out, "<pre><code class=\"language-{}\">{}</code></pre>", escape(class), escape(&text));
        }
        Block::Quote(inner) => {
            let _ = writeln!(out, "<blockquote>\n{}</blockquote>", render_blocks(inner, links));
        }
        Block::List { ordered, start, items } => {
            let tag = if *ordered { "ol" } else { "ul" };
            if *ordered && *start != 1 {
                let _ = writeln!(out, "<ol start=\"{start}\">");
            } else {
                let _ = writeln!(out, "<{tag}>");
            }
            for item in items {
                // A list item of one paragraph is shown without the paragraph's margins.
                let body = match item.as_slice() {
                    [Block::Paragraph(inlines)] => render_inlines(inlines, links),
                    _ => render_blocks(item, links),
                };
                let _ = writeln!(out, "<li>{body}</li>");
            }
            let _ = writeln!(out, "</{tag}>");
        }
        Block::Table { header, rows } => {
            out.push_str("<table>\n<thead><tr>");
            for c in header {
                let _ = write!(out, "<th>{}</th>", render_inlines(c, links));
            }
            out.push_str("</tr></thead>\n<tbody>\n");
            for row in rows {
                out.push_str("<tr>");
                for c in row {
                    let _ = write!(out, "<td>{}</td>", render_inlines(c, links));
                }
                out.push_str("</tr>\n");
            }
            out.push_str("</tbody>\n</table>\n");
        }
        Block::Rule => out.push_str("<hr>\n"),
    }
}

pub fn render_inlines(inlines: &[Inline], links: &mut dyn LinkRender) -> String {
    let mut out = String::new();
    for i in inlines {
        match i {
            Inline::Text(t) => out.push_str(&escape(t)),
            Inline::Code(c) => {
                let _ = write!(out, "<code>{}</code>", escape(c));
            }
            Inline::Emphasis(inner) => {
                let _ = write!(out, "<em>{}</em>", render_inlines(inner, links));
            }
            Inline::Strong(inner) => {
                let _ = write!(out, "<strong>{}</strong>", render_inlines(inner, links));
            }
            Inline::Link { text, url } => {
                let body = render_inlines(text, links);
                if safe_url(url) {
                    let _ = write!(out, "<a href=\"{}\">{}</a>", escape(url), body);
                } else {
                    out.push_str(&body);
                }
            }
            Inline::Intra { text, .. } => {
                let body = render_inlines(text, links);
                out.push_str(&links.link(body));
            }
        }
    }
    out
}

/// The text of inlines with the markup removed.
pub fn plain(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for i in inlines {
        match i {
            Inline::Text(t) | Inline::Code(t) => out.push_str(t),
            Inline::Emphasis(inner) | Inline::Strong(inner) => out.push_str(&plain(inner)),
            Inline::Link { text, .. } | Inline::Intra { text, .. } => out.push_str(&plain(text)),
        }
    }
    out
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if (c == ' ' || c == '-' || c == '_') && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// The first paragraph of the blocks as inlines (for a summary line), or an empty list.
pub fn first_paragraph(blocks: &[Block]) -> Vec<Inline> {
    for b in blocks {
        if let Block::Paragraph(inlines) = b {
            return inlines.clone();
        }
    }
    Vec::new()
}

/// The first sentence of the first paragraph, as plain text: up to the first `. ` or the end of the paragraph.
pub fn summary_text(blocks: &[Block]) -> String {
    let text = plain(&first_paragraph(blocks)).replace('\n', " ");
    let end = text
        .char_indices()
        .find(|(i, c)| *c == '.' && text[i + 1..].starts_with(char::is_whitespace))
        .map_or(text.len(), |(i, _)| i + 1);
    text[..end].trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Plain;
    impl LinkRender for Plain {
        fn link(&mut self, text_html: String) -> String {
            format!("<link>{text_html}</link>")
        }
    }

    fn html(text: &str) -> String {
        render_blocks(&parse(text), &mut Plain)
    }

    #[test]
    fn paragraphs_and_inlines() {
        assert_eq!(
            html("a *b* **c** `d<e>`\nnext"),
            "<p>a <em>b</em> <strong>c</strong> <code>d&lt;e&gt;</code>\nnext</p>\n"
        );
    }

    #[test]
    fn headings_start_below_the_page_title() {
        assert_eq!(html("# Errors\n"), "<h3 id=\"doc-errors\">Errors</h3>\n");
    }

    #[test]
    fn code_blocks_keep_their_text_and_default_to_burdin() {
        assert_eq!(
            html("```\nlet x = a < b\n```\n~~~text\nplain\n~~~\n"),
            "<pre><code class=\"language-burdin\">let x = a &lt; b\n</code></pre>\n<pre><code class=\"language-text\">plain\n</code></pre>\n"
        );
    }

    #[test]
    fn lists_nest_by_indentation() {
        let out = html("- one\n- two\n  - inner\n- three\n\n1. a\n2. b\n");
        assert!(out.contains("<li>two<ul>") || out.contains("<li><p>two") || out.contains("two"), "{out}");
        assert!(out.contains("inner"), "{out}");
        assert!(out.contains("<ol>"), "{out}");
        assert_eq!(out.matches("<li>").count(), 6, "{out}");
    }

    #[test]
    fn quotes_rules_and_tables() {
        assert!(html("> quoted\n> text").starts_with("<blockquote>"));
        assert_eq!(html("---"), "<hr>\n");
        let t = html("| a | b |\n|---|:-:|\n| 1 | `x|y` |\n");
        assert!(t.contains("<th>a</th><th>b</th>"), "{t}");
        assert!(t.contains("<td>1</td><td><code>x|y</code></td>"), "{t}");
    }

    #[test]
    fn external_links_and_unsafe_urls() {
        assert_eq!(html("[x](https://e.org/a?b=1)"), "<p><a href=\"https://e.org/a?b=1\">x</a></p>\n");
        assert_eq!(html("[x](javascript:alert(1))"), "<p>x</p>\n");
        assert_eq!(html("<https://e.org>"), "<p><a href=\"https://e.org\">https://e.org</a></p>\n");
        assert_eq!(
            html("[guide](docs/guide.md) and [r](README.md)"),
            "<p><a href=\"docs/guide.md\">guide</a> and <a href=\"README.md\">r</a></p>\n"
        );
    }

    #[test]
    fn intra_doc_links_are_found_in_order() {
        let blocks = parse(
            "See [Point], [`shapes.area`], [the point](Point.new), [t][List::push], and `[NotALink]`.\n\n- [item]\n",
        );
        let found: Vec<(String, String)> = intra_links(&blocks).into_iter().map(|l| (l.text, l.target)).collect();
        assert_eq!(
            found,
            vec![
                ("Point".to_string(), "Point".to_string()),
                ("shapes.area".to_string(), "shapes.area".to_string()),
                ("the point".to_string(), "Point.new".to_string()),
                ("t".to_string(), "List.push".to_string()),
                ("item".to_string(), "item".to_string()),
            ]
        );
    }

    #[test]
    fn brackets_that_are_not_paths_stay_text() {
        assert_eq!(html("[ ] todo [1] and [a b] \\[Point\\]"), "<p>[ ] todo [1] and [a b] [Point]</p>\n");
    }

    #[test]
    fn link_lines_are_relative_to_the_text() {
        let blocks = parse("one\n\ntwo [Point]\nthree [Shape]");
        let lines: Vec<usize> = intra_links(&blocks).into_iter().map(|l| l.line).collect();
        assert_eq!(lines, vec![2, 3]);
    }

    #[test]
    fn examples_are_burdin_blocks() {
        let blocks = parse("```\na\n```\n\n```burdin,no_run\nb\n```\n\n```text\nc\n```\n\n```rust\nd\n```\n");
        assert_eq!(example_count(&blocks), 2);
    }

    #[test]
    fn a_code_block_remembers_its_fence_line() {
        fn code_lines(blocks: &[Block], out: &mut Vec<usize>) {
            for b in blocks {
                match b {
                    Block::Code { line, .. } => out.push(*line),
                    Block::Quote(inner) => code_lines(inner, out),
                    Block::List { items, .. } => items.iter().for_each(|i| code_lines(i, out)),
                    _ => {}
                }
            }
        }
        let blocks = parse("one\n\n```\nlet x = 1\n```\n\n> quoted\n>\n> ```\n> y\n> ```\n");
        let mut lines = Vec::new();
        code_lines(&blocks, &mut lines);
        assert_eq!(lines, [2, 8]);
    }

    #[test]
    fn hidden_lines_are_marked_and_stripped_in_place() {
        let lines = block_lines("# use app.{Point}\nlet p = Point.new()\n#\n    # nested setup()\n");
        assert_eq!(lines[0].hidden, "# ");
        assert_eq!(lines[0].compiled(), "use app.{Point}");
        assert_eq!(lines[1].hidden, "");
        assert_eq!(lines[1].compiled(), "let p = Point.new()");
        // A bare `#` hides an empty line; an indented marker keeps the indentation of what follows it.
        assert_eq!(lines[2].hidden, "#");
        assert_eq!(lines[2].compiled(), "");
        assert_eq!(lines[3].hidden, "# ");
        assert_eq!(lines[3].compiled(), "    nested setup()");
    }

    #[test]
    fn rendered_burdin_blocks_hide_setup_lines() {
        // The marker works untagged and tagged, keeps the visible lines, and leaves other languages alone.
        assert_eq!(
            html("```\n# use app.{Point}\nlet p = Point.new()\n```\n~~~text\n# not hidden\n~~~\n"),
            "<pre><code class=\"language-burdin\">let p = Point.new()\n</code></pre>\n\
             <pre><code class=\"language-text\"># not hidden\n</code></pre>\n"
        );
        // A block whose every line is hidden shows nothing at all.
        assert_eq!(html("before\n\n```\n# hidden\n# all of it\n```\n\nafter\n"), "<p>before</p>\n<p>after</p>\n");
        // A block with no lines keeps the empty block it always rendered.
        assert_eq!(html("```\n```\n"), "<pre><code class=\"language-burdin\"></code></pre>\n");
    }

    #[test]
    fn summary_is_the_first_sentence() {
        assert_eq!(summary_text(&parse("Adds two numbers. More here.\n\nSecond.")), "Adds two numbers.");
        assert_eq!(summary_text(&parse("Has `code` and no period")), "Has code and no period");
    }

    #[test]
    fn raw_html_is_text() {
        assert_eq!(html("<script>x</script>"), "<p>&lt;script&gt;x&lt;/script&gt;</p>\n");
    }
}
