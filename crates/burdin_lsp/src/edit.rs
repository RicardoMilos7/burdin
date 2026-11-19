//! Help while editing (`spec/lsp.md`, LS11-LS15): hover, completion, signature help, inlay hints, semantic tokens,
//! and formatting through `burdin fmt`.

use crate::index::{FileIndex, Item, KEYWORDS, Kind, Tk};
use crate::json::Json;
use crate::nav::{Resolver, SymKind, base_type};
use crate::project::{Loc, Package};
use crate::server::{Reply, ReqError, Server};
use crate::text::LineIndex;
use burdin_query::{Analysis, DefinitionKind};
use std::collections::HashMap;
use std::rc::Rc;

/// The semantic token types, in legend order.
pub const TOKEN_TYPES: &[&str] = &[
    "namespace",
    "type",
    "struct",
    "enum",
    "interface",
    "enumMember",
    "function",
    "method",
    "parameter",
    "variable",
    "property",
];

/// The semantic token modifiers, in legend order (bit `i` is modifier `i`).
pub const TOKEN_MODIFIERS: &[&str] = &["declaration", "readonly", "mutable", "defaultLibrary"];

/// Functions every program can call without a `use` (the compiler's built-ins).
const BUILTIN_FUNCTIONS: &[(&str, &str)] = &[
    ("print", "fn print(value)"),
    ("println", "fn println(value)"),
    ("eprint", "fn eprint(value)"),
    ("eprintln", "fn eprintln(value)"),
    ("panic", "fn panic(message: str): !"),
    ("format", "fn format(template: str, ...): String"),
];

/// Types every program can name without a `use`.
const BUILTIN_TYPES: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "bool",
    "char", "str", "String", "List", "Map", "Option", "Result", "Heap",
];

// ---- hover ------------------------------------------------------------------------------------------------------

pub fn hover(server: &mut Server, params: &Json) -> Reply {
    let (i, path) = server.document(params, true)?;
    let offset = server.offset(i, &path, params.get("position"))?;
    let mut hovered_local: Option<(String, u32)> = None;
    let (mut markdown, at) = {
        let mut r = Resolver::new(&mut server.packages[i]);
        let tok = r.index(&path).and_then(|ix| ix.ident_at(offset).map(|t| Loc::new(&path, t.lo, t.hi)));
        let (markdown, at) = match r.symbol_at(&path, offset) {
            Some(symbol) => {
                let code = match symbol.kind {
                    SymKind::Local => format!("let {}: {}", symbol.name, symbol.ty.clone().unwrap_or_default()),
                    SymKind::Parameter => format!("{}: {}", symbol.name, symbol.ty.clone().unwrap_or_default()),
                    SymKind::Module => format!("module {}", symbol.name),
                    SymKind::Item(_) => symbol.detail.clone().unwrap_or_default(),
                };
                let mut text = format!("```burdin\n{code}\n```");
                match (symbol.kind, &symbol.owner) {
                    (SymKind::Item(Kind::Method | Kind::InterfaceMethod), Some(owner)) => {
                        text.push_str(&format!("\n\nmethod of `{owner}`"));
                        if let Some(iface) = &symbol.interface {
                            text.push_str(&format!(" (implements `{iface}`)"));
                        }
                    }
                    (SymKind::Item(Kind::Field), Some(owner)) => text.push_str(&format!("\n\nfield of `{owner}`")),
                    (SymKind::Item(Kind::Variant), Some(owner)) => text.push_str(&format!("\n\nvariant of `{owner}`")),
                    (SymKind::Parameter, _) => text.push_str("\n\nparameter"),
                    _ => {}
                }
                if let Some(docs) = &symbol.docs {
                    text.push_str("\n\n---\n\n");
                    text.push_str(docs);
                }
                // A local or parameter also lists what the checker saw done to it (`spec/lsp.md`, LS11/LS13).
                if matches!(symbol.kind, SymKind::Local | SymKind::Parameter) {
                    hovered_local = Some((symbol.name.clone(), symbol.def.lo));
                }
                (text, tok)
            }
            None => match r.p.current().and_then(|a| a.type_at(&path, offset)) {
                Some(typed) => (format!("```burdin\n{}\n```", typed.ty), Some(Loc::from(&typed.at))),
                None => return Ok(Json::Null),
            },
        };
        (markdown, at)
    };
    if let Some((name, declared)) = hovered_local
        && let Some(notes) = ownership_notes(server, i, &path, &name, declared)
    {
        markdown.push_str(&notes);
    }
    let mut out = Json::obj().with("contents", Json::obj().with("kind", "markdown").with("value", markdown));
    if let Some(at) = at {
        out.set("range", server.range_of(&at));
    }
    Ok(out)
}

