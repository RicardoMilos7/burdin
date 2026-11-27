//! Help while editing (`spec/lsp.md`, LS11-LS15): hover, completion, signature help, inlay hints, semantic tokens,
//! and formatting through `burdin fmt`.

use crate::index::{FileIndex, Item, KEYWORDS, Kind, Tk, Tok};
use crate::json::Json;
use crate::nav::{Owners, Resolver, SymKind, base_type, enclosing_brace, leading_dot, literal_owner, sequence_element};
use crate::official::OfficialIndex;
use crate::project::{Loc, Package};
use crate::server::{Reply, ReqError, Server};
use crate::text::{Encoding, LineIndex, position_json};
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
            None => match r.p.current_typed().and_then(|a| a.type_at(&path, offset)) {
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

/// An analysis with a program and a way to translate positions of the current text into it. When the current text
/// has a syntax error, the analysis is the partial one of the current text, and `older` is the last good analysis:
/// the statement being typed is missing from the first and is still in the second.
pub(crate) struct Answering<'a> {
    pub(crate) analysis: &'a Analysis,
    old: Option<Rc<str>>,
    new: String,
    older: Option<(&'a Analysis, Rc<str>)>,
}

impl Answering<'_> {
    pub(crate) fn of<'a>(p: &'a Package, path: &str) -> Option<Answering<'a>> {
        let (analysis, texts) = p.program_analysis()?;
        let new = p.text(path)?.to_string();
        let old = texts.and_then(|t| t.get(path).cloned());
        let older = p.older_analysis().and_then(|(a, texts)| Some((a, texts.get(path)?.clone())));
        Some(Answering { analysis, old, new, older })
    }

    /// The analyses to ask, with the text each was computed on (`None`: the current text): the primary one, then the
    /// last good one when the primary is partial.
    fn sources(&self) -> impl Iterator<Item = (&Analysis, Option<&str>)> {
        std::iter::once((self.analysis, self.old.as_deref()))
            .chain(self.older.iter().map(|(a, text)| (*a, Some(&**text))))
    }

    /// The offset in the primary analysis's text of an offset of the current text.
    fn old_offset(&self, offset: u32) -> Option<u32> {
        to_old(self.old.as_deref(), &self.new, offset)
    }

    /// The offset in the current text of an offset of the primary analysis's text.
    fn new_offset(&self, offset: u32) -> Option<u32> {
        to_new(self.old.as_deref(), &self.new, offset)
    }

    fn type_at(&self, path: &str, offset: u32) -> Option<String> {
        self.sources().find_map(|(analysis, old)| {
            let at = to_old(old, &self.new, offset)?;
            analysis.type_at(path, at).map(|t| t.ty)
        })
    }

    /// The type of the expression that ends where the token `lo..hi` of the current text does and starts at or before
    /// it: the receiver of a member dot. The innermost expression at an offset of an older text may be an unrelated
    /// one (text typed over the start of a line maps onto what was there before), so one that ends elsewhere is not
    /// the receiver. The partial analysis of the current text lacks the statement being typed (`p.`), so the last good
    /// one is asked next.
    fn type_ending_at(&self, path: &str, lo: u32, hi: u32) -> Option<String> {
        self.sources().find_map(|(analysis, old)| {
            let at = to_old(old, &self.new, lo)?;
            let end = to_old(old, &self.new, hi.checked_sub(1)?)? + 1;
            let typed = analysis.type_at(path, at)?;
            (typed.at.hi == end).then_some(typed.ty)
        })
    }

    /// The type of the local `name` declared last before `offset` of the current text: what a name typed since the
    /// last good analysis most likely stands for.
    pub(crate) fn local_before(&self, path: &str, name: &str, offset: u32) -> Option<String> {
        self.sources().find_map(|(analysis, old)| {
            analysis
                .locals(path)
                .into_iter()
                .filter(|l| l.name == name)
                .filter_map(|l| Some((to_new(old, &self.new, l.at.lo)?, l.ty)))
                .filter(|(at, _)| *at < offset)
                .max_by_key(|(at, _)| *at)
                .map(|(_, ty)| ty)
        })
    }
}

/// The offset in the text an analysis was computed on (`old`; `None` is the current text) of an offset of `new`.
fn to_old(old: Option<&str>, new: &str, offset: u32) -> Option<u32> {
    match old {
        Some(old) => map_to_old(old, new, offset),
        None => Some(offset),
    }
}

fn to_new(old: Option<&str>, new: &str, offset: u32) -> Option<u32> {
    match old {
        Some(old) => map_to_new(old, new, offset),
        None => Some(offset),
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

/// The derives the compiler knows (`spec/derives.md`).
const BUILTIN_DERIVES: &[(&str, &str)] = &[
    ("Copy", "copied by assignment, passing, and returning"),
    ("Clone", "`clone()`, field by field"),
    ("Eq", "`==` and `!=`, field by field"),
    ("Ord", "`<`, `<=`, `>`, `>=`, and `cmp`, in declaration order"),
    ("Hash", "hashing, field by field"),
    ("Default", "`Type.default()`, every field's default"),
    ("Debug", "the `{x:?}` format"),
    ("Serialize", "the standard serialization representation"),
    ("Deserialize", "the standard deserialization representation"),
];

/// The snippets at item position: label, body, and what it declares (`spec/lsp.md`, LS12).
const ITEM_SNIPPETS: &[(&str, &str, &str)] = &[
    ("main", "fn main() {\n    $0\n}", "the program's entry point"),
    ("fn", "fn ${1:name}(${2}): ${3:Type} {\n    $0\n}", "a function"),
    ("struct", "struct ${1:Name} {\n    $0\n}", "a struct"),
    ("enum", "enum ${1:Name} {\n    $0\n}", "an enum"),
    ("error enum", "error enum ${1:Name} {\n    ${2:Variant} \"${3:message}\"\n}", "an error enum"),
    ("interface", "interface ${1:Name} {\n    $0\n}", "an interface"),
    ("impl", "${1:Type} {\n    $0\n}", "an impl block"),
    ("impl interface", "${1:Type}: ${2:Interface} {\n    $0\n}", "an interface implementation"),
    ("test", "test \"${1:name}\" {\n    $0\n}", "a test"),
    ("bench", "bench \"${1:name}\" {\n    $0\n}", "a benchmark"),
    ("fuzz", "fuzz ${1:name}(${2:data}: [u8]) {\n    $0\n}", "a fuzz target"),
];

/// The snippet inside an impl block or an interface.
const METHOD_SNIPPETS: &[(&str, &str, &str)] = &[("fn", "fn ${1:name}(${2:self}): ${3:Type} {\n    $0\n}", "a method")];

/// The snippets at statement position, inside a body.
const STATEMENT_SNIPPETS: &[(&str, &str, &str)] = &[
    ("if", "if ${1:condition} {\n    $0\n}", "an `if` statement"),
    ("match", "match ${1:value} {\n    ${2:pattern} => ${3:result}\n}", "a `match`"),
    ("for", "for ${1:item} in ${2:items} {\n    $0\n}", "a `for` loop"),
    ("while", "while ${1:condition} {\n    $0\n}", "a `while` loop"),
];

struct Completions {
    items: Vec<Json>,
    seen: Vec<String>,
    /// The client inserts snippets.
    snippets: bool,
    /// A function's completion inserts the call's parentheses (LS12): snippets work, the position is not a `use`
    /// path, and no `(` follows already.
    calls: bool,
    /// The client asks for documentation with `completionItem/resolve` (LS23): the items carry none, and `docs` keeps
    /// it by label until they are resolved.
    lazy_docs: bool,
    docs: HashMap<String, String>,
}

impl Completions {
    /// Documentation goes on the item, or waits for its resolve.
    fn document(&mut self, item: &mut Json, label: &str, docs: &str) {
        if self.lazy_docs {
            self.docs.insert(label.to_string(), docs.to_string());
        } else {
            item.set("documentation", Json::obj().with("kind", "markdown").with("value", docs));
        }
    }

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
            self.document(&mut item, label, docs);
        }
        if self.calls
            && matches!(kind, 2 | 3)
            && let Some(params) = has_parameters(detail)
        {
            item.set("insertTextFormat", 2);
            if params {
                item.set("insertText", format!("{label}($0)"));
                item.set(
                    "command",
                    Json::obj().with("title", "Signature help").with("command", "editor.action.triggerParameterHints"),
                );
            } else {
                item.set("insertText", format!("{label}()$0"));
            }
        }
        self.items.push(item);
    }

    fn item(&mut self, item: &Item, group: u8) {
        if item.kind == Kind::Impl || item.kind == Kind::Test || item.name.starts_with("__") {
            return;
        }
        self.add(&item.name, completion_kind(item.kind), &item.detail, item.docs.as_deref(), group);
    }

    /// An official package's name, offered as a module that `http.Client` starts with (`htt` offers `http`), with the
    /// edit that loads the package when the file does not (`spec/lsp.md`, LS12).
    fn package(&mut self, name: &str, edit: Option<Json>, group: u8) {
        if self.seen.iter().any(|seen| seen == name) {
            return;
        }
        self.seen.push(name.to_string());
        let mut json = Json::obj()
            .with("label", name)
            .with("kind", 9)
            .with("detail", "official package")
            .with("sortText", format!("{group}{name}"));
        if let Some(edit) = edit {
            json.set("additionalTextEdits", Json::Arr(vec![edit]));
        }
        self.items.push(json);
    }

    /// An item of an official package offered by its qualified name (`http.Client`), filtered by its own name, with
    /// the edit that imports the package when the file does not (`spec/lsp.md`, LS12). `edit` is `None` when the
    /// package's graph already holds it, so the qualified name compiles as written.
    fn qualified(&mut self, package: &str, item: &Item, edit: Option<Json>, group: u8) {
        let label = format!("{package}.{}", item.name);
        // A bare name already offered (an import, a local, or the file's own item) needs no qualified duplicate.
        if self.seen.iter().any(|seen| seen == &item.name || seen == &label) {
            return;
        }
        self.seen.push(label.clone());
        let kind = completion_kind(item.kind);
        let mut json = Json::obj()
            .with("label", label.as_str())
            .with("filterText", item.name.as_str())
            .with("kind", kind)
            .with("detail", item.detail.as_str())
            .with("sortText", format!("{group}{label}"));
        if let Some(docs) = &item.docs {
            self.document(&mut json, &label, docs);
        }
        if self.calls
            && matches!(kind, 2 | 3)
            && let Some(parameters) = has_parameters(&item.detail)
        {
            json.set("insertTextFormat", 2);
            if parameters {
                json.set("insertText", format!("{label}($0)"));
                json.set(
                    "command",
                    Json::obj().with("title", "Signature help").with("command", "editor.action.triggerParameterHints"),
                );
            } else {
                json.set("insertText", format!("{label}()$0"));
            }
        }
        if let Some(edit) = edit {
            json.set("additionalTextEdits", Json::Arr(vec![edit]));
        }
        self.items.push(json);
    }

    /// Snippet items; without the client's support, nothing (the keywords are added as plain words).
    fn snippets(&mut self, list: &[(&str, &str, &str)]) {
        if !self.snippets {
            return;
        }
        for (label, body, what) in list {
            if self.seen.iter().any(|s| s == label) {
                continue;
            }
            self.seen.push(label.to_string());
            self.items.push(
                Json::obj()
                    .with("label", *label)
                    .with("kind", 15)
                    .with("detail", *what)
                    .with("insertText", *body)
                    .with("insertTextFormat", 2)
                    .with("sortText", format!("5{label}")),
            );
        }
    }
}

