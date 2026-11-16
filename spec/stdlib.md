# Burdin standard library (stage 0)

The names and signatures a stage 0 program may use. They are a language decision, not an implementation detail: a
change here changes the tests and the compiler together. Issues #28, #29, #30, and #31 settled this surface; the
tests named under each section witness it.

Signatures are written in Burdin. A parameter without `mut` or `own` is read-only and borrowed for the call; `mut`
takes `mut` at the call site; `own` moves and takes no marker (`memory-rules.md`, CV). `T` is a value type that can be
stored (not a second-class view). The one implicit conversion applies throughout: a `String` passed where a `str`
is expected is borrowed as a `str` (IC1).

## Output

```
fn format(fmt: str, ...): String
fn print(fmt: str, ...)
fn println(fmt: str, ...)
fn eprint(fmt: str, ...)
fn eprintln(fmt: str, ...)
```

Each takes a string literal, with or without `{...}` placeholders, followed by the arguments its placeholders use
(`format.md`, FM1 and FM6). The output functions write the text to stdout or stderr; `println`/`eprintln` add one `\n`,
and `format` returns the `String`. A call with one argument takes no format string, so `println(text)`
still writes any `str` value, and a `String` argument is converted by IC1.
Standard output is buffered and standard error is not; the buffer is flushed when it fills, after a line when
stdout is a terminal, and before the process ends through a normal return, `process.exit`, a panic, or the
stack-overflow guard. A program never sees the difference except through the timing of the bytes (there is no
explicit output handle in stage 0); the flush points are what keep the order of stdout and stderr correct before
an abnormal end.

## Numbers and parsing

Every integer type (`i8`..`i64`, `isize`, `u8`..`u64`, `usize`) has:

```
fn to_string(self): String
fn wrapping_add(self, other: Self): Self
fn wrapping_sub(self, other: Self): Self
fn wrapping_mul(self, other: Self): Self
fn checked_add(self, other: Self): Option<Self>
fn checked_sub(self, other: Self): Option<Self>
fn checked_mul(self, other: Self): Option<Self>
fn min(self, other: Self): Self
fn max(self, other: Self): Self
fn abs(self): Self                    // signed types only
fn parse(s: str): Result<Self, ParseIntError>
```

Floats have `to_string(self): String`, through `Display` (`format.md`, FM8); `f64` and `f32` have no other
methods in stage 0. `bool`, `char`, and `str` have `to_string(self): String`. `bool` also works in `if`/`while` conditions. `parse` is
the associated function `i32.parse(s)`; it accepts an optional leading `-` and digits in the base 10 and returns
`Err(ParseIntError.Invalid)`, whose message is `invalid integer`, for anything else.

Arithmetic is the built-in operators only: `+ - * / %`, `<< >>`, `& | ^`, unary `-`. Overflow panics in debug and
wraps in release for `+ - *` and unary `-`; division by zero and out-of-range shifts always panic. `/` and `%`
truncate toward zero. `as` converts between numeric types (and `char` to an integer), following Rust's cast rules.

## Operators

`std` declares the operator interfaces in `std.ops`; the prelude makes the names visible in every module, so a user
type takes part in an operator by implementing one of them (`operators.md`, OP1-OP2):

```
interface Add      { fn add(self, other: Self): Self }
interface Sub      { fn sub(self, other: Self): Self }
interface Mul      { fn mul(self, other: Self): Self }
interface Div      { fn div(self, other: Self): Self }
interface Rem      { fn rem(self, other: Self): Self }
interface BitAnd   { fn bitand(self, other: Self): Self }
interface BitOr    { fn bitor(self, other: Self): Self }
interface BitXor   { fn bitxor(self, other: Self): Self }
interface Neg      { fn neg(self): Self }
interface Not      { fn not(self): Self }

interface PartialEq {
    fn eq(self, other: Self): bool
    fn ne(self, other: Self): bool      // default: !self.eq(other)
}

interface PartialOrd {
    fn lt(self, other: Self): bool
    fn le(self, other: Self): bool      // default: !self.gt(other)
    fn gt(self, other: Self): bool      // default: other.lt(self)
    fn ge(self, other: Self): bool      // default: !self.lt(other)
}

interface Index<T>    { fn index(self, i: usize): &T }
interface IndexMut<T> { fn index_mut(mut self, i: usize): &mut T }
```

The built-in operators and the `Eq`/`Ord` derives always win over an implementation, both operands have the same
type, and `&&`, `||`, `=`, compound assignment, and the shifts are not overloadable. `Index` names the element type
and returns a reference; `IndexMut` is selected when the indexed place is written. The exact resolution, the
diagnostics, and the ownership rules are in [`operators.md`](operators.md).

## `String` and `str`

A string literal has type `str`. `str` is a copied, second-class view; `String` is the owning type.

