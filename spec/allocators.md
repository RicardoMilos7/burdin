# Allocators, arenas, and shared values

Milestone 23 settles where a container's memory comes from, what an allocator parameter is, how a container
stores one, and what `Shared<T>` means. This page is the exact behavior; the tests are `tests/m23_allocators/`,
the leak fixture is `tests/runner/leaks/`, and the runtime side is checked by `tests/check_test_runner.py`. Rule
IDs are AL1-AL10.

```
List<i32>                       // the default allocator, stored nowhere: the stage 0 layout
List<i32, Arena>                // a list that stores the arena value it allocates through
struct Buffer<A: Allocator = Global> { items: List<i32, A> }
fn build<A: Allocator>(own a: A): List<i32, A> { List<i32>.new_in(a) }
Shared.new(value)               // an owned reference-counted cell; .clone() shares it
```

## AL1. The model

Every buffer a container owns comes from an allocator value. There are two cases. With the default allocator,
`Global`, the type carries no allocator argument and no value is stored: `List<T>` is spelled and laid out as it
was in stage 0. With any other allocator, the container's type names it (`List<T, Arena>`) and the container
stores the allocator value as a field and owns it. There is no global allocation policy, no garbage collector,
and no hidden allocator: the allocator a buffer came from is the allocator that frees it, and it travels with
the container.

Allocator values are ordinary Burdin values. A container owns its allocator the way it owns its elements, so an
arena stored in a list lives exactly as long as that list and is freed when the list is dropped.

## AL2. The `Allocator` interface

`std.alloc` declares the interface and the two implementations the language provides:

```
interface Allocator {
    fn allocate(mut self, size: usize, align: usize): CPtr<u8>
    fn reallocate(mut self, ptr: CPtr<u8>, old_size: usize, new_size: usize, align: usize): CPtr<u8>
    fn deallocate(mut self, ptr: CPtr<u8>, size: usize, align: usize)
}
```

The methods take `mut self` because an allocator has state: a bump allocator advances a pointer, and a pool
keeps free lists. The compiler calls them on the container's stored value, so the calls mutate the stored
allocator in place.

* `size` is at least one byte: a container asks for at least one byte even for a zero-sized element. `align` is a
  power of two from 1 to 8. That is the most the compiler asks an allocator for, which is deliberately lower than the
  alignment some types have: `i128` and `u128` are 16-aligned (`int128.md`, I128-1), but a container's storage for
  them is requested at 8 until #917 decides whether the cap is raised.
* `allocate` returns a block of at least `size` bytes, aligned to `align`, for the caller to write. The contents
  are unspecified; nothing is zeroed.
* `reallocate` returns a block of at least `new_size` bytes, copies `min(old_size, new_size)` bytes from `ptr`
  into it, and releases the old block. `old_size` and `align` are those of the old block.
* `deallocate` receives exactly the pointer, size, and alignment of a live block from the same allocator. Freeing
  a block twice, or with the wrong values, is a bug; in a test build the checking allocator catches it (AL9).

`Global` is a zero-sized struct in `std.alloc`. Its methods call the compiler's unsafe primitives `__try_alloc` and
`__try_realloc`, which return null when the heap refuses (AL11), together with `__dealloc`; `__alloc` and
`__realloc` are the panicking forms. They reach the C heap (`malloc`/`realloc`/`free`) in an ordinary build and the
checking allocator in a `burdin test` build. A program may implement `Allocator` for its own type with an
ordinary impl block; the compiler does not verify the pointer arithmetic inside it, which is why those bodies
use `unsafe`. A user allocator that reaches the OS heap by its own `extern` calls is outside the checking
allocator's tracking.

## AL3. Allocator-aware container types

