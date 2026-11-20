//! `derives (Debug, Serialize, Deserialize)`: the generated impl blocks (spec/derives.md and
//! spec/serialization.md).
//!
//! Each impl is written as Burdin source text, parsed with the ordinary parser, and added to the file's items, so
//! a derived impl is the same thing as one written by hand. Every token of the generated text carries the span of
//! the derive in the `: A, B` list, which is where a diagnostic about the generated code is reported.

use crate::ast::*;
use crate::lexer::lex;
use crate::parser::parse_file;
use burdin_diag::{Diagnostics, FileId, Span};

/// Appends the impl block of every struct and enum that derives `Debug`, `Serialize`, or `Deserialize`.
pub fn expand_derives(file: &mut File, id: FileId, diags: &mut Diagnostics) {
    let mut generated: Vec<(String, Span)> = Vec::new();
    for item in &file.items {
        match item {
            Item::Struct(s) => {
                if let Some(span) = derive_span(&s.derives, "Debug") {
                    generated.push((struct_impl(s), span));
                }
                if let Some(span) = derive_span(&s.derives, "Serialize") {
                    generated.push((serialize_struct_impl(s), span));
                }
                if let Some(span) = derive_span(&s.derives, "Deserialize") {
                    generated.push((deserialize_struct_impl(s), span));
                }
            }
            Item::Enum(e) => {
                if let Some(span) = derive_span(&e.derives, "Debug") {
                    generated.push((enum_impl(e), span));
                }
                if let Some(span) = derive_span(&e.derives, "Serialize") {
                    generated.push((serialize_enum_impl(e), span));
                }
                if let Some(span) = derive_span(&e.derives, "Deserialize") {
                    generated.push((deserialize_enum_impl(e), span));
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
        debug_assert!(!scratch.has_errors(), "generated impl does not parse:\n{text}");
        let _ = &diags;
        file.items.extend(parsed.items);
    }
}

/// The span of the derive named `name` in a declaration's `: A, B` list, if it is there.
fn derive_span(derives: &[Type], name: &str) -> Option<Span> {
    derives.iter().find_map(|t| match &t.kind {
        TypeKind::Path(segs) if segs.len() == 1 && segs[0].args.is_empty() && segs[0].name.name == name => Some(t.span),
        _ => None,
    })
}

/// `Name<A, B>: Iface where A: Iface, B: Iface`.
fn head(name: &str, generics: &[GenericParam], iface: &str) -> String {
    if generics.is_empty() {
        return format!("{name}: {iface}");
    }
    let params: Vec<&str> = generics.iter().map(|g| g.name.name.as_str()).collect();
    // A const parameter is a number: it has nothing to implement.
    let wheres: Vec<String> =
        generics.iter().filter(|g| g.const_ty.is_none()).map(|g| format!("{}: {iface}", g.name.name)).collect();
    if wheres.is_empty() {
        return format!("{name}<{}>: {iface}", params.join(", "));
    }
    format!("{name}<{}>: {iface} where {}", params.join(", "), wheres.join(", "))
}

// ----- Debug ---------------------------------------------------------------------------------------------------

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
    format!("{} {{\n    fn debug(self, mut f: Formatter) {{\n{body}    }}\n}}\n", head(name, &s.generics, "Debug"))
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
        head(name, &e.generics, "Debug")
    )
}

// ----- Serialize -----------------------------------------------------------------------------------------------

/// The source text of a type, as written, for the annotations the generated reader needs.
fn type_text(t: &Type) -> String {
    match &t.kind {
        TypeKind::Path(segs) => segs
            .iter()
            .map(|seg| {
                if seg.args.is_empty() {
                    seg.name.name.clone()
                } else {
                    let args: Vec<String> = seg.args.iter().map(type_text).collect();
                    format!("{}<{}>", seg.name.name, args.join(", "))
                }
            })
            .collect::<Vec<_>>()
            .join("."),
        TypeKind::Unit => "()".to_string(),
        TypeKind::Tuple(items) => {
            let items: Vec<String> = items.iter().map(type_text).collect();
            format!("({})", items.join(", "))
        }
        TypeKind::Array(item, len) => format!("[{}; {}]", type_text(item), expr_text(len)),
        TypeKind::Slice(item) => format!("[{}]", type_text(item)),
        TypeKind::Ref { mutable, ty } => {
            if *mutable {
                format!("&mut {}", type_text(ty))
            } else {
                format!("&{}", type_text(ty))
            }
        }
        TypeKind::SelfType => "Self".to_string(),
        TypeKind::Any(inner) => format!("any {}", type_text(inner)),
        TypeKind::Impl(inner) => format!("impl {}", type_text(inner)),
        TypeKind::Fn { .. } => "Fn(..)".to_string(),
        TypeKind::Const(e) => expr_text(e),
        TypeKind::Never => "!".to_string(),
    }
}

/// The text of a constant argument, for an array's length.
fn expr_text(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Int(n) => n.to_string(),
        ExprKind::Name { name, .. } => name.name.clone(),
        _ => "0".to_string(),
    }
}

/// A single-segment path with no arguments, the shape the built-in type names take.
fn bare_name(t: &Type) -> Option<&str> {
    match &t.kind {
        TypeKind::Path(segs) if segs.len() == 1 && segs[0].args.is_empty() => Some(segs[0].name.name.as_str()),
        _ => None,
    }
}

