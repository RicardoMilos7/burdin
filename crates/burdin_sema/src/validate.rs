//! Checks that look at the whole set of declarations once everything is resolved.

use crate::generics::Head;
use crate::globals::Globals;
use crate::session::AdtSrc;
use burdin_diag::{Code, Diagnostics};
use burdin_tir::Ty;

/// The struct or enum an impl's own methods are registered on: the type itself, the instance whose arguments are
/// the impl's pattern (the identity instance for an impl that names the type's parameters), or `None` when no such
/// instance exists.
fn impl_instance(globals: &Globals, info: &crate::generics::ImplInfo) -> Option<(bool, usize)> {
    match info.head {
        Head::Plain(key) => Some(key),
        Head::Generic(template) => {
            let identity = globals.generic_adts[template].identity;
            let own = globals.adt_instances.get(&(template, info.args.clone())).copied();
            own.or_else(|| crate::impl_match::is_plain_pattern(&info.args).then_some(identity))
        }
    }
}

/// E0141: an impl of an interface must also implement everything the interface requires.
pub(crate) fn check_impl_requirements(globals: &Globals, diags: &mut Diagnostics) {
    for info in &globals.impls {
        let Some((is_enum, id)) = impl_instance(globals, info) else { continue };
        let ty =
            if is_enum { Ty::Enum(globals.enum_refs[id].clone()) } else { Ty::Struct(globals.struct_refs[id].clone()) };
        let iface = &globals.interfaces[info.iface];
        // A generic interface's `requires` bounds name its parameters; the impl's arguments replace them.
        let subst: Vec<(String, Ty)> = iface.params.iter().cloned().zip(info.iface_args.iter().cloned()).collect();
        for r in &iface.requires {
            let bound = globals.subst_bound(r, &subst);
            if !globals.satisfies(&ty, &bound, &info.wheres) {
                diags.error(
                    Code::BOUND_NOT_SATISFIED,
                    info.span,
                    format!(
                        "`{}` requires `{}`, which `{ty}` does not implement",
                        iface.name,
                        globals.bound_name(&bound)
                    ),
                );
            }
        }
    }
}

/// E0003: a method in a `Type: Interface` implementation must have the interface method's signature: the same
/// receiver and conventions, the same parameter types, and the same return type (issue #68).
pub(crate) fn check_impl_signatures(globals: &Globals, diags: &mut Diagnostics) {
    for info in &globals.impls {
        let Some((is_enum, id)) = impl_instance(globals, info) else { continue };
        let key = (is_enum, id);
        let self_ty =
            if is_enum { Ty::Enum(globals.enum_refs[id].clone()) } else { Ty::Struct(globals.struct_refs[id].clone()) };
        let mut env: Vec<(String, Ty)> = vec![("Self".to_string(), self_ty)];
        // A generic interface's parameters stand for the impl's arguments in every method signature.
        env.extend(globals.interfaces[info.iface].params.iter().cloned().zip(info.iface_args.iter().cloned()));
        for expected in &globals.interfaces[info.iface].methods {
            let Some(&func) = globals.methods.get(&(key, expected.name.clone())) else { continue };
            let Some(&span) = globals.method_spans.get(&(key, expected.name.clone())) else { continue };
            // A method with its own type parameters (`fn map<U>`) has no single concrete signature to compare; its
            // calls are checked against the bounds instead.
            if expected.generic || globals.method_templates.contains_key(&func) {
                continue;
            }
            let got = &globals.sigs[func.0];
            let want_params: Vec<burdin_tir::ParamSig> =
                expected.sig.params.iter().map(|p| globals.subst_param(p, &env)).collect();
            let want_ret = globals.subst_ty(&expected.sig.ret, &env);
            // A generic instance the substitution needed may not exist yet; the next pass compares it.
            if want_ret == Ty::Error || want_params.iter().any(|p| p.ty == Ty::Error) {
                continue;
            }
            let mut what = Vec::new();
            if got.has_self != expected.sig.has_self {
                what.push(if expected.sig.has_self { "it needs a receiver" } else { "it must not have a receiver" });
            }
            if got.params.len() != want_params.len() {
                what.push("it has a different number of parameters");
            }
            for (i, (g, w)) in got.params.iter().zip(&want_params).enumerate() {
                if g.ty != w.ty {
                    what.push(if i == 0 && got.has_self {
                        "the receiver type differs"
                    } else {
                        "a parameter type differs"
                    });
                } else if g.by_ref != w.by_ref
                    || g.own != w.own
                    || g.mut_param != w.mut_param
                    || g.exclusive != w.exclusive
                {
                    what.push(if i == 0 && got.has_self {
                        "the receiver convention differs"
                    } else {
                        "a parameter convention differs"
                    });
                }
            }
            if got.ret != want_ret {
                what.push("the return type differs");
            }
            if globals.fn_unsafe[func.0] != expected.is_unsafe {
                what.push("`unsafe` differs");
            }
            if !what.is_empty() {
                what.dedup();
                diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!(
                        "method `{}` does not match the signature of `{}`: {}",
                        expected.name,
                        globals.interfaces[info.iface].name,
                        what.join(", ")
                    ),
                );
            }
        }
    }
}

