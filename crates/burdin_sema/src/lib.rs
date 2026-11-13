//! Name resolution and type checking. Produces the typed tree in [`tir`].
//!
//! Milestone 1 scope: functions, calls, `str` parameters and literals, `let` bindings, blocks, and the output
//! built-ins. Everything else is reported with `Code::UNSUPPORTED` until its milestone.

pub mod tir;

use burdin_diag::{Code, Diagnostics, FileId, Span};
use burdin_syntax::ast;
use std::collections::HashMap;
use tir::{Builtin, FuncId, LocalId, Ty};

struct Signature {
    params: Vec<Ty>,
    ret: Ty,
}

enum Callee {
    User(FuncId),
    Builtin(Builtin),
}

/// Checks a parsed file. Returns `None` if any error was reported (here or earlier).
pub fn check(file: FileId, ast: &ast::File, diags: &mut Diagnostics) -> Option<tir::Program> {
    let mut names: HashMap<String, FuncId> = HashMap::new();
    let mut sigs = Vec::new();
    let mut decls = Vec::new();

    for item in &ast.items {
        let ast::Item::Fn(f) = item;
        if names.contains_key(&f.name.name) || builtin(&f.name.name).is_some() {
            diags.error(Code::UNSUPPORTED, f.name.span, format!("`{}` is defined more than once", f.name.name));
            continue;
        }
        if !f.generics.is_empty() || !f.where_clause.is_empty() {
            diags.error(Code::UNSUPPORTED, f.name.span, "generic functions are not implemented in this compiler yet");
        }
        let mut params = Vec::new();
        for p in &f.params {
            if p.convention != ast::Convention::Read {
                diags.error(
                    Code::UNSUPPORTED,
                    p.span,
                    "`mut` and `own` parameters are not implemented in this compiler yet",
                );
            }
            match &p.kind {
                ast::ParamKind::SelfParam => {
                    diags.error(
                        Code::UNSUPPORTED,
                        p.span,
                        "`self` parameters are not implemented in this compiler yet",
                    );
                    params.push(Ty::Error);
                }
                ast::ParamKind::Named { ty, .. } => params.push(resolve_type(ty, diags)),
            }
        }
        let ret = f.ret.as_ref().map(|t| resolve_type(t, diags)).unwrap_or(Ty::Unit);
        names.insert(f.name.name.clone(), FuncId(decls.len()));
        sigs.push(Signature { params, ret });
        decls.push(f);
    }

    let main = match names.get("main") {
        Some(&id) => {
            let f = decls[id.0];
            if !f.params.is_empty() || sigs[id.0].ret != Ty::Unit {
                diags.error(Code::TYPE_MISMATCH, f.name.span, "`main` must take no parameters and return nothing");
            }
            Some(id)
        }
        None => {
            diags.error(Code::UNKNOWN_NAME, Span::new(file, 0, 0), "the program has no `main` function");
            None
        }
    };

    let mut functions = Vec::new();
    for (i, f) in decls.iter().enumerate() {
        let mut cx = FnCx {
            names: &names,
            sigs: &sigs,
            diags,
            locals: Vec::new(),
            scopes: vec![HashMap::new()],
            ret: sigs[i].ret,
        };
        let mut params = Vec::new();
        for (p, &ty) in f.params.iter().zip(&sigs[i].params) {
            if let ast::ParamKind::Named { name, .. } = &p.kind {
                params.push(cx.declare(&name.name, ty));
            }
        }
        let body = cx.block(&f.body);
        let body_ty = block_ty(&body);
        if !compatible(body_ty, cx.ret) {
            let span = f.body.stmts.last().map(stmt_span).unwrap_or(f.body.span);
            cx.diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`{}` returns `{}`, but its body produces `{}`", f.name.name, cx.ret, body_ty),
            );
        }
        let locals = cx.locals;
        functions.push(tir::Function { name: f.name.name.clone(), params, ret: sigs[i].ret, locals, body });
    }

    if diags.has_errors() {
        return None;
    }
    Some(tir::Program { functions, main: main? })
}

