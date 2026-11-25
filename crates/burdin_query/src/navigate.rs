//! Whole-program questions for a language server (`spec/lsp.md`): every name the checker resolved, every local,
//! every call with its arguments, every declared function, and every test, as plain data. A tool finds references,
//! inlay hints, and the call hierarchy from these without reading the typed tree (`spec/query.md`, QL7).

use crate::api::{DefinitionKind, Location, ParamInfo};
use crate::db::Analysis;
use burdin_diag::Span;
use burdin_tir::visit::{self, Visit};
use burdin_tir::{Expr, ExprKind, Function, LocalId, LocalKind, Program};

/// A name the checker resolved: a use of a local, a parameter, a function, or a constant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    /// The name the use refers to (`add`, `new`, `x`), without the module path.
    pub name: String,
    pub kind: DefinitionKind,
    /// The expression that names it. For a call this is the whole call, so a tool looks for the name inside it.
    pub at: Location,
    /// Where the name is declared: the binding of a local, the declaration of a function or constant. `None` for a
    /// declaration the compiler generated.
    pub definition: Option<Location>,
}

/// A local variable or parameter of a function or closure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalInfo {
    pub name: String,
    /// The binding: the name in the `let`, the pattern, or the parameter list.
    pub at: Location,
    pub ty: String,
    pub is_parameter: bool,
    pub mutable: bool,
}

/// What a `match` leaves out and what each of its arms covers, as patterns written in source form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchCases {
    /// The cases no unguarded arm matches: empty when the `match` is exhaustive. An open domain shows as `_`.
    pub missing: Vec<String>,
    /// For each arm, the cases that no unguarded arm before it matches: the values the arm is the first to see.
    pub before: Vec<Vec<String>>,
}

struct MatchFinder<'a> {
    analysis: &'a Analysis,
    path: &'a str,
    lo: u32,
    hi: u32,
    defs: burdin_tir::TypeDefs<'a>,
    found: Option<MatchCases>,
}

impl Visit for MatchFinder<'_> {
    fn expr(&mut self, e: &Expr) {
        if self.found.is_some() {
            return;
        }
        if let ExprKind::Match(scrutinee, arms) = &e.kind
            && e.span.lo == self.lo
            && e.span.hi == self.hi
            && self.analysis.files.get(e.span.file.0 as usize).is_some_and(|p| p == self.path)
        {
            let unguarded = |n: usize| -> Vec<&burdin_tir::Pat> {
                arms[..n].iter().filter(|a| a.guard.is_none()).map(|a| &a.pat).collect()
            };
            let cases = |n: usize| burdin_sema::missing_patterns(&scrutinee.ty, &unguarded(n), &self.defs);
            self.found = Some(MatchCases { missing: cases(arms.len()), before: (0..arms.len()).map(cases).collect() });
            return;
        }
        visit::walk_expr(self, e);
    }
}

/// A call of a function or method the program declares, with its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallSite {
    /// The function or test whose body contains the call; for a call inside a closure, the function the closure is
    /// written in.
    pub caller: String,
    /// The caller's declaration. `None` for code the compiler generated.
    pub caller_location: Option<Location>,
    /// The name of the callee as source writes it (`add`, `new`).
    pub callee: String,
    pub callee_location: Option<Location>,
    /// The whole call expression.
    pub at: Location,
    /// The arguments written at the call, in parameter order, each with its parameter. An argument the call left out
    /// (a default value) is not listed; a method's receiver is, as `self`.
    pub args: Vec<(ParamInfo, Location)>,
}

/// A function or method the program declares (one entry for a generic function, not one per instance).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionInfo {
    /// The name as source writes it.
    pub name: String,
    /// The qualified name the compiler uses (`geometry.shapes.Rect.new`).
    pub qualified: String,
    /// `fn add(a: i32, b: i32): i32`.
    pub display: String,
    /// The parameters, a method's receiver first (as `self`).
    pub params: Vec<ParamInfo>,
    pub ret: String,
    pub location: Location,
}

/// A `test` item of the entry package (only a test-mode analysis has them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestInfo {
    pub name: String,
    /// The dotted module path, empty for the package's entry file (`spec/test-runner.md`).
    pub module: String,
    /// The full path `burdin test --exact` takes inside the package: `<module>.<name>`, or `<name>`.
    pub path: String,
    /// The test's name string.
    pub location: Location,
    pub ignored: bool,
}

/// The ownership of a package file: which package and module it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleInfo {
    /// The path of the file, as [`Db::file_paths`] lists it.
    pub path: String,
    /// Index into [`Db::packages`].
    pub package: usize,
    /// The module path inside the package (`["geometry", "shapes"]`), empty for a one-file target.
    pub module: Vec<String>,
}

