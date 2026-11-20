//! `burdin toolchain ...` (`spec/toolchain.md` TC6-TC12): the manager's commands. Results go to standard output and
//! progress to standard error, so the output of `list`, `show`, `which`, and the rest can be read by a script.

use std::fs;

use crate::home::io_error;
use crate::install::{COMPONENTS, Context, check_component};
use crate::proxy::{context, current_dir, install_for_file, require_selection};
use crate::select::{self, Reason};
use crate::version::{EXE, TARGETS, ToolchainName};
use crate::{Failure, Outcome, selfcmd};

pub const USAGE: &str = "\
Usage: burdin [+<toolchain>] toolchain <command> [arguments]

Manage Burdin toolchains. A toolchain is `stable`, `nightly`, or an exact version such as `0.4.1`.

Commands:
  install <toolchain>... [--component <name>]...   install toolchains (the first one becomes the default if none is set)
  update [<toolchain>...]                          update channel toolchains to their latest release (default: all)
  list                                             list the installed toolchains
  default [<toolchain>]                            print or set the default toolchain
  remove <toolchain>...                            remove toolchains
  show                                             print the active toolchain and why it was selected
  which [burdin|burdin-lsp]                        print the path of the active toolchain's binary
  component list|add|remove [<name>...] [--toolchain <toolchain>]
                                                   manage the optional components: debuggers, docs, lsp, std-src
  target list|add|remove [<triple>...] [--toolchain <toolchain>]
                                                   manage the targets a toolchain can build for
  self show|update|uninstall [--yes]               show where everything lives, update the manager, or remove it

Toolchain selection, first match wins: `burdin +<toolchain> ...`, the BURDIN_TOOLCHAIN environment variable, the
nearest burdin-toolchain.toml, then the default toolchain.";

/// Runs `burdin [+toolchain] toolchain <args>`.
pub fn run(toolchain: Option<&str>, args: &[String]) -> Outcome {
    let Some(command) = args.first() else {
        eprintln!("{USAGE}");
        return Ok(2);
    };
    let rest = &args[1..];
    match command.as_str() {
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(0)
        }
        "install" => install(rest),
        "update" => update(rest),
        "list" => no_arguments("list", rest).and_then(|_| list()),
        "default" => default(rest),
        "remove" => remove(rest),
        "show" => no_arguments("show", rest).and_then(|_| show(toolchain)),
        "which" => which(toolchain, rest),
        "component" => component(toolchain, rest),
        "target" => target(toolchain, rest),
        "self" => selfcmd::run(rest),
        other => Err(Failure::usage(format!("unknown command `toolchain {other}` (see `burdin toolchain --help`)"))),
    }
}

fn no_arguments(command: &str, rest: &[String]) -> Result<(), Failure> {
    match rest.first() {
        Some(extra) => Err(Failure::usage(format!("unexpected argument `{extra}` for `burdin toolchain {command}`"))),
        None => Ok(()),
    }
}

/// Splits `args` into positional arguments and the values of `option` (which may repeat).
fn split_option(command: &str, args: &[String], option: &str) -> Result<(Vec<String>, Vec<String>), Failure> {
    let mut positional = Vec::new();
    let mut values = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == option {
            let value = args.get(index + 1).ok_or_else(|| Failure::usage(format!("{option} needs a value")))?;
            values.push(value.clone());
            index += 2;
        } else if let Some(value) = arg.strip_prefix(&format!("{option}=")) {
            values.push(value.to_string());
            index += 1;
        } else if arg.starts_with('-') {
            return Err(Failure::usage(format!("unknown option `{arg}` for `burdin toolchain {command}`")));
        } else {
            positional.push(arg.clone());
            index += 1;
        }
    }
    Ok((positional, values))
}

fn parse_names(names: &[String]) -> Result<Vec<ToolchainName>, Failure> {
    names.iter().map(|name| ToolchainName::parse(name).map_err(Failure::usage)).collect()
}

fn install(args: &[String]) -> Outcome {
    let (names, components) = split_option("install", args, "--component")?;
    if names.is_empty() {
        return Err(Failure::usage("`burdin toolchain install` needs at least one toolchain"));
    }
    let names = parse_names(&names)?;
    for component in &components {
        check_component(component).map_err(Failure::usage)?;
    }
    let ctx = context()?;
    for name in &names {
        match ctx.home.record(name) {
            Some(record) => {
                println!("{name} {} is already installed", record.version);
                for component in components.iter().filter(|c| !record.components.contains(c)) {
                    ctx.add_component_into(&ctx.home.toolchain_dir(name), &record.version, component)?;
                    println!("installed component {component} in {name}");
                }
            }
            None => {
                let version = ctx.resolve(name)?;
                ctx.install(name, &version, &components)?;
                println!("installed {name} {version}");
            }
        }
    }
    if ctx.home.default_toolchain()?.is_none() {
        ctx.home.set_default(Some(&names[0]))?;
        println!("default toolchain: {}", names[0]);
    }
    Ok(0)
}

