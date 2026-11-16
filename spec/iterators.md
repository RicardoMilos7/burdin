# The iteration protocol (IT4-IT12)

Stage 0 `for` loops are built into the compiler: arrays, slices, `List`, `Map`, `Chars`, and ranges have their own
lowering (`memory-rules.md`, IT1-IT3). Stage 1 adds a protocol so that user types join `for`, collections can be
walked by hand, and a chain of adapters composes without allocating.

This page is the rule for the protocol. `spec/memory-rules.md` IT1-IT3 keep the stage 0 loop semantics, which the
protocol preserves. The declarations are `std/src/iter.bd`; `spec/stdlib.md` lists the surface.

**IT4 Types.** The standard library declares two generic interfaces:

```
interface Iterator<T> {
    fn next(mut self): Option<T>
    ...
}

interface Iterable<T> { }
```

`Iterator<T>` is the stateful object a chain is made of: `next` borrows the iterator exclusively, advances it, and
returns the next item, or `None` once it is exhausted. `Iterable<T>` is a marker: a type implements it to say that
`for` accepts it, and the compiler checks that the type provides `iter(self): I` where `I: Iterator<T>`. A type may
implement one or both. Any later call to `next` after it returned `None` is unspecified but must not misbehave: the
built-in iterators keep returning `None`.

**IT5 Built-in implementations.** The compiler knows the protocol for its own types; they need no impl block and no
method of their own:

| Type | `iter()` gives | `next()` yields | `into_iter()` gives |
| --- | --- | --- | --- |
| `[T; N]` | `[T]` (a view of the array) | `T` if `Copy`, else `&T` | `ArrayIntoIter<T, N>` |
| `[T]` | itself | `T` if `Copy`, else `&T` | `[T]` |
| `List<T>` | `[T]` (a view of the list) | `T` if `Copy`, else `&T` | `ListIntoIter<T>` |
| `Map<K, V>` | `MapIter<K, V>` | `(&K, &V)` | `MapIntoIter<K, V>` (moves entries) |
| `Chars` | itself | `char` | `Chars` |
| `Range<T>` | `RangeIter<T>` | `T` | `RangeIter<T>` |

`ArrayIntoIter`, `ListIntoIter`, `MapIntoIter`, `MapIter`, and `RangeIter` are compiler-known iterator types. The
owning ones (`ArrayIntoIter`, `ListIntoIter`, `MapIntoIter`) drop the items they have not yielded; `MapIntoIter`
also clears each entry it moves out, so the map's storage is never dropped twice. `MapIter` and `RangeIter` only
read their source and have no drop glue. `Chars.next` decodes one scalar value with the same UTF-8 decoder as
`for c in s.chars()` and advances by its byte width.

**IT6 `next`.** `it.next()` requires a mutable place (E0113), like any `mut self` receiver, and returns
`Option<T>`. On a type parameter the call goes through the `Iterator` bound; on a user type it is the `next` method
of its impl; on a built-in iterator it is the compiler's own implementation. Calling `next` directly on an
`Iterable` that is not itself an iterator is E0002; call `.iter()` first.

**IT7 `iter` and `into_iter`.** `iter()` borrows the collection (`self`), so the collection stays usable and the
items that are not `Copy` are yielded as shared views of it. `into_iter()` takes the collection (`own self`), so
using the collection afterwards is E0100, and the items are yielded by value. `List` and `Map` implement both in the
compiler; `Map.into_iter()` yields owned `(K, V)` pairs and moves each entry out as it goes. For a user type, the
program writes both methods itself; `for` only requires `iter`.

**IT8 `for` over the protocol.** `for pattern in e { ... }`, when `e` is not one of the built-in `for` forms,
follows the protocol:

1. If the type implements `Iterable<T>`, the loop calls `e.iter()` and requires its result to be an
   `Iterator<T>` returning the same `T` (E0141).
2. Otherwise, if the type is itself an `Iterator<T>`, the loop consumes it.
3. Otherwise the loop is E0003.

A type that implements both is iterated through `iter()`, because `for` borrows its source (IT1). The loop lowers
to exactly:

```
let mut __iter = e
loop {
    match __iter.next() {
        Some(pattern) => { body },
        None => break,
    }
}
```

so the stage 0 semantics carry over: the loop binding lives for one iteration, `break` and `continue` target the
loop, items and the body's bindings drop each iteration, and the pattern must match every item (E0006;
`memory-rules.md` IT2). `__iter` is a compiler-made name.

A literal range (`for 2 in 0..4`) still uses the stage 0 lowering when its pattern is a binding or `_`; any other
pattern matches through `(lo..hi).iter()`, so it gets the protocol's item type and exhaustiveness rule. A stored
range value (`for x in r`) keeps the stage 0 lowering for a binding or `_`.

**IT9 Lazy adapters.** `Iterator<T>` declares these default methods. They build an adapter value that holds the
source iterator and move it forward; no collection is allocated, and an item is computed only when the consumer
asks for it.

