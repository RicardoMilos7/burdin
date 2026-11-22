//! The text of a `comptime generate` block, parsed into declarations (`spec/generate.md`, GN4 and GN6).

use crate::ast::*;
use crate::{derive, lexer, parser, spans};
use burdin_diag::{Diagnostics, Severity, Span};

/// Parses `text` as the items of a module and returns its `struct`, `enum`, and `fn` declarations, each with every
/// span replaced by `at`, the keyword of the block that generated it, so a diagnostic about generated code lands on
/// the block. `Err` is the E0242 message: the parser's own, with its line and column inside `text`, or the name of
/// the item that may not be generated.
pub fn parse_generated(text: &str, at: Span) -> Result<Vec<Item>, String> {
    let mut diags = Diagnostics::default();
    let lexed = lexer::lex(at.file, text, &mut diags);
    let mut file = parser::parse_file(at.file, lexed, &mut diags);
    if let Some(error) = diags.iter().find(|d| d.severity == Severity::Error) {
        // The diagnostic's span points into `text`, not into a source file.
        let before = &text[..(error.span.lo as usize).min(text.len())];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().map_or(0, |last| last.chars().count()) + 1;
        return Err(format!(
            "the generated text does not parse: {} (line {line}, column {column} of the text)",
            error.message
        ));
    }
    if !file.generators.is_empty() {
        return Err(not_allowed("a nested `comptime generate`"));
    }
    if !file.uses.is_empty() {
        return Err(not_allowed("a `use`"));
    }
    if !file.tests.is_empty() {
        return Err(not_allowed("a `test` item"));
    }
    if !file.benches.is_empty() {
        return Err(not_allowed("a `bench` item"));
    }
    if !file.fuzzes.is_empty() {
        return Err(not_allowed("a `fuzz` item"));
    }
    for item in &file.items {
        let what = match item {
            Item::Fn(_) | Item::Struct(_) | Item::Enum(_) => continue,
            Item::Extension(_) => "an extension method",
            Item::Const(c) if c.name.name.starts_with("comptime assert") => "a `comptime assert`",
            Item::Const(_) => "a `const`",
            Item::ErrorEnum(_) => "an `error enum`",
            Item::TypeAlias(_) => "a `type` alias",
            Item::Impl(_) => "an `impl` block",
            Item::Interface(_) => "an `interface`",
            Item::Extern(_) | Item::ExternFn(_) => "an `extern` declaration",
            Item::Derive(_) => "a `derive`",
        };
        return Err(not_allowed(what));
    }
    spans::map_spans(&mut file, &mut |span| *span = at);
    // A `struct` that derives `Debug` gets its impl block, as a hand-written one does.
    derive::expand_derives(&mut file, at.file, &mut diags);
    Ok(file.items)
}

fn not_allowed(what: &str) -> String {
    format!("the generated text declares {what}; only a `struct`, an `enum`, or a `fn` may be generated")
}

#[cfg(test)]
mod tests {
    use super::*;
    use burdin_diag::FileId;

    fn at() -> Span {
        Span::new(FileId(3), 10, 26)
    }

    #[test]
    fn declarations_take_the_span_of_the_block() {
        let items = parse_generated("struct P { x: i32 }\nfn f(): i32 { 1 }\n", at()).expect("it parses");
        assert_eq!(items.len(), 2);
        let Item::Struct(s) = &items[0] else { panic!("a struct") };
        assert_eq!((s.name.span, s.span), (at(), at()));
        let Item::Fn(f) = &items[1] else { panic!("a function") };
        assert_eq!((f.name.span, f.body.span), (at(), at()));
    }

    #[test]
    fn an_empty_text_declares_nothing() {
        assert!(parse_generated("", at()).expect("it parses").is_empty());
    }

    #[test]
    fn a_parse_error_names_its_position_in_the_text() {
        let message = parse_generated("fn ok() {}\nfn broken( {\n", at()).expect_err("it does not parse");
        assert!(message.contains("line 2, column 12"), "{message}");
    }

    #[test]
    fn only_structs_enums_and_functions_are_allowed() {
        for (text, what) in [
            ("const N: i32 = 1\n", "a `const`"),
            ("use a.b\n", "a `use`"),
            ("Point: Display { fn f(self) {} }\n", "an `impl` block"),
            ("type A = i32\n", "a `type` alias"),
            ("interface I { fn f(self) }\n", "an `interface`"),
            ("comptime assert(true)\n", "a `comptime assert`"),
            ("comptime generate { String.new() }\n", "a nested `comptime generate`"),
            ("test \"t\" { }\n", "a `test` item"),
        ] {
            let message = parse_generated(text, at()).expect_err(text);
            assert!(message.contains(what), "{text}: {message}");
        }
    }
}
