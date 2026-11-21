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
- `http` — an HTTP/1.1 client with redirects, timeouts, safe retries, and per-origin connection reuse, and `https` over
  `tls` (on by default, so a program that uses it links OpenSSL 3; `http = { official = true, default-features = false }`
  leaves TLS out) (issues #260, #262, #878; `packages/http/README.md`, `spec/http-client.md`).
- `crypto` — hashes, HMAC, HKDF, PBKDF2, ChaCha20-Poly1305, AES-GCM, X25519, and Ed25519 in pure Burdin; an experiment,
  not a security claim (issue #266; `packages/crypto/README.md`, `spec/crypto.md`).

- `tls` — TLS 1.2 and 1.3 over OpenSSL 3 through the C bridge: certificates and keys, verified client connections, servers,
  and a `Read`/`Write` stream over `TcpStream` (issue #262; `packages/tls/README.md`, `spec/tls.md`). It links a system
  OpenSSL; an experiment, not a security claim.

Later libraries are added here, one folder each, as they are written.

A test that needs an official package points `BURDIN_PACKAGES` at the fixtures in `tests/packages/official` instead of
using the real ones, so it does not change when a package here does.
