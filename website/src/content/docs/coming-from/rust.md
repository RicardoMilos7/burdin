---
title: Coming from Rust
description: How Rust ideas map onto Burdin, what is simpler, and what is different on purpose.
---

Burdin is closest to Rust in spirit: memory safety at compile time, no garbage collector, and values with clear owners.
Most of your Rust intuition carries over. The differences are mostly simplifications. Burdin does not ask you to annotate
lifetimes, and it does not have traits with the same breadth of Rust's trait system.

## The same program, both ways

Rust:

```rust
use std::collections::HashMap;

fn count(words: &[&str]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for w in words {
        *counts.entry(w.to_string()).or_insert(0) += 1;
    }
    counts
}

fn main() {
    let counts = count(&["a", "b", "a"]);
    println!("a = {}", counts["a"]);
}
```

Burdin, with a `Map` and a `bump` helper that takes the key by value:

```burdin
fn bump(mut counts: Map<String, i32>, own word: String) {
    let current = match counts.get(word.as_str()) {
        Option.Some(n) => *n,
        Option.None => 0,
    }
    counts.insert(word, current + 1)
}

fn main() {
    let mut counts = Map<String, i32>.new()
    for w in ["a", "b", "a"] {
        bump(mut counts, String.from(w))
    }
    match counts.get("a") {
        Option.Some(n) => println("a = {n}"),
        Option.None => println("a is missing"),
    }
}
```

The shape is familiar. The differences are the ones to learn: there is no `&` in the signature for a read-only
argument, the mutable one is marked at the call site (`bump(mut counts, ...)`), and `+=` does not exist, so the update is
written out.

## Vocabulary

| Rust | Burdin | Notes |
| --- | --- | --- |
| `println!("{}", x)` | `println("{x}")` | No macro. The braces hold the expression. |
| `let mut x` | `let mut x` | Same. |
| `&x` in an argument | `x` | A parameter is a read-only borrow by default. |
| `&mut x` in an argument | `mut x` at the call site, `mut x: T` in the signature | The call site shows the mutation. |
| `self` / `&self` | `self` | Read-only. |
| `&mut self` | `mut self` | Can change the value. |
| `self` (by value) | `own self` | Consumes the value. |
| `Vec<T>` | `List<T>` | `List<T>.new()`, `push`, `pop`, `len`. |
| `&[T]` | `[T]` | A slice view. |
| `&str` | `str` | A text view. |
| `String` | `String` | Same. |
| `HashMap<K, V>` | `Map<K, V>` | Iteration order is unspecified. |
| `HashSet<T>` | `Set<T>` | |
| `VecDeque<T>` | `Deque<T>` | |
| `BinaryHeap<T>` | `PriorityHeap<T>` | |
| `Box<T>` | `Heap<T>` | `Heap.new(x)` and `*b`. |
| `Rc<T>` / `Arc<T>` | `Shared<T>` | `Shared.new`, `clone()`, `get()`. |
| `Mutex<T>` | `Mutex<T>` | `lock` takes a closure instead of returning a guard. |
| `thread::spawn` | `task.spawn` (or `thread.spawn`) | Tasks run on a worker pool; `thread.spawn` for blocking calls. |
| `mpsc::channel()` | `Channel<T>.unbounded()` | `send`, `recv`, `clone()` for more senders. |
| `Option<T>`, `Result<T, E>` | same | `?`, `unwrap`, `expect`, and `match` all work as before. |
| `Box<dyn Error>`, `anyhow::Result` | `Result<T>` | The default error type carries any error. |
| `thiserror` derive | `error enum` | Variants carry the message: `Missing(String) "missing {0}"`. |
| `#[derive(...)]` | `struct P: Debug, Copy, ...` | The derive names go after a colon. |
| `impl Type { }` | `Type { }` | A block of methods, no `impl` keyword. |
| `trait Name` | `interface Name` | |
| `impl Trait for Type` | `Type: Trait { }` | The block names the interface. |
| `dyn Trait` | `any Trait` | A boxed value of any type that implements it. |
| `T: Trait` / `where` | same | |
| `async fn` / `.await` | none | Use tasks and channels instead. |
| `#[test]` and `assert_eq!` | `test "name" { assert_eq(a, b)? }` | |
| `cargo new`, `cargo run`, `cargo test` | `burdin new`, `burdin run`, `burdin test` | |
| `Cargo.toml` `[dependencies]` | `burdin.toml` `[dependencies]` | Path and official packages. |
| `mod`, `pub`, `pub(crate)` | files are modules, public by default, `priv` | See [packages and modules](/guide/packages-and-modules/). |

## Lifetimes

Burdin has no lifetime annotations. Where Rust would write `fn trimmed<'a>(s: &'a str) -> &'a str`, Burdin writes the same
function with a view parameter and a view result, and the checker reads one function at a time:

```burdin
fn trimmed(text: str): str {
    text.trim()
}

fn main() {
    println("[{trimmed("  hello  ")}]")
}
```

The rule that replaces lifetimes is simple: a function may return a view only of **one** parameter, and never of a local.
If a result could come from two parameters, the compiler refuses and asks you to return an owned `String` instead. This is
less flexible than Rust's lifetimes, and it keeps every function checkable from its own signature.

## Ownership, side by side

Moves work as in Rust. Assigning a non-`Copy` value moves it. Two differences matter in practice:

- A **parameter** is read-only by default, so passing a value does not move it. Rust passes a value by moving it; Burdin
  passes it by borrowing it. To move it in, write `own`.
- A **method** receiver works the same way. `self` is a read-only borrow, `own self` consumes.

This means that a function taking a `Counter` by name leaves the caller's `Counter` alone, and the caller can keep using it.
The [ownership chapter](/guide/ownership/) has the full rules.

## Things that are simpler

- **No macros for printing or formatting.** Interpolation is part of the string literal and is checked by the compiler.
- **No attributes for derives.** `struct P: Copy, Clone, Eq` is the whole syntax.
- **No `unsafe` in ordinary code.** The C bridge uses `unsafe` for calls into C, and nothing else needs it.
- **No `async`.** Tasks and channels cover the same ground for the programs Burdin is aimed at.
- **Closures take annotated parameters.** `|x: i32| x + 1`. Burdin asks for the type; Rust infers it.

## Things that are missing

- **No crates.io.** Dependencies come from paths, from git (in the package manager), and from the official packages.
- **Implementations are explicit.** A type has an interface's methods but does not implement the interface until its block
  says `Type: Interface`. Go's implicit style does not apply here either.
- **No `+=`, `++`, or `--`.** Write `x = x + 1`.
- **`as` never fails.** For a conversion that can fail, use `T.try_from(value)`, which returns a `Result` (see the
  [standard library](/reference/standard-library/)).
