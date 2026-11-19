//! `burdin-dap`: the Debug Adapter Protocol server without the driver (`spec/debug.md`, DBG11). It needs
//! `burdin` on `PATH` (or `BURDIN_DAP_DRIVER`) only when a launch request names a `.bd` target.

fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(burdin_dap::serve_stdio() as u8)
}