/// The ownership a local or parameter took part in, as a hover section (`spec/lsp.md`, LS11/LS13). It reads the
/// events the memory checks recorded (`spec/check.md`, CK10); nothing is recomputed here.
fn ownership_notes(server: &Server, package: usize, path: &str, name: &str, declared: u32) -> Option<String> {
    let p = &server.packages[package];
    let analysis = p.current()?;
    let function = analysis.ownership_at(path, declared)?;
    let text = p.text(path).unwrap_or("");
    let index = LineIndex::new(text, server.encoding);
    let mut lines = Vec::new();
    for event in &function.events {
        if event.local.as_deref() != Some(name) || event.at.lo < declared {
            continue;
        }
        let what = match &event.kind {
            burdin_query::OwnershipEventKind::Move { target } => match target {
                Some(target) => format!("moved to `{target}`"),
                None => "moved".to_string(),
            },
            burdin_query::OwnershipEventKind::Copy => "copied".to_string(),
            burdin_query::OwnershipEventKind::BorrowRead => "borrowed (read)".to_string(),
            burdin_query::OwnershipEventKind::BorrowMut => "borrowed (mut)".to_string(),
            burdin_query::OwnershipEventKind::Drop { conditional } => {
                format!("dropped{}", if *conditional { " (conditionally)" } else { "" })
            }
            burdin_query::OwnershipEventKind::Clone { derived } => {
                format!("cloned{}", if *derived { " (derived)" } else { "" })
            }
            _ => continue,
        };
        let (line, column) = index.position(event.at.lo);
        lines.push(format!("- {what} at {}:{}", line + 1, column + 1));
    }
    (!lines.is_empty()).then(|| format!("\n\n**Ownership**\n{}", lines.join("\n")))
}

// ---- stale analyses -----------------------------------------------------------------------------------------------

/// The offset in `old` of an offset of `new`, when it lies outside the one changed stretch between them. The start of
/// the stretch belongs to it: text typed there is new, even when it happens to begin like the old text after it.
fn map_to_old(old: &str, new: &str, offset: u32) -> Option<u32> {
    let (prefix, suffix) = common(old, new);
    let offset = offset as usize;
    if offset < prefix {
        Some(offset as u32)
    } else if offset >= new.len() - suffix {
        Some((old.len() - (new.len() - offset)) as u32)
    } else {
        None
    }
}

/// The offset in `new` of an offset of `old`.
fn map_to_new(old: &str, new: &str, offset: u32) -> Option<u32> {
    map_to_old(new, old, offset)
}

/// The lengths of the common prefix and suffix of two texts, on character boundaries.
fn common(a: &str, b: &str) -> (usize, usize) {
    let (x, y) = (a.as_bytes(), b.as_bytes());
    let mut prefix = x.iter().zip(y).take_while(|(p, q)| p == q).count();
    while prefix > 0 && !(a.is_char_boundary(prefix) && b.is_char_boundary(prefix)) {
        prefix -= 1;
    }
    let max = x.len().min(y.len()) - prefix;
    let mut suffix = x.iter().rev().zip(y.iter().rev()).take(max).take_while(|(p, q)| p == q).count();
    while suffix > 0 && !(a.is_char_boundary(x.len() - suffix) && b.is_char_boundary(y.len() - suffix)) {
        suffix -= 1;
    }
    (prefix, suffix)
}

/// An analysis with a program and a way to translate positions of the current text into it.
struct Answering<'a> {
    analysis: &'a Analysis,
    old: Option<Rc<str>>,
    new: String,
}

impl Answering<'_> {
    fn of<'a>(p: &'a Package, path: &str) -> Option<Answering<'a>> {
        let (analysis, texts) = p.program_analysis()?;
        let new = p.text(path)?.to_string();
        let old = texts.and_then(|t| t.get(path).cloned());
        Some(Answering { analysis, old, new })
    }

    /// The offset in the analyzed text of an offset of the current text.
    fn old_offset(&self, offset: u32) -> Option<u32> {
        match &self.old {
            Some(old) => map_to_old(old, &self.new, offset),
            None => Some(offset),
        }
    }

    fn new_offset(&self, offset: u32) -> Option<u32> {
        match &self.old {
            Some(old) => map_to_new(old, &self.new, offset),
            None => Some(offset),
        }
    }

    fn type_at(&self, path: &str, offset: u32) -> Option<String> {
        let at = self.old_offset(offset)?;
        self.analysis.type_at(path, at).map(|t| t.ty)
    }

    /// The type of the local `name` declared last before `offset` of the current text: what a name typed since the
    /// last good analysis most likely stands for.
    fn local_before(&self, path: &str, name: &str, offset: u32) -> Option<String> {
        self.analysis
            .locals(path)
            .into_iter()
            .filter(|l| l.name == name)
            .filter_map(|l| Some((self.new_offset(l.at.lo)?, l.ty)))
            .filter(|(at, _)| *at < offset)
            .max_by_key(|(at, _)| *at)
            .map(|(_, ty)| ty)
    }
}

// ---- completion -------------------------------------------------------------------------------------------------

