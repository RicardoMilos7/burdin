//! Derives: reading the built-in names of a declaration's `: A, B` list and checking their requirements
//! (memory-rules.md, CP). The other names in the list (user derives and interfaces with default methods) are
//! applied by `Sema::register_user_derives`, which also reports the names that are neither.

use crate::globals::Globals;
use burdin_diag::{Code, Diagnostics, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, Ty};

/// The derives the compiler implements itself. A `derive` declaration cannot take one of these names.
pub(crate) const BUILTIN_DERIVES: [&str; 9] =
    ["Copy", "Clone", "Eq", "Ord", "Hash", "Default", "Debug", "Serialize", "Deserialize"];

/// The built-in derive a name in a declaration's `: A, B` list stands for, if it is one.
pub(crate) fn builtin_derive(t: &ast::Type) -> Option<&str> {
    match &t.kind {
        ast::TypeKind::Path(segs) if segs.len() == 1 && segs[0].args.is_empty() => {
            let name = segs[0].name.name.as_str();
            BUILTIN_DERIVES.contains(&name).then_some(name)
        }
        _ => None,
    }
}

/// Reads the built-in derives of a declaration's `: A, B` list. Every other name is left to
/// `Sema::register_user_derives`.
pub(crate) fn parse_derives(list: &[ast::Type]) -> tir::Derives {
    let mut d = tir::Derives::default();
    for t in list {
        let Some(name) = builtin_derive(t) else { continue };
        let flag = match name {
            "Copy" => &mut d.copy,
            "Clone" => &mut d.clone,
            "Eq" => &mut d.eq,
            "Ord" => &mut d.ord,
            "Hash" => &mut d.hash,
            "Default" => &mut d.default,
            "Serialize" => &mut d.serialize,
            "Deserialize" => &mut d.deserialize,
            _ => &mut d.debug,
        };
        *flag = true;
        match name {
            // `Ord` brings `Eq`, as the bound does (`spec/derives.md`, issue #918).
            "Ord" => d.eq = true,
            "Copy" => d.copy_span = Some(t.span),
            "Clone" => d.clone_span = Some(t.span),
            _ => {}
        }
    }
    d
}

/// The requirements of the derives that depend on the memory rules (CP in memory-rules.md): `Copy` needs every
/// field to be Copy and the type to have no `Drop`; `Clone` needs every field to be Clone.
pub(crate) fn validate_derives(globals: &Globals, diags: &mut Diagnostics) {
    // A derive on a generic type holds for the instances whose fields allow it (`Pair<A, B>` is `Eq` when `A` and
    // `B` are), so only the fields that do not mention a type parameter are checked, once, on the identity
    // instance.
    let checked = |key: (bool, usize)| match globals.adt_origin.get(&key) {
        Some(_) => globals.generic_adts.iter().any(|g| g.identity == key),
        None => true,
    };
    let mut fields: Vec<(Ty, Span)> = Vec::new();
    for i in 0..globals.structs.len() {
        if !checked((false, i)) {
            continue;
        }
        let s = &globals.structs[i];
        let ty = Ty::Struct(globals.struct_refs[i].clone());
        fields.clear();
        fields.extend(s.fields.iter().filter(|f| !globals.ty_has_param(&f.ty)).map(|f| (f.ty.clone(), f.span)));
        check_derives(globals, diags, &ty, &s.derives, &fields);
    }
    for i in 0..globals.enums.len() {
        if !checked((true, i)) {
            continue;
        }
        let e = &globals.enums[i];
        let ty = Ty::Enum(globals.enum_refs[i].clone());
        fields.clear();
        fields.extend(
            e.variants
                .iter()
                .flat_map(|v| v.fields.iter())
                .filter(|f| !globals.ty_has_param(&f.ty))
                .map(|f| (f.ty.clone(), f.span)),
        );
        check_derives(globals, diags, &ty, &e.derives, &fields);
    }
}

pub(crate) fn check_derives(
    globals: &Globals,
    diags: &mut Diagnostics,
    ty: &Ty,
    derives: &tir::Derives,
    fields: &[(Ty, Span)],
) {
    if derives.copy {
        if globals.has_drop_impl(ty)
            && let Some(span) = derives.copy_span
        {
            let d = diags.error(Code::DERIVE_REQUIREMENT, span, "a type that implements `Drop` cannot derive `Copy`");
            d.label(span, "cannot derive `Copy` on a type with `Drop`");
        }
        for (fty, span) in fields {
            if !globals.is_copy(fty) {
                let d = diags.error(
                    Code::DERIVE_REQUIREMENT,
                    *span,
                    format!("`Copy` requires every field to be `Copy`, but this one is `{fty}`"),
                );
                d.label(*span, format!("field type `{fty}` is not `Copy`"));
                if let Some(cspan) = derives.copy_span {
                    d.label(cspan, "`Copy` derived here");
                }
            }
        }
    }
    // `Serialize`/`Deserialize` are not here on purpose: their field requirement (every field implements the
    // interface) is the next phase (spec/serialization.md, SER12). The generated body calls the per-shape helpers
    // directly, so it is correct for the types a type can hold today, including a concrete `i32` field that has no
    // `Serialize` impl yet.
    type Requirement = (bool, &'static str, fn(&Globals, &Ty) -> bool);
    let requirements: [Requirement; 6] = [
        (derives.clone, "Clone", Globals::is_clone),
        (derives.eq, "Eq", Globals::has_eq),
        (derives.ord, "Ord", Globals::has_ord),
        (derives.hash, "Hash", Globals::has_hash),
        (derives.default, "Default", Globals::has_default),
        (derives.debug, "Debug", Globals::has_debug),
    ];
    for (derived, name, holds) in requirements {
        if !derived {
            continue;
        }
        for (fty, span) in fields {
            if !holds(globals, fty) {
                let d = diags.error(
                    Code::DERIVE_REQUIREMENT,
                    *span,
                    format!("`{name}` requires every field to be `{name}`, but this one is `{fty}`"),
                );
                d.label(*span, format!("field type `{fty}` does not implement `{name}`"));
            }
        }
    }
    let _ = ty;
}