The built-in containers take the allocator as their last type argument: `List<T, A>`, `Map<K, V, A>`, and
`Heap<T, A>`. `A` must implement `Allocator` at every use — a written annotation, an inferred instance, a field
type, a signature, or a type argument of a struct (`E0270` otherwise). `A` defaults to `Global`, so the stage 0
spellings `List<T>`, `Map<K, V>`, and `Heap<T>` mean `List<T, Global>`, `Map<K, V, Global>`, and
`Heap<T, Global>` and are the same types (`AL5`).

`Global` is not stored. A `List<T>` keeps its three-word layout (pointer, length, capacity); a `List<T, Arena>`
appends the arena value as a fourth field, and a `Heap<T, Arena>` is the pair `{ptr, allocator}` instead of a
bare pointer. The allocator field is part of the value: moving the container moves the allocator, and the two
types are not interchangeable. A function that takes `List<i32>` does not accept `List<i32, Arena>` (a type
mismatch), and the reverse.

`Shared<T>` takes no allocator argument: its cell comes from `Global` (`AL8`).

## AL4. Allocator parameters and the constructors

An allocator parameter is an ordinary generic parameter with the `Allocator` bound: `A: Allocator` in a
declaration, or `own a: impl Allocator` as sugar for one hidden parameter. It is monomorphized per allocator
type as every generic parameter is, so `build(Arena.new())` and `build(Global { })` compile to separate
functions.

A container that stores an allocator is made with a constructor that takes the value:

| constructor | type |
| --- | --- |
| `List<T>.new_in(alloc)` | `List<T, A>` |
| `Map<K, V>.new_in(alloc)` | `Map<K, V, A>` |
| `Heap.new_in(alloc, value)` | `Heap<T, A>` |

The value is moved into the container, which owns it from then on. The allocator type argument may be written
on the receiver (`List<i32, Arena>.new_in(a)`) or left to inference from the argument or the expected type. The
value must implement `Allocator` (`E0270` at the argument otherwise) and must be storable: a view or a reference
is not, because the container stores the value (`E0120`).

`new` remains the allocator-free constructor and is available only for the default layout. Writing
`List<i32, Arena>.new()`, `Map<K, V, Arena>.new()`, or `Heap.new(value)` where a stored allocator is expected is
`E0272`, because there is no allocator value to store; it names the `new_in` form to use. For `Global` the
allocator argument stores nothing, so `List<i32, Global>.new()` is the same call as `List<i32>.new()`.

`from_iter`, `from`, and `collect` build with the default allocator in the same way, so they are `E0272` (or, for
`collect`, `E0141`) for a stored allocator; a container that stores one is filled with `extend_iter`
(`iterators.md` IT22).

`fn sum_with(own alloc: impl Allocator)` in `tests/m23_allocators/allocator_function_param.bd` passes the same
parameter to `new_in` and returns the list: the returned `List<i32, A>` carries the allocator value out of the
function.

## AL5. Default type parameters

A struct or enum may give a type parameter a default: `struct Buffer<A: Allocator = Global> { ... }`. The
default follows the parameter's bounds and is a type expression resolved where the type is declared, with the
declaring module's imports and the preceding parameters in scope. A use may omit trailing arguments that have
defaults: `Buffer` in a type position is `Buffer<Global>`, and `Buffer<Arena>` overrides it.

The rules and their diagnostics (`E0271`):

* only a struct's or enum's type parameters take a default: a function's parameters are inferred at each call,
  an interface's parameters are written by the impl, an alias is expanded where it is used, and a const
  parameter takes a value, not a type;
* a parameter without a default cannot follow one with a default, because a use fills the trailing ones;
* a struct or enum declaration may mix defaulted and non-defaulted parameters only with every default at the
  end: `struct Pair<A, B = i32>` is valid, `struct Bad<A = i32, B>` is `E0271`.

The default is filled at each use, and the bounds of every argument are checked there, defaults included. The
`default_type_param.bd` test declares `Buffer<A: Allocator = Global>`, writes `Buffer` with no arguments, and
shows that `Buffer` is `Buffer<Global>` while `Buffer<Arena>` stores an arena.