fn completion_kind(kind: Kind) -> u32 {
    match kind {
        Kind::Function | Kind::ExternFn | Kind::Test => 3,
        Kind::Method | Kind::InterfaceMethod => 2,
        Kind::Field => 5,
        Kind::Struct | Kind::Alias => 22,
        Kind::Enum => 13,
        Kind::Variant => 20,
        Kind::Interface | Kind::Derive => 8,
        Kind::Const => 21,
        Kind::Impl => 7,
    }
}

struct Completions {
    items: Vec<Json>,
    seen: Vec<String>,
}

impl Completions {
    fn add(&mut self, label: &str, kind: u32, detail: &str, docs: Option<&str>, group: u8) {
        if self.seen.iter().any(|s| s == label) {
            return;
        }
        self.seen.push(label.to_string());
        let mut item = Json::obj()
            .with("label", label)
            .with("kind", kind)
            .with("detail", detail)
            .with("sortText", format!("{group}{label}"));
        if let Some(docs) = docs {
            item.set("documentation", Json::obj().with("kind", "markdown").with("value", docs));
        }
        self.items.push(item);
    }

    fn item(&mut self, item: &Item, group: u8) {
        if item.kind == Kind::Impl || item.kind == Kind::Test || item.name.starts_with("__") {
            return;
        }
        self.add(&item.name, completion_kind(item.kind), &item.detail, item.docs.as_deref(), group);
    }
}

pub fn completion(server: &mut Server, params: &Json) -> Reply {
    let (i, path) = server.document(params, false)?;
    server.analyzed(i)?;
    let offset = server.offset(i, &path, params.get("position"))?;
    let text = server.packages[i].text(&path).unwrap_or("").to_string();
    let mut start = offset as usize;
    while start > 0 && (text.as_bytes()[start - 1].is_ascii_alphanumeric() || text.as_bytes()[start - 1] == b'_') {
        start -= 1;
    }
    let mut out = Completions { items: Vec::new(), seen: Vec::new() };
    let line_start = text[..start].rfind('\n').map_or(0, |n| n + 1);
    let line = text[line_start..start].trim_start();
    let in_use = line.starts_with("use ") || line.starts_with("pub use ");
    let mut r = Resolver::new(&mut server.packages[i]);
    let ix = r.index(&path).unwrap_or_default();
    if in_use {
        use_completions(&mut r, &path, line, &mut out);
    } else if start > 0 && text.as_bytes()[start - 1] == b'.' {
        member_completions(&mut r, &path, &ix, (start - 1) as u32, &mut out);
    } else {
        scope_completions(&mut r, &path, &ix, offset, &mut out);
    }
    let mut items = out.items;
    items.sort_by(|a, b| a.get("sortText").as_str().cmp(&b.get("sortText").as_str()));
    Ok(Json::obj().with("isIncomplete", false).with("items", Json::Arr(items)))
}

/// After `use a.b.`: the modules and items below `a.b`; at the start, the modules and packages.
fn use_completions(r: &mut Resolver, path: &str, line: &str, out: &mut Completions) {
    let Some(package) = r.p.package_of(path) else { return };
    let after = line.trim_start_matches("pub ").trim_start_matches("use ").trim();
    let written = after.split('{').next().unwrap_or(after);
    let segments: Vec<String> =
        written.split('.').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
    let complete = if written.ends_with('.') || after.contains('{') {
        segments.clone()
    } else {
        segments[..segments.len().saturating_sub(1)].to_vec()
    };
    for (module, _) in r.p.visible_modules(package) {
        if module.len() > complete.len() && module[..complete.len()] == complete[..] {
            out.add(&module[complete.len()], 9, &format!("module {}", module.join(".")), None, 0);
        }
    }
    if complete.is_empty() {
        out.add("std", 9, "the standard library", None, 1);
    } else if let Some(file) = r.p.module_file(package, &complete)
        && let Some(ix) = r.index(&file)
    {
        for item in ix.items.iter().filter(|i| i.parent.is_none() && !i.is_priv) {
            out.item(item, 1);
        }
    }
}

