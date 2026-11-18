# Tasks, the scheduler, scoped tasks, and threads

Milestone 2.1 (issue #198) adds tasks: closures that run concurrently with the code that started them, on a pool of
worker threads that Burdin schedules itself. This page is the exact behavior and the decisions behind it; the tests
are `tests/s2_tasks/`. Rule IDs are TK1-TK12. The surface is `std.task` (`task.spawn`, `task.yield`, `task.scope`,
`task.set_workers`, `task.JoinHandle`, `task.Scope`) and `std.thread` (`thread.spawn`, `thread.Thread`).

```
let h = task.spawn(|| compute(data))        // data is moved into the task
let answer = h.join()                       // waits; returns what the closure returned

let mut left = 0
let mut right = 0
task.scope(|s| {                            // the tasks may borrow; scope waits for them all
    s.spawn(|| { left = work(a) })
    s.spawn(|| { right = work(b) })
})

task.yield()                                // let another task run
let t = thread.spawn(|| blocking_call())    // an operating-system thread
```

## TK1. What a task is (decision, issue #199)

A task is a closure that runs on its own stack, a *stackful coroutine*. `spawn<T, F: FnOnce(): T>(own f: F):
JoinHandle<T>` takes the closure by value. Because the bound is `FnOnce`, the closure captures its variables by move
(CL2-CL4), so a task never borrows from the code that started it, and it can run after that code has returned.

Tasks are scheduled *cooperatively*: a task runs on a worker thread until it finishes, waits (`join`, the end of a
`scope`), or calls `yield`. There is no preemption in stage 2.1; a task that computes without calling any of these
keeps its worker until it is done. (Cooperative preemption points are a later item of the concurrency stage.)

Rejected alternatives:

* *Stackless state machines* (the compiler transforms a function into a state machine at each suspension point).
  They need a function coloring or a whole-program transform, and a `yield` inside a nested ordinary call would be
  impossible. Burdin's goal is that a blocking-looking call inside a task just works, so every call may suspend.
* *One OS thread per task.* It is `thread.spawn` (TK9), kept for work that blocks the thread. It does not scale to
  tens of thousands of tasks.

## TK2. The runtime primitives and the scheduler

The scheduler is Burdin code in `std/src/task.bd`. It is built on about two dozen small primitives the compiler
provides, named with the same `__` prefix as `__alloc`: `__thread_start`, `__thread_yield`, `__mutex_*`, `__cond_*`,
`__ctx_main`, `__ctx_new`, `__ctx_switch`, `__ctx_free`, `__tls_get`/`__tls_set` (one thread-local slot),
`__global_get`/`__global_set` (one process-wide slot), `__spin_lock`/`__spin_unlock`, `__config_get`/`__config_set`,
and `__cpu_count`. Each needs `unsafe`. They are LLVM IR text linked into the program only when one is used
(`crates/burdin_codegen/src/taskrt/`), so a program that never starts a task carries none of it.

* On Unix a context switch is a few instructions of assembly (x86-64 and arm64) that save the callee-saved
  registers, swap the stack pointer, and return into the other context. A new context's first frame is built by
  `ctx_new` and enters `ctx_entry`.
* On Windows a context is a fiber (`CreateFiberEx`, `SwitchToFiber`).
* The scheduler's records are `repr(C)` structs of integers and raw pointers behind `CPtr`; they have no destructors.

**Contract between the scheduler and `ctx_entry`.** The first word of the worker record that `__tls_get` returns is
the worker's own context (its `main_ctx`). When a task's body returns, `ctx_entry` switches there and never returns:
the worker then reads the action the task left (`ACTION_DONE`) and finishes the task. A task never "returns" into its
stack, so the stack can be freed from the worker's own stack.

## TK3. Stacks (decision, issue #199, #202)

Every task has a stack of **256 KiB** of usable space above a **64 KiB** guard region. The stack is one `mmap` of
address space marked `MAP_NORESERVE`; the system commits pages as the task touches them, so an idle task costs a few
kilobytes of memory. The guard region is `PROT_NONE`. The stack does not grow and is not copied.

Running off the end of a task stack touches the guard region. The signal handler that already turns a main-thread
stack overflow into `panic: stack overflow` also recognizes a fault inside the running task stack's guard (the
thread-local `burdin_rt_cur_lo` is set to the low end of the stack at every context switch), prints the same message,
and aborts. Worker threads install an alternate signal stack so the handler can run. On Windows the fiber's stack is
reserved and committed by the system, which raises the stack-overflow exception; the existing filter reports it.

A task that needs deep recursion or a large local array should run on a thread (TK9) or keep the data on the heap. The
size is fixed in this milestone; a per-task option is deferred.

## TK4. Values and ownership

* Captured variables move into the task (TK1). What the closure returns is stored in the task's result slot, in the
  same allocation as the task record, and `join(own self): T` takes it out and frees the record.
* A value that is moved into a task is dropped once, where the task drops it: the closure's captures are dropped when
  the body is done, exactly as for any `FnOnce`.
