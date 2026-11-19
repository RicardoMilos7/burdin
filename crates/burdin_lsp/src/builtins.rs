//! The compiler's built-in functions, types, and methods, which have no Burdin source (`spec/lsp.md`, LS8, LS12,
//! LS13): `println`, `String.new`, `i32.to_string`, `List.push`, and the rest of what `burdin_sema` provides itself
//! (`spec/stdlib.md`). The server writes them out as one read-only file, `<std>/builtins.bd`, so a definition has a
//! place to land, and indexes it like a standard-library module, so hover, completion, and signature help answer from
//! the same declarations. The file is never compiled.

use crate::index::{FileIndex, Item, Kind, tokens};
use std::sync::OnceLock;

/// The path the databases would give the file; it is materialized beside the standard library's modules.
pub const PATH: &str = "<std>/builtins.bd";

/// The integer types.
const INTS: &[&str] = &["i8", "i16", "i32", "i64", "isize", "u8", "u16", "u32", "u64", "usize"];
const SIGNED: &[&str] = &["i8", "i16", "i32", "i64", "isize"];
const FLOATS: &[&str] = &["f32", "f64"];

/// The built-in types, in the order the file declares them, with the header and the documentation.
const TYPES: &[(&str, &str, &str)] = &[
    ("String", "struct String", "The owning, growable UTF-8 text."),
    ("str", "struct str", "A read-only view of UTF-8 text; a string literal has this type."),
    ("i8", "struct i8", "A signed 8-bit integer."),
    ("i16", "struct i16", "A signed 16-bit integer."),
    ("i32", "struct i32", "A signed 32-bit integer; an integer literal's type unless the context says otherwise."),
    ("i64", "struct i64", "A signed 64-bit integer."),
    ("isize", "struct isize", "A signed integer the size of a pointer (64 bits)."),
    ("u8", "struct u8", "An unsigned 8-bit integer."),
    ("u16", "struct u16", "An unsigned 16-bit integer."),
    ("u32", "struct u32", "An unsigned 32-bit integer."),
    ("u64", "struct u64", "An unsigned 64-bit integer."),
    ("usize", "struct usize", "An unsigned integer the size of a pointer (64 bits)."),
    ("f32", "struct f32", "A 32-bit IEEE 754 float."),
    ("f64", "struct f64", "A 64-bit IEEE 754 float; a float literal's type unless the context says otherwise."),
    ("bool", "struct bool", "`true` or `false`."),
    ("char", "struct char", "A Unicode scalar value."),
    ("List", "struct List<T>", "A growable list of values, stored contiguously."),
    ("Map", "struct Map<K, V>", "A hash map from keys to values."),
    ("Option", "enum Option<T>", "`Some(T)` or `None`."),
    ("Result", "enum Result<T, E>", "`Ok(T)` or `Err(E)`; `Result<T>` means `Result<T, Error>`."),
    ("Heap", "struct Heap<T>", "One value in its own allocation."),
];

