//! The prelude: the standard library's public names in every module.
//!
//! The driver embeds `std` and makes it a dependency of every package. This pass copies the public values,
//! types, and interfaces of every standard-library module into every other module, and binds the standard
//! modules themselves under the names they are used with (`env.args()`, `process.exit()`). A module keeps its
//! own declaration or `use` when a prelude name would collide.

use crate::globals::{Global, TypeName};
use crate::session::Sema;

impl Sema<'_> {
    pub(crate) fn install_prelude(&mut self) {
        let Some(std_id) = self.globals.packages.iter().position(|p| p.name == "std" && !p.is_root) else { return };
        let std_modules: Vec<usize> =
            (0..self.globals.modules.len()).filter(|&m| self.globals.modules[m].package == std_id).collect();
        if std_modules.is_empty() {
            return;
        }
        // Where the compiler finds the standard library's internal names: the iterator protocol's interfaces and
        // the eager `List` helpers, which are `priv` and therefore not copied into other modules.
        self.globals.std_modules = std_modules.clone();
        let find_iface = |globals: &crate::globals::Globals, name: &str| {
            (0..globals.interfaces.len()).find(|&i| {
                globals.modules[globals.interfaces[i].module].package == std_id
                    && globals.interfaces[i].name == globals.qualified(globals.interfaces[i].module, name)
            })
        };
        self.globals.iterator_iface = find_iface(&self.globals, "Iterator");
        self.globals.iterable_iface = find_iface(&self.globals, "Iterable");
        self.globals.display_iface = find_iface(&self.globals, "Display");
        self.globals.debug_iface = find_iface(&self.globals, "Debug");
        // The error surface: the `Error` marker interface, the dynamic `Error` value, and the helpers that build
        // one (`spec/errors.md`, ERR4). Everything is optional, so a program without `std` still checks.
        self.globals.error_iface = find_iface(&self.globals, "Error");
        let error_ty = self.globals.std_adt_ty("Error");
        self.globals.error_ty = error_ty;
        if let Some(&std_module) = std_modules.iter().find(|&&m| self.globals.modules[m].path == ["std"]) {
            // `exported_value` also finds `priv` declarations, which is how the compiler reaches these helpers.
            let func = |globals: &crate::globals::Globals, name: &str| match globals.exported_value(std_module, name) {
                Some(Global::Func(f)) => Some(f),
                _ => None,
            };
            self.globals.error_plain = func(&self.globals, "__error_plain");
            self.globals.error_at = func(&self.globals, "__error_at");
            self.globals.error_frame = func(&self.globals, "__error_frame");
            self.globals.error_context = func(&self.globals, "__error_context");
            if self.test_mode {
                self.globals.test_assert = func(&self.globals, "__test_assert");
                self.globals.test_assert_eq = match self.globals.exported_value(std_module, "__test_assert_eq") {
                    Some(Global::Generic(g)) => Some(g),
                    _ => None,
                };
            }
        }
        self.globals.io_error_ty = self.globals.std_adt_ty("IoError");
        self.globals.parse_int_error_ty = self.globals.std_adt_ty("ParseIntError");
        // The one-segment standard modules are usable as modules: `env.args()`, `mem.swap(...)`.
        let aliases: Vec<(String, usize)> = std_modules
            .iter()
            .filter_map(|&m| {
                let path = &self.globals.modules[m].path;
                (path.len() == 1).then(|| (path[0].clone(), m))
            })
            .collect();
        // The public names each standard module exports.
        type Exports =
            (Vec<(String, Global)>, Vec<(String, TypeName)>, Vec<(String, usize)>, Vec<(String, Vec<usize>)>);
        let mut exports: Vec<Exports> = Vec::new();
        for &src in &std_modules {
            let values = self.globals.modules[src]
                .values
                .keys()
                .filter(|name| !self.globals.modules[src].value_priv.get(*name).copied().unwrap_or(false))
                .filter_map(|name| self.globals.exported_value(src, name).map(|g| (name.clone(), g)))
                .collect();
            let types = self.globals.modules[src]
                .types
                .keys()
                .filter(|name| !self.globals.modules[src].type_priv.get(*name).copied().unwrap_or(false))
                .filter_map(|name| self.globals.exported_type(src, name).map(|t| (name.clone(), t)))
                .collect();
            let ifaces = self.globals.modules[src]
                .ifaces
                .keys()
                .filter(|name| !self.globals.modules[src].iface_priv.get(*name).copied().unwrap_or(false))
                .filter_map(|name| self.globals.exported_iface(src, name).map(|i| (name.clone(), i)))
                .collect();
            let extensions = self.globals.modules[src]
                .extensions
                .keys()
                .filter(|name| !self.globals.modules[src].extension_priv.get(*name).copied().unwrap_or(false))
                .filter_map(|name| self.globals.exported_extension(src, name).map(|e| (name.clone(), e)))
                .collect();
            exports.push((values, types, ifaces, extensions));
        }
        for module in 0..self.globals.modules.len() {
            if self.globals.modules[module].package == std_id {
                continue;
            }
            for (name, target) in &aliases {
                self.globals.modules[module].modules.entry(name.clone()).or_insert(*target);
            }
            for (values, types, ifaces, extensions) in &exports {
                for (name, global) in values {
                    self.globals.modules[module].values.entry(name.clone()).or_insert(*global);
                }
                for (name, ty) in types {
                    self.globals.modules[module].types.entry(name.clone()).or_insert(*ty);
                }
                for (name, iface) in ifaces {
                    self.globals.modules[module].ifaces.entry(name.clone()).or_insert(*iface);
                }
                for (name, exts) in extensions {
                    self.globals.modules[module].extensions.entry(name.clone()).or_insert_with(|| exts.clone());
                }
            }
        }
        if self.test_mode {
            self.install_test_assertions(std_modules.as_slice());
        }
    }

    /// In a test build, `assert` and `assert_eq` return `Result<(), AssertError>` instead of panicking, so
    /// `assert(...)?` in a test or in a helper that returns `Result` reports a failure to the runner. The
    /// definitions live in `std` behind names the prelude does not otherwise use; this replaces the prelude's
    /// panicking bindings wherever they were installed and leaves a module's own `assert` alone.
    fn install_test_assertions(&mut self, std_modules: &[usize]) {
        let Some(&std_module) = std_modules.iter().find(|&&m| self.globals.modules[m].path == ["std"]) else {
            return;
        };
        for (public, hidden) in [("assert", "__test_assert"), ("assert_eq", "__test_assert_eq")] {
            let Some(test_global) = self.globals.exported_value(std_module, hidden) else { continue };
            let panicking = self.globals.exported_value(std_module, public);
            for module in 0..self.globals.modules.len() {
                if self.globals.modules[module].package == self.globals.modules[std_module].package {
                    continue;
                }
                let current = self.globals.modules[module].values.get(public).copied();
                if current.is_some() && current.zip(panicking).is_some_and(|(c, p)| same_global(&c, &p)) {
                    self.globals.modules[module].values.insert(public.to_string(), test_global);
                }
            }
        }
    }
}

/// Whether two global names refer to the same item.
fn same_global(a: &Global, b: &Global) -> bool {
    match (a, b) {
        (Global::Func(x), Global::Func(y)) => x == y,
        (Global::Const(x), Global::Const(y)) => x == y,
        (Global::Generic(x), Global::Generic(y)) => x == y,
        (Global::Extern(x), Global::Extern(y)) => x == y,
        _ => false,
    }
}
