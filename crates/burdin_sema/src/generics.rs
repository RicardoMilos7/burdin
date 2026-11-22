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
use burdin_tir::{self as tir, AnyRef, FnKind, FnSig, FuncId, ParamSig, Signature, Ty};
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
    /// `From<S>`: a value of the source type `S` converts into the bounded type, through a built-in pair or a `From`
    /// block (`spec/stdlib.md`, FR7).
    From(Ty),
    /// A closure capability: `Fn(A, B): R`, `FnMut(...)`, or `FnOnce(...)`. The index selects the signature in
    /// [`Globals::fn_bounds`].
    Fn(usize, tir::FnKind),
    /// The parameter is a const parameter of this type (`const N: usize`); it has no interfaces to meet
    /// (`spec/const-generics.md`).
    ConstOf(Ty),
}

pub(crate) const BUILTIN_INTERFACES: &[&str] =
    &["Copy", "Clone", "Eq", "Ord", "Hash", "Default", "Drop", "From", "TryFrom"];

/// The bounds of each type parameter in scope: `(name, bounds)`. A name may appear more than once.
pub(crate) type Env = [(String, Vec<Bound>)];

/// A method of a bound that has type parameters of its own (see `Globals::bound_method_generics`): the names of those
/// parameters, their bounds, and the bound's own arguments for the interface's parameters.
pub(crate) type BoundMethodGenerics = (Vec<String>, Vec<Vec<Bound>>, Vec<(String, Ty)>);

pub(crate) struct IfaceMethod {
    pub name: String,
    /// Written with `Self` as `Ty::Param("Self")`.
    pub sig: Signature,
    pub has_default: bool,
    /// An `unsafe fn` of the interface: calling it requires an `unsafe` context.
    pub is_unsafe: bool,
    /// A method with its own type parameters; its signature is compared only structurally.
    pub generic: bool,
    /// The method's own type parameters (`fn pair<U, F: FnMut(T): U>` has `U` and `F`), in order.
    pub params: Vec<String>,
    /// The bounds of each of `params`, written with the interface's parameters as `Ty::Param`. A bound reached through
    /// a type parameter's bound has them replaced by that bound's arguments (see `bound_method_generics`).
    pub bounds: Vec<Vec<Bound>>,
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
    /// The default type of each parameter, when it was written (`struct Pool<A: Allocator = Global>`), as a
    /// type expression resolved when a use leaves the argument out (`spec/allocators.md`, AL5).
    pub defaults: Vec<Option<ast::Type>>,
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
    /// The receiver type of a generic extension method, with the parameters as `Ty::Param`; `None` for an
    /// ordinary generic function. An instance's `self` type is this with the arguments substituted.
    pub recv: Option<Ty>,
    /// Whether a parameter is written `impl I`, which adds a hidden type parameter the call infers: such a
    /// function takes no explicit type arguments (`spec/arguments.md`, TA4).
    pub hidden: bool,
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
    /// The type's arguments as the impl writes them, with the impl's own names as `Ty::Param` (`[i32, B]` for
    /// `Pair<i32, B>`). The impl applies to the instances these match (`spec/impls.md`, IM1); empty for a type that
    /// is not generic.
    pub args: Vec<Ty>,
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
    /// Whether a parameter is written `impl I` (see [`GenericFn::hidden`]).
    pub(crate) hidden: bool,
}

