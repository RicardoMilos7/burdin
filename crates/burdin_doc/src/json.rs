//! `burdin doc --output-format json` (`spec/doc.md`, DC19): the documented API as one JSON document with a versioned
//! schema, for API diffing, search engines, and other renderers. The schema is the one in `spec/doc.md`; the version
//! changes only when a field is removed or changes meaning.

use crate::model::*;
use burdin_diag::{SourceMap, Span};
use std::fmt::Write;

/// The version of the JSON schema.
pub const FORMAT_VERSION: u32 = 1;

enum Json {
    Null,
    Bool(bool),
    Num(u64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(&'static str, Json)>),
}

impl Json {
    fn str(s: &str) -> Json {
        Json::Str(s.to_string())
    }

    fn opt(s: &Option<String>) -> Json {
        s.as_ref().map_or(Json::Null, |s| Json::Str(s.clone()))
    }

    fn write(&self, out: &mut String, depth: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                let _ = write!(out, "{n}");
            }
            Json::Str(s) => quote(s, out),
            Json::Arr(items) if items.is_empty() => out.push_str("[]"),
            Json::Arr(items) => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    indent(out, depth + 1);
                    item.write(out, depth + 1);
                    out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                }
                indent(out, depth);
                out.push(']');
            }
            Json::Obj(fields) => {
                out.push_str("{\n");
                for (i, (key, value)) in fields.iter().enumerate() {
                    indent(out, depth + 1);
                    quote(key, out);
                    out.push_str(": ");
                    value.write(out, depth + 1);
                    out.push_str(if i + 1 < fields.len() { ",\n" } else { "\n" });
                }
                indent(out, depth);
                out.push('}');
            }
        }
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn span_json(sources: &SourceMap, span: Span, file_path: &str) -> Json {
    let file = sources.get(span.file);
    let (line, column) = file.line_col(span.lo);
    let (end_line, end_column) = file.line_col(span.hi);
    Json::Obj(vec![
        ("file", Json::str(file_path)),
        ("line", Json::Num(line as u64)),
        ("column", Json::Num(column as u64)),
        ("end_line", Json::Num(end_line as u64)),
        ("end_column", Json::Num(end_column as u64)),
    ])
}

fn docs_json(doc: &Option<Doc>) -> Json {
    doc.as_ref().map_or(Json::Null, |d| Json::Str(d.text.clone()))
}

fn path_of(module: &[String], name: &str, member: Option<&str>) -> String {
    let mut parts: Vec<&str> = module.iter().map(String::as_str).collect();
    parts.push(name);
    if let Some(m) = member {
        parts.push(m);
    }
    parts.join(".")
}

fn resolution_json(res: &Resolution) -> (&'static str, Json) {
    match res {
        Resolution::Module(path) => ("module", Json::Str(path.join("."))),
        Resolution::Item { module, name, member } => ("item", Json::Str(path_of(module, name, member.as_deref()))),
        Resolution::External => ("external", Json::Null),
        Resolution::Hidden => ("hidden", Json::Null),
        Resolution::Broken => ("broken", Json::Null),
    }
}

fn links_json(doc: &Option<Doc>) -> Json {
    let links = doc.as_ref().map_or(&[][..], |d| d.links.as_slice());
    Json::Arr(
        links
            .iter()
            .map(|l| {
                let (status, path) = resolution_json(&l.resolution);
                Json::Obj(vec![
                    ("text", Json::str(&l.text)),
                    ("target", Json::str(&l.target)),
                    ("status", Json::str(status)),
                    ("path", path),
                ])
            })
            .collect(),
    )
}