/// E0150: a public function's signature and a public struct's fields may not name a package-private type; another
/// package could otherwise be handed a value whose type it cannot name (issue #78).
pub(crate) fn check_public_signatures(
    globals: &Globals,
    items: &[crate::session::FnItem<'_>],
    diags: &mut Diagnostics,
) {
    for (i, item) in items.iter().enumerate() {
        if item.decl.is_priv {
            continue;
        }
        // An instance of a generic function has the type arguments of its caller, which may be package-private
        // (`drop` of a `Frame`); the check belongs to the declaration, whose signature names only parameters.
        if !item.symbolic && !item.subst.is_empty() {
            continue;
        }
        // A method or associated function of a package-private type is unreachable from another package: that
        // package cannot name the type, so nothing in its signature can leak. This is what makes an impl on a
        // `priv` type (the standard library's `ContextError: Display`, or a `priv error enum`) legal.
        if item.self_ty.as_ref().is_some_and(|ty| globals.private_adt(ty).is_some()) {
            continue;
        }
        let Some(sig) = globals.sigs.get(i) else { continue };
        let private =
            sig.params.iter().find_map(|p| globals.private_adt(&p.ty)).or_else(|| globals.private_adt(&sig.ret));
        if let Some(name) = private {
            diags.error(
                Code::PRIVATE_ITEM,
                item.decl.name.span,
                format!("`{name}` is `priv`, but this public signature uses it"),
            );
        }
    }
    for (id, s) in globals.structs.iter().enumerate() {
        if globals.adt_priv.get(&(false, id)).copied().unwrap_or(false) || globals.adt_origin.contains_key(&(false, id))
        {
            continue;
        }
        for f in &s.fields {
            if f.is_priv {
                continue;
            }
            if let Some(name) = globals.private_adt(&f.ty) {
                diags.error(Code::PRIVATE_ITEM, f.span, format!("`{name}` is `priv`, but this public field uses it"));
            }
        }
    }
    for (id, e) in globals.enums.iter().enumerate() {
        if globals.adt_priv.get(&(true, id)).copied().unwrap_or(false) || globals.adt_origin.contains_key(&(true, id)) {
            continue;
        }
        for v in &e.variants {
            for f in &v.fields {
                if f.is_priv {
                    continue;
                }
                if let Some(name) = globals.private_adt(&f.ty) {
                    diags.error(
                        Code::PRIVATE_ITEM,
                        f.span,
                        format!("`{name}` is `priv`, but this public variant field uses it"),
                    );
                }
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
            Ty::Array(t, _) => contained(t, out),
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
    // A wrapper of an `impl Interface` return has no declaration, so it has no source name to report.
    let keys = (0..src.structs.len())
        .filter(|&i| src.structs[i].is_some())
        .map(|i| (false, i))
        .chain((0..src.enums.len()).map(|i| (true, i)));
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
            let name = if key.0 {
                &src.enums[key.1].decl.name
            } else {
                // Only declared structs are keys (see `keys` above).
                &src.structs[key.1].as_ref().expect("a declared struct").decl.name
            };
            diags.error(
                Code::CYCLIC_DECLARATION,
                name.span,
                format!(
                    "`{}` contains itself, so it would have infinite size; put the inner value in a `Heap`",
                    name.name
                ),
            );
        }
    }
}
