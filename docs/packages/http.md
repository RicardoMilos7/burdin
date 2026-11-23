# http

An HTTP/1.1 client over `std`'s `TcpStream`, with `https` through the `tls` package (issues #260, #328, #329, and #330; `spec/http-client.md`). It has timeouts, redirects, retries of safe requests, and per-host connection reuse.

Version `0.1.0`. Use it with `use http`; it is an official package, so `burdin.toml` needs no entry for it. The package's own guide is [`packages/http/README.md`](../../packages/http/README.md).

## Requirements

Nothing beyond the compiler on Windows. On Linux and macOS, `http` imports `tls`, so a program that uses it links OpenSSL 3 (see the [`tls` page](tls.md#requirements)).

## Worked example

Starts a one-shot server on a free local port, sends a request with `Client`, and reads the response. The server runs in a task, so the program needs no network.

```burdin
use http

fn main() {
    // A one-shot server on a free local port: it reads the request, answers, and closes.
    let listener = net.TcpListener.bind("127.0.0.1:0").unwrap()
    let port = listener.local_addr().unwrap().port()
    let url = format("http://127.0.0.1:{}/hello", port)
    task.scope(|s| {
        s.spawn(|| {
            let (mut conn, _) = listener.accept().unwrap()
            let mut request: List<u8> = List<u8>.new()
            conn.read(mut request, 4096).unwrap()
            let reply = "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello"
            let _sent = conn.write(reply.to_utf8().as_slice()).unwrap()
        })
        let client = http.Client.new()
        let mut response = client.get(url.as_str()).unwrap()
        assert(response.is_success())
        assert_eq(response.header("content-length").unwrap().as_str(), "5")
        let text = response.text().unwrap()
        assert_eq(text.as_str(), "hello")
        println("{} {}", response.is_success(), text)
    })
}
```

Save it as `main.bd` and run it with `burdin run main.bd`, or copy the body into a package that declares `use http`. Every example in the package's doc comments is also compiled and run by `burdin test --doc`.

## Surface

Every public item and public method, in source order. Names are shown as `burdin doc` shows them: an item is `module.Name`, and a package whose root re-exports it (`use tls.{Certificate}`) also reaches it from the root. Each row's summary is the first sentence of the item's doc comment; the full text and the examples are on the generated site.

| Item | Signature | Summary |
| --- | --- | --- |
| `http.HttpError` | `error enum HttpError: Clone, Debug` | Every way a request can fail. |
| `http.HttpError.is_timeout` | `fn is_timeout(self): bool` | Whether the request timed out (connecting, writing, or waiting for the response or the body). |
| `http.HttpError.is_tls` | `fn is_tls(self): bool` | Whether the failure is a TLS failure while opening an `https` connection (HC17). |
| `http.HttpError.is_verification` | `fn is_verification(self): bool` | Whether the server's certificate did not verify: the chain, the dates, or the host name (HC17). |
| `http.HttpError.is_connection_failure` | `fn is_connection_failure(self): bool` | Whether the failure is one of the connection kind: the request never got a response and may not have been seen by the server. |
| `http.Method` | `enum Method: Clone, Eq, Debug` | A request method. |
| `http.Method.parse` | `fn parse(text: str): Result<Method, HttpError>` | Reads a method name. |
| `http.Method.to_text` | `fn to_text(self): String` | The method as written on the request line. |
| `http.Method.is_head` | `fn is_head(self): bool` | Whether the method is `HEAD`, whose response has no body. |
| `http.Method.is_idempotent` | `fn is_idempotent(self): bool` | Whether repeating the request has the same effect as sending it once (RFC 9110, 9.2.2): the methods a client may retry on its own. |
| `http.Header` | `struct Header: Clone, Eq, Debug` | One header field. |
| `http.Headers` | `struct Headers: Clone, Debug` | A list of header fields. |
| `http.Headers.new` | `fn new(): Headers` | An empty list of header fields. |
| `http.Headers.len` | `fn len(self): usize` | How many fields there are, counting repeated names. |
| `http.Headers.is_empty` | `fn is_empty(self): bool` | Whether there are no fields. |
| `http.Headers.name_at` | `fn name_at(self, index: usize): String` | The name of field `index`, as it was added. |
| `http.Headers.value_at` | `fn value_at(self, index: usize): String` | The value of field `index`. |
| `http.Headers.add` | `fn add(mut self, name: str, value: str): Result<(), HttpError>` | Appends a field, after any with the same name. |
| `http.Headers.set` | `fn set(mut self, name: str, value: str): Result<(), HttpError>` | Makes `name` have exactly one field: the first one with that name takes the new value (and the spelling of the name given here), the others are removed. |
| `http.Headers.get` | `fn get(self, name: str): Option<String>` | The value of the first field called `name`. |
| `http.Headers.get_all` | `fn get_all(self, name: str): List<String>` | The values of every field called `name`, in order. |
| `http.Headers.contains` | `fn contains(self, name: str): bool` | Whether some field called `name` exists, whatever its case. |
| `http.Headers.remove` | `fn remove(mut self, name: str): usize` | Removes every field called `name` and returns how many there were. |
| `http.Headers.has_token` | `fn has_token(self, name: str, token: str): bool` | Whether some field called `name` lists `token` among its comma-separated elements, ignoring case: `has_token("Connection", "close")`. |
| `http.Headers.to_wire` | `fn to_wire(self): String` | The fields as they go on the wire: `Name: value` and a CRLF for each. |
| `http.Url` | `struct Url: Clone, Eq, Debug` | A parsed `http` or `https` URL. |
| `http.Url.parse` | `fn parse(text: str): Result<Url, HttpError>` | Reads an absolute `http` or `https` URL (HC1). |
| `http.Url.is_https` | `fn is_https(self): bool` | Whether the scheme is `https`. |
| `http.Url.default_port` | `fn default_port(self): u16` | The port a connection to this URL uses when the URL names none: 443 for `https`, 80 for `http`. |
| `http.Url.authority` | `fn authority(self): String` | `host`, or `host:port` when the port is not the scheme's default: the value of the `Host` header. |
| `http.Url.address` | `fn address(self): String` | `host:port`, the address a connection is made to. |
| `http.Url.target` | `fn target(self): String` | The path and query: the target of the request line. |
| `http.Url.origin` | `fn origin(self): String` | The scheme, host, and port: what two URLs share when a redirect stays on the same origin (HC12). |
| `http.Url.same_origin` | `fn same_origin(self, other: Url): bool` | Whether `other` has the same scheme, host, and port: the test a redirect makes before it keeps credentials. |
| `http.Url.to_text` | `fn to_text(self): String` | The URL as text. |
| `http.Url.join` | `fn join(self, reference: str): Result<Url, HttpError>` | Resolves `reference` against this URL, as a browser resolves a `Location` header (HC2, RFC 3986, 5): an absolute URL replaces this one, `//host/path` keeps the scheme, `/path` keeps the origin, ... |
| `http.Request` | `struct Request: Clone` | A request: a method, a URL, headers, and a body that is held in memory. |
| `http.Request.new` | `fn new(own method: Method, url: str): Result<Request, HttpError>` | A request with no headers and no body. |
| `http.Request.get` | `fn get(url: str): Result<Request, HttpError>` | A `GET` request for `url`. |
| `http.Request.head` | `fn head(url: str): Result<Request, HttpError>` | A `HEAD` request for `url`: the response has the headers of the resource and no body. |
| `http.Request.post` | `fn post(url: str): Result<Request, HttpError>` | A `POST` request for `url`, with no body until `body_bytes` or `body_text` sets one. |
| `http.Request.put` | `fn put(url: str): Result<Request, HttpError>` | A `PUT` request for `url`, with no body until `body_bytes` or `body_text` sets one. |
| `http.Request.delete` | `fn delete(url: str): Result<Request, HttpError>` | A `DELETE` request for `url`. |
| `http.Request.header` | `fn header(own self, name: str, value: str): Request` | Adds a header field. |
| `http.Request.body_bytes` | `fn body_bytes(own self, data: [u8]): Request` | Sets the body to a copy of `data`. |
| `http.Request.body_text` | `fn body_text(own self, text: str): Request` | Sets the body to the UTF-8 bytes of `text`. |
| `http.Request.to_wire_head` | `fn to_wire_head(self, user_agent: str, close: bool): String` | The request head as it goes on the wire (HC6): the request line, `Host` (unless the caller set one), the caller's fields in order, `User-Agent` (unless the caller set one), `Content-Length` when ... |
| `http.ResponseHead` | `struct ResponseHead: Clone, Debug` | A parsed status line and header block. |
| `http.find_head_end` | `fn find_head_end(buffer: List<u8>): Option<usize>` | The offset just after the first blank line (`CRLF CRLF`) of `buffer`, which is where the body starts, or `None` when the head is not complete yet. |
| `http.parse_response_head` | `fn parse_response_head(head: [u8]): Result<ResponseHead, HttpError>` | Reads a response head (HC7): `HTTP/1.0` or `HTTP/1.1`, a three-digit status, an optional reason, and header fields that each end in CRLF. |
| `http.Framing` | `enum Framing: Copy, Clone, Eq, Debug` | How the end of a response body is known (HC8). |
| `http.framing_of` | `fn framing_of(method: Method, status: u16, headers: Headers): Result<Framing, HttpError>` | The framing of the response to a request with `method`, from its status and headers (RFC 9112, 6.3): no body for `HEAD`, `1xx`, `204`, and `304`; `chunked` when `Transfer-Encoding` ends in it; ... |
| `http.keeps_alive` | `fn keeps_alive(minor: u8, headers: Headers): bool` | Whether the connection may carry another request after this response (HC16): the response has no `Connection: close`, and an `HTTP/1.0` response asked for `keep-alive`. |
| `http.BodyReader` | `struct BodyReader<R>` | Decodes a response body from `source` according to its framing (HC8 to HC10). |
| `http.BodyReader.new` | `fn new(own source: R, framing: Framing, own leftover: List<u8>): BodyReader<R>` | A reader over `source`. |
| `http.BodyReader.is_complete` | `fn is_complete(self): bool` | Whether the whole body has been read. |
| `http.BodyReader.is_clean` | `fn is_clean(self): bool` | Whether the body was read to its end with nothing left over, so the connection can carry another response (HC16): a close-delimited body never is, and bytes that follow the body mean the peer sent ... |
| `http.BodyReader.trailers` | `fn trailers(self): Headers` | The trailer fields of a chunked body; empty until the last chunk has been read. |
| `http.BodyReader.delivered` | `fn delivered(self): u64` | How many body bytes `read_body` has returned. |
| `http.BodyReader.into_source` | `fn into_source(own self): R` | Gives back the source, with whatever it has not yet been asked for still in it. |
| `http.BodyReader.read_body` | `fn read_body(mut self, mut out: List<u8>, max: usize): Result<usize, HttpError>` | Appends up to `max` body bytes to `out` and returns how many. |
| `http.Idle` | `struct Idle` | An idle connection waiting in the pool, with the origin it goes to and when it was put back. |
| `http.Conn` | `struct Conn` | A connection in use: the socket and what is needed to hand it back to the pool when a response has been read to its end. |
| `http.Body` | `struct Body` | A response body: a `Read` over the connection that decodes the framing. |
| `http.Body.is_finished` | `fn is_finished(self): bool` | Whether the whole body has been read. |
| `http.Body.trailers` | `fn trailers(self): Headers` | The trailer fields of a chunked body (HC9); empty until the body has been read to its end. |
| `http.Body.read_http` | `fn read_http(mut self, mut buf: List<u8>, max: usize): Result<usize, HttpError>` | Appends up to `max` bytes of the decoded body to `buf`; `Ok(0)` is the end. |
| `http.Body.bytes` | `fn bytes(mut self): Result<List<u8>, HttpError>` | Reads the rest of the body into a list; more than the limit is `BodyTooLarge` and closes the connection. |
| `http.Body.discard` | `fn discard(mut self, limit: usize): Result<bool, HttpError>` | Reads and discards up to `limit` bytes, so a connection whose body is small can be reused. |
| `http.Response` | `struct Response` | A response: the status line and headers, and a `Body` to read the rest from. |
| `http.Response.is_success` | `fn is_success(self): bool` | Whether the status is `2xx`. |
| `http.Response.is_redirect` | `fn is_redirect(self): bool` | Whether the status is one that points to another URL: `301`, `302`, `303`, `307`, or `308`. |
| `http.Response.header` | `fn header(self, name: str): Option<String>` | The value of the first header field called `name`. |
| `http.Response.trailers` | `fn trailers(self): Headers` | The trailer fields of a chunked body, once it has been read. |
| `http.Response.bytes` | `fn bytes(mut self): Result<List<u8>, HttpError>` | Reads the rest of the body (at most `Client.max_body` bytes). |
| `http.Response.text` | `fn text(mut self): Result<String, HttpError>` | Reads the rest of the body as UTF-8 text. |
| `http.Response.discard` | `fn discard(mut self, limit: usize): Result<bool, HttpError>` | Reads and discards the rest of the body, up to `limit` bytes, so the connection can be reused. |
| `http.Response.close` | `fn close(own self)` | Closes the connection without reading the rest of the body. |
| `http.Client` | `struct Client: Clone` | An HTTP client: its settings are public fields, set after `Client.new()`. |
| `http.Client.new` | `fn new(): Client` | A client with the defaults: 10 seconds to connect, 30 seconds per read, no total limit, up to 10 redirects, two retries of safe requests with a 100 millisecond backoff, and up to 4 idle connections ... |
| `http.Client.tls_config` | `fn tls_config(mut self, own config: ClientConfig)` | Uses `config` for every `https` connection from now on, as it is (HC17): its trust store, versions, ALPN, and client certificate. |
| `http.Client.trust` | `fn trust(mut self, store: TrustStore): Result<(), HttpError>` | Verifies `https` servers against exactly the authorities of `store` and offers `http/1.1` by ALPN (HC17). |
| `http.Client.insecure_skip_verify` | `fn insecure_skip_verify(mut self): Result<(), HttpError>` | Turns certificate and host name verification off for `https` (HC17). |
| `http.Client.idle_connections` | `fn idle_connections(self): usize` | How many idle connections the pool holds, over all hosts. |
| `http.Client.close_idle` | `fn close_idle(self)` | Closes every idle connection. |
| `http.Client.get` | `fn get(self, url: str): Result<Response, HttpError>` | A `GET` of `url`: `send` of `Request.get(url)`. |
| `http.Client.head` | `fn head(self, url: str): Result<Response, HttpError>` | A `HEAD` of `url`: the headers of the resource, with no body. |
| `http.Client.delete` | `fn delete(self, url: str): Result<Response, HttpError>` | A `DELETE` of `url`. |
| `http.Client.post` | `fn post(self, url: str, body: str): Result<Response, HttpError>` | `POST` with a text body. |
| `http.Client.put` | `fn put(self, url: str, body: str): Result<Response, HttpError>` | `PUT` with a text body. |
| `http.Client.send` | `fn send(self, request: Request): Result<Response, HttpError>` | Sends a request and returns the response as soon as its head has arrived; the body is read from `response.body`. |
