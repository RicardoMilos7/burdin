//! Generics and interfaces.
//!
//! Generic functions and types are monomorphized while checking. Each use with concrete type arguments names an
//! *instance*: a function or a struct or enum of its own, with the type parameters replaced. Instances are
//! allocated on request: resolving a type or checking a call that needs an instance that does not exist yet
//! records a [`Request`] and goes on with a placeholder, and the driver in `check` allocates the instance and
//! checks everything again until no request is left.
//!
//! Every generic function and every impl of a generic type is also checked once against its declared bounds, with
//! the type parameters as opaque `Ty::Param` types. That check is what rejects `a > b` in a body whose `T` has no
//! `Ord` bound (E0141). The functions and types it creates are *symbolic*: they are checked and then dropped.

use crate::globals::{Globals, adt_key};
use burdin_diag::{Code, Diagnostics, Span};
use burdin_syntax::ast;
use burdin_tir::{FuncId, ParamSig, Signature, Ty};
use std::rc::Rc;

/// A requirement on a type parameter: a built-in interface or a user interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Bound {
    User(usize),
    Copy,
    Clone,
    Eq,
    Ord,
    Hash,
    Default,
}

pub(crate) const BUILTIN_INTERFACES: &[&str] = &["Copy", "Clone", "Eq", "Ord", "Hash", "Default", "Drop", "From"];

/// The bounds of each type parameter in scope: `(name, bounds)`. A name may appear more than once.
pub(crate) type Env = [(String, Vec<Bound>)];

pub(crate) struct IfaceMethod {
    pub name: String,
    /// Written with `Self` as `Ty::Param("Self")`.
    pub sig: Signature,
    pub has_default: bool,
    /// An `unsafe fn` of the interface: calling it requires an `unsafe` context.
    pub is_unsafe: bool,
}

pub(crate) struct InterfaceInfo {
    pub name: String,
    /// The module that declares the interface, whose names its bounds and default bodies use.
    pub module: usize,
    pub requires: Vec<Bound>,
    pub methods: Vec<IfaceMethod>,
}

pub(crate) struct GenericAdt {
    pub name: String,
    /// The module that declares the type.
    pub module: usize,
    /// Package-private: other packages cannot name it.
    pub is_priv: bool,
    pub is_enum: bool,
    pub params: Vec<String>,
    pub bounds: Vec<Vec<Bound>>,
    /// Index into the template list of structs or of enums.
    pub decl: usize,
    /// The instance whose arguments are its own parameters. Its fields are the pattern that struct literals and
    /// variant constructors are matched against to infer the arguments.
    pub identity: (bool, usize),
}

pub(crate) struct GenericFn {
    pub name: String,
    /// The module that declares the function, whose names its body uses.
    pub module: usize,
    pub is_unsafe: bool,
    pub params: Vec<String>,
    pub bounds: Vec<Vec<Bound>>,
    pub decl: usize,
    /// The symbolic instance; its signature is the pattern that call arguments are matched against.
    pub identity: FuncId,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Head {
    Plain((bool, usize)),
    Generic(usize),
}

/// One `Type: Interface { ... }` block, for answering "does this type implement that interface".
pub(crate) struct ImplInfo {
    pub head: Head,
    pub iface: usize,
    /// The names the impl gives the type's arguments (`T` in `Wrapper<T>: Describe where T: Describe`).
    pub params: Vec<String>,
    pub wheres: Vec<(String, Vec<Bound>)>,
    pub span: Span,
}

pub(crate) enum Request {
    Adt(usize, Vec<Ty>),
    Func(usize, Vec<Ty>),
}

/// The function id that stands for a call inside symbolic code, which has no code to call.
pub(crate) const SYMBOLIC_CALL: FuncId = FuncId(usize::MAX);

impl Globals {
    /// E0141: the arguments of a generic type instantiation must meet the bounds on its parameters. An argument
    /// that still mentions a type parameter is left to the symbolic check of the enclosing body.
    pub(crate) fn check_adt_bounds(&self, template: usize, args: &[Ty], span: Span, diags: &mut Diagnostics) {
        let info = &self.generic_adts[template];
        for ((pname, bounds), arg) in info.params.iter().zip(&info.bounds).zip(args) {
            if self.ty_has_param(arg) {
                continue;
            }
            for bound in bounds {
                if !self.satisfies(arg, *bound, &[]) {
                    diags.error(
                        Code::BOUND_NOT_SATISFIED,
                        span,
                        format!(
                            "`{arg}` does not implement `{}`, which `{pname}` of `{}` requires",
                            self.bound_name(*bound),
                            info.name
                        ),
                    );
                }
            }
        }
    }

