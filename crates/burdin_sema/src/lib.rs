//! Name resolution and type checking. Produces the typed tree in [`tir`].
//!
//! Milestone 4 scope: functions, constants, the primitive types, arrays, tuples, structs, enums, type aliases,
//! method blocks (`self` and `mut self` receivers), arithmetic, bindings and assignment, control flow (`if`,
//! `if let`, `let ... else`, `while`, `loop`, `for` over ranges and arrays, `match` with destructuring), the built-in
//! methods of numbers, and the output and panic built-ins. Everything else is reported with `Code::UNSUPPORTED`
//! until its milestone.

mod borrows;
mod exhaust;
mod moves;
mod origins;
pub mod tir;

use burdin_diag::{Code, Diagnostics, FileId, Span};
use burdin_syntax::ast;
use exhaust::Adts;
use std::collections::HashMap;
use std::rc::Rc;
use tir::{
    AdtRef, BinOp, BindMode, Builtin, ConstId, FloatTy, FuncId, IntTy, LocalId, LocalKind, Method, Pat, Ty, UnOp,
};

#[derive(Clone)]
struct ParamSig {
    ty: Ty,
    /// The callee sees the caller's place: every parameter except an `own` one (a read-only or `mut` parameter,
    /// and every receiver except `own self`). Passed by address.
    by_ref: bool,
    /// A named `mut` parameter, which the call site writes as `f(mut x)`. (`mut self` needs no marker.)
    mut_param: bool,
    /// An `own` parameter (including `own self`): the callee takes ownership and drops the value.
    own: bool,
    /// Borrowed exclusively for the duration of the call (`mut` and `own` parameters, `mut self`), so no other
    /// argument of the same call may touch an overlapping place (EX1).
    exclusive: bool,
}

impl ParamSig {
    /// A plain read-only parameter: borrowed for the call, never dropped by the callee. Used for built-in
    /// functions and methods, whose arguments code generation evaluates directly.
    fn plain(ty: Ty) -> ParamSig {
        ParamSig { ty, by_ref: false, mut_param: false, own: false, exclusive: false }
    }
}

struct Signature {
    params: Vec<ParamSig>,
    ret: Ty,
    /// The first parameter is `self`.
    has_self: bool,
    /// Which parameter a second-class return value derives from (memory-rules.md, RT1). `None` when the result
    /// is static. Filled in by [`origins`] once the body is known.
    ret_origin: Option<usize>,
}

enum Callee {
    User(FuncId),
    Builtin(Builtin),
}

#[derive(Clone, Copy)]
enum Global {
    Func(FuncId),
    Const(ConstId),
}

#[derive(Clone, Copy)]
enum TypeName {
    Struct(usize),
    Enum(usize),
    Alias(usize),
}

struct Globals {
    names: HashMap<String, Global>,
    types: HashMap<String, TypeName>,
    aliases: Vec<Ty>,
    structs: Vec<tir::StructDef>,
    enums: Vec<tir::EnumDef>,
    struct_refs: Vec<AdtRef>,
    enum_refs: Vec<AdtRef>,
    /// Methods by the type they belong to (`(false, struct)` or `(true, enum)`) and name.
    methods: HashMap<((bool, usize), String), FuncId>,
    /// The `Drop::drop` method of a type that has a `Type: Drop` block.
    drop_methods: HashMap<(bool, usize), FuncId>,
    sigs: Vec<Signature>,
    const_tys: Vec<Ty>,
}

fn adt_key(ty: &Ty) -> Option<(bool, usize)> {
    match ty {
        Ty::Struct(a) => Some((false, a.id)),
        Ty::Enum(a) => Some((true, a.id)),
        _ => None,
    }
}

/// The name of the interface in a `Type: Interface { ... }` block, when it is a plain path.
fn interface_name(ty: &ast::Type) -> String {
    match &ty.kind {
        ast::TypeKind::Path(segments) if segments.len() == 1 && segments[0].args.is_empty() => {
            segments[0].name.name.clone()
        }
        _ => String::new(),
    }
}

/// A function to check: a top-level function or a method, with the type `Self` stands for.
struct FnItem<'a> {
    decl: &'a ast::FnDecl,
    name: String,
    self_ty: Option<Ty>,
}

/// Checks a parsed file. Returns `None` if any error was reported (here or earlier).
pub fn check(file: FileId, ast: &ast::File, diags: &mut Diagnostics) -> Option<tir::Program> {
    let mut globals = Globals {
        names: HashMap::new(),
        types: HashMap::new(),
        aliases: Vec::new(),
        structs: Vec::new(),
        enums: Vec::new(),
        struct_refs: Vec::new(),
        enum_refs: Vec::new(),
        methods: HashMap::new(),
        drop_methods: HashMap::new(),
        sigs: Vec::new(),
        const_tys: Vec::new(),
    };

    // Type names first, so that every signature and field can refer to every type.
    let mut struct_decls = Vec::new();
    let mut enum_decls = Vec::new();
    let mut alias_decls = Vec::new();
    for item in &ast.items {
        let (name, generics, entry) = match item {
            ast::Item::Struct(s) => (&s.name, &s.generics, TypeName::Struct(struct_decls.len())),
            ast::Item::Enum(e) => (&e.name, &e.generics, TypeName::Enum(enum_decls.len())),
            ast::Item::TypeAlias(a) => (&a.name, &a.generics, TypeName::Alias(alias_decls.len())),
            _ => continue,
        };
        if !generics.is_empty() {
            diags.error(Code::UNSUPPORTED, name.span, "generic types are not implemented in this compiler yet");
        }
        if globals.types.contains_key(&name.name) || builtin_type(&name.name).is_some() {
            diags.error(Code::UNSUPPORTED, name.span, format!("type `{}` is defined more than once", name.name));
            continue;
        }
        globals.types.insert(name.name.clone(), entry);
        let adt = |id| AdtRef { id, name: Rc::from(name.name.as_str()) };
        match item {
            ast::Item::Struct(s) => {
                globals.struct_refs.push(adt(struct_decls.len()));
                struct_decls.push(s);
            }
            ast::Item::Enum(e) => {
                globals.enum_refs.push(adt(enum_decls.len()));
                enum_decls.push(e);
            }
            ast::Item::TypeAlias(a) => {
                globals.aliases.push(Ty::Error);
                alias_decls.push(a);
            }
            _ => unreachable!(),
        }
    }
    let mut alias_state = vec![0u8; alias_decls.len()];
    for i in 0..alias_decls.len() {
        resolve_alias(i, &alias_decls, &mut globals, &mut alias_state, diags);
    }
    for s in &struct_decls {
        let fields = s
            .fields
            .iter()
            .map(|f| {
                let ty = globals.resolve_type(&f.ty, None, diags);
                check_storable(&ty, f.ty.span, "a struct field", diags);
                tir::FieldDef { name: f.name.name.clone(), ty, span: f.name.span }
            })
            .collect();
        check_unique_fields(s.fields.iter().map(|f| &f.name), diags);
        let derives = derives(&s.derives, diags);
        globals.structs.push(tir::StructDef { name: s.name.name.clone(), fields, derives, drop_method: None });
    }
    for e in &enum_decls {
        let mut variants: Vec<tir::VariantDef> = Vec::new();
        for v in &e.variants {
            if variants.iter().any(|o| o.name == v.name.name) {
                diags.error(
                    Code::UNSUPPORTED,
                    v.name.span,
                    format!("variant `{}` is defined more than once", v.name.name),
                );
            }
            let (shape, fields) = match &v.kind {
                ast::VariantKind::Unit => (tir::VariantShape::Unit, Vec::new()),
                ast::VariantKind::Tuple(tys) => {
                    let fields = tys
                        .iter()
                        .enumerate()
                        .map(|(i, t)| {
                            let ty = globals.resolve_type(t, None, diags);
                            check_storable(&ty, t.span, "an enum variant field", diags);
                            tir::FieldDef { name: i.to_string(), ty, span: t.span }
                        })
                        .collect();
                    (tir::VariantShape::Tuple, fields)
                }
                ast::VariantKind::Struct(fs) => {
                    check_unique_fields(fs.iter().map(|f| &f.name), diags);
                    let fields = fs
                        .iter()
                        .map(|f| {
                            let ty = globals.resolve_type(&f.ty, None, diags);
                            check_storable(&ty, f.ty.span, "an enum variant field", diags);
                            tir::FieldDef { name: f.name.name.clone(), ty, span: f.name.span }
                        })
                        .collect();
                    (tir::VariantShape::Struct, fields)
                }
            };
            variants.push(tir::VariantDef { name: v.name.name.clone(), shape, fields });
        }
        let derives = derives(&e.derives, diags);
        globals.enums.push(tir::EnumDef { name: e.name.name.clone(), variants, derives, drop_method: None });
    }
    check_infinite_size(&globals, &struct_decls, &enum_decls, diags);

    // Functions, methods, and constants.
    let mut fn_items: Vec<FnItem> = Vec::new();
    let mut const_decls = Vec::new();
    for item in &ast.items {
        match item {
            ast::Item::Fn(f) => {
                if globals.names.contains_key(&f.name.name) || builtin(&f.name.name).is_some() {
                    diags.error(Code::UNSUPPORTED, f.name.span, format!("`{}` is defined more than once", f.name.name));
                    continue;
                }
                let sig = fn_signature(f, None, &globals, diags);
                globals.names.insert(f.name.name.clone(), Global::Func(FuncId(fn_items.len())));
                globals.sigs.push(sig);
                fn_items.push(FnItem { decl: f, name: f.name.name.clone(), self_ty: None });
            }
            ast::Item::Const(c) => {
                if globals.names.contains_key(&c.name.name) || builtin(&c.name.name).is_some() {
                    diags.error(Code::UNSUPPORTED, c.name.span, format!("`{}` is defined more than once", c.name.name));
                    continue;
                }
                let ty = globals.resolve_type(&c.ty, None, diags);
                globals.names.insert(c.name.name.clone(), Global::Const(ConstId(const_decls.len())));
                globals.const_tys.push(ty);
                const_decls.push(c);
            }
            ast::Item::Impl(imp) => {
                if !imp.where_clause.is_empty() {
                    diags.error(
                        Code::UNSUPPORTED,
                        imp.span,
                        "`where` clauses are not implemented in this compiler yet",
                    );
                }
                let interface = imp.interface.as_ref().map(interface_name);
                if let Some(name) = &interface
                    && name != "Drop"
                {
                    diags.error(
                        Code::UNSUPPORTED,
                        imp.interface.as_ref().expect("checked").span,
                        "interface implementations are not implemented in this compiler yet",
                    );
                    continue;
                }
                let is_drop = interface.as_deref() == Some("Drop");
                let self_ty = globals.resolve_type(&imp.ty, None, diags);
                let Some(key) = adt_key(&self_ty) else {
                    if self_ty != Ty::Error {
                        diags.error(
                            Code::TYPE_MISMATCH,
                            imp.ty.span,
                            format!("methods can only be added to structs and enums declared here, not to `{self_ty}`"),
                        );
                    }
                    continue;
                };
                if is_drop && imp.methods.len() != 1 {
                    diags.error(
                        Code::UNSUPPORTED,
                        imp.span,
                        "a `Drop` implementation must contain exactly one `drop` method",
                    );
                    continue;
                }
                for m in &imp.methods {
                    let mkey = (key, m.name.name.clone());
                    let is_variant = match &self_ty {
                        Ty::Enum(a) => globals.enums[a.id].variants.iter().any(|v| v.name == m.name.name),
                        _ => false,
                    };
                    if is_drop && m.name.name != "drop" {
                        diags.error(Code::UNSUPPORTED, m.name.span, "a `Drop` implementation must define `drop`");
                        continue;
                    }
                    if globals.methods.contains_key(&mkey) || is_variant || globals.drop_methods.contains_key(&key) {
                        diags.error(
                            Code::UNSUPPORTED,
                            m.name.span,
                            format!("`{self_ty}` already has a member named `{}`", m.name.name),
                        );
                        continue;
                    }
                    let sig = fn_signature(m, Some(&self_ty), &globals, diags);
                    if is_drop {
                        let ok = sig.has_self && sig.params.len() == 1 && sig.params[0].by_ref && sig.ret == Ty::Unit;
                        if !ok {
                            diags.error(
                                Code::TYPE_MISMATCH,
                                m.span,
                                "`drop` must be declared as `fn drop(mut self)` and return nothing",
                            );
                        }
                        let id = FuncId(fn_items.len());
                        globals.drop_methods.insert(key, id);
                        match key.0 {
                            false => globals.structs[key.1].drop_method = Some(id),
                            true => globals.enums[key.1].drop_method = Some(id),
                        }
                        globals.sigs.push(sig);
                        fn_items.push(FnItem {
                            decl: m,
                            name: format!("{self_ty}.drop"),
                            self_ty: Some(self_ty.clone()),
                        });
                        continue;
                    }
                    globals.methods.insert(mkey, FuncId(fn_items.len()));
                    globals.sigs.push(sig);
                    fn_items.push(FnItem {
                        decl: m,
                        name: format!("{self_ty}.{}", m.name.name),
                        self_ty: Some(self_ty.clone()),
                    });
                }
            }
            ast::Item::Struct(_) | ast::Item::Enum(_) | ast::Item::TypeAlias(_) => {}
        }
    }

    let main = match globals.names.get("main") {
        Some(&Global::Func(id)) => {
            let f = fn_items[id.0].decl;
            if !f.params.is_empty() || globals.sigs[id.0].ret != Ty::Unit {
                diags.error(Code::TYPE_MISMATCH, f.name.span, "`main` must take no parameters and return nothing");
            }
            Some(id)
        }
        _ => {
            diags.error(Code::UNKNOWN_NAME, Span::new(file, 0, 0), "the program has no `main` function");
            None
        }
    };

    let mut consts = Vec::new();
    for (i, c) in const_decls.iter().enumerate() {
        let ty = globals.const_tys[i].clone();
        let mut cx = FnCx::new(&globals, diags, Ty::Unit, None);
        let value = cx.expr(&c.value, Some(&ty));
        let value = cx.expect_ty(value, &ty);
        if !is_const_expr(&value) {
            cx.diags.error(
                Code::UNSUPPORTED,
                c.value.span,
                "only literals, other constants, operators, and casts are allowed in a constant for now",
            );
        }
        consts.push(tir::Const { name: c.name.name.clone(), value });
    }
    check_const_cycles(&consts, &const_decls, diags);

    let mut functions = Vec::new();
    for (i, item) in fn_items.iter().enumerate() {
        let f = item.decl;
        let sig = &globals.sigs[i];
        let ret = sig.ret.clone();
        let mut cx = FnCx::new(&globals, diags, ret.clone(), item.self_ty.clone());
        let mut params = Vec::new();
        let mut sig_params = sig.params.iter();
        for p in &f.params {
            let (name, mutable) = match &p.kind {
                ast::ParamKind::Named { name, .. } => (name.name.as_str(), p.convention == ast::Convention::Mut),
                ast::ParamKind::SelfParam if sig.has_self && params.is_empty() => {
                    ("self", p.convention == ast::Convention::Mut)
                }
                ast::ParamKind::SelfParam => continue,
            };
            let Some(ps) = sig_params.next() else { break };
            let kind = if ps.own { LocalKind::OwnParam } else { LocalKind::BorrowedParam };
            let id = cx.declare(name, ps.ty.clone(), mutable, kind);
            cx.locals[id.0].by_ref = ps.by_ref;
            if name == "self" {
                cx.self_local = Some(id);
            }
            params.push(id);
        }
        let (body, body_ty) = cx.block(&f.body, Some(&ret));
        if !compatible(&body_ty, &ret) {
            let span = match f.body.stmts.last() {
                Some(ast::Stmt::Expr(e)) => e.span,
                Some(ast::Stmt::Let { span, .. }) => *span,
                None => f.body.span,
            };
            cx.diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`{}` returns `{ret}`, but its body produces `{body_ty}`", item.name),
            );
        }
        let locals = cx.locals;
        functions.push(tir::Function {
            name: item.name.clone(),
            span: f.span,
            params,
            ret,
            locals,
            body,
            moved: Vec::new(),
            ret_origin: None,
        });
    }

    validate_derives(&globals, diags);

    // The return origin of a signature is inferred from the body (RT): first to a fixpoint, then reported.
    origins::check(&mut globals, &mut functions, diags);

    for func in &mut functions {
        func.moved = moves::check_function(func, &globals, diags);
    }
    for func in &functions {
        borrows::check(func, &globals, diags);
    }

    if diags.has_errors() {
        return None;
    }
    let Globals { structs, enums, .. } = globals;
    Some(tir::Program { functions, consts, structs, enums, main: main? })
}

