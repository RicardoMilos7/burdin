//! `burdin check`: type-check only (`spec/check.md`). The command runs the same front end as `burdin build` (build
//! scripts, parsing, name resolution and type checking, the memory checks, and compile-time evaluation) and stops
//! before code generation, so it needs no LLVM work and no linker. It prints the same diagnostics, in the same
//! formats, and exits 0 exactly when `burdin build` would get as far as code generation.
//!
//! It also renders the ownership facts the memory checks recorded (`spec/check.md`, CK10-CK11; issue #587): the
//! source of each function with margin annotations (`--explain-ownership`), the same facts as JSON, and a
//! step-by-step trace for a memory error (`--explain`).

use crate::{
    CompileOptions, DiagnosticOptions, EntryMode, FrontEnd, SourceOverrides, flag_without_value, front_end,
    jobs_or_default, option_value, parse_diagnostic_option, parse_jobs, print_warnings, report_errors,
};
use burdin_diag::{Json, SourceMap, Span};
use burdin_query::{Convention, FunctionOwnership, OwnershipEventInfo, OwnershipEventKind};
use burdin_tir::{Function, MOVED_DEREF, Program};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OwnershipFormat {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Default)]
struct CheckArgs {
    target: PathBuf,
    release: bool,
    tests: bool,
    /// `--locked`: refuse to update `burdin.lock`.
    locked: bool,
    /// `--features`, `--no-default-features`, and `--all-features` (`spec/packages.md`, PKG25).
    features: burdin_query::package::FeatureRequest,
    /// `--print-instances`: print the monomorphization report after a clean check.
    print_instances: bool,
    comptime_steps: Option<u64>,
    jobs: Option<usize>,
    diagnostics: DiagnosticOptions,
    explain_ownership: bool,
    explain: bool,
    function: Option<String>,
    /// `--format`: `human`/`text` or `json`, for `--print-instances` and `--explain-ownership`.
    format: Option<OwnershipFormat>,
}

fn parse(argv: &[String]) -> Result<CheckArgs, String> {
    let mut args = CheckArgs::default();
    let mut target = None;
    let mut it = argv.iter();
    while let Some(raw) = it.next() {
        let (a, inline) = match raw.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            // `-j4` is `-j 4`, as in cargo and make.
            _ if raw.len() > 2 && raw.starts_with("-j") => ("-j", Some(&raw[2..])),
            _ => (raw.as_str(), None),
        };
        if parse_diagnostic_option(&mut it, inline, a, &mut args.diagnostics)? {
            continue;
        }
        match a {
            "--release" => args.release = flag_without_value(a, inline)?,
            "--tests" => args.tests = flag_without_value(a, inline)?,
            "--locked" => args.locked = flag_without_value(a, inline)?,
            "--features" => {
                let list = option_value(&mut it, inline, a, "a list of features")?;
                args.features.items.extend(burdin_query::features::split_request(&list));
            }
            "--no-default-features" => args.features.no_default_features = flag_without_value(a, inline)?,
            "--all-features" => args.features.all_features = flag_without_value(a, inline)?,
            "--print-instances" => args.print_instances = flag_without_value(a, inline)?,
            "--explain-ownership" => args.explain_ownership = flag_without_value(a, inline)?,
            "--explain" => args.explain = flag_without_value(a, inline)?,
            "-j" | "--jobs" => args.jobs = Some(parse_jobs(&mut it, inline, a)?),
            "--fn" => args.function = Some(option_value(&mut it, inline, a, "a function name")?),
            "--format" => {
                let value = option_value(&mut it, inline, a, "`text` or `json`")?;
                args.format = Some(match value.as_str() {
                    "human" | "text" => OwnershipFormat::Text,
                    "json" => OwnershipFormat::Json,
                    _ => return Err(format!("`--format` takes `human`, `text`, or `json`, got `{value}`")),
                });
            }
            "--comptime-steps" => {
                let value = option_value(&mut it, inline, a, "a number of steps")?;
                let steps: u64 = value
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("`--comptime-steps` needs a positive number of steps, got `{value}`"))?;
                args.comptime_steps = Some(steps);
            }
            _ if a.starts_with('-') => return Err(format!("unknown option `{a}`")),
            _ if target.is_none() => target = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument `{a}`")),
        }
    }
    if args.format.is_some() && !args.print_instances && !args.explain_ownership {
        return Err("`--format` is only valid with `--print-instances` or `--explain-ownership`".to_string());
    }
    if args.function.is_some() && !args.explain_ownership {
        return Err("`--fn` needs `--explain-ownership`".to_string());
    }
    args.target = target.ok_or("missing target file")?;
    Ok(args)
}

