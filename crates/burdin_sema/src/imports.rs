//! `use` items. A use path names a module (`use math`, `use geometry.shapes as sh`, `use lib`) or an item of a
//! module (`use math.add`, `use lib.{visible, Gadget}`). Imports are copied into the importing module's namespace,
//! and importing a package-private item from another package is E0150.

use crate::session::Sema;
use burdin_diag::{Code, Diagnostics};
use burdin_syntax::ast;
use std::collections::hash_map::Entry;

impl Sema<'_> {
    pub(crate) fn resolve_imports(&mut self, diags: &mut Diagnostics) {
        for module in 0..self.module_asts.len() {
            let ast = self.module_asts[module];
            for u in &ast.uses {
                self.resolve_use(module, u, diags);
            }
        }
    }

    fn resolve_use(&mut self, module: usize, u: &ast::UseDecl, diags: &mut Diagnostics) {
        let names: Vec<String> = u.path.iter().map(|i| i.name.clone()).collect();
        let path_text = names.join(".");

        // `use path.{a, b}`: every listed name is an item of the module.
        if !u.items.is_empty() {
            let Some(target) = self.globals.resolve_module_path(module, &names) else {
                diags.error(Code::UNKNOWN_NAME, u.span, format!("unknown module `{path_text}`"));
                return;
            };
            for item in &u.items {
                self.import_item(module, target, item, diags);
            }
            return;
        }

        // `use path`: a module, or the last segment of the path is an item.
        if let Some(target) = self.globals.resolve_module_path(module, &names) {
            let name = u.alias.clone().unwrap_or_else(|| u.path.last().expect("a use path is not empty").clone());
            self.bind_module(module, name, target, diags);
            return;
        }
        if names.len() > 1
            && let Some(target) = self.globals.resolve_module_path(module, &names[..names.len() - 1])
        {
            let item = u.path.last().expect("checked above");
            let name = u.alias.clone().unwrap_or_else(|| item.clone());
            self.import_item_as(module, target, item, name, diags);
            return;
        }
        // A dependency without a `src/lib.bd` or `src/<name>.bd` cannot be imported as a whole.
        if names.len() == 1
            && let Some(&dep) = self.globals.packages[self.globals.package_of(module)].deps.get(&names[0])
            && self.globals.packages[dep].root_module.is_none()
        {
            diags.error(
                Code::UNKNOWN_NAME,
                u.span,
                if names[0] == "lib" {
                    "package `lib` has no root module; add `src/lib.bd`".to_string()
                } else {
                    format!("package `{}` has no root module; add `src/{}.bd` (or `src/lib.bd`)", names[0], names[0])
                },
            );
            return;
        }
        diags.error(Code::UNKNOWN_NAME, u.span, format!("unknown module `{path_text}`"));
    }

    /// Binds `name` to the module `target` in `module`.
    fn bind_module(&mut self, module: usize, name: ast::Ident, target: usize, diags: &mut Diagnostics) {
        if let Some(&previous) = self.globals.modules[module].modules.get(&name.name) {
            if previous != target {
                diags.error(Code::UNSUPPORTED, name.span, format!("`{}` is imported more than once", name.name));
            }
            return;
        }
        if self.globals.modules[module].values.contains_key(&name.name)
            || self.globals.modules[module].types.contains_key(&name.name)
        {
            diags.error(
                Code::UNSUPPORTED,
                name.span,
                format!("`{}` is imported here and also defined in this module", name.name),
            );
            return;
        }
        self.globals.modules[module].modules.insert(name.name, target);
    }

    /// Imports one item, keeping its name.
    fn import_item(&mut self, module: usize, target: usize, item: &ast::Ident, diags: &mut Diagnostics) {
        self.import_item_as(module, target, item, item.clone(), diags);
    }

    /// Imports one item under `name`, which may be an alias.
    fn import_item_as(
        &mut self,
        module: usize,
        target: usize,
        item: &ast::Ident,
        name: ast::Ident,
        diags: &mut Diagnostics,
    ) {
        let info = &self.globals.modules[target];
        // Only names the target module declares can be imported: a module's own imports are private to it.
        let value = self.globals.exported_value(target, &item.name);
        let type_name = self.globals.exported_type(target, &item.name);
        let iface = self.globals.exported_iface(target, &item.name);
        if value.is_none() && type_name.is_none() && iface.is_none() {
            let module_name = self.globals.module_display(target);
            diags.error(Code::UNKNOWN_NAME, item.span, format!("module `{module_name}` has no item `{}`", item.name));
            return;
        }
        // `priv` is package-private: another package may not name the item at all.
        if self.globals.package_of(target) != self.globals.package_of(module) {
            let private = info.value_priv.get(&item.name).copied().unwrap_or(false)
                || info.type_priv.get(&item.name).copied().unwrap_or(false)
                || info.iface_priv.get(&item.name).copied().unwrap_or(false);
            if private {
                diags.error(
                    Code::PRIVATE_ITEM,
                    item.span,
                    format!(
                        "`{}` is `priv` in package `{}`",
                        item.name,
                        self.globals.packages[self.globals.package_of(target)].name
                    ),
                );
            }
        }
        let name_text = name.name.clone();
        let mut duplicate = false;
        if let Some(value) = value {
            match self.globals.modules[module].values.entry(name_text.clone()) {
                Entry::Occupied(_) => duplicate = true,
                Entry::Vacant(slot) => {
                    slot.insert(value);
                }
            }
        }
        if let Some(type_name) = type_name {
            match self.globals.modules[module].types.entry(name_text.clone()) {
                Entry::Occupied(_) => duplicate = true,
                Entry::Vacant(slot) => {
                    slot.insert(type_name);
                }
            }
        }
        if let Some(iface) = iface {
            match self.globals.modules[module].ifaces.entry(name_text.clone()) {
                Entry::Occupied(_) => duplicate = true,
                Entry::Vacant(slot) => {
                    slot.insert(iface);
                }
            }
        }
        if duplicate {
            diags.error(Code::UNSUPPORTED, name.span, format!("`{name_text}` is defined more than once"));
        }
    }
}
