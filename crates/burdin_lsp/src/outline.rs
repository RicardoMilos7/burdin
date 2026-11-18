//! The shape of a file and a workspace (`spec/lsp.md`, LS16-LS17): document and workspace symbols, folding ranges,
//! selection ranges, and document links.

use crate::index::{Item, Kind, Tk};
use crate::json::Json;
use crate::nav::{Resolver, symbol_kind};
use crate::project::Loc;
use crate::server::{Reply, ReqError, Server};
use crate::text::LineIndex;
use burdin_query::TriviaKind;
use std::collections::BTreeSet;

// ---- symbols ----------------------------------------------------------------------------------------------------

pub fn document_symbols(server: &mut Server, params: &Json) -> Reply {
    let (i, path) = server.document(params, false)?;
    let text = server.packages[i].text(&path).unwrap_or("").to_string();
    let index = LineIndex::new(&text, server.encoding);
    let Some(ix) = server.packages[i].index(&path) else { return Ok(Json::Arr(Vec::new())) };
    if !server.caps.hierarchical_symbols {
        let uri = server.uri_of(&path);
        let flat: Vec<Json> = ix
            .items
            .iter()
            .map(|item| {
                let mut s = Json::obj().with("name", item.name.as_str()).with("kind", symbol_kind(item.kind)).with(
                    "location",
                    Json::obj().with("uri", uri.as_str()).with("range", index.range(item.span.0, item.span.1)),
                );
                if let Some(parent) = item.parent {
                    s.set("containerName", ix.items[parent].name.as_str());
                }
                s
            })
            .collect();
        return Ok(Json::Arr(flat));
    }
    fn build(items: &[Item], parent: Option<usize>, index: &LineIndex) -> Vec<Json> {
        items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.parent == parent)
            .map(|(n, item)| {
                let children = build(items, Some(n), index);
                let detail = match item.kind {
                    Kind::Test => "test".to_string(),
                    _ => item.detail.clone(),
                };
                let mut s = Json::obj()
                    .with("name", item.name.as_str())
                    .with("detail", detail)
                    .with("kind", symbol_kind(item.kind))
                    .with("range", index.range(item.span.0, item.span.1))
                    .with("selectionRange", index.range(item.name_span.0, item.name_span.1));
                if !children.is_empty() {
                    s.set("children", Json::Arr(children));
                }
                s
            })
            .collect()
    }
    Ok(Json::Arr(build(&ix.items, None, &index)))
}

/// Whether every character of `query` appears in `name` in order, ignoring case.
fn fuzzy(query: &str, name: &str) -> bool {
    let mut chars = name.chars().flat_map(char::to_lowercase);
    query.chars().flat_map(char::to_lowercase).all(|q| chars.any(|c| c == q))
}

pub fn workspace_symbols(server: &mut Server, params: &Json) -> Reply {
    let query = params.get("query").as_str().unwrap_or("").to_string();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut found: Vec<(u8, String, Loc, Kind, String)> = Vec::new();
    for i in 0..server.packages.len() {
        for path in server.packages[i].user_files() {
            if !seen.insert(path.clone()) {
                continue;
            }
            let module = server.packages[i].module(&path).map(|m| m.module.join(".")).unwrap_or_default();
            let Some(ix) = server.packages[i].index(&path) else { continue };
            for item in ix.items.iter().filter(|i| i.kind != Kind::Impl) {
                if !fuzzy(&query, &item.name) {
                    continue;
                }
                let lower = item.name.to_lowercase();
                let q = query.to_lowercase();
                let rank = if lower == q {
                    0
                } else if lower.starts_with(&q) {
                    1
                } else {
                    2
                };
                let container = match &item.owner {
                    Some(owner) if !module.is_empty() => format!("{module}.{owner}"),
                    Some(owner) => owner.clone(),
                    None => module.clone(),
                };
                found.push((
                    rank,
                    item.name.clone(),
                    Loc::new(&path, item.name_span.0, item.name_span.1),
                    item.kind,
                    container,
                ));
            }
        }
    }
    found.sort();
    found.truncate(500);
    Ok(Json::Arr(
        found
            .iter()
            .map(|(_, name, loc, kind, container)| {
                Json::obj()
                    .with("name", name.as_str())
                    .with("kind", symbol_kind(*kind))
                    .with("location", server.location(loc))
                    .with("containerName", container.as_str())
            })
            .collect(),
    ))
}

// ---- folding ----------------------------------------------------------------------------------------------------

