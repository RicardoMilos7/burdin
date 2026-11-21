# HTTP client

The `http` package (stage 3, issue #260, with #328 requests, responses, headers, and the body stream; #329 redirects,
timeouts, and retries; #330 connection reuse and body framing; `packages/http`). The package is an ordinary official
package (`spec/packages.md`, PKG15-PKG18) written in Burdin over `std`'s `TcpStream` (`spec/io.md`) and, for `https`,
over the official `tls` package (`spec/tls.md`), which it declares as a dependency (`tls = true`). This page says what
it sends, what it accepts, and which tests witness each rule. The rule IDs are HC1-HC17. The package documentation,
`packages/http/README.md`, repeats the decisions for users.

A failure is an `HttpError` returned from a `Result`, never a panic and never a compiler diagnostic, so the rules are
pinned by golden fixtures and a conformance program rather than by registry codes. The pure parts (URLs, headers, heads,
and body decoders) are pinned by the fixtures under `tests/http/`, one folder per mode, each fixture a text file of
one-line cases with an `.expected` golden. The behavior that needs a connection is pinned by the conformance package
`tests/s3_http/client`, which starts a scripted server in the same process (a `TcpListener` on `127.0.0.1:0`, one task per
connection) and so touches no network outside the machine. The `https` behavior (HC17) is pinned by
`tests/s3_http/tls`, whose scripted server also speaks TLS (`tls.TlsStream.accept` with the test certificates). No test
reaches a real server on the Internet. `tests/check_http.py` runs all three.

## What is not here

- **No TLS settings beyond trust.** `https` is supported (HC17) through the `tls` package, which links OpenSSL 3
  (`spec/tls.md`, TLS7). The client verifies the server by default; it offers no client-certificate shortcut, no session
  resumption, no pinning, and no HTTP/2 (it offers the ALPN protocol `http/1.1` only). It never falls back to plain
  text.
- No proxies (so no `CONNECT` tunnels), no cookies, no authentication schemes, no `Expect: 100-continue`, no compression (`gzip` or `deflate`
  content or transfer codings), no HTTP/2, no upgrade (a `101` response is an error), no IPv6 literal hosts, and no
  streaming request bodies: a request body is bytes held in memory.

## HC1. URLs

`Url.parse(text)` reads `scheme://host[:port][/path][?query]`. The scheme and the host are lower-cased. The scheme must be
`http` or `https`, which have default ports 80 and 443; any other scheme is `UnsupportedScheme` ("only http and https are supported"), and a text without
`://` is `InvalidUrl` ("the scheme is missing"). The port is 1 to 65535. A fragment (`#...`) is dropped. An empty path is
`/`. A user name or password (`user:pw@`), an IPv6 literal (`[::1]`), an empty host, and a space or control character
(which must be percent-encoded) are `InvalidUrl`; escapes are neither decoded nor re-encoded. `to_text` writes the
default port as nothing, `authority` is `host` or `host:port`, `target` is the path and query sent on the request line,
and `origin` is the scheme, host, and port that decide whether two URLs are the same origin (HC12).

Tests: `tests/http/url/basic.txt`; `ftp` and userinfo in `tests/s3_http/client/src/basics.bd`; what an `https` URL does
when it is sent is HC17.

## HC2. Reference resolution

`url.join(reference)` resolves a `Location` value against the URL it came from: an absolute URL replaces it, `//host/p`
keeps the scheme, `/p` replaces the path and query, `?q` keeps the path and replaces the query, and a relative path
replaces the last segment of the base path. `.` and `..` segments are removed and `..` never goes above the root. A
fragment in the reference is dropped. The result is checked like a parsed URL.

Tests: `tests/http/join/references.txt`; the `locations` scenario of `redirects.bd`.

## HC3. Methods

`Method` is `Get`, `Head`, `Post`, `Put`, `Delete`, `Options`, `Patch`, `Trace`, or `Custom(token)`. Names are
case-sensitive and a method is a token: `parse` rejects anything else with `InvalidMethod`. `is_idempotent` is true for
`GET`, `HEAD`, `PUT`, `DELETE`, `OPTIONS`, and `TRACE`; `POST`, `PATCH`, and every custom method are not. Retries (HC14) are limited to idempotent methods.

Tests: the `request` fixtures; `post_not_retried` and `put_retried` in `retries.bd`.

## HC4. Headers

`Headers` is an ordered list of fields. Names compare without regard to case and keep the spelling they were added with.
`add` appends, `set` makes one field of that name, `get` is the first value, `get_all` every value in order, and
`has_token` looks for a token in a comma-separated value, ignoring case. A name must be a token and a value may not
contain a line break, NUL, or another control character except a tab, so a value can never end its own field: both
`add` and `set` return `InvalidHeader` for that, and a request holding such a field is never sent (CRLF injection).
A value is stored without its leading and trailing spaces and tabs.

Tests: `tests/http/headers/fields.txt`; `bad header value` and `bad header name` in `basics.bd`, which also show that no
connection was made.

## HC5. Requests

`Request.new(method, url)` returns `Result`; `get`, `head`, `post`, `put`, and `delete` are shorthands. `header` adds a
field, `body_text` and `body_bytes` set the body. A request whose URL or header is invalid is not sent: `send` returns
the problem. `Client.get`, `head`, `delete`, `post`, and `put` build the request and send it.

Tests: the `request` fixtures; `get_basic`, `post_body`, and `overrides` in `basics.bd`.

## HC6. The request head

The head is `METHOD target HTTP/1.1`, then `Host` (unless the request has one), the request's fields in order,
`User-Agent` (unless the request has one, else the client's `user_agent`), `Content-Length` when there is a body or the
method is `POST`, `PUT`, or `PATCH`, and `Connection: close` when the client's pool is off (HC15) and the request has no
`Connection`; then a blank line. `Content-Length` and `Transfer-Encoding` the caller set are replaced, never sent twice.
The body follows in the same write.

Tests: `tests/http/request/wire.txt`; the `sent:` lines of `basics.bd`.

## HC7. The response head

The status line is `HTTP/1.0` or `HTTP/1.1`, a three-digit status from 100 to 999, and an optional reason, which may be
empty. Anything else is `BadResponse`. Each field is `name: value` with a token name; a field line that starts with a
space or tab (a folded line), a line without a colon, and a bad name are `BadResponse`. A head larger than 64 KiB, or with
more than 128 fields, is `BadResponse`. Interim `1xx` responses (`100 Continue`) are skipped and a `101` is an error.
Bare line feeds are not accepted as line ends.

Tests: `tests/http/head/heads.txt`; `garbage head` and `folded head` in `basics.bd`.

## HC8. Framing

How the body ends is decided from the request method, the status, and the headers (RFC 9112, 6.3), in this order:
no body for the response to `HEAD` and for `1xx`, `204`, and `304`; `Transfer-Encoding` whose last coding is `chunked`
means chunked; any other `Transfer-Encoding`, or one together with a `Content-Length`, is `BadResponse` (the ways
messages get smuggled); `Content-Length` is the exact length, and repeated or comma-listed values must agree and be
plain digits that fit in 64 bits; otherwise the body runs to the end of the connection. A connection whose body ran
to its end is not reused.

Tests: `tests/http/head/heads.txt` (framing for `GET` and `HEAD` per head), `tests/http/body/framing.txt`; `status 204`,
`status 304`, `head`, `both lengths`, `two lengths`, `other coding`, and `close-delimited` in `basics.bd`.

## HC9. Chunked bodies

A chunk is a hexadecimal size, optional `;extensions` (ignored), CRLF, that many bytes, and CRLF. A zero chunk is
followed by trailer fields and a blank line; the trailers are kept (`response.trailers()`). A size that is not
hexadecimal, is missing, or does not fit in 64 bits, chunk data not followed by CRLF, and a trailer without a colon are
`BadBody`. A connection that ends anywhere before the final blank line is `BodyTruncated`. The decoder gives the same
result however the bytes are split across reads.

Tests: `tests/http/body/framing.txt` (every case at read sizes 1, 3, and 100); `chunked`, `chunk size`, `chunk crlf`,
and `chunk cut` in `basics.bd`.

## HC10. Reading the body

`response.body` is a `Read` over the connection, so a large body streams. `bytes` and `text` read the rest; they fail
with `BodyTooLarge` past the client's `max_body` (default 16 MiB), and `text` fails with `BadBody` when the bytes are not
UTF-8.
A body that ends early is `BodyTruncated`, which is also what a `Content-Length` body cut short reports. `discard(limit)`
reads and drops up to `limit` bytes and says whether the body ended. `Response.close` and dropping the response close
the connection unless the body was read to its end (HC16).

Tests: `tests/http/body/framing.txt`; `stream`, `truncated`, `truncated stream`, `body limit`, and `body limit later`
in `basics.bd`.

## HC11. The client

`Client.new()` has these settings, all public fields: `connect_timeout` 10 s, `read_timeout` 30 s, `total_timeout`
none, `follow_redirects` true, `max_redirects` 10 (clamped to 64), `max_retries` 2, `retry_backoff` 100 ms,
`max_backoff` 2 s, `retry_on_503` false, `user_agent` `burdin-http/0.1`, `max_idle_per_host` 4, `idle_timeout` 30 s,
and `max_body` 16 MiB. A clone of a client shares its pool (HC15) and its settings are then independent.

Tests: the `clones_share` scenario of `pooling.bd`; every scenario sets the settings it depends on.

## HC12. Redirects

When `follow_redirects` is on and a response is `301`, `302`, `303`, `307`, or `308` with a `Location`, the client
sends the request again to the resolved URL (HC2). A redirect without a `Location`, and any status when following is off,
is returned as the response. The method and body change as follows: `POST` becomes `GET` for `301`, `302`, and `303`;
`PUT`, `DELETE`, and the other methods become `GET` only for `303`; `HEAD` stays `HEAD`; `307` and `308` keep the
method and the body. When the method becomes `GET` the body and `Content-Type`, `Content-Encoding`, and
`Content-Language` are dropped. When the new URL is on another origin (scheme, host, or port differ, so a redirect from
`https` to `http` or the reverse always is), `Authorization`, `Cookie`, `Proxy-Authorization`, and `Host` are dropped; all
other fields are kept. The body of a redirect response is
read away (up to 64 KiB) so its connection is reused. `response.url` is the last URL and `response.redirects` the
count.

More redirects than `max_redirects` is `TooManyRedirects { limit }` (the redirect that would exceed the limit is not
followed); a redirect to a URL already visited in this call, the starting URL included, is `RedirectLoop`, reported at
once; a `Location` that does not resolve is `BadRedirect`; one to a scheme other than `http` and `https` is `UnsupportedScheme`. A redirect to `https` is followed like any
other (HC17); a redirect from `https` to `http` is followed too (the rules above do not forbid it) and carries none of
the credentials, but a program that wants to refuse a downgrade sets `follow_redirects` to `false` and checks the
`Location` itself.

Tests: `redirects.bd`: `statuses_for_get` (each status), `method_matrix` (every method and status above, with the body
and header checks), `locations`, `limit`, `loops`, `cross_origin` (two servers), `not_followed`, `bad_locations`
(which follows a redirect to an `https` URL and gets the connection refusal), and `redirect_with_body`; the `https`
redirects are in `tests/s3_http/tls` (HC17).

## HC13. Timeouts

Three limits, all `Option<Duration>`. `connect_timeout` bounds opening a connection (`TcpStream.connect_timeout`,
`spec/io.md`). `read_timeout` bounds each wait for bytes (and each write): while waiting for the head, and again for
every read of the body, so a server that sends a byte at a time keeps the request alive. `total_timeout` bounds one
`send`, redirects, retries, backoff sleeps, and the body included, and every wait is cut to what is left of it. Expiry
is `HttpError.Timeout { phase }`, which is both `is_timeout` and `is_connection_failure`; a limit of zero is never read
as "no limit". A timeout while reading the body is reported by the body read, after the status was already returned.

For an `https` URL the TLS handshake is part of opening the connection: each wait of the handshake is bounded by
`connect_timeout` (and cut to what is left of `total_timeout`), and the expiry is `Timeout` with the phase
`TLS handshake with host:port` (HC17).

The expiry of `connect_timeout` is not tested with a real stalled connection: a loopback connect never stalls, so
nothing deterministic can make one. Its plumbing is covered by `tests/s2_io/net_tcp_connect_timeout.bd` (a connection
within the limit succeeds, and a refused one reports the refusal, not a timeout).

Tests: `retries.bd`: `read_timeout`, `body_timeout`, `total_timeout`, `timeout_retried`, `connect_refused`.

## HC14. Retries

A request is repeated, on a new connection, after a failure to connect, a connection that closes or resets, or a timeout
(`is_connection_failure`), only when all of these hold: the method is idempotent (HC3); no byte of the response has
arrived (a head cut off in the middle is `BadResponse` and is not repeated, because the request may have been acted
on); and fewer than `max_retries` repeats were made. Each repeat waits `retry_backoff`, doubled each time up to
`max_backoff`, and no wait runs past `total_timeout`. A request with a body repeats the whole body. With
`retry_on_503` on, an idempotent request that got `503` is repeated the same way, after the seconds of its
`Retry-After` (at most `max_backoff`) or the backoff; when the repeats run out the `503` is the response.
Errors that are not connection failures (a malformed response, a body error, a redirect error) are never retried.

Tests: `retries.bd`: `get_retried`, `post_not_retried`, `put_retried`, `retries_exhausted`, `retry_503` (asked for, not
asked for, a `POST`, exhausted), `timeout_retried`, and `partial_head_not_retried`.

## HC15. Connection reuse

After a response whose body was read to its end and whose connection may be kept (HC16), the connection goes into the
client's pool, keyed by scheme, host, and port (the `origin`), and the next request to the same origin takes it, so an
`http` and an `https` connection to the same host and port are never exchanged (HC17). The pool keeps at most
`max_idle_per_host` idle connections per key (a connection past the limit is closed) and drops one idle for more than
`idle_timeout`. Before a pooled connection is used, a non-blocking probe checks that the server has not closed it (over TLS the
probe reads through the TLS layer, so a `close_notify` or a cut connection discards it too), and a closed one is
discarded. A pooled connection can still be closed between the probe and the write; when a reused
connection fails before any response byte, an idempotent request is repeated on a new connection without using up a
retry, and any other request returns the error (HC14). `max_idle_per_host = 0` turns reuse off and sends
`Connection: close`. `idle_connections` counts the idle connections and `close_idle` closes them all.

Tests: `pooling.bd`: `reuse`, `per_host`, `max_idle`, `no_pool`, `stale_probe`, `stale_race` (a `GET` repeated, a `POST`
not), `idle_timeout`, and `clones_share`, each counting the connections the server accepted.

## HC16. Closing and keep-alive

A connection is kept when the response is `HTTP/1.1` without `Connection: close`, or `HTTP/1.0` with
`Connection: keep-alive`, and the request did not ask to close, and the body was read completely and exactly (no
bytes left over, and not a close-delimited body). Otherwise it is closed when the body ends or when the response is
dropped. A body that is dropped or closed before its end closes the connection: its remaining bytes are unknown, so
the connection is never put back; `discard` can read a small remainder away first so the connection survives. The
server closing the connection after the response (`Connection: close`, `HTTP/1.0`, or a close-delimited body) makes the
next request open a new one.

Tests: `pooling.bd`: `unread_body` (the server sees the client close), `partial_then_discard`, `connection_close`,
`http_10` (with and without `keep-alive`), `no_pool`; `basics.bd`: `close-delimited used two connections`.

## HC17. `https`

An `https` URL is sent over TLS provided by the `tls` package (`spec/tls.md`). The rules:

- **Opening.** The client connects TCP to the URL's host and port (443 unless the URL names one), then runs
  `TlsStream.connect` with the URL's host as the server name. That one value is the SNI and the name the certificate must
  match (`spec/tls.md`, TLS3); a host that is an IP address is matched against the certificate's IP entries and is not
  sent as SNI. The request is written only after the handshake has finished, so a connection whose handshake failed has
  carried no HTTP byte. The request line, `Host`, framing, bodies, redirects, and retries then work exactly as for
  `http`: the connection is one `Read` and `Write` underneath, and everything above it is shared.
- **Verification is on by default.** The default configuration is `tls.ClientConfig.new()`: the system trust store
  (`SSL_CERT_FILE` and `SSL_CERT_DIR` apply), TLS 1.2 or later, the certificate chain and the host name checked. A failure
  is `HttpError.Tls(TlsError)`: the `tls` error with its message (`certificate verification failed: ...`, `the certificate
  does not match the host: ...`, or a handshake failure), and `is_tls()` and `is_verification()` classify it.
  `HttpError.Tls` is not a connection failure (HC14): it is never retried, and the client never makes the request again
  without TLS, on the same connection or another. There is no fallback to plain text and no "try `http` instead".
- **Configuration.** `client.trust(store)` verifies against exactly the authorities of a `tls.TrustStore`;
  `client.insecure_skip_verify()` turns verification off (it is named for what it does and is for tests; see the warning
  in `spec/tls.md`, TLS3); `client.tls_config(config)` takes a whole `tls.ClientConfig` (a minimum version, a client
  certificate, and so on) and uses it as it is. The default and the first two offer the ALPN protocol `http/1.1` and no
  other; a server that selects nothing is fine. The package speaks HTTP/1.1 only. A clone of a client has the
  configuration its original had when it was cloned and then its own.
- **Timeouts.** The handshake is bounded by `connect_timeout` per wait and by `total_timeout` (HC13); the expiry is
  `Timeout { phase: "TLS handshake with host:port" }`. `read_timeout` bounds the reads and writes after it, as for `http`.
  A transport failure during the handshake (a reset) is `Io` and a connection failure, like one while connecting; a TLS
  failure, a server that closes during the handshake included, is `Tls`.
- **The pool.** A connection is pooled under its origin: `https://host:port` and `http://host:port` are different keys
  (HC15). A pooled TLS connection is probed through the TLS layer before it is reused (a `close_notify`, stray data, or a
  cut connection discards it), and a stale one is replaced on a new connection, with a new handshake, when the request is
  safe to repeat (HC15). Discarding a pooled TLS connection (`close_idle`, the idle limit, or dropping a response whose
  body was not read) sends a `close_notify`, best effort and never waiting more than 200 ms.
- **Truncation.** A TLS connection that ends without a `close_notify` is not an orderly end (`spec/tls.md`, TLS5). A body
  whose framing says more bytes follow (a `Content-Length` or chunked body) that ends that way is `BodyTruncated`, and so is
  a close-delimited body: over TLS it ends cleanly only with a `close_notify`, so a server that closes such a body by
  closing the socket is reported as truncated. A response head that ends that way before any byte of it is
  `ConnectionClosed` (and repeated when safe, HC14). A TLS error after the handshake (a bad record, a fatal alert) is
  `Io`, with the TLS message as its reason.
- **Redirects.** A redirect from `http` to `https`, from `https` to `https`, and from `https` to `http` follows the rules
  of HC12; the last one changes the origin, so it drops `Authorization`, `Cookie`, and `Proxy-Authorization`.
- **What is not provided:** proxies and `CONNECT`, session resumption, HTTP/2, a way to read the negotiated protocol or the
  peer certificate from a `Response`, and any check beyond what `tls` does (no revocation, no pinning, no certificate
  transparency). This is an experiment, not a security claim (`spec/tls.md`).

Not verified on macOS and Windows: the TLS tests run in CI after the job installs OpenSSL (`spec/tls.md`, TLS7), but only
Linux with OpenSSL 3.5 has been run by hand.

Tests: `tests/s3_http/tls`, a scripted server that speaks TLS with the certificates of `tests/tls/regenerate.py`:
`get`, `post`, `bodies`, `keep-alive`, `pool keys`, `redirects`, `unknown authority`, `wrong host`, `expired`, `skip verify`,
`trust`, `truncated`, `handshake timeout`, `plain port`, and `retries`; and the scheme message in
`tests/http/url/basic.txt`, `tests/http/join/references.txt`, and `tests/http/request/wire.txt`.
