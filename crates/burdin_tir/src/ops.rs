//! Ids, built-in functions and methods, and operators.

use crate::ty::IntTy;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FuncId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConstId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Print,
    Println,
    Eprint,
    Eprintln,
    Panic,
    /// `String.new()`: an empty string.
    StringNew,
    /// `String.from(str)`: a new string with a copy of the bytes.
    StringFrom,
    /// `Heap.new(value)`: allocates and moves `value` into a fresh heap allocation.
    HeapNew,
    /// `CPtr.null()`: a null C pointer, typed by the expected type.
    CPtrNull,
    /// `CString.from(str)`: a new NUL-terminated copy of the bytes in the C heap.
    CStringFrom,
    /// `CStr.to_string(ptr)`: copies a NUL-terminated C string into a `String`.
    CStrToString,
    /// `List<T>.new()`: an empty list.
    ListNew,
    /// `Map<K, V>.new()`: an empty map.
    MapNew,
    /// `i32.parse(str)` (and every integer type): the number, or an error.
    IntParse(IntTy),
    /// `read_to_string(path)`: the file's bytes as a `String`.
    ReadToString,
    /// `write_file(path, text)`: writes the bytes.
    WriteFile,
    /// `env.args()`: the process arguments, program name first.
    EnvArgs,
    /// `process.exit(code)`: ends the process.
    ProcessExit,
    /// `assert(condition)`: panics when it is false.
    Assert,
    /// `assert_eq(a, b)`: panics when the two values differ.
    AssertEq,
    /// `unreachable()`: panics.
    Unreachable,
    /// `process.run(command)`: runs a command through the system shell.
    RunCommand,
    /// `mem.swap(a, b)`: exchanges two places.
    Swap,
    /// `mem.replace(place, value)`: stores a value and returns the old one.
    Replace,
}

/// Built-in methods of the primitive types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    ToString,
    WrappingAdd,
    WrappingSub,
    WrappingMul,
    CheckedAdd,
    CheckedSub,
    CheckedMul,
    Min,
    Max,
    Abs,
    /// `String.push_str(str)`: appends bytes, growing the buffer.
    PushStr,
    /// `String.push(char)`: appends one UTF-8 character.
    PushChar,
    /// `List<T>.push(value)`: appends an element.
    Push,
    /// `List<T>.pop(): Option<T>`: removes and returns the last element.
    ListPop,
    /// `List<T>.last(): Option<&T>`: a view of the last element.
    ListLast,
    /// `List<T>.insert(index, value)`: inserts at `index`.
    ListInsert,
    /// `List<T>.remove(index): T`: removes and returns the element at `index`.
    ListRemove,
    /// `List<T>.contains(value): bool`.
    ListContains,
    /// `List<T>.sort()`: orders the elements with `Ord`.
    ListSort,
    /// `List<T>.clear()` and `Map<K, V>.clear()`: drops the elements and leaves the collection empty.
    Clear,
    /// `Map<K, V>.insert(key, value)`: adds or replaces a value.
    MapInsert,
    /// `Map<K, V>.get(key): Option<&V>`: a view of the value.
    MapGet,
    /// `Map<K, V>.remove(key): Option<V>`.
    MapRemove,
    /// `Map<K, V>.contains_key(key): bool`.
    MapContainsKey,
    /// `Option<T>.unwrap()` and `Result<T, E>.unwrap()`: the payload, panicking on the empty case.
    Unwrap,
    /// `Option<T>.expect(message)` and `Result<T, E>.expect(message)`.
    Expect,
    /// `Option<T>.is_some()`.
    IsSome,
    /// `Option<T>.is_none()`.
    IsNone,
    /// `Result<T, E>.is_ok()`.
    IsOk,
    /// `Result<T, E>.is_err()`.
    IsErr,
    /// `str.find_str(needle): Option<usize>`: the byte index of a substring.
    FindStr,
    /// `str.chars()`: the characters of a string, for a `for` loop.
    Chars,
    /// `String.as_str()`: a `str` view of the bytes.
    AsStr,
    /// `x.len()` on `str`, `[T]`, `String`, and arrays.
    Len,
    /// `s.starts_with(prefix)` on `str` (and `String`).
    StartsWith,
    /// `s.find(c): Option<usize>` on `str` (and `String`); a byte index.
    Find,
    /// `CString.as_ptr(): CPtr<CChar>`: the address of the NUL-terminated bytes.
    AsPtr,
    /// `x.clone()`: a deep copy derived by the compiler.
    Clone,
    /// `x.hash(): u64` of a type that derives `Hash`.
    Hash,
}

impl Method {
    /// Whether the method takes ownership of argument `index` (the receiver is argument 0).
    pub fn owns_argument(self, index: usize) -> bool {
        match self {
            Method::Push => index == 1,
            Method::ListInsert => index == 2,
            Method::MapInsert => index == 1 || index == 2,
            // Unwrapping consumes the `Option` or `Result`.
            Method::Unwrap | Method::Expect => index == 0,
            _ => false,
        }
    }

    /// Whether the method changes its receiver, so the call borrows it exclusively.
    pub fn mutates_receiver(self) -> bool {
        matches!(
            self,
            Method::PushStr
                | Method::PushChar
                | Method::Push
                | Method::ListPop
                | Method::ListInsert
                | Method::ListRemove
                | Method::ListSort
                | Method::Clear
                | Method::MapInsert
                | Method::MapRemove
        )
    }

    /// Whether the result is a view derived from the receiver.
    pub fn result_borrows_receiver(self) -> bool {
        matches!(self, Method::ListLast | Method::MapGet | Method::Chars)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_value_parameters_are_owned() {
        assert!(Method::Push.owns_argument(1));
        assert!(!Method::Push.owns_argument(0));
        assert!(Method::ListInsert.owns_argument(2));
        assert!(Method::MapInsert.owns_argument(1));
        assert!(Method::MapInsert.owns_argument(2));
        assert!(Method::Unwrap.owns_argument(0));
        assert!(Method::Expect.owns_argument(0));
        assert!(!Method::Len.owns_argument(0));
        assert!(!Method::ListContains.owns_argument(1));
    }

    #[test]
    fn mutation_and_borrowed_results_follow_the_method() {
        for method in [
            Method::PushStr,
            Method::PushChar,
            Method::Push,
            Method::ListPop,
            Method::ListInsert,
            Method::ListRemove,
            Method::ListSort,
            Method::Clear,
            Method::MapInsert,
            Method::MapRemove,
        ] {
            assert!(method.mutates_receiver(), "{method:?}");
        }
        for method in [Method::ToString, Method::Len, Method::Clone, Method::ListLast, Method::MapGet, Method::AsStr] {
            assert!(!method.mutates_receiver(), "{method:?}");
        }
        for method in [Method::ListLast, Method::MapGet, Method::Chars] {
            assert!(method.result_borrows_receiver(), "{method:?}");
        }
        for method in [Method::Push, Method::ListPop, Method::ListContains, Method::Len] {
            assert!(!method.result_borrows_receiver(), "{method:?}");
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}