/// After `x.`: a module's items, a type's variants and methods, or a value's fields and methods.
fn member_completions(r: &mut Resolver, path: &str, ix: &FileIndex, dot: u32, out: &mut Completions) {
    let Some(ti) = ix.tokens.iter().position(|t| t.hi == dot) else { return };
    let receiver = ix.tokens[ti].clone();

    if let Tk::Ident(name) = &receiver.kind {
        let is_value = r.p.program_analysis().is_some_and(|(a, _)| {
            a.resolve_at(path, receiver.lo)
                .is_some_and(|x| matches!(x.kind, DefinitionKind::Local | DefinitionKind::Parameter))
        });
        if !is_value {
            if let Some(file) = module_file_named(r, path, ix, name)
                && let Some(mix) = r.index(&file)
            {
                for item in mix.items.iter().filter(|i| i.parent.is_none()) {
                    out.item(item, 0);
                }
                return;
            }
            let all = r.all_items();
            if all.iter().any(|(_, i)| i.kind.is_type() && i.name == *name) {
                for (_, item) in all.iter().filter(|(_, i)| i.owner.as_deref() == Some(name.as_str())) {
                    if matches!(item.kind, Kind::Variant | Kind::Method | Kind::InterfaceMethod) {
                        out.item(item, if item.kind == Kind::Variant { 0 } else { 1 });
                    }
                }
                return;
            }
        }
    }
    let ty = Answering::of(r.p, path).and_then(|a| {
        // A half-written member (`sq.` before the next line) may parse as part of a larger expression that does not
        // type-check; the receiver's own declaration still says what it is.
        a.type_at(path, receiver.lo).filter(|t| !t.contains("{error}")).or_else(|| match &receiver.kind {
            Tk::Ident(name) => a.local_before(path, name, receiver.lo),
            _ => None,
        })
    });
    let Some(ty) = ty else { return };
    let owner = base_type(&ty);
    let all = r.all_items();
    for (_, item) in all.iter().filter(|(_, i)| i.owner.as_deref() == Some(owner.as_str())) {
        match item.kind {
            Kind::Field => out.item(item, 0),
            Kind::Method | Kind::InterfaceMethod => out.item(item, 1),
            _ => {}
        }
    }
}

/// The module a name stands for in a file: an alias, an imported module, a module of the package, or a dependency.
fn module_file_named(r: &mut Resolver, path: &str, ix: &FileIndex, name: &str) -> Option<String> {
    let package = r.p.package_of(path)?;
    for u in &ix.uses {
        let segments: Vec<String> = u.path.iter().map(|(s, _)| s.clone()).collect();
        if (u.alias.as_ref().is_some_and(|(a, _)| a == name)
            || (u.items.is_empty() && u.alias.is_none() && segments.last().is_some_and(|s| s == name)))
            && let Some(f) = r.p.module_file(package, &segments)
        {
            return Some(f);
        }
    }
    r.p.module_file(package, &[name.to_string()])
}

/// A plain name: the locals in scope, the file's items, what its `use` declarations import, the built-ins, and the
/// keywords.
fn scope_completions(r: &mut Resolver, path: &str, ix: &FileIndex, offset: u32, out: &mut Completions) {
    // Locals and parameters of the function around the cursor, declared before it.
    if let Some(function) = ix.function_around(offset).cloned()
        && let Some(answer) = Answering::of(r.p, path)
    {
        let mut locals = answer.analysis.locals(path);
        locals.sort_by_key(|l| std::cmp::Reverse(l.at.lo));
        for local in locals {
            let Some(at) = answer.new_offset(local.at.lo) else { continue };
            if function.span.0 <= at && at < offset {
                let detail = format!("{}: {}", local.name, local.ty);
                out.add(&local.name, 6, &detail, None, 0);
            }
        }
    }
    for item in ix.items.iter().filter(|i| i.parent.is_none()) {
        out.item(item, 1);
    }
    // Imported names.
    if let Some(package) = r.p.package_of(path) {
        for u in &ix.uses {
            let segments: Vec<String> = u.path.iter().map(|(s, _)| s.clone()).collect();
            if !u.items.is_empty() {
                let file = r.p.module_file(package, &segments);
                for (name, _) in &u.items {
                    let found = file.as_ref().and_then(|f| {
                        r.index(f)
                            .and_then(|mix| mix.items.iter().find(|i| i.name == *name && i.parent.is_none()).cloned())
                    });
                    match found {
                        Some(item) => out.item(&item, 2),
                        None => out.add(name, 9, &format!("module {}.{name}", segments.join(".")), None, 2),
                    }
                }
            } else if let Some((alias, _)) = &u.alias {
                out.add(alias, 9, &format!("module {}", segments.join(".")), None, 2);
            } else if let Some(last) = segments.last() {
                let file = r.p.module_file(package, &segments);
                match file {
                    Some(_) => out.add(last, 9, &format!("module {}", segments.join(".")), None, 2),
                    None => {
                        let module = r.p.module_file(package, &segments[..segments.len() - 1]);
                        let found = module.as_ref().and_then(|f| {
                            r.index(f).and_then(|mix| {
                                mix.items.iter().find(|i| i.name == *last && i.parent.is_none()).cloned()
                            })
                        });
                        if let Some(item) = found {
                            out.item(&item, 2);
                        }
                    }
                }
            }
        }
    }
    // The standard library's root module is visible everywhere.
    if let Some(ix) = r.index("<std>/std.bd") {
        for item in ix.items.iter().filter(|i| i.parent.is_none() && !i.is_priv) {
            out.item(item, 3);
        }
    }
    for (name, detail) in BUILTIN_FUNCTIONS {
        out.add(name, 3, detail, None, 3);
    }
    for name in BUILTIN_TYPES {
        out.add(name, 22, "built-in type", None, 4);
    }
    for word in KEYWORDS {
        out.add(word, 14, "keyword", None, 5);
    }
}

