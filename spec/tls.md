# TLS

The `tls` package (stage 3, issue #262, with #322 the backend decision and binding, #323 the client, and #324 the
server and the tests; `packages/tls`). It is an ordinary official package (`spec/packages.md`, PKG15-PKG18) whose
engine is OpenSSL 3, called through the C bridge (`spec/stdlib.md`, "C interop"; `spec/bindgen.md`) and written as
hand-made `extern "C"` declarations. This page says what the package is built on, what it exposes, and which test
witnesses each rule. The rule IDs are TLS1-TLS8. Phase A (issue #322) landed TLS1, TLS2, and TLS7 and the bindings; phase B (#323, #324)
landed TLS3-TLS6: certificates and keys, the client and server configurations, the stream, and the in-process tests.

> **This is an experiment, not a security claim.** Burdin is a personal, AI-assisted language experiment. The package
> has had no audit. It keeps the verification defaults of OpenSSL on and offers no way around them except one loudly
> named switch (TLS3), which proves it checks what it says it checks, not that it protects anything you care about.

## TLS1. The backend per platform (decision, issues #322 and #878)

TLS is provided by the operating system's TLS stack on each platform, reached through a hand-written binding and one
engine interface (TLS2). The bindings declare the functions they use by hand, with `CPtr<T>` handles and `unsafe` at
every call; the safe layer is Burdin code on top of them. The compiler gains no TLS code.

| Platform | Stack | Binding |
| --- | --- | --- |
| Windows | SChannel (`secur32.dll`), with Crypt32 for certificates and CNG (`ncrypt.dll`) for keys | `packages/tls/src/schannel.bd` |
| Linux | the system's OpenSSL 3 (`libssl`, `libcrypto`) | `packages/tls/src/openssl.bd` |
| macOS | OpenSSL 3 (Homebrew) until the Secure Transport backend lands | `packages/tls/src/openssl.bd` |

Windows uses the platform's own stack, not OpenSSL. The earlier choice of OpenSSL 3 on every platform (#322) needed a
third-party library on Windows, which #878 made the driver download; SChannel ships with Windows and needs nothing
installed. The stacks are different APIs (their handshake models, their certificate objects, and what a program may
configure), so each backend carries its own implementation. The rules of TLS2 to TLS8 are the same on every platform,
except where TLS1a names a difference. The package reports its stack through `tls.backend()`: `SChannel` on Windows, and
on Linux and macOS the OpenSSL version text, which starts with `OpenSSL 3`.

The macOS Secure Transport backend is planned and not yet implemented; until it lands, macOS keeps OpenSSL 3. Native TLS
written in Burdin is #884.

Tests: `tests/s3_tls/conformance` runs on every platform (the same expected output); `tests/s3_tls/link_windows` checks
that the Windows build reports the SChannel backend through `tls.backend()`.

**TLS1a. Windows: where SChannel differs.** The rules below are the Windows exceptions to the rest of this page. Each one
is either a behavior that matches the other platforms or a named limit with its `TlsError`.

- The Windows build links `secur32`, `crypt32`, and `ncrypt` and no OpenSSL library; the `tls` package's `openssl_link`
  dependency is declared for platforms other than Windows (PKG27), so the `openssl` feature has no effect on Windows.
- The trust store of `ClientConfig.new()` is the Windows root store. A `TrustStore` with its own roots (`with_trust`,
  `require_client_certificate`) verifies a chain against those roots only: the Windows root store is not consulted, as
  with OpenSSL.
- Protocol versions, ALPN, SNI, and hostname checks follow TLS3 and TLS8. Where SChannel cannot provide one of them,
  that item is listed here with the error it returns.
- Ed25519 private keys cannot be used by SChannel: CNG's key import rejects an Ed25519 PKCS#8 key. On Windows,
  `ServerConfig.from_pem` and `ServerConfig.new` with an Ed25519 certificate and key return
  `TlsError.Unsupported("Ed25519 private keys cannot be used by SChannel on Windows")`, whose text is
  `unsupported: Ed25519 private keys cannot be used by SChannel on Windows`. `PrivateKey.from_pem` still loads an
  Ed25519 key on Windows; the refusal comes when a configuration takes it. Linux and macOS support Ed25519 as before.
  Checked by `tests/s3_tls/ed25519_windows` (Windows) and `tests/s3_tls/ed25519` (Linux and macOS).

## TLS2. The engine model: memory BIOs

A connection is an OpenSSL `SSL` object whose two `BIO`s are **memory BIOs** (`BIO_s_mem`, installed with
`SSL_set_bio`). OpenSSL never sees a socket descriptor. The Burdin side moves the bytes:

- bytes read from the `TcpStream` are written into the connection's *input* BIO with `BIO_write`;
- bytes OpenSSL wants to send are taken from the *output* BIO with `BIO_read` and written to the `TcpStream`;
- `SSL_connect`, `SSL_accept`, `SSL_read`, `SSL_write`, and `SSL_shutdown` run between those two steps, and a result of
  `SSL_ERROR_WANT_READ` means "write the output to the socket, then read more from it", `SSL_ERROR_WANT_WRITE` means
  "write the output to the socket and try again".

So no OpenSSL call blocks: a call that needs the peer returns at once, and the wait happens in `TcpStream`'s reads and
writes, which park the task on the reactor (`spec/io.md`, IO1 and IO2) and leave the worker free. The engine is
therefore task-friendly with no helper-pool thread and no `blocking` extern function (IO4), and it works over any
`Read`/`Write` pair, not only a socket. The output BIO is drained after every OpenSSL call, including the ones that
returned success, because a successful `SSL_read` may have queued a key update or an alert.

An OpenSSL object is owned by one Burdin value with a destructor, so a dropped `TlsStream` frees its `SSL` (and with it
the BIOs), and a context is freed when its last connection and its configuration are gone. The error queue of OpenSSL is
per thread, so a failing call is followed at once, with no task switch between them, by the code that drains it into a
`TlsError` (TLS6); a task never runs a call and its error read on different threads.

Tests: `tests/s3_tls/link` (a client with memory BIOs produces a ClientHello with no socket) and the stream tests of
TLS5, which run the pump over real sockets in both directions.

## TLS3. Configuration and verification

The surface (the full list is `packages/tls/README.md`):

```
struct ClientConfig {
    fn new(): Result<ClientConfig, TlsError>                          // verification on, system roots, TLS 1.2 minimum
    fn with_trust(trust: TrustStore): Result<ClientConfig, TlsError>  // exactly these roots
    fn min_version(mut self, v: TlsVersion): Result<(), TlsError>
    fn max_version(mut self, v: TlsVersion): Result<(), TlsError>
    fn alpn(mut self, protocols: List<String>): Result<(), TlsError>
    fn client_certificate(mut self, chain: List<Certificate>, key: PrivateKey): Result<(), TlsError>   // mutual TLS
    fn insecure_skip_verify(mut self)                                 // see below
}
struct ServerConfig {
    fn new(chain: List<Certificate>, key: PrivateKey): Result<ServerConfig, TlsError>
    fn from_pem(chain_pem: str, key_pem: str): Result<ServerConfig, TlsError>
    fn min_version(mut self, v: TlsVersion): Result<(), TlsError>
    fn max_version(mut self, v: TlsVersion): Result<(), TlsError>
    fn require_client_certificate(mut self, roots: TrustStore): Result<(), TlsError>
}
enum TlsVersion { Tls12, Tls13 }
```

A configuration owns an `SSL_CTX`; every connection made with it shares the context. It is built on one task with the
builder methods and is only read afterwards, so it is `Send` and `Sync` (an `unsafe` impl, SH5).

- **Verification is on by default.** A client verifies the server's certificate chain against the trust store and
  checks that the certificate names the host it connected to. A handshake whose chain does not verify, whose
  certificate is expired, or whose name does not match fails with a `TlsError` (TLS6); the connection is never usable
  "with a warning". A chain that depends on an intermediate the server did not send fails too.
- **The trust store** is a `TrustStore` (TLS4): the system's (`SSL_CTX_set_default_verify_paths`: OpenSSL's default
  certificate file and directory, or those named by `SSL_CERT_FILE` and `SSL_CERT_DIR`), certificates added by the
  program, or both. `ClientConfig.new()` trusts the system's; `with_trust` trusts exactly what the store holds, so a
  program that pins its own authority trusts nothing else. An empty store fails every verification; it is not an error
  to build one.
