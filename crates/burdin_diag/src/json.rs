//! A small JSON value with a compact, deterministic serialization, and the version 1 JSON form of a diagnostic.
//!
//! The `json` and `sarif` diagnostic formats are both built from [`Json`] values, so key order is a property of
//! the code (an object keeps the order its keys were added in) and escaping is done in one place. The shape of the
//! diagnostic object is documented in `docs/diagnostics.md` and frozen by `docs/diagnostic.schema.json`
//! (`spec/diagnostics.md`, DG1-DG6, issue #642).

use std::fmt::Write as _;

use crate::{Diagnostic, SourceMap, Span, json_escape};

/// The version of the JSON diagnostic format. It appears as `"version"` in every object the `json` format prints.
/// A change that removes or renames a field, or changes what a field means, raises it; adding a field does not.
pub const JSON_VERSION: u64 = 1;

/// A JSON value. Objects keep the order of their keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(u64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn str(text: impl Into<String>) -> Json {
        Json::Str(text.into())
    }

    /// An object from `(key, value)` pairs, in the order given.
    pub fn obj<const N: usize>(pairs: [(&str, Json); N]) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// An empty object that fields are pushed onto with [`Json::set`].
    pub fn object() -> Json {
        Json::Obj(Vec::new())
    }

    /// Adds a field to an object. Calling it on another kind of value does nothing.
    pub fn set(&mut self, key: &str, value: Json) {
        if let Json::Obj(fields) = self {
            fields.push((key.to_string(), value));
        }
    }

    pub fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                let _ = write!(out, "{n}");
            }
            Json::Str(s) => {
                out.push('"');
                out.push_str(&json_escape(s));
                out.push('"');
            }
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Obj(fields) => {
                out.push('{');
                for (i, (key, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push('"');
                    out.push_str(&json_escape(key));
                    out.push_str("\":");
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

impl std::fmt::Display for Json {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut text = String::new();
        self.write(&mut text);
        f.write_str(&text)
    }
}

/// A source location in the JSON format (DG2): the file, the line and column of both ends, and the byte offsets.
///
/// `column` counts Unicode scalar values from the start of the line, which is the number the human format
/// prints. `utf8_column` counts bytes and `utf16_column` counts UTF-16 code units (what the Language Server
/// Protocol's default position encoding uses), so an editor can use whichever its buffer indexes by without
/// re-reading the file. Lines and columns start at 1; byte offsets start at 0 and `byte_end` is exclusive.
pub(crate) fn location(sources: &SourceMap, span: Span) -> Json {
    let file = sources.get(span.file);
    let start = file.position(span.lo);
    let end = file.position(span.hi);
    Json::obj([
        ("file", Json::str(file.path.as_str())),
        ("line", Json::Num(start.line.into())),
        ("column", Json::Num(start.column.into())),
        ("end_line", Json::Num(end.line.into())),
        ("end_column", Json::Num(end.column.into())),
        ("byte_start", Json::Num(span.lo.into())),
        ("byte_end", Json::Num(span.hi.into())),
        ("utf8_column", Json::Num(start.utf8_column.into())),
        ("utf16_column", Json::Num(start.utf16_column.into())),
        ("end_utf8_column", Json::Num(end.utf8_column.into())),
        ("end_utf16_column", Json::Num(end.utf16_column.into())),
    ])
}

fn optional_location(sources: &SourceMap, span: Option<Span>) -> Json {
    span.map_or(Json::Null, |span| location(sources, span))
}

/// The fields a diagnostic and its children share (DG3): everything except the envelope (`type`, `version`) and the
/// `rendered` text, which only a top-level diagnostic carries.
fn body(d: &Diagnostic, sources: &SourceMap, out: &mut Json) {
    out.set("severity", Json::str(d.severity.as_str()));
    out.set("code", Json::str(d.code.0));
    out.set("message", Json::str(d.message.as_str()));
    out.set("url", d.code.url().map_or(Json::Null, Json::Str));
    out.set("span", location(sources, d.span));
    out.set(
        "labels",
        Json::Arr(
            d.labels
                .iter()
                .map(|label| {
                    Json::obj([
                        ("span", location(sources, label.span)),
                        ("message", Json::str(label.message.as_str())),
                        ("primary", Json::Bool(label.span == d.span)),
                    ])
                })
                .collect(),
        ),
    );
    out.set(
        "notes",
        Json::Arr(
            d.notes
                .iter()
                .map(|note| {
                    Json::obj([
                        ("message", Json::str(note.message.as_str())),
                        ("span", optional_location(sources, note.span)),
                    ])
                })
                .collect(),
        ),
    );
    out.set("help", Json::Arr(d.help.iter().map(|text| Json::str(text.as_str())).collect()));
    // Related information (DG4) is the notes that point at a place, in the shape the Language Server Protocol's
    // `relatedInformation` takes, so an editor does not have to filter them.
    out.set(
        "related",
        Json::Arr(
            d.notes
                .iter()
                .filter_map(|note| {
                    note.span.map(|span| {
                        Json::obj([("span", location(sources, span)), ("message", Json::str(note.message.as_str()))])
                    })
                })
                .collect(),
        ),
    );
    out.set("children", Json::Arr(d.children.iter().map(|child| child_diagnostic(child, sources)).collect()));
    out.set(
        "suggestions",
        Json::Arr(
            d.suggestions
                .iter()
                .map(|suggestion| {
                    Json::obj([
                        ("message", Json::str(suggestion.message.as_str())),
                        ("applicability", Json::str(suggestion.applicability.json_name())),
                        (
                            "edits",
                            Json::Arr(
                                suggestion
                                    .edits
                                    .iter()
                                    .map(|edit| {
                                        Json::obj([
                                            ("span", location(sources, edit.span)),
                                            ("replacement", Json::str(edit.replacement.as_str())),
                                        ])
                                    })
                                    .collect(),
                            ),
                        ),
                    ])
                })
                .collect(),
        ),
    );
}

fn child_diagnostic(d: &Diagnostic, sources: &SourceMap) -> Json {
    let mut out = Json::object();
    body(d, sources, &mut out);
    out
}

/// The version 1 JSON object of a diagnostic: the envelope, the shared body, and the `rendered` human text.
pub(crate) fn diagnostic(d: &Diagnostic, sources: &SourceMap, rendered: String) -> Json {
    let mut out = Json::object();
    out.set("type", Json::str("diagnostic"));
    out.set("version", Json::Num(JSON_VERSION));
    body(d, sources, &mut out);
    out.set("rendered", Json::Str(rendered));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_keep_the_order_of_their_keys_and_escape_strings() {
        let value =
            Json::obj([("b", Json::Num(1)), ("a", Json::Arr(vec![Json::Null, Json::Bool(true), Json::str("q\"\n")]))]);
        assert_eq!(value.to_string(), "{\"b\":1,\"a\":[null,true,\"q\\\"\\n\"]}");
    }
}