fn resolve_alias(
    i: usize,
    decls: &[&ast::TypeAliasDecl],
    globals: &mut Globals,
    state: &mut [u8],
    diags: &mut Diagnostics,
) {
    match state[i] {
        2 => return,
        1 => {
            diags.error(
                Code::UNSUPPORTED,
                decls[i].name.span,
                format!("type alias `{}` refers to itself", decls[i].name.name),
            );
            state[i] = 3;
            return;
        }
        3 => return,
        _ => {}
    }
    state[i] = 1;
    let mut names = Vec::new();
    type_names(&decls[i].ty, &mut names);
    for n in names {
        if let Some(TypeName::Alias(j)) = globals.types.get(&n).copied() {
            resolve_alias(j, decls, globals, state, diags);
        }
    }
    if state[i] == 1 {
        globals.aliases[i] = globals.resolve_type(&decls[i].ty, None, diags);
    }
    state[i] = 2;
}

/// The names used in a type, for ordering the type aliases.
fn type_names(ty: &ast::Type, out: &mut Vec<String>) {
    match &ty.kind {
        ast::TypeKind::Path(segments) => {
            for s in segments {
                out.push(s.name.name.clone());
                s.args.iter().for_each(|a| type_names(a, out));
            }
        }
        ast::TypeKind::Tuple(elems) => elems.iter().for_each(|e| type_names(e, out)),
        ast::TypeKind::Array(e, _) | ast::TypeKind::Slice(e) | ast::TypeKind::Ref { ty: e, .. } => type_names(e, out),
        ast::TypeKind::SelfType | ast::TypeKind::Unit | ast::TypeKind::Never => {}
    }
}

fn check_unique_fields<'a>(names: impl Iterator<Item = &'a ast::Ident>, diags: &mut Diagnostics) {
    let mut seen: Vec<&str> = Vec::new();
    for n in names {
        if seen.contains(&n.name.as_str()) {
            diags.error(Code::UNSUPPORTED, n.span, format!("field `{}` is declared more than once", n.name));
        }
        seen.push(&n.name);
    }
}