An impl block follows the same filling. An inherent or `Drop` block written with no type arguments covers every
instance of the type, defaulted parameters included: `Buffer { fn ... }` adds methods to every `Buffer<A>`, and
`Buffer: Drop` drops every instance. An impl block that writes some arguments fills the omitted trailing
defaults, so `List<T> { ... }` written on a generic type with an `A = Global` parameter would cover only
`List<T, Global>`; to cover every instance, write the parameter (`List<T, A>`) or no arguments.

## AL6. `Arena`

`std.alloc.Arena` is a bump allocator written in Burdin. `Arena.new()` makes an empty arena; the first
`allocate` gets a 4096-byte block (or the request, if larger). Each request is rounded up to a multiple of eight
and carved from the current block. When the block cannot hold the request, the current block is pushed onto
`blocks` and a new block of at least the request's size is allocated. `Arena` also offers
`bytes_allocated(): usize`, the number of rounded bytes it has handed out since it was created or last reset.

`reallocate` allocates a fresh block and copies the smaller of the two sizes; the old memory stays part of its
block. `deallocate` does nothing: arena memory is released in blocks, not individually. `reset` frees the
current block and every full block, clears the list, and leaves the arena usable and empty. `Arena: Drop` calls
`reset`, so dropping the last container that stores an arena frees everything the arena handed out.

An arena is a first-class ordinary value, not a second-class type: it can be created, moved, stored in a
container, and returned. There are no arena handles and no compile-time tracking of which buffers a reset
invalidates. Safety comes from ownership: every safe way to get an arena-backed buffer (a `List`, `Map`, or
`Heap` made with `new_in`) also takes the arena value, so the arena cannot be reset while that container lives.
Manual allocation returns a raw `CPtr` and is `unsafe`; a program that resets an arena and then reads a buffer it
handed out is using unsafe code incorrectly. `arena_reset.bd` exercises allocation, `bytes_allocated`, reset,
and reuse.

## AL7. Drop, clone, and iteration

Dropping a container with a stored allocator drops each element, frees the buffer through the allocator with the
buffer's exact size and alignment, and then drops the allocator value. `drop_through_allocator.bd` shows the
element order, and `tests/m23_allocators/list_into_iter_arena.bd` checks that the freeing goes through the
allocator. Moving the pointee out of an allocator-aware `Heap` frees the allocation through the heap's allocator
(`heap_arena_move_out.bd`). These are the ordinary drop and move rules of `spec/memory-rules.md` (DR1-DR7,
MV6); the allocator only supplies the function that frees.

`List<T, A>` and `Map<K, V, A>` are `Clone` when their elements and their allocator value are `Clone` — a
container with a non-`Clone` allocator cannot be cloned (`E0141` on the call). Cloning a container clones the
allocator value first, then allocates the new buffer through that clone and deep-clones the elements, so the
clone frees its buffer through its own allocator. `Heap<T, A>` likewise clones its allocator and its pointee.
`Global` is a zero-sized struct with no stored value, so the default-layout containers keep the clone rules
they had. `allocator_clone.bd` clones an arena-backed list through a `Clone` pool allocator.

The owning iterators carry the allocator. `List.into_iter()` on a `List<T, A>` yields a `ListIntoIter<T, A>`
that owns the buffer and the allocator value; `Map.into_iter()` yields a `MapIntoIter<K, V, A>`. Dropping such
an iterator before it is exhausted drops the remaining elements, frees the buffer through the allocator, and
drops the allocator. The borrowing iterators (`iter()`) borrow the container and never touch its allocator.
`list_into_iter_arena.bd` checks both the exhausted and the partial-iteration paths.

## AL8. `Shared<T>`

`Shared<T>` is the language's reference-counted owner: a handle to a heap cell `{strong: usize, value: T}`
allocated through `Global`.

