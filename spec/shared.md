# Shared state: `Shared<T>`, locks, atomics, `Send` and `Sync`

Milestone 2.3 (issue #213) is how tasks and threads share memory when messages are not enough. This page is the exact
behavior and the decisions behind it; the tests are `tests/s2_shared/`. Rule IDs are SH1-SH11. The surface is
`std.marker` (`Send`, `Sync`), `std.atomic` (`AtomicBool`, `AtomicI32`, `AtomicU32`, `AtomicI64`, `AtomicU64`,
`AtomicIsize`, `AtomicUsize`), `std.sync` (`Mutex<T>`, `RwLock<T>`), and `Shared<T>` from `std.alloc`.

```
let hits = Shared.new(AtomicUsize.new(0))          // a counter every task can bump
let log = Shared.new(Mutex.new(List<String>.new()))  // a list one task at a time can change

let h = hits.clone()
let l = log.clone()
let t = task.spawn(|| {
    let _ = h.get().fetch_add(1)
    l.get().lock(|entries| { entries.push("worked".to_string()) })
})
t.join()

let total = AtomicUsize.new(0)                      // or borrow it in a scope, no `Shared` needed
task.scope(|s| {
    s.spawn(|| { let _ = total.fetch_add(1) })
    s.spawn(|| { let _ = total.fetch_add(1) })
})
```

The model is the one of `spec/tasks.md` and `spec/channels.md`: a value has one owner, a task takes what it uses, and
the compiler decides what may cross to another task. This milestone adds the three things that make shared memory
possible without giving that up: handles whose count is safe to change from any task, types whose operations are
atomic or locked, and a rule that says which values may be shared at all.

## SH1. `Shared<T>` is atomic (decision, issue #214)

`Shared<T>` (`spec/allocators.md`, AL8) counts its handles. The count is now changed with atomic operations in every
program, not only in programs that use tasks:

* `clone` adds one with a relaxed atomic increment: a new handle comes from an existing one, so no ordering is needed.
* Dropping a handle subtracts one with a release atomic decrement. The handle that takes the count to zero runs an
  acquire fence before it drops the value and frees the cell, so everything every other handle did to the value
  happens before the drop.
* `strong_count()` is an acquire load. The number it returns can be out of date by the time the caller looks at it
  when other tasks hold handles; it is exact when the caller holds the only handle.

Decision: one `Shared<T>`, always atomic, rather than a non-atomic `Shared<T>` and a separate `Atomic Shared`. The cost
is one atomic read-modify-write per clone and per drop, which is small against the allocation a `Shared` already is, and
the gain is that no program has to choose a type before it knows whether a task will ever hold a handle. A handle
whose value is not `Send` and `Sync` still cannot cross to another task (SH2), so the atomic count never protects a
value that is unsafe to share. `Weak` handles remain deferred.

`get()` gives a shared view of the value (`&T`); there is no way to mutate through a `Shared`, so a changeable value in
one is a type that changes through a shared view: a lock (SH8, SH10) or an atomic (SH7).

Tests: `s2_shared/shared_count_stress`, `atomic_shared_handle`, `shared_across_tasks`.

## SH2. `Shared<T>` is `Send` and `Sync` when `T` is both

A `Shared<T>` hands out handles that several tasks hold at once, each reading the same value, and the last handle to go
away frees it. So moving a handle to another task needs more than `T: Send`:

* `Shared<T>` is `Send` when `T` is `Send` and `Sync`. Moving a handle lets another task read `T` while the first task
  still may.
* `Shared<T>` is `Sync` under the same condition.

`Shared<Mutex<T>>` is the usual shape for a changing value: `Mutex<T>` is `Send` and `Sync` when `T` is `Send` (SH8),
so the handle crosses. `Shared<Plain>` where `Plain` is `Send` but not `Sync` does not (E0280, `s2_shared/shared_not_send`).

## SH3. A shared value is freed once, by the last handle

When the count reaches zero the value is dropped and its cell freed, in whichever task held the last handle. The drop
runs exactly once, whatever the schedule. A handle moved into a task lives as long as that task's closure, and goes
away when the task finishes.

Tests: `s2_shared/shared_across_tasks`, `leak_free`, `shared_count_stress`.

## SH4. `Send` and `Sync` are derived (decision, issue #214, #218)

`Send` and `Sync` are empty interfaces in `std.marker`. They are in the prelude like `Copy`. A type does not write them;
the compiler derives them from what the type is made of:

* A value is `Send` when everything it owns is `Send`, and `Sync` when everything it holds is `Sync`. For a struct,
  enum, tuple, array, `Option`, `List`, `Map`, `Heap`, and `Result`, "everything" is every field, element, and type
  argument. Numbers, `bool`, `char`, `String`, and `()` are both.
* A raw pointer `CPtr<T>` is never `Send` and never `Sync`. This is the one primitive reason a type is not.
* `any I` (a boxed existential) is never `Send` and never `Sync`: the compiler does not know the type behind it.
  (`impl I` is the concrete type, and follows it.)
* A closure is `Send` when each value it moves is `Send` and each value it borrows is `Sync`. A closure is `Sync` when
  it is an `Fn` closure and every value it captures, moved or borrowed, is `Sync` (decision, issue #971): an `Fn`
  closure cannot change its captures, so a shared view of the closure only reads them. An `FnMut` or `FnOnce`
  closure is never `Sync`. The closure expression decides both from the captures the compiler found, so a closure
  passed straight to `task.spawn` or a parallel adapter, or kept in a variable, carries its answer. A closure type
  written as an annotation (`f: Fn(): i32`) says nothing about what the closure behind it captured, so it is neither
  `Send` nor `Sync` (`s2_shared/closure_annotation_not_send`). A function used as a value is both.
* A type parameter `T` is `Send` or `Sync` only through a bound, `fn run<T: Send>(own value: T)`; calling such a
  function with a type that is not is E0280 at the call (`s2_shared/generic_send_bound`).
* `Shared<T>` follows SH2, `Mutex<T>` and `RwLock<T>` follow SH8 and SH10, and the atomics, `Channel<T>` (when `T` is
  `Send`), and the task types are `Send` through explicit declarations (SH5) in the standard library.

Where `Send` is required: the closure of `task.spawn`, `thread.spawn`, and `Scope.spawn`, the result type of a task
(it travels back through `join`), and the element of a `Channel<T>`. A value that is not `Send` there is **E0280**
with a message that follows the chain down to the field that decides it:

```
error[E0280]: `Fn(): i32` is not `Send`, which `F` of `spawn` requires: the closure moves `source` of type `Source`,
which is not `Send`: field `cursor` of type `Cursor` is not `Send`: field `ptr` of type `CPtr<u8>` is not `Send`:
a raw pointer is never `Send`
```

An error caused by a failed requirement does not cascade: a type that was rejected as a whole is not rejected again
inside the library code that uses it.

Tests: `s2_shared/send_derived` (accepted), `spawn_non_send`, `spawn_result_non_send`, `thread_non_send`,
`channel_non_send`, `generic_send_bound`, `closure_annotation_not_send` (E0280); `closure_fn_is_sync` (accepted), `closure_fnmut_not_sync`, `closure_capture_not_sync` (E0280).

## SH5. Declaring them yourself: `unsafe` impl blocks (decision, issue #218)

A type that holds a raw pointer is neither `Send` nor `Sync`, and so is a type whose safety rests on something the
compiler cannot see (a lock around a raw cell). The programmer who knows better says so with an `unsafe` impl block:

```
struct Handle { cell: CPtr<i32> }

unsafe Handle: Send { }
unsafe Handle: Sync { }

struct Wrapper<T> { cell: CPtr<u8>, value: T }
unsafe Wrapper<T>: Send where T: Send { }     // the condition is a `where` clause
```

* Only `Send` and `Sync` are implemented this way. The block is empty, and `unsafe` is mandatory: the declaration is a
  promise, and the keyword says where to look when a data race appears.
* An `unsafe` impl replaces the derivation for that type: the compiler no longer looks at the fields. For a generic
  type, the `where` clause decides for which type arguments the promise holds; for others the type is not `Send`.
* **E0281** is an invalid marker implementation: `Send` or `Sync` without `unsafe`, `unsafe` on an interface other than
  those two, or a body that contains methods.

The standard library uses these blocks for the atomics, `Mutex<T>`, `RwLock<T>`, `Channel<T>`, and the task types.

Tests: `s2_shared/send_explicit_impl` (accepted), `marker_impl_errors` (E0281).

## SH6. What a scoped task may borrow

A task started by `Scope.spawn` may borrow locals that live outside the scope (`spec/tasks.md`, TK7). The borrow decides
what the type must be:

* A value the scoped tasks only *read* is shared among them, so it must be `Sync`.
* A value a scoped task *writes* is held by that task alone (TK7 already rejects two tasks that both write it), so it
  only needs to be `Send`.
* A value the task moves in must be `Send`, as for `task.spawn`.

`Mutex<T>` and the atomics are `Sync`, which is why scoped tasks can borrow them and take turns without a `Shared`
(`s2_shared/atomic_counter`, `mutex_scoped`). A borrow of a value that is not `Sync` is E0280 at the `spawn`
(`s2_shared/scope_borrow_not_sync`, `lock_not_send`).

## SH7. Atomics (decision, issue #217)

`AtomicBool`, `AtomicI32`, `AtomicU32`, `AtomicI64`, `AtomicU64`, `AtomicIsize`, and `AtomicUsize` hold one value that
any number of tasks can read and change without a lock. Their operations take `self` (a shared view), so
`counter.fetch_add(1)` needs no `mut`:

```
new(value)                      load()                         store(value)
swap(value): old                compare_exchange(current, new): Result<T, T>   // Ok(old) on success, Err(actual) on failure
fetch_add / fetch_sub / fetch_and / fetch_or / fetch_xor / fetch_max / fetch_min (value): old   // not on AtomicBool
fetch_and / fetch_or / fetch_xor (value): old                  // the three that AtomicBool has
```

* **Memory order: every operation is sequentially consistent.** All atomic operations of a program happen in one total
  order that every task agrees on. Weaker orders (`Relaxed`, `Acquire`, `Release`) are not offered: they are how
  concurrent programs get subtly wrong, and nothing in this language needs them yet.
* **Widths: 32 and 64 bits.** The 8- and 16-bit atomics are not offered; a `bool` is stored in a 32-bit word.
  `isize` and `usize` are 64 bits, as everywhere in Burdin.
* `fetch_add` and `fetch_sub` wrap around on overflow; an atomic operation never panics, so a counter that wraps is
  not a surprise stop.
* `compare_exchange` is strong: it fails only when the value was not `current`.
* An atomic is a small heap cell, so handles of one `Shared` and a borrow of one in a scope all reach the same value.
  The types are `Send` and `Sync` (SH5).

`std/src/atomic.bd` is generated by `tests/gen_atomic_std.py` (Burdin has no numeric interface to write one body for
every integer), and `tests/check_tests.py` fails when it is out of date. The operations are the compiler builtins
`__atomic_load`, `__atomic_store`, `__atomic_swap`, `__atomic_cas`, `__atomic_add`, `__atomic_sub`, `__atomic_and`,
`__atomic_or`, and `__atomic_xor` (each with a `_u32` and a `_u64` form, on a `CPtr` to the word) and `__atomic_fence`,
which only the standard library calls and only in `unsafe`. `fetch_max` and `fetch_min` are compare-exchange loops over
them.

Tests: `s2_shared/atomic_ops`, `atomic_counter`, `atomic_shared_handle`, `shared_count_stress`.

## SH8. `Mutex<T>` (decision, issue #216)

A `Mutex<T>` owns a `T` and lets one task at a time use it.

```
Mutex.new(value): Mutex<T>
m.lock(|v| ...): R              // waits for the value, runs the closure with `&mut T`, lets go, returns the closure's result
m.try_lock(|v| ...): Option<R>  // None if the value is taken right now; never waits
m.get_mut(): &mut T             // `mut self`: nobody else can hold the mutex, so no lock is needed
m.into_inner(): T               // takes the value out
```

* **A closure instead of a guard.** `lock` does not return a guard value to keep. A view cannot be stored (the rule that
  stops any view from outliving its owner), so a guard could not be a field or returned, and a closure gives the same
  guarantee without a new kind of value: the exclusive view exists exactly while the closure runs, and the lock is
  released when it returns. A lock cannot be forgotten and a view of the protected value cannot escape. The closure
  cannot return a view into the value either (E0171); it returns an owned result.
* The closure's bound is `FnOnce`, so the captures of the closure move into it (`spec/memory-rules.md`, CL2). To use a
  non-`Copy` local both inside and outside a lock closure, keep a second `Shared` handle for the inside.
* `Mutex<T>` is `Send` when `T` is `Send`, and `Sync` when `T` is `Send` (the mutex lets only one task at a time touch
  the value, so `T` need not be `Sync`).
* **No poisoning.** A panic aborts the process (`spec/tasks.md`, TK8), so no task can see a lock that a panicking task
  left held.
* The mutex is not re-entrant. Calling `lock` on a mutex the same task already holds waits forever; `try_lock` returns
  `Option.None` and is how a task tests. (The deadlock is not detected.)
* Dropping a `Mutex<T>` drops the value; the lock's state is freed with it.

Tests: `s2_shared/mutex_basic`, `mutex_counter`, `mutex_scoped`, `stress_mutex`, `stress_mutex_threads`, `lock_not_send`.

## SH9. Waiting parks the task

A task that has to wait for a lock does not spin and does not hold its worker: it parks, the worker runs other tasks,
and the task that releases the lock wakes the waiters. A thread that has to wait blocks. The lock's state and its
waiters are guarded by the scheduler's lock (`spec/channels.md`, CH7), so a wake-up cannot be lost, and a task may keep
a lock while it waits for something else, such as a channel message; the other tasks that want the lock wait for it.

With one worker, a waiter that kept the worker would deadlock the program, which is what
`s2_shared/mutex_waits_for_holder` checks. A woken waiter looks at the lock again and waits again if it must; no order
among waiters is promised (the schedule decides), only that every waiter eventually runs when the lock is released.

## SH10. `RwLock<T>` (issue #407)

A `RwLock<T>` lets any number of tasks read a value at once or one task write it.

```
RwLock.new(value)
rw.read(|v| ...): R              // &T; waits while a writer holds the lock or waits for it
rw.write(|v| ...): R             // &mut T; waits for the readers and any writer
rw.try_read(|v| ...): Option<R>  rw.try_write(|v| ...): Option<R>
rw.get_mut(): &mut T             rw.into_inner(): T
```

* A writer that is waiting blocks new readers, so a steady stream of readers cannot starve it. The last reader to leave
  wakes the waiters.
* `RwLock<T>` is `Send` when `T` is `Send`, and `Sync` when `T` is `Send` and `Sync`: readers use the value at the same
  time, so it must be safe to share. A `RwLock<T>` of a value that is not `Sync` can be moved, not borrowed by
  several tasks (E0280, `s2_shared/lock_not_send`).
* Like `Mutex`, the closure forms mean no guard, no poisoning, and no re-entrance: `read` inside `write` on the same
  lock waits forever, `try_read` returns `Option.None`.

Tests: `s2_shared/rwlock_basic`, `stress_rwlock`, `lock_not_send`.

## SH11. Nothing is leaked

`burdin test` runs with the checking allocator (`spec/allocators.md`, AL8), so a lock, an atomic, or a `Shared` cell that
left its state, a waiter record, or a value behind fails the test with a leak report. A `try_lock` that fails frees
nothing it did not take, a mutex that made a task wait leaves no waiter, and the last handle of a `Shared<Mutex<_>>` frees
the mutex and its value.

Tests: `s2_shared/leak_free`.

## Decisions that are not made here

* **`Weak<T>`** for `Shared<T>` stays deferred; it is needed for cycles, which no test needs yet.
* **Weaker memory orders** for atomics, 8- and 16-bit atomics, and atomic pointers are not offered (SH7).
* **Lock guards** (a value that holds the lock, as in Rust's `MutexGuard`) are not offered (SH8). Revisit them when a
  stored view is possible.
* **Condition variables, barriers, and `Once`/`OnceCell`** are not part of this milestone. A channel (`spec/channels.md`)
  is the way to wait for a message, and `Mutex` plus a channel covers the cases the tests have.
* **Deadlock detection** is not offered (SH8).

## Rule coverage

| Rule | Tests |
|---|---|
| SH1 | `s2_shared/shared_count_stress`, `atomic_shared_handle`, `shared_across_tasks` |
| SH2 | `s2_shared/shared_not_send` (E0280), `shared_across_tasks`, `send_derived` |
| SH3 | `s2_shared/shared_across_tasks`, `leak_free`, `shared_count_stress` |
| SH4 | `s2_shared/send_derived`, `spawn_non_send`, `spawn_result_non_send`, `thread_non_send`, `channel_non_send`, `generic_send_bound`, `closure_annotation_not_send` (E0280) |
| SH5 | `s2_shared/send_explicit_impl`, `marker_impl_errors` (E0281) |
| SH6 | `s2_shared/scope_borrow_not_sync` (E0280), `lock_not_send`, `atomic_counter`, `mutex_scoped` |
| SH7 | `s2_shared/atomic_ops`, `atomic_counter`, `atomic_shared_handle`, `shared_count_stress` |
| SH8 | `s2_shared/mutex_basic`, `mutex_counter`, `mutex_scoped`, `stress_mutex`, `stress_mutex_threads`, `lock_not_send` |
| SH9 | `s2_shared/mutex_waits_for_holder`, `stress_mutex` |
| SH10 | `s2_shared/rwlock_basic`, `stress_rwlock`, `lock_not_send` |
| SH11 | `s2_shared/leak_free` |
