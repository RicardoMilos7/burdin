---
title: Testing
description: test blocks, assertions, the ? operator in tests, attributes such as should_panic, and running tests with burdin test.
---

A test is a block of code that checks behavior. Tests live next to the code they check, in the same file, and `burdin test`
runs them. They are not part of a normal build, so they add nothing to the program you ship.

## Writing a test

A `test` item has a name in quotes and a body. Use an assertion to state what must hold:

```burdin
fn add(a: i32, b: i32): i32 {
    a + b
}

test "adds two numbers" {
    assert_eq(add(2, 3), 5)?
}

test "addition is commutative" {
    assert_eq(add(2, 7), add(7, 2))?
}
```

Two assertions cover most needs:

| Assertion | Passes when |
| --- | --- |
| `assert(condition)` | the condition is `true` |
| `assert_eq(a, b)` | `a` and `b` are equal |
| `assert_ne(a, b)` | `a` and `b` are different |

Each assertion returns a `Result`, so it ends with `?`. When an assertion fails, the test stops at that line and reports the
values, and `?` is how that failure reaches the test runner.

`assert_eq` needs both values to have the same type. To compare a `String` with a `str`, view the `String` with `as_str()`
first, as the [quickstart](/getting-started/quickstart/) shows.

## Running tests

```console
$ burdin test .
test result: 2 passed, 0 failed
```

Pass a package directory, or a single `.bd` file. Extra words after the target filter the tests by name, so
`burdin test . adds` runs only the tests whose names contain `adds`. `burdin test . --list` prints the tests without
running them.

A failing test shows the assertion, the values that differ, and the location, and the summary counts it:

```console
$ burdin test .
---- test "adds wrongly" stderr ----
assertion failed: assert_eq(add(2, 3), 6) at src/main.bd:14:5: 5 != 6
FAIL adds wrongly
test result: 1 passed, 1 failed
```

`burdin test . --list` prints each test's name, one per line, without running anything.

Tests run in parallel by default. Use `-j 1` to run them one at a time when they share something outside the program.

## Attributes

A test can carry attributes in square brackets after its name:

```burdin
fn checked_divide(a: i32, b: i32): i32 {
    if b == 0 {
        panic("division by zero")
    }
    a / b
}

test "dividing by zero panics" [should_panic("division by zero")] {
    checked_divide(1, 0)
}

test "a slow check" [ignore] {
    assert(checked_divide(10, 2) == 5)?
}
```

| Attribute | Effect |
| --- | --- |
| `should_panic` | The test passes only if the body panics. `should_panic("text")` also requires the panic message to contain `text`. |
| `ignore` | The test is compiled and checked, but not run by default. `burdin test --include-ignored` runs it. |
| `timeout(ms)` | The test fails if it runs longer than `ms` milliseconds. |

A test that should fail cannot fail silently: `should_panic` turns the expected panic into the pass, and an unexpected panic
still fails the test.

## Where tests live

A test block can sit anywhere in a file, and a file can hold many tests. Tests can use private items of their own package. Keep
each test close to the function it checks, so a reader sees the rule and its check together.

Doc comments can also hold examples. A code block in a `///` comment is compiled and run as a test, so an example in the
documentation cannot go out of date.

## Testing with tasks

A test that starts tasks should wait for them before it ends, with `join()`, as the [concurrency](/guide/concurrency/)
chapter shows. A test that leaves tasks running may see them finish or not, depending on timing.