pub(crate) fn run(argv: &[String]) -> ExitCode {
    let args = match parse(argv) {
        Ok(a) => a,
        Err(message) => return crate::cli::usage_error("check", &message),
    };
    let options = CompileOptions {
        release: args.release,
        debug: false,
        strip: false,
        lto: false,
        pgo_generate: false,
        pgo_use: None,
        emit_ir: None,
        emit: None,
        link_libs: &[],
        mode: if args.tests { EntryMode::Test } else { EntryMode::Build },
        diagnostics: &args.diagnostics,
        comptime_steps: args.comptime_steps,
        emit_deps: None,
        // A CI gate gets a log even when the package is clean, like `burdin build` (`spec/diagnostics.md`, DG11).
        report_clean: true,
        run_build_scripts: true,
        locked: args.locked,
        features: args.features.clone(),
        jobs: jobs_or_default(args.jobs),
        sanitize: Default::default(),
        fuzz_target: None,
    };
    let front = match front_end(&args.target, &options, &SourceOverrides::new()) {
        Ok(front) => front,
        Err(code) => return code,
    };
    if front.diags.has_errors() {
        report_errors("check", &args.target, &front.diags, &front.sources, &args.diagnostics);
        if args.explain {
            explain_memory(&front);
        }
        if args.explain_ownership {
            show_ownership(&front, &args);
        }
        return ExitCode::from(1);
    }
    print_warnings(&front.diags, &front.sources, &args.diagnostics, options.report_clean);
    if args.print_instances {
        let program = front.program.as_ref().expect("no errors means checking produced a program");
        let files = &front.files;
        let is_std = |file: burdin_diag::FileId| files.get(file.0 as usize).is_some_and(|(_, embedded)| *embedded);
        let report = burdin_inspect::instances::build(program, &front.sources, &is_std);
        let text = if args.format == Some(OwnershipFormat::Json) {
            burdin_inspect::instances::render_json(&report)
        } else {
            burdin_inspect::instances::render_human(&report)
        };
        print!("{text}");
    }
    if args.explain_ownership {
        show_ownership(&front, &args);
    }
    ExitCode::SUCCESS
}

// ---- ownership events (--explain-ownership) -------------------------------------------------------------

/// The functions one `--explain-ownership` run shows: the source functions of the entry package, in source order.
/// The embedded standard library, generic instantiations, and compiler-generated items are left out.
fn ownership_functions<'a>(
    front: &'a FrontEnd,
    filter: Option<&str>,
) -> Vec<(&'a Function, FunctionOwnership, String)> {
    let Some(program) = &front.program else { return Vec::new() };
    let mut out: Vec<(&Function, FunctionOwnership, String)> = Vec::new();
    let bodies =
        program.functions.iter().filter(|f| !f.symbolic).chain(program.closures.iter().map(|closure| &closure.func));
    for function in bodies {
        // Skip the compiler's own items: a generic instance, a comptime specialization, a test wrapper.
        let generated = function.name.rsplit('.').next().is_some_and(|name| name.starts_with("__"));
        if function.span.lo >= function.span.hi
            || function.name.contains('<')
            || function.name.contains(".comptime")
            || generated
        {
            continue;
        }
        let path = front.sources.get(function.span.file).path.clone();
        if path.starts_with("<std>") {
            continue;
        }
        if !filter.is_none_or(|want| name_matches(&function.name, want)) {
            continue;
        }
        let ownership = burdin_query::function_ownership(function, &path);
        out.push((function, ownership, path));
    }
    out.sort_by(|a, b| (a.2.as_str(), a.0.span.lo, a.0.span.hi).cmp(&(b.2.as_str(), b.0.span.lo, b.0.span.hi)));
    out
}