/// One package of a database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageInfo {
    pub name: String,
    /// The package directory; `None` for a one-file target and the embedded standard library.
    pub dir: Option<std::path::PathBuf>,
    /// `(name used by use, package index)`.
    pub deps: Vec<(String, usize)>,
    /// The file other packages reach as `use <package>`.
    pub root_file: Option<String>,
    /// The package being analyzed (its `test` items are checked in test mode).
    pub is_entry: bool,
    /// The embedded standard library: its files are not on disk.
    pub embedded: bool,
}

impl Analysis {
    fn loc(&self, span: Span) -> Location {
        Location { path: self.files.get(span.file.0 as usize).cloned().unwrap_or_default(), lo: span.lo, hi: span.hi }
    }

    fn real(&self, span: Span) -> Option<Location> {
        (span.lo < span.hi).then(|| self.loc(span))
    }

    /// Every resolved name in every function, method, test, and closure body, once per span and name. `at` is the span
    /// of the expression that resolves: the name itself for a path, the whole call for a method call
    /// (`s.area()`), and the whole literal for the calls an interpolated string makes; a tool looks for the name's token
    /// inside it.
    pub fn occurrences(&self) -> Vec<Occurrence> {
        let Some(program) = self.program.as_ref() else { return Vec::new() };
        let mut out: Vec<Occurrence> = Vec::new();
        for function in bodies(program) {
            let mut walker = Occurrences { analysis: self, program, func: function, out: &mut out };
            walker.block(&function.body);
        }
        out.sort_by(|a, b| (&a.at.path, a.at.lo, a.at.hi, &a.name).cmp(&(&b.at.path, b.at.lo, b.at.hi, &b.name)));
        out.dedup_by(|a, b| a.at == b.at && a.name == b.name);
        out
    }

    /// What the `match` whose whole expression spans `lo..hi` of `path` leaves out and what each arm covers, for the
    /// actions that write arms (`spec/lsp.md`, LS18). `None` when no checked `match` has that span.
    pub fn match_cases(&self, path: &str, lo: u32, hi: u32) -> Option<MatchCases> {
        let program = self.program.as_ref()?;
        let defs = burdin_tir::TypeDefs { structs: &program.structs, enums: &program.enums };
        for function in bodies(program) {
            let mut finder = MatchFinder { analysis: self, path, lo, hi, defs, found: None };
            finder.block(&function.body);
            if finder.found.is_some() {
                return finder.found;
            }
        }
        None
    }

    /// The locals and parameters whose binding is in `path`, in source order, once each.
    pub fn locals(&self, path: &str) -> Vec<LocalInfo> {
        let Some(program) = self.typed() else { return Vec::new() };
        let mut out: Vec<LocalInfo> = Vec::new();
        for function in bodies(program) {
            for local in &function.locals {
                if local.span.lo >= local.span.hi
                    || self.files.get(local.span.file.0 as usize).is_none_or(|p| p != path)
                {
                    continue;
                }
                out.push(LocalInfo {
                    name: local.name.clone(),
                    at: self.loc(local.span),
                    ty: crate::api::type_text(&local.ty),
                    is_parameter: matches!(local.kind, LocalKind::BorrowedParam | LocalKind::OwnParam),
                    mutable: local.mutable,
                });
            }
        }
        out.sort_by_key(|l| (l.at.lo, l.at.hi));
        // A generic function has one copy of each local per instance; its types differ, and the first one (the
        // symbolic declaration's, when it is listed first) stands for all of them.
        out.dedup_by(|a, b| a.at == b.at);
        out
    }

    /// Every call of a declared function or method in the program, in source order.
    pub fn calls(&self) -> Vec<CallSite> {
        let Some(program) = self.program.as_ref() else { return Vec::new() };
        let mut out: Vec<CallSite> = Vec::new();
        let functions: Vec<&Function> = program.functions.iter().collect();
        for function in bodies(program) {
            // A closure's calls belong to the function it is written in: the smallest declared function around it.
            let owner = if function.name.starts_with("burdin.closure.") {
                functions
                    .iter()
                    .filter(|f| {
                        f.span.file == function.span.file
                            && f.span.lo <= function.span.lo
                            && function.span.hi <= f.span.hi
                    })
                    .min_by_key(|f| f.span.hi - f.span.lo)
                    .copied()
            } else {
                Some(function)
            };
            let (caller, caller_location) = match owner {
                Some(f) => (display_name(program, f), self.real(f.span)),
                None => (function.name.clone(), None),
            };
            let mut walker = Calls { analysis: self, program, caller, caller_location, out: &mut out };
            walker.block(&function.body);
        }
        out.sort_by(|a, b| (&a.at.path, a.at.lo, a.at.hi).cmp(&(&b.at.path, b.at.lo, b.at.hi)));
        out.dedup_by(|a, b| a.at == b.at && a.callee_location == b.callee_location);
        out
    }