fn derives(list: &[ast::Type], diags: &mut Diagnostics) -> tir::Derives {
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
            _ => {
                diags.error(
                    Code::UNKNOWN_NAME,
                    t.span,
                    "only `Copy`, `Clone`, `Eq`, `Ord`, `Hash`, and `Default` can be derived",
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

/// A struct or enum that contains itself by value would have infinite size.
fn check_infinite_size(
    globals: &Globals,
    structs: &[&ast::StructDecl],
    enums: &[&ast::EnumDecl],
    diags: &mut Diagnostics,
) {
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
    let keys = (0..structs.len()).map(|i| (false, i)).chain((0..enums.len()).map(|i| (true, i)));
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
            let name = if key.0 { &enums[key.1].name } else { &structs[key.1].name };
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

/// The requirements of the derives that depend on the memory rules (CP in memory-rules.md): `Copy` needs every
/// field to be Copy and the type to have no `Drop`; `Clone` needs every field to be Clone.
fn validate_derives(globals: &Globals, diags: &mut Diagnostics) {
    let mut fields: Vec<(Ty, Span)> = Vec::new();
    for i in 0..globals.structs.len() {
        let s = &globals.structs[i];
        let ty = Ty::Struct(AdtRef { id: i, name: Rc::from(s.name.as_str()) });
        fields.clear();
        fields.extend(s.fields.iter().map(|f| (f.ty.clone(), f.span)));
        check_derives(globals, diags, &ty, &s.derives, &fields);
    }
    for i in 0..globals.enums.len() {
        let e = &globals.enums[i];
        let ty = Ty::Enum(AdtRef { id: i, name: Rc::from(e.name.as_str()) });
        fields.clear();
        fields.extend(e.variants.iter().flat_map(|v| v.fields.iter()).map(|f| (f.ty.clone(), f.span)));
        check_derives(globals, diags, &ty, &e.derives, &fields);
    }
}

fn check_derives(globals: &Globals, diags: &mut Diagnostics, ty: &Ty, derives: &tir::Derives, fields: &[(Ty, Span)]) {
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
    if derives.clone {
        for (fty, span) in fields {
            if !globals.is_clone(fty) {
                diags.error(
                    Code::DERIVE_REQUIREMENT,
                    *span,
                    format!("`Clone` requires every field to be `Clone`, but this one is `{fty}`"),
                );
            }
        }
    }
}

fn fn_signature(f: &ast::FnDecl, self_ty: Option<&Ty>, globals: &Globals, diags: &mut Diagnostics) -> Signature {
    if !f.generics.is_empty() || !f.where_clause.is_empty() {
        diags.error(Code::UNSUPPORTED, f.name.span, "generic functions are not implemented in this compiler yet");
    }
    if f.is_unsafe {
        diags.error(Code::UNSUPPORTED, f.name.span, "`unsafe fn` is not implemented in this compiler yet");
    }
    let mut params = Vec::new();
    let mut has_self = false;
    for (i, p) in f.params.iter().enumerate() {
        match &p.kind {
            ast::ParamKind::SelfParam => {
                let Some(st) = self_ty.filter(|_| i == 0) else {
                    diags.error(Code::SYNTAX, p.span, "`self` can only be the first parameter of a method");
                    continue;
                };
                has_self = true;
                // `self` reads the caller's value (read-only, `mut`, or `own`): only `own self` takes it (CV4).
                params.push(ParamSig {
                    ty: st.clone(),
                    by_ref: p.convention != ast::Convention::Own,
                    mut_param: false,
                    own: p.convention == ast::Convention::Own,
                    exclusive: p.convention != ast::Convention::Read,
                });
            }
            ast::ParamKind::Named { ty, .. } => {
                let ty = globals.resolve_type(ty, self_ty, diags);
                params.push(match p.convention {
                    // A read-only parameter borrows the caller's place, so a view derived from it can be
                    // returned (RT1).
                    ast::Convention::Read => {
                        ParamSig { ty, by_ref: true, mut_param: false, own: false, exclusive: false }
                    }
                    // The callee sees the caller's place, so it is passed by address and the call site
                    // marks it `mut` (CV2).
                    ast::Convention::Mut => ParamSig { ty, by_ref: true, mut_param: true, own: false, exclusive: true },
                    ast::Convention::Own => {
                        ParamSig { ty, by_ref: false, mut_param: false, own: true, exclusive: true }
                    }
                });
            }
        }
    }
    let ret = f.ret.as_ref().map(|t| globals.resolve_type(t, self_ty, diags)).unwrap_or(Ty::Unit);
    Signature { params, ret, has_self, ret_origin: None }
}

/// A constant is expanded at every use, so it must not refer to itself, directly or through other constants.
fn check_const_cycles(consts: &[tir::Const], decls: &[&ast::ConstDecl], diags: &mut Diagnostics) {
    fn refs(e: &tir::Expr, out: &mut Vec<ConstId>) {
        use tir::ExprKind as K;
        match &e.kind {
            K::Const(id) => out.push(*id),
            K::Unary(_, a) | K::Cast(a) => refs(a, out),
            K::Binary(_, a, b) => {
                refs(a, out);
                refs(b, out);
            }
            K::If(c, then, other) => {
                refs(c, out);
                if let Some(t) = &then.tail {
                    refs(t, out);
                }
                if let Some(o) = other {
                    refs(o, out);
                }
            }
            K::Block(b) => {
                if let Some(t) = &b.tail {
                    refs(t, out);
                }
            }
            _ => {}
        }
    }
    let deps: Vec<Vec<ConstId>> = consts
        .iter()
        .map(|c| {
            let mut v = Vec::new();
            refs(&c.value, &mut v);
            v
        })
        .collect();
    for i in 0..consts.len() {
        if deps[i].iter().any(|d| reaches(d.0, i, &deps)) {
            diags.error(
                Code::UNSUPPORTED,
                decls[i].name.span,
                format!("constant `{}` depends on itself", decls[i].name.name),
            );
        }
    }
    fn reaches(from: usize, to: usize, deps: &[Vec<ConstId>]) -> bool {
        let mut seen = vec![false; deps.len()];
        let mut stack = vec![from];
        while let Some(n) = stack.pop() {
            if n == to {
                return true;
            }
            if !std::mem::replace(&mut seen[n], true) {
                stack.extend(deps[n].iter().map(|d| d.0));
            }
        }
        false
    }
}

fn is_const_expr(e: &tir::Expr) -> bool {
    use tir::ExprKind as K;
    match &e.kind {
        K::Unit | K::Bool(_) | K::Int(_) | K::Float(_) | K::Char(_) | K::Str(_) | K::Const(_) => true,
        K::Unary(_, a) | K::Cast(a) => is_const_expr(a),
        K::Binary(_, a, b) => is_const_expr(a) && is_const_expr(b),
        _ => e.ty == Ty::Error,
    }
}

fn builtin(name: &str) -> Option<Builtin> {
    Some(match name {
        "print" => Builtin::Print,
        "println" => Builtin::Println,
        "eprint" => Builtin::Eprint,
        "eprintln" => Builtin::Eprintln,
        "panic" => Builtin::Panic,
        _ => return None,
    })
}

fn builtin_sig(b: Builtin) -> Signature {
    let ret = match b {
        Builtin::Panic => Ty::Never,
        Builtin::StringNew | Builtin::StringFrom => Ty::String,
        _ => Ty::Unit,
    };
    let params = match b {
        Builtin::StringNew => Vec::new(),
        Builtin::StringFrom => vec![ParamSig::plain(Ty::Str)],
        // `Heap.new` is checked separately: its result type depends on the value.
        Builtin::HeapNew => vec![ParamSig::plain(Ty::Error)],
        _ => vec![ParamSig::plain(Ty::Str)],
    };
    Signature { params, ret, has_self: false, ret_origin: None }
}

fn int_ty(name: &str) -> Option<IntTy> {
    Some(match name {
        "i8" => IntTy::I8,
        "i16" => IntTy::I16,
        "i32" => IntTy::I32,
        "i64" => IntTy::I64,
        "isize" => IntTy::Isize,
        "u8" => IntTy::U8,
        "u16" => IntTy::U16,
        "u32" => IntTy::U32,
        "u64" => IntTy::U64,
        "usize" => IntTy::Usize,
        _ => return None,
    })
}

/// The built-in types that are written as a plain name.
fn builtin_type(name: &str) -> Option<Ty> {
    if let Some(i) = int_ty(name) {
        return Some(Ty::Int(i));
    }
    Some(match name {
        "str" => Ty::Str,
        "bool" => Ty::Bool,
        "char" => Ty::Char,
        "f32" => Ty::Float(FloatTy::F32),
        "f64" => Ty::Float(FloatTy::F64),
        "String" => Ty::String,
        _ => return None,
    })
}

impl Globals {
    /// The type a user-declared type name stands for.
    fn named_type(&self, name: &str) -> Option<Ty> {
        Some(match *self.types.get(name)? {
            TypeName::Struct(i) => Ty::Struct(self.struct_refs[i].clone()),
            TypeName::Enum(i) => Ty::Enum(self.enum_refs[i].clone()),
            TypeName::Alias(i) => self.aliases[i].clone(),
        })
    }

    /// Resolves a written type. `self_ty` is what `Self` means here (inside a method block).
    fn resolve_type(&self, ty: &ast::Type, self_ty: Option<&Ty>, diags: &mut Diagnostics) -> Ty {
        match &ty.kind {
            ast::TypeKind::Unit => Ty::Unit,
            ast::TypeKind::Never => Ty::Never,
            ast::TypeKind::SelfType => match self_ty {
                Some(t) => t.clone(),
                None => {
                    diags.error(Code::UNKNOWN_NAME, ty.span, "`Self` is only available inside a method block");
                    Ty::Error
                }
            },
            ast::TypeKind::Path(segments) if segments.len() == 1 => {
                let seg = &segments[0];
                let name = seg.name.name.as_str();
                if !self.types.contains_key(name) {
                    match (name, seg.args.len()) {
                        ("Option", 1) => {
                            return Ty::Option(Box::new(self.resolve_type(&seg.args[0], self_ty, diags)));
                        }
                        ("Heap", 1) => {
                            let inner = self.resolve_type(&seg.args[0], self_ty, diags);
                            check_storable(&inner, seg.args[0].span, "a `Heap` element", diags);
                            return Ty::Heap(Box::new(inner));
                        }
                        // The standard-library containers arrive with milestone 9. The stage 0 storage rule
                        // (memory-rules.md, SC2) already applies to their type arguments.
                        ("List", 1) | ("Map", 2) => {
                            for arg in &seg.args {
                                let t = self.resolve_type(arg, self_ty, diags);
                                let what = format!("a `{name}` element");
                                check_storable(&t, arg.span, &what, diags);
                            }
                            return Ty::Error;
                        }
                        _ => {}
                    }
                }
                if !seg.args.is_empty() {
                    diags.error(Code::UNSUPPORTED, ty.span, "generic types are not implemented in this compiler yet");
                    return Ty::Error;
                }
                if let Some(t) = self.named_type(name).or_else(|| builtin_type(name)) {
                    return t;
                }
                diags.error(Code::UNKNOWN_NAME, ty.span, format!("unknown type `{name}`"));
                Ty::Error
            }
            ast::TypeKind::Ref { mutable, ty: inner } => {
                let inner = self.resolve_type(inner, self_ty, diags);
                Ty::Ref { mutable: *mutable, inner: Box::new(inner) }
            }
            ast::TypeKind::Slice(elem) => Ty::Slice(Box::new(self.resolve_type(elem, self_ty, diags))),
            ast::TypeKind::Tuple(elems) => {
                Ty::Tuple(elems.iter().map(|e| self.resolve_type(e, self_ty, diags)).collect())
            }
            ast::TypeKind::Array(elem, len) => {
                let elem = self.resolve_type(elem, self_ty, diags);
                match len.kind {
                    ast::ExprKind::Int(n) if n <= u128::from(u32::MAX) => Ty::Array(Box::new(elem), n as u64),
                    _ => {
                        diags.error(
                            Code::UNSUPPORTED,
                            len.span,
                            "array lengths other than integer literals are not implemented in this compiler yet",
                        );
                        Ty::Error
                    }
                }
            }
            _ => {
                diags.error(Code::UNSUPPORTED, ty.span, "this type is not implemented in this compiler yet");
                Ty::Error
            }
        }
    }

    fn adts(&self) -> Adts<'_> {
        Adts { structs: &self.structs, enums: &self.enums }
    }

    /// The type a path with generic arguments stands for, as written in an expression (`List<i32>.new()`).
    fn generic_type(&self, name: &ast::Ident, args: &[ast::Type], self_ty: Option<&Ty>, diags: &mut Diagnostics) -> Ty {
        let n = name.name.as_str();
        if self.types.contains_key(n) {
            diags.error(Code::UNSUPPORTED, name.span, "generic types are not implemented in this compiler yet");
            return Ty::Error;
        }
        match (n, args.len()) {
            ("Option", 1) => Ty::Option(Box::new(self.resolve_type(&args[0], self_ty, diags))),
            ("Heap", 1) => {
                let inner = self.resolve_type(&args[0], self_ty, diags);
                check_storable(&inner, args[0].span, "a `Heap` element", diags);
                Ty::Heap(Box::new(inner))
            }
            // The standard-library containers arrive with milestone 9, but their type arguments are already
            // checked against the storage rule (SC2).
            ("List", 1) | ("Map", 2) => {
                for arg in args {
                    let t = self.resolve_type(arg, self_ty, diags);
                    let what = format!("a `{n}` element");
                    check_storable(&t, arg.span, &what, diags);
                }
                Ty::Error
            }
            _ => {
                diags.error(Code::UNKNOWN_NAME, name.span, format!("unknown type `{n}`"));
                Ty::Error
            }
        }
    }

    /// Whether `==` and `!=` apply to values of `ty`.
    fn has_eq(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Unit | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Float(_) | Ty::Error | Ty::Never => true,
            Ty::Option(t) => self.has_eq(t),
            Ty::Tuple(ts) => ts.iter().all(|t| self.has_eq(t)),
            Ty::Struct(a) => {
                let s = &self.structs[a.id];
                s.derives.eq && s.fields.iter().all(|f| self.has_eq(&f.ty))
            }
            Ty::Enum(a) => {
                let e = &self.enums[a.id];
                e.derives.eq && e.variants.iter().all(|v| v.fields.iter().all(|f| self.has_eq(&f.ty)))
            }
            Ty::Str | Ty::String | Ty::Array(..) | Ty::Heap(_) => false,
            Ty::Ref { .. } | Ty::Slice(_) => false,
        }
    }

    /// Whether values of `ty` are copied instead of moved (memory-rules.md, Copy types).
    fn is_copy(&self, ty: &Ty) -> bool {
        self.is_copy_seen(ty, &mut Vec::new())
    }

    fn is_copy_seen(&self, ty: &Ty, seen: &mut Vec<(bool, usize)>) -> bool {
        match ty {
            Ty::Unit | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Float(_) | Ty::Str | Ty::Never | Ty::Error => true,
            // A shared reference and a view are copied; a mutable reference is not (RF4).
            Ty::Ref { mutable, .. } => !*mutable,
            Ty::Slice(_) => true,
            Ty::Array(t, _) | Ty::Option(t) => self.is_copy_seen(t, seen),
            Ty::Tuple(ts) => ts.iter().all(|t| self.is_copy_seen(t, seen)),
            Ty::String | Ty::Heap(_) => false,
            Ty::Struct(a) => {
                if !self.structs[a.id].derives.copy {
                    return false;
                }
                if seen.contains(&(false, a.id)) {
                    return true;
                }
                seen.push((false, a.id));
                self.structs[a.id].fields.iter().all(|f| self.is_copy_seen(&f.ty, seen))
            }
            Ty::Enum(a) => {
                if !self.enums[a.id].derives.copy {
                    return false;
                }
                if seen.contains(&(true, a.id)) {
                    return true;
                }
                seen.push((true, a.id));
                self.enums[a.id].variants.iter().all(|v| v.fields.iter().all(|f| self.is_copy_seen(&f.ty, seen)))
            }
        }
    }

    /// Whether `x.clone()` applies to values of `ty` (the `Clone` requirement of derives, CP).
    fn is_clone(&self, ty: &Ty) -> bool {
        self.is_clone_seen(ty, &mut Vec::new())
    }

    fn is_clone_seen(&self, ty: &Ty, seen: &mut Vec<(bool, usize)>) -> bool {
        match ty {
            Ty::Unit | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Float(_) | Ty::Str | Ty::Never | Ty::Error => true,
            Ty::String | Ty::Heap(_) => true,
            // Cloning a view yields the same view, which is already a copy of the pointer.
            Ty::Ref { .. } | Ty::Slice(_) => true,
            Ty::Array(t, _) | Ty::Option(t) => self.is_clone_seen(t, seen),
            Ty::Tuple(ts) => ts.iter().all(|t| self.is_clone_seen(t, seen)),
            Ty::Struct(a) => {
                if !self.structs[a.id].derives.clone {
                    return false;
                }
                if seen.contains(&(false, a.id)) {
                    return true;
                }
                seen.push((false, a.id));
                self.structs[a.id].fields.iter().all(|f| self.is_clone_seen(&f.ty, seen))
            }
            Ty::Enum(a) => {
                if !self.enums[a.id].derives.clone {
                    return false;
                }
                if seen.contains(&(true, a.id)) {
                    return true;
                }
                seen.push((true, a.id));
                self.enums[a.id].variants.iter().all(|v| v.fields.iter().all(|f| self.is_clone_seen(&f.ty, seen)))
            }
        }
    }

    /// Whether the type itself declares `Type: Drop { ... }` (fields cannot be moved out, DR8).
    fn has_drop_impl(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Struct(a) => self.structs[a.id].drop_method.is_some(),
            Ty::Enum(a) => self.enums[a.id].drop_method.is_some(),
            _ => false,
        }
    }
}

fn compatible(actual: &Ty, expected: &Ty) -> bool {
    actual == expected || *actual == Ty::Error || *expected == Ty::Error || *actual == Ty::Never
}

/// Reports E0120 when a view type is used where a stored type is required (memory-rules.md, SC2).
fn check_storable(ty: &Ty, span: Span, what: &str, diags: &mut Diagnostics) {
    if ty.is_second_class() {
        diags.error(
            Code::SECOND_CLASS_POSITION,
            span,
            format!("`{ty}` is a view of other storage, so it cannot be used as {what}"),
        );
    }
}

/// An expression whose type comes only from its context: a literal, or operators applied to literals.
fn is_untyped_literal(e: &ast::Expr) -> bool {
    use ast::ExprKind as K;
    match &e.kind {
        K::Int(_) | K::Float(_) => true,
        K::Unary(ast::UnOp::Neg | ast::UnOp::Not, a) => is_untyped_literal(a),
        K::Binary(op, a, b) => is_arith(*op) && is_untyped_literal(a) && is_untyped_literal(b),
        _ => false,
    }
}

fn is_arith(op: ast::BinOp) -> bool {
    use ast::BinOp as B;
    matches!(op, B::Add | B::Sub | B::Mul | B::Div | B::Rem | B::BitAnd | B::BitOr | B::BitXor)
}

fn bin_op(op: ast::BinOp) -> BinOp {
    use ast::BinOp as B;
    match op {
        B::Or => BinOp::Or,
        B::And => BinOp::And,
        B::Eq => BinOp::Eq,
        B::Ne => BinOp::Ne,
        B::Lt => BinOp::Lt,
        B::Gt => BinOp::Gt,
        B::Le => BinOp::Le,
        B::Ge => BinOp::Ge,
        B::BitOr => BinOp::BitOr,
        B::BitXor => BinOp::BitXor,
        B::BitAnd => BinOp::BitAnd,
        B::Shl => BinOp::Shl,
        B::Shr => BinOp::Shr,
        B::Add => BinOp::Add,
        B::Sub => BinOp::Sub,
        B::Mul => BinOp::Mul,
        B::Div => BinOp::Div,
        B::Rem => BinOp::Rem,
    }
}

fn can_cast(from: &Ty, to: &Ty) -> bool {
    let scalar = |t: &Ty| matches!(t, Ty::Int(_) | Ty::Float(_));
    match (from, to) {
        (Ty::Error | Ty::Never, _) | (_, Ty::Error) => true,
        _ if from == to => true,
        (f, t) if scalar(f) && scalar(t) => true,
        (Ty::Bool | Ty::Char, Ty::Int(_)) => true,
        (Ty::Int(IntTy::U8), Ty::Char) => true,
        _ => false,
    }
}

fn method(name: &str) -> Option<Method> {
    Some(match name {
        "to_string" => Method::ToString,
        "wrapping_add" => Method::WrappingAdd,
        "wrapping_sub" => Method::WrappingSub,
        "wrapping_mul" => Method::WrappingMul,
        "checked_add" => Method::CheckedAdd,
        "checked_sub" => Method::CheckedSub,
        "checked_mul" => Method::CheckedMul,
        "min" => Method::Min,
        "max" => Method::Max,
        "abs" => Method::Abs,
        "push_str" => Method::PushStr,
        "as_str" => Method::AsStr,
        "len" => Method::Len,
        "starts_with" => Method::StartsWith,
        "find" => Method::Find,
        _ => return None,
    })
}

/// The parameter types (after the receiver) and the result type of a built-in method, if `recv` has it.
fn method_sig(m: Method, recv: &Ty) -> Option<(Vec<Ty>, Ty)> {
    match (m, recv) {
        (Method::ToString, Ty::Int(_) | Ty::Bool | Ty::Char | Ty::Str) => Some((vec![], Ty::String)),
        (Method::WrappingAdd | Method::WrappingSub | Method::WrappingMul | Method::Min | Method::Max, Ty::Int(_)) => {
            Some((vec![recv.clone()], recv.clone()))
        }
        (Method::CheckedAdd | Method::CheckedSub | Method::CheckedMul, Ty::Int(_)) => {
            Some((vec![recv.clone()], Ty::Option(Box::new(recv.clone()))))
        }
        (Method::Abs, Ty::Int(i)) if i.signed() => Some((vec![], recv.clone())),
        (Method::PushStr, Ty::String) => Some((vec![Ty::Str], Ty::Unit)),
        (Method::AsStr, Ty::String | Ty::Str) => Some((vec![], Ty::Str)),
        (Method::Len, Ty::Str | Ty::String | Ty::Slice(_) | Ty::Array(_, _)) => Some((vec![], Ty::Int(IntTy::Usize))),
        (Method::StartsWith, Ty::Str | Ty::String) => Some((vec![Ty::Str], Ty::Bool)),
        (Method::Find, Ty::Str | Ty::String) => Some((vec![Ty::Char], Ty::Option(Box::new(Ty::Int(IntTy::Usize))))),
        _ => None,
    }
}

/// The local a place expression starts from: a local followed by fields, indexing, and dereferences.
fn place_root(e: &tir::Expr) -> Option<LocalId> {
    match &e.kind {
        tir::ExprKind::Local(id) => Some(*id),
        tir::ExprKind::Index(base, _) | tir::ExprKind::Field(base, _) | tir::ExprKind::Deref(base) => place_root(base),
        tir::ExprKind::DerefRef(base) => place_root(base),
        _ => None,
    }
}

/// Whether a place expression reaches through a reference or a slice, so its storage belongs to another value.
fn through_view(e: &tir::Expr) -> bool {
    match &e.kind {
        tir::ExprKind::DerefRef(_) => true,
        tir::ExprKind::Index(base, _) => through_view(base) || matches!(base.ty, Ty::Slice(_)),
        tir::ExprKind::Field(base, _) => through_view(base),
        _ => false,
    }
}

/// Whether an expression is a place: a local followed by fields, indexing, and dereferences. A reference
/// dereference is always a place: it names the referent, whatever expression produced the reference.
fn is_place_expr(e: &tir::Expr) -> bool {
    match &e.kind {
        tir::ExprKind::Local(_) | tir::ExprKind::DerefRef(_) => true,
        tir::ExprKind::Field(base, _) | tir::ExprKind::Index(base, _) | tir::ExprKind::Deref(base) => {
            is_place_expr(base)
        }
        _ => false,
    }
}

/// The place an expression names: a root local and a field path, which stops at indexing, a `Heap`
/// dereference, or a reference dereference (memory-rules.md, place definition). The flag is false once the path
/// was cut, so a later field selection does not extend it.
fn tir_place(e: &tir::Expr) -> Option<(LocalId, Vec<usize>, bool)> {
    match &e.kind {
        tir::ExprKind::Local(id) => Some((*id, Vec::new(), true)),
        tir::ExprKind::Field(base, i) => {
            let (root, mut path, direct) = tir_place(base)?;
            if direct {
                path.push(*i);
            }
            Some((root, path, direct))
        }
        tir::ExprKind::Index(base, _) | tir::ExprKind::Deref(base) | tir::ExprKind::DerefRef(base) => {
            let (root, _, _) = tir_place(base)?;
            Some((root, Vec::new(), false))
        }
        _ => None,
    }
}

/// A place as the exclusivity check compares them: the local at its root and the field path below it.
type Place = (LocalId, Vec<usize>);

/// Every place an expression mentions (EX1). Index expressions and the parts of compound expressions are
/// visited too; a place on its own contributes its truncated place.
fn place_mentions(e: &tir::Expr, out: &mut Vec<Place>) {
    use tir::ExprKind as K;
    let record = |e: &tir::Expr, out: &mut Vec<Place>| {
        if let Some((root, path, _)) = tir_place(e) {
            out.push((root, path));
        }
    };
    match &e.kind {
        K::Unit
        | K::Bool(_)
        | K::Int(_)
        | K::Float(_)
        | K::Char(_)
        | K::Str(_)
        | K::Const(_)
        | K::None
        | K::Break
        | K::Continue => {}
        K::Local(_) => record(e, out),
        K::Field(base, _) | K::Deref(base) | K::DerefRef(base) => {
            record(e, out);
            if tir_place(base).is_none() {
                place_mentions(base, out);
            }
        }
        K::Index(base, index) => {
            record(e, out);
            place_mentions(index, out);
            if tir_place(base).is_none() {
                place_mentions(base, out);
            }
        }
        K::Ref { place, .. } => {
            record(place, out);
            if tir_place(place).is_none() {
                place_mentions(place, out);
            }
        }
        K::Slice { base, lo, hi } => {
            place_mentions(base, out);
            place_mentions(lo, out);
            place_mentions(hi, out);
        }
        K::StringAsStr(inner) | K::Some(inner) | K::Return(Some(inner)) | K::Cast(inner) => {
            place_mentions(inner, out);
        }
        K::Unary(_, inner) | K::ArrayRepeat(inner, _) => place_mentions(inner, out),
        K::Array(elems) | K::Tuple(elems) => elems.iter().for_each(|x| place_mentions(x, out)),
        K::Struct(fields) | K::Variant(_, fields) => fields.iter().for_each(|(_, x)| place_mentions(x, out)),
        K::Call(_, args) | K::Builtin(_, args) | K::Method(_, args) => {
            args.iter().for_each(|x| place_mentions(x, out));
        }
        K::Binary(_, l, r) | K::Assign(l, r) => {
            place_mentions(l, out);
            place_mentions(r, out);
        }
        K::Block(b) => {
            for s in &b.stmts {
                match s {
                    tir::Stmt::Let(_, init, _) => place_mentions(init, out),
                    tir::Stmt::Expr(x) => place_mentions(x, out),
                }
            }
            if let Some(t) = &b.tail {
                place_mentions(t, out);
            }
        }
        K::If(c, then, other) => {
            place_mentions(c, out);
            place_mentions_block(then, out);
            if let Some(o) = other {
                place_mentions(o, out);
            }
        }
        K::While(c, body) => {
            place_mentions(c, out);
            place_mentions_block(body, out);
        }
        K::Loop(body) => place_mentions_block(body, out),
        K::ForRange { lo, hi, body, .. } => {
            place_mentions(lo, out);
            place_mentions(hi, out);
            place_mentions_block(body, out);
        }
        K::ForArray { array, body, .. } => {
            place_mentions(array, out);
            place_mentions_block(body, out);
        }
        K::ForSlice { slice, body, .. } => {
            place_mentions(slice, out);
            place_mentions_block(body, out);
        }
        K::Match(scrutinee, arms) => {
            place_mentions(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    place_mentions(g, out);
                }
                place_mentions(&arm.body, out);
            }
        }
        K::Return(None) => {}
    }
}

