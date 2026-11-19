//! The part of rustdoc's JSON output this tool reads (`spec/rust-interop.md`, RI2).
//!
//! rustdoc's JSON format is unstable: it carries a `format_version` that changes with the toolchain, and an item's
//! shape changes with it. This module reads **one** version ([`SUPPORTED_FORMAT`]) and refuses every other, so a
//! toolchain bump is a deliberate edit of this file together with the golden outputs, never a silent
//! misreading. It keeps only what the generator needs: modules and re-exports, structs, enums, functions, constants,
//! and inherent impls, with the types of their signatures.

use burdin_json::json::Json;
use std::collections::HashMap;

/// The rustdoc JSON `format_version` this tool reads (the one Rust 1.100 writes).
pub const SUPPORTED_FORMAT: u32 = 61;

/// A crate's public API as rustdoc described it.
pub struct Krate {
    /// The crate's name: the root module's name, as Rust code names it.
    pub name: String,
    pub version: Option<String>,
    pub format_version: u32,
    /// The target triple the JSON was produced for (`x86_64-unknown-linux-gnu`).
    pub triple: String,
    pub root: u32,
    pub items: HashMap<u32, Item>,
}

pub struct Item {
    pub id: u32,
    pub name: Option<String>,
    pub public: bool,
    pub attrs: Attrs,
    /// The file and line of the item's span, as rustdoc wrote them (relative to the crate).
    pub file: Option<String>,
    pub line: u32,
    pub kind: Kind,
}

/// The attributes the generator cares about.
#[derive(Default, Clone)]
pub struct Attrs {
    pub repr_c: bool,
    /// `repr(u8)`, `repr(i32)`, ...
    pub repr_int: Option<String>,
    /// `repr(packed)`, `repr(align(N))`, or `repr(transparent)`: a layout this tool does not reproduce.
    pub repr_other: bool,
    /// `#[no_mangle]` or `#[export_name = "..."]`, and the exported name in the second case.
    pub no_mangle: bool,
    pub export_name: Option<String>,
}

pub enum StructKind {
    Unit,
    Tuple,
    /// Named fields. `stripped` is true when the struct has a private field.
    Plain {
        fields: Vec<u32>,
        stripped: bool,
    },
}

pub enum Kind {
    Module(Vec<u32>),
    Struct { kind: StructKind, generic: bool, impls: Vec<u32> },
    Enum { variants: Vec<u32>, stripped: bool, generic: bool, impls: Vec<u32> },
    Variant { plain: bool, discriminant: Option<String> },
    Field(Ty),
    Function(Func),
    Constant { ty: Ty, expr: String, literal: bool },
    Use { target: Option<u32>, name: String, glob: bool },
    Impl(ImplDef),
    Trait,
    TypeAlias,
    Other(String),
}

pub struct Func {
    /// `(name, type)`; the receiver of a method is the first entry, named `self`.
    pub inputs: Vec<(String, Ty)>,
    pub output: Option<Ty>,
    pub variadic: bool,
    /// A type or const parameter (or an `impl Trait` argument): the function has no single C signature.
    pub generic: bool,
    pub is_async: bool,
    pub is_unsafe: bool,
    /// `extern "C"`.
    pub abi_c: bool,
}

pub struct ImplDef {
    /// An `impl Type { ... }` block: no trait, not synthetic, not a blanket impl.
    pub inherent: bool,
    pub generic: bool,
    /// `impl Drop for Type`.
    pub drop_trait: bool,
    /// The type the block is for, when it is a local path.
    pub for_id: Option<u32>,
    pub items: Vec<u32>,
}

/// A type as it appears in a signature or a field.
#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Prim(String),
    Generic(String),
    Path {
        path: String,
        id: Option<u32>,
        args: Vec<Ty>,
    },
    Ref {
        mutable: bool,
        inner: Box<Ty>,
    },
    Ptr {
        mutable: bool,
        inner: Box<Ty>,
    },
    Slice(Box<Ty>),
    Array(Box<Ty>, String),
    Tuple(Vec<Ty>),
    /// Anything else (function pointers, trait objects, `impl Trait`, qualified paths), by the name of its JSON key.
    Other(String),
}

