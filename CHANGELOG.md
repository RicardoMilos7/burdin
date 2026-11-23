# Changelog

Burdin has no backward-compatibility promise (see `Burdin Language Design.md`). Entries are written per milestone; the
GitHub milestones and the roadmap track the work in progress. A milestone closes with an entry here (see the
close-out checklist in `AGENTS.md`).

## Unreleased

- Official packages are part of the standard library's surface: `use json` reaches `packages/json`, and an official
  package is never declared in `burdin.toml` (`json = true`, `json = { official = true }`, and `burdin add json` are
  errors that point at `use json`). The compiler scans each loaded package's `.bd` files for `use` declarations and
  loads only the official packages they name, with default features; an official package the program never imports is
  not loaded, checked, linked, or pinned. A generated module cannot bring one in on its own, since the dependency
  belongs to the package; a hand-written file of the package that imports it loads it for all its modules. Rules:
  `spec/packages.md` PKG15, PKG17, PKG18; tests: `tests/check_packages.py` and the loader's unit tests.
- Issue #277: compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`, `&=`, `|=`, `^=`, `<<=`, `>>=`) on the built-in
  integers, floats, and `bool`. The value is evaluated first, the place's address once, and the operator follows the
  binary operator's overflow and division rules in both profiles. On a user type or a type parameter, `a op= b` is
  `a = a op b` through the operator's interface (`Add` for `+=`, and so on), so no new interface is needed; a missing
  one is E0141. The place is evaluated twice on a user type, a known gap. Rules: `spec/compound-assignment.md` CA1-CA6;
  tests `tests/compound_assign/`, `tests/fmt/compound_assign`, and the parser unit tests in `burdin_syntax`.
- Issue #280: declaration generation in comptime. A module-scope `comptime generate { ... }` block is evaluated at
  compile time to a `String` of Burdin source, and the compiler adds the `struct`, `enum`, and `fn` declarations it
  holds to its module. Generation runs before the whole-program check and sees only hand-written declarations; its
  names are not hygienic, as derives are, and errors in generated code are reported at the block. Other item kinds,
  a generator that fails, and `comptime generate` outside module scope are E0242, E0222/E0224/E0225, and E0243.
  Rules: `spec/generate.md` GN1-GN10; tests `tests/generate/`; decision 34 in `tests/README.md`.
