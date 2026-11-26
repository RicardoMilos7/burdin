//! The monomorphization report (`spec/inspect.md`, INS12-INS15): every generic function and type instance the
//! checked program contains, how many instances each generic has, and why each one exists.
//!
//! Sema allocates an instance when a call or a type needs it, so the program holds only the instances something
//! asked for. The *reason* is derived from the checked program rather than recorded during checking: the call graph
//! of the typed tree is walked breadth-first from the code that is not an instance itself (every ordinary function,
//! the program's own code before the standard library's), and the first call that reaches an instance is the call
//! that required it. The chain of instance callers ends at the first function that is not generic. A breadth-first
//! walk gives the shortest chain, and ties go to the earlier function and the earlier call in its body, so the
//! answer is the same on every run.

use crate::reach::{Refs, roots};
use burdin_diag::{FileId, Json, SourceMap, Span};
use burdin_tir::{Alloc, Function, Program, Ty};
use std::collections::{HashMap, HashSet, VecDeque};

/// A place in a source file: the path, the 1-based line, and the 1-based column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

impl Site {
    pub fn of(sources: &SourceMap, span: Span) -> Site {
        let file = sources.get(span.file);
        let (line, column) = file.line_col(span.lo);
        Site { file: file.path.clone(), line, column }
    }

    fn text(&self) -> String {
        format!("{}:{}:{}", self.file, self.line, self.column)
    }

    fn line_text(&self) -> String {
        format!("{}:{}", self.file, self.line)
    }
}

/// One step of the chain that required an instance: the call, and the function the call is written in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub function: String,
    pub site: Site,
}

/// An instance of a generic function or method that code generation emits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnInstance {
    /// The instance's name as the program spells it: `largest<i32>`, `Stack<i32>.push`.
    pub name: String,
    /// Where the generic function is declared.
    pub declared: Site,
    /// Declared in the standard library.
    pub std: bool,
    /// The call that required the instance, then the call that required the function that call is in, and so on
    /// to the first function that is not generic. Empty when the instance is itself a root (a `Drop` implementation,
    /// a vtable entry).
    pub required_by: Vec<Step>,
}

/// An instance of a generic struct or enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeInstance {
    pub name: String,
    pub is_enum: bool,
    /// The first function (in the order above) whose signature or locals mention the type.
    pub first_use: Step,
}

/// All the instances of one generic, under its name without type arguments (`Stack.push`, `largest`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group<T> {
    pub generic: String,
    pub instances: Vec<T>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Report {
    /// Generic functions and methods, the generic with the most instances first.
    pub functions: Vec<Group<FnInstance>>,
    /// Generic structs and enums, the generic with the most instances first.
    pub types: Vec<Group<TypeInstance>>,
    /// Function instances created for code nothing reachable calls: the standard library's bodies are all checked,
    /// and the ones the program never reaches are not emitted. They are counted, not listed.
    pub unreachable_functions: usize,
    /// Type instances only unreachable code mentions.
    pub unreachable_types: usize,
}

impl Report {
    pub fn function_instances(&self) -> usize {
        self.functions.iter().map(|g| g.instances.len()).sum()
    }

    pub fn type_instances(&self) -> usize {
        self.types.iter().map(|g| g.instances.len()).sum()
    }
}

