//! Renaming a module's file in the editor (`spec/lsp.md`, LS22): the edits that keep the `use` paths of the package
//! pointing at it.

use crate::json::Json;
use crate::server::{Reply, Server};
use crate::text::LineIndex;
use std::collections::BTreeMap;

/// A module of a package and the module it becomes.
struct Moved {
    old: Vec<String>,
    new: Vec<String>,
}

/// `workspace/willRenameFiles`: for each file or folder moved inside a package's `src` directory, every `use` that
/// names a module of it is rewritten to the new module path. A `use shapes` that is renamed to `geometry` becomes
/// `use geometry as shapes`, so the code that writes `shapes.Square` still compiles. A file moved out of `src`, or
/// outside every package, changes nothing.
pub fn will_rename(server: &mut Server, params: &Json) -> Reply {
    let mut edits: BTreeMap<String, Vec<(u32, u32, String)>> = BTreeMap::new();
    let mut packages: Vec<usize> = Vec::new();
    let mut moved: Vec<Vec<Moved>> = Vec::new();
    for file in params.get("files").as_array() {
        let (Some(old), Some(new)) = (file.get("oldUri").as_str(), file.get("newUri").as_str()) else { continue };
        let (Some(old), Some(new)) = (server.key_of_uri(old), server.key_of_uri(new)) else { continue };
        let owner = match owner_of_folder(server, &old) {
            Some(i) => Some(i),
            None => server.owner(&old),
        };
        let Some(i) = owner else { continue };
        let list = modules_moved(server, i, &old, &new);
        if list.is_empty() {
            continue;
        }
        match packages.iter().position(|p| *p == i) {
            Some(at) => moved[at].extend(list),
            None => {
                packages.push(i);
                moved.push(list);
            }
        }
    }
    for (i, list) in packages.into_iter().zip(moved) {
        for path in server.packages[i].user_files() {
            let Some(ix) = server.packages[i].index(&path) else { continue };
            for u in &ix.uses {
                if let Some(edit) = rewrite_use(u, &list) {
                    edits.entry(path.clone()).or_default().push(edit);
                }
            }
        }
    }
    let mut changes = Json::obj();
    for (path, list) in edits {
        let text = server.text_of(&path).unwrap_or_default();
        let index = LineIndex::new(&text, server.encoding);
        let items: Vec<Json> = list
            .iter()
            .map(|(lo, hi, new)| Json::obj().with("range", index.range(*lo, *hi)).with("newText", new.as_str()))
            .collect();
        changes = changes.with(&server.uri_of(&path), Json::Arr(items));
    }
    Ok(Json::obj().with("changes", changes))
}

/// The package that holds a folder: the one whose modules have files under it.
fn owner_of_folder(server: &Server, folder: &str) -> Option<usize> {
    let prefix = format!("{folder}/");
    (0..server.packages.len()).find(|&i| server.packages[i].user_files().iter().any(|f| f.starts_with(&prefix)))
}

/// The modules of package `i` that moving `old` to `new` renames. A folder moves every module under it.
fn modules_moved(server: &Server, i: usize, old: &str, new: &str) -> Vec<Moved> {
    let package = &server.packages[i];
    let folder = format!("{old}/");
    let mut out = Vec::new();
    for m in package.modules.iter().filter(|m| !package.packages[m.package].embedded && !m.module.is_empty()) {
        let target = if m.path == old {
            new.to_string()
        } else if let Some(rest) = m.path.strip_prefix(&folder) {
            format!("{new}/{rest}")
        } else {
            continue;
        };
        // The source directory is what the old path has before its module path.
        let suffix = format!("/{}.bd", m.module.join("/"));
        let Some(src) = m.path.strip_suffix(&suffix) else { continue };
        let Some(inner) = target.strip_prefix(&format!("{src}/")).and_then(|t| t.strip_suffix(".bd")) else {
            continue;
        };
        let renamed: Vec<String> = inner.split('/').map(str::to_string).collect();
        if renamed != m.module {
            out.push(Moved { old: m.module.clone(), new: renamed });
        }
    }
    out
}

/// The edit that points a `use` at the new module path, when it names a module that moved (the longest match wins).
fn rewrite_use(u: &crate::index::Use, moved: &[Moved]) -> Option<(u32, u32, String)> {
    let names: Vec<&str> = u.path.iter().map(|(n, _)| n.as_str()).collect();
    let hit = moved
        .iter()
        .filter(|m| names.len() >= m.old.len() && names.iter().zip(&m.old).all(|(a, b)| *a == b.as_str()))
        .max_by_key(|m| m.old.len())?;
    let lo = u.path.first()?.1.0;
    let hi = u.path[hit.old.len() - 1].1.1;
    let mut text = hit.new.join(".");
    // `use shapes` names the module by its last segment in the code; keep that name.
    let module_only = names.len() == hit.old.len() && u.items.is_empty() && u.alias.is_none();
    let last = hit.old.last()?;
    if module_only && hit.new.last() != Some(last) {
        text.push_str(&format!(" as {last}"));
    }
    Some((lo, hi, text))
}
