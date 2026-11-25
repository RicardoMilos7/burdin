//! From the rustdoc model to the exposed API: which items get a Burdin declaration, which get a shim function, which
//! are skipped and why (`spec/rust-interop.md`, RI3-RI5).
//!
//! The rules are deliberately few. A `#[repr(C)]` struct whose fields are all public and C-compatible is a **value**:
//! Burdin gets the same struct, and the shim passes it through a pointer. Every other struct, and every enum that is
//! not a fieldless enum with an integer representation, is a **handle**: an opaque pointer to a boxed value that the
//! shim creates and frees. A function becomes a shim function when each parameter and its result has one of the
//! shapes of RI4; any other signature is skipped with a note that names the reason. Nothing is guessed.

use crate::rustdoc::{Func, Item, Kind, Krate, StructKind, Ty};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The words Burdin reserves. A Rust field or parameter that would need one gains a trailing underscore.
const KEYWORDS: &[&str] = &[
    "any",
    "as",
    "break",
    "const",
    "continue",
    "derive",
    "else",
    "enum",
    "error",
    "extern",
    "false",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "interface",
    "let",
    "loop",
    "match",
    "mut",
    "orelse",
    "own",
    "priv",
    "return",
    "self",
    "Self",
    "struct",
    "true",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
];

pub fn burdin_name(name: &str) -> String {
    if KEYWORDS.contains(&name) { format!("{name}_") } else { name.to_string() }
}

/// A name Burdin accepts for a type, constant, or function: not a keyword, not empty, ASCII.
fn is_burdin_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !KEYWORDS.contains(&name)
}

