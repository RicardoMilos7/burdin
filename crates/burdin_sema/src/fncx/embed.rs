//! `embed("path")` (`spec/const-eval.md`, CE12): the contents of a file, read while compiling.
//!
//! The result is `str` for a text file, `[u8; N]` when the expected type is an array of the file's length, or a
//! static `[u8]` view when the expected type is `[u8]` (no length is written, and the view is static data, RT4).
//! The path is relative to the directory of the source file that writes the call, and every file read is recorded
//! as an input of the build, so `--emit-deps` can list it.

use super::{FnCx, error_expr, mk};
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, IntTy, Ty};
use std::path::{Component, Path};

impl FnCx<'_> {
    pub(super) fn embed_call(&mut self, args: &[ast::Arg], span: Span, expect: Option<&Ty>) -> tir::Expr {
        let [arg] = args else {
            self.diags.error(Code::INVALID_EMBED, span, "`embed` takes one argument: the path of the file");
            return self.args_then_error(args, span);
        };
        let ast::ExprKind::Str(text) = &arg.expr.kind else {
            self.diags.error(
                Code::INVALID_EMBED,
                arg.expr.span,
                "the path of `embed` must be a string literal, so that the file is known while compiling",
            );
            self.expr(&arg.expr, None);
            return error_expr(span);
        };
        let relative = Path::new(text);
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
        {
            self.diags.error(
                Code::INVALID_EMBED,
                arg.expr.span,
                "the path of `embed` must be relative to the source file and stay inside its directory (no `..`)",
            );
            return error_expr(span);
        }
        self.globals.impure.set(true);
        let source = &self.sources.get(span.file).path;
        let full = Path::new(source).parent().unwrap_or(Path::new("")).join(relative);
        let bytes = match std::fs::read(&full) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.diags.error(
                    Code::EMBED_FAILED,
                    arg.expr.span,
                    format!("cannot read `{}`: {}", full.display(), describe_io_error(&e)),
                );
                return error_expr(span);
            }
        };
        {
            let mut embedded = self.globals.embedded.borrow_mut();
            let shown = full.display().to_string();
            if !embedded.contains(&shown) {
                embedded.push(shown);
            }
        }
        if let Some(Ty::Array(elem, len)) = expect
            && **elem == Ty::Int(IntTy::U8)
        {
            if *len != bytes.len() as u64 {
                self.diags.error(
                    Code::INVALID_EMBED,
                    span,
                    format!("`{text}` has {} bytes, but the expected type is `[u8; {len}]`", bytes.len()),
                );
                return error_expr(span);
            }
            let elems = bytes.iter().map(|b| mk(tir::ExprKind::Int(*b as i128), Ty::Int(IntTy::U8), span)).collect();
            return mk(tir::ExprKind::Array(elems), Ty::Array(Box::new(Ty::Int(IntTy::U8)), *len), span);
        }
        if let Some(Ty::Slice(elem)) = expect
            && **elem == Ty::Int(IntTy::U8)
        {
            return mk(tir::ExprKind::Bytes(bytes), Ty::Slice(Box::new(Ty::Int(IntTy::U8))), span);
        }
        match String::from_utf8(bytes) {
            Ok(content) => mk(tir::ExprKind::Str(content), Ty::Str, span),
            Err(_) => {
                self.diags.error(
                    Code::EMBED_FAILED,
                    arg.expr.span,
                    format!("`{text}` is not valid UTF-8 text; embed it as bytes with `[u8; N]`"),
                );
                error_expr(span)
            }
        }
    }
}

/// The reason an I/O error gives, without the operating system's wording of the path.
fn describe_io_error(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "no such file".to_string(),
        std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        _ => e.to_string(),
    }
}
