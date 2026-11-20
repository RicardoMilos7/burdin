//! Workspace selection for the commands that build packages (`spec/workspaces.md`, WS5-WS10). The flags `-p`,
//! `--workspace`, and `--exclude` pick members of the workspace the target belongs to, whatever the target is (WS6).
//! Outside a workspace the flags select nothing a command does not already take (WS7).

use burdin_query::package;
use burdin_query::workspace::{self as query, Layout, Member};
use std::path::{Path, PathBuf};

/// What a command runs on, as the flags and the target decide it.
#[derive(Clone, Debug, Default)]
pub(crate) enum Scope {
    /// The target is not a member of a workspace: the command's own selection applies.
    #[default]
    Standalone,
    /// The selected members, in workspace order. `root` is the workspace root, where the outputs a run shares live
    /// (WS11). `named` are the names `-p` gave, which a command may refuse to skip (WS9).
    Members { root: PathBuf, members: Vec<Member>, named: Vec<String> },
}

/// The selection flags as the command line gave them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Flags<'a> {
    pub(crate) packages: &'a [String],
    pub(crate) workspace: bool,
    pub(crate) exclude: &'a [String],
}

/// Why the selection failed: a usage error (exit status 2) or a problem with the packages (exit status 1).
#[derive(Debug)]
pub(crate) enum Failure {
    Usage(String),
    Error(String),
}

/// The scope of a command run on `target`. `graph` is true for `test` and `bench`, which keep their selection of
/// packages of the build outside a workspace (WS7).
pub(crate) fn choose(target: &Path, flags: Flags<'_>, graph: bool) -> Result<Scope, Failure> {
    check_combination(flags)?;
    let dir = target.canonicalize().ok().filter(|dir| dir.is_dir());
    let root = match &dir {
        Some(dir) => query::root_of(dir).map_err(Failure::Error)?,
        None => None,
    };
    match (dir, root) {
        (Some(dir), Some(root)) => {
            let layout = query::layout(&root).map_err(Failure::Error)?;
            let selected = select(&layout, &dir, flags)?;
            let members = selected.iter().map(|&index| layout.members[index].clone()).collect();
            Ok(Scope::Members { root: layout.root.clone(), members, named: flags.packages.to_vec() })
        }
        (dir, _) => standalone(dir.as_deref(), flags, graph),
    }
}

/// The members of `layout` that the flags select for a command run in `here`, a canonical directory: the indices into
/// `layout.members`, in workspace order. Without a flag, a member's own directory selects that member, and any other
/// directory of the workspace selects the default members (WS5, WS6).
pub(crate) fn select(layout: &Layout, here: &Path, flags: Flags<'_>) -> Result<Vec<usize>, Failure> {
    check_combination(flags)?;
    let index_of = |name: &str| {
        layout.members.iter().position(|member| member.name == name).ok_or_else(|| {
            Failure::Usage(format!(
                "no member named `{name}` in this workspace; its members are {}",
                member_names(&layout.members)
            ))
        })
    };
    let named = flags.packages.iter().map(|name| index_of(name)).collect::<Result<Vec<usize>, Failure>>()?;
    let excluded = flags.exclude.iter().map(|name| index_of(name)).collect::<Result<Vec<usize>, Failure>>()?;
    let mut selected = if flags.workspace {
        (0..layout.members.len()).filter(|index| !excluded.contains(index)).collect()
    } else if !named.is_empty() {
        named
    } else {
        match layout.member_at(here) {
            Some(index) if here != layout.root => vec![index],
            _ => layout.default_members.clone(),
        }
    };
    selected.sort_unstable();
    selected.dedup();
    if selected.is_empty() {
        return Err(Failure::Usage("no package is selected: every member is excluded or the default is empty".into()));
    }
    Ok(selected)
}

/// The combinations of flags that never make sense (WS6), whatever the target is.
fn check_combination(flags: Flags<'_>) -> Result<(), Failure> {
    if flags.workspace && !flags.packages.is_empty() {
        return Err(Failure::Usage("`-p` and `--workspace` cannot be used together".to_string()));
    }
    if !flags.exclude.is_empty() && !flags.workspace {
        return Err(Failure::Usage("`--exclude` needs `--workspace`".to_string()));
    }
    Ok(())
}

/// A package outside any workspace (WS7). It takes `-p` naming itself, and `--workspace` means the one package.
/// `test` and `bench` (`graph`) select packages of the build instead, so their flags pass through unchecked.
fn standalone(dir: Option<&Path>, flags: Flags<'_>, graph: bool) -> Result<Scope, Failure> {
    // `--exclude` takes members out of a workspace, and this package has none (WS7).
    if !flags.exclude.is_empty() {
        return Err(Failure::Usage("`--exclude` selects members of a workspace, and this package is not one".into()));
    }
    if graph || flags.packages.is_empty() {
        return Ok(Scope::Standalone);
    }
    let own =
        dir.and_then(|dir| package::parse_manifest(&dir.join(query::MANIFEST)).ok()).map(|manifest| manifest.name);
    for name in flags.packages {
        if own.as_deref() != Some(name.as_str()) {
            let message = match &own {
                Some(own) => format!("no package named `{name}`: outside a workspace the only package is `{own}`"),
                None => format!("no package named `{name}`"),
            };
            return Err(Failure::Usage(message));
        }
    }
    Ok(Scope::Standalone)
}