// ---- signature help ---------------------------------------------------------------------------------------------

pub fn signature_help(server: &mut Server, params: &Json) -> Reply {
    let (i, path) = server.document(params, false)?;
    server.analyzed(i)?;
    let offset = server.offset(i, &path, params.get("position"))?;
    let mut r = Resolver::new(&mut server.packages[i]);
    let Some(ix) = r.index(&path) else { return Ok(Json::Null) };
    // The innermost unclosed `(` before the cursor, and how many commas at its depth come after it.
    let mut depth = 0i32;
    let mut commas = 0u32;
    let mut open = None;
    for (ti, t) in ix.tokens.iter().enumerate().rev() {
        if t.hi > offset {
            continue;
        }
        match t.kind {
            Tk::RParen | Tk::RBracket | Tk::RBrace => depth += 1,
            Tk::LBracket | Tk::LBrace if depth > 0 => depth -= 1,
            Tk::LBrace => break,
            Tk::LParen if depth > 0 => depth -= 1,
            Tk::LParen => {
                open = Some(ti);
                break;
            }
            Tk::Comma if depth == 0 => commas += 1,
            _ => {}
        }
    }
    let Some(open) = open else { return Ok(Json::Null) };
    let Some(callee) =
        open.checked_sub(1).and_then(|c| ix.tokens.get(c)).and_then(|t| t.ident().map(|n| (n.to_string(), t.lo)))
    else {
        return Ok(Json::Null);
    };
    let dotted = open >= 3 && ix.tokens[open - 2].kind == Tk::Dot;
    let receiver = if dotted { Some(ix.tokens[open - 3].clone()) } else { None };
    let Some(answer) = Answering::of(r.p, &path) else { return Ok(Json::Null) };
    let functions = answer.analysis.functions();
    // A method call on a value names the receiver's type; a call through a type or module names it directly.
    let receiver_type = receiver.as_ref().and_then(|t| match &t.kind {
        Tk::Ident(n) if n.starts_with(|c: char| c.is_ascii_uppercase()) => Some(n.clone()),
        _ => answer.type_at(&path, t.lo).map(|ty| base_type(&ty)),
    });
    let by_value = receiver.as_ref().is_some_and(|t| match &t.kind {
        Tk::Ident(n) => !n.starts_with(|c: char| c.is_ascii_uppercase()) && answer.type_at(&path, t.lo).is_some(),
        _ => true,
    });
    let candidates: Vec<&burdin_query::FunctionInfo> = functions.iter().filter(|f| f.name == callee.0).collect();
    let best = candidates
        .iter()
        .min_by_key(|f| {
            let owner_match =
                receiver_type.as_ref().is_some_and(|t| f.qualified.ends_with(&format!("{t}.{}", callee.0)));
            (!owner_match, f.location.path != path, f.location.path.starts_with("<std>"), f.qualified.clone())
        })
        .copied();
    let Some(f) = best else { return Ok(Json::Null) };
    let label = f.display.clone();
    let parameters = parameter_labels(&label);
    let skip_self = by_value && f.params.first().is_some_and(|p| p.name == "self");
    let active = commas + u32::from(skip_self);
    let signature = Json::obj()
        .with("label", label.as_str())
        .with(
            "parameters",
            Json::Arr(
                parameters
                    .iter()
                    .map(|(a, b)| Json::obj().with("label", Json::Arr(vec![Json::from(*a), Json::from(*b)])))
                    .collect(),
            ),
        )
        .with("activeParameter", active);
    Ok(Json::obj()
        .with("signatures", Json::Arr(vec![signature]))
        .with("activeSignature", 0)
        .with("activeParameter", active))
}

/// The `[start, end)` of each parameter in a signature label, in UTF-16 code units.
fn parameter_labels(label: &str) -> Vec<(u32, u32)> {
    let Some(open) = label.find('(') else { return Vec::new() };
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = open + 1;
    let units = |byte: usize| label[..byte].encode_utf16().count() as u32;
    for (i, c) in label.char_indices().skip_while(|(i, _)| *i <= open) {
        match c {
            '(' | '<' | '[' => depth += 1,
            ')' if depth == 0 => {
                if i > start {
                    out.push((units(start), units(i)));
                }
                break;
            }
            ')' | '>' | ']' => depth -= 1,
            ',' if depth == 0 => {
                out.push((units(start), units(i)));
                start = i + 2;
            }
            _ => {}
        }
    }
    out
}

// ---- inlay hints ------------------------------------------------------------------------------------------------

