//! SARIF 2.1.0 output (`--diagnostic-format sarif`, `spec/diagnostics.md` DG8-DG11, issue #643).
//!
//! A SARIF log is one JSON document, so unlike the `json` format it is built from the whole sorted list of
//! diagnostics at once. It has one run: the tool (name, version, and the error-code registry as `rules` with a link
//! to each code's entry in the error reference), and one result for each diagnostic, with its locations, the notes
//! that point somewhere as related locations, and each suggestion as a `fix`.

use crate::json::Json;
use crate::{Diagnostic, Severity, SourceMap, Span};

pub const SARIF_VERSION: &str = "2.1.0";
pub const SARIF_SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";

/// One entry of the run's `rules`: a diagnostic code and where it is explained.
#[derive(Clone, Debug)]
pub struct Rule {
    pub id: String,
    /// The short name, such as `Syntax error` or `unused_variables`.
    pub name: String,
    pub help_uri: Option<String>,
}

/// The tool that produced the log.
#[derive(Clone, Debug)]
pub struct Tool {
    pub name: String,
    pub version: String,
    pub information_uri: String,
    pub rules: Vec<Rule>,
}

impl Default for Tool {
    fn default() -> Tool {
        Tool {
            name: "burdin".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            information_uri: crate::URL_BASE.to_string(),
            rules: Vec::new(),
        }
    }
}

/// A SARIF `artifactLocation` for a path as diagnostics spell it. A relative path stays relative to the source root
/// (`%SRCROOT%`, which is where code-scanning services resolve it); an absolute path becomes a `file:` URI.
fn artifact_location(path: &str) -> Json {
    let normalized = path.replace('\\', "/");
    let (uri, absolute) = if let Some(rest) = normalized.strip_prefix("//") {
        (format!("file://{}", encode_uri(rest)), true)
    } else if normalized.starts_with('/') {
        (format!("file://{}", encode_uri(&normalized)), true)
    } else if normalized.len() >= 3 && normalized.as_bytes()[1] == b':' && normalized.as_bytes()[2] == b'/' {
        (format!("file:///{}", encode_uri(&normalized)), true)
    } else {
        (encode_uri(normalized.strip_prefix("./").unwrap_or(&normalized)), false)
    };
    let mut out = Json::object();
    out.set("uri", Json::Str(uri));
    if !absolute {
        out.set("uriBaseId", Json::str("%SRCROOT%"));
    }
    out
}

