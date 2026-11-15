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
use burdin_tir::{self as tir, FnKind, FnSig, FuncId, ParamSig, Signature, Ty};
use std::rc::Rc;

/// A requirement on a type parameter: a built-in interface, a user interface, or an `Fn`-family closure bound.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Bound {
    /// A user interface with its type arguments (`Iterator<i32>`). `Copy`-style bounds below carry no arguments.
    User(usize, Vec<Ty>),
    Copy,
    Clone,
    Eq,
    Ord,
    Hash,
    Default,
    /// A closure capability: `Fn(A, B): R`, `FnMut(...)`, or `FnOnce(...)`. The index selects the signature in
    /// [`Globals::fn_bounds`].
    Fn(usize, tir::FnKind),
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
    /// A method with its own type parameters; its signature is compared only structurally.
    pub generic: bool,
}

pub(crate) struct InterfaceInfo {
    pub name: String,
    /// The module that declares the interface, whose names its bounds and default bodies use.
    pub module: usize,
    /// The interface's own type parameters (`iterator<T>` has one named `T`), as `Ty::Param` in its signatures.
    pub params: Vec<String>,
    pub bounds: Vec<Vec<Bound>>,
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
    /// The interface's type arguments, resolved in the impl's scope (`T` in `Wrapper<T>: Iterator<T>`).
    pub iface_args: Vec<Ty>,
    /// The names the impl gives the type's arguments (`T` in `Wrapper<T>: Describe where T: Describe`).
    pub params: Vec<String>,
    pub wheres: Vec<(String, Vec<Bound>)>,
    pub span: Span,
}

/// A generic method, as the checker needs it: which template instantiates it, its identity function (whose
/// signature is the inference pattern), and its own type parameters and bounds.
#[derive(Clone)]
pub(crate) struct GenericMethod {
    pub(crate) template: usize,
    pub(crate) identity: FuncId,
    pub(crate) params: Vec<String>,
    pub(crate) bounds: Vec<Vec<Bound>>,
}

pub(crate) enum Request {
    Adt(usize, Vec<Ty>),
    Func(usize, Vec<Ty>),
    /// An instance of the generic method whose template is [`crate::session::Templates::methods`] entry `usize`,
    /// for these method-level type arguments.
    Method(usize, Vec<Ty>),
    /// The default method of an interface for a built-in receiver type that has no impl block; the next pass
    /// creates its template from the interface declaration.
    BuiltinMethodTemplate {
        iface: usize,
        method: usize,
        self_ty: Ty,
        iface_args: Vec<Ty>,
    },
}

/// The function id that stands for a call inside symbolic code, which has no code to call.
pub(crate) const SYMBOLIC_CALL: FuncId = FuncId(usize::MAX);

/// The `where` clauses of one impl block: each parameter name and the bounds it must meet.
type Wheres<'a> = [(String, Vec<Bound>)];