- **Hostname verification and SNI are on by default and tied to one input.** `TlsStream.connect(tcp, server_name,
  config)` takes the name to verify. It sets the server name indication (`SSL_ctrl` with
  `SSL_CTRL_SET_TLSEXT_HOSTNAME`) and the name OpenSSL matches against the certificate (`SSL_set1_host` for a DNS name
  against the subject alternative names; `SSL_get0_param` and `X509_VERIFY_PARAM_set1_ip_asc` for an IP address against
  its IP entries) from that same value, so the two cannot differ. An IP address is not sent as SNI. A mismatch is
  `TlsError.HostnameMismatch`, reported from the verify result (`X509_V_ERR_HOSTNAME_MISMATCH` or
  `X509_V_ERR_IP_ADDRESS_MISMATCH`).
- **The minimum protocol version is TLS 1.2** (`SSL_CTX_set_min_proto_version`, the `SSL_CTX_ctrl` code 123), and TLS 1.3
  is negotiated when both sides offer it. SSL 3, TLS 1.0, and TLS 1.1 are not available. `min_version` can raise the
  minimum to 1.3 and `max_version` can cap the maximum at 1.2 (a server limited to 1.2 is how the tests make a version
  mismatch); a range that no version satisfies fails the handshake, not the setter.
- **ALPN** (`ClientConfig.alpn`) offers the protocols in order on a client; the stream reports the one the peer chose,
  or `None` (`TlsStream.alpn_protocol()`). A server's selection is TLS8.
