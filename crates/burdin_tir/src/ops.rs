//! Ids, built-in functions and methods, and operators.

use crate::ty::{FloatTy, IntTy};

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
    /// `List<T>.with_capacity(n)`: an empty list with room for `n` elements in the default allocator.
    ListWithCapacity,
    /// `List<T>.with_capacity_in(n, alloc)`: an empty list with room for `n` elements, allocated through the
    /// allocator value, which the list then stores.
    ListWithCapacityIn,
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
    /// `Map<K, V>.with_capacity(n)`: an empty map whose slot table holds `n` entries without growing.
    MapWithCapacity,
    /// `i32.parse(str)` (and every integer type): the number, or an error.
    IntParse(IntTy),
    /// `read_to_string(path)`: the file's bytes as a `String`.
    ReadToString,
    /// `write_file(path, text)`: writes the bytes.
    WriteFile,
    /// `__file_open(path, read, write, append, truncate, create, create_new)`: opens a file and returns its
    /// descriptor (`spec/stdlib.md`, "Files and file handles").
    FileOpen,
    /// `__file_read(fd, buf, max)`: appends up to `max` bytes of the file to `buf` and returns how many.
    FileRead,
    /// `__file_write(fd, data)`: writes a prefix of `data` and returns how many bytes it took.
    FileWrite,
    /// `__file_seek(fd, offset, whence)`: moves the position and returns it (`whence` is 0, 1, or 2).
    FileSeek,
    /// `__file_sync(fd)`: makes the file's contents durable.
    FileSync,
    /// `__file_set_len(fd, size)`: truncates the file, or extends it with zeros.
    FileSetLen,
    /// `__file_close(fd)`: closes the descriptor.
    FileClose,
    /// `__file_error()`: the OS code of the last failed file operation (`spec/errors.md`, ERR11).
    FileError,
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
    /// `__clock(kind)`: nanoseconds from the real-time clock (kind 0, since the Unix epoch) or the monotonic clock
    /// (any other kind, from an unspecified start) (`spec/stdlib.md`, issue #195).
    Clock,
    /// `__entropy()`: 64 random bits from the operating system.
    Entropy,
    /// `__sleep(nanos)`: suspends the thread for about that long.
    Sleep,
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
    /// `__env_list()`: every environment entry as `NAME=VALUE`, in the order the system reports them.
    EnvList,
    /// `__process_spawn(program, args, env, cwd, command_line, stdin, stdout, stderr)`: starts a child without a
    /// shell. The tuple is `(pid, process, stdin, stdout, stderr)`; `process` is the Windows process handle (0 on
    /// Unix) and each pipe handle is 0 when the stream is not piped. `command_line` is the quoted Windows command
    /// line and is ignored on Unix.
    ProcessSpawn,
    /// `__process_wait(pid, process)`: waits for the child. The result is the exit code, `-1` for a failure to
    /// wait, or `-(signal + 1)` when a signal ended the child.
    ProcessWait,
    /// `__process_close(process)`: releases the Windows process handle. It does nothing on Unix.
    ProcessClose,
    /// `__pipe_read_to_string(handle)`: every byte left in the pipe.
    PipeReadToString,
    /// `__pipe_read_line(handle)`: one line without its terminator, or `None` at the end of the pipe.
    PipeReadLine,
    /// `__pipe_read_both(first, second)`: both pipes read to the end, in one pass, so a child writing a lot to one
    /// of them cannot block the other.
    PipeReadBoth,
    /// `__pipe_write(handle, text)`: writes every byte. The result is `0` on success and `-1` on failure.
    PipeWrite,
    /// `__pipe_close(handle)`: closes a pipe end. It is a no-op for handle 0.
    PipeClose,
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
    /// `f64.from_bits(bits)` and `f32.from_bits(bits)`: the value with those bits (`spec/stdlib.md`, "Float
    /// functions").
    FloatFromBits(FloatTy),
    /// `__view_as_text(view)`: reinterprets a `[u8]` view as a `str` without checking it. `std` uses it after
    /// validating the bytes (`str.from_utf8`); it requires `unsafe` (E0161).
    ViewAsText,
}