/// The scalar helper's suffix, for a type written as a scalar name: `i32` gives `i32`.
fn scalar_helper(t: &Type) -> Option<&str> {
    let name = bare_name(t)?;
    matches!(
        name,
        "bool"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "usize"
            | "f32"
            | "f64"
            | "char"
    )
    .then_some(name)
}

/// The built-in container shapes a field can be written as (spec/serialization.md, SER2).
enum Container {
    List,
    Map,
    Option,
    Result,
    Tuple2,
    Tuple3,
    Tuple4,
}

/// The container a type is written as, if any.
fn container(t: &Type) -> Option<Container> {
    match &t.kind {
        TypeKind::Path(segs) if segs.len() == 1 => {
            let seg = &segs[0];
            match (seg.name.name.as_str(), seg.args.len()) {
                ("List", 1) => Some(Container::List),
                ("Map", 2) => Some(Container::Map),
                ("Option", 1) => Some(Container::Option),
                ("Result", 2) => Some(Container::Result),
                _ => None,
            }
        }
        TypeKind::Tuple(items) => match items.len() {
            2 => Some(Container::Tuple2),
            3 => Some(Container::Tuple3),
            4 => Some(Container::Tuple4),
            _ => None,
        },
        _ => None,
    }
}

/// Whether `t` is `List<u8>`, the bytes shape (spec/serialization.md, SER2).
fn is_bytes(t: &Type) -> bool {
    let TypeKind::Path(segs) = &t.kind else { return false };
    segs.len() == 1
        && segs[0].name.name == "List"
        && segs[0].args.len() == 1
        && bare_name(&segs[0].args[0]) == Some("u8")
}

/// The expression that writes `value` (a place of type `t`) with `s`, without the trailing `?`.
fn write_call(t: &Type, value: &str) -> String {
    if let Some(name) = scalar_helper(t) {
        return format!("serialize_{name}(mut s, {value})");
    }
    if bare_name(t) == Some("String") {
        return format!("serialize_str(mut s, {value}.as_str())");
    }
    if matches!(t.kind, TypeKind::Unit) {
        return "serialize_unit(mut s)".to_string();
    }
    if is_bytes(t) {
        return format!("serialize_bytes(mut s, {value}.as_slice())");
    }
    // A path to a user type, a type alias, or a type parameter: that type implements `Serialize`.
    format!("{value}.serialize(mut s)")
}

/// The expression that reads a value of type `t` from `d`, without the trailing `?`. Only the scalars, strings,
/// bytes, unit, and a type that implements `Deserialize` itself: the containers are written out by `write_value`
/// and `read_value`.
fn read_call(t: &Type) -> String {
    if let Some(name) = scalar_helper(t) {
        return format!("deserialize_{name}(mut d)");
    }
    if bare_name(t) == Some("String") {
        return "deserialize_str(mut d)".to_string();
    }
    if matches!(t.kind, TypeKind::Unit) {
        return "deserialize_unit(mut d)".to_string();
    }
    if is_bytes(t) {
        return "deserialize_bytes(mut d)".to_string();
    }
    format!("{}.deserialize(mut d)", type_text(t))
}

/// The type arguments of a one-segment path, or an empty slice.
fn path_args(t: &Type) -> &[Type] {
    match &t.kind {
        TypeKind::Path(segs) if segs.len() == 1 => &segs[0].args,
        _ => &[],
    }
}

