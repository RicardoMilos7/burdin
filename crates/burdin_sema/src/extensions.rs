//! Extension methods: `fn Type.name(self, ...)` (`spec/extensions.md`).
//!
//! An extension is an ordinary function whose first parameter is `self`, registered under the receiver type it
//! names instead of under the type's method table. It is visible in its declaring module and in modules that
//! import it by name (EX2), and a call falls back to it only when the receiver type has no method of that name
//! (EX3). An extension whose name a method of the receiver type shadows gets E0215 at its declaration (EX4).

use crate::builtins::{builtin_type, method as builtin_method};
use crate::generics::{Bound, GenericFn, param_ty};
use crate::globals::{ExtensionInfo, Globals, adt_key};
use crate::session::{FnItem, Sema};
use burdin_diag::{Code, Diagnostics};
use burdin_syntax::ast;
use burdin_tir::Ty;

impl<'t> Sema<'t> {
    /// Registers the function of every extension declaration. This runs after imports (`use ext.shout` names the
    /// extension) and after the impl blocks, so the shadowing check can see the receiver type's methods.
    pub(crate) fn register_extensions(&mut self, diags: &mut Diagnostics) {
        for id in 0..self.extension_srcs.len() {
            self.register_extension(id, diags);
        }
    }

    fn register_extension(&mut self, id: usize, diags: &mut Diagnostics) {
        let decl = self.extension_srcs[id].decl;
        let module = self.extension_srcs[id].module;
        let f = &decl.func;
        let name = f.name.name.clone();
        let Some(segments) = receiver_segments(&decl.recv) else {
            diags.error(
                Code::INVALID_EXTENSION,
                decl.recv.span,
                format!("the receiver of an extension method must be a type name, as in `fn String.{name}(self)`"),
            );
            return;
        };
        if segments[..segments.len() - 1].iter().any(|s| !s.args.is_empty()) {
            diags.error(
                Code::INVALID_EXTENSION,
                decl.recv.span,
                "type arguments belong on the type name at the end of an extension method's receiver path",
            );
            return;
        }
        if !matches!(f.params.first().map(|p| &p.kind), Some(ast::ParamKind::SelfParam)) {
            diags.error(
                Code::INVALID_EXTENSION,
                f.name.span,
                format!("an extension method needs `self` as its first parameter, as in `fn Type.{name}(self, ...)`"),
            );
            return;
        }
        // The receiver's type arguments that are names declare the extension's type parameters; the method's
        // own `<...>` list follows them. Bounds come from the method's inline bounds and its `where` clause.
        let last = segments.last().expect("a receiver path has a segment");
        let mut params: Vec<String> = Vec::new();
        let mut recv_params: Vec<String> = Vec::new();
        for arg in &last.args {
            let Some(param) = receiver_param(&self.globals, module, arg) else { continue };
            if params.contains(&param) {
                diags.error(Code::DUPLICATE_NAME, arg.span, format!("type parameter `{param}` is declared twice"));
                continue;
            }
            params.push(param.clone());
            recv_params.push(param);
        }
        for g in &f.generics {
            if params.contains(&g.name.name) {
                diags.error(
                    Code::DUPLICATE_NAME,
                    g.name.span,
                    format!("type parameter `{}` is declared twice", g.name.name),
                );
                continue;
            }
            params.push(g.name.name.clone());
        }
        let subst: Vec<(String, Ty)> = params.iter().map(|p| (p.clone(), param_ty(p))).collect();
        let bounds = extension_bounds(f, &params, &subst, module, &self.globals, diags);
        let recv = self.globals.resolve_type_in(&decl.recv, None, &subst, module, diags);
        let internal = self.globals.qualified(module, &format!("ext.{}.{name}", receiver_text(&decl.recv)));
        let (func, generic) = if params.is_empty() {
            let func = self.push_fn(FnItem::plain(f, internal, module, Some(recv.clone())));
            (Some(func), None)
        } else {
            let env: Vec<(String, Vec<Bound>)> = params.iter().cloned().zip(bounds.iter().cloned()).collect();
            let identity = self.push_fn(FnItem {
                decl: f,
                name: internal.clone(),
                module,
                self_ty: Some(recv.clone()),
                subst: subst.clone(),
                bounds: env,
                symbolic: true,
                check_body: true,
                is_unsafe: f.is_unsafe,
                is_exported: false,
            });
            let index = self.globals.generic_fns.len();
            self.globals.generic_fns.push(GenericFn {
                name: internal,
                module,
                is_unsafe: f.is_unsafe,
                params: params.clone(),
                bounds: bounds.clone(),
                decl: self.tpl.fns.len(),
                identity,
                recv: Some(recv.clone()),
            });
            self.tpl.fns.push(f);
            (Some(identity), Some(index))
        };
        let info = &mut self.globals.extensions[id];
        info.recv = recv;
        info.params = params;
        info.recv_params = recv_params;
        info.bounds = bounds;
        info.generic = generic;
        info.func = func;
    }