/// Percent-encodes everything a URI path does not allow. `/` and `:` stay, so a drive letter keeps its colon.
fn encode_uri(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// A SARIF `region`: lines and columns count Unicode code points (the run says so with `columnKind`), `endColumn` is
/// the column after the last character, and the byte offset and length say the same thing for a consumer that
/// indexes bytes.
fn region(sources: &SourceMap, span: Span) -> Json {
    let file = sources.get(span.file);
    let start = file.position(span.lo);
    let end = file.position(span.hi);
    Json::obj([
        ("startLine", Json::Num(start.line.into())),
        ("startColumn", Json::Num(start.column.into())),
        ("endLine", Json::Num(end.line.into())),
        ("endColumn", Json::Num(end.column.into())),
        ("byteOffset", Json::Num(span.lo.into())),
        ("byteLength", Json::Num((span.hi - span.lo).into())),
    ])
}

fn physical_location(sources: &SourceMap, span: Span) -> Json {
    Json::obj([(
        "physicalLocation",
        Json::obj([
            ("artifactLocation", artifact_location(&sources.get(span.file).path)),
            ("region", region(sources, span)),
        ]),
    )])
}

fn message(text: &str) -> Json {
    Json::obj([("text", Json::str(text))])
}

fn level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

fn result(d: &Diagnostic, sources: &SourceMap, rules: &[Rule]) -> Json {
    let mut out = Json::object();
    out.set("ruleId", Json::str(d.code.0));
    if let Some(index) = rules.iter().position(|rule| rule.id == d.code.0) {
        out.set("ruleIndex", Json::Num(index as u64));
    }
    out.set("level", Json::str(level(d.severity)));
    out.set("message", message(&d.message));
    out.set("locations", Json::Arr(vec![physical_location(sources, d.span)]));

    // Every other place the diagnostic points at: the notes and children that have a span, and the labels.
    let mut related: Vec<Json> = Vec::new();
    let mut add = |span: Span, text: &str| {
        let mut location = physical_location(sources, span);
        location.set("id", Json::Num(related.len() as u64));
        location.set("message", message(text));
        related.push(location);
    };
    for label in &d.labels {
        add(label.span, &label.message);
    }
    for note in &d.notes {
        if let Some(span) = note.span {
            add(span, &note.message);
        }
    }
    for child in &d.children {
        add(child.span, &format!("{}: {}", level(child.severity), child.message));
    }
    if !related.is_empty() {
        out.set("relatedLocations", Json::Arr(related));
    }

    // A suggestion is a fix: its edits become replacements of the file they are in.
    if !d.suggestions.is_empty() {
        let fixes = d
            .suggestions
            .iter()
            .map(|suggestion| {
                // One artifact change per file the edits touch, in the order the files first appear.
                let mut files: Vec<crate::FileId> = Vec::new();
                for edit in &suggestion.edits {
                    if !files.contains(&edit.span.file) {
                        files.push(edit.span.file);
                    }
                }
                let changes = files
                    .iter()
                    .map(|file| {
                        let replacements = suggestion
                            .edits
                            .iter()
                            .filter(|edit| edit.span.file == *file)
                            .map(|edit| {
                                Json::obj([
                                    ("deletedRegion", region(sources, edit.span)),
                                    ("insertedContent", Json::obj([("text", Json::str(edit.replacement.as_str()))])),
                                ])
                            })
                            .collect();
                        Json::obj([
                            ("artifactLocation", artifact_location(&sources.get(*file).path)),
                            ("replacements", Json::Arr(replacements)),
                        ])
                    })
                    .collect();
                Json::obj([("description", message(&suggestion.message)), ("artifactChanges", Json::Arr(changes))])
            })
            .collect();
        out.set("fixes", Json::Arr(fixes));
    }

    // What has no place in SARIF's result model travels as properties: the notes without a span, the help lines,
    // and whether a fix is one the tool applies on its own.
    let mut properties = Json::object();
    let spanless: Vec<Json> =
        d.notes.iter().filter(|note| note.span.is_none()).map(|note| Json::str(note.message.as_str())).collect();
    if !spanless.is_empty() {
        properties.set("notes", Json::Arr(spanless));
    }
    if !d.help.is_empty() {
        properties.set("help", Json::Arr(d.help.iter().map(|text| Json::str(text.as_str())).collect()));
    }
    if d.suggestions.iter().any(|s| s.is_machine_applicable()) {
        let flags = d.suggestions.iter().map(|s| Json::Bool(s.is_machine_applicable())).collect();
        properties.set("fixIsMachineApplicable", Json::Arr(flags));
    }
    if let Json::Obj(fields) = &properties
        && !fields.is_empty()
    {
        out.set("properties", properties);
    }
    out
}

fn rule(rule: &Rule) -> Json {
    let mut out = Json::object();
    out.set("id", Json::str(rule.id.as_str()));
    out.set("name", Json::str(rule.name.as_str()));
    out.set("shortDescription", message(&rule.name));
    if let Some(uri) = &rule.help_uri {
        out.set("helpUri", Json::str(uri.as_str()));
        out.set(
            "help",
            Json::obj([
                ("text", Json::Str(format!("See the entry for {} in the error reference: {uri}", rule.id))),
                (
                    "markdown",
                    Json::Str(format!(
                        "See the entry for {id} in the [error reference]({uri}) (`docs/error-codes.md`).",
                        id = rule.id
                    )),
                ),
            ]),
        );
    }
    out
}

/// The SARIF log of `diagnostics` (already sorted and limited). `hidden` counts the diagnostics an error limit
/// left out; the log records it as a property of the run so a consumer knows the list is partial.
pub(crate) fn log(diagnostics: &[&Diagnostic], sources: &SourceMap, tool: &Tool, hidden: usize) -> String {
    // A code the registry does not list (a lint, or `E9000`) still needs a rule for the result to point at.
    let mut rules = tool.rules.clone();
    for d in diagnostics {
        if !rules.iter().any(|rule| rule.id == d.code.0) {
            rules.push(Rule { id: d.code.0.to_string(), name: d.code.0.to_string(), help_uri: d.code.url() });
        }
    }
    let driver = Json::obj([
        ("name", Json::str(tool.name.as_str())),
        ("version", Json::str(tool.version.as_str())),
        ("informationUri", Json::str(tool.information_uri.as_str())),
        ("rules", Json::Arr(rules.iter().map(rule).collect())),
    ]);
    let succeeded = !diagnostics.iter().any(|d| d.severity == Severity::Error);
    let mut run = Json::object();
    run.set("tool", Json::obj([("driver", driver)]));
    run.set("columnKind", Json::str("unicodeCodePoints"));
    run.set("invocations", Json::Arr(vec![Json::obj([("executionSuccessful", Json::Bool(succeeded))])]));
    run.set("results", Json::Arr(diagnostics.iter().map(|d| result(d, sources, &rules)).collect()));
    if hidden > 0 {
        run.set("properties", Json::obj([("diagnosticsNotShown", Json::Num(hidden as u64))]));
    }
    let log = Json::obj([
        ("$schema", Json::str(SARIF_SCHEMA)),
        ("version", Json::str(SARIF_VERSION)),
        ("runs", Json::Arr(vec![run])),
    ]);
    format!("{log}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_stay_relative_and_absolute_paths_become_file_uris() {
        assert_eq!(
            artifact_location("src/a b.bd").to_string(),
            "{\"uri\":\"src/a%20b.bd\",\"uriBaseId\":\"%SRCROOT%\"}"
        );
        assert_eq!(artifact_location("./a.bd").to_string(), "{\"uri\":\"a.bd\",\"uriBaseId\":\"%SRCROOT%\"}");
        assert_eq!(artifact_location("/home/me/a.bd").to_string(), "{\"uri\":\"file:///home/me/a.bd\"}");
        assert_eq!(artifact_location("C:\\work\\a.bd").to_string(), "{\"uri\":\"file:///C:/work/a.bd\"}");
        assert_eq!(artifact_location("\\\\server\\share\\a.bd").to_string(), "{\"uri\":\"file://server/share/a.bd\"}");
    }
}
