//! Milestone 16's lowering (spec/errors.md): `error enum` declarations become an ordinary enum plus generated
//! `Display`, `Error`, and `From` impl blocks, and two small wrappers turn `fn main(): Result<...>` and typed test
//! failures into the forms the rest of the compiler already understands.
//!
//! Every generated impl is written as Burdin source text, parsed with the ordinary parser, and added to the file's
//! items, so an error type is an ordinary value and nothing about it is special to the checker, the memory checks,
//! or code generation. Every token of the generated text carries the span of the declaration's name, which is where
//! a diagnostic about the generated code is reported.

use crate::ast::*;
use crate::lexer::lex;
use crate::parser::parse_file;
use burdin_diag::{Code, Diagnostics, FileId, Span};

const INVALID_ERROR: Code = Code::INVALID_ERROR;
const DUPLICATE_FROM: Code = Code::DUPLICATE_FROM;

/// Replaces every `error` declaration with the enum and impls it lowers to. Called before `derive::expand_derives`
/// so a `derives (...)` clause on the lowered enum still gets its derived impls.
pub fn lower_error_enums(file: &mut File, id: FileId, diags: &mut Diagnostics) {
    let items = std::mem::take(&mut file.items);
    let mut lowered = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Item::ErrorEnum(e) => lowered.extend(lower_error_enum(e, id, diags)),
            other => lowered.push(other),
        }
    }
    file.items = lowered;
}

fn lower_error_enum(e: ErrorEnumDecl, id: FileId, diags: &mut Diagnostics) -> Vec<Item> {
    let name_span = e.name.span;
    // Validate the templates and the `from` clauses first, while the variants can still be borrowed.
    let messages: Vec<String> = e.variants.iter().map(|v| message_source(v, diags)).collect();
    let mut seen: Vec<String> = Vec::new();
    let mut conversions: Vec<(String, String)> = Vec::new();
    for v in &e.variants {
        if let Some(source) = from_source(&e, v, &mut seen, diags) {
            conversions.push((v.name.name.clone(), source));
        }
    }
    let mut sources = vec![display_impl(&e, &messages), error_impl(&e)];
    for (variant, source) in &conversions {
        sources.push(from_impl(&e.name.name, variant, source));
    }

    let mut items = Vec::with_capacity(1 + sources.len());
    items.push(Item::Enum(EnumDecl {
        is_priv: e.is_priv,
        name: e.name,
        generics: e.generics,
        derives: e.derives,
        variants: e.variants.into_iter().map(|v| Variant { name: v.name, kind: v.kind }).collect(),
        span: e.span,
    }));
    for text in sources {
        items.extend(parse_generated(&text, id, name_span));
    }
    items
}

/// Parses one piece of generated source text with every token at `span`, like `derive::expand_derives` does.
fn parse_generated(text: &str, id: FileId, span: Span) -> Vec<Item> {
    let mut scratch = Diagnostics::default();
    let mut lexed = lex(id, text, &mut scratch);
    for token in &mut lexed.tokens {
        token.span = span;
        // An interpolated string carries its holes' own spans (the lexer lexed each hole separately); every one of
        // them points into the generated text, so remap them too, or a diagnostic about the generated message
        // lands at a scratch offset.
        if let crate::token::TokenKind::Interp(pieces) = &mut token.kind {
            for piece in pieces {
                if let crate::token::InterpPiece::Hole(hole) = piece {
                    hole.span = span;
                    if let Some((_, spec_span)) = &mut hole.spec {
                        *spec_span = span;
                    }
                    for inner in &mut hole.tokens {
                        inner.span = span;
                    }
                }
            }
        }
    }
    let parsed = parse_file(id, lexed, &mut scratch);
    debug_assert!(!scratch.has_errors(), "generated error item does not parse:\n{text}");
    parsed.items
}

// ----- message templates ---------------------------------------------------