/// Whether `--fn` names `name`: the qualified name, the source name after the last `.`, or the declaration name
/// before any type arguments.
fn name_matches(name: &str, want: &str) -> bool {
    let base = name.split('<').next().unwrap_or(name);
    name == want || base == want || base.rsplit('.').next() == Some(want)
}

/// `place` as the annotation writes it: `a`, `p.n`, `h.*.n`.
fn place_text(local: &str, path: &[usize]) -> String {
    let mut out = local.to_string();
    for step in path {
        out.push('.');
        if *step == MOVED_DEREF {
            out.push('*');
        } else {
            out.push_str(&step.to_string());
        }
    }
    out
}

fn convention_text(convention: Convention) -> &'static str {
    match convention {
        Convention::Read => "read",
        Convention::Mut => "mut",
        Convention::Own => "own",
    }
}

/// The text of one event in the annotated source. `Declare` is left out: the `let` that binds the name already
/// says it, and it only matters to the step-by-step trace.
fn event_text(event: &OwnershipEventInfo) -> Option<String> {
    // A drop with no local is a temporary the statement leaves behind (DR5).
    let place = event.local.as_deref().map(|local| place_text(local, &event.path));
    match &event.kind {
        OwnershipEventKind::Declare => None,
        OwnershipEventKind::Move { target } => {
            let mut text = format!("move {}", place?);
            if let Some(target) = target {
                text.push_str(" -> ");
                text.push_str(target);
            }
            Some(text)
        }
        OwnershipEventKind::Copy => Some(format!("copy {}", place?)),
        OwnershipEventKind::BorrowRead => Some(format!("borrow (read) {}", place?)),
        OwnershipEventKind::BorrowMut => Some(format!("borrow (mut) {}", place?)),
        OwnershipEventKind::Drop { conditional } => Some(format!(
            "drop {}{}",
            place.unwrap_or_else(|| "temporary".to_string()),
            if *conditional { " (conditional)" } else { "" }
        )),
        OwnershipEventKind::DropFlag => Some(format!("drop flag {}", place?)),
        OwnershipEventKind::CallArg { parameter, convention } => {
            Some(format!("call arg `{parameter}`: {}", convention_text(*convention)))
        }
        OwnershipEventKind::Clone { derived } => {
            Some(format!("clone {}{}", place?, if *derived { " (derived)" } else { "" }))
        }
    }
}

fn show_ownership(front: &FrontEnd, args: &CheckArgs) {
    let functions = ownership_functions(front, args.function.as_deref());
    match args.format.unwrap_or_default() {
        OwnershipFormat::Text => show_ownership_text(&functions, &front.sources),
        OwnershipFormat::Json => show_ownership_json(&functions, &front.sources),
    }
}

fn show_ownership_text(functions: &[(&Function, FunctionOwnership, String)], sources: &SourceMap) {
    for (index, (function, ownership, path)) in functions.iter().enumerate() {
        if index > 0 {
            println!();
        }
        let file = sources.get(function.span.file);
        let start = file.line_col(function.span.lo).0;
        let end = file.line_col(function.span.hi.saturating_sub(1)).0.max(start);
        let label = if function.name.starts_with("closure.") { "closure" } else { ownership.function.as_str() };
        println!("fn {label} ({path}:{start}):");
        // One entry per line for the events inside the function, in source order.
        let mut notes: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        for event in &ownership.events {
            let Some(text) = event_text(event) else { continue };
            let line = file.line_col(event.at.lo).0;
            if (start..=end).contains(&line) {
                notes.entry(line).or_default().push(text);
            }
        }
        for line in start..=end {
            println!("{line:>4} | {}", file.line_text(line));
            if let Some(texts) = notes.get(&line) {
                for text in texts {
                    println!("     | {text}");
                }
            }
        }
    }
}