impl Krate {
    /// Parses rustdoc's JSON text. The error names what was wrong, for the command to print.
    pub fn parse(text: &str) -> Result<Krate, String> {
        let json = Json::parse(text).map_err(|e| format!("the rustdoc JSON is not valid JSON: {e}"))?;
        let format_version = json.get("format_version").as_u32().ok_or_else(|| {
            "the rustdoc JSON has no `format_version`; is it rustdoc's `--output-format json`?".to_string()
        })?;
        if format_version != SUPPORTED_FORMAT {
            return Err(format!(
                "the rustdoc JSON has format version {format_version}, but this `burdin` reads version {SUPPORTED_FORMAT} \
                 (the format Rust 1.100 writes); pin the toolchain with `[rust] toolchain = \"1.100.0\"`"
            ));
        }
        let root = json.get("root").as_u32().ok_or("the rustdoc JSON has no `root`")?;
        let triple = json.at(&["target", "triple"]).as_str().unwrap_or_default().to_string();
        let mut items = HashMap::new();
        for (key, value) in json.get("index").as_object() {
            let Ok(id) = key.parse::<u32>() else { continue };
            // Items of other crates are never in the index of a `--no-deps` run, but a stray one is not ours.
            if value.get("crate_id").as_u32().unwrap_or(0) != 0 {
                continue;
            }
            items.insert(id, parse_item(id, value));
        }
        let root_item = items.get(&root).ok_or("the rustdoc JSON's root item is missing from its index")?;
        let name = root_item.name.clone().ok_or("the rustdoc JSON's root module has no name")?;
        let version = json.get("crate_version").as_str().map(str::to_string);
        Ok(Krate { name, version, format_version, triple, root, items })
    }

    pub fn item(&self, id: u32) -> Option<&Item> {
        self.items.get(&id)
    }
}

fn parse_item(id: u32, json: &Json) -> Item {
    let name = json.get("name").as_str().map(str::to_string);
    let public = json.get("visibility").as_str() == Some("public");
    // Windows writes `src\lib.rs`; a note should read the same on every platform.
    let file = json.at(&["span", "filename"]).as_str().map(|name| name.replace('\\', "/"));
    let line = json.at(&["span", "begin"]).as_array().first().and_then(Json::as_u32).unwrap_or(0);
    let attrs = parse_attrs(json.get("attrs"));
    let inner = json.get("inner");
    let (key, body) = match inner.as_object().first() {
        Some((key, body)) => (key.as_str(), body),
        None => ("", &Json::Null),
    };
    let kind = match key {
        "module" => Kind::Module(ids(body.get("items"))),
        "struct" => Kind::Struct {
            kind: parse_struct_kind(body.get("kind")),
            generic: has_generics(body.get("generics")),
            impls: ids(body.get("impls")),
        },
        "enum" => Kind::Enum {
            variants: ids(body.get("variants")),
            stripped: body.get("has_stripped_variants").as_bool().unwrap_or(false),
            generic: has_generics(body.get("generics")),
            impls: ids(body.get("impls")),
        },
        "variant" => Kind::Variant {
            plain: body.get("kind").as_str() == Some("plain"),
            discriminant: body.at(&["discriminant", "value"]).as_str().map(str::to_string),
        },
        "struct_field" => Kind::Field(parse_ty(body)),
        "function" => Kind::Function(parse_function(body)),
        "constant" => Kind::Constant {
            ty: parse_ty(body.get("type")),
            expr: body.at(&["const", "expr"]).as_str().unwrap_or_default().to_string(),
            literal: body.at(&["const", "is_literal"]).as_bool().unwrap_or(false),
        },
        "use" => Kind::Use {
            target: body.get("id").as_u32(),
            name: body.get("name").as_str().unwrap_or_default().to_string(),
            glob: body.get("is_glob").as_bool().unwrap_or(false),
        },
        "impl" => {
            let blanket = !body.get("blanket_impl").is_null();
            let synthetic = body.get("is_synthetic").as_bool().unwrap_or(false);
            let for_id = body.at(&["for", "resolved_path", "id"]).as_u32();
            Kind::Impl(ImplDef {
                inherent: body.get("trait").is_null() && !blanket && !synthetic,
                generic: has_generics(body.get("generics")),
                drop_trait: body.at(&["trait", "path"]).as_str().is_some_and(|p| p.rsplit("::").next() == Some("Drop")),
                for_id,
                items: ids(body.get("items")),
            })
        }
        "trait" => Kind::Trait,
        "type_alias" => Kind::TypeAlias,
        other => Kind::Other(other.to_string()),
    };
    Item { id, name, public, attrs, file, line, kind }
}

fn ids(json: &Json) -> Vec<u32> {
    json.as_array().iter().filter_map(Json::as_u32).collect()
}

/// Whether a `generics` object has a type or const parameter. Lifetime parameters do not count: a lifetime has no
/// effect on the C signature.
fn has_generics(generics: &Json) -> bool {
    generics.get("params").as_array().iter().any(|param| param.get("kind").get("lifetime").is_null())
}