/// The payload fields of a variant: the field count, and the field declarations of a struct variant (a tuple
/// variant has none, so an expression placeholder can never name one).
fn fields_of(kind: &VariantKind) -> (usize, Option<&[FieldDecl]>) {
    match kind {
        VariantKind::Unit => (0, None),
        VariantKind::Tuple(types) => (types.len(), None),
        VariantKind::Struct(fields) => (fields.len(), Some(fields)),
    }
}

/// Resolves a `{name}` placeholder to the position of that field of a struct variant.
fn expr_field(expr: &Expr, fields: Option<&[FieldDecl]>) -> Option<usize> {
    let ExprKind::Name { name, args } = &expr.kind else { return None };
    if !args.is_empty() {
        return None;
    }
    fields?.iter().position(|f| f.name.name == name.name)
}

/// Writes the message template back as a Burdin interpolated string, with every hole naming its payload field as
/// `__eN`. Invalid holes are reported and left out, so the generated `Display` still parses.
fn message_source(v: &ErrorVariant, diags: &mut Diagnostics) -> String {
    let (count, fields) = fields_of(&v.kind);
    let mut out = String::new();
    let mut next = 0usize;
    for part in &v.message {
        match part {
            InterpPart::Lit(text) => escape_lit(text, &mut out),
            InterpPart::Hole(hole) => {
                if count == 0 {
                    diags.error(
                        INVALID_ERROR,
                        hole.span,
                        format!("variant `{}` has no fields, so its message cannot have placeholders", v.name.name),
                    );
                    continue;
                }
                let position = match &hole.arg {
                    HoleArg::Next => {
                        let position = next;
                        next += 1;
                        if position < count {
                            Some(position)
                        } else {
                            diags.error(
                                INVALID_ERROR,
                                hole.span,
                                format!("placeholder `{{}}` has no payload field left in variant `{}`", v.name.name),
                            );
                            None
                        }
                    }
                    HoleArg::Index(n) => {
                        if *n < count {
                            Some(*n)
                        } else {
                            diags.error(
                                INVALID_ERROR,
                                hole.span,
                                format!(
                                    "placeholder `{{{n}}}` does not name a payload field of variant `{}`",
                                    v.name.name
                                ),
                            );
                            None
                        }
                    }
                    HoleArg::Expr(expr) => {
                        let position = expr_field(expr, fields);
                        if position.is_none() {
                            diags.error(
                                INVALID_ERROR,
                                hole.span,
                                format!(
                                    "a message placeholder must name a field of variant `{}`, not an expression",
                                    v.name.name
                                ),
                            );
                        }
                        position
                    }
                };
                // A fill character that cannot be written back as source is invalid; skip the hole so that the
                // generated impl still parses.
                let bad_fill = matches!(hole.spec.fill, '{' | '}' | '"' | '\\');
                if bad_fill && position.is_some() {
                    diags.error(
                        INVALID_ERROR,
                        hole.span,
                        format!("the fill character `{}` cannot be written in a message template", hole.spec.fill),
                    );
                }
                if let Some(position) = position
                    && !bad_fill
                {
                    out.push_str("{ __e");
                    out.push_str(&position.to_string());
                    // The `:` introduces the specifier; a plain `{}` has none, but `{:?}` does.
                    if !hole.spec.is_default() || !matches!(hole.spec.kind, SpecKind::Display) {
                        out.push(':');
                    }
                    render_spec(&hole.spec, &mut out);
                    out.push('}');
                }
            }
        }
    }
    out
}

/// Escapes literal text so it survives the trip through the generated source and the lexer.
fn escape_lit(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '{' => out.push_str("{{"),
            '}' => out.push_str("}}"),
            _ => out.push(c),
        }
    }
}

