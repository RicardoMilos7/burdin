# Workspaces: members, selection, and inherited fields

A workspace is one `burdin.toml` with a `[workspace]` section that lists the packages (its *members*) building together:
they share one `burdin.lock`, one set of pinned dependency versions, and one place to inherit a version, an edition, or a
dependency from. Stage 3 issues #342 (members, default members, excludes, and selection) and #343 (inheritance of package
fields). This file is the exact behavior, with a rule ID for each point and the tests that cover it. Until this change a
workspace meant the build's package graph (`spec/test-runner.md`, `spec/packages.md`); a `[workspace]` section now makes
the root a manifest-level workspace, and every rule below applies to it.

The pieces: the manifest parser and the inheritance in the package loader (`crates/burdin_query/src/package.rs`), the
membership and the layout of a workspace in `crates/burdin_query/src/workspace.rs`, the lockfile location and the
resolution of the whole workspace in `crates/burdin_pkg/src/resolve.rs`, and the selection in the driver
(`crates/burdin_driver/src/workspace.rs`). The black-box check is `tests/check_workspace.py`, run by every CI job with a
built driver; the unit tests of the parser, the membership rules, and the selection run under `cargo test --workspace`.

## The root

**WS1. A workspace root.** A `burdin.toml` with a `[workspace]` section is a workspace root. A root that also has a
`[package]` section is a package as well (the *root package*). A root without one is *virtual*: it has no package of its
own, so it may contain only `[workspace]`, `[workspace.package]`, and `[workspace.dependencies]`; a `[dependencies]`
section in a virtual root is an error. Unknown sections and keys are ignored, as in every manifest.
Tests: `package.rs` (`a_virtual_root_takes_only_workspace_sections`), `tests/check_workspace.py` (virtual roots).

**WS2. Membership.** The members of a workspace are its root package (when it has one) and every package directory that
`members` lists, minus the ones `exclude` removes. A package belongs to the nearest workspace root at or above its own
directory: the directory itself when it has a `[workspace]` section, otherwise the first ancestor with one. If that root
does not list the directory as a member (it is not listed, or `exclude` removes it), the directory is a *standalone*
package, and no rule in this file applies to it: it keeps its own lockfile and its own selection. A workspace root nested
in a member is an error (WS11).
Tests: `workspace.rs` (`membership_follows_the_nearest_root`), `tests/check_workspace.py` (an excluded package).

