#!/usr/bin/env python3
"""Writes the known-answer fixtures of the `math` official package (issue #268; spec/math.md, MA11).

    python3 tests/math/regenerate.py           write the .kat files next to this script
    python3 tests/math/regenerate.py --check   fail (status 1) when the committed files differ from what this writes

No expected value is computed by the package under test. A rational result is computed exactly with
`fractions.Fraction`, a square root with `decimal.Decimal` at 60 digits, and the transcendental results (`gamma`,
`ln_gamma`, `acos`) to 60 digits by the series and bisection written in this file. The C library's `math.gamma` and
`math.acos` are not used as oracles: they are accurate to a few ulps, but their last bits differ between platforms, so
a file written from them would not be the same everywhere. Random inputs come from a fixed linear congruential
generator, so the files are the same on every machine.

A fixture line is `name|op|fields...|expected`. A line that starts with `#` is a comment. A vector or list is
comma-separated (`-` is empty), a matrix is `RxC:v1,v2,...` in row-major order, and an absent answer (`None`) is `none`.
A float is written with `repr`, so it reads back exactly; `nan`, `inf`, and `-inf` are the non-finite values.
"""

import math
import sys
from decimal import Decimal, getcontext
from fractions import Fraction
from pathlib import Path

getcontext().prec = 60
HERE = Path(__file__).resolve().parent
U64_MAX = 2**64 - 1

fixtures = {}


def add(file, line):
    fixtures.setdefault(file, []).append(line)


def fmt(x):
    """A float as its shortest round-trip text; `nan`, `inf`, and `-inf` for the non-finite values."""
    if isinstance(x, Fraction):
        x = float(x)
    if math.isnan(x):
        return "nan"
    if math.isinf(x):
        return "inf" if x > 0 else "-inf"
    return repr(float(x))


# High-precision oracles for the transcendental results. Each returns a 60-digit Decimal of the true value, so a float
# written from it is the correctly rounded value on every machine.

PI = Decimal("3.14159265358979323846264338327950288419716939937510582097494459")
TWO_PI = 2 * PI
LN_TWO_PI = TWO_PI.ln()


def bernoulli(count):
    """The Bernoulli numbers B_0 to B_count as Fractions (B_1 = -1/2; the even ones feed Stirling's series)."""
    numbers = [Fraction(1)]
    for m in range(1, count + 1):
        numbers.append(-sum(math.comb(m + 1, j) * numbers[j] for j in range(m)) / (m + 1))
    return numbers


BERNOULLI = bernoulli(40)


def to_decimal(q):
    """A Fraction as a 60-digit Decimal."""
    return Decimal(q.numerator) / Decimal(q.denominator)


def sin_digits(y):
    """sin of a Decimal: reduce to [-pi, pi], then sum the Taylor series."""
    r = y - TWO_PI * (y / TWO_PI).to_integral_value()
    total = r
    term = r
    n = 1
    while True:
        term = -term * r * r / ((2 * n) * (2 * n + 1))
        if abs(term) < Decimal("1e-70"):
            return total
        total += term
        n += 1


def ln_gamma_positive(x):
    """ln Gamma(x) for a positive Decimal: the recurrence lifts x to at least 60, then Stirling's series with 20 terms."""
    z = x
    shift = Decimal(0)
    while z < 60:
        shift += z.ln()
        z += 1
    total = (z - Decimal("0.5")) * z.ln() - z + LN_TWO_PI / 2
    for k in range(1, 21):
        total += to_decimal(BERNOULLI[2 * k]) / (2 * k * (2 * k - 1) * z ** (2 * k - 1))
    return total - shift


def gamma_digits(x):
    """Gamma(x) for a float x that is not a pole, as a 60-digit Decimal."""
    d = to_decimal(Fraction(x))
    if x > 0:
        return ln_gamma_positive(d).exp()
    # Reflection: Gamma(x) Gamma(1 - x) = pi / sin(pi x).
    return PI / (sin_digits(PI * d) * ln_gamma_positive(1 - d).exp())


