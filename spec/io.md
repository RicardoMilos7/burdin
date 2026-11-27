# Asynchronous I/O: the reactor, the helper pool, and `blocking` extern functions

Milestone 2.4 (issue #220) makes I/O inside a task look blocking while pausing only that task. This page is the exact
behavior and the decisions behind it; the tests are `tests/s2_io/`. Rule IDs are IO1-IO8. The surface is `std.net`
(`TcpListener`, `TcpStream`, `UdpSocket`, `SocketAddr`, `lookup_host`), the reactor in `std/src/reactor.bd`, the
socket primitives in the compiler's runtime, and the `blocking` marker on an `extern` declaration.

```
let listener = net.TcpListener.bind("127.0.0.1:0")?     // a real port, chosen by the system
let port = listener.local_addr()?.port()
task.scope(|s| {
    s.spawn(|| echo_once(listener)?)                     // parks in accept, then in read
    s.spawn(|| connect_and_send(port)?)
})

let mut socket = net.UdpSocket.bind("127.0.0.1:0")?
let (n, from) = socket.recv_from(mut bytes, 4096)?       // parks until a datagram arrives

let addrs = net.lookup_host("localhost", 8080)?          // parks while the system resolves the name

extern "C" {
    blocking fn slow_call(arg: u32): i32                // a C function that may block
}
let answer = unsafe { slow_call(7) }                     // inside a task: runs on a helper thread
```

## IO1. Blocking-looking calls; only the task waits (decision, issue #221)

Every socket, file, name-resolution, and `blocking` extern call has one spelling and one behavior: it returns when the
operation is done. What waits depends on the caller:

* **Inside a task**, the task records where a waker will find it and parks. Its worker runs other tasks. When the
  operation is ready or finished, the waker makes the task ready again, possibly on another worker. A task never holds
  a worker while it waits for the system.
* **On the main thread or a `thread.spawn` thread**, the call blocks the thread on the scheduler's condition variable.
  There is no task to park and no worker to free.

This is the whole point of the milestone (`Burdin Language Design.md`, "Concurrency"): there is no `async`/`await` and
no function coloring, so a function that reads a socket is an ordinary function and works the same from a task, a
thread, or `main`.

Rejected alternatives:

* *A separate async API* (`AsyncTcpStream`, futures, callbacks). It brings function coloring back and splits the
  library in two.
* *Blocking the worker.* A few blocked workers stop the pool from making progress; with one worker, any blocking I/O
  deadlocks the program.
* *A user-space poller in `std` only.* The event facility (epoll, kqueue, a completion port) is per platform and must
  live under the scheduler's primitives, next to the context switch and the locks.

## IO2. One reactor behind the operating systems (decision, issue #222)

`std/src/reactor.bd` owns one reactor for the process, created and started on the first call that needs it. The
reactor is three things:

* **The event facility**, provided by the compiler's task runtime (`crates/burdin_codegen/src/taskrt/`):
  * Linux: `epoll`, with an `eventfd` in the set as the wakeup token.
  * macOS: `kqueue`, with an `EVFILT_USER` event as the wakeup token.
  * Windows: an I/O completion port carries finished helper calls, Winsock `WSAPoll` reports socket readiness, and a
    loopback UDP datagram wakes the poll. The datagram is sent from a second socket, because Winsock serializes the
    socket a thread is polling or receiving on, so the polled socket cannot also be the sender. (A fully overlapped,
    per-operation I/O path for Windows is a later refinement; the socket layer is readiness-based, so it waits the
    same way on every platform.)
* **The reactor thread**, a Burdin thread started with `__thread_start`. It waits for events or the nearest deadline,
  claims each fired wait exactly once, wakes the waiting task or thread, and repeats. It is not a worker: the tasks it
  wakes run on the scheduler's workers.
* **The timer list**, sorted by deadline, and its lock. A new timer and a new descriptor registration each wake the
  reactor so it recomputes its wait: `epoll` and `kqueue` take a new set when one is added, but `WSAPoll` keeps
  waiting on the set it was given.

A **wait record** (`Wait`) is allocated on the waiting task's stack. The runtime reads two fields of it by fixed
offset: the first word is the result-list link and the field at offset 24 is the event kind (1 ready, 2 completed).
The standard library owns the rest: the waiter, the descriptor, the timer node, and the claimed flag. A wait is
claimed under the reactor's lock before it is woken, so an event and a timeout that happen together wake the task
once. A descriptor registration is **one-shot**: the runtime removes it when it reports the event, so a level-triggered
facility cannot report the same task repeatedly. The task re-registers whenever it retries the operation.

The reactor primitives are `__io_new`, `__io_watch`, `__io_unwatch`, `__io_poll`, `__io_wake`, `__io_global_get`,
`__io_global_set`, `__pool_submit`, and `__pool_submit_raw` (`spec/tasks.md`, TK2; the names are
`crates/burdin_tir/src/ops.rs`). They are linked in only when used, like the rest of the task runtime.

A wait is registered under the scheduler's state lock and then parks through `waiter_block` (`spec/channels.md`,
CH10), which is the same mechanism a channel uses. A notification cannot be lost: the waker takes the state lock
before it notifies, and the waiting task holds it from registration to parking.

