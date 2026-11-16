//! Derives: reading the `derives (...)` clause and checking its requirements (memory-rules.md, CP).

use crate::globals::Globals;
use burdin_diag::{Code, Diagnostics, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, Ty};

/// The derives the compiler implements itself. A `derive` declaration cannot take one of these names.
pub(crate) const BUILTIN_DERIVES: [&str; 7] = ["Copy", "Clone", "Eq", "Ord", "Hash", "Default", "Debug"];

/// Reads the built-in derives of a `derives (...)` clause. A name `user` accepts is a `derive` declaration, which
/// `Sema::register_user_derives` expands; any other name is unknown.
pub(crate) fn parse_derives(list: &[ast::Type], user: &dyn Fn(&str) -> bool, diags: &mut Diagnostics) -> tir::Derives {
    let mut d = tir::Derives::default();
    for t in list {
        let name = match &t.kind {
            ast::TypeKind::Path(segs) if segs.len() == 1 && segs[0].args.is_empty() => segs[0].name.name.as_str(),
            _ => "",
        };
        let flag = match name {
            "Copy" => &mut d.copy,
            "Clone" => &mut d.clone,
            "Eq" => &mut d.eq,
            "Ord" => &mut d.ord,
            "Hash" => &mut d.hash,
            "Default" => &mut d.default,
            "Debug" => &mut d.debug,
            _ if user(name) => continue,
            _ => {
                diags.error(
                    Code::UNKNOWN_NAME,
                    t.span,
                    "only `Copy`, `Clone`, `Eq`, `Ord`, `Hash`, `Default`, `Debug`, and the `derive` declarations in \
                     scope can be derived",
                );
                continue;
            }
        };
        *flag = true;
        match name {
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
            diags.error(Code::DERIVE_REQUIREMENT, span, "a type that implements `Drop` cannot derive `Copy`");
        }
        for (fty, span) in fields {
            if !globals.is_copy(fty) {
                diags.error(
                    Code::DERIVE_REQUIREMENT,
                    *span,
                    format!("`Copy` requires every field to be `Copy`, but this one is `{fty}`"),
                );
            }
        }
    }
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
                diags.error(
                    Code::DERIVE_REQUIREMENT,
                    *span,
                    format!("`{name}` requires every field to be `{name}`, but this one is `{fty}`"),
                );
            }
        }
    }
    let _ = ty;
}