```
struct String {
    fn new(): String                  // associated function: String.new()
    fn from(s: str): String           // associated function: String.from(s); copies
    fn push_str(mut self, s: str)
    fn push(mut self, c: char)
    fn len(self): usize               // bytes, not characters
    fn starts_with(self, prefix: str): bool
    fn find(self, c: char): Option<usize>        // byte offset of the first occurrence
    fn find_str(self, needle: str): Option<usize> // byte offset of the first occurrence
    fn as_str(self): str
    fn chars(self): Chars             // Unicode scalar values
    fn clone(self): String            // deep copy of the buffer
}
```

`str` has the same read methods (`len`, `starts_with`, `find`, `find_str`, `as_str`, `chars`, `to_string`), without
`push*`, `from`, or `clone`. `Chars` is only iterable: `for c in s.chars()`.

Slicing `s[a..b]` and `s[a..=b]` yields a `str` view of the bytes in the range; it works on `str`, `String`, arrays,
and slices. `a` and `b` are `usize`; a reversed or out-of-range range panics. `len` counts bytes, and a slice may
split a multi-byte character: the language does not check UTF-8 boundaries.

`==` and `!=` compare two `str` values or two `String` values byte for byte. `<`, `<=`, `>`, `>=` compare two `str`
values or two `String` values lexicographically by bytes. The operands must have the same type; `s == "x"` does not
convert, and there is no `Ord` for a mixed pair.

## Allocators

```
interface Allocator {
    fn allocate(mut self, size: usize, align: usize): CPtr<u8>
    fn reallocate(mut self, ptr: CPtr<u8>, old_size: usize, new_size: usize, align: usize): CPtr<u8>
    fn deallocate(mut self, ptr: CPtr<u8>, size: usize, align: usize)
}

struct Global { }                       // the default allocator; zero-sized, stored nowhere
struct Arena { ... }                    // a bump allocator with blocks, reset, and bytes_allocated
Shared<T>                               // a built-in reference-counted cell: Shared.new(value), get, strong_count
```

`Global` and `Arena` live in `std.alloc` and are visible through the prelude; they implement `Allocator`.
`List`, `Map`, and `Heap` take an allocator as their last type argument (`List<T, A = Global>`) and store it
when it is not `Global`; `new_in` is the constructor that takes the value (`List<T>.new_in(alloc)`,
`Map<K, V>.new_in(alloc)`, `Heap.new_in(alloc, value)`). `Arena.new()` makes an empty arena, `bytes_allocated()`
reports the rounded bytes handed out since the last reset, and `reset()` frees every block and leaves the arena
usable. `Shared<T>` is shared by `clone()`, read by `get(): &T`, counted by `strong_count(): usize`, and is not
`Copy`. The rules are [`allocators.md`](allocators.md) AL1-AL8.

## `List<T>`

```
struct List<T, A = Global> {
    fn new(): List<T, A>              // only when A is `Global`: the allocator-free constructor
    fn new_in(alloc: A): List<T, A>   // takes the allocator value to store (`allocators.md`, AL4)
    fn push(mut self, value: T)       // takes ownership of the value
    fn pop(mut self): Option<T>
    fn len(self): usize
    fn last(self): Option<&T>
    fn insert(mut self, index: usize, value: T)
    fn remove(mut self, index: usize): T
    fn clear(mut self)
    fn contains(self, value: T): bool // needs == on T
    fn sort(mut self)                 // ascending; needs < on T
    fn iter(self): [T]                // borrows: `T` if Copy, else `&T`
    fn into_iter(own self): ListIntoIter<T, A>   // consumes the list, carrying the allocator
    fn map<U, F: FnMut(&T): U>(self, f: F): List<U>
    fn filter<F: FnMut(&T): bool>(self, f: F): List<T>   // needs Clone on T
    fn fold<A, F: FnMut(A, &T): A>(self, init: A, f: F): A
    fn any<F: FnMut(&T): bool>(self, f: F): bool
    fn all<F: FnMut(&T): bool>(self, f: F): bool
    fn find<F: FnMut(&T): bool>(self, f: F): Option<T>   // needs Clone on T
}
```

Indexing `xs[i]` is a place: it reads or writes one element, takes a `usize`, and panics when out of bounds.
`for x in xs` borrows the list for the loop and yields read-only element places (`memory-rules.md`, IT). `T` must be
storable, so `List<str>` and `List<&T>` are rejected (E0120). The last six methods are the eager iteration helpers:
they borrow the list, hand the closure a view of each element, and return owned results (`iterators.md`, IT10);
`.any` is resolved to the `std` function named `__list_any` and spelled with the reserved word. Every method
except `new` exists for every allocator argument; `List<T, A>.new()` where `A` is not `Global` is E0272 because
the constructor has no allocator value to store (`allocators.md`, AL4).

## `Map<K, V>`

```
struct Map<K, V, A = Global> {
    fn new(): Map<K, V, A>            // only when A is `Global`
    fn new_in(alloc: A): Map<K, V, A> // takes the allocator value to store
    fn insert(mut self, key: K, value: V)   // replaces an existing key's value
    fn get(self, key: K): Option<&V>
    fn remove(mut self, key: K): Option<V>
    fn contains_key(self, key: K): bool
    fn clear(mut self)
    fn len(self): usize
    fn iter(self): MapIter<K, V>          // yields (&K, &V)
    fn into_iter(own self): MapIntoIter<K, V, A>   // consumes the map, yielding (K, V)
}
```

