//! Changing a function's signature (RF13): reordering its parameters, adding one that has a default, removing one,
//! and renaming one, with every call site updated to match. Calls written with named arguments keep their names, so
//! the argument keeps reaching the parameter it named; a call that left a parameter out keeps leaving it out.
//!
//! Only free functions take part: a method's receiver is part of the call's syntax in a way that a reordering would
//! have to rewrite more than a list of arguments.

use crate::tree;
use crate::{Engine, Outcome, apply, refuse};
use burdin_syntax::ast::{self, Item, ParamKind};
use std::collections::BTreeMap;

/// What to change about a signature. Every part is optional; the parts apply in this order: remove, reorder, add,
/// rename.
#[derive(Clone, Debug, Default)]
pub struct Change {
    /// Parameters to take out, by name.
    pub remove: Vec<String>,
    /// The new order of the parameters that stay, by name. Every one of them must be named, once.
    pub reorder: Option<Vec<String>>,
    /// Parameters to add at the end: `(name, type, default)`. The default is required: a caller written before the
    /// change does not pass the new parameter.
    pub add: Vec<(String, String, String)>,
    /// New names, `(old, new)`.
    pub rename: Vec<(String, String)>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.remove.is_empty() && self.reorder.is_none() && self.add.is_empty() && self.rename.is_empty()
    }
}

/// One parameter after the change.
struct NewParam {
    old: Option<String>,
    text: String,
}

