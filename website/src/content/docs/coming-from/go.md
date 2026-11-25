---
title: Coming from Go
description: How Go's structs, interfaces, errors, and goroutines map onto Burdin, and where Burdin is stricter.
---

Go and Burdin share a lot of taste: small structs, methods on types, explicit errors, and concurrency with lightweight
units of work. The biggest differences are in memory and in errors. Burdin has no garbage collector, so the compiler
decides when every value is freed. It has no `nil` in ordinary code, and failures are values checked by the compiler, not
pairs that you must remember to test.

## The same program, both ways

Go:

```go
package main

import "fmt"

type Stack struct {
    items []int
}

func (s *Stack) Push(v int) {
    s.items = append(s.items, v)
}

func (s *Stack) Pop() (int, bool) {
    if len(s.items) == 0 {
        return 0, false
    }
    v := s.items[len(s.items)-1]
    s.items = s.items[:len(s.items)-1]
    return v, true
}

func main() {
    s := &Stack{}
    s.Push(1)
    s.Push(2)
    if v, ok := s.Pop(); ok {
        fmt.Println("popped", v)
    }
}
```

Burdin:

```burdin
struct Stack {
    items: List<i32>
}

Stack {
    fn new(): Stack {
        Stack { items: List<i32>.new() }
    }

    fn push(mut self, v: i32) {
        self.items.push(v)
    }

    fn pop(mut self): Option<i32> {
        self.items.pop()
    }
}

fn main() {
    let mut s = Stack.new()
    s.push(1)
    s.push(2)
    match s.pop() {
        Option.Some(v) => println("popped {v}"),
        Option.None => println("empty"),
    }
}
```

A pointer receiver `(s *Stack)` becomes `mut self`, and the call site stays the same: `s.Push(1)` is `s.push(1)`. The
value is still a value; Burdin does not need a `&` or a pointer to mutate it.

## Vocabulary

| Go | Burdin | Notes |
| --- | --- | --- |
| `:=` | `let` | Declarations need `let`. |
| `var x int` | `let mut x: i32` | |
| `int`, `int64`, `uint8` | `i32`, `i64`, `u8` | Integer sizes are explicit. `int` is not a type. |
| `string` | `String` (owned) or `str` (view) | |
| `[]int` | `List<i32>` (growable), `[i32]` (view) | |
| `[3]int` | `[i32; 3]` | |
| `map[string]int` | `Map<String, i32>` | |
| `struct` | `struct` | Same idea. Fields are public by default. |
| `func (p Point) M()` | `Point { fn m(self) }` | Methods go in a block that names the type. |
| `func (p *Point) M()` | `Point { fn m(mut self) }` | A pointer receiver becomes `mut self`. |
| `type I interface { ... }` | `interface I { ... }` | |
| implicit interface satisfaction | `Point: I { ... }` | The type states it implements the interface. |
| `nil` | `Option.None` (`Option<T>`) | There is no nil in ordinary code. |
| `(T, error)` | `Result<T, E>` | `?` passes the error up. |
| `errors.New("x")` | an `error enum` variant with a message | |
| `if err != nil { return err }` | `value = expr?` | |
| `defer f()` | no `defer` | Use a `Drop` impl, or end the scope. |
| `go f()` | `task.spawn(\|\| f())` | Tasks run on a worker pool. |
| `chan int` | `Channel<i32>.unbounded()` | `send`, `recv`, `clone()` for more senders. |
| `sync.Mutex` | `Mutex<T>` | `lock` takes a closure. |
| `sync.WaitGroup` | `JoinHandle`s, `join()` | Keep the handles and join them. |
| `fmt.Println` | `println` | |
| `fmt.Sprintf("%d", n)` | `"{n}"` | Interpolation builds a `String` directly. Use `format(...)` for the same thing with positional arguments. |
| `switch` | `match` | Must cover every case. |
| `for i, v := range xs` | `for (i, v) in xs.iter().enumerate()` | |
| `for i := 0; i < n; i++` | `for i in 0..n` | No `++`. |
| `x++`, `x += 1` | `x = x + 1` | No compound assignment. |
| `len(xs)` | `xs.len()` | |
| `os.Args` | `env.args()` | |
| `os.Exit(1)` | `process.exit(1)` | |

## Errors

Go returns errors as a second value and expects you to check them. Burdin returns a `Result`, and the compiler will not let
you ignore it. A function that can fail says so in its return type, and the caller decides how to handle it:

```burdin
error enum ConfigError {
    Missing(String) "missing key {0}"
    Parse(ParseIntError) from "not a number: {0}"
}

fn port(text: str): Result<i32, ConfigError> {
    let n = i32.parse(text)?
    if n <= 0 {
        return Result.Err(ConfigError.Missing(String.from("port")))
    }
    Result.Ok(n)
}

fn main() {
    match port("80") {
        Result.Ok(p) => println("port {p}"),
        Result.Err(e) => println("error: {e}"),
    }
}
```

The `from` in the error enum does the wrapping that Go programmers write by hand as `fmt.Errorf("...: %w", err)`. The
[errors chapter](/guide/errors/) explains the rest.

## Interfaces

In Go, a type satisfies an interface just by having its methods. In Burdin, the type says so in a block:

```burdin
interface Speaker {
    fn speak(self): String
}

struct Dog {
    name: String
}

Dog: Speaker {
    fn speak(self): String {
        "{self.name} says woof"
    }
}

fn announce<T: Speaker>(s: T) {
    println(s.speak())
}

fn main() {
    announce(Dog { name: String.from("rex") })
}
```

The explicit form costs one line per pair, and it makes "what can this type do" answerable by reading its file. A
`List` of different types that share an interface uses `any Speaker`, which is covered in
[generics and interfaces](/guide/generics-and-interfaces/).

## Memory without a collector

Go allocates on the heap whenever a value escapes, and the collector frees it later. Burdin decides at compile time:

- A value lives in the scope that declares it, and it is freed when that scope ends.
- Heap memory is explicit: `Heap<T>`, and the collections (`List`, `Map`) allocate for you.
- A reference to a local cannot leave the function. Returning `&local` is a compile error, where Go would move the local to
  the heap silently.
- Cleanup belongs to the type: a `Drop` impl runs when the value goes away. There is no `defer`, and a scope block does the
  same job for one resource.

## Goroutines and channels

A goroutine is a task. The closure runs on a pool of worker threads, and `join()` waits for its result:

```burdin
fn main() {
    let results = Channel<i32>.unbounded()
    let out = results.clone()
    let h = task.spawn(|| {
        let _sent = out.send(6 * 7)
    })
    println("answer {results.recv().unwrap()}")
    h.join()
}
```

Two differences from Go: a task's closure takes its variables by move, so the caller cannot touch them afterwards, and a
shared value needs a `Mutex<T>` or `Shared<T>` made explicit in the code.