/// A floating-point function the compiler provides on `f64` and `f32` (`spec/stdlib.md`, "Float functions").
/// The receiver's type picks the precision: an exactly rounded operation lowers to an LLVM intrinsic, and one
/// without an intrinsic calls the C math library. The functions `std` can write in Burdin are extension methods
/// there instead (`min`, `max`, `clamp`, `signum`, and the predicates).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatOp {
    Abs,
    Sqrt,
    Cbrt,
    Powi,
    Powf,
    Exp,
    Exp2,
    Ln,
    Log2,
    Log10,
    Floor,
    Ceil,
    Round,
    RoundTiesEven,
    Trunc,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Sinh,
    Cosh,
    Tanh,
    Hypot,
    MulAdd,
    Copysign,
    /// `to_bits(self)`: the bit pattern as an unsigned integer of the same width.
    ToBits,
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
    /// An `f64` or `f32` function (`spec/stdlib.md`, "Float functions"); the receiver's type picks the
    /// precision.
    Float(FloatOp),
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
    /// `List<T>.first(): Option<&T>`: a view of the first element.
    ListFirst,
    /// `List<T>.first_mut(): Option<&mut T>`: a mutable view of the first element.
    ListFirstMut,
    /// `List<T>.last_mut(): Option<&mut T>`: a mutable view of the last element.
    ListLastMut,
    /// `List<T>.get(index): Option<&T>`: a view of one element, or `None` when the index is out of bounds.
    ListGet,
    /// `List<T>.get_mut(index): Option<&mut T>`: a mutable view of one element, or `None`.
    ListGetMut,
    /// `List<T>.capacity(): usize`: how many elements fit without growing.
    ListCapacity,
    /// `List<T>.is_empty(): bool`: whether the list has no elements.
    ListIsEmpty,
    /// `List<T>.reserve(additional)`: makes room for `additional` more elements.
    ListReserve,
    /// `List<T>.reserve_exact(additional)`: makes room for exactly `additional` more elements.
    ListReserveExact,
    /// `List<T>.shrink_to_fit()`: releases the capacity past the length.
    ListShrinkToFit,
    /// `List<T>.swap(a, b)`: exchanges two elements.
    ListSwap,
    /// `List<T>.swap_remove(index): T`: removes one element by moving the last element into its place.
    ListSwapRemove,
    /// `List<T>.truncate(len)`: drops the elements from `len` on.
    ListTruncate,
    /// `List<T>.split_off(index): List<T>`: moves the tail into a new list with the default allocator.
    ListSplitOff,
    /// `List<T>.append(other)`: moves every element of `other` to the end, then frees `other`'s buffer.
    ListAppend,
    /// `List<T>.drain(range): ListIntoIter<T>`: removes a range and yields the removed elements as an owning
    /// iterator over a new buffer with the default allocator.
    ListDrain,
    /// `List<T>.extend_from_slice(other)`: appends a clone of every element of a slice view.
    ListExtendFromSlice,
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
    /// `Map<K, V>.get_or(key, default): &V`: the read-only form of `get_or_insert`; an existing value is kept and
    /// the incoming key and default are dropped.
    MapGetOr,
    /// `Map<K, V>.remove_entry(key): Option<(K, V)>`: the stored key and value, moved out.
    MapRemoveEntry,
    /// `Map<K, V>.retain(f)`: keeps the entries `f` accepts and drops the rest.
    MapRetain,
    /// `Map<K, V>.reserve(additional)`: grows the slot table so it can hold `additional` more entries.
    MapReserve,
    /// `Map<K, V>.shrink_to_fit()`: shrinks the slot table to the smallest one that holds the entries.
    MapShrinkToFit,
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
    /// `str.as_bytes()` and `String.as_bytes()`: the UTF-8 bytes as a `[u8]` view.
    AsBytes,
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
    /// `xs.iter_mut()`: the mutable iterator of a `List`, an array, or a `Map` (IT15). It yields `&mut T` and borrows
    /// the collection exclusively.
    IterMut,
    /// `xs.into_iter()`: the owning iterator of a `List`, a `Map`, an array, a range, or `Chars` (IT5).
    IntoIter,
    /// `it.next()`: advances an iterator and returns its next item (IT6).
    IterNext,
    /// `it.next_back()`: takes the last item of a double-ended built-in iterator (IT14).
    IterNextBack,
    /// `x.clone()`: a deep copy derived by the compiler.
    Clone,
    /// `x.hash(): u64` of a type that derives `Hash`.
    Hash,
    /// `shared.get(): &T`: a view of the value a `Shared<T>` owns (`spec/allocators.md`, AL8).
    SharedGet,
    /// `shared.strong_count(): usize`: how many owners the `Shared<T>` has.
    SharedStrongCount,
    /// `List<T>.as_slice(): [T]`: a view of the list's elements (`spec/stdlib.md`, "Slices").
    AsSlice,
    /// `[T].get(i): Option<&T>`: a view of the element at `i`, or `None` when it is out of bounds.
    SliceGet,
    /// `[T].last(): Option<&T>`: a view of the last element.
    SliceLast,
    /// `[T].to_list(): List<T>`: an owned list holding a clone of each element.
    SliceToList,
    /// `[T].get(range): Option<[T]>`: a view of the sub-slice, or `None` when the range is out of bounds or
    /// reversed. The argument is a `Range<usize>` or `RangeInclusive<usize>`.
    SliceGetRange,
    /// `[T].swap(a, b)`: exchanges two elements in place.
    SliceSwap,
    /// `[T].fill(value)`: overwrites every element with a clone of `value`.
    SliceFill,
    /// `[T].copy_from_slice(other)`: copies every element of `other` into the receiver.
    SliceCopyFromSlice,
    /// `[T].clone_from_slice(other)`: clones every element of `other` into the receiver.
    SliceCloneFromSlice,
    /// `[T].chunks(n)` and `[T].chunks_exact(n)`: the iterator over fixed-size windows of the slice.
    SliceChunks,
    /// `[T].windows(n)`: the iterator over the overlapping windows of the slice.
    SliceWindows,
    /// `[T].__sort_by(less)`: sorts the slice in place with a comparator that says whether its first argument is
    /// strictly less. Insertion sort, so the sort is stable. `std`'s `sort_by`/`sort`/`sort_by_key` call it.
    SliceSortBy,
}

