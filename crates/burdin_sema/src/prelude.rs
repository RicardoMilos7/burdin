//! The prelude: the standard library's public names in every module.
//!
//! The driver embeds `std` and makes it a dependency of every package. This pass copies the public values,
//! types, and interfaces of every standard-library module into every other module, and binds the standard
//! modules themselves under the names they are used with (`env.args()`, `process.exit()`). A module keeps its
//! own declaration or `use` when a prelude name would collide.

use crate::globals::{Global, OpIfaces, TypeName};
use crate::session::Sema;

impl Sema<'_> {
    /// Finds `std`'s `Option` and `Result` enums, which the language itself knows, as soon as the types are declared:
    /// signatures are resolved before the prelude is installed and write both names.
    pub(crate) fn find_lang_items(&mut self) {
        let Some(std_id) = self.globals.packages.iter().position(|p| p.name == "std" && !p.is_root) else { return };
        let Some(root) = (0..self.globals.modules.len())
            .find(|&m| self.globals.modules[m].package == std_id && self.globals.modules[m].path == ["std"])
        else {
            return;
        };
        let find = |globals: &crate::globals::Globals, name: &str| match globals.modules[root].types.get(name) {
            Some(TypeName::GenericAdt(g)) => Some(*g),
            _ => None,
        };
        self.globals.lang_option = find(&self.globals, "Option");
        self.globals.lang_result = find(&self.globals, "Result");
        self.globals.lang_list = find(&self.globals, "List");
        self.globals.lang_list_into_iter = find(&self.globals, "ListIntoIter");
        self.globals.lang_map = find(&self.globals, "Map");
        self.globals.lang_map_into_iter = find(&self.globals, "MapIntoIter");
        self.globals.lang_manually_drop = find(&self.globals, "ManuallyDrop");
        self.globals.lang_array_into_iter = find(&self.globals, "ArrayIntoIter");
        for (slot, name) in ["Range", "RangeInclusive", "RangeIter", "RangeIterInclusive"].into_iter().enumerate() {
            self.globals.lang_range[slot] = find(&self.globals, name);
        }
    }

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
        self.globals.parallel_ifaces = ["ParallelIterator", "ParallelValues", "IndexedParallelIterator"]
            .iter()
            .filter_map(|name| find_iface(&self.globals, name))
            .collect();
        self.globals.from_iterator_iface = find_iface(&self.globals, "FromIterator");
        self.globals.double_ended_iface = find_iface(&self.globals, "DoubleEndedIterator");
        self.globals.display_iface = find_iface(&self.globals, "Display");
        self.globals.debug_iface = find_iface(&self.globals, "Debug");
        self.globals.serialize_iface = find_iface(&self.globals, "Serialize");
        self.globals.deserialize_iface = find_iface(&self.globals, "Deserialize");
        self.globals.send_iface = find_iface(&self.globals, "Send");
        self.globals.sync_iface = find_iface(&self.globals, "Sync");
        // Allocators (`spec/allocators.md`): the interface a container's allocator must implement, and the
        // zero-sized default allocator `Global` that `List<T>` means when no allocator is written.
        self.globals.allocator_iface = find_iface(&self.globals, "Allocator");
        self.globals.global_alloc_ty = self.globals.std_adt_ty("Global");
        // The operator interfaces (`spec/operators.md`, OP2). All of them must be present; a standard library that
        // replaces the embedded one without them leaves the built-in operators only.
        self.globals.op_ifaces = (|| {
            Some(OpIfaces {
                add: find_iface(&self.globals, "Add")?,
                sub: find_iface(&self.globals, "Sub")?,
                mul: find_iface(&self.globals, "Mul")?,
                div: find_iface(&self.globals, "Div")?,
                rem: find_iface(&self.globals, "Rem")?,
                bitand: find_iface(&self.globals, "BitAnd")?,
                bitor: find_iface(&self.globals, "BitOr")?,
                bitxor: find_iface(&self.globals, "BitXor")?,
                neg: find_iface(&self.globals, "Neg")?,
                not: find_iface(&self.globals, "Not")?,
                eq: find_iface(&self.globals, "Eq")?,
                ord: find_iface(&self.globals, "Ord")?,
                index: find_iface(&self.globals, "Index")?,
                index_mut: find_iface(&self.globals, "IndexMut")?,
            })
        })();
        // The error surface: the `Error` marker interface, the dynamic `AnyError` value, and the helpers that build
        // one (`spec/errors.md`, ERR4). Everything is optional, so a program without `std` still checks.
        self.globals.error_iface = find_iface(&self.globals, "Error");
        let error_ty = self.globals.std_adt_ty("AnyError");
        self.globals.error_ty = error_ty;
        if let Some(&std_module) = std_modules.iter().find(|&&m| self.globals.modules[m].path == ["std"]) {
            // `exported_value` also finds `priv` declarations, which is how the compiler reaches these helpers.
            let func = |globals: &crate::globals::Globals, name: &str| match globals.exported_value(std_module, name) {
                Some(Global::Func(f)) => Some(f),
                _ => None,
            };
            let generic = |globals: &crate::globals::Globals, name: &str| match globals.exported_value(std_module, name)
            {
                Some(Global::Generic(g)) => Some(g),
                _ => None,
            };
            self.globals.error_plain = func(&self.globals, "error_plain");
            self.globals.error_at = func(&self.globals, "error_at");
            self.globals.error_frame = func(&self.globals, "error_frame");
            self.globals.error_context = func(&self.globals, "error_context");
            // The assertion declarations the compiler lowers to `Builtin::Assertion` (`spec/stdlib.md`).
            self.globals.assert_fn = func(&self.globals, "assert");
            self.globals.assert_eq_gen = generic(&self.globals, "assert_eq");
            self.globals.assert_ne_gen = generic(&self.globals, "assert_ne");
            self.globals.debug_assert_fn = func(&self.globals, "debug_assert");
            self.globals.debug_assert_eq_gen = generic(&self.globals, "debug_assert_eq");
            self.globals.debug_assert_ne_gen = generic(&self.globals, "debug_assert_ne");
            self.globals.snapshot_gen = generic(&self.globals, "assert_snapshot");
            self.globals.snapshot_debug_gen = generic(&self.globals, "snapshot_debug");
            self.globals.snapshot_display_gen = generic(&self.globals, "snapshot_display");
            if self.mode == crate::EntryMode::Test {
                self.globals.test_assert = func(&self.globals, "test_assert");
                self.globals.test_assert_eq = generic(&self.globals, "test_assert_eq");
                self.globals.test_assert_ne = generic(&self.globals, "test_assert_ne");
            }
        }
        self.globals.io_error_kind_ty = self.globals.std_adt_ty("IoErrorKind");
        self.globals.parse_int_error_ty = self.globals.std_adt_ty("ParseIntError");
        self.globals.alloc_error_ty = self.globals.std_adt_ty("AllocError");
        self.globals.try_from_int_error_ty = self.globals.std_adt_ty("TryFromIntError");
        self.globals.de_error_ty = self.globals.std_adt_ty("DeError");
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
        // The methods of `Option` and `Result` (`option.bd`) are extensions, and the standard library's own modules
        // call them without a `use`, so every std module sees that module's extensions.
        let option_module = std_modules.iter().position(|&m| self.globals.modules[m].path == ["option"]);
        if let Some(i) = option_module {
            let src = std_modules[i];
            for module in 0..self.globals.modules.len() {
                if module == src || self.globals.modules[module].package != std_id {
                    continue;
                }
                for (name, exts) in &exports[i].3 {
                    if self.globals.modules[module].extensions.contains_key(name) {
                        continue;
                    }
                    self.globals.modules[module].extensions.insert(name.clone(), exts.clone());
                }
            }
        }
        for module in 0..self.globals.modules.len() {
            if self.globals.modules[module].package == std_id {
                continue;
            }
            // The extension names the module declares or imports itself; the prelude never adds to those.
            let own_extensions: std::collections::HashSet<String> =
                self.globals.modules[module].extensions.keys().cloned().collect();
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
                // Two standard modules may extend different receivers under one name (`Option.replace` and
                // `str.replace`), so the prelude collects the extensions of all of them.
                for (name, exts) in extensions {
                    if own_extensions.contains(name) {
                        continue;
                    }
                    let known = self.globals.modules[module].extensions.entry(name.clone()).or_default();
                    for ext in exts {
                        if !known.contains(ext) {
                            known.push(*ext);
                        }
                    }
                }
            }
        }
        if self.mode == crate::EntryMode::Test {
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
        for (public, hidden) in
            [("assert", "test_assert"), ("assert_eq", "test_assert_eq"), ("assert_ne", "test_assert_ne")]
        {
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
        (Global::Static(x), Global::Static(y)) => x == y,
        (Global::Generic(x), Global::Generic(y)) => x == y,
        (Global::Extern(x), Global::Extern(y)) => x == y,
        _ => false,
    }
}