pub fn folding_ranges(server: &mut Server, params: &Json) -> Reply {
    let (i, path) = server.document(params, false)?;
    let text = server.packages[i].text(&path).unwrap_or("").to_string();
    let index = LineIndex::new(&text, server.encoding);
    let line = |offset: u32| index.position(offset).0;
    let mut ranges: Vec<(u32, u32, Option<&str>)> = Vec::new();
    let Some(ix) = server.packages[i].index(&path) else { return Ok(Json::Arr(Vec::new())) };
    // Blocks and multi-line lists: matching brackets on different lines. The closing line stays visible.
    let mut stack: Vec<(Tk, u32)> = Vec::new();
    for t in &ix.tokens {
        match t.kind {
            Tk::LBrace | Tk::LBracket | Tk::LParen => stack.push((t.kind.clone(), t.lo)),
            Tk::RBrace | Tk::RBracket | Tk::RParen => {
                if let Some((_, open)) = stack.pop() {
                    let (a, b) = (line(open), line(t.lo));
                    if b > a + 1 || (b > a && t.kind == Tk::RBrace) {
                        ranges.push((a, b - 1, None));
                    }
                }
            }
            _ => {}
        }
    }
    // Runs of `use` declarations.
    let mut uses: Vec<(u32, u32)> = ix.uses.iter().map(|u| (line(u.span.0), line(u.span.1))).collect();
    uses.sort();
    let mut run: Option<(u32, u32)> = None;
    for (a, b) in uses {
        run = match run {
            Some((s, e)) if a <= e + 1 => Some((s, e.max(b))),
            Some((s, e)) => {
                if e > s {
                    ranges.push((s, e, Some("imports")));
                }
                Some((a, b))
            }
            None => Some((a, b)),
        };
    }
    if let Some((s, e)) = run
        && e > s
    {
        ranges.push((s, e, Some("imports")));
    }
    // Comments: block comments over several lines, and runs of line comments.
    if let Some(tree) = server.packages[i].db.syntax_tree(&path) {
        let mut run: Option<(u32, u32)> = None;
        for t in &tree.trivia {
            match t.kind {
                TriviaKind::BlockComment => {
                    let (a, b) = (line(t.lo), line(t.hi));
                    if b > a {
                        ranges.push((a, b, Some("comment")));
                    }
                }
                TriviaKind::LineComment | TriviaKind::DocComment => {
                    let l = line(t.lo);
                    run = match run {
                        Some((s, e)) if l == e + 1 => Some((s, l)),
                        Some((s, e)) => {
                            if e > s {
                                ranges.push((s, e, Some("comment")));
                            }
                            Some((l, l))
                        }
                        None => Some((l, l)),
                    };
                }
                TriviaKind::Whitespace => {}
            }
        }
        if let Some((s, e)) = run
            && e > s
        {
            ranges.push((s, e, Some("comment")));
        }
    }
    ranges.sort();
    ranges.dedup();
    Ok(Json::Arr(
        ranges
            .into_iter()
            .map(|(a, b, kind)| {
                let mut r = Json::obj().with("startLine", a).with("endLine", b);
                if let Some(kind) = kind {
                    r.set("kind", kind);
                }
                r
            })
            .collect(),
    ))
}

// ---- selection ranges -------------------------------------------------------------------------------------------

pub fn selection_ranges(server: &mut Server, params: &Json) -> Reply {
    let (i, path) = server.document(params, false)?;
    let text = server.packages[i].text(&path).unwrap_or("").to_string();
    let index = LineIndex::new(&text, server.encoding);
    let parsed = server.packages[i].parsed(&path).ok_or_else(|| ReqError::failed("the file has no syntax tree"))?;
    let ix = server.packages[i].index(&path).ok_or_else(|| ReqError::failed("the file has no syntax tree"))?;
    // Every span the parser recorded, plus the tokens, the items, and the whole file.
    let mut spans: Vec<(u32, u32)> = Vec::new();
    let mut tree = parsed.ast.clone();
    burdin_syntax::spans::map_spans(&mut tree, &mut |s| spans.push((s.lo, s.hi)));
    spans.extend(ix.tokens.iter().map(|t| (t.lo, t.hi)));
    spans.extend(ix.items.iter().map(|t| t.span));
    spans.push((0, text.len() as u32));
    // Generated code carries one identifier's span, which nests like the identifier itself.
    spans.retain(|(lo, hi)| lo < hi && (*hi as usize) <= text.len());
    spans.sort_by_key(|(lo, hi)| (hi - lo, *lo));
    spans.dedup();
    let mut out = Vec::new();
    for position in params.get("positions").as_array() {
        let offset = index.offset_of(position).unwrap_or(0);
        let mut chain: Vec<(u32, u32)> = Vec::new();
        for &(lo, hi) in &spans {
            if lo <= offset
                && offset <= hi
                && chain.last().is_none_or(|&(a, b)| lo <= a && b <= hi && (lo, hi) != (a, b))
            {
                chain.push((lo, hi));
            }
        }
        let mut node = Json::Null;
        for (lo, hi) in chain.into_iter().rev() {
            let mut range = Json::obj().with("range", index.range(lo, hi));
            if !node.is_null() {
                range.set("parent", node);
            }
            node = range;
        }
        out.push(node);
    }
    Ok(Json::Arr(out))
}

