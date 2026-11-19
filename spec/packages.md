# Packages: dependencies, `burdin.lock`, and `burdin add`

A package's `burdin.toml` names its dependencies; a dependency comes from a directory (`path`) or from a git
repository (`git`), and `burdin.lock` records the exact packages a build used, so the same checkout builds the same
program tomorrow and on another machine. `burdin add` edits the manifest and the lockfile together (stage 3; issues
#238 and #239). This file is the exact behavior, with a rule ID for each point and the tests that cover it.

The pieces live where the behavior belongs: the manifest parser and the lockfile format in the package loader
(`crates/burdin_query/src/package.rs`, `crates/burdin_query/src/lockfile.rs`), which reads a git dependency from the
checkout the lockfile pins and never fetches; resolution, the git cache, the checksum, and `burdin add` in the new
crate `burdin_pkg` (`git.rs`, `resolve.rs`, `add.rs`, `sha256.rs`); and the command line in the driver
(`crates/burdin_driver/src/add.rs`, and `sync_dependencies` in `main.rs`). The black-box check is
`tests/check_packages.py`, run by every CI job with a built driver; `crates/burdin_pkg/tests/git.rs` and the unit
tests of both crates run under `cargo test --workspace`.

## The decisions

- **Two sources now, a registry later.** The design document says dependencies come "from a path or a git URL; a
  central registry is a later possibility". A `version` with no `path` or `git` is an error that says there is no
  registry yet (PKG2, PKG11), so the syntax is free for one.
- **Git is run, not linked.** Resolution runs the `git` executable, as cargo can. No new Rust dependency, and every
  URL form and credential helper git understands works.
- **The lockfile pins commits and hashes files.** A commit id already names a tree, but a checkout in the cache is
  just files on disk; the SHA-256 checksum proves they are still the files that commit had (PKG7). A path package
  is part of the same checkout as the entry package, so it is not hashed: hashing it would make every edit a stale
  lockfile.
- **A path-only graph needs no lockfile.** There is nothing to pin, so a build does not create one, which keeps the
  example and test packages free of generated files. Once a lockfile exists (or `burdin add` writes one) it is kept
  current (PKG6).
- **Every build keeps the lockfile current; `--locked` turns that into a check.** Like cargo: a build rewrites a
  lockfile that no longer matches the manifests, and `--locked` (for CI) fails instead and says why (PKG8).

## Manifest dependencies

**PKG1. A path dependency.** `name = { path = "../lib" }`, or the shorthand `name = "../lib"`: the package in that
directory, relative to the manifest that names it. `name` is how `use` reaches it; the package's own name comes from
its `[package] name`, and package names are unique in a graph. Tests: `tests/check_packages.py`, the loader's unit
tests (`dependency_forms`).

**PKG2. A git dependency.** `name = { git = "<url>" }` builds the repository's default branch; at most one of
`branch = "<b>"`, `tag = "<t>"`, and `rev = "<commit>"` picks another commit. The repository's root is the package.
`path` and `git` together, two references, a reference without `git`, an empty URL, and a table with neither source
are errors naming the manifest line; a table with only `version` says that there is no package registry yet. Keys
the compiler does not know are ignored, so a newer manifest loads. A script's inline manifest (`spec/scripts.md`,
SC3) and `[rust.crates]` take only paths. Tests: the loader's unit tests (`git_dependency_forms`,
`invalid_dependency_tables_are_rejected`, `scripts_and_rust_crates_take_only_paths`).

**PKG3. The git cache.** Git dependencies are kept under the Burdin home (`BURDIN_HOME`, otherwise the platform's
application-data directory; `spec/scripts.md` SC6): `git/db/<key>` is a bare clone of the repository, and
`git/checkouts/<key>/<commit>` holds one commit's files, without a `.git` directory. `<key>` is the repository's last
path segment and a hash of its URL. A clone or checkout is written beside its final name and renamed, so an
interrupted one is never mistaken for a finished one. Git runs with line-ending conversion off, so a checkout has the
committed bytes on every platform. Tests: `crates/burdin_pkg/tests/git.rs`, `tests/check_packages.py`.