fn parse_struct_kind(kind: &Json) -> StructKind {
    if kind.as_str() == Some("unit") {
        return StructKind::Unit;
    }
    if !kind.get("tuple").is_null() {
        return StructKind::Tuple;
    }
    let plain = kind.get("plain");
    StructKind::Plain {
        fields: ids(plain.get("fields")),
        stripped: plain.get("has_stripped_fields").as_bool().unwrap_or(false),
    }
}

fn parse_attrs(json: &Json) -> Attrs {
    let mut attrs = Attrs::default();
    for attr in json.as_array() {
        if attr.as_str() == Some("no_mangle") {
            attrs.no_mangle = true;
            continue;
        }
        let repr = attr.get("repr");
        if !repr.is_null() {
            match repr.get("kind").as_str() {
                Some("c") => attrs.repr_c = true,
                Some("transparent") => attrs.repr_other = true,
                _ => {}
            }
            if let Some(int) = repr.get("int").as_str() {
                attrs.repr_int = Some(int.to_string());
            }
            if !repr.get("packed").is_null() || !repr.get("align").is_null() {
                attrs.repr_other = true;
            }
        }
        if let Some(name) = attr.get("export_name").as_str() {
            attrs.no_mangle = true;
            attrs.export_name = Some(name.to_string());
        }
    }
    attrs
}

fn parse_function(body: &Json) -> Func {
    let sig = body.get("sig");
    let inputs = sig
        .get("inputs")
        .as_array()
        .iter()
        .map(|pair| {
            let pair = pair.as_array();
            let name = pair.first().and_then(Json::as_str).unwrap_or("_").to_string();
            let ty = pair.get(1).map(parse_ty).unwrap_or(Ty::Other("missing".into()));
            (name, ty)
        })
        .collect();
    let output = match sig.get("output") {
        Json::Null => None,
        // A function that returns `()` writes `null`, but an explicit empty tuple is the same thing.
        other => match parse_ty(other) {
            Ty::Tuple(items) if items.is_empty() => None,
            ty => Some(ty),
        },
    };
    let header = body.get("header");
    let abi = header.get("abi");
    Func {
        inputs,
        output,
        variadic: sig.get("is_c_variadic").as_bool().unwrap_or(false),
        generic: has_generics(body.get("generics")),
        is_async: header.get("is_async").as_bool().unwrap_or(false),
        is_unsafe: header.get("is_unsafe").as_bool().unwrap_or(false),
        abi_c: abi.as_str() == Some("C") || !abi.get("C").is_null(),
    }
}

fn parse_ty(json: &Json) -> Ty {
    let Some((key, body)) = json.as_object().first() else {
        return Ty::Other("missing".to_string());
    };
    match key.as_str() {
        "primitive" => Ty::Prim(body.as_str().unwrap_or_default().to_string()),
        "generic" => Ty::Generic(body.as_str().unwrap_or_default().to_string()),
        "resolved_path" => {
            let args = body
                .at(&["args", "angle_bracketed", "args"])
                .as_array()
                .iter()
                .filter_map(|arg| {
                    let ty = arg.get("type");
                    if ty.is_null() { None } else { Some(parse_ty(ty)) }
                })
                .collect();
            Ty::Path {
                path: body.get("path").as_str().unwrap_or_default().to_string(),
                id: body.get("id").as_u32(),
                args,
            }
        }
        "borrowed_ref" => Ty::Ref {
            mutable: body.get("is_mutable").as_bool().unwrap_or(false),
            inner: Box::new(parse_ty(body.get("type"))),
        },
        "raw_pointer" => Ty::Ptr {
            mutable: body.get("is_mutable").as_bool().unwrap_or(false),
            inner: Box::new(parse_ty(body.get("type"))),
        },
        "slice" => Ty::Slice(Box::new(parse_ty(body))),
        "array" => {
            Ty::Array(Box::new(parse_ty(body.get("type"))), body.get("len").as_str().unwrap_or_default().to_string())
        }
        "tuple" => Ty::Tuple(body.as_array().iter().map(parse_ty).collect()),
        other => Ty::Other(other.to_string()),
    }
}

impl Ty {
    /// The last segment of a path type (`core::ffi::c_char` is `c_char`).
    pub fn tail(&self) -> Option<&str> {
        match self {
            Ty::Path { path, .. } => Some(path.rsplit("::").next().unwrap_or(path)),
            _ => None,
        }
    }