/// The built-in functions and methods: who has them (`""` for a free function; `int`, `signed`, `float`, and `text`
/// stand for groups of types), the signature with `Self` for the receiver's type, and the documentation.
const DECLS: &[(&str, &str, &str)] = &[
    ("", "fn print(fmt: str, ...)", "Writes the text to standard output; `{...}` placeholders take the arguments."),
    ("", "fn println(fmt: str, ...)", "Writes the text and a line feed to standard output."),
    ("", "fn eprint(fmt: str, ...)", "Writes the text to standard error."),
    ("", "fn eprintln(fmt: str, ...)", "Writes the text and a line feed to standard error."),
    ("", "fn format(fmt: str, ...): String", "The text, with the placeholders filled in, as a new `String`."),
    ("", "fn panic(message: str): !", "Stops the program with a message."),
    // `String` and `str`.
    ("String", "fn new(): String", "An empty string."),
    ("String", "fn from(s: str): String", "A copy of the text."),
    ("String", "fn with_capacity(capacity: usize): String", "An empty string with room for `capacity` bytes."),
    ("String", "fn push_str(mut self, s: str)", "Appends the text."),
    ("String", "fn push(mut self, c: char)", "Appends one character."),
    ("text", "fn len(self): usize", "The length in bytes, not characters."),
    ("text", "fn starts_with(self, prefix: str): bool", "Whether the text starts with `prefix`."),
    ("text", "fn find(self, c: char): Option<usize>", "The byte offset of the first `c`."),
    ("text", "fn find_str(self, needle: str): Option<usize>", "The byte offset of the first `needle`."),
    ("text", "fn as_str(self): str", "A `str` view of the text."),
    ("text", "fn chars(self): Chars", "The Unicode scalar values, in order."),
    ("text", "fn as_bytes(self): [u8]", "A view of the UTF-8 bytes; copies nothing."),
    ("String", "fn capacity(self): usize", "The buffer's capacity in bytes."),
    ("String", "fn reserve(mut self, additional: usize)", "Room for `additional` more bytes."),
    ("String", "fn shrink_to_fit(mut self)", "Shrinks the buffer to the length."),
    ("String", "fn insert_str(mut self, byte_idx: usize, s: str)", "Inserts the text at a character boundary."),
    (
        "String",
        "fn replace_range(mut self, from: usize, to: usize, s: str)",
        "Replaces the bytes `from..to` with the text.",
    ),
    ("String", "fn clone(self): String", "A deep copy of the buffer."),
    ("str", "fn to_string(self): String", "A `String` copy of the text."),
    // Numbers, `bool`, and `char`.
    ("int", "fn parse(s: str): Result<Self, ParseIntError>", "The base-10 number in the text (`Self.parse(s)`)."),
    ("int", "fn to_string(self): String", "The number in base 10."),
    ("int", "fn wrapping_add(self, other: Self): Self", "The sum, wrapping around on overflow."),
    ("int", "fn wrapping_sub(self, other: Self): Self", "The difference, wrapping around on overflow."),
    ("int", "fn wrapping_mul(self, other: Self): Self", "The product, wrapping around on overflow."),
    ("int", "fn checked_add(self, other: Self): Option<Self>", "The sum, or `None` on overflow."),
    ("int", "fn checked_sub(self, other: Self): Option<Self>", "The difference, or `None` on overflow."),
    ("int", "fn checked_mul(self, other: Self): Option<Self>", "The product, or `None` on overflow."),
    ("int", "fn min(self, other: Self): Self", "The smaller of the two."),
    ("int", "fn max(self, other: Self): Self", "The larger of the two."),
    ("signed", "fn abs(self): Self", "The absolute value."),
    ("float", "fn from_bits(bits: u64): Self", "The float with these bits (`u32` for `f32`)."),
    ("float", "fn to_string(self): String", "The shortest text that reads back as the same number."),
    ("float", "fn to_bits(self): u64", "The float's bits (`u32` for `f32`)."),
    ("float", "fn abs(self): Self", "The absolute value."),
    ("float", "fn sqrt(self): Self", "The square root."),
    ("float", "fn cbrt(self): Self", "The cube root."),
    ("float", "fn powi(self, exp: i32): Self", "The number to an integer power."),
    ("float", "fn powf(self, exp: Self): Self", "The number to a float power."),
    ("float", "fn exp(self): Self", "`e` to the power of the number."),
    ("float", "fn exp2(self): Self", "2 to the power of the number."),
    ("float", "fn ln(self): Self", "The natural logarithm."),
    ("float", "fn log2(self): Self", "The base-2 logarithm."),
    ("float", "fn log10(self): Self", "The base-10 logarithm."),
    ("float", "fn floor(self): Self", "The largest integer not above the number."),
    ("float", "fn ceil(self): Self", "The smallest integer not below the number."),
    ("float", "fn round(self): Self", "The nearest integer, halves away from zero."),
    ("float", "fn round_ties_even(self): Self", "The nearest integer, halves to even."),
    ("float", "fn trunc(self): Self", "The integer part."),
    ("float", "fn sin(self): Self", "The sine (radians)."),
    ("float", "fn cos(self): Self", "The cosine (radians)."),
    ("float", "fn tan(self): Self", "The tangent (radians)."),
    ("float", "fn asin(self): Self", "The arcsine, in radians."),
    ("float", "fn acos(self): Self", "The arccosine, in radians."),
    ("float", "fn atan(self): Self", "The arctangent, in radians."),
    ("float", "fn atan2(self, other: Self): Self", "The angle of the point `(other, self)`, in radians."),
    ("float", "fn sinh(self): Self", "The hyperbolic sine."),
    ("float", "fn cosh(self): Self", "The hyperbolic cosine."),
    ("float", "fn tanh(self): Self", "The hyperbolic tangent."),
    ("float", "fn hypot(self, other: Self): Self", "The length of the hypotenuse."),
    ("float", "fn mul_add(self, a: Self, b: Self): Self", "`self * a + b` with one rounding."),
    ("float", "fn copysign(self, sign: Self): Self", "The number with the sign of `sign`."),
    ("bool", "fn to_string(self): String", "`true` or `false`."),
    ("char", "fn to_string(self): String", "The character as a one-character `String`."),
    // `List<T>`.
    ("List", "fn new(): List<T>", "An empty list."),
    ("List", "fn with_capacity(n: usize): List<T>", "An empty list with room for `n` values."),
    ("List", "fn new_in(alloc: A): List<T, A>", "An empty list that stores the allocator."),
    ("List", "fn with_capacity_in(n: usize, alloc: A): List<T, A>", "`with_capacity` with an allocator."),
    ("List", "fn push(mut self, value: T)", "Appends the value; takes ownership of it."),
    ("List", "fn pop(mut self): Option<T>", "Removes and returns the last value."),
    ("List", "fn len(self): usize", "The number of values."),
    ("List", "fn capacity(self): usize", "How many values fit without growing."),
    ("List", "fn is_empty(self): bool", "Whether the list has no values."),
    ("List", "fn first(self): Option<&T>", "The first value."),
    ("List", "fn last(self): Option<&T>", "The last value."),
    ("List", "fn first_mut(mut self): Option<&mut T>", "The first value, to change."),
    ("List", "fn last_mut(mut self): Option<&mut T>", "The last value, to change."),
    ("List", "fn get(self, index: usize): Option<&T>", "The value at `index`, or `None` past the end."),
    ("List", "fn get_mut(mut self, index: usize): Option<&mut T>", "The value at `index`, to change."),
    ("List", "fn reserve(mut self, additional: usize)", "Room for `additional` more values."),
    ("List", "fn reserve_exact(mut self, additional: usize)", "Room for exactly `additional` more values."),
    ("List", "fn shrink_to_fit(mut self)", "Shrinks the buffer to the length."),
    ("List", "fn insert(mut self, index: usize, value: T)", "Inserts the value at `index`."),
    ("List", "fn remove(mut self, index: usize): T", "Removes and returns the value at `index`."),
    ("List", "fn swap(mut self, a: usize, b: usize)", "Swaps two values."),
    ("List", "fn swap_remove(mut self, index: usize): T", "Removes the value, moving the last one into its place."),
    ("List", "fn truncate(mut self, len: usize)", "Keeps the first `len` values."),
    ("List", "fn split_off(mut self, at: usize): List<T>", "The values from `at` on, as a new list."),
    ("List", "fn append(mut self, other: List<T>)", "Moves every value of `other` to the end."),
    ("List", "fn drain(mut self, range: Range<usize>): ListIntoIter<T>", "Removes the values in the range."),
    ("List", "fn extend_from_slice(mut self, values: [T])", "Appends copies of the values."),
    ("List", "fn contains(self, value: T): bool", "Whether a value equals `value`."),
    ("List", "fn sort(mut self)", "Sorts the values ascending."),
    ("List", "fn clear(mut self)", "Removes every value."),
    ("List", "fn as_slice(self): [T]", "A view of the values."),
    ("List", "fn iter(self): [T]", "Iterates over the values."),
    ("List", "fn iter_mut(mut self): IterMut<T>", "Iterates over the values, to change them."),
    ("List", "fn into_iter(own self): ListIntoIter<T>", "Consumes the list, yielding its values."),
    // `Map<K, V>`.
    ("Map", "fn new(): Map<K, V>", "An empty map."),
    ("Map", "fn with_capacity(n: usize): Map<K, V>", "An empty map with room for `n` entries."),
    ("Map", "fn new_in(alloc: A): Map<K, V, A>", "An empty map that stores the allocator."),
    ("Map", "fn insert(mut self, key: K, value: V)", "Inserts the entry, replacing an existing key's value."),
    ("Map", "fn get(self, key: K): Option<&V>", "The value of the key."),
    ("Map", "fn get_mut(mut self, key: K): Option<&mut V>", "The value of the key, to change."),
    ("Map", "fn get_or(self, key: K, fallback: V): &V", "The value of the key, or `fallback`."),
    ("Map", "fn get_or_insert(mut self, key: K, value: V): &mut V", "The value of the key, inserted when missing."),
    ("Map", "fn remove(mut self, key: K): Option<V>", "Removes the key and returns its value."),
    ("Map", "fn remove_entry(mut self, key: K): Option<(K, V)>", "Removes the key and returns the entry."),
    ("Map", "fn contains_key(self, key: K): bool", "Whether the key is in the map."),
    ("Map", "fn entry(mut self, key: K): MapEntry<K, V>", "The key's entry, to insert or update in one probe."),
    ("Map", "fn retain(mut self, keep: FnMut(&K, &V): bool)", "Keeps the entries `keep` accepts."),
    ("Map", "fn reserve(mut self, additional: usize)", "Room for `additional` more entries."),
    ("Map", "fn shrink_to_fit(mut self)", "Shrinks the table to the entries."),
    ("Map", "fn clear(mut self)", "Removes every entry."),
    ("Map", "fn len(self): usize", "The number of entries."),
    ("Map", "fn iter(self): MapIter<K, V>", "Iterates over the entries as `(&K, &V)`."),
    ("Map", "fn iter_mut(mut self): MapIterMut<K, V>", "Iterates over the entries, to change the values."),
    ("Map", "fn into_iter(own self): MapIntoIter<K, V>", "Consumes the map, yielding `(K, V)`."),
    // `Option<T>`, `Result<T, E>`, and `Heap<T>`.
    ("Option", "fn unwrap(own self): T", "The value; panics on `None`."),
    ("Option", "fn expect(own self, msg: str): T", "The value; panics with `msg` on `None`."),
    ("Option", "fn is_some(self): bool", "Whether there is a value."),
    ("Option", "fn is_none(self): bool", "Whether there is no value."),
    ("Result", "fn unwrap(own self): T", "The value; panics on `Err`."),
    ("Result", "fn expect(own self, msg: str): T", "The value; panics with `msg` on `Err`."),
    ("Result", "fn is_ok(self): bool", "Whether it is `Ok`."),
    ("Result", "fn is_err(self): bool", "Whether it is `Err`."),
    ("Result", "fn context(own self, message: str): Result<T, Error>", "Prefixes a context message to the error."),
    ("Heap", "fn new(value: T): Heap<T>", "Moves the value into a new allocation."),
    ("Heap", "fn new_in(alloc: A, value: T): Heap<T, A>", "`new`, with an allocator."),
];