**PKG4. Resolving a reference.** A reference that the lockfile does not pin resolves now: the bare clone is fetched
(every branch and tag), then the default branch is the clone's `HEAD`, a branch is `refs/heads/<b>`, a tag is
`refs/tags/<t>`, and a rev is any commit name git accepts (a rev the clone already has needs no fetch). A reference
the repository does not have is an error: `<url> has no tag nope`. Tests: `tests/check_packages.py`.

## `burdin.lock`

**PKG5. The format.** The lockfile sits beside the entry package's `burdin.toml`. It is a TOML file that burdin
writes and reads itself: a header comment, `version = 1`, and one `[[package]]` table per package of the graph, the
entry package included, sorted by name:

```toml
# burdin.lock: the exact packages this package builds with. It is generated by burdin;
# do not edit it by hand.
version = 1

[[package]]
name = "app"
version = "0.1.0"
dependencies = ["colors", "greet"]

[[package]]
name = "colors"
version = "1.0.0"
git = "https://example.com/colors.git"
tag = "v1"
commit = "4f6c0a7e4f1f1d3f0f2b8f9a5c1e7d2b3a4c5d6e"
checksum = "sha256:9b0c6f..."
dependencies = []

[[package]]
name = "greet"
version = "0.2.0"
dependencies = []
```

`version` is the package's `[package] version` and is left out when it has none; `dependencies` lists package names
(not `use` names). A git package also records its `git` URL, the `branch`, `tag`, or `rev` the manifest asked for
(none for the default branch), the full `commit`, and the `checksum`. Rendering is deterministic, so two lockfiles
of one graph are the same text. Tests: `crates/burdin_query/src/lockfile.rs` (`render_is_the_documented_text`,
`parse_reads_what_render_writes`), `tests/check_packages.py` (golden lockfiles).

**PKG6. When the lockfile is written.** Before a package directory is loaded, `build`, `run`, `test`, `bench`,
`check`, and every other command that compiles a package resolve its graph. The lockfile is written when it exists
and no longer matches, or when there is none and the graph has a git package. A graph of path packages with no
lockfile gets none; `burdin add` always writes one. A one-file target, a script, and the language server never write
a lockfile. Tests: `tests/check_packages.py`, `resolve.rs` (`a_path_graph_resolves_to_sorted_packages`).

**PKG7. Pinning and the checksum.** A git dependency whose repository and reference the lockfile records builds
that commit: it is checked out if the cache does not have it (fetching it if needed), and a moved branch changes
nothing until the lockfile no longer pins it. The checksum is `sha256:` and the hexadecimal SHA-256 of every file of
the checkout except the top-level `.git` and `target` directories (build-script output), in byte order of their
`/`-separated relative paths, each contributing its path, a NUL, its length in decimal, a NUL, and its bytes. A
checkout whose checksum differs from the lockfile's is an error, with or without `--locked`, that says to delete the
checkout so it is fetched again. Tests: `tests/check_packages.py`, `crates/burdin_pkg/tests/git.rs`, `resolve.rs`
(`checksums_cover_paths_and_contents_but_not_build_output`).

**PKG8. `--locked`.** `burdin build`, `run`, `test`, `bench`, and `check` take `--locked`: a lockfile that would
change is an error (exit status 1) that names every difference, and nothing is written:

```text
error: burdin.lock is out of date with burdin.toml, and --locked forbids updating it:
  - `greet` is version 0.2.0 but burdin.lock records 0.1.0
run the command without --locked to update burdin.lock
```