impl Method {
    /// Whether the method takes ownership of argument `index` (the receiver is argument 0).
    pub fn owns_argument(self, index: usize) -> bool {
        match self {
            Method::Push => index == 1,
            Method::ListInsert => index == 2,
            Method::ListAppend => index == 1,
            Method::MapInsert | Method::MapGetOrInsert | Method::MapGetOr => index == 1 || index == 2,
            // `retain` consumes the closure it is given.
            Method::MapRetain => index == 1,
            // `entry` takes the key it will insert. The entry operations consume the entry and the value (or the
            // closure) they are given.
            Method::MapEntry => index == 1,
            Method::MapEntryOrInsert | Method::MapEntryOrInsertWith => index == 0 || index == 1,
            // The slice sort consumes its comparator.
            Method::SliceSortBy => index == 1,
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
                | Method::ListReserve
                | Method::ListReserveExact
                | Method::ListShrinkToFit
                | Method::ListSwap
                | Method::ListSwapRemove
                | Method::ListTruncate
                | Method::ListSplitOff
                | Method::ListAppend
                | Method::ListDrain
                | Method::ListExtendFromSlice
                | Method::ListGetMut
                | Method::ListFirstMut
                | Method::ListLastMut
                | Method::Clear
                | Method::MapInsert
                | Method::MapGetOrInsert
                | Method::MapGetOr
                | Method::MapEntry
                | Method::MapGetMut
                | Method::MapRemove
                | Method::MapRemoveEntry
                | Method::MapRetain
                | Method::MapReserve
                | Method::MapShrinkToFit
                | Method::IterMut
                | Method::IterNext
                | Method::IterNextBack
                | Method::SliceSwap
                | Method::SliceFill
                | Method::SliceCopyFromSlice
                | Method::SliceCloneFromSlice
                | Method::SliceSortBy
        )
    }