    /// The instance of a generic type for these arguments. If there is none yet, asks for it and gives `Error`
    /// for now.
    pub(crate) fn adt_instance_ty(&self, template: usize, args: Vec<Ty>) -> Ty {
        if args.contains(&Ty::Error) {
            return Ty::Error;
        }
        match self.adt_instances.get(&(template, args.clone())) {
            Some(&(is_enum, id)) => {
                if is_enum {
                    Ty::Enum(self.enum_refs[id].clone())
                } else {
                    Ty::Struct(self.struct_refs[id].clone())
                }
            }
            None => {
                self.requests.borrow_mut().push(Request::Adt(template, args));
                Ty::Error
            }
        }
    }

    /// The generic type and arguments a struct or enum instance came from.
    pub(crate) fn origin_of(&self, ty: &Ty) -> Option<&(usize, Vec<Ty>)> {
        self.adt_origin.get(&adt_key(ty)?)
    }

    /// Whether a type mentions a type parameter, so it belongs to symbolic code.
    pub(crate) fn ty_has_param(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Param(_) => true,
            Ty::Ref { inner, .. }
            | Ty::Slice(inner)
            | Ty::Array(inner, _)
            | Ty::Option(inner)
            | Ty::Heap(inner)
            | Ty::CPtr(inner)
            | Ty::Range(inner, _)
            | Ty::List(inner) => self.ty_has_param(inner),
            Ty::Result(ok, err) => self.ty_has_param(ok) || self.ty_has_param(err),
            Ty::Map(k, v) => self.ty_has_param(k) || self.ty_has_param(v),
            Ty::Tuple(ts) => ts.iter().any(|t| self.ty_has_param(t)),
            Ty::Struct(_) | Ty::Enum(_) => match self.origin_of(ty) {
                Some((_, args)) => args.iter().any(|a| self.ty_has_param(a)),
                None => false,
            },
            _ => false,
        }
    }

    /// Matches a type written with type parameters (`pat`) against an actual type, recording what each parameter
    /// stands for. A parameter keeps its first binding.
    pub(crate) fn unify(&self, pat: &Ty, act: &Ty, bindings: &mut Vec<(String, Ty)>) {
        match (pat, act) {
            (_, Ty::Error | Ty::Never) => {}
            (Ty::Param(n), _) => {
                if !bindings.iter().any(|(m, _)| m == &**n) {
                    bindings.push((n.to_string(), act.clone()));
                }
            }
            (Ty::Ref { inner: p, .. }, Ty::Ref { inner: a, .. })
            | (Ty::Slice(p), Ty::Slice(a))
            | (Ty::Array(p, _), Ty::Array(a, _))
            | (Ty::Option(p), Ty::Option(a))
            | (Ty::Heap(p), Ty::Heap(a))
            | (Ty::CPtr(p), Ty::CPtr(a))
            | (Ty::Range(p, _), Ty::Range(a, _))
            | (Ty::List(p), Ty::List(a)) => self.unify(p, a, bindings),
            (Ty::Result(po, pe), Ty::Result(ao, ae)) => {
                self.unify(po, ao, bindings);
                self.unify(pe, ae, bindings);
            }
            (Ty::Map(pk, pv), Ty::Map(ak, av)) => {
                self.unify(pk, ak, bindings);
                self.unify(pv, av, bindings);
            }
            (Ty::Tuple(ps), Ty::Tuple(acts)) if ps.len() == acts.len() => {
                for (p, a) in ps.iter().zip(acts) {
                    self.unify(p, a, bindings);
                }
            }
            (Ty::Struct(_), Ty::Struct(_)) | (Ty::Enum(_), Ty::Enum(_)) => {
                if let (Some((pt, pargs)), Some((at, aargs))) = (self.origin_of(pat), self.origin_of(act))
                    && pt == at
                {
                    for (p, a) in pargs.iter().zip(aargs) {
                        self.unify(p, a, bindings);
                    }
                }
            }
            _ => {}
        }
    }

