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

`Iterator<T>` is the stateful object a chain is made of (IT14 adds the second interface, `DoubleEndedIterator<T>`): `next` borrows the iterator exclusively, advances it, and
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
`for c in s.chars()` and advances by its byte width. `List.drain(range)` also yields a `ListIntoIter<T>`: it removes
the range from the list and hands the removed elements over in a buffer with the default allocator
(`stdlib.md`).

`[T].chunks(n)`, `[T].chunks_exact(n)`, and `[T].windows(n)` return compiler-known iterators (`Chunks<T>`,
`ChunksExact<T>`, `Windows<T>`) that borrow the slice and yield `[T]` views (`spec/stdlib.md`, "Slices"). They are
iterators only, not `Iterable`: `for chunk in xs.chunks(n)` enters through the protocol (IT8), and `iter()` is not
one of their methods. Like every view they are second-class (SC2), and the source stays borrowed until the last
use of the iterator or of a view it yielded. The slice's own `last` and `to_list` win over the `Iterator` defaults
of the same name (EX3): an `Iterator` method a receiver's own table also provides is the receiver's method.

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
`memory-rules.md` IT2). `__iter` is a compiler-made name. A label on the `for` goes on this generated `loop`
(the node `break 'label` and `continue 'label` target), not on the surrounding `let`/block.

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
| `any<F: FnMut(T): bool>(mut self, own f: F)` | `bool` | stops at the first accepted item; `std` declares it as `any_item` (see below) |
| `all<F: FnMut(T): bool>(mut self, own f: F)` | `bool` | stops at the first rejected item |
| `find<F: FnMut(&T): bool>(mut self, own f: F)` | `Option<T>` | stops at the first accepted item |
| `enumerate(own self)` | `EnumerateIter<Self, T>` | `(index, item)` pairs, counting from 0 |
| `zip<U, J: Iterator<U>>(own self, own other: J)` | `ZipIter<Self, T, J, U>` | `(item, other item)` pairs; ends when either source ends |
| `chain<J: Iterator<T>>(own self, own other: J)` | `ChainIter<Self, J, T>` | every item of `self`, then every item of `other`; both yield the same type |
| `flat_map<U, J: Iterator<U>, F: FnMut(T): J>(own self, own f: F)` | `FlatMapIter<Self, T, J, U, F>` | the items of the iterator `f` returns for each item, one iterator at a time |
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
| `try_fold<A, E, F: FnMut(A, T): Result<A, E>>(mut self, own init: A, own f: F)` | `Result<A, E>` | folds until `f` returns an `Err`, which ends the fold and is returned; the rest of the items are not visited |
| `try_for_each<E, F: FnMut(T): Result<(), E>>(mut self, own f: F)` | `Result<(), E>` | calls `f` on every item until it returns an `Err` |
| `to_list(mut self)` | `List<T>` | collects the remaining items; a chain over borrowed items gives a second-class list of views, which cannot be stored |
| `collect<C: FromIterator<T>>(own self)` | `C` | builds any collection that implements `FromIterator<T>`; the expected type names `C` (IT17) |

**Adapters for a particular kind of source (IT13).** `flatten`, `cloned`, `copied`, `cycle`, and `rev` need a bound on
the interface's own item type or on the source itself (the items are iterators, the items are views, the source is
`Clone`, the source is double-ended), which a default method of `Iterator<T>` cannot state. The compiler resolves
`it.flatten()`, `it.cloned()`, `it.copied()`, `it.cycle()`, and `it.rev()` on any iterator, built in or user-defined, to
private generic functions in `std` that take the iterator as `own it: I`:

