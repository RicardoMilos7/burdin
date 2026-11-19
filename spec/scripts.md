# Single-file scripts

A `.bd` file can be a script: run it directly, declare its own dependencies in a leading doc block, and let the
driver cache the compiled binary by content (stage 3; issue #586, under #578). This file is the exact behavior,
with a rule ID for each point and the tests that cover it.

The pieces live where the behavior belongs: the shebang in the lexer (`crates/burdin_syntax/src/lexer.rs`), the
manifest fragment in the package loader (`crates/burdin_query/src/package.rs`), and the binary cache in the driver
(`crates/burdin_driver/src/scripts.rs`). The conformance tests are `tests/s3_scripts/`; the black-box driver checks
are `tests/check_scripts.py`, run by every CI job with a built driver.

## The decision

- **A script is a one-file target that says so.** A `.bd` file is a script when its first line is a shebang, or
  when its leading `//!` doc block declares `[dependencies]`. Everything else stays exactly what a one-file target
  always was: no cache, no manifest. `burdin run plain.bd` behaves as before (SC5).
- **The manifest is the leading doc block.** The issue asked for an "optional leading doc comment block" holding a
  manifest fragment; this page fixes the exact syntax (SC3). The alternative — a fenced code block inside the
  doc — was rejected: the block form is unambiguous, reads as documentation, and needs no Markdown machinery in
  the loader.
- **The cache is content-addressed, not path-addressed (SC5).** Two copies of the same script in different
  directories share one cache entry, and touching a file changes nothing. A cached binary is only ever reused for
  content that produced it, so a stale binary is impossible by construction.
- **Shebangs are lexical.** `#!` at byte 0 is skipped by the lexer, so every command that reads a `.bd` file —
  `build`, `run`, `check`, `test`, `lint`, `doc`, `fmt` — accepts a script unchanged. `#!` anywhere else is E0300.
  Burdin has no other use for `#`, so there is no comment syntax to confuse it with.

## The shebang line

**SC1. Accepted on line one.** A file whose very first bytes are `#!` is a script: the lexer skips the line,
exactly like a comment, so `#!/usr/bin/env burdin run` is not part of the program. The line ends at the first
newline (a `\r` before it is white space); a shebang-only first line with a `\r\n` ending is accepted on every
platform. A file that starts with anything else — including a blank line or a space — has no shebang, even if
`#!` appears later.
Tests: `tests/s3_scripts/shebang.bd`, `shebang_not_first.bd`.

**SC2. Rejected anywhere else.** `#!` that is not at byte 0 is E0300 (`Code::SHEBANG_PLACEMENT`), reported at the
`#!` with the span of those two bytes; the rest of the line is skipped so one mistake is one error. A bare `#`
without `!` keeps the plain "unexpected character" E0001. Registry: `tests/README.md`. Reference entry with a
checked example: `docs/error-codes.md` (`tests/check_error_docs.py` compiles it).
Tests: `tests/s3_scripts/shebang_not_first.bd`, `tests/check_scripts.py` (`misplaced.bd`).

## The manifest fragment

**SC3. The syntax.** A script's manifest is its leading doc block: after the optional shebang line and any blank
lines, the run of lines that start with `//!`. The parser scans that block for a line that is exactly
`[dependencies]`; everything before it is ordinary documentation and is ignored. From that header to the end of
the block, every non-blank line must be a dependency entry in one of the forms `burdin.toml` accepts —

```burdin
#!/usr/bin/env burdin run
//! [dependencies]
//! greet = { path = "../greet" }
//! numbers = "../numbers"

use greet.hello
fn main() {
    println(hello())
}
```

— where the path is relative to the script's own directory. A line that is not an entry, a name that is not a
Burdin identifier, a dependency declared twice, and a second `[section]` header (a script has no other manifest
sections; its name is its file stem) are load errors reported as `<script>:<line>: <message>` before any source
is checked. An empty `[dependencies]` section is a valid script with no dependencies. A doc block that does not
contain `[dependencies]` is documentation only, never a manifest.
Tests: `tests/s3_scripts/manifest_deps.bd`, `tests/check_scripts.py` (the malformed and sectioned fragments).

**SC4. Loading.** Any command that loads a one-file target resolves the fragment: the entry package is the script
itself (one module, the empty module path, the file's stem as the package name), and its `deps` are the packages
the entries name, loaded by the ordinary path-dependency loader (`spec/build-scripts.md`'s rules included: a
dependency with a `build.bd` is not granted by a script, so it is E0260). The package exists only in memory —
"the cached, hidden package": no directory or `burdin.toml` is created for it. Dependency names must be unique and
must differ from the script's own stem.
Tests: `manifest_deps.bd`; `tests/check_scripts.py` (`build` and `check` on a script with a dependency).

## The binary cache

**SC5. What is cached, and when.** `burdin run` on a script hashes its content and runs the binary of that hash
from the cache when there is one. The key is a 128-bit hash of: the script's text; every package of the dependency
closure — its name, its `burdin.toml`, and every file under its directory in sorted order (hidden files and
`target/` skipped); the compiler's version, commit, and LLVM version; and the flags that change a build
(`--release`, `-g`, `--strip`, `--lto`, PGO, `--link-lib`). Program arguments, the working directory, the script's
path, and `BURDIN_CC` are not part of it: the same content builds the same binary. A plain one-file target — no
shebang, no fragment — is never cached.
Tests: `tests/check_scripts.py` (the cache hit with a broken linker, the content change that forces a rebuild, the
dependency change, `touch`, and the plain-file negative).

**SC6. Where, and how it is published.** Entries live at `<burdin home>/cache/scripts/<key>/script[.exe]`
(`spec/repl.md`, RP14, fixes the home). A build goes into a staging directory beside the entry, and only the
finished binary is published, by one file rename (retried briefly, then a copy renamed into place), so an
interrupted build never leaves a binary a later run would trust; a build that fails leaves nothing. The file is
renamed, not the staging directory, because Windows refuses to rename a directory while anything inside it is open,
and a freshly linked executable is often held for a moment by a virus scanner. The staging directory is removed
afterwards, so an entry holds the binary and nothing else. A Burdin home that cannot be written is not a failure of
the script: the build is made in a temporary directory and run, uncached.
Tests: `tests/check_scripts.py`.

**SC7. Running.** The cached binary is started with the arguments after `--`, its exit status passed through as
`burdin run` always does, and its standard streams inherited.

## Command-line surface

```
burdin run <script.bd> [-- program arguments...]
```

No new options: the script's manifest carries the dependencies and the cache is automatic. `--release` and the
other compile options apply as they do to any build, and they are part of the key (SC5).