/// Whether a function's declaration (`fn add(a: i32, b: i32): i32`) takes parameters besides `self`; `None` when it
/// does not read as a function.
fn has_parameters(detail: &str) -> Option<bool> {
    let params = parameter_list(detail)?;
    Some(params.iter().any(|p| !is_self_param(p)))
}

/// Whether a function's declaration takes `self` first (a method, not an associated function).
fn takes_self(detail: &str) -> bool {
    parameter_list(detail).is_some_and(|p| p.first().is_some_and(|p| is_self_param(p)))
}

fn is_self_param(p: &str) -> bool {
    matches!(p.trim(), "self" | "mut self" | "own self")
}

/// The parameters of a function's declaration, as written.
fn parameter_list(detail: &str) -> Option<Vec<String>> {
    let at = detail.find("fn ")? + 3;
    let rest = &detail[at..];
    // The `(` of the parameter list: after the name and its generic parameters.
    let mut depth = 0i32;
    let mut open = None;
    for (i, c) in rest.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            '(' if depth == 0 => {
                open = Some(i);
                break;
            }
            _ => {}
        }
    }
    let open = open?;
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for c in rest[open + 1..].chars() {
        match c {
            '(' | '<' | '[' => depth += 1,
            ')' if depth == 0 => {
                if !current.trim().is_empty() {
                    out.push(current.trim().to_string());
                }
                return Some(out);
            }
            ')' | '>' | ']' => depth -= 1,
            ',' if depth == 0 => {
                out.push(current.trim().to_string());
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    None
}

/// Where completion was asked for.
enum Context {
    /// In a `use` declaration.
    Use,
    /// After `x.`; the offset of the dot.
    Member(u32),
    /// After a leading dot (`.Red`, `.new(3)`): the offset of the dot. The members are those of the type the position
    /// expects (LS12).
    Leading(u32),
    /// At a field's name inside a struct literal's braces (`Point { x: 1, `, `.{ x: 1, `): the struct the literal builds
    /// and the `{`'s token index. The members are the struct's fields the literal has not written, then the names in
    /// scope for shorthand (LS12).
    Field { owner: String, brace: usize },
    /// At the start of a function's parameter: the first one, and whether the function is a method.
    Parameter { first: bool, method: bool },
    /// In the `: A, B` list after `struct Name` or `enum Name`, at the start of a name.
    Derives,
    /// Where a type is written: a `let` annotation, a parameter's or a return type, a field's type (LS12).
    Type,
    /// Where an interface is named: a generic bound, a `where` clause, an interface's `requires` list, an impl block's
    /// `Type: ` at the top level, or the interface after `any` or `impl` (LS12).
    Bound,
    /// The name a declaration is introducing: nothing to offer.
    NewName,
    /// At the start of a line at the top level.
    Item,
    /// At the start of a line inside an impl block, an interface, or a derive.
    MethodItem,
    /// At the start of a line inside a body.
    Statement,
    /// Anywhere else: an expression or a type.
    Expression,
}

/// What kind of block a `{` opens, from the tokens before it.
#[derive(PartialEq)]
enum Block {
    /// An impl block, an interface, or a derive: methods go inside.
    Methods,
    /// A struct, an enum, or another declaration that is not code.
    Declaration,
    /// A function's or test's body, or a block inside one.
    Code,
}

/// The unmatched `{`s before token `end`, outermost first, with what each opens.
fn open_blocks(text: &str, ix: &FileIndex, end: usize) -> Vec<(usize, Block)> {
    let mut stack: Vec<usize> = Vec::new();
    for (i, t) in ix.tokens[..end].iter().enumerate() {
        match t.kind {
            Tk::LBrace => stack.push(i),
            Tk::RBrace => {
                stack.pop();
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (depth, &brace) in stack.iter().enumerate() {
        let kind = if depth > 0 {
            match out.last() {
                Some((_, Block::Methods)) => Block::Code,
                Some((_, Block::Code)) => Block::Code,
                _ => Block::Declaration,
            }
        } else {
            top_level_block(text, ix, brace)
        };
        out.push((brace, kind));
    }
    out
}

/// What a top-level `{` at token `brace` opens: the first token of its header says. The header is the last line
/// before the brace that starts at the first column with a word (a top-level item does), after the previous `}`.
fn top_level_block(text: &str, ix: &FileIndex, brace: usize) -> Block {
    let after = ix.tokens[..brace].iter().rposition(|t| t.kind == Tk::RBrace).map_or(0, |i| i + 1);
    let column_0 = |t: &Tok| t.lo == 0 || text.as_bytes().get(t.lo as usize - 1) == Some(&b'\n');
    let start = (after..brace)
        .rev()
        .find(|&i| {
            let t = &ix.tokens[i];
            column_0(t) && matches!(t.kind, Tk::Keyword(_) | Tk::Ident(_) | Tk::SelfType)
        })
        .unwrap_or(after);
    let mut head = ix.tokens[start..brace].iter().map(|t| &t.kind);
    let mut first = head.next();
    while matches!(first, Some(Tk::Keyword("priv" | "unsafe"))) || first.is_some_and(|k| *k == Tk::Ident("pub".into()))
    {
        first = head.next();
    }
    match first {
        Some(Tk::Keyword("interface" | "derive")) => Block::Methods,
        Some(Tk::Keyword("fn" | "extern" | "const")) => Block::Code,
        Some(Tk::Ident(word)) if word == "test" || word == "bench" => Block::Code,
        Some(Tk::Ident(_) | Tk::SelfType) => Block::Methods,
        _ => Block::Declaration,
    }
}

/// The index of the unmatched `(` before token `end` that is still open at it, with the number of commas at its
/// depth after it.
fn open_paren(ix: &FileIndex, end: usize) -> Option<(usize, u32)> {
    let mut depth = 0i32;
    let mut commas = 0u32;
    for ti in (0..end).rev() {
        match ix.tokens[ti].kind {
            Tk::RParen | Tk::RBracket => depth += 1,
            Tk::LBracket if depth > 0 => depth -= 1,
            Tk::LParen if depth > 0 => depth -= 1,
            Tk::LParen => return Some((ti, commas)),
            Tk::LBrace | Tk::RBrace | Tk::LBracket => return None,
            Tk::Comma if depth == 0 => commas += 1,
            _ => {}
        }
    }
    None
}

/// Whether the `(` at token `open` starts a function declaration's parameters, and whether that function is an
/// extension method (`fn Type.name(`).
fn declares_function(ix: &FileIndex, open: usize) -> Option<bool> {
    let mut i = open.checked_sub(1)?;
    if ix.tokens[i].kind == Tk::Gt {
        // Skip the generic parameters.
        let mut depth = 0;
        loop {
            match ix.tokens[i].kind {
                Tk::Gt => depth += 1,
                Tk::Lt => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            i = i.checked_sub(1)?;
        }
        i = i.checked_sub(1)?;
    }
    ix.tokens[i].ident()?;
    let mut extension = false;
    let mut j = i.checked_sub(1)?;
    // `fn Type.name` and `fn List<T>.name`: walk back over the receiver's type path.
    while ix.tokens[j].kind != Tk::Keyword("fn") {
        if !matches!(ix.tokens[j].kind, Tk::Dot | Tk::Ident(_) | Tk::Lt | Tk::Gt | Tk::Comma | Tk::SelfType) {
            return None;
        }
        extension = true;
        j = j.checked_sub(1)?;
    }
    Some(extension)
}

/// Whether the tokens before `end` are a `struct Name<T>: A, B` or `enum Name: A` head up to its list, so the
/// next word is a name in the list (`spec/derives.md`). A `:` inside the generic parameters is a bound, not the list.
fn in_derive_list(ix: &FileIndex, end: usize) -> bool {
    let mut depth = 0i32;
    let mut colon = false;
    for i in (0..end).rev() {
        match &ix.tokens[i].kind {
            Tk::Gt => depth += 1,
            Tk::Lt => depth -= 1,
            Tk::Colon if depth == 0 => colon = true,
            Tk::Keyword("struct" | "enum") => return colon && depth == 0,
            Tk::Ident(_) | Tk::Comma | Tk::Dot | Tk::Colon => {}
            _ => return false,
        }
    }
    false
}

/// Whether the tokens before `end` are inside a bound list: a generic parameter list (`fn f<T: `), a `where` clause, or
/// an interface's `requires` list. The scan stops at the braces and parentheses that close the header.
fn in_bound_list(ix: &FileIndex, end: usize) -> bool {
    let mut depth = 0i32;
    for i in (0..end).rev() {
        match &ix.tokens[i].kind {
            Tk::LBrace | Tk::RBrace | Tk::LParen | Tk::RParen => return false,
            Tk::Gt => depth += 1,
            Tk::Lt if depth == 0 => return i > 0 && ix.tokens[i - 1].ident().is_some(),
            Tk::Lt => depth -= 1,
            Tk::Keyword("where") => return true,
            Tk::Ident(word) if word.as_str() == "requires" => return true,
            _ => {}
        }
    }
    false
}

/// Whether the tokens before `end` are `let name: `, `let mut name: `, or `const NAME: `: a type annotation.
fn let_annotation(ix: &FileIndex, end: usize) -> bool {
    let kind = |back: usize| end.checked_sub(back).map(|i| &ix.tokens[i].kind);
    if !kind(2).is_some_and(|k| matches!(k, Tk::Ident(_))) {
        return false;
    }
    kind(3) == Some(&Tk::Keyword("let"))
        || kind(3) == Some(&Tk::Keyword("const"))
        || (kind(3) == Some(&Tk::Keyword("mut")) && kind(4) == Some(&Tk::Keyword("let")))
}

/// Whether the tokens before `end` are the `): ` of a function declaration: the colon of its return type.
fn return_type(ix: &FileIndex, end: usize) -> bool {
    let Some(close) = end.checked_sub(2) else { return false };
    if ix.tokens[close].kind != Tk::RParen {
        return false;
    }
    let mut depth = 0i32;
    for i in (0..=close).rev() {
        match ix.tokens[i].kind {
            Tk::RParen => depth += 1,
            Tk::LParen => {
                depth -= 1;
                if depth == 0 {
                    return declares_function(ix, i).is_some();
                }
            }
            _ => {}
        }
    }
    false
}

/// Whether the word before `end` is the first word of its line: `Type: ` written at the start of a line.
fn at_line_start_word(text: &str, ix: &FileIndex, end: usize) -> bool {
    let Some(word) = end.checked_sub(2).map(|i| &ix.tokens[i]) else { return false };
    word.ident().is_some() && text[..word.lo as usize].rsplit('\n').next().is_some_and(|s| s.trim().is_empty())
}

/// The position of a type or an interface, by the word before the cursor (LS12): an interface after `any`, `impl`,
/// `requires`, a `+` in a bound list, a colon in a bound list or at the top level (`Type: ` starts an impl block); a
/// type after a colon that annotates a `let`, a `const`, a return, a field, or (by the caller) a parameter.
fn type_or_bound(text: &str, ix: &FileIndex, end: usize, prev: Option<&Tk>, before: Option<&Tk>) -> Option<Context> {
    let plus = end > 0
        && ix.tokens[end - 1].kind == Tk::Other
        && text.get(ix.tokens[end - 1].lo as usize..ix.tokens[end - 1].hi as usize) == Some("+");
    let names_interface = matches!(prev, Some(Tk::Keyword("any" | "impl")))
        || matches!(prev, Some(Tk::Ident(word)) if word.as_str() == "requires");
    if names_interface || (plus && in_bound_list(ix, end)) {
        return Some(Context::Bound);
    }
    if !matches!(prev, Some(Tk::Colon)) {
        return None;
    }
    if in_bound_list(ix, end) {
        return Some(Context::Bound);
    }
    let blocks = open_blocks(text, ix, end);
    // `Type: ` at the top level, outside any parentheses, starts an impl block's interface.
    if blocks.is_empty() && open_paren(ix, end).is_none() && at_line_start_word(text, ix, end) {
        return Some(Context::Bound);
    }
    let in_declaration = blocks.last().is_some_and(|(_, b)| *b == Block::Declaration);
    if let_annotation(ix, end) || return_type(ix, end) || (in_declaration && matches!(before, Some(Tk::Ident(_)))) {
        return Some(Context::Type);
    }
    None
}

fn context(text: &str, ix: &FileIndex, start: usize) -> Context {
    let line_start = text[..start].rfind('\n').map_or(0, |n| n + 1);
    let line = text[line_start..start].trim_start();
    if line.starts_with("use ") || line.starts_with("pub use ") {
        return Context::Use;
    }
    if start > 0 && text.as_bytes()[start - 1] == b'.' {
        let dot = (start - 1) as u32;
        let leading =
            ix.tokens.iter().position(|t| t.lo == dot && t.kind == Tk::Dot).is_some_and(|i| leading_dot(ix, i));
        return if leading { Context::Leading(dot) } else { Context::Member(dot) };
    }
    // The tokens before the word being completed.
    let end = ix.tokens.partition_point(|t| (t.hi as usize) <= start && (t.lo as usize) < start);
    let prev = end.checked_sub(1).map(|i| &ix.tokens[i].kind);
    let before = end.checked_sub(2).map(|i| &ix.tokens[i].kind);
    if let Some((open, commas)) = open_paren(ix, end) {
        let at_start = matches!(prev, Some(Tk::LParen | Tk::Comma));
        if let Some(extension) = declares_function(ix, open) {
            if !at_start {
                // `fn f(x: ` is a parameter's type, and `fn f(x: any ` or `fn f(x: T + ` an interface.
                let fallback = if matches!(prev, Some(Tk::Colon)) { Context::Type } else { Context::Expression };
                return type_or_bound(text, ix, end, prev, before).unwrap_or(fallback);
            }
            let blocks = open_blocks(text, ix, end);
            let method = extension || blocks.last().is_some_and(|(_, b)| *b == Block::Methods);
            return Context::Parameter { first: commas == 0, method };
        }
    }
    if matches!(prev, Some(Tk::Colon | Tk::Comma)) && in_derive_list(ix, end) {
        return Context::Derives;
    }
    match (before, prev) {
        // After `struct Name` comes `:`, `repr(C)`, or the body: nothing to complete.
        (Some(Tk::Keyword("struct" | "enum")), Some(Tk::Ident(_))) => return Context::NewName,
        (_, Some(Tk::Gt)) => {
            // `struct Name<T> `: the generic parameters' `<` follows the name, which follows the keyword.
            let lt = ix.tokens[..end].iter().rposition(|t| t.kind == Tk::Lt);
            if let Some(lt) = lt
                && lt >= 2
                && ix.tokens[lt - 1].ident().is_some()
                && matches!(ix.tokens[lt - 2].kind, Tk::Keyword("struct" | "enum"))
                && !ix.tokens[lt..end].iter().any(|t| matches!(t.kind, Tk::LBrace | Tk::RBrace | Tk::LParen))
            {
                return Context::NewName;
            }
        }
        (_, Some(Tk::Keyword("struct" | "enum" | "interface" | "fn"))) => return Context::NewName,
        _ => {}
    }
    if let Some(context) = type_or_bound(text, ix, end, prev, before) {
        return context;
    }
    if !line.is_empty() {
        return Context::Expression;
    }
    match open_blocks(text, ix, end).last() {
        None => Context::Item,
        Some((_, Block::Methods)) => Context::MethodItem,
        Some((_, Block::Code)) => Context::Statement,
        Some((_, Block::Declaration)) => Context::Expression,
    }
}

/// Where an auto-imported official package's `use` is inserted (`spec/lsp.md`, LS12): after the file's last `use`, or
/// at its top, with a blank line after, when it has none.
struct ImportPlace {
    offset: u32,
    blank_after: bool,
}

/// The `use <package>` an accepted official-package completion inserts, and whether the client applies it.
struct Imports {
    encoding: Encoding,
    supported: bool,
    place: ImportPlace,
    text: String,
}

impl Imports {
    fn edit(&self, package: &str) -> Json {
        let index = LineIndex::new(&self.text, self.encoding);
        let (line, character) = index.position(self.place.offset);
        let at = position_json(line, character);
        let new_text = use_text(&self.place, package);
        Json::obj().with("range", Json::obj().with("start", at.clone()).with("end", at)).with("newText", new_text)
    }
}

/// Gives the items added since index `from` the edit that loads their package, when there is one.
fn attach_edit(out: &mut Completions, from: usize, edit: Option<&Json>) {
    let Some(edit) = edit else { return };
    for item in &mut out.items[from..] {
        item.set("additionalTextEdits", Json::Arr(vec![edit.clone()]));
    }
}

/// The `use <path>` line, placed where `place` says.
fn use_text(place: &ImportPlace, path: &str) -> String {
    if place.blank_after { format!("use {path}\n\n") } else { format!("use {path}\n") }
}

/// The byte offset and the text of a `use <path>` line in `text`, for a code action that imports an official package
/// (`http`) or one of its items (`http.Client`): the place a completion's edit uses too (`spec/lsp.md`, LS12, LS18).
pub(crate) fn use_at(text: &str, ix: &FileIndex, path: &str) -> (u32, String) {
    let place = import_place(text, ix);
    (place.offset, use_text(&place, path))
}

/// After the last `use` declaration's line, or at the start of a file with none. A `use` spans more than one line
/// only for a `.{...}` list, whose `}` ends it; a trailing comment stays on its own line.
fn import_place(text: &str, ix: &FileIndex) -> ImportPlace {
    match ix.uses.iter().map(|u| u.span.1).max() {
        Some(mut offset) => {
            while (offset as usize) < text.len() && text.as_bytes()[offset as usize] != b'\n' {
                offset += 1;
            }
            if (offset as usize) < text.len() {
                offset += 1;
            }
            ImportPlace { offset, blank_after: false }
        }
        None => ImportPlace { offset: 0, blank_after: true },
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
    let mut end = offset as usize;
    while end < text.len() && (text.as_bytes()[end].is_ascii_alphanumeric() || text.as_bytes()[end] == b'_') {
        end += 1;
    }
    let paren_follows = text.as_bytes().get(end) == Some(&b'(');
    let snippets = server.caps.snippets;
    let official = server.official();
    let encoding = server.encoding;
    let additional_edits = server.caps.additional_edits;
    let ix = Resolver::new(&mut server.packages[i]).index(&path).unwrap_or_default();
    let prefix = &text[start..offset as usize];
    let imports =
        Imports { encoding, supported: additional_edits, place: import_place(&text, &ix), text: text.clone() };
    let site = Site { path: &path, ix: &ix, offset, prefix, imports: &imports };
    let plain = context(&text, &ix, start);
    // A field name in a struct literal's braces (LS12): the literal builds the struct before its `{`, or for a leading
    // `.{` the type its position expects, which the analysis gives at the dot.
    let literal = match plain {
        Context::Statement | Context::Expression => literal_brace(&text, &ix, start),
        _ => None,
    };
    let literal_type = match literal {
        Some((brace, None)) => literal_owner(&ix, brace + 1),
        Some((_, Some(dot))) => expected_type_at(server, i, &path, &text, &text, dot)?.map(|ty| base_type(&ty)),
        None => None,
    };
    // A leading dot's members are those of the type its position expects, which the analysis gives at the dot.
    let expected = match plain {
        Context::Leading(dot) => {
            let repaired = repaired_for_completion(&text, &ix, dot);
            expected_type_at(server, i, &path, &text, &repaired, dot)?
        }
        _ => None,
    };
    // The receiver of a member dot is typed from the text without the dot (see `repaired_receiver_type`).
    let receiver = match plain {
        Context::Member(dot) => member_receiver_type(server, i, &path, &text, &ix, dot)?,
        _ => None,
    };
    let mut r = Resolver::new(&mut server.packages[i]);
    let all = r.all_items();
    // `Point {` and `.{` build a struct; `match Shape.Circle {` or `if FLAG {` do not, and keep their own context.
    let context = match (literal, literal_type) {
        (Some((brace, _)), Some(owner))
            if all.iter().any(|(_, item)| item.kind == Kind::Struct && item.name == owner) =>
        {
            Context::Field { owner, brace }
        }
        _ => plain,
    };
    let calls = snippets && !paren_follows && !matches!(context, Context::Use);
    let lazy_docs = server.caps.completion_resolve;
    let mut out = Completions { items: Vec::new(), seen: Vec::new(), snippets, calls, lazy_docs, docs: HashMap::new() };
    match context {
        Context::Use => {
            let line_start = text[..start].rfind('\n').map_or(0, |n| n + 1);
            use_completions(&mut r, &path, text[line_start..start].trim_start(), &mut out);
        }
        Context::Member(dot) => {
            member_completions(&mut r, &official, &imports, &path, &ix, &text, dot, receiver, &mut out);
        }
        Context::Leading(_) => {
            leading_completions(&mut r, expected.as_deref(), &mut out);
        }
        Context::Field { owner, brace } => {
            literal_fields(&all, &owner, &ix, brace, start, &mut out);
            scope_completions(&mut r, &official, &site, Scope::Values, &mut out);
        }
        Context::Parameter { first, method } => {
            if first && method {
                for (label, what) in [
                    ("self", "read the receiver"),
                    ("mut self", "change the receiver"),
                    ("own self", "consume the receiver"),
                ] {
                    out.add(label, 14, what, None, 0);
                }
            }
            for word in ["mut", "own"] {
                out.add(word, 14, "keyword", None, 1);
            }
        }
        Context::Derives => derive_completions(&mut r, &path, &ix, &mut out),
        Context::NewName => {}
        Context::Type => {
            scope_completions(&mut r, &official, &site, Scope::Types, &mut out);
        }
        Context::Bound => {
            scope_completions(&mut r, &official, &site, Scope::Interfaces, &mut out);
        }
        Context::Item => {
            out.snippets(ITEM_SNIPPETS);
            scope_completions(&mut r, &official, &site, Scope::Values, &mut out);
        }
        Context::MethodItem => {
            out.snippets(METHOD_SNIPPETS);
            scope_completions(&mut r, &official, &site, Scope::Values, &mut out);
        }
        Context::Statement => {
            out.snippets(STATEMENT_SNIPPETS);
            scope_completions(&mut r, &official, &site, Scope::Values, &mut out);
        }
        Context::Expression => {
            scope_completions(&mut r, &official, &site, Scope::Values, &mut out);
        }
    }
    let mut items = out.items;
    server.completion_docs = out.docs;
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

/// The type of a literal receiver, from its spelling: `"text"` is a `str` (an interpolated one a `String`), `'c'` a
/// `char`, `1.5` an `f64`, `7` an `i32`, and `true` a `bool`.
fn literal_type(text: &str, tok: &Tok) -> Option<&'static str> {
    let spelled = text.get(tok.lo as usize..tok.hi as usize).unwrap_or("");
    match &tok.kind {
        Tk::Str if spelled.contains('{') => Some("String"),
        Tk::Str => Some("str"),
        Tk::Num if spelled.starts_with('\'') => Some("char"),
        Tk::Num if !spelled.starts_with("0x") && spelled.contains(['.', 'e', 'E']) => Some("f64"),
        Tk::Num => Some("i32"),
        Tk::Keyword("true" | "false") => Some("bool"),
        _ => None,
    }
}

/// After `x.`: a module's items, a type's variants and associated functions, or a value's fields and methods. An
/// official package is a module even before anything imports it: the index (`official.rs`) answers for it.
#[allow(clippy::too_many_arguments)]
fn member_completions(
    r: &mut Resolver,
    official: &OfficialIndex,
    imports: &Imports,
    path: &str,
    ix: &FileIndex,
    text: &str,
    dot: u32,
    receiver_ty: Option<String>,
    out: &mut Completions,
) {
    let Some(ti) = ix.tokens.iter().position(|t| t.hi == dot) else { return };
    let receiver = ix.tokens[ti].clone();
    // A name after a `.` is a member of the value before it, so it is never a local, a module, or a type.
    let member = dot_before(ix, ti);

    if let Tk::Ident(name) = &receiver.kind
        && !member
    {
        // The answering analysis may be older than the text: its offsets are mapped.
        // Text typed since that analysis has no position in it, so a local declared before it still says the name is a
        // value (`list.` is not the `list` module when a local `list` is in scope).
        let is_value = Answering::of(r.p, path).is_some_and(|a| {
            a.old_offset(receiver.lo)
                .and_then(|at| a.analysis.resolve_at(path, at))
                .is_some_and(|x| matches!(x.kind, DefinitionKind::Local | DefinitionKind::Parameter))
                || a.local_before(path, name, receiver.lo).is_some()
        });
        if !is_value {
            let file = module_file_named(r, path, ix, name);
            // An official package's root is not in the graph until something imports it; the index holds the API
            // either way, `pub use` re-exports included. A local module or a path or git dependency of the same name
            // resolves to another file and wins.
            if let Some(api) = official.get(name)
                && file.as_deref().is_none_or(|file| official.is_root(name, file))
            {
                // The package is not in the graph until this file loads it: accepting one of its names adds the `use`
                // (`spec/lsp.md`, LS12), as a completion of a bare name does.
                let edit = (file.is_none() && imports.supported).then(|| imports.edit(name));
                let from = out.items.len();
                for module in &api.modules {
                    out.add(module, 9, &format!("module {name}.{module}"), None, 0);
                }
                for item in &api.items {
                    if item.kind != Kind::Method {
                        out.item(item, 0);
                    }
                }
                attach_edit(out, from, edit.as_ref());
                return;
            }
            if let Some(file) = file
                && let Some(mix) = r.index(&file)
            {
                let foreign = r.p.package_of(&file) != r.p.package_of(path);
                for item in mix.items.iter().filter(|i| i.parent.is_none() && !(foreign && i.is_priv)) {
                    if item.kind != Kind::Method {
                        out.item(item, 0);
                    }
                }
                return;
            }
            let all = r.all_items();
            if all.iter().any(|(_, i)| i.kind.is_type() && i.name == *name) {
                type_members(&all, name, out);
                return;
            }
        }
    }
    // `self.` in a method: the members of the type the method belongs to, whether or not the body parses (issue #852).
    if receiver.kind == Tk::SelfValue {
        if let Some(owner) = ix.self_owner(receiver.lo) {
            let all = r.all_items();
            value_members(&all, &owner, out);
        }
        return;
    }
    let all = r.all_items();
    let ty = receiver_ty.or_else(|| receiver_type(Answering::of(r.p, path).as_ref(), ix, &all, path, text, ti));
    // A literal's spelling says its type when the analysis has not seen it yet.
    let ty = match (ty, literal_type(text, &receiver)) {
        (Some(ty), _) => ty,
        (None, Some(literal)) => literal.to_string(),
        (None, None) => return,
    };
    match ix.bounds_of(receiver.lo, &base_type(&ty)) {
        // A generic parameter (`s: S` with `S: Serializer`): the methods of its bounds.
        Some(bounds) => bound_members(&all, &bounds, out),
        None => value_members(&all, &ty, out),
    }
}

/// The type of the receiver of the member dot at `dot`, cheapest answer first: the last analysis and the declarations
/// ([`receiver_type`]), then [`repaired_receiver_type`], which analyzes the text again (a file that does not parse is
/// checked without the memo, so that is the costly one). A receiver that no `let` or parameter in sight declares is a
/// module, a type, or a name nothing types, and is not worth it.
fn member_receiver_type(
    server: &mut Server,
    i: usize,
    path: &str,
    text: &str,
    ix: &FileIndex,
    dot: u32,
) -> Result<Option<String>, ReqError> {
    let Some(ti) = ix.tokens.iter().position(|t| t.hi == dot) else { return Ok(None) };
    let mut r = Resolver::new(&mut server.packages[i]);
    let all = r.all_items();
    if let Some(ty) = receiver_type(Answering::of(r.p, path).as_ref(), ix, &all, path, text, ti) {
        return Ok(Some(ty));
    }
    let receiver = &ix.tokens[ti];
    let known = match &receiver.kind {
        Tk::Ident(name) => declaration_before(ix, receiver.lo, name).is_some() || dot_before(ix, ti),
        Tk::RParen | Tk::RBracket => true,
        _ => false,
    };
    if known { repaired_receiver_type(server, i, path, text, ix, dot) } else { Ok(None) }
}

/// The type of the receiver of the member dot at `dot` (`x.`, `x.na`), from an analysis of the text without the dot and
/// the member name typed after it: the line being typed does not parse with them, and a file that does not parse has no
/// typed tree, but the receiver alone does, so the cursor's own expression is typed whatever the rest of the file holds
/// (`spec/lsp.md`, LS12). `None` when the receiver is not an expression the analysis types (a module, a type name, a
/// file that still does not parse): the stale analysis and the declarations answer then. The document's text is put
/// back before the answer, as in [`expected_type_at`].
fn repaired_receiver_type(
    server: &mut Server,
    i: usize,
    path: &str,
    text: &str,
    ix: &FileIndex,
    dot: u32,
) -> Result<Option<String>, ReqError> {
    let Some(receiver) = ix.tokens.iter().find(|t| t.hi == dot) else { return Ok(None) };
    let after = dot as usize + 1;
    if text.as_bytes().get(dot as usize) != Some(&b'.') {
        return Ok(None);
    }
    let word = text.as_bytes()[after..].iter().take_while(|b| b.is_ascii_alphanumeric() || **b == b'_').count();
    let repaired = format!("{}{}", &text[..dot as usize], &text[after + word..]);
    expected_type_at(server, i, path, text, &repaired, receiver.lo)
}

/// The type a leading dot at `dot` stands for: the type its position expects (`spec/target-typing.md`). `repaired` is
/// the text analyzed: the document's own, or [`repaired_for_completion`]'s with a placeholder. The document's text is
/// put back before the answer is returned, so the server's state is the document's again.
pub(crate) fn expected_type_at(
    server: &mut Server,
    i: usize,
    path: &str,
    text: &str,
    repaired: &str,
    dot: u32,
) -> Result<Option<String>, ReqError> {
    server.packages[i].set_text(path, repaired);
    let analyzed = server.analyzed(i);
    let ty = server.packages[i].current().and_then(|a| a.type_at(path, dot)).map(|t| t.ty);
    server.packages[i].set_text(path, text);
    analyzed?;
    server.analyzed(i)?;
    Ok(ty.filter(|ty| usable(ty)))
}

/// The text with a placeholder name after the dot at `dot`, and the closing brackets its line still needs before the
/// dot's end: `name(.` becomes `name(.__complete)`, and `Point { x: .` becomes `Point { x: .__complete}`. A bracket the
/// line closes after the dot is counted, so an editor that closed it already is not closed twice (LS12).
fn repaired_for_completion(text: &str, ix: &FileIndex, dot: u32) -> String {
    let line_start = text[..dot as usize].rfind('\n').map_or(0, |n| n + 1);
    let line_end = text[dot as usize..].find('\n').map_or(text.len(), |n| dot as usize + n) as u32;
    let mut open: Vec<char> = Vec::new();
    for t in ix.tokens.iter().filter(|t| t.lo as usize >= line_start && t.hi <= dot) {
        match t.kind {
            Tk::LParen => open.push(')'),
            Tk::LBracket => open.push(']'),
            Tk::LBrace => open.push('}'),
            Tk::RParen | Tk::RBracket | Tk::RBrace => {
                open.pop();
            }
            _ => {}
        }
    }
    for t in ix.tokens.iter().filter(|t| t.lo > dot && t.hi <= line_end) {
        if matches!(t.kind, Tk::RParen | Tk::RBracket | Tk::RBrace) {
            open.pop();
        }
    }
    let at = dot as usize + 1;
    let closers: String = open.iter().rev().collect();
    format!("{}__complete{}{}", &text[..at], closers, &text[at..])
}

/// After a leading dot: the members of the type its position expects, as `T.` offers them: its variants and its
/// associated functions (`.Red`, `.new(3)`, TT6), and the variants of `Option` (`Some`, `None`) or of `Result` (`Ok`,
/// `Err`, TT14).
fn leading_completions(r: &mut Resolver, expected: Option<&str>, out: &mut Completions) {
    let Some(ty) = expected else { return };
    let base = base_type(ty);
    match base.as_str() {
        "Option" => {
            out.add("Some", 20, ".Some(value): Option", None, 0);
            out.add("None", 20, ".None: Option", None, 0);
        }
        "Result" => {
            out.add("Ok", 20, ".Ok(value): Result", None, 0);
            out.add("Err", 20, ".Err(error): Result", None, 0);
        }
        _ => {}
    }
    let all = r.all_items();
    type_members(&all, &base, out);
}

/// The `{` of the struct literal a field name at `start` is written in (LS12): the name starts an entry, after a `{` or
/// a `,`, inside a code body. Its token index, and the offset of the dot when the `{` is a leading `.{`.
fn literal_brace(text: &str, ix: &FileIndex, start: usize) -> Option<(usize, Option<u32>)> {
    let end = ix.tokens.partition_point(|t| (t.hi as usize) <= start && (t.lo as usize) < start);
    if !matches!(end.checked_sub(1).map(|i| &ix.tokens[i].kind), Some(Tk::LBrace | Tk::Comma)) {
        return None;
    }
    // A struct's own declaration (`struct Point { x: i32 }`) is not a literal: only a code body holds one.
    if !matches!(open_blocks(text, ix, end).last(), Some((_, Block::Code))) {
        return None;
    }
    let brace = enclosing_brace(ix, end)?;
    let dot = brace.checked_sub(1).filter(|&d| ix.tokens[d].kind == Tk::Dot && leading_dot(ix, d));
    Some((brace, dot.map(|d| ix.tokens[d].lo)))
}

/// The fields of struct `owner` that the literal whose `{` is token `brace` has not written yet, then the names in
/// scope for shorthand (`Point { x, y }`). A name starting an entry of the literal is written, except the one being
/// completed, at byte `start`.
fn literal_fields(
    all: &[(String, Item)],
    owner: &str,
    ix: &FileIndex,
    brace: usize,
    start: usize,
    out: &mut Completions,
) {
    let mut written: Vec<String> = Vec::new();
    let mut depth = 0i32;
    for (i, t) in ix.tokens.iter().enumerate().skip(brace + 1) {
        match t.kind {
            Tk::LParen | Tk::LBracket | Tk::LBrace => depth += 1,
            Tk::RParen | Tk::RBracket => depth -= 1,
            Tk::RBrace if depth == 0 => break,
            Tk::RBrace => depth -= 1,
            _ => {
                let entry = matches!(ix.tokens[i - 1].kind, Tk::LBrace | Tk::Comma);
                if let Some(name) = t.ident()
                    && depth == 0
                    && entry
                    && t.lo as usize != start
                {
                    written.push(name.to_string());
                }
            }
        }
    }
    for (_, item) in all.iter().filter(|(_, i)| i.kind == Kind::Field && i.owner.as_deref() == Some(owner)) {
        if !written.contains(&item.name) {
            out.item(item, 0);
        }
    }
}

/// Whether a type the analysis reports can be used: an error type says nothing about the value.
fn usable(ty: &str) -> bool {
    !ty.is_empty() && !ty.contains("{error}")
}

/// The type of the value the expression ending at token `ti` has, for a member completion (LS12). The typed tree
/// answers first, when it holds the expression and types it. Otherwise the chain is read from the declarations: the
/// root's type (a local, a parameter, or `self`), then each field's declared type and each method's return type. So
/// `resp.body.` and `client.send(req).` resolve while the member after the dot is still missing (issue #879).
fn receiver_type(
    a: Option<&Answering<'_>>,
    ix: &FileIndex,
    all: &[(String, Item)],
    path: &str,
    text: &str,
    ti: usize,
) -> Option<String> {
    let tok = &ix.tokens[ti];
    if let Some(ty) = a.and_then(|a| a.type_ending_at(path, tok.lo, tok.hi)).filter(|t| usable(t)) {
        return Some(ty);
    }
    match &tok.kind {
        Tk::SelfValue => ix.self_owner(tok.lo),
        // `resp.body`: the field's declared type, read from the value before the dot.
        Tk::Ident(name) if dot_before(ix, ti) => {
            let owner = base_type(&receiver_type(a, ix, all, path, text, ti.checked_sub(2)?)?);
            field_type(all, &owner, name)
        }
        Tk::Ident(name) => a
            .and_then(|a| a.local_before(path, name, tok.lo))
            .filter(|t| usable(t))
            .or_else(|| ix.parameter_type(tok.lo, name))
            .or_else(|| declared_type(ix, all, text, tok.lo, name)),
        // `[1, 2]`: an array literal, typed by its first element (`[_]` when that is not a literal). The typed tree
        // answers first; this holds when the line being typed has no tree yet.
        Tk::RBracket => {
            let open = open_before(ix, ti, Tk::LBracket, Tk::RBracket)?;
            let indexes = open.checked_sub(1).is_some_and(|b| {
                matches!(ix.tokens[b].kind, Tk::Ident(_) | Tk::RParen | Tk::RBracket | Tk::SelfValue)
                    && ix.tokens[b].hi == ix.tokens[open].lo
            });
            if indexes {
                return None;
            }
            Some(array_literal_type(ix, text, open))
        }
        // `client.send(req)` or `read(req)`: the return type of the method or function called.
        Tk::RParen => {
            let open = open_before(ix, ti, Tk::LParen, Tk::RParen)?;
            let callee = ix.tokens.get(open.checked_sub(1)?)?.ident()?;
            if open >= 3 && dot_before(ix, open - 1) {
                let owner = receiver_type(a, ix, all, path, text, open - 3)?;
                method_return(all, &owner, callee)
            } else {
                function_return(all, callee)
            }
        }
        _ => None,
    }
}

/// The type of the array literal whose `[` is token `open`, by its first element: `[i32; _]` for `[1, 2]`, and `[_; _]`
/// when the first element is not a literal, which only the extensions that fit any element (`[T]`) match.
fn array_literal_type(ix: &FileIndex, text: &str, open: usize) -> String {
    let element = ix.tokens.get(open + 1).and_then(|t| literal_type(text, t)).unwrap_or("_");
    format!("[{element}; _]")
}

/// The type of the local or parameter `name` read from the text before `at`, for a file that has no analysis to say
/// (it never parsed since it was opened): the annotation after the name (`let p: Point`, `xs: [i32]`), or what its
/// initializer says: a struct literal (`Point { ... }`), a call of an associated function (`Point.origin()`) or a
/// function (`make()`) by the type it returns, an array literal by its first element, and a literal alone on its line.
fn declared_type(ix: &FileIndex, all: &[(String, Item)], text: &str, at: u32, name: &str) -> Option<String> {
    let after = declaration_before(ix, at, name)? + 1;
    match &ix.tokens.get(after)?.kind {
        Tk::Colon => {
            let (mut depth, mut end) = (0i32, after + 1);
            while let Some(t) = ix.tokens.get(end) {
                match t.kind {
                    Tk::LParen | Tk::LBracket | Tk::Lt => depth += 1,
                    Tk::RBracket | Tk::Gt => depth -= 1,
                    Tk::RParen if depth == 0 => break,
                    Tk::RParen => depth -= 1,
                    Tk::Comma | Tk::LBrace | Tk::Other if depth == 0 => break,
                    _ => {}
                }
                end += 1;
            }
            let (lo, hi) = (ix.tokens.get(after + 1)?.lo, ix.tokens.get(end.checked_sub(1)?)?.hi);
            text.get(lo as usize..hi as usize).map(|ty| ty.trim().to_string()).filter(|ty| usable(ty))
        }
        Tk::Other if text.get(ix.tokens[after].lo as usize..ix.tokens[after].hi as usize) == Some("=") => {
            initializer_type(ix, all, text, after + 1)
        }
        _ => None,
    }
}

/// The token of the last `let`, parameter, or `mut` binding of `name` before `at` in the function around it.
fn declaration_before(ix: &FileIndex, at: u32, name: &str) -> Option<usize> {
    let function = ix.function_around(at)?;
    (1..ix.tokens.len()).rev().find(|&j| {
        let t = &ix.tokens[j];
        t.ident() == Some(name)
            && t.lo >= function.span.0
            && t.hi <= at
            && matches!(ix.tokens[j - 1].kind, Tk::Keyword("let" | "mut" | "own") | Tk::LParen | Tk::Comma)
    })
}

/// The type of the initializer that starts at token `first` (see [`declared_type`]).
fn initializer_type(ix: &FileIndex, all: &[(String, Item)], text: &str, first: usize) -> Option<String> {
    let head = ix.tokens.get(first)?;
    let next = ix.tokens.get(first + 1);
    match &head.kind {
        Tk::Ident(name) if name.starts_with(|c: char| c.is_ascii_uppercase()) => match next.map(|t| &t.kind) {
            Some(Tk::LBrace) => Some(name.clone()),
            // `Point.origin()` or `List<i32>.new()`: the type the associated function returns.
            Some(Tk::Dot) => method_return(all, name, ix.tokens.get(first + 2)?.ident()?),
            Some(Tk::Lt) => {
                let close = (first + 1..ix.tokens.len()).find(|&k| ix.tokens[k].kind == Tk::Gt)?;
                let ty = text.get(head.lo as usize..ix.tokens[close].hi as usize)?.to_string();
                (ix.tokens.get(close + 1)?.kind == Tk::Dot).then_some(ty)
            }
            _ => None,
        },
        Tk::Ident(name) if next.is_some_and(|t| t.kind == Tk::LParen) => function_return(all, name),
        Tk::LBracket => Some(array_literal_type(ix, text, first)),
        _ => {
            let literal = literal_type(text, head)?;
            // `1 + x` is not an `i32`: the literal must be the whole initializer.
            let alone = next.is_none_or(|t| text[head.hi as usize..t.lo as usize].contains('\n'));
            alone.then(|| literal.to_string())
        }
    }
}

/// Whether token `i` is a name a `.` touches on its left (`resp.body`, not a name on the next line after a `.`).
fn dot_before(ix: &FileIndex, i: usize) -> bool {
    i > 0 && ix.tokens[i - 1].kind == Tk::Dot && ix.tokens[i - 1].hi == ix.tokens[i].lo
}

/// The `open` bracket matching the `close` one at token `close_at`.
fn open_before(ix: &FileIndex, close_at: usize, open: Tk, close: Tk) -> Option<usize> {
    let mut depth = 0i32;
    for i in (0..=close_at).rev() {
        if ix.tokens[i].kind == close {
            depth += 1;
        } else if ix.tokens[i].kind == open {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// The declared type of a field of `owner` (`body: Body` gives `Body`).
fn field_type(all: &[(String, Item)], owner: &str, name: &str) -> Option<String> {
    let (_, field) =
        all.iter().find(|(_, i)| i.kind == Kind::Field && i.owner.as_deref() == Some(owner) && i.name == name)?;
    field.detail.split_once(':').map(|(_, ty)| ty.trim().to_string()).filter(|ty| usable(ty))
}

/// The return type of the method `name` of a value of type `ty`, read from its declaration.
fn method_return(all: &[(String, Item)], ty: &str, name: &str) -> Option<String> {
    let owners = Owners::of(ty);
    let (_, method) = all.iter().find(|(_, i)| {
        matches!(i.kind, Kind::Method | Kind::InterfaceMethod) && owners.owns(i.owner.as_deref()) && i.name == name
    })?;
    return_type_of(&method.detail, &base_type(ty))
}

/// The return type of the function `name`, preferring the file's own over the standard library's.
fn function_return(all: &[(String, Item)], name: &str) -> Option<String> {
    let (_, function) = all
        .iter()
        .filter(|(_, i)| i.kind == Kind::Function && i.owner.is_none() && i.name == name)
        .min_by_key(|(path, _)| path.starts_with("<std>/"))?;
    return_type_of(&function.detail, "")
}

/// The return type a declaration names (`fn send(self, req: Request): Response` gives `Response`), with `Self` read
/// as `owner`; `None` when it names none.
fn return_type_of(detail: &str, owner: &str) -> Option<String> {
    let rest = &detail[detail.find("fn ")? + 3..];
    let (mut angles, mut parens, mut open) = (0i32, 0i32, false);
    let mut after = None;
    for (i, c) in rest.char_indices() {
        match c {
            '<' if !open => angles += 1,
            '>' if !open => angles -= 1,
            '(' if !open && angles == 0 => {
                open = true;
                parens = 1;
            }
            '(' if open => parens += 1,
            ')' if open => {
                parens -= 1;
                if parens == 0 {
                    after = Some(&rest[i + 1..]);
                    break;
                }
            }
            _ => {}
        }
    }
    let ty = after?.trim().strip_prefix(':')?.trim().trim_end_matches('{').trim();
    let ty = ty.split(" where ").next().unwrap_or(ty).trim();
    match ty {
        "" => None,
        "Self" => Some(owner.to_string()),
        _ => Some(ty.to_string()),
    }
}

/// The associated functions of a type written by name (`String.new`, `Rect.new`) and its variants: what follows
/// `Type.`. A built-in type's associated functions that the standard library declares as `__type_name` are listed
/// by their name (`f64.parse`, `i32.from_le_bytes`).
fn type_members(all: &[(String, Item)], name: &str, out: &mut Completions) {
    for (_, item) in all.iter().filter(|(_, i)| i.owner.as_deref() == Some(name)) {
        match item.kind {
            Kind::Variant => out.item(item, 0),
            Kind::Method | Kind::InterfaceMethod if !takes_self(&item.detail) => out.item(item, 1),
            _ => {}
        }
    }
    // The standard library's package-private `<type>_<name>` functions and constants are the members the compiler
    // maps `T.name` onto (spec/stdlib.md, "Integer functions" and "Floats").
    let hidden = [format!("{name}_"), format!("{}_", name.to_lowercase())];
    for (_, item) in all.iter().filter(|(p, i)| p.starts_with("<std>/") && i.parent.is_none() && i.is_priv) {
        let Some(short) = hidden.iter().find_map(|h| item.name.strip_prefix(h.as_str())) else { continue };
        if !matches!(item.kind, Kind::Function | Kind::Const) || short.is_empty() {
            continue;
        }
        let detail = item.detail.replacen(&item.name, short, 1);
        out.add(short, completion_kind(item.kind), &detail, item.docs.as_deref(), 1);
    }
}

/// The iterator types the compiler provides (`spec/iterators.md`, IT5), which `Iterator`'s methods apply to, and the
/// ones among them that also take items from the back.
const COMPILER_ITERATORS: &[&str] = &[
    "RangeIter",
    "MapIter",
    "MapIterMut",
    "IterMut",
    "ListIntoIter",
    "ArrayIntoIter",
    "MapIntoIter",
    "Chunks",
    "ChunksExact",
    "Windows",
];
const DOUBLE_ENDED: &[&str] = &["RangeIter", "IterMut", "ListIntoIter", "ArrayIntoIter"];

/// The fields and methods of a value of type `ty`: its own, its extension methods, the methods of the
/// interfaces it implements, and the ones an implementation or a derive provides (`to_string` through `Display`,
/// `clone` through `Clone`, `cmp` through `Ord`). An array or a slice has the methods of `[T]` (the compiler's and
/// the extensions of `std/src/slice.bd` that fit its element), and a slice is an `Iterator`, so it has the adapters;
/// an array needs `iter()` first, and a `List` needs `as_slice()` (`spec/stdlib.md`, "Slices").
fn value_members(all: &[(String, Item)], ty: &str, out: &mut Completions) {
    let owners = Owners::of(ty);
    let owner = base_type(ty);
    let owner = owner.as_str();
    for (_, item) in all.iter().filter(|(_, i)| owners.owns(i.owner.as_deref())) {
        match item.kind {
            Kind::Field => out.item(item, 0),
            Kind::Method | Kind::InterfaceMethod if takes_self(&item.detail) => out.item(item, 1),
            _ => {}
        }
    }
    let mut interfaces: Vec<String> = all
        .iter()
        .filter(|(_, i)| i.kind == Kind::Impl && i.owner.as_deref() == Some(owner))
        .filter_map(|(_, i)| i.interface.clone())
        .collect();
    match owners {
        Owners::Sequence { array: false, .. } => {
            interfaces.extend(["Iterator", "DoubleEndedIterator"].map(str::to_string));
        }
        Owners::Sequence { array: true, .. } | Owners::Named(_) => {}
    }
    // The types the compiler knows without declaring them: tuples and arrays clone and compare, a range does too and
    // gives its iterator, and its own iterators are `Iterator`s (`spec/iterators.md`, IT5; `spec/stdlib.md`).
    let shown = ty.trim().trim_start_matches('&').trim();
    let range = matches!(owner, "Range" | "RangeInclusive");
    let tuple = shown.starts_with('(') && shown != "()";
    if tuple || range || matches!(owners, Owners::Sequence { array: true, .. }) {
        out.add("clone", 2, &format!("fn clone(self): {shown}"), Some("A copy, field by field."), 1);
        out.add("cmp", 2, &format!("fn cmp(self, other: {shown}): Ordering"), Some("The order, through `Ord`."), 1);
    }
    if range {
        out.add("iter", 2, "fn iter(self): RangeIter", Some("Iterates over the range."), 1);
        out.add("into_iter", 2, "fn into_iter(own self): RangeIter", Some("Iterates over the range."), 1);
    }
    if COMPILER_ITERATORS.contains(&owner) {
        interfaces.push("Iterator".to_string());
    }
    if DOUBLE_ENDED.contains(&owner) {
        interfaces.push("DoubleEndedIterator".to_string());
    }
    // `struct Point: Clone, Ord` in the type's declaration.
    if let Some((_, decl)) = all.iter().find(|(_, i)| i.kind.is_type() && i.name == owner) {
        interfaces.extend(declared_interfaces(&decl.detail));
    }
    for iface in &interfaces {
        for (_, item) in
            all.iter().filter(|(_, i)| i.kind == Kind::InterfaceMethod && i.owner.as_deref() == Some(iface))
        {
            if takes_self(&item.detail) {
                out.item(item, 1);
            }
        }
        let provided = match iface.as_str() {
            "Display" => Some(("to_string", "fn to_string(self): String".to_string(), "The text `Display` writes.")),
            "Clone" => Some(("clone", format!("fn clone(self): {owner}"), "A copy, through `Clone`.")),
            "Ord" => Some(("cmp", format!("fn cmp(self, other: {owner}): Ordering"), "The order, through `Ord`.")),
            _ => None,
        };
        if let Some((name, detail, docs)) = provided {
            out.add(name, 2, &detail, Some(docs), 1);
        }
    }
}

/// The methods with a `self` receiver that the interfaces a generic parameter is bounded by declare (`S: Serializer`
/// gives `serialize_unit`, `serialize_i32`, ...), so `s.` in a body offers what `s` can do.
fn bound_members(all: &[(String, Item)], bounds: &[String], out: &mut Completions) {
    for (_, item) in all
        .iter()
        .filter(|(_, i)| i.kind == Kind::InterfaceMethod && i.owner.as_ref().is_some_and(|o| bounds.contains(o)))
    {
        if takes_self(&item.detail) {
            out.item(item, 1);
        }
    }
}

/// The names in the `: A, B` list of a struct's or enum's declaration head (`struct Pair<T: Ord>: Clone, Eq`): what
/// follows the first `:` outside the generic parameters, up to `repr` or the body.
fn declared_interfaces(head: &str) -> Vec<String> {
    let mut depth = 0i32;
    let mut list = None;
    for (i, c) in head.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ':' if depth == 0 => {
                list = Some(&head[i + 1..]);
                break;
            }
            '{' => break,
            _ => {}
        }
    }
    let Some(list) = list else { return Vec::new() };
    let list = list.split(['{', '\n']).next().unwrap_or("");
    let list = list.split(" repr").next().unwrap_or("");
    list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// In the `: A, B` list after a type's name: the compiler's derives and the `derive` declarations the file can name.
fn derive_completions(r: &mut Resolver, path: &str, ix: &FileIndex, out: &mut Completions) {
    for (name, what) in BUILTIN_DERIVES {
        out.add(name, 8, what, None, 0);
    }
    let imported: Vec<&str> = ix
        .uses
        .iter()
        .flat_map(|u| {
            let mut names: Vec<&str> = u.items.iter().map(|(n, _)| n.as_str()).collect();
            if u.items.is_empty() && u.alias.is_none() {
                names.extend(u.path.last().map(|(n, _)| n.as_str()));
            }
            names
        })
        .collect();
    let all = r.all_items();
    for (file, item) in all.iter().filter(|(_, i)| i.kind == Kind::Derive) {
        let visible =
            file == path || imported.contains(&item.name.as_str()) || (file.starts_with("<std>/") && !item.is_priv);
        if visible {
            out.item(item, 1);
        }
    }
}

/// The module a name stands for in a file: an alias, an imported module, a module of the package, a dependency, or
/// a standard-library module the prelude binds (`env`, `fs`).
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
    // A dependency is a path root by its name whether or not this file wrote a `use` for it: the package's graph
    // already has it, and `json.from_str` compiles in a file with no `use json` (`spec/lsp.md`, LS12).
    if let Some(f) = r.p.module_file(package, &[name.to_string()]) {
        return Some(f);
    }
    crate::nav::std_module(r.p, package, name)
}

/// What a plain name may stand for at a position (LS12): a value (the usual case), a type where a type is written, or
/// an interface where a bound is written.
#[derive(Clone, Copy, PartialEq)]
enum Scope {
    Values,
    Types,
    Interfaces,
}

impl Scope {
    /// Whether a declaration of kind `kind` may be named here. Modules stay admitted: they are path prefixes.
    fn admits(self, kind: Kind) -> bool {
        match self {
            Scope::Values => true,
            Scope::Types => matches!(kind, Kind::Struct | Kind::Enum | Kind::Alias),
            Scope::Interfaces => kind == Kind::Interface,
        }
    }
}

/// One completion request's place: the file, the cursor, the word being typed, and the `use` an official-package
/// item inserts.
#[derive(Clone, Copy)]
struct Site<'a> {
    path: &'a str,
    ix: &'a FileIndex,
    offset: u32,
    prefix: &'a str,
    imports: &'a Imports,
}

/// A plain name: the locals in scope, the file's items, what its `use` declarations import, the prelude, the
/// built-ins, the keywords, and the official packages' items by their qualified name while a name is being typed,
/// narrowed to what `scope` admits.
fn scope_completions(r: &mut Resolver, official: &OfficialIndex, site: &Site<'_>, scope: Scope, out: &mut Completions) {
    let Site { path, ix, offset, prefix, imports } = *site;
    if scope == Scope::Types {
        // A generic parameter of the function around the cursor is a type.
        for (name, _) in ix.generic_params(offset) {
            out.add(&name, 25, "generic parameter", None, 0);
        }
    }
    // Locals and parameters of the function around the cursor, declared before it.
    if let Some(function) = ix.function_around(offset).cloned()
        && let Some(answer) = Answering::of(r.p, path)
        && scope == Scope::Values
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
    for item in ix.items.iter().filter(|i| i.parent.is_none() && i.kind != Kind::Method && scope.admits(i.kind)) {
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
                        Some(item) if scope.admits(item.kind) => out.item(&item, 2),
                        Some(_) => {}
                        None if scope == Scope::Values => {
                            out.add(name, 9, &format!("module {}.{name}", segments.join(".")), None, 2)
                        }
                        None => {}
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
                        if let Some(item) = found.filter(|item| scope.admits(item.kind)) {
                            out.item(&item, 2);
                        }
                    }
                }
            }
        }
    }
    // The built-in functions (`println`), then the prelude: every public item of every standard-library module, and
    // the one-segment modules by name (`env`, `fs`).
    let builtins = r.index(crate::builtins::PATH).unwrap_or_default();
    for item in builtins.items.iter().filter(|i| i.kind == Kind::Function && scope == Scope::Values) {
        out.item(item, 3);
    }
    let std_files: Vec<(String, String)> =
        r.p.modules
            .iter()
            .filter(|m| m.path.starts_with("<std>/"))
            .map(|m| (m.path.clone(), m.module.join(".")))
            .collect();
    for (file, module) in &std_files {
        if let Some(ix) = r.index(file) {
            for item in ix
                .items
                .iter()
                .filter(|i| i.parent.is_none() && !i.is_priv && i.kind != Kind::Method && scope.admits(i.kind))
            {
                out.item(item, 3);
            }
        }
        if !module.is_empty() && !module.contains('.') && module != "std" {
            out.add(module, 9, &format!("module std.{module}"), None, 4);
        }
    }
    if scope != Scope::Interfaces {
        for name in BUILTIN_TYPES {
            out.add(name, 22, "built-in type", None, 4);
        }
    }
    if scope == Scope::Values {
        for word in KEYWORDS {
            out.add(word, 14, "keyword", None, 5);
        }
    }
    if let Some(current) = r.p.package_of(path) {
        official_completions(r.p, official, imports, current, prefix, scope, out);
    }
}

/// The official packages' names (`http`) and their items by their qualified name (`http.Client`) while a name is being
/// typed: the whole standard-library surface without loading a package the file does not use (`spec/lsp.md`, LS12).
/// A local module or another dependency of the same name shadows the official package, and a package that is not in
/// the graph is offered only when the client applies the `use` that loads it.
fn official_completions(
    p: &Package,
    official: &OfficialIndex,
    imports: &Imports,
    current: usize,
    prefix: &str,
    scope: Scope,
    out: &mut Completions,
) {
    if prefix.is_empty() {
        return;
    }
    for (package, api) in official.packages() {
        if shadows(p, official, current, package) {
            continue;
        }
        let loaded = p.module_file(current, &[package.to_string()]).is_some();
        if !loaded && !imports.supported {
            continue;
        }
        let edit = (!loaded && imports.supported).then(|| imports.edit(package));
        if package.starts_with(prefix) {
            out.package(package, edit.clone(), 6);
        }
        for item in &api.items {
            if !scope.admits(item.kind) || !item.name.starts_with(prefix) {
                continue;
            }
            if out.seen.iter().any(|seen| seen == &item.name) {
                continue;
            }
            out.qualified(package, item, edit.clone(), 6);
        }
    }
}

/// Whether the package itself, a local module, or a path or git dependency of the same name shadows the official
/// package `name` (`spec/packages.md`, PKG15).
fn shadows(p: &Package, official: &OfficialIndex, current: usize, name: &str) -> bool {
    if p.packages.get(current).is_some_and(|package| package.name == name) {
        return true;
    }
    if p.modules.iter().any(|m| m.package == current && m.module.len() == 1 && m.module[0] == name) {
        return true;
    }
    match p.module_file(current, &[name.to_string()]) {
        Some(file) => !official.is_root(name, &file),
        None => false,
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
    let all = r.all_items();
    let Some(answer) = Answering::of(r.p, &path) else { return Ok(Json::Null) };
    let functions = answer.analysis.functions();
    // A method call on a value names the receiver's type; a call through a type or module names it directly.
    let is_type_name = |n: &str| n.starts_with(|c: char| c.is_ascii_uppercase()) || BUILTIN_TYPES.contains(&n);
    let receiver_type = receiver.as_ref().and_then(|t| match &t.kind {
        Tk::Ident(n) if is_type_name(n) => Some(n.clone()),
        // An array or a slice is owned by `[T]` (the declarations' name for it, `nav::Owners`).
        _ => answer.type_at(&path, t.lo).map(|ty| match sequence_element(&ty) {
            Some(_) => "[T]".to_string(),
            None => base_type(&ty),
        }),
    });
    let by_value = receiver.as_ref().is_some_and(|t| match &t.kind {
        Tk::Ident(n) => !is_type_name(n) && answer.type_at(&path, t.lo).is_some(),
        _ => true,
    });
    let owner_match = |f: &burdin_query::FunctionInfo| {
        receiver_type.as_ref().is_some_and(|t| f.qualified.ends_with(&format!("{t}.{}", callee.0)))
    };
    let candidates: Vec<&burdin_query::FunctionInfo> = functions.iter().filter(|f| f.name == callee.0).collect();
    let best = candidates
        .iter()
        .min_by_key(|f| {
            (!owner_match(f), f.location.path != path, f.location.path.starts_with("<std>"), f.qualified.clone())
        })
        .copied();
    // The compiler's built-ins (`println`, `String.push_str`) are not functions of the program: their signatures come
    // from the built-ins' file.
    let builtin = match (best, &receiver_type) {
        (Some(f), Some(_)) if owner_match(f) => None,
        (Some(_), None) if !dotted => None,
        (_, Some(t)) => {
            let builtins = crate::builtins::get();
            // An array's own methods (`iter_mut`) are declared on `[T; N]`.
            builtins.find(t, &callee.0).or_else(|| if t == "[T]" { builtins.find("[T; N]", &callee.0) } else { None })
        }
        (None, None) if !dotted => crate::builtins::get().find("", &callee.0),
        _ => None,
    };
    // An extension on an array or a slice is not found by name among the functions: the declarations of the
    // standard library say which fits the receiver's element (`[u8].is_ascii`).
    let declared = receiver
        .as_ref()
        .and_then(|t| answer.type_at(&path, t.lo))
        .map(|ty| Owners::of(&ty))
        .filter(|owners| matches!(owners, Owners::Sequence { .. }))
        .and_then(|owners| {
            all.iter()
                .map(|(_, i)| i)
                .find(|i| i.kind == Kind::Method && i.name == callee.0 && owners.owns(i.owner.as_deref()))
        });
    let (label, has_self) = match (builtin, best) {
        (Some(item), _) => (item.detail.clone(), takes_self(&item.detail)),
        (None, _) if declared.is_some() => {
            let item = declared.expect("checked");
            (item.detail.clone(), takes_self(&item.detail))
        }
        (None, Some(f)) => (f.display.clone(), f.params.first().is_some_and(|p| p.name == "self")),
        (None, None) => return Ok(Json::Null),
    };
    let parameters = parameter_labels(&label);
    let skip_self = by_value && has_self;
    let mut active = commas + u32::from(skip_self);
    // The variadic part of `println(fmt: str, ...)` takes every argument after the ones before it.
    if label.contains("...)") && !parameters.is_empty() {
        active = active.min(parameters.len() as u32 - 1);
    }
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
    // The built-in types stay unclassified, as the editor's grammar colors them.
    for (_, item) in all.iter().filter(|(file, _)| file != crate::builtins::PATH) {
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
    if let Some(analysis) = r.p.current_typed() {
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

/// `completionItem/resolve`: the documentation of an item of the latest completion (`spec/lsp.md`, LS23). An item the
/// server does not know, or one that has its documentation, comes back as it is.
pub fn resolve_completion(server: &mut Server, params: &Json) -> Reply {
    let mut item = params.clone();
    if item.get("documentation").is_null()
        && let Some(label) = item.get("label").as_str()
        && let Some(docs) = server.completion_docs.get(label)
    {
        let docs = Json::obj().with("kind", "markdown").with("value", docs.as_str());
        item.set("documentation", docs);
    }
    Ok(item)
}

/// `textDocument/onTypeFormatting`: typing `}` formats the line it closes, and a newline the line it ends, with the
/// rules of `burdin fmt` (`spec/lsp.md`, LS24). A file that does not parse yet is left alone.
pub fn on_type_formatting(server: &mut Server, params: &Json) -> Reply {
    let (i, path) = server.document(params, false)?;
    let text = server.packages[i].text(&path).unwrap_or("").to_string();
    let index = LineIndex::new(&text, server.encoding);
    let Some(at) = index.offset_of(params.get("position")) else { return Err(ReqError::params("missing position")) };
    let at = (at as usize).min(text.len());
    let line_start = text[..at].rfind('\n').map_or(0, |n| n + 1);
    // A newline was typed: the line it ended is the one before the cursor's.
    let (lo, hi) = match params.get("ch").as_str() {
        Some("\n") if line_start > 0 => {
            let before = line_start - 1;
            (text[..before].rfind('\n').map_or(0, |n| n + 1), before)
        }
        _ => (line_start, text[at..].find('\n').map_or(text.len(), |n| at + n)),
    };
    let range = index.range(lo as u32, hi as u32);
    formatting(server, &doc_range(params, range), true)
}

fn doc_range(params: &Json, range: Json) -> Json {
    Json::obj().with("textDocument", params.get("textDocument").clone()).with("range", range)
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
