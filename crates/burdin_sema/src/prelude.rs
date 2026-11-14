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
        // The one-segment standard modules are usable as modules: `env.args()`, `mem.swap(...)`.
        let aliases: Vec<(String, usize)> = std_modules
            .iter()
            .filter_map(|&m| {
                let path = &self.globals.modules[m].path;
                (path.len() == 1).then(|| (path[0].clone(), m))
            })
            .collect();
        // The public names each standard module exports.
        type Exports = (Vec<(String, Global)>, Vec<(String, TypeName)>, Vec<(String, usize)>);
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
            exports.push((values, types, ifaces));
        }
        for module in 0..self.globals.modules.len() {
            if self.globals.modules[module].package == std_id {
                continue;
            }
            for (name, target) in &aliases {
                self.globals.modules[module].modules.entry(name.clone()).or_insert(*target);
            }
            for (values, types, ifaces) in &exports {
                for (name, global) in values {
                    self.globals.modules[module].values.entry(name.clone()).or_insert(*global);
                }
                for (name, ty) in types {
                    self.globals.modules[module].types.entry(name.clone()).or_insert(*ty);
                }
                for (name, iface) in ifaces {
                    self.globals.modules[module].ifaces.entry(name.clone()).or_insert(*iface);
                }
            }
        }
    }
}