## IO3. The file I/O helper pool (decision, issue #224)

File operations cannot be made ready-based on every platform (a file descriptor is "ready" nearly always, but the
call may still block on a disk; Windows has no epoll at all). They run on the **helper pool**: a few threads
(`max(2, min(4, processors))`) started with the reactor, a FIFO queue of jobs, and the same wait records as sockets.
When a file operation finishes, the helper pushes the waiting task's record onto the reactor's completion list and
wakes the reactor; the reactor thread wakes the task.

`std/src/reactor.bd` wraps this as one function:

```
fn offload<T, F: FnMut(): T>(own f: F): T
```

It runs `f` on a helper thread and waits for it, parking the task or blocking the thread. `f` needs **no `Send`
bound**, and the reason is the whole argument for its soundness: the call does not return until the helper is done, so

* everything `f` borrows outlives the call, and
* nothing else can reach those places during it, because the closure is passed to an exclusive (`own`) parameter and
  the caller is parked.

The closure is relabelled as an owned `FnOnce` with the same unsafe primitive scoped tasks use (`__closure_own`,
`spec/tasks.md` TK7), which is what lets a closure that borrows a `List` buffer or a `str` path cross to the helper.
The compiler checks the loan against the call, not against the helper thread's lifetime, and the two coincide.

Outside a task, `offload` blocks the calling thread; it still uses a helper, so the same code path runs everywhere.
A panic in a helper aborts the process, like a panic in a task (TK8).