* `Shared.new(value)` moves `value` into a new cell whose count is one.
* `s.clone()` (and a derived `Clone` on a struct that contains it) shares the cell: the count goes up and the
  handle points at the same value. A `Shared<T>` is `Clone` for every `T`, including a `T` that is not `Clone`,
  because no value is copied. It is never `Copy`: copying the handle without counting would free the cell twice.
* `s.get(): &T` borrows the value. The cell offers no exclusive access, so a shared value is read through
  `get()`; a type with interior mutability would add its own methods on top.
* `s.strong_count(): usize` is the number of live handles.
* Dropping a handle decrements the count. The last handle drops the value and frees the cell, so the value is
  dropped exactly once (`shared_drop_once.bd`) and a handle moved or cloned out of a function keeps the value
  alive past that scope (`shared_scope.bd`). A struct that derives `Clone` and holds a `Shared<T>` shares the
  cell when it is cloned (`shared_in_struct.bd`).
* The value of a `Shared<T>` is stored, so it must be storable: `Shared.new(&n)` is `E0120` (`shared_view.bd`).

The count was not atomic in stage 1. Milestone 2.3 made it atomic and added the `Send` marker and the sharing rules
that go with it (`spec/shared.md`, SH1-SH2): `clone` is a relaxed increment, a drop is a release decrement with an
acquire fence before the value is freed, and a `Shared<T>` is `Send` and `Sync` when `T` is both. A cycle of
`Shared` handles leaks, as an `Rc` cycle does; there are no weak handles yet. The cell is allocated with
`Global`, so a `Shared` cannot be arena-backed today.

## AL9. The checking allocator in `burdin test`

A `burdin test` build allocates through a checking allocator; `burdin build` and `burdin run` use the plain C
heap. The checking allocator prepends a 40-byte header to every block: the next live block, the requested size,
the pointer and length of the source site, and a magic value. `free` validates the header, unlinks the block,
and panics with `panic: invalid free (double free or a pointer the allocator did not give out)` on a mismatch.
The tracking applies to every allocation made through the compiler's allocation primitives, which includes
`Global`, `Arena`, `String`, `List`, `Map`, `Heap`, `Shared`, `any` boxes, and a user allocator that calls
`__alloc` (the `Leaky` fixture does). Reallocation is a fresh tracked block plus a tracked free of the old one,
and the new block records the site of the growing expression.

After a test returns success, the test binary asks the allocator for the blocks still live. If there are any, it
prints to standard error

```
test leak: <n> allocation(s) leaked, <bytes> byte(s)
  allocated at <path>:<line>:<column>
```

one `allocated at` line per block, and fails the test. `<path>:<line>:<column>` is the source location of the
expression that allocated the block, recorded by the compiler; a block allocated by compiler-generated glue
(drop or clone) has no expression and is reported as `<runtime>`. The bytes are the requested (unrounded) sizes
summed. A test that already failed keeps its own verdict and is not leak-checked, and a leak does not abort the
run: the remaining tests still execute. Each test runs in its own process, so one test's allocations cannot be
reported as a later test's leak. The leak report is `leak_detection.bd` in `tests/m23_allocators/` and the
`tests/runner/leaks/` fixture; `tests/check_test_runner.py` checks the exact report. A use-after-free is still
not detected, and an allocator that bypasses the compiler's primitives is outside the check.

## AL10. Allocation at compile time

The compile-time interpreter (`spec/const-eval.md`) models the containers on its own heap: a `List`, `Map`, or
`String` created with `new()` inside a `const` initializer is an ordinary interpreter value. Allocating with
an explicit allocator is an impure operation (`E0224`): `new_in`, `__alloc`, `__realloc`, and `__dealloc` reach
outside the evaluator, so a `const` initializer that calls them is rejected
(`const_allocation_is_impure.bd`). A `Shared<T>` is modeled as the value it holds, so a pure expression may
create, clone, and read one (`shared_const.bd`); the model is transient and a value whose type contains a
`Shared` is not materializable into the program (`E0221`).

