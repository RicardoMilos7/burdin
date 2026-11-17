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
| `last(mut self)` | `Option<T>` | the last item |
| `nth(mut self, n: usize)` | `Option<T>` | the item `n` places on, counting from 0; consumes the ones before |
| `position<F: FnMut(T): bool>(mut self, own f: F)` | `Option<usize>` | index of the first accepted item |
| `for_each<F: FnMut(T)>(mut self, own f: F)` | `()` | calls `f` on every item |
| `reduce<F: FnMut(T, T): T>(mut self, own f: F)` | `Option<T>` | folds with the first item as the start; `None` when empty |
| `partition<F: FnMut(&T): bool>(mut self, own f: F)` | `(List<T>, List<T>)` | accepted items, then rejected items, in order |
| `min_by` / `max_by` `<F: FnMut(&T, &T): Ordering>(mut self, own f: F)` | `Option<T>` | least (first of equals) / greatest (last of equals) under the comparator |
| `min_by_key` / `max_by_key` `<K: Ord, F: FnMut(&T): K>(mut self, own f: F)` | `Option<T>` | the same, ordered by a key |
| `to_list(mut self)` | `List<T>` | collects the remaining items; a chain over borrowed items gives a second-class list of views, which cannot be stored |
| `collect<C: FromIterator<T>>(own self)` | `C` | builds any collection that implements `FromIterator<T>`; the expected type names `C` (IT14) |

