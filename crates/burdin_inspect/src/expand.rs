//! `burdin expand`: the program's source after the compiler has expanded it (`spec/inspect.md`, INS1-INS6).
//!
//! The expansion starts from the text of each source file and replaces only what the compiler rewrote, so everything
//! it did not touch (names, comments, layout) is the author's own text. A rewrite is found by asking the typed tree
//! what it holds at the span of a syntax node: the tree records the span of every expression it keeps, so a node
//! whose typed counterpart looks different has been expanded. The rewrites are:
//!
//! - an `if` with a compile-time condition becomes the branch it chose, and a `for` over `Type.fields` or
//!   `Type.variants` becomes one block per item, each with the item's own typed tree (INS2);
//! - a reflection member (`field.name`, `Self.name`) and `x.{name}` become the literal or the field they resolved
//!   to (INS2);
//! - an operator that resolved to an interface method becomes the method call (`a + b` is `a.add(b)`), and a call
//!   with named or omitted arguments lists every argument in parameter order (INS3);
//! - a constant's initializer becomes its evaluated value (INS2);
//! - `struct Type: Name` with a derive written in Burdin becomes the `Type: Name { ... }` impl block it generated,
//!   with the body expanded the same way (INS4). The built-in derives have no source form (a hand-written impl of a
//!   built-in interface is E9000), so `: Copy, Clone, ...` stays and a comment says so (INS5). An interface whose
//!   methods all have defaults stays in the list as written.
//!
//! Everything else is left as written. A rewrite that cannot be printed as source is skipped, never guessed, so
//! the output re-parses and type-checks wherever the input did (INS6).

use burdin_diag::{FileId, Span};
use burdin_syntax::ast;
use burdin_tir::visit::{Visit, walk_expr};
use burdin_tir::{ExprKind as T, Program, Ty};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};

/// One source file the program was built from, outside the standard library.
pub struct SourceFile<'a> {
    pub id: FileId,
    pub path: &'a str,
    pub text: &'a str,
    pub ast: &'a ast::File,
    /// The package the file belongs to and its module path inside it (empty for the package's entry file), for
    /// `--item`.
    pub package: &'a str,
    pub module: &'a str,
}

/// What to print.
#[derive(Default)]
pub struct Options {
    /// Only this item: a function, type, constant, or method, by name (`area`, `Circle.area`, `shapes.area`).
    pub item: Option<String>,
    /// After the files, each monomorphized instance of a generic function, expanded for its type arguments.
    pub instances: bool,
}

/// The expanded text of one source file (or, with `--item`, of one item).
pub struct Expanded {
    pub path: String,
    pub text: String,
}

/// An error that stops the expansion: an item that does not exist, or one that names several.
pub type ExpandError = String;

pub fn expand(program: &Program, files: &[SourceFile], options: &Options) -> Result<Vec<Expanded>, ExpandError> {
    let ex = Expander::new(program, files);
    let mut out = Vec::new();
    for file in files {
        let plan = ex.plan(file);
        out.push((file, plan));
    }
    if let Some(wanted) = &options.item {
        let mut found: Vec<(&SourceFile, &Unit, String)> = Vec::new();
        for (file, plan) in &out {
            for unit in &plan.units {
                if unit_matches(file, &unit.name, wanted) {
                    found.push((file, unit, plan.unit_text(file, unit)));
                }
            }
        }
        return match found.len() {
            0 => {
                let mut names: Vec<String> = Vec::new();
                for (file, plan) in &out {
                    for unit in &plan.units {
                        names.push(qualified(file, &unit.name));
                    }
                }
                names.sort();
                names.dedup();
                Err(format!("no item named `{wanted}`; the items are: {}", names.join(", ")))
            }
            1 => Ok(vec![Expanded { path: found[0].0.path.to_string(), text: found.remove(0).2 }]),
            _ => {
                let names: Vec<String> = found.iter().map(|(f, u, _)| qualified(f, &u.name)).collect();
                Err(format!("`{wanted}` names several items: {}", names.join(", ")))
            }
        };
    }
    let mut result: Vec<Expanded> = out
        .iter()
        .map(|(file, plan)| Expanded { path: file.path.to_string(), text: plan.apply(file.text, 0, file.text.len()) })
        .collect();
    if options.instances {
        let text = ex.instances_text(files);
        if !text.is_empty() {
            result.push(Expanded { path: "instances".to_string(), text });
        }
    }
    Ok(result)
}

fn qualified(file: &SourceFile, name: &str) -> String {
    if file.module.is_empty() { name.to_string() } else { format!("{}.{name}", file.module) }
}

fn unit_matches(file: &SourceFile, name: &str, wanted: &str) -> bool {
    let module = if file.module.is_empty() { String::new() } else { format!("{}.", file.module) };
    let package = if file.package.is_empty() { String::new() } else { format!("{}.", file.package) };
    wanted == name || wanted == format!("{module}{name}") || wanted == format!("{package}{module}{name}")
}

// ---------------------------------------------------------------------------------------------------------------
// Plans: the replacements of one file

struct Patch {
    lo: usize,
    hi: usize,
    text: String,
}

/// A named piece of a file that `--item` can print on its own.
struct Unit {
    name: String,
    lo: usize,
    hi: usize,
    /// The text of a generated item (an impl method a derive produced); a written item is its patched range.
    own_text: Option<String>,
}

#[derive(Default)]
struct Plan {
    patches: Vec<Patch>,
    units: Vec<Unit>,
}

impl Plan {
    /// `text[lo..hi]` with the patches inside it applied. Patches are non-overlapping and sorted by start; an
    /// insertion has `lo == hi`.
    fn apply(&self, text: &str, lo: usize, hi: usize) -> String {
        let mut out = String::new();
        let mut at = lo;
        for patch in &self.patches {
            if patch.lo < at || patch.hi > hi {
                continue;
            }
            out.push_str(&text[at..patch.lo]);
            out.push_str(&patch.text);
            at = patch.hi;
        }
        out.push_str(&text[at..hi]);
        out
    }

