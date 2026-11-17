//! Ids, built-in functions and methods, and operators.

use crate::ty::IntTy;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FuncId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// `Shared.new(value)`: allocates a reference-counted cell holding `value` (`spec/allocators.md`, AL8).
    SharedNew,
    /// `List<T>.new_in(alloc)`: an empty list whose buffer uses the allocator value (`spec/allocators.md`, AL5).
    ListNewIn,
    /// `Map<K, V>.new_in(alloc)`.
    MapNewIn,
    /// `Heap.new_in(alloc, value)`: one heap allocation through the allocator value.
    HeapNewIn,
    /// `__alloc(size, align, site)`: a `Global` allocation, panicking when it fails. The site is the source
    /// location the checking allocator records for a leak report (`spec/allocators.md`, AL9).
    RawAlloc,
    /// `__realloc(ptr, old_size, new_size, align, site)`: grows or shrinks a `Global` allocation.
    RawRealloc,
    /// `__dealloc(ptr, size, align)`: frees a `Global` allocation.
    RawDealloc,
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
    /// `__stdout_write(text)`: appends to the standard-output buffer.
    StdoutWrite,
    /// `__stdout_write_line(text)`: appends `text` and a newline to the standard-output buffer.
    StdoutWriteLine,
    /// `__stdout_flush()`: writes the standard-output buffer now.
    StdoutFlush,
    /// `__stdout_write_bytes(data)`: appends the bytes of a `[u8]` to the standard-output buffer.
    StdoutWriteBytes,
    /// `__stderr_write_bytes(data)`: writes the bytes of a `[u8]` to standard error, unbuffered.
    StderrWriteBytes,
    /// `__stderr_write(text)`: writes `text` to standard error, unbuffered.
    StderrWrite,
    /// `__stderr_write_line(text)`: writes `text` and a newline to standard error, unbuffered.
    StderrWriteLine,
    /// `__is_terminal(fd)`: whether the file descriptor is a terminal.
    IsTerminal,
    /// `__stdin_read_line()`: one line of standard input without its terminator, or `None` at end of input.
    StdinReadLine,
    /// `__stdin_read_to_string()`: the rest of standard input as a `String`.
    StdinReadToString,
    /// `__stdin_avail()`: how many bytes of standard input are buffered, reading a block first when none are; 0 is
    /// the end of the input and -1 a read error.
    StdinAvail,
    /// `__stdin_next_byte()`: takes the next buffered byte of standard input (only after `__stdin_avail` was positive).
    StdinNextByte,
    /// `__stdin_error()`: the OS code of the last standard-input read error (`spec/errors.md`, ERR11).
    StdinError,
    /// `__read_dir(path)`: the names of the entries in a directory (`spec/stdlib.md`, issue #411).
    ReadDir,
    /// `__create_dir(path)`: creates one directory.
    CreateDir,
    /// `__remove_dir(path)`: removes an empty directory.
    RemoveDir,
    /// `__remove_file(path)`: removes a file or a symbolic link.
    RemoveFile,
    /// `__rename(from, to)`: renames a file or a directory, replacing `to`.
    RenamePath,
    /// `__metadata(path)`: `(size, mode, modified, accessed, flags)`, the system's `stat` fields.
    Metadata,
    /// `__canonicalize(path)`: the absolute, normalized path.
    Canonicalize,
    /// `__io_kind(code)`: the portable `IoErrorKind` of an operating system error code (`spec/errors.md`,
    /// ERR11). The compiler knows the target's table (`ioerror`).
    IoErrorKind,
    /// `env.args()`: the process arguments, program name first.
    EnvArgs,
    /// `__env_get(name)`: the raw bytes of an environment variable, or `None` when it is unset (or, on
    /// Windows, not valid Unicode). `spec/stdlib.md`, "Environment variables and the working directory".
    EnvGet,
    /// `__env_set(name, value)`: sets a variable from bytes. The result is `0` on success, `1` for an invalid
    /// name, `2` for an invalid value, and `3` for an operating-system failure.
    EnvSet,
    /// `__env_set_bytes(name, value)`: the same with a byte view instead of a `str`.
    EnvSetBytes,
    /// `__env_remove(name)`: removes a variable. The result is `0` on success, `1` for an invalid name, and `3`
    /// for an operating-system failure.
    EnvRemove,
    /// `__current_dir()`: the raw bytes of the working directory, or `None` when it cannot be read.
    CurrentDir,
    /// `__set_current_dir(path)`: changes the working directory. The result is `0` on success, `1` for an
    /// invalid path, and `3` for an operating-system failure.
    SetCurrentDir,
    /// `__set_current_dir_bytes(path)`: the same with a byte view instead of a `str`.
    SetCurrentDirBytes,
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
    /// `__float_str(v, prec, single)`: the text of a float (see the runtime's `f64_to_string`).
    FloatStr,
    /// `mem.swap(a, b)`: exchanges two places.
    Swap,
    /// `mem.replace(place, value)`: stores a value and returns the old one.
    Replace,
    /// `comptime assert(condition, message)` (`spec/comptime.md`, CT8): only ever inside a comptime expression.
    /// The arguments are the condition and the message (a `str`, empty when none was written).
    ComptimeAssert,
    /// `target.os()`'s primitive: `"linux"`, `"macos"`, or `"windows"`, the system the program is compiled for.
    TargetOs,
    /// `target.arch()`'s primitive: `"x86_64"` or `"aarch64"`.
    TargetArch,
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
    /// `Map<K, V>.get_mut(key): Option<&mut V>`: a mutable view of the value.
    MapGetMut,
    /// `Map<K, V>.get_or_insert(key, value): &mut V`: the value stored under `key`, inserting `value` first when
    /// the key is absent. An existing value is kept and the incoming key and value are dropped.
    MapGetOrInsert,
    /// `Map<K, V>.entry(key): MapEntry<K, V>`: a pending lookup that owns `key` and borrows the map exclusively.
    /// The probe happens once, here; the entry operations insert into the slot it found.
    MapEntry,
    /// `MapEntry<K, V>.or_insert(value): &mut V`: the value stored under the entry's key, inserting `key` and
    /// `value` first when the key is absent. An existing value is kept and the incoming value is dropped.
    MapEntryOrInsert,
    /// `MapEntry<K, V>.or_insert_with(make): &mut V`: like `or_insert`, but the value comes from calling `make`,
    /// which runs only when the key is absent. The closure is consumed either way.
    MapEntryOrInsertWith,
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
    /// `CPtr<T>.is_null(): bool`: whether the raw pointer is null.
    IsNull,
    /// `xs.iter()`: the borrowing iterator of a collection, a range, or `Chars` (IT4).
    Iter,
    /// `xs.into_iter()`: the owning iterator of a `List`, a `Map`, an array, a range, or `Chars` (IT5).
    IntoIter,
    /// `it.next()`: advances an iterator and returns its next item (IT6).
    IterNext,
    /// `x.clone()`: a deep copy derived by the compiler.
    Clone,
    /// `x.hash(): u64` of a type that derives `Hash`.
    Hash,
    /// `shared.get(): &T`: a view of the value a `Shared<T>` owns (`spec/allocators.md`, AL8).
    SharedGet,
    /// `shared.strong_count(): usize`: how many owners the `Shared<T>` has.
    SharedStrongCount,
}

