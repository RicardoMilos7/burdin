# math

Numbers, statistics, vectors, and matrices for `f64` (issue #268; `spec/math.md`). It is an official package: it ships
with the compiler and needs no install.

```toml
[dependencies]
math = true
```

```burdin
use math.{gcd, mean, Matrix, Vector}
```

or `burdin add math`. The standard library already has the scalar float functions (`sqrt`, `powi`, `ln`, `sin`,
`hypot`, `clamp`, `to_radians`, and the constants `f64.PI`, `f64.E`, and so on); this package adds what std does not.

## What it provides

- **Integers** (`u64`): `gcd`, `lcm`, `mod_pow`, `is_prime` (exact for every `u64`), `factorial`, and `binomial`.
  A result that does not fit in a `u64` is `None`, never a wrapped value.
- **Scalars** (`f64`): `lerp`, `approx_eq`, `poly_eval`, `gamma`, `ln_gamma`, and the root finder `bisect` and the
  integrator `simpson`, which take a closure.
- **Statistics** over a slice `[f64]`: `sum`, `mean`, `variance` and `sample_variance`, `std_dev` and
  `sample_std_dev`, `minimum`, `maximum`, `median`, `quantile`, `covariance`, and `correlation`. A statistic with
  too few values is `None`.
- **`Vector<N>`**: a vector of `N` components, a value like a number. `+`, `-`, unary `-`, `v[i]`, `dot`, `norm`,
  `distance`, `scale`, `normalized`, `angle`, and `cross` for `Vector<3>`.
- **`Matrix<R, C>`**: a fixed-size matrix, stored inline with its shape in its type, so `Matrix<2, 3>` times
  `Matrix<3, 4>` is a `Matrix<2, 4>` and a mismatch does not compile. It has `get`, `set`, `transpose`, `times`,
  `mul_vector`, `scale`, `+`, `-`, `det`, `inverse`, `solve`, and `trace`.
- **`DynMatrix`**: a heap-backed matrix whose shape is known only when the program runs, with the same operations.

A slice is passed as a range of an array or a list, such as `values[0..values.len()]` or `list.as_slice()`. An array
literal is not a slice, so bind it to a name first.

```burdin
use math.{mean, quantile, Matrix, Vector}

fn main() {
    let data = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]
    println("mean {}", mean(data[0..8]).unwrap())            // mean 5
    println("median {}", quantile(data[0..8], 0.5).unwrap()) // median 4.5

    let a: Vector<3> = Vector.from_array([1.0, 2.0, 2.0])
    let b: Vector<3> = Vector.from_array([4.0, -5.0, 6.0])
    println("cross {}", a.cross(b)[0])                       // cross 22

    let m: Matrix<2, 2> = Matrix.from_array([4.0, 3.0, 6.0, 3.0])
    println("det {}", m.det())                               // det -6
}
```

## Decisions

- **`f64` only.** There is no `f32` version and no generic float interface. The standard library has the `f32`
  functions; a generic interface would need a bound that std does not declare.
- **Fixed-size and heap-backed matrices, both.** `Matrix<R, C>` is the default for small matrices: no allocation, the
  shape is checked by the compiler, and it is a value. `DynMatrix` is for a size the program only learns when it
  runs. Both use the same elimination with partial pivoting, so one correct implementation serves both.
- **Square-only operations check at run time.** `det`, `inverse`, `solve`, `trace`, and `identity` need a square
  matrix. The language cannot say "square" in a type here, so a non-square call panics with a message that names
  the operation, as an index out of range does.
- **Absent answers are `None`; programming errors panic.** An empty statistic, a singular matrix, a zero vector's
  direction, and an overflowing product are `None`. A shape mismatch, a zero modulus, a probability outside `[0, 1]`,
  and a non-positive bisection tolerance panic.
- **A singular matrix** is one whose pivot is exactly zero, as in LAPACK. A nearly singular matrix has a tiny pivot
  and a large inverse, as with any LU solver.

## Precision

The arithmetic is IEEE `f64` in a fixed order, so the results of `+`, `-`, `*`, `/`, and `sqrt` are the same bits on
every platform. `gamma` and `ln_gamma` are within `1e-12` relative error for `0 < x <= 171`. `angle` uses `acos`,
which loses accuracy near `0` and `pi` (about `1e-8` at a parallel pair), so compare such an angle with a tolerance.

## Tests

`tests/math` holds about 2,400 known-answer cases in seven fixture files (`tests/math/regenerate.py` writes them with
exact rational arithmetic). `tests/check_math.py` runs them. `tests/s3_math` holds the conformance programs, including
the end-to-end `usage` case and the panic cases. The rules are in `spec/math.md`.