pub fn inlay_hints(server: &mut Server, params: &Json) -> Reply {
    let (i, path) = server.document(params, true)?;
    let text = server.packages[i].text(&path).unwrap_or("").to_string();
    let index = LineIndex::new(&text, server.encoding);
    let (lo, hi) = index.offsets(params.get("range")).unwrap_or((0, text.len() as u32));
    let (types, names) = (server.config.inlay_types, server.config.inlay_parameters);
    let p = &mut server.packages[i];
    let Some(ix) = p.index(&path) else { return Ok(Json::Arr(Vec::new())) };
    let Some(analysis) = p.current() else { return Ok(Json::Arr(Vec::new())) };
    let mut hints: Vec<(u32, Json)> = Vec::new();
    if types {
        let locals = analysis.locals(&path);
        for (ti, t) in ix.tokens.iter().enumerate() {
            if t.kind != Tk::Keyword("let") || t.lo < lo || t.lo > hi {
                continue;
            }
            let mut n = ti + 1;
            if ix.tokens.get(n).is_some_and(|x| x.kind == Tk::Keyword("mut")) {
                n += 1;
            }
            let Some(name) = ix.tokens.get(n).filter(|x| x.ident().is_some()) else { continue };
            if ix.tokens.get(n + 1).is_some_and(|x| x.kind == Tk::Colon) {
                continue;
            }
            if let Some(local) = locals.iter().find(|l| l.at.lo == name.lo) {
                if local.ty.contains('?') || local.ty.contains("{error}") || local.ty.is_empty() {
                    continue;
                }
                let (line, character) = index.position(name.hi);
                hints.push((
                    name.hi,
                    Json::obj()
                        .with("position", crate::text::position_json(line, character))
                        .with("label", format!(": {}", local.ty))
                        .with("kind", 1),
                ));
            }
        }
    }
    if server.config.inlay_ownership {
        // The ownership lens renders the events the memory checks recorded (`spec/check.md`, CK10); the server
        // computes nothing. A drop is shown at the closing brace it runs at, and a move at its use.
        for function in analysis.ownership_events(&path) {
            for event in &function.events {
                if event.at.lo < lo || event.at.lo > hi {
                    continue;
                }
                let Some(local) = &event.local else { continue };
                let label = match &event.kind {
                    burdin_query::OwnershipEventKind::Move { target } => match target {
                        Some(target) => format!("move {local} -> {target}"),
                        None => format!("move {local}"),
                    },
                    burdin_query::OwnershipEventKind::Drop { conditional } => {
                        format!("drop {local}{}", if *conditional { " (conditional)" } else { "" })
                    }
                    burdin_query::OwnershipEventKind::DropFlag => format!("drop flag {local}"),
                    _ => continue,
                };
                let (line, character) = index.position(event.at.lo);
                hints.push((
                    event.at.lo,
                    Json::obj()
                        .with("position", crate::text::position_json(line, character))
                        .with("label", label)
                        .with("kind", 1),
                ));
            }
        }
    }
    if names {
        for call in analysis.calls() {
            if call.at.path != path || call.at.hi < lo || call.at.lo > hi {
                continue;
            }
            for (param, arg) in &call.args {
                if param.name.is_empty() || param.name == "self" || arg.lo < lo || arg.lo > hi {
                    continue;
                }
                let written = text.get(arg.lo as usize..arg.hi as usize).unwrap_or("");
                if written == param.name || written.ends_with(&format!(".{}", param.name)) {
                    continue;
                }
                let Some(ti) = ix.tokens.iter().position(|t| t.lo == arg.lo) else { continue };
                // Only an argument written in an argument list gets a hint (not one the compiler made up, such as a
                // placeholder of an interpolated string), and a named argument already says its name. A `mut`
                // marker comes after the hint.
                let mut at = arg.lo;
                let mut before = ti.checked_sub(1).and_then(|p| ix.tokens.get(p));
                if before.is_some_and(|p| p.kind == Tk::Keyword("mut")) {
                    at = before.map_or(at, |p| p.lo);
                    before = ti.checked_sub(2).and_then(|p| ix.tokens.get(p));
                }
                if !before.is_some_and(|p| matches!(p.kind, Tk::LParen | Tk::Comma)) {
                    continue;
                }
                let (line, character) = index.position(at);
                hints.push((
                    at,
                    Json::obj()
                        .with("position", crate::text::position_json(line, character))
                        .with("label", format!("{}:", param.name))
                        .with("kind", 2)
                        .with("paddingRight", true),
                ));
            }
        }
    }
    hints.sort_by_key(|(at, _)| *at);
    Ok(Json::Arr(hints.into_iter().map(|(_, h)| h).collect()))
}

// ---- semantic tokens --------------------------------------------------------------------------------------------

fn token_type(name: &str) -> u32 {
    TOKEN_TYPES.iter().position(|t| *t == name).unwrap_or(0) as u32
}

const DECLARATION: u32 = 1;
const READONLY: u32 = 2;
const MUTABLE: u32 = 4;
const DEFAULT_LIBRARY: u32 = 8;