| Method | Result | Needs |
| --- | --- | --- |
| `it.flatten()` | `FlattenIter<I, J, U>` | the items are iterators: `I: Iterator<J>`, `J: Iterator<U>`; yields every `U` |
| `it.cloned()` | `ClonedIter<I, U>` | the items are views: `I: Iterator<&U>`, `U: Clone`; yields owned clones |
| `it.copied()` | `CopiedIter<I, U>` | the items are views: `I: Iterator<&U>`, `U: Copy`; yields copies |
| `it.cycle()` | `CycleIter<I, T>` | `I: Iterator<T> + Clone`; clones the source at the end of every pass, forever |
| `it.rev()` | `RevIter<I, T>` | `I: Iterator<T> + DoubleEndedIterator<T>` (IT14) |

A source that does not fit is E0141 (the bound is not met) or E0003 (the type parameter has nothing to be inferred
from: `(0..3).iter().flatten()`, `[1, 2].iter().cloned()`). A list of `Copy` items yields the items themselves, so
`cloned`/`copied` apply to the iterators that yield views: a list of non-`Copy` items, a `Deque`, and the like. A method
declared on the type itself (inherent or interface) wins over these names, like every method over an extension.

**Inference from `Iterator` bounds.** A type parameter that appears only inside an `Iterator<...>` bound
(`zip<U, J: Iterator<U>>`) is bound by the item type of the argument's type, once the owner of the bound (`J`) is
known: the built-in iterator's item or the `T` of the user's `Type: Iterator<T>` block. One bound can feed another
(`flatten`: `I` fixes `J`, then `J` fixes `U`). It is the counterpart of the closure-signature inference for `Fn`
bounds.

**IT14 Walking backwards.** `rev` takes a `DoubleEndedIterator<T>`, a second interface `std` declares:

```
interface DoubleEndedIterator<T> {
    fn next_back(mut self): Option<T>
}
```