/// Writes `[[fill]align][+][#][0][width][.precision][kind]`, leaving out everything that is the default.
fn render_spec(spec: &FormatSpec, out: &mut String) {
    if spec.align != Align::Default {
        if spec.fill != ' ' {
            out.push(spec.fill);
        }
        out.push(match spec.align {
            Align::Default => unreachable!("just checked"),
            Align::Left => '<',
            Align::Center => '^',
            Align::Right => '>',
        });
    }
    if spec.plus {
        out.push('+');
    }
    if spec.alt {
        out.push('#');
    }
    if spec.zero {
        out.push('0');
    }
    if let Some(width) = spec.width {
        out.push_str(&width.to_string());
    }
    if let Some(precision) = spec.precision {
        out.push('.');
        out.push_str(&precision.to_string());
    }
    out.push_str(match spec.kind {
        SpecKind::Display => "",
        SpecKind::Debug => "?",
        SpecKind::LowerHex => "x",
        SpecKind::UpperHex => "X",
        SpecKind::Binary => "b",
        SpecKind::Octal => "o",
    });
}

// ----- generated impls -----------------------------------------------------

/// `Name` or `Name<A, B>: Interface where A: Display, B: Display`.
fn head(e: &ErrorEnumDecl, interface: &str) -> String {
    if e.generics.is_empty() {
        return format!("{}: {interface}", e.name.name);
    }
    let params: Vec<&str> = e.generics.iter().map(|g| g.name.name.as_str()).collect();
    let wheres: Vec<String> = params.iter().map(|p| format!("{p}: Display")).collect();
    format!("{}<{}>: {interface} where {}", e.name.name, params.join(", "), wheres.join(", "))
}

fn display_impl(e: &ErrorEnumDecl, messages: &[String]) -> String {
    let name = &e.name.name;
    let mut arms = String::new();
    for (v, message) in e.variants.iter().zip(messages) {
        let variant = &v.name.name;
        match &v.kind {
            VariantKind::Unit => {
                arms.push_str(&format!(
                    "        {name}.{variant} => {{\n            f.write_str(\"{message}\")\n        }}\n"
                ));
            }
            VariantKind::Tuple(types) => {
                let binds: Vec<String> = (0..types.len()).map(|i| format!("__e{i}")).collect();
                arms.push_str(&format!(
                    "        {name}.{variant}({}) => {{\n            f.write_str(\"{message}\")\n        }}\n",
                    binds.join(", ")
                ));
            }
            VariantKind::Struct(fields) => {
                let binds: Vec<String> =
                    fields.iter().enumerate().map(|(i, f)| format!("{}: __e{i}", f.name.name)).collect();
                arms.push_str(&format!(
                    "        {name}.{variant} {{ {} }} => {{\n            f.write_str(\"{message}\")\n        }}\n",
                    binds.join(", ")
                ));
            }
        }
    }
    if e.variants.is_empty() {
        arms.push_str("        else => {}\n");
    }
    format!(
        "{} {{\n    fn fmt(self, mut f: Formatter) {{\n        match self {{\n{arms}        }}\n    }}\n}}\n",
        head(e, "Display")
    )
}

fn error_impl(e: &ErrorEnumDecl) -> String {
    format!("{} {{ }}\n", head(e, "Error"))
}

/// `Name: From<Source> { fn from(__e: Source): Name { Name.Variant(__e) } }`.
fn from_impl(name: &str, variant: &str, source: &str) -> String {
    format!(
        "{name}: From<{source}> {{\n    fn from(__e: {source}): {name} {{\n        {name}.{variant}(__e)\n    }}\n}}\n"
    )
}

