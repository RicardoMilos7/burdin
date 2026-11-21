# http

An HTTP/1.1 client over `std`'s `TcpStream` (issue #260, with #328, #329, and #330; the behavior is
`spec/http-client.md`). Declare it in `burdin.toml` and `use http`:

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

`https` is not supported: the standard library has no TLS. An `https` URL parses, and sending a request to one fails
with `HttpError.TlsUnsupported` before any connection is made; it never falls back to plain text, and a redirect to an
`https` URL fails the same way.

## Surface

```
Client.new()                                    a client with the defaults below
client.get(url) / head(url) / delete(url)       Result<Response, HttpError>
client.post(url, body) / put(url, body)         the body is text
client.send(request)                            any method, headers, and body
client.idle_connections() / close_idle()        the connection pool

Request.new(method, url)                        Result<Request, HttpError>; also get, head, post, put, delete
request.header(name, value) / body_text(text) / body_bytes(bytes)

Response { status, reason, headers, url, redirects }
response.header(name) / is_success() / is_redirect() / trailers()
response.text() / bytes() / discard(limit) / close()
response.body                                   a `Read` over the body, for streaming

Url.parse(text) / join(reference) / to_text()   Url { scheme, host, port, path, query }
Headers                                         add, set, get, get_all, contains, remove, has_token
HttpError                                       an `error enum`: one variant per kind of failure
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
127.0.0.1, so no test touches the network. `python3 tests/check_http.py` runs both and checks `burdin add http` and the
official pin.
