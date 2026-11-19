//! The proxy (`spec/toolchain.md` TC7, TC9): `burdin [+toolchain] <args>` runs the selected toolchain's `burdin`,
//! and `burdin-lsp [+toolchain] <args>` its `burdin-lsp`; `burdin [+toolchain] toolchain ...` is the manager.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::dist::Source;
use crate::home::Home;
use crate::install::Context;
use crate::select::{self, Reason, Selection};
use crate::version::{EXE, host_target};
use crate::{Failure, Outcome, cli};

/// Runs the proxy or the manager and returns the exit status.
pub fn main(argv0: &OsStr, args: Vec<OsString>) -> i32 {
    remove_replaced_self();
    let tool =
        if Path::new(argv0).file_stem().is_some_and(|stem| stem == "burdin-lsp") { "burdin-lsp" } else { "burdin" };
    let mut args = args;
    let mut toolchain = None;
    if let Some(first) = args.first().and_then(|a| a.to_str())
        && let Some(name) = first.strip_prefix('+')
    {
        toolchain = Some(name.to_string());
        args.remove(0);
    }
    let outcome = if tool == "burdin" && args.first().is_some_and(|a| a == "toolchain") {
        let rest: Result<Vec<String>, _> = args[1..].iter().map(|a| a.clone().into_string()).collect();
        match rest {
            Ok(rest) => cli::run(toolchain.as_deref(), &rest),
            Err(_) => Err(Failure::usage("the arguments of `burdin toolchain` must be valid Unicode")),
        }
    } else {
        run_tool(tool, toolchain.as_deref(), &args)
    };
    match outcome {
        Ok(status) => status,
        Err(failure) => {
            eprintln!("error: {}", failure.message);
            failure.status
        }
    }
}

/// The context every command shares: the home, the release source, and the host target.
pub fn context() -> Result<Context, Failure> {
    let home = Home::locate()?;
    let target = host_target().ok_or("this platform is not one of the six release targets")?;
    Ok(Context { home, source: Source::from_env(), target })
}

pub fn current_dir() -> Result<PathBuf, Failure> {
    std::env::current_dir().map_err(|e| Failure::from(format!("cannot read the current directory: {e}")))
}

/// The selected toolchain; an error when nothing selects one.
pub fn require_selection(ctx: &Context, toolchain: Option<&str>) -> Result<Selection, Failure> {
    select::select(&ctx.home, toolchain, &current_dir()?)?.ok_or_else(|| {
        Failure::from(
            "no toolchain is selected and no default is set: install one with `burdin toolchain install stable`",
        )
    })
}

/// Makes sure what a `burdin-toolchain.toml` asks for is installed: the toolchain, then its components (TC8). A
/// target other than the host is refused, because there is no cross-compilation yet (issue #282).
pub fn install_for_file(ctx: &Context, selection: &Selection) -> Result<(), Failure> {
    let Reason::File(file) = &selection.reason else { return Ok(()) };
    for target in &file.targets {
        if target != ctx.target {
            return Err(Failure::from(format!("{}: {}", file.path.display(), crate::cli::cross_target_error(target))));
        }
    }
    let name = &selection.name;
    let dir = ctx.home.toolchain_dir(name);
    match ctx.home.record(name) {
        None => {
            eprintln!("info: installing {name} for {}", file.path.display());
            let version = ctx.resolve(name)?;
            ctx.install(name, &version, &file.components)?;
        }
        Some(record) => {
            for component in &file.components {
                if !record.components.contains(component) {
                    eprintln!("info: installing component {component} of {name} for {}", file.path.display());
                    ctx.add_component_into(&dir, &record.version, component)?;
                }
            }
        }
    }
    Ok(())
}

fn run_tool(tool: &str, toolchain: Option<&str>, args: &[OsString]) -> Outcome {
    let ctx = context()?;
    let selection = require_selection(&ctx, toolchain)?;
    install_for_file(&ctx, &selection)?;
    let name = &selection.name;
    if ctx.home.record(name).is_none() {
        return Err(Failure::from(format!(
            "toolchain {name} is not installed; install it with `burdin toolchain install {name}`"
        )));
    }
    let dir = ctx.home.toolchain_dir(name);
    let exe = dir.join(format!("{tool}{EXE}"));
    if !exe.is_file() {
        return Err(Failure::from(if tool == "burdin-lsp" {
            format!("toolchain {name} has no burdin-lsp; add it with `burdin toolchain component add lsp`")
        } else {
            format!("toolchain {name} has no {tool}{EXE}; reinstall it with `burdin toolchain install {name}`")
        }));
    }
    if same_file(&exe, &std::env::current_exe().unwrap_or_default()) {
        return Err(Failure::from(format!("toolchain {name}'s {tool} is this proxy itself")));
    }
    let mut command = Command::new(&exe);
    command.args(args).env("BURDIN_TOOLCHAIN", name.to_string()).env("BURDIN_TOOLCHAIN_DIR", &dir);
    exec(command, &exe)
}

#[cfg(unix)]
fn exec(mut command: Command, exe: &Path) -> Outcome {
    use std::os::unix::process::CommandExt;
    let error = command.exec();
    Err(Failure::from(format!("cannot run {}: {error}", exe.display())))
}

#[cfg(not(unix))]
fn exec(mut command: Command, exe: &Path) -> Outcome {
    let status = command.status().map_err(|e| Failure::from(format!("cannot run {}: {e}", exe.display())))?;
    Ok(status.code().unwrap_or(1))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// On Windows a running executable cannot be overwritten, so `self update` renames the old proxy to `<name>.old`;
/// the next run removes it.
fn remove_replaced_self() {
    if cfg!(windows)
        && let Ok(exe) = std::env::current_exe()
    {
        let mut old = exe.into_os_string();
        old.push(".old");
        let _ = std::fs::remove_file(PathBuf::from(old));
    }
}