// ---- document links ---------------------------------------------------------------------------------------------

pub fn document_links(server: &mut Server, params: &Json) -> Reply {
    let (i, path) = server.document(params, false)?;
    let text = server.packages[i].text(&path).unwrap_or("").to_string();
    let index = LineIndex::new(&text, server.encoding);
    let mut links: Vec<(u32, u32, String, String)> = Vec::new();
    let package = server.packages[i].package_of(&path);
    let ix = server.packages[i].index(&path).ok_or_else(|| ReqError::failed("the file has no syntax tree"))?;
    // `use` paths: the longest prefix that names a module links to its file.
    if let Some(package) = package {
        for u in &ix.uses {
            let mut segments: Vec<String> = Vec::new();
            let mut best: Option<(u32, String)> = None;
            for (name, span) in &u.path {
                segments.push(name.clone());
                if let Some(file) = server.packages[i].module_file(package, &segments) {
                    best = Some((span.1, file));
                }
            }
            if let (Some((hi, file)), Some(first)) = (best, u.path.first()) {
                links.push((first.1.0, hi, server.uri_of(&file), format!("open module {}", segments.join("."))));
            }
        }
    }
    // Links in comments: URLs, and `[name]` references in documentation comments.
    if let Some(tree) = server.packages[i].db.syntax_tree(&path) {
        let comments: Vec<(u32, u32, TriviaKind)> =
            tree.trivia.iter().filter(|t| t.kind != TriviaKind::Whitespace).map(|t| (t.lo, t.hi, t.kind)).collect();
        for (lo, hi, kind) in comments {
            let comment = &text[lo as usize..hi as usize];
            for scheme in ["https://", "http://"] {
                let mut from = 0;
                while let Some(at) = comment[from..].find(scheme) {
                    let start = from + at;
                    let end = comment[start..]
                        .find(|c: char| c.is_whitespace() || matches!(c, ')' | '>' | '`' | '"'))
                        .map_or(comment.len(), |e| start + e);
                    let url = comment[start..end].trim_end_matches(['.', ',']).to_string();
                    links.push((lo + start as u32, lo + (start + url.len()) as u32, url, "open the link".to_string()));
                    from = end;
                }
            }
            if kind != TriviaKind::DocComment {
                continue;
            }
            let mut from = 0;
            while let Some(open) = comment[from..].find('[') {
                let start = from + open;
                let Some(close) = comment[start..].find(']') else { break };
                let end = start + close;
                from = end + 1;
                if comment[from..].starts_with('(') {
                    continue;
                }
                let name = comment[start + 1..end].trim_matches('`');
                if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
                    continue;
                }
                let last = name.rsplit('.').next().unwrap_or(name).to_string();
                let mut r = Resolver::new(&mut server.packages[i]);
                let all = r.all_items();
                let target = all
                    .iter()
                    .filter(|(_, it)| it.name == last && it.kind != Kind::Impl)
                    .min_by_key(|(p, _)| (p != &path, p.starts_with("<std>/"), p.clone()))
                    .map(|(p, it)| Loc::new(p, it.name_span.0, it.name_span.1));
                if let Some(target) = target {
                    let target_text = server.text_of(&target.path).unwrap_or_default();
                    let line = LineIndex::new(&target_text, server.encoding).position(target.lo).0 + 1;
                    let uri = format!("{}#L{line}", server.uri_of(&target.path));
                    links.push((lo + start as u32 + 1, lo + end as u32, uri, format!("go to {name}")));
                }
            }
        }
    }
    links.sort();
    Ok(Json::Arr(
        links
            .into_iter()
            .map(|(lo, hi, target, tooltip)| {
                Json::obj().with("range", index.range(lo, hi)).with("target", target).with("tooltip", tooltip)
            })
            .collect(),
    ))
}