fn member_json(sources: &SourceMap, file_path: &str, m: &Member) -> Json {
    let origin = match &m.origin {
        Some(o) => Json::Obj(vec![("module", Json::str(&o.module)), ("import", Json::opt(&o.import))]),
        None => Json::Null,
    };
    Json::Obj(vec![
        ("name", Json::str(&m.name)),
        ("display", Json::str(&m.display)),
        ("kind", Json::str(m.kind.label())),
        ("signature", Json::str(&m.signature)),
        ("private", Json::Bool(m.private)),
        ("docs", docs_json(&m.docs)),
        ("links", links_json(&m.docs)),
        ("span", span_json(sources, m.span, file_path)),
        ("instance", Json::opt(&m.instance)),
        ("origin", origin),
    ])
}

fn members_json(sources: &SourceMap, file_path: &str, members: &[Member]) -> Json {
    Json::Arr(members.iter().map(|m| member_json(sources, file_path, m)).collect())
}

fn item_json(sources: &SourceMap, module: &Module, item: &Item) -> Json {
    let file = module.file_path.as_str();
    let by_kind = |kind: MemberKind| -> Json {
        Json::Arr(item.members.iter().filter(|m| m.kind == kind).map(|m| member_json(sources, file, m)).collect())
    };
    let implementations = Json::Arr(
        item.implementations
            .iter()
            .map(|i| {
                let target = i.target.as_ref().map_or(Json::Null, |t| resolution_json(t).1);
                Json::Obj(vec![
                    ("interface", Json::str(&i.interface)),
                    ("derived", Json::Bool(i.derived)),
                    ("instance", Json::opt(&i.instance)),
                    ("where", Json::str(&i.where_clause)),
                    ("target", target),
                    ("members", members_json(sources, file, &i.members)),
                ])
            })
            .collect(),
    );
    Json::Obj(vec![
        ("name", Json::str(&item.name)),
        ("path", Json::Str(path_of(&module.path, &item.name, None))),
        ("kind", Json::str(item.kind.label())),
        ("private", Json::Bool(item.private)),
        ("signature", Json::str(&item.signature)),
        ("docs", docs_json(&item.docs)),
        ("links", links_json(&item.docs)),
        ("span", span_json(sources, item.span, file)),
        ("fields", by_kind(MemberKind::Field)),
        ("variants", by_kind(MemberKind::Variant)),
        ("methods", members_json(sources, file, &item.methods)),
        ("implementations", implementations),
        ("operators", members_json(sources, file, &item.operators)),
        ("extensions", members_json(sources, file, &item.extensions)),
        ("reexported_by", Json::Arr(item.reexported_by.iter().map(|s| Json::str(s)).collect())),
    ])
}

fn module_json(api: &Api, sources: &SourceMap, m: &Module) -> Json {
    let file = m.file_path.as_str();
    let reexports = Json::Arr(
        m.reexports
            .iter()
            .map(|r| {
                let (status, path) = resolution_json(&r.target);
                Json::Obj(vec![
                    ("name", Json::str(&r.name)),
                    ("path", Json::str(&r.path)),
                    ("status", Json::str(status)),
                    ("target", path),
                    ("span", span_json(sources, r.span, file)),
                ])
            })
            .collect(),
    );
    Json::Obj(vec![
        ("name", Json::Str(m.display(&api.package))),
        ("segments", Json::Arr(m.path.iter().map(|s| Json::str(s)).collect())),
        ("file", Json::str(file)),
        ("root", Json::Bool(m.is_root)),
        ("docs", docs_json(&m.docs)),
        ("links", links_json(&m.docs)),
        ("items", Json::Arr(m.items.iter().map(|i| item_json(sources, m, i)).collect())),
        ("extensions", members_json(sources, file, &m.extensions)),
        ("reexports", reexports),
    ])
}

/// The documented API as JSON text, ending in a newline.
pub fn render(api: &Api, sources: &SourceMap) -> String {
    let doc = Json::Obj(vec![
        ("format_version", Json::Num(FORMAT_VERSION as u64)),
        ("package", Json::str(&api.package)),
        ("private_items", Json::Bool(api.private)),
        ("modules", Json::Arr(api.modules.iter().map(|m| module_json(api, sources, m)).collect())),
    ]);
    let mut out = String::new();
    doc.write(&mut out, 0);
    out.push('\n');
    out
}