The differences are a package missing from the lockfile, a package no longer in the graph, a changed version, a
changed source (path or git, URL, or reference), a different commit, and changed dependencies. A git dependency the
lockfile does not record fails before anything is fetched. Without a lockfile, `--locked` passes for a path-only
graph (there is nothing to pin) and fails for one with a git package. Tests: `tests/check_packages.py`, `resolve.rs`
(`an_existing_lockfile_is_kept_current_and_locked_reports_why`, `differences_name_each_change`).

**PKG9. The loader reads the lockfile.** The package loader finds a git dependency in the checkout of the commit the
entry package's lockfile records for its repository and reference, everywhere in the graph. A git dependency the
lockfile does not record, or whose checkout is missing, is a load error that says to run `burdin build`; the loader
itself never runs git. Tests: `crates/burdin_pkg/tests/git.rs`.

**PKG10. A lockfile from elsewhere.** A malformed lockfile is an error naming its line; a lockfile with another
format `version` is refused (`lockfile format version 2 is not supported`) and never overwritten. Keys the compiler
does not know are ignored. Tests: `lockfile.rs` (`malformed_lockfiles_are_rejected_with_a_line`,
`unknown_keys_are_ignored`), `tests/check_packages.py`.

## `burdin add`

```text
burdin add [<name>] --path <dir> [--manifest-path <path>]
burdin add [<name>] --git <url> [--branch <b> | --tag <t> | --rev <commit>] [--manifest-path <path>]
```

**PKG11. The command line.** Exactly one of `--path` and `--git`; at most one of `--branch`, `--tag`, and `--rev`,
and only with `--git`. `<name>` is the `use` name and defaults to the dependency's package name; it must be an
identifier that is not a reserved word and not `std`. A bare name (`burdin add json`, `json@1.0`) is a usage error
that says there is no package registry yet. `--manifest-path` names the `burdin.toml` to edit or its directory;
without it, the nearest `burdin.toml` at or above the working directory. Usage errors exit 2 and `--help` exits 0.
Tests: `tests/check_packages.py`, `crates/burdin_driver/src/add.rs` unit tests.

**PKG12. The manifest edit.** `--path` is relative to the working directory and is written relative to the
manifest, with `/` separators: `greet = { path = "../greet" }`. `--git` writes the URL and the reference as given:
`colors = { git = "<url>", tag = "v1" }`. The line goes after the last entry of `[dependencies]`, or the section is
appended to the end of the file; a dependency of the same name has its line replaced in place. Every other line,
comment, and blank line stays, and a file with `\r\n` line endings keeps them. Tests: `add.rs` unit tests,
`tests/check_packages.py` (golden manifests).

**PKG13. Resolution and the report.** The dependency is resolved (a git reference to a commit, which is checked
out), it must be a package (a `burdin.toml` at its root), and the whole graph is then resolved against the new
manifest, keeping every commit the lockfile already pins except the added dependency's, which builds the commit
just resolved (so adding a branch dependency again moves it to the branch's head). Then `burdin.toml` and `burdin.lock` are written and one
line is printed: `Added dependency `greet` (path ../greet)`, or `Added dependency `colors` (git <url>, tag v1,
commit 4f6c0a7e4f1f)`; `Replaced` when the name was already a dependency. Tests: `tests/check_packages.py`.

**PKG14. A failed `add` changes nothing.** A missing path, a directory that is not a package, a reference the
repository does not have, an invalid name, a package that would depend on itself, and any error resolving the graph
exit 1 and leave `burdin.toml` and `burdin.lock` as they were. Tests: `tests/check_packages.py`.

## Not yet

- A package registry, version requirements, and `burdin update` (re-resolving one dependency without deleting the
  lockfile). Moving a branch dependency forward today means `burdin add` with the same branch again, or removing
  the lockfile (which re-resolves every git dependency).
- A git dependency whose package is in a subdirectory of the repository, and `--offline`.
- Doc tests build each example as its own package, so an example of a package with git dependencies resolves them
  for that example instead of reading the documented package's lockfile.