/// The statements that write `value` (a place of type `t`) at `indent`, each ending in a newline.
///
/// A scalar, a string, bytes, or a type that implements `Serialize` calls its helper or method. A built-in container
/// is written out here instead of through `serialize_list` and its friends: those helpers require their element or
/// part types to implement `Serialize`, and the compiler does not provide the primitive impls yet (SER12). Writing
/// the shape directly keeps the derive usable in this phase, and it produces the same sequence of serializer calls.
fn write_value(t: &Type, value: &str, indent: &str) -> String {
    // `List<u8>` is the bytes shape, not a sequence (SER2), so it goes through `serialize_bytes`.
    let shape = if is_bytes(t) { None } else { container(t) };
    match shape {
        Some(Container::List) => {
            let elem = &path_args(t)[0];
            let mut out = format!("{indent}s.begin_seq({value}.len())?\n");
            out.push_str(&format!("{indent}for __item in {value} {{\n"));
            out.push_str(&write_value(elem, "__item", &format!("{indent}    ")));
            out.push_str(&format!("{indent}}}\n"));
            out.push_str(&format!("{indent}s.end_seq()?\n"));
            out
        }
        Some(Container::Map) => {
            let value_ty = &path_args(t)[1];
            let mut out = format!("{indent}s.begin_map({value}.len())?\n");
            out.push_str(&format!("{indent}for (__key, __value) in {value} {{\n"));
            out.push_str(&format!("{indent}    s.map_key(__key.as_str())?\n"));
            out.push_str(&write_value(value_ty, "__value", &format!("{indent}    ")));
            out.push_str(&format!("{indent}}}\n"));
            out.push_str(&format!("{indent}s.end_map()?\n"));
            out
        }
        Some(Container::Option) => {
            let inner = &path_args(t)[0];
            let mut out = format!("{indent}match {value} {{\n");
            out.push_str(&format!("{indent}    Some(__inner) => {{\n"));
            out.push_str(&format!("{indent}        s.begin_variant(\"Option\", \"Some\", 0, 1)?\n"));
            out.push_str(&write_value(inner, "__inner", &format!("{indent}        ")));
            out.push_str(&format!("{indent}        s.end_variant(1)?\n"));
            out.push_str(&format!("{indent}    }}\n"));
            out.push_str(&format!("{indent}    None => {{\n"));
            out.push_str(&format!("{indent}        s.begin_variant(\"Option\", \"None\", 1, 0)?\n"));
            out.push_str(&format!("{indent}        s.end_variant(0)?\n"));
            out.push_str(&format!("{indent}    }}\n"));
            out.push_str(&format!("{indent}}}\n"));
            out
        }
        Some(Container::Result) => {
            let args = path_args(t);
            let mut out = format!("{indent}match {value} {{\n");
            out.push_str(&format!("{indent}    Ok(__inner) => {{\n"));
            out.push_str(&format!("{indent}        s.begin_variant(\"Result\", \"Ok\", 0, 1)?\n"));
            out.push_str(&write_value(&args[0], "__inner", &format!("{indent}        ")));
            out.push_str(&format!("{indent}        s.end_variant(1)?\n"));
            out.push_str(&format!("{indent}    }}\n"));
            out.push_str(&format!("{indent}    Err(__inner) => {{\n"));
            out.push_str(&format!("{indent}        s.begin_variant(\"Result\", \"Err\", 1, 1)?\n"));
            out.push_str(&write_value(&args[1], "__inner", &format!("{indent}        ")));
            out.push_str(&format!("{indent}        s.end_variant(1)?\n"));
            out.push_str(&format!("{indent}    }}\n"));
            out.push_str(&format!("{indent}}}\n"));
            out
        }
        Some(Container::Tuple2) | Some(Container::Tuple3) | Some(Container::Tuple4) => {
            let TypeKind::Tuple(items) = &t.kind else { unreachable!() };
            let mut out = format!("{indent}s.begin_seq({})?\n", items.len());
            for (i, item) in items.iter().enumerate() {
                out.push_str(&write_value(item, &format!("{value}.{i}"), indent));
            }
            out.push_str(&format!("{indent}s.end_seq()?\n"));
            out
        }
        None => format!("{indent}{}?\n", write_call(t, value)),
    }
}

/// `match <call> { Ok(_) => {}, Err(e) => return Err(e<wrap>) }` at `indent`.
fn try_unit(call: &str, wrap: &str, indent: &str) -> String {
    format!(
        "{indent}match {call} {{\n\
         {indent}    Ok(_) => {{}}\n\
         {indent}    Err(e) => return Err(e{wrap}),\n\
         {indent}}}\n"
    )
}

/// `let <target> = match <call> { Ok(value) => value, Err(e) => return Err(e<wrap>) }` at `indent`.
fn try_value(call: &str, target: &str, wrap: &str, indent: &str) -> String {
    format!(
        "{indent}let {target} = match {call} {{\n\
         {indent}    Ok(value) => value,\n\
         {indent}    Err(e) => return Err(e{wrap}),\n\
         {indent}}}\n"
    )
}