/// The types an owner of [`DECLS`] stands for.
fn owners(owner: &'static str) -> Vec<&'static str> {
    match owner {
        "int" => INTS.to_vec(),
        "signed" => SIGNED.to_vec(),
        "float" => FLOATS.to_vec(),
        "text" => vec!["String", "str"],
        other => vec![other],
    }
}

/// The signature of a declaration on `ty`: `Self` replaced, and the float bit types made exact.
fn signature(sig: &str, ty: &str) -> String {
    let mut s = sig.replace("Self", ty);
    if ty == "f32" {
        s = s.replace("bits: u64", "bits: u32").replace("): u64", "): u32");
    }
    s
}

/// The file's text and its index, built together so every span is exact.
pub struct Builtins {
    pub text: String,
    pub index: FileIndex,
}

/// The one copy of the file, built on first use.
pub fn get() -> &'static Builtins {
    static BUILTINS: OnceLock<Builtins> = OnceLock::new();
    BUILTINS.get_or_init(Builtins::new)
}

impl Builtins {
    fn new() -> Builtins {
        let mut text = String::from(
            "//! The compiler's built-in functions, types, and methods (`spec/stdlib.md`). They have no Burdin source;\n\
             //! the language server shows these declarations for hover, completion, signature help, and go to\n\
             //! definition. This file is generated by the server and is not compiled.\n\n",
        );
        let mut items: Vec<Item> = Vec::new();
        let doc = |text: &mut String, indent: &str, d: &str| {
            text.push_str(&format!("{indent}/// {d}\n"));
        };
        let push_fn = |text: &mut String,
                       items: &mut Vec<Item>,
                       indent: &str,
                       sig: &str,
                       d: &str,
                       owner: Option<(&str, usize)>| {
            doc(text, indent, d);
            text.push_str(indent);
            let lo = text.len() as u32;
            text.push_str(sig);
            let hi = text.len() as u32;
            text.push('\n');
            let name_lo = lo + 3;
            let name_len = sig[3..].find(['(', '<']).unwrap_or(0) as u32;
            items.push(Item {
                name: sig[3..3 + name_len as usize].to_string(),
                kind: if owner.is_some() { Kind::Method } else { Kind::Function },
                name_span: (name_lo, name_lo + name_len),
                span: (lo, hi),
                parent: owner.map(|(_, p)| p),
                owner: owner.map(|(o, _)| o.to_string()),
                interface: None,
                requires: Vec::new(),
                detail: sig.to_string(),
                docs: Some(d.to_string()),
                body: None,
                is_priv: false,
            });
        };
        for (_, sig, d) in DECLS.iter().filter(|(o, _, _)| o.is_empty()) {
            push_fn(&mut text, &mut items, "", sig, d, None);
            text.push('\n');
        }
        for (name, header, d) in TYPES {
            doc(&mut text, "", d);
            let lo = text.len() as u32;
            text.push_str(header);
            let name_lo = lo + header.find(' ').map_or(0, |n| n + 1) as u32;
            let parent = items.len();
            let kind = if header.starts_with("enum") { Kind::Enum } else { Kind::Struct };
            items.push(Item {
                name: name.to_string(),
                kind,
                name_span: (name_lo, name_lo + name.len() as u32),
                span: (lo, lo),
                parent: None,
                owner: None,
                interface: None,
                requires: Vec::new(),
                detail: header.to_string(),
                docs: Some(d.to_string()),
                body: None,
                is_priv: false,
            });
            text.push_str(" {\n");
            for (_, sig, d) in DECLS.iter().filter(|(o, _, _)| !o.is_empty() && owners(o).contains(name)) {
                push_fn(&mut text, &mut items, "    ", &signature(sig, name), d, Some((name, parent)));
            }
            text.push_str("}\n\n");
            items[parent].span.1 = text.len() as u32 - 2;
        }
        text.pop();
        let index = FileIndex { items, uses: Vec::new(), tokens: tokens(&text) };
        Builtins { text, index }
    }