/// The source type of one `from` conversion, or `None` when the clause is invalid (and reported).
fn from_source(e: &ErrorEnumDecl, v: &ErrorVariant, seen: &mut Vec<String>, diags: &mut Diagnostics) -> Option<String> {
    let from_span = v.from?;
    if !e.generics.is_empty() {
        diags.error(INVALID_ERROR, from_span, "a generic error enum cannot declare `from`");
        return None;
    }
    let field = match &v.kind {
        VariantKind::Tuple(types) if types.len() == 1 => &types[0],
        _ => {
            diags.error(
                INVALID_ERROR,
                from_span,
                format!("`from` on variant `{}` needs a tuple payload with exactly one field", v.name.name),
            );
            return None;
        }
    };
    let source = match render_type(field) {
        Some(source) => source,
        None => {
            diags.error(
                INVALID_ERROR,
                field.span,
                "this payload type cannot be written back as the source type of a `from` impl",
            );
            return None;
        }
    };
    if seen.iter().any(|s| s == &source) {
        diags.error(DUPLICATE_FROM, from_span, format!("two variants convert from the same type `{source}`"));
        return None;
    }
    seen.push(source.clone());
    Some(source)
}

/// Renders a type back to source text for a generated `from` impl. Only the forms a conversion can use are
/// supported; anything else reports E0210 at the type.
fn render_type(ty: &Type) -> Option<String> {
    match &ty.kind {
        TypeKind::Path(segments) => {
            let mut out = String::new();
            for (i, segment) in segments.iter().enumerate() {
                if i > 0 {
                    out.push('.');
                }
                out.push_str(&segment.name.name);
                if !segment.args.is_empty() {
                    let args: Vec<String> = segment.args.iter().map(render_type).collect::<Option<_>>()?;
                    out.push('<');
                    out.push_str(&args.join(", "));
                    out.push('>');
                }
            }
            Some(out)
        }
        TypeKind::Unit => Some("()".to_string()),
        TypeKind::Tuple(types) => {
            let args: Vec<String> = types.iter().map(render_type).collect::<Option<_>>()?;
            if args.len() == 1 { Some(format!("({},)", args[0])) } else { Some(format!("({})", args.join(", "))) }
        }
        TypeKind::Slice(inner) => Some(format!("[{}]", render_type(inner)?)),
        TypeKind::Ref { mutable, ty } => {
            let inner = render_type(ty)?;
            Some(if *mutable { format!("&mut {inner}") } else { format!("&{inner}") })
        }
        TypeKind::Array(inner, len) => {
            let ExprKind::Int(n) = &len.kind else { return None };
            Some(format!("[{}; {n}]", render_type(inner)?))
        }
        TypeKind::Never => Some("!".to_string()),
        _ => None,
    }
}

// ----- main and test wrappers ----------------------------------------------

/// `fn main(): Result<T>`: renames the function and appends a wrapper that prints an `Err` to standard error and
/// exits 1 (spec/errors.md, ERR8). The return type must be written directly, so only a syntactic `Result` path is
/// wrapped; an alias is left for the checker's main signature rule.
pub fn wrap_result_main(file: &mut File, id: FileId, diags: &mut Diagnostics) {
    let _ = &diags;
    let mut name_span = None;
    for item in &mut file.items {
        let Item::Fn(f) = item else { continue };
        // A `main` with parameters is still E0003, so it stays a `main` for the checker to reject.
        if f.name.name != "main" || !f.params.is_empty() || f.body.stmts.is_empty() {
            continue;
        }
        let Some(ret) = &f.ret else { continue };
        let TypeKind::Path(segments) = &ret.kind else { continue };
        if segments.len() != 1 || segments[0].name.name != "Result" || segments[0].args.is_empty() {
            continue;
        }
        name_span = Some(f.name.span);
        f.name.name = "__burdin_main_result".to_string();
        break;
    }
    let Some(name_span) = name_span else { return };
    let text = "\
fn main() {
    match __burdin_main_result() {
        Ok(_) => (),
        Err(__err) => {
            eprintln(\"{__err}\")
            process.exit(1)
        }
    }
}
";
    file.items.extend(parse_generated(text, id, name_span));
}