fn update(args: &[String]) -> Outcome {
    if let Some(option) = args.iter().find(|arg| arg.starts_with('-')) {
        return Err(Failure::usage(format!("unknown option `{option}` for `burdin toolchain update`")));
    }
    let names = args;
    let ctx = context()?;
    let names: Vec<ToolchainName> = if names.is_empty() {
        let channels: Vec<ToolchainName> = ctx
            .home
            .installed()
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| matches!(name, ToolchainName::Channel(_)))
            .collect();
        if channels.is_empty() {
            println!("no channel toolchain is installed; exact versions never change");
            return Ok(0);
        }
        channels
    } else {
        parse_names(names)?
    };
    let mut failed = false;
    for name in &names {
        if let Err(message) = update_one(&ctx, name) {
            eprintln!("error: {name}: {message}");
            failed = true;
        }
    }
    Ok(if failed { 1 } else { 0 })
}

fn update_one(ctx: &Context, name: &ToolchainName) -> Result<(), String> {
    let record = ctx.home.record(name).ok_or_else(|| "not installed".to_string())?;
    if let ToolchainName::Exact(version) = name {
        println!("{name}: {version} is an exact version and never changes");
        return Ok(());
    }
    let latest = ctx.resolve(name)?;
    if latest == record.version {
        println!("{name}: {latest} is up to date");
        return Ok(());
    }
    ctx.install(name, &latest, &record.components)?;
    println!("{name}: updated {} -> {latest}", record.version);
    Ok(())
}

fn list() -> Outcome {
    let ctx = context()?;
    let installed = ctx.home.installed();
    if installed.is_empty() {
        println!("no toolchains are installed");
        return Ok(0);
    }
    let default = ctx.home.default_toolchain()?;
    for (name, record) in installed {
        let marker = if default.as_ref() == Some(&name) { " (default)" } else { "" };
        println!("{name} {}{marker}", record.version);
    }
    Ok(0)
}

fn default(args: &[String]) -> Outcome {
    let ctx = context()?;
    match args {
        [] => match ctx.home.default_toolchain()? {
            Some(name) => {
                println!("{name}");
                Ok(0)
            }
            None => Err(Failure::from("no default toolchain is set")),
        },
        [name] => {
            let name = ToolchainName::parse(name).map_err(Failure::usage)?;
            if ctx.home.record(&name).is_none() {
                let version = ctx.resolve(&name)?;
                ctx.install(&name, &version, &[])?;
                println!("installed {name} {version}");
            }
            ctx.home.set_default(Some(&name))?;
            println!("default toolchain: {name}");
            Ok(0)
        }
        [_, extra, ..] => Err(Failure::usage(format!("unexpected argument `{extra}` for `burdin toolchain default`"))),
    }
}

fn remove(args: &[String]) -> Outcome {
    if args.is_empty() {
        return Err(Failure::usage("`burdin toolchain remove` needs at least one toolchain"));
    }
    let names = parse_names(args)?;
    let ctx = context()?;
    for name in &names {
        if ctx.home.record(name).is_none() {
            return Err(Failure::from(format!("toolchain {name} is not installed")));
        }
        let dir = ctx.home.toolchain_dir(name);
        fs::remove_dir_all(&dir).map_err(|e| io_error("remove", &dir, e))?;
        println!("removed {name}");
        if ctx.home.default_toolchain()?.as_ref() == Some(name) {
            ctx.home.set_default(None)?;
            println!("{name} was the default toolchain; no default is set now");
        }
    }
    Ok(0)
}

fn show(toolchain: Option<&str>) -> Outcome {
    let ctx = context()?;
    let Some(selection) = select::select(&ctx.home, toolchain, &current_dir()?)? else {
        println!("active toolchain: none");
        println!("selected by: nothing (no +toolchain, BURDIN_TOOLCHAIN, burdin-toolchain.toml, or default toolchain)");
        return Ok(0);
    };
    println!("active toolchain: {}", selection.name);
    println!("selected by: {}", selection.reason);
    match ctx.home.record(&selection.name) {
        Some(record) => println!("installed: {}", record.version),
        None if matches!(selection.reason, Reason::File(_)) => println!("installed: no (installed on first use)"),
        None => println!("installed: no"),
    }
    Ok(0)
}