fn kind_name(kind: &OwnershipEventKind) -> &'static str {
    match kind {
        OwnershipEventKind::Declare => "declare",
        OwnershipEventKind::Move { .. } => "move",
        OwnershipEventKind::Copy => "copy",
        OwnershipEventKind::BorrowRead | OwnershipEventKind::BorrowMut => "borrow",
        OwnershipEventKind::Drop { .. } => "drop",
        OwnershipEventKind::DropFlag => "drop_flag",
        OwnershipEventKind::CallArg { .. } => "call_arg",
        OwnershipEventKind::Clone { .. } => "clone",
    }
}

fn event_json(event: &OwnershipEventInfo, sources: &SourceMap, file: burdin_diag::FileId) -> Json {
    let mut out = Json::object();
    out.set("kind", Json::str(kind_name(&event.kind)));
    out.set("span", burdin_diag::span_json(sources, Span::new(file, event.at.lo, event.at.hi)));
    if let Some(local) = &event.local {
        out.set("local", Json::str(local.as_str()));
    }
    out.set("path", Json::Arr(event.path.iter().map(|step| Json::Num(*step as u64)).collect()));
    match &event.kind {
        OwnershipEventKind::Move { target } => {
            if let Some(target) = target {
                out.set("target", Json::str(target.as_str()));
            }
        }
        OwnershipEventKind::BorrowRead => out.set("mutable", Json::Bool(false)),
        OwnershipEventKind::BorrowMut => out.set("mutable", Json::Bool(true)),
        OwnershipEventKind::Drop { conditional } => out.set("conditional", Json::Bool(*conditional)),
        OwnershipEventKind::CallArg { parameter, convention } => {
            out.set("parameter", Json::str(parameter.as_str()));
            out.set("convention", Json::str(convention_text(*convention)));
        }
        OwnershipEventKind::Clone { derived } => out.set("derived", Json::Bool(*derived)),
        OwnershipEventKind::Declare | OwnershipEventKind::Copy | OwnershipEventKind::DropFlag => {}
    }
    out
}

fn show_ownership_json(functions: &[(&Function, FunctionOwnership, String)], sources: &SourceMap) {
    let items: Vec<Json> = functions
        .iter()
        .map(|(function, ownership, _)| {
            let mut out = Json::object();
            out.set("function", Json::str(ownership.function.as_str()));
            out.set("span", burdin_diag::span_json(sources, function.span));
            out.set(
                "events",
                Json::Arr(
                    ownership.events.iter().map(|event| event_json(event, sources, function.span.file)).collect(),
                ),
            );
            out
        })
        .collect();
    println!("[");
    for (index, item) in items.iter().enumerate() {
        let mut text = String::new();
        item.write(&mut text);
        if index + 1 < items.len() {
            text.push(',');
        }
        println!("{text}");
    }
    println!("]");
}

// ---- step-by-step memory explanations (--explain) -------------------------------------------------------

/// The rule ID in `spec/memory-rules.md` for a memory error code. These are the codes a value's path explains;
/// a positional rule (`SC`) or a convention marker (`CV`) has no path of moves and borrows.
fn memory_rule(code: &str) -> Option<&'static str> {
    Some(match code {
        "E0100" => "MV2",
        "E0101" => "MV7",
        "E0103" => "MV5",
        "E0104" => "MV3",
        "E0110" => "EX1",
        "E0111" => "RF3",
        "E0121" => "RT3",
        "E0122" => "RT2",
        "E0123" => "RT5",
        _ => return None,
    })
}

