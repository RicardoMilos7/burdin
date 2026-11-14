//! Checks that look at the whole set of declarations once everything is resolved.

use crate::generics::Head;
use crate::globals::Globals;
use crate::session::AdtSrc;
use burdin_diag::{Code, Diagnostics};
use burdin_tir::Ty;

/// E0141: an impl of an interface must also implement everything the interface requires.
pub(crate) fn check_impl_requirements(globals: &Globals, diags: &mut Diagnostics) {
    for info in &globals.impls {
        let (is_enum, id) = match info.head {
            Head::Plain(key) => key,
            Head::Generic(template) => globals.generic_adts[template].identity,
        };
        let ty =
            if is_enum { Ty::Enum(globals.enum_refs[id].clone()) } else { Ty::Struct(globals.struct_refs[id].clone()) };
        for r in &globals.interfaces[info.iface].requires {
            if !globals.satisfies(&ty, *r, &info.wheres) {
                diags.error(
                    Code::BOUND_NOT_SATISFIED,
                    info.span,
                    format!(
                        "`{}` requires `{}`, which `{ty}` does not implement",
                        globals.interfaces[info.iface].name,
                        globals.bound_name(*r)
                    ),
                );
            }
        }
    }
}

/// A struct or enum that contains itself by value would have infinite size.
pub(crate) fn check_infinite_size(globals: &Globals, src: &AdtSrc, diags: &mut Diagnostics) {
    fn contained(ty: &Ty, out: &mut Vec<(bool, usize)>) {
        match ty {
            Ty::Struct(a) => out.push((false, a.id)),
            Ty::Enum(a) => out.push((true, a.id)),
            Ty::Array(t, _) | Ty::Option(t) => contained(t, out),
            Ty::Tuple(ts) => ts.iter().for_each(|t| contained(t, out)),
            _ => {}
        }
    }
    let direct = |key: (bool, usize)| -> Vec<(bool, usize)> {
        let mut out = Vec::new();
        if key.0 {
            for v in &globals.enums[key.1].variants {
                v.fields.iter().for_each(|f| contained(&f.ty, &mut out));
            }
        } else {
            globals.structs[key.1].fields.iter().for_each(|f| contained(&f.ty, &mut out));
        }
        out
    };
    let keys = (0..src.structs.len()).map(|i| (false, i)).chain((0..src.enums.len()).map(|i| (true, i)));
    for key in keys {
        let mut seen = Vec::new();
        let mut stack = direct(key);
        let mut cyclic = false;
        while let Some(k) = stack.pop() {
            if k == key {
                cyclic = true;
                break;
            }
            if !seen.contains(&k) {
                seen.push(k);
                stack.extend(direct(k));
            }
        }
        if cyclic {
            let name = if key.0 { &src.enums[key.1].0.name } else { &src.structs[key.1].0.name };
            diags.error(
                Code::UNSUPPORTED,
                name.span,
                format!(
                    "`{}` contains itself, so it would have infinite size; put the inner value in a `Heap`",
                    name.name
                ),
            );
        }
    }
}
