# Math

The math package (stage 3, issue #268; `packages/math`). It is an ordinary official package (`spec/packages.md`,
PKG15-PKG18) written entirely in Burdin, so this page says what each function accepts and reports, what it does on
bad input, and which tests witness each rule. The rule IDs are MA1-MA12. The package documentation,
`packages/math/README.md`, repeats the decisions for users.

The standard library already has the scalar float functions (`stdlib.md`, "Float functions": `sqrt`, `powi`, `ln`,
`sin`, `hypot`, `clamp`, `to_radians`, and the constants). This package does not repeat them. It adds what std does
not have: integer number theory, a few more scalar functions, statistics over a slice, vectors, and matrices.

Every rule is witnessed by the known-answer fixtures under `tests/math/` (run by `tests/check_math.py`) or by the
conformance case `tests/s3_math/usage`.

## MA1. Scope (decision)

The package works on `f64`. It does not provide `f32` versions, a generic `Float` interface, or complex numbers.
`f32` has the std functions; a generic float interface would need a bound that std does not declare, and writing every
function for both precisions doubles the surface without changing an algorithm. Witnessed by `packages/math/src`
having no `f32` and no interface declaration.

Integers are `u64` and `usize` for counts, and `u64` for the number theory of MA3. There is no bignum: a result that
does not fit `u64` is `None`, never a wrapped value.

## MA2. Modules and exports

`packages/math/src/math.bd` is the root. It re-exports every public name with `pub use`, so a program writes
`use math.{gcd, Vector, DynMatrix}`. The modules are:

| File | Holds |
| --- | --- |
| `num.bd` | integer functions (MA3) and scalar functions (MA4) |
| `special.bd` | `gamma`, `ln_gamma` (MA4) |
| `stats.bd` | statistics over a slice (MA5) |
| `vector.bd` | `Vector<N>` (MA6) |
| `matrix.bd` | `Matrix<R, C>`, fixed size (MA7) |
| `dyn_matrix.bd` | `DynMatrix`, heap-backed, and the linear algebra (MA8) |

A dependency's non-root modules are hidden, so every public name is re-exported in the root.

## MA3. Integers

| Function | Result |
| --- | --- |
| `gcd(a: u64, b: u64): u64` | the greatest common divisor; `gcd(0, 0)` is `0` |
| `lcm(a: u64, b: u64): Option<u64>` | the least common multiple; `Some(0)` when either is `0`; `None` when it does not fit |
| `mod_pow(base: u64, exp: u64, modulus: u64): u64` | `base^exp mod modulus`, in `0..modulus`; `mod_pow(x, 0, 1)` is `0` |
| `is_prime(n: u64): bool` | deterministic Miller-Rabin with the first twelve primes as witnesses, correct for every `u64` |
| `factorial(n: u64): Option<u64>` | `n!`; `None` from `21!` on, which does not fit |
| `binomial(n: u64, k: u64): Option<u64>` | `n` choose `k`; `Some(0)` when `k > n`; `None` when it does not fit |

`mod_pow` and `gcd` never overflow: the product inside `mod_pow` is taken in `u128`. Panics: `mod_pow` with a modulus of
`0` panics with `mod_pow: the modulus is zero`.

## MA4. Scalar functions

| Function | Result |
| --- | --- |
| `lerp(a: f64, b: f64, t: f64): f64` | `(1 - t) * a + t * b`, so `t = 0` gives `a` and `t = 1` gives `b` exactly |
| `approx_eq(a: f64, b: f64, rel_tol: f64, abs_tol: f64): bool` | `|a - b| <= max(abs_tol, rel_tol * max(|a|, |b|))`; equal infinities are equal; a NaN is never equal |
| `poly_eval(coefficients: [f64], x: f64): f64` | `c[0] + c[1] x + c[2] x^2 + ...` by Horner's rule; `0.0` for no coefficients |
| `gamma(x: f64): f64` | the gamma function; `NaN` at a non-positive integer |
| `ln_gamma(x: f64): f64` | `ln |Gamma(x)|`; `INFINITY` at a non-positive integer |
| `bisect<F: Fn(f64): f64>(f: F, low: f64, high: f64, tolerance: f64): Option<f64>` | a root of `f` between `low` and `high` |
| `simpson<F: Fn(f64): f64>(f: F, a: f64, b: f64, intervals: usize): f64` | the composite Simpson rule |

`approx_eq` panics with `approx_eq: a tolerance is negative` when either tolerance is negative. `approx_eq` treats a
NaN as unequal even to itself.