/// `Counter` is `counter`, `Vec2` is `vec2`, `HTTPServer` is `http_server`, `into_total` is unchanged.
pub fn snake(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() {
            let prev_lower = i > 0 && (chars[i - 1].is_ascii_lowercase() || chars[i - 1].is_ascii_digit());
            let acronym_end = i > 0
                && chars[i - 1].is_ascii_uppercase()
                && chars.get(i + 1).is_some_and(|next| next.is_ascii_lowercase());
            if prev_lower || acronym_end {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The kind of integer an enumeration is stored as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Int {
    I8,
    I16,
    I32,
    I64,
    Isize,
    U8,
    U16,
    U32,
    U64,
    Usize,
}

impl Int {
    fn parse(name: &str) -> Option<Int> {
        Some(match name {
            "i8" => Int::I8,
            "i16" => Int::I16,
            "i32" => Int::I32,
            "i64" => Int::I64,
            "isize" => Int::Isize,
            "u8" => Int::U8,
            "u16" => Int::U16,
            "u32" => Int::U32,
            "u64" => Int::U64,
            "usize" => Int::Usize,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Int::I8 => "i8",
            Int::I16 => "i16",
            Int::I32 => "i32",
            Int::I64 => "i64",
            Int::Isize => "isize",
            Int::U8 => "u8",
            Int::U16 => "u16",
            Int::U32 => "u32",
            Int::U64 => "u64",
            Int::Usize => "usize",
        }
    }

    fn bytes(self, pointer: u64) -> u64 {
        match self {
            Int::I8 | Int::U8 => 1,
            Int::I16 | Int::U16 => 2,
            Int::I32 | Int::U32 => 4,
            Int::I64 | Int::U64 => 8,
            Int::Isize | Int::Usize => pointer,
        }
    }

    fn range(self, pointer: u64) -> (i128, i128) {
        let bits = self.bytes(pointer) * 8;
        match self {
            Int::I8 | Int::I16 | Int::I32 | Int::I64 | Int::Isize => {
                (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
            }
            _ => (0, (1i128 << bits) - 1),
        }
    }
}

/// A field of a value struct.
pub struct Field {
    /// The name in Rust, for `offset_of!`.
    pub rust_name: String,
    pub ty: Ty,
    pub offset: u64,
    /// The field's Burdin type; filled in once every exposed type is known.
    pub burdin_ty: String,
}

/// What an exposed type is.
pub enum Class {
    /// A `#[repr(C)]` struct passed by pointer and read field by field.
    Value { fields: Vec<Field>, size: u64, align: u64 },
    /// An opaque boxed value.
    Handle,
    /// A fieldless enum with an integer representation: `(variant name, discriminant)` for each variant, and the
    /// name of the Burdin constant that stands for it (empty when another item had the name).
    Enum { int: Int, variants: Vec<(String, i128)>, constants: Vec<String> },
}

pub struct TypeDef {
    pub id: u32,
    /// The Burdin name, which is the name Rust code uses for it (a re-export's name when it was renamed).
    pub name: String,
    /// `::demo::shapes::Rect`.
    pub rust_path: String,
    pub class: Class,
}

pub struct ConstDef {
    pub name: String,
    pub burdin_ty: String,
    /// The Burdin literal.
    pub value: String,
}

/// One function of the shim, with everything both sides need.
pub struct Wrapper {
    /// The exported symbol, which is also the Burdin name.
    pub symbol: String,
    pub shim_params: Vec<(String, String)>,
    pub shim_ret: Option<String>,
    pub burdin_params: Vec<(String, String)>,
    pub burdin_ret: Option<String>,
    /// The statements of the Rust body, the last of them the value returned.
    pub body: Vec<String>,
}

/// A `#[no_mangle] extern "C"` function of the crate: declared on the Burdin side as it is, with no shim.
pub struct DirectFn {
    pub symbol: String,
    pub params: Vec<(String, String)>,
    pub ret: Option<String>,
}

pub struct Api {
    pub crate_name: String,
    pub crate_version: Option<String>,
    pub consts: Vec<ConstDef>,
    pub types: Vec<TypeDef>,
    pub wrappers: Vec<Wrapper>,
    pub direct: Vec<DirectFn>,
    /// Whether a wrapper returns a `String` as a C string, so the shim also exports `<crate>_string_free`.
    pub string_free: Option<String>,
    pub notes: Vec<String>,
    /// The bytes of a pointer on the target; every `usize` field and pointer is this wide.
    pub pointer_bytes: u64,
}

/// Everything about one public item the walk found.
struct Found {
    id: u32,
    name: String,
    /// The module path from the crate root (`["util"]`), empty for the root.
    path: Vec<String>,
}

/// Reads the crate's public API.
pub fn build(krate: &Krate) -> Result<Api, String> {
    let pointer_bytes = pointer_bytes(&krate.triple)?;
    let long_bytes = if krate.triple.contains("windows") { 4 } else { pointer_bytes.min(8) };
    let mut builder = Builder {
        krate,
        pointer_bytes,
        long_bytes,
        notes: Vec::new(),
        classes: HashMap::new(),
        in_progress: HashSet::new(),
        names: HashSet::new(),
    };
    let found = builder.walk_public();
    Ok(builder.expose(found))
}

fn pointer_bytes(triple: &str) -> Result<u64, String> {
    let arch = triple.split('-').next().unwrap_or("");
    match arch {
        "x86_64" | "aarch64" | "arm64" | "arm64e" | "riscv64gc" | "riscv64" | "powerpc64" | "powerpc64le" | "s390x"
        | "loongarch64" | "wasm64" => Ok(8),
        "i686" | "i586" | "i386" | "arm" | "armv7" | "thumbv7em" | "riscv32gc" | "riscv32" | "wasm32" => Ok(4),
        _ => Err(format!(
            "the rustdoc JSON was produced for the target `{triple}`, whose pointer width this tool does not know"
        )),
    }
}

struct Builder<'a> {
    krate: &'a Krate,
    pointer_bytes: u64,
    long_bytes: u64,
    notes: Vec<String>,
    /// Memoized classes of the local structs and enums, by item id; `None` is a skipped type.
    classes: HashMap<u32, Option<usize>>,
    in_progress: HashSet<u32>,
    /// The names already taken in the Burdin module.
    names: HashSet<String>,
}

impl<'a> Builder<'a> {
    /// A note about an item, with where Rust declared it: `the function `f` at src/lib.rs:12 was skipped: why`.
    fn skipped(&mut self, item: &Item, what: &str, why: &str) {
        let at = self.location(item);
        self.notes.push(format!("{what}{at} was skipped: {why}"));
    }

    /// A note that is not a skip: `the struct `S` at src/lib.rs:12 ...`.
    fn remark(&mut self, item: &Item, what: &str, text: &str) {
        let at = self.location(item);
        self.notes.push(format!("{what}{at} {text}"));
    }

    fn location(&self, item: &Item) -> String {
        match &item.file {
            Some(file) if item.line > 0 => format!(" at {file}:{}", item.line),
            _ => String::new(),
        }
    }

    // ---- the walk -------------------------------------------------------------------------------------------

    /// Every public type, function, and constant reachable from the crate root, through public modules and `pub
    /// use`, each once, under its shortest public path.
    fn walk_public(&mut self) -> Vec<Found> {
        let mut all = Vec::new();
        let mut visited = HashSet::new();
        let mut path = Vec::new();
        self.walk_module(self.krate.root, &mut path, &mut all, &mut visited);
        // The same item can be public at two paths; keep the shortest, then the first alphabetically.
        let mut best: BTreeMap<u32, Found> = BTreeMap::new();
        for found in all {
            match best.get(&found.id) {
                Some(current)
                    if (current.path.len(), &current.path, &current.name)
                        <= (found.path.len(), &found.path, &found.name) => {}
                _ => {
                    best.insert(found.id, found);
                }
            }
        }
        let mut found: Vec<Found> = best.into_values().collect();
        found.sort_by(|a, b| (&a.path, &a.name, a.id).cmp(&(&b.path, &b.name, b.id)));
        found
    }

    fn walk_module(&mut self, module: u32, path: &mut Vec<String>, out: &mut Vec<Found>, visited: &mut HashSet<u32>) {
        if !visited.insert(module) {
            return;
        }
        let Some(Kind::Module(items)) = self.krate.item(module).map(|item| &item.kind) else { return };
        for &id in items {
            let Some(item) = self.krate.item(id) else { continue };
            if !item.public {
                continue;
            }
            match &item.kind {
                Kind::Module(_) => {
                    if let Some(name) = &item.name {
                        path.push(name.clone());
                        self.walk_module(id, path, out, visited);
                        path.pop();
                    }
                }
                Kind::Use { target, name, glob } => {
                    let Some(target) = target.and_then(|id| self.krate.item(id).map(|item| (id, item))) else {
                        self.notes.push(
                            "an item re-exported from another crate was skipped: only the crate's own items are \
                             exposed"
                                .to_string(),
                        );
                        continue;
                    };
                    match (&target.1.kind, glob) {
                        (Kind::Module(_), true) => self.walk_module(target.0, path, out, visited),
                        (Kind::Module(_), false) => {
                            path.push(name.clone());
                            self.walk_module(target.0, path, out, visited);
                            path.pop();
                        }
                        (_, true) => {}
                        (_, false) => out.push(Found { id: target.0, name: name.clone(), path: path.clone() }),
                    }
                }
                Kind::Trait => {
                    let name = item.name.clone().unwrap_or_default();
                    self.skipped(item, &format!("the trait `{name}`"), "traits have no C form");
                }
                Kind::TypeAlias => {
                    let name = item.name.clone().unwrap_or_default();
                    self.skipped(item, &format!("the type alias `{name}`"), "use the type it names");
                }
                Kind::Struct { .. } | Kind::Enum { .. } | Kind::Function(_) | Kind::Constant { .. } => {
                    out.push(Found { id, name: item.name.clone().unwrap_or_default(), path: path.clone() });
                }
                _ => {}
            }
        }
    }

    // ---- exposing -------------------------------------------------------------------------------------------

    fn rust_path(&self, found: &Found) -> String {
        let mut path = format!("::{}", self.krate.name);
        for segment in &found.path {
            path.push_str("::");
            path.push_str(segment);
        }
        path.push_str("::");
        path.push_str(&found.name);
        path
    }

    fn expose(&mut self, found: Vec<Found>) -> Api {
        let mut api = Api {
            crate_name: self.krate.name.clone(),
            crate_version: self.krate.version.clone(),
            consts: Vec::new(),
            types: Vec::new(),
            wrappers: Vec::new(),
            direct: Vec::new(),
            string_free: None,
            notes: Vec::new(),
            pointer_bytes: self.pointer_bytes,
        };
        let prefix = snake(&self.krate.name);
        // Types first: a function's signature needs their classes.
        let mut type_defs: Vec<TypeDef> = Vec::new();
        let mut by_id: HashMap<u32, usize> = HashMap::new();
        for entry in &found {
            let item = self.krate.item(entry.id).expect("a found item is in the index");
            if !matches!(item.kind, Kind::Struct { .. } | Kind::Enum { .. }) {
                continue;
            }
            self.classify(entry, item, &found, &mut type_defs, &mut by_id);
        }
        // Names of types are claimed before functions and constants, so a clash is reported against the later one.
        api.types = type_defs;
        let by_id: HashMap<u32, usize> = api.types.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
        self.spell_fields(&mut api, &by_id);
        self.enum_constants(&mut api);

        for entry in &found {
            let item = self.krate.item(entry.id).expect("a found item is in the index");
            match &item.kind {
                Kind::Constant { .. } => self.constant(&mut api, entry, item),
                Kind::Function(func) => self.free_function(&mut api, &prefix, entry, item, func, &by_id),
                _ => {}
            }
        }
        // Methods of the exposed types, and a free function for each handle.
        for index in 0..api.types.len() {
            self.methods(&mut api, &prefix, index, &by_id);
            if matches!(api.types[index].class, Class::Handle) {
                self.handle_free(&mut api, &prefix, index);
            }
        }
        if api.string_free.is_some() {
            let symbol = format!("{prefix}_string_free");
            if self.names.contains(&symbol) {
                self.notes.push(format!("`{symbol}` clashes with another name, so strings cannot be freed"));
            }
            self.names.insert(symbol);
        }
        api.notes = std::mem::take(&mut self.notes);
        api
    }

    /// Fills in the Burdin type of every field of every value struct.
    fn spell_fields(&mut self, api: &mut Api, by_id: &HashMap<u32, usize>) {
        let mut spelled = Vec::new();
        for (t, def) in api.types.iter().enumerate() {
            let Class::Value { fields, .. } = &def.class else { continue };
            for (f, field) in fields.iter().enumerate() {
                spelled.push((t, f, self.field_ty(&field.ty, &api.types, by_id)));
            }
        }
        for (t, f, text) in spelled {
            if let Class::Value { fields, .. } = &mut api.types[t].class {
                fields[f].burdin_ty = text;
            }
        }
    }

    fn field_ty(&self, ty: &Ty, types: &[TypeDef], by_id: &HashMap<u32, usize>) -> String {
        match ty {
            Ty::Prim(name) if name == "bool" => "u8".to_string(),
            Ty::Prim(name) => name.clone(),
            Ty::Ptr { inner, .. } | Ty::Ref { inner, .. } => {
                let pointee = self
                    .pointee(inner)
                    .map(|p| p.1)
                    .or_else(|| match &**inner {
                        Ty::Path { id: Some(id), .. } => by_id.get(id).map(|&i| types[i].name.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "CVoid".to_string());
                format!("CPtr<{pointee}>")
            }
            Ty::Array(inner, len) => format!("[{}; {len}]", self.field_ty(inner, types, by_id)),
            Ty::Path { id, .. } => {
                if let Some(num) = self.c_alias(ty) {
                    return if ty.tail() == Some("c_char") { "CChar".to_string() } else { num.1 };
                }
                id.and_then(|id| by_id.get(&id)).map(|&i| types[i].name.clone()).unwrap_or_default()
            }
            _ => String::new(),
        }
    }

    /// The constants that stand for an enumeration's variants: `KIND_POINT` for `Kind::Point`.
    fn enum_constants(&mut self, api: &mut Api) {
        for def in &mut api.types {
            let Class::Enum { variants, constants, .. } = &mut def.class else { continue };
            for (variant, _) in variants.iter() {
                let name = format!("{}_{}", snake(&def.name).to_ascii_uppercase(), snake(variant).to_ascii_uppercase());
                if self.names.insert(name.clone()) {
                    constants.push(name);
                } else {
                    constants.push(String::new());
                    self.notes.push(format!(
                        "the constant `{name}` for `{}::{variant}` was skipped: another item has the name",
                        def.name
                    ));
                }
            }
        }
    }

    /// Classifies one public struct or enum (memoized), appending a definition when it is exposed. The index into
    /// `defs` is the class memo's value.
    fn classify(
        &mut self,
        entry: &Found,
        item: &Item,
        found: &[Found],
        defs: &mut Vec<TypeDef>,
        by_id: &mut HashMap<u32, usize>,
    ) -> Option<usize> {
        if let Some(done) = self.classes.get(&entry.id) {
            return *done;
        }
        if !self.in_progress.insert(entry.id) {
            return None;
        }
        let result = self.classify_inner(entry, item, found, defs, by_id);
        self.in_progress.remove(&entry.id);
        self.classes.insert(entry.id, result);
        result
    }

    fn classify_inner(
        &mut self,
        entry: &Found,
        item: &Item,
        found: &[Found],
        defs: &mut Vec<TypeDef>,
        by_id: &mut HashMap<u32, usize>,
    ) -> Option<usize> {
        let name = entry.name.clone();
        let (kind_word, generic) = match &item.kind {
            Kind::Struct { generic, .. } => ("struct", *generic),
            Kind::Enum { generic, .. } => ("enum", *generic),
            _ => return None,
        };
        if generic {
            self.skipped(item, &format!("the {kind_word} `{name}`"), "it is generic, and C has no generics");
            return None;
        }
        if !is_burdin_ident(&name) || !self.names.insert(name.clone()) {
            let why =
                if is_burdin_ident(&name) { "another item has the name" } else { "its name is reserved or invalid" };
            self.skipped(item, &format!("the {kind_word} `{name}`"), why);
            return None;
        }
        let rust_path = self.rust_path(entry);
        let class = match &item.kind {
            Kind::Struct { kind, impls, .. } => self.struct_class(item, kind, impls, found, defs, by_id),
            Kind::Enum { variants, stripped, impls, .. } => self.enum_class(item, variants, *stripped, impls),
            _ => unreachable!(),
        };
        let index = defs.len();
        defs.push(TypeDef { id: entry.id, name, rust_path, class });
        by_id.insert(entry.id, index);
        Some(index)
    }

    fn has_drop(&self, impls: &[u32]) -> bool {
        impls
            .iter()
            .any(|id| self.krate.item(*id).is_some_and(|item| matches!(&item.kind, Kind::Impl(i) if i.drop_trait)))
    }

    fn struct_class(
        &mut self,
        item: &Item,
        kind: &StructKind,
        impls: &[u32],
        found: &[Found],
        defs: &mut Vec<TypeDef>,
        by_id: &mut HashMap<u32, usize>,
    ) -> Class {
        let name = item.name.clone().unwrap_or_default();
        let StructKind::Plain { fields, stripped } = kind else {
            return Class::Handle;
        };
        if !item.attrs.repr_c || item.attrs.repr_other {
            return Class::Handle;
        }
        if *stripped {
            self.remark(
                item,
                &format!("the struct `{name}`"),
                "is `repr(C)` but has private fields, so it is an opaque handle",
            );
            return Class::Handle;
        }
        if self.has_drop(impls) {
            self.remark(item, &format!("the struct `{name}`"), "implements `Drop`, so it is an opaque handle");
            return Class::Handle;
        }
        // Lay the fields out as C does: each at the next multiple of its alignment, the whole padded to the largest.
        let mut laid = Vec::new();
        let (mut offset, mut max_align) = (0u64, 1u64);
        for &field_id in fields {
            let Some(field) = self.krate.item(field_id) else { return Class::Handle };
            let (Kind::Field(ty), Some(field_name)) = (&field.kind, field.name.clone()) else { return Class::Handle };
            if !field.public {
                self.remark(
                    item,
                    &format!("the struct `{name}`"),
                    "is `repr(C)` but has private fields, so it is an opaque handle",
                );
                return Class::Handle;
            }
            let Some((size, align)) = self.layout(ty, found, defs, by_id) else {
                self.remark(
                    item,
                    &format!("the struct `{name}`"),
                    &format!(
                        "is `repr(C)` but its field `{field_name}` has type `{}`, which C cannot hold, so it is an opaque handle",
                        ty.describe()
                    ),
                );
                return Class::Handle;
            };
            offset = offset.next_multiple_of(align);
            laid.push(Field { rust_name: field_name, ty: ty.clone(), offset, burdin_ty: String::new() });
            offset += size;
            max_align = max_align.max(align);
        }
        Class::Value { fields: laid, size: offset.next_multiple_of(max_align), align: max_align }
    }

    fn enum_class(&mut self, item: &Item, variants: &[u32], stripped: bool, _impls: &[u32]) -> Class {
        let name = item.name.clone().unwrap_or_default();
        let int = match (&item.attrs.repr_int, item.attrs.repr_c) {
            (Some(int), _) => Int::parse(int),
            (None, true) => Some(Int::I32),
            (None, false) => None,
        };
        let Some(int) = int.filter(|_| !stripped && !item.attrs.repr_other) else {
            return Class::Handle;
        };
        let (low, high) = int.range(self.pointer_bytes);
        let mut list = Vec::new();
        for &id in variants {
            let Some(variant) = self.krate.item(id) else { return Class::Handle };
            let Kind::Variant { plain, discriminant } = &variant.kind else { return Class::Handle };
            if !plain {
                self.remark(item, &format!("the enum `{name}`"), "has variants with fields, so it is an opaque handle");
                return Class::Handle;
            }
            let value = discriminant.as_deref().and_then(|d| d.parse::<i128>().ok());
            let (Some(value), Some(variant_name)) = (value.filter(|v| (low..=high).contains(v)), variant.name.clone())
            else {
                return Class::Handle;
            };
            list.push((variant_name, value));
        }
        if list.is_empty() {
            return Class::Handle;
        }
        Class::Enum { int, variants: list, constants: Vec::new() }
    }

    /// The size and alignment of a type held in a `repr(C)` struct, or `None` when C cannot hold it.
    fn layout(
        &mut self,
        ty: &Ty,
        found: &[Found],
        defs: &mut Vec<TypeDef>,
        by_id: &mut HashMap<u32, usize>,
    ) -> Option<(u64, u64)> {
        match ty {
            Ty::Prim(name) => self.prim_layout(name),
            Ty::Ptr { inner, .. } | Ty::Ref { inner, .. } => {
                // A pointer to a slice or `str` is two words.
                if matches!(**inner, Ty::Slice(_)) || matches!(&**inner, Ty::Prim(p) if p == "str") {
                    return None;
                }
                Some((self.pointer_bytes, self.pointer_bytes))
            }
            Ty::Array(inner, len) => {
                let (size, align) = self.layout(inner, found, defs, by_id)?;
                let len = len.parse::<u64>().ok()?;
                Some((size * len, align))
            }
            Ty::Path { id, .. } => {
                if let Some(num) = self.c_alias(ty) {
                    return self.prim_layout(&num.1);
                }
                let id = (*id)?;
                let entry = found.iter().find(|f| f.id == id)?;
                let item = self.krate.item(id)?;
                let index = self.classify(entry, item, found, defs, by_id)?;
                match &defs[index].class {
                    Class::Value { size, align, .. } => Some((*size, *align)),
                    Class::Enum { int, .. } => {
                        let bytes = int.bytes(self.pointer_bytes);
                        Some((bytes, bytes))
                    }
                    Class::Handle => None,
                }
            }
            _ => None,
        }
    }

    fn prim_layout(&self, name: &str) -> Option<(u64, u64)> {
        let size = match name {
            "i8" | "u8" | "bool" => 1,
            "i16" | "u16" => 2,
            "i32" | "u32" | "f32" => 4,
            "i64" | "u64" | "f64" => 8,
            "isize" | "usize" => self.pointer_bytes,
            _ => return None,
        };
        Some((size, size))
    }

    /// `c_int` and its relatives (`core::ffi::c_int`, `std::os::raw::c_int`, `libc`'s): the Rust spelling and the
    /// fixed-width primitive they are on this target. `c_void` is not a number.
    fn c_alias(&self, ty: &Ty) -> Option<(String, String)> {
        let Ty::Path { id, .. } = ty else { return None };
        if id.is_some_and(|id| self.krate.item(id).is_some()) {
            return None;
        }
        let tail = ty.tail()?;
        let long = if self.long_bytes == 4 { ("i32", "u32") } else { ("i64", "u64") };
        let prim = match tail {
            "c_char" | "c_schar" => "i8",
            "c_uchar" => "u8",
            "c_short" => "i16",
            "c_ushort" => "u16",
            "c_int" => "i32",
            "c_uint" => "u32",
            "c_long" => long.0,
            "c_ulong" => long.1,
            "c_longlong" => "i64",
            "c_ulonglong" => "u64",
            "c_float" => "f32",
            "c_double" => "f64",
            _ => return None,
        };
        Some((format!("::core::ffi::{tail}"), prim.to_string()))
    }

    // ---- constants ------------------------------------------------------------------------------------------

    fn constant(&mut self, api: &mut Api, entry: &Found, item: &Item) {
        let Kind::Constant { ty, expr, literal } = &item.kind else { return };
        let name = if entry.path.is_empty() {
            entry.name.clone()
        } else {
            format!("{}_{}", entry.path.join("_").to_ascii_uppercase(), entry.name)
        };
        let skip = |this: &mut Self, why: String| {
            this.skipped(item, &format!("the constant `{name}`"), &why);
        };
        if !is_burdin_ident(&name) {
            return skip(self, "its name is reserved or invalid".into());
        }
        if !*literal {
            return skip(self, "its value is not a literal".into());
        }
        let Some((burdin_ty, value)) = self.const_value(ty, expr) else {
            return skip(self, format!("its type `{}` or value `{expr}` has no Burdin form", ty.describe()));
        };
        if !self.names.insert(name.clone()) {
            return skip(self, "another item has the name".into());
        }
        api.consts.push(ConstDef { name, burdin_ty, value });
    }

    /// A constant's Burdin type and literal, from its Rust type and source text.
    fn const_value(&self, ty: &Ty, expr: &str) -> Option<(String, String)> {
        let expr = expr.trim();
        match ty {
            Ty::Prim(name) if name == "bool" && (expr == "true" || expr == "false") => {
                Some(("bool".into(), expr.to_string()))
            }
            Ty::Prim(name) if Int::parse(name).is_some() => {
                let text = expr.replace('_', "");
                let (negative, digits) = match text.strip_prefix('-') {
                    Some(rest) => (true, rest.to_string()),
                    None => (false, text),
                };
                let digits = strip_suffix(&digits, name);
                let value = if let Some(hex) = digits.strip_prefix("0x") {
                    i128::from_str_radix(hex, 16).ok()?
                } else if let Some(bin) = digits.strip_prefix("0b") {
                    i128::from_str_radix(bin, 2).ok()?
                } else if let Some(oct) = digits.strip_prefix("0o") {
                    i128::from_str_radix(oct, 8).ok()?
                } else {
                    digits.parse::<i128>().ok()?
                };
                let value = if negative { -value } else { value };
                let int = Int::parse(name)?;
                let (low, high) = int.range(self.pointer_bytes);
                (low..=high).contains(&value).then(|| (name.clone(), value.to_string()))
            }
            Ty::Prim(name) if name == "f32" || name == "f64" => {
                let text = strip_suffix(&expr.replace('_', ""), name);
                let value: f64 = text.parse().ok()?;
                if !value.is_finite() {
                    return None;
                }
                // A literal like `2` is `2.0` in Rust; Burdin wants the point.
                let shown = if text.contains(['.', 'e', 'E']) { text } else { format!("{text}.0") };
                Some((name.clone(), shown))
            }
            Ty::Ref { inner, .. } if matches!(&**inner, Ty::Prim(p) if p == "str") => {
                Some(("str".into(), burdin_string(&rust_string_literal(expr)?)))
            }
            _ => None,
        }
    }

    // ---- types, methods, and functions ----------------------------------------------------------------------

    fn handle_free(&mut self, api: &mut Api, prefix: &str, index: usize) {
        let def = &api.types[index];
        let symbol = format!("{prefix}_{}_free", snake(&def.name));
        if !self.names.insert(symbol.clone()) {
            self.notes.push(format!("`{symbol}` clashes with another name, so `{}` cannot be freed", def.name));
            return;
        }
        let ty = def.rust_path.clone();
        api.wrappers.push(Wrapper {
            symbol,
            shim_params: vec![("this".into(), format!("*mut {ty}"))],
            shim_ret: None,
            burdin_params: vec![("this".into(), format!("CPtr<{}>", def.name))],
            burdin_ret: None,
            body: vec!["if !this.is_null() { drop(unsafe { ::std::boxed::Box::from_raw(this) }); }".to_string()],
        });
    }

    /// The inherent methods of an exposed type.
    fn methods(&mut self, api: &mut Api, prefix: &str, index: usize, by_id: &HashMap<u32, usize>) {
        let id = api.types[index].id;
        let item = self.krate.item(id).expect("an exposed type is in the index");
        let impls = match &item.kind {
            Kind::Struct { impls, .. } | Kind::Enum { impls, .. } => impls.clone(),
            _ => return,
        };
        let type_name = api.types[index].name.clone();
        for impl_id in impls {
            let Some(Kind::Impl(block)) = self.krate.item(impl_id).map(|i| &i.kind) else { continue };
            if !block.inherent || block.for_id != Some(id) {
                continue;
            }
            if block.generic {
                continue;
            }
            for &method_id in &block.items {
                let Some(method) = self.krate.item(method_id) else { continue };
                let (Kind::Function(func), true, Some(method_name)) = (&method.kind, method.public, &method.name)
                else {
                    continue;
                };
                let symbol = format!("{prefix}_{}_{method_name}", snake(&type_name));
                let label = format!("{type_name}::{method_name}");
                let self_ty = Ty::Path { path: type_name.clone(), id: Some(id), args: Vec::new() };
                self.function(api, method, func, &symbol, &label, Some(&self_ty), by_id);
            }
        }
    }

    fn free_function(
        &mut self,
        api: &mut Api,
        prefix: &str,
        entry: &Found,
        item: &Item,
        func: &Func,
        by_id: &HashMap<u32, usize>,
    ) {
        // `extern "C"` with `#[no_mangle]`: it already has a C symbol, so Burdin declares it as it is.
        if func.abi_c && item.attrs.no_mangle {
            self.direct_function(api, item, func, &entry.name);
            return;
        }
        let mut symbol = prefix.to_string();
        for segment in &entry.path {
            symbol.push('_');
            symbol.push_str(segment);
        }
        symbol.push('_');
        symbol.push_str(&entry.name);
        let rust_path = self.rust_path(entry);
        self.function_at(api, item, func, &symbol, &entry.name, None, &rust_path, by_id);
    }

    fn direct_function(&mut self, api: &mut Api, item: &Item, func: &Func, name: &str) {
        let symbol = item.attrs.export_name.clone().unwrap_or_else(|| name.to_string());
        let skip = |this: &mut Self, why: String| {
            this.skipped(item, &format!("the function `{symbol}`"), &why);
        };
        if func.generic || func.is_async || func.variadic {
            return skip(self, "it is generic, async, or variadic".into());
        }
        if !is_burdin_ident(&symbol) {
            return skip(self, "its name is reserved or invalid".into());
        }
        let mut params: Vec<(String, String)> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        for (i, (param, ty)) in func.inputs.iter().enumerate() {
            let Some(burdin) = self.direct_ty(ty) else {
                return skip(
                    self,
                    format!("its parameter `{param}` has type `{}`, which C cannot pass", ty.describe()),
                );
            };
            let name = param_name(param, i, &names);
            names.push(name.clone());
            params.push((name, burdin));
        }
        let ret = match &func.output {
            None => None,
            Some(ty) => match self.direct_ty(ty) {
                Some(burdin) => Some(burdin),
                None => return skip(self, format!("its result has type `{}`, which C cannot return", ty.describe())),
            },
        };
        if !self.names.insert(symbol.clone()) {
            return skip(self, "another item has the name".into());
        }
        api.direct.push(DirectFn { symbol, params, ret });
    }

    /// The Burdin spelling of a type in the signature of a `#[no_mangle] extern "C"` function.
    fn direct_ty(&self, ty: &Ty) -> Option<String> {
        match ty {
            Ty::Prim(name) if name == "bool" => Some("u8".into()),
            Ty::Prim(name) if self.prim_layout(name).is_some() => Some(name.clone()),
            Ty::Ptr { inner, .. } => {
                let pointee = self.pointee(inner).map(|p| p.1).unwrap_or_else(|| "CVoid".to_string());
                Some(format!("CPtr<{pointee}>"))
            }
            Ty::Path { .. } => {
                if let Some(num) = self.c_alias(ty) {
                    return Some(if ty.tail() == Some("c_char") { "CChar".to_string() } else { num.1 });
                }
                None
            }
            _ => None,
        }
    }

    /// A pointee's Rust and Burdin spelling when it is exactly the type behind the pointer. `None` for a type the
    /// shim cannot name on the Burdin side (a type this crate does not expose, a slice, a trait object).
    fn pointee(&self, ty: &Ty) -> Option<(String, String)> {
        match ty {
            Ty::Prim(name) if name == "bool" => Some(("bool".into(), "u8".into())),
            Ty::Prim(name) if self.prim_layout(name).is_some() => Some((name.clone(), name.clone())),
            Ty::Path { .. } => {
                if self.is_external(ty) && ty.tail() == Some("c_void") {
                    return Some(("::core::ffi::c_void".into(), "CVoid".into()));
                }
                if let Some(num) = self.c_alias(ty) {
                    let rust = format!("::core::ffi::{}", ty.tail()?);
                    let burdin = if ty.tail() == Some("c_char") { "CChar".to_string() } else { num.1 };
                    return Some((rust, burdin));
                }
                None
            }
            _ => None,
        }
    }

    /// A method or associated function: the shim's name is `symbol`, and the call is `<Type>::name(..)`.
    #[allow(clippy::too_many_arguments)]
    fn function(
        &mut self,
        api: &mut Api,
        item: &Item,
        func: &Func,
        symbol: &str,
        label: &str,
        self_ty: Option<&Ty>,
        by_id: &HashMap<u32, usize>,
    ) {
        let Some(Ty::Path { id: Some(type_id), .. }) = self_ty else { return };
        let Some(&index) = by_id.get(type_id) else { return };
        let rust_path = format!("{}::{}", api.types[index].rust_path, item.name.clone().unwrap_or_default());
        self.function_at(api, item, func, symbol, label, self_ty, &rust_path, by_id);
    }

    #[allow(clippy::too_many_arguments)]
    fn function_at(
        &mut self,
        api: &mut Api,
        item: &Item,
        func: &Func,
        symbol: &str,
        label: &str,
        self_ty: Option<&Ty>,
        rust_path: &str,
        by_id: &HashMap<u32, usize>,
    ) {
        let skip = |this: &mut Self, why: String| {
            this.skipped(item, &format!("the function `{label}`"), &why);
        };
        if func.generic {
            return skip(self, "it is generic, and C has no generics".into());
        }
        if func.is_async {
            return skip(self, "it is `async`".into());
        }
        if func.variadic {
            return skip(self, "it is variadic".into());
        }
        if !is_burdin_ident(symbol) {
            return skip(self, "its name is reserved or invalid".into());
        }
        let mut lowering = Lowering::default();
        for (i, (name, ty)) in func.inputs.iter().enumerate() {
            let ty = substitute_self(ty, self_ty);
            let name = if name == "self" {
                unique_name("this", &lowering.names)
            } else {
                param_name(name, i, &lowering.names)
            };
            lowering.names.push(name.clone());
            if let Err(why) = self.lower_param(&mut lowering, api, by_id, &name, &ty) {
                return skip(self, format!("its parameter `{name}` {why}"));
            }
        }
        let output = func.output.as_ref().map(|ty| substitute_self(ty, self_ty));
        let call = format!("{rust_path}({})", lowering.args.join(", "));
        let call = if func.is_unsafe { format!("unsafe {{ {call} }}") } else { call };
        let mut string_result = false;
        if let Err(why) = self.lower_result(&mut lowering, api, by_id, output.as_ref(), &call, &mut string_result) {
            return skip(self, format!("its result {why}"));
        }
        if !self.names.insert(symbol.to_string()) {
            return skip(self, "another item has the name".into());
        }
        if string_result {
            api.string_free.get_or_insert_with(|| format!("{}_string_free", snake(&self.krate.name)));
        }
        let Lowering { shim, burdin, prelude, shim_ret, burdin_ret, body, .. } = lowering;
        let mut statements = prelude;
        statements.extend(body);
        api.wrappers.push(Wrapper {
            symbol: symbol.to_string(),
            shim_params: shim,
            shim_ret,
            burdin_params: burdin,
            burdin_ret,
            body: statements,
        });
    }

    /// The shape of one parameter (RI4). `Err` is the end of a sentence that starts with the parameter's name.
    fn lower_param(
        &mut self,
        out: &mut Lowering,
        api: &Api,
        by_id: &HashMap<u32, usize>,
        name: &str,
        ty: &Ty,
    ) -> Result<(), String> {
        let class_of = |ty: &Ty| -> Option<&TypeDef> {
            let Ty::Path { id: Some(id), .. } = ty else { return None };
            by_id.get(id).map(|&i| &api.types[i])
        };
        let unsupported = |ty: &Ty| format!("has type `{}`, which has no C form", ty.describe());
        match ty {
            Ty::Prim(p) if p == "bool" => {
                out.param(name, "u8", "u8");
                out.args.push(format!("{name} != 0"));
            }
            Ty::Prim(p) if self.prim_layout(p).is_some() => {
                out.param(name, p, p);
                out.args.push(name.to_string());
            }
            Ty::Prim(_) => return Err(unsupported(ty)),
            Ty::Ref { mutable: false, inner } if matches!(&**inner, Ty::Prim(p) if p == "str") => {
                self.text_param(out, name, false);
            }
            Ty::Ref { mutable, inner } if matches!(**inner, Ty::Slice(_)) => {
                let Ty::Slice(elem) = &**inner else { unreachable!() };
                let (rust, burdin) = self.element(elem, api, by_id).ok_or_else(|| unsupported(ty))?;
                let (pointer, call) =
                    if *mutable { ("*mut", "from_raw_parts_mut") } else { ("*const", "from_raw_parts") };
                out.param(name, &format!("{pointer} {rust}"), &format!("CPtr<{burdin}>"));
                out.param(&format!("{name}_len"), "usize", "usize");
                out.prelude.push(format!("let {name} = unsafe {{ ::core::slice::{call}({name}, {name}_len) }};"));
                out.args.push(name.to_string());
            }
            Ty::Ref { mutable, inner } => {
                let (rust, burdin) = self.element(inner, api, by_id).ok_or_else(|| unsupported(ty))?;
                let (pointer, access) = if *mutable { ("*mut", "as_mut") } else { ("*const", "as_ref") };
                out.param(name, &format!("{pointer} {rust}"), &format!("CPtr<{burdin}>"));
                out.prelude.push(format!(
                    "let {name} = unsafe {{ {name}.{access}() }}.expect(\"`{name}` must not be null\");"
                ));
                out.args.push(name.to_string());
            }
            Ty::Ptr { mutable, inner } => {
                let (rust, burdin) =
                    self.pointee(inner).or_else(|| self.element(inner, api, by_id)).ok_or_else(|| unsupported(ty))?;
                let pointer = if *mutable { "*mut" } else { "*const" };
                out.param(name, &format!("{pointer} {rust}"), &format!("CPtr<{burdin}>"));
                out.args.push(name.to_string());
            }
            Ty::Path { .. } if ty.tail() == Some("String") && self.is_external(ty) => {
                self.text_param(out, name, true);
            }
            Ty::Path { .. } => {
                if let Some(num) = self.c_alias(ty) {
                    let burdin = if ty.tail() == Some("c_char") { "CChar".to_string() } else { num.1 };
                    out.param(name, &num.0, &burdin);
                    out.args.push(name.to_string());
                    return Ok(());
                }
                let def = class_of(ty).ok_or_else(|| unsupported(ty))?;
                match &def.class {
                    Class::Handle => {
                        out.param(name, &format!("*mut {}", def.rust_path), &format!("CPtr<{}>", def.name));
                        out.prelude.push(format!("assert!(!{name}.is_null(), \"`{name}` must not be null\");"));
                        out.prelude.push(format!("let {name} = *unsafe {{ ::std::boxed::Box::from_raw({name}) }};"));
                        out.args.push(name.to_string());
                    }
                    Class::Value { .. } => {
                        out.param(name, &format!("*const {}", def.rust_path), &format!("CPtr<{}>", def.name));
                        out.prelude.push(format!("assert!(!{name}.is_null(), \"`{name}` must not be null\");"));
                        out.prelude.push(format!("let {name} = unsafe {{ {name}.read() }};"));
                        out.args.push(name.to_string());
                    }
                    Class::Enum { int, variants, .. } => {
                        out.param(name, int.name(), int.name());
                        let arms: Vec<String> =
                            variants.iter().map(|(v, value)| format!("{value} => {}::{v},", def.rust_path)).collect();
                        out.prelude.push(format!(
                            "let {name} = match {name} {{ {} _ => panic!(\"`{name}` is not a `{}` value: {{}}\", {name}), }};",
                            arms.join(" "),
                            def.name
                        ));
                        out.args.push(name.to_string());
                    }
                }
            }
            _ => return Err(unsupported(ty)),
        }
        Ok(())
    }

    /// `&str` and `String` parameters: a pointer and a byte length, checked as UTF-8.
    fn text_param(&mut self, out: &mut Lowering, name: &str, owned: bool) {
        out.param(name, "*const u8", "CPtr<u8>");
        out.param(&format!("{name}_len"), "usize", "usize");
        let own = if owned { ".to_owned()" } else { "" };
        out.prelude.push(format!(
            "let {name} = ::core::str::from_utf8(unsafe {{ ::core::slice::from_raw_parts({name}, {name}_len) }}).expect(\"`{name}` must be valid UTF-8\"){own};"
        ));
        out.args.push(name.to_string());
    }

    /// The type behind a reference or the element of a slice: a number, `bool`, or a type this crate exposes as a
    /// value or a handle (never an enum, whose invalid bit patterns a pointer could not rule out).
    fn element(&self, ty: &Ty, api: &Api, by_id: &HashMap<u32, usize>) -> Option<(String, String)> {
        if let Some(found) = self.pointee(ty) {
            return Some(found);
        }
        let Ty::Path { id: Some(id), .. } = ty else { return None };
        let def = &api.types[*by_id.get(id)?];
        match def.class {
            Class::Enum { .. } => None,
            _ => Some((def.rust_path.clone(), def.name.clone())),
        }
    }

    fn is_external(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Path { id, .. } if id.is_none_or(|id| self.krate.item(id).is_none()))
    }

    /// The shape of the result (RI4).
    fn lower_result(
        &mut self,
        out: &mut Lowering,
        api: &Api,
        by_id: &HashMap<u32, usize>,
        ty: Option<&Ty>,
        call: &str,
        string_result: &mut bool,
    ) -> Result<(), String> {
        let Some(ty) = ty else {
            out.body.push(format!("{call};"));
            return Ok(());
        };
        let unsupported = |ty: &Ty| format!("has type `{}`, which has no C form", ty.describe());
        match ty {
            Ty::Prim(p) if p == "bool" => {
                out.returns("u8", "u8");
                out.body.push(format!("u8::from({call})"));
            }
            Ty::Prim(p) if self.prim_layout(p).is_some() => {
                out.returns(p, p);
                out.body.push(call.to_string());
            }
            Ty::Ref { mutable, inner } | Ty::Ptr { mutable, inner } => {
                if matches!(**inner, Ty::Slice(_)) || matches!(&**inner, Ty::Prim(p) if p == "str") {
                    return Err(
                        "is a borrowed string or slice, which has no single C value; return a `String` or a handle"
                            .to_string(),
                    );
                }
                let (rust, burdin) =
                    self.element(inner, api, by_id).or_else(|| self.pointee(inner)).ok_or_else(|| unsupported(ty))?;
                let pointer = if *mutable { "*mut" } else { "*const" };
                out.returns(&format!("{pointer} {rust}"), &format!("CPtr<{burdin}>"));
                out.body.push(format!("{call} as {pointer} {rust}"));
            }
            Ty::Path { .. } if ty.tail() == Some("String") && self.is_external(ty) => {
                *string_result = true;
                out.returns("*mut ::core::ffi::c_char", "CPtr<CChar>");
                out.body.push(format!(
                    "::std::ffi::CString::new({call}).expect(\"a returned string must not contain NUL\").into_raw()"
                ));
            }
            Ty::Path { .. } => {
                if let Some(num) = self.c_alias(ty) {
                    let burdin = if ty.tail() == Some("c_char") { "CChar".to_string() } else { num.1 };
                    out.returns(&num.0, &burdin);
                    out.body.push(call.to_string());
                    return Ok(());
                }
                let Ty::Path { id: Some(id), .. } = ty else { return Err(unsupported(ty)) };
                let def = &api.types[*by_id.get(id).ok_or_else(|| unsupported(ty))?];
                match &def.class {
                    Class::Handle => {
                        out.returns(&format!("*mut {}", def.rust_path), &format!("CPtr<{}>", def.name));
                        out.body.push(format!("::std::boxed::Box::into_raw(::std::boxed::Box::new({call}))"));
                    }
                    Class::Value { .. } => {
                        let name = unique_name("result", &out.names);
                        out.param(&name, &format!("*mut {}", def.rust_path), &format!("CPtr<{}>", def.name));
                        out.body.push(format!("let value = {call};"));
                        out.body.push(format!("assert!(!{name}.is_null(), \"`{name}` must not be null\");"));
                        out.body.push(format!("unsafe {{ {name}.write(value) }};"));
                    }
                    Class::Enum { int, .. } => {
                        out.returns(int.name(), int.name());
                        out.body.push(format!("{call} as {}", int.name()));
                    }
                }
            }
            _ => return Err(unsupported(ty)),
        }
        Ok(())
    }
}

/// The two signatures and the code between them, built parameter by parameter.
#[derive(Default)]
pub struct Lowering {
    names: Vec<String>,
    shim: Vec<(String, String)>,
    burdin: Vec<(String, String)>,
    prelude: Vec<String>,
    args: Vec<String>,
    shim_ret: Option<String>,
    burdin_ret: Option<String>,
    body: Vec<String>,
}

impl Lowering {
    fn param(&mut self, name: &str, shim: &str, burdin: &str) {
        self.shim.push((name.to_string(), shim.to_string()));
        self.burdin.push((burdin_name(name), burdin.to_string()));
    }

    fn returns(&mut self, shim: &str, burdin: &str) {
        self.shim_ret = Some(shim.to_string());
        self.burdin_ret = Some(burdin.to_string());
    }
}

fn unique_name(base: &str, taken: &[String]) -> String {
    let mut name = base.to_string();
    while taken.iter().any(|t| t == &name) {
        name.push('_');
    }
    name
}

/// A parameter's name: the Rust name when it is an identifier, `argN` when it is a pattern, and never a name that
/// Burdin reserves or that an earlier parameter already took.
fn param_name(name: &str, index: usize, taken: &[String]) -> String {
    let base = if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && name != "_" && !name.is_empty() {
        burdin_name(name)
    } else {
        format!("arg{index}")
    };
    unique_name(&base, taken)
}

fn substitute_self(ty: &Ty, self_ty: Option<&Ty>) -> Ty {
    let Some(replacement) = self_ty else { return ty.clone() };
    match ty {
        Ty::Generic(name) if name == "Self" => replacement.clone(),
        Ty::Ref { mutable, inner } => Ty::Ref { mutable: *mutable, inner: Box::new(substitute_self(inner, self_ty)) },
        Ty::Ptr { mutable, inner } => Ty::Ptr { mutable: *mutable, inner: Box::new(substitute_self(inner, self_ty)) },
        Ty::Slice(inner) => Ty::Slice(Box::new(substitute_self(inner, self_ty))),
        Ty::Array(inner, len) => Ty::Array(Box::new(substitute_self(inner, self_ty)), len.clone()),
        other => other.clone(),
    }
}

fn strip_suffix(text: &str, ty: &str) -> String {
    text.strip_suffix(ty).unwrap_or(text).to_string()
}

/// The text of a Rust string literal with the escapes that mean the same in Burdin; `None` for the rest.
fn rust_string_literal(expr: &str) -> Option<String> {
    let inner = expr.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            _ => return None,
        }
    }
    Some(out)
}

/// A Burdin string literal: braces are doubled because `{` starts an interpolation.
pub fn burdin_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '{' => out.push_str("{{"),
            '}' => out.push_str("}}"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_snake_case() {
        assert_eq!(snake("Counter"), "counter");
        assert_eq!(snake("Vec2"), "vec2");
        assert_eq!(snake("HTTPServer"), "http_server");
        assert_eq!(snake("into_total"), "into_total");
        assert_eq!(snake("Rect"), "rect");
    }

    #[test]
    fn a_reserved_word_gains_an_underscore() {
        assert_eq!(burdin_name("type"), "type_");
        assert_eq!(burdin_name("len"), "len");
    }

    #[test]
    fn string_literals_keep_their_meaning() {
        assert_eq!(rust_string_literal("\"a\\nb\"").as_deref(), Some("a\nb"));
        assert_eq!(rust_string_literal("\"\\u{41}\""), None);
        assert_eq!(burdin_string("a{b}\"c"), "\"a{{b}}\\\"c\"");
    }

    #[test]
    fn parameter_names_are_unique_identifiers() {
        assert_eq!(param_name("type", 0, &[]), "type_");
        assert_eq!(param_name("(a, b)", 2, &[]), "arg2");
        assert_eq!(param_name("x", 1, &["x".to_string()]), "x_");
    }

    #[test]
    fn the_pointer_width_comes_from_the_triple() {
        assert_eq!(pointer_bytes("x86_64-unknown-linux-gnu"), Ok(8));
        assert_eq!(pointer_bytes("i686-pc-windows-msvc"), Ok(4));
        assert!(pointer_bytes("mystery-none").is_err());
    }
}