* `JoinHandle<T>` is a type with a destructor. Dropping a handle that was not joined *detaches* the task: it keeps
  running, and when it finishes its result is dropped (not stored). The record is freed by whichever of the handle and
  the scheduler lets go last, tracked by a count.
* `is_finished(self): bool` reports whether the task has finished, without waiting.
* `join` called from a task parks that task and lets its worker run others; called from any other thread (the main
  thread, a `thread.spawn` thread) it blocks the thread on a condition variable.

## TK5. The scheduler (issues #201, #301-#304)

* **Workers.** The pool starts on the first call of `task.spawn` or `task.scope`: one worker thread per processor,
  at least two and at most 64. `task.set_workers(n)` sets the number before that; it panics with `set_workers: the
  scheduler is already running` afterward and with `set_workers: at least one worker is needed` for zero. The workers
  are never stopped (TK10).
* **Queues.** Each worker has a first-in first-out queue of ready tasks with its own lock, and there is one shared
  queue. A task started by a worker goes to that worker's queue; a task started by any other thread goes to the shared
  queue; a task that yields goes to the shared queue, behind every task already waiting there, so yielding hands the
  worker to somebody else.
* **Picking.** A worker looks at its own queue, then the shared queue, then steals: it visits the other workers in
  order starting after itself and takes **one task from the head** of the first non-empty queue. Every 61st pick starts
  with the shared queue, so tasks started from outside the pool cannot starve behind a worker that always has local
  work.
* **Parking.** A worker with nothing to do registers as a sleeper under the pool's `park_lock` and then looks at every
  queue once more before it waits on the condition variable; every operation that makes a task ready queues it first
  and then signals if a sleeper is registered. A wake-up cannot be lost, and an idle pool uses no processor time.
* **Waiting.** A task that waits (join, the end of a scope) records itself where the waker will find it and parks:
  it switches back to its worker with `ACTION_PARK`, and the *worker* releases the lock the task held after the switch,
  so the waker cannot run the task on another worker while it is still on the first one's stack.
* **Finishing.** When a task ends its worker frees the stack, wakes the task waiting for it and the scope waiting for
  its last task, and drops the scheduler's share of the record.

Fairness is by queue order only: it holds for tasks that yield or wait. A task that never yields is not interrupted.

## TK6. `yield`

`task.yield()` puts the calling task at the back of the shared queue and runs another ready task; it returns when the
task is picked again, possibly on another worker. Called outside a task it yields the operating-system thread
(`sched_yield`/`SwitchToThread`). With one worker, tasks that all yield take turns in queue order
(`tests/s2_tasks/yield_interleave.bd`). The call never returns an error and nothing is promised about when the task
runs again.

## TK7. Scoped tasks (issue #204)

```
fn scope<R, F: FnMut(Scope): R>(own f: F): R
Scope { fn spawn<F: FnMut(): ()>(self, own f: F) }
```

`scope` creates a state with a count of unfinished tasks, calls `f` with a `Scope`, then waits until the count is zero
(parking if it is a task, blocking if it is not) and returns what `f` returned. `Scope.spawn` starts a task that runs
`f` and counts it. Because `scope` does not return before its tasks are done, **the closure given to `Scope.spawn` may
borrow**, which `task.spawn`'s may not.

The exclusivity checker learns three rules, all checked on one function body (the scope body is a closure, and the
checker looks at it alone):

1. **The loans last to the end of the body.** What a closure given to `Scope.spawn` borrows stays borrowed until the
   scope body ends, not just until the call returns. Writing a place a task borrows mutably, reading a place a task
   borrows mutably, or borrowing it mutably a second time is E0111, like any two overlapping borrows. Places that are
   only read may be borrowed by any number of tasks.
2. **A task may borrow only what outlives the body.** The places a task borrows must be captures of the scope body
   (variables of the code around the scope), not locals of the body: the body's locals are gone before the scope
   waits. Borrowing a local of the body is E0171.
3. **Loops.** A `Scope.spawn` inside a loop is many tasks. A mutable borrow taken there would be taken once per
   iteration, so it is E0111; reading is fine.

A closure given to `Scope.spawn` must be written at the call when it borrows: a closure kept in a variable and passed
by name, if it borrows, is E0171, because the checker follows what the closure borrows from the place the call is.
(A closure with only move captures may be passed by name.)

Disjointness is by captured variable: a closure captures a whole variable, so two tasks writing `v[0]` and `v[1]` of the
same array conflict; give each task its own variable (or its own slice of the data). Struct fields are separate
captures only when they are separate variables.

`Scope.spawn` is implemented with the unsafe primitive `__closure_own(f)`, which relabels a borrowing closure as an
owned `FnOnce` (the representation is the same). It is not part of the user surface. The checks above, not the type,
are what make it sound.

## TK8. A panic in a task (decision, issue #205)

A panic in a task aborts the process, exactly like a panic on the main thread: the message and backtrace go to standard
error, buffered standard output is flushed first, and the process ends abnormally. It is not caught, not delivered to
whoever joins the task, and there is no unwinding. The same holds for a panic in a thread (TK9).