    /// A short description for a note: `&str`, `Vec<u8>`, `*const T`.
    pub fn describe(&self) -> String {
        match self {
            Ty::Prim(name) | Ty::Generic(name) => name.clone(),
            Ty::Path { path, args, .. } => {
                let tail = path.rsplit("::").next().unwrap_or(path);
                if args.is_empty() {
                    tail.to_string()
                } else {
                    let args: Vec<String> = args.iter().map(Ty::describe).collect();
                    format!("{tail}<{}>", args.join(", "))
                }
            }
            Ty::Ref { mutable, inner } => format!("&{}{}", if *mutable { "mut " } else { "" }, inner.describe()),
            Ty::Ptr { mutable, inner } => {
                format!("*{} {}", if *mutable { "mut" } else { "const" }, inner.describe())
            }
            Ty::Slice(inner) => format!("[{}]", inner.describe()),
            Ty::Array(inner, len) => format!("[{}; {len}]", inner.describe()),
            Ty::Tuple(items) => {
                let items: Vec<String> = items.iter().map(Ty::describe).collect();
                format!("({})", items.join(", "))
            }
            Ty::Other(kind) => match kind.as_str() {
                "function_pointer" => "a function pointer".to_string(),
                "dyn_trait" => "a trait object".to_string(),
                "impl_trait" => "`impl Trait`".to_string(),
                other => other.replace('_', " "),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn krate(items: &str) -> String {
        format!(
            r#"{{"root":0,"crate_version":"1.2.3","format_version":{SUPPORTED_FORMAT},"target":{{"triple":"x86_64-unknown-linux-gnu"}},"index":{{{items}}},"paths":{{}},"external_crates":{{}},"includes_private":false}}"#
        )
    }

    const ROOT: &str = r#""0":{"id":0,"crate_id":0,"name":"demo","visibility":"public","span":null,"attrs":[],"inner":{"module":{"is_crate":true,"items":[1],"is_stripped":false}}}"#;

    #[test]
    fn another_format_version_is_refused() {
        let text = krate(ROOT).replace(&format!("\"format_version\":{SUPPORTED_FORMAT}"), "\"format_version\":99");
        let error = Krate::parse(&text).err().unwrap();
        assert!(error.contains("format version 99"), "{error}");
        assert!(error.contains("reads version 61"), "{error}");
    }

    #[test]
    fn text_that_is_not_rustdoc_json_is_refused() {
        assert!(Krate::parse("not json").err().unwrap().contains("not valid JSON"));
        assert!(Krate::parse("{}").err().unwrap().contains("format_version"));
    }

    #[test]
    fn a_function_item_is_read() {
        let function = r#""1":{"id":1,"crate_id":0,"name":"add","visibility":"public","span":{"filename":"src/lib.rs","begin":[3,1],"end":[3,9]},"attrs":["no_mangle"],"inner":{"function":{"sig":{"inputs":[["a",{"primitive":"i32"}],["p",{"raw_pointer":{"is_mutable":true,"type":{"primitive":"u8"}}}]],"output":{"primitive":"i32"},"is_c_variadic":false},"generics":{"params":[{"name":"'a","kind":{"lifetime":{"outlives":[]}}}],"where_predicates":[]},"header":{"is_const":false,"is_unsafe":false,"is_async":false,"abi":{"C":{"unwind":false}}},"has_body":true}}}"#;
        let krate = Krate::parse(&krate(&format!("{ROOT},{function}"))).unwrap();
        assert_eq!(krate.name, "demo");
        assert_eq!(krate.version.as_deref(), Some("1.2.3"));
        let item = krate.item(1).unwrap();
        assert!(item.public && item.attrs.no_mangle);
        assert_eq!(item.line, 3);
        let Kind::Function(function) = &item.kind else { panic!("not a function") };
        assert!(function.abi_c && !function.generic, "a lifetime parameter is not a generic");
        assert_eq!(function.inputs[1].1, Ty::Ptr { mutable: true, inner: Box::new(Ty::Prim("u8".into())) });
        assert_eq!(function.output, Some(Ty::Prim("i32".into())));
    }

    #[test]
    fn types_are_described_for_notes() {
        let ty = Ty::Path { path: "alloc::vec::Vec".into(), id: Some(7), args: vec![Ty::Prim("u8".into())] };
        assert_eq!(ty.describe(), "Vec<u8>");
        assert_eq!(
            Ty::Ref { mutable: true, inner: Box::new(Ty::Slice(Box::new(Ty::Prim("i32".into())))) }.describe(),
            "&mut [i32]"
        );
        assert_eq!(Ty::Other("function_pointer".into()).describe(), "a function pointer");
    }
}