#[derive(Clone, Debug)]
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
    ///
    /// Returns `false` when a `Send` or `Sync` bound is not met, so the caller does not create an instance whose
    /// methods would report the same failure again from inside the standard library.
    pub(crate) fn check_adt_bounds(&self, template: usize, args: &[Ty], span: Span, diags: &mut Diagnostics) -> bool {
        let mut ok = true;
        let info = &self.generic_adts[template];
        let subst: Vec<(String, Ty)> = info.params.iter().cloned().zip(args.iter().cloned()).collect();
        for ((pname, bounds), arg) in info.params.iter().zip(&info.bounds).zip(args) {
            if self.ty_has_param(arg) {
                continue;
            }
            for bound in bounds {
                if !self.satisfies_with(arg, bound, &[], &subst) {
                    let (code, message) = self.bound_unmet(arg, bound, &[], &format!("`{pname}` of `{}`", info.name));
                    diags.error(code, span, message);
                    ok &= code != Code::NOT_SEND;
                }
            }
        }
        ok
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
            Ty::Const(v) => *v >= crate::consteval::SYM_CONST,
            Ty::Array(inner, n) => *n >= crate::consteval::SYM_LEN || self.ty_has_param(inner),
            Ty::Ref { inner, .. }
            | Ty::Slice(inner)
            | Ty::Option(inner)
            | Ty::CPtr(inner)
            | Ty::Range(inner, _)
            | Ty::Shared(inner) => self.ty_has_param(inner),
            Ty::Heap(inner, alloc) => {
                self.ty_has_param(inner) || alloc.allocator_ty().is_some_and(|a| self.ty_has_param(a))
            }
            Ty::List(inner, alloc) => {
                self.ty_has_param(inner) || alloc.allocator_ty().is_some_and(|a| self.ty_has_param(a))
            }
            Ty::Result(ok, err) => self.ty_has_param(ok) || self.ty_has_param(err),
            Ty::Map(k, v, alloc) | Ty::MapEntry(k, v, alloc) => {
                self.ty_has_param(k)
                    || self.ty_has_param(v)
                    || alloc.allocator_ty().is_some_and(|a| self.ty_has_param(a))
            }
            Ty::Fn { sig, .. } => sig.params.iter().any(|t| self.ty_has_param(t)) || self.ty_has_param(&sig.ret),
            Ty::Any(a) => a.args.iter().any(|t| self.ty_has_param(t)),
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
            | (Ty::Option(p), Ty::Option(a))
            | (Ty::Shared(p), Ty::Shared(a))
            | (Ty::CPtr(p), Ty::CPtr(a))
            | (Ty::Range(p, _), Ty::Range(a, _)) => self.unify(p, a, bindings),
            (Ty::Heap(p, pa), Ty::Heap(a, aa)) => {
                self.unify(p, a, bindings);
                self.unify_alloc(pa, aa, bindings);
            }
            (Ty::List(p, pa), Ty::List(a, aa)) => {
                self.unify(p, a, bindings);
                self.unify_alloc(pa, aa, bindings);
            }
            (Ty::Array(p, pn), Ty::Array(a, an)) => {
                self.unify(p, a, bindings);
                self.unify_len(*pn, *an, bindings);
            }
            (Ty::Result(po, pe), Ty::Result(ao, ae)) => {
                self.unify(po, ao, bindings);
                self.unify(pe, ae, bindings);
            }
            (Ty::Map(pk, pv, pa), Ty::Map(ak, av, aa)) | (Ty::MapEntry(pk, pv, pa), Ty::MapEntry(ak, av, aa)) => {
                self.unify(pk, ak, bindings);
                self.unify(pv, av, bindings);
                self.unify_alloc(pa, aa, bindings);
            }
            // A closure value is usable where the same signature at a lower capability is expected (CL5), so the
            // actual kind must be at least as capable as the declared one: an `Fn` closure meets `FnMut`/`FnOnce`.
            (Ty::Fn { sig: psig, kind: pk, borrowed: pb, .. }, Ty::Fn { sig: asig, kind: ak, borrowed: ab, .. })
                if ak.at_least(*pk) && pb == ab && psig.params.len() == asig.params.len() =>
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
            // `any I` instances unify when they name the same interface with the same arguments.
            (Ty::Any(p), Ty::Any(a)) if p.iface == a.iface && p.args.len() == a.args.len() => {
                for (p, a) in p.args.iter().zip(&a.args) {
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

    /// Matches the allocators of two container types. A pattern allocator parameter binds to the default
    /// allocator when the actual container stores none.
    fn unify_alloc(&self, pat: &burdin_tir::Alloc, act: &burdin_tir::Alloc, bindings: &mut Vec<(String, Ty)>) {
        match (pat.allocator_ty(), act.allocator_ty()) {
            (Some(p), Some(a)) => self.unify(p, a, bindings),
            (Some(p), None) => {
                if let Some(global) = &self.global_alloc_ty {
                    self.unify(p, global, bindings);
                }
            }
            (None, _) => {}
        }
    }

    /// Replaces type parameters by the types in `env`.
    pub(crate) fn subst_ty(&self, ty: &Ty, env: &[(String, Ty)]) -> Ty {
        let go = |t: &Ty| Box::new(self.subst_ty(t, env));
        match ty {
            Ty::Param(n) => env.iter().find(|(m, _)| m == &**n).map(|(_, t)| t.clone()).unwrap_or_else(|| ty.clone()),
            Ty::Ref { mutable, inner } => Ty::Ref { mutable: *mutable, inner: go(inner) },
            Ty::Slice(t) => Ty::Slice(go(t)),
            Ty::Array(t, n) if *n >= crate::consteval::SYM_LEN => match self.subst_len(*n, env) {
                Some(len) => Ty::Array(go(t), len),
                None => Ty::Error,
            },
            Ty::Array(t, n) => Ty::Array(go(t), *n),
            Ty::Const(v) if *v >= crate::consteval::SYM_CONST => self.subst_const(*v, env),
            Ty::Option(t) => Ty::Option(go(t)),
            Ty::Result(t, e) => Ty::Result(go(t), go(e)),
            Ty::List(t, a) => Ty::List(go(t), self.subst_alloc(a, env)),
            Ty::Map(k, v, a) => Ty::Map(go(k), go(v), self.subst_alloc(a, env)),
            Ty::MapEntry(k, v, a) => Ty::MapEntry(go(k), go(v), self.subst_alloc(a, env)),
            Ty::Heap(t, a) => Ty::Heap(go(t), self.subst_alloc(a, env)),
            Ty::Shared(t) => Ty::Shared(go(t)),
            Ty::CPtr(t) => Ty::CPtr(go(t)),
            Ty::Range(t, inclusive) => Ty::Range(go(t), *inclusive),
            Ty::Fn { sig, kind, borrowed, send } => Ty::Fn {
                sig: Rc::new(FnSig {
                    params: sig.params.iter().map(|t| self.subst_ty(t, env)).collect(),
                    ret: Box::new(self.subst_ty(&sig.ret, env)),
                }),
                kind: *kind,
                borrowed: *borrowed,
                send: send.clone(),
            },
            Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| self.subst_ty(t, env)).collect()),
            Ty::Any(a) => {
                let args: Vec<Ty> = a.args.iter().map(|t| self.subst_ty(t, env)).collect();
                if args == a.args { ty.clone() } else { Ty::Any(self.any_iface(a.iface, args)) }
            }
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
            Bound::From(source) => Bound::From(self.subst_ty(source, subst)),
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
            Bound::From(source) => format!("From<{source}>"),
            Bound::Fn(i, kind) => self.fn_sig_name(*i, *kind),
            Bound::ConstOf(ty) => format!("const {ty}"),
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
        // `any`/`impl` are types, not interfaces, and a bound names an interface.
        if let ast::TypeKind::Any(_) = &ty.kind {
            diags.error(
                Code::NOT_AN_INTERFACE,
                ty.span,
                "`any Interface` cannot be used as a bound; write the interface itself",
            );
            return None;
        }
        if let ast::TypeKind::Impl(_) = &ty.kind {
            diags.error(Code::IMPL_NOT_PARAMETER, ty.span, "`impl Interface` is only allowed on a parameter");
            return None;
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
            if name == "From" {
                if first.args.len() != 1 {
                    let message = format!("`From` takes 1 type argument, but {} were given", first.args.len());
                    diags.error(Code::TYPE_MISMATCH, ty.span, message);
                    return None;
                }
                let source = self.resolve_type_in(&first.args[0], None, subst, module, diags);
                return Some(Bound::From(source));
            }
            let builtin = match name {
                "Copy" => Some(Bound::Copy),
                "Clone" => Some(Bound::Clone),
                "Eq" => Some(Bound::Eq),
                "Ord" => Some(Bound::Ord),
                "Hash" => Some(Bound::Hash),
                "Default" => Some(Bound::Default),
                "Drop" | "TryFrom" => {
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

    /// Resolves `any Interface` (spec/existentials.md, AN2/AN12). The written interface is a type path with
    /// optional type arguments; anything that is not an interface is E0200, object safety is checked later by
    /// [`Globals::check_any_uses`], and the instance is interned so `Ty::Any` has a stable identity.
    pub(crate) fn resolve_any(
        &self,
        inner: &ast::Type,
        span: Span,
        subst: &[(String, Ty)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> Ty {
        let ast::TypeKind::Path(segs) = &inner.kind else {
            diags.error(Code::NOT_AN_INTERFACE, span, "`any` needs an interface name, not this type");
            return Ty::Error;
        };
        let Some(first) = segs.first() else {
            diags.error(Code::NOT_AN_INTERFACE, span, "`any` needs an interface name");
            return Ty::Error;
        };
        if segs.len() == 1 {
            let name = first.name.name.as_str();
            if BUILTIN_INTERFACES.contains(&name) {
                diags.error(
                    Code::NOT_AN_INTERFACE,
                    span,
                    format!("`{name}` is a built-in bound, not an interface; `any` needs an interface"),
                );
                return Ty::Error;
            }
            if subst.iter().any(|(n, _)| n == name) {
                diags.error(Code::NOT_AN_INTERFACE, span, format!("`{name}` is a type, not an interface"));
                return Ty::Error;
            }
            // A name may be both an interface and a type (`std.Error` is): `any Name` names the interface, so the
            // interface lookup comes first; only when there is no interface is the type rejection reported.
            if let Some(iface) = self.iface_name(module, name) {
                return self.any_type_of(iface, &first.args, subst, module, span, diags);
            }
            if self.type_name(module, name).is_some()
                || crate::builtins::builtin_type(name).is_some()
                || matches!(name, "Option" | "Result" | "List" | "Map" | "Heap" | "CPtr" | "Range" | "RangeInclusive")
            {
                diags.error(Code::NOT_AN_INTERFACE, span, format!("`{name}` is a type, not an interface"));
                return Ty::Error;
            }
            diags.error(Code::UNKNOWN_NAME, span, format!("unknown interface `{name}`"));
            return Ty::Error;
        }
        let names: Vec<String> = segs[..segs.len() - 1].iter().map(|s| s.name.name.clone()).collect();
        let Some(target) = self.resolve_module_path(module, &names) else {
            diags.error(Code::UNKNOWN_NAME, span, format!("unknown module `{}`", names.join(".")));
            return Ty::Error;
        };
        let last = segs.last().expect("the path has a last segment");
        let name = last.name.name.as_str();
        if self.package_of(target) != self.package_of(module)
            && self.modules[target].iface_priv.get(name).copied().unwrap_or(false)
        {
            diags.error(Code::PRIVATE_ITEM, span, format!("interface `{name}` is `priv` in another package"));
        }
        match self.exported_iface(target, name) {
            Some(iface) => self.any_type_of(iface, &last.args, subst, target, span, diags),
            None => {
                if self.exported_type(target, name).is_some() {
                    diags.error(Code::NOT_AN_INTERFACE, span, format!("`{name}` is a type, not an interface"));
                } else {
                    diags.error(Code::UNKNOWN_NAME, span, format!("unknown interface `{name}`"));
                }
                Ty::Error
            }
        }
    }

    /// The `Ty::Any` for interface `iface` with the written arguments, after interning the instance.
    fn any_type_of(
        &self,
        iface: usize,
        args: &[ast::Type],
        subst: &[(String, Ty)],
        module: usize,
        span: Span,
        diags: &mut Diagnostics,
    ) -> Ty {
        match self.check_generic_bound(iface, args, subst, module, span, diags) {
            Some(Bound::User(iface, args)) => {
                let any = self.any_iface(iface, args);
                self.record_any_use(&any, span);
                Ty::Any(any)
            }
            Some(_) => {
                diags.error(Code::NOT_AN_INTERFACE, span, "`any` needs an interface");
                Ty::Error
            }
            None => Ty::Error,
        }
    }

    /// Whether values of `ty` meet `bound`. `env` gives the bounds of the type parameters in scope.
    pub(crate) fn satisfies(&self, ty: &Ty, bound: &Bound, env: &Env) -> bool {
        self.satisfies_with(ty, bound, env, &[])
    }

    /// `satisfies`, with the type parameters of the enclosing declaration replaced by `subst`. A closure bound's
    /// signature may name other type parameters (`F: Fn(T): T`), so a call checks the substituted signature.
    /// Whether a value of `from` converts into `to` (FR7): a built-in pair (FR1), a `From` block of `to` whose source
    /// is `from`, or, for a type parameter `to`, a `From` bound in `env`.
    pub(crate) fn converts_into(&self, to: &Ty, from: &Ty, env: &Env) -> bool {
        if *from == Ty::Error {
            return true;
        }
        match to {
            Ty::Error => true,
            Ty::Param(name) => self.bound_in_env(name, &Bound::From(from.clone()), env),
            _ => {
                crate::fncx::conversion::lossless_from(to, from)
                    || self.conversions_into(to, "from").iter().any(|c| c.source == *from)
            }
        }
    }

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
                Bound::Ord => self.has_ord_in(ty, env) || self.implements_partial_ord(ty, env),
                Bound::Hash => self.has_hash_in(ty, env),
                Bound::Default => self.has_default(ty),
                Bound::From(source) => self.converts_into(ty, &source, env),
                Bound::User(i, args) => self.implements(ty, i, &args, env),
                Bound::Fn(i, kind) => match ty {
                    Ty::Fn { sig, kind: actual, .. } => **sig == self.bound_fn_sig(i, subst) && actual.at_least(kind),
                    _ => false,
                },
                Bound::ConstOf(_) => matches!(ty, Ty::Const(_)),
            },
        }
    }

    /// Whether a struct or enum has a `Type: Interface<Args> { ... }` block for a user interface, with its `where`
    /// clauses met. A built-in collection satisfies the standard iterator protocol without an impl block.
    fn implements(&self, ty: &Ty, iface: usize, args: &[Ty], env: &Env) -> bool {
        // `Send` and `Sync` are derived from the type's parts unless the type declares them itself
        // (`spec/shared.md`, SH4).
        if let Some(sync) = self.marker_kind(iface) {
            if matches!(ty, Ty::Struct(_) | Ty::Enum(_)) && self.has_impl_head(ty, iface) {
                return self.impl_blocks_cover(ty, iface, args, env);
            }
            return self.marker_violation(ty, sync, env).is_none();
        }
        // An `any I` satisfies `I` itself and every interface `I` requires, through its vtable (AN8).
        if let Ty::Any(a) = ty {
            return self.any_satisfies(a, iface, args);
        }
        if (Some(iface) == self.display_iface || Some(iface) == self.debug_iface)
            && let Some(answer) = self.builtin_formattable(ty, Some(iface) == self.debug_iface, iface, env)
        {
            return answer;
        }
        // The built-in types implement the serialization interfaces without a block (SER14).
        if (Some(iface) == self.serialize_iface || Some(iface) == self.deserialize_iface)
            && let Some(answer) = self.builtin_serial(ty, iface, Some(iface) == self.deserialize_iface, env)
        {
            return answer;
        }
        if Some(iface) == self.from_iterator_iface
            && let Some(answer) = self.builtin_from_iterator(ty, args, env)
        {
            return answer;
        }
        // A view satisfies what its target does: a chain is generic over the view type of its source, and its
        // bounds (`D: Index<T>`) ask about the borrowed collection (`spec/iterators.md` IT12). A mutable view
        // carries a mutable iterator's source, whose bounds (`D: IndexMut<T>`) do the same (IT15).
        if let Ty::Ref { inner, .. } = ty {
            return self.implements(inner, iface, args, env);
        }
        if let Some(have) = self.builtin_iface_args(ty, iface, env) {
            return self.bound_args_match(&have, args);
        }
        self.impl_blocks_cover(ty, iface, args, env)
    }

    /// Whether some impl block of `iface` covers the struct or enum `ty`, with its `where` clauses met.
    pub(crate) fn impl_blocks_cover(&self, ty: &Ty, iface: usize, args: &[Ty], env: &Env) -> bool {
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
                    Some((t, cargs)) if *t == template => match self.match_impl_args(&info.params, &info.args, cargs) {
                        Some(subst) => (subst, &info.wheres),
                        None => return false,
                    },
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

    /// Whether the box's interface is `iface`, or requires it with matching arguments (AN2, AN8).
    fn any_satisfies(&self, any: &AnyRef, iface: usize, args: &[Ty]) -> bool {
        let Some((stored_iface, stored_args)) = self.any.borrow().ifaces.get(any.id).cloned() else {
            return false;
        };
        let want = Bound::User(iface, args.to_vec());
        self.bound_closure(&Bound::User(stored_iface, stored_args))
            .iter()
            .any(|have| self.bound_provides(have, &want, &[]))
    }

    /// Whether a built-in type implements `Display` or (`debug`) `Debug` without an impl block, or `None` for a
    /// struct or enum, which implements them with a block or a derive (spec/format.md).
    fn builtin_formattable(&self, ty: &Ty, debug: bool, iface: usize, env: &Env) -> Option<bool> {
        let part = |t: &Ty| self.satisfies_with(t, &Bound::User(iface, Vec::new()), env, &[]);
        Some(match ty {
            Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Char | Ty::Str | Ty::String | Ty::Error | Ty::Never => true,
            Ty::Ref { inner, .. } => part(inner),
            Ty::Unit => debug,
            Ty::Option(t) | Ty::List(t, _) | Ty::Heap(t, _) => debug && part(t),
            Ty::Map(k, v, _) => debug && part(k) && part(v),
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
            // `None < Some(_)`, and `Ok(_) < Err(_)`; two of a kind compare their payloads
            // (`spec/stdlib.md`, "Interface coverage").
            Ty::Option(t) => self.has_ord_in(t, env),
            Ty::Result(t, e) => self.has_ord_in(t, env) && self.has_ord_in(e, env),
            Ty::Tuple(ts) => ts.iter().all(|t| self.has_ord_in(t, env)),
            // A fixed-size array orders lexicographically.
            Ty::Array(t, _) => self.has_ord_in(t, env),
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

    /// Whether a user type implements `PartialOrd` by hand. Such a type satisfies an `Ord` bound: its `lt` is the
    /// order, which is how `Reverse<T>` plugs into a container or a sort that asks for `T: Ord`. The built-in
    /// `<` still does not apply to it (`has_ord`), so a generic body instantiated for it calls `lt`.
    fn implements_partial_ord(&self, ty: &Ty, env: &Env) -> bool {
        self.op_ifaces.as_ref().is_some_and(|ops| self.implements(ty, ops.partial_ord, &[], env))
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
            // Text hashes its bytes (`spec/derives.md`, `Hash`).
            Ty::Str | Ty::String => true,
            Ty::Param(name) => self.bound_in_env(name, &Bound::Hash, env),
            Ty::Option(t) => self.has_hash_in(t, env),
            // A list hashes its elements in order and a map its entries in any order
            // (`spec/stdlib.md`, "Interface coverage").
            Ty::List(t, _) => self.has_hash_in(t, env),
            Ty::Map(k, v, _) => self.has_hash_in(k, env) && self.has_hash_in(v, env),
            Ty::Range(t, _) => self.has_hash_in(t, env),
            Ty::Result(t, e) => self.has_hash_in(t, env) && self.has_hash_in(e, env),
            Ty::Tuple(ts) => ts.iter().all(|t| self.has_hash_in(t, env)),
            // A fixed-size array hashes its elements in order.
            Ty::Array(t, _) => self.has_hash_in(t, env),
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
            // The default list is empty and belongs to the default allocator, like the default map.
            Ty::List(_, alloc) => alloc.is_global(),
            // The default map is empty and belongs to the default allocator: a stored allocator has no value to
            // build its default from.
            Ty::Map(_, _, alloc) => alloc.is_global(),
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
                            Signature { params, ret, has_self: sig.has_self, ret_origins: Vec::new() },
                            sig.has_self,
                            m.is_unsafe,
                        ));
                    }
                }
            }
        }
        None
    }

    /// The own type parameters of the method `name` that the bounds of a type parameter provide, when it has any
    /// (`fn from_iter<I: Iterator<T>>(own it: I): Self`, or the default `fn map<U, F: FnMut(T): U>` of `Iterator<T>`).
    /// The result is the names, their bounds with the interface's parameters replaced by the bound's arguments, and
    /// those replacements themselves, which a caller seeds its inference with. `None` for a method without its own
    /// type parameters.
    pub(crate) fn bound_method_generics(&self, recv: &Ty, name: &str, env: &Env) -> Option<BoundMethodGenerics> {
        let Ty::Param(pname) = recv else { return None };
        for (n, bounds) in env {
            if n != &**pname {
                continue;
            }
            for bound in bounds {
                for b in self.bound_closure(bound) {
                    let Bound::User(i, args) = b else { continue };
                    let iface = &self.interfaces[i];
                    let Some(m) = iface.methods.iter().find(|m| m.name == name) else { continue };
                    if !m.generic {
                        return None;
                    }
                    let subst: Vec<(String, Ty)> = iface.params.iter().cloned().zip(args.iter().cloned()).collect();
                    let bounds =
                        m.bounds.iter().map(|bs| bs.iter().map(|b| self.subst_bound(b, &subst)).collect()).collect();
                    return Some((m.params.clone(), bounds, subst));
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

    fn closure_ty(kind: FnKind, ret: Ty) -> Ty {
        Ty::Fn {
            sig: Rc::new(FnSig { params: vec![i32_()], ret: Box::new(ret) }),
            kind,
            borrowed: false,
            send: tir::FnSend::unknown(),
        }
    }

    #[test]
    fn a_closure_unifies_with_its_signature_at_a_higher_capability() {
        // An `Fn` closure value may fill a declared `FnMut` or `FnOnce` parameter (CL5), so unifying it binds
        // the declared signature's parameters: `fn once<U>(f: FnOnce(i32): U)` infers `U` from `|n| n * 2`.
        assert_eq!(
            bindings(&closure_ty(FnKind::FnOnce, param("U")), &closure_ty(FnKind::Fn, i32_())),
            vec![("U".to_string(), i32_())]
        );
        assert_eq!(
            bindings(&closure_ty(FnKind::FnMut, param("U")), &closure_ty(FnKind::Fn, i32_())),
            vec![("U".to_string(), i32_())]
        );
        // The other direction is not usable (CL5), so it binds nothing.
        assert_eq!(bindings(&closure_ty(FnKind::Fn, param("U")), &closure_ty(FnKind::FnMut, i32_())), vec![]);
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
            (
                Ty::List(Box::new(param("T")), burdin_tir::Alloc::Global),
                Ty::List(Box::new(bool_()), burdin_tir::Alloc::Global),
                vec![("T".into(), bool_())],
            ),
            (
                Ty::Map(Box::new(param("K")), Box::new(param("V")), burdin_tir::Alloc::Global),
                Ty::Map(Box::new(Ty::Str), Box::new(i32_()), burdin_tir::Alloc::Global),
                vec![("K".into(), Ty::Str), ("V".into(), i32_())],
            ),
            (
                Ty::Heap(Box::new(param("T")), burdin_tir::Alloc::Global),
                Ty::Heap(Box::new(i32_()), burdin_tir::Alloc::Global),
                vec![("T".into(), i32_())],
            ),
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
        assert!(bindings(&opt(param("T")), &Ty::List(Box::new(i32_()), burdin_tir::Alloc::Global)).is_empty());
        assert!(bindings(&Ty::Tuple(vec![param("T")]), &Ty::Tuple(vec![i32_(), bool_()])).is_empty());
        assert!(
            bindings(
                &Ty::List(Box::new(param("T")), burdin_tir::Alloc::Global),
                &Ty::Map(Box::new(i32_()), Box::new(i32_()), burdin_tir::Alloc::Global)
            )
            .is_empty()
        );
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
            Ty::Map(Box::new(param("E")), Box::new(param("T")), burdin_tir::Alloc::Global),
            Ty::Ref { mutable: false, inner: Box::new(param("T")) },
        ]);
        let want = Ty::Tuple(vec![
            i32_(),
            Ty::Result(Box::new(i32_()), Box::new(bool_())),
            Ty::Map(Box::new(bool_()), Box::new(i32_()), burdin_tir::Alloc::Global),
            Ty::Ref { mutable: false, inner: Box::new(i32_()) },
        ]);
        assert_eq!(g.subst_ty(&ty, &env), want);
    }

    #[test]
    fn ty_has_param_sees_through_constructors() {
        let mut g = Globals::default();
        assert!(g.ty_has_param(&param("T")));
        assert!(g.ty_has_param(&opt(param("T"))));
        assert!(g.ty_has_param(&Ty::Tuple(vec![i32_(), Ty::List(Box::new(param("T")), burdin_tir::Alloc::Global)])));
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
    fn interface_queries_cover_std_types() {
        let g = Globals::default();
        let list = Ty::List(Box::new(i32_()), burdin_tir::Alloc::Global);
        let map = Ty::Map(Box::new(Ty::String), Box::new(i32_()), burdin_tir::Alloc::Global);
        let floats = Ty::List(Box::new(Ty::Float(burdin_tir::FloatTy::F64)), burdin_tir::Alloc::Global);

        // `Hash` covers text, lists, and maps, and requires the element types (issue #716).
        assert!(g.has_hash(&Ty::String));
        assert!(g.has_hash(&Ty::Str));
        assert!(g.has_hash(&list));
        assert!(g.has_hash(&map));
        assert!(!g.has_hash(&Ty::Float(burdin_tir::FloatTy::F64)), "a float has no Hash");
        assert!(!g.has_hash(&floats), "a list of floats has no Hash");

        // `Default` is the empty container, and only for the default allocator.
        assert!(g.has_default(&list));
        assert!(g.has_default(&map));
        let stored = Ty::List(Box::new(i32_()), burdin_tir::Alloc::Value(Box::new(Ty::String)));
        assert!(!g.has_default(&stored), "a stored allocator has no default");

        // `Ord` covers `Option` and `Result` (issue #716).
        assert!(g.has_ord(&opt(i32_())));
        assert!(g.has_ord(&Ty::Result(Box::new(i32_()), Box::new(i32_()))));
        assert!(!g.has_ord(&list), "a list has no Ord");
    }

    #[test]
    fn a_bound_provides_its_generic_methods_with_the_arguments_substituted() {
        // `interface Mapper<T> { fn get(self): T; fn pair<U, F: Show<T>>(self, f: F): U }`, reached through `M: Mapper<i32>`.
        let mut g = Globals::default();
        let method = |name: &str, generic: bool, params: Vec<String>, bounds: Vec<Vec<Bound>>| IfaceMethod {
            name: name.into(),
            sig: Signature::placeholder(),
            has_default: true,
            is_unsafe: false,
            generic,
            params,
            bounds,
        };
        g.interfaces.push(InterfaceInfo {
            name: "Mapper".into(),
            module: 0,
            params: vec!["T".into()],
            bounds: vec![Vec::new()],
            requires: Vec::new(),
            methods: vec![
                method("get", false, Vec::new(), Vec::new()),
                method(
                    "pair",
                    true,
                    vec!["U".into(), "F".into()],
                    vec![Vec::new(), vec![Bound::User(1, vec![param("T")])]],
                ),
            ],
        });
        let env: &[(String, Vec<Bound>)] = &[("M".to_string(), vec![Bound::User(0, vec![i32_()])])];

        let (own, bounds, base) = g.bound_method_generics(&param("M"), "pair", env).expect("pair is generic");
        assert_eq!(own, vec!["U".to_string(), "F".to_string()]);
        assert_eq!(bounds, vec![Vec::new(), vec![Bound::User(1, vec![i32_()])]], "T is the bound's argument");
        assert_eq!(base, vec![("T".to_string(), i32_())]);

        // A method without type parameters of its own is not generic, and a name no bound provides is absent.
        assert!(g.bound_method_generics(&param("M"), "get", env).is_none());
        assert!(g.bound_method_generics(&param("M"), "missing", env).is_none());
        assert!(g.bound_method_generics(&i32_(), "pair", env).is_none());
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