    /// Whether the result is a view derived from the receiver.
    pub fn result_borrows_receiver(self) -> bool {
        matches!(
            self,
            Method::ListLast
                | Method::ListFirst
                | Method::ListFirstMut
                | Method::ListLastMut
                | Method::ListGet
                | Method::ListGetMut
                | Method::MapGet
                | Method::MapGetMut
                | Method::MapGetOrInsert
                | Method::MapGetOr
                | Method::MapEntry
                | Method::MapEntryOrInsert
                | Method::MapEntryOrInsertWith
                | Method::Chars
                | Method::AsBytes
                | Method::Iter
                | Method::IterMut
                | Method::IterNext
                | Method::IterNextBack
                | Method::SharedGet
                | Method::AsSlice
                | Method::SliceGet
                | Method::SliceLast
                | Method::SliceGetRange
                | Method::SliceChunks
                | Method::SliceWindows
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
        assert!(Method::MapGetOr.owns_argument(1));
        assert!(Method::MapGetOr.owns_argument(2));
        assert!(Method::MapRetain.owns_argument(1));
        assert!(!Method::MapRemoveEntry.owns_argument(1));
        // `entry` moves its key into the entry; the entry operations consume the entry and their value.
        assert!(Method::MapEntry.owns_argument(1));
        assert!(!Method::MapEntry.owns_argument(0));
        for method in [Method::MapEntryOrInsert, Method::MapEntryOrInsertWith] {
            assert!(method.owns_argument(0), "{method:?}");
            assert!(method.owns_argument(1), "{method:?}");
        }
        assert!(Method::Unwrap.owns_argument(0));
        assert!(Method::Expect.owns_argument(0));
        // `append` takes the other list by ownership; the capacity operations take only a count.
        assert!(Method::ListAppend.owns_argument(1));
        assert!(!Method::ListAppend.owns_argument(0));
        for method in [Method::ListReserve, Method::ListReserveExact, Method::ListTruncate, Method::ListDrain] {
            assert!(!method.owns_argument(0), "{method:?}");
            assert!(!method.owns_argument(1), "{method:?}");
        }
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
            Method::ListReserve,
            Method::ListReserveExact,
            Method::ListShrinkToFit,
            Method::ListSwap,
            Method::ListSwapRemove,
            Method::ListTruncate,
            Method::ListSplitOff,
            Method::ListAppend,
            Method::ListDrain,
            Method::ListExtendFromSlice,
            Method::ListGetMut,
            Method::ListFirstMut,
            Method::ListLastMut,
            Method::Clear,
            Method::MapInsert,
            Method::MapGetOrInsert,
            Method::MapGetOr,
            Method::MapEntry,
            Method::MapGetMut,
            Method::MapRemove,
            Method::MapRemoveEntry,
            Method::MapRetain,
            Method::MapReserve,
            Method::MapShrinkToFit,
            Method::IterMut,
        ] {
            assert!(method.mutates_receiver(), "{method:?}");
        }
        for method in [Method::ToString, Method::Len, Method::Clone, Method::ListLast, Method::MapGet, Method::AsStr] {
            assert!(!method.mutates_receiver(), "{method:?}");
        }
        // The capacity and length queries only read the list and hand out no view.
        for method in [Method::ListCapacity, Method::ListIsEmpty, Method::ListFirst, Method::ListGet] {
            assert!(!method.mutates_receiver(), "{method:?}");
        }
        for method in [Method::ListCapacity, Method::ListIsEmpty] {
            assert!(!method.result_borrows_receiver(), "{method:?}");
        }
        for method in [Method::ListFirst, Method::ListGet] {
            assert!(method.result_borrows_receiver(), "{method:?}");
        }
        for method in [
            Method::ListLast,
            Method::ListFirst,
            Method::ListFirstMut,
            Method::ListLastMut,
            Method::ListGet,
            Method::ListGetMut,
            Method::MapGet,
            Method::MapEntry,
            Method::MapEntryOrInsert,
            Method::MapEntryOrInsertWith,
            Method::Chars,
            Method::IterMut,
        ] {
            assert!(method.result_borrows_receiver(), "{method:?}");
        }
        for method in [Method::Push, Method::ListPop, Method::ListContains, Method::Len] {
            assert!(!method.result_borrows_receiver(), "{method:?}");
        }
        // `get_mut` and the `*_mut` accessors need a mutable receiver and hand out a view of it.
        for method in [Method::ListGetMut, Method::ListFirstMut, Method::ListLastMut] {
            assert!(method.mutates_receiver(), "{method:?}");
            assert!(method.result_borrows_receiver(), "{method:?}");
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
