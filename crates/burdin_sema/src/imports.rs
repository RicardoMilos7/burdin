//! `use` items. A use path names a module (`use math`, `use geometry.shapes as sh`, `use lib`) or an item of a
//! module (`use math.add`, `use lib.{visible, Gadget}`). Imports are copied into the importing module's namespace,
//! and importing a package-private item from another package is E0150.

use crate::session::Sema;
use burdin_diag::{Code, Diagnostics};
use burdin_syntax::ast;
use std::collections::hash_map::Entry;

impl Sema<'_> {
    pub(crate) fn resolve_imports(&mut self, diags: &mut Diagnostics) {
        // A `pub use` may import a name another module re-exports, and that may come later in module order. Resolve
        // the uses that can be resolved now, repeatedly, and only report the ones that never become ready.
        let mut pending: Vec<(usize, usize)> = self
            .module_asts
            .iter()
            .enumerate()
            .flat_map(|(module, ast)| (0..ast.uses.len()).map(move |index| (module, index)))
            .collect();
        while !pending.is_empty() {
            let mut next = Vec::new();
            let mut progress = false;
            for (module, index) in pending {
                let u = &self.module_asts[module].uses[index];
                if self.use_ready(module, u) {
                    self.resolve_use(module, u, diags);
                    progress = true;
                } else {
                    next.push((module, index));
                }
            }
            if !progress {
                // Nothing became ready: resolve what is left so its error is reported once.
                for (module, index) in next {
                    let u = &self.module_asts[module].uses[index];
                    self.resolve_use(module, u, diags);
                }
                break;
            }
            pending = next;
        }
    }

    /// Whether every name a `use` needs is already bound. A `pub use` chain is resolved from its far end inward.
    fn use_ready(&self, module: usize, u: &ast::UseDecl) -> bool {
        let names: Vec<String> = u.path.iter().map(|i| i.name.clone()).collect();
        if !u.items.is_empty() {
            let Some(target) = self.globals.resolve_module_path(module, &names) else { return false };
            return u.items.iter().all(|item| self.item_exported(target, &item.name));
        }
        if self.globals.resolve_module_path(module, &names).is_some() {
            return true;
        }
        if names.len() > 1
            && let Some(target) = self.globals.resolve_module_path(module, &names[..names.len() - 1])
        {
            let item = &names[names.len() - 1];
            return self.item_exported(target, item);
        }
        false
    }

    fn item_exported(&self, module: usize, name: &str) -> bool {
        self.globals.exported_value(module, name).is_some()
            || self.globals.exported_type(module, name).is_some()
            || self.globals.exported_iface(module, name).is_some()
            || self.globals.exported_extension(module, name).is_some()
            || self.globals.exported_derive(module, name).is_some()
    }

    fn resolve_use(&mut self, module: usize, u: &ast::UseDecl, diags: &mut Diagnostics) {
        let names: Vec<String> = u.path.iter().map(|i| i.name.clone()).collect();

        // `use path.{a, b}`: every listed name is an item of the module.
        if !u.items.is_empty() {
            let Some(target) = self.globals.resolve_module_path(module, &names) else {
                self.unknown_module(module, &names, u.span, diags);
                return;
            };
            for item in &u.items {
                self.import_item(module, target, item, u.is_pub, diags);
            }
            return;
        }

        // `use path`: a module, or the last segment of the path is an item.
        if let Some(target) = self.globals.resolve_module_path(module, &names) {
            let name = u.alias.clone().unwrap_or_else(|| u.path.last().expect("a use path is not empty").clone());
            self.bind_module(module, name, target, u.is_pub, diags);
            return;
        }
        if names.len() > 1
            && let Some(target) = self.globals.resolve_module_path(module, &names[..names.len() - 1])
        {
            let item = u.path.last().expect("checked above");
            let name = u.alias.clone().unwrap_or_else(|| item.clone());
            self.import_item_as(module, target, item, name, u.is_pub, diags);
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
        self.unknown_module(module, &names, u.span, diags);
    }

    /// `unknown module`. When its first name is an official package the importing package does not load, the error
    /// says how to load it (`spec/packages.md`, PKG18).
    fn unknown_module(&self, module: usize, names: &[String], span: burdin_diag::Span, diags: &mut Diagnostics) {
        let error = diags.error(Code::UNKNOWN_NAME, span, format!("unknown module `{}`", names.join(".")));
        if self.globals.packages[self.globals.package_of(module)].official.contains(&names[0]) {
            let name = &names[0];
            error.help(format!(
                "`{name}` is an official package: `use {name}` in a hand-written file of this package loads it for \
                 the whole package (a generated module cannot load it on its own)"
            ));
        }
    }

    /// Binds `name` to the module `target` in `module`. A `pub use` also makes the alias a public module of
    /// `module`, so another package can reach it as a qualified path (issue #79).
    fn bind_module(&mut self, module: usize, name: ast::Ident, target: usize, is_pub: bool, diags: &mut Diagnostics) {
        if let Some(&previous) = self.globals.modules[module].modules.get(&name.name) {
            if previous != target {
                diags.error(Code::DUPLICATE_NAME, name.span, format!("`{}` is imported more than once", name.name));
            }
            return;
        }
        if self.globals.modules[module].values.contains_key(&name.name)
            || self.globals.modules[module].types.contains_key(&name.name)
        {
            diags.error(
                Code::DUPLICATE_NAME,
                name.span,
                format!("`{}` is imported here and also defined in this module", name.name),
            );
            return;
        }
        if is_pub {
            self.globals.modules[module].pub_modules.insert(name.name.clone());
        }
        self.globals.modules[module].modules.insert(name.name, target);
    }

    /// Imports one item, keeping its name.
    fn import_item(&mut self, module: usize, target: usize, item: &ast::Ident, is_pub: bool, diags: &mut Diagnostics) {
        self.import_item_as(module, target, item, item.clone(), is_pub, diags);
    }

    /// Imports one item under `name`, which may be an alias.
    fn import_item_as(
        &mut self,
        module: usize,
        target: usize,
        item: &ast::Ident,
        name: ast::Ident,
        is_pub: bool,
        diags: &mut Diagnostics,
    ) {
        let info = &self.globals.modules[target];
        // Only names the target module declares can be imported: a module's own imports are private to it.
        let value = self.globals.exported_value(target, &item.name);
        let type_name = self.globals.exported_type(target, &item.name);
        let iface = self.globals.exported_iface(target, &item.name);
        // An extension is imported by its method name; every declaration with that name comes along.
        let extensions = self.globals.exported_extension(target, &item.name);
        // A derive is imported by its name, together with the interface of that name (`spec/reflection.md`, RFL8).
        let derive = self.globals.exported_derive(target, &item.name);
        if value.is_none() && type_name.is_none() && iface.is_none() && extensions.is_none() && derive.is_none() {
            let module_name = self.globals.module_display(target);
            diags.error(Code::UNKNOWN_NAME, item.span, format!("module `{module_name}` has no item `{}`", item.name));
            return;
        }
        // `priv` is package-private: another package may not name the item at all.
        if self.globals.package_of(target) != self.globals.package_of(module) {
            let private = info.value_priv.get(&item.name).copied().unwrap_or(false)
                || info.type_priv.get(&item.name).copied().unwrap_or(false)
                || info.iface_priv.get(&item.name).copied().unwrap_or(false)
                || info.extension_priv.get(&item.name).copied().unwrap_or(false)
                || info.derive_priv.get(&item.name).copied().unwrap_or(false);
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
        // `pub use` may not widen a package-private item beyond its package.
        let declared_private = info.value_priv.get(&item.name).copied().unwrap_or(false)
            || info.type_priv.get(&item.name).copied().unwrap_or(false)
            || info.iface_priv.get(&item.name).copied().unwrap_or(false)
            || info.extension_priv.get(&item.name).copied().unwrap_or(false)
            || info.derive_priv.get(&item.name).copied().unwrap_or(false);
        let mut reexport = is_pub;
        if is_pub && declared_private {
            diags.error(
                Code::PRIVATE_ITEM,
                item.span,
                format!("cannot re-export `{}` with `pub use`: it is `priv`", item.name),
            );
            reexport = false;
        }
        let has_value = value.is_some();
        let has_type = type_name.is_some();
        let has_iface = iface.is_some();
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
        let mut has_extensions = false;
        if let Some(extensions) = extensions {
            if name.name != item.name {
                // An extension method is called by its declared name; a `use ... as` would bind a name no call
                // can reach (`spec/extensions.md`, EX2).
                diags.error(
                    Code::INVALID_EXTENSION,
                    name.span,
                    format!(
                        "an extension method cannot be imported under another name: `{}` is called by its declared name",
                        item.name
                    ),
                );
            } else {
                has_extensions = true;
                match self.globals.modules[module].extensions.entry(name_text.clone()) {
                    Entry::Occupied(_) => duplicate = true,
                    Entry::Vacant(slot) => {
                        slot.insert(extensions);
                    }
                }
            }
        }
        let mut has_derive = false;
        if let Some(derive) = derive {
            has_derive = true;
            match self.globals.modules[module].derives.entry(name_text.clone()) {
                Entry::Occupied(_) => duplicate = true,
                Entry::Vacant(slot) => {
                    slot.insert(derive);
                }
            }
        }
        if duplicate {
            diags.error(Code::DUPLICATE_NAME, name.span, format!("`{name_text}` is defined more than once"));
        }
        if reexport {
            let module_info = &mut self.globals.modules[module];
            if has_value {
                module_info.pub_values.insert(name_text.clone());
            }
            if has_type {
                module_info.pub_types.insert(name_text.clone());
            }
            if has_iface {
                module_info.pub_ifaces.insert(name_text.clone());
            }
            if has_extensions {
                module_info.pub_extensions.insert(name_text.clone());
            }
            if has_derive {
                module_info.pub_derives.insert(name_text.clone());
            }
        }
    }
}