fn which(toolchain: Option<&str>, args: &[String]) -> Outcome {
    let tool = match args {
        [] => "burdin",
        [tool] if tool == "burdin" || tool == "burdin-lsp" => tool.as_str(),
        [tool] => return Err(Failure::usage(format!("`{tool}` is not a toolchain binary (burdin or burdin-lsp)"))),
        [_, extra, ..] => {
            return Err(Failure::usage(format!("unexpected argument `{extra}` for `burdin toolchain which`")));
        }
    };
    let ctx = context()?;
    let selection = require_selection(&ctx, toolchain)?;
    install_for_file(&ctx, &selection)?;
    let name = &selection.name;
    if ctx.home.record(name).is_none() {
        return Err(Failure::from(format!("toolchain {name} is not installed")));
    }
    let exe = ctx.home.toolchain_dir(name).join(format!("{tool}{EXE}"));
    if !exe.is_file() {
        return Err(Failure::from(format!("toolchain {name} has no {tool}{EXE}")));
    }
    println!("{}", exe.display());
    Ok(0)
}

/// The toolchain a `component` or `target` command acts on: `--toolchain`, else the selected one.
fn chosen_toolchain(ctx: &Context, toolchain: Option<&str>, option: &[String]) -> Result<ToolchainName, Failure> {
    let name = match option {
        [] => require_selection(ctx, toolchain)?.name,
        [name] => ToolchainName::parse(name).map_err(Failure::usage)?,
        _ => return Err(Failure::usage("--toolchain is given more than once")),
    };
    if ctx.home.record(&name).is_none() {
        return Err(Failure::from(format!("toolchain {name} is not installed")));
    }
    Ok(name)
}

fn component(toolchain: Option<&str>, args: &[String]) -> Outcome {
    let Some(action) = args.first() else {
        return Err(Failure::usage("`burdin toolchain component` needs list, add, or remove"));
    };
    let (names, option) = split_option("component", &args[1..], "--toolchain")?;
    for name in &names {
        check_component(name).map_err(Failure::usage)?;
    }
    let ctx = context()?;
    let toolchain = chosen_toolchain(&ctx, toolchain, &option)?;
    let dir = ctx.home.toolchain_dir(&toolchain);
    let record = ctx.home.record(&toolchain).ok_or("the toolchain record disappeared")?;
    match action.as_str() {
        "list" => {
            if let Some(extra) = names.first() {
                return Err(Failure::usage(format!("unexpected argument `{extra}` for `component list`")));
            }
            for (name, about) in COMPONENTS {
                let state = if record.components.iter().any(|c| c == name) { "installed" } else { "available" };
                println!("{name} ({state}): {about}");
            }
        }
        "add" | "remove" if names.is_empty() => {
            return Err(Failure::usage(format!("`component {action}` needs at least one component")));
        }
        "add" => {
            for name in &names {
                if record.components.contains(name) {
                    println!("{name} is already installed in {toolchain}");
                } else {
                    ctx.add_component_into(&dir, &record.version, name)?;
                    println!("installed {name} in {toolchain}");
                }
            }
        }
        "remove" => {
            for name in &names {
                ctx.remove_component(&dir, name).map_err(|e| format!("{toolchain}: {e}"))?;
                println!("removed {name} from {toolchain}");
            }
        }
        other => return Err(Failure::usage(format!("unknown command `component {other}` (list, add, or remove)"))),
    }
    Ok(0)
}

/// Why a target other than the host cannot be added yet.
pub fn cross_target_error(target: &str) -> String {
    format!(
        "cannot add the target {target}: cross-compilation is not available yet (issue #282), so a toolchain builds \
         only for the host it runs on"
    )
}

fn target(toolchain: Option<&str>, args: &[String]) -> Outcome {
    let Some(action) = args.first() else {
        return Err(Failure::usage("`burdin toolchain target` needs list, add, or remove"));
    };
    let (triples, option) = split_option("target", &args[1..], "--toolchain")?;
    for triple in &triples {
        if !TARGETS.contains(&triple.as_str()) {
            return Err(Failure::usage(format!("`{triple}` is not one of the six release targets")));
        }
    }
    let ctx = context()?;
    let toolchain = chosen_toolchain(&ctx, toolchain, &option)?;
    match action.as_str() {
        "list" => {
            if let Some(extra) = triples.first() {
                return Err(Failure::usage(format!("unexpected argument `{extra}` for `target list`")));
            }
            for triple in TARGETS {
                let state = if triple == ctx.target { "installed, host" } else { "not available" };
                println!("{triple} ({state})");
            }
        }
        "add" | "remove" if triples.is_empty() => {
            return Err(Failure::usage(format!("`target {action}` needs at least one target")));
        }
        "add" => {
            for triple in &triples {
                if triple == ctx.target {
                    println!("{triple} is already installed in {toolchain}: it is the host target");
                } else {
                    return Err(Failure::from(cross_target_error(triple)));
                }
            }
        }
        "remove" => {
            // Only the host target is ever installed, and it cannot be removed.
            let triple = &triples[0];
            return Err(Failure::from(if triple == ctx.target {
                format!("{triple} is the host target of {toolchain} and cannot be removed")
            } else {
                format!("{triple} is not installed in {toolchain}")
            }));
        }
        other => return Err(Failure::usage(format!("unknown command `target {other}` (list, add, or remove)"))),
    }
    Ok(0)
}
