# Official packages

The packages that ship with the compiler. Each folder here is an ordinary Burdin package with its own `burdin.toml`
and `src/`; a program uses one by declaring it in its manifest with `name = true`, or with `burdin add <name>`:

```toml
[dependencies]
json = true
```

and then `use json`. An official package is versioned with the compiler: the release archive and the installers put
this folder next to the `burdin` executable, and the lockfile pins the files of each one (a different compiler brings
different files). The behavior, including where the compiler looks for this folder, is `spec/packages.md` (PKG15 to
PKG18).

Official packages today:

- `json` — JSON reading and writing on the serialization framework (issue #255; `packages/json/README.md`).
- `toml` — TOML 1.0.0 documents, the canonical form, and the framework mapping (issue #256; `packages/toml/README.md`,
  `spec/toml.md`).
- `csv` — a record-at-a-time CSV reader and writer (issue #257; `packages/csv/README.md`).
- `yaml` — a safe YAML subset with anchors resolved (issue #258; `packages/yaml/README.md`).
- `xml` — an XML reader and writer with namespaces and a small DOM (issue #259; `packages/xml/README.md`).
<<<<<<< HEAD
- `http` — an HTTP/1.1 client with redirects, timeouts, safe retries, and per-host connection reuse (issue #260; `packages/http/README.md`).
=======
- `crypto` — hashes, HMAC, HKDF, PBKDF2, ChaCha20-Poly1305, AES-GCM, X25519, and Ed25519 in pure Burdin; an experiment,
  not a security claim (issue #266; `packages/crypto/README.md`, `spec/crypto.md`).
>>>>>>> origin/main

Later libraries are added here, one folder each, as they are written.

A test that needs an official package points `BURDIN_PACKAGES` at the fixtures in `tests/packages/official` instead of
using the real ones, so it does not change when a package here does.