def ln_gamma_digits(x):
    """ln |Gamma(x)| for a float x that is not a pole, as a 60-digit Decimal. The true value is exactly zero at 1 and 2,
    where the series leaves a residue of about 1e-57; anything this small is written as zero."""
    d = to_decimal(Fraction(x))
    if x > 0:
        value = ln_gamma_positive(d)
    else:
        value = PI.ln() - abs(sin_digits(PI * d)).ln() - ln_gamma_positive(1 - d)
    return Decimal(0) if abs(value) < Decimal("1e-40") else value


def acos_digits(c):
    """acos of a Fraction in [-1, 1], as a 60-digit Decimal, by bisection on cos (decreasing on [0, pi])."""
    target = to_decimal(c)
    low, high = Decimal(0), PI
    for _ in range(200):
        middle = (low + high) / 2
        if sin_digits(middle + PI / 2) > target:  # cos(middle) > target: the answer lies to the right
            low = middle
        else:
            high = middle
    return (low + high) / 2


def fmt_list(values):
    return ",".join(fmt(v) for v in values) if values else "-"


def fmt_matrix(rows, cols, values):
    return f"{rows}x{cols}:" + ",".join(fmt(v) for v in values)


def exact(value):
    """A Fraction for an int, a decimal string, or a float (a float is taken as the exact binary value it stores)."""
    if isinstance(value, Fraction):
        return value
    if isinstance(value, float):
        return Fraction(value)
    return Fraction(value)


def dsqrt(x):
    """The square root of a non-negative Fraction, as a Fraction close to it to 60 digits."""
    if x == 0:
        return Fraction(0)
    root = (Decimal(x.numerator) / Decimal(x.denominator)).sqrt()
    return Fraction(root)


class Lcg:
    """A fixed generator: a linear congruential sequence of 32-bit values from `seed`."""

    def __init__(self, seed):
        self.state = seed & 0xFFFFFFFF

    def next(self):
        self.state = (1664525 * self.state + 1013904223) & 0xFFFFFFFF
        return self.state

    def below(self, bound):
        return self.next() % bound


# ---------------------------------------------------------------------------------------------------------------------
# num.kat: integer number theory (MA3) and the scalar functions (MA4).

NUM = "num.kat"
num_cases = []
for a, b in [(0, 0), (0, 7), (7, 0), (12, 18), (17, 5), (1071, 462), (2**40 * 3, 2**35 * 9), (U64_MAX, U64_MAX - 1),
             (1, U64_MAX), (48, 180), (2**63, 2**62)]:
    num_cases.append(("gcd", [a, b], math.gcd(a, b)))
for a, b in [(4, 6), (0, 5), (21, 6), (2**31, 2**32), (2**32, 2**32 + 1), (3, 5), (2, U64_MAX), (U64_MAX, 1),
             (12, 18), (7, 7)]:
    value = 0 if a == 0 or b == 0 else a // math.gcd(a, b) * b
    num_cases.append(("lcm", [a, b], value if value <= U64_MAX else None))
for base, exp, m in [(2, 10, 1000), (3, 0, 7), (3, 0, 1), (7, 128, 13), (5, 3, 2), (2, U64_MAX, 10**9 + 7),
                     (U64_MAX, 2, 18446744073709551557), (123456789, 987654321, 998244353),
                     (2, 2**40, 2**61 - 1), (10, 18, 19)]:
    num_cases.append(("modpow", [base, exp, m], pow(base, exp, m)))

# Primes: every value below 2000 by trial division, then the known large ones. 2^61 - 1 (a Mersenne prime), 2^32 - 5
# and 2^64 - 59 (the largest prime below 2^64) are prime, and 2^32 + 1 = 641 * 6700417 and the products are composite.
def trial_prime(n):
    if n < 2:
        return False
    d = 2
    while d * d <= n:
        if n % d == 0:
            return False
        d += 1
    return True


for n in range(0, 2000):
    num_cases.append(("isprime", [n], trial_prime(n)))
for n, expected in [(4294967291, True), (4294967297, False), (2**61 - 1, True), (18446744073709551557, True),
                    (18446744073709551615, False), (1000000007 * 998244353, False),
                    (3215031751, False), (3277, False), (4759123141, False)]:
    num_cases.append(("isprime", [n], expected))

