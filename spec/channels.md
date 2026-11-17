# Channels and `select`

Milestone 2.2 (issue #207) adds channels, the way tasks and threads talk to each other without sharing memory. This
page is the exact behavior and the decisions behind it; the tests are `tests/s2_channels/`. Rule IDs are CH1-CH10. The
surface is `std.channel` (`Channel<T>`, `TrySend<T>`, `TryRecv<T>`) and the `select` expression, which the parser
rewrites into calls of `Channel`'s members.

```
let jobs = Channel<Job>.bounded(8)          // at most 8 queued values; send waits while it is full
let results = Channel<i32>.unbounded()

let rx = jobs.clone()                       // another handle to the same channel
let out = results.clone()
let worker = task.spawn(|| {
    for job in rx {                         // receives until the channel is closed and empty
        let _ = out.send(job.run())         // moves the value in
    }
})
jobs.send(Job.new(1))                       // moves the job into the channel
jobs.close()                                // the worker's loop ends after the queued jobs

select {
    v = results.recv() => println("result {:?}", v)         // v: Option<i32>
    jobs.send(Job.new(2)) => println("queued another job")
    default => println("nothing ready")
}
```

## CH1. What a channel is (decision, issue #208)

`Channel<T>` is a typed queue of values of one owned type `T`, shared by tasks and threads. It is a **handle**: a small
value that points at the queue. `clone` makes another handle to the same queue; the queue lives until the last handle is
dropped. Any handle can send, receive, and close. A channel is *multi-producer, multi-consumer*: many handles can send
and many can receive at the same time, and each value goes to exactly one receiver.

Values are received in the order they were sent (first in, first out). With several senders, the order between two
senders is whatever the schedule produced, and the order of one sender's values is kept.

Everything that can wait works from every kind of caller. In a task, a `send` or `recv` that has to wait *parks* the task
and its worker runs other tasks (the wake-up is the task's scheduler wake-up, `tasks.md` TK5). On any other thread, the
main thread or a `thread.spawn` thread, it blocks the thread. A channel can connect a task, a thread, and the main
thread to each other.

Rejected alternatives:

* *Separate `Sender<T>` and `Receiver<T>` types* (Rust's `mpsc`). They make "the consumer's loop ends when every sender is
  dropped" possible, but every channel then needs two types, two names, and a pair-returning constructor, and the design
  asks for one `Channel<T>`. An explicit `close` (CH3) is what ends a loop here, as in Go.
* *Single-consumer.* It would make `Channel<T>` movable only, with no `clone`, and a worker pool would need an extra
  queue in front of it.

## CH2. Bounded and unbounded (decision, issue #208)

* `Channel<T>.bounded(capacity)` holds at most `capacity` values. `send` waits while it is full. The capacity must be at
  least 1: `bounded(0)` panics with `Channel.bounded: the capacity must be at least 1`. There are no rendezvous
  (capacity 0) channels in this milestone; a channel of capacity 1 is the nearest thing, and `select` makes a send arm
  ready as soon as there is room.
* `Channel<T>.unbounded()` has no limit: `send` never waits and the queue grows with the memory it uses.
* `len(self): usize` is the number of queued values and `capacity(self): Option<usize>` is the limit (`None` for an
  unbounded channel). Both are a snapshot: another task may change the channel at once.

## CH3. Closing (decision, issue #208)

`close(self)` marks the channel closed for every handle and wakes every task and thread waiting on it. Closing twice does
nothing.

* The values already queued stay in the channel. `recv` keeps returning them, in order, and returns `None` only when the
  channel is closed *and* empty.
* `send` on a closed channel does not queue anything. It returns `Some(value)`: the value it was given, so nothing is
  lost. (`send` returns `None` when the value was queued.) A sender that was waiting for room when the channel closed
  gets its value back the same way.
* `is_closed(self): bool` reports whether `close` has been called.

A channel is also freed (CH4) when its last handle is dropped, but dropping handles does not close it for the others: a
receiver's own handle keeps the channel open, so a loop `for v in rx` ends because somebody called `close`, not because the
senders went away.

Why `send` hands the value back instead of panicking (Go) or failing without the value (`Result<()>`): a close races with
senders by design, so a send on a closed channel is an ordinary outcome, not a bug; and a value that is moved in and then
refused would otherwise be lost silently.

## CH4. Ownership, drops, and no sharing (decision, issues #208 and #211)

* `send(self, own value: T)` takes its value by move. The sender cannot use it afterwards (E0100, the ordinary
  use-after-move), and the channel owns it until a receiver takes it, which moves it out again: the value is dropped
  exactly once, by whoever holds it last (`tests/s2_channels/queued_values_dropped`, `send_moves_value`).
* A handle moved into a task, as `task.spawn`'s closure captures it (`tasks.md` TK1), is gone from the code that started
  the task; the code keeps its own handle through `clone` (`tests/s2_channels/use_after_send`).
* **Values must own their storage.** `T` may not be a reference or a view: `Channel<&i32>` and `Channel<str>` are E0120,
  and `ch.send(&x)` on a `Channel<i32>` is a type error (E0003), because a borrow cannot outlive the code that lent it,
  and a value in a channel can outlive the sender (`tests/s2_channels/send_borrowed`). Send a `String`, a `List`, or a
  struct, not a view of one.
* When the last handle is dropped, the values still queued are dropped with it, in the order they were sent, outside the
  channel's lock, so a value's `Drop` may use channels itself. Under `burdin test` the checking allocator reports a node, a
  waiter, or a value that is left over (`tests/s2_channels/leak_free`).
* A channel handle may be borrowed by scoped tasks. `send`, `recv`, `close`, and the rest take `self`, a read-only borrow,
  so any number of tasks in a `scope` can use the one handle, which is allowed by TK7 (reads are shared), and every
  operation synchronizes inside the channel (`tests/s2_channels/scoped_tasks`).
* **`Send` marker.** The derived `Send` marker waits for milestone 2.3, as `tasks.md` TK11 decided. When it exists, `T:
  Send` becomes a requirement of `Channel<T>.send`, and a non-`Send` type across a channel is rejected then. Until then
  the program must not send a value that holds a raw `CPtr` or a `Shared<T>` it also uses elsewhere: those are the types
  whose sharing is unsafe in 2.1 and 2.2.

## CH5. The operations

```
fn Channel<T>.bounded(capacity: usize): Channel<T>
fn Channel<T>.unbounded(): Channel<T>
Channel<T> {
    fn send(self, own value: T): Option<T>      // None: queued. Some(value): the channel is closed
    fn recv(self): Option<T>                    // None: closed and empty
    fn try_send(self, own value: T): TrySend<T> // never waits
    fn try_recv(self): TryRecv<T>               // never waits
    fn close(self)
    fn is_closed(self): bool
    fn len(self): usize
    fn capacity(self): Option<usize>
    fn clone(self): Channel<T>                  // another handle; cheap
    fn iter(self): ChannelIter<T>               // Iterable: `for v in ch`
}
enum TrySend<T> { Sent, Full(T), Closed(T) }
enum TryRecv<T> { Value(T), Empty, Closed }
```

`send` waits while a bounded channel is full and the channel is open. `recv` waits while the channel is empty and open.
`try_send` and `try_recv` never wait: `try_send` returns the value in `Full` or `Closed`, and `try_recv` tells an open,
empty channel (`Empty`) from a closed one (`Closed`).

`for v in ch` (and `for v in ch.iter()`) receives until the channel is closed and empty. The iterator holds its own
handle, so the loop does not borrow `ch`, and it removes values from the channel like `recv` does.

`clone` is a method of `Channel<T>`, not an implementation of the `Clone` interface: writing a built-in interface by
hand is not implemented yet (issue #402). Because `send` returns the value or `None`, a call used as the last expression
of a block is an `Option<T>`; write `let _ = ch.send(x)` where the block should be `()`.

## CH6. `select` (decision, issue #208; implemented in issue #210)

```
select {
    v = rx.recv() => body          // receive: v is the Option<T> that rx.recv() returns
    tx.send(value) => body         // send; "r = tx.send(value) => body" binds the Option<T> that send returns
    default => body                // optional
}
```

`select` waits on several channel operations and runs the arm of **exactly one**: an operation that could finish at once.
It is an expression: its value is the value of the arm that ran, so all the arms have one type, and a `select` with no
`default` whose arms are blocks of type `()` is a statement.

**Syntax.** `select` is a contextual keyword: the word `select` followed by `{` on the same line starts the expression
(nowhere else is it special, so `select` stays a valid name). Each arm is

```
[ binding "=" ] channel "." "recv" "(" ")" [ "if" guard ] "=>" body
channel "." "send" "(" value ")"           [ "if" guard ] "=>" body       (also with a binding)
"default" "=>" body
```

The binding is a name, `mut name`, or `_`. The arm separators are those of `match`: a comma or a line break, and none after
a block body. A `select` needs at least one arm and may have one `default`; anything else is E0001
(`tests/s2_channels/select_syntax_errors`).

**Rewriting.** The parser rewrites a `select` into ordinary code, so the type checker, the move checker, and code
generation have no `select` of their own, and a type error in an arm is the usual one. The rewriting is documented at the
top of `crates/burdin_syntax/src/parser/select.rs`: every channel expression is evaluated once, in source order, into a
cloned handle; a `send` arm's value moves into a slot; a loop tries the arms and, with no arm ready and no `default`,
waits; then a `match` on the arm index runs the chosen arm's body. The bodies are *outside* the loop, so `break`,
`continue`, `return`, and `?` in an arm mean what they mean anywhere else in the function (`select_break_continue`). The
members the rewriting calls (`__empty`, `__wrap`, `__take`, `__try_recv_slot`, `__try_send_slot`, and the `__Select`
type with `__select_new`) are public only because the rewritten code lives in the user's module; a program does not call
them.

**What the arm's binding is.** The same thing the method would have returned: for a `recv` arm an `Option<T>` (`None` when
the channel is closed and empty), for a `send` arm an `Option<T>` (`None` when the value was queued, `Some(value)` when
the channel was closed). The arm may leave the binding out or use `_`.

**When an arm can run.** A `recv` arm can run when the channel has a value or is closed. A `send` arm can run when the
channel has room or is closed. So a closed channel's arms can always run (the receive returns `None`, the send hands the
value back); CH8 shows how a loop turns such an arm off.

## CH7. Which arm runs, and what happens while none can (decision, issue #208)

* **Priority is source order.** When more than one arm can run, the *first* in the source runs. The rule is deterministic
  on purpose: a test, or a program, can know which arm it gets. Writing the arm that must not starve first is the way to
  give it priority; and an arm that is always ready (a busy channel, or a closed one) starves the arms after it, which is
  what guards (CH8) are for. (`tests/s2_channels/select_priority`.)
* **No arm can run and there is no `default`:** the caller waits, parked if it is a task, blocked if it is a thread. It
  registers with every channel of its arms *under the lock that guards all channel state*, so a change to any of them
  after the arms were looked at wakes it and it looks again. A wake-up says "look again", not "this arm is ready": the
  arms are tried again in source order, and the arm that can run then is the one that runs. (`select_waits`, and the
  stress tests, CH10.)
* **Only one arm's operation happens.** An operation is performed when its arm is chosen, not before: a value is taken
  from a channel only by the arm that runs, and a value is queued only by the `send` arm that runs.

## CH8. `default`, guards, and the value of a `send` arm (decision, issue #208)

* **`default`** makes the `select` non-blocking: if no other arm can run, the `default` body runs at once, without waiting.
  It is never chosen when another arm can run. At most one `default` is allowed. A `default` that is not last still has
  the lowest priority. (`select_default`.) There is no timeout arm in this milestone; a loop with `default` and
  `task.yield()` polls.
* **Guards.** An arm may end its head with `if condition`. The condition is a `bool`, evaluated once, when the `select`
  starts, in source order with the other channel expressions. An arm whose condition is false is *switched off*: it
  cannot run and is not waited on. This is how a loop stops selecting on a channel that is closed (CH7: such an arm would
  run forever) without a special "nil channel" (`select_priority`, `select_fan_in`, `select_stress`). If every arm is
  switched off and there is no `default`, the `select` could wait forever, so it panics with a message that says every arm is switched off by its guard and there is no `default` (`select_all_off_panics`).
* **The value of a `send` arm moves when the `select` starts**, because the value is part of the arm's head, which is
  evaluated once up front like the channel expression. If the arm does not run, the value is dropped when the `select`
  ends; it is not given back to the variable it came from. The move is visible to the move checker, so using the value after
  the `select` is E0100 (`select_value_moved`), and an unchosen value is dropped exactly once (`select_unchosen_dropped`).
  To keep a value for the case where its arm does not run, send a clone, or test with `try_send` first.
* The *channel* expressions are evaluated once too, whatever they are (`a`, `self.jobs`, `make_channel()`), by cloning the
  handle, so the handle count goes up by one for the length of the `select`.

## CH9. Typing

The arms of a `select` give one type, as the arms of a `match` do (E0003); the value of a `send` arm has the channel's
element type (E0003); a guard is a `bool` (E0003). A `recv` or `send` arm whose receiver is not a `Channel` is E0002 (no
such method). `select` introduces no new error code (`select_type_errors`, `send_wrong_type`).

## CH10. Implementation and what is guaranteed

* **One lock.** The state of every channel, every waiter list, and a change counter are guarded by the scheduler's one lock
  (`state_lock`, `tasks.md` TK5), the lock that also guards `join` and `scope`. That keeps `select` simple: looking at a
  channel, queueing a value, taking one, and registering to be woken are each a step under the lock, so there is no lock
  order to get wrong. The price is that channel operations from different tasks serialize on that lock; splitting it per
  channel is a later optimization that would not change the behavior described here.
* **No lost wake-ups.** A task or thread that has to wait registers as a waiter in the channel's list *before* it gives up
  the lock; the worker that took over from a parking task releases the lock only after the task has switched away, so a
  waker (which needs the lock) cannot ready a task that is still on its worker's stack. A `select` reads the change counter
  before it looks at its arms and passes it to `wait`, which returns at once if the counter moved, so a change between the
  look and the wait is never missed. Every change (a value queued, a value taken, a close) bumps the counter and wakes the
  waiters of the channel that changed: the receivers when a value is queued, the senders when room appears, both when the
  channel closes. Waking everyone who might use the change is simple and correct; a woken waiter checks again and waits
  again if the change was not for it.
* **Each value is delivered once**, to one receiver, and is dropped once (CH4).
* **The test runner's guarantees:** `tests/s2_channels/ping_pong`, `stress_ping_pong_tasks`, `stress_mpmc`,
  `stress_close_race`, and `select_stress` would hang (and fail by timeout) on a lost wake-up and would fail their checks on
  a lost or doubled value. CI runs them in debug and release on every platform, and the dedicated step repeats them
  (`tests/run_tests.py --repeat`).
* **No deadlock detection.** If every task and thread is waiting for a channel nobody will use, the program hangs, as a
  program waiting on `join` forever does.

## Tests

| Rule | Tests |
| --- | --- |
| CH1 | `s2_channels/producer_consumer`, `many_producers`, `many_consumers`, `pipeline`, `thread_and_task` |
| CH2 | `s2_channels/fifo_unbounded`, `bounded_capacity`, `bounded_zero_panics`, `ping_pong` |
| CH3 | `s2_channels/close_semantics`, `close_wakes_receiver`, `close_wakes_sender`, `stress_close_race` |
| CH4 | `s2_channels/queued_values_dropped`, `send_moves_value`, `scoped_tasks`, `leak_free`, `send_borrowed` (E0003, E0120), `use_after_send` (E0100) |
| CH5 | `s2_channels/try_recv`, `bounded_capacity`, `fifo_unbounded`, `producer_consumer` |
| CH6 | `s2_channels/select_binding_forms`, `select_break_continue`, `select_fan_in`, `select_syntax_errors` (E0001) |
| CH7 | `s2_channels/select_priority`, `select_waits`, `select_stress` |
| CH8 | `s2_channels/select_default`, `select_send`, `select_priority`, `select_fan_in`, `select_all_off_panics`, `select_value_moved` (E0100), `select_unchosen_dropped` |
| CH9 | `s2_channels/select_type_errors` (E0003), `send_wrong_type` (E0003) |
| CH10 | `s2_channels/ping_pong`, `stress_ping_pong_tasks`, `stress_mpmc`, `stress_close_race`, `select_stress`, `leak_free` |

## Known limits

* Channel operations serialize on the scheduler's one lock (CH10). A program with one very hot channel per task does not
  scale across workers yet.
* `select` has no timeout arm, no rendezvous channels, and no "nil channel"; a guard switches an arm off. The deadline
  arm belongs with timers, which the I/O milestone (2.4) brings.
* The `Send` marker, and with it the rejection of a non-`Send` value across a channel, is milestone 2.3 (CH4, `tasks.md`
  TK11).
* A `select` that uses a channel expression with a side effect evaluates it once, but clones its handle, so the handle count
  of the channel is one higher while the `select` runs.
* Like the rest of stage 2, exercised so far only on Linux x86-64 (`tasks.md`, Known limits).
