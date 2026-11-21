//! Reflection (`spec/reflection.md`): the members `Type.name`, `.size`, `.align`, `.fields`, and `.variants`,
//! `comptime for` over the last two, and the computed field access `x.{name}`.
//!
//! The members are ordinary compile-time values: `Type.fields` is an array of the standard library's `FieldInfo`
//! structs, built from the declaration. A `comptime for` over `Type.fields` or `Type.variants` is unrolled here,
//! while the body is checked, because `x.{field.name}` has a type only once the name is known: each copy of the
//! body is checked with the loop variable standing for that item's literal value (RFL4, RFL5).

use super::{FnCx, LoopScope, PathTy, error_expr, mk};
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, IntTy, LocalKind, Ty};
use std::collections::HashMap;

/// The names every type reserves for its reflection members (RFL3).
pub(crate) const RESERVED_MEMBERS: [&str; 5] = ["name", "size", "align", "fields", "variants"];

/// One item of `Type.fields` or `Type.variants`, as the checker holds it while a `comptime for` is unrolled.
#[derive(Clone)]
pub(super) struct ReflectItem {
    /// `FieldInfo` or `VariantInfo`.
    ty: Ty,
    name: String,
    /// A field's type, as the checker prints it. `None` for a variant.
    type_name: Option<String>,
    index: usize,
    /// The number of fields a variant carries. `None` for a field.
    arity: Option<usize>,
}

impl ReflectItem {
    /// The struct literal of this item's `FieldInfo` or `VariantInfo`.
    fn literal(&self, span: Span) -> tir::Expr {
        let usize_ty = Ty::Int(IntTy::Usize);
        let text = |s: &str| {
            let literal = mk(tir::ExprKind::Str(s.to_string()), Ty::Str, span);
            mk(tir::ExprKind::Builtin(tir::Builtin::StringFrom, vec![literal]), Ty::String, span)
        };
        let number = |n: usize| mk(tir::ExprKind::Int(n as i128), usize_ty.clone(), span);
        let fields = match (&self.type_name, self.arity) {
            (Some(type_name), _) => vec![(0, text(&self.name)), (1, text(type_name)), (2, number(self.index))],
            (None, arity) => vec![(0, text(&self.name)), (1, number(self.index)), (2, number(arity.unwrap_or(0)))],
        };
        mk(tir::ExprKind::Struct(fields), self.ty.clone(), span)
    }

    /// The value of `item.member` as a literal, for the members `FieldInfo` and `VariantInfo` have. Reading a
    /// member of the loop variable gives the member itself, not a field of a temporary struct.
    fn member_literal(&self, member: &str, span: Span) -> Option<tir::Expr> {
        let text = |s: &str| {
            let literal = mk(tir::ExprKind::Str(s.to_string()), Ty::Str, span);
            mk(tir::ExprKind::Builtin(tir::Builtin::StringFrom, vec![literal]), Ty::String, span)
        };
        let number = |n: usize| mk(tir::ExprKind::Int(n as i128), Ty::Int(IntTy::Usize), span);
        match (member, &self.type_name, self.arity) {
            ("name", _, _) => Some(text(&self.name)),
            ("type_name", Some(type_name), _) => Some(text(type_name)),
            ("index", _, _) => Some(number(self.index)),
            ("arity", None, Some(arity)) => Some(number(arity)),
            _ => None,
        }
    }

    /// The compile-time text of `item.name` or `item.type_name`, for a computed field access.
    fn text(&self, member: &str) -> Option<String> {
        match member {
            "name" => Some(self.name.clone()),
            "type_name" => self.type_name.clone(),
            _ => None,
        }
    }
}

/// The loop variables of the `comptime for` bodies being checked: each stands for one item's literal.
pub(super) type ReflectBindings = HashMap<tir::LocalId, ReflectItem>;

