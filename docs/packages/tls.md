# tls

TLS on the platform's own stack: OpenSSL 3 on Linux and macOS, SChannel on Windows (issue #262; `spec/tls.md`). Verification is on by default and a failure is a value, never a panic.

Version `0.1.0`. Use it with `use tls`; it is an official package, so `burdin.toml` needs no entry for it. The package's own guide is [`packages/tls/README.md`](../../packages/tls/README.md).

## Requirements

Linux and macOS: OpenSSL 3, with its headers and libraries for the linker; the package README's *Installing OpenSSL* section says how. Windows: nothing to install, since the package links SChannel.

## Worked example

Configures a server and a client with a self-signed certificate for `localhost`, then runs a real TLS handshake over the loopback interface and echoes a message. The certificate is test material.

```burdin
use tls.{Certificate, ClientConfig, PrivateKey, ServerConfig, TlsStream, TrustStore}

// A self-signed certificate for `localhost` and its key. Test material only.
const CERT: str = "-----BEGIN CERTIFICATE-----
MIIBczCCARigAwIBAgIBCDAKBggqhkjOPQQDAjAUMRIwEAYDVQQDDAlsb2NhbGhv
c3QwIBcNMjUxMTIxMTYwMDQ1WhgPMjEyNjEwMjgxNjAwNDVaMBQxEjAQBgNVBAMM
CWxvY2FsaG9zdDBZMBMGByqGSM49AgEGCCqGSM49AwEHA0IABLNTRhI9deSqQkH/
NiFLg+Fe/zaKJC7lI8UTOi2V68Uc3izmbXb5WUJ8AqS+A6igo5a2sQwa94QknXaZ
83d2N1+jWTBXMAwGA1UdEwEB/wQCMAAwHQYDVR0OBBYEFOcIZAeKeTy0tqIDOlRa
DO1kZAnzMCgGA1UdEQQhMB+CCWxvY2FsaG9zdIIMZXhhbXBsZS50ZXN0hwR/AAAB
MAoGCCqGSM49BAMCA0kAMEYCIQDEfYToC6DvEmFBBk0+z914XNS5Vl0O/qrhwl5I
OvX3UAIhAM64DLHDSAdQc1r4kNgrRuoIGxh+XzAM8JpUn60qQ4DQ
-----END CERTIFICATE-----
"
const KEY: str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg1tyNCLqQEA/R2tKM
+KnOalrrIjLeUShT6y4KcwRTObehRANCAASzU0YSPXXkqkJB/zYhS4PhXv82iiQu
5SPFEzotlevFHN4s5m12+VlCfAKkvgOooKOWtrEMGveEJJ12mfN3djdf
-----END PRIVATE KEY-----
"

fn main() {
    // The server side: its certificate chain and key.
    let chain = Certificate.chain_from_pem(CERT).unwrap()
    let key = PrivateKey.from_pem(KEY).unwrap()
    let server_config = ServerConfig.new(chain, key).unwrap()

    // The client side: trust exactly the certificate the server presents.
    let mut trust = TrustStore.empty()
    trust.add_pem(CERT).unwrap()
    let client_config = ClientConfig.with_trust(trust).unwrap()

    let listener = net.TcpListener.bind("127.0.0.1:0").unwrap()
    let port = listener.local_addr().unwrap().port()
    let work = Channel<net.TcpStream>.bounded(1)
    task.scope(|s| {
        s.spawn(|| {
            match work.recv() {
                Option.Some(conn) => {
                    let mut server = TlsStream.accept(conn, server_config).unwrap()
                    let mut buf: List<u8> = List<u8>.new()
                    server.recv(mut buf, 1024).unwrap()
                    server.send_all(buf.as_slice()).unwrap()
                    server.close().unwrap()
                }
                Option.None => {}
            }
        })
        let tcp = net.TcpStream.connect(format("127.0.0.1:{}", port).as_str()).unwrap()
        let (conn, _) = listener.accept().unwrap()
        let _ = work.send(conn)
        let mut client = TlsStream.connect(tcp, "localhost", client_config).unwrap()
        client.send_all("ping".to_utf8().as_slice()).unwrap()
        let mut reply: List<u8> = List<u8>.new()
        client.recv(mut reply, 1024).unwrap()
        assert_eq(reply.len(), 4)
        println("echoed {} bytes over {}", reply.len(), client.tls_version())
        client.close().unwrap()
    })
}
```

Save it as `main.bd` and run it with `burdin run main.bd`, or copy the body into a package that declares `use tls`. Every example in the package's doc comments is also compiled and run by `burdin test --doc`.

## Surface

Every public item and public method, in source order. Names are shown as `burdin doc` shows them: an item is `module.Name`, and a package whose root re-exports it (`use tls.{Certificate}`) also reaches it from the root. Each row's summary is the first sentence of the item's doc comment; the full text and the examples are on the generated site.