    /// Resolves each extension's receiver again, after instances were allocated. A receiver that names a generic
    /// type is an instance, which may not exist in the first pass; `resolve_all` repeats until it does.
    pub(crate) fn refresh_extensions(&mut self, diags: &mut Diagnostics) {
        for id in 0..self.globals.extensions.len() {
            let Some(func) = self.globals.extensions[id].func else { continue };
            let decl = self.extension_srcs[id].decl;
            let module = self.extension_srcs[id].module;
            let params = self.globals.extensions[id].params.clone();
            let subst: Vec<(String, Ty)> = params.iter().map(|p| (p.clone(), param_ty(p))).collect();
            let recv = self.globals.resolve_type_in(&decl.recv, None, &subst, module, diags);
            self.globals.extensions[id].recv = recv.clone();
            self.fn_items[func.0].self_ty = Some(recv.clone());
            if let Some(g) = self.globals.extensions[id].generic {
                self.globals.generic_fns[g].recv = Some(recv);
            }
        }
    }

    /// The two whole-program diagnostics of extension methods: a second declaration for the same receiver type
    /// in one module (E0007), and an extension a method of its receiver type shadows (E0215, a warning).
    pub(crate) fn check_extension_problems(&self, diags: &mut Diagnostics) {
        for module in 0..self.globals.modules.len() {
            for ids in self.globals.modules[module].extensions.values() {
                for (k, &id) in ids.iter().enumerate() {
                    let ext = &self.globals.extensions[id];
                    if ext.module != module || ext.func.is_none() || ext.recv == Ty::Error {
                        continue;
                    }
                    let duplicate = ids[..k].iter().any(|&other| {
                        let other = &self.globals.extensions[other];
                        other.module == module
                            && other.func.is_some()
                            && same_receiver_shape(&self.globals, &other.recv, &ext.recv)
                    });
                    if duplicate {
                        diags.error(
                            Code::DUPLICATE_NAME,
                            ext.span,
                            format!(
                                "`{}` is declared for the same receiver type more than once in this module",
                                ext.name
                            ),
                        );
                        continue;
                    }
                    if self.receiver_has_method(&ext.recv, &ext.name) {
                        diags.warning(
                            Code::EXTENSION_SHADOWED,
                            ext.span,
                            format!(
                                "the extension method `{}` is shadowed by a method of `{}`; calls use the method",
                                ext.name, ext.recv
                            ),
                        );
                    }
                }
            }
        }
    }

    /// Whether `recv` has a method named `name` without looking at extensions: one the type declares, one an
    /// interface implementation provides, or one the compiler provides for it.
    fn receiver_has_method(&self, recv: &Ty, name: &str) -> bool {
        if let Some(key) = adt_key(recv)
            && self.globals.methods.contains_key(&(key, name.to_string()))
        {
            return true;
        }
        if builtin_method(name, recv).is_some() {
            return true;
        }
        match name {
            "clone" => self.globals.is_clone(recv),
            "hash" => self.globals.has_hash_in(recv, &[]),
            "cmp" => self.globals.has_ord_in(recv, &[]),
            "to_string" => self.to_string_is_provided(recv),
            _ => self.iterator_name(recv, name),
        }
    }

    fn to_string_is_provided(&self, recv: &Ty) -> bool {
        match recv {
            Ty::Float(_) => true,
            Ty::Struct(_) | Ty::Enum(_) | Ty::Param(_) | Ty::Any(_) => self
                .globals
                .display_iface
                .is_some_and(|iface| self.globals.satisfies(recv, &Bound::User(iface, Vec::new()), &[])),
            _ => false,
        }
    }