`gamma` and `ln_gamma` use Stirling's series with seven terms for an argument of at least `15`. A smaller positive
argument is raised to that size with the recurrence `gamma(x) = gamma(x + 1) / x`, and the logarithms of the steps are
summed, so a small argument does not underflow. A negative argument uses the reflection formula. The result is within
`1e-12` relative error of the true value for `0 < x <= 171`, which the fixtures in `tests/math/gamma.kat` check. Above
171 the result is `INFINITY`. `ln_gamma` returns the log of the absolute value, so it is sign-correct for a negative
non-integer. (The package first used the Lanczos approximation; its coefficients as written were accurate only to about
`1e-8`, so it was replaced.)

`bisect` returns `None` when `f(low)` and `f(high)` are both non-zero with the same sign. It returns the endpoint when
an endpoint is a root. It halves the interval until its width is at most `tolerance`, so the answer is within
`tolerance` of a root. It panics with `bisect: the tolerance must be positive` when `tolerance` is not greater than
`0`, and it makes at most 2100 steps, which is enough for every pair of finite doubles.

`simpson` rounds an odd `intervals` up to the next even number and panics with `simpson: needs at least one interval`
when `intervals` is `0`.

## MA5. Statistics

All functions take a slice `[f64]`. A caller passes an array as a range, `values[0..values.len()]`, or a `List` as
`list.as_slice()`. The empty slice is an absent answer, `None`, wherever a mean or a spread would divide by zero.

| Function | Result |
| --- | --- |
| `sum(xs: [f64]): f64` | the sum, added left to right; `0.0` for no values |
| `mean(xs: [f64]): Option<f64>` | the arithmetic mean; `None` for no values |
| `variance(xs: [f64]): Option<f64>` | the population variance, the mean of the squared deviations; `None` for no values |
| `sample_variance(xs: [f64]): Option<f64>` | the sample variance, divided by `n - 1`; `None` for fewer than two values |
| `std_dev(xs: [f64]): Option<f64>` | the square root of `variance` |
| `sample_std_dev(xs: [f64]): Option<f64>` | the square root of `sample_variance` |
| `minimum(xs: [f64]): Option<f64>`, `maximum(xs: [f64]): Option<f64>` | the least and greatest value; NaN values are skipped |
| `quantile(xs: [f64], p: f64): Option<f64>` | the value at fraction `p`, by linear interpolation between the two nearest ranks (the NumPy default) |
| `median(xs: [f64]): Option<f64>` | `quantile(xs, 0.5)` |
| `covariance(xs: [f64], ys: [f64]): Option<f64>` | the population covariance |
| `correlation(xs: [f64], ys: [f64]): Option<f64>` | the Pearson correlation coefficient; `None` when either side has zero variance |

`mean` is the sum divided by `n`, and `variance` is computed in two passes: the mean first, then the squared
deviations, so the result does not lose the precision of a one-pass formula. `minimum` and `maximum` skip NaN, since
`min` and `max` on `f64` return the other value when one side is NaN; they return NaN only when every value is NaN.
`quantile` sorts a copy with `total_cmp`, so a NaN sorts after every number and the sort is deterministic. It panics
with `quantile: p must be between 0 and 1` when `p` is outside that range or NaN.

`covariance` and `correlation` panic with `covariance: the slices differ in length` when the two slices have different
lengths.

## MA6. Vectors

`Vector<const N: usize>` holds `N` values of `f64` in `data: [f64; N]`. It derives `Copy` and `Clone`, so it has value
semantics like a number.

| Member | Result |
| --- | --- |
| `Vector<N>.zero()`, `Vector<N>.from_array(values: [f64; N])` | the zero vector and the vector of the given values |
| `len(self): usize` | `N` |
| `get(self, i: usize): f64` | the component; an out-of-range `i` panics as an array index does |
| `dot(self, other: Vector<N>): f64` | the dot product |
| `norm_squared(self): f64`, `norm(self): f64` | the squared length and the length |
| `distance(self, other: Vector<N>): f64` | the length of the difference |
| `scale(self, k: f64): Vector<N>` | each component times `k` |
| `map<F: Fn(f64): f64>(self, f: F): Vector<N>` | `f` applied to each component |
| `normalized(self): Option<Vector<N>>` | the vector of length one in the same direction; `None` for the zero vector |
| `angle(self, other: Vector<N>): Option<f64>` | the angle between the vectors in radians; `None` when either is zero |
| `approx_eq(self, other: Vector<N>, tolerance: f64): bool` | every component differs by at most `tolerance` |
| `+`, `-`, unary `-` | componentwise (`Add`, `Sub`, `Neg`) |
| `v[i]` | `Index<f64>`: read a component; `IndexMut<f64>` writes one |
| `Vector<3>.cross(self, other: Vector<3>): Vector<3>` | the cross product (a specific-instance impl, `spec/impls.md`) |

