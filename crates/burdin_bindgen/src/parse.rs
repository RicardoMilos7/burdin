//! Walks a header's declarations through libclang and fills a [`Module`] (`spec/bindgen.md`, BG2-BG9).
//!
//! Only the header named on the command line is generated. A type that header uses from an included file (a typedef
//! from `<stdint.h>`, a `struct` from another header) is collapsed to what it is made of, or declared on demand when
//! it has an identity of its own: a struct reached through a pointer is declared without fields, because the caller
//! only passes it around, and one used by value is declared in full (BG3).

use std::collections::{HashMap, HashSet};

use crate::clang::{Cursor, CursorKind, Evaluated, Storage, TokenKind, TranslationUnit, Ty as CTy, TypeKind};
use crate::macros::{self, Config, IntTy, Lookup, Value};
use crate::model::{Field, Function, Item, Layout, Module, Param, Record, Ty, word_for_alignment};

/// The words Burdin reserves. A C name that would need one is renamed when it does not matter to C (a field or a
/// parameter) and skipped when it does (a function, type, or constant).
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

fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !KEYWORDS.contains(&name)
}

/// A field or parameter name that is a Burdin identifier: a reserved word gets a trailing underscore.
fn safe_name(name: &str) -> String {
    if KEYWORDS.contains(&name) { format!("{name}_") } else { name.to_string() }
}

/// What the generator was asked to do.
pub struct Settings {
    /// Generate wrappers for `static inline` functions (BG8). When false they are skipped with a note.
    pub wrappers: bool,
    /// The target's `long` width in bits.
    pub long_bits: u8,
}

/// The output of one run: the Burdin module, the C text of the wrappers, and the notes as `file:line: message`.
pub struct Generated {
    pub module: Module,
    pub wrapper_source: Vec<String>,
}

struct Gen<'a> {
    unit: &'a TranslationUnit,
    settings: Settings,
    module: Module,
    wrapper_source: Vec<String>,
    /// Names the module uses, so a second declaration of the same name is skipped.
    names: HashSet<String>,
    /// Function names of the header: a struct tag that equals one is renamed, because C keeps them apart.
    function_names: HashSet<String>,
    /// A typedef name given to an otherwise anonymous struct, union, or enum, or a synthetic name for a nested one.
    anon_names: HashMap<String, String>,
    /// Records already declared or being declared, by definition key.
    records: HashMap<String, String>,
    /// Typedefs already declared or being declared, by key.
    typedefs: HashMap<String, String>,
    enums: HashMap<String, Option<String>>,
    /// Burdin names of the structs, unions, and aliases of them, to find a by-value aggregate in a signature.
    record_names: HashSet<String>,
    alias_targets: HashMap<String, Ty>,
    /// Constants known so far (enumerators and evaluated macros), for later macros.
    constants: HashMap<String, Value>,
    macros: HashMap<String, Vec<crate::clang::Token>>,
    function_pointers: u32,
    function_like_macros: u32,
}

/// Generates the module for the translation unit.
pub fn generate(unit: &TranslationUnit, header: &str, settings: Settings) -> Generated {
    let module = Module { header: header.to_string(), ..Module::default() };
    let mut generator = Gen {
        unit,
        settings,
        module,
        wrapper_source: Vec::new(),
        names: HashSet::new(),
        function_names: HashSet::new(),
        anon_names: HashMap::new(),
        records: HashMap::new(),
        typedefs: HashMap::new(),
        enums: HashMap::new(),
        record_names: HashSet::new(),
        alias_targets: HashMap::new(),
        constants: HashMap::new(),
        macros: HashMap::new(),
        function_pointers: 0,
        function_like_macros: 0,
    };
    generator.run();
    Generated { module: generator.module, wrapper_source: generator.wrapper_source }
}

fn key(cursor: &Cursor) -> String {
    let location = cursor.location();
    format!("{}:{}", location.file, location.offset)
}

fn is_unnamed(spelling: &str) -> bool {
    spelling.is_empty() || spelling.contains("(unnamed") || spelling.contains("(anonymous")
}

