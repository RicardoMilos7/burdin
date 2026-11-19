//! Tab completion (`spec/repl.md`, RP13): names in scope, and the fields and methods after a `.`. Everything comes
//! from the query library — the session's typed statements for bindings, the session module's syntax tree for
//! items, and `Analysis::members_of_type` for a receiver — so completion agrees with what the checker accepts.

use crate::text::{SESSION_FILE, SESSION_FN};
use burdin_query::{Analysis, Db, StmtKind};

/// The keywords and modifiers completions offer (the lexer's set, `burdin_syntax::token`).
const KEYWORDS: &[&str] = &[
    "any",
    "as",
    "break",
    "comptime",
    "const",
    "continue",
    "else",
    "enum",
    "error",
    "extern",
    "false",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "interface",
    "let",
    "loop",
    "match",
    "mut",
    "orelse",
    "own",
    "priv",
    "return",
    "self",
    "struct",
    "true",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
];

/// The primitive type names (the grammar's `support.type.primitive`).
const PRIMITIVES: &[&str] = &[
    "bool", "char", "str", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128",
    "usize",
];

/// Every candidate for the word `prefix` at the end of `before`, sorted and deduplicated.
pub(crate) fn candidates(db: &mut Db, analysis: &Analysis, before: &str, prefix: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // `x.` (or `x.pre`): the members of the receiver's type.
    if let Some(receiver) = member_receiver(before) {
        let ty = receiver_type(db, analysis, &receiver);
        if let Some(ty) = ty {
            for member in analysis.members_of_type(&ty) {
                if member.name.starts_with(prefix) {
                    out.push(member.name);
                }
            }
        }
        return sorted(out);
    }
    for (name, _) in bindings(analysis) {
        if name.starts_with(prefix) && !name.starts_with("__") {
            out.push(name);
        }
    }
    for name in module_items(db, SESSION_FILE) {
        if name.starts_with(prefix) {
            out.push(name);
        }
    }
    // The standard library's root is visible everywhere.
    for name in module_items(db, "<std>/std.bd") {
        if name.starts_with(prefix) {
            out.push(name);
        }
    }
    for name in KEYWORDS.iter().chain(PRIMITIVES.iter()) {
        if name.starts_with(prefix) {
            out.push((*name).to_string());
        }
    }
    sorted(out)
}

fn sorted(mut out: Vec<String>) -> Vec<String> {
    out.sort();
    out.dedup();
    out
}

/// The session's bindings, in declaration order, with their types (the last binding of a name shadows earlier
/// ones).
pub(crate) fn bindings(analysis: &Analysis) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for stmt in analysis.statements_of(SESSION_FILE, SESSION_FN).unwrap_or_default() {
        if let StmtKind::Let(names) = stmt.kind {
            for (name, ty) in names {
                out.retain(|(have, _)| have != &name);
                out.push((name, ty));
            }
        }
    }
    out
}

/// The receiver written before a trailing `.`: the dotted name before it. `foo().` and other expressions are not
/// resolved (they have no binding to ask about).
fn member_receiver(before: &str) -> Option<String> {
    let trimmed = before.trim_end();
    let text = trimmed.strip_suffix('.')?;
    let text = text.trim_end();
    let start = text
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_' || *c == '.')
        .last()
        .map(|(i, _)| i)?;
    let receiver = &text[start..];
    (!receiver.is_empty() && !receiver.ends_with('.')).then(|| receiver.to_string())
}

/// The type text of a receiver: the session's binding of that name, or a type the session declares.
fn receiver_type(db: &mut Db, analysis: &Analysis, receiver: &str) -> Option<String> {
    let name = receiver.rsplit('.').next().unwrap_or(receiver);
    if let Some((_, ty)) = bindings(analysis).into_iter().find(|(binding, _)| binding == name) {
        return Some(ty);
    }
    // A module-qualified receiver falls back to its last segment: `p.x` where `p` is a binding was handled above.
    let _ = db;
    None
}

/// The names of a module's top-level items and `use` declarations, from its syntax tree.
fn module_items(db: &mut Db, path: &str) -> Vec<String> {
    let Some(tree) = db.syntax_tree(path) else { return Vec::new() };
    let file = tree.ast();
    let mut out = Vec::new();
    for item in &file.items {
        if let Some(name) = item_name(item) {
            out.push(name);
        }
    }
    for u in &file.uses {
        if let Some(alias) = &u.alias {
            out.push(alias.name.clone());
        } else if u.items.is_empty() {
            if let Some(last) = u.path.last() {
                out.push(last.name.clone());
            }
        } else {
            out.extend(u.items.iter().map(|i| i.name.clone()));
        }
    }
    out
}

/// The name an item declares, if it has one.
fn item_name(item: &burdin_syntax::ast::Item) -> Option<String> {
    use burdin_syntax::ast::Item;
    match item {
        Item::Fn(f) => Some(f.name.name.clone()),
        Item::Extension(e) => Some(e.func.name.name.clone()),
        Item::Const(c) => Some(c.name.name.clone()),
        Item::Struct(s) => Some(s.name.name.clone()),
        Item::Enum(e) => Some(e.name.name.clone()),
        Item::ErrorEnum(e) => Some(e.name.name.clone()),
        Item::TypeAlias(a) => Some(a.name.name.clone()),
        Item::Interface(i) => Some(i.name.name.clone()),
        Item::ExternFn(f) => Some(f.decl.name.name.clone()),
        Item::Derive(d) => Some(d.name.name.clone()),
        Item::Impl(_) | Item::Extern(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dotted_receiver_is_recognized() {
        assert_eq!(member_receiver("s."), Some("s".to_string()));
        assert_eq!(member_receiver("p.x."), Some("p.x".to_string()));
        assert_eq!(member_receiver("let y = s."), Some("s".to_string()));
        assert_eq!(member_receiver("let y = s"), None);
        assert_eq!(member_receiver("foo()."), None);
    }
}
