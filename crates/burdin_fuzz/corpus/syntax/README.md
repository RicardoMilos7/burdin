# Lexer and parser fuzz corpus

Each `.bd` file is a source text the fuzzer parses. The directory is committed and replayed by
`crates/burdin_fuzz/tests/corpus.rs` during `cargo test --workspace`, so a panic raised by any file fails
every CI job.

- **A seed that found a panic** is written by the fuzzer as `crash-<hash>.bd`. Keep its bytes exactly as
  found: it is the reproduction, and it stays in the corpus with the fix. Do not rename it to something
  friendlier or re-indent it.
- **A hand-written seed** may start with a comment describing what it reaches; the comment is part of the
  input but only the lexer's trivia handling sees it.
- The files are checked in with LF endings by `.gitattributes`; a test does not rely on CRLF, which the
  mutator inserts at run time.

The scheduled `fuzz` job in `.github/workflows/ci.yml` runs a longer session. When it fails, it uploads
the failing inputs as the `fuzz-crashes` artifact; download it, copy each `.bd` file here, fix the panic,
and commit both together. The replay test then keeps the panic from coming back.

Run the long session locally with:

```
tests/fuzz.sh --seconds 60
```
