# Build profiles

A profile is the set of settings a build compiles with: the optimization level, whether debug information is
emitted, whether debug assertions and integer-overflow checks are on, and the panic strategy. The switch used to be
one `--release` flag; issue #339 replaced it with named profiles that a package can define and customize. The
behavior is `spec/profiles.md`; the tests are `tests/check_profiles.py`, plus the unit tests in
`crates/burdin_driver/src/profile.rs`.

## BP1. The built-in profiles

Four profiles exist without a manifest:

| Profile | `opt-level` | `debug` | `debug-assertions` | `overflow-checks` | `panic` | Default for |
| --- | --- | --- | --- | --- | --- | --- |
| `dev` | 0 | false | true | true | abort | `build`, `run`, `check`, `fuzz`, `debug` |
| `release` | 2 | false | false | false | abort | `--release` |
| `test` | 0 | false | true | true | abort | `burdin test` |
| `bench` | 2 | false | false | false | abort | `burdin bench` |

`test` inherits `dev` and `bench` inherits `release`. Their only difference from their parent is the name, which
selects the default of a command; the settings are the same. A package customizes a built-in by writing a
`[profile.<name>]` table; it may change every setting but not `inherits` (BP2).

`debug` is the source-level debug information `-g` also turns on. It is off in every built-in profile, so a normal
build is byte-for-byte what it was before profiles existed; `-g` and a profile's `debug = true` both turn it on.
`debug-assertions` keeps `debug_assert*` calls and error trace frames (`spec/errors.md`, ERR7). `overflow-checks` makes `+`, `-`, `*`, negation, and `MIN / -1`
panic on overflow; when it is off they wrap, and `comptime` evaluation follows the same rule
(`spec/comptime.md`, CT1).

`panic = "abort"` is the only strategy: a Burdin panic writes its message and ends the process, so `unwind` is
rejected with an explanation rather than accepted and ignored.

## BP2. Custom profiles

A package defines a profile in its `burdin.toml`:

```toml
[profile.fast]
inherits = "release"
opt-level = 3
overflow-checks = true
```

- `inherits` is required, and names a built-in profile or another custom one. A custom profile that inherits
  `dev` starts with every `dev` setting and changes only the keys it names.
- The inheritance graph is acyclic; a cycle is an error that prints the chain.
- A profile setting the parent does not name is inherited unchanged.
- Only the entry package's `[profile...]` tables are read. A dependency's tables are ignored, as in Cargo, so a
  dependency cannot change the build it is part of.

## BP3. Settings

A `[profile.<name>]` table accepts exactly these keys:

| Key | Values | Default |
| --- | --- | --- |
| `inherits` | a profile name (custom profiles only) | required for a custom profile |
| `opt-level` | `0`, `1`, `2`, `3` | inherited |
| `debug` | `true`, `false` | inherited |
| `debug-assertions` | `true`, `false` | inherited |
| `overflow-checks` | `true`, `false` | inherited |
| `panic` | `"abort"` | `abort` |

`opt-level` maps to LLVM's four levels: 0 runs no optimization pipeline, 1 is `O1`, 2 is `O2`, and 3 is `O3`.
There is no `s` or `z` yet. An unknown key, a value of the wrong type, an unsupported `opt-level`, and
`panic = "unwind"` are errors reported at the entry's line. Every profile table the entry manifest writes is
validated, not only the selected one, so a typo cannot hide in an unused profile; the first error in file order is
reported. A package that names a profile that does not exist sees a usage error that lists the profiles available
(the built-ins and the ones the manifest defines).

The `opt-level` of a `[profile.<name>]` table is the level a build with that profile uses. The command line can
replace it for one invocation with `-O` (BP4). A profile with no `opt-level` keeps its built-in level: 0 for `dev`
and `test`, 2 for `release` and `bench`.

## BP4. The command line

`--profile <name>` selects a profile on `build`, `run`, `test`, `bench`, `check`, `fuzz`, and `debug`.
`--release` is exactly `--profile release`; giving both is an error unless the name is `release`.

The default is the profile the command's name suggests (BP1): `build` and `run` use `dev`, `test` uses `test`,
`bench` uses `bench`, and the others use `dev`. `burdin watch` passes the arguments through, so it selects the
same profile as the command it reruns.