/// The Burdin type of a C enumeration: the smallest of `i32`, `u32`, `i64`, and `u64` that holds every
/// enumerator. C leaves the underlying type to the implementation and only its size has to agree with the layout,
/// and the Itanium ABI picks an unsigned type where the MSVC ABI picks a signed one for the same values. Choosing
/// from the values makes the generated module the same on every platform (BG5).
fn enum_primitive(definition: &Cursor<'_>) -> &'static str {
    let unsigned = enum_unsigned(definition);
    let mut lowest: i128 = 0;
    let mut highest: i128 = 0;
    for constant in definition.children().into_iter().filter(|cursor| cursor.kind() == CursorKind::EnumConstant) {
        let value = constant.enum_value(unsigned);
        lowest = lowest.min(value);
        highest = highest.max(value);
    }
    if lowest < 0 {
        if lowest >= i32::MIN as i128 && highest <= i32::MAX as i128 { "i32" } else { "i64" }
    } else if highest <= i32::MAX as i128 {
        "i32"
    } else if highest <= u32::MAX as i128 {
        "u32"
    } else if highest <= i64::MAX as i128 {
        "i64"
    } else {
        "u64"
    }
}

/// Whether C gave an enumeration an unsigned type, which decides how an enumerator that does not fit in `int` is
/// read.
fn enum_unsigned(definition: &Cursor<'_>) -> bool {
    matches!(
        definition.enum_integer_type().kind(),
        TypeKind::UInt | TypeKind::ULong | TypeKind::ULongLong | TypeKind::UShort | TypeKind::UChar
    )
}

/// The type a pointer or typedef names, with sugar removed.
fn strip(ty: CTy<'_>) -> CTy<'_> {
    let mut ty = ty;
    while ty.kind() == TypeKind::Elaborated {
        ty = ty.named();
    }
    ty
}

impl<'a> Gen<'a> {
    fn run(&mut self) {
        let children = self.unit.root().children();
        // Names that decide a conflict, and the typedefs that name an anonymous aggregate.
        for child in &children {
            match child.kind() {
                CursorKind::Function if child.location().in_main_file => {
                    self.function_names.insert(child.spelling());
                }
                CursorKind::Typedef => {
                    let underlying = strip(child.typedef_underlying());
                    if matches!(underlying.kind(), TypeKind::Record | TypeKind::Enum) {
                        let declaration = underlying.declaration();
                        let declaration = declaration.definition().unwrap_or(declaration);
                        if is_unnamed(&declaration.spelling()) {
                            self.anon_names.entry(key(&declaration)).or_insert_with(|| child.spelling());
                        }
                    }
                }
                CursorKind::Enum => {
                    // Every enumerator is known before any macro is evaluated, since a header may put the macro
                    // first and the enumeration after it.
                    let definition = child.definition().unwrap_or(*child);
                    let unsigned = enum_unsigned(&definition);
                    for constant in
                        definition.children().into_iter().filter(|cursor| cursor.kind() == CursorKind::EnumConstant)
                    {
                        let value = constant.enum_value(unsigned);
                        self.constants.insert(constant.spelling(), Value::Int(value, IntTy::INT));
                    }
                }
                CursorKind::MacroDefinition if !child.is_function_like_macro() && !child.is_builtin_macro() => {
                    let mut tokens = child.tokens();
                    if !tokens.is_empty() {
                        let name = tokens.remove(0).text;
                        self.macros.entry(name).or_insert(tokens);
                    }
                }
                _ => {}
            }
        }
        for child in &children {
            let location = child.location();
            if !location.in_main_file {
                continue;
            }
            match child.kind() {
                CursorKind::Function => self.function(child),
                CursorKind::Typedef => {
                    if let Err(reason) = self.typedef(child) {
                        self.skip(child, &format!("typedef `{}`", child.spelling()), &reason);
                    }
                }
                CursorKind::Struct | CursorKind::Union => {
                    let definition = child.definition().unwrap_or(*child);
                    if let Err(reason) = self.record(&definition, None) {
                        self.skip(child, &format!("`{}`", child.spelling()), &reason);
                    }
                }
                CursorKind::Enum => {
                    let definition = child.definition().unwrap_or(*child);
                    self.enumeration(&definition);
                }
                CursorKind::Variable => self.variable(child),
                CursorKind::MacroDefinition => self.macro_constant(child),
                _ => {}
            }
        }
        if self.function_like_macros > 0 {
            let count = self.function_like_macros;
            let plural = if count == 1 { "" } else { "s" };
            self.module.notes.push(format!(
                "{count} function-like macro{plural} {} skipped: a macro that takes arguments is not a constant",
                if count == 1 { "was" } else { "were" }
            ));
        }
    }

    fn skip(&mut self, at: &Cursor, what: &str, reason: &str) {
        let location = at.location();
        self.module.notes.push(format!(
            "{what} at {}:{} was skipped: {reason}",
            base_name(&location.file),
            location.line
        ));
    }

    /// Claims a Burdin name; false when it is taken.
    fn claim(&mut self, name: &str) -> bool {
        self.names.insert(name.to_string())
    }

    // --- Types -------------------------------------------------------------------------------------------------

    /// The Burdin type of a C type, or the reason it has none.
    fn convert(&mut self, ty: CTy<'a>, hint: &str, behind_pointer: bool) -> Result<Ty, String> {
        match ty.kind() {
            TypeKind::Elaborated => self.convert(ty.named(), hint, behind_pointer),
            TypeKind::Typedef => {
                let declaration = ty.declaration();
                let name = declaration.spelling();
                if declaration.location().in_main_file {
                    // A typedef of the header being read keeps its name, which is what a person reading the C
                    // header expects to find in the bindings.
                    return self.typedef(&declaration);
                }
                // A typedef from an included header is only what it stands for.
                if let Some(primitive) = fixed_typedef(&name) {
                    return Ok(Ty::Prim(primitive));
                }
                self.convert(declaration.typedef_underlying(), hint, behind_pointer)
            }
            TypeKind::Void => Ok(Ty::Void),
            TypeKind::Bool => Ok(Ty::Prim("u8")),
            TypeKind::CharU | TypeKind::CharS => Ok(Ty::Prim("CChar")),
            TypeKind::SChar => Ok(Ty::Prim("i8")),
            TypeKind::UChar => Ok(Ty::Prim("u8")),
            TypeKind::Short => Ok(Ty::Prim("i16")),
            TypeKind::UShort => Ok(Ty::Prim("u16")),
            TypeKind::Int => Ok(Ty::Prim("i32")),
            TypeKind::UInt => Ok(Ty::Prim("u32")),
            TypeKind::Long => Ok(Ty::Prim(if ty.size() == Some(4) { "i32" } else { "i64" })),
            TypeKind::ULong => Ok(Ty::Prim(if ty.size() == Some(4) { "u32" } else { "u64" })),
            TypeKind::LongLong => Ok(Ty::Prim("i64")),
            TypeKind::ULongLong => Ok(Ty::Prim("u64")),
            TypeKind::WChar => Ok(Ty::Prim(if ty.size() == Some(2) { "u16" } else { "i32" })),
            TypeKind::Float => Ok(Ty::Prim("f32")),
            TypeKind::Double => Ok(Ty::Prim("f64")),
            TypeKind::Pointer => {
                let pointee = strip(ty.pointee());
                let canonical = pointee.canonical();
                if matches!(canonical.kind(), TypeKind::FunctionProto | TypeKind::FunctionNoProto) {
                    self.function_pointers += 1;
                    return Ok(Ty::FnPtr);
                }
                let inner = self.convert(pointee, hint, true)?;
                Ok(Ty::Ptr(Box::new(inner)))
            }
            TypeKind::Record => {
                let declaration = ty.declaration();
                let definition = declaration.definition().unwrap_or(declaration);
                self.record_reference(&definition, hint, behind_pointer)
            }
            TypeKind::Enum => {
                let declaration = ty.declaration();
                let definition = declaration.definition().unwrap_or(declaration);
                if definition.location().in_main_file
                    && let Some(Some(name)) = self.enumeration(&definition)
                {
                    return Ok(Ty::Named(name));
                }
                Ok(Ty::Prim(enum_primitive(&definition)))
            }
            TypeKind::ConstantArray => {
                let element = self.convert(ty.array_element(), hint, false)?;
                let len = ty.array_len().ok_or_else(|| "an array of unknown length".to_string())?;
                Ok(Ty::Array(Box::new(element), len))
            }
            TypeKind::IncompleteArray => Err("an array of unknown length".to_string()),
            TypeKind::FunctionProto | TypeKind::FunctionNoProto => Err("a function type used as a value".to_string()),
            _ => {
                // Attributed, decayed, and other sugar libclang does not name: look at what it stands for.
                let canonical = ty.canonical();
                if canonical.kind() == ty.kind() {
                    Err(format!("the type `{}` has no Burdin equivalent", ty.spelling()))
                } else {
                    self.convert(canonical, hint, behind_pointer)
                }
            }
        }
    }

    /// A reference to a struct or union: a name, declared first if the header being read does not declare it.
    fn record_reference(&mut self, definition: &Cursor<'a>, hint: &str, behind_pointer: bool) -> Result<Ty, String> {
        if let Some(name) = self.records.get(&key(definition)) {
            return Ok(Ty::Named(name.clone()));
        }
        let in_main = definition.location().in_main_file;
        // An included struct that is only passed by pointer is declared without fields.
        let opaque = !in_main && behind_pointer && !self.records.contains_key(&key(definition));
        if opaque {
            return self.opaque_record(definition);
        }
        self.record(definition, Some(hint))
    }

    /// A struct declared without fields, for one that only appears behind a pointer.
    fn opaque_record(&mut self, definition: &Cursor<'a>) -> Result<Ty, String> {
        let name = self.record_name(definition, None).ok_or_else(|| "an unnamed struct".to_string())?;
        let name = self.unique_type_name(&name);
        self.records.insert(key(definition), name.clone());
        self.record_names.insert(name.clone());
        self.module.items.push(Item::Record(Record {
            name: name.clone(),
            is_union: definition.kind() == CursorKind::Union,
            fields: Vec::new(),
            members: Vec::new(),
            size: 0,
            align: 1,
            layout: Layout::Opaque {
                reason: Some("it is declared in another header and only used through pointers".into()),
            },
        }));
        Ok(Ty::Named(name))
    }

    fn record_name(&self, definition: &Cursor<'a>, hint: Option<&str>) -> Option<String> {
        let spelling = definition.spelling();
        if !is_unnamed(&spelling) {
            return Some(spelling);
        }
        self.anon_names.get(&key(definition)).cloned().or_else(|| hint.filter(|h| !h.is_empty()).map(str::to_string))
    }

    /// A type name no other declaration of the module has taken. A struct tag that equals a function's name is
    /// renamed, because C keeps tags and functions in separate namespaces and Burdin does not.
    fn unique_type_name(&mut self, name: &str) -> String {
        let mut candidate = name.to_string();
        while self.names.contains(&candidate) || (self.function_names.contains(&candidate) && candidate == name) {
            candidate.push('_');
        }
        self.names.insert(candidate.clone());
        candidate
    }

    // --- Records -----------------------------------------------------------------------------------------------

    /// Declares a struct or union and returns its name.
    fn record(&mut self, definition: &Cursor<'a>, hint: Option<&str>) -> Result<Ty, String> {
        if let Some(name) = self.records.get(&key(definition)) {
            return Ok(Ty::Named(name.clone()));
        }
        let is_union = definition.kind() == CursorKind::Union;
        let name = self.record_name(definition, hint).ok_or_else(|| "an unnamed struct or union".to_string())?;
        if !is_name(&name) {
            return Err(format!("`{name}` is not a name Burdin can declare"));
        }
        let name = self.unique_type_name(&name);
        self.records.insert(key(definition), name.clone());
        self.record_names.insert(name.clone());
        if !definition.is_definition() {
            self.module.items.push(Item::Record(Record {
                name: name.clone(),
                is_union,
                fields: Vec::new(),
                members: Vec::new(),
                size: 0,
                align: 1,
                layout: Layout::Opaque { reason: None },
            }));
            return Ok(Ty::Named(name));
        }
        let ty = definition.ty();
        let (size, align) = (ty.size().unwrap_or(0), ty.align().unwrap_or(1));
        let mut fields: Vec<Field> = Vec::new();
        let mut members: Vec<String> = Vec::new();
        // Why the fields cannot be declared one by one; the first reason found.
        let mut reason: Option<String> = None;
        let mut natural_end: u64 = 0;
        let mut natural_align: u64 = 1;
        for field in definition.children().into_iter().filter(|c| c.kind() == CursorKind::Field) {
            let field_name = field.spelling();
            let field_ty = strip(field.ty());
            let field_hint = format!("{name}_{field_name}");
            if !field_name.is_empty() {
                // A nested anonymous struct or union gets a name made from its parent and the field.
                self.name_nested(&field_ty, &field_hint);
            }
            if field.is_bit_field() {
                reason.get_or_insert_with(|| format!("`{field_name}` is a bit-field"));
                continue;
            }
            if field_name.is_empty() {
                reason.get_or_insert_with(|| "it has an anonymous member".to_string());
                continue;
            }
            if field_ty.canonical().kind() == TypeKind::IncompleteArray {
                reason.get_or_insert_with(|| format!("`{field_name}` is a flexible array member"));
                continue;
            }
            let pointers = self.function_pointers;
            let converted = match self.convert(field_ty, &field_hint, false) {
                Ok(converted) => converted,
                Err(why) => {
                    reason.get_or_insert_with(|| format!("`{field_name}` cannot be represented: {why}"));
                    continue;
                }
            };
            if self.function_pointers > pointers {
                self.module.notes.push(format!(
                    "the function pointer field `{name}.{field_name}` is declared as `CPtr<CVoid>`, because Burdin has no function-pointer type across the C ABI"
                ));
            }
            match (field.field_offset_bits(), field_ty.size(), field_ty.align()) {
                (Some(bits), Some(field_size), Some(field_align)) if bits % 8 == 0 => {
                    let offset = bits / 8;
                    members.push(format!("`{field_name}`: {} at {offset}", converted.burdin()));
                    let expected = natural_end.next_multiple_of(field_align.max(1));
                    if !is_union && offset != expected {
                        reason.get_or_insert_with(|| {
                            format!("`{field_name}` is at offset {offset} in C, not {expected}")
                        });
                    }
                    natural_end = expected + field_size;
                    natural_align = natural_align.max(field_align);
                }
                _ => {
                    reason.get_or_insert_with(|| format!("`{field_name}` has no fixed offset"));
                }
            }
            fields.push(Field { name: safe_name(&field_name), ty: converted });
        }
        // An anonymous struct or union member has no field to name it; its members belong to this struct.
        for member in definition.children() {
            if matches!(member.kind(), CursorKind::Struct | CursorKind::Union) {
                let nested = member.definition().unwrap_or(member);
                if is_unnamed(&nested.spelling()) && !self.anon_names.contains_key(&key(&nested)) {
                    reason.get_or_insert_with(|| "it has an anonymous struct or union member".to_string());
                }
            }
        }
        let natural_size = natural_end.next_multiple_of(natural_align);
        let layout = if is_union {
            Layout::Bytes { reason: None }
        } else if let Some(reason) = reason {
            Layout::Bytes { reason: Some(reason) }
        } else if natural_size != size {
            Layout::Bytes { reason: Some(format!("its size is {size} bytes in C, not {natural_size}")) }
        } else if natural_align != align {
            Layout::Bytes { reason: Some(format!("its alignment is {align} bytes in C, not {natural_align}")) }
        } else {
            Layout::Natural
        };
        let (layout, fields) = match layout {
            // A blob can only carry an alignment Burdin has an integer for.
            Layout::Bytes { .. } if word_for_alignment(align).is_none() || size % align.max(1) != 0 => {
                self.module.notes.push(format!(
                    "the layout of `{name}` (size {size}, alignment {align}) cannot be reproduced, so it is declared without fields"
                ));
                let why = format!("its alignment of {align} bytes has no Burdin integer");
                (Layout::Opaque { reason: Some(why) }, Vec::new())
            }
            Layout::Bytes { reason } => {
                if let Some(why) = &reason {
                    self.module.notes.push(format!(
                        "`{name}` is declared as bytes with C's size and alignment, not field by field: {why}"
                    ));
                }
                (Layout::Bytes { reason }, Vec::new())
            }
            other => (other, fields),
        };
        self.module.items.push(Item::Record(Record {
            name: name.clone(),
            is_union,
            fields,
            members,
            size,
            align,
            layout,
        }));
        Ok(Ty::Named(name))
    }

    /// Gives a nested anonymous struct or union a name, before the field that uses it is converted.
    fn name_nested(&mut self, field_ty: &CTy<'a>, hint: &str) {
        let inner = strip(*field_ty);
        if inner.kind() != TypeKind::Record {
            return;
        }
        let declaration = inner.declaration();
        let declaration = declaration.definition().unwrap_or(declaration);
        if is_unnamed(&declaration.spelling()) {
            self.anon_names.entry(key(&declaration)).or_insert_with(|| hint.to_string());
        }
    }

    // --- Typedefs ----------------------------------------------------------------------------------------------

    /// Declares a typedef of the header and returns the type that refers to it.
    fn typedef(&mut self, declaration: &Cursor<'a>) -> Result<Ty, String> {
        let name = declaration.spelling();
        let declaration_key = key(declaration);
        if let Some(existing) = self.typedefs.get(&declaration_key) {
            return Ok(Ty::Named(existing.clone()));
        }
        if !is_name(&name) {
            return Err(format!("`{name}` is not a name Burdin can declare"));
        }
        let underlying = declaration.typedef_underlying();
        // The typedef that names a struct, union, or enumeration (`typedef struct Foo Foo;` or
        // `typedef struct { ... } Foo;`) declares that type under the name and needs no alias of its own.
        let stripped = strip(underlying);
        if matches!(stripped.kind(), TypeKind::Record | TypeKind::Enum) {
            let target = stripped.declaration();
            let target = target.definition().unwrap_or(target);
            let target_name = target.spelling();
            let names_it = if is_unnamed(&target_name) {
                self.anon_names.get(&key(&target)) == Some(&name)
            } else {
                target_name == name
            };
            if names_it {
                // Recorded before the target is declared, so a pointer back to this name finds it.
                self.typedefs.insert(declaration_key.clone(), name.clone());
                let declared = if stripped.kind() == TypeKind::Record {
                    self.record(&target, None)
                } else {
                    match self.enumeration(&target) {
                        Some(Some(enum_name)) => Ok(Ty::Named(enum_name)),
                        _ => Ok(Ty::Prim(enum_primitive(&target))),
                    }
                };
                match declared {
                    Ok(ty) if ty == Ty::Named(name.clone()) => return Ok(ty),
                    Ok(ty) => {
                        self.typedefs.remove(&declaration_key);
                        return self.alias(declaration, &name, ty, None);
                    }
                    Err(reason) => {
                        self.typedefs.remove(&declaration_key);
                        return Err(reason);
                    }
                }
            }
        }
        self.typedefs.insert(declaration_key.clone(), name.clone());
        let pointers = self.function_pointers;
        let converted = self.convert(underlying, &name, false);
        self.typedefs.remove(&declaration_key);
        let ty = converted?;
        if ty == Ty::Void {
            return Err("a typedef of `void` has no Burdin equivalent".to_string());
        }
        let note = if ty == Ty::FnPtr && self.function_pointers > pointers {
            self.module.notes.push(format!("the function pointer type `{name}` is declared as `CPtr<CVoid>`"));
            Some(format!(
                "`{name}` is a C function pointer type; Burdin has none across the C ABI, so it is a `CPtr<CVoid>`"
            ))
        } else {
            None
        };
        // The portable names are their primitive wherever they are declared.
        let ty = fixed_typedef(&name).map_or(ty, Ty::Prim);
        self.alias(declaration, &name, ty, note)
    }

    fn alias(&mut self, declaration: &Cursor<'a>, name: &str, ty: Ty, note: Option<String>) -> Result<Ty, String> {
        if ty == Ty::Named(name.to_string()) || self.alias_targets.contains_key(name) {
            return Ok(Ty::Named(name.to_string()));
        }
        if !self.claim(name) {
            return Err(format!("`{name}` is already declared"));
        }
        self.typedefs.insert(key(declaration), name.to_string());
        self.alias_targets.insert(name.to_string(), ty.clone());
        self.module.items.push(Item::Alias { name: name.to_string(), ty, note });
        Ok(Ty::Named(name.to_string()))
    }

    // --- Enumerations ------------------------------------------------------------------------------------------

    /// Declares an enumeration: a type alias for its integer type, when it is named, and one constant per enumerator.
    /// `None` when the cursor is only a forward declaration, `Some(None)` for an enumeration with no usable name
    /// (its enumerators are still constants), and `Some(Some(name))` for a named one.
    fn enumeration(&mut self, definition: &Cursor<'a>) -> Option<Option<String>> {
        if let Some(done) = self.enums.get(&key(definition)) {
            return Some(done.clone());
        }
        if !definition.is_definition() {
            return None;
        }
        let spelling = definition.spelling();
        let name = if is_unnamed(&spelling) { self.anon_names.get(&key(definition)).cloned() } else { Some(spelling) };
        let enumerators: Vec<Cursor<'a>> =
            definition.children().into_iter().filter(|c| c.kind() == CursorKind::EnumConstant).collect();
        let primitive = enum_primitive(definition);
        let unsigned = primitive.starts_with('u');
        let name = name.filter(|n| is_name(n));
        if let Some(name) = &name
            && !self.claim(name)
        {
            self.module.notes.push(format!("the enumeration `{name}` was skipped: the name is already declared"));
            self.enums.insert(key(definition), None);
            return Some(None);
        }
        self.enums.insert(key(definition), name.clone());
        if let Some(name) = &name {
            self.alias_targets.insert(name.clone(), Ty::Prim(primitive));
            self.module.items.push(Item::Alias { name: name.clone(), ty: Ty::Prim(primitive), note: None });
        }
        for constant in enumerators {
            let constant_name = constant.spelling();
            let value = constant.enum_value(unsigned);
            if !is_name(&constant_name) {
                self.skip(&constant, &format!("the enumerator `{constant_name}`"), "its name is reserved or invalid");
                continue;
            }
            if !self.claim(&constant_name) {
                continue;
            }
            self.constants.insert(constant_name.clone(), Value::Int(value, IntTy::INT));
            self.module.items.push(Item::Const {
                name: constant_name,
                ty: primitive.to_string(),
                value: int_text(value, primitive),
            });
        }
        Some(name)
    }

    // --- Variables ---------------------------------------------------------------------------------------------

    fn variable(&mut self, declaration: &Cursor<'a>) {
        let name = declaration.spelling();
        let ty = declaration.ty();
        let is_const = ty.spelling().starts_with("const ") || ty.spelling().contains(" const");
        let canonical = ty.canonical();
        if is_const
            && is_name(&name)
            && matches!(
                canonical.kind(),
                TypeKind::Int
                    | TypeKind::UInt
                    | TypeKind::Long
                    | TypeKind::ULong
                    | TypeKind::LongLong
                    | TypeKind::ULongLong
                    | TypeKind::Short
                    | TypeKind::UShort
                    | TypeKind::Double
                    | TypeKind::Float
                    | TypeKind::CharS
                    | TypeKind::CharU
                    | TypeKind::SChar
                    | TypeKind::UChar
            )
            && let Some(value) = declaration.evaluate()
            && let Ok(Ty::Prim(primitive)) = self.convert(canonical, "", false)
        {
            if !self.claim(&name) {
                return;
            }
            let text = match value {
                Evaluated::Int(value) => int_text(value, primitive),
                Evaluated::Float(value) => macros::burdin_float(value),
            };
            self.module.items.push(Item::Const { name, ty: primitive.to_string(), value: text });
            return;
        }
        self.skip(
            declaration,
            &format!("the variable `{name}`"),
            "the C bridge declares functions only, not global variables",
        );
    }

    // --- Macros ------------------------------------------------------------------------------------------------

    fn macro_constant(&mut self, definition: &Cursor<'a>) {
        if definition.is_builtin_macro() {
            return;
        }
        let mut tokens = definition.tokens();
        if tokens.is_empty() {
            return;
        }
        let name = tokens.remove(0).text;
        if definition.is_function_like_macro() {
            self.function_like_macros += 1;
            return;
        }
        if tokens.is_empty() {
            return;
        }
        let config = Config { long_bits: self.settings.long_bits };
        let constants = self.constants.clone();
        let macros = self.macros.clone();
        let mut lookup = |ident: &str| -> Lookup {
            if let Some(value) = constants.get(ident) {
                Lookup::Value(value.clone())
            } else if ident != name
                && let Some(tokens) = macros.get(ident)
            {
                Lookup::Tokens(tokens.clone())
            } else {
                Lookup::Unknown
            }
        };
        let Some(value) = macros::evaluate(&tokens, config, &mut lookup) else {
            if tokens.iter().any(|t| t.kind == TokenKind::Literal) {
                self.skip(
                    definition,
                    &format!("the macro `{name}`"),
                    "its value is not a constant expression Burdin can evaluate",
                );
            }
            return;
        };
        if !is_name(&name) {
            self.skip(definition, &format!("the macro `{name}`"), "its name is reserved or invalid");
            return;
        }
        if !self.claim(&name) {
            return;
        }
        let (ty, text) = match &value {
            Value::Int(v, int_ty) => (int_ty.name().to_string(), int_text(*v, int_ty.name())),
            Value::Float(v, single) => ((if *single { "f32" } else { "f64" }).to_string(), macros::burdin_float(*v)),
            Value::Str(text) => match macros::burdin_string(text) {
                Some(literal) => ("str".to_string(), literal),
                None => {
                    self.names.remove(&name);
                    self.skip(
                        definition,
                        &format!("the macro `{name}`"),
                        "its string has a character Burdin cannot spell",
                    );
                    return;
                }
            },
        };
        self.constants.insert(name.clone(), value);
        self.module.items.push(Item::Const { name, ty, value: text });
    }

    // --- Functions ---------------------------------------------------------------------------------------------

    fn function(&mut self, declaration: &Cursor<'a>) {
        let name = declaration.spelling();
        if !is_name(&name) {
            self.skip(declaration, &format!("the function `{name}`"), "its name is reserved or invalid");
            return;
        }
        if self.names.contains(&name) {
            // A second prototype of the same function.
            return;
        }
        let function_ty = declaration.ty();
        let hint = String::new();
        let argument_types = function_ty.arg_types();
        let arguments = declaration.arguments();
        let mut params: Vec<Param> = Vec::new();
        let before = self.function_pointers;
        for (index, argument_ty) in argument_types.iter().enumerate() {
            // A parameter declared as an array is a pointer to its element, which is how it is passed.
            let canonical = argument_ty.canonical();
            let converted = match if matches!(canonical.kind(), TypeKind::IncompleteArray | TypeKind::ConstantArray) {
                self.convert(canonical.array_element(), &hint, false).map(|element| Ty::Ptr(Box::new(element)))
            } else {
                self.convert(*argument_ty, &hint, false)
            } {
                Ok(ty) => ty,
                Err(reason) => {
                    self.skip(
                        declaration,
                        &format!("the function `{name}`"),
                        &format!("parameter {}: {reason}", index + 1),
                    );
                    return;
                }
            };
            if self.is_aggregate(&converted) {
                self.skip(
                    declaration,
                    &format!("the function `{name}`"),
                    &format!(
                        "parameter {} is a struct or union passed by value, which the C bridge cannot call",
                        index + 1
                    ),
                );
                return;
            }
            let given = arguments.get(index).map(|argument| argument.spelling()).unwrap_or_default();
            let param_name = if given.is_empty() { format!("arg{index}") } else { safe_name(&given) };
            params.push(Param { name: param_name, ty: converted });
        }
        let result = match self.convert(declaration.result_type(), &hint, false) {
            Ok(ty) => ty,
            Err(reason) => {
                self.skip(declaration, &format!("the function `{name}`"), &format!("result: {reason}"));
                return;
            }
        };
        if self.is_aggregate(&result) {
            self.skip(
                declaration,
                &format!("the function `{name}`"),
                "it returns a struct or union by value, which the C bridge cannot call",
            );
            return;
        }
        let variadic = function_ty.is_variadic() || declaration.is_variadic();
        if self.function_pointers > before {
            self.module.notes.push(format!(
                "a function pointer in `{name}` is passed as `CPtr<CVoid>`, because Burdin has no function-pointer type across the C ABI"
            ));
        }
        let internal =
            declaration.storage() == Storage::Static || (declaration.is_inline() && declaration.is_definition());
        if internal && !declaration.is_definition() {
            self.skip(
                declaration,
                &format!("the static function `{name}`"),
                "it is only declared here, so there is no definition to call",
            );
            return;
        }
        if internal {
            self.wrapper(declaration, &name, params, result, variadic, &argument_types);
            return;
        }
        if !self.claim(&name) {
            return;
        }
        self.module.functions.push(Function { name, ret: result, params, variadic, wrapper_of: None });
    }

    /// A `static inline` function has no symbol to link against, so a C file calls it and exports the call under
    /// `<name>__extern` (BG8).
    fn wrapper(
        &mut self,
        declaration: &Cursor<'a>,
        name: &str,
        params: Vec<Param>,
        result: Ty,
        variadic: bool,
        argument_types: &[CTy<'a>],
    ) {
        if !self.settings.wrappers {
            self.skip(
                declaration,
                &format!("the static function `{name}`"),
                "it has no symbol; pass `--wrappers <file.c>` to generate a C wrapper for it",
            );
            return;
        }
        if variadic {
            self.skip(declaration, &format!("the static function `{name}`"), "a variadic function cannot be wrapped");
            return;
        }
        let wrapper_name = format!("{name}__extern");
        if !self.claim(&wrapper_name) {
            return;
        }
        let mut c_params: Vec<String> = Vec::new();
        let mut c_args: Vec<String> = Vec::new();
        for (index, (param, argument_ty)) in params.iter().zip(argument_types).enumerate() {
            let local = format!("arg{index}");
            if param.ty == Ty::FnPtr {
                // A function pointer cannot be written as `type name` without its declarator, so the wrapper takes
                // a `void *` and casts it back.
                c_params.push(format!("void *{local}"));
                c_args.push(format!("({}){local}", argument_ty.spelling()));
            } else {
                c_params.push(format!("{} {local}", argument_ty.spelling()));
                c_args.push(local);
            }
        }
        let result_spelling = declaration.result_type().spelling();
        if result == Ty::FnPtr {
            self.names.remove(&wrapper_name);
            self.skip(declaration, &format!("the static function `{name}`"), "it returns a function pointer");
            return;
        }
        let parameters = if c_params.is_empty() { "void".to_string() } else { c_params.join(", ") };
        let call = format!("{name}({})", c_args.join(", "));
        let body = if result == Ty::Void { format!("{call};") } else { format!("return {call};") };
        self.wrapper_source.push(format!("{result_spelling} {wrapper_name}({parameters}) {{\n    {body}\n}}\n"));
        self.module.functions.push(Function {
            name: wrapper_name,
            ret: result,
            params,
            variadic: false,
            wrapper_of: Some(name.to_string()),
        });
    }

    /// Whether a type is a struct or a union, directly or through aliases: those cannot cross the C ABI by value.
    fn is_aggregate(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Named(name) => {
                self.record_names.contains(name)
                    || self.alias_targets.get(name).is_some_and(|target| self.is_aggregate(target))
            }
            _ => false,
        }
    }
}

/// The last component of a path, so a note does not depend on where the header lives.
fn base_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The portable fixed-size and pointer-sized typedef names, which are their primitive in every header.
fn fixed_typedef(name: &str) -> Option<&'static str> {
    Some(match name {
        "size_t" | "uintptr_t" => "usize",
        "ssize_t" | "ptrdiff_t" | "intptr_t" => "isize",
        "int8_t" => "i8",
        "uint8_t" => "u8",
        "int16_t" => "i16",
        "uint16_t" => "u16",
        "int32_t" => "i32",
        "uint32_t" => "u32",
        "int64_t" => "i64",
        "uint64_t" => "u64",
        _ => return None,
    })
}

/// An integer as a Burdin literal of the given type. The most negative `i64` has no literal of its own.
fn int_text(value: i128, primitive: &str) -> String {
    if primitive == "i64" && value == i64::MIN as i128 {
        return "-9223372036854775807 - 1".to_string();
    }
    value.to_string()
}