fn builtin(name: &str) -> Option<Builtin> {
    Some(match name {
        "print" => Builtin::Print,
        "println" => Builtin::Println,
        "eprint" => Builtin::Eprint,
        "eprintln" => Builtin::Eprintln,
        _ => return None,
    })
}

fn builtin_sig(_b: Builtin) -> Signature {
    Signature { params: vec![Ty::Str], ret: Ty::Unit }
}

fn resolve_type(ty: &ast::Type, diags: &mut Diagnostics) -> Ty {
    match &ty.kind {
        ast::TypeKind::Unit => Ty::Unit,
        ast::TypeKind::Never => Ty::Never,
        ast::TypeKind::Path(segments) if segments.len() == 1 && segments[0].args.is_empty() => {
            let name = &segments[0].name.name;
            match name.as_str() {
                "str" => Ty::Str,
                "bool" => Ty::Bool,
                "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "isize" | "usize" | "f32" | "f64"
                | "char" | "String" => {
                    diags.error(
                        Code::UNSUPPORTED,
                        ty.span,
                        format!("type `{name}` is not implemented in this compiler yet"),
                    );
                    Ty::Error
                }
                _ => {
                    diags.error(Code::UNKNOWN_NAME, ty.span, format!("unknown type `{name}`"));
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

fn compatible(actual: Ty, expected: Ty) -> bool {
    actual == expected || actual == Ty::Error || expected == Ty::Error || actual == Ty::Never
}

fn block_ty(b: &tir::Block) -> Ty {
    match &b.tail {
        Some(e) => e.ty,
        None => Ty::Unit,
    }
}

fn stmt_span(s: &ast::Stmt) -> Span {
    match s {
        ast::Stmt::Let { span, .. } => *span,
        ast::Stmt::Expr(e) => e.span,
    }
}

struct FnCx<'a> {
    names: &'a HashMap<String, FuncId>,
    sigs: &'a [Signature],
    diags: &'a mut Diagnostics,
    locals: Vec<tir::Local>,
    scopes: Vec<HashMap<String, LocalId>>,
    #[allow(dead_code)]
    ret: Ty,
}

impl FnCx<'_> {
    fn declare(&mut self, name: &str, ty: Ty) -> LocalId {
        let id = LocalId(self.locals.len());
        self.locals.push(tir::Local { name: name.to_string(), ty });
        self.scopes.last_mut().unwrap().insert(name.to_string(), id);
        id
    }

    fn lookup(&self, name: &str) -> Option<LocalId> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    fn unsupported(&mut self, span: Span, what: &str) -> tir::Expr {
        self.diags.error(Code::UNSUPPORTED, span, format!("{what} is not implemented in this compiler yet"));
        error_expr()
    }

    fn block(&mut self, b: &ast::Block) -> tir::Block {
        self.scopes.push(HashMap::new());
        let mut stmts = Vec::new();
        let mut tail = None;
        for (i, s) in b.stmts.iter().enumerate() {
            let last = i + 1 == b.stmts.len();
            match s {
                ast::Stmt::Let { pattern, ty, init, else_block, span } => {
                    let init = self.expr(init);
                    if else_block.is_some() {
                        self.unsupported(*span, "`let ... else`");
                        continue;
                    }
                    let declared = ty.as_ref().map(|t| resolve_type(t, self.diags));
                    if let Some(d) = declared
                        && !compatible(init.ty, d)
                    {
                        self.diags.error(
                            Code::TYPE_MISMATCH,
                            init_span(s),
                            format!("expected `{d}`, found `{}`", init.ty),
                        );
                    }
                    let local_ty = declared.unwrap_or(init.ty);
                    match &pattern.kind {
                        ast::PatternKind::Binding { name, mutable: false } => {
                            let id = self.declare(&name.name, local_ty);
                            stmts.push(tir::Stmt::Let(id, init));
                        }
                        ast::PatternKind::Wild => stmts.push(tir::Stmt::Expr(init)),
                        _ => {
                            self.unsupported(pattern.span, "this `let` pattern");
                        }
                    }
                }
                ast::Stmt::Expr(e) => {
                    let e = self.expr(e);
                    if last {
                        tail = Some(Box::new(e));
                    } else {
                        stmts.push(tir::Stmt::Expr(e));
                    }
                }
            }
        }
        self.scopes.pop();
        tir::Block { stmts, tail }
    }

    fn expr(&mut self, e: &ast::Expr) -> tir::Expr {
        use ast::ExprKind as K;
        match &e.kind {
            K::Str(s) => tir::Expr { kind: tir::ExprKind::Str(s.clone()), ty: Ty::Str },
            K::Bool(b) => tir::Expr { kind: tir::ExprKind::Bool(*b), ty: Ty::Bool },
            K::Unit => tir::Expr { kind: tir::ExprKind::Unit, ty: Ty::Unit },
            K::Block(b) => {
                let b = self.block(b);
                let ty = block_ty(&b);
                tir::Expr { kind: tir::ExprKind::Block(b), ty }
            }
            K::Name(id) => match self.lookup(&id.name) {
                Some(local) => tir::Expr { kind: tir::ExprKind::Local(local), ty: self.locals[local.0].ty },
                None if self.names.contains_key(&id.name) || builtin(&id.name).is_some() => {
                    self.unsupported(id.span, "using a function as a value")
                }
                None => {
                    self.diags.error(Code::UNKNOWN_NAME, id.span, format!("unknown name `{}`", id.name));
                    error_expr()
                }
            },
            K::Call(callee, args) => self.call(e, callee, args),
            K::Int(_) | K::Float(_) | K::Char(_) => self.unsupported(e.span, "numeric and character literals"),
            _ => self.unsupported(e.span, "this expression"),
        }
    }

    fn call(&mut self, e: &ast::Expr, callee: &ast::Expr, args: &[ast::Arg]) -> tir::Expr {
        let target = match &callee.kind {
            ast::ExprKind::Name(id) if self.lookup(&id.name).is_none() => {
                if let Some(&f) = self.names.get(&id.name) {
                    Some((Callee::User(f), id))
                } else if let Some(b) = builtin(&id.name) {
                    Some((Callee::Builtin(b), id))
                } else {
                    self.diags.error(Code::UNKNOWN_NAME, id.span, format!("unknown function `{}`", id.name));
                    None
                }
            }
            _ => {
                self.unsupported(callee.span, "calling this expression");
                None
            }
        };
        let checked: Vec<tir::Expr> = args.iter().map(|a| self.expr(&a.expr)).collect();
        let Some((target, name)) = target else { return error_expr() };
        let (params, ret) = match target {
            Callee::User(f) => (self.sigs[f.0].params.clone(), self.sigs[f.0].ret),
            Callee::Builtin(b) => {
                let sig = builtin_sig(b);
                (sig.params, sig.ret)
            }
        };
        if params.len() != args.len() {
            self.diags.error(
                Code::ARG_COUNT,
                e.span,
                format!("`{}` takes {} argument(s), but {} were given", name.name, params.len(), args.len()),
            );
            return error_expr();
        }
        for ((arg, a), &p) in checked.iter().zip(args).zip(&params) {
            if a.mut_marker {
                self.diags.error(
                    Code::UNSUPPORTED,
                    a.expr.span,
                    "`mut` arguments are not implemented in this compiler yet",
                );
            }
            if !compatible(arg.ty, p) {
                self.diags.error(Code::TYPE_MISMATCH, a.expr.span, format!("expected `{p}`, found `{}`", arg.ty));
            }
        }
        let kind = match target {
            Callee::User(f) => tir::ExprKind::Call(f, checked),
            Callee::Builtin(b) => tir::ExprKind::Builtin(b, checked),
        };
        tir::Expr { kind, ty: ret }
    }
}

fn init_span(s: &ast::Stmt) -> Span {
    match s {
        ast::Stmt::Let { init, .. } => init.span,
        ast::Stmt::Expr(e) => e.span,
    }
}

fn error_expr() -> tir::Expr {
    tir::Expr { kind: tir::ExprKind::Unit, ty: Ty::Error }
}