fn member_names(members: &[Member]) -> String {
    members.iter().map(|member| format!("`{}`", member.name)).collect::<Vec<_>>().join(", ")
}

/// The directory a run's shared outputs live under: the workspace root for members (WS11), otherwise `None`, and the
/// caller uses the entry package's own directory.
pub(crate) fn shared_root(scope: &Scope) -> Option<PathBuf> {
    match scope {
        Scope::Members { root, .. } => Some(root.clone()),
        Scope::Standalone => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, dir: &str) -> Member {
        Member { name: name.to_string(), dir: PathBuf::from(dir) }
    }

    /// A workspace with a root package `app` and two more members; `app` is the default.
    fn layout() -> Layout {
        Layout {
            root: PathBuf::from("/ws"),
            members: vec![member("app", "/ws"), member("lib", "/ws/lib"), member("tool", "/ws/tool")],
            root_package: Some(0),
            default_members: vec![0],
        }
    }

    fn flags<'a>(packages: &'a [String], workspace: bool, exclude: &'a [String]) -> Flags<'a> {
        Flags { packages, workspace, exclude }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn selection_follows_the_flags() {
        let layout = layout();
        let root = Path::new("/ws");
        let lib = Path::new("/ws/lib");
        let none: Vec<String> = Vec::new();
        // Without a flag: the root's defaults from the root, and a member's own directory from inside it.
        assert_eq!(select(&layout, root, flags(&none, false, &none)).unwrap(), vec![0]);
        assert_eq!(select(&layout, lib, flags(&none, false, &none)).unwrap(), vec![1]);
        // `-p` selects whatever the target is, and a name given twice is selected once.
        let tool = names(&["tool", "tool"]);
        assert_eq!(select(&layout, lib, flags(&tool, false, &none)).unwrap(), vec![2]);
        // `--workspace` is every member, and `--exclude` takes members out of it.
        assert_eq!(select(&layout, root, flags(&none, true, &none)).unwrap(), vec![0, 1, 2]);
        let excluded = names(&["lib"]);
        assert_eq!(select(&layout, root, flags(&none, true, &excluded)).unwrap(), vec![0, 2]);
    }

    #[test]
    fn default_members_apply_from_the_root_only() {
        let mut layout = layout();
        layout.default_members = vec![1];
        let none: Vec<String> = Vec::new();
        assert_eq!(select(&layout, Path::new("/ws"), flags(&none, false, &none)).unwrap(), vec![1]);
        // A member's own directory selects that member, not the default.
        assert_eq!(select(&layout, Path::new("/ws/tool"), flags(&none, false, &none)).unwrap(), vec![2]);
    }

    #[test]
    fn unknown_names_and_empty_selections_are_usage_errors() {
        let layout = layout();
        let none: Vec<String> = Vec::new();
        let missing = names(&["nope"]);
        let Failure::Usage(message) = select(&layout, Path::new("/ws"), flags(&missing, false, &none)).unwrap_err()
        else {
            panic!("an unknown member is a usage error");
        };
        assert!(message.contains("no member named `nope`") && message.contains("`app`, `lib`, `tool`"));
        let Failure::Usage(message) = select(&layout, Path::new("/ws"), flags(&none, true, &missing)).unwrap_err()
        else {
            panic!("an unknown excluded member is a usage error");
        };
        assert!(message.contains("no member named `nope`"));
        let all = names(&["app", "lib", "tool"]);
        let Failure::Usage(message) = select(&layout, Path::new("/ws"), flags(&none, true, &all)).unwrap_err() else {
            panic!("excluding every member selects nothing");
        };
        assert!(message.contains("no package is selected"));
    }

    #[test]
    fn the_flags_must_fit_together() {
        let none: Vec<String> = Vec::new();
        let packages = names(&["lib"]);
        let Failure::Usage(message) = check_combination(flags(&packages, true, &none)).unwrap_err() else {
            panic!("`-p` and `--workspace` are a usage error");
        };
        assert_eq!(message, "`-p` and `--workspace` cannot be used together");
        let excluded = names(&["lib"]);
        let Failure::Usage(message) = check_combination(flags(&none, false, &excluded)).unwrap_err() else {
            panic!("`--exclude` without `--workspace` is a usage error");
        };
        assert_eq!(message, "`--exclude` needs `--workspace`");
    }
}