impl FnCx<'_> {
    /// `Type.name`, `.size`, `.align`, `.fields`, or `.variants` (RFL2), or `None` when `name` is no reflection
    /// member.
    pub(super) fn reflect_member(&mut self, ty: &Ty, name: &ast::Ident, span: Span) -> Option<tir::Expr> {
        let member = name.name.as_str();
        if !RESERVED_MEMBERS.contains(&member) {
            return None;
        }
        if *ty == Ty::Error {
            return Some(error_expr(span));
        }
        Some(match member {
            "name" => mk(tir::ExprKind::Str(short_type_name(&ty.to_string())), Ty::Str, span),
            "size" | "align" => mk(
                tir::ExprKind::LayoutOf { ty: ty.clone(), alignment: member == "align" },
                Ty::Int(IntTy::Usize),
                span,
            ),
            _ => {
                let Some(items) = self.reflect_items(ty, member, name.span) else { return Some(error_expr(span)) };
                let Some(info) = self.reflect_info_ty(member) else { return Some(error_expr(span)) };
                let len = items.len() as u64;
                let elems = items.iter().map(|item| item.literal(span)).collect();
                mk(tir::ExprKind::Array(elems), Ty::Array(Box::new(info), len), span)
            }
        })
    }

    /// `FieldInfo` for `fields`, `VariantInfo` for `variants`, from the standard library.
    fn reflect_info_ty(&self, member: &str) -> Option<Ty> {
        self.globals.std_adt_ty(if member == "fields" { "FieldInfo" } else { "VariantInfo" })
    }

    /// The items `ty.fields` or `ty.variants` lists, in declaration order. Reports E0230 and returns `None` when
    /// the member does not apply to `ty`.
    fn reflect_items(&mut self, ty: &Ty, member: &str, at: Span) -> Option<Vec<ReflectItem>> {
        let Some(info) = self.reflect_info_ty(member) else {
            self.diags.error(
                Code::INVALID_REFLECTION,
                at,
                format!("`{member}` needs the standard library's reflection types, which are not available"),
            );
            return None;
        };
        match (member, ty) {
            ("fields", Ty::Struct(a)) => Some(
                self.globals.structs[a.id]
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(index, f)| ReflectItem {
                        ty: info.clone(),
                        name: f.name.clone(),
                        type_name: Some(f.ty.to_string()),
                        index,
                        arity: None,
                    })
                    .collect(),
            ),
            ("variants", Ty::Enum(a)) => Some(
                self.globals.enums[a.id]
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(index, v)| ReflectItem {
                        ty: info.clone(),
                        name: v.name.clone(),
                        type_name: None,
                        index,
                        arity: Some(v.fields.len()),
                    })
                    .collect(),
            ),
            // A struct has no variants and an enum has no fields, so a derive written for both kinds can ask for
            // either one (RFL2).
            ("fields", Ty::Enum(_)) | ("variants", Ty::Struct(_)) => Some(Vec::new()),
            // A type parameter has no members until the body is checked for an instance; the symbolic check of a
            // generic body sees none (RFL2).
            (_, Ty::Param(_)) => Some(Vec::new()),
            (_, Ty::Error) => None,
            _ => {
                self.diags.error(
                    Code::INVALID_REFLECTION,
                    at,
                    format!("`{ty}` has no `{member}`: only a struct has fields and only an enum has variants (spec/reflection.md, RFL2)"),
                );
                None
            }
        }
    }

    /// `comptime for item in Type.fields { ... }` and the same over `Type.variants`: the body is checked once per
    /// item with the loop variable standing for the item's literal, and the checked copies are the result (RFL4).
    /// Returns `None` when `iter` is not a reflection member of a type or the pattern is not a plain name, and
    /// the ordinary `comptime for` applies.
    pub(super) fn comptime_for_reflect(
        &mut self,
        pattern: &ast::Pattern,
        iter: &ast::Expr,
        body: &ast::Block,
        span: Span,
    ) -> Option<tir::Expr> {
        let ast::ExprKind::Field(base, member) = &iter.kind else { return None };
        if member.name != "fields" && member.name != "variants" {
            return None;
        }
        let binding = match &pattern.kind {
            ast::PatternKind::Binding { name, mutable: false } => Some(name),
            ast::PatternKind::Wild => None,
            _ => return None,
        };
        let ty = match self.type_path(base) {
            Some(PathTy::Ty(ty)) => ty,
            Some(PathTy::Template(_)) => return None,
            None => self.qualified_type(base)?,
        };
        let items = self.reflect_items(&ty, &member.name, member.span);
        let Some(items) = items else { return Some(error_expr(span)) };
        let mut copies = Vec::with_capacity(items.len());
        for item in items {
            self.scopes.push(HashMap::new());
            if let Some(name) = binding {
                let local = self.declare(&name.name, item.ty.clone(), false, LocalKind::Owned, name.span);
                self.reflect.insert(local, item);
            }
            // `break` and `continue` cannot leave the unrolled body: it is not a loop in the generated code.
            let saved_barrier = self.comptime_loop_barrier.replace(self.loops.len());
            self.loops.push(LoopScope::loop_body());
            let (block, ty) = self.block(body, None);
            self.loops.pop();
            self.comptime_loop_barrier = saved_barrier;
            self.scopes.pop();
            copies.push(tir::Stmt::Expr(mk(tir::ExprKind::Block(block), ty, span)));
        }
        Some(mk(tir::ExprKind::Block(tir::Block { stmts: copies, tail: None, close: span }), Ty::Unit, span))
    }

    /// `variable.member` where `variable` is a reflection loop variable: the member's literal.
    pub(super) fn reflect_member_of_loop_variable(
        &self,
        base: &ast::Expr,
        member: &ast::Ident,
        span: Span,
    ) -> Option<tir::Expr> {
        let ast::ExprKind::Name { name, args } = &base.kind else { return None };
        if !args.is_empty() {
            return None;
        }
        let local = self.lookup(&name.name)?;
        self.reflect.get(&local)?.member_literal(&member.name, span)
    }

    /// The literal a reflection loop variable stands for, when `local` is one.
    pub(super) fn reflect_literal(&self, local: tir::LocalId, span: Span) -> Option<tir::Expr> {
        self.reflect.get(&local).map(|item| item.literal(span))
    }

    /// `base.{name}`: the field called by a compile-time string (RFL5). The name is a string literal or
    /// `item.name` of a reflection loop variable; the access then means `base.that_name`.
    pub(super) fn dyn_field(
        &mut self,
        base: &ast::Expr,
        name: &ast::Expr,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let Some(text) = self.computed_name(name) else {
            self.diags.error(
                Code::INVALID_COMPUTED_FIELD,
                name.span,
                "the name in `x.{...}` must be a string literal or the `name` of a `comptime for` variable over \
                 `Type.fields`, so that the field is known while compiling (spec/reflection.md, RFL5)",
            );
            self.expr(base, None);
            return error_expr(span);
        };
        let ident = ast::Ident { name: text, span: name.span };
        self.field(base, &ident, expect, span)
    }

    /// The compile-time string of a computed field name.
    fn computed_name(&self, e: &ast::Expr) -> Option<String> {
        match &e.kind {
            ast::ExprKind::Str(text) => Some(text.clone()),
            ast::ExprKind::Field(base, member) => {
                let ast::ExprKind::Name { name, args } = &base.kind else { return None };
                if !args.is_empty() {
                    return None;
                }
                let local = self.lookup(&name.name)?;
                self.reflect.get(&local)?.text(&member.name)
            }
            _ => None,
        }
    }
}

/// A type as `Type.name` shows it: the name as written, without the module a type lives in (`geometry.Square`
/// is `Square`), with the arguments of a generic instance (`Pair<Leaf>`).
fn short_type_name(full: &str) -> String {
    let mut out = String::new();
    let mut word = String::new();
    for c in full.chars() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else if c == '.' {
            // A module qualifier: what came before the dot is a module, not part of the name.
            word.clear();
        } else {
            out.push_str(&word);
            word.clear();
            out.push(c);
        }
    }
    out.push_str(&word);
    out
}

#[cfg(test)]
mod tests {
    use super::short_type_name;

    #[test]
    fn module_qualifiers_are_dropped() {
        assert_eq!(short_type_name("main.User"), "User");
        assert_eq!(short_type_name("geometry.shapes.Square"), "Square");
        assert_eq!(short_type_name("main.Pair<main.Leaf>"), "Pair<Leaf>");
        assert_eq!(short_type_name("List<i32>"), "List<i32>");
        assert_eq!(short_type_name("[i32; 4]"), "[i32; 4]");
    }
}