    /// The names the iteration protocol provides for `recv`: `next`, `sum`, the `Iterator` defaults (spelled
    /// `any` for `any_item`), and the eager `List` helpers.
    fn iterator_name(&self, recv: &Ty, name: &str) -> bool {
        let spelled = if name == "any" { "any_item" } else { name };
        let iterator = self.globals.iterator_item(recv, &[]).is_some();
        if iterator
            && (spelled == "next"
                || spelled == "sum"
                || spelled == "product"
                || self.globals.iterator_item_fn(spelled).is_some())
        {
            return true;
        }
        if iterator
            && let Some(iface) = self.globals.iterator_iface
            && self.globals.interfaces[iface].methods.iter().any(|m| m.name == spelled)
        {
            return true;
        }
        if matches!(recv, Ty::List(_, _)) && self.globals.eager_list_fn(name).is_some() {
            return true;
        }
        if self.globals.iterable_item(recv, &[]).is_some()
            && let Some(iface) = self.globals.iterable_iface
            && self.globals.interfaces[iface].methods.iter().any(|m| m.name == spelled)
        {
            return true;
        }
        false
    }
}

/// Whether a visible extension applies to a receiver of type `recv_ty`: its receiver pattern matches, and every
/// type parameter that occurs in the pattern is bound by the match.
pub(crate) fn extension_applies(globals: &Globals, ext: &ExtensionInfo, recv_ty: &Ty) -> bool {
    if ext.func.is_none() || *recv_ty == Ty::Error || ext.recv == Ty::Error {
        return false;
    }
    if ext.recv_params.is_empty() {
        return ext.recv == *recv_ty;
    }
    let mut bindings = Vec::new();
    globals.unify(&ext.recv, recv_ty, &mut bindings);
    ext.recv_params.iter().all(|p| bindings.iter().any(|(n, t)| n == p && *t != Ty::Error))
}

fn receiver_segments(ty: &ast::Type) -> Option<&[ast::PathSegment]> {
    match &ty.kind {
        ast::TypeKind::Path(segments) if !segments.is_empty() => Some(segments),
        _ => None,
    }
}

/// A receiver type argument that is an ordinary name not bound to a type: a type parameter of the extension.
fn receiver_param(globals: &Globals, module: usize, arg: &ast::Type) -> Option<String> {
    match &arg.kind {
        ast::TypeKind::Path(segments)
            if segments.len() == 1
                && segments[0].args.is_empty()
                && globals.type_name(module, &segments[0].name.name).is_none()
                && builtin_type(&segments[0].name.name).is_none() =>
        {
            Some(segments[0].name.name.clone())
        }
        _ => None,
    }
}

/// The bounds of every parameter of an extension, in the order of `params`: the method's own inline bounds, and
/// each `where` predicate on one of the parameters (`where T: Ord`).
fn extension_bounds(
    f: &ast::FnDecl,
    params: &[String],
    subst: &[(String, Ty)],
    module: usize,
    globals: &Globals,
    diags: &mut Diagnostics,
) -> Vec<Vec<Bound>> {
    let mut all: Vec<Vec<Bound>> = vec![Vec::new(); params.len()];
    for g in &f.generics {
        let Some(index) = params.iter().position(|p| *p == g.name.name) else { continue };
        for bound in &g.bounds {
            if let Some(bound) = globals.resolve_bound(bound, subst, module, diags) {
                all[index].push(bound);
            }
        }
    }
    for pred in &f.where_clause {
        let index = match &pred.ty.kind {
            ast::TypeKind::Path(segments) if segments.len() == 1 && segments[0].args.is_empty() => {
                params.iter().position(|p| *p == segments[0].name.name)
            }
            _ => None,
        };
        let Some(index) = index else {
            diags.error(
                Code::INVALID_EXTENSION,
                pred.ty.span,
                "a `where` clause of an extension method must name one of its type parameters",
            );
            continue;
        };
        for bound in &pred.bounds {
            if let Some(bound) = globals.resolve_bound(bound, subst, module, diags) {
                all[index].push(bound);
            }
        }
    }
    all
}

/// The text of a receiver type, for the internal function name. Only the shapes the grammar allows in a receiver
/// path occur here, plus the arguments in them.
fn receiver_text(ty: &ast::Type) -> String {
    match &ty.kind {
        ast::TypeKind::Path(segments) => segments
            .iter()
            .map(|segment| {
                if segment.args.is_empty() {
                    segment.name.name.clone()
                } else {
                    let args: Vec<String> = segment.args.iter().map(receiver_text).collect();
                    format!("{}<{}>", segment.name.name, args.join(", "))
                }
            })
            .collect::<Vec<_>>()
            .join("."),
        ast::TypeKind::Ref { ty, .. } => format!("&{}", receiver_text(ty)),
        ast::TypeKind::Slice(ty) => format!("[{}]", receiver_text(ty)),
        ast::TypeKind::Array(ty, len) => format!("[{}; {len:?}]", receiver_text(ty)),
        _ => "_".to_string(),
    }
}

