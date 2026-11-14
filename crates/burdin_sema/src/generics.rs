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
}

pub(crate) struct InterfaceInfo {
    pub name: String,
    pub requires: Vec<Bound>,
    pub methods: Vec<IfaceMethod>,
}

pub(crate) struct GenericAdt {
    pub name: String,
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
            Ty::Ref { inner, .. } | Ty::Slice(inner) | Ty::Array(inner, _) | Ty::Option(inner) | Ty::Heap(inner) => {
                self.ty_has_param(inner)
            }
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
            | (Ty::Heap(p), Ty::Heap(a)) => self.unify(p, a, bindings),
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
            Ty::Heap(t) => Ty::Heap(go(t)),
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

    /// Resolves a bound written in a generic parameter list, a `where` clause, `requires`, or a derive.
    pub(crate) fn resolve_bound(&self, ty: &ast::Type, diags: &mut Diagnostics) -> Option<Bound> {
        let name = match &ty.kind {
            ast::TypeKind::Path(segs) if segs.len() == 1 && segs[0].args.is_empty() => segs[0].name.name.as_str(),
            _ => {
                diags.error(Code::UNSUPPORTED, ty.span, "this kind of bound is not implemented in this compiler yet");
                return None;
            }
        };
        Some(match name {
            "Copy" => Bound::Copy,
            "Clone" => Bound::Clone,
            "Eq" => Bound::Eq,
            "Ord" => Bound::Ord,
            "Hash" => Bound::Hash,
            "Default" => Bound::Default,
            "Drop" | "From" => {
                diags.error(
                    Code::UNSUPPORTED,
                    ty.span,
                    format!("`{name}` as a bound is not implemented in this compiler yet"),
                );
                return None;
            }
            _ => match self.iface_names.get(name) {
                Some(&i) => Bound::User(i),
                None => {
                    diags.error(Code::UNKNOWN_NAME, ty.span, format!("unknown interface `{name}`"));
                    return None;
                }
            },
        })
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
                Bound::Eq => self.has_eq(ty),
                Bound::Ord => self.has_ord(ty),
                Bound::Hash => self.has_hash(ty),
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
        match ty {
            Ty::Unit | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Error | Ty::Never => true,
            Ty::Tuple(ts) => ts.iter().all(|t| self.has_ord(t)),
            Ty::Struct(a) => {
                let s = &self.structs[a.id];
                s.derives.ord && s.fields.iter().all(|f| self.has_ord(&f.ty))
            }
            Ty::Enum(a) => {
                let e = &self.enums[a.id];
                e.derives.ord && e.variants.iter().all(|v| v.fields.iter().all(|f| self.has_ord(&f.ty)))
            }
            _ => false,
        }
    }

    /// Whether `x.hash()` applies to values of `ty`.
    pub(crate) fn has_hash(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Unit | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Error | Ty::Never => true,
            Ty::Option(t) => self.has_hash(t),
            Ty::Tuple(ts) => ts.iter().all(|t| self.has_hash(t)),
            Ty::Struct(a) => {
                let s = &self.structs[a.id];
                s.derives.hash && s.fields.iter().all(|f| self.has_hash(&f.ty))
            }
            Ty::Enum(a) => {
                let e = &self.enums[a.id];
                e.derives.hash && e.variants.iter().all(|v| v.fields.iter().all(|f| self.has_hash(&f.ty)))
            }
            _ => false,
        }
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
    /// has one) and result, with `Self` replaced by the parameter.
    pub(crate) fn bound_method(&self, recv: &Ty, name: &str, env: &Env) -> Option<(Signature, bool)> {
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
