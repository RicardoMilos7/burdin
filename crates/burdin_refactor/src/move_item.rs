//! Moving a top-level item to another module of the same package (RF14). The item keeps its name, so the module it
//! left re-exports it with `pub use`: every file that used it keeps working and needs no edit. The names the item
//! used from its old module are imported into the new one, since a module does not see its siblings' items.

use crate::tree;
use crate::{Engine, Outcome, Refusal, apply, refuse};
use burdin_syntax::ast::{Item, UseDecl};
use std::collections::BTreeMap;

/// Moves the item at `lo..hi` of `path` into the module `to` of the same package (RF14).
pub fn item(engine: &mut Engine<'_>, path: &str, lo: u32, hi: u32, to: &str) -> Outcome {
    let parsed = engine.ast(path)?;
    let text = parsed.text.to_string();
    let mut moved: Option<(Item, String)> = None;
    for item in &parsed.ast.items {
        let span = tree::item_span(item);
        if !(span.lo <= lo && hi <= span.hi) {
            continue;
        }
        let Some(name) = tree::item_name(item).map(|n| n.name.clone()) else {
            return refuse("the selection is not an item that can be moved by name");
        };
        match item {
            Item::Fn(_)
            | Item::Const(_)
            | Item::Struct(_)
            | Item::Enum(_)
            | Item::TypeAlias(_)
            | Item::Interface(_) => {
                moved = Some((item.clone(), name));
            }
            _ => {
                return refuse("only a function, constant, struct, enum, type alias, or interface can be moved");
            }
        }
    }
    let Some((item, name)) = moved else { return refuse("the selection is not a top-level item") };
    let span = tree::item_span(&item);
    let from_module = engine.module_of(path).ok_or_else(|| Refusal(format!("`{path}` is not a module")))?;
    // The target module, by its dotted path.
    let packages = engine.db.packages();
    let modules = engine.db.modules();
    let Some(target) = modules.iter().position(|m| {
        packages.get(m.package).is_some_and(|p| p.is_entry) && m.path != path && m.module.join(".") == to
    }) else {
        return refuse(format!("the package has no module `{to}`; name the module of a file under src/"));
    };
    let target_path = modules[target].path.clone();
    let target_module = modules[target].module.clone();
    if target_module == from_module {
        return refuse(format!("`{name}` is already in module `{to}`"));
    }
    let target_text = engine.text(&target_path);
    // The names the item uses that the target module does not see.
    let old_names: Vec<String> =
        parsed.ast.items.iter().filter_map(|i| tree::item_name(i).map(|n| n.name.clone())).collect();
    let target_names: Vec<String> =
        engine.ast(&target_path)?.ast.items.iter().filter_map(|i| tree::item_name(i).map(|n| n.name.clone())).collect();
    let target_uses = imported_names(&engine.ast(&target_path)?.ast.uses);
    let item_text = text[span.lo as usize..span.hi as usize].to_string();
    let mut needed: Vec<String> = Vec::new();
    for (_, word) in tree::identifiers(&item_text) {
        if word == name
            || target_names.contains(&word)
            || target_uses.contains(&word)
            || needed.contains(&word)
            || !old_names.contains(&word)
        {
            continue;
        }
        needed.push(word);
    }
    if !needed.is_empty() && from_module.is_empty() {
        return refuse(format!(
            "`{name}` uses {} from the package root, which no other module can import; move that code with it, or \
             move it from a named module",
            needed.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")
        ));
    }
    // The new target module: the item, and the imports it needs.
    let mut target_new = target_text.clone();
    if !target_new.ends_with('\n') {
        target_new.push('\n');
    }
    if !target_new.trim_end().ends_with('{') {
        target_new.push('\n');
    }
    let mut header: Vec<String> = needed.iter().map(|n| format!("use {}.{n}", from_module.join("."))).collect();
    // The source re-exports the item, so the files that used it need no edit.
    let reexport = format!("pub use {}.{name}", target_module.join("."));
    if let Some(at) = insertion_point(&target_new)
        && !header.is_empty()
    {
        header.push(String::new());
        target_new.insert_str(at, &header.join("\n"));
    }
    target_new.push_str(&item_text);
    target_new.push('\n');
    // The source loses the item and gains the re-export.
    let (remove_lo, remove_hi) = with_doc_comment(&text, span.lo as usize, span.hi as usize);
    let mut source_new = apply(&text, vec![(remove_lo, remove_hi, String::new())]);
    let at = insertion_point(&source_new)
        .ok_or_else(|| Refusal(format!("cannot find where the `use` declarations of `{path}` end")))?;
    source_new.insert_str(at, &format!("{reexport}\n"));
    let mut edited = BTreeMap::new();
    edited.insert(path.to_string(), source_new);
    edited.insert(target_path, target_new);
    engine.verify(edited, &std::collections::HashMap::new())
}

/// The names a file's `use` declarations bring into scope.
fn imported_names(uses: &[UseDecl]) -> Vec<String> {
    let mut out = Vec::new();
    for u in uses {
        if let Some(alias) = &u.alias {
            out.push(alias.name.clone());
        } else if !u.items.is_empty() {
            out.extend(u.items.iter().map(|i| i.name.clone()));
        } else if let Some(last) = u.path.last() {
            out.push(last.name.clone());
        }
    }
    out
}

/// Where a new `use` line goes: after the last top-level `use`, or after the `//!` header at the top, with a blank
/// line after the new line.
fn insertion_point(text: &str) -> Option<usize> {
    let mut header = 0usize;
    let mut in_header = true;
    let mut last_use: Option<usize> = None;
    let mut at = 0usize;
    for raw in text.split_inclusive('\n') {
        let trimmed = raw.trim_end();
        if in_header && (trimmed.starts_with("//!") || trimmed.starts_with("#!")) {
            header = at + raw.len();
        } else if !trimmed.is_empty() {
            in_header = false;
        }
        if trimmed.starts_with("use ") || trimmed.starts_with("pub use ") {
            last_use = Some(at + raw.len());
        }
        at += raw.len();
    }
    match last_use {
        Some(end) if text[..end].ends_with('\n') => Some(end),
        Some(end) => Some(end),
        None => Some(header),
    }
}

/// The span of an item with the doc comment lines above it and the line break after it.
fn with_doc_comment(text: &str, lo: usize, hi: usize) -> (usize, usize) {
    let mut start = tree::line_start(text, lo);
    while start > 0 {
        let previous_end = start - 1;
        let previous_start = tree::line_start(text, previous_end);
        if !text[previous_start..previous_end].trim().starts_with("///") {
            break;
        }
        start = previous_start;
    }
    (start, tree::line_end(text, hi))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{after, db, span_of};

    #[test]
    fn moves_an_item_and_reexports_it() {
        let main = "fn helper(n: i32): i32 {\n    n * 2\n}\n\nfn main() {\n    println(\"{helper(3)}\")\n}\n";
        let util = "//! Utilities.\n";
        let mut db = db(&[("main.bd", main), ("util.bd", util)]);
        let (lo, hi) = span_of(main, "fn helper", 0);
        let mut engine = Engine::new(&mut db).expect("engine");
        let changes = item(&mut engine, "main.bd", lo, hi, "util").expect("moved");
        let source = changes.iter().find(|c| c.path == "main.bd").expect("source").after.clone();
        let target = changes.iter().find(|c| c.path == "util.bd").expect("target").after.clone();
        assert!(source.contains("pub use util.helper"), "{source}");
        assert!(!source.contains("fn helper"), "{source}");
        assert!(target.contains("fn helper"), "{target}");
        let _ = after(Ok(changes), "main.bd", main);
    }
}