`next_back` takes the last remaining item, or `None`; `next` and `next_back` take from the two ends and meet in the
middle, and an exhausted iterator keeps returning `None` from both. The compiler implements the interface, with the
same item type as `next`, for the slice iterator (the `iter()` of an array or a `List`), the range iterator (an
inclusive range yields its upper endpoint first and never computes a bound outside its type), and `Chars` (walking back
by scalar value, so `"añ😀".chars().rev()` yields `😀`, `ñ`, `a`). The owning iterators (`ArrayIntoIter`,
`ListIntoIter`, `MapIntoIter`) and `MapIter` are not double-ended: `list.into_iter().rev()` is E0141. `std` implements it
for `DequeIter`, for `RevIter` (its `next_back` is the source's `next`), and for the adapters whose order follows their
source's: `MapIter`, `FilterIter`, `InspectIter`, `ChainIter`, `ClonedIter`, and `CopiedIter`. The adapters that count
from the front or hold a lookahead (`enumerate`, `zip`, `take`, `skip`, `step_by`, `take_while`, `skip_while`,
`peekable`) are not, so `rev` after them is E0141; put `rev` before them. A user type implements
`DoubleEndedIterator<T>` with a `next_back` method next to its `Iterator<T>` block to join `rev`. A `for` loop over a
range, an array, a list, or a string can use `.iter().rev()` (a range written `(0..n)` is `Iterable`, not an
iterator, so it needs `.iter()` first).

**Several sources in one result (RT3a).** `zip` and `chain` return a chain over two views, so the return origin of a
function is a set of parameters, not one (`memory-rules.md`, RT3a): the result of a call derives from the storage of
each argument in the set, and both stay borrowed while the result is live. A result that is a view itself (`&T`,
`str`, `[T]`, or an `Option` of one) still derives from exactly one parameter (RT3). An adapter that takes `own` iterators
as arguments is not rejected by the exclusivity rule when its receiver is a chain computed in the same call, so
`xs.iter().zip(xs.iter().skip(1))` is allowed: both are shared views of the same list.

**Not provided.** `cycle` needs a clonable source, and a closure is never `Clone` (`memory-rules.md`, CL10), so a chain
with `map`, `filter`, `inspect`, or a closure-taking adapter cannot be cycled; `cycle` the plain source and apply the
adapter after it. An owning iterator is not `Clone`, so `into_iter().cycle()` is E0141. `rev` over `enumerate`, `zip`,
`take`, and `skip` would need the length of the source (an `ExactSizeIterator`), which stage 1 does not have.

**Consumers that need a bound on the item (issue #707).** `min`, `max`, `eq`, and `cmp` between iterators need
`T: Ord` (`T: Eq` for `eq`) on the interface's own item type, which a default method cannot state (a method-level
`where` names only the method's own parameters, E9000). They are therefore generic functions in `std`
(`__iter_min`, `__iter_max`, `__iter_eq`, `__iter_cmp`) that the compiler calls for an iterator receiver, handing them the
item type as `T`; they are not interface methods, so a user type cannot override them, and on a type parameter bounded by
`Iterator<T>` they work too (`fn smallest<I: Iterator<i32>>(own it: I) { it.min() }`).

| Method | Result | Notes |
| --- | --- | --- |
| `min()` | `Option<T>` | the least item (the first of equals); `T: Ord`; `None` when empty |
| `max()` | `Option<T>` | the greatest item (the last of equals); `T: Ord`; `None` when empty |
| `eq(other)` | `bool` | both iterators yield equal items and end together; `T: Eq` |
| `cmp(other)` | `Ordering` | lexicographic; a prefix is less than the longer iterator; `T: Ord` |

The item must itself have `Ord` (or `Eq`): an owned item does, a shared view (`&String`, the item of a `List` of
non-`Copy` values) does not, and a float has neither. Both are E0141; `min_by`/`max_by` with a comparator
(`a.cmp(*b)`), or `into_iter` over an owned list, are the ways around it. Integers and other `Copy` items are yielded by
value, so `xs.iter().min()` works on a `List<i32>`.

**`sum` and `product`.** The compiler writes both as an ordinary loop over `+` or `*`, starting from 0 and 1, on an
iterator of integers or floats or of views of them (anything else is E0141). Overflow is therefore the overflow of the
operator: an integer `sum` or `product` that overflows panics in the checked (debug) build and wraps in the release
build, exactly as `a + b` does; a float never traps. An empty iterator gives 0 for `sum` and 1 for `product`.

**`try_fold` and `try_for_each`** short-circuit on a `Result`. The error type `E` is inferred from the closure's
return type; a closure with an early `return Err(..)` needs its return type written (`|x|: Result<(), i32> { ... }`). An
`Option` version is not provided: `fold` with an `Option` accumulator, or a `for` loop with `?`, covers it. They are
interface default methods, and they are callable on a type parameter bounded by `Iterator<T>` like every other default
method (IT23).

**Consumers that are not provided (issue #707).** `unzip` needs the pair's two types, which only appear in a bound, so
a call cannot infer them. `rposition` needs a double-ended source (IT14), which exists now, but it is not written yet. These stay open on the
issue, as do the language changes they wait for.


`any` is a keyword (it begins the `any I` existential type, `existentials.md`), so a method cannot be declared with that
name and `std` declares it as `any_item` (decided in issue #707: the name stays); the compiler spells the call
`.any(...)`. The default methods are instantiated on demand for each receiver type, whether the
receiver is built in or a user type. They are ordinary `std` code: `MapIter`, `FilterIter`, `TakeIter`, and
`SkipIter` are generic structs whose `next` calls the source's `next` and the closure.

`it.sum()` and `it.product()` are compiler-known: on an iterator of integers or floats they synthesize the loop that
adds or multiplies every item, and they are E0141 on an iterator that yields anything else. They also accept views of
numbers (`&T` where `T` is an integer or a float).

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
Returning a chain from a function is allowed when it derives from view-typed parameters (RT2): from exactly one for
`iter()`/`into_iter()` and most adapters, and from several for `zip` and `chain` (RT3a). The loans a chain holds last as long as the chain value
(CL7/RF2): while a chain over a collection is live and used, the collection cannot be written or moved.

**IT15 Mutable iteration (issue #709).** A loop can write every element of a collection in place. Decisions:

- *Spelling.* Both forms exist and mean the same thing: `xs.iter_mut()` returns an iterator, and `for pattern in mut xs`
  is `for pattern in xs.iter_mut()`. The `mut` after `in` follows the call-site marker of `mut` arguments (CV2): it says
  the loop borrows `xs` exclusively. `xs` must be a mutable place (E0113), like any `mut self` receiver.
- *The item is a `&mut T`, not a new kind of place.* The loop binding is an ordinary `&mut T` value, so an element is
  read and written with `*x` (`*x = *x + 1`), and fields and methods work through it without a `*` (RF1). This keeps
  IT2 unchanged (a plain loop binding is still a read-only place) and needs no new binding rule: `Option<&mut T>`
  and `&mut V` already come out of `get_mut` and `entry`.
- *Sources.*

| Type | `iter_mut()` gives | `next()` yields |
| --- | --- | --- |
| `[T; N]` (a mutable place) | `IterMut<T>` | `&mut T` |
| `List<T>` | `IterMut<T>` | `&mut T` |
| `Map<K, V>` | `MapIterMut<K, V>` | `(&K, &mut V)` |
| `Deque<T>` | `DequeIterMut<&mut Deque<T>, T>` | `&mut T` |

  A map's keys stay read-only (assigning through `&K` is E0005), and the order of a map walk is unspecified. `Deque`
  declares `iter_mut(mut self)` in `std`; a user type joins `for x in mut c` by declaring an `iter_mut(mut self)` method
  that returns an `Iterator<U>`, with no interface to implement. A type without `iter_mut` is E0002. A slice `[T]`
  has no `iter_mut`: a slice is a read-only view, and a writable slice type does not exist in the language yet.
  `Heap` and `Set` have none either, because writing an element would break their ordering or hashing.
- *Exclusivity.* `iter_mut()` borrows the collection exclusively (`memory-rules.md`, IT15): while the iterator or an
  item derived from it is live, any use of the collection is E0111. Inside `for x in mut xs`, that makes `xs.push`,
  `xs.insert`, `xs.remove`, `xs.len()`, reading `xs[i]`, and a second `for ... in mut xs` all E0111, so the table or
  buffer can never move under the loop. A view copied out of the loop (`held = x`) keeps the collection borrowed
  until the copy's last use. Liveness is per top-level statement (RF2), so when the loop sits inside a nested block
  (an `if`, a `while`, or a block expression) the collection stays borrowed until the end of the enclosing
  top-level statement; finish the loop in its own statement of the function body to use the collection again.
- *Adapters.* The iterator is an ordinary `Iterator<&mut T>`, so the adapters of IT9 apply to it unchanged:
  `enumerate` gives `(usize, &mut T)`, and `take`, `skip`, `step_by`, `take_while`, `skip_while`, `peekable`, `nth`,
  `last`, `for_each`, and `count` work as they do over any iterator. `zip` and `chain` work too (`a.iter_mut().zip(b.iter())`), each source borrowing its own collection. `rev` does not: a
  mutable iterator is not a `DoubleEndedIterator` (IT14), so `rev` over it is E0141.
  `filter`, `find`, and the other adapters that hand the closure `&T` see a `&&mut T`.
- *Compile time.* A mutable iterator mutates a run-time collection, so `iter_mut` is impure at compile time (E0224,
  CT9), like `get_mut`.
- *Drops.* A mutable iterator owns nothing and has no drop glue. Writing `*x = v` drops the element it replaces.

Tests: `tests/m13_iterators/`; the adapters of IT9, IT13, and IT14 are covered by `tests/m24_stdlib/iter_zip`,
`iter_chain`, `iter_flat_map`, `iter_flatten`, `iter_cloned_copied`, `iter_cycle`, `iter_rev`, `iter_adapters_drops`,
`iter_adapters_no_leaks`, and `iter_adapters_comptime`; errors: `iter_zip_from_temporary` (E0123),
`iter_zip_in_struct_field`, `iter_chain_in_collection` (E0120), `iter_chain_while_mutating_second`,
`iter_zip_while_mutating_first` (E0111), `iter_rev_needs_double_ended`, `iter_rev_through_adapter`,
`iter_cloned_needs_clone`, `iter_cycle_needs_clone`, `iter_chain_item_mismatch` (E0141), `iter_cloned_needs_views`,
`iter_flatten_needs_iterators`, `iter_zip_needs_iterator` (E0003).
Mutable iteration (IT15) is covered by `tests/m24_stdlib/iter_mut_*`.

## Collecting and extending (issue #708; IT16-IT22)

`to_list` is the one way a chain becomes a collection that stage 1 started with. These rules give every collection the
same two entry points: build one from an iterator (`FromIterator`, `collect`, `from`) and add an iterator's items to one
that exists (`extend_iter`). They live in `std/src/iter.bd` and, for the collections, next to each collection.

**IT16 `FromIterator<T>`.** `std` declares

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

**IT17 `collect` and its target.** `Iterator<T>` has the default method `collect<C: FromIterator<T>>(own self): C`, which
is `C.from_iter(self)`. **The target is named in one of two ways.** Either the call gives it as the explicit type
argument `x.collect<Set<i32>>()` (`arguments.md`, TA5; grammar.md, "member type arguments"), which seeds `C` with that
type, or the call takes it from the **expected type**: the annotation of a `let`, the declared type of a parameter, the
return type, a field's type. A call with neither (`let xs = [1, 2].into_iter().collect()`, with no annotation) cannot tell
what `C` stands for (E0003). An explicit list of another length is E0003 (TA2), and a target that does not implement
`FromIterator<T>` for the items the iterator yields is E0141, whichever way it was named. On a value of a type parameter
bounded by `Iterator<T>` the target is checked against `C`'s bounds the same way, and `C` may be a type parameter of the
enclosing function. Collecting is eager: it consumes the whole iterator, except where IT19 says it stops early.

```
let names: Set<String> = people.iter().map(|p| p.name.clone()).collect()
let ids = people.iter().map(|p| p.id).collect<List<i32>>()
fn evens(xs: List<i32>): List<i32> { xs.into_iter().filter(|x| *x % 2 == 0).collect() }
```

Only `collect` takes an explicit target. The other `Iterator<T>` defaults and the adapters keep their refusal (TA5).

**IT18 Built-in targets.** The compiler implements `FromIterator` for these types without an impl block, and
`Type.from_iter(it)` is the call of a `std` function that builds it (`__list_from_iter` and the others in `iter.bd`):

| Target | Items | Rule |
| --- | --- | --- |
| `List<T>` | `T` | in order; `T` must be an owned type, so a chain of views is not a target (E0141) |
| `Map<K, V>` | `(K, V)` | `K: Eq`, and neither type a view; a later pair replaces the value of an earlier one with the same key, like `insert`, and the replaced value is dropped |
| `String` | `char`, `str`, `String`, or `&String` | the pieces are appended in order |
| `Set<T>`, `Deque<T>`, `PriorityHeap<T>` | `T` | by their `std` impl blocks; a set keeps one of each element, a heap orders by `Ord` |

Anything else is E0141: a `String` from integers, a `Map` from items that are not pairs, a `List<i64>` from `i32` items.

**IT19 `Option` and `Result`.** `Option<C>` implements `FromIterator<Option<T>>` and `Result<C, E>` implements
`FromIterator<Result<T, E>>` whenever `C: FromIterator<T>`. The iterator is read until its first `None` or `Err`, which
is the result, and it is not asked for another item. When every item is `Some`/`Ok`, the payloads are gathered in a list
and then handed to `C.from_iter`, so the target is built only on success and the gathered items of a failed collection
are dropped at once. The rule nests: `Result<Option<List<i32>>, E>` is a target for items of type
`Result<Option<i32>, E>`, because `Option<List<i32>>` implements `FromIterator<Option<i32>>`. The payload type `T` of
`Iterator<Result<T, E>>` is found by the inference from an `Iterator` bound (IT13).

**IT20 `extend_iter`.** Every collection adds the items of any iterator to itself: `list.extend_iter(it)`,
`map.extend_iter(it)` (pairs), `set.extend_iter(it)`, `deque.extend_iter(it)`, `heap.extend_iter(it)`, and
`string.extend_iter(it)` (`char` items). It takes an `Iterator<T>`, so a collection is passed through `into_iter()` (the
items move) or `iter()` (for a collection of `Copy` elements the items are the elements themselves). `extend(own
List<T>)` keeps its meaning; the names differ because the language has no overloading, and `extend_iter` is the form
that takes a chain. Unlike `collect`, `extend_iter` works for every allocator (IT22).

**IT21 `from` an array.** `List<T>.from([a, b, c])` and `Map<K, V>.from([(k, v), ...])`, and `Set<T>.from`,
`Deque<T>.from`, and `PriorityHeap<T>.from`, build a collection from an array literal, moving the elements. `List.from`
and `Map.from` also work without type arguments, taking them from the array or the expected type; the other three need
the type arguments written or an expected type for a `Set`. The elements of the array take their type from the expected
type, so `let xs: List<i64> = List.from([1, 2, 3])` is a list of `i64`. A map key is an owned value, so
`Map.from([("a", 1)])` is E0120 at the call, because a `str` is a view: write `String.from("a")`.

**IT22 Allocators.** `from_iter`, `collect`, and `from` build a collection that uses the default allocator, because a
constructor has no allocator value to store (`allocators.md`, AL4). `List<T, A>.from_iter(it)` and the other
constructors on a `List` or `Map` with another allocator are E0272, and `collect` into one is E0141 (it does not
implement `FromIterator`); `Set<T, A>` with another allocator is E0141 too. To fill a container that stores an allocator,
make it with `new_in` and call `extend_iter`, which is generic over the allocator (`m24_stdlib/collect_allocator`). There
is no `collect_in`.

Tests: `tests/m24_stdlib/collect_*`, `extend_iter`, and `from_array` (listed in `stdlib.md`, "Collecting and extending").

**IT23 Generic interfaces and their default methods (issue #390).** These rules apply to every generic interface, the
standard ones included:

1. An interface parameter may carry bounds (`interface Keyed<K: Eq>`). Every impl and every bound that names the
   interface must give arguments that meet them, which is E0141.
2. `requires` names interfaces with arguments, and the arguments may be the interface's own parameters
   (`interface Labeled<T> requires Show + Store<T>`). An impl must implement each required instance (E0141).
3. A named generic interface takes exactly its type arguments, in a bound, an impl, or a `requires` list. A missing or
   extra argument is E0003 at the name.
4. A default method may have type parameters of its own. Through a bound on a type parameter, the call infers them
   from the arguments as it does on a concrete receiver: a parameter that appears only in a closure bound
   (`F: FnMut(T): U`) is the closure's return type, and one that appears only in an `Iterator` bound is that
   iterator's item type. The interface's parameters stand for the bound's arguments throughout. A parameter that
   nothing fixes is E0003 at the call.
5. A closure argument to such a method takes its parameter types from the closure bound, as a generic function's
   argument does.

Tests: `tests/m13_iterators/generic_bound_default_methods` (rules 4 and 5, with `map`, `zip`, `flat_map`, and `max`
on an `Iterator<i32>` parameter), `generic_interface_requires` (rules 1 and 2 and default methods that use a bound of
their parameter), `generic_interface_unmet` (E0141), `generic_interface_arity` (E0003), and
`generic_method_uninferred` (E0003 at the call). The unit test `generics::tests::a_bound_provides_its_generic_methods_with_the_arguments_substituted`
covers the lookup.