/// Changes the signature of the free function declared at `lo..hi` of `path` and updates every call of it (RF13).
pub fn change(engine: &mut Engine<'_>, path: &str, lo: u32, hi: u32, change: &Change) -> Outcome {
    if change.is_empty() {
        return refuse("nothing to change: pass --reorder, --add, --remove, or --rename");
    }
    let parsed = engine.ast(path)?;
    let text = parsed.text.to_string();
    let mut found: Option<ast::FnDecl> = None;
    for item in &parsed.ast.items {
        if let Item::Fn(decl) = item
            && decl.span.lo <= lo
            && hi <= decl.span.hi
        {
            found = Some(decl.clone());
        }
    }
    let Some(decl) = found else { return refuse("the selection is not inside a free function") };
    let name = decl.name.name.clone();
    if decl.params.iter().any(|p| matches!(p.kind, ParamKind::SelfParam)) {
        return refuse(format!("`{name}` is a method; only a free function's signature can be changed"));
    }
    let old: Vec<(String, String)> = decl
        .params
        .iter()
        .filter_map(|p| match &p.kind {
            ParamKind::Named { name, .. } => {
                Some((name.name.clone(), text[p.span.lo as usize..p.span.hi as usize].to_string()))
            }
            ParamKind::SelfParam => None,
        })
        .collect();
    for removed in &change.remove {
        if !old.iter().any(|(n, _)| n == removed) {
            return refuse(format!("`{removed}` is not a parameter of `{name}`"));
        }
        if body_mentions(&text, &decl, removed) {
            return refuse(format!(
                "the body of `{name}` uses `{removed}`; removing the parameter would leave that use with no value"
            ));
        }
    }
    for (from, to) in &change.rename {
        if !old.iter().any(|(n, _)| n == from) {
            return refuse(format!("`{from}` is not a parameter of `{name}`"));
        }
        if !tree::is_identifier(to) {
            return refuse(format!("`{to}` is not a name a parameter can have"));
        }
        let taken = old.iter().any(|(n, _)| n == to && n != from);
        if taken || change.add.iter().any(|(n, _, _)| n == to) {
            return refuse(format!("`{to}` is already a parameter of `{name}`"));
        }
    }
    // The parameters that stay, in their new order.
    let kept: Vec<String> = old.iter().map(|(n, _)| n.clone()).filter(|n| !change.remove.contains(n)).collect();
    let order: Vec<String> = match &change.reorder {
        Some(list) => {
            let mut seen: Vec<&String> = Vec::new();
            for n in list {
                if !kept.contains(n) {
                    return refuse(format!("`--reorder` names `{n}`, which is not a parameter that stays"));
                }
                if seen.contains(&n) {
                    return refuse(format!("`--reorder` names `{n}` twice"));
                }
                seen.push(n);
            }
            if seen.len() != kept.len() {
                return refuse(format!(
                    "`--reorder` must name every parameter that stays, in its new order: {}",
                    kept.join(", ")
                ));
            }
            list.clone()
        }
        None => kept,
    };
    let mut new_params: Vec<NewParam> = Vec::new();
    for n in &order {
        let (old_name, param_text) = old.iter().find(|(o, _)| o == n).expect("kept");
        let new_name = change.rename.iter().find(|(f, _)| f == old_name).map(|(_, t)| t).unwrap_or(old_name);
        let renamed = rename_param(engine, path, &decl, old_name, new_name, param_text);
        new_params.push(NewParam { old: Some(old_name.clone()), text: renamed });
    }
    for (new_name, ty, default) in &change.add {
        if !tree::is_identifier(new_name) {
            return refuse(format!("`{new_name}` is not a name a parameter can have"));
        }
        if default.trim().is_empty() {
            return refuse(format!(
                "a new parameter needs a default value, or every call written before the change would break: \
                 write `--add '{new_name}: {ty} = value'`"
            ));
        }
        let existing = new_params.iter().any(|p| p.old.as_deref() == Some(new_name.as_str()));
        if existing || old.iter().any(|(n, _)| n == new_name) {
            return refuse(format!("`{new_name}` is already a parameter of `{name}`"));
        }
        new_params.push(NewParam { old: None, text: format!("{new_name}: {ty} = {default}") });
    }
    let new_list = new_params.iter().map(|p| p.text.clone()).collect::<Vec<_>>().join(", ");
    // The signature's parentheses.
    let Some((open, close)) = param_parens(&text, decl.name.span.hi as usize) else {
        return refuse(format!("cannot find the parameter list of `{name}`"));
    };
    let mut edits: Vec<(usize, usize, String)> = vec![(open + 1, close, new_list)];
    // The body uses the new names.
    let body = decl.body.span;
    for (from, to) in &change.rename {
        for (at, word) in tree::identifiers(&text[body.lo as usize..body.hi as usize]) {
            if &word != from {
                continue;
            }
            let absolute = body.lo as usize + at;
            let before = text[..absolute].trim_end().chars().next_back();
            if before == Some('.') {
                continue;
            }
            edits.push((absolute, absolute + word.len(), to.clone()));
        }
    }
    // Every call: the arguments in the new order, named ones still named, left-out defaults still left out.
    for call in engine.analysis.calls() {
        let Some(location) = &call.callee_location else { continue };
        if location.path != path || location.lo != decl.span.lo || location.hi != decl.span.hi {
            continue;
        }
        let written: Vec<(String, String, u32)> = call
            .args
            .iter()
            .map(|(param, at)| {
                let value = text[at.lo as usize..at.hi as usize].to_string();
                (param.name.clone(), value, at.lo)
            })
            .collect();
        let mut new_args: Vec<String> = Vec::new();
        for param in &new_params {
            let Some(old_name) = &param.old else { continue };
            if let Some((_, value, arg_lo)) = written.iter().find(|(n, _, _)| n == old_name) {
                let named = is_named(&text, *arg_lo as usize, old_name);
                let new_name = change.rename.iter().find(|(f, _)| f == old_name).map(|(_, t)| t);
                new_args.push(match (named, new_name) {
                    (true, Some(to)) => format!("{to}: {value}"),
                    (true, None) => format!("{old_name}: {value}"),
                    (false, _) => value.clone(),
                });
            }
        }
        let Some((open, close)) = call_parens(&text, call.at.lo as usize, call.at.hi as usize) else {
            return refuse(format!("cannot find the arguments of the call at {}", engine.place(path, call.at.lo)));
        };
        edits.push((open + 1, close, new_args.join(", ")));
    }
    let mut edited = BTreeMap::new();
    edited.insert(path.to_string(), apply(&text, edits));
    engine.verify(edited, &std::collections::HashMap::new())
}

/// Whether the argument at `at` was written with its parameter's name: the call site's text is the only record of
/// that, so the text right before the argument's value is read for `name:`.
fn is_named(text: &str, at: usize, name: &str) -> bool {
    text[..at].trim_end().ends_with(&format!("{name}:"))
}

/// Whether the body of `decl` mentions `name`.
fn body_mentions(text: &str, decl: &ast::FnDecl, name: &str) -> bool {
    let body = &text[decl.body.span.lo as usize..decl.body.span.hi as usize];
    tree::identifiers(body).iter().any(|(_, n)| n == name)
}