| Item | Signature | Summary |
| --- | --- | --- |
| `certs.Certificate` | `struct Certificate` | An X.509 certificate. |
| `certs.Certificate.from_pem` | `fn from_pem(pem: str): Result<Certificate, TlsError>` | Reads the first certificate of a PEM text. |
| `certs.Certificate.chain_from_pem` | `fn chain_from_pem(pem: str): Result<List<Certificate>, TlsError>` | Reads every certificate of a PEM text (a chain or a bundle), in order. |
| `certs.Certificate.from_der` | `fn from_der(data: [u8]): Result<Certificate, TlsError>` | Reads one certificate from its DER encoding. |
| `certs.Certificate.load_pem_file` | `fn load_pem_file(path: str): Result<Certificate, TlsError>` | Reads the first certificate of a PEM file. |
| `certs.Certificate.load_chain_file` | `fn load_chain_file(path: str): Result<List<Certificate>, TlsError>` | Reads every certificate of a PEM file. |
| `certs.Certificate.duplicate` | `fn duplicate(self): Certificate` | A second owner of the same certificate (the library's `Clone` cannot be written by hand yet). |
| `certs.Certificate.to_der` | `fn to_der(self): List<u8>` | The DER encoding. |
| `certs.Certificate.subject` | `fn subject(self): String` | The subject in OpenSSL's one-line form (`/CN=localhost`). |
| `certs.Certificate.issuer` | `fn issuer(self): String` | The issuer in OpenSSL's one-line form. |
| `certs.Certificate.not_before` | `fn not_before(self): String` | The start of the validity period (`Jan 1 00:00:00 2020 GMT`). |
| `certs.Certificate.not_after` | `fn not_after(self): String` | The end of the validity period. |
| `certs.Certificate.fingerprint` | `fn fingerprint(self): String` | The SHA-256 of the DER encoding, as 64 lower-case hex digits. |
| `certs.PrivateKey` | `struct PrivateKey` | A private key: RSA, ECDSA, or Ed25519, unencrypted (an encrypted key is refused; spec/tls.md, TLS4). |
| `certs.PrivateKey.from_pem` | `fn from_pem(pem: str): Result<PrivateKey, TlsError>` | Reads a PEM private key (PKCS#8, or the older RSA and EC forms). |
| `certs.PrivateKey.from_der` | `fn from_der(data: [u8]): Result<PrivateKey, TlsError>` | Reads a private key from its DER encoding (PKCS#8, or the RSA and EC forms). |
| `certs.PrivateKey.load_pem_file` | `fn load_pem_file(path: str): Result<PrivateKey, TlsError>` | Reads a PEM private key file. |
| `certs.TrustStore` | `struct TrustStore` | The certificate authorities a client trusts: the system's, some of your own, or both. |
| `certs.TrustStore.system` | `fn system(): TrustStore` | The system's trust store (OpenSSL's default certificate file and directory, or `SSL_CERT_FILE` and `SSL_CERT_DIR`). |
| `certs.TrustStore.empty` | `fn empty(): TrustStore` | A store with no authority at all; add the ones to trust. |
| `certs.TrustStore.add` | `fn add(mut self, own cert: Certificate)` | Trusts `cert` too. |
| `certs.TrustStore.add_pem` | `fn add_pem(mut self, pem: str): Result<(), TlsError>` | Trusts every certificate of a PEM text (one certificate or a bundle). |
| `certs.TrustStore.add_pem_file` | `fn add_pem_file(mut self, path: str): Result<(), TlsError>` | Trusts every certificate of a PEM file. |
| `certs.TrustStore.len` | `fn len(self): usize` | How many certificates were added (the system's are not counted). |
| `config.TlsVersion` | `enum TlsVersion: Copy, Clone, Eq, Debug` | A protocol version a configuration can require. |
| `config.ClientConfig` | `struct ClientConfig` | How a client connection is set up. |
| `config.ClientConfig.new` | `fn new(): Result<ClientConfig, TlsError>` | A configuration that trusts the system's authorities. |
| `config.ClientConfig.with_trust` | `fn with_trust(trust: TrustStore): Result<ClientConfig, TlsError>` | A configuration that trusts exactly the authorities of `trust`. |
| `config.ClientConfig.min_version` | `fn min_version(mut self, version: TlsVersion): Result<(), TlsError>` | The lowest protocol version the client accepts (TLS 1.2 unless raised). |
| `config.ClientConfig.max_version` | `fn max_version(mut self, version: TlsVersion): Result<(), TlsError>` | The highest protocol version the client offers. |
| `config.ClientConfig.alpn` | `fn alpn(mut self, protocols: List<String>): Result<(), TlsError>` | The application protocols to offer, most preferred first (`h2`, `http/1.1`). |
| `config.ClientConfig.client_certificate` | `fn client_certificate(mut self, chain: List<Certificate>, key: PrivateKey): Result<(), TlsError>` | The certificate chain (leaf first) and key to present when the server asks for a client certificate. |
| `config.ClientConfig.insecure_skip_verify` | `fn insecure_skip_verify(mut self)` | Turns certificate and hostname verification off. |
| `config.ClientConfig.skips_verification` | `fn skips_verification(self): bool` | Whether verification was turned off. |
| `config.ServerConfig` | `struct ServerConfig` | How a server connection is set up: one certificate chain and its key. |
| `config.ServerConfig.new` | `fn new(chain: List<Certificate>, key: PrivateKey): Result<ServerConfig, TlsError>` | A configuration serving `chain` (leaf first) with `key`. |
| `config.ServerConfig.from_pem` | `fn from_pem(chain_pem: str, key_pem: str): Result<ServerConfig, TlsError>` | `new` from PEM texts: the chain (leaf first, then intermediates) and an unencrypted key. |
| `config.ServerConfig.min_version` | `fn min_version(mut self, version: TlsVersion): Result<(), TlsError>` | The lowest protocol version the server accepts (TLS 1.2 unless raised). |
| `config.ServerConfig.max_version` | `fn max_version(mut self, version: TlsVersion): Result<(), TlsError>` | The highest protocol version the server offers. |
| `config.ServerConfig.require_client_certificate` | `fn require_client_certificate(mut self, roots: TrustStore): Result<(), TlsError>` | Requires every client to present a certificate that chains to `roots` (mutual TLS). |
| `errors.TlsError` | `error enum TlsError: Clone, Debug` | Every way a TLS operation can fail. |
| `errors.TlsError.is_timeout` | `fn is_timeout(self): bool` | Whether the transport timed out (a read or write timeout set on the connection expired). |
| `errors.TlsError.is_verification` | `fn is_verification(self): bool` | Whether this is a certificate or hostname verification failure. |
| `errors.TlsError.to_io` | `fn to_io(own self): IoError` | The error as an `IoError`, which is what `Read` and `Write` report: a transport error keeps its own value, a truncated stream is `IoError.UnexpectedEof`, and every other failure is ... |
| `stream.NET_CHUNK` | `const NET_CHUNK: usize = 16640` | How much one socket read asks for: a TLS record with its framing. |
| `stream.TlsStream` | `struct TlsStream` | A TLS connection. |
| `stream.TlsStream.connect` | `fn connect(own tcp: TcpStream, server_name: str, config: ClientConfig): Result<TlsStream, TlsError>` | Runs the client handshake over `tcp`, verifying the server's certificate against the configuration's trust store and `server_name` against its names (an IP address is matched against its IP entries). |
| `stream.TlsStream.accept` | `fn accept(own tcp: TcpStream, config: ServerConfig): Result<TlsStream, TlsError>` | Runs the server handshake over `tcp`. |
| `stream.TlsStream.set_read_timeout` | `fn set_read_timeout(mut self, timeout: Option<Duration>)` | How long one socket read may wait; `Option.None` waits forever. |
| `stream.TlsStream.set_write_timeout` | `fn set_write_timeout(mut self, timeout: Option<Duration>)` | How long one socket write may wait for room. |
| `stream.TlsStream.alpn_protocol` | `fn alpn_protocol(self): Option<String>` | The application protocol the server chose by ALPN. |
| `stream.TlsStream.tls_version` | `fn tls_version(self): String` | The negotiated protocol version, as `TLSv1.2` or `TLSv1.3`. |
| `stream.TlsStream.cipher` | `fn cipher(self): String` | The negotiated cipher suite. |
| `stream.TlsStream.peer_certificate` | `fn peer_certificate(self): Option<Certificate>` | The peer's leaf certificate, if it sent one (a server always does; a client only when asked). |
| `stream.TlsStream.server_name` | `fn server_name(self): Option<String>` | On a server, the name the client sent as SNI. |
| `stream.TlsStream.recv` | `fn recv(mut self, mut buf: List<u8>, max: usize): Result<usize, TlsError>` | Reads up to `max` decrypted bytes and appends them to `buf`. |
| `stream.TlsStream.send` | `fn send(mut self, data: [u8]): Result<usize, TlsError>` | Encrypts a prefix of `data` (at most one record) and sends it, and returns how many bytes it took. |
| `stream.TlsStream.send_all` | `fn send_all(mut self, data: [u8]): Result<(), TlsError>` | Sends all of `data`. |
| `stream.TlsStream.close` | `fn close(mut self): Result<(), TlsError>` | Sends `close_notify` and flushes it. |
| `stream.TlsStream.abort` | `fn abort(mut self)` | Ends the connection without a `close_notify`, as a crash or an attacker would. |
| `tls.backend` | `fn backend(): String` | The TLS stack this build uses (spec/tls.md, TLS1): `SChannel` on Windows, and on the other platforms the text of the linked OpenSSL, such as `OpenSSL 3.0.13 30 Jan 2024`. |