    fn unit_text(&self, file: &SourceFile, unit: &Unit) -> String {
        match &unit.own_text {
            Some(text) => text.clone(),
            None => self.apply(file.text, unit.lo, unit.hi),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The typed tree at a span

type Key = (u32, u32, u32);

fn key(span: Span) -> Key {
    (span.file.0, span.lo, span.hi)
}

/// The typed expressions of one function (and of the closures inside it), by span. Where several share a span (a
/// cast around an expression, the pieces a format string lowers to) the outermost wins.
#[derive(Clone, Default)]
struct Cx<'p> {
    map: HashMap<Key, &'p burdin_tir::Expr>,
}

struct Collect<'a, 'p> {
    program: &'p Program,
    map: &'a mut HashMap<Key, &'p burdin_tir::Expr>,
    /// Whether an existing entry is replaced: the typed tree of one unrolled `for` item overrides its siblings'.
    replace: bool,
    seen_closures: HashSet<usize>,
}

impl<'a, 'p> Visit for Collect<'a, 'p> {
    fn expr(&mut self, e: &burdin_tir::Expr) {
        // `e` borrows from the program, which outlives the collector; the trait hands out a shorter lifetime.
        let e: &'p burdin_tir::Expr = unsafe_lifetime(e);
        if self.replace {
            self.map.insert(key(e.span), e);
        } else {
            self.map.entry(key(e.span)).or_insert(e);
        }
        if let T::Closure { closure, .. } = &e.kind
            && self.seen_closures.insert(*closure)
        {
            let program = self.program;
            self.block(&program.closures[*closure].func.body);
        }
        walk_expr(self, e);
    }
}

/// Extends the lifetime of a reference into the program. The collector only ever visits nodes of the `Program` it
/// was built with, which the caller keeps alive for as long as the map exists.
fn unsafe_lifetime<'p>(e: &burdin_tir::Expr) -> &'p burdin_tir::Expr {
    // SAFETY: see above; no collector is created for a tree that is not part of the program.
    unsafe { &*(e as *const burdin_tir::Expr) }
}

impl<'p> Cx<'p> {
    fn of_block(program: &'p Program, block: &'p burdin_tir::Block) -> Cx<'p> {
        let mut cx = Cx::default();
        let mut c = Collect { program, map: &mut cx.map, replace: false, seen_closures: HashSet::new() };
        c.block(block);
        cx
    }

    /// This context with the expressions of `e` added over it: what one item of an unrolled `for` holds.
    fn with(&self, program: &'p Program, e: &'p burdin_tir::Expr) -> Cx<'p> {
        let mut cx = self.clone();
        let mut c = Collect { program, map: &mut cx.map, replace: true, seen_closures: HashSet::new() };
        c.expr(e);
        cx
    }

    fn at(&self, span: Span) -> Option<&'p burdin_tir::Expr> {
        self.map.get(&key(span)).copied()
    }
}

/// The spans of every expression under `e`, including `e`.
fn spans_under(e: &burdin_tir::Expr) -> Vec<Span> {
    struct Spans(Vec<Span>);
    impl Visit for Spans {
        fn expr(&mut self, e: &burdin_tir::Expr) {
            self.0.push(e.span);
            walk_expr(self, e);
        }
    }
    let mut s = Spans(Vec::new());
    s.expr(e);
    s.0
}

fn within(inner: Span, outer: Span) -> bool {
    inner.file == outer.file && inner.lo >= outer.lo && inner.hi <= outer.hi
}

// ---------------------------------------------------------------------------------------------------------------
// The expander

struct Expander<'a> {
    program: &'a Program,
    files: HashMap<u32, &'a SourceFile<'a>>,
    /// The typed functions by the position of their declaration: a generic function has one per instance and a
    /// symbolic one.
    functions: HashMap<(u32, u32), Vec<usize>>,
    /// The `derive` declarations of the program, by name.
    derives: HashMap<String, (&'a ast::DeriveDecl, FileId)>,
    /// Nesting depth of interpolation holes being rendered: a string literal cannot be printed inside one.
    in_hole: Cell<u32>,
}

