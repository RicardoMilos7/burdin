# math

Integer, scalar, statistics, vector, and matrix functions over `f64` (`spec/math.md`). A function that can fail gives `Option.None`; the spec lists which bad inputs panic instead.

Version `0.1.0`. Use it with `use math`; it is an official package, so `burdin.toml` needs no entry for it. The package's own guide is [`packages/math/README.md`](../../packages/math/README.md).

## Requirements

Nothing beyond the compiler.

## Worked example

Computes statistics of a sample, a GCD and an LCM, and the determinant of a 2 by 2 matrix.

```burdin
use math.{approx_eq, gcd, lcm, mean, sample_std_dev, Matrix}

fn main() {
    let samples = List<f64>.from([2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0])
    let spread = sample_std_dev(samples.as_slice()).unwrap()
    assert(approx_eq(mean(samples.as_slice()).unwrap(), 5.0, 1e-12, 1e-12))
    assert(approx_eq(spread, 2.13808993, 1e-6, 1e-6))
    assert_eq(gcd(84, 36), 12)
    assert_eq(lcm(4, 6), Option.Some(12))
    let m: Matrix<2, 2> = Matrix.from_array([2.0, 1.0, 1.0, 3.0])
    assert(approx_eq(m.det(), 5.0, 1e-12, 1e-12))
    println("spread {}", spread)
}
```

Save it as `main.bd` and run it with `burdin run main.bd`, or copy the body into a package that declares `use math`. Every example in the package's doc comments is also compiled and run by `burdin test --doc`.

## Surface

Every public item and public method, in source order. Names are shown as `burdin doc` shows them: an item is `module.Name`, and a package whose root re-exports it (`use tls.{Certificate}`) also reaches it from the root. Each row's summary is the first sentence of the item's doc comment; the full text and the examples are on the generated site.