`*` is not overloaded for vectors: a vector times a vector has no single meaning, so the scalar product is `scale` and
the dot product is `dot`. `angle` clamps the cosine to `[-1, 1]` before `acos`, so rounding never makes it NaN.
`norm` is `sqrt(norm_squared)`: a component so large that its square overflows gives `INFINITY`, and this function does
not rescale to avoid that.

## MA7. Fixed-size matrices (decision)

`Matrix<const R: usize, const C: usize>` holds `R` rows and `C` columns in `data: [f64; R * C]`, row-major. It derives
`Copy` and `Clone`. Its shape is part of its type, so `Matrix<2, 3>` times `Matrix<3, 4>` gives `Matrix<2, 4>`, and a
product of mismatched shapes is a compile-time error (E0003), not a run-time one.

| Member | Result |
| --- | --- |
| `Matrix<R, C>.zero()`, `Matrix<R, C>.from_array(data: [f64; R * C])` | the zero matrix and a matrix from row-major values |
| `rows(self): usize`, `cols(self): usize` | `R` and `C` |
| `get(self, row: usize, col: usize): f64` | the element; an out-of-range index panics |
| `set(mut self, row: usize, col: usize, value: f64)` | writes one element |
| `transpose(self): Matrix<C, R>` | the transpose |
| `times<const K: usize>(self, other: Matrix<C, K>): Matrix<R, K>` | the matrix product |
| `mul_vector(self, v: Vector<C>): Vector<R>` | the matrix times a column vector |
| `scale(self, k: f64): Matrix<R, C>` | each element times `k` |
| `frobenius_norm(self): f64` | the square root of the sum of the squares of the elements |
| `+`, `-`, unary `-` | elementwise (`Add`, `Sub`, `Neg`) |
| `identity(): Matrix<R, C>` | the identity; panics when `R != C` |
| `trace(self): f64`, `det(self): f64`, `inverse(self): Option<Matrix<R, C>>`, `solve<const K: usize>(self, b: Matrix<R, K>): Option<Matrix<R, K>>` | the square-matrix operations of MA8, through a `DynMatrix` |

**Square-only operations check at run time.** A square matrix is not a type the language can express here: a
`Matrix<N, N>` impl header is E0007, because a const parameter may not appear twice. So `identity`, `trace`, `det`,
`inverse`, and `solve` panic with `Matrix: the matrix is not square` when `R != C`. This is the same kind of check as an
index that is out of range, and it is witnessed by the `run-abort` case `tests/s3_math/abort_not_square`. A
specific-instance impl (`Matrix<2, 2> { ... }`) would give a compile-time check for one shape, but it would repeat the
code for each size, so the package does not use one.

A product of mismatched shapes is a compile-time error, not a run-time one: `Matrix<2, 3>.times(Matrix<2, 3>)` is E0003,
witnessed by `tests/s3_math/compile_shape`.

## MA8. Heap-backed matrices and linear algebra (decision)

`DynMatrix` is the matrix whose shape is known only at run time. It holds `rows`, `cols`, and `data: List<f64>`, row-major,
and it derives `Clone`. The decision is: the package has both a fixed-size matrix (MA7) and a heap-backed one. The fixed-size
type is the default for small matrices: no allocation, the shape is checked by the compiler, and it is a value like a
number. The heap-backed type is for a size the program does not know until it runs, such as a matrix read from a file or a
system of equations of unknown size. The linear algebra is written once, on `DynMatrix`, and the fixed-size operations of
MA7 convert to it, so one correct implementation serves both.

| Member | Result |
| --- | --- |
| `DynMatrix.zeros(rows: usize, cols: usize)`, `DynMatrix.identity(n: usize)` | the zero matrix and the identity |
| `DynMatrix.from_list(rows: usize, cols: usize, data: List<f64>)` | a matrix from row-major values; panics when the count is not `rows * cols` |
| `get`, `set`, `rows`, `cols`, `is_square` | as for `Matrix` |
| `transpose(self)`, `scale(self, k: f64)`, `mul_vector(self, v: [f64]): List<f64>` | as for `Matrix` |
| `times(self, other: DynMatrix): DynMatrix` | the matrix product; panics when `self.cols() != other.rows()` |
| `+`, `-`, unary `-` | elementwise; panics when the shapes differ |
| `trace(self)`, `det(self)` | a square matrix; panics when not square |
| `inverse(self): Option<DynMatrix>` | the inverse; `None` when singular |
| `solve(self, b: DynMatrix): Option<DynMatrix>` | the `x` with `self * x = b`; `None` when singular; panics when `self` is not square or when `b.rows()` differs from `self.rows()` |

