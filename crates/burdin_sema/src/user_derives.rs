//! User derives (`spec/reflection.md`, RFL8-RFL10): `derive Name { fn ... }` declares the methods of an impl
//! block, and `derives (Name)` on a struct or enum applies them. The impl block is generated from the derive's
//! syntax, so it is checked like a hand-written `Type: Name { ... }`; reflection in its bodies (`Self.fields`) sees
//! the type the block is for.

use crate::derives::BUILTIN_DERIVES;
use crate::session::Sema;
use burdin_diag::{Code, Diagnostics, Span};
use burdin_syntax::ast;

impl<'t> Sema<'t> {
    /// Generates and registers the impl block of every user derive a type's `derives (...)` clause names. This runs
    /// after the hand-written impl blocks: the interface a derive implements and the type it is applied to are
    /// both known by then.
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
                    let ast::TypeKind::Path(segments) = &clause.kind else { continue };
                    if segments.len() != 1 || !segments[0].args.is_empty() {
                        continue;
                    }
                    let derive = &segments[0].name.name;
                    if BUILTIN_DERIVES.contains(&derive.as_str()) {
                        continue;
                    }
                    // An unknown name was reported when the derives were read.
                    let Some(&index) = self.globals.modules[module].derives.get(derive) else { continue };
                    if self.globals.iface_name(module, derive).is_none() {
                        diags.error(
                            Code::INVALID_DERIVE,
                            clause.span,
                            format!(
                                "`derives ({derive})` generates `{}: {derive}`, but no interface named `{derive}` is \
                                 in scope here; import it together with the derive",
                                name.name
                            ),
                        );
                        continue;
                    }
                    let decl = self.derive_srcs[index].decl;
                    // The generated syntax lives as long as the compilation does: sema holds references into it
                    // for every method it checks, and the compiler exits when the compilation ends.
                    let imp: &'t ast::ImplBlock =
                        Box::leak(Box::new(generated_impl(name, generics, decl, clause.span)));
                    self.register_impl(imp, module, diags);
                }
            }
        }
    }
}

/// `Name<A, B>: Derive where A: Derive, B: Derive { <the derive's methods> }`. A derive on a generic type needs
/// the same bound on its type parameters, like the built-in derives (RFL9); a const parameter takes none.
fn generated_impl(
    type_name: &ast::Ident,
    generics: &[ast::GenericParam],
    derive: &ast::DeriveDecl,
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
        .map(|g| ast::WherePred {
            ty: path(&g.name.name, Vec::new()),
            bounds: vec![path(&derive.name.name, Vec::new())],
        })
        .collect();
    ast::ImplBlock {
        is_unsafe: false,
        ty: path(&type_name.name, args),
        interface: Some(path(&derive.name.name, Vec::new())),
        where_clause,
        methods: derive.methods.clone(),
        span: at,
    }
}