- **`insecure_skip_verify`** is the single way to turn verification off. It is named for what it does, it is the only
  setting that can make a handshake succeed with an unverified peer, and the documentation says so in a warning. It
  sets `SSL_VERIFY_NONE` and skips the hostname check; it does not skip the protocol-version minimum.
  `skips_verification()` reports it. The package's own tests use it to show that verification is what rejects the
  other cases (the same connections succeed with the switch on).
- **Mutual TLS** is `ClientConfig.client_certificate` (the chain and key a client presents) and
  `ServerConfig.require_client_certificate(roots)` (`SSL_VERIFY_PEER | SSL_VERIFY_FAIL_IF_NO_PEER_CERT` against the
  roots). In TLS 1.3 a client's handshake can finish before the server has judged its certificate, so a rejected client
  may see the failure on its first read.

Tests: `tests/s3_tls/conformance` sections "verification" (`expired`, `unknown authority`, `system trust store`,
`self-signed`, the two wrong-host cases, `wrong address`, `intermediate missing from the chain`, the four
`insecure_skip_verify` cases, `self-signed, trusted by name`), "versions" (`TLS 1.2 server, client needs 1.3`,
`TLS 1.2 server, default client`, `TLS 1.3 server, client limited to 1.2`, `defaults`), "names and protocols" (`sni`,
`ip address`, `alpn`, `rsa leaf`, `ed25519 leaf`), and "mutual TLS" (`with a certificate`, `without a certificate`,
`with an untrusted certificate`).

## TLS4. Certificates and keys

- **Inputs are PEM text, DER bytes, or files.** `Certificate.from_pem`, `chain_from_pem`, `from_der`, `load_pem_file`,
  and `load_chain_file`; `PrivateKey.from_pem`, `from_der`, and `load_pem_file`; `TrustStore.add_pem` and
  `add_pem_file`. Memory BIOs (`BIO_new_mem_buf`) and `PEM_read_bio_X509`, `d2i_X509`, `PEM_read_bio_PrivateKey`, and
  `d2i_AutoPrivateKey` do the parsing; the file functions read the file with `std` and parse the text.