/// The first backticked name in a diagnostic message (`use of moved value \`t\`` gives `t`).
fn message_local(message: &str) -> Option<&str> {
    let start = message.find('`')? + 1;
    let end = message[start..].find('`')? + start;
    Some(&message[start..end])
}

/// The innermost function or closure whose source contains `span`.
fn innermost(program: &Program, span: Span) -> Option<&Function> {
    let mut best: Option<&Function> = None;
    for function in program.functions.iter().filter(|f| !f.symbolic).chain(program.closures.iter().map(|c| &c.func)) {
        let s = function.span;
        if s.file == span.file
            && s.lo <= span.lo
            && span.lo < s.hi
            && best.is_none_or(|b| s.hi - s.lo < b.span.hi - b.span.lo)
        {
            best = Some(function);
        }
    }
    best
}

/// The local a move or borrow event at `span` concerns, for a message that does not name it.
fn event_local_at(function: &Function, span: Span) -> Option<String> {
    function
        .events
        .iter()
        .find(|event| {
            event.span == span
                && matches!(
                    event.kind,
                    burdin_tir::OwnershipEventKind::Move { .. }
                        | burdin_tir::OwnershipEventKind::BorrowRead
                        | burdin_tir::OwnershipEventKind::BorrowMut
                )
        })
        .and_then(|event| event.local)
        .and_then(|id| function.locals.get(id.0).map(|local| local.name.clone()))
}

/// One step of a value's path, for the trace: its creation, a move, a borrow, or a clone.
fn trace_step(function: &Function, event: &burdin_tir::OwnershipEvent) -> Option<String> {
    let local = event.local.and_then(|id| function.locals.get(id.0))?;
    let place = place_text(&local.name, &event.path);
    Some(match &event.kind {
        burdin_tir::OwnershipEventKind::Declare => format!("created `{place}`"),
        burdin_tir::OwnershipEventKind::Move { target } => match target {
            Some(target) => format!("moved `{place}` -> {target}"),
            None => format!("moved `{place}`"),
        },
        burdin_tir::OwnershipEventKind::Copy => format!("copied `{place}`"),
        burdin_tir::OwnershipEventKind::BorrowRead => format!("borrowed `{place}`"),
        burdin_tir::OwnershipEventKind::BorrowMut => format!("borrowed `{place}` exclusively"),
        burdin_tir::OwnershipEventKind::Clone { .. } => format!("cloned `{place}`"),
        _ => return None,
    })
}