**The algorithm (MA8 is the decision).** `det`, `inverse`, and `solve` use Gaussian elimination with partial pivoting: at
each column the row with the largest absolute value, from the diagonal down, is the pivot (the first one on a tie).
`det` runs the elimination on the matrix alone and multiplies the pivots, negated once for each row swap. `solve` runs
the same elimination and applies its row operations to every column of the right-hand side at once, then substitutes
backward. `inverse` is `solve` with the identity as the right-hand side. A matrix is **singular** when a pivot is exactly
`0.0`; then `det` is `0.0` and `inverse` and `solve` are `None`. No tolerance is used, the same rule as LAPACK and numpy.
A nearly singular matrix has a tiny non-zero pivot: it is not reported as singular, and its inverse is large, as with
any elimination.

The fixed-size `det`, `inverse`, and `solve` of MA7 convert to a `DynMatrix`, run the same code, and convert back.

## MA9. Panics

Programmer errors panic, and a value that is only absent returns `None`, as a rule of the package:

- a shape mismatch in `DynMatrix` (`times`, `+`, `-`, `solve`, `from_list`) and a square-only operation on a non-square
  matrix (MA7);
- an index out of range in `get` or `set`, and a component index out of range in a vector, as the array index does;
- a bad argument: `mod_pow` with a zero modulus, `approx_eq` with a negative tolerance, `bisect` with a non-positive
  tolerance, `simpson` with zero intervals, `quantile` with `p` outside `[0, 1]`, and `covariance` or `correlation` on
  slices of different lengths.

Every panic message begins with the function's name, so a panic in a program names the call that failed.

`None` is the answer for a value that does not exist: an empty statistic, a zero vector's direction, a singular matrix,
an overflowing product, a bisection with no sign change. No function here returns a `Result`, and no registry error code
is involved (`tests/README.md`).

## MA10. Precision and reproducibility

The package's arithmetic is IEEE 754 `f64`, and every function does its operations in a fixed order written in its
source. The results of `+`, `-`, `*`, `/`, and `sqrt` are therefore the same bits on every platform, and so is every
result that uses only those operations: `sum`, `mean`, `dot`, `transpose`, `times`, and the LU of MA8. The transcendental
functions (`exp`, `ln`, `sin`, `cos`, `acos`, `powf`) are the C library's, as the std float functions are
(`stdlib.md`), so `gamma`, `ln_gamma`, `angle`, and `norm` through `sqrt` may differ in the last bit between libraries.

The fixtures therefore compare a float result to its expected value with a relative tolerance: `1e-12` for a formula
(the scalar, statistics, vector, and `gamma` fixtures) and `1e-9` for a matrix result, whose elimination loses a few
digits to cancellation. An integer result and an `Option` are compared exactly. An `angle` near `0` or `pi` carries the
error of `acos`, about the square root of machine epsilon, so the fixtures keep to angles away from those values.

## MA11. Tests

Every rule is witnessed by one of these:

- `tests/math/` holds the known-answer fixtures, one file per module: `num.kat` (MA3), `scalar.kat` (MA4), `gamma.kat`
  (MA4), `stats.kat` (MA5), `vector.kat` (MA6), `matrix.kat` (MA7), and `linalg.kat` (MA8), about 2,400 cases.
  `tests/math/regenerate.py` writes the expected values with exact rational arithmetic (`fractions.Fraction`) where a
  result is rational, and with Python's `math` module where it is not, so no expected value is copied from the code
  under test. `--check` fails when the committed files differ from what it writes.
- `tests/math/runner` is the program that reads a fixture and calls the package. `tests/check_math.py` builds it, runs
  every fixture, and checks that the runner notices a wrong value.
- `tests/s3_math/` holds the conformance cases, run by `tests/run_tests.py`: `usage` (an end-to-end program), `surface`
  (every other public member, with values small enough to check by hand), `compile_shape` (E0003 for a product of
  mismatched shapes), and the `run-abort` cases `abort_mod_pow`, `abort_not_square`, `abort_dyn_shape`,
  `abort_quantile`, `abort_bisect`, `abort_covariance`, `abort_approx_tolerance`, `abort_simpson`, and
  `abort_from_list`, one for each panic of MA9 that has a stated message.
- `tests/check_math.py` also checks the manifest, the README, the official pin of the package in `burdin.lock`, and
  `burdin add math`.

## MA12. Deferred

Not in this version, and each is a later package or a later decision:

- `f32` and a generic float interface (MA1);
- complex numbers and quaternions;
- sparse matrices, eigenvalues, and singular value decomposition: the LU of MA8 is the only factorization;
- an integer matrix or a rational type;
- a random-number source for statistics (`std` has `Random`);
- a square-matrix static type (MA7 explains why it is a run-time check).