    /// Replaces type parameters by the types in `env`.
    pub(crate) fn subst_ty(&self, ty: &Ty, env: &[(String, Ty)]) -> Ty {
        let go = |t: &Ty| Box::new(self.subst_ty(t, env));
        match ty {
            Ty::Param(n) => env.iter().find(|(m, _)| m == &**n).map(|(_, t)| t.clone()).unwrap_or_else(|| ty.clone()),
            Ty::Ref { mutable, inner } => Ty::Ref { mutable: *mutable, inner: go(inner) },
            Ty::Slice(t) => Ty::Slice(go(t)),
            Ty::Array(t, n) => Ty::Array(go(t), *n),
            Ty::Option(t) => Ty::Option(go(t)),
            Ty::Result(t, e) => Ty::Result(go(t), go(e)),
            Ty::List(t) => Ty::List(go(t)),
            Ty::Map(k, v) => Ty::Map(go(k), go(v)),
            Ty::Heap(t) => Ty::Heap(go(t)),
            Ty::CPtr(t) => Ty::CPtr(go(t)),
            Ty::Range(t, inclusive) => Ty::Range(go(t), *inclusive),
            Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| self.subst_ty(t, env)).collect()),
            Ty::Struct(_) | Ty::Enum(_) => match self.origin_of(ty) {
                Some((template, args)) => {
                    let args = args.iter().map(|a| self.subst_ty(a, env)).collect();
                    self.adt_instance_ty(*template, args)
                }
                None => ty.clone(),
            },
            _ => ty.clone(),
        }
    }

    pub(crate) fn subst_param(&self, p: &ParamSig, env: &[(String, Ty)]) -> ParamSig {
        ParamSig { ty: self.subst_ty(&p.ty, env), ..p.clone() }
    }

    // ----- bounds ---------------------------------------------------------------------------

    /// Everything a bound brings with it: `Ord` brings `Eq`, `Copy` brings `Clone`, and an interface brings what
    /// it requires.
    pub(crate) fn bound_closure(&self, bound: Bound) -> Vec<Bound> {
        let mut out = Vec::new();
        let mut stack = vec![bound];
        while let Some(b) = stack.pop() {
            if out.contains(&b) {
                continue;
            }
            out.push(b);
            match b {
                Bound::Ord => stack.push(Bound::Eq),
                Bound::Copy => stack.push(Bound::Clone),
                Bound::User(i) => stack.extend(self.interfaces[i].requires.iter().copied()),
                _ => {}
            }
        }
        out
    }

    pub(crate) fn bound_name(&self, bound: Bound) -> String {
        match bound {
            Bound::User(i) => self.interfaces[i].name.clone(),
            Bound::Copy => "Copy".into(),
            Bound::Clone => "Clone".into(),
            Bound::Eq => "Eq".into(),
            Bound::Ord => "Ord".into(),
            Bound::Hash => "Hash".into(),
            Bound::Default => "Default".into(),
        }
    }

    /// Resolves a bound written in a generic parameter list, a `where` clause, `requires`, or a derive. A user
    /// interface may be named through a module (`T: sh.Describe`).
    pub(crate) fn resolve_bound(&self, ty: &ast::Type, module: usize, diags: &mut Diagnostics) -> Option<Bound> {
        let ast::TypeKind::Path(segs) = &ty.kind else {
            diags.error(Code::UNSUPPORTED, ty.span, "this kind of bound is not implemented in this compiler yet"); // e9000: bound-non-path
            return None;
        };
        let Some(first) = segs.first() else {
            diags.error(Code::UNSUPPORTED, ty.span, "this kind of bound is not implemented in this compiler yet"); // e9000: bound-empty
            return None;
        };
        // A plain name: a built-in interface, or one in scope.
        if segs.len() == 1 {
            let name = first.name.name.as_str();
            if !first.args.is_empty() {
                diags.error(Code::UNSUPPORTED, ty.span, "this kind of bound is not implemented in this compiler yet"); // e9000: bound-generic-args
                return None;
            }
            return Some(match name {
                "Copy" => Bound::Copy,
                "Clone" => Bound::Clone,
                "Eq" => Bound::Eq,
                "Ord" => Bound::Ord,
                "Hash" => Bound::Hash,
                "Default" => Bound::Default,
                "Drop" | "From" => {
                    diags.error(
                        Code::UNSUPPORTED, // e9000: bound-drop-from
                        ty.span,
                        format!("`{name}` as a bound is not implemented in this compiler yet"),
                    );
                    return None;
                }
                _ => match self.iface_name(module, name) {
                    Some(i) => Bound::User(i),
                    None => {
                        diags.error(Code::UNKNOWN_NAME, ty.span, format!("unknown interface `{name}`"));
                        return None;
                    }
                },
            });
        }
        let names: Vec<String> = segs[..segs.len() - 1].iter().map(|s| s.name.name.clone()).collect();
        let Some(target) = self.resolve_module_path(module, &names) else {
            diags.error(Code::UNKNOWN_NAME, ty.span, format!("unknown module `{}`", names.join(".")));
            return None;
        };
        let last = segs.last().expect("the path has a last segment");
        let name = last.name.name.as_str();
        if !last.args.is_empty() {
            diags.error(Code::UNSUPPORTED, ty.span, "this kind of bound is not implemented in this compiler yet"); // e9000: bound-qualified-generic
            return None;
        }
        if self.package_of(target) != self.package_of(module)
            && self.modules[target].iface_priv.get(name).copied().unwrap_or(false)
        {
            diags.error(Code::PRIVATE_ITEM, last.name.span, format!("interface `{name}` is `priv` in another package"));
        }
        match self.exported_iface(target, name) {
            Some(i) => Some(Bound::User(i)),
            None => {
                diags.error(Code::UNKNOWN_NAME, last.name.span, format!("unknown interface `{name}`"));
                None
            }
        }
    }

    /// Whether values of `ty` meet `bound`. `env` gives the bounds of the type parameters in scope.
    pub(crate) fn satisfies(&self, ty: &Ty, bound: Bound, env: &Env) -> bool {
        match ty {
            Ty::Error | Ty::Never => true,
            Ty::Param(name) => {
                env.iter().any(|(n, bs)| n == &**name && bs.iter().any(|b| self.bound_closure(*b).contains(&bound)))
            }
            _ => match bound {
                Bound::Copy => self.is_copy(ty),
                Bound::Clone => self.is_clone(ty),
                Bound::Eq => self.has_eq_in(ty, env),
                Bound::Ord => self.has_ord_in(ty, env),
                Bound::Hash => self.has_hash_in(ty, env),
                Bound::Default => self.has_default(ty),
                Bound::User(i) => self.implements(ty, i, env),
            },
        }
    }

    /// Whether a struct or enum has a `Type: Interface { ... }` block for a user interface, with its `where`
    /// clauses met.
    fn implements(&self, ty: &Ty, iface: usize, env: &Env) -> bool {
        let Some(key) = adt_key(ty) else { return false };
        self.impls.iter().any(|info| {
            if info.iface != iface {
                return false;
            }
            match info.head {
                Head::Plain(k) => k == key,
                Head::Generic(template) => match self.adt_origin.get(&key) {
                    Some((t, args)) if *t == template => info.wheres.iter().all(|(n, bounds)| {
                        match info.params.iter().position(|p| p == n).and_then(|i| args.get(i)) {
                            Some(arg) => bounds.iter().all(|b| self.satisfies(arg, *b, env)),
                            None => true,
                        }
                    }),
                    _ => false,
                },
            }
        })
    }

    /// Whether `<`, `>`, `<=`, and `>=` apply to values of `ty` through `Ord`.
    pub(crate) fn has_ord(&self, ty: &Ty) -> bool {
        self.has_ord_in(ty, &[])
    }

    /// `has_ord` with the bounds of the type parameters in scope (issue #69).
    pub(crate) fn has_ord_in(&self, ty: &Ty, env: &Env) -> bool {
        match ty {
            Ty::Unit | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Error | Ty::Never => true,
            Ty::Str | Ty::String => true,
            Ty::Param(name) => self.bound_in_env(name, Bound::Ord, env),
            Ty::Tuple(ts) => ts.iter().all(|t| self.has_ord_in(t, env)),
            Ty::Range(t, _) => self.has_ord_in(t, env),
            Ty::Struct(a) => {
                let s = &self.structs[a.id];
                s.derives.ord && s.fields.iter().all(|f| self.has_ord_in(&f.ty, env))
            }
            Ty::Enum(a) => {
                let e = &self.enums[a.id];
                e.derives.ord && e.variants.iter().all(|v| v.fields.iter().all(|f| self.has_ord_in(&f.ty, env)))
            }
            _ => false,
        }
    }

    /// Whether `x.hash()` applies to values of `ty`.
    pub(crate) fn has_hash(&self, ty: &Ty) -> bool {
        self.has_hash_in(ty, &[])
    }

    /// `has_hash` with the bounds of the type parameters in scope (issue #69).
    pub(crate) fn has_hash_in(&self, ty: &Ty, env: &Env) -> bool {
        match ty {
            Ty::Unit | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Error | Ty::Never => true,
            Ty::Param(name) => self.bound_in_env(name, Bound::Hash, env),
            Ty::Option(t) => self.has_hash_in(t, env),
            Ty::Range(t, _) => self.has_hash_in(t, env),
            Ty::Result(t, e) => self.has_hash_in(t, env) && self.has_hash_in(e, env),
            Ty::Tuple(ts) => ts.iter().all(|t| self.has_hash_in(t, env)),
            Ty::Struct(a) => {
                let s = &self.structs[a.id];
                s.derives.hash && s.fields.iter().all(|f| self.has_hash_in(&f.ty, env))
            }
            Ty::Enum(a) => {
                let e = &self.enums[a.id];
                e.derives.hash && e.variants.iter().all(|v| v.fields.iter().all(|f| self.has_hash_in(&f.ty, env)))
            }
            _ => false,
        }
    }

    /// Whether a type parameter's bounds provide `bound`.
    pub(crate) fn bound_in_env(&self, name: &str, bound: Bound, env: &Env) -> bool {
        env.iter().any(|(n, bounds)| n == name && bounds.iter().any(|b| self.bound_closure(*b).contains(&bound)))
    }

    /// Whether `Type.default()` applies.
    pub(crate) fn has_default(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Unit
            | Ty::Bool
            | Ty::Char
            | Ty::Int(_)
            | Ty::Float(_)
            | Ty::Str
            | Ty::String
            | Ty::Option(_)
            | Ty::Error
            | Ty::Never => true,
            Ty::Tuple(ts) => ts.iter().all(|t| self.has_default(t)),
            Ty::Struct(a) => {
                let s = &self.structs[a.id];
                s.derives.default && s.fields.iter().all(|f| self.has_default(&f.ty))
            }
            _ => false,
        }
    }

    /// The method `name` that the bounds of a type parameter provide: its parameters (the receiver first, if it
    /// has one) and result, with `Self` replaced by the parameter, whether it has a receiver, and whether it is
    /// `unsafe`.
    pub(crate) fn bound_method(&self, recv: &Ty, name: &str, env: &Env) -> Option<(Signature, bool, bool)> {
        let Ty::Param(pname) = recv else { return None };
        let self_env = [("Self".to_string(), recv.clone())];
        for (n, bounds) in env {
            if n != &**pname {
                continue;
            }
            for bound in bounds {
                for b in self.bound_closure(*bound) {
                    let Bound::User(i) = b else { continue };
                    if let Some(m) = self.interfaces[i].methods.iter().find(|m| m.name == name) {
                        let sig = &m.sig;
                        let params = sig.params.iter().map(|p| self.subst_param(p, &self_env)).collect();
                        let ret = self.subst_ty(&sig.ret, &self_env);
                        return Some((
                            Signature { params, ret, has_self: sig.has_self, ret_origin: None },
                            sig.has_self,
                            m.is_unsafe,
                        ));
                    }
                }
            }
        }
        None
    }

    /// `Self`-free spelling of a type for names of instances: `Stack<i32>`.
    pub(crate) fn instance_name(name: &str, args: &[Ty]) -> String {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        format!("{name}<{}>", args.join(", "))
    }
}