fn place_mentions_block(b: &tir::Block, out: &mut Vec<Place>) {
    for s in &b.stmts {
        match s {
            tir::Stmt::Let(_, init, _) => place_mentions(init, out),
            tir::Stmt::Expr(x) => place_mentions(x, out),
        }
    }
    if let Some(t) = &b.tail {
        place_mentions(t, out);
    }
}

/// Two places overlap when one is a prefix of the other (EX2).
fn places_overlap(a: &(LocalId, Vec<usize>), b: &(LocalId, Vec<usize>)) -> bool {
    a.0 == b.0 && (a.1.starts_with(&b.1) || b.1.starts_with(&a.1))
}

struct FnCx<'a> {
    globals: &'a Globals,
    diags: &'a mut Diagnostics,
    locals: Vec<tir::Local>,
    scopes: Vec<HashMap<String, LocalId>>,
    ret: Ty,
    /// One entry per enclosing loop: whether it contains a `break`.
    loops: Vec<bool>,
    /// What `Self` means: the type of the enclosing method block.
    self_ty: Option<Ty>,
    /// The `self` parameter of the method being checked.
    self_local: Option<LocalId>,
}

fn mk(kind: tir::ExprKind, ty: Ty, span: Span) -> tir::Expr {
    tir::Expr { kind, ty, span }
}

fn error_expr(span: Span) -> tir::Expr {
    mk(tir::ExprKind::Unit, Ty::Error, span)
}

impl<'a> FnCx<'a> {
    fn new(globals: &'a Globals, diags: &'a mut Diagnostics, ret: Ty, self_ty: Option<Ty>) -> Self {
        FnCx {
            globals,
            diags,
            locals: Vec::new(),
            scopes: vec![HashMap::new()],
            ret,
            loops: Vec::new(),
            self_ty,
            self_local: None,
        }
    }

    fn resolve(&mut self, ty: &ast::Type) -> Ty {
        self.globals.resolve_type(ty, self.self_ty.as_ref(), self.diags)
    }

    fn exhaustive(&self, ty: &Ty, pats: &[&Pat]) -> bool {
        exhaust::exhaustive(ty, pats, &self.globals.adts())
    }

    /// If `e` names a type (and no value hides it), that type: the left side of `Point.new` or `Shape.Circle`.
    fn type_path(&mut self, e: &ast::Expr) -> Option<Ty> {
        let ast::ExprKind::Name { name, args } = &e.kind else { return None };
        if self.lookup(&name.name).is_some() || self.globals.names.contains_key(&name.name) {
            return None;
        }
        if args.is_empty() {
            return self.globals.named_type(&name.name).or_else(|| builtin_type(&name.name));
        }
        let self_ty = self.self_ty.clone();
        Some(self.globals.generic_type(name, args, self_ty.as_ref(), self.diags))
    }

    /// A name that is not hidden by a local or a global: `Some`, `None`.
    fn is_prelude(&self, name: &str) -> bool {
        self.lookup(name).is_none() && !self.globals.names.contains_key(name)
    }

    fn declare(&mut self, name: &str, ty: Ty, mutable: bool, kind: LocalKind) -> LocalId {
        let id = LocalId(self.locals.len());
        self.locals.push(tir::Local { name: name.to_string(), ty, mutable, by_ref: false, kind });
        self.scopes.last_mut().expect("a scope is open").insert(name.to_string(), id);
        id
    }

    fn lookup(&self, name: &str) -> Option<LocalId> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    /// Inserts the implicit dereferences of a receiver: field access and method calls go through `&T` (RF1).
    fn auto_deref(&self, mut e: tir::Expr) -> tir::Expr {
        loop {
            let Ty::Ref { inner, .. } = &e.ty else { return e };
            let ty = (**inner).clone();
            let span = e.span;
            e = mk(tir::ExprKind::DerefRef(Box::new(e)), ty, span);
        }
    }

    /// Whether the place `e` may be assigned to or borrowed with `&mut` (MU1, MU3).
    fn place_mutable(&self, e: &tir::Expr) -> bool {
        match &e.kind {
            tir::ExprKind::Local(id) => self.locals[id.0].mutable,
            tir::ExprKind::Field(base, _) | tir::ExprKind::Deref(base) => self.place_mutable(base),
            tir::ExprKind::DerefRef(base) => matches!(base.ty, Ty::Ref { mutable: true, .. }),
            // Elements of an array place are mutable with it; elements of a slice view are read-only.
            tir::ExprKind::Index(base, _) => matches!(base.ty, Ty::Array(..)) && self.place_mutable(base),
            _ => false,
        }
    }

    /// Whether a pattern matching `e` takes ownership of it: a place rooted at an owned local, or a temporary.
    /// A place rooted at a read-only or `mut` parameter (or a view), or reached through a reference or a slice,
    /// is only borrowed (MV8).
    fn owns_scrutinee(&self, e: &tir::Expr) -> bool {
        if through_view(e) {
            return false;
        }
        match place_root(e) {
            Some(root) => matches!(self.locals[root.0].kind, LocalKind::Owned | LocalKind::OwnParam),
            None => true,
        }
    }

    fn unsupported(&mut self, span: Span, what: &str) -> tir::Expr {
        self.diags.error(Code::UNSUPPORTED, span, format!("{what} is not implemented in this compiler yet"));
        error_expr(span)
    }

    fn mismatch(&mut self, span: Span, expected: &Ty, found: &Ty) {
        self.diags.error(Code::TYPE_MISMATCH, span, format!("expected `{expected}`, found `{found}`"));
    }

    /// Reports E0003 unless `e` has type `want` (or a type that converts to nothing at run time, like `!`).
    fn expect_ty(&mut self, e: tir::Expr, want: &Ty) -> tir::Expr {
        if !compatible(&e.ty, want) {
            self.mismatch(e.span, want, &e.ty);
        }
        e
    }

    /// Like `expect_ty`, but a `String` argument is borrowed where a `str` is expected.
    fn coerce_arg(&mut self, e: tir::Expr, want: &Ty) -> tir::Expr {
        if e.ty == Ty::String && *want == Ty::Str {
            let span = e.span;
            return mk(tir::ExprKind::StringAsStr(Box::new(e)), Ty::Str, span);
        }
        self.expect_ty(e, want)
    }

    // ----- blocks and statements ---------------------------------------------

    fn block(&mut self, b: &ast::Block, expect: Option<&Ty>) -> (tir::Block, Ty) {
        self.scopes.push(HashMap::new());
        let mut stmts = Vec::new();
        let mut tail = None;
        let mut diverges = false;
        for (i, s) in b.stmts.iter().enumerate() {
            let last = i + 1 == b.stmts.len();
            match s {
                ast::Stmt::Let { pattern, ty, init, else_block, .. } => {
                    let declared = ty.as_ref().map(|t| self.resolve(t));
                    let init = self.expr(init, declared.as_ref());
                    let init = match &declared {
                        Some(d) => self.expect_ty(init, d),
                        None => init,
                    };
                    diverges |= init.ty == Ty::Never;
                    let local_ty = declared.unwrap_or_else(|| init.ty.clone());
                    // The `else` block runs when the pattern fails, so the pattern's bindings are not in scope there.
                    let else_checked = else_block.as_ref().map(|b| {
                        let (checked, ty) = self.block(b, None);
                        if !matches!(ty, Ty::Never | Ty::Error) {
                            self.diags.error(
                                Code::TYPE_MISMATCH,
                                b.span,
                                "the `else` block of `let ... else` must not finish normally; \
                                 end it with `return`, `break`, `continue`, or `panic`",
                            );
                        }
                        checked
                    });
                    let pat = self.pattern(pattern, &local_ty, true);
                    let refutable = !self.exhaustive(&local_ty, &[&pat]);
                    if refutable && else_checked.is_none() {
                        self.diags.error(
                            Code::NON_EXHAUSTIVE,
                            pattern.span,
                            format!("this pattern does not match every value of `{local_ty}`; add an `else` block"),
                        );
                    } else if !refutable && else_checked.is_some() {
                        self.diags.error(
                            Code::TYPE_MISMATCH,
                            pattern.span,
                            "this pattern always matches, so `let` takes no `else` block",
                        );
                    }
                    stmts.push(tir::Stmt::Let(pat, init, else_checked));
                }
                ast::Stmt::Expr(e) => {
                    let e = self.expr(e, if last { expect } else { None });
                    diverges |= e.ty == Ty::Never;
                    if last {
                        tail = Some(Box::new(e));
                    } else {
                        stmts.push(tir::Stmt::Expr(e));
                    }
                }
            }
        }
        self.scopes.pop();
        let ty = match &tail {
            Some(e) if !(diverges && e.ty == Ty::Unit) => e.ty.clone(),
            _ if diverges => Ty::Never,
            _ => Ty::Unit,
        };
        (tir::Block { stmts, tail }, ty)
    }

    // ----- expressions -------------------------------------------------------