`-O<n>` sets the optimization level of one invocation: `-O0`, `-O1`, `-O2`, or `-O3`, or `-O <n>` (issue #289).
It takes the commands `--profile` takes, and `burdin watch` passes it through. It replaces the `opt-level` of the
selected profile, however the profile was chosen, and changes nothing else (BP5). `-O2` is the level `release` and
`bench` use; it does not select them. A level outside 0 to 3 is a usage error, as is an `-O` with no level, and two
`-O` options that name different levels, as `--release` with another `--profile` is. A repeated `-O` with the same
level is accepted.

`burdin asm` and `burdin bloat` keep their `--debug` switch, which selects `dev` instead of their `release`
default; `burdin repl` keeps `--release`. They do not take `--profile` or `-O`.

## BP5. What a profile changes

A profile changes code generation deterministically:

- `opt-level` sets the target machine's optimization level and the LLVM pass pipeline.
- `debug` adds DWARF/CodeView metadata, the same as `-g`.
- `debug-assertions` removes or keeps `debug_assert*` calls and error trace frames.
- `overflow-checks` emits or removes the overflow checks of arithmetic.
- `panic` is fixed to `abort`.

The optimization level and the checks are independent (issue #289). `opt-level` and `-O<n>` choose the LLVM pipeline
and the target machine, and nothing else: `overflow-checks` and `debug-assertions` keep the value the profile gives
them at every level. So `dev -O3` still panics on overflow, `--release -O0` still wraps, and an optimized build has
exactly the checks its profile asks for. The built-in `release` and `bench` profiles turn both checks off, as they
always have; a profile that wants them in an optimized build sets `overflow-checks = true`, as the `fast` example in
BP2 does. No check is removed or added by a level.

Two builds of one program with two profiles that differ only in a setting that does not affect it (for example,
`debug` with optimization off) differ only in the metadata. The generated code of a program whose source has no
overflow, assertion, or debug-information consequence is otherwise the same.

## BP6. Output directories and caches

The directory under `target/burdin/` names the profile's *output*, not its settings: `dev` and `test` write
`debug`, `release` and `bench` write `release`, and a custom profile writes its own name.

- A build script's cache key contains the settings (`opt-level`, `debug`, `debug-assertions`,
  `overflow-checks`, `panic`), not the profile name (`spec/build-scripts.md`, BS5). `dev` and `test` therefore
  share a script run while they have the same settings, and a custom profile that changes a setting gets its own.
- `BURDIN_PROFILE` and a script's `argv[2]` name the same directory (`spec/build-scripts.md`, BS3).
- `burdin test --failed` keeps `test-failed.txt` in `<package>/target/burdin/<profile>/`, so the default `test`
  profile keeps it in `debug/`, as it always has.

## BP7. Per-package overrides

The program is compiled and linked as one binary, so `opt-level`, `debug`, and `panic` are properties of the whole
build. Two settings can differ per package, because they are decided at each operation:

```toml
[profile.release.package.crypto]
overflow-checks = true
debug-assertions = false
```

A `[profile.<name>.package.<package>]` table accepts `overflow-checks` and `debug-assertions`; any other key is
an error. The override applies to the code of the functions whose source file is under that package's directory.
A package name that is not in the build is ignored. A custom profile inherits its parent's package overrides, and
its own tables override them key by key.

The `comptime` interpreter evaluates the whole program with the profile's settings, not the per-package overrides;
those affect generated code only. This is the one place where a per-package setting is not observable in a
compile-time result.

## BP8. What is not a profile setting yet

`lto`, `strip`, `codegen-units`, `incremental`, `panic = "unwind"`, and `s`/`z` optimization are not profile
settings. `--lto`, `--strip`, and `-j` stay command-line options, because they change the link or the build's
parallelism rather than the meaning of the code.

## Tests

`tests/check_profiles.py` runs fixture packages and checks:

| Check | Covers |
| --- | --- |
| the default `dev` build panics on overflow, and `--profile release` wraps | BP1, BP5 |
| a custom profile that inherits `dev` and sets `overflow-checks = false` wraps | BP2, BP3 |
| `--release` and `--profile release` produce the same outcome | BP4 |
| `debug-assertions` decides whether `debug_assert` runs in a build and in a test | BP1, BP5 |
| `[profile.test] overflow-checks = false` changes `burdin test` | BP4 |
| a per-package override turns checks on for a dependency's function only | BP7 |
| `[profile.<name>.package.<pkg>]` rejects `opt-level` | BP7 |
| an unknown profile, an unknown setting, a bad value, `panic = "unwind"`, and an inheritance cycle fail with stable messages | BP3, BP4 |
| the `--help` of `build` lists `--profile` and `-O` | BP4 |
| `-O0` is the default `dev` IR, `-O2` and `-O 2` match each other and differ from `-O0` | BP4, BP5 |
| `[profile.dev] opt-level = 2` builds the level the manifest names, and `-O0` replaces it | BP3, BP4 |
| `dev -O3` still panics on overflow, and `--release -O0` still wraps | BP4, BP5 |
| `-O4`, `-O` with no level, and `-O1 -O2` are usage errors (exit 2) with stable messages | BP4 |

The unit tests in `crates/burdin_driver/src/profile.rs` cover the built-in table, inheritance, cycles, and the
`opt-level` mapping without a compiler run.
