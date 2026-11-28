---
title: Coming from C and C++
description: How pointers, ownership, RAII, headers, and errors in C and C++ map onto Burdin's references, Drop, and modules.
---

Burdin has no pointer arithmetic, no null pointers in ordinary code, no undefined behavior from bad indexes, and no
header files. It does have destructors, value semantics, and a direct way to call C, so most C and C++ habits still apply
at the level of design. This page maps the everyday constructs.

## The same program, both ways

C++:

```cpp
#include <vector>
#include <string>
#include <iostream>

struct Logger {
    std::string name;
    ~Logger() { std::cout << "closing " << name << "\n"; }
};

void log_all(const std::vector<int>& xs, int& count) {
    for (int x : xs) {
        count += x;
    }
}

int main() {
    Logger log{"main"};
    std::vector<int> xs{1, 2, 3};
    int count = 0;
    log_all(xs, count);
    std::cout << count << "\n";
}
```

Burdin:

```burdin
struct Logger {
    name: String
}

Logger: Drop {
    fn drop(mut self) {
        println("closing {self.name}")
    }
}

fn sum_all(xs: [i32]): i32 {
    let mut total = 0
    for x in xs {
        total = total + x
    }
    total
}

fn main() {
    let log = Logger { name: String.from("main") }
    let xs = [1, 2, 3]
    println("{log.name}: {sum_all(xs[0..3])}")
}
```

The destructor becomes `Drop`, and it runs when `log` goes out of scope, at the end of `main`. The `const std::vector<int>&`
parameter becomes a plain read-only slice, which is the default. The `int&` parameter in C++ has no direct copy here:
Burdin returns the result instead, and a parameter that must change is marked `mut` at both ends, as the
[ownership chapter](/guide/ownership/) shows.

## Vocabulary

| C / C++ | Burdin | Notes |
| --- | --- | --- |
| `int x = 5;` | `let x: i32 = 5` | Immutable by default. |
| `int x;` then `x = 5;` | `let mut x: i32 = 0` | Assign later with `mut`. |
| `const int N = 5;` | `const N: i32 = 5` | Compile-time constant. |
| `int n = 0;` at file scope | `static mut N: i32 = 0` | Written only in `unsafe`. See [global variables](/guide/globals/). |
| `int& r = x;` (a reference) | `let r = &x` (read-only) or `let m = &mut x` | Read through `*r`. |
| `void f(int& x)` | `fn f(mut x: i32)`, called as `f(mut x)` | The call site shows the mutation. |
| `void f(const T& x)` | `fn f(x: T)` | Read-only parameters are the default. |
| `void f(T x)` (by value) | `fn f(own x: T)` | Consumes the value. |
| `T* p = new T(...)` | `let p = Heap.new(value)` | A `Heap<T>` frees itself. |
| `delete p;` | nothing | The box is freed when its owner goes out of scope. |
| `std::unique_ptr<T>` | `Heap<T>` | |
| `std::shared_ptr<T>` | `Shared<T>` | Reference counted. |
| `std::vector<T>` | `List<T>` | |
| `std::string` | `String` | |
| `std::string_view` | `str` | |
| `std::map<K, V>` | `Map<K, V>` | Order is unspecified. |
| `std::unordered_set<T>` | `Set<T>` | |
| `~T()` destructor | `Drop` impl | `T: Drop { fn drop(mut self) { ... } }` |
| RAII guard | a value with `Drop` | Cleanup runs at the end of its scope. |
| `class`, `struct` | `struct`, with a block of methods | Fields and methods are public by default. |
| `private:` | `priv` on the item | Private to the package. |
| `enum class` | `enum` | Variants can carry data. |
| `std::variant<A, B>` | `enum` with variants | |
| `template <typename T>` | `fn f<T: Bound>` | Generics. |
| `auto x = ...` | `let x = ...` | Inference. |
| `#include "x.h"` | `use x` | Files are modules. |
| `namespace n { }` | a module, one file per module | |
| `throw E()` / `try` / `catch` | `Result.Err(E.Variant)` and `?` | Errors are returned. |
| `T* p = nullptr;` | `Option<T>` with `Option.None` | No null in ordinary code. |
| `std::thread` | `task.spawn`, or `thread.spawn` for blocking calls | |
| `std::mutex` | `Mutex<T>` with `lock(\|v\| ...)` | The lock is held while the closure runs. |
| `CMake`, `make` | `burdin build`, `burdin run`, `burdin test` | |

## Undefined behavior becomes a panic

A C array access past the end is undefined behavior, and a signed overflow is too. In Burdin, an index past the end is a
checked panic, and so is an integer overflow in a debug build. A release build wraps on overflow, which is defined. Either
way the program does not continue on garbage.

```burdin
fn main() {
    let xs = [1, 2, 3]
    let i: usize = 7
    println("{xs[i]}")
}
```

The program stops with `index out of bounds`, and the message says the index and the length.

## Calling C

C functions are declared in an `extern "C"` block. Calling one takes an `unsafe` block, because the compiler cannot check
what the C code does:

```burdin
extern "C" {
    fn abs(n: i32): i32
}

fn main() {
    let n = unsafe { abs(-7) }
    println("{n}")
}
```

The C bridge also has `CPtr<T>` for raw pointers, used only at this boundary. Ordinary Burdin code does not use raw pointers.
The [C bridge section of the standard library](/reference/standard-library/) lists the types.

## What is different in practice

- **No headers.** A module is a file, and a function is visible to the rest of its package without a declaration.
- **Values are copied only when asked.** Assignment moves a non-`Copy` value, so two names cannot own one buffer.
- **No pointer arithmetic, no `sizeof`-driven code.** Arrays and lists carry their length.
- **Cleanup is the type's job.** Put cleanup in `Drop`. Burdin has no `goto cleanup` pattern to copy, and no `defer`.
- **Build settings are in `burdin.toml`.** Optimization is set per profile, for example `burdin build --release`.