Keys need `==` (`Eq`). `for (k, v) in map` borrows the map for the loop; iteration order is not specified, so a
program must not depend on it. The implementation is an open-addressing hash table: lookup, insert, and remove are
expected O(1), the table keeps a 3/4 load factor and rehashes as it grows, and removing a key shifts the rest of its
probe cluster back instead of leaving tombstones. Keys are hashed structurally — integers, `bool`, `char`, floats,
`String` by its bytes, tuples, structs, enums, `Option`, `Result`, and ranges — so a key needs only `==`, not
`Hash`.

## `Set<T>` (issue #190)

`std.set` is a thin layer over `Map<T, bool, A>`, so it follows the map's rules: an element needs only `==` (`Eq`),
lookup, insert, and remove are expected O(1), and the iteration order is not specified. Like `List` and `Map`, a set
takes an allocator as its last type argument and stores it (`allocators.md`).

```
struct Set<T: Eq, A: Allocator = Global> {
    fn new(): Set<T>                               // only when A is `Global`
    fn new_in(own alloc: A): Set<T, A>
    fn from_list(own items: List<T>): Set<T>       // a repeated element is stored once
    fn len(self): usize
    fn is_empty(self): bool
    fn contains(self, value: T): bool
    fn insert(mut self, own value: T): bool        // false (and the new value dropped) when already present
    fn remove(mut self, value: T): bool
    fn clear(mut self)
    fn extend(mut self, own items: List<T>)
    fn into_list(own self): List<T>                // consumes the set, in no particular order
    fn iter(self): SetIter<MapEntries<T, bool>, T> // yields &T; `for x in set` borrows the set
}

// with `T: Clone`, the default allocator, and the result uses the default allocator
fn to_list(self): List<T>
fn clone(self): Set<T>
fn union(self, other: Set<T>): Set<T>
fn intersection(self, other: Set<T>): Set<T>
fn difference(self, other: Set<T>): Set<T>             // in `self` and not in `other`
fn symmetric_difference(self, other: Set<T>): Set<T>   // in exactly one of the two

fn is_subset(self, other: Set<T>): bool
fn is_superset(self, other: Set<T>): bool
fn is_disjoint(self, other: Set<T>): bool
```

`Set<T>` implements `PartialEq` (same elements, regardless of order), `Debug` (`{1, 2, 3}`, in iteration order), and
`Iterable<&T>`. `MapEntries<K, V>` is the nameable type of the iterator `map.iter()` returns, added so a library type
can wrap it. Operations that copy elements need `Clone`; they return a set that uses the default allocator.

Tests: `m24_stdlib/set_basic`, `set_algebra`, `set_iteration`, `set_strings_and_drops`, `set_needs_eq`.

## `Deque<T>` (issue #191)

`std.deque` is a double-ended queue made of two lists: `front` holds the first elements in reverse order and `back`
holds the rest, so a push at either end is one `List.push`. A pop from an end whose list is empty first moves the other
list over, which makes a run of pops from one end amortized O(1); a program that alternates `pop_front` and `pop_back`
on a deque held in one half pays O(n) for each switch. `List` is unchanged, and `Deque` always uses the default
allocator.

```
struct Deque<T> {
    fn new(): Deque<T>
    fn from_list(own items: List<T>): Deque<T>
    fn len(self): usize
    fn is_empty(self): bool
    fn push_front(mut self, own value: T)
    fn push_back(mut self, own value: T)
    fn pop_front(mut self): Option<T>
    fn pop_back(mut self): Option<T>
    fn front(self): Option<&T>
    fn back(self): Option<&T>
    fn get(self, i: usize): Option<&T>
    fn clear(mut self)
    fn extend(mut self, own items: List<T>)        // at the back, in order
    fn into_list(own self): List<T>                 // front to back
    fn iter(self): DequeIter<&Deque<T>, T>          // yields &T from the front to the back
    fn clone(self): Deque<T>                        // T: Clone
}
```

`Deque<T>` implements `Index<T>` and `IndexMut<T>` (`d[i]` counts from the front and panics with `Deque index out of
bounds` past the end), `PartialEq` (same length and equal elements), `Debug` (`[1, 2, 3]`), and `Iterable<&T>`.
`DequeIter<D, T>` is generic over the view of the deque it walks and needs `D: Index<T> + DequeLen`; for that, a
shared view `&X` satisfies an interface bound that `X` satisfies.

Tests: `m24_stdlib/deque_basic`, `deque_index_iteration`, `deque_strings_and_drops`, `deque_index_out_of_bounds`.

## `PriorityHeap<T>` (issue #192)

