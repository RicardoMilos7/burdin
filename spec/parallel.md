# Parallel iteration

Issue #971 adds a Rayon-style facility to the standard library: a few methods that turn a collection into a parallel
pipeline on the scheduler's own worker pool. It is `std.parallel` (`std/src/parallel.bd`), plain library code; the one
compiler rule it needs is the `Sync` closure of `spec/shared.md` SH4. The tests are `tests/s3_parallel/` and
`tests/s2_shared/closure_*`. Rule IDs are PR1-PR12.

```
let xs: List<i64> = ...
let total = xs.par_iter().fold(0, |sum, x| sum + *x, |a, b| a + b)
let evens = xs.par_iter().filter(|x| **x % 2 == 0).map(|x| *x).collect()
let longest = words.par_iter().map(|w| w.clone()).max_by_key(|w| w.len())
let any_bad = checks.par_iter().any(|c| c.is_bad())
xs.par_iter_mut().for_each(|x| { *x = *x * 2 })
let squares = par_range(0, 1000).map(|i| i * i).collect()
```

## PR1. The surface (decision)

A pipeline is a source, then adapters, then one consumer. No new syntax, nothing implicit: a pipeline is parallel only
because the program wrote `par_iter()` (or another source) and never because a `for` or an `Iterator` chain was
rewritten.

| Part | Names |
| --- | --- |
| Sources | `[T].par_iter()` and `List<T>.par_iter()` (items `&T`), `par_iter_mut()` (items `&mut T`, PR6), `List<T>.par_into_iter()` (items `T`, moved out), `par_range(start, end)` (items `i64`) |
| Adapters | `map`, `filter`, `filter_map`, `enumerate`, `take`, `skip` |
| Consumers | `for_each`, `count`, `fold`, `any`, `all` on every pipeline; `reduce`, `find`, `min_by`, `max_by`, `min_by_key`, `max_by_key`, `collect` on a pipeline of owned items (PR3) |

Differences from the first cut in the issue, each forced by the language as it is:

* **`any` is spelled `any`.** As on `Iterator` (`spec/iterators.md`, IT9), the method is called `any`. It was
  `any_item` until issue #933 let a keyword name a declared method.
* **`fold` takes three arguments**: `fold(init, op, combine)`. A chunk starts from a clone of `init` (`A: Clone`) and
  folds its items with `op(acc, item)`; the chunks' results are then combined in source order with `combine(a, b)`.
  `fold(|| 0, |sum, x| sum + x)` with two closures cannot combine two chunks when the accumulator and the item differ,
  and the checker cannot infer `A` from an unannotated identity closure, so the initial value is an argument.
* **`reduce(identity, op)`** takes an identity closure (`|| 0`) because there the closure's type is known (`T`). It is
  `fold` with `A = T` and `combine = op`.
* **Ranges.** A `Range` value does not expose its endpoints to library code (`spec/stdlib.md`, Slices), so the range
  source is a function, `par_range(start, end)`, over `i64`; `end <= start` is empty. Other integer types `map` it.
* **Arrays.** `[T; N]` is not a receiver of its own; write `xs[..].par_iter()` or keep the data in a `List<T>`.

The interfaces are `ParallelIterator<T>` (every pipeline), `ParallelValues<T>` (PR3), and `IndexedParallelIterator<T>`
(PR5). Their `__`-prefixed methods are the protocol between a source and a consumer, not part of the surface. Names do
not collide with the eager `List` methods (`iterators.md`, IT10): `map` and `filter` here are methods of the pipeline
interfaces, which only a pipeline value has.

Tests: `s3_parallel/ordered_basic`, `sources_adapters`.

## PR2. Order (decision)

* `collect` preserves source order, and so does `enumerate`'s numbering.
* `find` returns the lowest-index match, `min_by`/`min_by_key` the first of equal items, `max_by`/`max_by_key` the last,
  as `Iterator` does (`spec/iterators.md`).
* `for_each` promises no order, and `any`/`all` may stop early without visiting every item.
* `fold` and `reduce` promise only that an associative operation gives the sequential answer: the grouping is not
  promised. Chunks are combined left to right, so a merely associative (not commutative) operation is also right.

Tests: `s3_parallel/ordered_basic`, `sources_adapters`, `stress`.

## PR3. Owned items and views (decision)