for n in [0, 1, 2, 5, 10, 20, 21, 25, 100]:
    value = math.factorial(n)
    num_cases.append(("factorial", [n], value if value <= U64_MAX else None))
for n, k in [(5, 2), (10, 3), (0, 0), (3, 5), (52, 5), (60, 30), (68, 34), (63, 29), (64, 31), (40, 20), (1, 1),
             (100, 50), (10, 0)]:
    value = math.comb(n, k)
    num_cases.append(("binomial", [n, k], value if value <= U64_MAX else None))

for name, values, expected in num_cases:
    fields = "|".join(str(v) for v in values)
    if expected is None:
        expected_text = "none"
    elif isinstance(expected, bool):
        expected_text = "true" if expected else "false"
    else:
        expected_text = str(expected)
    add(NUM, f"{name}_{'_'.join(str(v) for v in values)}|{name}|{fields}|{expected_text}")

# Scalar functions.
SCALAR = "scalar.kat"
for a, b, t in [(0.0, 10.0, 0.25), (1.0, 3.0, 0.0), (1.0, 3.0, 1.0), (-2.5, 4.0, 0.5), (1.5, -7.0, 2.0)]:
    value = (1 - exact(t)) * exact(a) + exact(t) * exact(b)
    add(SCALAR, f"lerp_{a}_{b}_{t}|lerp|{fmt(a)}|{fmt(b)}|{fmt(t)}|{fmt(value)}")
for a, b, rel, absolute, expected in [
    (1.0, 1.0000001, 1e-6, 0.0, "true"),
    (1.0, 1.1, 1e-6, 0.0, "false"),
    (0.0, 1e-12, 0.0, 1e-9, "true"),
    (0.0, 1e-6, 0.0, 1e-9, "false"),
    (1e300, 1.0000000000001e300, 1e-12, 0.0, "true"),
    (float("inf"), float("inf"), 0.0, 0.0, "true"),
    (float("inf"), -float("inf"), 1.0, 1.0, "false"),
    (float("nan"), float("nan"), 1.0, 1.0, "false"),
]:
    add(SCALAR, f"approx_{len(fixtures.get(SCALAR, []))}|approx|{fmt(a)}|{fmt(b)}|{fmt(rel)}|{fmt(absolute)}|{expected}")
for coefficients, x in [([1.0, 2.0, 3.0], 2.0), ([], 5.0), ([7.0], -3.0), ([1.0, -1.0, 0.5, 0.25], 0.5),
                        ([0.0, 0.0, 1.0], 10.0), ([2.5, 0.0, -4.0, 1.0], -1.5)]:
    value = Fraction(0)
    for c in reversed(coefficients):
        value = value * exact(x) + exact(c)
    add(SCALAR, f"poly_{len(fixtures.get(SCALAR, []))}|poly|{fmt_list(coefficients)}|{fmt(x)}|{fmt(value)}")

# gamma and ln_gamma: 60-digit values from the Stirling series, rounded once to a float (see gamma_digits). Poles are NaN
# for gamma and infinity for ln_gamma. The identity gamma(x + 1) = x gamma(x) is checked against math.gamma below.
GAMMA = "gamma.kat"
for x in [0.5, 1.0, 1.5, 2.0, 3.0, 3.7, 5.0, 10.0, 14.9, 15.0, 20.0, 0.001, 0.1, 100.0, 150.0, 170.5, 171.0,
          -0.5, -1.5, -2.7, -10.25, 0.25, 7.75]:
    add(GAMMA, f"gamma_{x}|gamma|{fmt(x)}|{fmt(float(gamma_digits(x)))}")
    add(GAMMA, f"lngamma_{x}|lngamma|{fmt(x)}|{fmt(float(ln_gamma_digits(x)))}")
for x in [0.0, -1.0, -2.0, -7.0]:
    add(GAMMA, f"gamma_pole_{x}|gamma|{fmt(x)}|nan")
    add(GAMMA, f"lngamma_pole_{x}|lngamma|{fmt(x)}|inf")

