# Const generics

Milestone 21 adds const generics: values that a generic declaration takes next to its types, and array lengths
computed from them. This page is the exact behavior; the tests are `tests/m21_reflect/` (`const_*`). Rule IDs are
CG1-CG9.

```
struct Matrix<T, const R: usize, const C: usize> {
    data: [T; R * C]
}
```

## CG1. Const parameters

`const NAME: Type` in a generic parameter list declares a const parameter, on a struct, an enum, a function, and an
impl block (through the type's own parameters). The type is an integer type, `bool`, or `char`; any other type is
E0250. A const parameter has no bounds. Its name lives in the same namespace as the type parameters of the
declaration, so repeating a name is E0007.

## CG2. Const arguments

An argument for a const parameter is one of:

- an integer, `true`/`false`, or character literal, with an optional leading `-`: `Matrix<f64, 2, 3>`,
- a const parameter or constant in scope: `Matrix<T, R, C>`, `Buffer<SIZE>`,
- an expression in braces: `Matrix<T, { R + 1 }, C>`.

A type where a value is expected, or a value where a type is expected, is E0250, and so is a value that does not fit
the parameter's type (`Grid<-1>` for a `usize`). The wrong number of arguments is E0003, as for type arguments.

Two instances are the same type exactly when every argument is equal: `Matrix<i32, 2, 3>` and `Matrix<i32, 3, 2>`
are different types, and `Matrix<i32, 2, 3>` is the type of every `Matrix` made for those numbers.

## CG3. Const expressions

Array lengths and `{ ... }` arguments are const expressions: integer, `bool`, and `char` literals, const
parameters, constants (`const SIZE: usize = 4`, including constants built from constants), unary `-`, and the binary
operators `+ - * / % << >> & | ^`. They are computed by the compiler, in 128-bit arithmetic, before type checking
continues. A name that is none of those, a call, an overflow, a division by zero, or an out-of-range shift is E0251.
A constant that refers to itself is E0251 as well.

## CG4. Array lengths

`[T; len]` and `[value; len]` take a const expression. A length is from 0 to 4294967295; anything else is E0251.
This replaces the integer-literal-only rule of stage 0.

## CG5. Using a const parameter as a value

Inside the declaration, the name of a const parameter is an expression with the parameter's type, so
`fn rows(self): usize { R }` returns the number. The value is known per instance: the function body is checked once
symbolically, where the parameter stands for an unknown number, and once for each instance, where it is the number.
The symbolic check treats an array length that mentions a parameter (`R * C`) as the written expression: two lengths
are the same type there only when they are written the same way, so `[T; R * C]` does not match `[T; C * R]`.
Rewrite the code or give the type a name.

## CG6. Inference

A call to a generic function infers a const parameter the way it infers a type parameter: from an array argument
(`fn sum<const N: usize>(a: [i32; N])` called with `[1, 2, 3]` gives `N = 3`), from the arguments of a generic type
(`m: Matrix<T, R, C>`), or from the expected type. A const parameter that nothing determines is E0252. Functions
have no explicit generic arguments at a call; a struct literal takes its arguments from its annotation.

## CG7. Derives

A derive on a type with a const parameter skips it: `Clone, Debug, Eq` on `Tagged<T, const N: usize>`
requires `T: Clone`, `T: Debug`, and `T: Eq`, and nothing for `N`. A user derive (`spec/reflection.md` RFL9) does the
same.

## CG8. What does not exist yet

Const parameters on interfaces, default values for const parameters, a function call in a const expression (a
`comptime` call is `spec/comptime.md` CT2, not an array length), predicates on a const parameter (`where N > 0`),
an `impl` for one specific value (`Matrix<T, 2, 2> { ... }`, E9000 as for any specific instance), and unifying two
different spellings of the same length (CG5) are not part of this milestone.

## CG9. Errors

| Code | Meaning |
| --- | --- |
| E0250 | A const parameter of a type that is not an integer, `bool`, or `char`; a type given where a value is expected or the reverse; a value that does not fit |
| E0251 | A const expression that cannot be computed: an unknown name, overflow, a division by zero, a length out of range, a constant that depends on itself |
| E0252 | A const argument of a call that nothing determines |
| E0003 | The wrong number of arguments, or two different types (as always) |

## Tests

`const_matrix` (the matrix of the design document, methods that read `R` and `C`, inference from an array),
`const_params` (`bool` parameters, constants as lengths, inference from a generic type, derives), `const_invalid`
(E0250), `const_eval_errors` (E0251), and `const_infer` (E0252).