/// Whether two receiver types name the same type, treating every type parameter as the same placeholder. This is
/// what makes `fn Foo<T>.m` and `fn Foo<U>.m` the same declaration.
fn same_receiver_shape(globals: &Globals, a: &Ty, b: &Ty) -> bool {
    let pair = |x: &Ty, y: &Ty| same_receiver_shape(globals, x, y);
    match (a, b) {
        (Ty::Param(_), Ty::Param(_)) => true,
        (Ty::Ref { mutable: m, inner: i }, Ty::Ref { mutable: n, inner: j }) => m == n && pair(i, j),
        (Ty::Slice(i), Ty::Slice(j))
        | (Ty::Option(i), Ty::Option(j))
        | (Ty::Heap(i, _), Ty::Heap(j, _))
        | (Ty::CPtr(i), Ty::CPtr(j))
        | (Ty::List(i, _), Ty::List(j, _)) => pair(i, j),
        (Ty::Array(i, m), Ty::Array(j, n)) => m == n && pair(i, j),
        (Ty::Range(i, m), Ty::Range(j, n)) => m == n && pair(i, j),
        (Ty::Result(io, ie), Ty::Result(jo, je)) => pair(io, jo) && pair(ie, je),
        (Ty::Map(ik, iv, _), Ty::Map(jk, jv, _)) | (Ty::MapEntry(ik, iv, _), Ty::MapEntry(jk, jv, _)) => {
            pair(ik, jk) && pair(iv, jv)
        }
        (Ty::Tuple(i), Ty::Tuple(j)) => i.len() == j.len() && i.iter().zip(j).all(|(x, y)| pair(x, y)),
        (Ty::Struct(_), Ty::Struct(_)) | (Ty::Enum(_), Ty::Enum(_)) => {
            match (globals.origin_of(a), globals.origin_of(b)) {
                (Some((ta, aa)), Some((tb, ab))) => {
                    ta == tb && aa.len() == ab.len() && aa.iter().zip(ab).all(|(x, y)| pair(x, y))
                }
                _ => a == b,
            }
        }
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burdin_tir::{FuncId, IntTy};

    fn extension(recv: Ty, params: &[&str], recv_params: &[&str]) -> ExtensionInfo {
        ExtensionInfo {
            name: "m".into(),
            recv,
            module: 0,
            params: params.iter().map(|p| p.to_string()).collect(),
            recv_params: recv_params.iter().map(|p| p.to_string()).collect(),
            bounds: vec![Vec::new(); params.len()],
            generic: None,
            func: Some(FuncId(0)),
            span: burdin_diag::Span::new(burdin_diag::FileId(0), 0, 0),
        }
    }

    #[test]
    fn an_extension_applies_only_to_its_receiver_shape() {
        let globals = Globals::default();
        let generic = extension(Ty::List(Box::new(Ty::Param("T".into())), burdin_tir::Alloc::Global), &["T"], &["T"]);
        assert!(extension_applies(
            &globals,
            &generic,
            &Ty::List(Box::new(Ty::Int(IntTy::I32)), burdin_tir::Alloc::Global)
        ));
        assert!(!extension_applies(
            &globals,
            &generic,
            &Ty::Map(Box::new(Ty::Int(IntTy::I32)), Box::new(Ty::Int(IntTy::I32)), burdin_tir::Alloc::Global)
        ));
        assert!(!extension_applies(&globals, &generic, &Ty::Error));

        let concrete = extension(Ty::String, &[], &[]);
        assert!(extension_applies(&globals, &concrete, &Ty::String));
        assert!(!extension_applies(&globals, &concrete, &Ty::Str));
    }

    #[test]
    fn receiver_shapes_ignore_parameter_names() {
        let globals = Globals::default();
        let left = Ty::List(Box::new(Ty::Param("T".into())), burdin_tir::Alloc::Global);
        let right = Ty::List(Box::new(Ty::Param("U".into())), burdin_tir::Alloc::Global);
        assert!(same_receiver_shape(&globals, &left, &right));
        assert!(same_receiver_shape(&globals, &Ty::Int(IntTy::I32), &Ty::Int(IntTy::I32)));
        assert!(!same_receiver_shape(&globals, &Ty::Int(IntTy::I32), &Ty::Int(IntTy::I64)));
        assert!(!same_receiver_shape(
            &globals,
            &Ty::List(Box::new(Ty::Int(IntTy::I32)), burdin_tir::Alloc::Global),
            &Ty::List(Box::new(Ty::String), burdin_tir::Alloc::Global)
        ));
    }
}
