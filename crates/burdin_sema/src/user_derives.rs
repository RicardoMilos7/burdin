//! The names after `:` on a struct or enum declaration that are not built-in derives (`spec/derives.md`,
//! `spec/reflection.md` RFL8-RFL10). A user derive (`derive Name { fn ... }`) supplies the methods of an impl block;
//! an interface whose methods all have default bodies gets an impl block with no methods. Either way the impl block
//! is generated as syntax, so it is checked like a hand-written `Type: Name { ... }`; reflection in a derive's
//! bodies (`Self.fields`) sees the type the block is for.

use crate::derives::builtin_derive;
use crate::session::Sema;
use burdin_diag::{Code, Diagnostics, Span};
use burdin_syntax::ast;

impl<'t> Sema<'t> {
    /// Generates and registers the impl block of every name in a declaration's `: A, B` list that is not a
    /// built-in derive. This runs after the hand-written impl blocks: the interfaces and the types they are
    /// applied to are all known by then.
    pub(crate) fn register_user_derives(&mut self, diags: &mut Diagnostics) {
        for module in 0..self.module_asts.len() {
            let ast = self.module_asts[module];
            for item in &ast.items {
                let (name, generics, derives): (&ast::Ident, &[ast::GenericParam], &[ast::Type]) = match item {
                    ast::Item::Struct(s) => (&s.name, &s.generics, &s.derives),
                    ast::Item::Enum(e) => (&e.name, &e.generics, &e.derives),
                    _ => continue,
                };
                for clause in derives {
                    if builtin_derive(clause).is_some() {
                        continue;
                    }
                    let Some(methods) = self.declared_impl_methods(module, &name.name, clause, diags) else {
                        continue;
                    };
                    // The generated syntax lives as long as the compilation does: sema holds references into it
                    // for every method it checks, and the compiler exits when the compilation ends.
                    let imp: &'t ast::ImplBlock =
                        Box::leak(Box::new(generated_impl(name, generics, clause, methods, clause.span)));
                    self.register_impl(imp, module, diags);
                }
            }
        }
    }

    /// The methods of the impl block `type_name: <clause>` that a declaration's list generates: a user derive's
    /// methods, or none for an interface whose methods all have defaults. `None` after reporting why the name
    /// cannot be listed there.
    fn declared_impl_methods(
        &self,
        module: usize,
        type_name: &str,
        clause: &ast::Type,
        diags: &mut Diagnostics,
    ) -> Option<Vec<ast::FnDecl>> {
        let ast::TypeKind::Path(segments) = &clause.kind else {
            diags.error(Code::UNKNOWN_NAME, clause.span, "only an interface or a derive can follow `:` here");
            return None;
        };
        let last = segments.last().expect("a path has a segment");
        let iface_name = &last.name.name;
        if segments.len() == 1
            && last.args.is_empty()
            && let Some(&index) = self.globals.modules[module].derives.get(iface_name)
        {
            if self.globals.iface_name(module, iface_name).is_none() {
                diags.error(
                    Code::INVALID_DERIVE,
                    clause.span,
                    format!(
                        "`{type_name}: {iface_name}` applies the derive `{iface_name}`, but no interface named \
                         `{iface_name}` is in scope here; import it together with the derive"
                    ),
                );
                return None;
            }
            return Some(self.derive_srcs[index].decl.methods.clone());
        }
        let iface = if segments.len() == 1 {
            if let Some(method) = required_builtin_method(iface_name) {
                not_derivable(type_name, iface_name, method, clause.span, diags);
                return None;
            }
            self.globals.iface_name(module, iface_name)
        } else {
            let names: Vec<String> = segments[..segments.len() - 1].iter().map(|s| s.name.name.clone()).collect();
            self.globals.resolve_module_path(module, &names).and_then(|m| self.globals.exported_iface(m, iface_name))
        };
        let Some(iface) = iface else {
            diags.error(
                Code::UNKNOWN_NAME,
                clause.span,
                format!(
                    "`{iface_name}` is not a derive or an interface in scope; only `Copy`, `Clone`, `Eq`, `Ord`, \
                     `Hash`, `Default`, `Debug`, the `derive` declarations, and the interfaces in scope can follow `:`"
                ),
            );
            return None;
        };
        let info = &self.globals.interfaces[iface];
        if let Some(m) = info.methods.iter().find(|m| !m.has_default) {
            not_derivable(type_name, iface_name, &m.name, clause.span, diags);
            return None;
        }
        Some(Vec::new())
    }
}

/// E0234: `iface` has the method `method` with no default body, so a declaration cannot implement it by itself.
fn not_derivable(type_name: &str, iface: &str, method: &str, at: Span, diags: &mut Diagnostics) {
    diags.error(
        Code::NOT_DERIVABLE,
        at,
        format!(
            "`{iface}` cannot be derived: its method `{method}` has no default; write \
             `{type_name}: {iface} {{ fn {method}(...) {{ ... }} }}` instead"
        ),
    );
}

/// The method a built-in interface that is not derivable requires (`Drop`, `From`), by the interface's name.
fn required_builtin_method(name: &str) -> Option<&'static str> {
    match name {
        "Drop" => Some("drop"),
        "From" => Some("from"),
        "TryFrom" => Some("try_from"),
        _ => None,
    }
}

/// `Name<A, B>: Iface where A: Iface, B: Iface { <methods> }`. A generic type needs the same bound on its type
/// parameters, like the built-in derives (RFL9); a const parameter takes none.
fn generated_impl(
    type_name: &ast::Ident,
    generics: &[ast::GenericParam],
    iface: &ast::Type,
    methods: Vec<ast::FnDecl>,
    at: Span,
) -> ast::ImplBlock {
    let path = |name: &str, args: Vec<ast::Type>| ast::Type {
        kind: ast::TypeKind::Path(vec![ast::PathSegment {
            name: ast::Ident { name: name.to_string(), span: at },
            args,
        }]),
        span: at,
    };
    let args: Vec<ast::Type> = generics.iter().map(|g| path(&g.name.name, Vec::new())).collect();
    let where_clause = generics
        .iter()
        .filter(|g| g.const_ty.is_none())
        .map(|g| ast::WherePred { ty: path(&g.name.name, Vec::new()), bounds: vec![iface.clone()] })
        .collect();
    ast::ImplBlock {
        is_unsafe: false,
        ty: path(&type_name.name, args),
        interface: Some(iface.clone()),
        where_clause,
        assoc: Vec::new(),
        methods,
        span: at,
    }
}