/// The statements that read a value of type `t` from `d` into a new local `target` at `indent`. An error is
/// returned with `wrap` attached, so a field, an element, or a payload value reaches the caller with its path.
fn read_value(t: &Type, target: &str, wrap: &str, indent: &str) -> String {
    // `List<u8>` is the bytes shape, not a sequence (SER2), so it goes through `deserialize_bytes`.
    let shape = if is_bytes(t) { None } else { container(t) };
    match shape {
        Some(Container::List) => {
            let elem = &path_args(t)[0];
            let ty = type_text(t);
            let mut out = try_unit("d.begin_seq()", wrap, indent);
            out.push_str(&format!("{indent}let mut __items: {ty} = {ty}.new()\n"));
            out.push_str(&format!("{indent}let mut __index: usize = 0\n"));
            out.push_str(&format!("{indent}loop {{\n"));
            let inner = format!("{indent}    ");
            out.push_str(&try_value("d.has_next_seq_element()", "__more", wrap, &inner));
            out.push_str(&format!("{inner}if !__more {{\n{inner}    break\n{inner}}}\n"));
            let elem_wrap = format!(".index(__index){wrap}");
            out.push_str(&read_value(elem, "__item", &elem_wrap, &inner));
            out.push_str(&format!("{inner}__items.push(__item)\n"));
            out.push_str(&format!("{inner}__index = __index + 1\n"));
            out.push_str(&format!("{indent}}}\n"));
            out.push_str(&try_unit("d.end_seq()", wrap, indent));
            out.push_str(&format!("{indent}let {target}: {ty} = __items\n"));
            out
        }
        Some(Container::Map) => {
            let value_ty = &path_args(t)[1];
            let ty = type_text(t);
            let mut out = try_unit("d.begin_map()", wrap, indent);
            out.push_str(&format!("{indent}let mut __items: {ty} = {ty}.new()\n"));
            out.push_str(&format!("{indent}loop {{\n"));
            let inner = format!("{indent}    ");
            out.push_str(&try_value("d.has_next_map_entry()", "__more", wrap, &inner));
            out.push_str(&format!("{inner}if !__more {{\n{inner}    break\n{inner}}}\n"));
            out.push_str(&try_value("d.map_key()", "__key", wrap, &inner));
            // The key's text is needed only on an error path, but a borrow of `__key` in that arm is treated
            // as live where the key is moved into the map, so the path holds its own copy.
            out.push_str(&format!("{inner}let __key_path = __key.clone()\n"));
            let value_wrap = format!(".key(__key_path.as_str()){wrap}");
            out.push_str(&read_value(value_ty, "__item", &value_wrap, &inner));
            out.push_str(&format!("{inner}__items.insert(__key, __item)\n"));
            out.push_str(&format!("{indent}}}\n"));
            out.push_str(&try_unit("d.end_map()", wrap, indent));
            out.push_str(&format!("{indent}let {target}: {ty} = __items\n"));
            out
        }
        Some(Container::Option) => {
            let inner = &path_args(t)[0];
            let ty = type_text(t);
            let mut out = try_unit("d.begin_enum(\"Option\")", wrap, indent);
            out.push_str(&try_value("d.variant_name()", "__variant", wrap, indent));
            out.push_str(&format!("{indent}let {target}: {ty} = if __variant.as_str() == \"Some\" {{\n"));
            let arm = format!("{indent}    ");
            out.push_str(&try_unit("d.begin_variant_payload(1)", wrap, &arm));
            out.push_str(&try_unit("d.variant_payload_field(0)", wrap, &arm));
            out.push_str(&read_value(inner, "__inner", &format!(".variant(\"Some\"){wrap}"), &arm));
            out.push_str(&try_unit("d.end_variant_payload(1)", wrap, &arm));
            out.push_str(&format!("{arm}Some(__inner)\n"));
            out.push_str(&format!("{indent}}} else if __variant.as_str() == \"None\" {{\n"));
            out.push_str(&try_unit("d.begin_variant_payload(0)", wrap, &arm));
            out.push_str(&try_unit("d.end_variant_payload(0)", wrap, &arm));
            out.push_str(&format!("{arm}None\n"));
            out.push_str(&format!("{indent}}} else {{\n"));
            out.push_str(&format!("{arm}let failure = unknown_variant(\"Option\", __variant.as_str())\n"));
            out.push_str(&format!("{arm}return Err(failure.variant(__variant.as_str()){wrap})\n"));
            out.push_str(&format!("{indent}}}\n"));
            out
        }
        Some(Container::Result) => {
            let args = path_args(t);
            let ty = type_text(t);
            let mut out = try_unit("d.begin_enum(\"Result\")", wrap, indent);
            out.push_str(&try_value("d.variant_name()", "__variant", wrap, indent));
            out.push_str(&format!("{indent}let {target}: {ty} = if __variant.as_str() == \"Ok\" {{\n"));
            let arm = format!("{indent}    ");
            out.push_str(&try_unit("d.begin_variant_payload(1)", wrap, &arm));
            out.push_str(&try_unit("d.variant_payload_field(0)", wrap, &arm));
            out.push_str(&read_value(&args[0], "__inner", &format!(".variant(\"Ok\"){wrap}"), &arm));
            out.push_str(&try_unit("d.end_variant_payload(1)", wrap, &arm));
            out.push_str(&format!("{arm}Ok(__inner)\n"));
            out.push_str(&format!("{indent}}} else if __variant.as_str() == \"Err\" {{\n"));
            out.push_str(&try_unit("d.begin_variant_payload(1)", wrap, &arm));
            out.push_str(&try_unit("d.variant_payload_field(0)", wrap, &arm));
            out.push_str(&read_value(&args[1], "__inner", &format!(".variant(\"Err\"){wrap}"), &arm));
            out.push_str(&try_unit("d.end_variant_payload(1)", wrap, &arm));
            out.push_str(&format!("{arm}Err(__inner)\n"));
            out.push_str(&format!("{indent}}} else {{\n"));
            out.push_str(&format!("{arm}let failure = unknown_variant(\"Result\", __variant.as_str())\n"));
            out.push_str(&format!("{arm}return Err(failure.variant(__variant.as_str()){wrap})\n"));
            out.push_str(&format!("{indent}}}\n"));
            out
        }
        Some(Container::Tuple2) | Some(Container::Tuple3) | Some(Container::Tuple4) => {
            let TypeKind::Tuple(items) = &t.kind else { unreachable!() };
            let ty = type_text(t);
            let mut out = try_unit("d.begin_seq()", wrap, indent);
            for (i, item) in items.iter().enumerate() {
                // A fixed-arity sequence still places a separator between its values: consume it before each
                // part after the first (the list loop consumes it through `has_next_seq_element`).
                if i > 0 {
                    out.push_str(&format!(
                        "{indent}let _ = match d.has_next_seq_element() {{\n\
                         {indent}    Ok(value) => value,\n\
                         {indent}    Err(e) => return Err(e{wrap}),\n\
                         {indent}}}\n"
                    ));
                }
                out.push_str(&read_value(item, &format!("__p{i}"), &format!(".index({i}){wrap}"), indent));
            }
            out.push_str(&try_unit("d.end_seq()", wrap, indent));
            let parts: Vec<String> = (0..items.len()).map(|i| format!("__p{i}")).collect();
            out.push_str(&format!("{indent}let {target}: {ty} = ({})\n", parts.join(", ")));
            out
        }
        None => {
            let ty = type_text(t);
            let call = read_call(t);
            format!(
                "{indent}let {target}: {ty} = match {call} {{\n\
                 {indent}    Ok(value) => value,\n\
                 {indent}    Err(e) => return Err(e{wrap}),\n\
                 {indent}}}\n"
            )
        }
    }
}