`par_iter()` yields `&T`, a view. A view cannot be a list element (E0120) or outlive the call, so a consumer that keeps
items (`collect`, `find`, `reduce`, `min_*`, `max_*`) exists only on pipelines whose items are owned values:
`par_into_iter`, `par_range`, and anything after a `map`/`filter_map` that makes owned items. `par_iter().collect()` is
E0002 ("no method"); write `par_iter().map(|x| x.clone()).collect()` (so `T: Clone` is explicit) or `par_into_iter()`.
`count`, `for_each`, `fold`, `any`, and `all` never keep an item, so they work on views.

A `par_iter` or `par_iter_mut` pipeline views its list: it is second-class like a slice (`spec/memory-rules.md`, SC2), so
the list cannot change while the pipeline is alive (E0111) and the pipeline cannot be stored in a field or collection.

Tests: `s3_parallel/shared_pipeline_borrow` (E0111, E0002).

## PR4. Splitting (decision)

Positions `0..len` are split into chunks. A pipeline of at most 1024 positions runs on the calling task and starts
nothing. A larger one makes at most 64 chunks of at least 1024 positions each, runs each as a scoped task (`task.scope`,
`spec/tasks.md` TK7), and merges the chunks' partial results on the calling task in chunk order. The threshold is fixed,
not derived from the worker count: results never depend on `task.set_workers` (PR7), and idle workers steal whole
chunks from the scheduler's queue as they steal any task. `task.set_workers` must still run before the first task, like
anywhere else; a pipeline that is the first thing to use the scheduler starts it with the default worker count.

Tests: `s3_parallel/empty_single` (empty, one item, below the threshold), `ordered_basic` (above it).

## PR5. Indexed adapters (decision)

`enumerate`, `take`, and `skip` number or cut by source position, so they are only on `IndexedParallelIterator<T>`:
sources, and `map` of an indexed pipeline. A `filter` or `filter_map` makes positions sparse, so after one the three are
E0002. `enumerate` ends the indexed part (a numbered pipeline is not numbered again). `take` and `skip` both return
`ParWindow`, which has `take` and `skip` of its own that return it again, so `skip(a).take(b)` and `take(b).skip(a)` chain
as written.

Tests: `s3_parallel/sources_adapters`, `indexed_after_filter` (E0002).

## PR6. `par_iter_mut` (decision)

`par_iter_mut()` borrows its slice or list exclusively until the pipeline is consumed (E0111 if the list is used in
between), and hands each position to exactly one worker, so the `&mut T` items are disjoint. It has two consumers of its
own, `for_each(|x| ..)` and `for_each_indexed(|i, x| ..)`, and is not a `ParallelIterator`: a reference made from the
slice's address cannot be returned from a method (E0122), only passed to the caller's closure. `T` must be `Send`.

The disjoint-halves route of the issue (`split_at_mut`) is not used: the chunks are index ranges of one slice, and the
single `unsafe` is the pointer arithmetic that turns an index into the one item the closure may write.

Tests: `s3_parallel/iter_mut`, `iter_mut_overlap`.

## PR7. The pool and nesting (decision)

The pool is the scheduler: a chunk is a scoped task, so a pipeline started inside a task, inside another pipeline's
closure, or from several tasks at once parks its caller on the scope instead of blocking a worker. There is no second
pool and no handle. `task.set_workers` is the only worker-count knob.

Tests: `s3_parallel/nested`, `stress`.

## PR8. Panics (decision)

A panic in an adapter or consumer closure aborts the process like any task panic (`spec/tasks.md` TK8). There is no
unwinding and no error channel, so no `try_*` consumers.

Test: `s3_parallel/panic_aborts`.

## PR9. Bounds (decision, SH4)

Every adapter and consumer closure is `Fn + Send + Sync`: all workers call one closure at once, so the closure is shared.
The sharing rule is `spec/shared.md` SH4: an `Fn` closure is `Sync` when every value it captures is `Sync`, and `FnMut`
and `FnOnce` closures never are. A closure that writes a capture is therefore E0141 (not an `Fn` closure), and one that
captures a value that is not `Sync` is E0280 at the call. Items of `par_into_iter` must be `Send`; the items of
`par_iter` need `T: Sync`; a result that crosses back to the caller must be `Send`.