    /// Every function and method the program declares, once each (a generic function is listed by its declaration).
    pub fn functions(&self) -> Vec<FunctionInfo> {
        let Some(program) = self.typed() else { return Vec::new() };
        let mut out: Vec<(bool, FunctionInfo)> = Vec::new();
        for (index, function) in program.functions.iter().enumerate() {
            let Some(location) = self.real(function.span) else { continue };
            let Some(sig) = program.signatures.get(index) else { continue };
            let qualified = function.name.split('<').next().unwrap_or(&function.name).to_string();
            let info = FunctionInfo {
                name: crate::api::source_text(&qualified),
                qualified: qualified.clone(),
                display: crate::api::signature_display(&qualified, sig),
                params: crate::api::param_infos(sig),
                ret: crate::api::type_text(&sig.ret),
                location,
            };
            // The symbolic declaration of a generic function describes it as written; its instances share its span.
            out.push((!function.symbolic && function.name.contains('<'), info));
        }
        out.sort_by(|(a, x), (b, y)| (&x.location.path, x.location.lo, *a).cmp(&(&y.location.path, y.location.lo, *b)));
        out.dedup_by(|a, b| a.1.location == b.1.location);
        out.into_iter().map(|(_, info)| info).collect()
    }

    /// The `test` items of the entry package, in declaration order.
    pub fn tests(&self) -> Vec<TestInfo> {
        let Some(program) = self.program.as_ref() else { return Vec::new() };
        program
            .tests
            .iter()
            .map(|test| TestInfo {
                name: test.name.clone(),
                module: test.module.clone(),
                path: if test.module.is_empty() { test.name.clone() } else { format!("{}.{}", test.module, test.name) },
                location: self.loc(test.span),
                ignored: test.attrs.ignore,
            })
            .collect()
    }
}

/// Every body of the program: functions (the symbolic declarations of generic functions too, so a generic body that
/// is never instantiated still has its names) and closures.
fn bodies(program: &Program) -> impl Iterator<Item = &Function> {
    program.functions.iter().chain(program.closures.iter().map(|c| &c.func))
}

/// The name a caller is shown with: a test body by its test name.
fn display_name(program: &Program, function: &Function) -> String {
    for test in &program.tests {
        if program.functions.get(test.body.0).is_some_and(|f| std::ptr::eq(f, function))
            || program.functions.get(test.func.0).is_some_and(|f| std::ptr::eq(f, function))
        {
            return format!("test \"{}\"", test.name);
        }
    }
    crate::api::source_text(function.name.split('<').next().unwrap_or(&function.name))
}

struct Occurrences<'a> {
    analysis: &'a Analysis,
    program: &'a Program,
    func: &'a Function,
    out: &'a mut Vec<Occurrence>,
}

impl Visit for Occurrences<'_> {
    fn expr(&mut self, e: &Expr) {
        let found = match &e.kind {
            ExprKind::Local(LocalId(i)) => self.func.locals.get(*i).map(|local| {
                let kind = if matches!(local.kind, LocalKind::BorrowedParam | LocalKind::OwnParam) {
                    DefinitionKind::Parameter
                } else {
                    DefinitionKind::Local
                };
                (local.name.clone(), kind, self.analysis.real(local.span))
            }),
            ExprKind::Call(id, _) => self.program.functions.get(id.0).map(|f| {
                let name = crate::api::source_text(f.name.split('<').next().unwrap_or(&f.name));
                (name, DefinitionKind::Function, self.analysis.real(f.span))
            }),
            ExprKind::Const(id) => self
                .program
                .consts
                .get(id.0)
                .map(|c| (crate::api::source_text(&c.name), DefinitionKind::Constant, self.analysis.real(c.span))),
            _ => None,
        };
        if let Some((name, kind, definition)) = found
            && e.span.lo < e.span.hi
        {
            self.out.push(Occurrence { name, kind, at: self.analysis.loc(e.span), definition });
        }
        visit::walk_expr(self, e);
    }
}

struct Calls<'a> {
    analysis: &'a Analysis,
    program: &'a Program,
    caller: String,
    caller_location: Option<Location>,
    out: &'a mut Vec<CallSite>,
}

impl Visit for Calls<'_> {
    fn expr(&mut self, e: &Expr) {
        if let ExprKind::Call(id, args) = &e.kind
            && e.span.lo < e.span.hi
            && let (Some(f), Some(sig)) = (self.program.functions.get(id.0), self.program.signatures.get(id.0))
        {
            let params = crate::api::param_infos(sig);
            let written = args
                .iter()
                .zip(params)
                .filter(|(a, _)| a.span.file == e.span.file && e.span.lo <= a.span.lo && a.span.hi <= e.span.hi)
                .map(|(a, p)| (p, self.analysis.loc(a.span)))
                .collect();
            self.out.push(CallSite {
                caller: self.caller.clone(),
                caller_location: self.caller_location.clone(),
                callee: crate::api::source_text(f.name.split('<').next().unwrap_or(&f.name)),
                callee_location: self.analysis.real(f.span),
                at: self.analysis.loc(e.span),
                args: written,
            });
        }
        visit::walk_expr(self, e);
    }
}