fn serialize_struct_impl(s: &StructDecl) -> String {
    let name = &s.name.name;
    let mut body = format!("        s.begin_struct(\"{name}\", {})?\n", s.fields.len());
    for f in &s.fields {
        let field = &f.name.name;
        body.push_str(&format!("        s.struct_field(\"{field}\")?\n"));
        body.push_str(&write_value(&f.ty, &format!("self.{field}"), "        "));
    }
    body.push_str("        s.end_struct()\n");
    format!(
        "{} {{\n    fn serialize<__S: Serializer>(self, mut s: __S): Result<(), DeError> {{\n{body}    }}\n}}\n",
        head(name, &s.generics, "Serialize")
    )
}

fn serialize_enum_impl(e: &EnumDecl) -> String {
    let name = &e.name.name;
    let mut arms = String::new();
    for (index, v) in e.variants.iter().enumerate() {
        let variant = &v.name.name;
        let (pattern, types) = match &v.kind {
            VariantKind::Unit => (format!("{name}.{variant}"), Vec::new()),
            VariantKind::Tuple(types) => {
                let binds: Vec<String> = (0..types.len()).map(|i| format!("__a{i}")).collect();
                (format!("{name}.{variant}({})", binds.join(", ")), types.clone())
            }
            VariantKind::Struct(fields) => {
                let binds: Vec<String> =
                    fields.iter().enumerate().map(|(i, f)| format!("{}: __a{i}", f.name.name)).collect();
                let types: Vec<Type> = fields.iter().map(|f| f.ty.clone()).collect();
                (format!("{name}.{variant} {{ {} }}", binds.join(", ")), types)
            }
        };
        let arity = types.len();
        let mut arm = format!(
            "            {pattern} => {{\n                s.begin_variant(\"{name}\", \"{variant}\", {index}, {arity})?\n"
        );
        for (i, ty) in types.iter().enumerate() {
            arm.push_str(&write_value(ty, &format!("__a{i}"), "                "));
        }
        arm.push_str(&format!("                s.end_variant({arity})\n            }}\n"));
        arms.push_str(&arm);
    }
    let body = if e.variants.is_empty() {
        // An empty enum has no value to write; the `else` arm keeps the match well-formed.
        "        match self {\n            else => {}\n        }\n        Ok(())\n".to_string()
    } else {
        format!("        match self {{\n{arms}        }}\n")
    };
    format!(
        "{} {{\n    fn serialize<__S: Serializer>(self, mut s: __S): Result<(), DeError> {{\n{body}    }}\n}}\n",
        head(name, &e.generics, "Serialize")
    )
}

// ----- Deserialize ---------------------------------------------------------------------------------------------

fn deserialize_struct_impl(s: &StructDecl) -> String {
    let name = &s.name.name;
    let mut body = format!("        d.begin_struct(\"{name}\")?\n");
    // A field is held as an `Option` of its type: there is no default value for every type, and the `None` state
    // is also what reports a field that never appeared.
    for (i, f) in s.fields.iter().enumerate() {
        body.push_str(&format!("        let mut __f{i}: Option<{}> = None\n", type_text(&f.ty)));
    }
    body.push_str("        loop {\n");
    body.push_str("            let __more = d.has_next_field()?\n");
    body.push_str("            if !__more {\n");
    body.push_str("                break\n");
    body.push_str("            }\n");
    body.push_str("            let __name = d.field_name()?\n");
    if s.fields.is_empty() {
        body.push_str("            let failure = unknown_field(__name.as_str())\n");
        body.push_str("            return Err(failure.field(__name.as_str()))\n");
    } else {
        for (i, f) in s.fields.iter().enumerate() {
            let field = &f.name.name;
            if i == 0 {
                body.push_str(&format!("            if __name.as_str() == \"{field}\" {{\n"));
            } else {
                body.push_str(&format!(" else if __name.as_str() == \"{field}\" {{\n"));
            }
            body.push_str(&format!("                if __f{i}.is_some() {{\n"));
            body.push_str(&format!("                    let failure = duplicate_field(\"{field}\")\n"));
            body.push_str(&format!("                    return Err(failure.field(\"{field}\"))\n"));
            body.push_str("                }\n");
            body.push_str(&read_value(&f.ty, "__value", &format!(".field(\"{field}\")"), "                "));
            body.push_str(&format!("                __f{i} = Some(__value)\n"));
            body.push_str("            }");
        }
        body.push_str(" else {\n");
        body.push_str("                let failure = unknown_field(__name.as_str())\n");
        body.push_str("                return Err(failure.field(__name.as_str()))\n");
        body.push_str("            }\n");
    }
    body.push_str("        }\n");
    body.push_str("        d.end_struct()?\n");
    for (i, f) in s.fields.iter().enumerate() {
        let field = &f.name.name;
        body.push_str(&format!("        let __v{i} = match __f{i} {{\n"));
        body.push_str("            Some(value) => value,\n");
        body.push_str("            None => {\n");
        body.push_str(&format!("                let failure = missing_field(\"{field}\")\n"));
        body.push_str("                return Err(failure)\n");
        body.push_str("            }\n");
        body.push_str("        }\n");
    }
    let init: Vec<String> = s.fields.iter().enumerate().map(|(i, f)| format!("{}: __v{i}", f.name.name)).collect();
    body.push_str(&format!("        Ok({name} {{ {} }})\n", init.join(", ")));
    format!(
        "{} {{\n    fn deserialize<__D: Deserializer>(mut d: __D): Result<Self, DeError> {{\n{body}    }}\n}}\n",
        head(name, &s.generics, "Deserialize")
    )
}