`std.heap` is a priority queue ordered by `Ord`. The elements are kept in ascending order in one list, so the greatest
element is the last one: `peek` and `pop` are O(1), `push` finds its place with a binary search (O(log n)
comparisons) and moves the elements above it, and iterating in priority order is walking the list backwards. It is
not a binary heap: swapping two elements of one list needs a placeholder value the language does not have, and the
sorted list gives iteration in priority order for free. Elements that compare equal come out in the order they were
pushed. The element type only needs an `Ord` bound, which a type with a `PartialOrd` impl also satisfies.

```
struct PriorityHeap<T: Ord> {
    fn new(): PriorityHeap<T>
    fn from_list(own items: List<T>): PriorityHeap<T>
    fn len(self): usize
    fn is_empty(self): bool
    fn push(mut self, own value: T)
    fn extend(mut self, own items: List<T>)
    fn pop(mut self): Option<T>                       // the greatest element
    fn peek(self): Option<&T>
    fn clear(mut self)
    fn into_sorted_list(own self): List<T>            // highest priority first
    fn iter(self): PriorityHeapIter<&PriorityHeap<T>, T>   // yields &T, highest priority first
    fn clone(self): PriorityHeap<T>                   // T: Clone
}
```

`PriorityHeap<T>` implements `Index<T>` (`h[0]` is the element `pop` returns next; past the end it panics with
`PriorityHeap index out of bounds`), `Debug` (`[9, 5, 1]`, highest priority first), and `Iterable<&T>`. To get a
min-heap, wrap the elements in `Reverse<T>`.

Tests: `m24_stdlib/heap_basic`, `heap_iteration`, `heap_equal_priorities`, `heap_min_heap`, `heap_drops`, `heap_index_out_of_bounds`,
`heap_needs_ord`.

## `Heap<T>`

```
struct Heap<T, A = Global> {
    fn new(value: T): Heap<T, A>         // only when A is `Global`; moves the value in
    fn new_in(alloc: A, value: T): Heap<T, A>   // takes the allocator value to store
}
```

`Heap<T>` owns one allocation. `*h` on an owned `Heap<T>` moves the whole pointee out and frees the allocation;
reading or assigning through `*h` does not move. A field of the pointee (`(*h).a`) is a partial move tracked like a
field of a local, and dropping the `Heap` drops the remaining fields and frees the allocation (`memory-rules.md`,
MV6).

## `Range<T>` and `RangeInclusive<T>`

```
let r: Range<i32> = 0..4
let q: RangeInclusive<i32> = 0..=4
```

Both are built-in first-class types. `a..b` builds the exclusive form and `a..=b` the inclusive one; the endpoints
must be integers, with the type coming from context or defaulting to `i32`. A range is `Copy`, `Clone`, `Eq`,
`Ord`, and `Hash`, may be stored in a field or collection, and is iterable by `for i in r` (which evaluates `r`
once). `r.iter()` and `r.into_iter()` give a `RangeIter<T>` that yields the endpoints in order; the inclusive form
stops after the upper endpoint without computing `upper + 1`, so it does not overflow (`iterators.md`, IT11).

## Iteration

`std` declares `Iterator<T>` (`next`, plus the default methods `map`, `filter`, `take`, `skip`, `fold`, `count`,
`any_item`, `all`, `find`, and `to_list`) and the marker `Iterable<T>`. The compiler implements both for arrays,
slices, `List`, `Map`, `Chars`, and ranges, and adds `sum` for iterators of integers; `std` declares the adapter
types `MapIter`, `FilterIter`, `TakeIter`, and `SkipIter`. A program implements the interfaces to make its own type
work with `for` and the adapters. The rules, the item types, and the second-class restrictions are in
[`iterators.md`](iterators.md).

## `Ordering`, `cmp`, and `Reverse<T>` (issue #705)

`std.cmp` declares the three-way comparison. The prelude makes its names visible in every module, and `cmp.min(a, b)`
names the module.

```
enum Ordering derives (Copy, Clone, Eq, Ord, Hash, Debug) { Less  Equal  Greater }

fn Ordering.reverse(self): Ordering
fn Ordering.then(self, other: Ordering): Ordering
fn Ordering.then_with<F: FnOnce(): Ordering>(self, f: F): Ordering
fn Ordering.is_lt(self): bool      is_le   is_eq   is_ne   is_gt   is_ge

fn cmp.min<T: Ord>(a: T, b: T): T                      // `a` when they are equal
fn cmp.max<T: Ord>(a: T, b: T): T                      // `b` when they are equal
fn cmp.clamp<T: Ord>(value: T, low: T, high: T): T     // panics when `low > high`
fn cmp.min_by<T, F: FnMut(&T, &T): Ordering>(a: T, b: T, compare: F): T
fn cmp.max_by<T, F: FnMut(&T, &T): Ordering>(a: T, b: T, compare: F): T
fn cmp.min_by_key<T, K: Ord, F: FnMut(&T): K>(a: T, b: T, key: F): T
fn cmp.max_by_key<T, K: Ord, F: FnMut(&T): K>(a: T, b: T, key: F): T

struct Reverse<T> derives (Copy, Clone, Eq, Debug) { value: T }   // `<` is `T`'s `>`; `T: Ord` through `PartialOrd`
```

