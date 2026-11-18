//! `use` declarations: reading them, and sorting and grouping a run of them (spec/fmt.md, FM9).

use crate::cursor::{D, Fmt, R};
use crate::doc::{Doc, concat, group, if_break, indent, text};
use burdin_syntax::token::TokenKind;

/// What a `use` declaration says, without its layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UseInfo {
    pub is_pub: bool,
    /// The module or item path, without the names of a brace list.
    pub path: Vec<String>,
    pub alias: Option<String>,
    /// The names of a `.{...}` list.
    pub items: Vec<String>,
}

/// A `use` declaration with the comments it carried.
pub struct UseEntry {
    pub info: UseInfo,
    /// Comments on their own lines in front of it.
    pub lead: D,
    /// Every other comment of the declaration, which follows it.
    pub suffix: D,
}

impl UseEntry {
    fn has_comments(&self) -> bool {
        !self.lead.is_nil() || !self.suffix.is_nil()
    }
}

/// How `use` declarations are grouped: the standard library, the dependencies of the package, and the package itself.
#[derive(Clone, Debug, Default)]
pub struct UseGroups {
    pub std_modules: Vec<String>,
    pub dependencies: Vec<String>,
}

impl UseGroups {
    fn class(&self, info: &UseInfo) -> u8 {
        let first = info.path.first().map(String::as_str).unwrap_or("");
        if first == "std" || self.std_modules.iter().any(|m| m == first) {
            0
        } else if self.dependencies.iter().any(|d| d == first) {
            1
        } else {
            2
        }
    }
}

impl UseInfo {
    /// The declaration as a document: the brace list breaks when the line is too long.
    pub fn doc(&self) -> D {
        let mut head = String::new();
        if self.is_pub {
            head.push_str("pub ");
        }
        head.push_str("use ");
        head.push_str(&self.path.join("."));
        if let Some(alias) = &self.alias {
            return text(format!("{head} as {alias}"));
        }
        if self.items.is_empty() {
            return text(head);
        }
        head.push_str(".{");
        let mut inner = Vec::new();
        for (i, item) in self.items.iter().enumerate() {
            if i > 0 {
                inner.push(text(","));
                inner.push(Doc::Line);
            }
            inner.push(text(item.clone()));
        }
        group(concat(vec![
            text(head),
            indent(concat(vec![Doc::Soft, concat(inner), if_break(text(","), Doc::Nil)])),
            Doc::Soft,
            text("}"),
        ]))
    }
}

impl Fmt<'_> {
    /// Whether the cursor is at `use` or `pub use`.
    pub fn at_use(&self) -> bool {
        self.at(&TokenKind::Use) || (self.at_word("pub") && self.nth(1) == &TokenKind::Use)
    }

    /// Reads one `use` declaration. Its comments are gathered up: the ones on their own lines in front of it stay
    /// there and the rest follow it, so a declaration can be moved without losing any.
    pub fn use_entry(&mut self) -> R<UseEntry> {
        let first = self.pos;
        let mut i = first;
        let is_pub = self.at_word("pub");
        if is_pub {
            i += 1;
        }
        i += 1; // `use`
        let name_at = |f: &Fmt<'_>, i: usize| -> R<String> {
            match f.toks.get(i).map(|t| &t.kind) {
                Some(TokenKind::Ident(name)) => Ok(name.clone()),
                _ => Err(f.error("expected a name in a `use` declaration")),
            }
        };
        let mut path = vec![name_at(self, i)?];
        i += 1;
        let mut items = Vec::new();
        let mut alias = None;
        while self.toks[i].kind == TokenKind::Dot {
            i += 1;
            if self.toks[i].kind == TokenKind::LBrace {
                i += 1;
                while self.toks[i].kind != TokenKind::RBrace {
                    items.push(name_at(self, i)?);
                    i += 1;
                    if self.toks[i].kind == TokenKind::Comma {
                        i += 1;
                    } else {
                        break;
                    }
                }
                if self.toks[i].kind != TokenKind::RBrace {
                    return Err(self.error("expected `}` in a `use` declaration"));
                }
                i += 1;
                break;
            }
            path.push(name_at(self, i)?);
            i += 1;
        }
        if self.toks[i].kind == TokenKind::As {
            alias = Some(name_at(self, i + 1)?);
            i += 2;
        }
        let last = i - 1;
        let lead = self.start_of_line();
        let mut suffix = Vec::new();
        for index in first..=last {
            let lead = std::mem::take(&mut self.toks[index].lead);
            let trail = std::mem::take(&mut self.toks[index].trail);
            self.comments += lead.len() + trail.len();
            for c in lead.iter().chain(trail.iter()) {
                suffix.push(Doc::Suffix(format!(" {}", c.text)));
            }
        }
        self.pos = i.min(self.toks.len() - 1);
        Ok(UseEntry { info: UseInfo { is_pub, path, alias, items }, lead, suffix: concat(suffix) })
    }
}