/// `param_text` with the parameter's name written as `new_name`.
fn rename_param(engine: &Engine<'_>, path: &str, decl: &ast::FnDecl, old: &str, new: &str, param_text: &str) -> String {
    if old == new {
        return param_text.to_string();
    }
    let name_span = decl.params.iter().find_map(|p| match &p.kind {
        ParamKind::Named { name, .. } if name.name == old => Some(name.span),
        _ => None,
    });
    let _ = (engine, path);
    match name_span {
        Some(span) if decl.span.file == span.file => {
            let (lo, hi) = ((span.lo - decl.params[0].span.lo) as usize, (span.hi - decl.params[0].span.lo) as usize);
            let _ = (lo, hi);
            // The parameter's text starts at the first parameter written, which is `lo` in this file's text; the
            // name is found within its own text instead, so no arithmetic across parameters is needed.
            let name_at = param_text.find(old).unwrap_or(0);
            let mut out = String::new();
            out.push_str(&param_text[..name_at]);
            out.push_str(new);
            out.push_str(&param_text[name_at + old.len()..]);
            out
        }
        _ => param_text.replacen(old, new, 1),
    }
}

/// The byte range between the parentheses of a parameter list that starts after `after`.
fn param_parens(text: &str, after: usize) -> Option<(usize, usize)> {
    let open = text[after..].find('(')? + after;
    let close = matching(text, open, '(', ')')?;
    Some((open, close))
}

/// The byte range between the parentheses of a call that spans `lo..hi`.
fn call_parens(text: &str, lo: usize, hi: usize) -> Option<(usize, usize)> {
    let open = text[lo..hi].find('(')? + lo;
    let close = matching(text, open, '(', ')')?;
    Some((open, close))
}

/// The index of the `close` that matches the `open` at `at`, skipping string literals.
fn matching(text: &str, at: usize, open: char, close: char) -> Option<usize> {
    let bytes: Vec<char> = text[at..].chars().collect();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut index = 0usize;
    let mut offset = at;
    while index < bytes.len() {
        let c = bytes[index];
        offset += c.len_utf8();
        index += 1;
        match quote {
            Some(q) => {
                if c == '\\' {
                    index += 1;
                    continue;
                }
                if c == q {
                    quote = None;
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                } else if c == open {
                    depth += 1;
                } else if c == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some(offset - c.len_utf8());
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{after, db, span_of};

    const SOURCE: &str = "fn area(width: i32, height: i32): i32 {\n    width * height\n}\n\nfn main() {\n    let a = area(width: 2, height: 3)\n    println(\"{a}\")\n}\n";

    #[test]
    fn reorders_parameters_and_their_named_arguments() {
        let mut db = db(&[("main.bd", SOURCE)]);
        let (lo, hi) = span_of(SOURCE, "fn area", 0);
        let change = Change { reorder: Some(vec!["height".to_string(), "width".to_string()]), ..Change::default() };
        let mut engine = Engine::new(&mut db).expect("engine");
        let text = after(super::change(&mut engine, "main.bd", lo, hi, &change), "main.bd", SOURCE).expect("changed");
        assert!(text.contains("fn area(height: i32, width: i32)"), "{text}");
        assert!(text.contains("area(height: 3, width: 2)"), "{text}");
    }

    #[test]
    fn renames_a_parameter_in_the_body_and_the_calls() {
        let mut db = db(&[("main.bd", SOURCE)]);
        let (lo, hi) = span_of(SOURCE, "fn area", 0);
        let change = Change { rename: vec![("width".to_string(), "w".to_string())], ..Change::default() };
        let mut engine = Engine::new(&mut db).expect("engine");
        let text = after(super::change(&mut engine, "main.bd", lo, hi, &change), "main.bd", SOURCE).expect("changed");
        assert!(text.contains("fn area(w: i32, height: i32)"), "{text}");
        assert!(text.contains("w * height"), "{text}");
        assert!(text.contains("area(w: 2, height: 3)"), "{text}");
    }

    #[test]
    fn refuses_to_remove_a_parameter_the_body_uses() {
        let mut db = db(&[("main.bd", SOURCE)]);
        let (lo, hi) = span_of(SOURCE, "fn area", 0);
        let change = Change { remove: vec!["width".to_string()], ..Change::default() };
        let mut engine = Engine::new(&mut db).expect("engine");
        let message =
            after(super::change(&mut engine, "main.bd", lo, hi, &change), "main.bd", SOURCE).expect_err("refused");
        assert!(message.contains("uses `width`"), "{message}");
    }
}