impl Method {
    /// Whether the method takes ownership of argument `index` (the receiver is argument 0).
    pub fn owns_argument(self, index: usize) -> bool {
        match self {
            Method::Push => index == 1,
            Method::ListInsert => index == 2,
            Method::MapInsert | Method::MapGetOrInsert => index == 1 || index == 2,
            // `entry` takes the key it will insert. The entry operations consume the entry and the value (or the
            // closure) they are given.
            Method::MapEntry => index == 1,
            Method::MapEntryOrInsert | Method::MapEntryOrInsertWith => index == 0 || index == 1,
            // Unwrapping consumes the `Option` or `Result`.
            Method::Unwrap | Method::Expect => index == 0,
            // `into_iter` takes the collection, the array, the map, or the range by value.
            Method::IntoIter => index == 0,
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
                | Method::MapGetOrInsert
                | Method::MapEntry
                | Method::MapGetMut
                | Method::MapRemove
                | Method::IterNext
        )
    }

    /// Whether the result is a view derived from the receiver.
    pub fn result_borrows_receiver(self) -> bool {
        matches!(
            self,
            Method::ListLast
                | Method::MapGet
                | Method::MapGetMut
                | Method::MapGetOrInsert
                | Method::MapEntry
                | Method::MapEntryOrInsert
                | Method::MapEntryOrInsertWith
                | Method::Chars
                | Method::Iter
                | Method::IterNext
                | Method::SharedGet
        )
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
        assert!(Method::MapGetOrInsert.owns_argument(1));
        assert!(Method::MapGetOrInsert.owns_argument(2));
        // `entry` moves its key into the entry; the entry operations consume the entry and their value.
        assert!(Method::MapEntry.owns_argument(1));
        assert!(!Method::MapEntry.owns_argument(0));
        for method in [Method::MapEntryOrInsert, Method::MapEntryOrInsertWith] {
            assert!(method.owns_argument(0), "{method:?}");
            assert!(method.owns_argument(1), "{method:?}");
        }
        assert!(Method::Unwrap.owns_argument(0));
        assert!(Method::Expect.owns_argument(0));
        assert!(!Method::Len.owns_argument(0));
        // The lookup methods borrow their key (`contains(self, value: T)`, `get(self, key: K)`).
        assert!(!Method::ListContains.owns_argument(1));
        assert!(!Method::MapGet.owns_argument(1));
        assert!(!Method::MapGetMut.owns_argument(1));
        assert!(!Method::MapRemove.owns_argument(1));
        assert!(!Method::MapContainsKey.owns_argument(1));
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
            Method::MapGetOrInsert,
            Method::MapEntry,
            Method::MapGetMut,
            Method::MapRemove,
        ] {
            assert!(method.mutates_receiver(), "{method:?}");
        }
        for method in [Method::ToString, Method::Len, Method::Clone, Method::ListLast, Method::MapGet, Method::AsStr] {
            assert!(!method.mutates_receiver(), "{method:?}");
        }
        for method in [
            Method::ListLast,
            Method::MapGet,
            Method::MapEntry,
            Method::MapEntryOrInsert,
            Method::MapEntryOrInsertWith,
            Method::Chars,
        ] {
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
