//! The naming table of the design (`spec/lint.md`, LN13): functions, variables, parameters, fields, and modules are
//! `snake_case`; types, interfaces, enum variants, aliases, and type parameters are `PascalCase`; constants are
//! `SCREAMING_SNAKE_CASE`. A rename is not a safe edit (every use must change too), so the findings carry the suggested
//! name in a note and no fix.

use crate::walk::{FnOrigin, Visit, for_each_fn, walk_expr, walk_pattern};
use crate::{Ctx, Finding, Lint};
use burdin_diag::Span;
use burdin_syntax::ast::*;

pub fn check(cx: &Ctx, out: &mut Vec<Finding>) {
    let first = out.len();
    check_files(cx, out);
    // A name the source does not spell (`impl0`, the type parameter of an `impl` argument, or the `comptime assert 1`
    // constants) belongs to generated code. A finding with an empty span (a module name) has no text to compare.
    let mut index = 0;
    out.retain(|f| {
        index += 1;
        index <= first || {
            let text = cx.slice(f.span);
            text.is_empty() || f.message.contains(&format!("`{text}`"))
        }
    });
}

fn check_files(cx: &Ctx, out: &mut Vec<Finding>) {
    for source in &cx.input.files {
        let file = source.ast;
        let whole_file = Span::new(source.file, 0, 0);
        for segment in source.module {
            if !is_snake(segment) {
                out.push(finding(Lint::NonSnakeCase, whole_file, "module", segment, to_snake(segment)));
            }
        }
        for item in &file.items {
            if cx.is_generated(crate::walk::item_span(item)) {
                continue;
            }
            item_names(item, out);
        }
        for_each_fn(file, &|span| cx.is_generated(span), &mut |origin, f| {
            if matches!(origin, FnOrigin::Free | FnOrigin::Method | FnOrigin::InterfaceDecl) && !is_snake(&f.name.name)
            {
                out.push(finding(Lint::NonSnakeCase, f.name.span, "function", &f.name.name, to_snake(&f.name.name)));
            }
            if origin == FnOrigin::Exported {
                return;
            }
            let mut names = Bindings { out: &mut *out };
            for p in &f.params {
                if let ParamKind::Named { name, .. } = &p.kind
                    && !is_snake(&name.name)
                {
                    names.out.push(finding(
                        Lint::NonSnakeCase,
                        name.span,
                        "parameter",
                        &name.name,
                        to_snake(&name.name),
                    ));
                }
                if let Some(d) = &p.default {
                    names.expr(d);
                }
            }
            names.block(&f.body);
        });
    }
}

fn finding(lint: Lint, span: Span, what: &str, name: &str, suggestion: String) -> Finding {
    let style = match lint {
        Lint::NonSnakeCase => "snake_case",
        Lint::NonPascalCase => "PascalCase",
        _ => "SCREAMING_SNAKE_CASE",
    };
    Finding::new(lint, span, format!("{what} `{name}` should be {style}"))
        .note(format!("rename it to `{suggestion}`, or write `// lint: allow({})` above it", lint.name()))
}

fn item_names(item: &Item, out: &mut Vec<Finding>) {
    match item {
        Item::Const(c) => {
            if !is_screaming(&c.name.name) {
                out.push(finding(
                    Lint::NonScreamingCase,
                    c.name.span,
                    "constant",
                    &c.name.name,
                    to_screaming(&c.name.name),
                ));
            }
            let mut names = Bindings { out };
            names.expr(&c.value);
        }
        Item::Struct(s) => {
            pascal(&s.name, "struct", out);
            generics(&s.generics, out);
            fields(&s.fields, out);
        }
        Item::Enum(e) => {
            pascal(&e.name, "enum", out);
            generics(&e.generics, out);
            for v in &e.variants {
                pascal(&v.name, "variant", out);
                if let VariantKind::Struct(fs) = &v.kind {
                    fields(fs, out);
                }
            }
        }
        Item::TypeAlias(t) => {
            pascal(&t.name, "type alias", out);
            generics(&t.generics, out);
        }
        Item::Interface(i) => {
            pascal(&i.name, "interface", out);
            generics(&i.generics, out);
        }
        Item::Derive(d) => pascal(&d.name, "derive", out),
        Item::Fn(f) => generics(&f.generics, out),
        Item::Extension(x) => generics(&x.func.generics, out),
        Item::Impl(b) => b.methods.iter().for_each(|m| generics(&m.generics, out)),
        Item::ErrorEnum(_) | Item::Extern(_) | Item::ExternFn(_) => {}
    }
}

fn pascal(name: &Ident, what: &str, out: &mut Vec<Finding>) {
    if !is_pascal(&name.name) {
        out.push(finding(Lint::NonPascalCase, name.span, what, &name.name, to_pascal(&name.name)));
    }
}

fn fields(fields: &[FieldDecl], out: &mut Vec<Finding>) {
    for f in fields {
        if !is_snake(&f.name.name) {
            out.push(finding(Lint::NonSnakeCase, f.name.span, "field", &f.name.name, to_snake(&f.name.name)));
        }
    }
}