`a.cmp(b): Ordering` is provided by the compiler for every type that satisfies `Ord`: the integers, `char`, `bool`,
`str`, `String`, `()`, tuples and ranges of ordered parts, a struct or enum that derives `Ord`, a type parameter with
an `Ord` bound, and `Reverse<T>`. It calls the private `std.cmp.__ord_cmp`, which is `if a < b { Less } else if b < a
{ Greater } else { Equal }`. `Option<T>` and `Result<T, E>` get `cmp` from extension methods in `std.cmp` (`None`
before every `Some`; every `Ok` before every `Err`; contents compared when the kinds match): they are not `Ord`, so
`<` still does not apply to them. Floats have `<` but no total order, so `f64.cmp` is E0002 (`total_cmp` is the float
order, issue #703). The decisions behind it:

- **`cmp` is not an interface method.** `PartialOrd` keeps `lt`/`le`/`gt`/`ge` and `Ord` stays the compiler-known
  bound, so a derive generates nothing new: `cmp` is `<` read twice. `Ord` and `Eq` remain independent derives.
- **A hand-written `PartialOrd` satisfies an `Ord` bound.** A type that implements `PartialOrd` (its `lt` is the
  order) is accepted wherever `T: Ord` is required, which is how `Reverse<T>` works with `cmp.min`, `max_by_key`, and
  later `PriorityHeap` and `sort_by_key`. The built-in `<` does not apply to such a type (`has_ord` is false), so a
  generic body instantiated for it calls `lt`; the compiler-implemented `List.sort` still wants a derived `Ord`
  (E0141). Hand-written `Eq`/`Ord`/`Hash` beyond this stay E9000 under #402.
- A closure comparator or key function sees views of the values (`&T`), so the helpers return an owned `T`: `min`
  and friends cannot be called on two `str` views (E0121, a view cannot come from two parameters); use `String`.

Tests: `m24_stdlib/ordering_methods`, `cmp_builtin_types`, `cmp_option_result`, `cmp_generic`, `cmp_functions`,
`reverse`, `cmp_float_has_no_cmp`, `cmp_needs_ord`.

## `Option<T>` and `Result<T, E>`

`Option<T>` is built in with the variants `Some(T)` and `None`; `None` takes its type from context. `Result<T, E>`
is built in with `Ok(T)` and `Err(E)`. `Result<T>` written with one argument means `Result<T, Error>`, the dynamic
error type described below (`errors.md`, ERR5).

```
Option<T>:  unwrap(), expect(msg: str): T, is_some(): bool, is_none(): bool
Result<T, E>: unwrap(), expect(msg: str): T, is_ok(): bool, is_err(): bool
Result<T, E>: context(message: str): Result<T, Error>
```

`unwrap` panics on `None`/`Err`; `expect(msg)` panics with `msg`. `expr?` returns early through the enclosing
`Option` or `Result` function (E0160 outside one) and converts the error through a user `Type: From<Source>` block;
in a function returning `Result<U>` / `Result<U, Error>`, any error type is boxed into the dynamic `Error`
(`errors.md`, ERR4). `expr orelse fallback` supplies a value for `None` or `Err`; a closure handler receives the
`Err` payload (`|e| ...`) and takes no argument for `None` (`|| ...`, issue #488). `match` handles both types; `Option` also supports `==`/`!=` when its
payload does, and `Hash` when its payload does. `.context(message)` prefixes a context message and returns
`Result<T, Error>`; it is E0212 when the error type does not implement `Error` (`errors.md`, ERR6). The combinators of the next section are
stage 1 (issue #481).

### `Option` and `Result` combinators (issue #481)

`std.option` declares them as extension methods (`extensions.md`), which the prelude makes visible in every module;
the methods the compiler provides (`unwrap`, `expect`, `is_some`, `is_none`, `is_ok`, `is_err`, `context`) win.
`own self` consumes the value, so `Option<String>.map(...)` moves the payload into the closure; `is_some_and`,
`is_none_or`, `is_ok_and`, and `is_err_and` borrow the receiver and give the closure a view (`&T`); `take` and
`replace` change the receiver in place.

```
Option<T>:     map, and_then, or_else, or, and, xor, zip, unwrap_or, unwrap_or_else, map_or, map_or_else,
               ok_or, ok_or_else, filter, is_some_and, is_none_or, take, replace
Result<T, E>:  map, map_err, and_then, or_else, unwrap_or, unwrap_or_else, map_or, ok, err, unwrap_err,
               is_ok_and, is_err_and
```

The error-conversion helpers are `map_err` (apply a conversion to the error), `ok_or`/`ok_or_else` (an absent value
becomes an error), `ok`/`err` (drop one side), and the `?` operator, which keeps converting through `From`. The two
`unwrap_or_default` methods, `Option.flatten`, and `as_ref` are not provided: `T.default()` does not exist for the
built-in types, an extension receiver may not nest a type parameter (`Option<Option<T>>`), and a view returned from
an extension would dangle. A closure whose result mentions a type parameter that only the closure fixes
(`and_then(|x| Some(x))`, `F: FnOnce(T): Option<U>`) is typed from its body; a body that cannot say every part of the
type by itself (`or_else(|e| Err(e.len()))`, which leaves the `Ok` type open) needs a named function or a typed
binding.

Tests: `m24_stdlib/option_combinators`, `option_take_replace`, `result_combinators`, `unwrap_err_on_ok`,
`option_wrong_default`, `m12_closures/closure_returns_generic_wrapper`.

### `List` convenience methods (issue #483)

`std.list` declares these as extension methods on `List<T>` (a list with the default allocator; `extensions.md`), so
the prelude makes them visible everywhere and the compiler's own methods (`push`, `pop`, `sort`, `contains`, `map`,
`filter`, `find`, ...) win. They move elements with `pop`/`push`, so nothing is cloned unless the name says so, and
every element is dropped exactly once.

```
fn List<T>.is_empty(self): bool
fn List<T>.retain<F: FnMut(&T): bool>(mut self, f: F)      // keeps the accepted elements, in order
fn List<T>.position<F: FnMut(&T): bool>(self, f: F): Option<usize>
fn List<T>.rposition<F: FnMut(&T): bool>(self, f: F): Option<usize>
fn List<T>.reverse(mut self)
fn List<T>.dedup(mut self) where T: Eq                      // removes consecutive duplicates
fn List<T>.extend(mut self, own other: List<T>)             // moves the elements of `other`
fn List<T>.extend_from(mut self, other: List<T>) where T: Clone
fn List<T>.resize(mut self, new_len: usize, own value: T) where T: Clone
fn List<T>.truncate(mut self, len: usize)
fn List<T>.swap(mut self, a: usize, b: usize)              // panics when an index is out of bounds
```

`find` is the eager helper of `iterators.md` IT10 (it clones the element). `dedup_by_key`, `drain`, and `first` are
not provided: a key function would need its result type fixed by a bound the checker cannot yet match against a type
parameter, and views returned from an extension would dangle.

Tests: `m24_stdlib/list_retain_reverse`, `list_dedup`, `list_position`, `list_extend_resize`, `list_drops`,
`list_swap_out_of_bounds`.

## Errors

`std` declares the marker interface `Error requires Display { }` and, under the same name in the type namespace,
the dynamic error value:

```
struct Error {
    value: any Error
    frames: List<TraceFrame>
    context: String
}

struct TraceFrame {
    file: String
    line: i32
    col: i32
}
```

A type is an error type when it implements `Error`, which requires `Display`. The dynamic `Error` is an owned
`any Error` box together with the context messages `.context` added (outermost first) and the frames a debug build
records. An error type is boxed implicitly wherever `Error` is expected; `e.is<T>()` reports whether the box holds
a `T`, `e.downcast<T>()` consumes the box and yields `Some(value)` on a match (both need `T: Error`), and `{}`
prints the wrapped error's `Display` (`errors.md`, ERR4). `TraceFrame` implements `Display` as `file:line:col`, and
`error.frames` is its public `List<TraceFrame>`; a debug build records a creation frame at each conversion into
`Error` and a propagation frame at each `?` that passes a `Result<..., Error>` outward, and a release build records
none (`errors.md`, ERR7).

`std` declares its error types (`errors.md`, ERR10):

```
error enum IoError {
    Read { path: String }    "cannot read {path}"
    Write { path: String }   "cannot write {path}"
    Run { command: String }  "cannot run {command}"
}

error enum ParseIntError {
    Invalid  "invalid integer"
}
```

`read_to_string` returns `IoError.Read` on failure, `write_file` returns `IoError.Write`, `process.run` returns
`IoError.Run`, and an integer `parse` returns `ParseIntError.Invalid`.

An `error enum` declaration lowers to an ordinary enum plus generated `Display` (from each variant's message
template), `Error`, and `From<Source>` impls, so an error value is an ordinary value (`errors.md`, ERR1-ERR3). A
function written `fn main(): Result<T>` prints an `Err` with `Display` to standard error and exits 1; `Ok` prints
nothing and exits 0 (`errors.md`, ERR8).

## Files, processes, and environment

```
fn read_to_string(path: str): Result<String, IoError>
fn write_file(path: str, text: str): Result<(), IoError>
fn env.args(): List<String>                 // program name first
fn process.exit(code: i32): !               // ends the process immediately
fn process.run(command: str): Result<i32, IoError>
```

`read_to_string` and `write_file` read and write raw bytes: there is no newline translation on Windows and no UTF-8
normalization, so a string written and read back is byte-identical (`len` is the byte count). `process.run` runs the
command through the system shell and returns its status: the POSIX wait status is shifted, the Windows status is
returned as is. On failure each returns the matching `IoError` variant, and an integer `parse` returns
`ParseIntError.Invalid` (`errors.md`, ERR10).

None of these runs at compile time: a comptime evaluation that reaches one fails with E0224 (`comptime.md`, CT9).

## The compilation target

```
enum target.Os derives (Copy, Clone, Eq) { Linux  MacOs  Windows }
enum target.Arch derives (Copy, Clone, Eq) { X86_64  Aarch64 }
fn target.os(): target.Os                   // the operating system the program is compiled for
fn target.arch(): target.Arch               // the processor architecture the program is compiled for
```

The compiler builds for the machine it runs on, so both are the host's. They are ordinary functions, usable at run
time and at compile time, which is how `comptime if target.os() == target.Os.Windows { ... }` selects a platform
branch (`comptime.md`, CT5). `tests/m20_comptime/comptime_if_platform.bd`, `comptime_target_values.bd`.

## Memory helpers and assertions

```
fn mem.swap(mut a: T, mut b: T)
fn mem.replace(mut place: T, own value: T): T   // moves the value in; returns the old one
fn panic(msg: str): !
fn assert(cond: bool)
fn assert_eq<T: Eq>(a: T, b: T)
fn unreachable(): !
```

`swap` and `replace` take places and are checked for exclusivity like any other call. `panic` prints its message and
location and aborts (`memory-rules.md`, DR10); `unreachable()` panics with `unreachable`; `assert` panics with
`assertion failed`; `assert_eq` panics when the values differ. In a `burdin test` build the prelude replaces
`assert`/`assert_eq` with `__test_assert`/`__test_assert_eq`, which return `Result<(), AssertError>` instead of
panicking: a failure is `AssertError.Failed { condition, location }` or
`AssertError.FailedEq { condition, left, right, location }`, where `condition` is the call's source text, `left`
and `right` are its values formatted with `Debug`, and `location` is its `path:line:col` (`errors.md`, ERR9). A
normal build keeps the panicking assertions.

## C bridge

```
struct CPtr<T>                       // a raw, Copy pointer; no drop
fn CPtr.null(): CPtr<T>              // associated function; no arguments; type from the expected type
fn CPtr.is_null(self): bool
type CChar = i8
type CSize = usize
type CVoid = ()                      // only ever behind a CPtr

struct CString {
    fn from(s: str): CString         // copies the bytes and appends a NUL
    fn as_ptr(self): CPtr<CChar>
}
type CStr = CPtr<CChar>              // a std alias
fn CStr.to_string(ptr: CStr): String // reads the NUL-terminated bytes; associated function
```

`CPtr<T>` supports `null`, assignment, copying, `==`/`!=`, `.is_null()`, element offsets (`p + n`, `p - n`,
`p - q` as a count of elements), dereference (`*p` as a place to read or write), and casts between `CPtr` types and
integers. Creating, holding, reading, passing, or moving a `CPtr` value requires an `unsafe` context (E0161,
`memory-rules.md` US); a field of a `repr(C)` struct behind a `CPtr` is read and written through `*p` and field
selection. A `CPtr` does not distinguish `const`: writing through the pointer is an `unsafe` operation and is allowed
whatever the parameter convention that produced the pointer. Stage 0 has no function-pointer types and no `union`
declaration: a C function pointer can only be stored and passed as `CPtr<CVoid>` until stage 1 adds a callable type
for the bindgen target, and a union is handled through a `repr(C)` struct over the same bytes or a `CPtr<u8>`.
`CString` owns its buffer, has `Drop`, and `clone` deep-copies the NUL-terminated buffer.

**`CStr` is an alias, not a view type (issue #83).** `CStr` is a `std` type alias for `CPtr<CChar>`: one type
under two names, with no length, no origin, and no conversion rules of its own. A `CPtr<T>` is a plain
first-class `Copy` value (`memory-rules.md`, RF4 and SC1), so a `CStr` may be a local, a parameter, a return
value, a struct field, or a type argument, and no lifetime rule applies to it, unlike `str` and `[T]`.

- There is no implicit conversion between text and a C string in either direction: a `str`, `String`, or
  `CString` is not a `CStr` (E0003), and a `CStr` is not a `str` or a `String` (E0003). `CString.from(s)`
  copies the bytes and appends the NUL, `.as_ptr()` gives the `CStr`, and `CStr.to_string(p)` copies the
  NUL-terminated bytes back. The `CString` must stay alive while its pointer is used; the compiler does not
  track that lifetime, because a raw pointer is not a view.
- `CStr.to_string` is the associated function `to_string` of `CPtr<CChar>`, not a method: `CStr.to_string(p)`
  is the spelling, and `p.to_string()` is E0002, as are `p.len()` and every other string method.
- Every operation on a `CStr` value is a raw-pointer operation and requires `unsafe` (E0161, US); the alias
  adds no safety. A `CStr` promises neither NUL termination nor validity.

The alternative, a second-class `CStr` view with its own conversions, was rejected: it would add a second
implicit conversion (IC1 admits only `String` to `str`), need its own origin and lifetime rules, and present a
C string as a safe borrowed string whose termination and lifetime the compiler cannot check. The alias keeps
the C bridge honest: unvalidated bytes are a pointer (`CStr`), and `CString` is the owning buffer.

`extern "C" { fn name(params): ret }` declares a C function. Parameters are passed by value and may be integers,
floats, or `CPtr<T>`; the result may be one of those or unit. Aggregates cross the ABI through `CPtr<T>` only: a
struct declared `repr(C)` has C field order and alignment (`Type.size()` and `Type.align()` report them), but passing
or returning one by value is E0162 because the platform by-value coercions are not implemented. A Burdin `bool` is a
one-bit value while C `_Bool` is a one-byte integer, so use `u8` (0 or 1); C `char` is `CChar`. `mut`, `own`, `self`,
generics, other ABIs, and a `main` or `burdin_*` symbol are rejected (E0162 for a type or convention, E0001 for the
syntax of an unsupported ABI). A symbol is declared once: a second declaration with the same signature reuses it, a
different signature is an error. The extra arguments of a variadic declaration (`...`) are each checked against the
C ABI; inside the body they are passed as written.

`extern "C" fn name(params): ret { ... }` exports a Burdin function under its plain C symbol, so C code (or another
module that declares it `extern "C"`) can call it. The signature follows the same rules as a declaration: ABI "C",
`own` parameters, no generics, no `...`, no receiver, and only the C-ABI types above. A panic does not unwind across
the boundary; it aborts like any other panic (`memory-rules.md`, DR10).

## Tests

| Area | Tests |
| --- | --- |
| Output, numbers, parsing | `m02_basics/arithmetic`, `casts_and_numeric_methods`, `m09_stdlib/string_ops` |
| Operators and `Index` | `m19_operators/*` (`operators.md`) |
| `String`, `str`, slicing, ordering | `m09_stdlib/string_ops`, `string_order`, `slice_inclusive`, `slice_inclusive_bounds`, `m06_conventions/str_views`, `slice_views` |
| `List` | `m09_stdlib/list_ops`, `list_of_drops`, `m06_conventions/for_borrows_list` |
| `Map` | `m09_stdlib/map_ops`, `map_key_types`, `map_remove_and_clear`, `map_many_keys`, `word_count` |
| Iteration | `m13_iterators/*` (`iterators.md`) |
| `Ordering`, `cmp`, `Reverse` | `m24_stdlib/ordering_methods`, `cmp_builtin_types`, `cmp_option_result`, `cmp_generic`, `cmp_functions`, `reverse` |
| `Heap` | `m05_ownership/heap_tree_eval` |
| `Option`, `Result`, `?`, `orelse` | `m09_stdlib/option_result_basics`, `question_and_from`, `m16_errors/dynamic_result`, `orelse_closure` |
| `PriorityHeap` | `m24_stdlib/heap_basic`, `heap_iteration`, `heap_equal_priorities`, `heap_min_heap`, `heap_drops`, `heap_index_out_of_bounds`, `heap_needs_ord` |
| `Deque` | `m24_stdlib/deque_basic`, `deque_index_iteration`, `deque_strings_and_drops`, `deque_index_out_of_bounds` |
| `Set` | `m24_stdlib/set_basic`, `set_algebra`, `set_iteration`, `set_strings_and_drops`, `set_needs_eq` |
| `List` convenience methods | `m24_stdlib/list_retain_reverse`, `list_dedup`, `list_position`, `list_extend_resize`, `list_drops` |
| `Option` and `Result` combinators | `m24_stdlib/option_combinators`, `option_take_replace`, `result_combinators` |
| Files, processes, environment | `m09_stdlib/file_io`, `file_io_binary`, `env_args`, `exit_code`, `process_run`, `m16_errors/std_errors` |
| Memory helpers and assertions | `m09_stdlib/std_helpers`, `unreachable_aborts`, `m10_test_runner/*`, `m16_errors/assert_typed` |
| Error declarations and conversions | `m16_errors/error_keyword`, `error_display`, `error_message_specs`, `error_bad_template`, `error_bad_from`, `error_duplicate_from`, `error_from_conversion`, `error_missing_conversion`, `error_not_display` |
| Dynamic errors, `main`, assertions | `m16_errors/dynamic_error_downcast`, `error_context_chain`, `error_context_non_error`, `traces_debug`, `main_result_ok`, `main_result_err`, `main_result_typed`, `main_result_alias`, `assert_typed`, `std_errors` |
| C bridge | `m08_modules/extern_libc`, `cstr_roundtrip`, `cptr_operations`, `cptr_null_arguments`, `c_out_parameter`, `c_abi_structs`, `cstring_clone`, `cstr_is_an_alias`, `export_c_abi`, `m11_llvm/llvm_return_42` |
| Rejected names, arities, and C ABI types | `m09_stdlib/unknown_stdlib_names` (E0002, E0004), `m08_modules/cptr_null_arguments` (E0004), `extern_signature` (E0162), `export_c_abi_invalid` (E0162), `cstr_no_implicit_conversion` (E0003), `cstr_is_not_a_str` (E0002, E0003) |