Reasons. Burdin has no unwinding, so a panic cannot run the destructors of the frames it leaves, and a task that
stopped in the middle would leave its captured values and any state it was changing in an unknown condition. Delivering
the panic to the joiner would make `join` return a `Result` and every caller handle a case that means "the program has a
bug". A single rule (a panic aborts the process) is the rule everywhere else in the language.

## TK9. Threads (issue #405)

`thread.spawn<T, F: FnOnce(): T>(own f: F): Thread<T>` runs the closure on a new operating-system thread with the
system's default stack. `Thread<T>` has `join(own self): T` (blocks the calling thread), `is_finished(self): bool`, and
drops like `JoinHandle`: dropping an unjoined handle detaches the thread. `thread.yield_now()` gives up the time
slice. A thread is for work that blocks its thread (a long system call); a task is for everything else. A task that
joins a thread keeps its worker blocked until the thread is done.

Thread-local storage is not a language feature in this milestone; the runtime uses one thread-local slot for the
scheduler and none is exposed. A thread that calls `task.spawn` starts the pool like any other caller and its tasks go
to the shared queue.

## TK10. Shutdown

The program ends when `main` returns (or `process.exit` runs), whatever the tasks and threads are doing: the pool's
workers are not waited for, and tasks that have not finished are abandoned without running their destructors. Standard
output is flushed at exit as it always is. A program that wants a task's work to finish joins it, or uses a `scope`.

## TK11. `Send` (decision, issue #199)

Milestone 2.1 did not check that a value is safe to move to another thread; the derived `Send` marker came with
milestone 2.3 (sharing), as this rule decided. It is now `spec/shared.md` SH4-SH6: `task.spawn`, `thread.spawn`, and
`Scope.spawn` require the closure to be `Send` (each value it moves is `Send`, each value it borrows is `Sync`), and a
task's result type must be `Send`; anything else is E0280 at the call. `Shared<T>`'s count is atomic (SH1). The tasks'
own data was always safe, because a captured value is moved and a scoped borrow is exclusive or read-only (TK7).

The decision order was deliberate: `Send` is derived from field types, and the types that make it interesting
(`Shared<T>` with an atomic count, `Mutex<T>`, atomics) arrived together in 2.3.

## TK12. Runtime state shared by threads, and the checking allocator

* The output buffer is protected by a spin lock, so lines from different tasks do not interleave inside a line; the
  order of lines from different tasks is whatever the schedule produced. Panic and signal paths do not take the lock,
  because the thread that is aborting may hold it.
* In a `burdin test` build the live-allocation list of the checking allocator is protected by a spin lock. The
  scheduler's own records (the pool, the queues, the worker records) come from the C allocator and the environments of
  the worker threads are taken off the list (`burdin_rt_untrack`), because they live until the process ends and are not
  part of any test. Task records, result slots, task closures, and everything the program allocates are tracked: a test
  that leaves a joined task's data behind fails with a leak report.
* File operations, sockets, name resolution, and `blocking` extern calls park the task and run on the reactor or the
  helper pool (`spec/io.md`, IO1-IO7). Standard-input reads and `thread.spawn` joins still block the worker that made
  them.

## Tests

| Rule | Tests |
| --- | --- |
| TK1, TK4 | `s2_tasks/two_tasks`, `moved_value`, `detach_runs`, `detach_and_drop` |
| TK2, TK3 | `s2_tasks/stack_overflow_aborts`, `spawn_from_tasks` |
| TK5 | `s2_tasks/workers_one`, `workers_steal`, `idle_wakeup`, `set_workers_late`, `set_workers_zero`, `stress_many_tasks`, `stress_mixed` |
| TK6 | `s2_tasks/yield_interleave`, `fairness_yield_wait` |
| TK7 | `s2_tasks/scope_disjoint`, `scope_shared_reads`, `scope_nested_and_in_task`, `scope_conflict_write`, `scope_conflict_body`, `scope_conflict_loop`, `scope_borrow_local`, `scope_named_closure` |
| TK8 | `s2_tasks/panic_aborts` |
| TK9 | `s2_tasks/thread_spawn`, `thread_is_finished`, `detach_and_drop` |
| TK10 | `s2_tasks/exit_with_running_tasks` |
| TK12 | `s2_tasks/detach_and_drop` (leak check) |

## Known limits

* Written for Linux, macOS, and Windows on x86-64 and arm64, but exercised so far only on Linux x86-64: the macOS,
  arm64, and Windows paths (`taskrt/windows.ll`, the arm64 context-switch assembly, the Darwin constants) have not been
  run, and `tests/s2_tasks` runs on every CI platform to find out.
* Generic inference of `T = !` (a task closure whose final expression never returns) is a pre-existing compiler
  limitation, not specific to tasks; write the closure's last expression as a unit-returning call.
* `Shared<T>` captured directly by two tasks is a data race (TK11).