fn generics(params: &[GenericParam], out: &mut Vec<Finding>) {
    for g in params {
        if g.const_ty.is_some() {
            if !is_screaming(&g.name.name) {
                out.push(finding(
                    Lint::NonScreamingCase,
                    g.name.span,
                    "const parameter",
                    &g.name.name,
                    to_screaming(&g.name.name),
                ));
            }
        } else {
            pascal(&g.name, "type parameter", out);
        }
    }
}

/// The bindings an expression or a block introduces: `let`, `for`, `match`, `if let`, and closure parameters.
struct Bindings<'a> {
    out: &'a mut Vec<Finding>,
}

impl Visit for Bindings<'_> {
    fn pattern(&mut self, p: &Pattern) {
        match &p.kind {
            // An uppercase name is a unit variant or a constant, not a binding (`spec/grammar.md`, PatPath).
            PatternKind::Binding { name, .. } if starts_lowercase(&name.name) => self.binding(name),
            PatternKind::At(name, _) | PatternKind::Typed { name: Some(name), .. } => self.binding(name),
            _ => {}
        }
        walk_pattern(self, p);
    }

    fn expr(&mut self, e: &Expr) {
        if let ExprKind::Closure { params, .. } = &e.kind {
            for p in params {
                self.binding(&p.name);
            }
        }
        walk_expr(self, e);
    }
}

impl Bindings<'_> {
    fn binding(&mut self, name: &Ident) {
        if !is_snake(&name.name) {
            self.out.push(finding(Lint::NonSnakeCase, name.span, "variable", &name.name, to_snake(&name.name)));
        }
    }
}

fn starts_lowercase(name: &str) -> bool {
    name.trim_start_matches('_').chars().next().is_some_and(|c| !c.is_uppercase())
}

/// Lowercase letters, digits, and underscores, not starting with a digit.
pub fn is_snake(name: &str) -> bool {
    let body = name.trim_start_matches('_');
    if body.is_empty() {
        return true;
    }
    !body.starts_with(|c: char| c.is_ascii_digit()) && !name.chars().any(|c| c.is_uppercase())
}

/// Starts with an uppercase letter and has no underscores in the middle (`HttpServer`, `V1`, `HTTPServer`).
pub fn is_pascal(name: &str) -> bool {
    let body = name.trim_start_matches('_');
    body.starts_with(|c: char| c.is_uppercase()) && !body.contains('_')
}

/// Uppercase letters, digits, and underscores, starting with a letter (`MAX_SIZE`, `N`).
pub fn is_screaming(name: &str) -> bool {
    let body = name.trim_start_matches('_');
    body.starts_with(|c: char| c.is_uppercase()) && !name.chars().any(|c| c.is_lowercase())
}

pub fn to_snake(name: &str) -> String {
    let leading = name.len() - name.trim_start_matches('_').len();
    let chars: Vec<char> = name.trim_start_matches('_').chars().collect();
    let mut out = "_".repeat(leading);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev = i.checked_sub(1).map(|p| chars[p]);
            let next = chars.get(i + 1).copied();
            let boundary = prev.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit())
                || (prev.is_some_and(|p| p.is_uppercase()) && next.is_some_and(|n| n.is_lowercase()));
            if boundary && !out.ends_with('_') {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

pub fn to_screaming(name: &str) -> String {
    to_snake(name).to_uppercase()
}

pub fn to_pascal(name: &str) -> String {
    let leading = name.len() - name.trim_start_matches('_').len();
    let mut out = "_".repeat(leading);
    for part in name.trim_start_matches('_').split('_').filter(|p| !p.is_empty()) {
        let all_upper = part.chars().all(|c| !c.is_lowercase());
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            if all_upper {
                out.extend(chars.flat_map(|c| c.to_lowercase()));
            } else {
                out.extend(chars);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_styles() {
        for ok in ["x", "area_of", "_unused", "v2", "__burdin_main_result", "_"] {
            assert!(is_snake(ok), "{ok}");
        }
        for bad in ["Area", "areaOf", "2d"] {
            assert!(!is_snake(bad), "{bad}");
        }
        for ok in ["Shape", "HttpServer", "V1", "T", "HTTPServer"] {
            assert!(is_pascal(ok), "{ok}");
        }
        for bad in ["shape", "Http_Server", "http"] {
            assert!(!is_pascal(bad), "{bad}");
        }
        for ok in ["MAX", "MAX_SIZE", "N", "LIMIT_2"] {
            assert!(is_screaming(ok), "{ok}");
        }
        for bad in ["Max", "max_size", "maxSize", "_"] {
            assert!(!is_screaming(bad), "{bad}");
        }
    }

    #[test]
    fn conversions() {
        assert_eq!(to_snake("areaOf"), "area_of");
        assert_eq!(to_snake("HTTPServer"), "http_server");
        assert_eq!(to_snake("Shape"), "shape");
        assert_eq!(to_snake("_Hidden"), "_hidden");
        assert_eq!(to_screaming("maxSize"), "MAX_SIZE");
        assert_eq!(to_pascal("http_server"), "HttpServer");
        assert_eq!(to_pascal("shape"), "Shape");
        assert_eq!(to_pascal("HTTP_SERVER"), "HttpServer");
    }
}