## AL11. Fallible allocation

An allocation can be refused: the C heap has no block to give, or the requested count cannot be addressed at all.
The infallible operations panic on a refusal, and the `try_` operations report it as a value. Both share one rule
set, and the error type is `AllocError`.

- **AL11.1 (the error type).** `std` declares `AllocError` as an error enum with two variants, in this order:
  `CapacityOverflow` ("capacity overflow") and `OutOfMemory` ("out of memory"). `Eq` and `Debug` are derived, and
  `Display` prints the message. `CapacityOverflow` means the request names more elements, or more bytes, than a
  `usize` holds, or that the length plus the requested additional elements wraps. `OutOfMemory` means the request
  is addressable but the allocator had no block to give.
- **AL11.2 (the `Allocator` contract).** `allocate` and `reallocate` (AL2) return a null pointer when they have no
  block to give. `reallocate` then leaves the old block valid. `Global` gets its blocks from `__try_alloc` and
  `__try_realloc`, which return null where `__alloc` and `__realloc` would panic. `Arena.allocate` returns null, and
  changes nothing, when the request cannot be rounded to eight bytes or the block it needs cannot be had.
- **AL11.3 (infallible operations).** A container operation without `try_` panics with `out of memory` when its
  buffer is refused: the default heap through the runtime's allocation, and a stored allocator when its `allocate`
  or `reallocate` returns null (the program's `out of memory` panic, written to stderr, and an abort). An infallible
  operation never returns an error.
- **AL11.4 (the `try_` operations).** Each returns `Result` with `AllocError` as the error:
  - `List<T>.try_with_capacity(n)` and `List<T>.try_with_capacity_in(n, alloc)`: `Result<List<T>, AllocError>`.
    A failed call that took a stored allocator releases it.
  - `xs.try_reserve(n)` and `xs.try_reserve_exact(n)` on a `List`: `Result<(), AllocError>`. `try_reserve` grows the
    capacity the way `reserve` does, and `try_reserve_exact` asks for exactly `len + n`.
  - `Map<K, V>.try_with_capacity(n)` and `m.try_reserve(n)`, with the same shapes.
  - `String.try_with_capacity(n)` and `s.try_reserve(n)`.

  A `try_` operation changes the container only when it returns `Ok`. A failed reserve leaves the length, the
  elements, and the capacity as they were.
- **AL11.5 (capacity overflow).** For a list, the request overflows when `n` elements of `T` take more bytes than a
  `usize` holds. For a map, it overflows when `len + n` wraps, when the slot count passes the map limit of 2^62, or
  when the table's bytes do not fit. For a string, it overflows only when `len + n` wraps. Every other refusal is
  `OutOfMemory`.
- **AL11.6 (scope).** The fallible forms are the ones above. `Heap`, `Deque`, `Shared<T>`, and the other containers
  keep their infallible growth for now, and a program that needs them to survive out-of-memory must not use them
  for the allocation that might fail.
- **AL11.7 (compile time).** The compile-time interpreter (AL10) does not model capacity, so a `try_` operation
  in a `const` initializer succeeds, and the overflow and out-of-memory refusals are not reproduced there.

Tests: `m24_stdlib/try_alloc_list`, `try_alloc_out_of_memory`, `try_alloc_reserve`, `try_alloc_map_string`,
`try_alloc_arena`, `try_alloc_infallible_aborts`, `try_alloc_stored_aborts`, and `try_alloc_test_no_leak`.

## The error codes

`E0270` (not an allocator), `E0271` (invalid type parameter default), and `E0272` (a stored-allocator container
made with `new`) are the new diagnostics; `E0120` and `E0224` apply as they do elsewhere. The registry is
`tests/README.md` and the user-facing reference is `docs/error-codes.md`.