| Item | Signature | Summary |
| --- | --- | --- |
| `dyn_matrix.DynMatrix` | `struct DynMatrix: Clone` | A matrix with `nrows` rows and `ncols` columns, stored row-major in `data`. |
| `dyn_matrix.DynMatrix.zeros` | `fn zeros(rows: usize, cols: usize): DynMatrix` | The matrix of zeros with `rows` rows and `cols` columns. |
| `dyn_matrix.DynMatrix.identity` | `fn identity(n: usize): DynMatrix` | The identity matrix of size `n`. |
| `dyn_matrix.DynMatrix.from_list` | `fn from_list(rows: usize, cols: usize, own data: List<f64>): DynMatrix` | The matrix with `rows` rows and `cols` columns and the given row-major values. |
| `dyn_matrix.DynMatrix.rows` | `fn rows(self): usize` | The number of rows. |
| `dyn_matrix.DynMatrix.cols` | `fn cols(self): usize` | The number of columns. |
| `dyn_matrix.DynMatrix.is_square` | `fn is_square(self): bool` | Whether the number of rows equals the number of columns. |
| `dyn_matrix.DynMatrix.get` | `fn get(self, row: usize, col: usize): f64` | The element at row `row` and column `col`. |
| `dyn_matrix.DynMatrix.set` | `fn set(mut self, row: usize, col: usize, value: f64)` | Writes the element at row `row` and column `col`. |
| `dyn_matrix.DynMatrix.transpose` | `fn transpose(self): DynMatrix` | The transpose: `cols` rows and `rows` columns. |
| `dyn_matrix.DynMatrix.scale` | `fn scale(self, k: f64): DynMatrix` | Each element times `k`. |
| `dyn_matrix.DynMatrix.times` | `fn times(self, other: DynMatrix): DynMatrix` | The matrix product `self * other`. |
| `dyn_matrix.DynMatrix.mul_vector` | `fn mul_vector(self, v: [f64]): List<f64>` | The product of the matrix and the column vector `v`. |
| `dyn_matrix.DynMatrix.trace` | `fn trace(self): f64` | The sum of the diagonal. |
| `dyn_matrix.DynMatrix.det` | `fn det(self): f64` | The determinant. |
| `dyn_matrix.DynMatrix.inverse` | `fn inverse(self): Option<DynMatrix>` | The inverse, or `Option.None` when the matrix is singular. |
| `dyn_matrix.DynMatrix.solve` | `fn solve(self, b: DynMatrix): Option<DynMatrix>` | The matrix `x` with `self * x = b`, or `Option.None` when `self` is singular. |
| `matrix.Matrix` | `struct Matrix<const R: usize, const C: usize>: Copy, Clone` | A matrix with `R` rows and `C` columns, stored row-major in `data`. |
| `matrix.Matrix.zero` | `fn zero(): Matrix<R, C>` | The matrix whose elements are all zero. |
| `matrix.Matrix.from_array` | `fn from_array(data: [f64; R * C]): Matrix<R, C>` | The matrix with the given row-major elements. |
| `matrix.Matrix.identity` | `fn identity(): Matrix<R, C>` | The identity matrix. |
| `matrix.Matrix.rows` | `fn rows(self): usize` | The number of rows. |
| `matrix.Matrix.cols` | `fn cols(self): usize` | The number of columns. |
| `matrix.Matrix.get` | `fn get(self, row: usize, col: usize): f64` | The element at row `row` and column `col`. |
| `matrix.Matrix.set` | `fn set(mut self, row: usize, col: usize, value: f64)` | Writes the element at row `row` and column `col`. |
| `matrix.Matrix.transpose` | `fn transpose(self): Matrix<C, R>` | The transpose: `C` rows and `R` columns. |
| `matrix.Matrix.times` | `fn times<const K: usize>(self, other: Matrix<C, K>): Matrix<R, K>` | The matrix product, with `K` columns in the result. |
| `matrix.Matrix.mul_vector` | `fn mul_vector(self, v: Vector<C>): Vector<R>` | The matrix times the column vector `v`. |
| `matrix.Matrix.scale` | `fn scale(self, k: f64): Matrix<R, C>` | Each element times `k`. |
| `matrix.Matrix.frobenius_norm` | `fn frobenius_norm(self): f64` | The square root of the sum of the squares of the elements. |
| `matrix.Matrix.trace` | `fn trace(self): f64` | The sum of the diagonal. |
| `matrix.Matrix.det` | `fn det(self): f64` | The determinant. |
| `matrix.Matrix.inverse` | `fn inverse(self): Option<Matrix<R, C>>` | The inverse, or `Option.None` when the matrix is singular. |
| `matrix.Matrix.solve` | `fn solve<const K: usize>(self, b: Matrix<R, K>): Option<Matrix<R, K>>` | The `x` with `self * x = b`, or `Option.None` when the matrix is singular. |
| `matrix.Matrix.to_dyn` | `fn to_dyn(self): DynMatrix` | The same matrix as a heap-backed `DynMatrix` (spec/math.md, MA8). |
| `num.gcd` | `fn gcd(a: u64, b: u64): u64` | The greatest common divisor; `gcd(0, 0)` is `0`. |
| `num.lcm` | `fn lcm(a: u64, b: u64): Option<u64>` | The least common multiple, or `Option.None` when it does not fit in a `u64`. |
| `num.mod_pow` | `fn mod_pow(base: u64, exp: u64, modulus: u64): u64` | `base` to the power `exp`, modulo `modulus`. |
| `num.is_prime` | `fn is_prime(n: u64): bool` | Whether `n` is prime. |
| `num.factorial` | `fn factorial(n: u64): Option<u64>` | `n!`, or `Option.None` when it does not fit in a `u64` (from `21!` on). |
| `num.binomial` | `fn binomial(n: u64, k: u64): Option<u64>` | `n` choose `k`: `Option.Some(0)` when `k > n`, and `Option.None` when the value does not fit in a `u64`. |
| `num.lerp` | `fn lerp(a: f64, b: f64, t: f64): f64` | The point `(1 - t) * a + t * b`: `t = 0` gives `a` and `t = 1` gives `b` exactly. |
| `num.approx_eq` | `fn approx_eq(a: f64, b: f64, rel_tol: f64, abs_tol: f64): bool` | Whether `a` and `b` are within `abs_tol`, or within `rel_tol` of the larger magnitude. |
| `num.poly_eval` | `fn poly_eval(coefficients: [f64], x: f64): f64` | The polynomial `c[0] + c[1] x + c[2] x^2 + ...` at `x`, by Horner's rule. |
| `num.bisect` | `fn bisect<F: Fn(f64): f64>(f: F, low: f64, high: f64, tolerance: f64): Option<f64>` | A root of `f` between `low` and `high`, found by halving the interval. |
| `num.simpson` | `fn simpson<F: Fn(f64): f64>(f: F, a: f64, b: f64, intervals: usize): f64` | The composite Simpson rule for the integral of `f` over `a..b`. |
| `special.gamma` | `fn gamma(x: f64): f64` | The gamma function. |
| `special.ln_gamma` | `fn ln_gamma(x: f64): f64` | `ln \|gamma(x)\|`. |
| `stats.sum` | `fn sum(xs: [f64]): f64` | The sum of the values, added left to right. |
| `stats.mean` | `fn mean(xs: [f64]): Option<f64>` | The arithmetic mean, or `Option.None` for no values. |
| `stats.variance` | `fn variance(xs: [f64]): Option<f64>` | The population variance: the mean of the squared deviations from the mean. |
| `stats.sample_variance` | `fn sample_variance(xs: [f64]): Option<f64>` | The sample variance, divided by `n - 1`. |
| `stats.std_dev` | `fn std_dev(xs: [f64]): Option<f64>` | The population standard deviation: the square root of `variance`. |
| `stats.sample_std_dev` | `fn sample_std_dev(xs: [f64]): Option<f64>` | The sample standard deviation: the square root of `sample_variance`. |
| `stats.minimum` | `fn minimum(xs: [f64]): Option<f64>` | The least value. |
| `stats.maximum` | `fn maximum(xs: [f64]): Option<f64>` | The greatest value. |
| `stats.quantile` | `fn quantile(xs: [f64], p: f64): Option<f64>` | The value at fraction `p` of the sorted values, interpolated linearly between the two nearest ranks (the NumPy default). |
| `stats.median` | `fn median(xs: [f64]): Option<f64>` | The median: `quantile` at one half. |
| `stats.covariance` | `fn covariance(xs: [f64], ys: [f64]): Option<f64>` | The population covariance of two equal-length slices. |
| `stats.correlation` | `fn correlation(xs: [f64], ys: [f64]): Option<f64>` | The Pearson correlation coefficient. |
| `vector.Vector` | `struct Vector<const N: usize>: Copy, Clone` | A vector of `N` components. |
| `vector.Vector.zero` | `fn zero(): Vector<N>` | The vector whose components are all zero. |
| `vector.Vector.from_array` | `fn from_array(values: [f64; N]): Vector<N>` | The vector with the given components. |
| `vector.Vector.len` | `fn len(self): usize` | The number of components. |
| `vector.Vector.get` | `fn get(self, i: usize): f64` | The component at `i`. |
| `vector.Vector.dot` | `fn dot(self, other: Vector<N>): f64` | The dot product. |
| `vector.Vector.norm_squared` | `fn norm_squared(self): f64` | The squared length. |
| `vector.Vector.norm` | `fn norm(self): f64` | The length. |
| `vector.Vector.distance` | `fn distance(self, other: Vector<N>): f64` | The distance between the two points. |
| `vector.Vector.scale` | `fn scale(self, k: f64): Vector<N>` | Each component times `k`. |
| `vector.Vector.map` | `fn map<F: Fn(f64): f64>(self, f: F): Vector<N>` | `f` applied to each component. |
| `vector.Vector.normalized` | `fn normalized(self): Option<Vector<N>>` | The vector of length one in the same direction, or `Option.None` for the zero vector. |
| `vector.Vector.angle` | `fn angle(self, other: Vector<N>): Option<f64>` | The angle between the vectors in radians, or `Option.None` when either is the zero vector. |
| `vector.Vector.approx_eq` | `fn approx_eq(self, other: Vector<N>, tolerance: f64): bool` | Whether every component differs from `other`'s by at most `tolerance`. |
| `vector.Vector.cross` | `fn cross(self, other: Vector<3>): Vector<3>` | The cross product of two three-dimensional vectors: a vector perpendicular to both, whose length is the area of the parallelogram they span. |
