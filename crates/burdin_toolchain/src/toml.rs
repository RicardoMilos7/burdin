//! The small TOML subset the manager reads and writes: `[table]` headers, `key = "string"`, and
//! `key = ["a", "b"]` (an array may span lines), with `#` comments. That covers `burdin-toolchain.toml`, the settings,
//! and each toolchain's record; anything else is an error that names the line.

/// A value: a string or an array of strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Str(String),
    List(Vec<String>),
}

/// One `key = value` line, with the table it is in (empty before the first header) and its 1-based line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub table: String,
    pub key: String,
    pub value: Value,
    pub line: usize,
}

/// Parses `text`; an error is `line N: message`.
pub fn parse(text: &str) -> Result<Vec<Entry>, String> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut table = String::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let number = index + 1;
        let line = strip_comment(lines[index]).trim().to_string();
        index += 1;
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            let name = rest
                .strip_suffix(']')
                .map(str::trim)
                .filter(|name| is_key(name))
                .ok_or_else(|| format!("line {number}: expected a table header such as `[toolchain]`"))?;
            table = name.to_string();
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| format!("line {number}: expected `key = value`"))?;
        let key = key.trim();
        if !is_key(key) {
            return Err(format!("line {number}: `{key}` is not a key"));
        }
        let mut value = value.trim().to_string();
        if value.starts_with('[') {
            while !value.ends_with(']') && index < lines.len() {
                value.push(' ');
                value.push_str(strip_comment(lines[index]).trim());
                index += 1;
            }
        }
        let value = parse_value(&value).map_err(|message| format!("line {number}: {message}"))?;
        if entries.iter().any(|e| e.table == table && e.key == key) {
            return Err(format!("line {number}: `{key}` is set twice"));
        }
        entries.push(Entry { table: table.clone(), key: key.to_string(), value, line: number });
    }
    Ok(entries)
}

fn is_key(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The line without a `#` comment; a `#` inside a string is kept.
fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (offset, c) in line.char_indices() {
        match c {
            '\\' if in_string && !escaped => {
                escaped = true;
                continue;
            }
            '"' if !escaped => in_string = !in_string,
            '#' if !in_string => return &line[..offset],
            _ => {}
        }
        escaped = false;
    }
    line
}

fn parse_value(text: &str) -> Result<Value, String> {
    if let Some(inner) = text.strip_prefix('[') {
        let inner = inner.strip_suffix(']').ok_or("an array is missing its `]`")?;
        let mut items = Vec::new();
        let mut rest = inner.trim();
        while !rest.is_empty() {
            let (item, after) = parse_string(rest)?;
            items.push(item);
            rest = after.trim_start();
            match rest.strip_prefix(',') {
                Some(after) => rest = after.trim_start(),
                None if rest.is_empty() => {}
                None => return Err("expected `,` between array items".into()),
            }
        }
        return Ok(Value::List(items));
    }
    let (value, rest) = parse_string(text)?;
    if !rest.trim().is_empty() {
        return Err("unexpected text after the value".into());
    }
    Ok(Value::Str(value))
}

/// A basic string at the start of `text`, and the text after it.
fn parse_string(text: &str) -> Result<(String, &str), String> {
    let body = text.strip_prefix('"').ok_or("expected a string in double quotes")?;
    let mut out = String::new();
    let mut chars = body.char_indices();
    while let Some((offset, c)) = chars.next() {
        match c {
            '"' => return Ok((out, &body[offset + 1..])),
            '\\' => match chars.next() {
                Some((_, '"')) => out.push('"'),
                Some((_, '\\')) => out.push('\\'),
                Some((_, 'n')) => out.push('\n'),
                Some((_, 't')) => out.push('\t'),
                _ => return Err("unsupported escape in a string".into()),
            },
            _ => out.push(c),
        }
    }
    Err("a string is missing its closing `\"`".into())
}

/// A string in double quotes, escaped so [`parse`] reads it back.
pub fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// An array of strings on one line.
pub fn quote_list(items: &[String]) -> String {
    let items: Vec<String> = items.iter().map(|item| quote(item)).collect();
    format!("[{}]", items.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_toolchain_file_shape() {
        let text = "# pinned\n[toolchain]\nchannel = \"nightly\" # the latest\ncomponents = [\n  \"lsp\",\n  \"std-src\",\n]\ntargets = []\n";
        let entries = parse(text).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].table, "toolchain");
        assert_eq!(entries[0].value, Value::Str("nightly".into()));
        assert_eq!(entries[1].value, Value::List(vec!["lsp".into(), "std-src".into()]));
        assert_eq!(entries[2].value, Value::List(vec![]));
        assert_eq!(entries[2].line, 8);
    }

    #[test]
    fn reports_the_line_of_an_error() {
        assert_eq!(
            parse("[toolchain]\nchannel = nightly\n").unwrap_err(),
            "line 2: expected a string in double quotes"
        );
        assert_eq!(parse("a = \"x\"\na = \"y\"\n").unwrap_err(), "line 2: `a` is set twice");
        assert_eq!(parse("[x\n").unwrap_err(), "line 1: expected a table header such as `[toolchain]`");
        assert_eq!(parse("a = [\"x\" \"y\"]\n").unwrap_err(), "line 1: expected `,` between array items");
    }

    #[test]
    fn quoting_round_trips() {
        let text = format!("k = {}\nl = {}\n", quote("a \"b\" \\ #c"), quote_list(&["x".into(), "y#".into()]));
        let entries = parse(&text).unwrap();
        assert_eq!(entries[0].value, Value::Str("a \"b\" \\ #c".into()));
        assert_eq!(entries[1].value, Value::List(vec!["x".into(), "y#".into()]));
    }
}
