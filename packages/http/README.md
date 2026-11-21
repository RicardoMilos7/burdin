# http

An HTTP/1.1 client over `std`'s `TcpStream`, and over TLS for `https` (issue #260, with #328, #329, and #330; the
behavior is `spec/http-client.md`). Declare it in `burdin.toml` and `use http`:

```toml
[dependencies]
http = true
```

```burdin
use http

fn main(): Result<()> {
    let client = http.Client.new()
    let mut response = client.get("http://example.test/greet?name=burdin")?
    println("{} {}", response.status, response.reason)
    println("{}", response.text()?)
    Ok(())
}
```

## https

`https` URLs work the same way as `http` ones: `client.get("https://example.test/")`. The connection is TLS provided by
the official `tls` package (`spec/tls.md`). That is the `tls` feature of `http`, **on by default**, so a program that
uses `http` links OpenSSL 3 (`libssl-dev` on Linux, `brew install openssl@3` with `LIBRARY_PATH` on macOS; on Windows
the driver finds an installation itself, and `packages/tls/README.md` has the table).

A program that never makes an `https` request can leave TLS out, and then it needs no OpenSSL at all:

```toml
[dependencies]
http = { official = true, default-features = false }
```

With the feature off, plain `http` URLs work as before and every `https` URL fails with `HttpError.Tls`, saying that
no OpenSSL is in the build. The feature is the same kind of switch as `reqwest`'s `default-tls` and `ureq`'s `tls`.

By default the server is verified: its certificate chain against the system's trust store, and its name against the
host in the URL (which is also sent as SNI). A failure is `HttpError.Tls(TlsError)` with the reason, it is never
retried, and the request is never sent, in plain text or otherwise. A program that names `tls` types (`TrustStore`,
`ClientConfig`) declares `tls = true` itself; `http` does not re-export them (the program then has `tls` in its graph
with the `openssl` feature on, as it does with `http`):

```burdin
// example: https
use http
use tls.TrustStore

fn main(): Result<()> {
    // The default client trusts the system's authorities.
    let client = http.Client.new()
    match client.get("https://example.test/") {
        Ok(response) => println("{}", response.status),
        Err(e) => {
            if e.is_verification() {
                println("the certificate is not acceptable: {}", e)
            } else {
                println("failed: {}", e)
            }
        }
    }

    // A program that knows its authority trusts exactly that one.
    let mut trust = TrustStore.empty()
    trust.add_pem_file("ca.pem")?
    let mut pinned = http.Client.new()
    pinned.trust(trust)?
    let mut response = pinned.get("https://internal.example/")?
    println("{}", response.text()?)
    Ok(())
}
```

`client.trust(store)` trusts exactly the authorities in a `tls.TrustStore`, `client.tls_config(config)` takes a whole
`tls.ClientConfig` (a minimum version, a client certificate), and `client.insecure_skip_verify()` turns verification
off: the connection is encrypted but talks to anyone, so it is for tests. The client offers the ALPN protocol
`http/1.1` and nothing else. Connections are pooled per scheme, host, and port, so an `http` and an `https` connection
to one port are never swapped; clones of a client share the pool, so give a client with another trust store its own
`Client.new()`. A TLS connection that ends without `close_notify` is not an orderly end: a body cut that way is
`HttpError.BodyTruncated`, even when the body has no length. The TLS handshake is bounded by `connect_timeout`.

Not provided: proxies, session resumption, HTTP/2, and any check beyond what `tls` does (no revocation, no pinning).
This is an experiment, not a security claim.

## Surface

```
Client.new()                                    a client with the defaults below
client.get(url) / head(url) / delete(url)       Result<Response, HttpError>
client.post(url, body) / put(url, body)         the body is text
client.send(request)                            any method, headers, and body
client.trust(store) / tls_config(config)        https: the authorities to trust, or a whole tls.ClientConfig
client.insecure_skip_verify()                   https: no verification (tests only)
client.idle_connections() / close_idle()        the connection pool

Request.new(method, url)                        Result<Request, HttpError>; also get, head, post, put, delete
request.header(name, value) / body_text(text) / body_bytes(bytes)

Response { status, reason, headers, url, redirects }
response.header(name) / is_success() / is_redirect() / trailers()
response.text() / bytes() / discard(limit) / close()
response.body                                   a `Read` over the body, for streaming

Url.parse(text) / join(reference) / to_text()   Url { scheme, host, port, path, query }
Headers                                         add, set, get, get_all, contains, remove, has_token
HttpError                                       an `error enum`: one variant per kind of failure (`Tls(TlsError)` for https)
```

The `Client` settings are public fields:

| field | default | meaning |
| --- | --- | --- |
| `connect_timeout` | 10 s | the limit on opening a connection |
| `read_timeout` | 30 s | the limit on any one wait for bytes, in the head and in the body |
| `total_timeout` | none | the limit on one `send`, redirects and retries included |
| `follow_redirects` | `true` | follow `301`, `302`, `303`, `307`, and `308` |
| `max_redirects` | 10 | more redirects than this is `HttpError.TooManyRedirects` |
| `max_retries` | 2 | how often a request that failed before any response byte is sent again |
| `retry_backoff`, `max_backoff` | 100 ms, 2 s | the wait before a retry, doubling up to the maximum |
| `retry_on_503` | `false` | repeat a safe request that got `503`, after its `Retry-After` |
| `user_agent` | `burdin-http/0.1` | sent unless the request has its own |
| `max_idle_per_host`, `idle_timeout` | 4, 30 s | the pool; `0` idle connections turns reuse off |
| `max_body` | 16 MiB | a body larger than this is `HttpError.BodyTooLarge` |

A body that is read to its end gives its connection back to the pool, so the next request to the same host and port
reuses it. A body that is dropped unread closes the connection instead.

## Tests

`tests/http/` holds golden fixtures for the parts that need no socket: URLs, headers, request and response heads, and
the body decoders. `tests/s3_http/client` runs the client against a scripted server in the same process, on port 0 of
127.0.0.1, and `tests/s3_http/tls` does the same over TLS with the test certificates (`tests/tls/regenerate.py`), so no
test touches the network. `python3 tests/check_http.py` runs all three and checks `burdin add http` and the official pins
(of `http` and of the `tls` it depends on).