Tests: `s3_parallel/not_send` (E0280, E0141); `s2_shared/closure_fn_is_sync`, `closure_fnmut_not_sync`,
`closure_capture_not_sync`.

## PR10. Compile-time evaluation (decision)

A pipeline is neither pure nor deterministic, so evaluating one at compile time is E0224 whatever its size; the standard
library touches a raw pointer at the top of every pipeline, which compile-time evaluation reports. `spec/const-eval.md` CE6 lists it.

Test: `s3_parallel/in_const`.

## PR11. Tests and benchmarks (decision)

Under `burdin test` the checking allocator (`spec/tasks.md` TK12) tracks the workers' allocations like any task's, and a
pipeline frees what it allocates, so a test that uses one reports no leak. `par_into_iter` keeps each item in a mutex
slot (`Mutex<Option<T>>`): a chunk takes its items out, and what a `take` or an early `any` never asks for is dropped
with the list.

`tests/s3_parallel/bench.bd` measures a `map`/`filter`/`reduce` pipeline against its sequential twin. Scaling is advisory:
change `task.set_workers(1|2|4|8)` in the bench's source to see it. The conformance run only checks that the benches
measure.

Tests: `s3_parallel/bench`, `stress`, `leak_free` (`burdin test`).

## PR12. Not in the first cut (decision)

`flat_map`, `rev` and the double-ended family, user sources (a `Split`/`Producer` protocol), `Set`/`Map` as `collect`
targets (`collect` makes a `List`), `par_sort`, `try_*` consumers, `[T; N]` and `Range` receivers, and early cancellation of
the other chunks for `find`. Implicit parallelization of `for` and of the sequential adapter chains, `async`, SIMD, GPU,
and a second pool are non-goals.

## PR13. The defaults are instantiated on demand (decision)

The default methods of `ParallelIterator<T>`, `ParallelValues<T>`, and `IndexedParallelIterator<T>` are instantiated
on demand at each call, as the `Iterator<T>` defaults are (`spec/iterators.md`, IT9), and an impl block does not
register them. A source whose impl is written in `std` (`ParRange`) would otherwise instantiate every consumer's body,
with its `__chunks` helper and its `Mutex` per output type, in every program that names `std`, and that count counts
against the instance limit (`spec/inspect.md`, INS12). The behavior is the same: a call finds the default by name on
the receiver's interface, and the arguments are checked the same way.

Tests: `s3_parallel/sources_adapters`, `ordered_basic` (the calls), and `tests/inspect/golden/print_instances.golden`
(the program that uses none of them lists no parallel instance).

## Tests

| Rule | Tests |
| --- | --- |
| PR1 | `s3_parallel/ordered_basic`, `sources_adapters` |
| PR2 | `s3_parallel/ordered_basic`, `sources_adapters`, `stress` |
| PR3 | `s3_parallel/shared_pipeline_borrow` (E0111, E0002), `sources_adapters` |
| PR4 | `s3_parallel/empty_single`, `ordered_basic` |
| PR5 | `s3_parallel/sources_adapters`, `indexed_after_filter` (E0002) |
| PR6 | `s3_parallel/iter_mut`, `iter_mut_overlap` (E0111) |
| PR7 | `s3_parallel/nested`, `stress` |
| PR8 | `s3_parallel/panic_aborts` |
| PR9 | `s3_parallel/not_send` (E0280, E0141), `s2_shared/closure_fn_is_sync`, `closure_fnmut_not_sync`, `closure_capture_not_sync` |
| PR10 | `s3_parallel/in_const` (E0224) |
| PR11 | `s3_parallel/bench`, `stress`, `leak_free` |
| PR12 | none (deferred) |

## Known limits

* Chunks run as scoped tasks that borrow the pipeline, so a consumer's closure cannot capture a value that is not `Sync`
  even when only one chunk would run. The error is at the adapter, not at the consumer.
* `find`, `any`, and `all` stop their own chunk early, and `any`/`all` stop the other chunks through a flag, but
  a chunk that has not started yet still starts (and returns at once).
* An instance-naming bug that made two same-spelled closure instances share one function was found by this feature and
  fixed with it (`m12_closures/closure_same_spelling_instances`).
* Like the rest of stage 2, the scheduler is exercised mostly on Linux x86-64 (`tasks.md`, Known limits).
