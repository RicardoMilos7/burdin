# tls

TLS over OpenSSL 3 (issue #262; the behavior is `spec/tls.md`). Declare it in `burdin.toml` and `use tls`:

```toml
[dependencies]
tls = true
```

> **This is an experiment, not a security claim.** The package has had no audit. Use a reviewed stack for anything you
> care about.

## Surface

```
Certificate, PrivateKey, TrustStore          loaded from PEM text, DER bytes, or files; freed when dropped
ClientConfig, ServerConfig, TlsVersion       verification on by default; TLS 1.2 or later
TlsStream                                    a Read and Write over a TcpStream; runs the handshake
TlsError                                     every failure is a value, never a panic
```

| Type | Functions |
| --- | --- |
| `Certificate` | `from_pem(str)`, `chain_from_pem(str)`, `from_der([u8])`, `load_pem_file(path)`, `load_chain_file(path)`; `to_der`, `subject`, `issuer`, `not_before`, `not_after`, `fingerprint` (SHA-256 hex), `duplicate` |
| `PrivateKey` | `from_pem(str)`, `from_der([u8])`, `load_pem_file(path)`: RSA, ECDSA, Ed25519, unencrypted |
| `TrustStore` | `system()`, `empty()`, `add(cert)`, `add_pem(str)`, `add_pem_file(path)`, `len()` |
| `ClientConfig` | `new()` (system roots), `with_trust(store)`; `min_version`, `max_version`, `alpn`, `client_certificate(chain, key)`, `insecure_skip_verify()` |
| `ServerConfig` | `new(chain, key)`, `from_pem(chain_pem, key_pem)`; `min_version`, `max_version`, `require_client_certificate(roots)` |
| `TlsStream` | `connect(tcp, server_name, config)`, `accept(tcp, config)`; `recv(buf, max)`, `send(data)`, `send_all(data)`, `close()`, `abort()`; `tls_version`, `cipher`, `alpn_protocol`, `peer_certificate`, `server_name`; `set_read_timeout`, `set_write_timeout`; `Read` and `Write` |

Verification is on by default and cannot be turned off by accident: a client checks the server's chain against its trust
store and the certificate's names against the `server_name` it passes to `connect` (a DNS name, or an IP address matched
against the certificate's IP entries). An expired, self-signed, untrusted, or wrongly named certificate is a `TlsError`
(`CertificateInvalid` or `HostnameMismatch`). The one switch that turns it off is `insecure_skip_verify()`, which is
named for what it does.

`recv` returns `Ok(0)` only after the peer's `close_notify`. A transport that ends without one is
`TlsError.UnexpectedEof`, so a connection cut by someone else does not look like a complete message. `Read.read` and
`Write.write` report the same failures as `IoError` (`UnexpectedEof` for a truncation, `InvalidData` for the rest);
use `recv` and `send` to see the `TlsError`.

A TCP read timeout set with `TcpStream.set_read_timeout` before `connect`, or `TlsStream.set_read_timeout` after it,
applies to every wait for the peer and surfaces as `TlsError.Io` with `IoErrorKind.TimedOut`
(`TlsError.is_timeout()`); the stream stays usable. No OpenSSL call blocks: waiting for the peer parks the task on the
reactor like any socket read.

### A client

```burdin
// example: client
use tls.{ClientConfig, TlsStream, TrustStore}

fn fetch(addr: str, host: str, ca_pem: str): Result<String> {
    let mut trust = TrustStore.system()
    trust.add_pem(ca_pem)?
    let config = ClientConfig.with_trust(trust)?
    let tcp = TcpStream.connect(addr)?
    let mut stream = TlsStream.connect(tcp, host, config)?
    stream.send_all("ping".as_bytes())?
    let mut reply = List<u8>.new()
    stream.recv(mut reply, 1024)?
    stream.close()?
    Ok(String.from_utf8_lossy(reply[0..reply.len()]))
}

fn main(): Result<()> {
    println("{}", fetch("127.0.0.1:8443", "example.test", "")?)
    Ok(())
}
```

### A server

```burdin
// example: server
use tls.{Certificate, PrivateKey, ServerConfig, TlsStream}

fn serve(own tcp: TcpStream, config: ServerConfig): Result<()> {
    let mut stream = TlsStream.accept(tcp, config)?
    let mut request = List<u8>.new()
    let n = stream.recv(mut request, 4096)?
    stream.send_all(request[0..n])?
    stream.close()?
    Ok(())
}

fn main(): Result<()> {
    let chain = Certificate.load_chain_file("chain.pem")?
    let key = PrivateKey.load_pem_file("key.pem")?
    let config = ServerConfig.new(chain, key)?
    let listener = net.TcpListener.bind("127.0.0.1:8443")?
    let (tcp, _) = listener.accept()?
    serve(tcp, config)
}
```

## Not provided

Server-side ALPN selection and SNI-based certificate selection (both are OpenSSL callbacks, and the C bridge cannot pass
a Burdin function to C), encrypted private keys (a password callback), session resumption and tickets, 0-RTT,
revocation checking, DTLS, and QUIC. A server speaks no ALPN, so `alpn_protocol()` is `None` against it. See
`spec/tls.md`, TLS8.

## Decision: OpenSSL 3, not native and not the platform stack

Native TLS 1.3 needs X.509, RSA and ECDSA verification, and a trust store, which `crypto` does not have (X.509 is
issue #559). SChannel, Secure Transport, and Linux's OpenSSL are three different APIs. OpenSSL 3 is one API on all
three platforms and supports client and server, SNI, ALPN, verification, and PEM/DER. The engine uses memory BIOs, so
OpenSSL never sees a socket and the Burdin side moves the bytes through `TcpStream` on the reactor (`spec/tls.md`,
TLS2).

## Installing OpenSSL

| Platform | Install | Build setting |
| --- | --- | --- |
| Linux | `apt install libssl-dev` | none |
| macOS | `brew install openssl@3` | `export LIBRARY_PATH="$(brew --prefix openssl@3)/lib"` |
| Windows | an OpenSSL 3 build (installer, `choco install openssl`, vcpkg) | the folder with `libssl.lib` and `libcrypto.lib` on `LIB`, the DLL folder on `PATH` |

A library in another place is found with the linker's own variables (`LIBRARY_PATH`, `LIB`) or `BURDIN_CC` flags.