The wrapped operations are file and directory handling: `fs.read`, `fs.write`, `fs.append`, `File.open`, `File.read`,
`File.write`, `File.seek`, `File.sync_all`, `File.set_len`, `read_dir`, `create_dir`, `remove_dir`, `remove_file`,
`rename`, `metadata`, `canonicalize`, `read_dir_iter` (its open and each step), and the whole-file
`__read_to_string`/`__write_file` primitives. Dropping a `File` closes it inline: closing a descriptor does not wait
for data. An operation that makes many system calls runs as one `offload` and calls the primitives directly on the
helper, so it pays one round trip rather than one per call: `walk` and `walk_glob` (every directory and every
entry's metadata), and `copy_file`'s read and write loop (issue #1021). Dropping a `ReadDir` closes the directory
inline too.

Tests: `s2_io/file_in_task` (correctness), `file_fifo_offload` (a blocking `open` on a FIFO parks the task while
another task writes; with one worker, an inline open would deadlock), `stress_files` (many tasks at once, repeated in
CI), `offload_leak_free` (the checking allocator sees no leak).

## IO4. `blocking` extern functions (decision, issue #223)

An `extern "C"` declaration may carry the contextual marker `blocking` before `fn` (or before `unsafe fn`):

```
extern "C" {
    blocking fn usleep(usec: u32): i32
}
```

It promises that the function may block its thread. A call to one runs the C function on the helper pool and parks
the calling task (or blocks the calling thread), exactly as a file operation does. The thunk is generated by the
compiler:

1. The call site stores every argument in an 8-byte-slot block on its stack (integers, floats, and pointers all fit).
2. A generated thunk `void (ptr block, i1 owned)` loads the arguments, calls the C function, and stores the result in
   the slot after the arguments.
3. The call site calls the standard library's `std.offload_blocking(thunk, block)`, which submits the thunk to the
   pool and waits, and then reads the result from its own block.

The rules:

* `blocking` is allowed only on a declaration inside an `extern "C" { ... }` block. Anywhere else it is **E0290**.
* A blocking declaration cannot be variadic: the thunk marshals a fixed parameter list, so it needs a fixed one. It is
  **E0290**.
* One C symbol has one declaration and one `blocking` answer; two declarations of one symbol that disagree are
  **E0290** (a second declaration with the same answer reuses the first, as any `extern` does).
* The call still requires `unsafe`, and its argument and return types follow the ordinary C ABI rules.

The same soundness argument as IO3 applies: the caller waits for the thunk, so a pointer argument stays valid, and the
call is exclusive for the duration. A program without a blocking extern carries neither the thunk machinery nor the
standard library's shim: reachability roots the shim only when `program.externs` has a blocking declaration.

Tests: `s2_io/blocking_parks_worker` (one worker; the fast task runs first), `blocking_two_tasks` (two helpers at
once), `blocking_outside_extern`, `blocking_variadic` (E0290).

## IO5. Sockets

`std/src/net.bd` provides:

```
struct SocketAddr                            // an IPv4 address and a port; parse, to_text, ip, port, octets
struct TcpListener { fn bind(str), accept(): (TcpStream, SocketAddr), local_addr(), close() }
struct TcpStream { fn connect(str), connect_timeout(str, Duration), read, write, peer_addr, local_addr, set_read_timeout,
                    set_write_timeout, set_nodelay, shutdown_read, shutdown_write, close }
struct UdpSocket { fn bind(str), recv_from(): (usize, SocketAddr), send_to, local_addr, set_read_timeout, close }
fn lookup_host(host: str, port: u16): Result<List<SocketAddr>, IoError>
```

* An address is `host:port`, split at the first `:`. The host part is a name or a dotted quad; `bind` with an empty
  host binds the wildcard address. The port is a decimal number from 0 to 65535. Port 0 asks the system for a free
  port, which `local_addr` reports; the conformance tests use it so they never collide with another process.
* The descriptor is always non-blocking. The operations map onto the C calls (`socket`, `bind`, `listen`, `accept`,
  `connect`, `send`/`sendto`, `recv`/`recvfrom`, `shutdown`, `setsockopt`), and their result convention is
  `>= 0` success (bytes, a descriptor, or zero), `-1` would-block or connect-in-progress, `-2` failure with the
  system code in a thread-local slot that `__net_last_error()` reports. `std` loops on `-1`: it registers the
  descriptor with the reactor (`readable` or `writable`) and retries when it is woken.
* `connect` is non-blocking; when it returns in progress, the task waits for writability and then reads `SO_ERROR`
  (`__net_error`) to learn whether the connection was established or refused. `connect_timeout(addr, timeout)` puts
  a deadline on that wait: when it passes the attempt is abandoned, its socket is closed, and the call reports
  `IoErrorKind.TimedOut`.
* A read returns `Result.Ok(0)` at the end of the stream (`recv` returning 0), which is what `Read.read_to_end` expects.
* `TcpStream` implements `Read` and `Write`, so `read_exact`, `read_to_end`, `read_to_string`, `write_all`, `write_text`,
  and `write_line` come from `std.rw`.
* `set_read_timeout`/`set_write_timeout` take an `Option<Duration>`; `Option.None` waits forever. A timeout is a deadline on
  the reactor's timer list, so a waiting task wakes at the deadline even with no traffic, and the call reports
  `IoError.Net` with `IoErrorKind.TimedOut`. A deadline is computed once per call and shared by its retries.
* A write to a peer that has closed is an error, never a signal. `send` passes `MSG_NOSIGNAL` on Linux, and a macOS
  socket sets `SO_NOSIGPIPE` when it is created (an accepted socket inherits it from its listener), so `SIGPIPE` does
  not end the process. The error is the system's own (`BrokenPipe`, `ConnectionReset`, or `ConnectionAborted`), and a
  write waiting for room returns it when the peer goes away, or `TimedOut` when its deadline passes first.
  `s2_io/net_write_peer_closed` covers a peer closed before the write, a peer closed during the wait, and a silent
  peer under a write timeout (issue #867).
* A write whose wait woke but made no room pauses for 10 ms (or until its deadline, if sooner) before it waits again.
  Windows reports a half-closed peer as a hang-up at once, even with a full send buffer (a `WSAPoll` probe confirms it),
  so without the pause a writer could wake and re-register in a tight loop. Measured runs of the writer did not spin,
  so the pause is a guard. The pause adds at most 10 ms to a real wake-up. `s2_io/net_write_waits_for_room`
  covers a write with no timeout that waits on a peer which shuts its write side and reads later: it must finish, with
  every byte delivered.
* Dropping a socket closes it; `close(own self)` does the same and reports a failure. `TcpListener`/`TcpStream`/
  `UdpSocket` are `Send` and `Sync` because every field is (SH4): a descriptor is an integer, and the socket's state
  lives in the kernel. A `TcpStream` cannot be `Copy`: closing is a destructor.
* Errors are `IoError.Net { op, addr, kind, os_code }` (`spec/errors.md`, ERR10). The portable kinds gained
  `ConnectionRefused`, `ConnectionReset`, `ConnectionAborted`, `NotConnected`, `AddrInUse`, `AddrNotAvailable`,
  `NetworkUnreachable`, and `HostUnreachable` (ERR11), and the code tables map the Unix `errno` values and the Winsock
  codes to them. A failure `std` detects itself (an unparsable address, a timeout) stores `os_code: Option.None`.

Limits: **IPv4 only**. `SocketAddr` holds four bytes, `lookup_host` returns IPv4 addresses, and a name with only IPv6
answers is `NotFound`. IPv6 is a later item; the address type and the primitives are shaped so it can be added
without changing the socket surface.

Tests: `s2_io/net_tcp_echo` (issue #225: a server task, a client task, one echo), `net_tcp_messages` (200 messages
each way), `net_tcp_timeout`, `net_tcp_refused`, `net_scope_borrow`, `stress_connections` (six clients at once,
repeated in CI), `net_borrow_conflict` (E0111), `net_view_channel` (E0120), `net_udp`.

## IO6. Name resolution (decision, issue #408)

`lookup_host` runs `getaddrinfo` on the helper pool (`offload`). The resolver may take hundreds of milliseconds or
block on a broken DNS server; running it on a worker would stall the pool. The primitive
`__dns_lookup(host) -> Result<String, i32>` returns one `a.b.c.d` per line, and `std` builds the `List<SocketAddr>`
with the caller's port. The first address is the preferred one. A failure carries the resolver's code (`EAI_*` on
Unix, `WSAHOST_NOT_FOUND` and friends on Windows), and an empty answer is `NotFound`. The primitive is impure: a
Compile-time evaluation that reaches it is E0224.

Tests: `s2_io/net_lookup_host`.

## IO7. Timers

`time.sleep` inside a task parks the task on the reactor's timer list instead of holding its worker; outside a task
it blocks the thread with the system's sleep, as before. `Duration.zero()` returns at once. A task's sleep lasts at
least the requested time (the system may round it up), and a socket timeout uses the same list, so a program that
waits on a socket and a program that sleeps use one mechanism. A `select` over a channel and a task that sleeps after
a deadline is the timeout pattern the tests use; there is no separate timer channel.

Tests: `s2_io/timer_parks_task`, `timer_sleep_accuracy`, `channel_watchdog`.

## IO8. What is not promised

* **The pool and the reactor are never stopped.** They live as long as the process, like the scheduler (TK10). Their
  records come from the C allocator and are not part of any test's leak report; a task's wait records and timer nodes
  are per call and are reported if leaked.
* **Every `blocking` call uses a helper**, even from the main thread: `offload_raw` submits the thunk and waits.
  A worker only runs tasks, so a call made on a worker is always made from a task and parks it.
* **Cancellation does not exist.** A wait ends when the event arrives, the deadline passes, or the program ends. There
  is no way for another task to cancel a pending read.
* **One reactor, one process.** There is no way to choose a backend, size the pool, or run a second event loop.
* **The file pool is FIFO and unbounded.** A job for every pending file operation, on the same few threads. A very
  large file is copied through the caller's buffer as the stages read it; there is no `sendfile`-style zero copy.
* **Windows' readiness path is `WSAPoll`**, not overlapped completion per operation (IO2); the completion port is used
  for the helper calls and the wakeup.

## Tests

| Rule | Tests |
| --- | --- |
| IO1, IO3 | `s2_io/file_in_task`, `stress_files`, `offload_leak_free` |
| IO2 | `s2_io/timer_parks_task`, `channel_watchdog`, `stress_connections` |
| IO3 | `s2_io/file_fifo_offload` |
| IO4 | `s2_io/blocking_parks_worker`, `blocking_two_tasks`, `blocking_outside_extern`, `blocking_variadic` |
| IO5 | `s2_io/net_tcp_echo`, `net_tcp_messages`, `net_tcp_timeout`, `net_tcp_connect_timeout`, `net_tcp_refused`, `net_scope_borrow`, `net_udp`, `stress_connections`, `net_borrow_conflict`, `net_view_channel`, `net_write_peer_closed`, `net_write_waits_for_room` |
| IO6 | `s2_io/net_lookup_host` |
| IO7 | `s2_io/timer_parks_task`, `timer_sleep_accuracy`, `channel_watchdog` |