| Method | Result | Notes |
| --- | --- | --- |
| `map<U, F: FnMut(T): U>(own self, own f: F)` | `MapIter<Self, T, U, F>` | applies `f` to every item |
| `filter<F: FnMut(&T): bool>(own self, own f: F)` | `FilterIter<Self, T, F>` | keeps the items `f` accepts; `f` sees a view so the item can still be returned |
| `take(own self, count: usize)` | `TakeIter<Self, T>` | at most `count` items |
| `skip(own self, count: usize)` | `SkipIter<Self, T>` | drops the first `count` items |
| `fold<A, F: FnMut(A, T): A>(mut self, own init: A, own f: F)` | `A` | left to right |
| `count(mut self)` | `usize` | consumes the iterator |
| `any(FnMut(T): bool)` | `bool` | stops at the first accepted item |
| `all(FnMut(T): bool)` | `bool` | stops at the first rejected item |
| `find<F: FnMut(&T): bool>(mut self, own f: F)` | `Option<T>` | stops at the first accepted item |
| `enumerate(own self)` | `EnumerateIter<Self, T>` | `(index, item)` pairs, counting from 0 |
| `take_while<F: FnMut(&T): bool>(own self, own f: F)` | `TakeWhileIter<Self, T, F>` | items up to, not including, the first one `f` rejects |
| `skip_while<F: FnMut(&T): bool>(own self, own f: F)` | `SkipWhileIter<Self, T, F>` | drops items while `f` accepts them, then yields the rest |
| `map_while<U, F: FnMut(T): Option<U>>(own self, own f: F)` | `MapWhileIter<Self, T, U, F>` | applies `f`, stops at the first `None` |
| `step_by(own self, step: usize)` | `StepByIter<Self, T>` | every `step`th item, starting with the first; `step` 0 panics |
| `inspect<F: FnMut(&T)>(own self, own f: F)` | `InspectIter<Self, T, F>` | calls `f` on a view of each item and passes it on |
| `peekable(own self)` | `PeekIter<Self, T>` | adds `peek(mut self): Option<&T>`, a view of the next item that is not taken |
| `to_list(mut self)` | `List<T>` | collects the remaining items; a chain over borrowed items gives a second-class list of views, which cannot be stored |

**Adapters that are not provided (issue #706).** Three constraints of the current language rule these out; each is a
language change, not a library one, and stays open on the issue:

- `zip` and `chain` take two iterators and return one chain over both. When both are second-class (a list `iter()`),
  the result derives from two parameters, which RT3 rejects with E0121. A change would need a return origin that is a
  set of parameters.
- `flat_map`, `flatten`, `cloned`, and `copied` need a type that appears only in a bound (`J: Iterator<U>`, a view's
  target), and a call cannot infer a type parameter from a bound alone ("cannot tell what `U` stands for").
- `rev` needs a source that can walk backwards, and the built-in iterators are not `Iterator` implementers that a
  `DoubleEndedIterator<T>` interface could be added to; `cycle` needs a clonable iterator. Neither exists yet.

`any` is reserved as an identifier (it belongs to m15), so `std` declares the method as `any_item`; the compiler
spells the call `.any(...)`. The default methods are instantiated on demand for each receiver type, whether the
receiver is built in or a user type. They are ordinary `std` code: `MapIter`, `FilterIter`, `TakeIter`, and
`SkipIter` are generic structs whose `next` calls the source's `next` and the closure.

`it.sum()` is compiler-known: on an iterator of integers it synthesizes the loop that adds every item, and it is
E0141 on an iterator that yields anything else. `sum` also accepts integer views (`&T` where `T` is an integer).

**IT10 Eager `List` methods.** On a `List<T>` receiver, these names resolve to the private generic functions in
`std` that borrow the list, pass the closure a view of each element, and return owned results:

| Method | Result | Bound |
| --- | --- | --- |
| `xs.map(f)` | `List<U>` | `F: FnMut(&T): U` |
| `xs.filter(f)` | `List<T>` | `F: FnMut(&T): bool`, `T: Clone` |
| `xs.fold(init, f)` | `A` | `F: FnMut(A, &T): A` |
| `xs.any(f)` | `bool` | `F: FnMut(&T): bool` |
| `xs.all(f)` | `bool` | `F: FnMut(&T): bool` |
| `xs.find(f)` | `Option<T>` | `F: FnMut(&T): bool`, `T: Clone` |

They are eager: each builds its result list immediately and the source list is not consumed. `filter` and `find`
clone the element they keep; `map`'s result type `U` comes from the closure's return type.

**IT11 Ranges.** `Range<T>` and `RangeInclusive<T>` are `Iterable<T>`: `r.iter()` and `r.into_iter()` both give a
`RangeIter<T>`, which yields the endpoints in order. An inclusive iterator yields the upper endpoint and then
stops, without computing `upper + 1`, so `(255 as u8)..=(255 as u8)` yields one item and never overflows. An
exhausted range iterator keeps returning `None`. `for x in r` over a stored range keeps the stage 0 lowering, which
evaluates `r` once and has the same no-overflow behavior.

**IT12 Second-class chains.** A chain borrows its source: its adapter types are generic instances parameterized by
the source's view type (and a closure may be second-class too), so the chain is second-class (`memory-rules.md`,
SC3). It may be a local, a parameter, a temporary, or an argument; it cannot be a struct field, an enum payload, a
`List`/`Map`/`Heap` element, or a `const` (E0120), and a chain over a temporary cannot be stored in a local (E0123).
Returning a chain from a function is allowed when it derives from exactly one view-typed parameter (RT2), which is
how `iter()`/`into_iter()` and the adapters return one. The loans a chain holds last as long as the chain value
(CL7/RF2): while a chain over a collection is live and used, the collection cannot be written or moved.

Tests: `tests/m13_iterators/`.
