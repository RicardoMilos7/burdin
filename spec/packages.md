# Packages: dependencies, `burdin.lock`, and `burdin add`

A package's `burdin.toml` names its dependencies; a dependency comes from a directory (`path`), from a git
repository (`git`), or from the official packages that ship with the compiler (`name = true`), and `burdin.lock`
records the exact packages a build used, so the same checkout builds the same program tomorrow and on another
machine. `burdin add` edits the manifest and the lockfile together (stage 3; issues
#238 and #239). Features, dev-dependencies, and platform-specific dependencies (issues #340 and #341) select which of
those dependencies a build loads (PKG19-PKG33). This file is the exact behavior, with a rule ID for each point and the
tests that cover it.

The pieces live where the behavior belongs: the manifest parser and the lockfile format in the package loader
(`crates/burdin_query/src/package.rs`, `crates/burdin_query/src/lockfile.rs`), which reads a git dependency from the
checkout the lockfile pins and never fetches; resolution, the git cache, the checksum, and `burdin add` in the new
crate `burdin_pkg` (`git.rs`, `resolve.rs`, `add.rs`, `sha256.rs`); and the command line in the driver
(`crates/burdin_driver/src/add.rs`, and `sync_dependencies` in `main.rs`). The black-box check is
`tests/check_packages.py`, run by every CI job with a built driver; `crates/burdin_pkg/tests/git.rs` and the unit
tests of both crates run under `cargo test --workspace`.

## The decisions

- **Three sources now, a registry later.** The design document says dependencies come "from a path or a git URL; a
  central registry is a later possibility". A `version` with no `path` or `git` is an error that says there is no
  registry yet (PKG2, PKG11), so the syntax is free for one. The third source is the official packages (PKG15): the
  formats and other libraries that live in this repository's `packages/` folder and ship with the compiler, so
  their version is the compiler's version and `json = true` needs no registry and no network.
- **Git is run, not linked.** Resolution runs the `git` executable, as cargo can. No new Rust dependency, and every
  URL form and credential helper git understands works.
- **The lockfile pins commits and hashes files.** A commit id already names a tree, but a checkout in the cache is
  just files on disk; the SHA-256 checksum proves they are still the files that commit had (PKG7). A path package
  is part of the same checkout as the entry package, so it is not hashed: hashing it would make every edit a stale
  lockfile. An official package is hashed too, because its files belong to the compiler, not to the checkout: a
  different compiler brings different files, and the lockfile says so (PKG17).
- **A path-only graph needs no lockfile.** There is nothing to pin, so a build does not create one, which keeps the
  example and test packages free of generated files. A graph with a git or an official package has something to
  pin, so a build writes one. Once a lockfile exists (or `burdin add` writes one) it is kept current (PKG6).
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

**PKG5. The format.** The lockfile sits beside the entry package's `burdin.toml`, or, for a member of a
manifest-level workspace, beside the *workspace root's* manifest: one lockfile records the whole workspace graph
(`spec/workspaces.md`, WS14). It is a TOML file that burdin
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
(none for the default branch), the full `commit`, and the `checksum`. An official package records
`source = "official"` and the `checksum` (PKG17). Rendering is deterministic, so two lockfiles
of one graph are the same text. Tests: `crates/burdin_query/src/lockfile.rs` (`render_is_the_documented_text`,
`parse_reads_what_render_writes`), `tests/check_packages.py` (golden lockfiles).

**PKG6. When the lockfile is written.** Before a package directory is loaded, `build`, `run`, `test`, `bench`,
`check`, and every other command that compiles a package resolve its graph. The lockfile is written when it exists
and no longer matches, or when there is none and the graph has a git or an official package. A graph of path
packages with no lockfile gets none; `burdin add` always writes one. A one-file target, a script, and the language server never write
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
changed source (path, git with its URL and reference, or official), a different commit, an official package whose
files are not the ones the lockfile pinned (PKG17), and changed dependencies. A git dependency the
lockfile does not record fails before anything is fetched. Without a lockfile, `--locked` passes for a path-only
graph (there is nothing to pin) and fails for one with a git or an official package. Tests: `tests/check_packages.py`, `resolve.rs`
(`an_existing_lockfile_is_kept_current_and_locked_reports_why`, `differences_name_each_change`).

**PKG9. The loader reads the lockfile.** The package loader finds a git dependency in the checkout of the commit the
entry package's lockfile records for its repository and reference, everywhere in the graph. A git dependency the
lockfile does not record, or whose checkout is missing, is a load error that says to run `burdin build`; the loader
itself never runs git. Only an active dependency is loaded (PKG30), so only an active one needs its checkout. Tests: `crates/burdin_pkg/tests/git.rs`.

**PKG10. A lockfile from elsewhere.** A malformed lockfile is an error naming its line; a lockfile with another
format `version` is refused (`lockfile format version 2 is not supported`) and never overwritten. Keys the compiler
does not know are ignored. Tests: `lockfile.rs` (`malformed_lockfiles_are_rejected_with_a_line`,
`unknown_keys_are_ignored`), `tests/check_packages.py`.

## `burdin add`

```text
burdin add [<name>] --path <dir> [--manifest-path <path>]
burdin add [<name>] --git <url> [--branch <b> | --tag <t> | --rev <commit>] [--manifest-path <path>]
burdin add <name> [--manifest-path <path>]
```

**PKG11. The command line.** Exactly one of `--path` and `--git`, or, with neither, a `<name>` that is an official
package (PKG15); at most one of `--branch`, `--tag`, and `--rev`, and only with `--git`. `<name>` is the `use` name
and defaults to the dependency's package name; it must be an identifier that is not a reserved word and not `std`.
A bare name that is not an official package (`burdin add json` when there is no official `json`, `json@1.0`) is a
usage error that says there is no package registry yet, names the official packages (or says there are none), and
asks for `--path` or `--git`. `--manifest-path` names the `burdin.toml` to edit or its directory;
without it, the nearest `burdin.toml` at or above the working directory. Usage errors exit 2 and `--help` exits 0.
Tests: `tests/check_packages.py`, `crates/burdin_driver/src/add.rs` unit tests.

**PKG12. The manifest edit.** `--path` is relative to the working directory and is written relative to the
manifest, with `/` separators: `greet = { path = "../greet" }`. `--git` writes the URL and the reference as given:
`colors = { git = "<url>", tag = "v1" }`. An official package writes `json = true`. The line goes after the last entry of `[dependencies]`, or the section is
appended to the end of the file; a dependency of the same name has its line replaced in place. Every other line,
comment, and blank line stays, and a file with `\r\n` line endings keeps them. Tests: `add.rs` unit tests,
`tests/check_packages.py` (golden manifests).

**PKG13. Resolution and the report.** The dependency is resolved (a git reference to a commit, which is checked
out; an official package to the compiler's directory for it), it must be a package (a `burdin.toml` at its root), and the whole graph is then resolved against the new
manifest, keeping every commit the lockfile already pins except the added dependency's, which builds the commit
just resolved (so adding a branch dependency again moves it to the branch's head). Then `burdin.toml` and `burdin.lock` are written and one
line is printed: `Added dependency `greet` (path ../greet)`, `Added dependency `colors` (git <url>, tag v1,
commit 4f6c0a7e4f1f)`, or `Added dependency `json` (official)`; `Replaced` when the name was already a dependency. Tests: `tests/check_packages.py`.

**PKG14. A failed `add` changes nothing.** A missing path, a directory that is not a package, a reference the
repository does not have, an invalid name, a package that would depend on itself, and any error resolving the graph
exit 1 and leave `burdin.toml` and `burdin.lock` as they were. Tests: `tests/check_packages.py`.

## Official packages

The packages in this repository's `packages/` folder (formats such as `json` and `toml`, and later libraries) are
official: each is an ordinary Burdin package with its own `burdin.toml` and `src/`, and they ship with the compiler
(the release archive and the installers put the folder beside the executable, PKG16). Their version is the
compiler's version.

**PKG15. The official source.** `name = true` in `[dependencies]` (of any package, official or not) is the official
package `name`; `name` is also how `use` reaches it. `name = false` is an error (`json = false` means nothing:
remove the line). `name = true` for a name that is not an official package is an error that names the official
packages the compiler has (or says it has none) and suggests `path` or `git`:

```text
error: burdin.toml: dependency `jsn = true`: it is not an official package; the official packages are: json, toml;
for any other package give `{ path = "..." }` or `{ git = "..." }`
```

The same official package may be written as a table that takes the keys of PKG25: `name = { official = true, default-features = false, features = ["a"] }`
(`official = false` is an error: remove the line, or give `path` or `git`). A table with `official = true` and a `path` or `git` key is an error.
`official = true` alone is `name = true`. This is how a package turns an official dependency's default features off, as `http` does for `tls` (`spec/http-client.md`, HC17).

A script's inline manifest (`spec/scripts.md`, SC3) and `[rust.crates]` take only paths, so `true` is an error
there. A version string alone stays reserved for a future registry (PKG2). Package names are unique in a graph
(PKG1), so an official package and a path or git package of the same name cannot be used together. An official package
may depend on another: `http` declares `tls = { official = true, default-features = false }` and turns the feature on by
default (`spec/http-client.md`, HC17), so a program that uses `http` has `tls`
in its graph, pinned in `burdin.lock` beside `http` (both with `source = "official"` and their own checksums), and links
`tls`'s native libraries (PKG34); the program names `tls` itself only to `use` its types. Tests: the loader's unit tests
(`official_dependency_forms`), `tests/check_packages.py`, `tests/check_http.py` (the lockfile of a program that depends
on `http` pins `tls` too).

**PKG16. Where the official packages are.** The first of these that is a directory is the root; each of its
subdirectories that has a `burdin.toml` is the official package of that name, and no later place is searched:

1. the directory `BURDIN_PACKAGES`, when the variable is set (and then only that one, so a mistyped value finds no
   packages instead of silently finding the installed ones);
2. `packages/` beside the directory of the `burdin` executable (`<exe dir>/../packages`: the installed layout is
   `bin/burdin` and `packages/`);
3. `packages/` in the directory of the executable (`<exe dir>/packages`: an unpacked release archive);
4. in a debug build of the compiler only, the `packages/` folder of the repository the compiler was built from
   (a build from a checkout needs no setup; a release build embeds no build path, so releases stay reproducible).

The loader and the resolver look the same way, so what resolves is what loads. An official dependency when there is
no root is an error that lists the places looked in. Tests: the loader's unit tests
(`official_roots_are_searched_in_order`), `tests/check_packages.py` (which sets `BURDIN_PACKAGES` to a fixture
folder, so it does not depend on the real packages), `tests/check_release.py` (the archive and the installers).

**PKG17. Pinning an official package.** An official package is a package of the lockfile like any other, with
`source = "official"` instead of a `git` origin, and a `checksum`: the SHA-256 of its files computed as PKG7 does,
from the compiler's directory for it. A graph with an official package has something to pin, so a build writes the
lockfile when there is none (PKG6). The files are not immutable the way a checkout is (a newer compiler brings
newer packages), so a checksum that differs is not an error in itself: it is a difference like a changed version.
A build without `--locked` rewrites the lockfile; `--locked` fails and says that the official package is not the
one `burdin.lock` pinned. Tests: `tests/check_packages.py` (golden lockfile and `--locked`), `lockfile.rs`
(`render_is_the_documented_text`), `resolve.rs` (`an_official_package_is_pinned_by_its_files`).

```toml
[[package]]
name = "json"
version = "0.1.0"
source = "official"
checksum = "sha256:3d5f1c..."
dependencies = []
```

**PKG18. The missing-dependency help.** A `use` of a name that resolves to nothing, when the name is an official
package that the importing package does not declare, keeps its error (E0002, `unknown module `json``) and gains a
help line: `` `json` is an official package: add `json = true` to [dependencies] or run `burdin add json` ``. A
one-file target and a script have no manifest to add it to, so they get no such help. Tests:
`tests/check_packages.py`, the checker's unit tests (`unknown_module_names_an_official_package`).

## Cleaning, packaging, vendoring, and offline builds

The package tools are `burdin clean`, `burdin package`, `burdin vendor`, and the `--offline` flag. They live in the
driver (`clean.rs`, `pack.rs`, `vendor.rs`) over the new modules of `burdin_pkg` (`archive.rs` for the archive and its
file selection, `vendor.rs` for the copies, and `git::restore` for offline checkouts). The black-box check is
`tests/check_package_tools.py`; `crates/burdin_pkg/tests/vendor.rs` and the unit tests of `archive.rs` run under
`cargo test --workspace`.

**PKG19. `burdin clean`.** `burdin clean [<dir>] [--cache]` removes the `target` directory of the package in `<dir>`
(default: the working directory), which holds everything a build writes: executables, generated modules and build
script output, and the test, bench, and documentation output. `<dir>` must hold a `burdin.toml`; otherwise the command
exits 1 and says so. It prints `Removed <path>`, or `Nothing to remove: <path> does not exist`. With `--cache` it also
removes `cache/` (compiled single-file scripts, `spec/scripts.md` SC6) and `git/` (the git cache, PKG3) under the
Burdin home, and prints each path it removed. It never removes `vendor/` (PKG23): that copy is what an offline build
needs. Cleaning a cache costs only time: the next build fetches or recompiles what it needs. Tests:
`tests/check_package_tools.py`.

**PKG20. `burdin package`.** `burdin package [<dir>]` writes an archive of the package to
`target/package/<name>-<version>.tar` (a USTAR tar, uncompressed; an existing archive is replaced) and prints
`Packaged <n> files into <path> (sha256:<hex>)`, the SHA-256 of the archive (`file` when `<n>` is 1). The package needs `[package] version`:
without one the command exits 1 and says so. The archive holds regular files only; a symbolic link among the selected
files is an error that names it. Tests: `tests/check_package_tools.py`, `archive.rs` (`tar_header_layout`).

**PKG21. Which files are packaged.** Without `[package] include`, every file under the package directory is selected
except the top-level `.git`, `target`, and `vendor` directories. With `include = [...]`, only the files matching one of
its patterns are selected, and the top-level directories above are then not excluded by default (a pattern names
them). `exclude = [...]` removes the files matching one of its patterns from the selection. `burdin.toml` is always
selected, and `target/package/` never is. A pattern is a `/`-separated path relative to the package: `*` matches any
run of characters within one path segment, `?` one character, and `**` as a whole segment matches zero or more
segments. A pattern matches a file when it matches the file's path or the path of a directory containing it, so
`docs` selects `docs/a.md`. Both keys are one-line arrays of strings (`include = ["src/**", "README.md"]`); an empty
pattern, a pattern that starts with `/`, or a value that is not an array of strings is an error naming the line. Tests:
`archive.rs` (`patterns_match_segments_and_directories`, `selection_defaults_and_overrides`),
`tests/check_package_tools.py`.

**PKG22. Reproducible archives.** The same tree gives the same bytes on every host and at every time. Entries are
sorted by their path's bytes; directories have no entries of their own; every header has mode `0644`, uid and gid `0`,
empty user and group names, modification time `0`, no device numbers, and no PAX extension. A path of up to 100 bytes
is the name; a longer one is split at a `/` into a prefix of up to 155 bytes and a name of up to 100 bytes, and a path
that fits neither is an error naming the file. The archive ends with two zero blocks. Tests: `archive.rs`
(`tar_header_layout`, `identical_trees_give_identical_bytes`), `tests/check_package_tools.py` (repacking after the
modification times change gives the same checksum).

**PKG23. `burdin vendor`.** `burdin vendor [<dir>]` resolves the package's graph as a build does (writing
`burdin.lock` when it is missing or out of date), then copies every git package of the lockfile in full into
`<dir>/vendor/<name>/`: the files PKG7 hashes, that is everything except the top-level `.git` and `target`. Each copy's
checksum must equal the lockfile's before the command succeeds. Official packages are not copied, because they ship
with the compiler, and path packages are read where they are, as always. The directory `vendor/` is burdin's: the
file `vendor/burdin-vendor.txt` lists the names it wrote (a header comment, then one name per line). A run removes the
directories that file lists and writes the current set. A `vendor/<name>` directory that the file does not list is an
error (exit 1) that says to move it aside: burdin never overwrites or removes what it did not write. A graph with no
git package prints `No git dependencies to vendor` and removes what an earlier run wrote. The success line is
`Vendored <n> packages into <path>` (`package` when `<n>` is 1). Like a build, vendoring fetches what the git cache lacks. Tests:
`crates/burdin_pkg/tests/vendor.rs`, `tests/check_package_tools.py`.

**PKG24. `--offline`.** `burdin build`, `run`, `test`, `bench`, and `check` take `--offline`: the build contacts no
network. A git package the lockfile pins is read from the git cache when its checkout is there, as always; when it is
not, it is restored into the cache from `vendor/<name>` (PKG23), and the restored files are checked against the
lockfile's checksum like any checkout (PKG7). When neither has the checkout, the command exits 1 and names both places
and says to run `burdin vendor` with network access. A git dependency the lockfile does not record is an error under
`--offline`, because a reference cannot be resolved without the network. `--offline` combines with `--locked`.
Tests: `crates/burdin_pkg/tests/vendor.rs`, `tests/check_package_tools.py` (the build succeeds after the cache and the
source repository are deleted, and fails without a vendored copy).

## Dependency tables, features, and targets

A build selects a part of the graph the manifests describe: the dependencies its features enable, the dev-dependencies
of a test build, and the dependencies of its own platform. The lockfile does not change with that selection (PKG33).

**PKG25. The dependency tables.** A manifest has three kinds of dependency table: `[dependencies]` (every build),
`[dev-dependencies]` (the entry package's test, bench, and fuzz builds, PKG26), and `[target.'cfg(<predicate>)'.dependencies]`
or `[target.'cfg(<predicate>)'.dev-dependencies]` (the builds on the platforms the predicate holds for, PKG27). An entry
takes the source of PKG1-PKG2 and PKG15, and the optional keys `optional = true|false` (default `false`),
`default-features = true|false` (default `true`), and `features = ["a", "b"]` (default none). Other keys are ignored.
A name may be declared once per manifest, in any table: a second declaration is an error naming its line. `optional`
on a dev-dependency is an error, because a feature cannot enable a dev-dependency. A `features` list or a
`default-features` key is checked when the dependency is active (PKG30). Tests: the loader's unit tests
(`dependency_tables_are_parsed`), `tests/check_features.py`.

**PKG26. Dev-dependencies.** The entry package's dev-dependencies are loaded by the builds that compile its tests:
`burdin test`, `burdin bench`, `burdin fuzz`, and `burdin check --tests`. In those builds they are visible to every module
of the entry package, as its normal dependencies are: a test names one by its package name (`testkit.answer()`), because
test items are stripped from a normal build while a module-level `use` would be read by every build. In every other
build of the entry package (`build`, `run`, and `check` without `--tests`) their sources are not loaded: not read, not
compiled, not checked, and not linked, so a dev-dependency whose source has a syntax error does not stop `burdin
build`. Their manifests are still resolved for the lockfile (PKG33), so a missing path, an unreachable git repository,
or a manifest that does not parse is an error in every build. A dependency's own dev-dependencies are never loaded, whatever the command: they are not part of the graph
a dependency contributes. Tests: `tests/check_features.py`.

**PKG27. Target-specific dependencies.** A `[target.'cfg(P)'...]` table applies on the host the compiler runs on, which is
the target it builds for (`std/src/target.bd`). The predicate `P` is one of `unix` (Linux and macOS), `windows`,
`target_os = "<os>"` (`linux`, `macos`, `windows`), `target_arch = "<arch>"` (`x86_64`, `aarch64`),
`target_family = "unix"|"windows"`, `all(P, ...)`, `any(P, ...)`, or `not(P)`. A value that names no operating system or
architecture the compiler knows is a valid predicate that is false. Other tables (a bare target triple, for example) are
errors, and so is a predicate name that is none of these, or a malformed one; each is reported with its manifest line.
A dependency whose predicate does not hold is not loaded, and its features and optional-ness do not matter (PKG30).
Tests: the loader's unit tests (`cfg::tests::the_predicates_match_their_platforms`,
`cfg::tests::combinators_compose`, `target_and_dev_dependencies_are_not_read_by_a_build`), `tests/check_features.py`.

**PKG28. Declaring features.** The `[features]` table names the package's features: `name = ["item", ...]` lists what
enabling the feature turns on. A feature name is an identifier that is not a keyword, because it names a constant of the
`features` module (PKG32). An item is one of:

- the name of a feature of this package;
- `"dep:x"`, which enables the optional dependency `x`;
- `"x/feature"`, which enables `feature` in the dependency `x`, and enables `x` as well when `x` is optional;
- the name of an optional dependency, which enables it: every optional dependency is an implicit feature of its name.

`default` is the feature enabled by default (PKG29); it is an ordinary feature in every other respect, and a package
without it has no default features. A feature may enable features in a cycle. When the manifest is read, an item that is
none of these is an error naming its line (`feature `fast` enables `ok`, which is not a feature or an optional
dependency`); so is `dep:x` for a dependency that is not optional, `x/feature` for a name that is not a dependency, and
a dev-dependency named by any item. A feature named like an optional dependency is an error, because the two share one
constant. Tests: the loader's unit tests (`feature_tables_are_parsed_and_checked`), `tests/check_features.py`.

**PKG29. Selecting features.** The features a build enables for the entry package are: `default`, when the package
declares it and `--no-default-features` is not given; the features `--features` names; and, with `--all-features`,
every declared feature and every optional dependency. `--features` takes a comma- or space-separated list and may repeat;
each item is as in PKG28, and an item naming a feature the entry package does not declare is an error (`package `app` has
no feature `x``). With `--all-features`, `--no-default-features` changes nothing. Tests: `tests/check_features.py`.

**PKG30. Active dependencies and unification.** A dependency is active in a build when its table applies to the build
(PKG26), its predicate holds (PKG27), and it is not optional or an enabled item names it (PKG28). An active dependency
enables its `features` list, and `default` unless `default-features = false`. Features unify across the graph: a package
has one feature set, the union of every request from the entry package and from every active dependent, and an enabled
feature stays enabled. Enabling a feature can activate an optional dependency and request features of another, so the
set is a fixed point: it is computed until no package gains a feature or a dependency. A request for a feature the
dependency does not declare is an error, naming the dependency and the feature, when the dependency is active; a request
for an inactive dependency is ignored. Dependency cycles in the active graph are errors (PKG1). Tests: the loader's unit
tests (`features_unify_across_the_graph`; `target_and_dev_dependencies_are_not_read_by_a_build` for an inactive
request), `tests/check_features.py`.

**PKG31. The command line.** `build`, `run`, `check`, `test`, `bench`, and `fuzz` take `--features <list>`,
`--no-default-features`, and `--all-features` (PKG29). A one-file target (a single `.bd` file or a script) has no features:
these flags on one are an error with exit status 1. `burdin watch` passes them to each run it starts. Tests:
`tests/check_features.py` (the flags and the refusal), `tests/check_watch.py` (the pass-through).

**PKG32. The `features` module.** A package whose manifest has a `[features]` table, or an optional dependency, has a
generated module `features`. It is written in memory and holds one constant per declared feature and per optional
dependency, sorted by name, each true exactly when the build enables it:

```text
const default: bool = true
const fast: bool = false
const json: bool = true
```

A module's declarations are importable unless marked `priv`, and `pub` is written only before `use` (`spec/grammar.md`),
so the constants carry no visibility word. Code reads one as `use features.fast` (or `use features` and then
`features.fast`), and in `comptime if` (`spec/comptime.md`, CT5). The module is an ordinary module of its package: it is
checked and documented with it. A source module named `features` in such a package is an error, because the name is
taken: rename the module. A package with neither a `[features]` table nor an optional dependency has no such module,
and `use features` in it is an unknown module. Tests: the loader's unit tests
(`features::tests::the_module_lists_constants_sorted_and_once`), `tests/check_features.py`.

**PKG33. The lockfile pins every package a build could use.** `burdin.lock` pins the potential graph, not the active one:
every dependency of every package the graph reaches, optional ones and every target's included, plus the entry package's
dev-dependencies and what they reach, but never a dependency's dev-dependencies (PKG26). Features, the platform, and the
command never change the lockfile, so one lockfile serves every platform and every feature set. Every build resolves the
potential graph, so a git dependency that is not active in a build is still fetched and checksummed (PKG8 applies to it)
and needs the network the first time, but it is not loaded. A package's `dependencies` field lists the names the walk
reaches from it: each of its normal, optional, and target dependencies, and, for the entry package only, its
dev-dependencies. A dependency's own dev-dependencies appear nowhere in the lockfile. Tests: `tests/check_features.py`
(the golden lockfile), `resolve.rs` (`the_lockfile_pins_the_potential_graph`).
## Native libraries

**PKG34. The `[link]` table.** A package that calls a C library (through `extern "C"`, `spec/stdlib.md`) names it in its
manifest, so a program that depends on the package links it without a `--link-lib` of its own:

```toml
[link]
libs = ["z"]

[target.'cfg(unix)'.link]
libs = ["ssl", "crypto"]

[target.'cfg(windows)'.link]
libs = ["libssl", "libcrypto"]
```

`[link]` has one key, `libs`, an array of strings; any other key is an error that names it and its line. A name is a
library as `--link-lib` takes it: `ssl` becomes `-lssl` on Unix and `ssl.lib` with MSVC, and a value with a path
separator or a library or object file extension names that file as it is. A name that is empty, starts with `-`, or
contains a space is an error (`write ssl, not -lssl or a command line`). `[target.'cfg(P)'.link]` takes the
predicates of PKG27 (a bad predicate is an error with its line) and applies on the platform the compiler builds for.
The tables are **additive**: a package's libraries on a platform are `[link]`'s followed by the matching target tables',
in file order, without duplicates; a target table does not replace `[link]`. There is no `search` key: a library in an
unusual place is found through the linker's variables (`LIBRARY_PATH`, `LIB`) or `BURDIN_CC` flags.

A build links the libraries of **every package in its graph** (the entry package, dependencies, and what they reach, as
loaded by PKG25-PKG27 and PKG30, so an inactive optional or target dependency contributes nothing). They are merged,
without duplicates, into the list `--link-lib` makes, after the command line's and in dependency order, and the program
links them in `burdin build`, `run`, `test`, `bench`, and `fuzz`. They are part of the build's cache fingerprint, so
changing a list relinks. `burdin check` and the other commands that do not link ignore the table. A package with no
`[link]` table is unchanged. A missing library is the linker's error, which names it. Tests: the loader's unit tests
(`link_tables_select_libraries_by_host`), `tests/check_link.py` (a dependency's libraries reach the link and fail it by
name, a table for another platform does not, and the manifest errors), `tests/s3_tls/link` (the `tls` package builds and
runs against the system OpenSSL with no `--link-lib`).

## Not yet

- Official packages in a script's inline manifest, a package registry, version requirements, and `burdin update` (re-resolving one dependency without deleting the
  lockfile).
- `burdin add --dev`, `--optional`, and `--target`: the manifest edit for the tables of PKG25 is by hand for now.
- Weak features (`dep?/feature`), and a resolver that does not unify features across the graph. Moving a branch dependency forward today means `burdin add` with the same branch again, or removing
  the lockfile (which re-resolves every git dependency).
- A git dependency whose package is in a subdirectory of the repository.
- Publishing a package (`burdin publish`, a registry), compressed archives, and `--offline` for `burdin add`.
  Vendoring copies no path packages.
- Doc tests build each example as its own package, so an example of a package with git dependencies resolves them
  for that example instead of reading the documented package's lockfile.