pub(crate) fn param_ty(name: &str) -> Ty {
    Ty::Param(Rc::from(name))
}

/// The position of the span used for diagnostics that have no better place (the whole program).
pub(crate) fn nowhere(file: burdin_diag::FileId) -> Span {
    Span::new(file, 0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use burdin_tir::{AdtRef, IntTy};

    fn param(name: &str) -> Ty {
        param_ty(name)
    }

    fn i32_() -> Ty {
        Ty::Int(IntTy::I32)
    }

    fn bool_() -> Ty {
        Ty::Bool
    }

    fn opt(ty: Ty) -> Ty {
        Ty::Option(Box::new(ty))
    }

    fn bindings(pat: &Ty, act: &Ty) -> Vec<(String, Ty)> {
        let mut out = Vec::new();
        Globals::default().unify(pat, act, &mut out);
        out
    }

    /// A pattern, the actual type, and the bindings unifying them must produce.
    type Case = (Ty, Ty, Vec<(String, Ty)>);

    #[test]
    fn binds_a_parameter_to_the_actual_type() {
        assert_eq!(bindings(&param("T"), &i32_()), vec![("T".to_string(), i32_())]);
        // A parameter that already stands for a type is not a binding site.
        assert_eq!(bindings(&i32_(), &i32_()), vec![]);
        assert_eq!(bindings(&param("T"), &Ty::Str), vec![("T".to_string(), Ty::Str)]);
    }

    #[test]
    fn binds_parameters_in_nested_positions() {
        let cases: Vec<Case> = vec![
            (opt(param("T")), opt(i32_()), vec![("T".into(), i32_())]),
            (
                Ty::Result(Box::new(param("T")), Box::new(param("E"))),
                Ty::Result(Box::new(i32_()), Box::new(bool_())),
                vec![("T".into(), i32_()), ("E".into(), bool_())],
            ),
            (Ty::List(Box::new(param("T"))), Ty::List(Box::new(bool_())), vec![("T".into(), bool_())]),
            (
                Ty::Map(Box::new(param("K")), Box::new(param("V"))),
                Ty::Map(Box::new(Ty::Str), Box::new(i32_())),
                vec![("K".into(), Ty::Str), ("V".into(), i32_())],
            ),
            (Ty::Heap(Box::new(param("T"))), Ty::Heap(Box::new(i32_())), vec![("T".into(), i32_())]),
            (Ty::CPtr(Box::new(param("T"))), Ty::CPtr(Box::new(bool_())), vec![("T".into(), bool_())]),
            (Ty::Slice(Box::new(param("T"))), Ty::Slice(Box::new(Ty::Str)), vec![("T".into(), Ty::Str)]),
            (Ty::Array(Box::new(param("T")), 3), Ty::Array(Box::new(i32_()), 3), vec![("T".into(), i32_())]),
            (
                Ty::Ref { mutable: true, inner: Box::new(param("T")) },
                Ty::Ref { mutable: true, inner: Box::new(bool_()) },
                vec![("T".into(), bool_())],
            ),
            (
                Ty::Tuple(vec![param("A"), param("B")]),
                Ty::Tuple(vec![i32_(), Ty::Str]),
                vec![("A".into(), i32_()), ("B".into(), Ty::Str)],
            ),
        ];
        for (pat, act, want) in cases {
            assert_eq!(bindings(&pat, &act), want, "unifying {pat} with {act}");
        }
    }

    #[test]
    fn keeps_the_first_binding_of_a_parameter() {
        let mut out = Vec::new();
        let pat = Ty::Tuple(vec![param("T"), param("T")]);
        let act = Ty::Tuple(vec![i32_(), bool_()]);
        Globals::default().unify(&pat, &act, &mut out);
        assert_eq!(out, vec![("T".to_string(), i32_())]);
    }

    #[test]
    fn error_and_never_actuals_bind_nothing() {
        for actual in [Ty::Error, Ty::Never] {
            let mut out = Vec::new();
            Globals::default().unify(&param("T"), &actual, &mut out);
            assert!(out.is_empty(), "{actual}");
            let mut out = Vec::new();
            Globals::default().unify(&opt(param("T")), &opt(actual), &mut out);
            assert!(out.is_empty());
        }
    }

    #[test]
    fn mismatched_shapes_bind_nothing() {
        assert!(bindings(&opt(param("T")), &Ty::List(Box::new(i32_()))).is_empty());
        assert!(bindings(&Ty::Tuple(vec![param("T")]), &Ty::Tuple(vec![i32_(), bool_()])).is_empty());
        assert!(bindings(&Ty::List(Box::new(param("T"))), &Ty::Map(Box::new(i32_()), Box::new(i32_()))).is_empty());
    }

    #[test]
    fn struct_and_enum_instances_unify_through_their_template() {
        let mut g = Globals::default();
        let pattern = Ty::Struct(AdtRef { id: 0, name: Rc::from("Pair<T>"), second_class: false });
        let actual = Ty::Struct(AdtRef { id: 1, name: Rc::from("Pair<i32>"), second_class: false });
        g.adt_origin.insert((false, 0), (7, vec![param("T")]));
        g.adt_origin.insert((false, 1), (7, vec![i32_()]));
        let mut out = Vec::new();
        g.unify(&pattern, &actual, &mut out);
        assert_eq!(out, vec![("T".to_string(), i32_())]);

        // A different template never unifies, even when both sides are structs.
        let other = Ty::Struct(AdtRef { id: 2, name: Rc::from("Other<i32>"), second_class: false });
        g.adt_origin.insert((false, 2), (8, vec![i32_()]));
        let mut out = Vec::new();
        g.unify(&pattern, &other, &mut out);
        assert!(out.is_empty());

        let enum_pattern = Ty::Enum(AdtRef { id: 0, name: Rc::from("Opt<T>"), second_class: false });
        let enum_actual = Ty::Enum(AdtRef { id: 1, name: Rc::from("Opt<bool>"), second_class: false });
        g.adt_origin.insert((true, 0), (9, vec![param("T")]));
        g.adt_origin.insert((true, 1), (9, vec![bool_()]));
        let mut out = Vec::new();
        g.unify(&enum_pattern, &enum_actual, &mut out);
        assert_eq!(out, vec![("T".to_string(), bool_())]);
    }

    #[test]
    fn subst_replaces_parameters_deeply() {
        let g = Globals::default();
        let env = vec![("T".to_string(), i32_()), ("E".to_string(), bool_())];
        assert_eq!(g.subst_ty(&param("T"), &env), i32_());
        // A parameter that is not in the environment stays as it is.
        assert_eq!(g.subst_ty(&param("U"), &env), param("U"));
        let ty = Ty::Tuple(vec![
            param("T"),
            Ty::Result(Box::new(param("T")), Box::new(param("E"))),
            Ty::Map(Box::new(param("E")), Box::new(param("T"))),
            Ty::Ref { mutable: false, inner: Box::new(param("T")) },
        ]);
        let want = Ty::Tuple(vec![
            i32_(),
            Ty::Result(Box::new(i32_()), Box::new(bool_())),
            Ty::Map(Box::new(bool_()), Box::new(i32_())),
            Ty::Ref { mutable: false, inner: Box::new(i32_()) },
        ]);
        assert_eq!(g.subst_ty(&ty, &env), want);
    }

    #[test]
    fn ty_has_param_sees_through_constructors() {
        let mut g = Globals::default();
        assert!(g.ty_has_param(&param("T")));
        assert!(g.ty_has_param(&opt(param("T"))));
        assert!(g.ty_has_param(&Ty::Tuple(vec![i32_(), Ty::List(Box::new(param("T")))])));
        assert!(!g.ty_has_param(&opt(i32_())));
        assert!(!g.ty_has_param(&Ty::Str));

        let instance = Ty::Struct(AdtRef { id: 0, name: Rc::from("Box<T>"), second_class: false });
        g.adt_origin.insert((false, 0), (7, vec![param("T")]));
        assert!(g.ty_has_param(&instance));
        g.adt_origin.insert((false, 0), (7, vec![i32_()]));
        assert!(!g.ty_has_param(&instance));
    }

    #[test]
    fn instance_names_spell_their_arguments() {
        assert_eq!(Globals::instance_name("Stack", &[i32_()]), "Stack<i32>");
        assert_eq!(Globals::instance_name("Pair", &[i32_(), bool_()]), "Pair<i32, bool>");
    }

    #[test]
    fn bounds_carry_what_they_imply() {
        let mut g = Globals::default();
        assert_eq!(g.bound_closure(Bound::Ord), vec![Bound::Ord, Bound::Eq]);
        assert_eq!(g.bound_closure(Bound::Copy), vec![Bound::Copy, Bound::Clone]);
        assert_eq!(g.bound_closure(Bound::Hash), vec![Bound::Hash]);

        // A user interface brings what its `requires` clause lists.
        g.interfaces.push(InterfaceInfo {
            name: "Show".into(),
            module: 0,
            requires: vec![Bound::Eq],
            methods: Vec::new(),
        });
        assert_eq!(g.bound_closure(Bound::User(0)), vec![Bound::User(0), Bound::Eq]);
    }

    #[test]
    fn satisfies_reads_the_bounds_of_a_type_parameter() {
        let g = Globals::default();
        let env: &[(String, Vec<Bound>)] = &[("T".to_string(), vec![Bound::Ord])];
        assert!(g.satisfies(&param("T"), Bound::Ord, env));
        assert!(g.satisfies(&param("T"), Bound::Eq, env), "Ord implies Eq");
        assert!(!g.satisfies(&param("T"), Bound::Hash, env));
        assert!(!g.satisfies(&param("U"), Bound::Copy, env));

        // Concrete types answer from their own rules; `Error` and `Never` satisfy everything.
        assert!(g.satisfies(&i32_(), Bound::Copy, &[]));
        assert!(g.satisfies(&opt(i32_()), Bound::Hash, &[]));
        assert!(!g.satisfies(&Ty::String, Bound::Copy, &[]));
        assert!(g.satisfies(&Ty::Error, Bound::Hash, &[]));
        assert!(g.satisfies(&Ty::Never, Bound::Copy, &[]));
    }
}