# ---------------------------------------------------------------------------------------------------------------------
# stats.kat: statistics over a slice (MA5). The expected values are exact rationals, rounded once.

STATS = "stats.kat"
datasets = {
    "classic": [2, 4, 4, 4, 5, 5, 7, 9],
    "mixed": ["1.5", "-2.25", "3.125", "0", "10"],
    "single": [7],
    "pair": [1, 2],
    "unsorted": ["9.5", "-3", "0.125", "4", "4", "11"],
    "tiny": ["0.000000001", "0.000000002", "0.000000004"],
}
for label, raw in datasets.items():
    xs = [exact(v) for v in raw]
    n = len(xs)
    xs_text = fmt_list(xs)
    m = sum(xs, Fraction(0)) / n
    pop_var = sum(((x - m) ** 2 for x in xs), Fraction(0)) / n
    add(STATS, f"sum_{label}|sum|{xs_text}|-|-|{fmt(sum(xs, Fraction(0)))}")
    add(STATS, f"mean_{label}|mean|{xs_text}|-|-|{fmt(m)}")
    add(STATS, f"variance_{label}|variance|{xs_text}|-|-|{fmt(pop_var)}")
    add(STATS, f"std_dev_{label}|std_dev|{xs_text}|-|-|{fmt(dsqrt(pop_var))}")
    if n >= 2:
        sample_var = pop_var * n / (n - 1)
        add(STATS, f"sample_variance_{label}|sample_variance|{xs_text}|-|-|{fmt(sample_var)}")
        add(STATS, f"sample_std_dev_{label}|sample_std_dev|{xs_text}|-|-|{fmt(dsqrt(sample_var))}")
    else:
        add(STATS, f"sample_variance_{label}|sample_variance|{xs_text}|-|-|none")
        add(STATS, f"sample_std_dev_{label}|sample_std_dev|{xs_text}|-|-|none")
    add(STATS, f"minimum_{label}|minimum|{xs_text}|-|-|{fmt(min(xs))}")
    add(STATS, f"maximum_{label}|maximum|{xs_text}|-|-|{fmt(max(xs))}")
    ordered = sorted(xs)
    for p in ["0", "1", "0.25", "0.5", "0.9", "0.3333333333333333"]:
        pf = exact(p)
        h = pf * (n - 1)
        lo = math.floor(h)
        frac = h - lo
        value = ordered[lo] if frac == 0 or lo + 1 >= n else ordered[lo] + frac * (ordered[lo + 1] - ordered[lo])
        add(STATS, f"quantile_{label}_{p}|quantile|{xs_text}|-|{p}|{fmt(value)}")
    median = ordered[(n - 1) // 2] if n % 2 == 1 else (ordered[n // 2 - 1] + ordered[n // 2]) / 2
    add(STATS, f"median_{label}|median|{xs_text}|-|-|{fmt(median)}")
add(STATS, "empty_sum|sum|-|-|-|0.0")
add(STATS, "empty_mean|mean|-|-|-|none")
add(STATS, "empty_variance|variance|-|-|-|none")
add(STATS, "empty_minimum|minimum|-|-|-|none")
add(STATS, "empty_median|median|-|-|-|none")
add(STATS, "empty_quantile|quantile|-|-|0.5|none")

pairs = {
    "rising": ([1, 2, 3, 4], [2, 4, 6, 8]),
    "falling": ([1, 2, 3], [9, 4, 1]),
    "scattered": (["1", "2", "3.5", "4", "6"], ["2", "1", "4", "3", "9"]),
    "flat": ([1, 2, 3], [5, 5, 5]),
}
for label, (xs_raw, ys_raw) in pairs.items():
    xs = [exact(v) for v in xs_raw]
    ys = [exact(v) for v in ys_raw]
    n = len(xs)
    mx = sum(xs, Fraction(0)) / n
    my = sum(ys, Fraction(0)) / n
    cov = sum(((x - mx) * (y - my) for x, y in zip(xs, ys)), Fraction(0)) / n
    vx = sum(((x - mx) ** 2 for x in xs), Fraction(0)) / n
    vy = sum(((y - my) ** 2 for y in ys), Fraction(0)) / n
    add(STATS, f"covariance_{label}|covariance|{fmt_list(xs)}|{fmt_list(ys)}|-|{fmt(cov)}")
    if vx == 0 or vy == 0:
        correlation = "none"
    else:
        correlation = fmt(cov / (dsqrt(vx) * dsqrt(vy)))
    add(STATS, f"correlation_{label}|correlation|{fmt_list(xs)}|{fmt_list(ys)}|-|{correlation}")
add(STATS, "covariance_empty|covariance|-|-|-|none")
add(STATS, "correlation_empty|correlation|-|-|-|none")

# ---------------------------------------------------------------------------------------------------------------------
# vector.kat: fixed-size vectors (MA6), of 2, 3, and 4 components.

VECTOR = "vector.kat"
vector_cases = [
    ("v2", [3, 4], [1, 2]),
    ("v3", [1, 2, 2], [4, -5, 6]),
    ("v4", [1, -2, 3, 4], [5, 6, -7, 8]),
    ("v3_axes", [1, 0, 0], [0, 1, 0]),
    ("v2_decimal", ["0.5", "1.25"], ["-2", "0.75"]),
]
for label, a_raw, b_raw in vector_cases:
    a = [exact(v) for v in a_raw]
    b = [exact(v) for v in b_raw]
    dot = sum((x * y for x, y in zip(a, b)), Fraction(0))
    nsq = sum((x * x for x in a), Fraction(0))
    norm = dsqrt(nsq)
    diff = [x - y for x, y in zip(a, b)]
    distance = dsqrt(sum((d * d for d in diff), Fraction(0)))
    add(VECTOR, f"dot_{label}|dot|{fmt_list(a)}|{fmt_list(b)}|{fmt(dot)}")
    add(VECTOR, f"norm_squared_{label}|norm_squared|{fmt_list(a)}|-|{fmt(nsq)}")
    add(VECTOR, f"norm_{label}|norm|{fmt_list(a)}|-|{fmt(norm)}")
    add(VECTOR, f"distance_{label}|distance|{fmt_list(a)}|{fmt_list(b)}|{fmt(distance)}")
    add(VECTOR, f"add_{label}|add|{fmt_list(a)}|{fmt_list(b)}|{fmt_list([x + y for x, y in zip(a, b)])}")
    add(VECTOR, f"sub_{label}|sub|{fmt_list(a)}|{fmt_list(b)}|{fmt_list(diff)}")
    add(VECTOR, f"scale_{label}|scale|{fmt_list(a)}|-2.5|{fmt_list([x * Fraction(-5, 2) for x in a])}")
    add(VECTOR, f"normalized_{label}|normalized|{fmt_list(a)}|-|"
                f"{fmt_list([x / norm for x in a])}")
    # The angle is acos of the cosine, rounded to a float and clamped to [-1, 1]; acos is computed to 60 digits.
    cosine = float(dot / (norm * dsqrt(sum((y * y for y in b), Fraction(0)))))
    add(VECTOR, f"angle_{label}|angle|{fmt_list(a)}|{fmt_list(b)}|"
                f"{fmt(float(acos_digits(Fraction(max(-1.0, min(1.0, cosine))))))}")
add(VECTOR, "cross_v3|cross|1,2,3|4,5,6|-3,6,-3")
add(VECTOR, "cross_axes|cross|1,0,0|0,1,0|0,0,1")
add(VECTOR, "cross_parallel|cross|2,4,6|1,2,3|0,0,0")
add(VECTOR, "zero_normalized|normalized|0,0,0|-|none")
add(VECTOR, "zero_angle|angle|0,0|1,1|none")
add(VECTOR, "zero_norm|norm|0,0,0,0|-|0.0")
add(VECTOR, "angle_right|angle|1,0|0,2|1.5707963267948966")

# ---------------------------------------------------------------------------------------------------------------------
# matrix.kat (fixed size, MA7) and linalg.kat (heap-backed, MA8). The same operations; matrix.kat holds only the shapes
# the runner dispatches to (2x2, 2x3, 3x2, 3x3, 4x4, and the column vectors 2x1, 3x1, 4x1). The expected values are
# exact rationals, rounded once.


def mat(rows, cols, values):
    return [[exact(values[r * cols + c]) for c in range(cols)] for r in range(rows)]


def mat_text(m):
    rows = len(m)
    cols = len(m[0]) if rows else 0
    return fmt_matrix(rows, cols, [m[r][c] for r in range(rows) for c in range(cols)])


def transpose(m):
    return [list(col) for col in zip(*m)]


def matmul(a, b):
    return [[sum((a[i][k] * b[k][j] for k in range(len(b))), Fraction(0)) for j in range(len(b[0]))]
            for i in range(len(a))]


def det_exact(m):
    n = len(m)
    work = [row[:] for row in m]
    sign = 1
    det = Fraction(1)
    for col in range(n):
        pivot = next((r for r in range(col, n) if work[r][col] != 0), None)
        if pivot is None:
            return Fraction(0)
        if pivot != col:
            work[col], work[pivot] = work[pivot], work[col]
            sign = -sign
        det *= work[col][col]
        for r in range(col + 1, n):
            f = work[r][col] / work[col][col]
            for c in range(col, n):
                work[r][c] -= f * work[col][c]
    return sign * det


def solve_exact(a, b):
    """The exact solution of a * x = b for square a, or None when a is singular."""
    n = len(a)
    k = len(b[0])
    work = [row[:] for row in a]
    rhs = [row[:] for row in b]
    for col in range(n):
        pivot = next((r for r in range(col, n) if work[r][col] != 0), None)
        if pivot is None:
            return None
        if pivot != col:
            work[col], work[pivot] = work[pivot], work[col]
            rhs[col], rhs[pivot] = rhs[pivot], rhs[col]
        for r in range(col + 1, n):
            f = work[r][col] / work[col][col]
            for c in range(col, n):
                work[r][c] -= f * work[col][c]
            for j in range(k):
                rhs[r][j] -= f * rhs[col][j]
    x = [[Fraction(0)] * k for _ in range(n)]
    for i in reversed(range(n)):
        for j in range(k):
            s = rhs[i][j] - sum((work[i][c] * x[c][j] for c in range(i + 1, n)), Fraction(0))
            x[i][j] = s / work[i][i]
    return x


def identity(n):
    return [[Fraction(1 if r == c else 0) for c in range(n)] for r in range(n)]


def emit_matrix_cases(file, prefix, a, b=None):
    """The operations on one matrix `a` (and `b` where the operation needs one), written as fixture lines."""
    rows, cols = len(a), len(a[0])
    add(file, f"{prefix}transpose|transpose|{mat_text(a)}|-|{mat_text(transpose(a))}")
    if rows == cols:
        add(file, f"{prefix}trace|trace|{mat_text(a)}|-|{fmt(sum((a[i][i] for i in range(rows)), Fraction(0)))}")
        d = det_exact(a)
        add(file, f"{prefix}det|det|{mat_text(a)}|-|{fmt(d)}")
        inv = solve_exact(a, identity(rows))
        add(file, f"{prefix}inverse|inverse|{mat_text(a)}|-|{'none' if inv is None else mat_text(inv)}")
    if b is not None:
        product_shape = cols == len(b)
        if product_shape:
            add(file, f"{prefix}times|times|{mat_text(a)}|{mat_text(b)}|{mat_text(matmul(a, b))}")
        if rows == cols and len(b) == rows:
            x = solve_exact(a, b)
            add(file, f"{prefix}solve|solve|{mat_text(a)}|{mat_text(b)}|{'none' if x is None else mat_text(x)}")


SINGULAR_MATRICES = {
    # Every pivot of these is exact in binary floating point, so the elimination finds the exact zero pivot too.
    "rank1_2x2": [[1, 2], [2, 4]],
    "zero_row_3x3": [[2, 4, 6], [1, 2, 3], [0, 0, 0]],
    "duplicate_rows_3x3": [[1, 2, 3], [1, 2, 3], [4, 5, 6]],
}

matrix_cases = {
    "m2x2": ([[3, 1], [2, 5]], [[1], [4]]),
    "m2x2_neg": ([[-4, 7], [2, -3]], [[1, 0], [0, 1]]),
    "m3x3": ([[2, -1, 0], [-1, 2, -1], [0, -1, 2]], [[1], [0], [2]]),
    "m3x3_frac": ([[1, 2, 3], [0, 1, 4], [5, 6, 0]], [[1, 2], [3, 4], [5, 6]]),
    "m4x4": ([[4, 1, 0, 0], [1, 4, 1, 0], [0, 1, 4, 1], [0, 0, 1, 4]], [[1], [2], [3], [4]]),
    "m2x3": ([[1, 2, 3], [4, 5, 6]], None),
    "m3x2": ([[1, 2], [3, 4], [5, 6]], None),
}
MATRIX_FIXED = "matrix.kat"
for label, (rows_data, rhs_data) in matrix_cases.items():
    a = [[exact(v) for v in row] for row in rows_data]
    b = [[exact(v) for v in row] for row in rhs_data] if rhs_data is not None else None
    if label == "m2x3":
        add(MATRIX_FIXED, f"{label}_times_3x2|times|{mat_text(a)}|{mat_text([[exact(1), exact(2)], [exact(3), exact(4)], [exact(5), exact(6)]])}|"
                          f"{mat_text(matmul(a, [[exact(1), exact(2)], [exact(3), exact(4)], [exact(5), exact(6)]]))}")
        add(MATRIX_FIXED, f"{label}_transpose|transpose|{mat_text(a)}|-|{mat_text(transpose(a))}")
        continue
    if label == "m3x2":
        add(MATRIX_FIXED, f"{label}_transpose|transpose|{mat_text(a)}|-|{mat_text(transpose(a))}")
        b23 = [[exact(1), exact(2), exact(3)], [exact(4), exact(5), exact(6)]]
        add(MATRIX_FIXED, f"{label}_times_2x3|times|{mat_text(a)}|{mat_text(b23)}|{mat_text(matmul(a, b23))}")
        continue
    emit_matrix_cases(MATRIX_FIXED, f"{label}_", a, b)

for label, rows_data in SINGULAR_MATRICES.items():
    a = [[exact(v) for v in row] for row in rows_data]
    add(MATRIX_FIXED, f"{label}_det|det|{mat_text(a)}|-|0.0")
    add(MATRIX_FIXED, f"{label}_inverse|inverse|{mat_text(a)}|-|none")
    add(MATRIX_FIXED, f"{label}_solve|solve|{mat_text(a)}|{mat_text([[exact(1)] for _ in a])}|none")

# Sums, differences, and scaling of two fixed matrices of the same shape.
add(MATRIX_FIXED, "m2x2_add|add|2x2:1,2,3,4|2x2:10,20,30,40|2x2:11,22,33,44")
add(MATRIX_FIXED, "m2x2_sub|sub|2x2:1,2,3,4|2x2:10,20,30,40|2x2:-9,-18,-27,-36")
add(MATRIX_FIXED, "m3x3_add|add|3x3:1,0,0,0,1,0,0,0,1|3x3:2,2,2,2,2,2,2,2,2|3x3:3,2,2,2,3,2,2,2,3")

# heap-backed matrices of any shape (MA8)
LINALG = "linalg.kat"
for label, rows_data, rhs_data in [
    ("one", [[5]], [[2]]),
    ("two", [[3, 1], [2, 5]], [[1], [4]]),
    ("three", [[6, 1, 1], [4, -2, 5], [2, 8, 7]], [[1], [2], [3]]),
    ("four", [[1, 2, 0, 0], [0, 1, 2, 0], [0, 0, 1, 2], [2, 0, 0, 1]], [[1, 0], [0, 1], [1, 1], [2, 2]]),
    ("five", [[2, 0, 1, 0, 0], [0, 3, 0, 1, 0], [1, 0, 4, 0, 1], [0, 1, 0, 5, 0], [0, 0, 1, 0, 6]],
     [[1], [2], [3], [4], [5]]),
    ("tall", [[1, 2], [3, 4], [5, 6]], None),
    ("wide", [[1, 2, 3, 4], [5, 6, 7, 8]], None),
]:
    a = [[exact(v) for v in row] for row in rows_data]
    b = [[exact(v) for v in row] for row in rhs_data] if rhs_data is not None else None
    if b is not None and len(b) == len(a):
        emit_matrix_cases(LINALG, f"{label}_", a, b)
    else:
        add(LINALG, f"{label}_transpose|transpose|{mat_text(a)}|-|{mat_text(transpose(a))}")
        add(LINALG, f"{label}_times|times|{mat_text(a)}|{mat_text(transpose(a))}|{mat_text(matmul(a, transpose(a)))}")
        add(LINALG, f"{label}_times_back|times|{mat_text(transpose(a))}|{mat_text(a)}|{mat_text(matmul(transpose(a), a))}")
# seeded pseudo-random integer matrices of sizes 2 to 5, nonsingular and singular
rng = Lcg(268)
for size in [2, 3, 3, 4, 4, 5]:
    entries = [rng.below(11) - 5 for _ in range(size * size)]
    a = [[exact(entries[r * size + c]) for c in range(size)] for r in range(size)]
    rhs = [[exact(rng.below(9) - 4)] for _ in range(size)]
    emit_matrix_cases(LINALG, f"random{size}_{len(fixtures.get(LINALG, []))}_", a, rhs)
for label, rows_data in SINGULAR_MATRICES.items():
    a = [[exact(v) for v in row] for row in rows_data]
    add(LINALG, f"dyn_{label}_det|det|{mat_text(a)}|-|0.0")
    add(LINALG, f"dyn_{label}_inverse|inverse|{mat_text(a)}|-|none")
    add(LINALG, f"dyn_{label}_solve|solve|{mat_text(a)}|{mat_text([[exact(1)] for _ in a])}|none")


# ---------------------------------------------------------------------------------------------------------------------
# The identities behind the cases: checked here, so a wrong expected value cannot be written.

def check_identities():
    # gamma(x + 1) = x gamma(x), within the accuracy the package promises.
    for x in [0.5, 1.5, 3.7, 9.2, 40.5]:
        assert abs(math.gamma(x + 1) - x * math.gamma(x)) <= 1e-12 * math.gamma(x + 1), x
    # Binomial and factorial relations used by the fixtures.
    assert math.comb(63, 29) == 759510004936100355
    assert math.comb(63, 29) <= U64_MAX
    # The fixed fixtures hold only the shapes the runner dispatches to.
    assert trial_prime(2**31 - 1) and not trial_prime(561)


check_identities()

# ---------------------------------------------------------------------------------------------------------------------

FILES = [NUM, SCALAR, GAMMA, STATS, VECTOR, MATRIX_FIXED, LINALG]


def render(file):
    header = [
        f"# {file}: known answers for the `math` package (issue #268; spec/math.md MA11).",
        "# Written by tests/math/regenerate.py. Do not edit by hand.",
    ]
    return "\n".join(header + fixtures.get(file, [])) + "\n"


def main(argv):
    check_only = "--check" in argv
    differences = []
    for file in FILES:
        text = render(file)
        path = HERE / file
        if check_only:
            current = path.read_text(encoding="utf-8") if path.exists() else None
            if current != text:
                differences.append(file)
        else:
            path.write_text(text, encoding="utf-8", newline="\n")
    seen = set()
    for file in FILES:
        for line in fixtures.get(file, []):
            name = line.split("|", 1)[0]
            if name in seen:
                raise SystemExit(f"duplicate fixture name {name!r} in {file}")
            seen.add(name)
    if check_only:
        if differences:
            print("out of date: " + ", ".join(differences) + " (run python3 tests/math/regenerate.py)")
            return 1
        print(f"tests/math fixtures are up to date ({len(seen)} cases)")
        return 0
    print(f"wrote {len(seen)} cases to {len(FILES)} fixture files in {HERE}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