**WS3. Member patterns.** `members`, `exclude`, and `default-members` are arrays of strings on one line (a single string
is also accepted), as in `[lint]`-style manifest values. Each entry is a path relative to the root, with `/` separators
(`\` is accepted too). An entry must not be absolute and must not contain `..`. A component may contain `*`, which matches
any run of characters inside that component (`libs/*`, `crates/core-*`); `*` never crosses a `/`. An entry without `*`
names one directory, which must exist and must have a `burdin.toml` (an error that names the entry otherwise). An entry
with `*` matches the directories whose names fit and that have a `burdin.toml`; other directories are skipped. Hidden
directories (names starting with `.`) are never matched by `*`. An `exclude` entry follows the same rules and removes
matched directories from the members; `exclude` may not name the root. A `default-members` entry must name a member.
Tests: `workspace.rs` (`member_patterns_match_one_component`, `explicit_entries_must_be_packages`), `tests/check_workspace.py`.

**WS4. Names.** Every member's `[package] name` is unique in its workspace, because a name identifies a package in the
selection flags, the test and bench names, and the lockfile. Two members with one name are an error that names both
directories. Tests: `workspace.rs` (`member_names_are_unique`).

**WS5. Default members.** `default-members` is the set a command works on when it selects nothing and runs in the
workspace root: the listed members. Without a `default-members` entry it is the root package when the root has one,
and every member otherwise. A command run in a member's own directory selects that member. Tests:
`workspace.rs` (`default_members_follow_the_rules`), `tests/check_workspace.py` (the default build, test, and run).

## Selection

**WS6. The selection flags.** `-p <name>` (repeatable), `--workspace`, and `--exclude <name>` (repeatable). `-p` and
`--workspace` cannot be combined, and `--exclude` needs `--workspace`; a combination that breaks these rules, a name that
is not a member (the message lists the members), and an `--exclude` name that is not a member are usage errors (exit
status 2). Inside a workspace the flags select members whatever the target is: the target only locates the workspace.
`--workspace` selects every member except the excluded ones; `-p` selects the named members. Without a flag the rule of
WS5 applies. The selected members keep the workspace's order (the order `members` lists them, with the root package
first), and a name given twice is selected once. Tests: `driver workspace.rs` (`selection_follows_the_flags`),
`tests/check_workspace.py` (every usage error).

**WS7. Outside a workspace.** A standalone package takes `--workspace` (the one package) and `-p <its own name>`; another
name is a usage error that names the package. `--exclude` outside a workspace is a usage error. `burdin test` and
`burdin bench` keep their graph selection outside a workspace (`spec/test-runner.md`): `-p` names any package of the
build. Tests: `tests/check_workspace.py` (a standalone package), `tests/check_test_runner.py` (the existing rules).

## Commands

**WS8. Which commands select.** `build`, `run`, `test`, `bench`, `doc`, and `fmt` take the selection flags. A command that
takes one package (`check`, `lint`, `fix`, `ssr`, `refactor`, `expand`, `asm`, `bloat`, `debug`, `bindgen`, `watch`) takes a
member's directory or a package's directory, and rejects a virtual root with an error that points at its members and
the flags that select them (exit status 1), because a virtual root has no package to check.
Tests: `tests/check_workspace.py` (the virtual root under `check`).

**WS9. `burdin build`.** Builds one executable per selected package that has a `src/main.bd`. A selected package without
one is skipped with `note: skipped <name>: it has no src/main.bd` when the default or `--workspace` selected it, and is an
error (exit 1) when `-p` named it. If nothing selected has an entry point, the command fails (exit 1) and says so. The
output options that name one file (`-o`, `--emit-ir`, `--emit-deps`) need exactly one package to build, and are a usage
error otherwise. Each executable is written under its own package's `target/burdin/<profile>/bin/`
(`spec/target.md`, TG1). The profile is resolved from the target's manifest (`spec/profiles.md`): a workspace root
declares the profiles its members build with, including `[profile.<name>.package.<member>]` overrides.
Tests: `tests/check_workspace.py` (the skip note, the usage errors, the executables' places).

**WS10. `burdin run`.** Runs one program: the selection must name exactly one package, which must have a `src/main.bd`
(usage error exit 2 when more than one is selected, and the message lists them). The program's exit status is the
command's, as before.
Tests: `tests/check_workspace.py`.

**WS11. `burdin test`, `burdin bench`.** Run the selected members together, with one report, as `spec/test-runner.md`
and `spec/bench.md` describe for a set of packages. A member that is a workspace root is an error (the selection never
contains a nested root). Shared outputs of the run (the `--failed` record and the benchmark baselines) live under the
workspace root's `target/`; a standalone package keeps them under its own.
Tests: `tests/check_workspace.py` (the test report, the `--failed` record location).

**WS12. `burdin doc`.** Writes one site per selected package to that package's `target/doc` (the default when `-o` is
absent), with the usual messages for each. `-o` names one place and needs exactly one selected package. `--coverage`
prints one report per selected package.
Tests: `tests/check_workspace.py` (one site per member).

**WS13. `burdin fmt`.** `-p`, `--workspace`, and `--exclude` select the members whose directories `fmt` formats, found
from the current directory: the workspace the current directory belongs to (WS2). Positional paths and selection flags
cannot be combined (usage error). Without any of them, `fmt` formats the paths as before (`.` by default), so from a
workspace root that is every member's `.bd` files and anything else under it.
Tests: `tests/check_workspace.py` (`fmt -p`, `fmt --workspace --check`).

## The lockfile

**WS14. One lockfile per workspace.** `burdin.lock` sits beside the workspace root's `burdin.toml`, and records the whole
workspace graph: every member and every package they reach, whatever the command selects. A command in a member's
directory reads and writes that same file, so `--locked` and the pins do not depend on the selection. A standalone package
keeps its lockfile beside its manifest (PKG5-PKG6). The rules of `spec/packages.md` (pinning, the checksum, `--locked`,
`burdin add`) apply to the whole graph unchanged. `burdin add` in a member edits that member's manifest and the workspace
lockfile. Tests: `tests/check_workspace.py` (the lockfile at the root, `--locked` after a change), `resolve.rs` and
`crates/burdin_pkg/tests/git.rs` (`a_workspace_locks_every_member`).

## Inheritance

**WS15. Package fields (`[workspace.package]`).** A workspace root may set `version`, `edition`, `license`, and `authors`
in `[workspace.package]`. A member's `[package]` takes one of them with `key.workspace = true`, or with
`key = { workspace = true }`. Only these four keys can be inherited; another key with `.workspace` is an error. A key
set both with a value and with `workspace = true` is an error that names both lines, and `workspace = false` is an error
(the value is `true` or absent). Taking a key the workspace does not set is an error that names `[workspace.package]`.
The inherited value means exactly what the same line in the member would mean; an inherited `edition` is checked like
any edition. `name` is never inherited. `burdin fix --migrate` moves a package's own `edition`, so it refuses a package
whose edition is inherited, and says so (exit 1).
Tests: `package.rs` (`package_fields_are_inherited`, `conflicting_and_missing_inheritance_is_rejected`), `tests/check_workspace.py` (the inherited version in the golden lockfile and the failure messages).

**WS16. Dependencies (`[workspace.dependencies]`).** A workspace root declares dependencies with the forms of
`[dependencies]` (a path, a git repository, or `true`), in `[workspace.dependencies]`. A path there is relative to the
workspace root. A member writes `name.workspace = true` (or `name = { workspace = true }`) in `[dependencies]`, and takes
the source the root declares under that name, resolved from the member's directory. Inheriting a name the workspace does
not declare is an error. `workspace = true` cannot be combined with another key of the same table (`path`, `git`,
`branch`, `tag`, `rev`, `version`). A workspace dependency may not inherit from the workspace. Inheritance applies only to
`[dependencies]`; `[rust.crates]` and a script's manifest take paths as before.
Tests: `package.rs` (`dependencies_are_inherited_from_the_root`), `tests/check_workspace.py` (a member built through an inherited path).

## Members' own rules

**WS17. A member.** A member is a package: its directory has a `burdin.toml` with a `[package]` section, and no
`[workspace]` section (WS11). A member's directory is inside the root, so its path has no `..` and is not absolute
(WS3). A member is an ordinary package in every other respect: its `src/`, its `[lint]`, its `[build] allow`, its
`[rust]`, and its `edition` apply to it alone. Members may depend on one another, by path or by inheritance, as on any
other package; a name identifies one package in the graph (PKG1). Tests: `workspace.rs` (`members_must_be_packages`),
`tests/check_workspace.py`.

**WS18. The language server.** A folder opened in the editor is one package per member: a `burdin.toml` with a
`[package]` section is a package, and a virtual root is not one. Each file opens in the database of the package it
belongs to (`spec/lsp.md`). Tests: `crates/burdin_lsp` (the folder discovery), `tests/check_workspace.py`.

## Limits

- `--offline` does not exist; a path-only workspace needs no network already, and `--offline` is a separate item of
  `spec/packages.md`.
- Multi-line arrays in a manifest are not supported (every value is on one line, as before), so a long `members` list
  is one line.
- A glob matches one path component at a time (`libs/*`), not `**`.
- `burdin fix --migrate` cannot move an inherited edition (WS15); the edition is set on the root, and the migration of the
  workspace is not implemented.
- Build outputs stay under each package's `target/burdin/` (`spec/target.md`, TG1), not under one workspace target
  directory; each member has its own artifact cache.