fn item_token(kind: Kind) -> Option<(&'static str, u32)> {
    Some(match kind {
        Kind::Function | Kind::ExternFn => ("function", 0),
        Kind::Method | Kind::InterfaceMethod => ("method", 0),
        Kind::Struct => ("struct", 0),
        Kind::Enum => ("enum", 0),
        Kind::Variant => ("enumMember", 0),
        Kind::Field => ("property", 0),
        Kind::Interface | Kind::Derive => ("interface", 0),
        Kind::Const => ("variable", READONLY),
        Kind::Alias => ("type", 0),
        Kind::Test | Kind::Impl => return None,
    })
}

pub fn semantic_tokens(server: &mut Server, params: &Json, ranged: bool) -> Reply {
    let (i, path) = server.document(params, true)?;
    let text = server.packages[i].text(&path).unwrap_or("").to_string();
    let index = LineIndex::new(&text, server.encoding);
    let (lo, hi) = if ranged { index.offsets(params.get("range")).unwrap_or((0, u32::MAX)) } else { (0, u32::MAX) };
    let classes = classify(&mut server.packages[i], &path);
    let mut data: Vec<Json> = Vec::new();
    let (mut last_line, mut last_char) = (0u32, 0u32);
    for (tlo, thi, ty, mods) in classes {
        if tlo < lo || thi > hi {
            continue;
        }
        let (line, character) = index.position(tlo);
        let (_, end) = index.position(thi);
        let delta_line = line - last_line;
        let delta_char = if delta_line == 0 { character - last_char } else { character };
        for v in [delta_line, delta_char, end - character, ty, mods] {
            data.push(Json::from(v));
        }
        last_line = line;
        last_char = character;
    }
    Ok(Json::obj().with("data", Json::Arr(data)))
}

/// Every identifier of the file the server can classify: `(lo, hi, type, modifiers)` in source order.
fn classify(p: &mut Package, path: &str) -> Vec<(u32, u32, u32, u32)> {
    let mut r = Resolver::new(p);
    let Some(ix) = r.index(path) else { return Vec::new() };
    let all = r.all_items();
    let mut types: HashMap<&str, Kind> = HashMap::new();
    for (_, item) in all.iter() {
        if item.kind.is_type() {
            types.entry(item.name.as_str()).or_insert(item.kind);
        }
    }
    let mut classes: HashMap<u32, (u32, u32)> = HashMap::new();
    // Declarations.
    for item in &ix.items {
        if let Some((ty, m)) = item_token(item.kind) {
            classes.insert(item.name_span.0, (token_type(ty), m | DECLARATION));
        }
    }
    // What the analysis resolved.
    if let Some(analysis) = r.p.current() {
        let locals = analysis.locals(path);
        let mutable: Vec<(u32, bool)> = locals.iter().map(|l| (l.at.lo, l.mutable)).collect();
        for local in &locals {
            let ty = if local.is_parameter { "parameter" } else { "variable" };
            let m = DECLARATION | if local.mutable { MUTABLE } else { 0 };
            classes.insert(local.at.lo, (token_type(ty), m));
        }
        for o in analysis.occurrences() {
            if o.at.path != path {
                continue;
            }
            let std = o.definition.as_ref().is_some_and(|d| d.path.starts_with("<std>/"));
            let lib = if std { DEFAULT_LIBRARY } else { 0 };
            match o.kind {
                DefinitionKind::Local | DefinitionKind::Parameter => {
                    let ty = if o.kind == DefinitionKind::Parameter { "parameter" } else { "variable" };
                    let m = o
                        .definition
                        .as_ref()
                        .and_then(|d| mutable.iter().find(|(lo, _)| *lo == d.lo))
                        .is_some_and(|(_, m)| *m);
                    if ix.tokens.iter().any(|t| t.lo == o.at.lo && t.ident() == Some(o.name.as_str())) {
                        classes.entry(o.at.lo).or_insert((token_type(ty), if m { MUTABLE } else { 0 }));
                    }
                }
                DefinitionKind::Function => {
                    let paren =
                        ix.tokens.iter().find(|t| t.lo >= o.at.lo && t.kind == Tk::LParen).map_or(o.at.hi, |t| t.lo);
                    if let Some(t) = ix
                        .tokens
                        .iter()
                        .rfind(|t| t.lo >= o.at.lo && t.hi <= paren && t.ident() == Some(o.name.as_str()))
                    {
                        let is_method = t.lo > o.at.lo && ix.tokens.iter().any(|d| d.hi == t.lo && d.kind == Tk::Dot);
                        let ty = if is_method { "method" } else { "function" };
                        classes.entry(t.lo).or_insert((token_type(ty), lib));
                    }
                }
                DefinitionKind::Constant => {
                    if let Some(t) = ix
                        .tokens
                        .iter()
                        .find(|t| t.lo >= o.at.lo && t.hi <= o.at.hi && t.ident() == Some(o.name.as_str()))
                    {
                        classes.entry(t.lo).or_insert((token_type("variable"), READONLY | lib));
                    }
                }
            }
        }
    }
    // Names the syntax tells: modules in `use`, members after `.`, and type names.
    let package = r.p.package_of(path);
    for u in &ix.uses {
        let mut segments: Vec<String> = Vec::new();
        for (name, span) in &u.path {
            segments.push(name.clone());
            if let Some(pk) = package
                && r.p.module_file(pk, &segments).is_some()
            {
                classes.entry(span.0).or_insert((token_type("namespace"), 0));
            }
        }
        if let Some((_, span)) = &u.alias {
            classes.entry(span.0).or_insert((token_type("namespace"), 0));
        }
    }
    let modules: Vec<String> = ix
        .uses
        .iter()
        .filter(|u| u.items.is_empty())
        .filter_map(|u| u.alias.as_ref().map(|a| a.0.clone()).or_else(|| u.path.last().map(|l| l.0.clone())))
        .collect();
    for (ti, t) in ix.tokens.iter().enumerate() {
        let Some(name) = t.ident() else { continue };
        if classes.contains_key(&t.lo) {
            continue;
        }
        let after_dot = ti > 0 && ix.tokens[ti - 1].kind == Tk::Dot;
        let class = if after_dot {
            let receiver = ti.checked_sub(2).and_then(|x| ix.tokens[x].ident()).unwrap_or("");
            if types.get(receiver) == Some(&Kind::Enum) && name.starts_with(|c: char| c.is_ascii_uppercase()) {
                Some(("enumMember", 0))
            } else if ix.tokens.get(ti + 1).is_some_and(|n| n.kind == Tk::LParen) {
                Some(("method", 0))
            } else {
                Some(("property", 0))
            }
        } else if let Some(kind) = types.get(name) {
            item_token(*kind)
        } else if modules.iter().any(|m| m == name) && ix.tokens.get(ti + 1).is_some_and(|n| n.kind == Tk::Dot) {
            Some(("namespace", 0))
        } else {
            None
        };
        if let Some((ty, m)) = class {
            classes.insert(t.lo, (token_type(ty), m));
        }
    }
    let mut out: Vec<(u32, u32, u32, u32)> = ix
        .tokens
        .iter()
        .filter(|t| t.ident().is_some())
        .filter_map(|t| classes.get(&t.lo).map(|(ty, m)| (t.lo, t.hi, *ty, *m)))
        .collect();
    out.sort_by_key(|c| c.0);
    out.dedup_by_key(|c| c.0);
    out
}