**Adapters that are not provided (issue #706).** Three constraints of the current language rule these out; each is a
language change, not a library one, and stays open on the issue:

- `zip` and `chain` take two iterators and return one chain over both. When both are second-class (a list `iter()`),
  the result derives from two parameters, which RT3 rejects with E0121. A change would need a return origin that is a
  set of parameters.
- `flat_map`, `flatten`, `cloned`, and `copied` need a type that appears only in a bound (`J: Iterator<U>`, a view's
  target), and a call cannot infer a type parameter from a bound alone ("cannot tell what `U` stands for"), except
  from an `Iterator<T>` bound (IT20), which does not cover these.
- `rev` needs a source that can walk backwards, and the built-in iterators are not `Iterator` implementers that a
  `DoubleEndedIterator<T>` interface could be added to; `cycle` needs a clonable iterator. Neither exists yet.

**Consumers that are not provided (issue #707).** `min`, `max`, `eq`, and `cmp` between iterators need a bound on the
interface's own item type (`where T: Ord`), which a default method cannot state (E9000); use `min_by`/`max_by` with
`a.cmp(*b)` and `fold`. `unzip` needs the pair's two types, which only appear in a bound, so a call cannot infer them.
`rposition` needs a double-ended source (see the adapters above). `product` and the float `sum` need compiler support
like integer `sum`, and `try_fold` and `try_for_each` are not written yet; `fold` covers the first, and a `for` loop with
`?` the others.

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

## Collecting and extending (issue #708; IT13-IT20)

`to_list` is the one way a chain becomes a collection that stage 1 started with. These rules give every collection the
same two entry points: build one from an iterator (`FromIterator`, `collect`, `from`) and add an iterator's items to one
that exists (`extend_iter`). They live in `std/src/iter.bd` and, for the collections, next to each collection.

**IT13 `FromIterator<T>`.** `std` declares

```
interface FromIterator<T> {
    fn from_iter<I: Iterator<T>>(own iter: I): Self
}
```

A type implements it with a `Type: FromIterator<T> { ... }` block, and `Type.from_iter(it)` builds a value from every
item `it` yields, in order. `Set<T>`, `Deque<T>`, and `PriorityHeap<T>` implement it in `std`, a program implements it
for its own types (`m24_stdlib/collect_user_type`), and a generic function can build any implementer with
`C.from_iter(it)` where `C: FromIterator<T>`. The method has a type parameter of its own (`I`), so a call through a bound
matches `I` against the argument (the checker's symbolic call), and a call on a concrete type is an ordinary generic
associated function.

**IT14 `collect` and its target.** `Iterator<T>` has the default method `collect<C: FromIterator<T>>(own self): C`, which
is `C.from_iter(self)`. The language has no explicit type arguments on a member call (`x.collect<Set<i32>>()` is E0002;
grammar.md, "member type arguments"), so **the target is named by the expected type**: the annotation of a `let`, the
declared type of a parameter, the return type, a field's type. A call with no expected type cannot tell what `C` stands
for (E0003), and a target that does not implement `FromIterator<T>` for the items the iterator yields is E0141. Collecting
is eager: it consumes the whole iterator, except where IT16 says it stops early.

```
let names: Set<String> = people.iter().map(|p| p.name.clone()).collect()
fn evens(xs: List<i32>): List<i32> { xs.into_iter().filter(|x| *x % 2 == 0).collect() }
```

**IT15 Built-in targets.** The compiler implements `FromIterator` for these types without an impl block, and
`Type.from_iter(it)` is the call of a `std` function that builds it (`__list_from_iter` and the others in `iter.bd`):

| Target | Items | Rule |
| --- | --- | --- |
| `List<T>` | `T` | in order; `T` must be an owned type, so a chain of views is not a target (E0141) |
| `Map<K, V>` | `(K, V)` | `K: Eq`, and neither type a view; a later pair replaces the value of an earlier one with the same key, like `insert`, and the replaced value is dropped |
| `String` | `char`, `str`, `String`, or `&String` | the pieces are appended in order |
| `Set<T>`, `Deque<T>`, `PriorityHeap<T>` | `T` | by their `std` impl blocks; a set keeps one of each element, a heap orders by `Ord` |

Anything else is E0141: a `String` from integers, a `Map` from items that are not pairs, a `List<i64>` from `i32` items.

**IT16 `Option` and `Result`.** `Option<C>` implements `FromIterator<Option<T>>` and `Result<C, E>` implements
`FromIterator<Result<T, E>>` whenever `C: FromIterator<T>`. The iterator is read until its first `None` or `Err`, which
is the result, and it is not asked for another item. When every item is `Some`/`Ok`, the payloads are gathered in a list
and then handed to `C.from_iter`, so the target is built only on success and the gathered items of a failed collection
are dropped at once. The rule nests: `Result<Option<List<i32>>, E>` is a target for items of type
`Result<Option<i32>, E>`, because `Option<List<i32>>` implements `FromIterator<Option<i32>>`.

**IT17 `extend_iter`.** Every collection adds the items of any iterator to itself: `list.extend_iter(it)`,
`map.extend_iter(it)` (pairs), `set.extend_iter(it)`, `deque.extend_iter(it)`, `heap.extend_iter(it)`, and
`string.extend_iter(it)` (`char` items). It takes an `Iterator<T>`, so a collection is passed through `into_iter()` (the
items move) or `iter()` (for a collection of `Copy` elements the items are the elements themselves). `extend(own
List<T>)` keeps its meaning; the names differ because the language has no overloading, and `extend_iter` is the form
that takes a chain. Unlike `collect`, `extend_iter` works for every allocator (IT19).

**IT18 `from` an array.** `List<T>.from([a, b, c])` and `Map<K, V>.from([(k, v), ...])`, and `Set<T>.from`,
`Deque<T>.from`, and `PriorityHeap<T>.from`, build a collection from an array literal, moving the elements. `List.from`
and `Map.from` also work without type arguments, taking them from the array or the expected type; the other three need
the type arguments written or an expected type for a `Set`. The elements of the array take their type from the expected
type, so `let xs: List<i64> = List.from([1, 2, 3])` is a list of `i64`. A map key is an owned value, so
`Map.from([("a", 1)])` is E0120 at the call, because a `str` is a view: write `String.from("a")`.

**IT19 Allocators.** `from_iter`, `collect`, and `from` build a collection that uses the default allocator, because a
constructor has no allocator value to store (`allocators.md`, AL4). `List<T, A>.from_iter(it)` and the other
constructors on a `List` or `Map` with another allocator are E0272, and `collect` into one is E0141 (it does not
implement `FromIterator`); `Set<T, A>` with another allocator is E0141 too. To fill a container that stores an allocator,
make it with `new_in` and call `extend_iter`, which is generic over the allocator (`m24_stdlib/collect_allocator`). There
is no `collect_in`.

**IT20 Inference from an `Iterator<T>` bound.** A generic call binds a type parameter that appears only in an
`Iterator<T>` bound (`fn f<T, I: Iterator<T>>(it: I)`) from the item type of the iterator `I` stands for, once `I` is
known. This is what lets `__result_from_iter` find the payload type from `Iterator<Result<T, E>>`. Other bounds still
infer nothing.

Tests: `tests/m24_stdlib/collect_*`, `extend_iter`, and `from_array` (listed in `stdlib.md`, "Collecting and extending").