fn explain_memory(front: &FrontEnd) {
    let Some(program) = &front.program else { return };
    let sources = &front.sources;
    for diagnostic in front.diags.iter() {
        let Some(rule) = memory_rule(diagnostic.code.0) else { continue };
        let file = sources.get(diagnostic.span.file);
        let (line, column) = file.line_col(diagnostic.span.lo);
        eprintln!();
        eprintln!(
            "trace for {}:{}:{}: {}[{}]: {}",
            file.path,
            line,
            column,
            diagnostic.severity.as_str(),
            diagnostic.code.0,
            diagnostic.message
        );
        eprintln!("  rule: spec/memory-rules.md {rule}");
        let function = innermost(program, diagnostic.span);
        let local = message_local(&diagnostic.message)
            .map(str::to_string)
            .or_else(|| function.and_then(|f| event_local_at(f, diagnostic.span)));
        let mut traced = Vec::new();
        if let (Some(function), Some(local)) = (function, local.as_deref()) {
            for event in &function.events {
                if event.span.lo > diagnostic.span.lo {
                    break;
                }
                // The move a failed `let` would have made is recorded after the error is reported at the same
                // span; it is what fails, not a step that precedes the failure.
                if event.span == diagnostic.span && diagnostic.code.0 == "E0100" {
                    continue;
                }
                let name = event.local.and_then(|id| function.locals.get(id.0)).map(|l| l.name.as_str());
                if name != Some(local) {
                    continue;
                }
                let Some(text) = trace_step(function, event) else { continue };
                let at = sources.get(event.span.file);
                let (l, c) = at.line_col(event.span.lo);
                eprintln!("  {text} at {}:{}:{}", at.path, l, c);
                traced.push(event.span);
            }
        }
        for note in &diagnostic.notes {
            if note.span.is_some_and(|span| traced.contains(&span)) {
                continue;
            }
            match note.span {
                Some(span) => {
                    let at = sources.get(span.file);
                    let (l, c) = at.line_col(span.lo);
                    eprintln!("  note at {}:{}:{}: {}", at.path, l, c, note.message);
                }
                None => eprintln!("  note: {}", note.message),
            }
        }
        eprintln!("  fails here: {}:{}:{}", file.path, line, column);
        if let Some(fix) = burdin_diag::explain_fix(diagnostic.code.0) {
            eprintln!("  fix: {fix}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn a_target_alone_checks_like_a_plain_build() {
        let args = parse(&argv(&["app"])).unwrap();
        assert_eq!(args.target, PathBuf::from("app"));
        assert!(!args.release && !args.tests);
        assert_eq!(args.comptime_steps, None);
        assert!(!args.explain_ownership && !args.explain);
        assert_eq!(args.format, None);
    }

    #[test]
    fn options_and_diagnostics_flags_are_read_in_any_order() {
        let args = parse(&argv(&["--tests", "app", "--release", "--comptime-steps=50", "--error-limit", "3"])).unwrap();
        assert!(args.release && args.tests);
        assert_eq!(args.comptime_steps, Some(50));
        assert_eq!(args.diagnostics.error_limit, Some(3));
    }

    #[test]
    fn ownership_and_trace_options_are_read() {
        let args = parse(&argv(&["app", "--explain-ownership", "--fn", "main", "--format=json"])).unwrap();
        assert!(args.explain_ownership);
        assert_eq!(args.function.as_deref(), Some("main"));
        assert_eq!(args.format, Some(OwnershipFormat::Json));
        assert!(parse(&argv(&["app", "--explain"])).unwrap().explain);
        assert_eq!(parse(&argv(&["app", "--explain-ownership"])).unwrap().format, None);
    }

    #[test]
    fn usage_errors() {
        assert!(parse(&argv(&[])).unwrap_err().contains("missing target"));
        assert!(parse(&argv(&["a", "b"])).unwrap_err().contains("unexpected argument `b`"));
        assert!(parse(&argv(&["a", "--bogus"])).unwrap_err().contains("unknown option `--bogus`"));
        assert!(parse(&argv(&["a", "--release=yes"])).unwrap_err().contains("takes no value"));
        assert!(parse(&argv(&["a", "--comptime-steps", "0"])).unwrap_err().contains("positive number"));
        assert!(parse(&argv(&["a", "--diagnostic-format", "xml"])).unwrap_err().contains("unknown diagnostic format"));
        assert!(
            parse(&argv(&["a", "--format", "xml", "--explain-ownership"]))
                .unwrap_err()
                .contains("`human`, `text`, or `json`")
        );
        assert!(parse(&argv(&["a", "--fn", "main"])).unwrap_err().contains("needs `--explain-ownership`"));
        assert!(parse(&argv(&["a", "--format", "json"])).unwrap_err().contains("--print-instances"));
        // `--format` serves `--print-instances` too: `human` is its default form.
        let instances = parse(&argv(&["a", "--print-instances", "--format", "human"])).unwrap();
        assert!(instances.print_instances && instances.format == Some(OwnershipFormat::Text));
        // Options of the other commands are not accepted here.
        assert!(parse(&argv(&["a", "--emit-ir", "x.ll"])).unwrap_err().contains("unknown option"));
        assert!(parse(&argv(&["a", "-o", "x"])).unwrap_err().contains("unknown option"));
    }
}
