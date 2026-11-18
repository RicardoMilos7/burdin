//! Positions and URIs (`spec/lsp.md`, LS3). The compiler counts byte offsets; LSP counts lines and, inside a line,
//! UTF-16 code units unless the client and the server agree on UTF-8. Every conversion between the two lives here.

use crate::json::Json;
use std::path::{Path, PathBuf};

/// How the `character` of an LSP position counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16,
}

impl Encoding {
    pub fn name(self) -> &'static str {
        match self {
            Encoding::Utf8 => "utf-8",
            Encoding::Utf16 => "utf-16",
        }
    }
}

/// The start of every line of a text, for converting in both directions.
pub struct LineIndex<'a> {
    text: &'a str,
    starts: Vec<usize>,
    encoding: Encoding,
}

impl<'a> LineIndex<'a> {
    pub fn new(text: &'a str, encoding: Encoding) -> LineIndex<'a> {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        LineIndex { text, starts, encoding }
    }

    /// The LSP position of a byte offset. An offset inside a character counts as the character's start.
    pub fn position(&self, offset: u32) -> (u32, u32) {
        let mut offset = (offset as usize).min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self.starts.partition_point(|&s| s <= offset) - 1;
        let before = &self.text[self.starts[line]..offset];
        let character = match self.encoding {
            Encoding::Utf8 => before.len(),
            Encoding::Utf16 => before.chars().map(char::len_utf16).sum(),
        };
        (line as u32, character as u32)
    }

    /// The byte offset of an LSP position. A character past the end of its line is the line's end (before the line
    /// break), and a line past the last is the end of the text, as the protocol asks.
    pub fn offset(&self, line: u32, character: u32) -> u32 {
        let line = line as usize;
        if line >= self.starts.len() {
            return self.text.len() as u32;
        }
        let start = self.starts[line];
        let end = self.starts.get(line + 1).map_or(self.text.len(), |&next| next - 1);
        let content = self.text[start..end].strip_suffix('\r').unwrap_or(&self.text[start..end]);
        let mut units = 0usize;
        for (i, c) in content.char_indices() {
            if units >= character as usize {
                return (start + i) as u32;
            }
            units += match self.encoding {
                Encoding::Utf8 => c.len_utf8(),
                Encoding::Utf16 => c.len_utf16(),
            };
        }
        (start + content.len()) as u32
    }

    /// The LSP range of a byte range.
    pub fn range(&self, lo: u32, hi: u32) -> Json {
        let (l1, c1) = self.position(lo);
        let (l2, c2) = self.position(hi.max(lo));
        range_json(l1, c1, l2, c2)
    }

    /// The byte range of an LSP range object.
    pub fn offsets(&self, range: &Json) -> Option<(u32, u32)> {
        let lo = self.offset_of(range.get("start"))?;
        let hi = self.offset_of(range.get("end"))?;
        Some((lo.min(hi), hi.max(lo)))
    }

    /// The byte offset of an LSP position object.
    pub fn offset_of(&self, position: &Json) -> Option<u32> {
        Some(self.offset(position.get("line").as_u32()?, position.get("character").as_u32()?))
    }

    /// The end of the text as an LSP position.
    pub fn end(&self) -> (u32, u32) {
        self.position(self.text.len() as u32)
    }
}

pub fn position_json(line: u32, character: u32) -> Json {
    Json::obj().with("line", line).with("character", character)
}

pub fn range_json(l1: u32, c1: u32, l2: u32, c2: u32) -> Json {
    Json::obj().with("start", position_json(l1, c1)).with("end", position_json(l2, c2))
}

/// The path of a `file:` URI; `None` for another scheme.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file://host/share/...` is a UNC path on Windows; `file:///...` has an empty host.
    let (host, path) = match rest.find('/') {
        Some(0) => ("", rest),
        Some(i) => (&rest[..i], &rest[i..]),
        None => return None,
    };
    let decoded = percent_decode(path);
    if cfg!(windows) {
        let trimmed = decoded.strip_prefix('/').unwrap_or(&decoded);
        let windows = trimmed.replace('/', "\\");
        if !host.is_empty() && host != "localhost" {
            return Some(PathBuf::from(format!(r"\\{host}\{windows}")));
        }
        // `/c:/x` -> `C:\x`.
        let mut chars = windows.chars();
        if let (Some(drive), Some(':')) = (chars.next(), chars.next()) {
            return Some(PathBuf::from(format!("{}{}", drive.to_ascii_uppercase(), &windows[1..])));
        }
        Some(PathBuf::from(windows))
    } else {
        Some(PathBuf::from(decoded))
    }
}

/// The `file:` URI of a path.
pub fn path_to_uri(path: &Path) -> String {
    let text = burdin_query::package::strip_verbatim_prefix(&path.to_string_lossy()).replace('\\', "/");
    let mut out = String::from("file://");
    if let Some(unc) = text.strip_prefix("//") {
        // A UNC path keeps its host.
        out.push_str(&percent_encode(unc));
        return out;
    }
    if !text.starts_with('/') {
        out.push('/');
    }
    out.push_str(&percent_encode(&text));
    out
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~' | b':') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_counts_surrogate_pairs_and_utf8_counts_bytes() {
        let text = "ab\nlet s = \"é😀\" + x\n";
        let x = text.find("x").unwrap() as u32;
        assert_eq!(LineIndex::new(text, Encoding::Utf16).position(x), (1, 16));
        assert_eq!(LineIndex::new(text, Encoding::Utf8).position(x), (1, 19));
        assert_eq!(LineIndex::new(text, Encoding::Utf16).offset(1, 16), x);
        assert_eq!(LineIndex::new(text, Encoding::Utf8).offset(1, 19), x);
    }

    #[test]
    fn positions_past_the_end_clamp() {
        let text = "ab\r\ncd";
        let index = LineIndex::new(text, Encoding::Utf16);
        assert_eq!(index.offset(0, 99), 2, "the end of the line, before the line break");
        assert_eq!(index.offset(9, 0), 6);
        assert_eq!(index.position(99), (1, 2));
    }

    #[test]
    fn uris_round_trip() {
        if cfg!(windows) {
            let path = uri_to_path("file:///c%3A/Users/a%20b/x.bd").unwrap();
            assert_eq!(path, PathBuf::from(r"C:\Users\a b\x.bd"));
            assert_eq!(path_to_uri(&path), "file:///C:/Users/a%20b/x.bd");
        } else {
            let path = uri_to_path("file:///home/a%20b/x%C3%A9.bd").unwrap();
            assert_eq!(path, PathBuf::from("/home/a b/xé.bd"));
            assert_eq!(path_to_uri(&path), "file:///home/a%20b/x%C3%A9.bd");
        }
        assert!(uri_to_path("untitled:1").is_none());
    }
}