/// Merges, sorts, and groups a run of `use` declarations of one visibility. The result is the documents to print,
/// one per line, with `true` before a document that starts a new group (a blank line goes there).
pub fn arrange(entries: Vec<UseEntry>, groups: &UseGroups) -> Vec<(bool, D)> {
    let mut merged: Vec<UseEntry> = Vec::new();
    for entry in entries {
        let plain = !entry.has_comments();
        if plain
            && let Some(same) = merged.iter_mut().find(|m| {
                !m.has_comments()
                    && m.info.is_pub == entry.info.is_pub
                    && m.info.path == entry.info.path
                    && m.info.alias == entry.info.alias
                    && m.info.items.is_empty() == entry.info.items.is_empty()
            })
        {
            // The same path twice: a repeated name, or two lists of names, are one declaration.
            for item in &entry.info.items {
                same.info.items.push(item.clone());
            }
            continue;
        }
        merged.push(entry);
    }
    for entry in &mut merged {
        entry.info.items.sort();
        entry.info.items.dedup();
    }
    merged.sort_by(|a, b| {
        let ka = (groups.class(&a.info), &a.info.path, &a.info.items, &a.info.alias);
        let kb = (groups.class(&b.info), &b.info.path, &b.info.items, &b.info.alias);
        ka.cmp(&kb)
    });
    let mut out = Vec::new();
    let mut previous_class = None;
    for entry in merged {
        let class = groups.class(&entry.info);
        let new_group = previous_class.is_some_and(|p| p != class);
        previous_class = Some(class);
        let doc = concat(vec![entry.lead, entry.info.doc(), entry.suffix]);
        out.push((new_group, doc));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc;

    fn entry(is_pub: bool, path: &[&str], items: &[&str]) -> UseEntry {
        UseEntry {
            info: UseInfo {
                is_pub,
                path: path.iter().map(|s| s.to_string()).collect(),
                alias: None,
                items: items.iter().map(|s| s.to_string()).collect(),
            },
            lead: Doc::Nil,
            suffix: Doc::Nil,
        }
    }

    fn render(entries: Vec<UseEntry>, groups: &UseGroups) -> String {
        let mut lines = Vec::new();
        for (new_group, mut d) in arrange(entries, groups) {
            doc::prepare(&mut d);
            if new_group {
                lines.push(String::new());
            }
            lines.push(doc::print(&d, 100));
        }
        lines.join("\n")
    }

    #[test]
    fn lists_with_the_same_path_merge_and_sort() {
        let out = render(
            vec![entry(false, &["text"], &["shout", "trim"]), entry(false, &["text"], &["lower", "shout"])],
            &UseGroups::default(),
        );
        assert_eq!(out, "use text.{lower, shout, trim}");
    }

    #[test]
    fn groups_are_std_then_dependencies_then_the_package() {
        let groups = UseGroups { std_modules: vec!["io".into()], dependencies: vec!["json".into()] };
        let out = render(
            vec![entry(false, &["shapes"], &[]), entry(false, &["json"], &[]), entry(false, &["io"], &[])],
            &groups,
        );
        assert_eq!(out, "use io\n\nuse json\n\nuse shapes");
    }

    #[test]
    fn plain_uses_are_not_merged_with_lists() {
        // `use a.b` may name a module, which `use a.{b}` does not import, so the two stay apart.
        let out = render(vec![entry(false, &["a", "b"], &[]), entry(false, &["a"], &["b"])], &UseGroups::default());
        assert_eq!(out, "use a.{b}\nuse a.b");
    }

    #[test]
    fn a_duplicate_is_removed() {
        let out = render(vec![entry(false, &["a", "b"], &[]), entry(false, &["a", "b"], &[])], &UseGroups::default());
        assert_eq!(out, "use a.b");
    }
}