    /// The declaration of a built-in function (`owner` empty) or of a method or associated function of `owner`.
    pub fn find(&self, owner: &str, name: &str) -> Option<&Item> {
        self.index
            .items
            .iter()
            .find(|i| i.name == name && i.kind.is_function() && i.owner.as_deref().unwrap_or("") == owner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_point_at_the_names_and_owners_expand() {
        let b = Builtins::new();
        for item in &b.index.items {
            assert_eq!(&b.text[item.name_span.0 as usize..item.name_span.1 as usize], item.name, "{item:?}");
        }
        let println = b.find("", "println").unwrap();
        assert_eq!(println.detail, "fn println(fmt: str, ...)");
        assert_eq!(b.find("i32", "wrapping_add").unwrap().detail, "fn wrapping_add(self, other: i32): i32");
        assert_eq!(b.find("f32", "to_bits").unwrap().detail, "fn to_bits(self): u32");
        assert!(b.find("u8", "abs").is_none(), "an unsigned type has no `abs`");
        assert!(b.find("str", "len").is_some() && b.find("String", "len").is_some());
        // Every type's methods sit inside its declaration.
        let string = b.index.items.iter().position(|i| i.name == "String" && i.kind == Kind::Struct).unwrap();
        let new = b.find("String", "new").unwrap();
        assert_eq!(new.parent, Some(string));
        let span = b.index.items[string].span;
        assert!(span.0 <= new.span.0 && new.span.1 <= span.1);
    }
}