impl<'a> Expander<'a> {
    fn new(program: &'a Program, files: &'a [SourceFile<'a>]) -> Expander<'a> {
        let mut functions: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (i, f) in program.functions.iter().enumerate() {
            functions.entry((f.span.file.0, f.span.lo)).or_default().push(i);
        }
        let mut derives = HashMap::new();
        for file in files {
            for item in &file.ast.items {
                if let ast::Item::Derive(d) = item {
                    derives.insert(d.name.name.clone(), (d, file.id));
                }
            }
        }
        Expander {
            program,
            files: files.iter().map(|f| (f.id.0, f)).collect(),
            functions,
            derives,
            in_hole: Cell::new(0),
        }
    }

    fn slice(&self, span: Span) -> Option<&'a str> {
        let file = self.files.get(&span.file.0)?;
        file.text.get(span.lo as usize..span.hi as usize)
    }

    // ----- the plan of one file ------------------------------------------------------------------------------

    fn plan(&self, file: &SourceFile<'a>) -> Plan {
        let mut plan = Plan::default();
        // Items the compiler generated (a derived `Debug`) carry the span of the derive clause; they are not text
        // the author wrote.
        let mut generated: HashSet<Key> = HashSet::new();
        for item in &file.ast.items {
            let derives = match item {
                ast::Item::Struct(s) => &s.derives,
                ast::Item::Enum(e) => &e.derives,
                _ => continue,
            };
            for t in derives {
                generated.insert(key(t.span));
            }
        }
        let mut unexpanded_derives: HashSet<String> = HashSet::new();
        for item in &file.ast.items {
            match item {
                ast::Item::Fn(f) if !generated.contains(&key(f.span)) => {
                    self.function_item(file, f, f.name.name.clone(), &mut plan);
                }
                ast::Item::Extension(x) => {
                    let recv = type_head(&x.recv);
                    self.function_item(file, &x.func, format!("{recv}.{}", x.func.name.name), &mut plan);
                }
                ast::Item::Impl(block) if !generated.contains(&key(block.span)) => {
                    let ty = type_head(&block.ty);
                    for m in &block.methods {
                        self.function_item(file, m, format!("{ty}.{}", m.name.name), &mut plan);
                    }
                }
                ast::Item::Interface(i) => {
                    for m in i.methods.iter().filter(|m| m.has_body) {
                        self.function_item(file, &m.decl, format!("{}.{}", i.name.name, m.decl.name.name), &mut plan);
                    }
                    plan.units.push(unit(&i.name.name, i.span));
                }
                ast::Item::Const(c) => self.const_item(file, c, &mut plan),
                ast::Item::Struct(s) => {
                    plan.units.push(unit(&s.name.name, s.span));
                    self.derive_item(
                        file,
                        &s.name.name,
                        !s.generics.is_empty(),
                        &s.derives,
                        s.span,
                        &mut plan,
                        &mut unexpanded_derives,
                    );
                }
                ast::Item::Enum(e) => {
                    plan.units.push(unit(&e.name.name, e.span));
                    self.derive_item(
                        file,
                        &e.name.name,
                        !e.generics.is_empty(),
                        &e.derives,
                        e.span,
                        &mut plan,
                        &mut unexpanded_derives,
                    );
                }
                _ => {}
            }
        }
        // A `derive` declaration is consumed when every type that names it got its impl block.
        for item in &file.ast.items {
            if let ast::Item::Derive(d) = item
                && !unexpanded_derives.contains(&d.name.name)
                && self.derive_used(&d.name.name)
            {
                plan.patches.push(Patch {
                    lo: d.span.lo as usize,
                    hi: d.span.hi as usize,
                    text: format!(
                        "// derive {}: expanded into the impl block of each type that derives it",
                        d.name.name
                    ),
                });
            }
        }
        plan.patches.sort_by_key(|p| (p.lo, p.hi));
        // Drop a patch that overlaps the one before it; the earlier (outer) one wins.
        let mut kept: Vec<Patch> = Vec::new();
        for p in plan.patches.drain(..) {
            if kept.last().is_some_and(|k| p.lo < k.hi) {
                continue;
            }
            kept.push(p);
        }
        plan.patches = kept;
        plan
    }

    /// Whether any type in the program names the derive.
    fn derive_used(&self, name: &str) -> bool {
        self.files.values().any(|f| {
            f.ast.items.iter().any(|item| match item {
                ast::Item::Struct(s) => s.derives.iter().any(|t| type_head(t) == name),
                ast::Item::Enum(e) => e.derives.iter().any(|t| type_head(t) == name),
                _ => false,
            })
        })
    }

    fn function_item(&self, file: &SourceFile<'a>, f: &ast::FnDecl, name: String, plan: &mut Plan) {
        plan.units.push(unit(&name, f.span));
        let Some(func) = self.single_function(f.span) else { return };
        let cx = Cx::of_block(self.program, &func.body);
        let text = self.block_text(&f.body, &cx);
        plan.patches.push(Patch { lo: f.body.span.lo as usize, hi: f.body.span.hi as usize, text });
        let _ = file;
    }

    /// The one typed function declared at `span`, when it is not generic.
    fn single_function(&self, span: Span) -> Option<&'a burdin_tir::Function> {
        let candidates = self.functions.get(&(span.file.0, span.lo))?;
        let [only] = candidates.as_slice() else { return None };
        let f = &self.program.functions[*only];
        (!f.symbolic).then_some(f)
    }

    fn const_item(&self, file: &SourceFile<'a>, c: &ast::ConstDecl, plan: &mut Plan) {
        plan.units.push(unit(&c.name.name, c.span));
        if matches!(c.value.kind, ast::ExprKind::Int(_) | ast::ExprKind::Str(_) | ast::ExprKind::Bool(_)) {
            return;
        }
        let value = self.program.consts.iter().find(|k| {
            k.span.file == file.id && within(k.span, c.span) && k.name.rsplit('.').next() == Some(c.name.name.as_str())
        });
        let Some(value) = value else { return };
        if let Some(text) = self.value_in(&value.value, true) {
            plan.patches.push(Patch { lo: c.value.span.lo as usize, hi: c.value.span.hi as usize, text });
        }
    }

