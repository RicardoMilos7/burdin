//! `derives (Debug)`: the generated `Type: Debug` impl block (spec/derives.md).
//!
//! The impl is written as Burdin source text, parsed with the ordinary parser, and added to the file's items, so
//! a derived `Debug` is the same thing as one written by hand. Every token of the generated text carries the span
//! of the `Debug` in the derive clause, which is where a diagnostic about the generated code is reported.

use crate::ast::*;
use crate::lexer::lex;
use crate::parser::parse_file;
use burdin_diag::{Diagnostics, FileId, Span};

/// Appends the `Debug` impl of every struct and enum that derives it.
pub fn expand_derives(file: &mut File, id: FileId, diags: &mut Diagnostics) {
    let mut generated: Vec<(String, Span)> = Vec::new();
    for item in &file.items {
        match item {
            Item::Struct(s) => {
                if let Some(span) = debug_derive(&s.derives) {
                    generated.push((struct_impl(s), span));
                }
            }
            Item::Enum(e) => {
                if let Some(span) = debug_derive(&e.derives) {
                    generated.push((enum_impl(e), span));
                }
            }
            _ => {}
        }
    }
    for (text, span) in generated {
        // The generated text is lexed and parsed like a file of its own; only its spans are replaced, so that
        // they point into the real file.
        let mut scratch = Diagnostics::default();
        let mut lexed = lex(id, &text, &mut scratch);
        for token in &mut lexed.tokens {
            token.span = span;
        }
        let parsed = parse_file(id, lexed, &mut scratch);
        debug_assert!(!scratch.has_errors(), "generated Debug impl does not parse:\n{text}");
        let _ = &diags;
        file.items.extend(parsed.items);
    }
}

fn debug_derive(derives: &[Type]) -> Option<Span> {
    derives.iter().find_map(|t| match &t.kind {
        TypeKind::Path(segs) if segs.len() == 1 && segs[0].args.is_empty() && segs[0].name.name == "Debug" => {
            Some(t.span)
        }
        _ => None,
    })
}

/// `Name<A, B>: Debug where A: Debug, B: Debug`.
fn head(name: &str, generics: &[GenericParam]) -> String {
    if generics.is_empty() {
        return format!("{name}: Debug");
    }
    let params: Vec<&str> = generics.iter().map(|g| g.name.name.as_str()).collect();
    // A const parameter is a number: it has nothing to implement.
    let wheres: Vec<String> =
        generics.iter().filter(|g| g.const_ty.is_none()).map(|g| format!("{}: Debug", g.name.name)).collect();
    if wheres.is_empty() {
        return format!("{name}<{}>: Debug", params.join(", "));
    }
    format!("{name}<{}>: Debug where {}", params.join(", "), wheres.join(", "))
}

/// The statements that write `fields` as `Name { a: .., b: .. }`, each value read with `value(index)`.
fn named_fields(name: &str, fields: &[String], value: &dyn Fn(usize) -> String) -> String {
    if fields.is_empty() {
        return format!("        f.write_str(\"{name}\")\n");
    }
    let mut out = String::new();
    for (i, field) in fields.iter().enumerate() {
        let lead = if i == 0 { format!("{name} {{{{ ") } else { ", ".to_string() };
        out.push_str(&format!("        f.write_str(\"{lead}{field}: \")\n"));
        out.push_str(&format!("        {}.debug(mut f)\n", value(i)));
    }
    out.push_str("        f.write_str(\" }}\")\n");
    out
}

fn tuple_fields(name: &str, count: usize, value: &dyn Fn(usize) -> String) -> String {
    let mut out = format!("        f.write_str(\"{name}(\")\n");
    for i in 0..count {
        if i > 0 {
            out.push_str("        f.write_str(\", \")\n");
        }
        out.push_str(&format!("        {}.debug(mut f)\n", value(i)));
    }
    out.push_str("        f.write_str(\")\")\n");
    out
}

fn struct_impl(s: &StructDecl) -> String {
    let name = &s.name.name;
    let fields: Vec<String> = s.fields.iter().map(|f| f.name.name.clone()).collect();
    let body = named_fields(name, &fields, &|i| format!("self.{}", fields[i]));
    format!("{} {{\n    fn debug(self, mut f: Formatter) {{\n{body}    }}\n}}\n", head(name, &s.generics))
}

fn enum_impl(e: &EnumDecl) -> String {
    let name = &e.name.name;
    let mut arms = String::new();
    for v in &e.variants {
        let variant = &v.name.name;
        match &v.kind {
            VariantKind::Unit => {
                arms.push_str(&format!(
                    "        {name}.{variant} => {{\n            f.write_str(\"{variant}\")\n        }}\n"
                ));
            }
            VariantKind::Tuple(types) => {
                let binds: Vec<String> = (0..types.len()).map(|i| format!("__a{i}")).collect();
                let body = tuple_fields(variant, types.len(), &|i| format!("__a{i}"));
                arms.push_str(&format!("        {name}.{variant}({}) => {{\n{body}        }}\n", binds.join(", ")));
            }
            VariantKind::Struct(fields) => {
                let names: Vec<String> = fields.iter().map(|f| f.name.name.clone()).collect();
                let binds: Vec<String> = names.iter().enumerate().map(|(i, n)| format!("{n}: __a{i}")).collect();
                let body = named_fields(variant, &names, &|i| format!("__a{i}"));
                arms.push_str(&format!(
                    "        {name}.{variant} {{ {} }} => {{\n{body}        }}\n",
                    binds.join(", ")
                ));
            }
        }
    }
    if e.variants.is_empty() {
        arms.push_str("        else => {}\n");
    }
    format!(
        "{} {{\n    fn debug(self, mut f: Formatter) {{\n        match self {{\n{arms}        }}\n    }}\n}}\n",
        head(name, &e.generics)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn derived(src: &str) -> usize {
        let mut diags = Diagnostics::default();
        let file = parse(FileId(0), src, &mut diags);
        assert!(!diags.has_errors());
        file.items.iter().filter(|i| matches!(i, Item::Impl(_))).count()
    }

    #[test]
    fn adds_one_impl_per_derive() {
        assert_eq!(derived("struct P derives (Debug) {\n x: i32\n y: i32\n}\n"), 1);
        assert_eq!(derived("enum E derives (Debug, Eq) {\n A\n B(i32, i32)\n C { w: i32 }\n}\n"), 1);
        assert_eq!(derived("struct W<T> derives (Debug) {\n v: T\n}\nstruct Q derives (Eq) {\n a: i32\n}\n"), 1);
        assert_eq!(derived("struct E0 derives (Debug) {\n}\n"), 1);
    }
}