// ---- formatting -------------------------------------------------------------------------------------------------

pub fn formatting(server: &mut Server, params: &Json, ranged: bool) -> Reply {
    if !server.config.formatting {
        return Ok(Json::Null);
    }
    let (i, path) = server.document(params, false)?;
    let text = server.packages[i].text(&path).unwrap_or("").to_string();
    let index = LineIndex::new(&text, server.encoding);
    let dependencies = server.packages[i]
        .packages
        .iter()
        .find(|p| p.is_entry)
        .and_then(|p| p.dir.as_ref())
        .map(|d| burdin_query::package::dependency_names(d))
        .unwrap_or_default();
    let options = burdin_fmt::Options {
        groups: burdin_fmt::UseGroups { std_modules: burdin_query::package::std_module_names(), dependencies },
    };
    let result = if ranged {
        let (lo, hi) = index.offsets(params.get("range")).ok_or_else(|| ReqError::params("missing range"))?;
        burdin_fmt::format_range(&text, lo as usize, hi as usize, &options)
    } else {
        burdin_fmt::format_source(&text, &options)
    };
    let formatted = match result {
        Ok(f) => f,
        Err(burdin_fmt::FormatError::Syntax(_)) => {
            // Formatting on save must not interrupt the user; the syntax errors are already published.
            server.log(format!("{path}: not formatted, the file has syntax errors"));
            return Ok(Json::Null);
        }
        Err(e) => return Err(ReqError::failed(e.to_string())),
    };
    if formatted == text {
        return Ok(Json::Arr(Vec::new()));
    }
    let (prefix, suffix) = common(&text, &formatted);
    let old_hi = text.len() - suffix;
    let new_hi = formatted.len() - suffix;
    let edit = Json::obj()
        .with("range", index.range(prefix as u32, old_hi.max(prefix) as u32))
        .with("newText", &formatted[prefix..new_hi.max(prefix)]);
    Ok(Json::Arr(vec![edit]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_labels_split_at_top_level_commas() {
        let label = "fn f(a: Map<K, V>, b: i32): i32";
        let spans = parameter_labels(label);
        let texts: Vec<&str> = spans.iter().map(|(a, b)| &label[*a as usize..*b as usize]).collect();
        assert_eq!(texts, vec!["a: Map<K, V>", "b: i32"]);
        assert!(parameter_labels("fn g()").is_empty());
    }

    #[test]
    fn offsets_map_around_one_edit() {
        let old = "let a = 1\nlet b = a\n";
        let new = "let a = 1\nlet b = a.\n";
        assert_eq!(map_to_old(old, new, 4), Some(4));
        assert_eq!(map_to_old(old, new, new.len() as u32), Some(old.len() as u32));
    }
}