fn deserialize_enum_impl(e: &EnumDecl) -> String {
    let name = &e.name.name;
    let mut body = format!("        d.begin_enum(\"{name}\")?\n");
    body.push_str("        let __variant = d.variant_name()?\n");
    if e.variants.is_empty() {
        body.push_str(&format!("        let failure = unknown_variant(\"{name}\", __variant.as_str())\n"));
        body.push_str("        Err(failure)\n");
    } else {
        for (vi, v) in e.variants.iter().enumerate() {
            let variant = &v.name.name;
            if vi == 0 {
                body.push_str(&format!("        if __variant.as_str() == \"{variant}\" {{\n"));
            } else {
                body.push_str(&format!(" else if __variant.as_str() == \"{variant}\" {{\n"));
            }
            let (ctor, types): (String, Vec<Type>) = match &v.kind {
                VariantKind::Unit => (format!("{name}.{variant}"), Vec::new()),
                VariantKind::Tuple(types) => {
                    let values: Vec<String> = (0..types.len()).map(|i| format!("__value{i}")).collect();
                    (format!("{name}.{variant}({})", values.join(", ")), types.clone())
                }
                VariantKind::Struct(fields) => {
                    let values: Vec<String> =
                        fields.iter().enumerate().map(|(i, f)| format!("{}: __value{i}", f.name.name)).collect();
                    let types: Vec<Type> = fields.iter().map(|f| f.ty.clone()).collect();
                    (format!("{name}.{variant} {{ {} }}", values.join(", ")), types)
                }
            };
            let arity = types.len();
            body.push_str(&format!("            d.begin_variant_payload({arity})?\n"));
            for (i, ty) in types.iter().enumerate() {
                body.push_str(&format!("            d.variant_payload_field({i})?\n"));
                let wrap = format!(".index({i}).variant(\"{variant}\")");
                body.push_str(&read_value(ty, &format!("__value{i}"), &wrap, "            "));
            }
            body.push_str(&format!("            d.end_variant_payload({arity})?\n"));
            body.push_str(&format!("            Ok({ctor})\n"));
            body.push_str("        }");
        }
        body.push_str(" else {\n");
        body.push_str(&format!("            let failure = unknown_variant(\"{name}\", __variant.as_str())\n"));
        body.push_str("            Err(failure)\n");
        body.push_str("        }\n");
    }
    format!(
        "{} {{\n    fn deserialize<__D: Deserializer>(mut d: __D): Result<Self, DeError> {{\n{body}    }}\n}}\n",
        head(name, &e.generics, "Deserialize")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn parsed(src: &str) -> File {
        let mut diags = Diagnostics::default();
        let file = parse(FileId(0), src, &mut diags);
        assert!(!diags.has_errors());
        file
    }

    fn derived(src: &str) -> usize {
        parsed(src).items.iter().filter(|i| matches!(i, Item::Impl(_))).count()
    }

    fn only_struct(src: &str) -> StructDecl {
        match parsed(src).items.into_iter().next() {
            Some(Item::Struct(s)) => s,
            other => panic!("expected a struct, got {other:?}"),
        }
    }

    fn only_enum(src: &str) -> EnumDecl {
        match parsed(src).items.into_iter().next() {
            Some(Item::Enum(e)) => e,
            other => panic!("expected an enum, got {other:?}"),
        }
    }

    #[test]
    fn adds_one_impl_per_derive() {
        assert_eq!(derived("struct P: Debug {\n x: i32\n y: i32\n}\n"), 1);
        assert_eq!(derived("enum E: Debug, Eq {\n A\n B(i32, i32)\n C { w: i32 }\n}\n"), 1);
        assert_eq!(derived("struct W<T>: Debug {\n v: T\n}\nstruct Q: Eq {\n a: i32\n}\n"), 1);
        assert_eq!(derived("struct E0: Debug {\n}\n"), 1);
    }

    #[test]
    fn adds_two_impls_for_the_serialization_derives() {
        assert_eq!(derived("struct P: Serialize {\n x: i32\n}\n"), 1);
        assert_eq!(derived("struct P: Deserialize {\n x: i32\n}\n"), 1);
        assert_eq!(derived("struct P: Serialize, Deserialize {\n x: i32\n}\n"), 2);
        assert_eq!(derived("enum E: Serialize, Deserialize {\n A\n B(i32)\n}\n"), 2);
        assert_eq!(derived("struct P: Debug, Serialize, Deserialize {\n x: i32\n}\n"), 3);
    }

    #[test]
    fn struct_text_is_pinned() {
        let s = only_struct("struct Point: Serialize, Deserialize {\n x: i32\n y: i32\n}\n");
        assert_eq!(
            serialize_struct_impl(&s),
            "Point: Serialize {\n\
             \x20   fn serialize<__S: Serializer>(self, mut s: __S): Result<(), DeError> {\n\
             \x20       s.begin_struct(\"Point\", 2)?\n\
             \x20       s.struct_field(\"x\")?\n\
             \x20       serialize_i32(mut s, self.x)?\n\
             \x20       s.struct_field(\"y\")?\n\
             \x20       serialize_i32(mut s, self.y)?\n\
             \x20       s.end_struct()\n\
             \x20   }\n\
             }\n"
        );
        assert_eq!(
            deserialize_struct_impl(&s),
            "Point: Deserialize {\n\
             \x20   fn deserialize<__D: Deserializer>(mut d: __D): Result<Self, DeError> {\n\
             \x20       d.begin_struct(\"Point\")?\n\
             \x20       let mut __f0: Option<i32> = None\n\
             \x20       let mut __f1: Option<i32> = None\n\
             \x20       loop {\n\
             \x20           let __more = d.has_next_field()?\n\
             \x20           if !__more {\n\
             \x20               break\n\
             \x20           }\n\
             \x20           let __name = d.field_name()?\n\
             \x20           if __name.as_str() == \"x\" {\n\
             \x20               if __f0.is_some() {\n\
             \x20                   let failure = duplicate_field(\"x\")\n\
             \x20                   return Err(failure.field(\"x\"))\n\
             \x20               }\n\
             \x20               let __value: i32 = match deserialize_i32(mut d) {\n\
             \x20                   Ok(value) => value,\n\
             \x20                   Err(e) => return Err(e.field(\"x\")),\n\
             \x20               }\n\
             \x20               __f0 = Some(__value)\n\
             \x20           } else if __name.as_str() == \"y\" {\n\
             \x20               if __f1.is_some() {\n\
             \x20                   let failure = duplicate_field(\"y\")\n\
             \x20                   return Err(failure.field(\"y\"))\n\
             \x20               }\n\
             \x20               let __value: i32 = match deserialize_i32(mut d) {\n\
             \x20                   Ok(value) => value,\n\
             \x20                   Err(e) => return Err(e.field(\"y\")),\n\
             \x20               }\n\
             \x20               __f1 = Some(__value)\n\
             \x20           } else {\n\
             \x20               let failure = unknown_field(__name.as_str())\n\
             \x20               return Err(failure.field(__name.as_str()))\n\
             \x20           }\n\
             \x20       }\n\
             \x20       d.end_struct()?\n\
             \x20       let __v0 = match __f0 {\n\
             \x20           Some(value) => value,\n\
             \x20           None => {\n\
             \x20               let failure = missing_field(\"x\")\n\
             \x20               return Err(failure)\n\
             \x20           }\n\
             \x20       }\n\
             \x20       let __v1 = match __f1 {\n\
             \x20           Some(value) => value,\n\
             \x20           None => {\n\
             \x20               let failure = missing_field(\"y\")\n\
             \x20               return Err(failure)\n\
             \x20           }\n\
             \x20       }\n\
             \x20       Ok(Point { x: __v0, y: __v1 })\n\
             \x20   }\n\
             }\n"
        );
    }

    #[test]
    fn generic_struct_text_is_pinned() {
        let s = only_struct("struct Pair<T>: Serialize, Deserialize {\n a: T\n b: T\n}\n");
        assert_eq!(
            serialize_struct_impl(&s),
            "Pair<T>: Serialize where T: Serialize {\n\
             \x20   fn serialize<__S: Serializer>(self, mut s: __S): Result<(), DeError> {\n\
             \x20       s.begin_struct(\"Pair\", 2)?\n\
             \x20       s.struct_field(\"a\")?\n\
             \x20       self.a.serialize(mut s)?\n\
             \x20       s.struct_field(\"b\")?\n\
             \x20       self.b.serialize(mut s)?\n\
             \x20       s.end_struct()\n\
             \x20   }\n\
             }\n"
        );
        assert_eq!(
            deserialize_struct_impl(&s),
            "Pair<T>: Deserialize where T: Deserialize {\n\
             \x20   fn deserialize<__D: Deserializer>(mut d: __D): Result<Self, DeError> {\n\
             \x20       d.begin_struct(\"Pair\")?\n\
             \x20       let mut __f0: Option<T> = None\n\
             \x20       let mut __f1: Option<T> = None\n\
             \x20       loop {\n\
             \x20           let __more = d.has_next_field()?\n\
             \x20           if !__more {\n\
             \x20               break\n\
             \x20           }\n\
             \x20           let __name = d.field_name()?\n\
             \x20           if __name.as_str() == \"a\" {\n\
             \x20               if __f0.is_some() {\n\
             \x20                   let failure = duplicate_field(\"a\")\n\
             \x20                   return Err(failure.field(\"a\"))\n\
             \x20               }\n\
             \x20               let __value: T = match T.deserialize(mut d) {\n\
             \x20                   Ok(value) => value,\n\
             \x20                   Err(e) => return Err(e.field(\"a\")),\n\
             \x20               }\n\
             \x20               __f0 = Some(__value)\n\
             \x20           } else if __name.as_str() == \"b\" {\n\
             \x20               if __f1.is_some() {\n\
             \x20                   let failure = duplicate_field(\"b\")\n\
             \x20                   return Err(failure.field(\"b\"))\n\
             \x20               }\n\
             \x20               let __value: T = match T.deserialize(mut d) {\n\
             \x20                   Ok(value) => value,\n\
             \x20                   Err(e) => return Err(e.field(\"b\")),\n\
             \x20               }\n\
             \x20               __f1 = Some(__value)\n\
             \x20           } else {\n\
             \x20               let failure = unknown_field(__name.as_str())\n\
             \x20               return Err(failure.field(__name.as_str()))\n\
             \x20           }\n\
             \x20       }\n\
             \x20       d.end_struct()?\n\
             \x20       let __v0 = match __f0 {\n\
             \x20           Some(value) => value,\n\
             \x20           None => {\n\
             \x20               let failure = missing_field(\"a\")\n\
             \x20               return Err(failure)\n\
             \x20           }\n\
             \x20       }\n\
             \x20       let __v1 = match __f1 {\n\
             \x20           Some(value) => value,\n\
             \x20           None => {\n\
             \x20               let failure = missing_field(\"b\")\n\
             \x20               return Err(failure)\n\
             \x20           }\n\
             \x20       }\n\
             \x20       Ok(Pair { a: __v0, b: __v1 })\n\
             \x20   }\n\
             }\n"
        );
    }

    #[test]
    fn enum_text_is_pinned() {
        let e = only_enum("enum E: Serialize, Deserialize {\n A\n B(i32, String)\n C { x: bool, y: f64 }\n}\n");
        assert_eq!(
            serialize_enum_impl(&e),
            "E: Serialize {\n\
             \x20   fn serialize<__S: Serializer>(self, mut s: __S): Result<(), DeError> {\n\
             \x20       match self {\n\
             \x20           E.A => {\n\
             \x20               s.begin_variant(\"E\", \"A\", 0, 0)?\n\
             \x20               s.end_variant(0)\n\
             \x20           }\n\
             \x20           E.B(__a0, __a1) => {\n\
             \x20               s.begin_variant(\"E\", \"B\", 1, 2)?\n\
             \x20               serialize_i32(mut s, __a0)?\n\
             \x20               serialize_str(mut s, __a1.as_str())?\n\
             \x20               s.end_variant(2)\n\
             \x20           }\n\
             \x20           E.C { x: __a0, y: __a1 } => {\n\
             \x20               s.begin_variant(\"E\", \"C\", 2, 2)?\n\
             \x20               serialize_bool(mut s, __a0)?\n\
             \x20               serialize_f64(mut s, __a1)?\n\
             \x20               s.end_variant(2)\n\
             \x20           }\n\
             \x20       }\n\
             \x20   }\n\
             }\n"
        );
        assert_eq!(
            deserialize_enum_impl(&e),
            "E: Deserialize {\n\
             \x20   fn deserialize<__D: Deserializer>(mut d: __D): Result<Self, DeError> {\n\
             \x20       d.begin_enum(\"E\")?\n\
             \x20       let __variant = d.variant_name()?\n\
             \x20       if __variant.as_str() == \"A\" {\n\
             \x20           d.begin_variant_payload(0)?\n\
             \x20           d.end_variant_payload(0)?\n\
             \x20           Ok(E.A)\n\
             \x20       } else if __variant.as_str() == \"B\" {\n\
             \x20           d.begin_variant_payload(2)?\n\
             \x20           d.variant_payload_field(0)?\n\
             \x20           let __value0: i32 = match deserialize_i32(mut d) {\n\
             \x20               Ok(value) => value,\n\
             \x20               Err(e) => return Err(e.index(0).variant(\"B\")),\n\
             \x20           }\n\
             \x20           d.variant_payload_field(1)?\n\
             \x20           let __value1: String = match deserialize_str(mut d) {\n\
             \x20               Ok(value) => value,\n\
             \x20               Err(e) => return Err(e.index(1).variant(\"B\")),\n\
             \x20           }\n\
             \x20           d.end_variant_payload(2)?\n\
             \x20           Ok(E.B(__value0, __value1))\n\
             \x20       } else if __variant.as_str() == \"C\" {\n\
             \x20           d.begin_variant_payload(2)?\n\
             \x20           d.variant_payload_field(0)?\n\
             \x20           let __value0: bool = match deserialize_bool(mut d) {\n\
             \x20               Ok(value) => value,\n\
             \x20               Err(e) => return Err(e.index(0).variant(\"C\")),\n\
             \x20           }\n\
             \x20           d.variant_payload_field(1)?\n\
             \x20           let __value1: f64 = match deserialize_f64(mut d) {\n\
             \x20               Ok(value) => value,\n\
             \x20               Err(e) => return Err(e.index(1).variant(\"C\")),\n\
             \x20           }\n\
             \x20           d.end_variant_payload(2)?\n\
             \x20           Ok(E.C { x: __value0, y: __value1 })\n\
             \x20       } else {\n\
             \x20           let failure = unknown_variant(\"E\", __variant.as_str())\n\
             \x20           Err(failure)\n\
             \x20       }\n\
             \x20   }\n\
             }\n"
        );
    }
}