impl Globals {
    /// E0141: the arguments of a generic type instantiation must meet the bounds on its parameters. An argument
    /// that still mentions a type parameter is left to the symbolic check of the enclosing body.
    pub(crate) fn check_adt_bounds(&self, template: usize, args: &[Ty], span: Span, diags: &mut Diagnostics) {
        let info = &self.generic_adts[template];
        let subst: Vec<(String, Ty)> = info.params.iter().cloned().zip(args.iter().cloned()).collect();
        for ((pname, bounds), arg) in info.params.iter().zip(&info.bounds).zip(args) {
            if self.ty_has_param(arg) {
                continue;
            }
            for bound in bounds {
                if !self.satisfies_with(arg, bound, &[], &subst) {
                    diags.error(
                        Code::BOUND_NOT_SATISFIED,
                        span,
                        format!(
                            "`{arg}` does not implement `{}`, which `{pname}` of `{}` requires",
                            self.bound_name(bound),
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
            Ty::Fn { sig, .. } => sig.params.iter().any(|t| self.ty_has_param(t)) || self.ty_has_param(&sig.ret),
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
            (Ty::Fn { sig: psig, kind: pk, borrowed: pb }, Ty::Fn { sig: asig, kind: ak, borrowed: ab })
                if pk.at_least(*ak) && pb == ab && psig.params.len() == asig.params.len() =>
            {
                for (p, a) in psig.params.iter().zip(&asig.params) {
                    self.unify(p, a, bindings);
                }
                self.unify(&psig.ret, &asig.ret, bindings);
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
            Ty::Fn { sig, kind, borrowed } => Ty::Fn {
                sig: Rc::new(FnSig {
                    params: sig.params.iter().map(|t| self.subst_ty(t, env)).collect(),
                    ret: Box::new(self.subst_ty(&sig.ret, env)),
                }),
                kind: *kind,
                borrowed: *borrowed,
            },
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
    /// it requires, with the interface's type parameters replaced by the bound's arguments.
    pub(crate) fn bound_closure(&self, bound: &Bound) -> Vec<Bound> {
        let mut out = Vec::new();
        let mut stack = vec![bound.clone()];
        while let Some(b) = stack.pop() {
            if out.contains(&b) {
                continue;
            }
            match &b {
                Bound::Ord => stack.push(Bound::Eq),
                Bound::Copy => stack.push(Bound::Clone),
                Bound::User(i, args) => {
                    let iface = &self.interfaces[*i];
                    let subst: Vec<(String, Ty)> = iface.params.iter().cloned().zip(args.iter().cloned()).collect();
                    for r in &iface.requires {
                        stack.push(self.subst_bound(r, &subst));
                    }
                }
                _ => {}
            }
            out.push(b);
        }
        out
    }

    /// A bound with the type parameters in `subst` replaced (a user interface's arguments).
    pub(crate) fn subst_bound(&self, bound: &Bound, subst: &[(String, Ty)]) -> Bound {
        match bound {
            Bound::User(i, args) => Bound::User(*i, args.iter().map(|a| self.subst_ty(a, subst)).collect()),
            other => other.clone(),
        }
    }

    pub(crate) fn bound_name(&self, bound: &Bound) -> String {
        match bound {
            Bound::User(i, args) => {
                let name = &self.interfaces[*i].name;
                if args.is_empty() {
                    name.clone()
                } else {
                    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                    format!("{name}<{}>", args.join(", "))
                }
            }
            Bound::Copy => "Copy".into(),
            Bound::Clone => "Clone".into(),
            Bound::Eq => "Eq".into(),
            Bound::Ord => "Ord".into(),
            Bound::Hash => "Hash".into(),
            Bound::Default => "Default".into(),
            Bound::Fn(i, kind) => self.fn_sig_name(*i, *kind),
        }
    }

    /// Whether a bound in scope (`have`) provides the required bound (`want`). User interfaces compare their
    /// type arguments; closure bounds compare their signatures after `subst`.
    fn bound_provides(&self, have: &Bound, want: &Bound, subst: &[(String, Ty)]) -> bool {
        match (have, want) {
            (Bound::User(i, a), Bound::User(j, b)) => i == j && self.bound_args_match(a, b),
            (Bound::Fn(hi, hk), Bound::Fn(wi, wk)) => {
                hk.at_least(*wk) && self.bound_fn_sig(*hi, subst) == self.bound_fn_sig(*wi, subst)
            }
            (h, w) => h == w,
        }
    }

    /// Whether two interface argument lists describe the same types. An unresolved `Error` matches anything so a
    /// reported mistake does not cascade.
    pub(crate) fn bound_args_match(&self, a: &[Ty], b: &[Ty]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x == y || *x == Ty::Error || *y == Ty::Error)
    }

    /// `Fn(A, B): R` for a closure bound signature.
    fn fn_sig_name(&self, index: usize, kind: FnKind) -> String {
        let sig = &self.fn_bounds.borrow()[index];
        let params: Vec<String> = sig.params.iter().map(|p| p.to_string()).collect();
        format!("{}({}): {}", kind.name(), params.join(", "), sig.ret)
    }

    /// The signature a closure bound stands for, with the type parameters in `subst` replaced.
    pub(crate) fn bound_fn_sig(&self, index: usize, subst: &[(String, Ty)]) -> FnSig {
        let sig = self.fn_bounds.borrow()[index].clone();
        FnSig {
            params: sig.params.iter().map(|p| self.subst_ty(p, subst)).collect(),
            ret: Box::new(self.subst_ty(&sig.ret, subst)),
        }
    }

    /// The signature a call through `ty` uses, when its type or its bounds provide one.
    pub(crate) fn fn_call_sig(&self, ty: &Ty, env: &Env) -> Option<(FnSig, FnKind)> {
        match ty {
            Ty::Fn { sig, kind, .. } => Some(((**sig).clone(), *kind)),
            Ty::Param(name) => env.iter().find(|(n, _)| n == &**name).and_then(|(_, bounds)| {
                bounds.iter().find_map(|b| match b {
                    Bound::Fn(i, kind) => Some((self.bound_fn_sig(*i, &[]), *kind)),
                    _ => None,
                })
            }),
            _ => None,
        }
    }

    /// Resolves a bound written in a generic parameter list, a `where` clause, `requires`, or a derive. A user
    /// interface may be named through a module (`T: sh.Describe`), and `Fn(...)`/`FnMut(...)`/`FnOnce(...)` are the
    /// closure bounds. `subst` is what the enclosing declaration's type parameters stand for.
    pub(crate) fn resolve_bound(
        &self,
        ty: &ast::Type,
        subst: &[(String, Ty)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> Option<Bound> {
        if let ast::TypeKind::Fn { kind, params, ret } = &ty.kind {
            let params: Vec<Ty> = params.iter().map(|p| self.resolve_type_in(p, None, subst, module, diags)).collect();
            let ret = ret.as_ref().map(|r| self.resolve_type_in(r, None, subst, module, diags)).unwrap_or(Ty::Unit);
            let sig = FnSig { params, ret: Box::new(ret) };
            let mut bounds = self.fn_bounds.borrow_mut();
            let index = bounds.iter().position(|b| *b == sig).unwrap_or_else(|| {
                bounds.push(sig);
                bounds.len() - 1
            });
            return Some(Bound::Fn(index, ast_kind(*kind)));
        }
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
            let builtin = match name {
                "Copy" => Some(Bound::Copy),
                "Clone" => Some(Bound::Clone),
                "Eq" => Some(Bound::Eq),
                "Ord" => Some(Bound::Ord),
                "Hash" => Some(Bound::Hash),
                "Default" => Some(Bound::Default),
                "Drop" | "From" => {
                    diags.error(
                        Code::UNSUPPORTED, // e9000: bound-drop-from
                        ty.span,
                        format!("`{name}` as a bound is not implemented in this compiler yet"),
                    );
                    return None;
                }
                _ => None,
            };
            if let Some(bound) = builtin {
                if !first.args.is_empty() {
                    diags.error(Code::TYPE_MISMATCH, ty.span, format!("`{name}` takes no type arguments"));
                    return None;
                }
                return Some(bound);
            }
            let found = match self.iface_name(module, name) {
                Some(i) => i,
                None => {
                    diags.error(Code::UNKNOWN_NAME, ty.span, format!("unknown interface `{name}`"));
                    return None;
                }
            };
            return self.check_generic_bound(found, &first.args, subst, module, ty.span, diags);
        }
        let names: Vec<String> = segs[..segs.len() - 1].iter().map(|s| s.name.name.clone()).collect();
        let Some(target) = self.resolve_module_path(module, &names) else {
            diags.error(Code::UNKNOWN_NAME, ty.span, format!("unknown module `{}`", names.join(".")));
            return None;
        };
        let last = segs.last().expect("the path has a last segment");
        let name = last.name.name.as_str();
        if self.package_of(target) != self.package_of(module)
            && self.modules[target].iface_priv.get(name).copied().unwrap_or(false)
        {
            diags.error(Code::PRIVATE_ITEM, last.name.span, format!("interface `{name}` is `priv` in another package"));
        }
        match self.exported_iface(target, name) {
            Some(i) => self.check_generic_bound(i, &last.args, subst, target, ty.span, diags),
            None => {
                diags.error(Code::UNKNOWN_NAME, last.name.span, format!("unknown interface `{name}`"));
                None
            }
        }
    }

    /// Resolves the type arguments of a generic bound (`Iterator<i32>`) and checks their number. An interface
    /// without parameters takes no arguments.
    fn check_generic_bound(
        &self,
        iface: usize,
        args: &[ast::Type],
        subst: &[(String, Ty)],
        module: usize,
        span: Span,
        diags: &mut Diagnostics,
    ) -> Option<Bound> {
        let info = &self.interfaces[iface];
        if args.len() != info.params.len() {
            diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`{}` takes {} type argument(s), but {} were given", info.name, info.params.len(), args.len()),
            );
            return None;
        }
        let args: Vec<Ty> = args.iter().map(|a| self.resolve_type_in(a, None, subst, module, diags)).collect();
        Some(Bound::User(iface, args))
    }

    /// Whether values of `ty` meet `bound`. `env` gives the bounds of the type parameters in scope.
    pub(crate) fn satisfies(&self, ty: &Ty, bound: &Bound, env: &Env) -> bool {
        self.satisfies_with(ty, bound, env, &[])
    }

    /// `satisfies`, with the type parameters of the enclosing declaration replaced by `subst`. A closure bound's
    /// signature may name other type parameters (`F: Fn(T): T`), so a call checks the substituted signature.
    pub(crate) fn satisfies_with(&self, ty: &Ty, bound: &Bound, env: &Env, subst: &[(String, Ty)]) -> bool {
        let want = self.subst_bound(bound, subst);
        match ty {
            Ty::Error | Ty::Never => true,
            Ty::Param(name) => env.iter().any(|(n, bs)| {
                n == &**name
                    && bs
                        .iter()
                        .any(|b| self.bound_closure(b).iter().any(|have| self.bound_provides(have, &want, subst)))
            }),
            _ => match want {
                Bound::Copy => self.is_copy(ty),
                Bound::Clone => self.is_clone(ty),
                Bound::Eq => self.has_eq_in(ty, env),
                Bound::Ord => self.has_ord_in(ty, env),
                Bound::Hash => self.has_hash_in(ty, env),
                Bound::Default => self.has_default(ty),
                Bound::User(i, args) => self.implements(ty, i, &args, env),
                Bound::Fn(i, kind) => match ty {
                    Ty::Fn { sig, kind: actual, .. } => **sig == self.bound_fn_sig(i, subst) && actual.at_least(kind),
                    _ => false,
                },
            },
        }
    }

    /// Whether a struct or enum has a `Type: Interface<Args> { ... }` block for a user interface, with its `where`
    /// clauses met. A built-in collection satisfies the standard iterator protocol without an impl block.
    fn implements(&self, ty: &Ty, iface: usize, args: &[Ty], env: &Env) -> bool {
        if (Some(iface) == self.display_iface || Some(iface) == self.debug_iface)
            && let Some(answer) = self.builtin_formattable(ty, Some(iface) == self.debug_iface, iface, env)
        {
            return answer;
        }
        if let Some(have) = self.builtin_iface_args(ty, iface, env) {
            return self.bound_args_match(&have, args);
        }
        let Some(key) = adt_key(ty) else { return false };
        self.impls.iter().any(|info| {
            if info.iface != iface {
                return false;
            }
            let (subst, wheres): (Vec<(String, Ty)>, &Wheres<'_>) = match info.head {
                Head::Plain(k) => {
                    if k != key {
                        return false;
                    }
                    (Vec::new(), &info.wheres)
                }
                Head::Generic(template) => match self.adt_origin.get(&key) {
                    Some((t, cargs)) if *t == template => {
                        (info.params.iter().cloned().zip(cargs.iter().cloned()).collect(), &info.wheres)
                    }
                    _ => return false,
                },
            };
            let have: Vec<Ty> = info.iface_args.iter().map(|a| self.subst_ty(a, &subst)).collect();
            if !self.bound_args_match(&have, args) {
                return false;
            }
            wheres.iter().all(|(n, bounds)| match info.params.iter().position(|p| p == n).and_then(|i| subst.get(i)) {
                // A `where` bound may name the impl's other parameters (`F: Fn(T): U`), so the check uses them.
                Some((_, arg)) => bounds.iter().all(|b| self.satisfies_with(arg, b, env, &subst)),
                None => true,
            })
        })
    }

    /// Whether a built-in type implements `Display` or (`debug`) `Debug` without an impl block, or `None` for a
    /// struct or enum, which implements them with a block or a derive (spec/format.md).
    fn builtin_formattable(&self, ty: &Ty, debug: bool, iface: usize, env: &Env) -> Option<bool> {
        let part = |t: &Ty| self.satisfies_with(t, &Bound::User(iface, Vec::new()), env, &[]);
        Some(match ty {
            Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Char | Ty::Str | Ty::String | Ty::Error | Ty::Never => true,
            Ty::Ref { inner, .. } => part(inner),
            Ty::Unit => debug,
            Ty::Option(t) | Ty::List(t) | Ty::Heap(t) => debug && part(t),
            Ty::Result(t, e) => debug && part(t) && part(e),
            Ty::Tuple(ts) => debug && (2..=4).contains(&ts.len()) && ts.iter().all(part),
            Ty::Struct(_) | Ty::Enum(_) => return None,
            _ => false,
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
            Ty::Param(name) => self.bound_in_env(name, &Bound::Ord, env),
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

    /// Whether values of `ty` implement `Debug`, for the derive requirement.
    pub(crate) fn has_debug(&self, ty: &Ty) -> bool {
        match self.debug_iface {
            Some(iface) => self.satisfies(ty, &Bound::User(iface, Vec::new()), &[]),
            None => true,
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
            Ty::Param(name) => self.bound_in_env(name, &Bound::Hash, env),
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
    pub(crate) fn bound_in_env(&self, name: &str, bound: &Bound, env: &Env) -> bool {
        env.iter().any(|(n, bounds)| {
            n == name
                && bounds.iter().any(|b| self.bound_closure(b).iter().any(|have| self.bound_provides(have, bound, &[])))
        })
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
        for (n, bounds) in env {
            if n != &**pname {
                continue;
            }
            for bound in bounds {
                for b in self.bound_closure(bound) {
                    let Bound::User(i, args) = b else { continue };
                    let iface = &self.interfaces[i];
                    if let Some(m) = iface.methods.iter().find(|m| m.name == name) {
                        // `Self` is the receiver and the interface's parameters are the bound's arguments.
                        let mut subst: Vec<(String, Ty)> = vec![("Self".to_string(), recv.clone())];
                        subst.extend(iface.params.iter().cloned().zip(args.iter().cloned()));
                        let sig = &m.sig;
                        let params = sig.params.iter().map(|p| self.subst_param(p, &subst)).collect();
                        let ret = self.subst_ty(&sig.ret, &subst);
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

/// The typed-tree capability of a written closure type or bound.
pub(crate) fn ast_kind(kind: ast::FnKind) -> FnKind {
    match kind {
        ast::FnKind::Fn => FnKind::Fn,
        ast::FnKind::FnMut => FnKind::FnMut,
        ast::FnKind::FnOnce => FnKind::FnOnce,
    }
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
        assert_eq!(g.bound_closure(&Bound::Ord), vec![Bound::Ord, Bound::Eq]);
        assert_eq!(g.bound_closure(&Bound::Copy), vec![Bound::Copy, Bound::Clone]);
        assert_eq!(g.bound_closure(&Bound::Hash), vec![Bound::Hash]);

        // A user interface brings what its `requires` clause lists.
        g.interfaces.push(InterfaceInfo {
            name: "Show".into(),
            module: 0,
            params: Vec::new(),
            bounds: Vec::new(),
            requires: vec![Bound::Eq],
            methods: Vec::new(),
        });
        assert_eq!(g.bound_closure(&Bound::User(0, Vec::new())), vec![Bound::User(0, Vec::new()), Bound::Eq]);
    }

    #[test]
    fn satisfies_reads_the_bounds_of_a_type_parameter() {
        let g = Globals::default();
        let env: &[(String, Vec<Bound>)] = &[("T".to_string(), vec![Bound::Ord])];
        assert!(g.satisfies(&param("T"), &Bound::Ord, env));
        assert!(g.satisfies(&param("T"), &Bound::Eq, env), "Ord implies Eq");
        assert!(!g.satisfies(&param("T"), &Bound::Hash, env));
        assert!(!g.satisfies(&param("U"), &Bound::Copy, env));

        // Concrete types answer from their own rules; `Error` and `Never` satisfy everything.
        assert!(g.satisfies(&i32_(), &Bound::Copy, &[]));
        assert!(g.satisfies(&opt(i32_()), &Bound::Hash, &[]));
        assert!(!g.satisfies(&Ty::String, &Bound::Copy, &[]));
        assert!(g.satisfies(&Ty::Error, &Bound::Hash, &[]));
        assert!(g.satisfies(&Ty::Never, &Bound::Copy, &[]));
    }
}