- **A `Certificate` and a `PrivateKey` own an OpenSSL object** (`X509`, `EVP_PKEY`) and free it when dropped. They are
  `Send` and `Sync` (an unmodified X.509 object is safe to share; its reference count is atomic). `Certificate.duplicate`
  makes a second owner (`X509_up_ref`); the library's `Clone` cannot be written by hand yet (E9000).
- **Accessors:** `subject` and `issuer` (`X509_NAME_oneline`, as `/CN=localhost`), `not_before` and `not_after`
  (`ASN1_TIME_print`), `fingerprint` (SHA-256 of the DER, lower-case hex), and `to_der`.
- **A chain** is the leaf first, then its intermediates, in one PEM text. The server sends all of them. A client
  verifies against its roots, using the intermediates the server sends.
- **A private key** may be RSA, ECDSA, or Ed25519, unencrypted. A PEM text that says `ENCRYPTED` is refused
  (`TlsError.BadKey`): the package has no password callback, because a callback is a C function pointer and the C bridge
  cannot pass one (TLS8). Reading a key passes a non-null empty password to OpenSSL, so a key it cannot read never makes
  OpenSSL prompt on the terminal. The key is checked against the certificate when the configuration is built, so a
  mismatch is `TlsError.KeyMismatch` there and not in the first handshake.
- **Errors at load time** are a `TlsError` (`BadCertificate`, `BadKey`, `KeyMismatch`, or `Io` for a file that cannot be
  read); text that holds no certificate is `BadCertificate` ("no certificate found in the PEM text"), not an empty
  success.
- **No X.509 object beyond the accessors is exposed.** A stream reports the peer's leaf as a `Certificate`
  (`peer_certificate()`), whose `to_der` a program may hand to the X.509 package when #559 lands.

Tests: `tests/s3_tls/conformance` section "certificates, keys, and configurations" (`root`, `der round trip`,
`duplicate`, `garbage der`, `chain`, `bundle`, `plain text`, `empty text`, `a key is not a certificate`, `file`,
`missing file`, the key cases, `encrypted key`, `server mismatched key`, `server empty chain`, the client configuration
cases). `tests/check_tls.py` checks the committed fixtures with the `openssl` tool.

## TLS5. The stream

```
struct TlsStream {
    fn connect(own tcp: TcpStream, server_name: str, config: ClientConfig): Result<TlsStream, TlsError>
    fn accept(own tcp: TcpStream, config: ServerConfig): Result<TlsStream, TlsError>
    fn recv(mut self, mut buf: List<u8>, max: usize): Result<usize, TlsError>
    fn send(mut self, data: [u8]): Result<usize, TlsError>
    fn send_all(mut self, data: [u8]): Result<(), TlsError>
    fn close(mut self): Result<(), TlsError>
    fn abort(mut self)
    fn alpn_protocol(self): Option<String>
    fn tls_version(self): String                  // "TLSv1.3"
    fn cipher(self): String
    fn peer_certificate(self): Option<Certificate>
    fn server_name(self): Option<String>          // on a server: the SNI the client sent
    fn set_read_timeout(mut self, Option<Duration>)
    fn set_write_timeout(mut self, Option<Duration>)
}
TlsStream: Read
TlsStream: Write
```

The stream owns a `TcpStream` (not a generic transport: a `Drop` type cannot give its field back, so `close` does not
return the socket; the socket closes when the stream is dropped) and the engine (TLS2).

- `connect` and `accept` run the handshake to completion on the calling task and return a stream, or fail with the
  `TlsError` that stopped it and close the socket. A read timeout set on the `TcpStream` before the handshake applies to
  the handshake's waits and surfaces as `TlsError.Io` with `IoErrorKind.TimedOut` (`TlsError.is_timeout()`). When a
  handshake fails, any alert OpenSSL queued is sent to the peer first, best effort.