    // ----- derives -------------------------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn derive_item(
        &self,
        file: &SourceFile<'a>,
        type_name: &str,
        generic: bool,
        derives: &[ast::Type],
        span: Span,
        plan: &mut Plan,
        unexpanded: &mut HashSet<String>,
    ) {
        if derives.is_empty() {
            return;
        }
        let mut builtin: Vec<String> = Vec::new();
        let mut rest: Vec<String> = Vec::new();
        let mut impls = String::new();
        let mut removed_any = false;
        for d in derives {
            let name = type_head(d);
            let user = self.derives.get(name.as_str());
            let text = self.slice(d.span).unwrap_or(&name).to_string();
            let Some((decl, decl_file)) = user else {
                if BUILTIN_DERIVES.contains(&name.as_str()) {
                    builtin.push(text.clone());
                }
                rest.push(text);
                continue;
            };
            if generic {
                unexpanded.insert(name.clone());
                rest.push(text);
                continue;
            }
            match self.derive_impl(type_name, decl, *decl_file, plan) {
                Some(block) => {
                    impls.push_str(&block);
                    removed_any = true;
                }
                None => {
                    unexpanded.insert(name.clone());
                    rest.push(text);
                }
            }
        }
        if removed_any {
            let first = derives[0].span;
            let last = derives[derives.len() - 1].span;
            if let Some((lo, hi)) = clause_range(file.text, first.lo as usize, last.hi as usize) {
                let text = if rest.is_empty() { String::new() } else { format!(": {}", rest.join(", ")) };
                plan.patches.push(Patch { lo, hi, text });
            }
        }
        let mut tail = String::new();
        if !builtin.is_empty() {
            tail.push_str(&format!(
                "\n// {}: built in; the compiler implements them structurally, so there is no source form",
                builtin.join(", ")
            ));
        }
        tail.push_str(&impls);
        if !tail.is_empty() {
            let at = span.hi as usize;
            plan.patches.push(Patch { lo: at, hi: at, text: tail });
        }
    }

    /// `Type: Name { ... }`, with every method of the derive expanded for this type, or `None` when a method has no
    /// typed counterpart for the type.
    fn derive_impl(&self, type_name: &str, decl: &ast::DeriveDecl, _file: FileId, plan: &mut Plan) -> Option<String> {
        let mut methods = String::new();
        let mut units: Vec<Unit> = Vec::new();
        for m in &decl.methods {
            let candidates = self.functions.get(&(m.span.file.0, m.span.lo))?;
            let func = candidates.iter().map(|i| &self.program.functions[*i]).find(|f| {
                !f.symbolic
                    && f.name
                        .rsplit_once('.')
                        .is_some_and(|(ty, method)| method == m.name.name && ty.rsplit('.').next() == Some(type_name))
            })?;
            let cx = Cx::of_block(self.program, &func.body);
            let header = self.slice(Span::new(m.span.file, m.span.lo, m.body.span.lo))?;
            let body = self.block_text(&m.body, &cx);
            let text = format!("{header}{body}");
            units.push(Unit {
                name: format!("{type_name}.{}", m.name.name),
                lo: 0,
                hi: 0,
                own_text: Some(text.clone()),
            });
            if !methods.is_empty() {
                methods.push_str("\n\n");
            }
            methods.push_str(&indent_block(&text, "    "));
        }
        plan.units.extend(units);
        Some(format!("\n\n{type_name}: {} {{\n{methods}\n}}", decl.name.name))
    }

    // ----- expressions ---------------------------------------------------------------------------------------

    /// The text of a block: its source with every expression inside it expanded.
    fn block_text(&self, b: &ast::Block, cx: &Cx<'a>) -> String {
        let mut kids: Vec<&ast::Expr> = Vec::new();
        block_children(b, &mut kids);
        self.with_children(b.span, kids, cx)
    }

    /// The source of `span` with the direct sub-expressions `kids` replaced by their expansions.
    fn with_children(&self, span: Span, mut kids: Vec<&ast::Expr>, cx: &Cx<'a>) -> String {
        let Some(file) = self.files.get(&span.file.0) else { return String::new() };
        kids.sort_by_key(|k| k.span.lo);
        let mut out = String::new();
        let mut at = span.lo as usize;
        for kid in kids {
            let (lo, hi) = (kid.span.lo as usize, kid.span.hi as usize);
            if lo < at || hi > span.hi as usize || kid.span.file != span.file {
                continue;
            }
            out.push_str(&file.text[at..lo]);
            out.push_str(&self.expr_text(kid, cx));
            at = hi;
        }
        out.push_str(&file.text[at..span.hi as usize]);
        out
    }

    fn default_text(&self, e: &ast::Expr, cx: &Cx<'a>) -> String {
        let mut kids = Vec::new();
        children(e, &mut kids);
        if matches!(e.kind, ast::ExprKind::Interp(_)) {
            self.in_hole.set(self.in_hole.get() + 1);
            let text = self.with_children(e.span, kids, cx);
            self.in_hole.set(self.in_hole.get() - 1);
            return text;
        }
        self.with_children(e.span, kids, cx)
    }

    fn expr_text(&self, e: &ast::Expr, cx: &Cx<'a>) -> String {
        use ast::ExprKind as K;
        let typed = cx.at(e.span);
        let replaced = match &e.kind {
            K::If { then_block, else_expr, .. } => self.const_if_text(then_block, else_expr.as_deref(), typed, cx),
            K::For { pattern, iter, body, .. } if is_reflection_iterable(iter) => {
                typed.and_then(|t| self.reflection_for_text(e, pattern, body, t, cx))
            }
            K::Binary(..) | K::Unary(..) | K::Index(..) => self.operator_text(e, typed, cx),
            K::Call(callee, args) => self.call_text(e, callee, args, typed, cx),
            K::DynField(recv, _) => self.dyn_field_text(e, recv, typed, cx),
            K::Field(..) => typed.and_then(|t| self.reflected_text(t)),
            K::Assign(lhs, rhs) | K::CompoundAssign(_, lhs, rhs) => return self.assign_text(e, lhs, rhs, cx),
            _ => None,
        };
        replaced.unwrap_or_else(|| self.default_text(e, cx))
    }

    /// `place = value`: the place is written as it is (an index on the left is a write, not a call of `index`).
    fn assign_text(&self, e: &ast::Expr, lhs: &ast::Expr, rhs: &ast::Expr, cx: &Cx<'a>) -> String {
        let mut kids: Vec<&ast::Expr> = Vec::new();
        if matches!(lhs.kind, ast::ExprKind::Index(..)) {
            let mut inner = Vec::new();
            children(lhs, &mut inner);
            let lhs_text = self.with_children(lhs.span, inner, cx);
            let rhs_text = self.expr_text(rhs, cx);
            let Some(file) = self.files.get(&e.span.file.0) else { return String::new() };
            let between = &file.text[lhs.span.hi as usize..rhs.span.lo as usize];
            return format!("{lhs_text}{between}{rhs_text}");
        }
        children(e, &mut kids);
        self.with_children(e.span, kids, cx)
    }

    /// An `if` whose condition was a compile-time condition (`spec/const-eval.md`, CE4): the typed tree holds only
    /// the branch the condition chose, so the text is that branch. An ordinary `if` is still an `if` in the typed
    /// tree, and is left as written.
    fn const_if_text(
        &self,
        then_block: &ast::Block,
        else_expr: Option<&ast::Expr>,
        typed: Option<&'a burdin_tir::Expr>,
        cx: &Cx<'a>,
    ) -> Option<String> {
        // The chosen branch keeps its own span when it is the `else` branch, so the node of the whole `if` is not
        // always there; then the branch whose block the tree holds is the chosen one.
        let Some(typed) = typed else {
            if cx.at(then_block.span).is_some() {
                return Some(self.block_text(then_block, cx));
            }
            let other = else_expr?;
            cx.at(other.span)?;
            return Some(self.expr_text(other, cx));
        };
        if !matches!(typed.kind, T::Block(_) | T::Unit) {
            return None;
        }
        let spans = spans_under(typed);
        if spans.iter().any(|s| within(*s, then_block.span) && s.hi > s.lo) {
            return Some(self.block_text(then_block, cx));
        }
        if let Some(other) = else_expr
            && spans.iter().any(|s| within(*s, other.span) && s.hi > s.lo)
        {
            return Some(self.expr_text(other, cx));
        }
        Some("{}".to_string())
    }

    /// Literal text where it can stand: inside an interpolation hole a number would read as a positional index, so
    /// it is parenthesized, and a string literal cannot be written there at all.
    fn hole_safe(&self, text: String) -> Option<String> {
        if self.in_hole.get() == 0 {
            return Some(text);
        }
        if text.contains('"') {
            return None;
        }
        Some(format!("({text})"))
    }

    /// One block per item of the loop, each with the typed tree of that item.
    fn reflection_for_text(
        &self,
        e: &ast::Expr,
        pattern: &ast::Pattern,
        body: &ast::Block,
        typed: &'a burdin_tir::Expr,
        cx: &Cx<'a>,
    ) -> Option<String> {
        let mut items: Vec<&'a burdin_tir::Expr> = Vec::new();
        match &typed.kind {
            T::Block(b) => {
                for s in &b.stmts {
                    if let burdin_tir::Stmt::Expr(item) = s {
                        items.push(item);
                    } else {
                        return None;
                    }
                }
                if let Some(tail) = &b.tail {
                    items.push(tail);
                }
            }
            _ => return None,
        }
        let indent = self.line_indent(e.span);
        let mut out = String::new();
        for (n, item) in items.iter().enumerate() {
            let icx = cx.with(self.program, item);
            let binding = self.iteration_binding(pattern, item);
            if n > 0 {
                out.push('\n');
                out.push_str(&indent);
            }
            let body_text = self.block_text(body, &icx);
            match binding {
                Some(Some(let_text)) => {
                    out.push_str(&format!("{{\n{indent}    {let_text}\n{indent}    {body_text}\n{indent}}}"));
                }
                Some(None) => out.push_str(&body_text),
                None => return None,
            }
        }
        if items.is_empty() {
            out.push_str("{}");
        }
        Some(out)
    }

    /// The `let` that binds the loop's pattern in one item: `Some(Some(text))`; `Some(None)` when the item has no
    /// such binding (the body folded every use of it); `None` when the binding cannot be printed.
    fn iteration_binding(&self, pattern: &ast::Pattern, item: &'a burdin_tir::Expr) -> Option<Option<String>> {
        let T::Block(b) = &item.kind else { return Some(None) };
        let Some(burdin_tir::Stmt::Let(pat, init, None)) = b.stmts.first() else { return Some(None) };
        let mut names = Vec::new();
        pattern_names(pattern, &mut names);
        if names.is_empty() {
            return Some(None);
        }
        let mut bound = Vec::new();
        pat_locals(pat, &mut bound);
        // The first `let` is the pattern's when it binds as many names as the pattern has.
        if bound.len() != names.len() {
            return Some(None);
        }
        let text = self.value_text(init)?;
        let source = self.slice(pattern.span)?;
        Some(Some(format!("let {source} = {text}")))
    }

    fn line_indent(&self, span: Span) -> String {
        let Some(file) = self.files.get(&span.file.0) else { return String::new() };
        let start = file.text[..span.lo as usize].rfind('\n').map_or(0, |i| i + 1);
        file.text[start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
    }

    /// `a + b` as `a.add(b)` when the operator resolved to an interface method.
    fn operator_text(&self, e: &ast::Expr, typed: Option<&'a burdin_tir::Expr>, cx: &Cx<'a>) -> Option<String> {
        use ast::ExprKind as K;
        let call = peel_to_call(typed?)?;
        let T::Call(fid, _) = &call.kind else { return None };
        let method = self.program.functions[fid.0].name.rsplit('.').next()?.to_string();
        if !self.program.signatures[fid.0].has_self {
            return None;
        }
        match &e.kind {
            K::Binary(_, l, r) => Some(format!("{}.{method}({})", self.receiver_text(l, cx), self.expr_text(r, cx))),
            K::Unary(ast::UnOp::Neg | ast::UnOp::Not, x) => Some(format!("{}.{method}()", self.receiver_text(x, cx))),
            // `index` returns a reference to the element; the operator reads through it.
            K::Index(base, idx) if method == "index" && matches!(typed?.kind, T::DerefRef(_)) => {
                Some(format!("(*{}.index({}))", self.receiver_text(base, cx), self.expr_text(idx, cx)))
            }
            _ => None,
        }
    }

    fn receiver_text(&self, e: &ast::Expr, cx: &Cx<'a>) -> String {
        use ast::ExprKind as K;
        let text = self.expr_text(e, cx);
        let atomic = matches!(
            e.kind,
            K::Name { .. }
                | K::Target(_)
                | K::SelfValue
                | K::Field(..)
                | K::TupleField(..)
                | K::Call(..)
                | K::Index(..)
                | K::Tuple(_)
                | K::Array(_)
                | K::Str(_)
                | K::Interp(_)
                | K::Bool(_)
        );
        if atomic { text } else { format!("({text})") }
    }

    /// A call with named or omitted arguments, written with every argument in parameter order.
    fn call_text(
        &self,
        e: &ast::Expr,
        callee: &ast::Expr,
        args: &[ast::Arg],
        typed: Option<&'a burdin_tir::Expr>,
        cx: &Cx<'a>,
    ) -> Option<String> {
        let call = peel_to_call(typed?)?;
        let T::Call(_, targs) = &call.kind else { return None };
        // A method call's receiver is the first argument of the typed call.
        let skip = match &callee.kind {
            ast::ExprKind::Field(recv, _) if targs.first().is_some_and(|t| within(t.span, recv.span)) => 1,
            _ => 0,
        };
        let named = args.iter().any(|a| a.name.is_some());
        let explicit = targs.len().checked_sub(skip)?;
        if !named && explicit == args.len() {
            return None;
        }
        // Every written argument must be one of the typed call's, or the call was rewritten into something else
        // and is left as written.
        let consumed =
            |a: &ast::Arg| targs[skip..].iter().any(|t| within(t.span, a.expr.span) && t.span.file == a.expr.span.file);
        if !args.iter().all(consumed) {
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        for targ in &targs[skip..] {
            let ast_arg = args.iter().find(|a| within(targ.span, a.expr.span) && a.expr.span.file == targ.span.file);
            match ast_arg {
                Some(a) => {
                    let mark = if a.mut_marker { "mut " } else { "" };
                    parts.push(format!("{mark}{}", self.expr_text(&a.expr, cx)));
                }
                // An omitted argument is the parameter's default. It is written as its value, because the
                // default's own text may name things that are not in scope at the call.
                None => parts.push(self.value_text(targ)?),
            }
        }
        let _ = e;
        Some(format!("{}({})", self.expr_text(callee, cx), parts.join(", ")))
    }

    /// `x.{name}` as `x.field`.
    fn dyn_field_text(
        &self,
        e: &ast::Expr,
        recv: &ast::Expr,
        typed: Option<&'a burdin_tir::Expr>,
        cx: &Cx<'a>,
    ) -> Option<String> {
        let _ = e;
        let T::Field(inner, index) = &peel_wrappers(typed?).kind else { return None };
        let mut ty = &inner.ty;
        while let Ty::Ref { inner, .. } = ty {
            ty = inner;
        }
        let Ty::Struct(adt) = ty else { return None };
        let field = self.program.structs.get(adt.id)?.fields.get(*index)?;
        Some(format!("{}.{}", self.receiver_text(recv, cx), field.name))
    }

    /// A reflection member (`field.name`, `Self.name`, `variant.arity`) that evaluated to a literal.
    fn reflected_text(&self, typed: &burdin_tir::Expr) -> Option<String> {
        let typed = peel_wrappers(typed);
        let literal = matches!(typed.kind, T::Int(_) | T::Bool(_) | T::Str(_) | T::Char(_))
            || matches!(&typed.kind, T::Builtin(burdin_tir::Builtin::StringFrom, a) if matches!(a[..], [burdin_tir::Expr { kind: T::Str(_), .. }]));
        if !literal {
            return None;
        }
        self.hole_safe(self.value_text(typed)?)
    }

    // ----- values --------------------------------------------------------------------------------------------

    /// A compile-time value as source text, or `None` when it has no literal form.
    fn value_text(&self, e: &burdin_tir::Expr) -> Option<String> {
        self.value_in(e, false)
    }

    /// `typed` says that the place the text goes in fixes the type of an integer or float literal (a constant's
    /// declared type, a field), so a literal of another type than `i32` needs no cast and may be out of `i32`'s
    /// range. Elsewhere a literal is written as `(5 as usize)`, which only works for a value that fits `i32`.
    fn value_in(&self, e: &burdin_tir::Expr, typed: bool) -> Option<String> {
        let p = self.program;
        Some(match &e.kind {
            T::Unit => "()".to_string(),
            T::Bool(b) => b.to_string(),
            T::Int(v) => match &e.ty {
                Ty::Int(burdin_tir::IntTy::I32) if *v >= 0 => v.to_string(),
                Ty::Int(_) if typed => v.to_string(),
                Ty::Int(_) if *v >= i32::MIN as i128 && *v <= i32::MAX as i128 => format!("({v} as {})", e.ty),
                _ => return None,
            },
            T::Float(f) => {
                if !f.is_finite() {
                    return None;
                }
                let s = format!("{f:?}");
                if s.contains('e') || s.contains('E') {
                    return None;
                }
                match &e.ty {
                    Ty::Float(burdin_tir::FloatTy::F64) if *f >= 0.0 => s,
                    Ty::Float(_) if typed => s,
                    Ty::Float(_) => format!("({s} as {})", e.ty),
                    _ => return None,
                }
            }
            T::Char(c) => format!("'{}'", escape(&c.to_string(), '\'')),
            T::Str(s) => format!("\"{}\"", escape(s, '"')),
            T::StringAsStr(inner) => match &inner.kind {
                T::Builtin(burdin_tir::Builtin::StringFrom, a)
                    if matches!(a[..], [burdin_tir::Expr { kind: T::Str(_), .. }]) =>
                {
                    self.value_text(&a[0])?
                }
                _ => return None,
            },
            T::Builtin(burdin_tir::Builtin::StringFrom, a)
                if matches!(a[..], [burdin_tir::Expr { kind: T::Str(_), .. }]) =>
            {
                format!("String.from({})", self.value_text(&a[0])?)
            }
            T::Array(items) => format!("[{}]", self.list(items, typed)?),
            T::ArrayRepeat(v, n) => format!("[{}; {n}]", self.value_in(v, typed)?),
            T::Tuple(items) => match items.len() {
                1 => format!("({},)", self.value_in(&items[0], typed)?),
                _ => format!("({})", self.list(items, typed)?),
            },
            T::Some(x) => format!("Option.Some({})", self.value_in(x, typed)?),
            T::None => "Option.None".to_string(),
            T::Ok(x) => format!("Result.Ok({})", self.value_in(x, typed)?),
            T::Err(x) => format!("Result.Err({})", self.value_in(x, typed)?),
            // A constant's name may not be in scope where the text goes, so its value is written instead.
            T::Const(id) => self.value_in(&p.consts.get(id.0)?.value, typed)?,
            T::ConstCond(inner) => self.value_in(inner, typed)?,
            T::Struct(fields) => {
                let Ty::Struct(adt) = &e.ty else { return None };
                let def = p.structs.get(adt.id)?;
                let mut parts = Vec::new();
                for (i, v) in fields {
                    parts.push(format!("{}: {}", def.fields.get(*i)?.name, self.value_in(v, true)?));
                }
                let name = def.name.rsplit('.').next()?;
                if parts.is_empty() { format!("{name} {{ }}") } else { format!("{name} {{ {} }}", parts.join(", ")) }
            }
            T::Variant(index, fields) => {
                let Ty::Enum(adt) = &e.ty else { return None };
                let def = p.enums.get(adt.id)?;
                let variant = def.variants.get(*index)?;
                let name = def.name.rsplit('.').next()?;
                match variant.shape {
                    burdin_tir::VariantShape::Unit => format!("{name}.{}", variant.name),
                    burdin_tir::VariantShape::Tuple => {
                        let mut sorted: Vec<&(usize, burdin_tir::Expr)> = fields.iter().collect();
                        sorted.sort_by_key(|(i, _)| *i);
                        let mut parts = Vec::new();
                        for (_, v) in sorted {
                            parts.push(self.value_in(v, true)?);
                        }
                        format!("{name}.{}({})", variant.name, parts.join(", "))
                    }
                    burdin_tir::VariantShape::Struct => {
                        let mut parts = Vec::new();
                        for (i, v) in fields {
                            parts.push(format!("{}: {}", variant.fields.get(*i)?.name, self.value_in(v, true)?));
                        }
                        format!("{name}.{} {{ {} }}", variant.name, parts.join(", "))
                    }
                }
            }
            _ => return None,
        })
    }

    fn list(&self, items: &[burdin_tir::Expr], typed: bool) -> Option<String> {
        let mut parts = Vec::new();
        for i in items {
            parts.push(self.value_in(i, typed)?);
        }
        Some(parts.join(", "))
    }

    // ----- instances -----------------------------------------------------------------------------------------

    /// Every reachable instance of a generic function declared in the files: the declaration with the typed tree
    /// of that instance, under a comment naming the instance. It is for reading; the declaration keeps its type
    /// parameters, so this part of the output is not a program of its own.
    fn instances_text(&self, files: &[SourceFile<'a>]) -> String {
        let reached = crate::reach::functions(self.program);
        let mut out = String::new();
        for file in files {
            let mut decls: Vec<&ast::FnDecl> = Vec::new();
            for item in &file.ast.items {
                match item {
                    ast::Item::Fn(f) => decls.push(f),
                    ast::Item::Extension(x) => decls.push(&x.func),
                    ast::Item::Impl(b) => decls.extend(b.methods.iter()),
                    _ => {}
                }
            }
            for decl in decls {
                let Some(candidates) = self.functions.get(&(decl.span.file.0, decl.span.lo)) else { continue };
                for &i in candidates {
                    let f = &self.program.functions[i];
                    if f.symbolic || !reached[i] || !crate::instances::is_instance(&f.name) {
                        continue;
                    }
                    let cx = Cx::of_block(self.program, &f.body);
                    let header = self.slice(Span::new(decl.span.file, decl.span.lo, decl.body.span.lo)).unwrap_or("");
                    let body = self.block_text(&decl.body, &cx);
                    out.push_str(&format!("// instance {}\n{header}{body}\n\n", f.name));
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Syntax-tree helpers

fn unit(name: &str, span: Span) -> Unit {
    Unit { name: name.to_string(), lo: span.lo as usize, hi: span.hi as usize, own_text: None }
}

/// The name of a type path without its generic arguments (`Pair<T>` is `Pair`).
fn type_head(t: &ast::Type) -> String {
    match &t.kind {
        ast::TypeKind::Path(segs) => segs.last().map(|s| s.name.name.clone()).unwrap_or_default(),
        ast::TypeKind::SelfType => "Self".to_string(),
        _ => String::new(),
    }
}

fn indent_block(text: &str, by: &str) -> String {
    text.lines().map(|l| if l.is_empty() { String::new() } else { format!("{by}{l}") }).collect::<Vec<_>>().join("\n")
}

/// The derives that the compiler implements itself (`spec/derives.md`); they have no source form.
const BUILTIN_DERIVES: [&str; 9] =
    ["Copy", "Clone", "Eq", "Ord", "Hash", "Default", "Debug", "Serialize", "Deserialize"];

/// The `: A, B` list around the derive types spanning `lo..hi`, from its `:` to the end of the last type.
fn clause_range(text: &str, lo: usize, hi: usize) -> Option<(usize, usize)> {
    let start = text[..lo].trim_end().strip_suffix(':')?.len();
    Some((start, hi))
}

fn escape(s: &str, quote: char) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            '{' if quote == '"' => out.push_str("{{"),
            '}' if quote == '"' => out.push_str("}}"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// The call under the wrappers that share its span (a cast, a `String` to `str` borrow).
fn peel_to_call(e: &burdin_tir::Expr) -> Option<&burdin_tir::Expr> {
    let e = peel_wrappers(e);
    matches!(e.kind, T::Call(..)).then_some(e)
}

fn peel_wrappers(mut e: &burdin_tir::Expr) -> &burdin_tir::Expr {
    loop {
        match &e.kind {
            T::Cast(inner) | T::StringAsStr(inner) | T::DerefRef(inner) | T::Deref(inner) | T::ConstCond(inner)
                if inner.span == e.span =>
            {
                e = inner
            }
            _ => return e,
        }
    }
}

fn pattern_names(p: &ast::Pattern, out: &mut Vec<String>) {
    use ast::PatternKind as P;
    match &p.kind {
        P::Binding { name, .. } => out.push(name.name.clone()),
        P::Tuple(ps) | P::Or(ps) | P::TupleStruct(_, ps) | P::Slice(ps) => {
            ps.iter().for_each(|p| pattern_names(p, out))
        }
        P::Struct(_, fields, _) => {
            for f in fields {
                match &f.pattern {
                    Some(p) => pattern_names(p, out),
                    None => out.push(f.name.name.clone()),
                }
            }
        }
        P::At(name, p) => {
            out.push(name.name.clone());
            pattern_names(p, out);
        }
        P::Rest(name) => out.extend(name.iter().map(|n| n.name.clone())),
        P::Typed { name, .. } => out.extend(name.iter().map(|n| n.name.clone())),
        P::Wild | P::Lit(_) | P::Range(..) | P::Path(_) => {}
    }
}

fn pat_locals(p: &burdin_tir::Pat, out: &mut Vec<burdin_tir::LocalId>) {
    use burdin_tir::Pat as P;
    match p {
        P::Binding(id, _) => out.push(*id),
        P::At(id, _, inner) => {
            out.push(*id);
            pat_locals(inner, out);
        }
        P::Or(ps) | P::Product(ps) | P::Variant(_, ps) => ps.iter().for_each(|p| pat_locals(p, out)),
        P::Slice { prefix, rest, suffix } => {
            for p in prefix.iter().chain(rest.as_deref()).chain(suffix) {
                pat_locals(p, out);
            }
        }
        P::Some(p) | P::Ok(p) | P::Err(p) => pat_locals(p, out),
        P::Wild | P::Bool(_) | P::Int(_) | P::Char(_) | P::Str(_) | P::Range(..) | P::None => {}
    }
}

fn block_children<'e>(b: &'e ast::Block, out: &mut Vec<&'e ast::Expr>) {
    for s in &b.stmts {
        match s {
            ast::Stmt::Let { init, else_block, .. } => {
                out.push(init);
                if let Some(eb) = else_block {
                    block_children(eb, out);
                }
            }
            ast::Stmt::Expr(e) => out.push(e),
        }
    }
}

/// The direct sub-expressions of `e`, in no particular order.
fn children<'e>(e: &'e ast::Expr, out: &mut Vec<&'e ast::Expr>) {
    use ast::ExprKind as K;
    match &e.kind {
        K::Int(_)
        | K::Float(_)
        | K::Char(_)
        | K::Str(_)
        | K::InlineSnapshot(_)
        | K::Bool(_)
        | K::Unit
        | K::Name { .. }
        | K::Target(_)
        | K::SelfValue
        | K::Continue(_) => {}
        K::Break { value, .. } => {
            if let Some(value) = value {
                out.push(value);
            }
        }
        K::Interp(parts) => {
            for p in parts {
                if let ast::InterpPart::Hole(h) = p
                    && let ast::HoleArg::Expr(x) = &h.arg
                {
                    out.push(x);
                }
            }
        }
        K::Tuple(items) | K::Array(items) => out.extend(items.iter()),
        K::ArrayRepeat(a, b)
        | K::Assign(a, b)
        | K::CompoundAssign(_, a, b)
        | K::Binary(_, a, b)
        | K::OrElse(a, b)
        | K::Index(a, b)
        | K::DynField(a, b) => {
            out.push(a);
            out.push(b);
        }
        K::Block(b) | K::UnsafeBlock(b) | K::Loop(b) => block_children(b, out),
        K::If { cond, then_block, else_expr } => {
            out.push(cond);
            block_children(then_block, out);
            out.extend(else_expr.iter().map(|b| &**b));
        }
        K::IfLet { scrutinee, then_block, else_expr, .. } => {
            out.push(scrutinee);
            block_children(then_block, out);
            out.extend(else_expr.iter().map(|b| &**b));
        }
        K::Labeled { body, .. } => out.push(body),
        K::While { cond, body } => {
            out.push(cond);
            block_children(body, out);
        }
        K::WhileLet { scrutinee, body, .. } => {
            out.push(scrutinee);
            block_children(body, out);
        }
        K::For { iter, body, .. } => {
            out.push(iter);
            block_children(body, out);
        }
        K::Return(x) => out.extend(x.iter().map(|b| &**b)),
        K::Unary(_, x) | K::Cast(x, _) | K::Try(x) | K::Field(x, _) | K::TypeArgs(x, _) | K::TupleField(x, ..) => {
            out.push(x);
        }
        K::Range { lo, hi, .. } => {
            out.push(lo);
            out.push(hi);
        }
        K::Call(callee, args) => {
            out.push(callee);
            out.extend(args.iter().map(|a| &a.expr));
        }
        K::Match(scrutinee, arms) => {
            out.push(scrutinee);
            for arm in arms {
                out.extend(arm.guard.iter());
                out.push(&arm.body);
            }
        }
        K::Closure { body, .. } => out.push(body),
        K::StructLit(_, _, inits, base) | K::TargetStruct(inits, base) => {
            out.extend(inits.iter().filter_map(|i| i.value.as_ref()));
            out.extend(base.as_deref());
        }
    }
}

/// Whether a `for` iterable is `Type.fields` or `Type.variants`: the loop `burdin expand` shows unrolled
/// (`spec/reflection.md`, RFL4).
fn is_reflection_iterable(iter: &ast::Expr) -> bool {
    matches!(&iter.kind, ast::ExprKind::Field(_, member) if member.name == "fields" || member.name == "variants")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_text_escapes_what_a_literal_cannot_hold() {
        assert_eq!(escape("a\"b\\c\n{d}", '"'), "a\\\"b\\\\c\\n{{d}}");
        assert_eq!(escape("'", '\''), "\\'");
        assert_eq!(escape("{", '\''), "{");
    }

    #[test]
    fn the_derives_clause_is_found_around_its_types() {
        let text = "struct P: Clone, Eq {\n}";
        let lo = text.find("Clone").unwrap();
        let hi = text.find("Eq").unwrap() + 2;
        let (start, end) = clause_range(text, lo, hi).unwrap();
        assert_eq!(&text[start..end], ": Clone, Eq");
        assert!(clause_range("struct P {", 9, 9).is_none());
    }

    #[test]
    fn a_unit_matches_by_name_module_or_package() {
        let ast = ast::File::default();
        let file = SourceFile { id: FileId(0), path: "a.bd", text: "", ast: &ast, package: "app", module: "shapes" };
        assert!(unit_matches(&file, "area", "area"));
        assert!(unit_matches(&file, "area", "shapes.area"));
        assert!(unit_matches(&file, "area", "app.shapes.area"));
        assert!(!unit_matches(&file, "area", "other.area"));
        assert_eq!(qualified(&file, "area"), "shapes.area");
    }

    #[test]
    fn patches_apply_in_order_and_inside_a_range() {
        let plan = Plan {
            patches: vec![
                Patch { lo: 2, hi: 4, text: "XX".to_string() },
                Patch { lo: 6, hi: 6, text: "!".to_string() },
            ],
            units: Vec::new(),
        };
        assert_eq!(plan.apply("abcdefgh", 0, 8), "abXXef!gh");
        assert_eq!(plan.apply("abcdefgh", 2, 5), "XXe");
    }
}
