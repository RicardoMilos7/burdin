//! The compiler's built-in functions, types, and methods, which have no Burdin body (`spec/lsp.md`, LS8, LS12,
//! LS13): `println`, `String.new`, `i32.to_string`, `List.push`, and the rest of what `burdin_sema` provides itself
//! (`spec/stdlib.md`, "Compiler-known types"). Their signatures and documentation are written once, as
//! declarations without bodies, in `std/src/builtins.bd`. The compiler never reads that file; the server parses it
//! like a standard-library module and indexes it, so hover, completion, signature help, and go to definition
//! answer from the same declarations.

use crate::index::{FileIndex, Item};
use burdin_diag::{Diagnostics, FileId};
use std::sync::OnceLock;

/// The path the databases would give the file; it is materialized beside the standard library's modules.
pub const PATH: &str = "<std>/builtins.bd";

/// The declarations, from the standard library's sources.
const SOURCE: &str = include_str!("../../../std/src/builtins.bd");

/// The file's text and its index.
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
        let mut diags = Diagnostics::default();
        let ast = burdin_syntax::parse(FileId(0), SOURCE, &mut diags);
        let mut index = FileIndex::new(&ast, SOURCE);
        // A method reads `fn push(mut self, ...)` on its type, not `fn List<T>.push(...)` as the file spells it.
        for item in &mut index.items {
            if item.owner.is_some()
                && let Some(rest) = item.detail.strip_prefix("fn ")
                && let Some(at) = rest.find(&format!(".{}", item.name))
            {
                item.detail = format!("fn {}", &rest[at + 1..]);
            }
        }
        Builtins { text: SOURCE.to_string(), index }
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
    use crate::index::Kind;

    #[test]
    fn the_file_parses_and_its_spans_point_at_the_names() {
        let mut diags = Diagnostics::default();
        burdin_syntax::parse(FileId(0), SOURCE, &mut diags);
        assert!(!diags.has_errors(), "std/src/builtins.bd has syntax errors");
        let b = Builtins::new();
        for item in &b.index.items {
            assert_eq!(&b.text[item.name_span.0 as usize..item.name_span.1 as usize], item.name, "{item:?}");
        }
    }

    #[test]
    fn owners_and_signatures() {
        let b = Builtins::new();
        let println = b.find("", "println").unwrap();
        assert!(println.detail.contains("println(fmt: str, ...)"), "{}", println.detail);
        assert!(b.find("i32", "wrapping_add").unwrap().detail.contains("wrapping_add(self, other: i32): i32"));
        assert!(b.find("f32", "to_bits").unwrap().detail.contains("): u32"));
        assert!(b.find("u8", "abs").is_none(), "an unsigned type has no `abs`");
        assert!(b.find("str", "len").is_some() && b.find("String", "len").is_some());
        // Arrays and slices: `[T]` has the compiler's methods, and only an array has `iter_mut`.
        assert!(b.find("[T]", "swap").is_some());
        assert!(b.find("[T]", "iter_mut").is_none() && b.find("[T; N]", "iter_mut").is_some());
        assert!(b.index.items.iter().any(|i| i.name == "Option" && i.kind == Kind::Enum));
    }
}