/// A name without its type arguments: `Stack<i32>.push<String>` is `Stack.push`.
pub fn generic_name(name: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in name.chars() {
        match c {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Whether a function or type name is an instance of a generic. The types of `std` that the language treats as its
/// own (`Option`, `Result`, `Shared`) are written in `std` but were compiler built-ins, so they are not reported as
/// generic instances: the reports keep to the generics a program or a library declares.
pub fn is_instance(name: &str) -> bool {
    name.contains('<') && !is_lang_type(name)
}

fn is_lang_type(name: &str) -> bool {
    [
        "std.Option<",
        "std.Result<",
        "std.Shared<",
        "std.SharedCell<",
        "std.List<",
        "std.ListIntoIter<",
        "std.option.ext.Option<",
        "std.option.ext.Result<",
    ]
    .iter()
    .any(|p| name.starts_with(p))
}

/// The ADT names a type mentions, in the order of their first appearance.
fn adts_in(ty: &Ty, out: &mut Vec<String>) {
    let alloc = |a: &Alloc, out: &mut Vec<String>| {
        if let Alloc::Value(t) = a {
            adts_in(t, out);
        }
    };
    match ty {
        Ty::Struct(a) | Ty::Enum(a) => out.push(a.name.to_string()),
        Ty::Fn { sig, .. } => {
            sig.params.iter().for_each(|p| adts_in(p, out));
            adts_in(&sig.ret, out);
        }
        Ty::Ref { inner, .. }
        | Ty::Slice(inner)
        | Ty::Array(inner, _)
        | Ty::CPtr(inner)
        | Ty::Range(inner, _)
        | Ty::IterMut(inner)
        | Ty::RangeIter(inner, _)
        | Ty::SliceChunks(inner, _)
        | Ty::SliceWindows(inner) => adts_in(inner, out),
        Ty::Heap(t, a) => {
            adts_in(t, out);
            alloc(a, out);
        }
        Ty::MapIter(a, b) | Ty::MapIterMut(a, b) => {
            adts_in(a, out);
            adts_in(b, out);
        }
        Ty::Map(k, v, a) | Ty::MapEntry(k, v, a) | Ty::MapIntoIter(k, v, a) => {
            adts_in(k, out);
            adts_in(v, out);
            alloc(a, out);
        }
        Ty::Any(a) => a.args.iter().for_each(|t| adts_in(t, out)),
        Ty::Tuple(items) => items.iter().for_each(|t| adts_in(t, out)),
        _ => {}
    }
}

/// The ADT names a function's signature and locals mention.
fn adts_of(f: &Function) -> Vec<String> {
    let mut out = Vec::new();
    adts_in(&f.ret, &mut out);
    for local in &f.locals {
        adts_in(&local.ty, &mut out);
    }
    out
}

/// Builds the report. `is_std` says whether a file belongs to the embedded standard library.
pub fn build(program: &Program, sources: &SourceMap, is_std: &dyn Fn(FileId) -> bool) -> Report {
    // The walk starts from what code generation starts from, the program's own code before the library's.
    let mut roots = roots(program);
    roots.sort_by_key(|&i| (is_std(program.functions[i].span.file), i));
    roots.dedup();

    let mut visited: HashSet<usize> = roots.iter().copied().collect();
    let mut order: Vec<usize> = roots.clone();
    let mut parent: HashMap<usize, (usize, Span)> = HashMap::new();
    let mut queue: VecDeque<usize> = roots.into_iter().collect();
    let mut refs = Refs::new(program);
    while let Some(i) = queue.pop_front() {
        refs.of_function(&program.functions[i]);
        for &(callee, span) in &refs.calls {
            if program.functions[callee.0].symbolic || !visited.insert(callee.0) {
                continue;
            }
            parent.insert(callee.0, (i, span));
            order.push(callee.0);
            queue.push_back(callee.0);
        }
    }

    let step = |function: usize, span: Span| Step {
        function: program.functions[function].name.clone(),
        site: Site::of(sources, span),
    };
    let mut report = Report::default();
    for (i, f) in program.functions.iter().enumerate() {
        if f.symbolic || !is_instance(&f.name) {
            continue;
        }
        if !visited.contains(&i) {
            report.unreachable_functions += 1;
            continue;
        }
        let mut required_by = Vec::new();
        let mut at = i;
        // The chain cannot loop: a parent was visited before its child. It ends at the first function that is not
        // an instance, because the generic callers are what explain an instance.
        while let Some(&(caller, span)) = parent.get(&at) {
            required_by.push(step(caller, span));
            if !is_instance(&program.functions[caller].name) {
                break;
            }
            at = caller;
        }
        // A generic that only the language's own `std` types call (`mem.drop` in `Shared`'s drop) is part of them.
        if required_by.last().is_some_and(|step| is_lang_type(&step.function)) {
            continue;
        }
        let instance = FnInstance {
            name: f.name.clone(),
            declared: Site::of(sources, f.span),
            std: is_std(f.span.file),
            required_by,
        };
        push_group(&mut report.functions, generic_name(&f.name), instance);
    }

    // The first function that mentions each instance of a type.
    let mut first_use: HashMap<String, Step> = HashMap::new();
    for &i in &order {
        let f = &program.functions[i];
        for name in adts_of(f) {
            if is_instance(&name) && !first_use.contains_key(&name) {
                first_use.insert(name, step(i, f.span));
            }
        }
    }
    let adts = program
        .structs
        .iter()
        .map(|s| (s.name.as_str(), false))
        .chain(program.enums.iter().map(|e| (e.name.as_str(), true)));
    for (name, is_enum) in adts {
        if !is_instance(name) {
            continue;
        }
        let Some(first_use) = first_use.get(name).cloned() else {
            report.unreachable_types += 1;
            continue;
        };
        push_group(&mut report.types, generic_name(name), TypeInstance { name: name.to_string(), is_enum, first_use });
    }

    // The generic with the most instances first; the sort is stable, so ties keep the program's order.
    report.functions.sort_by_key(|g| std::cmp::Reverse(g.instances.len()));
    report.types.sort_by_key(|g| std::cmp::Reverse(g.instances.len()));
    report
}

fn push_group<T>(groups: &mut Vec<Group<T>>, generic: String, instance: T) {
    match groups.iter_mut().find(|g| g.generic == generic) {
        Some(group) => group.instances.push(instance),
        None => groups.push(Group { generic, instances: vec![instance] }),
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The report as text: totals, then every generic with its instances and the chain that required each.
pub fn render_human(report: &Report) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} of {}; {} of {}\n",
        plural(report.function_instances(), "function instance", "function instances"),
        plural(report.functions.len(), "generic", "generics"),
        plural(report.type_instances(), "type instance", "type instances"),
        plural(report.types.len(), "generic", "generics"),
    ));
    if report.unreachable_functions + report.unreachable_types > 0 {
        out.push_str(&format!(
            "not listed: {} and {} that no emitted code reaches\n",
            plural(report.unreachable_functions, "function instance", "function instances"),
            plural(report.unreachable_types, "type instance", "type instances"),
        ));
    }
    for group in &report.functions {
        out.push_str(&format!("\nfn {}: {}\n", group.generic, plural(group.instances.len(), "instance", "instances")));
        for instance in &group.instances {
            let tag = if instance.std { " [std]" } else { "" };
            out.push_str(&format!("  {}  ({}){tag}\n", instance.name, instance.declared.line_text()));
            if instance.required_by.is_empty() {
                out.push_str("    a root: called by the runtime or through a vtable\n");
            }
            for (i, step) in instance.required_by.iter().enumerate() {
                let arrow = if i == 0 { "required at" } else { "which is required at" };
                out.push_str(&format!("    {arrow} {} in {}\n", step.site.text(), step.function));
            }
        }
    }
    for group in &report.types {
        out.push_str(&format!(
            "\ntype {}: {}\n",
            group.generic,
            plural(group.instances.len(), "instance", "instances")
        ));
        for instance in &group.instances {
            let kind = if instance.is_enum { "enum" } else { "struct" };
            out.push_str(&format!("  {}  ({kind})\n", instance.name));
            let step = &instance.first_use;
            out.push_str(&format!("    first used in {} ({})\n", step.function, step.site.line_text()));
        }
    }
    out
}

fn step_json(step: &Step) -> Json {
    Json::obj([("function", Json::str(&step.function)), ("at", Json::str(step.site.text()))])
}

fn count(n: usize) -> Json {
    Json::Num(n as u64)
}

/// The report as one JSON object on one line (`"version": 1`), with the same facts as the text.
pub fn render_json(report: &Report) -> String {
    let functions = report
        .functions
        .iter()
        .map(|g| {
            let instances = g
                .instances
                .iter()
                .map(|i| {
                    Json::obj([
                        ("name", Json::str(&i.name)),
                        ("declared", Json::str(i.declared.line_text())),
                        ("std", Json::Bool(i.std)),
                        ("required_by", Json::Arr(i.required_by.iter().map(step_json).collect())),
                    ])
                })
                .collect();
            Json::obj([
                ("generic", Json::str(&g.generic)),
                ("count", count(g.instances.len())),
                ("instances", Json::Arr(instances)),
            ])
        })
        .collect();
    let types = report
        .types
        .iter()
        .map(|g| {
            let instances = g
                .instances
                .iter()
                .map(|i| {
                    Json::obj([
                        ("name", Json::str(&i.name)),
                        ("kind", Json::str(if i.is_enum { "enum" } else { "struct" })),
                        ("first_use", step_json(&i.first_use)),
                    ])
                })
                .collect();
            Json::obj([
                ("generic", Json::str(&g.generic)),
                ("count", count(g.instances.len())),
                ("instances", Json::Arr(instances)),
            ])
        })
        .collect();
    let json = Json::obj([
        ("version", Json::Num(1)),
        ("function_instances", count(report.function_instances())),
        ("type_instances", count(report.type_instances())),
        ("unreachable_function_instances", count(report.unreachable_functions)),
        ("unreachable_type_instances", count(report.unreachable_types)),
        ("functions", Json::Arr(functions)),
        ("types", Json::Arr(types)),
    ]);
    let mut out = String::new();
    json.write(&mut out);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_names_drop_every_argument_list() {
        assert_eq!(generic_name("largest<i32>"), "largest");
        assert_eq!(generic_name("Stack<i32>.push"), "Stack.push");
        assert_eq!(generic_name("Stack<List<i32>>.map<String, (i32, bool)>"), "Stack.map");
        assert_eq!(generic_name("std.channel.Channel<i64>.send"), "std.channel.Channel.send");
        assert_eq!(generic_name("main"), "main");
    }

    #[test]
    fn instances_are_names_with_arguments() {
        assert!(is_instance("max_of<i32>"));
        assert!(is_instance("Pair<i32>.swap"));
        assert!(!is_instance("main.helper"));
    }

    #[test]
    fn groups_collect_instances_of_one_generic() {
        let mut groups = Vec::new();
        push_group(&mut groups, "f".to_string(), 1);
        push_group(&mut groups, "g".to_string(), 2);
        push_group(&mut groups, "f".to_string(), 3);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].instances, vec![1, 3]);
    }

    #[test]
    fn the_text_report_names_the_chain_of_callers() {
        let site = |line| Site { file: "src/main.bd".to_string(), line, column: 5 };
        let report = Report {
            functions: vec![Group {
                generic: "largest".to_string(),
                instances: vec![FnInstance {
                    name: "largest<i32>".to_string(),
                    declared: Site { file: "src/main.bd".to_string(), line: 3, column: 1 },
                    std: false,
                    required_by: vec![
                        Step { function: "twice<i32>".to_string(), site: site(8) },
                        Step { function: "main".to_string(), site: site(17) },
                    ],
                }],
            }],
            types: Vec::new(),
            unreachable_functions: 0,
            unreachable_types: 0,
        };
        let text = render_human(&report);
        assert!(text.starts_with("1 function instance of 1 generic; 0 type instances of 0 generics\n"), "{text}");
        assert!(text.contains("    required at src/main.bd:8:5 in twice<i32>\n"), "{text}");
        assert!(text.contains("    which is required at src/main.bd:17:5 in main\n"), "{text}");
        let json = render_json(&report);
        assert!(json.starts_with("{\"version\":1,\"function_instances\":1,"), "{json}");
        assert!(json.contains("\"required_by\":[{\"function\":\"twice<i32>\",\"at\":\"src/main.bd:8:5\"}"), "{json}");
    }
}
