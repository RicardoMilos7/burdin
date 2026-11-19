//! The debugger scripts a `-g` binary carries (`spec/debug.md`, DBG6): one `.debug_gdb_scripts` entry holding the
//! gdb pretty-printers inline. gdb reads the section when it loads the program and executes the script, so no
//! installation or configuration is needed. The entry format is the documented one: a type byte `4` (inline Python
//! script), the script's name and a newline (the name identifies the script, so gdb runs it once), the script
//! itself, and a terminating NUL. There is no version byte.
//!
//! The script is the repository's own `debuggers/gdb/burdin_printers.py`, embedded here so the compiler and the
//! printers always ship together.

use super::{Codegen, GenResult};
use inkwell::module::Linkage;

/// The pretty-printers, embedded at compile time.
const PRINTERS: &str = include_str!("../../../../debuggers/gdb/burdin_printers.py");

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// Emits the `.debug_gdb_scripts` section for a `-g` build. Mach-O has no such section (its section names are
    /// `segment,section` pairs, which LLVM rejects for this one) and macOS debugs with lldb, so a Darwin build
    /// carries no gdb script (`spec/debug.md`, DBG6).
    pub(super) fn emit_debug_scripts(&mut self) -> GenResult<()> {
        let triple = self.module.get_triple();
        let triple = triple.as_str().to_string_lossy();
        if triple.contains("darwin") || triple.contains("apple") {
            return Ok(());
        }
        let mut bytes: Vec<u8> = Vec::with_capacity(PRINTERS.len() + 32);
        bytes.push(4); // SECTION_SCRIPT_ID_PYTHON_TEXT
        bytes.extend_from_slice(b"burdin_printers.py\n");
        bytes.extend_from_slice(PRINTERS.as_bytes());
        bytes.push(0);
        let data = self.cx.const_string(&bytes, false);
        let global = self.module.add_global(data.get_type(), None, "burdin.gdb_scripts");
        global.set_initializer(&data);
        global.set_section(Some(".debug_gdb_scripts"));
        global.set_constant(true);
        // The entry is externally visible so `globaldce` keeps it: nothing in the program references it.
        global.set_linkage(Linkage::External);
        global.set_unnamed_addr(true);
        global.set_alignment(1);
        Ok(())
    }
}