- `recv` returns decrypted bytes, `Ok(0)` at a clean end of stream (the peer's `close_notify`, and every later call),
  and an error for anything else. **A socket that ends without a `close_notify` is `TlsError.UnexpectedEof`, not
  `Ok(0)`**: an attacker who cuts the connection must not look like an orderly end. After a fatal error every call is
  `TlsError.Closed`; after a timeout the stream stays usable. `send` encrypts at most one record (16 KiB) and sends all
  the ciphertext to the socket before it returns, so a short count is only the record limit and `send_all` is the usual
  way to write.
- `Read.read` and `Write.write` are `recv` and `send` with the error as an `IoError` (`TlsError.to_io`): a transport
  error keeps its own value, a truncation is `IoError.UnexpectedEof`, and the rest are `IoError.InvalidData`.
- `close` sends a `close_notify`, flushes it, and makes every later call `TlsError.Closed`. `abort` ends the stream with
  no `close_notify`, as a crash or an attacker would (the tests use it to make the truncation case). Dropping a
  stream that is neither closed nor failed sends a best-effort `close_notify` with a 200 ms write timeout and never
  blocks longer or panics.
- `TlsStream` is `Send` (an `unsafe` impl) and not `Sync`: the engine is one mutable state machine, and a second task
  needs a second connection or a channel.

Tests: `tests/s3_tls/conformance` sections "handshake and data" (`echo`, `large echo` with 200 KiB in one `send_all`
and 50 small messages, `push` of 300 000 bytes read to a clean end, `two streams` on two tasks), "the end of a stream"
(`close_notify`, `truncation`, `close`, `write after close`), and "bad peers" (`plain-text peer with a reply`,
`plain peer that closes`, `garbage record`, `silent peer`, `read timeout`, `after the timeout`, `Read`).
The planned engine-only test (two engines pumping each other with no sockets) is not written: `Engine` is not public,
and the stream tests cover the pump.

## TLS6. Errors

Every failure is a `TlsError` returned from a `Result`, never a panic and never a compiler diagnostic (so, like
`spec/http-client.md`, the rules are pinned by messages in a conformance program rather than by registry codes).
`TlsError` is an `error enum`, so a program can match it:

```
error enum TlsError: Clone, Debug {
    Io(IoError)                 // the transport failed (a timeout is IoErrorKind.TimedOut)
    Handshake(String)           // the peer or OpenSSL stopped the handshake; OpenSSL's reason text
    Protocol(String)            // an error after the handshake (a bad record, an alert); the reason text
    CertificateInvalid(String)  // the chain did not verify; X509_verify_cert_error_string's text
    HostnameMismatch(String)    // the certificate is valid but does not name this host
    UnexpectedEof               // the socket ended without close_notify (truncation)
    Closed                      // the stream was closed or has failed
    BadCertificate(String)      // TLS4
    BadKey(String)              // TLS4
    KeyMismatch                 // TLS4
    Config(String)              // an invalid setting (an ALPN name of 0 or over 255 bytes, a bad host name)
    Unsupported(String)         // the linked OpenSSL is older than 3.0
}
```

`CertificateInvalid`'s text is OpenSSL's for the verify result ("certificate has expired", "unable to get local issuer
certificate", "self-signed certificate"); a handshake's own text is the reason of the first entry of the error queue
(`ERR_reason_error_string`) and differs between OpenSSL versions, so the tests print only the variant of a `Handshake`
or `Protocol` error. A verification failure is recognized by the queue's "certificate verify failed" and then read
from `SSL_get_verify_result`, so a handshake that fails for another reason with `insecure_skip_verify` on is not
reported as a certificate error. `Display` is one line; `is_timeout()` and `is_verification()` classify an error. `TlsError` is `Clone` (`IoError` gained
`Clone` for it), so the `http` package can carry one inside its own `Clone` error (`HttpError.Tls`, `spec/http-client.md`,
HC17). The
package never swallows an error to retry as an insecure connection.

Tests: `tests/s3_tls/conformance` prints a message or a variant for each failure above, produced by real failures.

## TLS7. Linking, installing, and the optional OpenSSL

The native libraries are declared by a package of their own, `openssl_link` (`packages/tls/openssl_link`), which is an
optional dependency of `tls`:

```toml
[dependencies]
openssl_link = { path = "openssl_link", optional = true }

[features]
default = ["openssl"]
openssl = ["dep:openssl_link"]
```

Its manifest holds the `[link]` table (PKG34): `ssl` and `crypto` on Unix. The package is declared only for platforms other
than Windows (PKG27), since Windows links SChannel (TLS1). A
package's libraries reach the link only when the package is in the active graph (PKG34), so the `openssl` feature decides
whether a program links OpenSSL at all. The feature is on by default, as it is in the HTTP clients that make TLS a
feature (`reqwest`'s `default-tls` and `ureq`'s `tls`, both on unless the user turns the default off).

**The `openssl` feature is the only switch, and it is a compile-time one.** Every function of `tls` that calls an OpenSSL
symbol is written as `comptime if features.openssl { ... } else { ... }` (CT5). With the feature on, the body is the
real one. With it off, the body is a failure: a function that returns a `Result` returns `TlsError.Unsupported` ("no OpenSSL in this
build: the `openssl` feature of the tls package is off"), and any other function returns an empty value (an empty string or list,
`None`, zero, or nothing). The declarations and the safe types still compile, so a program's code is the same either way. A
build with the feature off references no OpenSSL symbol, so it links and runs on a machine with no OpenSSL installed. The
`http` package uses this (`spec/http-client.md`, HC17): its `tls` feature is on by default, and a program that writes
`http = { official = true, default-features = false }` gets plain `http` with no OpenSSL link and `https` URLs fail with
`HttpError.Tls`.

Why a feature and not a link-time error: the link is what fails. A program that does not use `https` should not need
OpenSSL installed, and a program that does should get OpenSSL's real libraries, not a stub.

| Platform | What is installed | What the program needs |
| --- | --- | --- |
| Linux | `libssl-dev` (Debian and Ubuntu), `openssl-devel` (Fedora) | nothing: `-lssl -lcrypto` finds the system libraries |
| macOS | `brew install openssl@3` | `LIBRARY_PATH=$(brew --prefix openssl@3)/lib` so the linker finds `libssl.dylib`; the dylibs are not in a default path |
| Windows | nothing: the backend is SChannel, which ships with the system | nothing |

**Windows links no OpenSSL (TLS1).** The Windows backend is SChannel, so a program that uses `tls` on Windows links
`secur32`, `crypt32`, and `ncrypt`, which ship with the system. The driver does not search for OpenSSL, does not copy
OpenSSL DLLs, and does not download anything for `tls` on Windows. The OpenSSL discovery and download that #878 first
added are withdrawn: the Windows backend no longer uses OpenSSL.

**The package's other rules.** A library in an unusual place is found through the linker's variables (`LIBRARY_PATH` on
Unix, `LIB` for MSVC) or `BURDIN_CC` flags, as for any `--link-lib`. A missing library on Unix is still the linker's own
error, which names it. A program built against OpenSSL 3 runs only where a compatible `libssl.so.3`/`libssl.3.dylib`/
`libssl-3-x64.dll` is present; the package targets the OpenSSL 3 series (3.0 and later) and its stable ABI, and refuses at
run time to start under an older one (`OpenSSL_version_num` below 3.0.0 is a `TlsError.Unsupported` from the first
configuration, not undefined behavior).

Hand-written declarations replace a generated module because the subset is small, because the macros that matter
(`SSL_set_tlsext_host_name`, `SSL_CTX_set_min_proto_version`) are not functions a header walker can bind, and because the
`long` parameters need the platform rule written at the top of `openssl.bd` (a `long` parameter is passed as 64 bits and
a `long` result is read as 32, which is all these functions return). `tests/check_tls.py` checks that every function the file declares is exported by
the installed `libssl` and `libcrypto` (`nm -D`; skipped where `nm` or a Unix library is missing), so a mistyped name is
caught by CI for the functions no test happens to call. It does not compare parameter lists with the headers: OpenSSL
declares many of these through macros, so a signature typo is caught only by a test that calls the function.

Tests: `tests/s3_tls/link` (Linux and macOS: links the system OpenSSL through the manifest alone and runs) and
`tests/s3_tls/link_windows` (Windows: links SChannel through the manifest alone and reports it through `tls.backend()`);
the `[link]` table itself: `tests/check_link.py`; the optional feature: `tests/check_tls.py` (a program with
`tls = { official = true, default-features = false }` builds and runs with no `-lssl` or `-lcrypto` on the linker's
command line, and its TLS calls report `Unsupported`). On Windows the feature has no effect (TLS1a), so that check runs on
Unix only.

## TLS8. What is not provided

- **Server-side ALPN selection and SNI-based certificate selection.** Both are OpenSSL callbacks
  (`SSL_CTX_set_alpn_select_cb`, `SSL_CTX_set_tlsext_servername_callback`), and the C bridge cannot pass a Burdin
  function to C (a C function pointer is a `CPtr<CVoid>` the program cannot make from a Burdin function). A server
  therefore speaks no ALPN and serves one certificate per configuration. The same is why an encrypted private key
  (a password callback) is refused (TLS4).
- **Session resumption and tickets, 0-RTT / early data, renegotiation controls, OCSP stapling and revocation
  (CRL/OCSP) checks, certificate transparency, custom cipher or group lists, and key logging.** OpenSSL's defaults
  apply: its session cache and tickets are left as they are for the server, and the client does not resume.
- **A generic transport.** `TlsStream` wraps a `TcpStream`, not any `Read + Write` (TLS5). The engine itself never sees
  a socket, so a generic stream is a change to `stream.bd` alone once a `Drop` type can give a field back.
- **The certificate-authority hints of a client-certificate request** (`SSL_CTX_set_client_CA_list`): a server that
  requires client certificates verifies them against its roots but does not tell the client which authorities it trusts,
  so a client with several identities cannot choose by hint (the package has one identity per configuration anyway).
- **DTLS and QUIC.** The package is TLS over a reliable byte stream only.
- **Hosts other than the ones OpenSSL 3 supports** (a platform without an OpenSSL 3 build), and any build that links
  OpenSSL statically.
- **Constant-time or side-channel claims beyond what OpenSSL itself makes**, and any audit of the Burdin side.

## Tests

`tests/s3_tls/link` and `tests/s3_tls/conformance` run in the conformance suite (`python3 tests/run_tests.py s3_tls`) on
every platform CI covers, after the job installs OpenSSL. `conformance` runs clients and servers in one process over
`127.0.0.1:0` (about 130 output lines, compared with the `//>` lines of its `src/main.bd`). Its certificates are
committed under `tests/s3_tls/conformance/src/certs` and read with `embed`; they are made by `tests/tls/regenerate.py`
with the `openssl` command-line tool (a test authority, an intermediate, leaves with long validity, an expired leaf, a
wrong-host leaf, a self-signed leaf, a client leaf, and an encrypted key). The keys are throw-away test keys, committed
on purpose so that no tool is needed to run the tests. `tests/check_tls.py` checks the fixtures with the `openssl`
tool, runs the conformance program, checks the declared symbols, the README's examples, `burdin add tls`, and the pin.
The manifest table is checked by `tests/check_link.py`.

The valid certificates are valid from one year before the day `tests/tls/regenerate.py` made them (it signs everything
with `openssl ca`, which takes both dates) until 100 years after it, so a machine whose clock or time zone is off by days
still verifies them; `tests/check_tls.py` checks that each starts at least 300 days in the past. The `http` package's tests
(`tests/s3_http/tls`, `spec/http-client.md`, HC17) keep copies of the files they use, which the script writes too and
`tests/check_tls.py` compares.