- Fallible allocation (issue #278; `spec/allocators.md`, AL11). `std` declares `AllocError` with `CapacityOverflow` and
  `OutOfMemory`. The `try_` forms return `Result<_, AllocError>`: `List<T>.try_with_capacity`, `try_with_capacity_in`,
  `try_reserve`, and `try_reserve_exact`; `Map<K, V>.try_with_capacity` and `try_reserve`; and `String.try_with_capacity`
  and `try_reserve`. A failed call leaves the container unchanged. `Global` allocates through `__try_alloc` and
  `__try_realloc`, and `Arena.allocate` returns null, changing nothing, when the heap refuses. The `Allocator` contract
  now allows a null result: an infallible container operation whose stored allocator returns null panics with
  `out of memory`. `Heap`, `Deque`, `Set`, `PriorityHeap`, and `Shared<T>` keep infallible growth. Tests:
  `tests/m24_stdlib/try_alloc_*`.

- Issue #289: `-O0`, `-O1`, `-O2`, and `-O3` (or `-O <n>`) on `build`, `run`, `test`, `bench`, `check`, `fuzz`, and
  `debug` replace the profile's `opt-level` for one invocation, and `burdin.toml`'s `[profile.<name>] opt-level` sets
  the level a profile builds with. The level changes the optimizer only: `overflow-checks` and `debug-assertions`
  stay as the profile sets them, so `dev -O3` keeps its overflow checks and `--release -O0` still wraps. A level
  outside 0 to 3, a bare `-O`, and two levels on one command are usage errors. Rules: `spec/profiles.md`, BP4 and BP5;
  tests `tests/check_profiles.py` (the `-O` IR comparisons, the checks, and the usage errors) and the unit tests in
  `crates/burdin_driver/src/profile.rs`.
- Associated types (issue #393; `spec/associated-types.md`, AT1-AT6). An interface may declare `type Name`; a
  `Type: Interface` block binds each one exactly once with `type Name = Ty`, and a projection reads it back as
  `Self.Name`, `T.Name`, or `<T as Interface>.Name`. The checker resolves a projection through the bound environment
  (on a type parameter) or the program's impl blocks (on any other type), normalizes it during substitution with
  `Self`, and monomorphization replaces it with the binding, so no projection reaches code generation. A missing,
  unknown, duplicate, or interface-less binding is E0144/E0007; a projection no interface can answer, or that two
  answer, is E0145; an interface with an associated type is not object-safe, so `any I` on it is E0202. `Iterator<T>`
  stays generic. Tests: `tests/m21_reflect/associated_types.bd`, `associated_types_errors.bd`, and
  `associated_type_projection_errors.bd`; the error reference has E0144 and E0145 entries.
- Method-level type parameters (issue #391): a method, associated function, or extension method declares its own
  type and `const` parameters after its name. The impl block's parameters stay in scope in its signature and body,
  each call infers the method's arguments from the receiver, the arguments, closure bounds, and the expected type,
  checks their bounds, and monomorphizes per instantiation. Two inference gaps are closed: a function-typed
  parameter with an open return (`f: Fn(i32): U`) now tells the closure its parameter types, and closure capability
  is checked in CL5's direction so an `Fn` value meets a declared `FnMut`/`FnOnce` and binds `U`. An undetermined
  const parameter reports E0252 as for a function, and a generic parameter list that repeats a name is E0007 for a
  function, a method, and an interface method, as it already was for a type and an impl block. Rules:
  `spec/arguments.md` GM1-GM6; tests `tests/m07_generics/generic_methods`, `generic_methods_errors`, and
  `duplicate_type_parameter`.
- Issue #487: `while let pattern = value { ... }`, the `while` form of `if let`. It repeats its body while the value
  matches, evaluates the value again on every pass, binds the pattern in the body only, and takes no `else`; a label
  names it, and a `break` with a value is E0003. Rules: `spec/grammar.md` rule 8.11, `spec/memory-rules.md` MV8 and RT5.
- The `regex` official package (stage 3, issue #264; `spec/regex.md`, RX1-RX14): a Pike VM with leftmost-first matches, named
  groups, the flags `i m s x`, Unicode and POSIX classes, replacement templates, splitting, and escaping, all over byte offsets.
  Backreferences, lookaround, atomic groups, and possessive quantifiers are errors, not emulations, so every search runs in
  time linear in the pattern and the text. `tests/regex/cases.tsv` holds the goldens, `tests/check_regex.py` runs them and a
  seeded differential against Python's `re`, and `tests/s3_regex/usage` is the conformance case.
- Issue #878: on Windows the `tls` package uses SChannel, the system's own TLS stack, so Windows needs no OpenSSL
  install and the driver downloads nothing. Linux and macOS still link the system's OpenSSL 3. The `tls` package has an
  `openssl` feature and `http` a `tls` feature, both on by default, so `http = { official = true, default-features = false }`
  builds a program that links no OpenSSL (on Linux and macOS; the feature has no effect on Windows). A dependency may be
  written `{ official = true, ... }` to take the keys of PKG25 (PKG15). Rules: `spec/tls.md` TLS1, TLS1a, and TLS7,
  `spec/http-client.md` HC17.
- Completion after the `.` of a member chain (`resp.body.`, `client.send(req).`) offers the members on that keystroke
  instead of an empty list, because the receiver's type is read from the declarations along the chain (issue #879;
  `spec/lsp.md` LS12).
- The `math` official package (stage 3, issue #268; `spec/math.md`, MA1-MA12). Over `f64`, in pure Burdin: integer number
  theory on `u64` (`gcd`, `lcm`, `mod_pow`, `is_prime` exact for every `u64`, `factorial`, `binomial`; an overflow is
  `None`); `lerp`, `approx_eq`, `poly_eval`, `gamma`, `ln_gamma`, and the closure-taking `bisect` and `simpson`;
  statistics over a slice (`mean`, the variances, `median`, `quantile`, `covariance`, `correlation`, and more); `Vector<N>`
  with `cross` for `Vector<3>`; and two matrices, `Matrix<R, C>` (fixed size, inline, its product's shape checked by the
  compiler) and `DynMatrix` (heap-backed), sharing one LU with partial pivoting. A bad argument panics with a message that
  names the function; an absent answer is `None`. `tests/math/` holds about 2,400 known-answer cases generated with exact
  arithmetic (`tests/math/regenerate.py`) and run by `tests/check_math.py`; `tests/s3_math/` is the conformance suite,
  with an end-to-end usage case, a surface case, a compile-fail case for mismatched shapes, and one abort case per panic.

## 0.1.2 — 2026-11-21

- `https` in the `http` package (stage 3, issue #262 phase C, issue #260; `spec/http-client.md`, HC17). A connection is a
  plain `TcpStream` or a `TlsStream` and everything above it (heads, framing, bodies, redirects, retries, timeouts) is
  shared; `https` URLs connect to port 443 (or the URL's), verify the server against the system's trust store and the
  URL's host by default, and fail with the new `HttpError.Tls(TlsError)` (which replaces `HttpError.TlsUnsupported`;
  `is_tls()` and `is_verification()` classify it) without a retry or a plain-text fallback. `Client.trust(store)`,
  `tls_config(config)`, and `insecure_skip_verify()` configure it; ALPN offers `http/1.1` only. The pool key is the origin
  (scheme, host, port), the TLS handshake is bounded by `connect_timeout`, a stale pooled TLS connection is found through
  the TLS layer, a connection that ends without `close_notify` is a truncated body, and an unread response closes with a
  `close_notify`. `http` now depends on `tls = true`, so programs that use it link OpenSSL 3. The `unsupported URL scheme`
  message says `only http and https are supported`. `IoError` and `TlsError` are `Clone`. `tests/s3_http/tls` is the
  conformance case (an in-process HTTPS server: verification failures, redirects, pooling, truncation, retries, and
  timeouts); `tests/tls/regenerate.py` backdates the test certificates by a year and writes the copies that test keeps.
- The `tls` official package (stage 3, issue #262, sub-issues #322, #323, #324; `spec/tls.md`, TLS1-TLS8). TLS 1.2 and
  1.3 over OpenSSL 3 through the C bridge, with hand-written `extern "C"` bindings and memory BIOs, so no OpenSSL call
  blocks and the stream parks the task on the reactor. `Certificate`, `PrivateKey`, and `TrustStore` load PEM, DER, and
  files; `ClientConfig` verifies the chain and the host name by default (system roots, TLS 1.2 minimum, ALPN, a client
  certificate, and the loudly named `insecure_skip_verify`); `ServerConfig` serves a chain and can require client
  certificates; `TlsStream` is a `Read` and `Write` over a `TcpStream` with `recv`/`send` (`TlsError` results), clean
  close versus truncation (`UnexpectedEof`), timeouts, SNI, and the negotiated version. The compiler gains the
  manifest `[link]` table (`spec/packages.md`, PKG34) so a package declares its native libraries. A server speaks no ALPN
  and serves one certificate (`spec/tls.md`, TLS8). `tests/s3_tls/` holds the conformance cases (about 130 output
  lines of verification, version, mutual TLS, truncation, and timeout cases against an in-process server),
  `tests/tls/regenerate.py` makes the committed test certificates, and `tests/check_tls.py` and `tests/check_link.py`
  run in CI after the jobs install OpenSSL.
- `break` with a value (issue #276; `spec/grammar.md`, rule 8.10). A `loop` gives back the value of its `break`, so
  `let x = loop { break 42 }` binds `i32`; `break` with no value is `break ()`. The loop's type is the expected type
  when the context gives one, otherwise the type of the first break value, and `!` when nothing breaks out; every
  break of one loop must agree. A `while` or `for` can end by its condition or its items, so it always gives back
  `()` and `break 1` there is E0003. A labeled `break` carries its value to the loop it names. The value is computed
  before the scopes the jump leaves are dropped, and its view origin joins the loop's result, so a view of a
  loop-body local is still E0123/E0122. `tests/m02_basics/break_value*.bd`,
  `tests/m05_ownership/break_value_*.bd`, and `tests/m06_conventions/break_value_view.bd` cover it. Also fixes a labeled `for` that lowers to the iteration protocol:
  the label now goes on the generated `loop` rather than on the generated block, so `break 'a`/`continue 'a` by the
  name of such a `for` work (issue #275 follow-up).
- 128-bit integers, `i128` and `u128` (issue #273; `spec/int128.md`, I128-1 to I128-13). Both are 16 bytes with
  16-byte alignment, lowered to LLVM `i128`. Literals, the full set of operators with debug overflow panics, shifts,
  `wrapping_`/`checked_`/`saturating_`/`overflowing_` methods, decimal and radix formatting, `parse`, casts (saturating
  from floats), ordering, ranges and iterators at compile time and at run time, patterns and exhaustiveness, hashing,
  and `List`/`Map`/`Set` all accept them. Division needs no compiler runtime library (MSVC has none). A 128-bit
  integer cannot cross the C boundary (E0162), and `burdin bindgen --export` cannot spell one (E0263); `burdin
  bindgen` skips a C `__int128` function with a note. `tests/int128/` (21 programs) and two checks in
  `tests/check_bindgen.py` witness the rules.
- `From` for the scalar pairs `as` converts exactly, and `v.into()`: `T.from(v)` and `as` use the same built-in
  conversion, and `into` converts to the type the context expects (`spec/stdlib.md`, "Numeric conversions"). `From<S>` is
  a bound: a generic function with `T: From<S>` can call `T.from(v)` and `v.into()`.
- `TryFrom` and `try_from` (issue #274; `spec/stdlib.md`, "Conversions"). A `Type: TryFrom<Source>` block gives a
  type a fallible conversion whose `Result` error type the block chooses, and every integer type has `T.try_from(v)`,
  which returns `Result<T, TryFromIntError>` and is `Err` when the value is out of range. A type now converts from
  several source types through `From` as well: a type with two `From` blocks of different sources was rejected with
  E0007, so an error enum with two `from` variants of different sources did not compile. `tests/m24_stdlib/try_from_*`
  covers the feature and its diagnostics.
- Labeled loops (issue #275; `spec/grammar.md`, rule 8.9; `spec/memory-rules.md`, DR6 and CL8). A `while`, `loop`, or
  `for` may carry a label, `'outer: for ...`, and `break 'outer` and `continue 'outer` leave or go on with it from
  inside any loop nested in it; an unlabeled `break` or `continue` still names the innermost loop. A label no enclosing
  loop has is E0002, a label that repeats an enclosing one is E0007, a label on anything but a loop is E0001, and a
  labeled jump cannot leave a closure (E0002) or a `comptime for` body (E0220). A labeled jump drops the scopes it
  leaves, innermost first. `tests/m02_basics/labeled_loops*.bd`, `labeled_loop_not_loop.bd`, and
  `tests/m05_ownership/labeled_loop_moves.bd` cover it.

- Fixed a use-after-free and double drop when a block, `if`, or `match` value was passed as a borrowed argument, used
  as a method receiver, or bound with `let _`: `f(g()?)` with a `?` temporary was the first report (issue #866;
  `spec/memory-rules.md`, MV9). The tail of such a value now moves out, so the temporary drops exactly once.
  `tests/m05_ownership/temporary_question_arg.bd` and `tests/m05_ownership/move_block_value_borrowed.bd` cover it.
- `burdin build`, `run`, and `check` take `-q`/`--quiet`, which prints only errors, and `-v`/`--verbose`, which prints
  each phase of the compilation and each codegen unit with its time, in a fixed order (issue #799;
  `spec/build-tool.md`, BT8). `tests/check_verbosity.py` checks both with the packages under `tests/verbosity/`.

- Type checking continues past a syntax error (issue #786; `spec/check.md`, CK4). A file with a syntax error is not
  type checked, but every other module is, against the declarations the broken file did read. The follow-on errors of
  a declaration that did not parse (a use of a function, type, or method the broken file failed to declare) are left
  out, and a diagnostic in the broken file other than its syntax errors is not reported. `burdin check`, `build`, and
  the language server share the rule (`crates/burdin_query/src/follow_on.rs`). `tests/check/err_syntax_declared.bd`,
  `tests/check/packages/syntax_neighbors`, and `tests/check/packages/syntax_no_main` cover it.
- The `http` official package (stage 3, issue #260, sub-issues #328, #329, #330; `spec/http-client.md`, HC1-HC16).
  `packages/http` is an HTTP/1.1 client over `std`'s `TcpStream`: URLs and relative references, requests, responses and
  headers, a body that streams or is read whole (content-length, chunked with trailers, or until close, with a size
  limit), redirects with a limit, loop detection, and credential stripping across origins, connect, read, and total
  timeouts, retries of safe requests with backoff, and per-host connection reuse with a staleness probe. `https` URLs
  parse but failed with `HttpError.TlsUnsupported` (there was no TLS in `std`; `https` works now, see the entry above). `std` gains `TcpStream.connect_timeout`
  (`spec/io.md`, IO5). `tests/http/` holds golden fixtures for the pure parts, `tests/s3_http/client` is the
  conformance case against a scripted in-process server, and `tests/check_http.py` runs both and checks
  `burdin add http` and the pin; CI runs it.
- The `crypto` official package (stage 3, issue #266; `spec/crypto.md`, CR1-CR14). Pure Burdin, no C library: the
  hashes SHA-224/256/384/512, SHA3-224/256/384/512, SHAKE128/256, SHA-1, and MD5 (the last two documented as broken),
  each one-shot and incremental; HMAC, HKDF, and PBKDF2 over any of them; ChaCha20, XChaCha20, Poly1305, and the
  ChaCha20-Poly1305 and XChaCha20-Poly1305 AEADs; AES-128/192/256 with CTR, CBC (PKCS#7), and GCM; X25519; Ed25519
  signing and verification; and `constant_time_eq` and `random_bytes` (from the OS entropy source). Failures are
  `CryptoError` values or `false`, never panics, and the package and its docs say plainly that it is an experiment, not
  a security claim (AES uses a secret-indexed S-box and is not cache-timing safe). `tests/crypto/` holds about 500
  known-answer vectors (published RFC/FIPS/NIST vectors, asserted by `tests/crypto/regenerate.py`, plus vectors from
  OpenSSL) and the error messages; `tests/check_crypto.py` runs them through `tests/crypto/runner` and checks
  `burdin add crypto` and the official pin; `tests/s3_crypto/usage` is the conformance case, and CI runs the check.

- Cached object files (stage 3, issue #243; `spec/target.md`, TG11-TG16). A cacheable build keeps each codegen unit's
  object under `target/burdin/<profile>/objects/<kind>/`, keyed by a hash of the unit's module, the compiler, the
  optimization level, and the target. A later build whose unit is unchanged copies the object instead of optimizing
  and compiling it, so an edit to one function rebuilds only the units that contain it and a comment edit rebuilds
  none; only the last build's objects are kept. Code generation stays whole-program and parallel by unit.
  `tests/check_target.py` and the unit tests in `crates/burdin_codegen/src/cache.rs` cover it.
- One target directory per workspace (stage 3, issue #240; `spec/workspaces.md`, WS19). The members of a workspace share
  the root's `target/`: executables, test binaries, the `--failed` record, and the benchmark baselines sit in
  `target/burdin/<profile>/`, and a member's build-script output and Rust shims move to
  `target/burdin/<profile>/packages/<name>/` so members never overwrite each other. A standalone package keeps its own
  `target/`. `tests/check_workspace.py` and a unit test in `workspace.rs` cover it.

- Built-in types implement `Serialize` and `Deserialize` (issue #853; `spec/serialization.md`, SER14). The scalars,
  `String`, `List`, `Map<String, V>`, `Option`, `Result`, and tuples of 2-4 parts implement both interfaces without an
  impl block when their parts do, so a manual impl delegates to them: `String.deserialize(mut d)?` reads a string and
  `value.serialize(mut s)` writes one. The checker lowers both forms to the standard library's helpers
  (`std/src/serialize.bd`). `tests/s3_serialize/builtin_impls`, `builtin_methods`, `builtin_map_key`, and
  `builtin_serialize_mut` cover it.

- Workspaces (stage 3, issues #342 and #343; `spec/workspaces.md`). A `burdin.toml` with a `[workspace]` section makes
  its directory a workspace root: `members` lists the packages that build together (a path, or a `*` pattern within one
  component), `exclude` removes some, and `default-members` picks what a command in the root selects. A root with a
  `[package]` section is a package too; without one it is virtual, and `check`, `lint`, `fix`, and the other
  single-package commands reject it by naming its members. `build`, `run`, `test`, `bench`, `doc`, and `fmt` take
  `-p`/`--package`, `--workspace`, and `--exclude`, which select members whatever the target is; outside a workspace
  they mean what they did. `build` writes one executable per member that has a `src/main.bd`, under that member's
  `target/burdin/<profile>/bin/`, and skips a library with a note; `run` runs exactly one; `test` and `bench` run the members together with
  one report and share their `--failed` record and baselines under the root's `target/`; `doc` writes one site per
  member; `fmt` formats the selected members' directories. One `burdin.lock` sits beside the root manifest and records
  the whole workspace, whatever the command selects, and `burdin add` in a member edits that member and the root's
  lockfile. `[workspace.package]` sets `version`, `edition`, `license`, and `authors`, which a member takes with
  `key.workspace = true` (or `key = { workspace = true }`); a key both set and inherited, a key the root does not set,
  and any other inherited key are errors, `name` is never inherited, and `burdin fix --migrate` refuses an inherited
  edition. `[workspace.dependencies]` declares dependencies a member takes with `name.workspace = true`, a path there
  relative to the root. The language server no longer treats a virtual root as a package. `tests/check_workspace.py`
  checks all of it in every CI job.

- Dependency tables, features, and platform targets (stage 3, issues #340 and #341; `spec/packages.md`, PKG25-PKG33).
  `[features]` declares named features whose items are other features, `dep:x` for an optional dependency, or
  `x/feature` to forward to a dependency; every optional dependency is an implicit feature of its name, `default` is
  enabled unless `--no-default-features`, and `--features <list>` and `--all-features` extend the selection on `build`,
  `run`, `check`, `test`, `bench`, and `fuzz` (and `burdin watch` passes the flags to each run). A package with a
  `[features]` table or an optional dependency gets a generated, in-memory `features` module of `const <name>: bool`
  constants, so code branches with `comptime if features.<name>`. Features unify across the graph — one package has one
  feature set, the union of the entry package's request and every active dependent's — and a request for a feature the
  dependency does not declare is an error when the dependency is active. `[dev-dependencies]` are loaded only by
  `test`, `bench`, `fuzz`, and `check --tests`, and a test reaches one by its package name, because test items are
  stripped from a normal build. `[target.'cfg(...)'.dependencies]` and `[target.'cfg(...)'.dev-dependencies]` apply only
  on the host platform (`unix`, `windows`, `target_os`, `target_arch`, `target_family`, `all`/`any`/`not`; an unknown
  predicate and a bare target triple are errors). The loader reads a dependency only when it is active, and
  `burdin.lock` pins the potential graph — optional, dev, and every target's dependencies, and what a dev-dependency
  reaches, but never a dependency's own dev-dependencies — so features, the platform, and the command never rewrite it.
  `tests/check_features.py` checks all of it in every CI job, `tests/check_watch.py` checks the pass-through, and the
  loader's unit tests cover the `cfg` grammar, feature validation, unification, and the generated module.
- The build tool's command surface (stage 3, issue #338; `spec/build-tool.md`). The page records every command the
  `burdin` tool has, the Cargo commands that are planned but not copied (and the issue that owns each), the Cargo
  behavior that is deliberately left out (plugins and external subcommands, proc-macros and the native
  `links`/`-sys` build protocol, a registry client, arbitrary installation, layered configuration, formatter
  options, and `--manifest-path` on build commands), the command-line and environment rules, and the exit statuses.
  `tests/check_cli.py` checks the golden command list, every command's help, unknown and out-of-scope commands, the
  version, and one fixture package through `build`, `run`, `check`, and `test`.
- Named build profiles (stage 3, issue #339; `spec/profiles.md`). `dev`, `release`, `test`, and `bench` are built in,
  and a package defines or customizes one in `[profile.<name>]` with `inherits`, `opt-level`, `debug`,
  `debug-assertions`, `overflow-checks`, and `panic` (`abort` only). `--profile <name>` selects one on `build`,
  `run`, `test`, `bench`, `check`, `fuzz`, and `debug`, and `--release` is `--profile release`. `dev` and `test`
  share `target/burdin/debug`, `release` and `bench` share `release`, and a build script's cache key holds the
  settings rather than the name. `[profile.<name>.package.<package>]` overrides `overflow-checks` and
  `debug-assertions` for one package's functions. The generator's options now carry an optimization level and the
  two flags instead of one `release` bit. `tests/check_profiles.py` checks overflow, debug assertions, per-package
  overrides, the IR, and every failure path; `crates/burdin_driver/src/profile.rs` has the unit tests.
- The link can be done by a fast linker (stage 3, issue #335; README "Choosing the linker"). On Linux the driver
  passes `-fuse-ld=mold` when `ld.mold` is on `PATH`, then `-fuse-ld=lld` when `ld.lld` is, and otherwise leaves the
  choice to the C compiler driver (ld64 on macOS, `link.exe` with `cl`). `BURDIN_LD` overrides it with a linker name,
  a path (clang), or `platform`/`default`; `--lto` keeps the driver's linker and refuses mold; an MSVC-style driver
  rejects any other value, since it has only `link.exe`. A sanitizer or fuzz build also keeps the driver's linker, and
  naming another one there is a usage error: those runtimes start up from module constructors in `.ctors`, which lld
  leaves in a section libc never runs. A failed link says whether `BURDIN_LD` or the automatic
  choice supplied the linker and how to go back to the default. `tests/check_linker.py` checks it in every CI job,
  and the Linux job installs `lld`, so the whole suite links through the automatic choice.
- Build scripts are covered end to end through the command line (stage 3, issue #362; `spec/build-scripts.md`). A
  declared input in `<out>/inputs` (the `rerun-if-changed` equivalent) may now be a directory: it is hashed
  recursively over every file under it, so adding, removing, renaming, or changing a file inside reruns the script
  while touching one does not, and `burdin watch` follows a directory input too. The new black-box suite
  `crates/burdin_driver/tests/build_scripts.rs` runs with `cargo test --workspace` and drives the `burdin` binary over
  fixture packages: the success path, the stdout/stderr split, the documented arguments and environment, every rerun
  rule (declared inputs including a directory, `embed` inputs, generated outputs, the script, the manifest, and the
  profile), the dependency grant (including `burdin test -p`, where the selected package is the entry), stale
  generated modules, nested module paths, and every failure path with its code (E0260, E0261, E0262) and exit status.
- Package templates (issue #354): `burdin new <name> --lib` creates a library, `src/lib.bd` with one function and a
  `test` that `burdin test` runs, and a `.gitignore` for `target/` only; `--bin` (the default) is the program with
  `src/main.bd`. `burdin init [<dir>] [--lib | --bin]` writes the same package into an existing directory (the working
  directory by default) and keeps its other files. `init` refuses, before writing anything, when `burdin.toml`, the root
  module of its kind, or `.gitignore` already exists; `new` still refuses any existing path. The unit tests are in
  `crates/burdin_driver/src/new.rs`, and `tests/check_new.py` checks both commands in every CI job.
- Target directory and artifact cache (issue #353, `spec/target.md`, TG1-TG10): `build`, `run`, and `test` of a package
  directory keep the executable they link under `<package>/target/burdin/<profile>/bin/` (the test binary under `test/`),
  next to a fingerprint file that lists every input (the compiler, the host, the flags, each manifest and source as
  checked, the build inputs, and the Rust shim libraries) and the executable's SHA-256. A build whose fingerprint matches
  copies the cached executable instead of linking again; the front end always runs, so diagnostics do not change. Debug
  (`-g`), `--emit`, `--emit-ir`, and `--pgo-use` builds, bench and fuzz builds, and one-file targets are not cached. An
  `-o` copy gets the `.exe` suffix the MSVC linker would have added. `tests/check_target.py` checks it with a broken
  linker.
- Official packages (`spec/packages.md`, PKG15-PKG18): `name = true` in `[dependencies]` declares a package that ships with
  the compiler, from the new top-level `packages/` folder (`packages/README.md` explains it, and `json` is the first
  package in it). `burdin add
  <name>` writes the line when `<name>` is an official package; `name = false` and an unknown official name are errors that
  say what to do. The lockfile records such a package with `source = "official"` and the checksum of its files, a build
  writes it when there is none, and `--locked` fails when an official package is not the one the lockfile pinned. A `use`
  of an official package the manifest does not declare keeps E0002 and gains a help line. The compiler finds the folder
  through `BURDIN_PACKAGES`, `<exe dir>/../packages`, `<exe dir>/packages`, and (debug builds) the repository it was built
  from; the release archive carries `packages/` beside `burdin`, and `install.sh` and `install.ps1` install it beside the
  compiler. `tests/check_packages.py` and `tests/check_release.py` check it against the fixtures in `tests/packages/`.
- The `toml` official package (stage 3, issue #256; `spec/toml.md`, decisions TOML1-TOML16): TOML 1.0.0 reading and
  writing on the serialization framework, as `packages/toml`. `parse` reads a document into a `Value` tree that keeps
  the position of every key and value, and `render` writes the canonical form (a table's scalars first, then its
  tables as sections, and a non-empty all-table array as `[[path]]`; sorted map keys; basic-string escaping; floats
  and datetimes in the form TOML 1.0.0 reads back). `to_string`/`from_str` map a struct, map, or payload variant
  through the framework: `Option.Some` is the value, a unit variant its name, a payload variant `{ Name = value }`
  or `{ Name = [values] }`. The parser rejects everything TOML 1.0.0 rejects, with the message at the place the rule
  is broken (duplicate and redefined tables, closed inline tables, arrays of tables versus static arrays, integer and
  float ranges, datetime fields, escapes), and the reader reports a type mismatch at the value's line and column plus
  the framework's path. `tests/check_toml.py` drives it through `tests/toml/` (round trips, spans, malformed input,
  the framework's mapping, and writing) together with `burdin add toml` and its official pin,
  `tests/s3_serialize/toml` is the conformance case, and every CI job runs it.

## 0.1.1 — 2026-11-20

- The toolchain manager (stage 3, issue #624: #626, #628, #630, #631; `spec/toolchain.md`). The new
  `crates/burdin_toolchain` builds `burdin-toolchain`, which the release archive carries and the installers copy into
  the install directory as the `burdin` and `burdin-lsp` proxies. `burdin toolchain install|update|list|default|remove`
  manages `stable`, `nightly`, and exact versions under the Burdin home, with downloads that resume and are verified
  against `SHA256SUMS` before they replace anything. `burdin +<toolchain> ...`, `BURDIN_TOOLCHAIN`, the nearest
  `burdin-toolchain.toml` (installed on first use), and the default select a toolchain, in that order, and
  `burdin toolchain show` says which applied and why. The `debuggers`, `docs`, `lsp`, and `std-src` components are
  separate verified archives in each release, installed on demand; a target other than the host is refused until
  cross-compilation exists (issue #282). `burdin toolchain self show|update|uninstall` prints where everything lives,
  replaces the proxies with a newer release's manager, and removes exactly what the installers and the manager
  created, including the startup-file line or user `PATH` entry. `tests/check_toolchain.py` checks it in every CI job.
  SHA-256 moved into the new `crates/burdin_sha256`, shared with the package resolver.


- `burdin watch [check|build|test|run] <target>`, and `--watch` on those commands (issue #632, `spec/watch.md`): rerun
  the command whenever a source, `burdin.toml`, a path dependency, or a build input changes, with a debounce, clearing the
  screen first unless `--no-clear`. `target/`, version-control metadata, `burdin.lock`, and the command's own output are
  ignored. `run` restarts the program; `test` runs the changed modules' and their dependents' tests first. The files are
  polled, so it needs no new dependency. `tests/check_watch.py` runs in every CI job.
- The `burdin` help is now cargo-style and built with clap (`crates/burdin_driver/src/cli.rs`): `burdin` or
  `burdin --help` lists the commands with a one-line summary each, and `burdin <command> --help` or
  `burdin help <command>` prints that command's usage, arguments, grouped options, and notes. A mistyped command gets
  a suggestion. A usage error prints its message, the command's usage line, and a pointer to its `--help`, instead of
  the usage of every command. Each command still reads its own options, so their error messages are unchanged.
- On Windows, linking works from an ordinary terminal (issue #826). When `LIB` is not set, the driver finds the
  Visual Studio installation (`VCINSTALLDIR` or `vswhere.exe`), its default MSVC tools version, and the newest Windows
  SDK, passes their library directories to `link.exe` as `/LIBPATH:`, and runs that installation's `cl.exe` unless
  `BURDIN_CC` names a linker. Without the build tools it says what is missing and how to install it, instead of
  `LNK1181: cannot open input file 'libcmt.lib'`. The installers and `docs/platform-support.md` state the requirement.
- `install.ps1` adds the install directory to the user's `PATH` (`HKCU\Environment`, once; a second run changes nothing),
  so new terminals find `burdin`. `install.sh` adds a PATH line to `~/.zshrc`, `~/.bashrc` (`~/.bash_profile` on macOS), or
  `~/.config/fish/config.fish`, once; other shells get a hint. `BURDIN_NO_MODIFY_PATH` or `-NoModifyPath` skips both.

### Serialization framework core (issues #297, #299, #300)

- `std.serialize` (`std/src/serialize.bd`) declares the data model (`Serializer`/`Deserializer`), the user-facing
  `Serialize`/`Deserialize` interfaces, `DeError` with its path and the `field`/`index`/`key`/`variant` operations, the
  per-shape helper functions, and a small human-readable test format (`TextSerializer`, `TextDeserializer`, `to_text`,
  `from_text`). `spec/serialization.md` is the design (SER1-SER16); map keys are `String` and v1 produces owned values.
- `struct T: Serialize, Deserialize` (and an enum) is a built-in derive (issue #298): `expand_derives` generates the
  two impl blocks, `std`'s scalars through their helpers, the built-in container shapes written out recursively
  because their helpers need the primitive impls that are still to come. A generic type gets one bound per type
  parameter, and the generated format parameter is `__S`/`__D` so it never shadows the type's own. The fields'
  requirement is not checked yet (`spec/serialization.md`, SER12).
- The compiler-provided impls for primitives and built-in containers, and the derive's field requirement, are the
  next phase; until then a hand-written impl and the derived scalars call the helpers. The format packages are
  separate official packages; `json` is the first one (below), csv, yaml, and xml follow it, and toml is still to come.
- `tests/s3_serialize/` covers the scalars (including the non-finite floats), nested sequences and maps, a hand-written
  enum, the field/index/key/variant error paths, the derived struct, enum, generic type, and errors, and the tuple
  helpers end to end. Every writer helper takes the serializer first and the value second, and every tuple reader
  consumes the separators between its parts and reports a short sequence as `expected N elements in a tuple`.

### The `json` official package (issue #255)

- `packages/json` is the first official package: `json.to_string`, `json.to_string_pretty`, and `json.from_str` over
  the framework's data model. `Writer` implements `Serializer` and `Parser` implements `Deserializer`; both are public
  so a hand-written impl can drive them. `Option` is `null`/the value itself, variants are externally tagged (`"Dot"`,
  `{"Circle": 1.5}`, `{"Rect": [1.5, 2.5]}`), and bytes are an array of numbers.
- Reading reports a line and a 1-based column for a syntax error, adds the framework's path as the error unwinds
  (`servers[0].port`, `["key"]`), rejects duplicate object names, decodes `\uXXXX` and surrogate pairs, and never
  panics on malformed text. Writing rejects a non-finite float instead of writing invalid JSON, and writes a negative
  zero as `-0.0` (the display form `-.0` is not a JSON number).
- `tests/json/` holds the fixtures (`roundtrip`, `pretty`, and one malformed document per file under `bad`) with
  their `.expected` goldens; `tests/check_json.py` runs them and checks `burdin add json`, the official pin in
  `burdin.lock`, and `--locked` against a changed package. `tests/s3_serialize/json` is the conformance case.

### The `csv`, `yaml`, and `xml` official packages (issues #257, #258, #259)

- `packages/csv`: a record-at-a-time reader and writer (`Reader`, `Writer`, `to_string`, `from_str`) with RFC 4180
  quoting, headers (a struct's field names), LF/CRLF/CR line endings, a custom delimiter, and typed records on the
  serialization framework. Errors carry a line, a column, and the record/field path; a malformed input never panics.
- `packages/yaml`: a safe YAML subset (block and flow collections, plain/quoted/block scalars, comments, anchors and
  aliases resolved while reading with node, size, and depth limits). Tags other than `!!str`, directives, merge keys,
  complex keys, and multiple documents are rejected with a located error, so nothing constructs arbitrary types.
  `yaml.to_string` and `yaml.from_str`; the README lists the subset exactly.
- `packages/xml`: a namespace-aware reader and writer with the predefined entities, numeric references, CDATA,
  comments, processing instructions, and a small DOM (`Document`, `Element`, `Node`, builder, compact and pretty
  output). A DOCTYPE is an error, so there are no external entities or entity-expansion bombs. `xml.to_string` and
  `xml.from_str` map types to elements.
- `tests/csv/`, `tests/yaml/`, and `tests/xml/` hold the fixtures with `.expected` goldens; `tests/check_csv.py`,
  `check_yaml.py`, and `check_xml.py` run them and check `burdin add`, the official pin, and `--locked`. The conformance
  cases are `tests/s3_serialize/{csv,yaml,xml}`, and CI runs all three checks.

### Advanced testing tools (issue #615, sub-issues #616, #618, #620, #622)

- `assert_snapshot(value)` (and `assert_snapshot(value, @"text", display: true)`) compares a value's `Debug` or
  `Display` output with `snapshots/<module>__<test>.snap`. A missing or changed snapshot fails with a diff and writes
  `.snap.new`; `burdin test --bless` accepts every change, `--review` asks per snapshot, and a snapshot no test checks
  any more is reported as stale (`spec/test-runner.md`, SN1-SN9).
- `fuzz name(data: [u8]) { ... }` declares a fuzz target (E0173 for any other signature). `burdin fuzz <target>
  <name>` builds it with libFuzzer's coverage and AddressSanitizer and keeps `fuzz/corpus/<name>/` and
  `fuzz/artifacts/<name>/` next to the package; `burdin test --fuzz-replay` replays saved crashes as test cases
  (`spec/fuzz.md`, FZ1-FZ7).
- `--sanitize=address,thread,leak,undefined` for `burdin build`, `run`, and `test` (`spec/sanitizers.md`,
  SAN1-SAN6), with the supported combinations listed in `docs/platform-support.md`. A scheduled CI step runs the
  stage 0 and stage 1 suite under `--sanitize=address`. The first such run found and fixed two code-generation bugs:
  the sanitizer's zero-sized loads and stores of empty aggregates, and six places where a statement temporary
  (a comparison operand, a `List` borrowed by `len`/`as_slice`/`for`/`hash`, a closure called where it was made) was
  never freed.
- `burdin test --interpret` runs the tests in the interpreter over a modeled C heap with strict checks: bounds,
  alignment, uninitialized reads, use after free, double free, and leaks, each reported at the source line. A test
  that calls C or the operating system is skipped with the reason (`spec/test-runner.md`, IN1-IN6).

### Derived interfaces after the type's name (issue #825)

- `struct Point: Copy, Clone, Eq { ... }` replaces `struct Point derives (Copy, Clone, Eq) { ... }` on structs, enums,
  and error enums; `repr(C)` comes after the list. The old clause is a syntax error, and `derives` is an ordinary
  identifier.
- The list also takes an interface whose methods all have default bodies: the declaration implements it as an empty
  `Type: Iface {}` would, with the interface required on each type parameter of a generic type, like a derive.
- E0234: an interface with a method that has no default (`Drop`, `From`, `Display`, ...) cannot be listed. `Drop`
  in the list was E0002 before.
- `burdin fmt`, `burdin expand`, `burdin doc`, `std/`, the conformance tests, and the specs use the new form.

### Language server: completion, snippets, and the built-ins (issues #819-#824)

`spec/lsp.md` (LS8, LS12, LS13) is the specification; `crates/burdin_lsp/tests/completion.rs` checks it.

- **Built-ins without source.** `println`, `format`, `panic`, the built-in types, and their compiler-provided methods
  (`String.new`, `str.len`, `i32.to_string`, `List.push`, ...) are declared in a generated, read-only `builtins.bd`
  beside the materialized standard library: go to definition lands there, and hover and signature help show the
  declaration (`fn println(fmt: str, ...)`, with the variadic part as the active parameter after `fmt`).
- **Members.** After a type name its associated functions (`String.`, `i32.`, `f64.parse`); after a literal or a
  primitive value its built-in methods, the prelude's extension methods, and what an implementation or derive provides
  (`to_string`, `clone`, `cmp`); after a prelude-bound standard module (`mem.`, `fs.`) its items. An empty statement
  offers the whole prelude and the standard modules.
- **Contexts.** `self`, `mut self`, and `own self` only as a method's first parameter; in the `: A, B` list after
  `struct Name` or `enum Name`, the seven built-in derives and the user `derive` declarations in scope.
- **Snippets and calls.** With the client's `snippetSupport`: item snippets at the top level (`main`, `fn`, `struct`,
  `enum`, `error enum`, `interface`, impl blocks, `test`, `bench`), a method snippet in an impl block, and statement
  snippets in a body (`if`, `match`, `for`, `while`); a completed function inserts `name($0)` (opening signature help)
  or `name()$0`, but not before an existing `(` or in a `use` path.

### Dependencies: `burdin add` and `burdin.lock` (issues #238, #239)

`spec/packages.md` (PKG1-PKG14) is the specification; `tests/check_packages.py` checks it and every CI job runs it.
The new crate `burdin_pkg` resolves dependencies; the manifest parser and the lockfile format stay in the package
loader in `burdin_query`.

- **Git dependencies.** `name = { git = "<url>" }` with an optional `branch`, `tag`, or `rev`. Repositories are cloned
  into a cache under `BURDIN_HOME` (`git/db`, `git/checkouts/<key>/<commit>`) by the `git` executable. A `version`
  alone is an error that says there is no package registry yet.
- **`burdin.lock`.** Every package of the graph with its version and dependencies; a git package also records its
  URL, reference, exact commit, and the SHA-256 checksum of its files. Builds keep it current, a path-only graph
  needs none until one exists, a changed checkout fails its checksum, and the loader builds a git dependency from
  the commit the lockfile pins, so a moved branch changes nothing.
- **`--locked`** on `build`, `run`, `test`, `bench`, and `check`: a lockfile that would change is an error (exit
  status 1) listing each difference, and nothing is written.
- **`burdin add [<name>] (--path <dir> | --git <url> [--branch | --tag | --rev])`** edits `[dependencies]` as text
  (one line added or replaced, everything else kept), resolves the whole graph, then writes `burdin.toml` and
  `burdin.lock`; any failure leaves both unchanged.

### Fixes, editions, and the refactoring engine (issues #604, #606, #607, #609, #611, #613)

`spec/fix.md` and `spec/refactor.md` are the specifications; `tests/check_fix.py`, `tests/check_ssr.py`, and
`tests/check_refactor.py` check them, and the conformance runner applies the suggestions the tests expect.

- **One edit model.** A diagnostic's suggestion is a message, a list of edits, and an applicability
  (`machine`, `maybe-incorrect`, `has-placeholders`, `manual`). The human renderer shows the diff inline and the JSON
  and SARIF carry the edits. The applier, the conflict check, and the diff live in `burdin_diag::edit`, and the linter
  and the language server use them.
- **Compiler suggestions.** A misspelled name with one near miss (or the one module that declares it), a missing
  `mut`, a `mut` marker to remove, and `.clone()` after a move (maybe-incorrect), with codes registered in
  `tests/README.md`, and two directives (`suggest:`, `fixed-error:`) the runner checks.
- **`burdin fix`** applies every machine-applicable suggestion of the compiler and the linter, round after round, and
  formats what it touched. `--dry-run` prints the diff; a dirty tree needs `--allow-dirty`; edits that clash are
  skipped and reported; a round that adds errors is undone.
- **Editions.** `[package] edition` with 2024 (default) and 2026. A removed form is deprecated one edition earlier
  (`E0301`) and `burdin fix --migrate` writes the fix and moves the manifest. `unimplemented` is the first form on
  this path. `docs/releasing.md` records the policy.
- **`burdin ssr`** replaces whole expressions matched by a pattern written as Burdin code, with `$name` placeholders
  that may carry a type (`where $a: Option<_>`), names matched by resolution rather than spelling, and every result
  checked before it is written; the language server's `burdin.structuralSearchReplace` command runs the same engine.
- **`burdin refactor`** extracts a variable or a function (parameters follow the memory rules: moved is `own`, written
  is `mut`, read is the default), inlines a local or a function, changes a signature (reorder, add with a default,
  remove, rename, with every call updated), and moves an item to another module. Each refuses with the reason when the
  result would not compile or would change what the program does, and the language server offers the extract and
  inline actions.

## 0.1.0 — 2026-11-19

The first published release of the Burdin compiler, for Linux, macOS, and Windows on x86-64 and arm64. Burdin is a
personal experiment, not for production, and makes no backward-compatibility promise. The per-milestone history is
in `CHANGELOG.md` in the repository; in short:

- **Stage 0 — the core language.** Lexer and parser, primitives and control flow, type checking, structs, enums and
  `match`, moves, `Drop` and `Heap`, conventions and references, generics, interfaces and derives, modules, `extern`
  and `unsafe`, and the standard library.
- **Stage 1 — the rest of the core language.** Closures, iterators, formatting, `any` and `impl`, errors, default and
  named arguments, extension methods, operator overloading and `Index`, comptime, reflection and user derives, const
  generics, `embed`, build scripts, and allocators with `Arena` and `Shared`.
- **Stage 2 — concurrency.** Tasks and the scheduler, channels and `select`, shared state, and asynchronous I/O.
- **Stage 3 — tooling.** `burdin fmt`, `lint`, `doc`, `check`, `new`, `bench`, doc tests and `burdin test` filters,
  machine-readable diagnostics, the query-based compiler library, `burdin lsp`, `burdin repl`, single-file scripts,
  C bindgen and Rust interop, debugger integration, and parallel compilation.
- **Releases.** Archives for six targets with SHA-256 checksums and one-line installers. Linux and macOS builds link
  LLVM statically; Windows archives carry `LLVM-C.dll` beside `burdin.exe`.

## Stage 3 — 0.1.0 (2026-11-19)

### Inspecting what the compiler generated (issue #595, sub-issues #597, #598, #600, #602)

`spec/inspect.md` (INS1-INS19) is the specification; `tests/check_inspect.py` checks it and every CI job runs it. The
new crate `burdin_inspect` holds the logic and reads what the compiler built; the driver calls it.

- **`burdin expand` (#597).** Prints the source after comptime evaluation, `comptime if`/`for` unrolling, user
  derives, default and named arguments, and operators: each file's own text with only the rewritten parts replaced,
  then formatted. `--item <path>` prints one item, `--instances` adds the instances of generic functions. The
  expansion of every conformance program of `m17_arguments`, `m19_operators`, `m20_comptime`, and `m21_reflect` goes
  back through `burdin check` and prints what the original prints. Extension calls stay method calls (an extension
  is not a value, EX1), and a rewrite with no source form is skipped, never guessed.
- **`--emit` and `burdin asm` (#598).** `burdin build --emit tir,llvm-ir,llvm-ir-opt,llvm-bc,asm,obj,deps` writes each
  artifact next to the output; `--intel`/`--att` choose the x86 dialect. `burdin asm <target> [<function>]` cuts one
  function out of the program's assembly with the source lines interleaved (`--llvm` for its IR), and lists the
  instances of a generic function named without type arguments.
- **`burdin bloat` (#600).** Reads the symbol sizes of the release object file (ELF, Mach-O, and COFF readers) and
  reports the largest functions and the size per package, module, and generic, with the glue, the runtime, and the rest
  counted apart; `--format json` for tools.
- **`burdin check --print-instances` (#602).** Every generic function and type instance the program created, the
  call that required it (the chain is derived from the call graph, so sema is unchanged), and the instances nothing
  reaches counted apart.

### Rust interop through C-ABI shims (issue #249, sub-issue #477)

`spec/rust-interop.md` (RI1-RI11) is the specification; `tests/check_rust_interop.py` checks it, and the packages
`tests/m22_build/rust_*` are conformance tests. `burdin bindgen --rust <crate> --out <file> --shim <dir>` reads a
crate's rustdoc JSON (format 61, Rust 1.100.0 pinned) and writes a C-ABI shim crate plus a Burdin module: `repr(C)`
structs by value with size, alignment, and offset checks on both sides, other types as opaque handles with generated
`_free` functions, fieldless enums as integers, `&str`/`String`/slices as pointer and length, `#[no_mangle]`
functions declared directly, and every item it cannot represent skipped with a `// bindgen:` note. A package binds a
crate with `[rust] toolchain` and `[rust.crates]` in `burdin.toml`: `build`, `run`, and `test` generate the shim when
the crate's sources changed, build it with cargo under the pinned toolchain, and link the static library and the
system libraries `rustc` reports. A failure is the new E0264; a dependency's crates need a `[build] allow` grant
(E0260). New crate `burdin_rustgen`; the macOS and Windows paths are written but not run.

### Release builds link LLVM statically

- Linux and macOS release archives hold a compiler with LLVM linked in: no `libLLVM` is needed at run time.
  `BURDIN_STATIC_LLVM=1` selects it in `crates/burdin_driver/build.rs`, which passes the archives as link arguments of the
  binary so they follow every crate on the link line; development builds stay shared.
  `tests/build_release.py` sets it, checks the binary with `ldd`/`otool -L`, and the reproducibility check covers it.
- Windows archives carry `LLVM-C.dll` beside `burdin.exe`, and `install.ps1` installs it with the compiler, so a fresh
  install starts without LLVM on `PATH`. `build_release.py` refuses to package a Windows build without the DLL and
  takes `--llvm-dll`. A static Windows build (`BURDIN_STATIC_LLVM=1`, static CRT) is wired but not yet proven.

### Ownership and drop visualizer (issue #587, sub-issues #589, #591, #593)

`spec/check.md` (CK10-CK11), `spec/lsp.md` (LS5, LS11, LS13), and `spec/query.md` (QL7) are the specification;
`tests/check_check.py`, the LSP tests, and the `tests/diagnostics/` goldens check it.

- **A stable ownership event list.** The move and borrow checks record every declaration, move, copy, borrow, drop,
  drop flag, argument convention, and derived clone in `tir::Function::events` (no new analysis), and `burdin_query`
  exposes it as `Analysis::ownership_events`. The command line, the language server, and the REPL render the same
  facts.
- **`burdin check --explain-ownership` (#589).** Prints each function's source with margin annotations — `move a -> b`,
  `copy`, `borrow (read)`/`borrow (mut)`, `drop a` at the closing brace, `drop flag`, the parameter conventions at each
  call, and derived clones. `--fn <name>` selects one function; `--format json` lists the events with spans.
- **The ownership lens (#591).** `inlayHints.ownership`, off by default, adds inlay hints for moves at their use and
  implicit drops at the closing brace; a hover on a local lists its moves, copies, borrows, drops, and clones.
- **`burdin check --explain` (#593).** Traces a memory error from the value's declaration through every move or
  borrow to the failing use, with the rule ID from `spec/memory-rules.md` and the reference's suggested fix over
  E0100, E0101, E0103, E0104, E0110, E0111, E0121, E0122, and E0123. The diagnostics carry a one-line pointer, and
  their notes (the JSON `related` information) name the declaration and the move or borrow sites. The plain
  diagnostic line and the conformance suite's markers are unchanged.

### Debugger integration (issue #565, sub-issues #567, #569, #571, #572, #574, #576)

`spec/debug.md` (DBG1-DBG12) is the specification; `tests/check_debug_info.py` and `tests/check_debug_printers.py`
check it with gdb and lldb, `tests/check_dap.py` and `crates/burdin_dap/tests/protocol.rs` check the adapter, and
every CI job runs them.

- **A stable panic hook (#572).** The runtime defines `burdin_panic_begin`, an empty external function every panic
  path calls before it prints anything, so a breakpoint there stops with the whole stack intact. With `-g` the
  generated module carries a frame table — `{ addr, name, name_len, file, file_len, line }` per emitted function —
  and `burdin_rt_frame_info(pc)` reads it, so a panic backtrace prints `  #0  at src/app.bd:12 in app.main` for the
  program's functions and raw addresses for everything else. Without `-g` the backtrace is exactly what it was.
  Error traces carry the function too: `TraceFrame` gains `func`, `Display` is `at {func} ({file}:{line}:{col})`,
  and `Error.print_trace()` prints a trace's frames.
- **Locals that tell the truth (#571).** With `-g`, a local's DWARF location starts at its initialization, ends
  where the move checker proved it moved or dropped, and at its scope end, so a debugger shows `<optimized out>`
  instead of stale bytes. The kill sites travel in `tir::Function::debug_kills` from `burdin_check::moves` to
  codegen; the mechanism is `dbg.value` alone (a `dbg.declare` would pin the slot and drop later changes). Early
  exits and partial moves keep their location, documented as the linear-range gaps they are.
- **Pretty-printers (#567).** `debuggers/gdb/burdin_printers.py` renders `String`/`str`, `List`, `Map`,
  `Option`, `Result`, enums, `Heap`, `Shared` with its strong count, `any I` with the concrete type name from the
  vtable, closures, and ranges; a `-g` binary embeds the script in a `.debug_gdb_scripts` entry, so gdb loads it
  with the program and nothing has to be installed. `debuggers/lldb/burdin_formatters.py` does the same for lldb;
  `burdin debug` and `burdin dap` write it beside the session and import it. The debug types the compiler emits are
  the layout contract (`emit/debug.rs`): typed element pointers, a `Map` slot struct, a `Shared` cell, and the
  vtable header — the printers read members by name, never by offset. A boxed `any I` now names its concrete type
  as an artificial variable so the type is in the debug info even when no local has it.
- **Windows visualizers (#569).** `debuggers/windows/burdin.natvis` mirrors the same layouts, and a `-g` build on
  MSVC writes it beside the executable and links with `/DEBUG` and `/NATVIS:`, so Visual Studio and WinDbg read it
  from the PDB.
- **`burdin debug` (#574).** Builds the target with `-g` in a temporary directory and starts the platform debugger
  on it (gdb, lldb, cdb, or `--debugger <name | path>`), with the printers loaded and its exit status passed
  through. `--break-on-panic` starts stopped on the panic hook, `--test <name | index>` debugs one `test` block
  (resolved like the runner's filters), and program arguments follow `--`. `burdin build --tests` is the same test
  build without a runner.
- **`burdin dap` (#576).** A Debug Adapter Protocol server that forwards to `lldb-dap` and adds Burdin behavior at
  four points: a `launch`/`attach` request may name a `target` (and a `test`) that the adapter builds with `-g` and
  whose formatters it imports; the "Burdin panic" exception filter sets a function breakpoint on the panic hook,
  merged with the client's own; steps that stop in generated code are stepped out of and never reported; and
  everything else is forwarded unchanged with the client's ids restored. New crates: `burdin_json` (the JSON value
  and `Content-Length` framing, shared with the language server) and `burdin_dap` (the adapter, with a
  `burdin-dap` binary without LLVM). The VS Code extension registers the `burdin` debug type against it, and
  `editors/README.md` has the Neovim and Helix DAP setup.

### Bench blocks and `burdin bench` (issue #251, sub-issues #653-#655)

- `bench "name" { ... }` is a module-scope item beside `test` (`spec/bench.md` BN1, `spec/grammar.md`): a contextual
  keyword, no attributes, stripped from every build but `burdin bench`. One call of the body is one iteration,
  and `black_box(value)` (`spec/stdlib.md`) hands a value back through an opaque barrier so measured work cannot be
  optimized away; at compile time it is the identity, and a view argument is E0120.
- `burdin bench` (new `crates/burdin_driver/src/bench_runner.rs`, the harness in
  `burdin_codegen/src/runtime.rs::define_bench_main`) compiles the selected packages in the release profile and
  measures each bench in its own process: a warmup, a growing-batch calibration so the clock's granularity cannot
  swamp the estimate, then `--sample-count` samples of about `--sample-time` milliseconds each.
- The report gives the median, the mean, a 95% confidence interval, the median absolute deviation, and the
  outliers; `--format json` streams a version 1 event schema, and `--list` prints the selected benches. The filters,
  `--exact`, `--skip`, `-p`, `--workspace`, and the full-path rules are the test runner's; `-j` bounds compilation
  only, because benches are measured one at a time.
- Baselines (#654): `--save-baseline <name>` stores the statistics under `<package>/target/burdin/bench/`, and
  `--baseline <name>` compares against them, calling a change beyond noise when the medians differ by more than
  three times the larger spread; `--fail-on-regression <percent>` fails the run for benches that regressed that
  far, for CI.
- Profiler hook (#655): `--profile-time <seconds>` runs one selected bench in a loop, with no clock, for an
  external profiler.
- `burdin fmt` lays out bench items, `tests/s3_bench/` adds seven conformance tests with the `bench-compile-fail`
  and `burdin-bench` directives, `tests/check_bench.py` checks the command black-box in every CI job, and the
  driver, runner, and parser unit tests cover the statistics, the baseline files, the result line, and the parser.

### Doc tests (issues #250, #650-#652)

- `burdin test` runs the `///` and `//!` code blocks of the selected packages' doc comments with the `test` blocks;
  `burdin test --doc` runs only them (#650). Every Burdin fenced block — untagged, or tagged `burdin` or `bd` — is
  built as its own one-file package that depends on the documented package, so an example sees its public API only.
  An example without `fn main` is wrapped in one, its leading declarations (the longest prefix that parses as items)
  are hoisted above the wrapper, and padding comment lines put every line on the line of its doc comment, so a panic
  reports the line a reader sees. A failure is reported at the doc comment's file and line: the compile's
  diagnostics are rewritten from the generated program to the original file before they are shown, and the headings
  read `---- doc test "<owner> (line N)" at <file>:<line>: <reason> ----`. The doc tests selected run in parallel
  under `-j`; a one-file target is copied into a package so its examples can import it, and a package with no
  importable root module gets one note and still runs its examples.
- The fence's info string carries the block's attributes (#651): `ignore` (not built), `no_run` (compiled, not run),
  `should_panic`, and `compile_fail` / `compile_fail E0123` (must not compile, with that code when named); an unknown
  word warns at the fence's line and the block still runs. `--list` prints them in source syntax, and the JSON
  listing and event stream carry `ignore`, `should_panic`, `no_run`, `compile_fail`, and `compile_fail_code`.
- A line that begins with `# ` (or is only `#`) inside a Burdin block is setup (#652): `burdin test` compiles it with
  the marker removed, and `burdin doc` does not show it (a block that is entirely hidden shows nothing). The marker
  is in `spec/doc.md` DC6; `burdin fmt` leaves a block with hidden lines as written and now recognizes attribute
  fences (`burdin,no_run`) as Burdin.
- The `tests/doc/geometry` fixture's example now runs: it hides its `use` line and constructs points with the new
  `Point.new`. `tests/check_test_runner.py` covers the fixtures in `tests/runner/doctests/`,
  `tests/runner/doctests-bin/`, and `tests/runner/doctest_one.bd`; `spec/test-runner.md` has the "Doc tests" section.

### The REPL (issue #578, sub-issues #580, #582, #584)

- `burdin repl` (new crate `burdin_repl`, `spec/repl.md` RP1-RP15) is a session built on the comptime interpreter
  and the query library: inputs are items, statements, or expressions, the session is one growing program — `main`'s
  body for statements, module content below it for items — so every diagnostic is the one `burdin build` reports and
  moves and borrows persist across lines with the same E-codes. An expression's value prints with `Debug` through
  the standard library's own formatter, interpreted, so the output is byte-for-byte the compiled program's. A failed
  input is discarded and the bindings restored.
- Meta-commands (#584): `:type` and `:ownership` analyze a throwaway probe and evaluate nothing; `:explain` shares
  the reference text with `burdin explain`; `:load` appends a file's module content atomically; `:reset` runs the
  bindings' drops; `:ir` and `:compile` come from the driver, the two operations that need the back end.
- Interactive mode (#582), only when standard input is a terminal: a raw-mode line editor (`libc` termios on Unix,
  the console API on Windows; no new dependency), history in the Burdin home, highlighting with the editor
  grammar's token classes, Tab completion through the query library, and multi-line input decided by the parser.
  A piped standard input disables all of it, so scripted sessions are deterministic.

### Single-file scripts (issue #586)

- A one-file target whose first line is `#!` or whose leading `//!` doc block declares `[dependencies]` is a script
  (`spec/scripts.md` SC1-SC7): the lexer skips the shebang at byte 0 (and only there; elsewhere it is the new
  E0300), and the loader resolves the fragment into a hidden in-memory package, so `build`, `check`, and `test`
  see the dependencies too.
- `burdin run script.bd` caches its binary by content under `<burdin home>/cache/scripts/<key>/`: the script text,
  the dependency closure's manifests and sources, the compiler's identity, and the flags that change a build. An
  unchanged script runs from the cache; changing it or a dependency rebuilds. A plain one-file target is unchanged.

### Parallel compilation and `-j` (issues #334, #352, #773)

- Code generation is parallel: after lowering, unused runtime and glue functions are dropped, and a program of more
  than about 8,000 LLVM instructions is split into up to 16 codegen units, each optimized and compiled to its own
  object on its own thread, then linked in order (`burdin_codegen/src/split.rs`). The unit count depends only on the
  program, so the executable does not depend on the thread count. A release build keeps small functions inlinable
  across units. On a generated 16-module package a debug build went from 0.63 s to 0.46 s and a release build from
  1.40 s to 0.62 s; hello world's debug executable shrank from 171 KB to 24 KB. `BURDIN_CODEGEN_UNITS` forces a
  split for testing.
- `-j <n>`/`--jobs <n>` (also `-jN`) is an option of `build`, `run`, `test`, `check`, `lint`, `doc`, and `fmt`: the
  most threads the command uses for parsing, code generation, and formatting. `BURDIN_JOBS` is its default.
- `burdin fmt` formats files on several threads with output, written files, and exit status independent of `-j`,
  and the benchmarks have `fmt/SIZE` cases and a smoke budget for the formatter.
- `tests/check_parallel.py` checks the determinism across `-j`, a forced split over the code-generation-heavy
  milestones, `-j` on every command, and `fmt -j`.

### The language server (issue #242, sub-issues #309-#313, #365-#376, #470, #662)

- `burdin lsp` is a Language Server Protocol 3.17 server over standard input and output (new crate `burdin_lsp`,
  `spec/lsp.md` LS1-LS20), answered from the query-based library, so an editor sees the command line's diagnostics,
  types, and names. Decision (#365): Rust, no new dependency (a small JSON value and `Content-Length` framing),
  stdio only, one thread owning the databases plus a reader thread for cancellation and worker threads for test runs.
  The crate's `burdin-lsp` binary is the same server without LLVM, for the integration tests.
- Lifecycle and document sync (#309): initialize/shutdown/exit with the protocol's exit codes, incremental changes,
  errors for requests out of order, malformed input survived. Positions in UTF-16 or, when the client offers it, UTF-8
  (#373). `$/cancelRequest` answers `-32800` and stops the analysis a request waits on; `$/progress` for loading,
  analyzing, and test runs.
- Diagnostics (#310): debounced, published once for the latest version, never for an analysis an edit overtook; every
  error of a file with its code, span, error-reference link, and related locations; `burdin lint` diagnostics with the
  manifest's levels and the `Unnecessary` tag.
- Navigation (#311, #370, #470): definition (into a materialized copy of the standard library too), references,
  highlights with writes, rename across files with shorthand fields kept, go to implementation, call and type
  hierarchies. Editing (#312, #313, #368, #369): hover with signatures and docs, member/scope/`use` completion (also on
  a line that does not parse yet), signature help, inlay hints for `let` types and parameter names (each toggleable),
  semantic tokens, and formatting through the `burdin fmt` library.
- Outline (#367, #371): nested or flat document symbols, fuzzy workspace symbols, folding, selection ranges, and links
  for `use` paths, URLs, and doc-comment `[name]` links. Code actions (#366): import an unknown name, make a binding
  `mut`, add the `mut` marker, add an implementation's missing methods, write a `let`'s type, organize imports.
- Lenses (#374, #662): run a test, a file's tests, or `main`, each carrying the exact `burdin test <root> --exact <path>`
  or `burdin run` command line; test runs happen in the server with `--format json` and their results come back as
  lenses; `Debug` lenses carry a DAP `launch` configuration for `burdin dap` (#576).
- Workspaces (#372, #376): one database per package, multi-root folders added and removed, watched files, pushed and
  pulled configuration; an edit re-checks only the bodies it can affect, so a dependency's bodies are reused.
- Editors: `editors/vscode` (extension, TextMate grammar whose keywords are the lexer's, language configuration),
  `editors/zed` (extension: the `.bd` language and `burdin lsp` from `PATH`), `editors/tree-sitter-burdin` (a lexical
  tree-sitter grammar and highlight queries, used by Zed, with corpus and highlight tests), and the Neovim and Helix
  setup in `editors/README.md`, with the manual checklist in `spec/lsp.md`.
- The query library gained `Analysis::occurrences`, `locals`, `calls`, `functions`, and `tests`, and `Db::modules` and
  `Db::packages` (`crates/burdin_query/src/navigate.rs`, `spec/query.md`).
- Tests: `crates/burdin_lsp/tests/` (lifecycle, diagnostics, navigation, editing, outline, actions, workspace; a
  JSON-RPC client in `tests/common/mod.rs`, fixtures, and golden files), the crate's unit tests, and
  `tests/check_lsp.py` (the driver's `burdin lsp`, a real test run through a lens, and the editor files).

### The query-based compiler library (issue #241, sub-issues #645-#649)

- The front end is a library with a memo (new crate `burdin_query`, `spec/query.md`): file texts in, diagnostics and
  the typed program out, with everything an edit cannot affect reused. `burdin build`, `run`, `test`, and `lint` run
  their front end through it (`burdin_driver::front_end`), so the command line and a tool share one implementation.
- The engine is an in-house memo, not salsa (QL1): the typed tree and the checker's registries are `Rc`/`RefCell`
  graphs, and salsa's storage wants `Send + Sync` values; the memo is a table keyed by 128-bit fingerprints.
- Granularity (QL2): a file's parse is keyed by its text, and a function or constant body is keyed by the text of its
  declaration span, the declaration fingerprint (every file's text with the parser-recorded body spans elided), and
  the pass chain. A body edit rechecks that body; a declaration edit rechecks every body. The memory checks and
  compile-time evaluation are whole-program passes on every analysis (QL3).
- An edit inside one function body reparses only that body (QL6): the parser records every body span, the new block
  is parsed alone, and the tree's spans after the edit are moved. `main`, `select`, a file with syntax errors, and an
  edit that is not strictly inside one body fall back to a full parse.
- The tool API (QL7) is plain data: a lossless syntax tree with tokens and trivia, the diagnostics of one file, and
  `resolve_at`, `type_at`, `signature_of`, and `ownership_of` at a byte offset. `spec/query.md` is the behavior, with
  a golden file of answers for the fixture package.
- Cancellation and the budget (QL8): `CancelHandle` stops an analysis from another thread and leaves the database
  usable; `Budget { bodies }` (40,000 by default) evicts the least recently used checked bodies, and `Options::no_cache`
  turns the memo off for a one-shot build.
- Parity (QL4): the answer of an edited database equals a fresh one (`tests/equivalence.rs`, random edits over the
  conformance corpus) and the library reports the command line's error diagnostics for every conformance test that
  must fail to compile (`crates/burdin_driver/tests/parity.rs`).
- Counters (QL5) make the work visible: `Db::stats()` per analysis and `Db::counters()` in total. The benchmark gains
  the query cases (QL9): `cargo bench -p burdin_bench -- query` measures a keystroke edit and a cold analysis on a
  generated package of one, two, and four modules, and `--smoke` fails unless a keystroke rechecks exactly one body.
- Tests: `crates/burdin_query/tests/` (`incremental.rs`, `reparse.rs`, `equivalence.rs`, `api.rs` with
  `fixtures/api.golden`, `stress.rs`, `memory.rs` with a counting allocator), the driver's `parity.rs`, and the new
  unit tests of `db.rs` and `spans.rs`.
### Machine-readable diagnostics (issue #236)

- The JSON format is frozen as version 1 (issue #642, `spec/diagnostics.md` DG2-DG7). Every object carries
  `"version": 1`. A diagnostic has its `code`, `severity`, `message`, `url`, primary `span`, `labels`, `notes`, `help`,
  `related` (the notes that have a span, shaped like the Language Server Protocol's `relatedInformation`), `children`,
  `suggestions` (each with `applicability` and a list of `edits`), and the `rendered` human text. A location has line
  and column in Unicode scalar values, UTF-8 bytes, and UTF-16 code units, and byte offsets. The schema is
  `docs/diagnostic.schema.json` (closed, so any change to the output changes it) and `docs/diagnostics.md` documents
  the fields and the versioning rule. `tests/check_diagnostics.py` validates every line printed for every failing
  conformance test against the schema, checks that mutated objects are rejected, and compares goldens in
  `tests/diagnostics/`. The model gained `labels`, `help`, and `children`; the human format is unchanged.
- `--diagnostic-format sarif` writes one SARIF 2.1.0 log (issue #643, DG8-DG11): the error-code registry and the lints
  as rules with a link to each entry, results with locations, related locations, and fixes, and `invocations` saying
  whether the run succeeded. `burdin build` and `burdin lint` write the log even when nothing was found; `run` and
  `test` write one only when the compilation reported something; `burdin fmt` refuses the format. A subset schema in
  `tests/diagnostics/sarif-subset.schema.json` and a golden result list prove the shape.
- Every diagnostic links to its entry in the hosted error reference (issue #644, DG12-DG14):
  `https://ricardomilos7.github.io/burdin/errors/E0003` (`/lints/L0007` for a lint), a scheme that ends in the code
  and is pinned by `tests/check_error_docs.py`. It is the `url` of JSON, the `helpUri` of SARIF, a line printed by
  `burdin explain`, and an OSC 8 hyperlink on the code in colored human output when the terminal supports it
  (`FORCE_HYPERLINK=1` or `0` decides). The short format stays one plain line.

### `burdin test` filters, `--list`, reports, reruns, and shards (issue #235)

- **Filters (#638).** `burdin test <target> [<filter>...]` runs the tests whose full path (`module.submodule.test name`,
  prefixed `<package>::` when several packages run) contains any filter; `--filter` is the same thing. `--exact`
  requires an equal path, `--skip <pattern>` (repeatable) leaves tests out, and the summary appends
  `, N filtered out`. `TestEntry` now carries the module path and the span of the test's name.
- **Listing (#639).** `burdin test --list` prints each selected test with its attributes and a count, without running
  anything; `--list --format json` prints one object per test with package, module, name, path, file, line, and
  attributes, for editors and CI.
- **Reports (#640).** `--format json` streams `suite_started`, `test_started`, `test_passed`/`test_failed` (with
  duration, reason, and captured output), `test_ignored`, and `suite_finished` events, one JSON object per line, with a
  versioned schema in `spec/test-runner.md`. `--junit <file>` writes a JUnit XML report. The human output does not
  change.
- **Reruns, shards, and slow tests (#641).** `--failed` reruns the tests that failed in the previous run, kept in
  `<package>/target/burdin/<profile>/test-failed.txt`; `--shard <i>/<n>` runs a deterministic round-robin slice; tests
  that ran at least `--slow-threshold` milliseconds (default 5000) are listed before the summary.
- The runner is split into `test_runner.rs` (compile, execute), `test_runner/select.rs` (selection and the failed
  record), and `test_runner/report.rs` (human, list, JSON, and JUnit output). `tests/check_test_runner.py` gains about
  150 black-box checks over the fixtures `tests/runner/selection/` and `tests/runner/slow/`, and the driver has unit
  tests for each rule.

### Tooling: `burdin check` (issue #237)

- `burdin check <file.bd | package directory>` type-checks without building: it runs the front end of `burdin build`
  (build scripts, parsing, name resolution and type checking, the return-origin, move, and borrow checks, and
  compile-time evaluation) and stops before code generation, so it needs no linker and writes no file. It is for fast CI
  gates and editor feedback (`spec/check.md`, CK1-CK8).
- The diagnostics, formats, and exit statuses are those of a build: a clean check prints nothing and exits 0, errors exit
  1 and end with `error: could not check <target> (N errors)`, and a usage error exits 2. `--tests` also checks `test`
  blocks and accepts a library package without `src/main.bd`; `--release` and `--comptime-steps` are as for a build;
  the options that only matter for an output (`-o`, `--emit-ir`, `-g`, `--lto`, `--link-lib`, ...) are rejected.
- `burdin check` reuses `front_end`, the shared front end that `build`, `run`, `test`, and `lint` also call; that
  front end runs through the query-based compiler library (the entry above).
- The error reporting of a failed compilation moved out of `compile` into `report_errors`, and the diagnostics options
  of the command line into `parse_diagnostic_option`, so the commands share them.
- `crates/burdin_driver/src/check.rs` has unit tests for the command line; `tests/check_check.py` black-boxes the
  command: the fixtures and golden outputs under `tests/check/`, equality with `burdin build`'s diagnostics, every
  conformance test (a `compile-fail` program must report exactly its markers, every other program must be accepted),
  no linker needed and nothing written, and the command line. It runs in every CI job.

### C bindgen (issue #244)

- `burdin bindgen <header> --out <file>` reads a C header through **libclang**, which the new `burdin_bindgen` crate
  loads at run time (`dlopen`/`dlsym`, `LoadLibraryA`/`GetProcAddress`), so no libclang is linked into the compiler and
  every other command works without it (issues #245, #246). `-I`, `-D`, and `--clang-arg` reach the C front end, so
  `#include`, the preprocessor's conditional state, and the system headers behave as they do for the C compiler
  (issue #245). `BURDIN_LIBCLANG` and `LIBCLANG_PATH` override where it is found; a missing library is exit 2 with
  every place that was tried.
- Types: the fixed-width and pointer-sized names become their Burdin primitive, `char` is `CChar`, `void *` is
  `CPtr<CVoid>`, and a typedef of the header keeps its name. A struct used by value is declared in full, one reached
  only through a pointer in an included header is declared with no fields, and a type with no Burdin equivalent skips
  the declaration that needs it (issues #246, #247).
- Structs and unions (issue #247): a struct whose layout Burdin can spell becomes a `repr(C)` struct with
  `comptime assert`s for its size and alignment against the C header; a union, a packed struct, a bit-field, a flexible
  array member, an anonymous member, or a field at an offset Burdin would not choose becomes a byte blob of C's size
  and alignment with its members listed, and an alignment with no Burdin integer makes the struct empty
  (issue #476). Enumerations become an alias of the smallest of `i32`, `u32`, `i64`, and `u64` that holds every
  enumerator, so the same header generates the same module on every platform while the layout still matches C.
- Function pointers are `CPtr<CVoid>` with a note, since Burdin has no function-pointer type across the C ABI, and a
  variadic function keeps its `...` (issue #248). A parameter declared as an array is a pointer to its element. On
  macOS the SDK that `xcrun --show-sdk-path` reports is passed as `-isysroot` unless the caller gives one, because a
  Homebrew libclang has no sysroot of its own and would not find the system headers. The directory of clang's
  built-in headers is passed as `-resource-dir`, because libclang derives it from where its shared library sits and
  a distribution that splits the library out of the LLVM tree (Ubuntu's multiarch directory) otherwise loses
  `<stddef.h>`.
- Object-like macros whose body is a constant expression are evaluated with C's rules (suffixes, bases, character and
  string literals, casts, arithmetic, comparisons, `?:`, names resolved to other macros or enumerators) and become
  `const`s; one that is not a constant is skipped with a note, and function-like macros are counted in one note
  (issue #475).
- `--wrappers <file.c>` writes C that calls each `static inline` function and exports it as `<name>__extern`, because
  it has no symbol; `--wrapper-include` sets the `#include` line (issue #475). `--link-lib` now accepts a file path
  (an object, static archive, or shared library) so that wrapper compiles into the program.
- Every skipped or changed declaration is named with its line and reason in a `// bindgen:` comment and on stderr;
  the command exits 0 when the header parsed, 1 when clang rejects it, and 2 for a usage or I/O error.
- `burdin bindgen --export <target> --out <header.h> [--guard NAME] [diagnostics]` writes the other direction (issue #663): a header
  with a prototype for every `extern "C" fn` the package exports, the `repr(C)` structs they reach, opaque
  declarations for the ones only used through pointers, and the public constants as `#define`s, in an include guard
  and an `extern "C"` block for C++. Its diagnostics take the `[diagnostics]` group every command takes, so E0263 is
  available in the JSON and SARIF formats too. A declaration a C header cannot spell is the new error **E0263** (every one is
  reported, and no header is written); on Linux the driver already links with `-rdynamic`, so a C shared library rings
  the exported functions through the header.
- Tests: `tests/check_bindgen.py` in every CI job compares the golden modules in `tests/bindgen/expected`, compiles
  and runs each generated module (its `comptime assert`s are the layout check), builds the wrappers with the system C
  compiler and calls them, compiles a C and a C++ program against the export header, and on Linux builds a C shared
  library that calls the Burdin exports and checks what it prints; `tests/bindgen/export_invalid` covers E0263, and
  `tests/m22_build/bindgen` invokes the tool from a build script. Linux CI installs `libclang-21-dev`. `spec/bindgen.md`
  (BG1-BG11) is the behavior.

### Tooling: `burdin new` (issue #228)

- `burdin new <name>` creates the package directory `<name>` with `burdin.toml` (`[package]` with the name and version
  `0.1.0`), `src/main.bd` (a hello-world `main`), and a `.gitignore` covering `target/` and the executable
  `burdin build .` leaves in the package directory. The package name is the last component of the path and must be an
  identifier that is not a reserved word and is not `std`; missing parent directories are created.
- It never overwrites: an existing directory (empty or not), file, or link is refused with exit status 2 before anything
  is written, and a failure while writing removes the directory it created. Usage errors and I/O failures also exit 2.
- `crates/burdin_driver/src/new.rs` has unit tests for the files, names, nesting, and refusals; `tests/check_new.py`
  black-boxes the command (golden contents, the new package builds, runs, and tests, refusals leave existing files
  untouched, invalid names create nothing, the command line) and runs in every CI job.

### `burdin fmt` (issue #229)

- `burdin fmt` formats Burdin source in one opinionated style with no options (`spec/fmt.md`): four-space indentation,
  lines of at most 120 columns, a magic trailing comma that keeps a list one element per line, and blank lines kept (at
  most one in a row). It runs on files, directories, and packages, rewrites only the files that change, and is
  idempotent. `--check` lists the files that are not formatted and exits 1, for CI (issues #229, #463).
- Comments are kept in place: own-line comments stay on their own lines, trailing comments stay at the end of their
  line, and a comment before a closing bracket stays inside it. A comment the grammar gives no line for moves to the end
  of the line; none is dropped or merged (issue #460).
- Breaking follows the width: a bracketed list breaks one element per line with a trailing comma, the last closure or
  literal argument of a call stays on its line, binary operators break before `&&`, `||`, and `orelse` and after the
  others, and a method chain of two calls or more breaks before each `.` (issue #461).
- `use` declarations are merged where the meaning is the same, sorted, and grouped into the standard library, the
  dependencies in `burdin.toml`, and the package, with comments kept; a plain `use a.b` is never merged into
  `use a.{b}` because it may name a module (issue #659).
- The code in the fenced blocks of `///` comments (an empty fence, `burdin`, or `bd`) is formatted as items or as the
  statements of a function; a block tagged with another word, or that does not parse, is left alone (issue #660).
- `--stdin` formats standard input to standard output for editors, `--stdin-filepath` names it and finds its package,
  and `--range lo:hi` formats only the top-level items that overlap a byte range (issue #463).
- The formatter checks itself: its output must parse to the same syntax tree with the same comments, and is laid out
  again until it stops changing, or the file is left alone and the failure reported (issue #462).
- The new crate `burdin_fmt` depends only on `burdin_diag` and `burdin_syntax`; the look-ahead rules the formatter
  shares with the parser moved to `burdin_syntax::lookahead`.
- Tests: `crates/burdin_fmt` (unit tests, `tests/corpus.rs` over every `.bd` file of `std/` and `tests/`,
  `tests/fuzz.rs` with 20,000 mutations in every CI job and a long session in the scheduled `fuzz` job, and
  `tests/regressions.rs`), and `tests/check_fmt.py` with the fixtures in `tests/fmt/`, which every CI job runs.

### `burdin lint` (issue #230)

- `burdin lint [path]` is a linter that is separate from the compiler (new crate `burdin_lint`, called only by the
  driver). It checks the program the compiler already accepted and reports lints `L0001`-`L0017` (plus `L0000` for an
  unknown lint name in a configuration or a suppression comment); `docs/lints.md` documents each one with an example
  that `tests/check_lint.py` runs, and `spec/lint.md` (LN1-LN32) is the behavior.
- Naming conventions: `non_snake_case`, `non_pascal_case`, `non_screaming_case`, with the suggested name in a note
  (issue #233). Syntax-tree lints: `else_on_local_enum` (an `else`/`_` arm that hides a new variant of an enum of the
  same package, with the explicit-variants form as the way to ignore it; issue #231), `unused_imports`,
  `unused_variables`, `unused_parameters`, `unreachable_code`, and `dead_code` for `priv` items (issue #465).
- Typed-tree lints: `eager_chain` (a long chain of eager list methods that builds a list per step suggests `.iter()`;
  issue #232), `redundant_clone`, `unused_mut`, `large_copy_in_loop`, `borrow_could_own`, `loop_invariant_alloc`, and
  `needless_to_list` (issue #661).
- Levels are `allow`, `warn`, and `deny`, set per lint or for the `all` and `warnings` groups, in the manifest `[lint]`
  table or on the command line, with `// lint: allow(...)` comments above an item, trailing on a line, or for the whole
  file with `allow-file` (issue #464). A denied lint makes the exit code 1.
- `burdin lint --fix` applies the machine-applicable suggestions in rounds, rechecks that the result still compiles,
  previews the unified diff (`--dry-run` stops there), and writes every file or none (issue #466).
- `tests/check_lint.py` runs the documented examples, the fixtures in `tests/lint/` (naming, `else`, eager chains,
  unused code, configuration, ownership, generated code, generic instances, `--fix` before and after, and the
  package fixtures), the all-or-nothing write, and the manifest and command-line checks; every CI job runs it, and
  `docs/lints.md` ships in the release archive.
- Two small changes to the shared crates support it: a typed `Local` carries its source span, and a diagnostic's
  suggestion can be marked machine-applicable (`Diagnostic::suggest_safe`).

### `burdin doc` (issues #234, #467-#469, #656-#658)

- `burdin doc [path]` writes the documented interface of a package from its `///` comments and `//!` module comments
  (new crate `burdin_doc`, called only by the driver): a self-contained static HTML site (default
  `<package>/target/doc`), versioned JSON (`--output-format json`), or a per-module coverage report (`--coverage`).
  `spec/doc.md` (DC1-DC23) is the behavior, and doc tests remain separate work (issues #250, #650-#652).
- The site has an overview, a page per module, a page per item, a sidebar, cross-links from signatures and doc comments,
  and client-side search. Its stylesheet and search script are compiled into the binary and copied under `static/`, and
  the index is a script (`search-index.js`), so the site loads nothing from another server and works from disk
  (issue #467).
- Doc comments are a Markdown subset (headings, lists, tables, quotes, fenced code, emphasis, links). Text is always
  escaped; a `javascript:` or other unsafe address is dropped. Intra-doc links `[Type]`, `[module.fn]`,
  ``[`Type::method`]``, and the explicit `[text]`-with-a-target and `[text][path]` forms resolve like code paths from a
  comment's module; an
  unresolved link is `D0001` and a link to a `priv` item is `D0002`, both warnings that `--deny-warnings` turns into a
  failure (issue #468).
- `priv` items, fields, and methods are left out by default and shown, marked, with `--private`; `pub use` re-exports
  are listed on the module page and "re-exported by" on the item (issue #469).
- A type's page lists its methods, its operators written as they are written (`a + b`, `-a`), its extension methods
  with the module that declares them and the `use` a caller needs, its derived and implemented interfaces, and the
  methods of impl blocks for a specific instance (`Stack<i32>`) (issue #658).
- The JSON output carries `format_version: 1`; the schema has modules, items, members, implementations, link
  resolutions, spans, and member origins (issue #656).
- `--coverage` prints documented and undocumented items per module, and the `missing_docs` lint (`L0017`, allow by
  default) reports the same items from the same reading of the source (issue #657).
- The output directory is guarded: `burdin doc` refuses a directory that holds files it did not write (status 2), keeps
  a `.burdin-doc` marker, and removes the pages of items that are gone. A package that does not compile is reported and
  not documented (status 1).
- Unit tests in `burdin_syntax::docs` and `burdin_doc`, the fixtures in `tests/doc/` with golden pages in
  `tests/doc/expect/`, and `tests/check_doc.py`, which every CI job runs.

## Stage 2 — 0.1.0 (2026-11-19)

### Milestone 2.4: asynchronous I/O (issue #220)

- Sockets, file operations, name resolution, and `blocking` extern calls look blocking but park only the calling
  task: its worker runs other tasks while the system works. On the main thread and on `thread.spawn` threads the
  same calls block the thread. There is no `async`/`await` and no second API (`spec/io.md` IO1).
- The reactor is one interface with three implementations: `epoll` on Linux, `kqueue` on macOS, and an I/O completion
  port with `WSAPoll` on Windows. A reactor thread waits for socket readiness or the nearest deadline and wakes the
  waiting task; the event facility, its wakeup, and a small helper pool are compiler runtime primitives
  (`crates/burdin_codegen/src/taskrt/reactor_*.ll`), and the timers and scheduler integration are Burdin
  (`std/src/reactor.bd`). A wait is claimed exactly once, and each descriptor registration is one-shot (issue #222;
  IO2).
- File and directory operations run on the helper pool, so a blocking disk call does not hold a worker. The pool is
  wrapped as `__offload`, which is sound without a `Send` bound because the call does not return before the helper
  is done (issue #224; IO3).
- `extern "C" { blocking fn ... }` marks a C function that may block: the compiler marshals its arguments, generates
  a thunk, and runs it on the helper pool. A `blocking` marker outside an `extern` block, on a variadic declaration,
  or contradicting another declaration of the same symbol is E0290 (issue #223; IO4).
- `std.net` adds `TcpListener`, `TcpStream`, `UdpSocket`, and `SocketAddr`, with blocking-looking `accept`, `read`,
  `write`, `recv_from`, and `send_to`; `TcpStream` implements `Read` and `Write`; read and write timeouts are
  deadlines on the reactor's timer list; `lookup_host` resolves names on the helper pool. Errors are
  `IoError.Net`, and `IoErrorKind` gains the connection, address, and reachability kinds (issues #408, #225; IO5-IO6).
- `time.sleep` inside a task parks on the reactor's timer instead of holding its worker (IO7).
- `tests/s2_io` has 21 tests, including a TCP echo server, UDP, DNS, timeouts, a FIFO that would deadlock without
  the helper pool, and two stress tests that every CI job repeats 20 times in both profiles. `spec/io.md` records
  the decisions IO1-IO8.

### Milestone 2.3: shared state (issue #213)

- `Shared<T>` counts its handles atomically in every program: relaxed increment on `clone`, release decrement on drop,
  and an acquire fence before the last handle frees the value (issue #215; `spec/shared.md` SH1-SH3).
- `std.atomic` adds `AtomicBool`, `AtomicI32`, `AtomicU32`, `AtomicI64`, `AtomicU64`, `AtomicIsize`, and `AtomicUsize`
  with `load`, `store`, `swap`, `compare_exchange`, and the `fetch_*` operations, all sequentially consistent, over new
  compiler builtins. The module is generated by `tests/gen_atomic_std.py` (issue #217; SH7).
- `Mutex<T>` and `RwLock<T>` (`std/src/sync.bd`) guard a value through closures: `m.lock(|v| ...)`, `try_lock`, `read`,
  `write`, `try_read`, `try_write`, `get_mut`, and `into_inner`. A waiting task parks and a waiting thread blocks, a
  waiting writer holds back new readers, and nothing is poisoned because a panic aborts the process (issues #216, #407;
  SH8-SH10).
- `Send` and `Sync` (`std.marker`) are derived from a type's parts and required by `task.spawn`, `thread.spawn`,
  `Scope.spawn`, a task's result, and `Channel<T>`. A value that is not `Send` is E0280, whose message follows the chain
  down to the field or capture that decides it; `unsafe Type: Send { }` declares one by hand, and a marker impl that is
  not `unsafe`, or an `unsafe` impl of another interface, is E0281 (issues #214, #218, #406; SH4-SH6).
- A task's closure carries whether it is `Send`; a raw-pointer dereference view now takes the origin of the storage that
  holds the pointer, so a handle type can return `&T` from a method.
- `tests/s2_shared` has 26 tests (atomics, mutex and read-write lock behavior, waiting with one worker, derived and
  declared `Send`/`Sync`, the E0280 and E0281 errors, leak checks, and stress tests). Every CI job repeats the stress
  tests 20 times in both profiles, which `tests/check_ci.py` checks (issue #219).

### Milestone 2.2: channels and `select` (issue #207)

- `Channel<T>` (`std/src/channel.bd`) is a cloneable handle to a bounded (`Channel<T>.bounded(n)`, `n >= 1`) or unbounded
  (`.unbounded()`) multi-producer, multi-consumer queue. `send` moves a value in and waits while a bounded channel is
  full; `recv` waits while it is empty and returns `None` once it is closed and empty; `try_send` and `try_recv` never
  wait; `close` wakes every waiter; `for v in ch` receives until the close. A `send` on a closed channel hands the value
  back. A task that waits parks and a thread blocks, so channels connect tasks, threads, and the main thread
  (issues #208, #209; `spec/channels.md` CH1-CH5).
- `select { v = rx.recv() => ..., tx.send(x) => ..., default => ... }` waits on several channel operations and runs
  exactly one arm. It is a contextual keyword that the parser rewrites into ordinary code over a few members of
  `Channel`, so `break`, `continue`, `return`, and `?` in an arm mean what they mean elsewhere. The first ready arm in
  source order runs, `default` makes the `select` non-blocking, and an arm guard (`if cond`) switches an arm off, which
  is how a loop stops selecting on a closed channel (issues #208, #210; CH6-CH8).
- A sent value is moved, the channel owns it until it is received, and a value still queued when the last handle goes
  is dropped with it; `Channel<&T>` is E0120 and `ch.send(&x)` is E0003. Using a sent value is E0100 (issue #211; CH4).
- All channel state lives under the scheduler's lock, so a wake-up cannot be lost: a waiter registers before it gives up
  the lock, and a `select` reads a change counter before it looks at its arms (CH10).
- `tests/s2_channels` has 36 tests (producer/consumer, close, bounded and unbounded, `select` with two ready arms,
  `default`, guards, the move and borrow errors, and the stress tests). `tests/run_tests.py --repeat N` runs each test
  N times, and every CI job repeats the channel stress tests 20 times in both profiles, which `tests/check_ci.py` checks
  (issue #212).

### Milestone 2.1: tasks and the scheduler (issue #198)

- `task.spawn(|| ...)` runs a closure as a task on a work-stealing pool of worker threads and returns a `JoinHandle<T>`
  (`join`, `is_finished`; dropping an unjoined handle detaches the task). A captured value moves into its task
  (issues #199, #200).
- The scheduler is Burdin code in `std/src/task.bd`: per-worker queues and parking, stealing one task from the head of
  another worker's queue, wake-ups that cannot be lost, a pool started on first use and sized by `task.set_workers`
  (issues #201, #301-#303). `tests/s2_tasks` has stress, fairness, one-worker, and idle-wake-up tests that run
  repeatedly in CI (issue #304).
- Tasks run on their own 256 KiB stacks with a 64 KiB guard region; running off the end is a clean
  `panic: stack overflow` (issue #202). `task.yield()` puts the caller behind the other ready tasks (issue #203).
- `task.scope(|s| { s.spawn(|| ...) })` starts tasks that may borrow places of the surrounding code and waits for them.
  The exclusivity checker keeps what a task borrows unavailable until the scope body ends: two tasks writing one place,
  or the body touching a borrowed place, is E0111; a task borrowing a local of the body, or a borrowing closure passed
  by name, is E0171 (issue #204; `spec/memory-rules.md` SK1-SK3).
- A panic in a task aborts the process (issue #205). `thread.spawn` runs a closure on an operating-system thread
  (`Thread<T>`: `join`, `is_finished`) (issue #405). `spec/tasks.md` records the decisions TK1-TK12, including that
  the derived `Send` marker waits for milestone 2.3.
- The runtime is thread-safe where tasks reach it: output is written under a lock, the checking allocator's live list
  is locked, and a task-stack overflow is recognized by the signal handler. The task primitives are LLVM IR text linked
  in only by a program that uses them (`crates/burdin_codegen/src/taskrt/`). Code generation emits a closure body only
  when a reachable body creates it, which also shrinks every program that does not use tasks.
- Written for Linux, macOS, and Windows on x86-64 and arm64; run so far only on Linux x86-64 (debug and release).

## Infrastructure — 0.1.0 (2026-11-19)

- The Windows stack-overflow guard reserves a 64 KiB stack guarantee (`SetThreadStackGuarantee`) before it registers
  its vectored filter. The system's default left the filter only a few kilobytes once the exception dispatcher had
  run, so the handler could fault before printing `panic: stack overflow`, which made
  `tests/m02_basics/stack_overflow_aborts.bd` flaky on the `windows` CI job; the guard now has the same room the
  Unix alternate signal stack gives its handler.
- Mutable iteration over `List`, arrays, `Map` values, and `Deque` (issue #709).
  - `for x in mut xs` and `xs.iter_mut()` yield `&mut T` (a map yields `(&K, &mut V)`): `*x = ...` rewrites each
    element in place. The collection is borrowed exclusively for the loop, so pushing, inserting, reading it, or
    starting a second mutable loop inside the body is E0111; the collection must be a mutable place (E0113).
    `Deque` declares `iter_mut` in `std`; a user type joins by declaring `iter_mut(mut self)`.
  - The mutable iterators are ordinary iterators, so `enumerate`, `take`, `skip`, and the other adapters apply.
    `zip` and `chain` work over them; `rev` does not (the mutable iterators are not double-ended), and slices
    have no mutable form.
  - The borrow checker now records the match binding and the target of `held = x` as holders of the loans they
    copy, so a view copied out of a loop, or out of `get_mut`, keeps the collection borrowed (E0111).
  - A mutable view `&mut X` satisfies the interface bounds `X` does, like a shared one.
  - The rules are `spec/iterators.md` and `spec/memory-rules.md` IT15; the tests are `tests/m24_stdlib/iter_mut_*`.
- Allocator follow-up (sub-issues #331-#333 of #182): the interface and metadata decision, the global libc allocator,
  and drop/clone glue through a stored allocator are verified end to end.
  - `List.contains`, `Map.get`, `Map.remove`, and `Map.contains_key` borrow their key, as `spec/stdlib.md` says,
    but the inlined implementation spilled a temporary key without recording it for drop, so a temporary `String`
    key leaked. The key is now read through a place: a local stays borrowed, and a temporary drops at the end of
    its statement.
  - `tests/m23_allocators/clone_through_allocator.bd` checks list, map, heap, and `String` clone and drop, plus
    temporary lookup keys, under the checking allocator.
- Differential tests between comptime and compiled code (issue #444).
  - A test item may carry `[differential]`: the compiler runs its body through the comptime interpreter after the
    CT13 rewrite, and `burdin test` requires the compiled test to produce the same `Ok`/`Err` outcome. A different
    outcome, or a compiled run that aborts, fails the test with a note naming both outcomes. Agreement passes even
    when both runs return `Err`; this stage compares only the outcome, because the dynamic error type carries no
    comparable value.
  - The body must be one the interpreter can execute: an impure operation is E0224, a panic is E0222, and an
    exhausted budget is E0225, each reported at the test item. `differential` cannot be combined with
    `should_panic` or `should_fail` (E0170). `[ignore]` and `[timeout(ms)]` still apply.
  - The behavior is in `spec/test-runner.md`. The conformance tests are
    `tests/m10_test_runner/attributes_differential.bd`, `differential_invalid.bd`, and `differential_impure.bd`;
    `tests/check_test_runner.py` runs `tests/runner/differential/`, whose divergent test depends on the
    unspecified `Map` iteration order, proving the check catches a real interpreter/compiled divergence.
- Every check and test tool under `tests/` is Python 3 (`tests/run_tests.py`, `tests/check_*.py`,
  `tests/build_release.py`, `tests/release_notes.py`, `tests/bench_regression.py`, `tests/fuzz.py`, and the shared
  `tests/harness.py`), one implementation for Linux, macOS, and Windows instead of the former shell scripts and the
  `check-tests.ps1` duplicate. The conformance runner keeps the same directives, diagnostics, output order, and exit
  codes, and runs tests in parallel by default (`-j N`; results are still printed in test order). CI invokes Python
  directly on all three platforms. This removes the Git Bash process-spawn cost that dominated the Windows CI job:
  the full suite takes 3 s instead of 31 s on WSL with the same compiler, and `check_docs.py` walks links in-process.
- Audit every E9000 site and assign it to a milestone (issue #84).
  - `spec/e9000-audit.md` lists every remaining `Code::UNSUPPORTED` site with the construct, the owning milestone,
    and the issue that removes it. Each site carries a `// e9000: <id>` marker, and `tests/check_e9000.py` fails
    when the source and the table disagree, when a row is stale, or when a site names a milestone that CI already
    runs. Every CI job runs it.
  - The stage 0 diagnostics that had no future milestone now have permanent codes and compile-fail tests: E0007 (a
    name declared or imported more than once; `duplicate_values`, `duplicate_type`, `duplicate_type_parameter`,
    `duplicate_member`, `duplicate_import`), E0009 (a cyclic or infinitely sized declaration; `constant_cycle`,
    `cyclic_types`), E0131 (invalid `Drop`; `drop_impl_shape`), E0163 (invalid `extern`; `extern_invalid`,
    `extern_signature_conflict`), E0164 (unsupported `repr`; `repr_other`), and E0180 (type arguments on a
    non-generic type; `type_args_on_non_generic`). The registry, `docs/error-codes.md`, and their examples are
    updated.
  - String literal patterns are implemented: a `str` scrutinee compares byte for byte, including slice views, the
    empty string, or-patterns, and guards. The domain is open, so a `match` on strings still needs an `else` or `_`
    arm (`m04_data/string_patterns`, `nonexhaustive_string`).
  - An inclusive slice range (`s[a..=b]`) is implemented; `spec/stdlib.md` already promised it. The slice end is
    checked as `hi < len` before it is advanced, so the maximum inclusive end panics as out of bounds rather than
    wrapping, and a reversed inclusive range (`a[2..=1]`) still panics. `m09_stdlib/slice_inclusive`,
    `slice_inclusive_bounds`, and `slice_inclusive_reversed` witness it. A dead "comparing strings" branch was
    removed.
  - The remaining E9000 sites are assigned to m19 (hand-built built-in interfaces), m20 (constant expressions),
    m21 (the remaining type-system forms), or Stage 1 for the cross-cutting pattern, bound, `where`,
    function-value, and runtime-index move forms.
- Close the stage 0 correctness gaps carried into stage 1 (issue #48, sub-issues #49, #53, #58, #64, #71, #77,
  #85, #86, #87).
  - **View lifetime (#49).** RT5 and the new E0123 reject a view stored in a `let` or an assignment that derives
    from a temporary or from a local of an inner block; static data, parameters, and same-or-enclosing-block locals
    are allowed. `match`/`if let`/`for` bindings are exempt. `burdin_check/src/origins.rs` now tracks parameter,
    local, and temporary origins with block ancestry; the rule is in `spec/memory-rules.md` RT5 and
    `docs/error-codes.md`, with `m06_conventions/view_lifetime_*` covering both outcomes.
  - **Ranges as values (#53).** `a..b` is a `Range<T>` and `a..=b` a `RangeInclusive<T>`: `Copy`, equal, ordered,
    hashable, cloneable, droppable, storable in fields and containers, and iterable with `for i in r` without
    borrowing the range. Endpoints are integers of one type; `..` has the lowest precedence. The type and its
    structural behavior are in `spec/grammar.md`, `spec/stdlib.md`, and `Ty::Range`; open-ended forms (`..b`, `a..`,
    `..`) do not exist. `range_values` (storage, clone, equality, hash, Map key, iteration),
    `range_endpoints_are_integers`, and `range_forms` (loop forms and the inclusive maximum) witness it.
  - **Move checker (#58).** Constant array and `List` indices, heap pointee fields (`(*h).a`), `match *h`, and a
    moving pattern with a guard on a temporary scrutinee are tracked instead of E9000. A temporary scrutinee is
    moved into a compiler-made local, so its other fields drop once and a failed guard restores the arm's moves. An
    index expression is evaluated once; a tracked move uses a constant index. A runtime index is still E9000.
    `spec/memory-rules.md` MV6/MV8 and `m05_ownership/move_index_*`, `index_evaluated_once`, `move_heap_field*`,
    `match_heap_pointee*`, `match_guard_temporary*`, `match_temporary_fields_drop`.
  - **Generic gaps (#64).** A generic static method infers its type arguments from its own parameters; a generic
    type's bounds are checked at instantiation (E0141); a view type argument makes the instance second-class (SC3);
    an impl method is compared with the interface method including `unsafe` (E0003); `==`/`<` work on symbolic
    instances whose bounds provide them; `Self` resolves in expressions, literals, patterns, and type paths. Tests
    in `m07_generics` (`infer_static_method`, `type_bounds_at_instantiation`, `view_instance_*`,
    `impl_signature_mismatch`, `symbolic_derives`, `self_in_expressions`, `self_outside_method`).
  - **Modules (#77).** `pub use` re-exports modules and items, with import resolution iterated to a fixpoint; a
    dependency's non-root modules are hidden unless its root re-exports them; `pub use` of a package-private item
    and a public signature or field naming a package-private type are E0150. `m08_modules/reexports`,
    `reexports_hidden`, `pub_use_private`, `public_signature_leaks`.
  - **C bridge (#71: #72-#76, #489, #490).** `CPtr<T>` gained load/store (`*p`), `.is_null()`, equality, element
    offsets and difference, and casts to/from integers and other pointers, all inside `unsafe`; creating, holding,
    passing, moving, or using a `CPtr` outside `unsafe` is E0161. `CString.clone` deep-copies the buffer.
    Out-parameters work through `&mut x as CPtr<...>`, and the cast ends the loan. `struct Name repr(C)` has the C
    layout and `Type.size()`/`Type.align()`; `...` declares a variadic C function; `extern "C" fn` exports a Burdin
    function under its plain C symbol with the C ABI rules (E0162). Aggregates cross the ABI through `CPtr<T>` only
    (by value is E0162), C `_Bool` is declared `u8`, and C `char` is `CChar`. There are no `union` declarations or
    function-pointer types: a union is bytes behind `CPtr<T>` and a function pointer is stored as `CPtr<CVoid>`
    (`spec/stdlib.md`). Tests: `cptr_operations`, `cptr_requires_unsafe`, `c_out_parameter`, `c_abi_structs`,
    `cstring_clone`, `export_c_abi`, `export_c_abi_invalid`, `export_c_abi_panics`; docs in `spec/stdlib.md`,
    `spec/grammar.md`, and `docs/error-codes.md`.
  - **`CStr` is an alias, not a view type (#83).** `CStr` remains a `std` alias for `CPtr<CChar>`: one type
    under two names, first-class and `Copy`, with no length, origin, or conversion rules of its own. There is no
    implicit conversion between text and a C string in either direction (E0003), `p.to_string()` and every other
    string method are E0002, and the only conversions are the explicit `CString.from(s).as_ptr()` and the
    associated `CStr.to_string(p)`. Every use keeps requiring `unsafe`. The rule and its rationale are in
    `spec/stdlib.md`; `m08_modules/cstr_is_an_alias` (run), `cstr_no_implicit_conversion`, and
    `cstr_is_not_a_str` witness it.
  - **Coverage (#85, #86, #87).** Run and compile-fail tests for `unsafe fn` declarations and unsafe interface
    methods (`unsafe_fn`, `unsafe_fn_call_requires_unsafe`, `unsafe_interface_method`,
    `unsafe_interface_mismatch`), and an audit of the `spec/memory-rules.md` test matrix against the suite, with US
    extended to the new pointer and unsafe-function tests.
  - The suite is 247 tests (245 in release); `tests/check_tests.py`, `check_unit_tests.py`, `check_docs.py`,
    `check_error_docs.py`, `check_diagnostics.py`, `check_codegen_opt.py`, `check_build_flags.py`,
    `check_parallel.py`, `check_test_runner.py`, and `check_debug_info.py` are green, and `cargo fmt --check`,
    `cargo clippy -- -D warnings`, and `cargo test` pass.
- `Map` is an open-addressing hash table (issue #82). The linear scan became linear probing in the map's existing
  three-word buffer: a zero hash marks an empty slot, the table doubles and rehashes at a 3/4 load factor, and a
  removal shifts the rest of the probe cluster back instead of leaving tombstones. Keys still need only `==`: they
  are hashed structurally (integers, floats, text by its bytes, tuples, structs, enums, `Option`, `Result`, and
  ranges), so `Hash` is not required. The API, the unspecified iteration order, and the iteration, drop, and clone
  behavior are unchanged. A 200,000-key insert-and-lookup program drops from 9.8 s to 0.01 s in a release build, and
  the million-key `m09_stdlib/map_many_keys` witness takes 0.07 s; `map_key_types` and `map_remove_and_clear` cover
  aggregate keys, deletion, reinsertion, and `clear`. The implementation is described in `spec/stdlib.md`.
- The release pipeline (issue #452, sub-issues #453-#456): a `Release` workflow builds the six supported targets
  natively (`x86_64`/`aarch64` Linux, macOS, and Windows), smoke-tests `build`, `run`, and `test`, writes
  `burdin-<version>-<target>.tar.gz` archives and SHA-256 checksums, verifies that two checkout paths produce the
  same Linux archive, and publishes only from a `v<version>` tag or an explicit manual `publish`. A manual run
  defaults to a dry run, and the nightly schedule never publishes. `install.sh` and `install.ps1` install a release
  after verifying `SHA256SUMS`, with `BURDIN_VERSION`, `BURDIN_CHANNEL`, `BURDIN_INSTALL_DIR`, and
  `BURDIN_RELEASE_DIR` for the rehearsal path. `burdin --version` gained a `channel:` line (`dev`, `nightly`, or
  `stable`) backed by `BURDIN_CHANNEL` and `BURDIN_VERSION`; the workspace version became `0.1.0`. The version
  scheme (SemVer for the compiler, editions versioned separately), channel policy, breaking-change rule, artifact
  protocol, reproducibility rules, and rehearsal checklist are in `docs/releasing.md`.
  `tests/build-release.sh` builds and packages a target (and proves reproducibility), `tests/release-notes.sh`
  extracts a changelog section and checks its links, and `tests/check-release.sh` guards the workflow, the archive,
  the checksums, both installers, and the metadata in every CI job. No release has been cut.
- Dependency and license audit (issue #449). `Cargo.lock` is committed and pins every external crate by exact
  version, crates.io source, and checksum, and every direct requirement is written `=x.y.z`.
  `tests/dependencies.txt` is the reviewed inventory: one line per external crate with its license and why the
  compiler needs it (18 crates today, all transitive through `inkwell` 0.11 and `llvm-sys` 211). `tests/check-deps.sh`
  fails on an unreviewed crate, a version change, a lockfile without a checksum, a non-exact requirement, a
  non-permissive license, or a license that disagrees with the crate's own metadata. Every CI job runs it after
  `cargo build` with `BURDIN_DEPS_REQUIRE_METADATA=1`; `tests/check-ci.sh` guards the step, its position, and the
  inventory. `docs/dependencies.md` records the policy and the review, including LLVM's host-supplied
  Apache-2.0 WITH LLVM-exception license.

- The platform support matrix and its known limitations are documented in
  [`docs/platform-support.md`](docs/platform-support.md) (issue #457): the tested and untested OS/architecture pairs,
  the runtime features each CI job proves, and the gaps (no PDB and no symbolized backtraces on Windows, the untested
  Linux and Windows arm64 hosts, the untested libc assumptions, and the terminal-only output path). The page is linked
  from this file and the documentation index. `tests/check-platform-support.sh` checks the matrix against
  `ci.yml`, compiles and runs the page's example, and proves binary-safe standard output and file I/O byte-for-byte;
  every CI job runs it.

- Compiler and runtime debug/observability/performance work (issue #421, sub-issues #422-#432):
  `-g` emits source-level DWARF for functions, parameters, locals, and every expression's line and column
  (`tests/check-debug-info.sh`, which also proves a gdb breakpoint and local read); panics print a symbolized
  `stack backtrace:` after the unchanged message line in both profiles; standard output is block buffered (line
  buffered on a terminal) and flushed by `atexit`, `process.exit`, panics, and the stack-overflow handler; the main
  thread has an alternate-signal-stack guard that turns unbounded recursion into `panic: stack overflow`; generic
  glue is shared by structural type shape; drop flags are omitted for locals the move checker proves dead at every
  exit; constant and `0..len` loop indices skip their bounds checks; `--lto`, `--pgo-generate`, and `--pgo-use`
  expose LLVM LTO and PGO; `--strip` and size budgets (128 KiB hello, 512 KiB std-heavy) cover release binaries;
  `burdin --version` prints the version, target, LLVM version, commit, and profile; and modules are parsed in
  parallel with byte-identical diagnostics (`BURDIN_JOBS`). Each has a CI check: `check-debug-info.sh`,
  `check-codegen-opt.sh`, `check-build-flags.sh`, and `check-parallel.sh`.
- `burdin test` gained the runner ergonomics of issue #438 (sub-issues #439-#443, #445). Tests may carry
  `[ignore]`, `[timeout(ms)]`, `[should_panic]`/`[should_panic("text")]`, and `[should_fail]` attributes; an invalid
  attribute is E0170. Tests run concurrently (`-j`/`--jobs`, default: the CPU count) with a deterministic report,
  each in its own fresh temporary working directory with a clean environment and cleaned up afterwards. Output is
  captured and printed only for failures, with `--verbose` to show it always. `-p`, `--workspace`, and `--filter`
  select packages and tests, and dependency tests do not run by default. The rules are in `spec/test-runner.md`;
  `tests/m10_test_runner` adds four conformance tests and `tests/check-test-runner.sh` checks the runner black-box;
  every CI job runs it (issues #438-#443). `tests/check-tests.sh` and `.ps1` now fail when a registered error code
  has no compile-fail test and no documented exception, so a new diagnostic cannot ship untested (issue #445).
  Differential tests between comptime and compiled code (issue #444) remain blocked on the comptime interpreter
  (milestone 20); `spec/test-runner.md` records their intended shape.
- Settle the 16 assumptions in `tests/README.md` (issue #21, sub-issues #22-#40). The decisions are now rules in
  `spec/grammar.md`, `spec/memory-rules.md`, two new spec files (`spec/stdlib.md` for the standard-library surface
  and `spec/derives.md` for the exact derived members and FNV-1a hash values), and `spec/stage0-subset.md` /
  `spec/stage1-subset.md`; `tests/README.md` links each decision to its section. 40 conformance tests witness them.
  Four implementation gaps closed with the decisions: `Drop` rejects a read-only or `own` receiver (E0003),
  `CPtr.null` rejects arguments (E0004), the ordering operators work on `str` and `String`, and an invalid
  `extern "C"` type or convention is E0162.
- Retire the claude.ai copies of the design and stage 0 documents (issue #44): the repository files are canonical and
  say so, the external copies are recorded as historical, and the design document marks its snippets as illustrative.
- The driver contract is documented in a new root `README.md`: `burdin build`, `run`, and `test`, the diagnostic
  format, `--emit-ir`, `--link-lib` (including how `LLVM-C` resolves through `llvm-config`), `BURDIN_CC`, and the
  test output format. The file is the documentation index, and `tests/check-docs.sh` checks its links and the
  documented behavior; every CI job runs it (issue #42).
- Pin the Rust toolchain to `1.100.0` with the `clippy` and `rustfmt` components in `rust-toolchain.toml`, so
  `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings` are reproducible from a clean machine.
  CI installs and verifies that toolchain, and `tests/check-ci.sh` fails if the pin or its verification goes missing
  (issue #19).
- CI gained a `macos` job on `macos-latest` (Apple Silicon): Homebrew's `llvm@21`, the pinned Rust toolchain, and the
  same checks as Linux, including every conformance milestone m01-m11. `tests/check-platform.sh` proves the emitted
  LLVM triple follows the host and that `usize`/`isize` are 64-bit; it runs on every job. `tests/check-ci.sh` guards
  all three jobs. The driver resolves `--link-lib LLVM-C` for Homebrew's `libLLVM.dylib` layout, finds `llvm-config`
  through `LLVM_SYS_211_PREFIX`, and its resolution logic has unit tests (issues #16, #17, #18).
- CI gained a `windows` job on `windows-latest`: the MSVC environment through `ilammy/msvc-dev-cmd`, the official
  LLVM 21 Windows archive, and the same checks as the other jobs, including every conformance milestone m01-m11 and a
  `BURDIN_CC=cl` run. LLVM is linked as a shared library on every platform, because the static Windows archives crash
  in the dynamic-CRT Rust binary; the driver maps `--link-lib LLVM-C` to `LLVM-C.lib` and reports a missing `cl` with
  an actionable message. `process.run` exit codes are covered on both the POSIX and Windows CRTs (issues #12, #13,
  #14, #15).
- Add `cargo bench -p burdin_bench`: generated inputs measure the lexer, the parser, name resolution and type
  checking, and code generation at three sizes, reporting median time per iteration. `--smoke` runs every stage
  once with generous time budgets and is part of CI, and `HANDOFF.md` records the first baseline (issue #41).
- CI caches the cargo registry, `target/`, and LLVM 21 in all three jobs. The cargo cache is keyed by
  `rust-toolchain.toml` and `Cargo.lock`, so a toolchain or dependency change starts fresh; Linux caches the installed
  LLVM tree (including the multiarch `libLLVM` it symlinks to), macOS caches the Homebrew bottle downloads, and
  Windows caches the unpacked official archive. `tests/check-ci.sh` fails if any of those caches goes missing
  (issue #46).
- The front-end crates have focused Rust unit tests next to the conformance suite: the lexer token stream, parser
  recovery, type unification, exhaustiveness, the type and place questions in `burdin_tir`, the origin and move
  state in `burdin_check`, and diagnostics. CI runs `cargo test --workspace` in every job, and
  `tests/check-unit-tests.sh` fails when a front-end crate or one of the four named areas loses its tests
  (issue #45).
- Every CI job runs the finished conformance milestones in the release profile as well as debug, covering release
  code generation; tests marked `//@ debug-only` (the m02 overflow abort test) are skipped. `tests/check-ci.sh` fails
  if a job loses its release step, if the runner stops selecting the profile or skipping debug-only tests, or if no
  test is marked debug-only (issue #47).
- Diagnostics gained presentation and tooling (issues #433-#437). The human format colors the location, severity,
  and code when stderr is a terminal, honors `--color`, `--no-color`, and `NO_COLOR`, and wraps to `--width`,
  `COLUMNS`, or 80; `--diagnostic-format short` is the one-line machine format and `json` is one JSON object per
  diagnostic plus a summary object. Diagnostics carry notes and suggestions with spans and replacement edits
  (E0100/E0110 attach notes; E0114/E0115 attach the suggested `mut` edit). Output is sorted deterministically, and
  `--error-limit` truncates after sorting while still counting every error (unlimited by default). The new
  `docs/error-codes.md` documents every registry code with a compiled example and a fix, `burdin explain <code>`
  prints an entry, and every CI job runs `tests/check-diagnostics.sh` and `tests/check-error-docs.sh`. Unit tests
  cover the formats, color decision, wrapping, JSON shape, ordering, and limit in `burdin_diag` and `burdin_driver`.
- The compiler benchmarks gained regression tracking (issue #446): `cargo bench -p burdin_bench -- --save FILE`
  records a run's medians in a plain-text baseline, and `--baseline FILE` compares a later run with it, reporting
  every case more than `--threshold` percent slower (default 25) and exiting 1. `tests/bench-regression.sh` runs the
  full suite, and every CI job records and compares a baseline kept in `actions/cache`, keyed by the runner OS and
  architecture, the pinned toolchain, and `Cargo.lock`; a main push updates it and a pull request compares against
  the last main run. The comparison is advisory until the runner numbers are stable. `tests/check-ci.sh` fails if
  the step, the script, the cache, or the harness modes go missing, and `cargo test -p burdin_bench` covers the
  baseline format and the threshold arithmetic.
- The lexer and parser are fuzzed (issues #288 and #447). `burdin_fuzz` is a dependency-free, deterministic harness
  on the pinned stable toolchain: `burdin-fuzz-syntax` replays the committed corpus and then mutates it with a fixed
  seed, catching panics and watching for an input that stops making progress. `crates/burdin_fuzz/corpus/syntax/`
  holds the hand-written edge cases and every input that found a panic, and `cargo test --workspace` replays them
  plus 20,000 deterministic mutations in every CI job, so a reintroduced panic fails. A scheduled `fuzz` job runs
  a ten-minute session seeded by the run id (so each run explores new inputs) and uploads any found input as the
  `fuzz-crashes` artifact; committing it with the fix keeps the regression covered. `tests/check-ci.sh` fails if
  the schedule, the fuzz job, the long session, the artifact upload, or the corpus goes missing.

## Stage 1 — 0.1.0 (2026-11-19)

Planned in `spec/stage1-subset.md`; tracked by milestones m12-m24 and the GitHub roadmap.

### Milestone 23 — allocators, `Arena`, and `Shared` (issue #180, sub-issues #181-#188)

- Decision (#181): an allocator is an ordinary value and an ordinary generic parameter (`A: Allocator`, or
  `own a: impl Allocator`), and `List<T, A = Global>`, `Map<K, V, A = Global>`, and `Heap<T, A = Global>` take
  it as their trailing type argument. `Global` is zero-sized and stored nowhere, so the stage 0 spellings and
  layouts are unchanged; any other allocator is stored as an owned field, given to `new_in` (`spec/allocators.md`,
  AL1-AL10).
- The `Allocator` interface and `Global` (#182): `std/src/alloc.bd` declares `allocate`, `reallocate`, and
  `deallocate` (all `mut self`, by address when stored) and the zero-sized `Global`, whose methods call the
  compiler's unsafe `__alloc`/`__realloc`/`__dealloc`.
- Allocator parameters (#183): `List<T>.new_in(alloc)`, `Map<K, V>.new_in(alloc)`, and `Heap.new_in(alloc, value)`
  move the value into the container; `new` for a container with a stored allocator is E0272 and a non-allocator type
  or value is E0270. Default type parameters (`struct Buffer<A: Allocator = Global>`) are resolved where the type
  is declared, with E0271 for a non-trailing default, a const parameter's default, and a default on a function,
  interface, or alias; a bare inherent or `Drop` impl covers every instance.
- `Arena` (#184): a first-class bump allocator with 4096-byte blocks, `bytes_allocated`, `reset`, and a `Drop` that
  resets it; `reallocate` copies into fresh arena memory and `deallocate` is a no-op. It is not second-class
  because every safe way to get an arena-backed buffer makes the container own the arena.
- `Shared<T>` (#185): a built-in non-atomic reference-counted cell `{count, value}` from `Global`; `.clone()`
  shares it, `.get()` borrows the value, `.strong_count()` counts, and the last handle drops the value and frees
  the cell. It is `Clone` for every `T` and never `Copy`; the cell's value is stored, so a view is E0120. Stage 2
  makes the count atomic and adds `Send`.
- Leak detection (#186): a `burdin test` build allocates through a checking allocator with a 40-byte header (next,
  size, site, magic) and a live list. A double free panics; a test that returns success with live allocations is
  reported as `test leak: <n> allocation(s) leaked, <bytes> byte(s)` with `allocated at <path>:<line>:<column>` for
  each block, and fails. `burdin build`/`run` keep the plain C heap.
- Allocator-aware containers (#187): `List`, `Map`, and `Heap` allocate, reallocate, free, clone, and iterate
  through the stored allocator, and the owning iterators carry it. Dropping a container frees its buffer through
  the allocator and then drops the allocator; cloning requires the allocator to be `Clone`.
- Tests (#188): `tests/m23_allocators/` adds 28 conformance tests (the default spelling and layout, allocator
  parameters and functions, arena-backed `List`/`Map`/`Heap`, arena reset and growth, `Shared` across scopes and in
  structs, default type parameters, owning iterators, cloning, the leak report, and the compile-fail cases of
  E0270-E0272, E0120, and E0224), plus the `tests/runner/leaks/` fixture and the leak checks in
  `tests/check_test_runner.py`. The suite is 577 tests. The checking allocator also exposed a temporary-drop leak in
  the non-consuming `Option`/`Result` methods, now fixed by reading the receiver through a place.
- Documentation: `spec/allocators.md` (new), `spec/grammar.md` (type parameter defaults), `spec/stdlib.md`,
  `spec/memory-rules.md` (DR8a), `spec/test-runner.md`, `spec/stage1-subset.md`, `tests/README.md` (decision 30),
  `docs/error-codes.md` (E0270-E0272), `README.md`, `AGENTS.md`, `CLAUDE.md`, `HANDOFF.md`, and CI's milestone list.

### Milestone 22 — build scripts (issue #173, sub-issues #174-#179)

- Decision (#174): `build.bd` next to a package's manifest is a standalone Burdin program the driver compiles and
  runs before the package's sources are read. The entry package's own script is trusted; a dependency's runs only
  when the entry package's manifest names it in `[build] allow`, because a script is arbitrary code execution. The
  grant is the whole permission: there is no OS sandbox, and only the generated-source directory is tracked
  (`spec/build-scripts.md`, BS1-BS8).
- Run before compilation (#175): the script is compiled in the build's profile, runs with the package directory as
  its working directory and `argv[1]`, the output directory `<package>/target/burdin/<profile>` as `argv[2]`, and
  the driver's directory first on `PATH`; its output goes to the compiler's stderr. A script that does not compile,
  cannot start, or ends non-zero stops the build with E0261. A package without a script builds as before.
- Generated modules (#176): every `.bd` under `<out>/src` is a module of the package under its relative path, with
  the package's own imports and visibility. A generated module that repeats a source module's path is E0262.
- Caching (#177): a content key per package and profile over the script, the manifest, the compiler binary, the
  files the script lists in `<out>/inputs`, the files its `embed` calls read, and the generated outputs. A changed
  or missing input or output reruns; touching does not. The declared inputs join `--emit-deps`.
- Minimal `bindgen` (#178): `burdin bindgen <header> --out <file>` generates `extern "C"` declarations, `repr(C)`
  structs, typedefs, enum constants, and object-like `#define`s. `#include`, function-pointer types, and unions are
  skipped with a comment; the full tool remains stage 3.
- Tests (#179): `tests/m22_build/` adds nine conformance tests (a generated module the package imports, a generated
  `test` block, a granted dependency, the compile-fail cases of E0260-E0262, a failing and a non-compiling script,
  and a bindgen run that calls libc `strlen`), with unit tests for the cache key and the header parser, and
  `tests/check_build_scripts.py` black-boxes the cache, `--emit-deps`, and the `bindgen` command line. The new
  diagnostics are E0260 (invalid build configuration), E0261 (a failed build script), and E0262 (invalid generated
  modules).
- Documentation: `spec/build-scripts.md` (new), `spec/stage1-subset.md`, `tests/README.md` (decision 29, the three
  codes), `docs/error-codes.md` (E0260-E0262), `README.md` (the driver contract and `[build] allow`),
  `CLAUDE.md`, `HANDOFF.md`, and CI's milestone list.

### Milestone 21 — reflection, user derives, const generics, and `embed` (issue #166, sub-issues #167-#172)

- Decisions (#167): reflection is five compile-time members of every type (`Type.name`, `.size`, `.align`, `.fields`,
  `.variants`), `x.{name}` is the field access by a compile-time name, and a user derive is `derive Name { fn ... }`,
  the methods of an impl block that `derives (Name)` registers (`spec/reflection.md` RFL1-RFL10).
- Reflection (#168): sema evaluates the members while it checks a body. `Type.fields` and `Type.variants` are arrays
  of the new `std` structs `FieldInfo` and `VariantInfo`; a `comptime for` over them is unrolled in sema, with the
  loop variable's `name`, `type_name`, `index`, and `arity` folded to literals; `x.{name}` resolves to the field.
  A function without `self` or a variant named like a member is E0231; a member of a type with neither fields nor
  variants is E0230; a computed name that is not a compile-time string is E0232.
- User derives (#169): the `derive` item, imports and `pub use` of derives next to their interface, and the
  generated impl (`Type<A>: Name where A: Name`) on structs and enums, generic or not. The built-in derives are
  unchanged, and a user derive cannot take their names (E0233).
- Const generics (#170): `const N: usize` parameters (integer, `bool`, `char`) on structs, enums, and functions;
  const arguments as literals, names, or `{ expr }`; a new `Ty::Const` so instances, keys, and inference work as for
  types; array lengths (`[T; R * C]`, `[0; N]`) are const expressions over numbers, const parameters, and constants;
  a const parameter is a value in its body and is inferred from an array argument, a generic type, or the expected
  type. E0250-E0252. The `type-array-length` and `array-repeat-length` E9000 sites are gone. An array literal passed
  to a generic function no longer takes the callee's unbound `T` as its element type (a bug const generics exposed).
- `embed` (#171): `embed("path")` is the file's contents as `str` or, with the expected type `[u8; N]`, as bytes,
  relative to the source file. The file is a build input: `burdin build --emit-deps <file>` lists them. A missing
  file is E0240 and a bad path or length is E0241 (`spec/comptime.md` CT15).
- Generic type aliases (#392): `type Pair<T> = (T, T)`, with parameters, bounds, and const parameters, expanded in
  every type position; an alias is the aliased type, so an impl block written through one is E0003
  (`spec/type-aliases.md` AL1-AL6). The `declare-generic-alias` E9000 site is gone.
- Impls for specific instances (#395): the arguments of an impl block are a pattern (`Stack<i32>`, `Pair<i32, B>`,
  `Holder<List<T>>`) and the block covers the instances it matches; the later of two overlapping impls of one
  interface is E0143, and a `Drop` block covers every instance (E0131 for one that names specific arguments)
  (`spec/impls.md` IM1-IM9). The `impl-specific-args` and `impl-builtin-generic` E9000 sites are gone.
- Built-in interfaces on generic types (#401): a derive holds for the instances whose parts allow it, which IM8
  records; writing one by hand stays E9000 under #402 for a plain and a generic type alike.
- Tests (#172): `tests/m21_reflect/` adds 25 conformance tests (a Burdin-written derive, reflection over fields and
  variants, a const-generic matrix, `embed`, generic aliases, impls for specific instances, and the compile-fail cases
  of E0230-E0233, E0240-E0241, E0250-E0252, E0131, and E0143), plus unit tests for the constant evaluator, the
  parser, impl matching, and the front end on source text. The suite is 537 tests.
- Documentation: `spec/reflection.md`, `spec/const-generics.md`, `spec/type-aliases.md`, and `spec/impls.md` (new),
  `spec/comptime.md` (CT15), `spec/grammar.md` (the `derive` item, const parameters and arguments, `x.{name}`, the
  type of an impl block), `spec/stage1-subset.md`, `spec/e9000-audit.md` (five rows removed),
  `tests/README.md` (decisions 26-28, the ten codes), `docs/error-codes.md` (E0143), `README.md` (`--emit-deps`),
  `Burdin Language Design.md`, `HANDOFF.md`, and `CLAUDE.md`.

### Milestone 20 — comptime (issue #157, sub-issues #158-#165 and #305-#308)

- Decision (#158): the comptime engine interprets the checked typed tree; there is no MIR (#159 closed as not
  chosen). The new crate `crates/burdin_comptime` depends only on `burdin_tir` and `burdin_diag` and runs after the
  memory checks and before code generation (`spec/comptime.md`, CT1).
- Syntax and sema: `comptime` is a keyword. `comptime E` and `comptime { ... }`, `comptime name: T` parameters,
  `comptime if`, `comptime for`, and `comptime assert(cond, "msg")` (also as a module item). Comptime code sees only
  constants, comptime parameters, `comptime for` bindings, and its own locals (E0220); a value that crosses into
  run time must be materializable (E0221). A `const` initializer is now any materializable expression, and the
  memory checks also run on const initializers.
- Interpreter (#160, #305-#308): every expression form, with owned values, places for `&mut`/`mut`, moves, user
  `Drop`, the profile's overflow rule, and the runtime's float text, integer parsing, hashing, and layout. Panics
  are E0222, a failed `comptime assert` is E0223, output/files/environment/processes/C are E0224, and the step
  budget (10,000,000, `--comptime-steps N`), call depth 512, and 1024 specializations per function are E0225. A
  constant cycle through calls is E0009.
- Rewriting: constants and comptime expressions become literals, `comptime if` keeps its selected branch, `comptime
  for` is unrolled, and each call to a function with comptime parameters goes to a specialization per argument
  value. `std/src/target.bd` adds `target.os()` and `target.arch()`. The driver runs the compiler on a thread with
  a 1 GiB reserved stack.
- Tests: `tests/m20_comptime/` adds 40 conformance tests, including `interp_*` tests that compare comptime and
  run-time results; `tests/check_build_flags.py` checks `--comptime-steps`. The suite is 512 tests.
- Documentation: `spec/comptime.md` (new, CT1-CT14), `spec/grammar.md`, `spec/stdlib.md`, `spec/stage1-subset.md`
  (decision 6 and the milestone note), `tests/README.md` (layout, E0220-E0225, E0009, and decision 25),
  `docs/error-codes.md`, `spec/e9000-audit.md` (the `const-expression` site is gone), `HANDOFF.md`, `CLAUDE.md`, and
  `AGENTS.md`.

### Milestone 19 — operator overloading and `Index` (issue #150, sub-issues #151-#156)

- Interfaces: `std/src/ops.bd` declares `Add`, `Sub`, `Mul`, `Div`, `Rem`, `BitAnd`, `BitOr`, `BitXor`, `Neg`,
  `Not`, `PartialEq`, `PartialOrd`, `Index<T>`, and `IndexMut<T>` as ordinary interfaces; the prelude makes them
  visible without a `use`. All operands are same-type, a type implements each interface once, arithmetic/unary
  methods borrow their operands, and `PartialEq`/`PartialOrd` provide `ne`/`le`/`gt`/`ge` as defaults. Comparisons
  use new interfaces rather than the derive-only `Eq`/`Ord`, so every stage 0 derive keeps its exact behavior.
- Resolution: the stage 0 built-in rule is always tried first, then a derived `Eq`/`Ord`, then the user type's
  interface impl, then a type parameter's bounds (`SYMBOLIC_CALL`, resolved by monomorphization). A missing
  implementation is the existing E0141 for a struct/enum/type parameter and E0003 for any other type; a failed
  derive keeps E0003. A literal never adopts a user type, so `v + 1` is E0003. `&&`, `||`, `=`, compound
  assignment, and the shifts are not overloadable (compound assignment stays E0001).
- `Index`/`IndexMut`: `Index<T>`/`IndexMut<T>` name the element type, take a `usize`, and return `&T`/`&mut T`, so
  `x[i]` lowers to `*x.index(i)` and `x[i] = v` to `*x.index_mut(i) = v`. A write-place mode selects `index_mut`
  for the left side of `=`, `&mut x[i]`, a `mut x[i]` argument, and an index inside a written place (`x[i].f = v`,
  nested `g[i][j] = v`). The receiver must be a mutable place (E0113); moving a non-`Copy` element out is E0101;
  indexing a user container without `Index`/`IndexMut` is E0141.
- Checker and backend: sema records the interface indices while installing the prelude, lowers operators and
  indexing to ordinary `Call` nodes, and threads a write-place mode through argument checking. The closure capture
  classifier follows calls under a field/index place, so a closure that writes `v[i]` is `FnMut`. The borrow
  checker treats a reference returned by a call and consumed by `*` as a temporary (no result loan) while tracking
  a re-borrow through the call's origin, so `v[i] = v[j] + 1` works and `&mut v[i]` still borrows `v`. Codegen now
  treats every reference dereference as a place, and `clone` reads it without registering a spurious temporary
  drop, so non-`Copy` elements are not freed twice.
- Tests: `tests/m19_operators/` adds 36 conformance tests: a `Vec`-like `+`/`[]` type, generic and nested index
  containers, write-place forms, closure capture, drop-on-assignment, the derived-wins rule, generic bounds, every
  missing-impl and mutability diagnostic, compound assignment, and stage 0 regression tests for
  integer/float/`str`/tuple/derived operators, overflow, division by zero, and shift overflow. A
  `burdin_tir::place` unit test pins that `*call()` is a rootless place. The suite is 472 tests.
- Documentation: `spec/operators.md` (new, OP1-OP13), `spec/stdlib.md`, `spec/stage1-subset.md` (decision 8 and the
  milestone note), `tests/README.md` (layout, the E0141 registry row, and decision 24), `docs/error-codes.md`,
  `spec/e9000-audit.md` (the hand-built built-in interface row moves to the #402 Stage 1 work), `HANDOFF.md`, and
  this file.

### Milestone 18 — extension methods (issue #143, sub-issues #144-#149)

- Declarations: `fn Type.name(self, ...)` at module scope adds a method to any type name in scope, local, imported,
  or built-in, without an impl block. The receiver's type arguments declare the extension's type parameters
  (`fn List<T>.largest(self): T where T: Copy + Ord`) and a `<...>` after the method name adds method-level ones;
  bounds come from inline method parameters and `where` predicates. `self`, `mut self`, and `own self` follow the
  method conventions. `priv` and `unsafe` apply as on any item. The parser recognizes the receiver path before the
  method name without backtracking (`generic_list_end`), and sema registers the declaration as an ordinary function
  whose first parameter is the receiver, monomorphized per instantiation like a generic function.
- Visibility and resolution: an extension is visible in its declaring module and in modules that import it by name
  (`use text.shout`, `use text.{shout}`); importing the module or another item does not bring it, an import is not
  re-exported, `pub use` re-exports it, and `priv` is package-private (E0150). `x.f()` tries the type's methods
  (inherent and interface, including the methods the compiler provides for built-in types) first and a visible
  extension last; the lookup stays local to the calling module.
- Shadowing: an extension whose receiver type already has a method of the same name is unreachable through dot
  syntax; the compiler reports the new warning E0215 at the extension's declaration, and the program still
  compiles. The driver now prints warnings and continues, and `//@ stderr:` works on `run` tests so a warning can be
  asserted.
- Diagnostics: E0216 (an extension without `self` first, type arguments on an intermediate receiver segment, a
  `where` predicate naming a non-parameter, or an import under another name) and E0007 for a second extension with
  the same receiver or a second import of one name. The registry, `docs/error-codes.md` (with the first
  `warning-example` block), and `burdin explain` cover both codes.
- Tests: `tests/m18_extension/` adds 32 conformance tests, including seven extension package targets: extensions
  on `String`, `i32`, `Option<i32>`, a user struct, an enum, a user generic type, and `List<T>`; `mut`/`own`
  receivers; view returns and their lifetime error; imports across modules and packages, a dependency's extension
  and a `pub use` re-export, an imported receiver type, duplicate and aliased imports, and the shadowing warning for
  an inherent, an interface, and a compiler-provided method. Parser unit tests cover the declaration syntax and sema
  unit tests cover receiver matching. The suite is 436 tests.
- Documentation: `spec/extensions.md` (new, EX1-EX8), `spec/grammar.md`, `spec/stage1-subset.md`, `tests/README.md`
  decision 23 and the registry, `docs/error-codes.md`, `tests/run_tests.py`, `HANDOFF.md`, and this file.

### Milestone 16 — errors (issue #126)

- `error enum` declarations: `error` is a keyword followed by `enum`; the declaration takes an ordinary enum's
  permissive layout, package visibility, `derives (...)`, and generics. Every variant carries a required message
  template whose `{...}` placeholders name its payload fields (positional for a tuple variant, by name for a struct
  variant; a unit variant has none), with the full format-specifier grammar. A tuple variant with one field may add
  `from`, which registers a conversion from that field's type.
- Lowering (`crates/burdin_syntax/src/error_item.rs`): an `error enum` becomes the same enum plus `Display` (from
  the templates), `Error` (the `std` marker interface, which requires `Display`), and one `From<Source>` impl per
  `from` variant, all generated as Burdin source before derive expansion. The checker, the memory checks, and code
  generation see only ordinary items, so an error value is moved, dropped, matched, stored, and printed like any
  enum. The new codes are E0210 (an invalid declaration: a `from` that is not a one-field tuple variant, `from` on
  a generic error enum, a placeholder that does not name a payload field) and E0211 (two variants converting from
  one source type).
- The dynamic `Error`: `std` declares `interface Error requires Display { }`, the struct
  `Error { value: any Error, frames: List<TraceFrame>, context: String }`, and
  `TraceFrame { file: String, line: i32, col: i32 }`. `Result<T>` written with one argument means
  `Result<T, Error>`; an error type is boxed implicitly wherever `Error` is expected (a value that does not
  implement `Error` is the ordinary E0003 mismatch there, and E0212 at `?`), so `?`, `Err(...)`, an annotation, and
  `.context(...)` all produce the dynamic error while keeping the original boxed value. `e.is<T>()`,
  `e.downcast<T>()` (which consumes the box), and `{}` inspect and print it.
- `.context(message)`: a compiler-known method on `Result<T, E>` returning `Result<T, Error>`; `Ok` passes through,
  and an `Err` records the message in front of the existing chain, keeps the boxed source, and returns E0212 when
  `E` is not an error type. `Option` has no `context`.
- Traces: a debug build records a `TraceFrame` when an error type converts into `Error` and at every `?` that
  propagates a `Result<..., Error>`, creation first; `.context` keeps the frames it receives. A release build
  allocates none and leaves `frames` empty, with the same `Error` layout in both profiles. `error.frames` is
  public and `TraceFrame` prints as `file:line:col`.
- `fn main(): Result<T>`: the parser renames the function to `__burdin_main_result` and appends a wrapper `main`
  that prints an `Err` with `Display` to standard error and exits 1; `Ok` prints nothing and exits 0. The `Result`
  must be written directly (an alias is E0003), and a `main` with parameters or another return type is still
  E0003.
- Typed test failures: in a `burdin test` build `assert`/`assert_eq` return `Result<(), AssertError>` and build
  `AssertError.Failed`/`FailedEq` with the call's source text, the values (formatted with `Debug`), and the call's
  `path:line:col`; the compiler appends the text and location at the call site, and the generated test wrapper
  prints the error to standard error before the runner reports the failure. A test body is compiled as a function
  returning `Result<()>`, the dynamic error, so the same `?` also accepts a file or parsing error; a normal build
  keeps the panicking assertions.
- Standard library: `std/src/std.bd` declares `Error`, `TraceFrame`, the `context`/`is`/`downcast` helpers, the
  `IoError` (`Read`/`Write`/`Run`), `ParseIntError` (`Invalid`), and `AssertError` error enums, and
  `__test_assert`/`__test_assert_eq`. `read_to_string`, `write_file`, `process.run`, and the integer parsers
  return the real error variants with their messages; the compiler primitives behind them produce a placeholder
  that the `std` wrappers replace. `orelse` accepts a closure handler: `|e| ...` for a `Result` and `|| ...` for
  an `Option` (issue #488).
- Tests: `tests/m16_errors/` adds 25 conformance tests covering `error` declarations and messages, `from` and `?`,
  the dynamic error and downcasts, `context` chains, debug traces, `main(): Result<T>`, typed assertions, the
  standard-library errors, the `orelse` handlers, and every new diagnostic. The suite is 379 tests (all pass in
  debug with one host skip; release passes all except the debug-only tests).
- Documentation: `spec/errors.md` (new), `spec/grammar.md`, `spec/stdlib.md`, `spec/test-runner.md`,
  `spec/stage1-subset.md`, `std/README.md`, `tests/README.md` decision 21 and the registry, `docs/error-codes.md`,
  `HANDOFF.md`, and this file.

### Milestone 17 — default and named arguments (issue #137, sub-issues #138-#142)

- Default arguments: a named parameter may be written `name: Type = expr`. The default is checked once at the
  declaration, against the parameter's type and in the declaration's module scope, and must be a constant
  expression (a literal, another constant, an operator on constants, or a cast). It is materialized at every call
  that omits the argument, so no state is captured and no code runs at the declaration. A default cannot depend on
  a type parameter, cannot be on a `mut` parameter (the callee writes to the caller's place), cannot appear in an
  interface method signature (shared by every implementation) or an `extern` declaration, and the parameters with
  defaults must be trailing. `burdin_sema` stores the checked expression in `ParamSig.default`; call checking
  inserts it.
- Named arguments: `f(name: value)` matches a parameter by name; positional arguments fill the leading parameters
  in order, named ones may be reordered, and a positional argument may not follow a named one. The receiver is not
  a name. Names work on functions, methods, associated functions, interface methods (static and through `any`),
  and generic functions and methods (matching happens before inference). Built-in functions and methods,
  `Some`/`Ok`/`Err`, variant constructors, closure values, and `extern` declarations take no named arguments. A
  missing required argument stays E0004.
- Evaluation order: every argument is evaluated left to right in parameter order, and a default in the position of
  the parameter it fills, so a reordered call runs in the callee's parameter order.
- Diagnostics: E0213 (invalid default: placement, `mut`, interface, `extern`, type parameter, or a non-constant
  expression) and E0214 (invalid named argument: unknown name, parameter named twice, positional after named, or a
  name on a callee that takes none). The registry, `docs/error-codes.md`, and `burdin explain` cover both.
- Tests: `tests/m17_arguments/` adds 25 conformance tests (defaults omitted and given, named arguments reordered,
  named `mut`/`own` arguments, parameter-order evaluation, a `str` view default, interface and generic calls, and
  every new diagnostic). Parser unit tests cover the default and named-argument syntax. The suite is 404 tests.
- Documentation: `spec/arguments.md` (new), `spec/grammar.md`, `spec/stage1-subset.md`, `tests/README.md`
  decision 22 and the registry, `docs/error-codes.md`, `HANDOFF.md`, and this file.

### Milestone 12 — closures (issue #88, sub-issues #89-#97)

- Closure expressions: `|a, b| expr`, `|a: i32|: bool { ... }`, and `||` (zero parameters), parsed at primary
  precedence and callable with the ordinary `f(args)` syntax. Parameter types come from annotations or from an
  expected closure type; the return type is written after the closing `|` or inferred from the body, and must be
  written when the body uses `return`.
- Closure interface types `Fn(A, B): R`, `FnMut(...)`, and `FnOnce(...)` in types, parameters, fields, containers,
  returns, and as generic bounds (`fn apply<F: Fn(i32): i32>(f: F, x: i32)`), monomorphized per closure type. A
  closure value may be used at a lower capability (`Fn` as `FnMut` as `FnOnce`), never the other way.
- Capture analysis: a closure captures the whole variables it uses and does not bind. It borrows when used in place
  and moves when an owned closure type is expected (annotation, field, element, return, or closure-interface
  parameter); a `Fn`/`FnMut` bound keeps its concrete borrow captures. Borrow captures are shared or mutable
  according to how the body writes them; a `FnOnce` bound or a move context captures by move. A closure that moves a
  captured non-Copy value is `FnOnce`, one that writes a capture is `FnMut`, otherwise `Fn`.
- Function values: a non-generic function name (including a built-in function) or `Some` used where a closure type
  is expected becomes a captureless `Fn` closure that forwards its owned arguments to the function with the
  function's own conventions. A function returning a view is E0171 and an `unsafe fn` is E0161; a generic or
  `extern` function as a value stays E9000 (Stage 1). The E9000 audit rows owned by m12 are removed.
- Memory rules CL1-CL11 in `spec/memory-rules.md`: borrowed captures are second-class, and so is a closure that
  moves a view into its record; neither can escape (E0171, registered in `tests/README.md` and
  `docs/error-codes.md`); their loans live until the closure's last use (E0111); `Fn` calls borrow, `FnMut` calls
  borrow exclusively (E0113), and `FnOnce` calls move the closure (E0100); moving a read-only parameter into a
  stored closure is E0101.
- Representation and code generation: a closure value is `{ code, env, drop }`; the capture record is heap-allocated
  and holds pointers for borrow captures and values for move captures. A body function takes the record and the
  call's consumed flag first, the record's destructor drops moved-in captures, and a `FnOnce` body frees the record
  it consumes. A `Fn`/`FnMut` value called through a weakened `FnOnce` type destroys the record after the call,
  including on an early `return`. Calls are indirect through the code pointer; drop and clone glue, `Option`/`Result`,
  arrays, `List`, and `Map` handle closure values. Fixed a latent array drop/clone GEP that strided by the whole
  array instead of one element (found by an array of closures).
- Tests: `tests/m12_closures/` adds 45 conformance tests (run, `burdin test`, and compile-fail) covering borrow and
  move captures, view captures, stored and returned closures, generic `Fn`/`FnMut`/`FnOnce`, weakened capabilities,
  function values, fields, arrays, lists, nesting, `self`, branches, drops, and every new diagnostic. The suite is
  292 tests (290 in release).
- Documentation: `spec/grammar.md` sections 1, 4, and 6, `spec/memory-rules.md` section 12, `spec/stage1-subset.md`,
  `spec/e9000-audit.md`, `tests/README.md`, `docs/error-codes.md`, and `HANDOFF.md`.

### Milestone 13 — iterators (issue #98, sub-issues #99-#108)

- Generic interfaces and generic methods in sema: `interface Name<T>` declares type parameters and bounds, an impl
  supplies the interface's arguments (`Type: Iterator<i32>`), and a method may have its own parameters
  (`fn map<U, F: FnMut(T): U>`). Each call instantiates the method for the types it infers, with method parameters
  first so a default method's `F` wins over an impl parameter of the same name. `Fn` bounds infer the parameters
  inside them (`F: FnMut(T): U` gives `U` from the closure), and a `where` clause is checked with the impl's own
  arguments substituted. A generic `From` method is E0003.
- The protocol in `std`: `interface Iterator<T> { fn next(mut self): Option<T> }` and the marker
  `interface Iterable<T> { }`, with lazy default methods `map`, `filter`, `take`, `skip`, `fold`, `count`,
  `any_item` (the reserved `any` is spelled `.any` at the call), `all`, `find`, and `to_list`. The adapter types
  `MapIter`, `FilterIter`, `TakeIter`, and `SkipIter` are generic structs whose `next` forwards to the source and
  the closure. Default methods are instantiated on demand for each receiver type, so a chain never instantiates
  every adapter's own adapters.
- Built-in implementations in the compiler: `iter()` and `into_iter()` on arrays, slices, `List`, `Map`, `Chars`,
  and ranges, and the compiler-known iterator types `ArrayIntoIter<T, N>`, `ListIntoIter<T>`, `MapIter<K, V>`,
  `MapIntoIter<K, V>`, and `RangeIter<T>`. A borrowing collection yields `T` when it is `Copy` and `&T` otherwise; a
  map yields `(&K, &V)` and `MapIntoIter` yields owned `(K, V)` and clears each entry it moves; `Chars.next`
  decodes one UTF-8 scalar value; an inclusive range iterator stops at its upper endpoint without computing
  `upper + 1`. `it.sum()` is compiler-known for integer items, and `next` on a non-mutable place is E0113.
- `for` over a user type or an iterator value lowers to `let mut __iter = e; loop { match __iter.next() { Some(pat)
  => body, None => break } }`. An `Iterable<T>` is iterated through `iter()` and its result must be an
  `Iterator<T>` (E0141); a type that is both an `Iterable` and an `Iterator` is iterated through `iter()`, because
  `for` borrows its source. The pattern must match every item (E0006). The stage 0 built-in loop forms and their
  rules are unchanged.
- Eager `List` methods `map`, `filter`, `fold`, `any`, `all`, and `find` in `std`: they borrow the list, pass the
  closure a view of each element, and return owned results (`filter` and `find` clone the element).
- Memory rules IT4-IT12 in the new `spec/iterators.md`: a chain is second-class (E0120, E0123), a view-typed
  parameter may be returned (RT2 now covers an `own` parameter whose type is a view), and a return-origin union
  counts only values that carry a view. Temporary drop flags (`tmp.live`) make a temporary created on one branch of
  a `match` drop only where it exists.
- Tests: `tests/m13_iterators/` adds 21 conformance tests (run and compile-fail) covering `for` over a user type,
  `iter`/`into_iter`/`next`, list, array, slice, map, `Chars`, and range iteration including the inclusive maximum,
  the lazy adapters and their laziness, the eager `List` methods, `sum`, generic iterator/iterable functions, and
  the second-class rejections. The suite is 313 tests (311 in release).
- Documentation: `spec/iterators.md` (new), `spec/memory-rules.md` section 7, `spec/stdlib.md`,
  `spec/stage1-subset.md`, `spec/e9000-audit.md`, `tests/README.md` decision 18, `CLAUDE.md`, and `HANDOFF.md`.


### Milestone 14 — formatting (issue #109, sub-issues #110-#116)

- The decision (`spec/format.md`, issue #110): format strings are a compile-time construct. A string literal's
  placeholders are checked against the call's arguments and each value's type when the program is compiled, and the
  whole string lowers to ordinary `Formatter` calls; nothing is parsed at run time. The page fixes the forms
  (`{expr}`, `{name}`, `{}`, `{n}`, `{expr:spec}`), the escapes (`{{`, `}}`), raw strings (`r"..."`, `r#"..."#`),
  multi-line literals, the specifier grammar, the output format of `Display` and `Debug`, floats, and the radixes.
- Lexer and parser (issue #111): a string literal lexes to a `Str` or an `Interp` token whose pieces are literal
  text and holes; each hole carries its own token stream and the raw specifier with its span, so a diagnostic points
  inside the placeholder. A placeholder's expression may hold brackets, character literals, and string literals, and
  the specifier starts at the first `:` outside brackets. An unterminated `{` or a stray `}` is E0001, reported once
  the string is known to end, and `r"..."` / `r#"..."#` are raw. The AST keeps each placeholder's parsed expression,
  the parsed `FormatSpec`, and spans; `crates/burdin_syntax/src/parser/format.rs` parses the specifier.
- `Display` and `Debug` (issue #112): `std/src/fmt.bd` declares the two interfaces, the `Formatter`, the built-in
  helpers (`__fmt_i64`, `__fmt_u64`, `__fmt_float`, `__fmt_str`, `__fmt_char`, `__fmt_bool`, `__fmt_*` debug forms,
  and the generic `__debug_option`/`__debug_result`/`__debug_list`/`__debug_heap`/`__debug_tuple2..4`), and the
  `__float_str` runtime binding. The compiler implements `Display`/`Debug` for integers, floats, `bool`, `char`,
  `str`, `String`, and `Debug` for `()`, tuples of 2 to 4, `Option`, `Result`, `List`, `Heap`, and `&T`, without an
  impl block. `derives (Debug)` generates the impl block as Burdin source at parse time
  (`crates/burdin_syntax/src/derive.rs`), so a derived `Debug` and a written one are the same; its diagnostics are
  suppressed in favor of E0130 at the offending field, and `Debug` joins the derive requirement table.
- Lowering (issue #113): an interpolated literal, `format(...)`, and `print`/`println`/`eprint`/`eprintln` with more
  than one argument lower to one block that builds the text in a `Formatter`. A user type writes to the formatter
  with `x.fmt(mut f)` / `x.debug(mut f)`; a specifier becomes `set_spec(...)` before the value and `reset_spec()`
  after it. `x.to_string()` formats any type that implements `Display` through the same path (floats included).
  A single-argument `println(text)` is unchanged, so `String`-to-`str` and existing programs behave as before.
- Specifiers (issue #114): width, fill, alignment (`<`, `^`, `>`), `+`, `#`, `0`, precision, and the `x`/`X`/`b`/`o`
  radixes, with `Formatter::pad`, `pad_plain`, and `pad_number` applying them. A signed value in a radix shows its
  two's complement in its own width. Floats are the shortest round-trip text (`snprintf("%.*e")` read back with
  `strtod`, in a new runtime function), or `snprintf("%.*f")` with a precision; `NaN`, `inf`, and `-inf` print as
  such. Precision counts characters for text and digits for floats.
- Compile-time checking (issue #115): every placeholder in a string is reported, not just the first. A missing
  argument, an argument no placeholder uses, a value that does not implement the interface, a specifier that does
  not apply, and a call with arguments whose format string is not a literal are the new codes E0191, E0192, E0193,
  E0190, and E0194. An argument used by more than one placeholder is evaluated once: a place is borrowed and a
  `Copy` value is copied; anything else is E0191. The code that resolves an ordinary method call was split so that
  an already-checked receiver can be used (`method_call_on`), which the lowering needs.
- Code generation emits only reachable functions (`crates/burdin_codegen/src/emit/reach.rs`): the embedded
  standard library used to be generated into every program, so new `std` code (as `fmt.bd` is) added drop flags and
  bounds checks to unrelated programs and made the IR proportional to the library rather than the program. A
  function nothing reaches now gets a two-line `unreachable` body; the roots are `main`, the tests, exported
  functions, `Drop` implementations, and closure bodies. Fixed with it: a function's prologue parameter stores used
  to inherit the previous function's debug location, which LLVM's verifier rejects under `-g`; the location is now
  cleared at each function start.
- Tests (issue #116): `tests/m14_format/` adds 17 conformance tests — interpolation with expressions, escapes, raw
  and multi-line strings, closures that capture into a placeholder, positional arguments and the shared-argument
  rule, every specifier for integers, floats, and text, a user `Display` (including one that honors `pad`), a
  derived `Debug` (structs, enums, generics, `Heap`), a hand-written `Debug` next to a `Display`, and compile-fail
  tests for every new code and for an unterminated brace. The suite is 330 tests (328 in release). The specs are
  `spec/format.md` (new), `spec/grammar.md`, `spec/stdlib.md`, `spec/derives.md`, and `spec/stage1-subset.md`; the
  registry, `docs/error-codes.md`, CI's milestone list, `tests/README.md`, `CLAUDE.md`, `HANDOFF.md`, and this file
  are updated with them.

### Milestone 15 — `any` and `impl` (issue #117)

- The decision (`spec/existentials.md`, AN1-AN12): `any Interface` is an owned, first-class two-word heap box
  `{data, vtable}` that owns its allocation and drops the boxed value through the vtable. A concrete value is boxed
  implicitly wherever an `any I` is the expected type (annotation, argument, return, field, element, `Some`/`Ok`/
  `Err` payload, match arm); the value must implement `I` (E0141) and be first-class, so boxing a view is E0120.
  `any I` satisfies `I` and every interface `I requires`, is never `Copy`, `Clone`, `Eq`, `Ord`, `Hash`, or
  `Default`, and formats through the vtable as `any Display`/`any Debug`.
- Object safety (E0202): an interface is usable as `any` only when no method has its own type parameters, every
  method has a `self` receiver that is not `own self`, no method mentions `Self` outside the receiver or returns a
  view, and every `requires` interface is object-safe. A generic interface instance (`any Store<i32>`) is allowed
  because the instance fixes its arguments. `impl Interface` is accepted only in a parameter list (E0201 anywhere
  else) and is sugar for one hidden type parameter with the interface as its bound, so dispatch is static and `I`
  need not be object-safe.
- Calls and downcasts: `x.m(args)` on `any I` lowers to an indirect call through the vtable slot, with the
  interface's `mut`/`own`/`unsafe` conventions applied. The compiler-known `x.is<T>(): bool` and
  `x.downcast<T>(): Option<T>` take a member type argument (`x.downcast<Circle>()`, the generic-argument spelling of
  grammar rule 8.2); `downcast` consumes the box, moves the value out on a match, and frees the allocation without
  running the drop again. A primitive's compiler-provided `Display`/`Debug` implementation has no vtable function, so
  boxing it is E0204.
- Code generation emits one private vtable constant per `(T, I)` pair with a program-wide type id, the drop glue, and
  one slot per interface method; calls are indirect through the slot. `is` and `downcast` compare the type id, and a
  downcast transfers ownership out of the allocation.
- Tests: `tests/m15_any/` adds 24 conformance tests (run and compile-fail) covering boxing at every expected
  position, a heterogeneous `List<any Shape>`, dispatch and `requires` methods, generic interface instances,
  `Display` through the box, moves and drops, downcasts (generic targets included), object safety, and the new
  diagnostics E0200, E0201, E0202, and E0204. The suite is 354 tests (352 in release).
- Documentation: `spec/existentials.md` (new), `spec/grammar.md` sections 1, 4, and 6, `spec/stage1-subset.md`,
  `tests/README.md` decision 20 and the error-code registry, `docs/error-codes.md`, the CI milestone list,
  `CLAUDE.md`, and this file.

## Stage 0 — 2026-11-14

- Milestones 1-11 complete: lexer and parser, primitives and control flow, type checking, structs/enums/match,
  moves/Drop/Heap, conventions and references, generics/interfaces/derives, modules/extern/unsafe, and the stage 0
  standard library.
- 134 conformance tests pass in debug; 133 pass in release plus one debug-only test. `process.run` reports the child's
  exit code on the POSIX and Windows CRTs.
- LLVM 21 backend through `inkwell`, linked as a shared library on every platform (`crates/burdin_codegen/build.rs`);
  the runtime is generated LLVM IR calling libc directly.
- `burdin build`, `burdin run`, and `burdin test`; the test runner runs one process per test and reports
  `test result: N passed, M failed`.
- `test` blocks, test-mode `assert`/`assert_eq` returning `Result<(), AssertError>`, and abort recovery.
- `std` embedded in the driver: `String`, `List`, `Map`, `Result` with `?`, file I/O, process/environment helpers,
  `CString`/`CPtr`/`CStr`, and the test-mode assertions.
- The m11 LLVM C API program builds and verifies a function through `extern "C"` and `--link-lib LLVM-C`.
- CI runs on the remote: the `linux` job on `ubuntu-24.04` installs LLVM 21 from apt.llvm.org and passes
  check-tests, fmt, clippy, unit tests, build, and every conformance milestone m01-m11. `tests/check-ci.sh`
  guards the workflow's milestone coverage and prerequisites.
