---
title: Concurrency
description: Tasks, scoped tasks, channels, and shared state with Mutex, using Burdin's own worker pool.
---

Burdin runs work concurrently with *tasks*. A task is a closure that runs on a pool of worker threads that the runtime
manages. Tasks communicate through `Channel<T>` values, or share state through a `Mutex<T>`. The compiler checks that
data is only shared in ways that are safe, so a data race is not something a program can express by accident.

## Spawning tasks

`task.spawn` starts a closure in the background and returns a `JoinHandle`. `join()` waits for the task and returns what
the closure returned:

```burdin
fn square_all(own values: List<i32>): i32 {
    let handle = task.spawn(|| {
        let mut total = 0
        for v in values {
            total = total + v * v
        }
        total
    })
    handle.join()
}

fn main() {
    let mut values = List<i32>.new()
    for v in [1, 2, 3, 4] {
        values.push(v)
    }
    println("{square_all(values)}")
}
```

The closure takes its variables by move, because a task can outlive the code that started it. That is why `values` is an
`own` parameter: a borrowed parameter cannot be moved into a task. To let a task use a value that the caller keeps, share it
with `Shared<T>` instead.

Tasks are scheduled cooperatively. A task runs until it finishes, waits (for `join` or a channel), or calls
`task.yield()`. Work that does not wait should be split into pieces of reasonable size.

## Scoped tasks

When tasks only need data for a short time, `task.scope` lets them borrow locals. The scope waits for every task it started
before it returns, so the borrows cannot outlive the data:

```burdin
fn main() {
    let mut left = 0
    let mut right = 0
    task.scope(|s| {
        s.spawn(|| {
            left = 10
        })
        s.spawn(|| {
            right = 32
        })
    })
    println("{left + right}")
}
```

## Channels

A `Channel<T>` carries values from one task to another. `send` queues a value, and `recv` takes the next one, waiting if
there is none yet. A channel can be cloned, and every clone sends to the same queue. Each task gets its own clone:

```burdin
fn sum_range(start: i32, end: i32): i32 {
    let mut total = 0
    for n in start..end {
        total = total + n
    }
    total
}

fn main() {
    let results = Channel<i32>.unbounded()

    let mut handles = List<JoinHandle<()>>.new()
    for part in 0..4 {
        let out = results.clone()
        handles.push(task.spawn(|| {
            let partial = sum_range(part * 250, (part + 1) * 250)
            let _queued = out.send(partial)
        }))
    }

    let mut total = 0
    for _ in 0..4 {
        total = total + results.recv().unwrap()
    }
    for handle in handles.into_iter() {
        handle.join()
    }
    println("sum of 0..1000 = {total}")
}
```

`Channel<T>.unbounded()` never blocks a sender. `Channel<T>.bounded(n)` holds at most `n` values, and `try_send` reports when it
is full, so the sender can decide what to do. A channel that is closed returns `None` from `recv` once it is empty.

## Shared state with Mutex

When several tasks must change one value, put it in a `Mutex<T>`. `lock` takes a closure that receives the value by
mutable reference. The lock is held only while the closure runs, and it cannot be forgotten, because there is no guard to
keep. To share the mutex between tasks, wrap it in a `Shared`:

```burdin
fn main() {
    let counter = Shared.new(Mutex.new(0))
    let mut handles = List<JoinHandle<()>>.new()
    for i in 0..3 {
        let shared = counter.clone()
        handles.push(task.spawn(|| {
            shared.get().lock(|v: &mut i32|: () { *v = *v + i + 1 })
        }))
    }
    for h in handles.into_iter() {
        h.join()
    }
    let total = counter.get().lock(|v: &mut i32|: i32 { *v })
    println("total {total}")
}
```

Each task adds its own amount under the lock, so the final total is the same on every run.

## Threads for blocking work

A task must not block its worker thread for long, because the other tasks on that worker wait. For a call that blocks,
such as a long read from a device, start an operating-system thread with `thread.spawn`. It has the same closure rules as a
task, and it returns a handle with `join()`.

## What the compiler checks

A value can cross from one task to another only when its type is safe to send. A reference to a local variable, for
example, cannot be moved into a task, and the compiler reports that before the program runs. `Shared<T>` and `Mutex<T>` are
the safe ways to share a value between tasks, and a `Channel<T>` carries any value that can be sent.
