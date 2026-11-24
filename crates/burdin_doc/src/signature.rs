//! Declarations as a reader sees them (`spec/doc.md`, DC9): the source text of the declaration's head, with its
//! whitespace collapsed to single spaces and without `priv` or the body. The text is the author's own, so defaults,
//! generic bounds, `where` clauses, and `impl Interface` parameters appear as written.

use burdin_diag::Span;
use burdin_syntax::ast::*;

/// Collapses every run of whitespace to one space.
pub(crate) fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn slice(text: &str, span: Span) -> &str {
    text.get(span.lo as usize..span.hi as usize).unwrap_or("")
}

fn between(text: &str, lo: u32, hi: u32) -> String {
    let s = text.get(lo as usize..hi as usize).unwrap_or("");
    let s = collapse(s);
    match s.strip_prefix("priv ") {
        Some(rest) => rest.to_string(),
        None => s,
    }
}

/// A type as written.
pub(crate) fn type_text(text: &str, ty: &Type) -> String {
    collapse(slice(text, ty.span))
}

/// A function's head: everything before its body.
pub(crate) fn fn_head(text: &str, f: &FnDecl) -> String {
    let end = if f.body.span.lo == f.span.lo { f.span.hi } else { f.body.span.lo };
    between(text, f.span.lo, end)
}

/// A declaration's head up to the `{` that opens its body.
pub(crate) fn brace_head(text: &str, span: Span) -> String {
    let body = text.get(span.lo as usize..span.hi as usize).unwrap_or("");
    let end = body.find('{').map_or(span.hi, |i| span.lo + i as u32);
    between(text, span.lo, end)
}

pub(crate) fn whole(text: &str, span: Span) -> String {
    between(text, span.lo, span.hi)
}

/// `const NAME: Type = value`, with the value left out when it is long or spans lines.
pub(crate) fn const_head(text: &str, c: &ConstDecl) -> String {
    let head = between(text, c.span.lo, c.value.span.lo);
    let value = collapse(slice(text, c.value.span));
    if value.len() <= 60 && !slice(text, c.value.span).contains('\n') {
        format!("{head} {value}")
    } else {
        head.trim_end().trim_end_matches('=').trim_end().to_string()
    }
}

/// `x: i32` for a field.
pub(crate) fn field_text(text: &str, f: &FieldDecl) -> String {
    format!("{}: {}", f.name.name, type_text(text, &f.ty))
}

/// `Name`, `Name(A, B)`, or `Name { a: A, b: B }`; for an `error enum`, the source of the variant to the end of its
/// line, which includes its message.
pub(crate) fn variant_text(text: &str, v: &Variant, error: bool) -> String {
    if error {
        let from = v.name.span.lo as usize;
        let rest = text.get(from..).unwrap_or("");
        let line = rest.lines().next().unwrap_or("");
        return collapse(line).trim_end_matches(',').to_string();
    }
    match &v.kind {
        VariantKind::Unit => v.name.name.clone(),
        VariantKind::Tuple(types) => {
            let inner: Vec<String> = types.iter().map(|t| type_text(text, t)).collect();
            format!("{}({})", v.name.name, inner.join(", "))
        }
        VariantKind::Struct(fields) => {
            let inner: Vec<String> = fields.iter().map(|f| field_text(text, f)).collect();
            format!("{} {{ {} }}", v.name.name, inner.join(", "))
        }
    }
}

/// A `where` clause as written: `where T: Display, U: Clone`.
pub(crate) fn where_text(text: &str, preds: &[WherePred]) -> String {
    if preds.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = preds
        .iter()
        .map(|p| {
            let bounds: Vec<String> = p.bounds.iter().map(|b| type_text(text, b)).collect();
            format!("{}: {}", type_text(text, &p.ty), bounds.join(" + "))
        })
        .collect();
    format!("where {}", parts.join(", "))
}

/// The operator a method of an operator interface stands for, written the way a call is, or `None` when the method
/// does not belong to that interface.
pub(crate) fn operator(interface: &str, method: &str) -> Option<&'static str> {
    Some(match (interface, method) {
        ("Add", "add") => "a + b",
        ("Sub", "sub") => "a - b",
        ("Mul", "mul") => "a * b",
        ("Div", "div") => "a / b",
        ("Rem", "rem") => "a % b",
        ("BitAnd", "bitand") => "a & b",
        ("BitOr", "bitor") => "a | b",
        ("BitXor", "bitxor") => "a ^ b",
        ("Neg", "neg") => "-a",
        ("Not", "not") => "!a",
        ("Eq", "eq") => "a == b",
        ("Eq", "ne") => "a != b",
        ("Ord", "lt") => "a < b",
        ("Ord", "le") => "a <= b",
        ("Ord", "gt") => "a > b",
        ("Ord", "ge") => "a >= b",
        ("Index", "index") => "a[i]",
        ("IndexMut", "index_mut") => "a[i] = v",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use burdin_diag::{Diagnostics, FileId};

    fn parse(src: &str) -> File {
        let mut d = Diagnostics::default();
        let f = burdin_syntax::parse(FileId(0), src, &mut d);
        assert!(!d.has_errors(), "{}", src);
        f
    }

    #[test]
    fn function_heads_drop_the_body_and_priv() {
        let src = "fn add<T: Add>(a: T,\n    b: T = zero()): T\n    where T: Copy {\n    a + b\n}\nextern \"C\" {\n    fn abs(x: i32): i32\n}\n";
        let ast = parse(src);
        let Item::Fn(f) = &ast.items[0] else { panic!() };
        assert_eq!(fn_head(src, f), "fn add<T: Add>(a: T, b: T = zero()): T where T: Copy");
        let Item::Extern(b) = &ast.items[1] else { panic!() };
        assert_eq!(fn_head(src, &b.decls[0]), "fn abs(x: i32): i32");
    }

    #[test]
    fn type_heads_stop_at_the_brace() {
        let src = "priv struct P<T>: Copy, Eq {\n    x: T\n}\nenum E {\n    A(i32), B { w: i32 }\n}\ntype Pair<T> = (T, T)\nconst N: i32 = 10\n";
        let ast = parse(src);
        let Item::Struct(s) = &ast.items[0] else { panic!() };
        assert_eq!(brace_head(src, s.span), "struct P<T>: Copy, Eq");
        let Item::Enum(e) = &ast.items[1] else { panic!() };
        assert_eq!(brace_head(src, e.span), "enum E");
        assert_eq!(variant_text(src, &e.variants[0], false), "A(i32)");
        assert_eq!(variant_text(src, &e.variants[1], false), "B { w: i32 }");
        let Item::TypeAlias(a) = &ast.items[2] else { panic!() };
        assert_eq!(whole(src, a.span), "type Pair<T> = (T, T)");
        let Item::Const(c) = &ast.items[3] else { panic!() };
        assert_eq!(const_head(src, c), "const N: i32 = 10");
    }

    #[test]
    fn operators_belong_to_their_interface() {
        assert_eq!(operator("Add", "add"), Some("a + b"));
        assert_eq!(operator("Add", "sub"), None);
        assert_eq!(operator("Display", "add"), None);
    }
}
