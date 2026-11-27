//! Calls: functions, methods, associated functions, extern functions, and the argument conventions.

/// The one-line pointer every exclusivity diagnostic carries to its step-by-step explanation
/// (`spec/check.md`, CK11; issue #593).
const EXCLUSIVITY_HELP: &str = "run `burdin check --explain` to trace the path to this error";
use super::pattern::is_removed_prelude_name;
use super::{FnCx, PathTy, error_expr, mk};
use crate::builtins::{builtin, builtin_sig, method};
use crate::globals::{Callee, Global, Globals, adt_key};
use crate::types::check_storable;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::place::{Place, is_place_expr, place_mentions, place_root, places_overlap};
use burdin_tir::{self as tir, AssertKind, Builtin, FloatTy, FuncId, IntTy, Method, ParamSig, Pat, Ty};

impl FnCx<'_> {
    pub(super) fn call(&mut self, callee: &ast::Expr, args: &[ast::Arg], span: Span, expect: Option<&Ty>) -> tir::Expr {
        // `.name(args)`: an associated function of the expected type (spec/target-typing.md, TT1).
        if let ast::ExprKind::Target(name) = &callee.kind {
            return self.target_call(name, args, span, expect);
        }
        if let ast::ExprKind::Field(recv, name) = &callee.kind {
            return self.method_call(recv, name, &[], args, span, expect);
        }
        // `x.m<T>(...)`, `module.f<T>(...)`, `Type.f<T>(...)`, and `x.downcast<T>()`: type arguments on a member name
        // (`spec/arguments.md`, TA1; AN6 for `is` and `downcast`).
        if let ast::ExprKind::TypeArgs(base, type_args) = &callee.kind {
            if let ast::ExprKind::Field(recv, name) = &base.kind {
                return self.method_call(recv, name, type_args, args, span, expect);
            }
            self.diags.error(Code::UNKNOWN_NAME, span, "type arguments on a call are only allowed on a named member");
            return self.args_then_error(args, span);
        }
        // A local is a closure value, not a generic function, so type arguments on its name are refused.
        if let ast::ExprKind::Name { name: id, args: type_args } = &callee.kind
            && !type_args.is_empty()
            && self.lookup(&id.name).is_some()
        {
            return self.reject_type_args(&id.name, type_args, args, span);
        }
        // A bare name that resolves to a local, or to a variable captured from the enclosing function, is a
        // closure call; only a name with no value meaning is looked up as a function.
        if let ast::ExprKind::Name { name: id, args: type_args } = &callee.kind
            && type_args.is_empty()
        {
            let local = match self.lookup(&id.name) {
                Some(local) => Some(local),
                None if self.capture.is_some() => {
                    if let Some(e) = self.capture_name(&id.name, id.span) {
                        return self.closure_call(e, args, span);
                    }
                    None
                }
                None => None,
            };
            if let Some(local) = local {
                let ty = self.locals[local.0].ty.clone();
                let value = mk(tir::ExprKind::Local(local), ty, id.span);
                return self.closure_call(value, args, span);
            }
        }
        let (params, ret, target) = match &callee.kind {
            ast::ExprKind::Name { name: id, args: type_args } if self.lookup(&id.name).is_none() => {
                // Only a generic function takes type arguments on its name (`f<T>(...)`).
                let generic = matches!(self.globals.value(self.module, &id.name), Some(Global::Generic(_)));
                if !type_args.is_empty() && !generic {
                    return self.reject_type_args(&id.name, type_args, args, span);
                }
                match self.globals.value(self.module, &id.name) {
                    Some(Global::Func(f)) => {
                        if let Some(rejected) = self.edition_gate(f, id, args, span) {
                            return rejected;
                        }
                        // In a test build the prelude's `assert` is the typed one (`spec/errors.md`, ERR9).
                        if self.globals.test_assert == Some(f) {
                            return self.test_assert_call(f, args, span);
                        }
                        // The message form of `assert` and `debug_assert` are compiler-side
                        // (`spec/stdlib.md`, "Memory helpers and assertions").
                        if self.globals.debug_assert_fn == Some(f) {
                            return self.debug_assert_call(args, span);
                        }
                        if self.globals.assert_fn == Some(f) && args.len() > 1 {
                            return self.assert_msg_call(args, span);
                        }
                        self.check_unsafe_call(f, &id.name, id.span);
                        let sig = &self.globals.sigs[f.0];
                        (sig.params.clone(), sig.ret.clone(), Callee::User(f))
                    }
                    Some(Global::Generic(g)) => {
                        if !type_args.is_empty() && self.compiler_generic(g) {
                            return self.reject_type_args(&id.name, type_args, args, span);
                        }
                        if self.globals.test_assert_eq == Some(g) {
                            return self.test_assert_eq_call(g, id, args, span, expect);
                        }
                        if self.globals.test_assert_ne == Some(g) {
                            return self.test_assert_ne_call(g, id, args, span, expect);
                        }
                        if self.globals.snapshot_gen == Some(g) {
                            return self.snapshot_call(id, args, span, expect);
                        }
                        if self.globals.debug_assert_eq_gen == Some(g) {
                            return self.debug_compare_call(args, span, AssertKind::DebugAssertEq);
                        }
                        if self.globals.debug_assert_ne_gen == Some(g) {
                            return self.debug_compare_call(args, span, AssertKind::DebugAssertNe);
                        }
                        if args.len() > 2 && self.globals.assert_eq_gen == Some(g) {
                            return self.assert_compare_msg_call(args, span, AssertKind::Eq);
                        }
                        if args.len() > 2 && self.globals.assert_ne_gen == Some(g) {
                            return self.assert_compare_msg_call(args, span, AssertKind::Ne);
                        }
                        return self.generic_call(g, id, args, type_args, span, expect);
                    }
                    Some(Global::Extern(e)) => return self.extern_call(id, e, args, span),
                    Some(Global::Const(_) | Global::Static(_)) => {
                        self.diags.error(
                            Code::TYPE_MISMATCH,
                            id.span,
                            format!("`{}` is a constant, not a function", id.name),
                        );
                        return self.args_then_error(args, span);
                    }
                    None if id.name == "swap" || id.name == "__swap" => return self.swap_call(args, span),
                    None if id.name == "replace" || id.name == "__replace" => {
                        return self.replace_call(args, span);
                    }
                    None if id.name == "__read" => return self.raw_read_call(args, span),
                    None if id.name == "__write" => return self.raw_write_call(args, span),
                    None if id.name == "__slice_raw" => return self.slice_raw_call(args, span, false),
                    None if id.name == "__iter_mut_raw" => return self.slice_raw_call(args, span, true),
                    None if id.name == "__int_step" => return self.int_step_call(args, span),
                    None if id.name == "__hash_key" => return self.hash_key_call(args, span),
                    None if id.name == "__closure_own" => return self.closure_own_call(args, span),
                    None if id.name == "embed" => return self.embed_call(args, span, expect),
                    None if id.name == "format" && self.lookup("format").is_none() => {
                        return self.format_call(args, span);
                    }
                    None if id.name == "black_box" => return self.black_box_call(args, span),
                    None => match builtin(&id.name) {
                        Some(b @ (Builtin::Print | Builtin::Println | Builtin::Eprint | Builtin::Eprintln))
                            if args.len() > 1 =>
                        {
                            let text = self.format_call(args, span);
                            let text = self.coerce_arg(text, &Ty::Str);
                            return mk(tir::ExprKind::Builtin(b, vec![text]), Ty::Unit, span);
                        }
                        // The unchecked bytes-to-text cast upholds the `str` invariant only when the bytes are
                        // valid UTF-8, which is what `unsafe` promises here (`spec/stdlib.md`).
                        Some(b @ Builtin::ViewAsText) => {
                            if self.unsafe_depth == 0 {
                                self.diags.error(
                                    Code::UNSAFE_REQUIRED,
                                    id.span,
                                    "`__view_as_text` treats bytes as text without checking them; it requires an \
                                     `unsafe` block",
                                );
                            }
                            let sig = builtin_sig(b, self.globals);
                            (sig.params, sig.ret, Callee::Builtin(b))
                        }
                        Some(b) => {
                            let sig = builtin_sig(b, self.globals);
                            (sig.params, sig.ret, Callee::Builtin(b))
                        }
                        None if is_removed_prelude_name(&id.name) => {
                            self.removed_prelude_name(id);
                            return self.args_then_error(args, span);
                        }
                        None => {
                            let fix = self.unknown_name_fix(&id.name, true);
                            let d = self.diags.error(
                                Code::UNKNOWN_NAME,
                                id.span,
                                format!("unknown function `{}`", id.name),
                            );
                            Self::attach_name_fix(fix, id.span, d);
                            return self.args_then_error(args, span);
                        }
                    },
                }
            }
            _ => {
                // A closure value: a local, a field access other than a method, or any expression that
                // evaluates to a closure interface.
                let value = self.expr(callee, None);
                if value.ty == Ty::Error {
                    return self.args_then_error(args, span);
                }
                return self.closure_call(value, args, span);
            }
        };
        let name = match &callee.kind {
            ast::ExprKind::Name { name, .. } => name.name.clone(),
            _ => String::new(),
        };
        let Some(checked) = self.args_ordered(&name, &params, args, None, span) else { return error_expr(span) };
        let kind = match target {
            Callee::User(f) => tir::ExprKind::Call(f, checked.args, checked.order),
            Callee::Builtin(b) => tir::ExprKind::Builtin(b, checked.args),
        };
        mk(kind, ret, span)
    }

    /// A call to an `extern "C"` function: it always requires an `unsafe` context, and its arguments are passed
    /// by value.
    fn extern_call(&mut self, name: &ast::Ident, id: usize, args: &[ast::Arg], span: Span) -> tir::Expr {
        let info = &self.globals.externs[id];
        let sig = info.sig.clone();
        let vararg = info.vararg;
        let symbol = info.symbol.clone();
        // A C declaration has parameters, but they are positional only (spec/arguments.md, AG4).
        if self.reject_named_args(&symbol, args) {
            for a in args {
                self.expr(&a.expr, None);
            }
            return error_expr(span);
        }
        // Visibility is checked where the name is bound (the `use` import, or a module-qualified call), so a
        // `priv` declaration in one module does not hide a public declaration of the same symbol in another.
        if self.unsafe_depth == 0 {
            self.diags.error(
                Code::UNSAFE_REQUIRED,
                span,
                format!("calling the C function `{symbol}` requires an `unsafe` block"),
            );
        }
        let fixed = sig.params.len();
        if args.len() < fixed || (!vararg && args.len() != fixed) {
            for a in args {
                self.expr(&a.expr, None);
            }
            let expected = if vararg { format!("at least {fixed}") } else { fixed.to_string() };
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`{symbol}` takes {expected} argument(s), but {} were given", args.len()),
            );
            return error_expr(span);
        }
        let (fixed_args, extra_args) = args.split_at(fixed);
        let Some(mut checked) = self.args(&name.name, &sig.params, fixed_args, None, span) else {
            return error_expr(span);
        };
        for a in extra_args {
            let value = self.expr(&a.expr, None);
            if value.ty != Ty::Error && !crate::resolve::is_c_abi_value(&value.ty) {
                self.diags.error(
                    Code::C_ABI,
                    value.span,
                    crate::resolve::c_abi_rejection(&value.ty, "passed through `...` to a C function"),
                );
            }
            checked.push(value);
        }
        mk(tir::ExprKind::ExternCall(id, checked), sig.ret, span)
    }

    /// Reports a named argument where the callee takes none: a built-in function or method, a constructor, a
    /// closure value, or an `extern` declaration (`spec/arguments.md`, AG4). The caller keeps checking the
    /// arguments as positional, so the same name is not reported twice.
    pub(super) fn reject_named_args(&mut self, what: &str, args: &[ast::Arg]) -> bool {
        let mut any = false;
        for a in args {
            if let Some(n) = &a.name {
                self.diags.error(Code::INVALID_NAMED_ARGUMENT, n.span, format!("`{what}` takes no named arguments"));
                any = true;
            }
        }
        any
    }

    /// `Some(value)`.
    pub(super) fn some(&mut self, args: &[ast::Arg], expect: Option<&Ty>, span: Span) -> tir::Expr {
        self.reject_named_args("Some", args);
        if args.len() != 1 {
            self.diags.error(Code::ARG_COUNT, span, format!("`Some` takes 1 argument, but {} were given", args.len()));
            return self.args_then_error(args, span);
        }
        let inner_expect = expect.and_then(|t| self.globals.option_payload(t));
        let v = self.expr(&args[0].expr, inner_expect.as_ref());
        let v = match &inner_expect {
            Some(t) => self.coerce_arg(v, t),
            None => v,
        };
        let ty = self.globals.option_ty(inner_expect.unwrap_or_else(|| v.ty.clone()));
        Globals::mk_some(v, ty, span)
    }

    /// `Ok(value)` and `Err(value)` of a `Result<T, E>`. The expected type gives both type arguments, so a
    /// `Result` built without one needs an annotation.
    pub(super) fn result_ctor(&mut self, ok: bool, args: &[ast::Arg], expect: Option<&Ty>, span: Span) -> tir::Expr {
        self.reject_named_args(if ok { "Ok" } else { "Err" }, args);
        if args.len() != 1 {
            let name = if ok { "Ok" } else { "Err" };
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`{name}` takes 1 argument, but {} were given", args.len()),
            );
            return self.args_then_error(args, span);
        }
        let (payload_expect, result_ty) = match expect {
            Some(t) if self.globals.result_parts(t).is_some() => {
                let (t_ok, t_err) = self.globals.result_parts(t).unwrap();
                (if ok { t_ok } else { t_err }, t.clone())
            }
            Some(Ty::Error) => (Ty::Error, Ty::Error),
            _ => {
                self.expr(&args[0].expr, None);
                let name = if ok { "Ok" } else { "Err" };
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!("cannot infer the type of this `{name}`; give the binding a `Result` type"),
                );
                return error_expr(span);
            }
        };
        let value = self.expr(&args[0].expr, Some(&payload_expect));
        let value = self.coerce_arg(value, &payload_expect);
        Globals::mk_result(ok, value, result_ty, span)
    }

    /// The test-build `assert`: the argument is the condition and an optional formatted message, and the
    /// compiler appends the call's source text and location, which the `AssertError` message reports
    /// (`spec/errors.md`, ERR9).
    fn test_assert_call(&mut self, f: FuncId, args: &[ast::Arg], span: Span) -> tir::Expr {
        self.reject_named_args("assert", args);
        let sig = self.globals.sigs[f.0].clone();
        if args.is_empty() {
            self.diags.error(Code::ARG_COUNT, span, "`assert` takes a condition");
            return self.args_then_error(args, span);
        }
        let cond = self.expr(&args[0].expr, Some(&Ty::Bool));
        let cond = self.expect_ty(cond, &Ty::Bool);
        let message = self.test_message(args, 1, span);
        let (condition, location) = self.assertion_texts(span);
        let message_ast = if args.len() > 1 { checked_arg(&args[1], span) } else { checked_arg(&args[0], span) };
        let augmented =
            vec![checked_arg(&args[0], span), message_ast, string_arg(condition, span), string_arg(location, span)];
        let Some(checked) = self.args_pre("assert", &sig.params, &augmented, None, span, Some(vec![cond, message]))
        else {
            return error_expr(span);
        };
        mk(tir::ExprKind::Call(f, checked, Vec::new()), sig.ret, span)
    }

    /// The test-build `assert_eq` (`spec/errors.md`, ERR9).
    fn test_assert_eq_call(
        &mut self,
        g: usize,
        id: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        self.test_compare_call(g, id, args, span, expect, "assert_eq")
    }

    /// The test-build `assert_ne` (`spec/errors.md`, ERR9).
    fn test_assert_ne_call(
        &mut self,
        g: usize,
        id: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        self.test_compare_call(g, id, args, span, expect, "assert_ne")
    }

    /// The test-build comparison assertions: the two values are checked here so that `T` is inferred from
    /// them, and the message (empty when none was written), the call's source text, and its location are
    /// appended (`spec/errors.md`, ERR9). The generic call then infers `T`, checks its `Eq + Debug` bounds,
    /// and instantiates the standard library's function.
    fn test_compare_call(
        &mut self,
        g: usize,
        id: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
        name: &str,
    ) -> tir::Expr {
        self.reject_named_args(name, args);
        if args.len() < 2 {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`{name}` takes two values, but {} were given", args.len()),
            );
            return self.args_then_error(args, span);
        }
        let (a, b) = self.comparison_args(args, name, span);
        let message = self.test_message(args, 2, span);
        let (condition, location) = self.assertion_texts(span);
        let condition_value = mk(tir::ExprKind::Str(condition.clone()), Ty::Str, span);
        let location_value = mk(tir::ExprKind::Str(location.clone()), Ty::Str, span);
        let message_ast = if args.len() > 2 { checked_arg(&args[2], span) } else { checked_arg(&args[0], span) };
        let augmented = vec![
            checked_arg(&args[0], span),
            checked_arg(&args[1], span),
            message_ast,
            string_arg(condition, span),
            string_arg(location, span),
        ];
        let checked = vec![a, b, message, condition_value, location_value];
        self.generic_call_checked(g, id, &augmented, checked, span, expect)
    }

    /// `assert_snapshot(value[, @"inline"][, display: bool])` (`spec/test-runner.md`, "Snapshot tests"). The call
    /// becomes `snapshot_debug(value, location, inline)` or `snapshot_display(...)`: the location is how the
    /// runner finds the test, the snapshot file, and an inline snapshot's literal, which it reads from the source
    /// rather than from the program. Anything else in the argument list is E0172.
    fn snapshot_call(&mut self, id: &ast::Ident, args: &[ast::Arg], span: Span, expect: Option<&Ty>) -> tir::Expr {
        let mut positional: Vec<&ast::Arg> = Vec::new();
        let mut display = false;
        let mut valid = true;
        for arg in args {
            match &arg.name {
                Some(name) if name.name == "display" => match arg.expr.kind {
                    ast::ExprKind::Bool(value) => display = value,
                    _ => {
                        self.diags.error(
                            Code::INVALID_SNAPSHOT,
                            arg.expr.span,
                            "`display:` takes `true` or `false` written out, so the snapshot's format is known \
                             when the test is compiled",
                        );
                        valid = false;
                    }
                },
                Some(name) => {
                    self.diags.error(
                        Code::INVALID_SNAPSHOT,
                        name.span,
                        format!("`assert_snapshot` has no argument named `{}`; it takes `display:`", name.name),
                    );
                    valid = false;
                }
                None => positional.push(arg),
            }
        }
        let inline = match positional.as_slice() {
            [_] => false,
            [_, second] if matches!(second.expr.kind, ast::ExprKind::InlineSnapshot(_)) => true,
            [_, second] => {
                self.diags.error(
                    Code::INVALID_SNAPSHOT,
                    second.expr.span,
                    "the second argument of `assert_snapshot` is an inline snapshot, `@\"...\"`",
                );
                self.expr(&second.expr, None);
                valid = false;
                true
            }
            [] => {
                self.diags.error(Code::INVALID_SNAPSHOT, span, "`assert_snapshot` takes the value to snapshot");
                return error_expr(span);
            }
            [_, _, extra, ..] => {
                self.diags.error(
                    Code::INVALID_SNAPSHOT,
                    extra.expr.span,
                    "`assert_snapshot` takes a value and at most one inline snapshot",
                );
                for arg in &positional[2..] {
                    if !matches!(arg.expr.kind, ast::ExprKind::InlineSnapshot(_)) {
                        self.expr(&arg.expr, None);
                    }
                }
                valid = false;
                true
            }
        };
        let value = self.expr(&positional[0].expr, None);
        let value = self.auto_deref(value);
        let helper = if display { self.globals.snapshot_display_gen } else { self.globals.snapshot_debug_gen };
        let Some(helper) = helper.filter(|_| valid) else {
            return error_expr(span);
        };
        let (_, location) = self.assertion_texts(span);
        let location_value = mk(tir::ExprKind::Str(location.clone()), Ty::Str, span);
        let inline_value = mk(tir::ExprKind::Bool(inline), Ty::Bool, span);
        let bool_arg = ast::Arg {
            name: None,
            mut_marker: false,
            mut_span: None,
            expr: ast::Expr { kind: ast::ExprKind::Bool(inline), span },
        };
        let augmented = vec![checked_arg(positional[0], span), string_arg(location, span), bool_arg];
        self.generic_call_checked(helper, id, &augmented, vec![value, location_value, inline_value], span, expect)
    }

    /// The message value of a typed assertion: the arguments from `first` on formatted, or an empty `str` when
    /// the call wrote none.
    fn test_message(&mut self, args: &[ast::Arg], first: usize, span: Span) -> tir::Expr {
        if args.len() > first {
            self.assertion_message(&args[first..], span)
        } else {
            mk(tir::ExprKind::Str(String::new()), Ty::Str, span)
        }
    }

    /// The source text of the whole call at `span` and its `path:line:col`, for a typed assertion (`ERR9`).
    fn assertion_texts(&self, span: Span) -> (String, String) {
        let file = self.sources.get(span.file);
        let text = file.text.get(span.lo as usize..span.hi as usize).map(str::trim).filter(|text| !text.is_empty());
        // The call's own text is what a failure shows; the fallback only covers an empty span.
        let condition = text.unwrap_or("assert").to_string();
        let (path, line, col) = self.location(span);
        (condition, format!("{path}:{line}:{col}"))
    }

    /// `res.context(message)`: `Ok` passes through; `Err(e)` becomes an `Err` whose dynamic `AnyError` wraps `e` and
    /// prints `message: e`. The message is checked in the `Err` arm only, so it is evaluated only on an error, and
    /// the source error's trace frames are kept (`spec/errors.md`, ERR6).
    fn context_call(&mut self, recv: tir::Expr, args: &[ast::Arg], span: Span) -> tir::Expr {
        self.reject_named_args("context", args);
        let Some((ok, err)) = self.globals.result_parts(&recv.ty) else {
            unreachable!("`context` is intercepted only on a `Result`")
        };
        if args.len() != 1 {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`.context` takes 1 argument, but {} were given", args.len()),
            );
            return self.args_then_error(args, span);
        }
        let Some(error_ty) = self.globals.error_ty.clone() else {
            self.diags.error(Code::ERROR_CONVERSION, span, "`.context` needs the standard library's `AnyError` type");
            return self.args_then_error(args, span);
        };
        let Some(context) = self.globals.error_context else {
            self.diags.error(
                Code::ERROR_CONVERSION,
                span,
                "`.context` needs the standard library's `error_context` helper",
            );
            return self.args_then_error(args, span);
        };
        let err_ty = err.clone();
        if err_ty != error_ty && !self.globals.implements_error(&err_ty, &self.bounds) {
            self.diags.error(
                Code::ERROR_CONVERSION,
                span,
                format!("`.context` needs an error type, but `{err_ty}` does not implement `Error`"),
            );
            return self.args_then_error(args, span);
        }
        // A temporary receiver is moved into a compiler-made local, so its parts move and drop correctly.
        let (value, owned_temp) = self.own_scrutinee_temp(recv);
        let owned = self.owns_scrutinee(&value);
        let (ok_id, ok_mode) = self.payload_local("ok", ok.clone(), owned);
        let (err_id, err_mode) = self.payload_local("err", err_ty.clone(), owned);
        let result_ty = self.globals.result_ty(ok.clone(), error_ty.clone());
        let ok_body =
            Globals::mk_result(true, mk(tir::ExprKind::Local(ok_id), ok.clone(), span), result_ty.clone(), span);
        let message = self.expr(&args[0].expr, Some(&Ty::Str));
        let message = self.coerce_arg(message, &Ty::Str);
        let source = mk(tir::ExprKind::Local(err_id), err_ty.clone(), span);
        let source = if err_ty == error_ty { source } else { self.coerce_error(source, &error_ty) };
        let call = mk(tir::ExprKind::Call(context, vec![source, message], Vec::new()), error_ty.clone(), span);
        let err_body = Globals::mk_result(false, call, result_ty.clone(), span);
        let arms = vec![
            tir::Arm { pat: Pat::Variant(0, vec![Pat::Binding(ok_id, ok_mode)]), guard: None, body: ok_body },
            tir::Arm { pat: Pat::Variant(1, vec![Pat::Binding(err_id, err_mode)]), guard: None, body: err_body },
        ];
        let matched = mk(tir::ExprKind::Match(Box::new(value), arms), result_ty, span);
        self.wrap_scrutinee(matched, owned_temp, span)
    }

    /// The message form of `assert`: `assert(cond, "message {x}")` checks the condition and formats the
    /// message, which the panic reports (`spec/stdlib.md`, "Memory helpers and assertions").
    fn assert_msg_call(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        self.reject_named_args("assert", args);
        let cond = self.expr(&args[0].expr, Some(&Ty::Bool));
        let cond = self.expect_ty(cond, &Ty::Bool);
        let message = self.assertion_message(&args[1..], span);
        mk(tir::ExprKind::Builtin(Builtin::Assertion(AssertKind::Assert), vec![cond, message]), Ty::Unit, span)
    }

    /// The message forms of `assert_eq` and `assert_ne`: the two values must have the same `Eq` type, and the
    /// message (from the third argument on) is formatted and reported by the panic.
    fn assert_compare_msg_call(&mut self, args: &[ast::Arg], span: Span, kind: AssertKind) -> tir::Expr {
        let name = if kind == AssertKind::Ne { "assert_ne" } else { "assert_eq" };
        self.reject_named_args(name, args);
        let (l, r) = self.comparison_args(args, name, span);
        let message = self.assertion_message(&args[2..], span);
        mk(tir::ExprKind::Builtin(Builtin::Assertion(kind), vec![l, r, message]), Ty::Unit, span)
    }

    /// `debug_assert(cond[, message])`: the compiler keeps the check in a debug build and drops it from a
    /// release build without evaluating either argument (`spec/stdlib.md`).
    fn debug_assert_call(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        self.reject_named_args("debug_assert", args);
        if args.is_empty() {
            self.diags.error(Code::ARG_COUNT, span, "`debug_assert` takes a condition");
            return self.args_then_error(args, span);
        }
        let cond = self.expr(&args[0].expr, Some(&Ty::Bool));
        let cond = self.expect_ty(cond, &Ty::Bool);
        let mut values = vec![cond];
        if args.len() > 1 {
            values.push(self.assertion_message(&args[1..], span));
        }
        mk(tir::ExprKind::Builtin(Builtin::Assertion(AssertKind::DebugAssert), values), Ty::Unit, span)
    }

    /// `debug_assert_eq(a, b[, message])` and `debug_assert_ne`: the comparison forms of `debug_assert`.
    fn debug_compare_call(&mut self, args: &[ast::Arg], span: Span, kind: AssertKind) -> tir::Expr {
        let name = if kind == AssertKind::DebugAssertNe { "debug_assert_ne" } else { "debug_assert_eq" };
        self.reject_named_args(name, args);
        if args.len() < 2 {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`{name}` takes two values, but {} were given", args.len()),
            );
            return self.args_then_error(args, span);
        }
        let (l, r) = self.comparison_args(args, name, span);
        let mut values = vec![l, r];
        if args.len() > 2 {
            values.push(self.assertion_message(&args[2..], span));
        }
        mk(tir::ExprKind::Builtin(Builtin::Assertion(kind), values), Ty::Unit, span)
    }

    /// The two values of a comparison assertion (`assert_eq`, `assert_ne`, and their `debug_` forms): the left
    /// value fixes the type, the right one takes it, and both must support `==` (`spec/stdlib.md`).
    fn comparison_args(&mut self, args: &[ast::Arg], name: &str, span: Span) -> (tir::Expr, tir::Expr) {
        // Check the left value first; the right one takes its type, so a literal right side fits a typed left
        // side, exactly as the ordinary `assert_eq` call does.
        let l = self.expr(&args[0].expr, None);
        let l = self.auto_deref(l);
        let r = self.expr(&args[1].expr, Some(&l.ty));
        let r = self.coerce_arg(r, &l.ty);
        if !self.globals.has_eq_in(&l.ty, &self.bounds) && !self.param_has(&l.ty, crate::generics::Bound::Eq) {
            self.diags.error(
                Code::BOUND_NOT_SATISFIED,
                span,
                format!("`{name}` needs `Eq` values, but `{}` does not support `==`", l.ty),
            );
        }
        (l, r)
    }

    /// The formatted message of an assertion's message form. It is a `str` view of the formatted `String`,
    /// which the panic reports when the assertion fails (`spec/stdlib.md`).
    fn assertion_message(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        let text = self.format_call(args, span);
        self.coerce_arg(text, &Ty::Str)
    }

    /// `swap(mut a, mut b)`: two places of one type, both mutable and non-overlapping.
    fn swap_call(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        self.reject_named_args("swap", args);
        if args.len() != 2 {
            self.diags.error(Code::ARG_COUNT, span, format!("`swap` takes 2 arguments, but {} were given", args.len()));
            return self.args_then_error(args, span);
        }
        let a = self.expr_arg(&args[0].expr, None, true);
        let b = self.expr_arg(&args[1].expr, None, true);
        for e in [&a, &b] {
            if e.ty != Ty::Error && (!is_place_expr(e) || !self.place_mutable(e)) {
                self.diags.error(Code::MUT_OF_IMMUTABLE, e.span, "`swap` needs two mutable places");
            }
        }
        if !crate::types::compatible(&a.ty, &b.ty) {
            self.diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`swap` needs two values of the same type, found `{}` and `{}`", a.ty, b.ty),
            );
        }
        let mut pa = Vec::new();
        let mut pb = Vec::new();
        place_mentions(&a, &mut pa);
        place_mentions(&b, &mut pb);
        if pa.iter().any(|p| pb.iter().any(|q| places_overlap(p, q))) {
            self.diags
                .error(Code::EXCLUSIVITY, span, "`swap` needs two places that do not overlap")
                .help(EXCLUSIVITY_HELP);
        }
        mk(tir::ExprKind::Builtin(Builtin::Swap, vec![a, b]), Ty::Unit, span)
    }

    /// `__closure_own(f)`: a closure value (or a parameter bounded by a closure interface) as an owned `FnOnce` of the
    /// same signature. The representation is the same, so only the type changes: the capture record is moved, and a
    /// borrow capture inside it is no longer second-class, which is why the operation needs `unsafe`. It exists for
    /// the standard library's scoped tasks (`spec/tasks.md`, TK7).
    fn closure_own_call(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        self.reject_named_args("__closure_own", args);
        if args.len() != 1 {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`__closure_own` takes 1 argument, but {} were given", args.len()),
            );
            return self.args_then_error(args, span);
        }
        if self.unsafe_depth == 0 {
            self.diags.error(Code::UNSAFE_REQUIRED, span, "`__closure_own` requires an `unsafe` block");
        }
        let f = self.expr(&args[0].expr, None);
        if f.ty == Ty::Error {
            return error_expr(span);
        }
        let Some((sig, _)) = self.globals.fn_call_sig(&f.ty, &self.bounds) else {
            self.diags.error(Code::TYPE_MISMATCH, span, format!("`__closure_own` needs a closure, found `{}`", f.ty));
            return error_expr(span);
        };
        let ty =
            Ty::Fn { sig: std::rc::Rc::new(sig), kind: tir::FnKind::FnOnce, borrowed: false, send: tir::FnSend::yes() };
        mk(tir::ExprKind::Builtin(Builtin::ClosureOwn, vec![f]), ty, span)
    }

    /// `black_box(value)`: the same value through a barrier the optimizer cannot see through, so a benchmark's
    /// measured work is not deleted (`spec/bench.md`, BN9). The value is moved in and moved out again; a view is
    /// rejected with E0120, because the barrier hands back a fresh value with no storage a view could point into.
    fn black_box_call(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        self.reject_named_args("black_box", args);
        if args.len() != 1 {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`black_box` takes 1 argument, but {} were given", args.len()),
            );
            return self.args_then_error(args, span);
        }
        let value = self.expr(&args[0].expr, None);
        if value.ty.is_second_class() {
            // The barrier hands back a fresh value with no storage a view could point into, so a view is
            // rejected the way a stored position rejects one, and the expression stops here rather than
            // reporting the view again downstream.
            self.diags.error(
                Code::SECOND_CLASS_POSITION,
                span,
                format!("`{}` is a view of other storage, so it cannot be the argument of `black_box`", value.ty),
            );
            return error_expr(span);
        }
        let ty = value.ty.clone();
        mk(tir::ExprKind::Builtin(Builtin::BlackBox, vec![value]), ty, span)
    }

    /// `__read(p)`: moves the value out of raw memory (`unsafe`). The result is owned by the caller.
    fn raw_read_call(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        self.reject_named_args("__read", args);
        if args.len() != 1 {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`__read` takes 1 argument, but {} were given", args.len()),
            );
            return self.args_then_error(args, span);
        }
        if self.unsafe_depth == 0 {
            self.diags.error(
                Code::UNSAFE_REQUIRED,
                span,
                "`__read` moves a value out of raw memory; it requires an `unsafe` block",
            );
        }
        let p = self.expr(&args[0].expr, None);
        let Ty::CPtr(inner) = &p.ty else {
            if p.ty != Ty::Error {
                self.diags.error(Code::TYPE_MISMATCH, p.span, format!("`__read` needs a `CPtr<T>`, found `{}`", p.ty));
            }
            return error_expr(span);
        };
        let ty = (**inner).clone();
        mk(tir::ExprKind::Builtin(Builtin::RawRead, vec![p]), ty, span)
    }

    /// `__write(p, value)`: moves `value` into raw memory without dropping the old contents (`unsafe`).
    fn raw_write_call(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        self.reject_named_args("__write", args);
        if args.len() != 2 {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`__write` takes 2 arguments, but {} were given", args.len()),
            );
            return self.args_then_error(args, span);
        }
        if self.unsafe_depth == 0 {
            self.diags.error(
                Code::UNSAFE_REQUIRED,
                span,
                "`__write` stores into raw memory; it requires an `unsafe` block",
            );
        }
        let p = self.expr(&args[0].expr, None);
        let Ty::CPtr(inner) = &p.ty else {
            if p.ty != Ty::Error {
                self.diags.error(Code::TYPE_MISMATCH, p.span, format!("`__write` needs a `CPtr<T>`, found `{}`", p.ty));
            }
            self.expr(&args[1].expr, None);
            return error_expr(span);
        };
        let ty = (**inner).clone();
        let value = self.expr(&args[1].expr, Some(&ty));
        let value = self.coerce_arg(value, &ty);
        mk(tir::ExprKind::Builtin(Builtin::RawWrite, vec![p, value]), Ty::Unit, span)
    }

    /// `__slice_raw(p, n)`: the `[T]` view of `n` elements at `p` (`unsafe`).
    fn slice_raw_call(&mut self, args: &[ast::Arg], span: Span, iter_mut: bool) -> tir::Expr {
        let name = if iter_mut { "__iter_mut_raw" } else { "__slice_raw" };
        self.reject_named_args(name, args);
        if args.len() != 2 {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`{name}` takes 2 arguments, but {} were given", args.len()),
            );
            return self.args_then_error(args, span);
        }
        if self.unsafe_depth == 0 {
            self.diags.error(
                Code::UNSAFE_REQUIRED,
                span,
                format!("`{name}` makes a view of raw memory; it requires an `unsafe` block"),
            );
        }
        let p = self.expr(&args[0].expr, None);
        let Ty::CPtr(inner) = &p.ty else {
            if p.ty != Ty::Error {
                self.diags.error(Code::TYPE_MISMATCH, p.span, format!("`{name}` needs a `CPtr<T>`, found `{}`", p.ty));
            }
            self.expr(&args[1].expr, None);
            return error_expr(span);
        };
        let ty = if iter_mut { Ty::IterMut(inner.clone()) } else { Ty::Slice(inner.clone()) };
        let usize_ty = Ty::Int(IntTy::Usize);
        let n = self.expr(&args[1].expr, Some(&usize_ty));
        let n = self.coerce_arg(n, &usize_ty);
        let op = if iter_mut { Builtin::IterMutRaw } else { Builtin::SliceRaw };
        mk(tir::ExprKind::Builtin(op, vec![p, n]), ty, span)
    }

    /// `__hash_key(&k)`: the structural hash of the value `k` references (`std`'s `Map`).
    fn hash_key_call(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        self.reject_named_args("__hash_key", args);
        if args.len() != 1 {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`__hash_key` takes 1 argument, but {} were given", args.len()),
            );
            return self.args_then_error(args, span);
        }
        let key = self.expr(&args[0].expr, None);
        if !matches!(key.ty, Ty::Ref { .. } | Ty::Error) {
            self.diags.error(
                Code::TYPE_MISMATCH,
                key.span,
                format!("`__hash_key` needs a reference, found `{}`", key.ty),
            );
            return error_expr(span);
        }
        mk(tir::ExprKind::Builtin(Builtin::HashKey, vec![key]), Ty::Int(IntTy::U64), span)
    }

    /// `__int_step(x, delta)`: the integer `x` moved by `delta`, wrapping (`std`'s range iterators).
    fn int_step_call(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        self.reject_named_args("__int_step", args);
        if args.len() != 2 {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`__int_step` takes 2 arguments, but {} were given", args.len()),
            );
            return self.args_then_error(args, span);
        }
        let x = self.expr(&args[0].expr, None);
        let delta = self.expr(&args[1].expr, Some(&Ty::Int(IntTy::I64)));
        let delta = self.coerce_arg(delta, &Ty::Int(IntTy::I64));
        // A generic body is checked once with the parameter itself; each instance is checked with its integer.
        if !matches!(x.ty, Ty::Int(_) | Ty::Param(_) | Ty::Error) {
            self.diags.error(Code::TYPE_MISMATCH, x.span, format!("`__int_step` needs an integer, found `{}`", x.ty));
            return error_expr(span);
        }
        let ty = x.ty.clone();
        mk(tir::ExprKind::Builtin(Builtin::IntStep, vec![x, delta]), ty, span)
    }

    /// `replace(mut place, value)`: stores `value` and returns what `place` held.
    fn replace_call(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        self.reject_named_args("replace", args);
        if args.len() != 2 {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`replace` takes 2 arguments, but {} were given", args.len()),
            );
            return self.args_then_error(args, span);
        }
        let place = self.expr_arg(&args[0].expr, None, true);
        if place.ty != Ty::Error && (!is_place_expr(&place) || !self.place_mutable(&place)) {
            self.diags.error(Code::MUT_OF_IMMUTABLE, place.span, "`replace` needs a mutable place");
        }
        let value = self.expr(&args[1].expr, Some(&place.ty));
        let value = self.coerce_arg(value, &place.ty);
        let ty = place.ty.clone();
        let mut pa = Vec::new();
        let mut pv = Vec::new();
        place_mentions(&place, &mut pa);
        place_mentions(&value, &mut pv);
        if pa.iter().any(|p| pv.iter().any(|q| places_overlap(p, q))) {
            self.diags
                .error(Code::EXCLUSIVITY, span, "`replace` needs a place that the new value does not mention")
                .help(EXCLUSIVITY_HELP);
        }
        mk(tir::ExprKind::Builtin(Builtin::Replace, vec![place, value]), ty, span)
    }

    /// Checks call arguments against the parameter conventions (CV1-CV3) and reports exclusivity violations
    /// (EX1). `receiver` is the method receiver and whether it is borrowed exclusively (`mut self`, or the
    /// receiver of `String.push_str`).
    pub(super) fn args(
        &mut self,
        name: &str,
        params: &[ParamSig],
        args: &[ast::Arg],
        receiver: Option<(&tir::Expr, bool)>,
        span: Span,
    ) -> Option<Vec<tir::Expr>> {
        self.args_pre(name, params, args, receiver, span, None)
    }

    /// Like [`Self::args`], also returning the source evaluation order when named arguments were written out of
    /// parameter order (`spec/arguments.md`, AG6).
    pub(super) fn args_ordered(
        &mut self,
        name: &str,
        params: &[ParamSig],
        args: &[ast::Arg],
        receiver: Option<(&tir::Expr, bool)>,
        span: Span,
    ) -> Option<CheckedArgs> {
        self.args_pre_ordered(name, params, args, receiver, span, None)
    }

    /// Like `args`, for arguments that were already checked (a generic call checks them first to infer its type
    /// arguments). `pre` has one entry per written argument, in written order.
    pub(super) fn args_pre(
        &mut self,
        name: &str,
        params: &[ParamSig],
        args: &[ast::Arg],
        receiver: Option<(&tir::Expr, bool)>,
        span: Span,
        pre_values: Option<Vec<tir::Expr>>,
    ) -> Option<Vec<tir::Expr>> {
        self.args_pre_ordered(name, params, args, receiver, span, pre_values).map(|c| c.args)
    }

    /// Like [`Self::args_pre`], also returning the source evaluation order when named arguments were written out of
    /// parameter order (`spec/arguments.md`, AG6).
    pub(super) fn args_pre_ordered(
        &mut self,
        name: &str,
        params: &[ParamSig],
        args: &[ast::Arg],
        receiver: Option<(&tir::Expr, bool)>,
        span: Span,
        pre_values: Option<Vec<tir::Expr>>,
    ) -> Option<CheckedArgs> {
        let Some(slots) = self.arg_slots(name, params, args, span) else {
            if pre_values.is_none() {
                for a in args {
                    self.expr(&a.expr, None);
                }
            }
            return None;
        };
        // The pre-checked values fill the first written arguments; anything left is checked from the AST (the
        // test-build `assert` supplies only the condition and leaves its two appended strings to be checked).
        let mut pending: Vec<Option<tir::Expr>> = (0..args.len()).map(|_| None).collect();
        if let Some(values) = pre_values {
            for (slot, value) in pending.iter_mut().zip(values) {
                *slot = Some(value);
            }
        }
        // Build the source evaluation order: every written argument in source order, then omitted defaults
        // (which are pure compile-time constants) in parameter order (`spec/arguments.md`, AG6).
        let mut order: Vec<usize> = Vec::with_capacity(params.len());
        for ai in 0..args.len() {
            if let Some(pi) = slots.iter().position(|s| *s == Some(ai)) {
                order.push(pi);
            }
        }
        for (pi, slot) in slots.iter().enumerate() {
            if slot.is_none() {
                order.push(pi);
            }
        }
        let mut checked_slots: Vec<Option<tir::Expr>> = (0..params.len()).map(|_| None).collect();
        for &pi in &order {
            let p = &params[pi];
            let arg = match slots[pi] {
                Some(ai) => {
                    let a = &args[ai];
                    if p.mut_param && !a.mut_marker {
                        let position = a.expr.span;
                        self.diags
                            .error(
                                Code::MISSING_MUT_MARKER,
                                position,
                                format!("this parameter of `{name}` is `mut`; pass the argument as `mut ...`"),
                            )
                            .suggest_safe(
                                Span::new(position.file, position.lo, position.lo),
                                "add `mut` before this argument",
                                "mut ",
                            );
                    } else if !p.mut_param && a.mut_marker {
                        let error = self.diags.error(
                            Code::UNNEEDED_MUT_MARKER,
                            a.expr.span,
                            format!("this parameter of `{name}` is not `mut`; the argument takes no `mut` marker"),
                        );
                        if let Some(mut_span) = a.mut_span {
                            // Delete the marker and the whitespace after it, so applying the edit leaves plain
                            // `name`.
                            error.suggest_safe(
                                Span::new(mut_span.file, mut_span.lo, a.expr.span.lo),
                                "remove the `mut` marker",
                                "",
                            );
                        }
                    }
                    match pending[ai].take() {
                        Some(arg) => arg,
                        None => self.expr_arg(&a.expr, Some(&p.ty), p.mut_param),
                    }
                }
                // The argument is omitted: insert the value checked at the declaration. A missing parameter
                // without a default was reported by `arg_slots`.
                None => p.default.clone().expect("a missing required argument was reported"),
            };
            // A parameter that borrows its argument may not borrow a `static mut` (spec/globals.md, GL4).
            if !p.own {
                self.static_mut_borrowed(&arg, arg.span);
            }
            let arg = self.coerce_arg(arg, &p.ty);
            if p.mut_param && !matches!(arg.ty, Ty::Error) {
                if !is_place_expr(&arg) {
                    self.diags.error(
                        Code::MUT_OF_IMMUTABLE,
                        arg.span,
                        "the argument of a `mut` parameter must be a place",
                    );
                } else if !self.place_mutable(&arg) {
                    let fix = self.mut_binding_fix(&arg);
                    let d = self.diags.error(
                        Code::MUT_OF_IMMUTABLE,
                        arg.span,
                        "cannot pass this place as `mut`: it is not mutable",
                    );
                    Self::attach_mut_fix(fix, d);
                }
            }
            checked_slots[pi] = Some(arg);
        }
        let checked: Vec<tir::Expr> = checked_slots.into_iter().map(|e| e.expect("every slot filled")).collect();
        self.exclusivity(receiver, params, &checked, span);
        if order.iter().copied().eq(0..params.len()) {
            order.clear();
        }
        Some(CheckedArgs { args: checked, order })
    }

    /// Matches the written arguments to parameters (`spec/arguments.md`, AG4-AG7): a positional argument fills
    /// the next parameter without one, and a named argument (`name: value`) fills the parameter with that name.
    /// Reports an unknown or repeated name, a positional argument after a named one (E0214), and a missing or
    /// extra argument (E0004); returns `None` when the call cannot be matched.
    pub(super) fn arg_slots(
        &mut self,
        name: &str,
        params: &[ParamSig],
        args: &[ast::Arg],
        span: Span,
    ) -> Option<Vec<Option<usize>>> {
        let mut slots: Vec<Option<usize>> = vec![None; params.len()];
        let mut named_seen = false;
        let mut name_error = false;
        let mut extra_positional = 0usize;
        let mut ok = true;
        for (ai, a) in args.iter().enumerate() {
            if let Some(n) = &a.name {
                named_seen = true;
                match params.iter().position(|p| p.name.as_deref() == Some(n.name.as_str())) {
                    Some(pi) => {
                        if let Some(first) = slots[pi] {
                            let first_span = args[first].name.as_ref().map(|i| i.span).unwrap_or(args[first].expr.span);
                            self.diags
                                .error(
                                    Code::INVALID_NAMED_ARGUMENT,
                                    n.span,
                                    format!("the parameter `{}` is given more than once", n.name),
                                )
                                .note(first_span, "the first value is here");
                            ok = false;
                            name_error = true;
                        } else {
                            slots[pi] = Some(ai);
                        }
                    }
                    None => {
                        if params.iter().all(|p| p.name.is_none()) {
                            self.diags.error(
                                Code::INVALID_NAMED_ARGUMENT,
                                n.span,
                                format!("`{name}` takes no named arguments"),
                            );
                        } else {
                            let names: Vec<String> =
                                params.iter().filter_map(|p| p.name.as_deref()).map(|n| format!("`{n}`")).collect();
                            self.diags.error(
                                Code::INVALID_NAMED_ARGUMENT,
                                n.span,
                                format!(
                                    "`{name}` has no parameter named `{}`; its parameters are {}",
                                    n.name,
                                    join_names(&names)
                                ),
                            );
                        }
                        ok = false;
                        name_error = true;
                    }
                }
            } else {
                if named_seen {
                    self.diags.error(
                        Code::INVALID_NAMED_ARGUMENT,
                        a.expr.span,
                        "a positional argument cannot follow a named one; name every later argument",
                    );
                    ok = false;
                    name_error = true;
                }
                match slots.iter().position(|s| s.is_none()) {
                    Some(pi) => slots[pi] = Some(ai),
                    None => extra_positional += 1,
                }
            }
        }
        // A name error already says the call cannot be matched; the count message would only repeat it.
        if !name_error {
            let missing: Vec<&ParamSig> = params
                .iter()
                .enumerate()
                .filter(|(i, p)| slots[*i].is_none() && p.default.is_none())
                .map(|(_, p)| p)
                .collect();
            if !missing.is_empty() {
                if params.iter().all(|p| p.name.is_none()) {
                    self.diags.error(
                        Code::ARG_COUNT,
                        span,
                        format!("`{name}` takes {} argument(s), but {} were given", params.len(), args.len()),
                    );
                } else {
                    let names: Vec<String> =
                        missing.iter().map(|p| format!("`{}`", p.name.as_deref().unwrap_or("self"))).collect();
                    self.diags.error(
                        Code::ARG_COUNT,
                        span,
                        format!("`{name}` is missing an argument for {}; it has no default", join_names(&names)),
                    );
                }
                ok = false;
            }
            if extra_positional > 0 {
                self.diags.error(
                    Code::ARG_COUNT,
                    span,
                    format!(
                        "`{name}` takes {} parameter(s), but {extra_positional} extra positional argument(s) were given",
                        params.len()
                    ),
                );
                ok = false;
            }
        }
        ok.then_some(slots)
    }

    /// No argument may mention a place that overlaps one passed as `mut` (EX1, EX2).
    pub(super) fn exclusivity(
        &mut self,
        receiver: Option<(&tir::Expr, bool)>,
        params: &[ParamSig],
        args: &[tir::Expr],
        span: Span,
    ) {
        let mut entries: Vec<(bool, Span, Vec<Place>)> = Vec::new();
        if let Some((receiver, exclusive)) = receiver {
            let mut places = Vec::new();
            place_mentions(receiver, &mut places);
            // A chain that is computed here (`xs.iter()`) is a temporary view of the places it mentions, not a
            // place the call borrows exclusively: its loans are the borrow checker's business (RF5), so `xs.iter()`
            // as the receiver of `zip` may sit beside another argument that reads `xs`.
            let temporary_view = receiver.ty.is_second_class() && !is_place_expr(receiver);
            entries.push((exclusive && !temporary_view, receiver.span, places));
        }
        for (p, a) in params.iter().zip(args) {
            let mut places = Vec::new();
            place_mentions(a, &mut places);
            entries.push((p.mut_param, a.span, places));
        }
        for i in 0..entries.len() {
            if !entries[i].0 {
                continue;
            }
            for j in 0..entries.len() {
                if i != j && entries[i].2.iter().any(|p| entries[j].2.iter().any(|q| places_overlap(p, q))) {
                    let other = entries[j].1;
                    let d = self.diags.error(
                        Code::EXCLUSIVITY,
                        span,
                        "another argument of this call touches the same value as a `mut` argument",
                    );
                    d.label(span, "conflicts with another argument");
                    d.label(other, "this argument touches the same place");
                    d.note_without_span(
                        "spec/memory-rules.md, EX1: a `mut` argument requires exclusive access for the call",
                    );
                    d.help(EXCLUSIVITY_HELP);
                    return;
                }
            }
        }
    }

    pub(super) fn args_then_error(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        for a in args {
            self.expr(&a.expr, None);
        }
        error_expr(span)
    }

    /// Reports explicit type arguments on a callee that takes none, then checks the arguments as usual (TA5).
    pub(super) fn reject_type_args(
        &mut self,
        what: &str,
        explicit: &[ast::Type],
        args: &[ast::Arg],
        span: Span,
    ) -> tir::Expr {
        if let (Some(first), Some(last)) = (explicit.first(), explicit.last()) {
            self.diags.error(
                Code::TYPE_MISMATCH,
                first.span.to(last.span),
                format!("`{what}` takes no type arguments"),
            );
        }
        self.args_then_error(args, span)
    }

    /// Whether `name` on a value of type `ty` (through references) is a method of the program that has type
    /// parameters of its own. Only such a method, or an associated function like it, takes explicit type arguments.
    pub(super) fn user_generic_method(&self, ty: &Ty, name: &str) -> bool {
        let mut base = ty;
        while let Ty::Ref { inner, .. } = base {
            base = &**inner;
        }
        adt_key(base)
            .and_then(|key| self.globals.methods.get(&(key, name.to_string())))
            .is_some_and(|f| self.globals.method_templates.contains_key(f))
    }

    /// Whether `ty` (through references) is an `any` value or the dynamic `AnyError`, whose `is` and `downcast` take a
    /// type argument (AN6).
    pub(super) fn is_any_like(&self, ty: &Ty) -> bool {
        let mut base = ty;
        while let Ty::Ref { inner, .. } = base {
            base = &**inner;
        }
        matches!(base, Ty::Any(_)) || self.globals.is_error_ty(base)
    }

    /// Whether `g` is one of the compiler-known assertion functions, which take no type arguments.
    fn compiler_generic(&self, g: usize) -> bool {
        [
            self.globals.test_assert_eq,
            self.globals.test_assert_ne,
            self.globals.snapshot_gen,
            self.globals.debug_assert_eq_gen,
            self.globals.debug_assert_ne_gen,
            self.globals.assert_eq_gen,
            self.globals.assert_ne_gen,
        ]
        .contains(&Some(g))
    }

    /// Whether `recv.name` is one of the built-in static functions written as a path through a name that is not a
    /// type (`env.args`, `process.exit`, `Heap.new`, `CPtr.null`, ...). They take no type arguments.
    fn builtin_static(&self, recv: &ast::Expr, name: &ast::Ident) -> bool {
        let ast::ExprKind::Name { name: id, args } = &recv.kind else { return false };
        args.is_empty()
            && self.lookup(&id.name).is_none()
            && self.globals.value(self.module, &id.name).is_none()
            && matches!(
                (id.name.as_str(), name.name.as_str()),
                ("env", "args")
                    | ("process", "exit")
                    | ("Heap", "new")
                    | ("Heap", "new_in")
                    | ("List", "from")
                    | ("Map", "from")
                    | ("CPtr", "null")
            )
    }

    pub(super) fn method_call(
        &mut self,
        recv: &ast::Expr,
        name: &ast::Ident,
        explicit: &[ast::Type],
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        // The built-in static functions below take no type arguments (TA5).
        if !explicit.is_empty() && self.builtin_static(recv, name) {
            return self.reject_type_args(&name.name, explicit, args, span);
        }
        // The prelude modules: `env.args()` and `process.exit`.
        if let ast::ExprKind::Name { name: id, args: type_args } = &recv.kind
            && type_args.is_empty()
            && self.lookup(&id.name).is_none()
            && self.globals.value(self.module, &id.name).is_none()
        {
            if id.name == "env" && name.name == "args" {
                if !args.is_empty() {
                    self.diags.error(Code::ARG_COUNT, span, "`env.args` takes no arguments");
                    return self.args_then_error(args, span);
                }
                let ty = self.globals.list_of(Ty::String);
                return mk(tir::ExprKind::Builtin(Builtin::EnvArgs, Vec::new()), ty, span);
            }
            if id.name == "process" && name.name == "exit" {
                let sig = builtin_sig(Builtin::ProcessExit, self.globals);
                let Some(checked) =
                    self.args("process.exit", &[ParamSig::plain(Ty::Int(IntTy::I32))], args, None, span)
                else {
                    return error_expr(span);
                };
                return mk(tir::ExprKind::Builtin(Builtin::ProcessExit, checked), sig.ret, span);
            }
        }
        // `Heap.new(value)` and `Heap.new_in(alloc, value)`: the associated functions of a built-in type that is
        // not written as a type path, because `Heap` alone is not a type.
        if let ast::ExprKind::Name { name: id, args: type_args } = &recv.kind
            && type_args.is_empty()
            && self.lookup(&id.name).is_none()
            && self.globals.value(self.module, &id.name).is_none()
        {
            if id.name == "Heap" && name.name == "new" {
                return self.heap_new(args, expect, span);
            }
            if id.name == "Heap" && name.name == "new_in" {
                return self.heap_new_in(args, span);
            }
            if (id.name == "List" || id.name == "Map") && name.name == "from" {
                return self.untyped_collection_from(&id.name, name, args, expect, span);
            }
        }
        // `CPtr.null()`, typed by the expected type.
        if let ast::ExprKind::Name { name: id, args: type_args } = &recv.kind
            && type_args.is_empty()
            && id.name == "CPtr"
            && name.name == "null"
            && self.lookup("CPtr").is_none()
            && self.globals.value(self.module, "CPtr").is_none()
        {
            return self.cptr_null(args, expect, span);
        }
        // `Option.Some(x)`, `Result.Ok(v)`, and `Result.Err(e)`: the arguments come from the value and the expected type.
        if let Some(owner) = self.prelude_head(recv) {
            if !explicit.is_empty() {
                return self.reject_type_args(&name.name, explicit, args, span);
            }
            return match (owner, name.name.as_str()) {
                ("Option", "Some") => self.some(args, expect, span),
                ("Result", "Ok") => self.result_ctor(true, args, expect, span),
                ("Result", "Err") => self.result_ctor(false, args, expect, span),
                _ => {
                    self.diags.error(
                        Code::UNKNOWN_NAME,
                        name.span,
                        format!("`{owner}` has no function or variant `{}`", name.name),
                    );
                    self.args_then_error(args, span)
                }
            };
        }
        if let Some(path) = self.type_path(recv) {
            return match path {
                PathTy::Ty(ty) => self.assoc_call(&ty, name, explicit, args, span, expect),
                PathTy::Template(g) => self.assoc_call_template(g, name, explicit, args, expect, span),
            };
        }
        // A module-qualified type: `sh.Square.new()`.
        if let Some(ty) = self.qualified_type(recv) {
            return self.assoc_call(&ty, name, explicit, args, span, expect);
        }
        // A module-qualified function: `math.add(...)`, `sh.area(...)`.
        if let Some((target, item)) = self.module_member(recv, name) {
            return self.module_call(target, &item, explicit, args, span, expect);
        }
        let recv = self.expr(recv, None);
        // `x.is<T>()` and `x.downcast<T>()` on an `any` value (AN6) are compiler-known methods, not templates.
        if !explicit.is_empty() && matches!(name.name.as_str(), "is" | "downcast") && self.is_any_like(&recv.ty) {
            return self.member_type_args_call(recv, name, explicit, args, span);
        }
        self.method_call_on_with(recv, name, explicit, args, span, expect)
    }

    /// The type of `e` when it is a range argument to `get`: a range expression, or a local whose value is a
    /// range. `None` for any other expression, which `get` treats as an index.
    fn range_argument(&self, e: &ast::Expr) -> Option<Ty> {
        match &e.kind {
            ast::ExprKind::Range { inclusive, .. } => Some(self.globals.range_ty(Ty::Int(IntTy::Usize), *inclusive)),
            ast::ExprKind::Name { name, args } if args.is_empty() => {
                let local = self.lookup(&name.name)?;
                let ty = self.locals[local.0].ty.clone();
                // A stored range is a slice range only when its endpoints are `usize`; there is no implicit
                // conversion, so `Range<i32>` is an ordinary type error at the index form.
                let (end, _) = self.globals.range_parts(&ty)?;
                (end == Ty::Int(IntTy::Usize)).then_some(ty)
            }
            _ => None,
        }
    }

    /// `array[0..N]`: the implicit slice view an array takes for a slice method or extension (`spec/stdlib.md`,
    /// "Slices"). Slicing is what takes the loan on the array, so the borrow checker sees the view like any other.
    fn array_slice_view(&self, base: tir::Expr, elem: &Ty, n: u64, span: Span) -> tir::Expr {
        let usize_ty = Ty::Int(IntTy::Usize);
        let lo = mk(tir::ExprKind::Int(0), usize_ty.clone(), span);
        let hi = mk(tir::ExprKind::Int(n as i128), usize_ty, span);
        let kind = tir::ExprKind::Slice { base: Box::new(base), lo: Box::new(lo), hi: Box::new(hi), inclusive: false };
        mk(kind, Ty::Slice(Box::new(elem.clone())), span)
    }

    /// The expression whose mutability a mutating slice method needs: the base of a slice view, or the list a
    /// `List.as_slice()` call views. `None` when the receiver is neither, so the ordinary rule applies.
    fn slice_view_source<'e>(&self, e: &'e tir::Expr) -> Option<&'e tir::Expr> {
        match &e.kind {
            tir::ExprKind::Slice { base, .. } => Some(base),
            tir::ExprKind::Method(tir::Method::AsSlice, args) => args.first(),
            // `List.as_slice()` is a method of `std`'s `List`.
            tir::ExprKind::Call(f, args, _) => {
                let list = args.first().filter(|a| self.globals.list_parts(&a.ty).is_some())?;
                let key = adt_key(&list.ty)?;
                (self.globals.methods.get(&(key, "as_slice".to_string())) == Some(f)).then_some(list)
            }
            _ => None,
        }
    }

    /// The name of the place a `mut self` extension over a slice view cannot write through, when the view is over
    /// an immutable place. `None` when the receiver is fine, including a receiver that is not a view at all (the
    /// ordinary rules for those are checked separately).
    pub(super) fn slice_receiver_problem(&self, e: &tir::Expr) -> Option<String> {
        let base = self.slice_view_source(e)?;
        if is_place_expr(base) && self.place_mutable(base) {
            return None;
        }
        Some(
            place_root(base)
                .map(|root| self.locals[root.0].name.clone())
                .unwrap_or_else(|| "this receiver".to_string()),
        )
    }

    /// `recv.name(args)` for a receiver that is already checked.
    pub(super) fn method_call_on(
        &mut self,
        recv: tir::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        self.method_call_on_with(recv, name, &[], args, span, expect)
    }

    /// `method_call_on` with the explicit type arguments of the call (`x.m<T>(...)`, TA1).
    pub(super) fn method_call_on_with(
        &mut self,
        recv: tir::Expr,
        name: &ast::Ident,
        explicit: &[ast::Type],
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        if recv.ty == Ty::Error {
            return self.args_then_error(args, span);
        }
        // Only a method of the program with type parameters of its own, or the `collect` of an iterator, which
        // names its target (IT17), takes explicit type arguments (TA5).
        if !explicit.is_empty()
            && !self.user_generic_method(&recv.ty, &name.name)
            && !self.iterator_collect(&recv.ty, &name.name)
        {
            return self.reject_type_args(&name.name, explicit, args, span);
        }
        // `x.clone()`: the deep copy the compiler derives (CP). It applies to the receiver as written, before
        // any auto-dereference, so cloning a shared reference clones the reference.
        if name.name == "clone" && args.is_empty() && self.globals.is_clone(&recv.ty) {
            let ty = recv.ty.clone();
            return mk(tir::ExprKind::Method(Method::Clone, vec![recv]), ty, span);
        }
        // Field access and method calls see through references (RF1).
        let recv = self.auto_deref(recv);
        // A value of an `impl Interface` return type has only the interface's methods (`fncx/opaque.rs`).
        if self.globals.opaque_of(&recv.ty).is_some() {
            return self.opaque_method_call(recv, explicit, name, args, span, expect);
        }
        // The map operations that look a key up or store one need a key with `==` (and a hash that agrees with it):
        // without it the call is a missing bound, not a missing method.
        if matches!(
            name.name.as_str(),
            "insert" | "get" | "get_mut" | "get_or" | "get_or_insert" | "remove" | "remove_entry" | "contains_key"
        ) && let Some((key, _, _)) = self.globals.map_parts(&recv.ty)
            && !self.globals.has_key_eq_in(&key, &self.bounds)
        {
            self.diags.error(
                Code::BOUND_NOT_SATISFIED,
                span,
                format!(
                    "cannot use `{key}` as a map key: it needs `Eq`, and a hand-written `Eq` implementation also needs `Hash`"
                ),
            );
            return self.args_then_error(args, span);
        }
        // `xs.drain(lo..=hi)` on a `List` is the inclusive form, a separate method of `std`'s `List`.
        if name.name == "drain"
            && args.len() == 1
            && matches!(args[0].expr.kind, ast::ExprKind::Range { inclusive: true, .. })
            && self.globals.list_parts(&recv.ty).is_some()
        {
            let inclusive = ast::Ident { name: "drain_inclusive".to_string(), span: name.span };
            return self.method_call_on_with(recv, &inclusive, explicit, args, span, expect);
        }
        // `xs.get(range): Option<[T]>`: the checked form of slicing. A range expression or a range-valued
        // local picks it; any other argument is the index form the method table provides (`spec/stdlib.md`,
        // "Slices").
        if name.name == "get"
            && args.len() == 1
            && matches!(recv.ty, Ty::Slice(_))
            && let Some(range_ty) = self.range_argument(&args[0].expr)
        {
            self.reject_named_args("get", args);
            let arg = self.expr(&args[0].expr, Some(&range_ty));
            let arg = self.expect_ty(arg, &range_ty);
            let ret = self.globals.option_ty(recv.ty.clone());
            return mk(tir::ExprKind::Method(Method::SliceGetRange, vec![recv, arg]), ret, span);
        }
        // `x.to_string()` on a value whose type implements `Display` (spec/format.md).
        if name.name == "to_string"
            && args.is_empty()
            && self.to_string_via_display(&recv.ty)
            && !adt_key(&recv.ty).is_some_and(|key| self.globals.methods.contains_key(&(key, "to_string".to_string())))
        {
            return self.display_to_string(recv, span);
        }
        if let Ty::Any(any) = recv.ty.clone() {
            return self.any_method_call(recv, &any, name, args, span);
        }
        if matches!(recv.ty, Ty::Param(_) | Ty::Projection { .. }) {
            return self.param_method_call(recv, name, explicit, args, span, expect);
        }
        // `v.into()`: the conversion into the type the context expects (spec/stdlib.md, FR4), unless the receiver
        // has an `into` method of its own.
        if name.name == "into"
            && args.is_empty()
            && !adt_key(&recv.ty).is_some_and(|key| self.globals.methods.contains_key(&(key, "into".to_string())))
        {
            return self.convert_into(recv, expect, span);
        }
        if let Some(key) = adt_key(&recv.ty)
            && let Some(&f) = self.globals.methods.get(&(key, name.name.clone()))
        {
            self.check_member_access(&recv.ty, key, name);
            self.check_unsafe_call(f, &name.name, name.span);
            // A method with its own type parameters is instantiated for the arguments this call infers.
            if let Some(info) = self.globals.method_templates.get(&f).cloned() {
                return self.generic_method_call(info, recv, &name.name, explicit, args, span, expect);
            }
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
                let fix = self.mut_binding_fix(&recv);
                let d = self.diags.error(
                    Code::MUT_OF_IMMUTABLE,
                    recv.span,
                    format!("`{}` changes its receiver, but `{whose}` is not mutable", name.name),
                );
                Self::attach_mut_fix(fix, d);
            }
            // A borrowing receiver may not be a `static mut` (spec/globals.md, GL4).
            if !receiver.own {
                self.static_mut_borrowed(&recv, recv.span);
            }
            let receiver_place = Some((&recv, receiver.exclusive));
            let Some(checked) = self.args_ordered(&name.name, &rest, args, receiver_place, span) else {
                return error_expr(span);
            };
            let checked = checked.with_receiver(recv);
            return mk(tir::ExprKind::Call(f, checked.args, checked.order), ret, span);
        }
        if name.name == "hash" && args.is_empty() && self.globals.has_hash_in(&recv.ty, &self.bounds) {
            return mk(tir::ExprKind::Method(Method::Hash, vec![recv]), Ty::Int(IntTy::U64), span);
        }
        if name.name == "hash_from" && self.globals.has_hash_in(&recv.ty, &self.bounds) {
            return self.hash_from_call(recv, args, span);
        }
        // `a.cmp(b)` on every type that supports `<` through `Ord` (`spec/stdlib.md`, `Ordering`).
        if name.name == "cmp"
            && args.len() == 1
            && self.globals.satisfies(&recv.ty, &crate::generics::Bound::Ord, &self.bounds)
            && let Some(g) = self.globals.std_generic("ord_cmp")
        {
            return self.generic_call_with_receiver(g, recv, "cmp", args, span);
        }
        // A field that holds a closure is callable: `holder.f(x)` calls the closure in `f` when no method of that
        // name exists (CL6). The field wins only when its type is a closure interface.
        if let Some((index, field_ty)) = self.closure_field(&recv, &name.name) {
            let field = mk(tir::ExprKind::Field(Box::new(recv), index), field_ty, name.span);
            return self.closure_call(field, args, span);
        }
        // `value.serialize(mut s)` on a built-in type: the serialization helpers of `std` write it (SER14).
        if let Some(e) = self.builtin_serialize_method(recv.clone(), name, args, span) {
            return e;
        }
        // `fmt` and `debug` of the built-in types, which the formatting helpers of `std` implement.
        if let Some(e) = self.builtin_format_method(recv.clone(), name, args, span) {
            return e;
        }
        // The iteration protocol: `next`, `sum`, the eager `List` helpers, and the `Iterator` default methods.
        if let Some(e) = self.iterator_method(recv.clone(), name, explicit, args, span, expect) {
            return e;
        }
        // `res.context(message)` is compiler-known on `Result` (`spec/errors.md`, ERR6).
        if name.name == "context" && self.globals.result_parts(&recv.ty).is_some() {
            return self.context_call(recv, args, span);
        }
        let recv_ty = recv.ty.clone();
        let Some((m, params, ret)) = method(self.globals, &name.name, &recv.ty) else {
            // An array has the slice methods through an implicit view of its elements (`spec/stdlib.md`,
            // "Slices"). The view is taken only when a slice method or a slice extension of that name exists, so
            // an unrelated call still reports the array's own type.
            if let Ty::Array(elem, n) = recv.ty.clone() {
                let view = self.array_slice_view(recv.clone(), &elem, n, span);
                if method(self.globals, &name.name, &view.ty).is_some() {
                    return self.method_call_on(view, name, args, span, expect);
                }
                if let Some(e) = self.extension_call(view, name, args, span) {
                    return e;
                }
            }
            // No method the type declares and none the compiler provides: a visible extension method may apply
            // (`spec/extensions.md`, EX3). It is tried last, so a method always wins.
            let list_elem = self.globals.list_parts(&recv.ty).map(|(elem, _)| elem);
            if let Some(e) = self.extension_call(recv, name, args, span) {
                return e;
            }
            // Three methods of a `List` exist only when its element has the interface they need; without it the
            // call is a missing bound, not a missing method.
            if let Some(elem) = list_elem {
                let needs = match name.name.as_str() {
                    "extend_from_slice" => Some("Clone"),
                    "contains" => Some("Eq"),
                    "sort" => Some("Ord"),
                    _ => None,
                };
                if let Some(iface) = needs {
                    self.diags.error(
                        Code::BOUND_NOT_SATISFIED,
                        name.span,
                        format!("`{elem}` does not implement `{iface}`, which `{}` requires", name.name),
                    );
                    return self.args_then_error(args, span);
                }
            }
            self.report_no_method(&recv_ty, &name.name, name.span);
            return self.args_then_error(args, span);
        };
        if m.mutates_receiver() && !matches!(recv.ty, Ty::Error) {
            // A mutating method writes through its receiver. The receiver is a mutable place, or a slice view
            // over one: `xs[0..2].sort()` and `xs.as_slice().swap(0, 1)` need a mutable `xs`
            // (`spec/stdlib.md`, "Slices").
            let checked = self.slice_view_source(&recv).unwrap_or(&recv);
            if !is_place_expr(checked) {
                self.diags.error(
                    Code::MUT_OF_IMMUTABLE,
                    recv.span,
                    format!("`{}` changes its receiver, so the receiver must be a mutable place", name.name),
                );
            } else if !self.place_mutable(checked) {
                let local = place_root(checked).map(|root| self.locals[root.0].name.clone());
                let whose = local.unwrap_or_else(|| "this receiver".to_string());
                let fix = self.mut_binding_fix(checked);
                let d = self.diags.error(
                    Code::MUT_OF_IMMUTABLE,
                    recv.span,
                    format!("`{}` changes its receiver, but `{whose}` is not mutable", name.name),
                );
                Self::attach_mut_fix(fix, d);
            }
        }
        match (&m, &recv.ty) {
            // The slice operations that clone their elements need `Clone`, and `copy_from_slice` needs `Copy`
            // (`spec/stdlib.md`, "Slices").
            (Method::SliceFill | Method::SliceCloneFromSlice | Method::SliceToList, Ty::Slice(elem))
                if !self.globals.satisfies(elem, &crate::generics::Bound::Clone, &self.bounds) =>
            {
                self.diags.error(
                    Code::BOUND_NOT_SATISFIED,
                    span,
                    format!("this slice operation needs `Clone`, but `{elem}` does not implement it"),
                );
            }
            (Method::SliceCopyFromSlice, Ty::Slice(elem))
                if !self.globals.satisfies(elem, &crate::generics::Bound::Copy, &self.bounds) =>
            {
                self.diags.error(
                    Code::BOUND_NOT_SATISFIED,
                    span,
                    format!("`copy_from_slice` needs `Copy`, but `{elem}` does not implement it"),
                );
            }
            _ => {}
        }
        // A mutating method borrows the receiver exclusively (EX1).
        let receiver_place = Some((&recv, m.mutates_receiver()));
        if m == Method::ListDrain {
            let Some(mut checked) = self.drain_args(&recv, &params, args, span) else {
                return error_expr(span);
            };
            checked.insert(0, recv);
            return mk(tir::ExprKind::Method(m, checked), ret, span);
        }
        let Some(mut checked) = self.args(&name.name, &params, args, receiver_place, span) else {
            return error_expr(span);
        };
        checked.insert(0, recv);
        mk(tir::ExprKind::Method(m, checked), ret, span)
    }

    /// `x.hash_from(state)` on a `Hash` receiver: the value folded into a running hash (`spec/stdlib.md`, HS3).
    pub(super) fn hash_from_call(&mut self, recv: tir::Expr, args: &[ast::Arg], span: Span) -> tir::Expr {
        let u64_ty = Ty::Int(IntTy::U64);
        let params = [ParamSig::plain(u64_ty.clone())];
        let Some(mut checked) = self.args("hash_from", &params, args, None, span) else { return error_expr(span) };
        checked.insert(0, recv);
        mk(tir::ExprKind::Method(Method::HashFrom, checked), u64_ty, span)
    }

    /// The one argument of `List.drain`: a range of `usize` in either spelling. `1..=3` has a different type
    /// from `1..3`, so the argument is checked here instead of against the method's fixed parameter type.
    fn drain_args(
        &mut self,
        recv: &tir::Expr,
        params: &[ParamSig],
        args: &[ast::Arg],
        span: Span,
    ) -> Option<Vec<tir::Expr>> {
        self.reject_named_args("drain", args);
        if args.len() != 1 {
            for a in args {
                self.expr(&a.expr, None);
            }
            self.diags.error(Code::ARG_COUNT, span, format!("`drain` takes 1 argument, but {} were given", args.len()));
            return None;
        }
        let hint = self.globals.range_ty(Ty::Int(IntTy::Usize), false);
        let arg = self.expr(&args[0].expr, Some(&hint));
        match &arg.ty {
            t if self.globals.range_parts(t).is_some_and(|(e, _)| e == Ty::Int(IntTy::Usize)) => {}
            Ty::Error => return None,
            other => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    arg.span,
                    format!("`drain` takes a range of `usize`, found `{other}`"),
                );
                return None;
            }
        }
        let checked = vec![arg];
        self.exclusivity(Some((recv, true)), params, &checked, span);
        Some(checked)
    }

    /// `x.name(args)` through an extension method visible in this module (`spec/extensions.md`, EX3). The
    /// extension must be declared here or imported by name; the caller reports the missing method when `None`.
    pub(super) fn extension_call(
        &mut self,
        recv: tir::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<tir::Expr> {
        let candidates = self.globals.modules[self.module].extensions.get(&name.name)?.clone();
        for id in candidates {
            let ext = self.globals.extensions[id].clone();
            if !crate::extensions::extension_applies(self.globals, &ext, &recv.ty) {
                continue;
            }
            return Some(self.extension_call_inner(&ext, recv, name, args, span));
        }
        None
    }

    /// `Type.name(recv, args)` through an extension method `name` that applies to `Type` (`spec/extensions.md`, EX9).
    /// The first argument is the receiver and the rest are the arguments of `recv.name(args)`, with the same
    /// conventions. `None` when no visible extension applies to `Type` or the call has no receiver argument; the
    /// caller then reports the missing function.
    pub(super) fn extension_assoc_call(
        &mut self,
        ty: &Ty,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<tir::Expr> {
        let first = args.first()?;
        let candidates = self.globals.modules[self.module].extensions.get(&name.name)?.clone();
        let ext = candidates
            .into_iter()
            .map(|id| self.globals.extensions[id].clone())
            .find(|ext| crate::extensions::extension_applies(self.globals, ext, ty))?;
        let recv = self.expr(&first.expr, Some(ty));
        // A receiver takes no call-site marker, as a method's does (CV4), so `Type.name(mut recv)` is E0115.
        if first.mut_marker {
            let error = self.diags.error(
                Code::UNNEEDED_MUT_MARKER,
                first.expr.span,
                format!("this parameter of `{}` is not `mut`; the argument takes no `mut` marker", name.name),
            );
            if let Some(mut_span) = first.mut_span {
                error.suggest_safe(
                    Span::new(mut_span.file, mut_span.lo, first.expr.span.lo),
                    "remove the `mut` marker",
                    "",
                );
            }
        }
        if recv.ty != Ty::Error && !crate::extensions::extension_applies(self.globals, &ext, &recv.ty) {
            let found = recv.ty.clone();
            self.diags.error(Code::TYPE_MISMATCH, recv.span, format!("expected `{ty}`, found `{found}`"));
            return Some(self.args_then_error(&args[1..], span));
        }
        Some(self.extension_call_inner(&ext, recv, name, &args[1..], span))
    }

    /// The error of `Type.name(...)` when `Type` has no function or variant `name`. A visible extension of that name
    /// that applies to `Type` is called instead (`spec/extensions.md`, EX9); otherwise `message` is reported. The
    /// type's own functions and the compiler's come first, so an extension never shadows them.
    fn no_assoc_function(
        &mut self,
        ty: &Ty,
        name: &ast::Ident,
        message: String,
        args: &[ast::Arg],
        span: Span,
    ) -> tir::Expr {
        if let Some(call) = self.extension_assoc_call(ty, name, args, span) {
            return call;
        }
        self.diags.error(Code::UNKNOWN_NAME, name.span, message);
        self.args_then_error(args, span)
    }

    fn extension_call_inner(
        &mut self,
        ext: &crate::globals::ExtensionInfo,
        recv: tir::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> tir::Expr {
        // A generic extension is instantiated from the receiver and the arguments, exactly like a generic
        // function whose first parameter is the receiver.
        if let Some(g) = ext.generic {
            return self.generic_call_with_receiver(g, recv, &name.name, args, span);
        }
        let Some(f) = ext.func else { return error_expr(span) };
        self.check_unsafe_call(f, &name.name, name.span);
        let sig = self.globals.sigs[f.0].clone();
        let Some(receiver) = sig.params.first().cloned() else {
            return self.args_then_error(args, span);
        };
        // `mut self` borrows the receiver exclusively, like a method's (`spec/extensions.md`, EX6).
        if receiver.by_ref && receiver.exclusive && is_place_expr(&recv) && !self.place_mutable(&recv) {
            let local = place_root(&recv).map(|root| self.locals[root.0].name.clone());
            let whose = local.unwrap_or_else(|| "this receiver".to_string());
            self.diags.error(
                Code::MUT_OF_IMMUTABLE,
                recv.span,
                format!("`{}` changes its receiver, but `{whose}` is not mutable", name.name),
            );
        }
        if receiver.by_ref
            && receiver.exclusive
            && let Some(whose) = self.slice_receiver_problem(&recv)
        {
            self.diags.error(
                Code::MUT_OF_IMMUTABLE,
                recv.span,
                format!("`{}` changes its receiver, but `{whose}` is not mutable", name.name),
            );
        }
        let rest = sig.params[1..].to_vec();
        if !receiver.own {
            self.static_mut_borrowed(&recv, recv.span);
        }
        let receiver_place = Some((&recv, receiver.exclusive));
        let Some(checked) = self.args_ordered(&name.name, &rest, args, receiver_place, span) else {
            return error_expr(span);
        };
        let checked = checked.with_receiver(recv);
        mk(tir::ExprKind::Call(f, checked.args, checked.order), sig.ret, span)
    }

    /// A struct field of closure type named `name`, for calling `holder.f(x)`. A field whose type is a type
    /// parameter with an `Fn`-family bound is callable the same way (the adapters in `std` store one).
    fn closure_field(&self, recv: &tir::Expr, name: &str) -> Option<(usize, Ty)> {
        let Ty::Struct(a) = &recv.ty else { return None };
        let (index, field) = self.globals.structs[a.id].fields.iter().enumerate().find(|(_, f)| f.name == name)?;
        self.globals.fn_call_sig(&field.ty, &self.bounds).map(|_| (index, field.ty.clone()))
    }

    /// A function or constant found through a module path: `math.add(...)`, `sh.area(...)`.
    fn module_call(
        &mut self,
        target: usize,
        item: &ast::Ident,
        explicit: &[ast::Type],
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        self.check_value_visibility(target, &item.name, item.span);
        let value = self.globals.exported_value(target, &item.name);
        if !explicit.is_empty()
            && matches!(value, Some(Global::Func(_) | Global::Extern(_) | Global::Const(_) | Global::Static(_)))
        {
            return self.reject_type_args(&item.name, explicit, args, span);
        }
        match value {
            Some(Global::Func(f)) => {
                self.check_unsafe_call(f, &item.name, item.span);
                let sig = &self.globals.sigs[f.0];
                let (params, ret) = (sig.params.clone(), sig.ret.clone());
                let Some(checked) = self.args_ordered(&item.name, &params, args, None, span) else {
                    return error_expr(span);
                };
                mk(tir::ExprKind::Call(f, checked.args, checked.order), ret, span)
            }
            Some(Global::Generic(g)) => self.generic_call(g, item, args, explicit, span, expect),
            Some(Global::Extern(e)) => self.extern_call(item, e, args, span),
            Some(Global::Const(_) | Global::Static(_)) => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    item.span,
                    format!("`{}` is a constant, not a function", item.name),
                );
                self.args_then_error(args, span)
            }
            None => {
                if self.globals.type_name(target, &item.name).is_some() {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        item.span,
                        format!("`{}` is a type, not a function", item.name),
                    );
                } else {
                    let d =
                        self.diags.error(Code::UNKNOWN_NAME, item.span, format!("unknown function `{}`", item.name));
                    d.label(item.span, "not found in this module");
                }
                self.args_then_error(args, span)
            }
        }
    }

    /// `CPtr.null()`.
    fn cptr_null(&mut self, args: &[ast::Arg], expect: Option<&Ty>, span: Span) -> tir::Expr {
        self.reject_named_args("CPtr.null", args);
        if !args.is_empty() {
            self.diags.error(Code::ARG_COUNT, span, "`CPtr.null` takes no arguments");
            return self.args_then_error(args, span);
        }
        match expect {
            Some(Ty::CPtr(inner)) => {
                mk(tir::ExprKind::Builtin(Builtin::CPtrNull, Vec::new()), Ty::CPtr(inner.clone()), span)
            }
            _ => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    "cannot tell the type of this `CPtr.null()`; annotate the binding with a `CPtr` type",
                );
                error_expr(span)
            }
        }
    }

    /// `Heap.new(value)`.
    pub(super) fn heap_new(&mut self, args: &[ast::Arg], expect: Option<&Ty>, span: Span) -> tir::Expr {
        self.reject_named_args("Heap.new", args);
        if let Some(Ty::Heap(_, alloc)) = expect
            && !alloc.is_global()
        {
            for a in args {
                self.expr(&a.expr, None);
            }
            let heap_ty = expect.map(|t| t.to_string()).unwrap_or_default();
            self.diags.error(
                Code::MISSING_ALLOCATOR,
                span,
                format!("`Heap.new` stores no allocator; a `{heap_ty}` is made with `Heap.new_in(alloc, value)`"),
            );
            return error_expr(span);
        }
        let inner = match expect {
            Some(Ty::Heap(t, _)) => Some((**t).clone()),
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
        let ty = Ty::Heap(Box::new(value.ty.clone()), burdin_tir::Alloc::Global);
        mk(tir::ExprKind::Builtin(Builtin::HeapNew, vec![value]), ty, span)
    }

    /// Whether an expression may be stored as a container's allocator: it is not a view and its type implements
    /// `Allocator` (`spec/allocators.md`, AL3).
    fn allocator_value_ok(&mut self, ty: &Ty, span: Span) -> bool {
        if ty.is_second_class() {
            self.diags.error(
                Code::SECOND_CLASS_POSITION,
                span,
                format!("`{ty}` is a view of other storage, so it cannot be stored as an allocator"),
            );
            return false;
        }
        if !self.globals.is_allocator(ty, &self.bounds) {
            self.diags.error(Code::NOT_AN_ALLOCATOR, span, format!("`{ty}` does not implement `Allocator`"));
            return false;
        }
        true
    }

    /// The constructors of a `List` that depend on the allocator (`spec/allocators.md`, AL5): `new_in` and its
    /// siblings take the allocator type from the value even when `List<T>` was written with the default one, and
    /// `new`, `with_capacity`, and `try_with_capacity` exist for the default allocator only (E0272). `None` leaves
    /// the call to the ordinary associated-function lookup.
    fn list_constructor(
        &mut self,
        ty: &Ty,
        elem: &Ty,
        written: &burdin_tir::Alloc,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<tir::Expr> {
        match name.name.as_str() {
            "new" | "with_capacity" | "try_with_capacity" if !written.is_global() => {
                let then = match name.name.as_str() {
                    "new" => "",
                    "with_capacity" => " and `reserve`",
                    _ => " and `try_reserve`",
                };
                self.diags.error(
                    Code::MISSING_ALLOCATOR,
                    span,
                    format!("a `{ty}` stores an allocator, so it is made with `List.new_in(alloc)`{then}"),
                );
                Some(self.args_then_error(args, span))
            }
            "new_in" | "with_capacity_in" | "try_with_capacity_in" if written.is_global() => {
                let at = usize::from(name.name != "new_in");
                if args.len() <= at {
                    return None;
                }
                let value = self.expr(&args[at].expr, None);
                if value.ty == Ty::Error || !self.allocator_value_ok(&value.ty, value.span) {
                    return Some(error_expr(span));
                }
                let alloc = self.globals.alloc_of(value.ty.clone());
                if alloc.is_global() {
                    return None;
                }
                let target = self.globals.list_ty(elem.clone(), alloc);
                if target == Ty::Error {
                    return Some(error_expr(span));
                }
                Some(self.assoc_call(&target, name, &[], args, span, None))
            }
            _ => None,
        }
    }

    /// The constructors of a `Map` that depend on the allocator (`spec/allocators.md`, AL5): `new_in` takes the
    /// allocator type from the value even when `Map<K, V>` was written with the default one, and `new`,
    /// `with_capacity`, and `try_with_capacity` exist for the default allocator only (E0272). `None` leaves the call
    /// to the ordinary associated-function lookup.
    #[allow(clippy::too_many_arguments)]
    fn map_constructor(
        &mut self,
        ty: &Ty,
        key: &Ty,
        value: &Ty,
        written: &burdin_tir::Alloc,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<tir::Expr> {
        match name.name.as_str() {
            "new" | "with_capacity" | "try_with_capacity" if !written.is_global() => {
                let then = match name.name.as_str() {
                    "new" => "",
                    "with_capacity" => " and `reserve`",
                    _ => " and `try_reserve`",
                };
                self.diags.error(
                    Code::MISSING_ALLOCATOR,
                    span,
                    format!("a `{ty}` stores an allocator, so it is made with `Map.new_in(alloc)`{then}"),
                );
                Some(self.args_then_error(args, span))
            }
            "new_in" if written.is_global() => {
                if args.is_empty() {
                    return None;
                }
                let value_arg = self.expr(&args[0].expr, None);
                if value_arg.ty == Ty::Error || !self.allocator_value_ok(&value_arg.ty, value_arg.span) {
                    return Some(error_expr(span));
                }
                let alloc = self.globals.alloc_of(value_arg.ty.clone());
                if alloc.is_global() {
                    return None;
                }
                let target = self.globals.map_ty(key.clone(), value.clone(), alloc);
                if target == Ty::Error {
                    return Some(error_expr(span));
                }
                Some(self.assoc_call(&target, name, &[], args, span, None))
            }
            _ => None,
        }
    }

    /// `Heap.new_in(alloc, value)`: one heap allocation through the allocator value.
    pub(super) fn heap_new_in(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        self.reject_named_args("Heap.new_in", args);
        if args.len() != 2 {
            for a in args {
                self.expr(&a.expr, None);
            }
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`Heap.new_in` takes 2 arguments, but {} were given", args.len()),
            );
            return error_expr(span);
        }
        let alloc_value = self.expr(&args[0].expr, None);
        if !self.allocator_value_ok(&alloc_value.ty, alloc_value.span) {
            return error_expr(span);
        }
        let alloc = self.globals.alloc_of(alloc_value.ty.clone());
        let value = self.expr(&args[1].expr, None);
        check_storable(&value.ty, value.span, "a `Heap` element", self.diags);
        let ty = Ty::Heap(Box::new(value.ty.clone()), alloc);
        mk(tir::ExprKind::Builtin(Builtin::HeapNewIn, vec![alloc_value, value]), ty, span)
    }

    /// `T.try_from(value)` for an integer type `T` and an integer `value` of any type (`spec/stdlib.md`, "Conversions"):
    /// `Ok` of the value converted to `T` when it fits, and `Err(TryFromIntError.OutOfRange)` when it does not. Only
    /// the bounds the source type can actually leave are checked, so a conversion that always fits has no check.
    fn int_try_from(&mut self, to: IntTy, args: &[ast::Arg], span: Span) -> tir::Expr {
        if args.len() != 1 {
            let message = format!("`{}.try_from` takes 1 argument, but {} were given", to.name(), args.len());
            self.diags.error(Code::ARG_COUNT, span, message);
            return self.args_then_error(args, span);
        }
        let value = self.expr(&args[0].expr, None);
        let from = match &value.ty {
            Ty::Int(from) => *from,
            Ty::Error => return error_expr(span),
            other => {
                let message = format!("`{}.try_from` converts an integer, not `{other}`", to.name());
                self.diags.error(Code::TYPE_MISMATCH, value.span, message);
                return error_expr(span);
            }
        };
        let Some(err_ty) = self.globals.try_from_int_error_ty.clone() else { return error_expr(span) };
        let Ty::Enum(err) = &err_ty else { return error_expr(span) };
        let Some(out_of_range) = self.globals.enums[err.id].variants.iter().position(|v| v.name == "OutOfRange") else {
            return error_expr(span);
        };
        let result = self.globals.result_ty(Ty::Int(to), err_ty.clone());
        // The value is evaluated once, into a local the checks and the conversion read.
        let (id, mode) = self.payload_local("value", Ty::Int(from), true);
        let read = || mk(tir::ExprKind::Local(id), Ty::Int(from), span);
        let bound = |n: i128| mk(tir::ExprKind::Int(n), Ty::Int(from), span);
        let mut checks = Vec::new();
        if to.min() > from.min() {
            let check = tir::ExprKind::Binary(tir::BinOp::Ge, Box::new(read()), Box::new(bound(to.min())));
            checks.push(mk(check, Ty::Bool, span));
        }
        if to.max() < from.max() {
            let check = tir::ExprKind::Binary(tir::BinOp::Le, Box::new(read()), Box::new(bound(to.max())));
            checks.push(mk(check, Ty::Bool, span));
        }
        let cond = checks
            .into_iter()
            .reduce(|a, b| mk(tir::ExprKind::Binary(tir::BinOp::And, Box::new(a), Box::new(b)), Ty::Bool, span))
            .unwrap_or_else(|| mk(tir::ExprKind::Bool(true), Ty::Bool, span));
        let converted = mk(tir::ExprKind::Cast(Box::new(read())), Ty::Int(to), span);
        let ok = Globals::mk_result(true, converted, result.clone(), span);
        let variant = mk(tir::ExprKind::Variant(out_of_range, Vec::new()), err_ty, span);
        let err = Globals::mk_result(false, variant, result.clone(), span);
        let fits = tir::Block { stmts: Vec::new(), tail: Some(Box::new(ok)), close: span };
        let branch = mk(tir::ExprKind::If(Box::new(cond), fits, Some(Box::new(err))), result.clone(), span);
        let bind = tir::Stmt::Let(Pat::Binding(id, mode), value, None);
        let block = tir::Block { stmts: vec![bind], tail: Some(Box::new(branch)), close: span };
        mk(tir::ExprKind::Block(block), result, span)
    }

    /// `Type.from(value)` and `Type.try_from(value)` for a type with conversion blocks of that method (`spec/stdlib.md`,
    /// "Conversions"). The block is chosen by the type of `value`. `None` when the type has no such block, so the
    /// caller reports the name as unknown.
    fn conversion_call(&mut self, ty: &Ty, method: &str, args: &[ast::Arg], span: Span) -> Option<tir::Expr> {
        let candidates: Vec<(Ty, FuncId)> =
            self.globals.conversions_into(ty, method).iter().map(|c| (c.source.clone(), c.func)).collect();
        if candidates.is_empty() {
            return None;
        }
        if args.len() != 1 {
            let message = format!("`{ty}.{method}` takes 1 argument, but {} were given", args.len());
            self.diags.error(Code::ARG_COUNT, span, message);
            return Some(self.args_then_error(args, span));
        }
        // With one source type the argument is checked against it, so a literal takes that type.
        let expect = match candidates.as_slice() {
            [(source, _)] => Some(source.clone()),
            _ => None,
        };
        let value = self.expr(&args[0].expr, expect.as_ref());
        if value.ty == Ty::Error {
            return Some(error_expr(span));
        }
        // A `String` is borrowed as a `str` where a `str` is expected (IC1), as for any argument.
        let exact = candidates.iter().find(|(source, _)| *source == value.ty);
        let borrowed = candidates.iter().find(|(source, _)| *source == Ty::Str);
        let found = match (exact, borrowed) {
            (Some(hit), _) => Some(hit.clone()),
            (None, Some(hit)) if value.ty == Ty::String => Some(hit.clone()),
            _ => None,
        };
        let Some((source, func)) = found else {
            let sources: Vec<String> = candidates.iter().map(|(source, _)| format!("`{source}`")).collect();
            let message = format!("`{ty}` converts from {}, not from `{}`", sources.join(" or "), value.ty);
            self.diags.error(Code::TYPE_MISMATCH, value.span, message);
            return Some(error_expr(span));
        };
        let value = self.coerce_arg(value, &source);
        let sig = self.globals.sigs[func.0].clone();
        let Some(checked) = self.args_pre(method, &sig.params, args, None, span, Some(vec![value])) else {
            return Some(error_expr(span));
        };
        Some(mk(tir::ExprKind::Call(func, checked, Vec::new()), sig.ret, span))
    }

    /// `Type.function(...)`, `Enum.Variant(...)`, and the built-in types' associated functions.
    pub(super) fn assoc_call(
        &mut self,
        ty: &Ty,
        name: &ast::Ident,
        explicit: &[ast::Type],
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        // The type already failed to resolve; do not report a second error for its associated items.
        if *ty == Ty::Error {
            for a in args {
                self.expr(&a.expr, None);
            }
            return error_expr(span);
        }
        // `Option<T>.Some(x)`, `Result<T, E>.Ok(v)`, and `Result<T, E>.Err(e)`: the type is written, so it is the expected type.
        if self.globals.option_payload(ty).is_some() && name.name == "Some" {
            return self.some(args, Some(ty), span);
        }
        if self.globals.result_parts(ty).is_some() && (name.name == "Ok" || name.name == "Err") {
            return self.result_ctor(name.name == "Ok", args, Some(ty), span);
        }
        // Only an associated function of the program with type parameters of its own takes type arguments (TA5).
        if !explicit.is_empty() && !self.user_generic_method(ty, &name.name) {
            return self.reject_type_args(&name.name, explicit, args, span);
        }
        // `T.size()` and `T.align()` of a type parameter are the layout of whatever it is instantiated with, unless
        // one of its bounds provides a function of that name.
        if let Ty::Param(_) = ty {
            if matches!(name.name.as_str(), "size" | "align") && args.is_empty() && !self.param_provides(ty, &name.name)
            {
                return mk(
                    tir::ExprKind::LayoutOf { ty: ty.clone(), alignment: name.name == "align" },
                    Ty::Int(IntTy::Usize),
                    span,
                );
            }
            return self.param_assoc_call(ty, name, args, span, expect);
        }
        // `String.deserialize(mut d)`, `List<T>.deserialize(mut d)`, and the other built-in types (SER14).
        if let Some(call) = self.builtin_deserialize_call(ty, name, args, span) {
            return call;
        }
        // `from_iter` of the built-in collections and `from` of a `List` or `Map` (`spec/iterators.md`, IT18).
        if let Some(call) = self.builtin_collection_call(ty, name, args, span) {
            return call;
        }
        // `T.from(v)` for a scalar `T`: the built-in pairs of `From` (spec/stdlib.md, FR2).
        if name.name == "from" && matches!(ty, Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Char) {
            return self.primitive_from_call(ty, args, span);
        }
        // `Type.size()` and `Type.align()`: the C layout of a type, for `repr(C)` size assertions (issue #490).
        if matches!(name.name.as_str(), "size" | "align") && args.is_empty() {
            let user_method = crate::globals::adt_key(ty)
                .is_some_and(|key| self.globals.methods.contains_key(&(key, name.name.clone())));
            if !user_method {
                let alignment = name.name == "align";
                return mk(tir::ExprKind::LayoutOf { ty: ty.clone(), alignment }, Ty::Int(IntTy::Usize), span);
            }
        }
        // A raw pointer has `null`; a pointer to `CChar` is how `CStr` is written, so it also reads the
        // NUL-terminated bytes into a `String`.
        if let Ty::CPtr(inner) = ty {
            if name.name == "null" && args.is_empty() {
                return mk(tir::ExprKind::Builtin(Builtin::CPtrNull, Vec::new()), ty.clone(), span);
            }
            if name.name == "to_string" && **inner == Ty::Int(IntTy::I8) {
                let sig = builtin_sig(Builtin::CStrToString, self.globals);
                let Some(checked) = self.args(&name.name, &sig.params, args, None, span) else {
                    return error_expr(span);
                };
                return mk(tir::ExprKind::Builtin(Builtin::CStrToString, checked), sig.ret, span);
            }
            return self.no_assoc_function(ty, name, format!("`CPtr` has no function `{}`", name.name), args, span);
        }
        // `i32.parse(s)`: the number in the text, or an error.
        if let Ty::Int(i) = ty {
            if name.name == "try_from" {
                return self.int_try_from(*i, args, span);
            }
            // The other associated functions of an integer type (`u32.from_le_bytes(b)`) are `std` functions named
            // `<type>_<name>` (spec/stdlib.md, "Integer functions").
            if name.name != "parse"
                && let Some(Global::Func(f)) = self.globals.std_value(&format!("{}_{}", i.name(), name.name))
            {
                let sig = self.globals.sigs[f.0].clone();
                let Some(checked) = self.args(&name.name, &sig.params, args, None, span) else {
                    return error_expr(span);
                };
                return mk(tir::ExprKind::Call(f, checked, Vec::new()), sig.ret, span);
            }
            if name.name != "parse" && name.name != "default" {
                let message = format!("`{}` has no function `{}`", i.name(), name.name);
                return self.no_assoc_function(ty, name, message, args, span);
            }
            // `default` falls through to the `Type.default()` lookup below, like the other built-in types.
            if name.name == "parse" {
                let Some(checked) = self.args("parse", &[ParamSig::plain(Ty::Str)], args, None, span) else {
                    return error_expr(span);
                };
                // The 128-bit types parse in `std`, in base 10 (`spec/int128.md`, I128-6): the runtime's parser
                // reads into 64 bits.
                if matches!(i, IntTy::I128 | IntTy::U128) {
                    let Some(Global::Func(f)) = self.globals.std_value(&format!("{}_parse_radix", i.name())) else {
                        unreachable!("std defines the 128-bit parsers")
                    };
                    let sig = self.globals.sigs[f.0].clone();
                    let mut call_args = checked;
                    call_args.push(mk(tir::ExprKind::Int(10), Ty::Int(IntTy::U32), span));
                    return mk(tir::ExprKind::Call(f, call_args, Vec::new()), sig.ret, span);
                }
                let error = self.globals.parse_int_error_ty.clone().unwrap_or(Ty::Unit);
                let ret = self.globals.result_ty(ty.clone(), error);
                return mk(tir::ExprKind::Builtin(Builtin::IntParse(*i), checked), ret, span);
            }
        }
        // `f64.parse(s)` and the other associated functions of a float type are `std` functions named
        // `<type>_<name>` (spec/stdlib.md, "Floats"), except `from_bits`, which is the compiler's bit cast.
        if let Ty::Float(ft) = ty {
            if name.name == "from_bits" {
                let bits = match ft {
                    FloatTy::F32 => Ty::Int(IntTy::U32),
                    FloatTy::F64 => Ty::Int(IntTy::U64),
                };
                let Some(checked) = self.args(&name.name, &[ParamSig::plain(bits)], args, None, span) else {
                    return error_expr(span);
                };
                return mk(tir::ExprKind::Builtin(Builtin::FloatFromBits(*ft), checked), ty.clone(), span);
            }
            if let Some(Global::Func(f)) = self.globals.std_value(&format!("{}_{}", ty, name.name)) {
                let sig = self.globals.sigs[f.0].clone();
                let Some(checked) = self.args(&name.name, &sig.params, args, None, span) else {
                    return error_expr(span);
                };
                return mk(tir::ExprKind::Call(f, checked, Vec::new()), sig.ret, span);
            }
        }
        // The associated functions of `String` (the only built-in owning type with any so far).
        if *ty == Ty::String && name.name != "default" {
            // `String.from_utf8` and `String.from_utf8_lossy` are `std` functions; the compiler only maps the
            // names (`spec/stdlib.md`, "String bytes and UTF-8 validation").
            let utf8 = match name.name.as_str() {
                "from_utf8" => Some("string_from_utf8"),
                "from_utf8_lossy" => Some("string_from_utf8_lossy"),
                _ => None,
            };
            if let Some(hidden) = utf8
                && let Some(Global::Func(f)) = self.globals.std_value(hidden)
            {
                let sig = self.globals.sigs[f.0].clone();
                let Some(checked) = self.args(&name.name, &sig.params, args, None, span) else {
                    return error_expr(span);
                };
                return mk(tir::ExprKind::Call(f, checked, Vec::new()), sig.ret, span);
            }
            if name.name == "with_capacity" {
                let sig = builtin_sig(Builtin::StringWithCapacity, self.globals);
                let Some(checked) = self.args(&name.name, &sig.params, args, None, span) else {
                    return error_expr(span);
                };
                return mk(tir::ExprKind::Builtin(Builtin::StringWithCapacity, checked), sig.ret, span);
            }
            if name.name == "try_with_capacity" {
                let sig = builtin_sig(Builtin::StringTryWithCapacity, self.globals);
                let Some(checked) = self.args(&name.name, &sig.params, args, None, span) else {
                    return error_expr(span);
                };
                return mk(tir::ExprKind::Builtin(Builtin::StringTryWithCapacity, checked), sig.ret, span);
            }
            let b = match name.name.as_str() {
                "new" => Builtin::StringNew,
                "from" => Builtin::StringFrom,
                _ => {
                    let message = format!("`String` has no function `{}`", name.name);
                    return self.no_assoc_function(ty, name, message, args, span);
                }
            };
            let sig = builtin_sig(b, self.globals);
            let Some(checked) = self.args(&name.name, &sig.params, args, None, span) else {
                return error_expr(span);
            };
            return mk(tir::ExprKind::Builtin(b, checked), sig.ret, span);
        }
        // `str.from_utf8`: the `std` function that validates the bytes and returns a `str` view of them.
        if *ty == Ty::Str && name.name != "default" {
            if name.name == "from_utf8"
                && let Some(Global::Func(f)) = self.globals.std_value("str_from_utf8")
            {
                let sig = self.globals.sigs[f.0].clone();
                let Some(checked) = self.args(&name.name, &sig.params, args, None, span) else {
                    return error_expr(span);
                };
                return mk(tir::ExprKind::Call(f, checked, Vec::new()), sig.ret, span);
            }
            return self.no_assoc_function(ty, name, format!("`str` has no function `{}`", name.name), args, span);
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
        if (name.name == "from" || name.name == "try_from")
            && let Some(call) = self.conversion_call(ty, &name.name, args, span)
        {
            return call;
        }
        if let Some((elem, written)) = self.globals.list_parts(ty)
            && let Some(call) = self.list_constructor(ty, &elem, &written, name, args, span)
        {
            return call;
        }
        if let Some((key, value, written)) = self.globals.map_parts(ty)
            && let Some(call) = self.map_constructor(ty, &key, &value, &written, name, args, span)
        {
            return call;
        }
        let method = adt_key(ty).and_then(|k| self.globals.methods.get(&(k, name.name.clone())).copied());
        let Some(f) = method else {
            if name.name == "default" && args.is_empty() && self.globals.has_default(ty) {
                return self.default_expr(ty, span);
            }
            let message = format!("`{ty}` has no function or variant `{}`", name.name);
            return self.no_assoc_function(ty, name, message, args, span);
        };
        if let Some(key) = adt_key(ty) {
            self.check_member_access(ty, key, name);
        }
        self.check_unsafe_call(f, &name.name, name.span);
        // An associated function with type parameters of its own is instantiated for the arguments of this call.
        if let Some(info) = self.globals.method_templates.get(&f).cloned()
            && !self.globals.sigs[f.0].has_self
        {
            return self.generic_assoc_method_call(info, &name.name, explicit, args, None, span, expect);
        }
        let sig = &self.globals.sigs[f.0];
        let (params, ret) = (sig.params.clone(), sig.ret.clone());
        let Some(checked) = self.args_ordered(&name.name, &params, args, None, span) else { return error_expr(span) };
        mk(tir::ExprKind::Call(f, checked.args, checked.order), ret, span)
    }
}

/// Checked call arguments in parameter order, plus the source evaluation order when named arguments were written
/// out of parameter order (`spec/arguments.md`, AG6).
pub(super) struct CheckedArgs {
    pub(super) args: Vec<tir::Expr>,
    pub(super) order: Vec<usize>,
}

impl CheckedArgs {
    /// Prepends a method receiver, shifting any non-empty evaluation order so the receiver runs first.
    pub(super) fn with_receiver(mut self, recv: tir::Expr) -> Self {
        self.args.insert(0, recv);
        if !self.order.is_empty() {
            for idx in &mut self.order {
                *idx += 1;
            }
            self.order.insert(0, 0);
        }
        self
    }
}

/// `` `a` ``, `` `a` and `b` ``, or `` `a`, `b`, and `c` ``.
fn join_names(names: &[String]) -> String {
    match names {
        [] => "no parameter".to_string(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        _ => {
            let (last, rest) = names.split_last().expect("the slice is not empty");
            format!("{}, and {last}", rest.join(", "))
        }
    }
}

/// A synthetic argument of a lowered call: the string literal `text`.
fn string_arg(text: String, span: Span) -> ast::Arg {
    ast::Arg { name: None, mut_marker: false, mut_span: None, expr: ast::Expr { kind: ast::ExprKind::Str(text), span } }
}

/// The placeholder for an argument whose checked expression the caller supplies separately. It keeps the `mut`
/// marker so the convention diagnostics are the same as for the written argument.
fn checked_arg(original: &ast::Arg, span: Span) -> ast::Arg {
    ast::Arg {
        name: None,
        mut_marker: original.mut_marker,
        mut_span: original.mut_span,
        expr: ast::Expr { kind: ast::ExprKind::Unit, span },
    }
}