/// Typed test failures (spec/errors.md, ERR9): the parsed test function becomes `__burdin_test_N_body`, and a
/// wrapper named `__burdin_test_N` prints the error before returning it to the test runner.
pub fn wrap_test_failures(file: &mut File, id: FileId, diags: &mut Diagnostics) {
    let _ = &diags;
    for index in 0..file.tests.len() {
        let name_span = file.tests[index].name_span;
        let text = format!(
            "\
priv fn __burdin_test_{index}(): Result<()> {{
    match __burdin_test_{index}_body() {{
        Ok(_) => Ok(()),
        Err(__e) => {{
            eprintln(\"{{__e}}\")
            Err(__e)
        }}
    }}
}}
"
        );
        let mut generated = parse_generated(&text, id, name_span);
        debug_assert_eq!(generated.len(), 1, "the generated test wrapper does not parse");
        if generated.len() != 1 {
            continue;
        }
        let Item::Fn(wrapper) = generated.remove(0) else {
            debug_assert!(false, "the generated test wrapper is not a function");
            continue;
        };
        let mut body = std::mem::replace(&mut file.tests[index].func, wrapper);
        body.name.name = format!("__burdin_test_{index}_body");
        file.tests[index].body_func = body;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn parse_src(src: &str) -> (File, String) {
        let mut sources = burdin_diag::SourceMap::default();
        let id = sources.add("t.bd".into(), src.into());
        let mut diags = Diagnostics::default();
        let file = parse(id, src, &mut diags);
        (file, diags.render(&sources))
    }

    fn path_name(ty: &Type) -> &str {
        let TypeKind::Path(segments) = &ty.kind else { panic!("not a path") };
        &segments[0].name.name
    }

    fn impls(file: &File) -> Vec<&ImplBlock> {
        file.items
            .iter()
            .filter_map(|item| match item {
                Item::Impl(block) => Some(block),
                _ => None,
            })
            .collect()
    }

    fn interface_name(block: &ImplBlock) -> &str {
        path_name(block.interface.as_ref().expect("an impl with an interface"))
    }

    fn span() -> Span {
        Span::new(FileId(0), 0, 0)
    }

    fn hole(arg: HoleArg, spec: FormatSpec) -> InterpPart {
        InterpPart::Hole(Hole { arg, spec, span: span() })
    }

    fn dummy_type() -> Type {
        Type { kind: TypeKind::Unit, span: span() }
    }

    fn variant(name: &str, kind: VariantKind, message: Vec<InterpPart>) -> ErrorVariant {
        ErrorVariant {
            name: Ident { name: name.into(), span: span() },
            kind,
            from: None,
            message,
            message_span: span(),
        }
    }

    #[test]
    fn lowers_an_error_enum_to_an_enum_and_impls() {
        let src = "error enum ConfigError {\n\
                   Io(IoError) from \"cannot read config: {0}\"\n\
                   Syntax { line: i32 } \"syntax error on line {line}\"\n\
                   Unknown \"unknown configuration\"\n\
                   }\n";
        let (file, out) = parse_src(src);
        assert_eq!(out, "");
        assert!(!file.items.iter().any(|item| matches!(item, Item::ErrorEnum(_))));
        let enums: Vec<&EnumDecl> = file
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Enum(e) => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(enums.len(), 1);
        assert_eq!(enums[0].name.name, "ConfigError");
        assert_eq!(enums[0].variants.len(), 3);
        let blocks = impls(&file);
        assert_eq!(blocks.len(), 3);
        assert_eq!(interface_name(blocks[0]), "Display");
        assert_eq!(interface_name(blocks[1]), "Error");
        assert_eq!(interface_name(blocks[2]), "From");
        // The `From` source is the payload type of the `from` variant.
        let TypeKind::Path(from_args) = &blocks[2].interface.as_ref().unwrap().kind else { panic!() };
        assert_eq!(from_args[0].args.len(), 1);
        assert_eq!(path_name(&from_args[0].args[0]), "IoError");
        assert_eq!(blocks[2].methods[0].name.name, "from");
        // The Display match has one arm per variant.
        assert_eq!(display_arms(blocks[0]), 3);
    }

    /// The declaration as parsed, before lowering, so the exact generated source can be checked.
    fn error_decl(src: &str) -> ErrorEnumDecl {
        let mut diags = Diagnostics::default();
        let lexed = crate::lexer::lex(FileId(0), src, &mut diags);
        let file = parse_file(FileId(0), lexed, &mut diags);
        assert!(!diags.has_errors());
        match file.items.into_iter().next() {
            Some(Item::ErrorEnum(e)) => e,
            other => panic!("not an error enum: {other:?}"),
        }
    }

    #[test]
    fn generated_source_has_the_documented_form() {
        let e = error_decl(
            "error enum ConfigError {\n\
             Io(IoError) from \"cannot read config: {0}\"\n\
             Syntax { line: i32 } \"syntax error on line {line}\"\n\
             Unknown \"unknown configuration\"\n\
             }\n",
        );
        let mut diags = Diagnostics::default();
        let messages: Vec<String> = e.variants.iter().map(|v| message_source(v, &mut diags)).collect();
        assert!(!diags.has_errors());
        let want = concat!(
            "ConfigError: Display {\n",
            "    fn fmt(self, mut f: Formatter) {\n",
            "        match self {\n",
            "        ConfigError.Io(__e0) => {\n",
            "            f.write_str(\"cannot read config: { __e0}\")\n",
            "        }\n",
            "        ConfigError.Syntax { line: __e0 } => {\n",
            "            f.write_str(\"syntax error on line { __e0}\")\n",
            "        }\n",
            "        ConfigError.Unknown => {\n",
            "            f.write_str(\"unknown configuration\")\n",
            "        }\n",
            "        }\n",
            "    }\n",
            "}\n",
        );
        assert_eq!(display_impl(&e, &messages), want);
        assert_eq!(error_impl(&e), "ConfigError: Error { }\n");
        assert_eq!(
            from_impl(&e.name.name, "Io", "IoError"),
            concat!(
                "ConfigError: From<IoError> {\n",
                "    fn from(__e: IoError): ConfigError {\n",
                "        ConfigError.Io(__e)\n",
                "    }\n",
                "}\n",
            )
        );

        let generic = error_decl("error enum E<T> {\n V(T) \"v {0}\"\n}\n");
        let messages: Vec<String> = generic.variants.iter().map(|v| message_source(v, &mut diags)).collect();
        assert!(display_impl(&generic, &messages).starts_with("E<T>: Display where T: Display {\n"));
        assert_eq!(error_impl(&generic), "E<T>: Error where T: Display { }\n");

        let empty = error_decl("error enum Never {\n}\n");
        assert!(display_impl(&empty, &[]).contains("        else => {}\n"));
    }

    #[test]
    fn derives_compose_with_the_generated_impls() {
        let (file, out) = parse_src("priv error enum E derives (Debug) {\n V \"v\"\n}\n");
        assert_eq!(out, "");
        let enums: Vec<&EnumDecl> = file
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Enum(e) => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(enums.len(), 1);
        assert!(enums[0].is_priv);
        let interfaces: Vec<&str> = impls(&file).iter().map(|b| interface_name(b)).collect();
        assert_eq!(interfaces, ["Display", "Error", "Debug"]);
    }

    /// The Display method's block holds `match self { ... }` with one arm per variant.
    fn display_arms(block: &ImplBlock) -> usize {
        let method = &block.methods[0];
        let Stmt::Expr(Expr { kind: ExprKind::Match(_, arms), .. }) = &method.body.stmts[0] else {
            panic!("the Display body is not a match")
        };
        arms.len()
    }

    #[test]
    fn a_tuple_template_renders_positional_fields() {
        let v = variant(
            "Io",
            VariantKind::Tuple(vec![dummy_type()]),
            vec![InterpPart::Lit("cannot read config: ".into()), hole(HoleArg::Index(0), FormatSpec::plain(span()))],
        );
        let mut diags = Diagnostics::default();
        assert_eq!(message_source(&v, &mut diags), "cannot read config: { __e0}");
        assert!(!diags.has_errors());
    }

    #[test]
    fn a_struct_template_names_fields_by_position() {
        let fields =
            vec![FieldDecl { is_priv: false, name: Ident { name: "line".into(), span: span() }, ty: dummy_type() }];
        let name = Expr {
            kind: ExprKind::Name { name: Ident { name: "line".into(), span: span() }, args: Vec::new() },
            span: span(),
        };
        let v = variant(
            "Syntax",
            VariantKind::Struct(fields),
            vec![
                InterpPart::Lit("syntax error on line ".into()),
                hole(HoleArg::Expr(Box::new(name)), FormatSpec::plain(span())),
            ],
        );
        let mut diags = Diagnostics::default();
        assert_eq!(message_source(&v, &mut diags), "syntax error on line { __e0}");
        assert!(!diags.has_errors());
    }

    #[test]
    fn bad_templates_are_reported() {
        for src in [
            "error enum E {\n A \"x {0}\"\n}\n",
            "error enum E {\n A(i32) \"x {1}\"\n}\n",
            "error enum E {\n A { line: i32 } \"x {nope}\"\n}\n",
            "error enum E {\n A { line: i32 } \"x {line.y}\"\n}\n",
            "error enum E {\n A(i32) \"x {0:{<8}\"\n}\n",
            "error enum E {\n A(i32) \"x {0:\\<8}\"\n}\n",
            "error enum E {\n A(i32) \"x {0:\"<8}\"\n}\n",
        ] {
            let (_, out) = parse_src(src);
            assert!(out.contains("E0210"), "{src}: {out}");
        }
    }

    #[test]
    fn duplicate_from_is_e0211() {
        let src = "error enum E {\n A(i32) from \"a {0}\"\n B(i32) from \"b {0}\"\n}\n";
        let (file, out) = parse_src(src);
        assert!(out.contains("E0211"), "{out}");
        // The first conversion is still generated; only the second one is skipped, so there is no duplicate impl.
        assert_eq!(impls(&file).len(), 3);
    }

    #[test]
    fn from_on_a_generic_error_enum_is_e0210() {
        let src = "error enum E<T> {\n V(T) from \"v {0}\"\n}\n";
        let (file, out) = parse_src(src);
        assert!(out.contains("E0210"), "{out}");
        assert_eq!(impls(&file).len(), 2);
    }

    #[test]
    fn from_needs_a_one_field_tuple_variant() {
        for src in [
            "error enum E {\n V from \"x\"\n}\n",
            "error enum E {\n V { a: i32 } from \"x {a}\"\n}\n",
            "error enum E {\n V(i32, i32) from \"x {0}\"\n}\n",
        ] {
            let (file, out) = parse_src(src);
            assert!(out.contains("E0210"), "{src}: {out}");
            assert_eq!(impls(&file).len(), 2, "{src}");
        }
    }

    #[test]
    fn an_unrenderable_source_type_is_e0210() {
        let src = "error enum E {\n V([i32; N]) from \"x {0}\"\n}\n";
        let (file, out) = parse_src(src);
        assert!(out.contains("E0210"), "{out}");
        assert_eq!(impls(&file).len(), 2);
    }

    #[test]
    fn a_generic_head_carries_the_display_where_clause() {
        let src = "error enum E<T> {\n V(T) \"v {0}\"\n}\n";
        let (file, out) = parse_src(src);
        assert_eq!(out, "");
        let blocks = impls(&file);
        assert_eq!(interface_name(blocks[0]), "Display");
        assert_eq!(blocks[0].where_clause.len(), 1);
        assert_eq!(path_name(&blocks[0].where_clause[0].ty), "T");
        assert_eq!(path_name(&blocks[0].where_clause[0].bounds[0]), "Display");
        assert_eq!(interface_name(blocks[1]), "Error");
        assert_eq!(blocks[1].where_clause.len(), 1);
        assert_eq!(display_arms(blocks[0]), 1);
    }

    #[test]
    fn a_spec_renders_in_the_documented_order() {
        let mut spec = FormatSpec::plain(span());
        spec.fill = '*';
        spec.align = Align::Center;
        spec.plus = true;
        spec.width = Some(8);
        spec.precision = Some(3);
        spec.kind = SpecKind::LowerHex;
        let mut out = String::new();
        render_spec(&spec, &mut out);
        assert_eq!(out, "*^+8.3x");

        let mut debug = FormatSpec::plain(span());
        debug.kind = SpecKind::Debug;
        let mut out = String::new();
        render_spec(&debug, &mut out);
        assert_eq!(out, "?");
    }

    #[test]
    fn liters_and_braces_are_escaped() {
        let mut out = String::new();
        escape_lit("a\\b \"q\"\n\r\t{x}", &mut out);
        assert_eq!(out, "a\\\\b \\\"q\\\"\\n\\r\\t{{x}}");
    }

    #[test]
    fn wraps_a_result_main() {
        let src = "fn main(): Result<(), MyError> {\n    work()\n}\n";
        let (file, out) = parse_src(src);
        assert_eq!(out, "");
        let names: Vec<&str> = file
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Fn(f) => Some(f.name.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, ["__burdin_main_result", "main"]);
        let Item::Fn(wrapper) = &file.items[1] else { unreachable!() };
        assert!(matches!(wrapper.body.stmts[0], Stmt::Expr(Expr { kind: ExprKind::Match(..), .. })));
    }

    #[test]
    fn does_not_wrap_other_mains() {
        for (src, names) in [
            ("fn main() {\n    work()\n}\n", vec!["main"]),
            ("type R = Result<(), MyError>\nfn main(): R {\n    work()\n}\n", vec!["main"]),
            ("fn main(): Result<(), MyError> {\n}\n", vec!["main"]),
            ("fn main(n: i32): Result<(), MyError> {\n    work()\n}\n", vec!["main"]),
            ("fn other(): Result<(), MyError> {\n    work()\n}\n", vec!["other"]),
        ] {
            let (file, out) = parse_src(src);
            assert_eq!(out, "", "{src}");
            let seen: Vec<&str> = file
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Fn(f) => Some(f.name.name.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(seen, names, "{src}");
        }
    }

    #[test]
    fn wraps_test_failures_in_a_body_function() {
        let src = "test \"one\" { assert(true) }\ntest \"two\" { }\n";
        let (file, out) = parse_src(src);
        assert_eq!(out, "");
        assert_eq!(file.tests.len(), 2);
        let first = &file.tests[0];
        assert_eq!(first.func.name.name, "__burdin_test_0");
        assert_eq!(first.body_func.name.name, "__burdin_test_0_body");
        // The body keeps the original block plus the trailing `Ok(())`, and its Result type.
        let ret = first.body_func.ret.as_ref().expect("the body returns Result");
        assert_eq!(path_name(ret), "Result");
        assert_eq!(first.body_func.body.stmts.len(), 2);
        // The wrapper is the small match the runner calls.
        assert_eq!(first.func.body.stmts.len(), 1);
        assert!(matches!(first.func.body.stmts[0], Stmt::Expr(Expr { kind: ExprKind::Match(..), .. })));
        assert_eq!(first.func.span, first.name_span);
        assert_eq!(file.tests[1].func.name.name, "__burdin_test_1");
        assert_eq!(file.tests[1].body_func.name.name, "__burdin_test_1_body");
    }
}
