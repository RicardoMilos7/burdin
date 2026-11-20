# `burdin watch`

Watch mode (stage 3; issue #632). `burdin watch [check|build|test|run] <target>` runs a command, waits for the files it
reads to change, and runs it again; `--watch` on `build`, `run`, `test`, and `check` is the same thing. This file is the
exact behavior, with a rule ID for each point and the tests that cover it. Every behavior below is covered by
`tests/check_watch.py` (the end-to-end runs against a built driver) and by the unit tests in
`crates/burdin_driver/src/watch.rs` (the pure parts: the flags, the positional words, the imports, the snapshot).

## The command

```
burdin watch [check|build|test|run] <file.bd | package directory> [OPTIONS] [--no-clear]
burdin <check|build|test|run> <target> --watch [--no-clear] [OPTIONS]
```

The command defaults to `check`. Every other word is the command's own: a watched command takes exactly the options the
plain command takes (`--release`, `-o`, filters, `-- <program arguments>`, the diagnostics options, ...), and
`--watch` and `--no-clear` are removed before the command sees them.

**WT1. The same commands.** Each run is the `burdin` executable started as a child process with the command's
arguments, so a watched run prints exactly what the plain command prints, exits with the same status (which the watcher
reports instead of exiting), and uses the cached and incremental builds when they exist (issue #243): there is no
second code path to drift from the first. `burdin` exits with status 2 for a usage error: a missing or unknown target,
a directory without `burdin.toml`, or `--no-clear` without `--watch` on a plain command. Otherwise it runs until it is
interrupted.
Tests: `tests/check_watch.py` (the usage errors; every run).

**WT2. What is watched.** For a package: every file under its directory, and the same for every path dependency
(transitively), so the sources, `burdin.toml`, `build.bd`, headers, and `embed`ded files are all inputs. For a one-file
target: that file. For `build` and `run`, the files and directories a build read besides the sources (`--emit-deps`,
the files `embed` and a build script declared) are watched too, wherever they are; a declared directory input stands
for every file under it, recursively (`spec/build-scripts.md` BS5). A change is a file that was added, removed, or
whose modification time or length changed.
Tests: `tests/check_watch.py` (an edit, a new source file, a file inside a build script's declared directory input
outside the package); `watch.rs` (the snapshot).

**WT3. What is not.** The `target` directory at the top of a watched directory, every entry whose name starts with a dot
(`.git` and other version-control metadata), `burdin.lock` (the commands rewrite it), and the files the command itself
writes: the executable `burdin build` leaves in the working directory (named after the package, or `-o`'s path),
`--emit-ir`'s and `--emit-deps`'s files. A command's own output never starts the next run.
Tests: `tests/check_watch.py` (files under `target/`, `.git/`, and `burdin.lock`; the build's executable inside the
package directory); `watch.rs` (the snapshot).

**WT4. Debounce.** The watcher looks at the files every 50 ms. After it sees a change it waits until the files have been
unchanged for 150 ms, so an editor's save that touches several files, or writes one in two steps, starts one run. An
edit made while a command is running is seen as soon as the run ends and starts the next run.
Tests: `tests/check_watch.py` (one run per edit).

**WT5. The screen.** Before each run, including the first, the screen is cleared with `ESC [ 2 J ESC [ 3 J ESC [ H`
on standard output, unless `--no-clear` is given. The watcher's own lines go to standard error, start with `watch:`, and
name what happened: `watch: <file> changed, rerunning` (the first changed file, and the number of others), `watch:
<command> finished` or `watch: <command> failed with exit status <n>`, and `watch: waiting for changes (Ctrl-C to
stop)` once a run is over. A failing run is not the end of watching: fixing the file starts the next run.
Tests: `tests/check_watch.py` (every run's lines; the clear sequence and `--no-clear`).

**WT6. `run` restarts the program.** `watch run` builds with `burdin build` (with the run options, into a temporary
file), starts the program with the arguments after `--`, and leaves it running while it waits for changes. A change
kills the program (it is not left running), builds again, and starts the new one; when the program exits by itself the
watcher says `watch: the program exited with exit status <n>` and keeps waiting. A build that fails starts no program.
Tests: `tests/check_watch.py` (`watch run`: the first output, the restart).

**WT7. `test` runs the changed modules first.** After a change, `burdin test` is run twice when it can be: first with a
filter for each changed module and each module that imports one of them, directly or through others (`use` declarations
are read from the source text; `use a.b.item` makes the module the importing one of `a.b`), written `<module>.`; the
watcher says `watch: running the tests of <modules> first`. Then, if that run passed, the whole suite runs as it would
without watching (so the first modules' tests run twice: a filter that excludes them cannot be written exactly). A
failing first run is the result of the change and the whole suite does not run, so the failure is not buried. The first
run after starting, and every run where the change cannot be mapped to modules, run the whole suite only: a change to
anything but a `.bd` file under the package's own `src/` (the manifest, a build input, a path dependency, a new file the
module list does not know), a change that reaches `src/main.bd` (the entry module's tests have no module prefix to
filter by), a one-file target, and a command line that selects tests itself (`--filter`, a positional filter, `--exact`,
`--shard`, `--failed`, `--list`, `--doc`, `--ignored`, `--workspace`).
Tests: `tests/check_watch.py` (`watch test`: the unsplit first run; an edit to `alpha` runs `alpha` and its dependent
`beta` and not `gamma`, and then all three); `watch.rs` (the imports and the dependents).

## Notes

- The files are polled, not subscribed to, so it works the same on every platform and over network and container file
  systems, at the price of a delay of up to the poll interval plus the debounce. A change that keeps both the
  modification time and the length of a file is not seen.
- Ctrl-C stops the watcher and the program `run` started with it.