    fn expr(&mut self, e: &ast::Expr, expect: Option<&Ty>) -> tir::Expr {
        use ast::ExprKind as K;
        let span = e.span;
        match &e.kind {
            K::Int(v) => self.int_literal(*v, false, expect, span),
            K::Float(text) => self.float_literal(text, false, expect, span),
            K::Char(c) => mk(tir::ExprKind::Char(*c), Ty::Char, span),
            K::Str(s) => mk(tir::ExprKind::Str(s.clone()), Ty::Str, span),
            K::Bool(b) => mk(tir::ExprKind::Bool(*b), Ty::Bool, span),
            K::Unit => mk(tir::ExprKind::Unit, Ty::Unit, span),
            K::Name { name, args } if args.is_empty() => self.name(name, expect),
            // A generic type path only means something as the left side of an associated call.
            K::Name { name, args: _ } => {
                self.diags.error(Code::TYPE_MISMATCH, span, format!("`{}` is a type, not a value", name.name));
                error_expr(span)
            }
            K::Block(b) => {
                let (b, ty) = self.block(b, expect);
                mk(tir::ExprKind::Block(b), ty, span)
            }
            K::If { cond, then_block, else_expr } => self.if_expr(cond, then_block, else_expr.as_deref(), expect, span),
            K::While { cond, body } => {
                let cond = self.expr(cond, Some(&Ty::Bool));
                let cond = self.expect_ty(cond, &Ty::Bool);
                self.loops.push(false);
                let (body, _) = self.block(body, None);
                self.loops.pop();
                mk(tir::ExprKind::While(Box::new(cond), body), Ty::Unit, span)
            }
            K::Loop(body) => {
                self.loops.push(false);
                let (body, _) = self.block(body, None);
                let has_break = self.loops.pop().unwrap_or(false);
                mk(tir::ExprKind::Loop(body), if has_break { Ty::Unit } else { Ty::Never }, span)
            }
            K::For { pattern, iter, body } => self.for_expr(pattern, iter, body, span),
            K::Match(scrutinee, arms) => self.match_expr(scrutinee, arms, expect, span),
            K::Break | K::Continue => {
                let Some(has_break) = self.loops.last_mut() else {
                    let word = if matches!(e.kind, K::Break) { "break" } else { "continue" };
                    self.diags.error(Code::SYNTAX, span, format!("`{word}` outside of a loop"));
                    return error_expr(span);
                };
                if matches!(e.kind, K::Break) {
                    *has_break = true;
                    mk(tir::ExprKind::Break, Ty::Never, span)
                } else {
                    mk(tir::ExprKind::Continue, Ty::Never, span)
                }
            }
            K::Return(value) => {
                let ret = self.ret.clone();
                let value = match value {
                    Some(v) => {
                        let v = self.expr(v, Some(&ret));
                        Some(Box::new(self.expect_ty(v, &ret)))
                    }
                    None => {
                        if !compatible(&Ty::Unit, &ret) {
                            self.mismatch(span, &ret, &Ty::Unit);
                        }
                        None
                    }
                };
                mk(tir::ExprKind::Return(value), Ty::Never, span)
            }
            K::Assign(lhs, rhs) => self.assign(lhs, rhs, span),
            K::Binary(op, l, r) => self.binary(*op, l, r, expect, span),
            K::Unary(op, operand) => self.unary(*op, operand, expect, span),
            K::Cast(operand, ty) => {
                let target = self.resolve(ty);
                let operand = self.expr(operand, None);
                if !can_cast(&operand.ty, &target) {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        span,
                        format!("cannot convert `{}` to `{target}` with `as`", operand.ty),
                    );
                    return error_expr(span);
                }
                mk(tir::ExprKind::Cast(Box::new(operand)), target, span)
            }
            K::Call(callee, args) => self.call(callee, args, span, expect),
            K::Index(base, index) => self.index(base, index, span),
            K::Array(elems) => self.array(elems, expect, span),
            K::ArrayRepeat(value, count) => {
                let elem_expect = match expect {
                    Some(Ty::Array(t, _)) => Some(&**t),
                    _ => None,
                };
                let value = self.expr(value, elem_expect);
                let n = match count.kind {
                    K::Int(n) if n <= u128::from(u32::MAX) => n as u64,
                    _ => return self.unsupported(count.span, "array lengths other than integer literals"),
                };
                let ty = Ty::Array(Box::new(value.ty.clone()), n);
                mk(tir::ExprKind::ArrayRepeat(Box::new(value), n), ty, span)
            }
            K::Field(base, name) => self.field(base, name, span),
            K::TupleField(base, index, index_span) => {
                let base = self.expr(base, None);
                let base = self.auto_deref(base);
                let ty = match &base.ty {
                    Ty::Tuple(elems) if (*index as usize) < elems.len() => elems[*index as usize].clone(),
                    Ty::Error => return error_expr(span),
                    other => {
                        self.diags.error(
                            Code::UNKNOWN_NAME,
                            *index_span,
                            format!("no field `{index}` on type `{other}`"),
                        );
                        return error_expr(span);
                    }
                };
                mk(tir::ExprKind::Field(Box::new(base), *index as usize), ty, span)
            }
            K::Tuple(elems) => {
                let expects: Vec<Option<Ty>> = match expect {
                    Some(Ty::Tuple(ts)) if ts.len() == elems.len() => ts.iter().cloned().map(Some).collect(),
                    _ => vec![None; elems.len()],
                };
                let checked: Vec<tir::Expr> =
                    elems.iter().zip(&expects).map(|(e, t)| self.expr(e, t.as_ref())).collect();
                let ty = Ty::Tuple(checked.iter().map(|e| e.ty.clone()).collect());
                mk(tir::ExprKind::Tuple(checked), ty, span)
            }
            K::StructLit(path, fields) => self.struct_literal(path, fields, span),
            K::IfLet { pattern, scrutinee, then_block, else_expr } => {
                self.if_let(pattern, scrutinee, then_block, else_expr.as_deref(), expect, span)
            }
            K::Range { .. } => self.unsupported(span, "ranges outside `for` loops"),
            K::OrElse(..) => self.unsupported(span, "`orelse`"),
            K::Try(_) => self.unsupported(span, "`?`"),
            K::SelfValue => match self.self_local {
                Some(id) => mk(tir::ExprKind::Local(id), self.locals[id.0].ty.clone(), span),
                None => {
                    self.diags.error(
                        Code::UNKNOWN_NAME,
                        span,
                        "`self` is only available in a method with a `self` parameter",
                    );
                    error_expr(span)
                }
            },
            K::UnsafeBlock(_) => self.unsupported(span, "`unsafe` blocks"),
        }
    }

    fn int_literal(&mut self, v: u128, negative: bool, expect: Option<&Ty>, span: Span) -> tir::Expr {
        let ity = match expect {
            Some(Ty::Int(i)) => *i,
            _ => IntTy::I32,
        };
        match self.int_value(v, negative, ity, span) {
            Some(value) => mk(tir::ExprKind::Int(value), Ty::Int(ity), span),
            None => error_expr(span),
        }
    }

    /// The value of an integer literal of type `ity`, or E0008 if it does not fit.
    fn int_value(&mut self, v: u128, negative: bool, ity: IntTy, span: Span) -> Option<i128> {
        let value = i128::try_from(v).ok().map(|v| if negative { -v } else { v });
        match value {
            Some(value) if value >= ity.min() && value <= ity.max() => Some(value),
            _ => {
                let sign = if negative { "-" } else { "" };
                self.diags.error(
                    Code::LITERAL_RANGE,
                    span,
                    format!("literal `{sign}{v}` is out of range for `{}`", ity.name()),
                );
                None
            }
        }
    }

    fn float_literal(&mut self, text: &str, negative: bool, expect: Option<&Ty>, span: Span) -> tir::Expr {
        let fty = match expect {
            Some(Ty::Float(f)) => *f,
            _ => FloatTy::F64,
        };
        let Ok(v) = text.parse::<f64>() else {
            self.diags.error(Code::SYNTAX, span, format!("invalid float literal `{text}`"));
            return error_expr(span);
        };
        let v = if negative { -v } else { v };
        if !v.is_finite() || (fty == FloatTy::F32 && !(v as f32).is_finite()) {
            self.diags.error(Code::LITERAL_RANGE, span, format!("literal `{text}` is out of range for its type"));
            return error_expr(span);
        }
        mk(tir::ExprKind::Float(v), Ty::Float(fty), span)
    }

    fn name(&mut self, id: &ast::Ident, expect: Option<&Ty>) -> tir::Expr {
        if let Some(local) = self.lookup(&id.name) {
            return mk(tir::ExprKind::Local(local), self.locals[local.0].ty.clone(), id.span);
        }
        if id.name == "None" && self.is_prelude("None") {
            return match expect {
                Some(t @ Ty::Option(_)) => mk(tir::ExprKind::None, t.clone(), id.span),
                _ => {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        id.span,
                        "cannot tell the type of this `None`; annotate the binding with an `Option` type",
                    );
                    error_expr(id.span)
                }
            };
        }
        match self.globals.names.get(&id.name) {
            Some(Global::Const(c)) => mk(tir::ExprKind::Const(*c), self.globals.const_tys[c.0].clone(), id.span),
            Some(Global::Func(_)) => self.unsupported(id.span, "using a function as a value"),
            None if builtin(&id.name).is_some() => self.unsupported(id.span, "using a function as a value"),
            None if id.name == "Some" => self.unsupported(id.span, "using `Some` as a function value"),
            None if self.globals.types.contains_key(&id.name) => {
                self.diags.error(Code::TYPE_MISMATCH, id.span, format!("`{}` is a type, not a value", id.name));
                error_expr(id.span)
            }
            None => {
                self.diags.error(Code::UNKNOWN_NAME, id.span, format!("unknown name `{}`", id.name));
                error_expr(id.span)
            }
        }
    }

    fn if_expr(
        &mut self,
        cond: &ast::Expr,
        then_block: &ast::Block,
        else_expr: Option<&ast::Expr>,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let cond = self.expr(cond, Some(&Ty::Bool));
        let cond = self.expect_ty(cond, &Ty::Bool);
        let (then, then_ty) = self.block(then_block, if else_expr.is_some() { expect } else { None });
        let Some(else_expr) = else_expr else {
            return mk(tir::ExprKind::If(Box::new(cond), then, None), Ty::Unit, span);
        };
        let else_expect = if then_ty == Ty::Never { expect.cloned() } else { Some(then_ty.clone()) };
        let other = self.expr(else_expr, else_expect.as_ref());
        let ty = self.unify(then_ty, &other.ty, other.span, "`if` and `else` have different types");
        mk(tir::ExprKind::If(Box::new(cond), then, Some(Box::new(other))), ty, span)
    }

    /// The type of two branches that must agree; reports E0003 at `span` when they do not.
    fn unify(&mut self, first: Ty, second: &Ty, span: Span, what: &str) -> Ty {
        if first == Ty::Never || first == Ty::Error {
            return second.clone();
        }
        if !compatible(second, &first) {
            self.diags.error(Code::TYPE_MISMATCH, span, format!("{what}: `{first}` and `{second}`"));
        }
        first
    }

    fn for_expr(&mut self, pattern: &ast::Pattern, iter: &ast::Expr, body: &ast::Block, span: Span) -> tir::Expr {
        let ast::ExprKind::Range { lo, hi, inclusive } = &iter.kind else {
            let array = self.expr(iter, None);
            let elem = match &array.ty {
                Ty::Array(t, _) | Ty::Slice(t) => (**t).clone(),
                Ty::Error | Ty::Never => Ty::Error,
                other => {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        iter.span,
                        format!("cannot iterate over a value of type `{other}`"),
                    );
                    Ty::Error
                }
            };
            self.scopes.push(HashMap::new());
            // `for` borrows the collection, so its elements are read-only views.
            let pat = self.pattern(pattern, &elem, false);
            if !self.exhaustive(&elem, &[&pat]) {
                self.diags.error(
                    Code::NON_EXHAUSTIVE,
                    pattern.span,
                    format!("this pattern does not match every value of `{elem}`"),
                );
            }
            self.loops.push(false);
            let (body, _) = self.block(body, None);
            self.loops.pop();
            self.scopes.pop();
            let kind = if matches!(array.ty, Ty::Slice(_)) {
                tir::ExprKind::ForSlice { pat, slice: Box::new(array), body }
            } else {
                tir::ExprKind::ForArray { pat, array: Box::new(array), body }
            };
            return mk(kind, Ty::Unit, span);
        };
        let (lo, hi) = self.operands(lo, hi, None);
        let ty = if lo.ty == Ty::Never { hi.ty.clone() } else { lo.ty.clone() };
        let hi = self.expect_ty(hi, &ty);
        if !matches!(ty, Ty::Int(_) | Ty::Error) {
            self.diags.error(Code::TYPE_MISMATCH, iter.span, format!("cannot iterate over a range of `{ty}`"));
        }
        self.scopes.push(HashMap::new());
        let var = match &pattern.kind {
            ast::PatternKind::Binding { name, mutable } => {
                Some(self.declare(&name.name, ty, *mutable, LocalKind::Owned))
            }
            ast::PatternKind::Wild => None,
            _ => {
                self.scopes.pop();
                return self.unsupported(pattern.span, "this `for` pattern");
            }
        };
        self.loops.push(false);
        let (body, _) = self.block(body, None);
        self.loops.pop();
        self.scopes.pop();
        let kind = tir::ExprKind::ForRange { var, lo: Box::new(lo), hi: Box::new(hi), inclusive: *inclusive, body };
        mk(kind, Ty::Unit, span)
    }

    fn match_expr(
        &mut self,
        scrutinee: &ast::Expr,
        arms: &[ast::MatchArm],
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let scrutinee = self.expr(scrutinee, None);
        let owned = self.owns_scrutinee(&scrutinee);
        let mut ty = Ty::Never;
        let mut checked = Vec::new();
        for arm in arms {
            self.scopes.push(HashMap::new());
            let pat = match &arm.pattern {
                Some(p) => self.pattern(p, &scrutinee.ty, owned),
                None => Pat::Wild,
            };
            let guard = arm.guard.as_ref().map(|g| {
                let g = self.expr(g, Some(&Ty::Bool));
                self.expect_ty(g, &Ty::Bool)
            });
            let arm_expect = if ty == Ty::Never { expect.cloned() } else { Some(ty.clone()) };
            let body = self.expr(&arm.body, arm_expect.as_ref());
            ty = self.unify(ty, &body.ty, body.span, "match arms have different types");
            self.scopes.pop();
            checked.push(tir::Arm { pat, guard, body });
        }
        let unguarded: Vec<&Pat> = checked.iter().filter(|a| a.guard.is_none()).map(|a| &a.pat).collect();
        if !self.exhaustive(&scrutinee.ty, &unguarded) {
            self.diags.error(
                Code::NON_EXHAUSTIVE,
                span,
                format!("this `match` does not cover every value of `{}`", scrutinee.ty),
            );
        }
        mk(tir::ExprKind::Match(Box::new(scrutinee), checked), ty, span)
    }

    /// How a binding of type `ty` takes its value: a view when the scrutinee is borrowed, otherwise a move for a
    /// non-Copy type and a copy for a Copy one.
    fn bind_mode(&self, ty: &Ty, owned: bool) -> BindMode {
        if !owned {
            BindMode::View
        } else if self.globals.is_copy(ty) {
            BindMode::Copy
        } else {
            BindMode::Move
        }
    }

    fn declare_binding(&mut self, name: &ast::Ident, ty: &Ty, mutable: bool, owned: bool) -> (LocalId, BindMode) {
        let mode = self.bind_mode(ty, owned);
        let kind = if mode == BindMode::View { LocalKind::View } else { LocalKind::Owned };
        (self.declare(&name.name, ty.clone(), mutable, kind), mode)
    }

    fn pattern(&mut self, p: &ast::Pattern, ty: &Ty, owned: bool) -> Pat {
        use ast::PatternKind as P;
        let pat_mismatch = |cx: &mut Self, what: &str| {
            if *ty != Ty::Error {
                cx.diags.error(Code::TYPE_MISMATCH, p.span, format!("expected `{ty}`, found {what}"));
            }
            Pat::Wild
        };
        match &p.kind {
            P::Wild => Pat::Wild,
            P::Binding { name, mutable } => {
                if name.name == "None" && !*mutable && self.is_prelude("None") {
                    return match ty {
                        Ty::Option(_) | Ty::Error => Pat::None,
                        _ => pat_mismatch(self, "`None`"),
                    };
                }
                if let Some(Global::Const(_)) = self.globals.names.get(&name.name) {
                    self.unsupported(p.span, "constants in patterns");
                    return Pat::Wild;
                }
                let (id, mode) = self.declare_binding(name, ty, *mutable, owned);
                Pat::Binding(id, mode)
            }
            P::At(name, inner) => {
                let inner = self.pattern(inner, ty, owned);
                let (id, mode) = self.declare_binding(name, ty, false, owned);
                Pat::At(id, mode, Box::new(inner))
            }
            P::Lit(lit) => self.lit_pattern(lit, ty, p.span).unwrap_or(Pat::Wild),
            P::Range(lo, hi) => {
                let (Some(lo), Some(hi)) = (self.lit_pattern(lo, ty, p.span), self.lit_pattern(hi, ty, p.span)) else {
                    return Pat::Wild;
                };
                let value = |p: &Pat| match p {
                    Pat::Int(v) => Some(*v),
                    Pat::Char(c) => Some(*c as i128),
                    _ => None,
                };
                match (value(&lo), value(&hi)) {
                    (Some(lo), Some(hi)) if lo <= hi => Pat::Range(lo, hi),
                    (Some(_), Some(_)) => {
                        self.diags.error(Code::TYPE_MISMATCH, p.span, "this range pattern is empty");
                        Pat::Wild
                    }
                    _ => pat_mismatch(self, "a range pattern"),
                }
            }
            P::TupleStruct(path, elems) if path.len() == 1 && path[0].name == "Some" && self.is_prelude("Some") => {
                match ty {
                    Ty::Option(inner) if elems.len() == 1 => Pat::Some(Box::new(self.pattern(&elems[0], inner, owned))),
                    Ty::Option(_) => {
                        self.diags.error(Code::ARG_COUNT, p.span, "`Some` takes one pattern");
                        Pat::Wild
                    }
                    Ty::Error => {
                        elems.iter().for_each(|e| {
                            self.pattern(e, &Ty::Error, owned);
                        });
                        Pat::Wild
                    }
                    _ => pat_mismatch(self, "`Some(...)`"),
                }
            }
            P::Or(alts) => {
                let before = self.locals.len();
                let alts: Vec<Pat> = alts.iter().map(|a| self.pattern(a, ty, owned)).collect();
                if self.locals.len() != before {
                    self.unsupported(p.span, "bindings inside `|` patterns");
                }
                Pat::Or(alts)
            }
            P::Tuple(elems) => match ty {
                Ty::Tuple(ts) if ts.len() == elems.len() => {
                    Pat::Product(elems.iter().zip(ts).map(|(e, t)| self.pattern(e, t, owned)).collect())
                }
                Ty::Unit if elems.is_empty() => Pat::Product(Vec::new()),
                Ty::Error => {
                    elems.iter().for_each(|e| {
                        self.pattern(e, &Ty::Error, owned);
                    });
                    Pat::Wild
                }
                _ => pat_mismatch(self, &format!("a tuple pattern with {} elements", elems.len())),
            },
            P::Path(path) => {
                let Some((index, shape, _)) = self.variant_pattern(path, ty, p.span) else { return Pat::Wild };
                if shape != tir::VariantShape::Unit {
                    let how = if shape == tir::VariantShape::Tuple { "(...)" } else { " { ... }" };
                    let name = path.last().map(|n| n.name.as_str()).unwrap_or("");
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        p.span,
                        format!("variant `{name}` has fields; match it with `{name}{how}`"),
                    );
                    return Pat::Wild;
                }
                Pat::Variant(index, Vec::new())
            }
            P::TupleStruct(path, elems) => {
                let Some((index, shape, field_tys)) = self.variant_pattern(path, ty, p.span) else {
                    elems.iter().for_each(|e| {
                        self.pattern(e, &Ty::Error, owned);
                    });
                    return Pat::Wild;
                };
                if shape != tir::VariantShape::Tuple || field_tys.len() != elems.len() {
                    let code = if shape == tir::VariantShape::Tuple { Code::ARG_COUNT } else { Code::TYPE_MISMATCH };
                    self.diags.error(
                        code,
                        p.span,
                        format!("this variant has {} field(s) and is not matched like this", field_tys.len()),
                    );
                    elems.iter().for_each(|e| {
                        self.pattern(e, &Ty::Error, owned);
                    });
                    return Pat::Wild;
                }
                Pat::Variant(index, elems.iter().zip(&field_tys).map(|(e, t)| self.pattern(e, t, owned)).collect())
            }
            P::Struct(path, fields) => {
                // A struct, or a variant with named fields.
                let (variant, defs): (Option<usize>, Vec<(String, Ty)>) = if path.len() == 1 {
                    match self.globals.named_type(&path[0].name) {
                        Some(Ty::Struct(a)) if *ty == Ty::Struct(a.clone()) || *ty == Ty::Error => {
                            let defs = self.globals.structs[a.id]
                                .fields
                                .iter()
                                .map(|f| (f.name.clone(), f.ty.clone()))
                                .collect();
                            (None, defs)
                        }
                        Some(_) => return pat_mismatch(self, &format!("a `{}` pattern", path[0].name)),
                        None => {
                            self.diags.error(
                                Code::UNKNOWN_NAME,
                                path[0].span,
                                format!("unknown type `{}`", path[0].name),
                            );
                            return Pat::Wild;
                        }
                    }
                } else {
                    let Some((index, shape, _)) = self.variant_pattern(path, ty, p.span) else { return Pat::Wild };
                    if shape != tir::VariantShape::Struct {
                        return pat_mismatch(self, "a pattern with named fields");
                    }
                    let Ty::Enum(a) = ty else { return Pat::Wild };
                    let defs = self.globals.enums[a.id].variants[index]
                        .fields
                        .iter()
                        .map(|f| (f.name.clone(), f.ty.clone()))
                        .collect();
                    (Some(index), defs)
                };
                let mut subs: Vec<Pat> = defs.iter().map(|_| Pat::Wild).collect();
                for f in fields {
                    let Some(i) = defs.iter().position(|(n, _)| *n == f.name.name) else {
                        self.diags.error(Code::UNKNOWN_NAME, f.name.span, format!("no field `{}` here", f.name.name));
                        if let Some(sub) = &f.pattern {
                            self.pattern(sub, &Ty::Error, owned);
                        }
                        continue;
                    };
                    let fty = defs[i].1.clone();
                    subs[i] = match &f.pattern {
                        Some(sub) => self.pattern(sub, &fty, owned),
                        None => {
                            let (id, mode) = self.declare_binding(&f.name, &fty, false, owned);
                            Pat::Binding(id, mode)
                        }
                    };
                }
                match variant {
                    Some(index) => Pat::Variant(index, subs),
                    None if *ty == Ty::Error => Pat::Wild,
                    None => Pat::Product(subs),
                }
            }
        }
    }

    /// Resolves `Enum.Variant` in a pattern against the scrutinee type: the variant's index, shape, and field
    /// types. Reports an error and returns `None` if the path is not a variant of `ty`.
    fn variant_pattern(
        &mut self,
        path: &[ast::Ident],
        ty: &Ty,
        span: Span,
    ) -> Option<(usize, tir::VariantShape, Vec<Ty>)> {
        let enum_ty = if path.len() == 2 { self.globals.named_type(&path[0].name) } else { None };
        let Some(Ty::Enum(a)) = enum_ty else {
            let text: Vec<&str> = path.iter().map(|p| p.name.as_str()).collect();
            self.diags.error(Code::UNKNOWN_NAME, span, format!("`{}` is not an enum variant", text.join(".")));
            return None;
        };
        let def = &self.globals.enums[a.id];
        let Some(index) = def.variants.iter().position(|v| v.name == path[1].name) else {
            self.diags.error(
                Code::UNKNOWN_NAME,
                path[1].span,
                format!("`{}` has no variant `{}`", a.name, path[1].name),
            );
            return None;
        };
        if *ty != Ty::Enum(a.clone()) {
            if *ty != Ty::Error {
                self.diags.error(Code::TYPE_MISMATCH, span, format!("expected `{ty}`, found a `{}` pattern", a.name));
            }
            return None;
        }
        let v = &def.variants[index];
        Some((index, v.shape, v.fields.iter().map(|f| f.ty.clone()).collect()))
    }

    fn lit_pattern(&mut self, lit: &ast::LitPat, ty: &Ty, span: Span) -> Option<Pat> {
        match (lit, ty) {
            (_, Ty::Error) => None,
            (ast::LitPat::Int { value, negative }, Ty::Int(i)) => {
                self.int_value(*value, *negative, *i, span).map(Pat::Int)
            }
            (ast::LitPat::Bool(b), Ty::Bool) => Some(Pat::Bool(*b)),
            (ast::LitPat::Char(c), Ty::Char) => Some(Pat::Char(*c)),
            (ast::LitPat::Str(_), Ty::Str) => {
                self.unsupported(span, "string patterns");
                None
            }
            _ => {
                let what = match lit {
                    ast::LitPat::Int { .. } => "an integer literal",
                    ast::LitPat::Bool(_) => "a `bool` literal",
                    ast::LitPat::Char(_) => "a character literal",
                    ast::LitPat::Str(_) => "a string literal",
                };
                self.diags.error(Code::TYPE_MISMATCH, span, format!("expected `{ty}`, found {what}"));
                None
            }
        }
    }

    fn assign(&mut self, lhs: &ast::Expr, rhs: &ast::Expr, span: Span) -> tir::Expr {
        let place = self.expr(lhs, None);
        let rhs = self.expr(rhs, Some(&place.ty));
        let rhs = self.expect_ty(rhs, &place.ty);
        if place.ty != Ty::Error {
            if !is_place_expr(&place) {
                self.diags.error(
                    Code::IMMUTABLE,
                    lhs.span,
                    "the left side of `=` is not a variable or an element of one",
                );
            } else if !self.place_mutable(&place) {
                let name = place_root(&place).map(|root| self.locals[root.0].name.clone());
                let whose = name.unwrap_or_else(|| "this place".to_string());
                self.diags.error(Code::IMMUTABLE, lhs.span, format!("cannot assign to `{whose}`: it is not mutable"));
            }
        }
        mk(tir::ExprKind::Assign(Box::new(place), Box::new(rhs)), Ty::Unit, span)
    }

    /// Checks two operands that must have the same type. A literal on the left takes its type from the right.
    fn operands(&mut self, l: &ast::Expr, r: &ast::Expr, hint: Option<&Ty>) -> (tir::Expr, tir::Expr) {
        let numeric_hint = hint.filter(|t| t.is_numeric());
        if numeric_hint.is_none() && is_untyped_literal(l) && !is_untyped_literal(r) {
            let r = self.expr(r, None);
            let h = if r.ty.is_numeric() { Some(r.ty.clone()) } else { None };
            let l = self.expr(l, h.as_ref());
            return (l, r);
        }
        let l = self.expr(l, numeric_hint);
        let h = if l.ty.is_numeric() { Some(l.ty.clone()) } else { numeric_hint.cloned() };
        let r = self.expr(r, h.as_ref());
        (l, r)
    }

    fn binary(&mut self, op: ast::BinOp, l: &ast::Expr, r: &ast::Expr, expect: Option<&Ty>, span: Span) -> tir::Expr {
        use ast::BinOp as B;
        let op_name = op_text(op);
        match op {
            B::And | B::Or => {
                let l = self.expr(l, Some(&Ty::Bool));
                let l = self.expect_ty(l, &Ty::Bool);
                let r = self.expr(r, Some(&Ty::Bool));
                let r = self.expect_ty(r, &Ty::Bool);
                mk(tir::ExprKind::Binary(bin_op(op), Box::new(l), Box::new(r)), Ty::Bool, span)
            }
            B::Shl | B::Shr => {
                let l = self.expr(l, expect.filter(|t| t.is_int()));
                let r = self.expr(r, if l.ty.is_int() { Some(&l.ty) } else { None });
                for operand in [&l, &r] {
                    if !matches!(operand.ty, Ty::Int(_) | Ty::Error | Ty::Never) {
                        self.diags.error(
                            Code::TYPE_MISMATCH,
                            operand.span,
                            format!("`{op_name}` needs integer operands, found `{}`", operand.ty),
                        );
                    }
                }
                let ty = l.ty.clone();
                mk(tir::ExprKind::Binary(bin_op(op), Box::new(l), Box::new(r)), ty, span)
            }
            _ => {
                let comparison = matches!(op, B::Eq | B::Ne | B::Lt | B::Gt | B::Le | B::Ge);
                let (l, r) = self.operands(l, r, if comparison { None } else { expect });
                let ty = if l.ty == Ty::Never { r.ty.clone() } else { l.ty.clone() };
                if !compatible(&r.ty, &ty) {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        r.span,
                        format!("`{op_name}` needs operands of the same type, found `{}` and `{}`", l.ty, r.ty),
                    );
                    return error_expr(span);
                }
                let ok = match op {
                    B::Add | B::Sub | B::Mul | B::Div | B::Rem => ty.is_numeric(),
                    B::BitAnd | B::BitOr | B::BitXor => matches!(ty, Ty::Int(_) | Ty::Bool),
                    B::Eq | B::Ne => self.globals.has_eq(&ty),
                    B::Lt | B::Gt | B::Le | B::Ge => matches!(ty, Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Char),
                    _ => unreachable!("handled above"),
                };
                if !ok && !matches!(ty, Ty::Error | Ty::Never) {
                    if matches!(ty, Ty::Str | Ty::String) && comparison {
                        return self.unsupported(span, "comparing strings");
                    }
                    self.diags.error(Code::TYPE_MISMATCH, span, format!("`{op_name}` cannot be applied to `{ty}`"));
                    return error_expr(span);
                }
                let result = if comparison { Ty::Bool } else { ty };
                mk(tir::ExprKind::Binary(bin_op(op), Box::new(l), Box::new(r)), result, span)
            }
        }
    }

    fn unary(&mut self, op: ast::UnOp, operand: &ast::Expr, expect: Option<&Ty>, span: Span) -> tir::Expr {
        match op {
            ast::UnOp::Neg => {
                // A minus sign directly before a literal is part of the literal's value, so `-128` fits in `i8`.
                match &operand.kind {
                    ast::ExprKind::Int(v) => {
                        if let Some(Ty::Int(i)) = expect
                            && !i.signed()
                        {
                            self.diags.error(Code::TYPE_MISMATCH, span, format!("cannot negate a `{}`", i.name()));
                            return error_expr(span);
                        }
                        return self.int_literal(*v, true, expect, span);
                    }
                    ast::ExprKind::Float(text) => return self.float_literal(text, true, expect, span),
                    _ => {}
                }
                let e = self.expr(operand, expect.filter(|t| t.is_numeric()));
                let ok = match &e.ty {
                    Ty::Int(i) => i.signed(),
                    Ty::Float(_) | Ty::Error | Ty::Never => true,
                    _ => false,
                };
                if !ok {
                    self.diags.error(Code::TYPE_MISMATCH, span, format!("cannot negate a `{}`", e.ty));
                    return error_expr(span);
                }
                let ty = e.ty.clone();
                mk(tir::ExprKind::Unary(UnOp::Neg, Box::new(e)), ty, span)
            }
            ast::UnOp::Not => {
                let e = self.expr(operand, expect.filter(|t| t.is_int() || **t == Ty::Bool));
                if !matches!(e.ty, Ty::Bool | Ty::Int(_) | Ty::Error | Ty::Never) {
                    self.diags.error(Code::TYPE_MISMATCH, span, format!("`!` cannot be applied to `{}`", e.ty));
                    return error_expr(span);
                }
                let ty = e.ty.clone();
                mk(tir::ExprKind::Unary(UnOp::Not, Box::new(e)), ty, span)
            }
            ast::UnOp::Deref => {
                let e = self.expr(operand, None);
                match &e.ty {
                    Ty::Heap(inner) => {
                        let inner = (**inner).clone();
                        mk(tir::ExprKind::Deref(Box::new(e)), inner, span)
                    }
                    Ty::Ref { inner, .. } => {
                        let inner = (**inner).clone();
                        mk(tir::ExprKind::DerefRef(Box::new(e)), inner, span)
                    }
                    Ty::Error | Ty::Never => error_expr(span),
                    other => {
                        self.diags.error(
                            Code::TYPE_MISMATCH,
                            span,
                            format!("cannot dereference a `{other}`; `*` applies to `Heap<T>` and references"),
                        );
                        error_expr(span)
                    }
                }
            }
            ast::UnOp::Ref | ast::UnOp::RefMut => {
                let mutable = op == ast::UnOp::RefMut;
                let place = self.expr(operand, None);
                if place.ty == Ty::Error {
                    return error_expr(span);
                }
                if !is_place_expr(&place) {
                    self.diags.error(Code::TYPE_MISMATCH, span, "`&` needs a place to borrow");
                    return error_expr(span);
                }
                if mutable && !self.place_mutable(&place) {
                    self.diags.error(
                        Code::MUT_OF_IMMUTABLE,
                        span,
                        "cannot take a mutable reference to a place that is not mutable",
                    );
                }
                let ty = Ty::Ref { mutable, inner: Box::new(place.ty.clone()) };
                mk(tir::ExprKind::Ref { place: Box::new(place), mutable }, ty, span)
            }
        }
    }

    fn call(&mut self, callee: &ast::Expr, args: &[ast::Arg], span: Span, expect: Option<&Ty>) -> tir::Expr {
        if let ast::ExprKind::Field(recv, name) = &callee.kind {
            return self.method_call(recv, name, args, span, expect);
        }
        let (params, ret, target) = match &callee.kind {
            ast::ExprKind::Name { name: id, args: type_args }
                if type_args.is_empty() && self.lookup(&id.name).is_none() =>
            {
                match self.globals.names.get(&id.name) {
                    Some(Global::Func(f)) => {
                        let f = *f;
                        let sig = &self.globals.sigs[f.0];
                        (sig.params.clone(), sig.ret.clone(), Callee::User(f))
                    }
                    Some(Global::Const(_)) => {
                        self.diags.error(
                            Code::TYPE_MISMATCH,
                            id.span,
                            format!("`{}` is a constant, not a function", id.name),
                        );
                        return self.args_then_error(args, span);
                    }
                    None => match builtin(&id.name) {
                        Some(b) => {
                            let sig = builtin_sig(b);
                            (sig.params, sig.ret, Callee::Builtin(b))
                        }
                        None if id.name == "Some" => return self.some(args, expect, span),
                        None => {
                            self.diags.error(Code::UNKNOWN_NAME, id.span, format!("unknown function `{}`", id.name));
                            return self.args_then_error(args, span);
                        }
                    },
                }
            }
            _ => {
                self.unsupported(callee.span, "calling this expression");
                return self.args_then_error(args, span);
            }
        };
        let name = match &callee.kind {
            ast::ExprKind::Name { name, .. } => name.name.clone(),
            _ => String::new(),
        };
        let Some(checked) = self.args(&name, &params, args, None, span) else { return error_expr(span) };
        let kind = match target {
            Callee::User(f) => tir::ExprKind::Call(f, checked),
            Callee::Builtin(b) => tir::ExprKind::Builtin(b, checked),
        };
        mk(kind, ret, span)
    }

    /// `Some(value)`.
    fn some(&mut self, args: &[ast::Arg], expect: Option<&Ty>, span: Span) -> tir::Expr {
        if args.len() != 1 {
            self.diags.error(Code::ARG_COUNT, span, format!("`Some` takes 1 argument, but {} were given", args.len()));
            return self.args_then_error(args, span);
        }
        let inner_expect = match expect {
            Some(Ty::Option(t)) => Some((**t).clone()),
            _ => None,
        };
        let v = self.expr(&args[0].expr, inner_expect.as_ref());
        let v = match &inner_expect {
            Some(t) => self.coerce_arg(v, t),
            None => v,
        };
        let ty = Ty::Option(Box::new(inner_expect.unwrap_or_else(|| v.ty.clone())));
        mk(tir::ExprKind::Some(Box::new(v)), ty, span)
    }

    /// Checks call arguments against the parameter conventions (CV1-CV3) and reports exclusivity violations
    /// (EX1). `receiver` is the method receiver and whether it is borrowed exclusively (`mut self`, or the
    /// receiver of `String.push_str`).
    fn args(
        &mut self,
        name: &str,
        params: &[ParamSig],
        args: &[ast::Arg],
        receiver: Option<(&tir::Expr, bool)>,
        span: Span,
    ) -> Option<Vec<tir::Expr>> {
        if params.len() != args.len() {
            for a in args {
                self.expr(&a.expr, None);
            }
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`{name}` takes {} argument(s), but {} were given", params.len(), args.len()),
            );
            return None;
        }
        let mut checked = Vec::new();
        for (a, p) in args.iter().zip(params) {
            if p.mut_param && !a.mut_marker {
                self.diags.error(
                    Code::MISSING_MUT_MARKER,
                    a.expr.span,
                    format!("this parameter of `{name}` is `mut`; pass the argument as `mut ...`"),
                );
            } else if !p.mut_param && a.mut_marker {
                self.diags.error(
                    Code::UNNEEDED_MUT_MARKER,
                    a.expr.span,
                    format!("this parameter of `{name}` is not `mut`; the argument takes no `mut` marker"),
                );
            }
            let arg = self.expr(&a.expr, Some(&p.ty));
            let arg = self.coerce_arg(arg, &p.ty);
            if p.mut_param && !matches!(arg.ty, Ty::Error) {
                if !is_place_expr(&arg) {
                    self.diags.error(
                        Code::MUT_OF_IMMUTABLE,
                        arg.span,
                        "the argument of a `mut` parameter must be a place",
                    );
                } else if !self.place_mutable(&arg) {
                    self.diags.error(
                        Code::MUT_OF_IMMUTABLE,
                        arg.span,
                        "cannot pass this place as `mut`: it is not mutable",
                    );
                }
            }
            checked.push(arg);
        }
        self.exclusivity(receiver, params, &checked, span);
        Some(checked)
    }

    /// No argument may mention a place that overlaps one passed as `mut` (EX1, EX2).
    fn exclusivity(
        &mut self,
        receiver: Option<(&tir::Expr, bool)>,
        params: &[ParamSig],
        args: &[tir::Expr],
        span: Span,
    ) {
        let mut entries: Vec<(bool, Vec<Place>)> = Vec::new();
        if let Some((receiver, exclusive)) = receiver {
            let mut places = Vec::new();
            place_mentions(receiver, &mut places);
            entries.push((exclusive, places));
        }
        for (p, a) in params.iter().zip(args) {
            let mut places = Vec::new();
            place_mentions(a, &mut places);
            entries.push((p.mut_param, places));
        }
        for i in 0..entries.len() {
            if !entries[i].0 {
                continue;
            }
            for j in 0..entries.len() {
                if i != j && entries[i].1.iter().any(|p| entries[j].1.iter().any(|q| places_overlap(p, q))) {
                    self.diags.error(
                        Code::EXCLUSIVITY,
                        span,
                        "another argument of this call touches the same value as a `mut` argument",
                    );
                    return;
                }
            }
        }
    }

    fn args_then_error(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        for a in args {
            self.expr(&a.expr, None);
        }
        error_expr(span)
    }

    fn method_call(
        &mut self,
        recv: &ast::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        // `Heap.new(value)`: the only associated function of a built-in type that is not written as a type path,
        // because `Heap` alone is not a type.
        if let ast::ExprKind::Name { name: id, args: type_args } = &recv.kind
            && type_args.is_empty()
            && id.name == "Heap"
            && name.name == "new"
            && self.lookup("Heap").is_none()
            && !self.globals.names.contains_key("Heap")
        {
            return self.heap_new(args, expect, span);
        }
        if let Some(ty) = self.type_path(recv) {
            return self.assoc_call(&ty, name, args, span);
        }
        let recv = self.expr(recv, None);
        if recv.ty == Ty::Error {
            return self.args_then_error(args, span);
        }
        // `x.clone()`: the deep copy the compiler derives (CP). It applies to the receiver as written, before
        // any auto-dereference, so cloning a shared reference clones the reference.
        if name.name == "clone" && args.is_empty() && self.globals.is_clone(&recv.ty) {
            let ty = recv.ty.clone();
            return mk(tir::ExprKind::Method(Method::Clone, vec![recv]), ty, span);
        }
        // Field access and method calls see through references (RF1).
        let recv = self.auto_deref(recv);
        if let Some(key) = adt_key(&recv.ty)
            && let Some(&f) = self.globals.methods.get(&(key, name.name.clone()))
        {
            let sig = &self.globals.sigs[f.0];
            if !sig.has_self {
                self.diags.error(
                    Code::UNKNOWN_NAME,
                    name.span,
                    format!("`{}` has no `self` parameter; call it as `{}.{}(...)`", name.name, recv.ty, name.name),
                );
                return self.args_then_error(args, span);
            }
            let (receiver, rest, ret) = (sig.params[0].clone(), sig.params[1..].to_vec(), sig.ret.clone());
            // Only `mut self` borrows the receiver exclusively; `own self` takes it, and read-only `self`
            // only reads it.
            if receiver.by_ref && receiver.exclusive && is_place_expr(&recv) && !self.place_mutable(&recv) {
                let local = place_root(&recv).map(|root| self.locals[root.0].name.clone());
                let whose = local.unwrap_or_else(|| "this receiver".to_string());
                self.diags.error(
                    Code::MUT_OF_IMMUTABLE,
                    recv.span,
                    format!("`{}` changes its receiver, but `{whose}` is not mutable", name.name),
                );
            }
            let receiver_place = Some((&recv, receiver.exclusive));
            let Some(mut checked) = self.args(&name.name, &rest, args, receiver_place, span) else {
                return error_expr(span);
            };
            checked.insert(0, recv);
            return mk(tir::ExprKind::Call(f, checked), ret, span);
        }
        let Some((params, ret)) = method(&name.name).and_then(|m| method_sig(m, &recv.ty)) else {
            self.diags.error(Code::UNKNOWN_NAME, name.span, format!("no method `{}` on type `{}`", name.name, recv.ty));
            return self.args_then_error(args, span);
        };
        let m = method(&name.name).expect("checked above");
        if m == Method::PushStr && is_place_expr(&recv) && !self.place_mutable(&recv) {
            let local = place_root(&recv).map(|root| self.locals[root.0].name.clone());
            let whose = local.unwrap_or_else(|| "this receiver".to_string());
            self.diags.error(
                Code::MUT_OF_IMMUTABLE,
                recv.span,
                format!("`push_str` changes its receiver, but `{whose}` is not mutable"),
            );
        }
        let params: Vec<ParamSig> = params.into_iter().map(ParamSig::plain).collect();
        // `push_str` mutates the receiver, so no other argument may touch it (EX1).
        let receiver_place = Some((&recv, m == Method::PushStr));
        let Some(mut checked) = self.args(&name.name, &params, args, receiver_place, span) else {
            return error_expr(span);
        };
        checked.insert(0, recv);
        mk(tir::ExprKind::Method(m, checked), ret, span)
    }

    /// `Heap.new(value)`.
    fn heap_new(&mut self, args: &[ast::Arg], expect: Option<&Ty>, span: Span) -> tir::Expr {
        let inner = match expect {
            Some(Ty::Heap(t)) => Some((**t).clone()),
            _ => None,
        };
        if args.len() != 1 {
            for a in args {
                self.expr(&a.expr, None);
            }
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`Heap.new` takes 1 argument, but {} were given", args.len()),
            );
            return error_expr(span);
        }
        let value = self.expr(&args[0].expr, inner.as_ref());
        let ty = Ty::Heap(Box::new(value.ty.clone()));
        mk(tir::ExprKind::Builtin(Builtin::HeapNew, vec![value]), ty, span)
    }

    /// `Type.function(...)` or `Enum.Variant(...)`.
    fn assoc_call(&mut self, ty: &Ty, name: &ast::Ident, args: &[ast::Arg], span: Span) -> tir::Expr {
        // The type already failed to resolve; do not report a second error for its associated items.
        if *ty == Ty::Error {
            for a in args {
                self.expr(&a.expr, None);
            }
            return error_expr(span);
        }
        // The associated functions of `String` (the only built-in type with any so far).
        if *ty == Ty::String {
            let b = match name.name.as_str() {
                "new" => Builtin::StringNew,
                "from" => Builtin::StringFrom,
                _ => {
                    self.diags.error(
                        Code::UNKNOWN_NAME,
                        name.span,
                        format!("`String` has no function `{}`", name.name),
                    );
                    return self.args_then_error(args, span);
                }
            };
            let sig = builtin_sig(b);
            let Some(checked) = self.args(&name.name, &sig.params, args, None, span) else {
                return error_expr(span);
            };
            return mk(tir::ExprKind::Builtin(b, checked), sig.ret, span);
        }
        if let Ty::Enum(a) = ty
            && let Some(index) = self.globals.enums[a.id].variants.iter().position(|v| v.name == name.name)
        {
            let v = &self.globals.enums[a.id].variants[index];
            if v.shape != tir::VariantShape::Tuple {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!("variant `{}` is not built with `(...)`", name.name),
                );
                return self.args_then_error(args, span);
            }
            let params: Vec<ParamSig> = v.fields.iter().map(|f| ParamSig::plain(f.ty.clone())).collect();
            let Some(checked) = self.args(&name.name, &params, args, None, span) else { return error_expr(span) };
            let fields = checked.into_iter().enumerate().collect();
            return mk(tir::ExprKind::Variant(index, fields), ty.clone(), span);
        }
        let method = adt_key(ty).and_then(|k| self.globals.methods.get(&(k, name.name.clone())).copied());
        let Some(f) = method else {
            self.diags.error(
                Code::UNKNOWN_NAME,
                name.span,
                format!("`{ty}` has no function or variant `{}`", name.name),
            );
            return self.args_then_error(args, span);
        };
        let sig = &self.globals.sigs[f.0];
        let (params, ret) = (sig.params.clone(), sig.ret.clone());
        let Some(checked) = self.args(&name.name, &params, args, None, span) else { return error_expr(span) };
        mk(tir::ExprKind::Call(f, checked), ret, span)
    }

    /// `base.name`: a field of a struct, or a unit variant (`Color.Red`).
    fn field(&mut self, base: &ast::Expr, name: &ast::Ident, span: Span) -> tir::Expr {
        if let Some(ty) = self.type_path(base) {
            if let Ty::Enum(a) = &ty
                && let Some(index) = self.globals.enums[a.id].variants.iter().position(|v| v.name == name.name)
            {
                if self.globals.enums[a.id].variants[index].shape != tir::VariantShape::Unit {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        span,
                        format!("variant `{}` has fields; give them to build a value", name.name),
                    );
                    return error_expr(span);
                }
                return mk(tir::ExprKind::Variant(index, Vec::new()), ty, span);
            }
            self.diags.error(Code::UNKNOWN_NAME, name.span, format!("`{ty}` has no variant `{}`", name.name));
            return error_expr(span);
        }
        let base = self.expr(base, None);
        let base = self.auto_deref(base);
        let field = match &base.ty {
            Ty::Struct(a) => self.globals.structs[a.id]
                .fields
                .iter()
                .enumerate()
                .find(|(_, f)| f.name == name.name)
                .map(|(i, f)| (i, f.ty.clone())),
            Ty::Error => return error_expr(span),
            _ => None,
        };
        let Some((index, ty)) = field else {
            self.diags.error(Code::UNKNOWN_NAME, name.span, format!("no field `{}` on type `{}`", name.name, base.ty));
            return error_expr(span);
        };
        mk(tir::ExprKind::Field(Box::new(base), index), ty, span)
    }

    /// `Point { x: 1, y }` or `Shape.Rect { w: 1, h: 2 }`.
    fn struct_literal(&mut self, path: &[ast::Ident], fields: &[ast::FieldInit], span: Span) -> tir::Expr {
        let text: Vec<&str> = path.iter().map(|p| p.name.as_str()).collect();
        let text = text.join(".");
        let target = match path {
            [name] => match self.globals.named_type(&name.name) {
                Some(ty @ Ty::Struct(_)) => Some((ty, None)),
                Some(Ty::Error) => None,
                Some(other) => {
                    self.diags.error(Code::TYPE_MISMATCH, name.span, format!("`{other}` is not a struct"));
                    None
                }
                None => {
                    self.diags.error(Code::UNKNOWN_NAME, name.span, format!("unknown type `{}`", name.name));
                    None
                }
            },
            [enum_name, variant] => match self.globals.named_type(&enum_name.name) {
                Some(ty @ Ty::Enum(_)) => {
                    let Ty::Enum(a) = &ty else { unreachable!() };
                    match self.globals.enums[a.id].variants.iter().position(|v| v.name == variant.name) {
                        Some(i) if self.globals.enums[a.id].variants[i].shape == tir::VariantShape::Struct => {
                            Some((ty, Some(i)))
                        }
                        Some(_) => {
                            self.diags.error(
                                Code::TYPE_MISMATCH,
                                span,
                                format!("variant `{text}` has no named fields"),
                            );
                            None
                        }
                        None => {
                            self.diags.error(Code::UNKNOWN_NAME, variant.span, format!("unknown variant `{text}`"));
                            None
                        }
                    }
                }
                _ => {
                    self.diags.error(Code::UNKNOWN_NAME, span, format!("`{text}` is not a struct or an enum variant"));
                    None
                }
            },
            _ => {
                self.diags.error(Code::UNKNOWN_NAME, span, format!("`{text}` is not a struct or an enum variant"));
                None
            }
        };
        let defs: Vec<(String, Ty)> = match &target {
            Some((Ty::Struct(a), _)) => {
                self.globals.structs[a.id].fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect()
            }
            Some((Ty::Enum(a), Some(i))) => {
                self.globals.enums[a.id].variants[*i].fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect()
            }
            _ => Vec::new(),
        };
        let mut given = vec![false; defs.len()];
        let mut checked = Vec::new();
        for f in fields {
            let shorthand;
            let value = match &f.value {
                Some(v) => v,
                None => {
                    shorthand = ast::Expr {
                        kind: ast::ExprKind::Name { name: f.name.clone(), args: Vec::new() },
                        span: f.name.span,
                    };
                    &shorthand
                }
            };
            let index = defs.iter().position(|(n, _)| *n == f.name.name);
            let Some(index) = index.filter(|_| target.is_some()) else {
                if target.is_some() {
                    self.diags.error(
                        Code::UNKNOWN_NAME,
                        f.name.span,
                        format!("`{text}` has no field `{}`", f.name.name),
                    );
                }
                self.expr(value, None);
                continue;
            };
            if std::mem::replace(&mut given[index], true) {
                self.diags.error(Code::TYPE_MISMATCH, f.name.span, format!("field `{}` is given twice", f.name.name));
            }
            let fty = defs[index].1.clone();
            let v = self.expr(value, Some(&fty));
            let v = self.coerce_arg(v, &fty);
            checked.push((index, v));
        }
        let Some((ty, variant)) = target else { return error_expr(span) };
        let missing: Vec<&str> = defs.iter().zip(&given).filter(|(_, g)| !**g).map(|((n, _), _)| n.as_str()).collect();
        if !missing.is_empty() {
            self.diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`{text}` is missing the field(s) `{}`", missing.join("`, `")),
            );
        }
        let kind = match variant {
            Some(i) => tir::ExprKind::Variant(i, checked),
            None => tir::ExprKind::Struct(checked),
        };
        mk(kind, ty, span)
    }

    /// `if let pattern = scrutinee { ... } else { ... }`, checked as a `match` with a catch-all arm.
    fn if_let(
        &mut self,
        pattern: &ast::Pattern,
        scrutinee: &ast::Expr,
        then_block: &ast::Block,
        else_expr: Option<&ast::Expr>,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let scrutinee = self.expr(scrutinee, None);
        let owned = self.owns_scrutinee(&scrutinee);
        self.scopes.push(HashMap::new());
        let pat = self.pattern(pattern, &scrutinee.ty, owned);
        let (then, then_ty) = self.block(then_block, if else_expr.is_some() { expect } else { None });
        self.scopes.pop();
        let then = mk(tir::ExprKind::Block(then), then_ty.clone(), then_block.span);
        let (other, ty) = match else_expr {
            Some(e) => {
                let else_expect = if then_ty == Ty::Never { expect.cloned() } else { Some(then_ty.clone()) };
                let other = self.expr(e, else_expect.as_ref());
                let ty = self.unify(then_ty, &other.ty, other.span, "`if let` and `else` have different types");
                (other, ty)
            }
            None => (mk(tir::ExprKind::Unit, Ty::Unit, span), Ty::Unit),
        };
        let arms =
            vec![tir::Arm { pat, guard: None, body: then }, tir::Arm { pat: Pat::Wild, guard: None, body: other }];
        mk(tir::ExprKind::Match(Box::new(scrutinee), arms), ty, span)
    }

    fn index(&mut self, base: &ast::Expr, index: &ast::Expr, span: Span) -> tir::Expr {
        let base = self.expr(base, None);
        let base = self.auto_deref(base);
        if let ast::ExprKind::Range { lo, hi, inclusive } = &index.kind {
            if *inclusive {
                return self.unsupported(index.span, "an inclusive slice range");
            }
            let usize_ty = Ty::Int(IntTy::Usize);
            let lo = self.expr(lo, Some(&usize_ty));
            let lo = self.expect_ty(lo, &usize_ty);
            let hi = self.expr(hi, Some(&usize_ty));
            let hi = self.expect_ty(hi, &usize_ty);
            let ty = match &base.ty {
                Ty::Str | Ty::String => Ty::Str,
                Ty::Slice(_) => base.ty.clone(),
                Ty::Array(elem, _) => Ty::Slice(elem.clone()),
                Ty::Error => Ty::Error,
                other => {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        base.span,
                        format!("cannot take a slice of a value of type `{other}`"),
                    );
                    return error_expr(span);
                }
            };
            let kind = tir::ExprKind::Slice { base: Box::new(base), lo: Box::new(lo), hi: Box::new(hi) };
            return mk(kind, ty, span);
        }
        let index = self.expr(index, Some(&Ty::Int(IntTy::Usize)));
        let index = self.expect_ty(index, &Ty::Int(IntTy::Usize));
        let ty = match &base.ty {
            Ty::Array(elem, _) | Ty::Slice(elem) => (**elem).clone(),
            Ty::Error => Ty::Error,
            other => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    base.span,
                    format!("cannot index into a value of type `{other}`"),
                );
                return error_expr(span);
            }
        };
        mk(tir::ExprKind::Index(Box::new(base), Box::new(index)), ty, span)
    }

    fn array(&mut self, elems: &[ast::Expr], expect: Option<&Ty>, span: Span) -> tir::Expr {
        let mut elem_ty = match expect {
            Some(Ty::Array(t, _)) => Some((**t).clone()),
            _ => None,
        };
        let mut checked = Vec::new();
        for e in elems {
            let c = self.expr(e, elem_ty.as_ref());
            let c = match &elem_ty {
                Some(t) => self.expect_ty(c, t),
                None => {
                    elem_ty = Some(c.ty.clone());
                    c
                }
            };
            checked.push(c);
        }
        let Some(elem_ty) = elem_ty else {
            self.diags.error(Code::TYPE_MISMATCH, span, "cannot infer the element type of an empty array");
            return error_expr(span);
        };
        let ty = Ty::Array(Box::new(elem_ty), checked.len() as u64);
        mk(tir::ExprKind::Array(checked), ty, span)
    }
}

fn op_text(op: ast::BinOp) -> &'static str {
    use ast::BinOp as B;
    match op {
        B::Or => "||",
        B::And => "&&",
        B::Eq => "==",
        B::Ne => "!=",
        B::Lt => "<",
        B::Gt => ">",
        B::Le => "<=",
        B::Ge => ">=",
        B::BitOr => "|",
        B::BitXor => "^",
        B::BitAnd => "&",
        B::Shl => "<<",
        B::Shr => ">>",
        B::Add => "+",
        B::Sub => "-",
        B::Mul => "*",
        B::Div => "/",
        B::Rem => "%",
    }
}
