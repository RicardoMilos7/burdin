//! What the refactorings ask of the syntax tree: the children of a node with the role each plays, the chain of
//! nodes from a function body down to one expression, whether an expression needs parentheses, and the text helpers
//! for lines and indentation.

use burdin_syntax::ast::*;

/// How a child expression or block is evaluated relative to its parent: what decides whether code can move out of
/// it without changing when, or how often, it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Evaluated once, in order, whenever the parent is.
    Plain,
    /// An operand of an operator, a cast, a field access, an index, or `?`: a replacement that is not atomic needs
    /// parentheses here.
    Operand,
    /// The callee of a call.
    Callee,
    /// An argument of a call.
    Arg,
    /// A `while` condition: evaluated once per iteration.
    LoopCond,
    /// The body of a loop: evaluated any number of times.
    LoopBody,
    /// The right side of `&&`, `||`, or `orelse`: evaluated only sometimes.
    ShortCircuit,
    /// A branch of `if` or `match` (a block, or an `else if`), or a match guard: evaluated only sometimes.
    Branch,
    /// The body of a closure: evaluated when the closure is called, not where it is written.
    Closure,
    /// A `{...}` placeholder of an interpolated string.
    Hole,
    /// A `comptime` expression: evaluated while compiling.
    Comptime,
}

impl Role {
    /// Whether code inside a child of this role runs exactly once each time the parent runs.
    pub fn runs_once(self) -> bool {
        matches!(self, Role::Plain | Role::Operand | Role::Callee | Role::Arg)
    }

    /// Why moving code out of a child of this role would change what it does.
    pub fn why_not(self) -> &'static str {
        match self {
            Role::LoopCond => "it is inside a `while` condition, which runs once per iteration",
            Role::LoopBody => "it is inside a loop body, which runs any number of times",
            Role::ShortCircuit => "it is on the right of `&&`, `||`, or `orelse`, which runs only sometimes",
            Role::Branch => "it is inside a branch or a guard, which runs only sometimes",
            Role::Closure => "it is inside a closure, which runs when it is called",
            Role::Hole => "it is inside an interpolated string",
            Role::Comptime => "it is inside a `comptime` expression, which runs while compiling",
            _ => "",
        }
    }
}

/// A child of an expression.
pub enum Child<'a> {
    Expr(Role, &'a Expr),
    Block(Role, &'a Block),
}

/// The children of an expression, in source order, with their roles. Patterns and types are not listed.
pub fn children(e: &Expr) -> Vec<Child<'_>> {
    use ExprKind as K;
    let mut out = Vec::new();
    match &e.kind {
        K::Int(_)
        | K::Float(_)
        | K::Char(_)
        | K::Str(_)
        | K::InlineSnapshot(_)
        | K::Bool(_)
        | K::Unit
        | K::SelfValue
        | K::Break
        | K::Continue
        | K::Name { .. }
        | K::Return(None) => {}
        K::Interp(parts) => {
            for part in parts {
                if let InterpPart::Hole(Hole { arg: HoleArg::Expr(x), .. }) = part {
                    out.push(Child::Expr(Role::Hole, x));
                }
            }
        }
        K::Tuple(es) | K::Array(es) => out.extend(es.iter().map(|x| Child::Expr(Role::Plain, x))),
        K::ArrayRepeat(a, b) => {
            out.push(Child::Expr(Role::Plain, a));
            out.push(Child::Expr(Role::Plain, b));
        }
        K::Assign(a, b) => {
            out.push(Child::Expr(Role::Plain, a));
            out.push(Child::Expr(Role::Plain, b));
        }
        K::Binary(op, a, b) => {
            out.push(Child::Expr(Role::Operand, a));
            let role = if matches!(op, BinOp::And | BinOp::Or) { Role::ShortCircuit } else { Role::Operand };
            out.push(Child::Expr(role, b));
        }
        K::OrElse(a, b) => {
            out.push(Child::Expr(Role::Operand, a));
            out.push(Child::Expr(Role::ShortCircuit, b));
        }
        K::Index(a, b) => {
            out.push(Child::Expr(Role::Operand, a));
            out.push(Child::Expr(Role::Plain, b));
        }
        K::Block(b) | K::UnsafeBlock(b) => out.push(Child::Block(Role::Plain, b)),
        K::Loop(b) => out.push(Child::Block(Role::LoopBody, b)),
        K::If { cond, then_block, else_expr } => {
            out.push(Child::Expr(Role::Plain, cond));
            out.push(Child::Block(Role::Branch, then_block));
            if let Some(x) = else_expr {
                out.push(Child::Expr(Role::Branch, x));
            }
        }
        K::IfLet { scrutinee, then_block, else_expr, .. } => {
            out.push(Child::Expr(Role::Plain, scrutinee));
            out.push(Child::Block(Role::Branch, then_block));
            if let Some(x) = else_expr {
                out.push(Child::Expr(Role::Branch, x));
            }
        }
        K::While { cond, body } => {
            out.push(Child::Expr(Role::LoopCond, cond));
            out.push(Child::Block(Role::LoopBody, body));
        }
        K::For { iter, body, .. } => {
            out.push(Child::Expr(Role::Plain, iter));
            out.push(Child::Block(Role::LoopBody, body));
        }
        K::Return(Some(x)) => out.push(Child::Expr(Role::Plain, x)),
        K::Unary(_, x) | K::Try(x) | K::Field(x, _) | K::TupleField(x, ..) | K::Cast(x, _) | K::TypeArgs(x, _) => {
            out.push(Child::Expr(Role::Operand, x))
        }
        K::Comptime(x) => out.push(Child::Expr(Role::Comptime, x)),
        K::Range { lo, hi, .. } => {
            out.push(Child::Expr(Role::Operand, lo));
            out.push(Child::Expr(Role::Operand, hi));
        }
        K::DynField(a, b) => {
            out.push(Child::Expr(Role::Operand, a));
            out.push(Child::Expr(Role::Comptime, b));
        }
        K::Call(callee, args) => {
            out.push(Child::Expr(Role::Callee, callee));
            out.extend(args.iter().map(|a| Child::Expr(Role::Arg, &a.expr)));
        }
        K::Match(scrutinee, arms) => {
            out.push(Child::Expr(Role::Plain, scrutinee));
            for arm in arms {
                if let Some(g) = &arm.guard {
                    out.push(Child::Expr(Role::Branch, g));
                }
                out.push(Child::Expr(Role::Branch, &arm.body));
            }
        }
        K::Closure { body, .. } => out.push(Child::Expr(Role::Closure, body)),
        K::StructLit(_, fields) => {
            out.extend(fields.iter().filter_map(|f| f.value.as_ref()).map(|x| Child::Expr(Role::Plain, x)))
        }
        K::ComptimeAssert(c, m) => {
            out.push(Child::Expr(Role::Comptime, c));
            if let Some(m) = m {
                out.push(Child::Expr(Role::Comptime, m));
            }
        }
    }
    out
}

/// The expressions a statement holds directly, and the `else` block of a `let`.
pub fn stmt_children(s: &Stmt) -> Vec<Child<'_>> {
    match s {
        Stmt::Let { init, else_block, .. } => {
            let mut out = vec![Child::Expr(Role::Plain, init)];
            if let Some(b) = else_block {
                out.push(Child::Block(Role::Branch, b));
            }
            out
        }
        Stmt::Expr(e) => vec![Child::Expr(Role::Plain, e)],
    }
}

/// The span of a statement.
pub fn stmt_span(s: &Stmt) -> burdin_diag::Span {
    match s {
        Stmt::Let { span, .. } => *span,
        Stmt::Expr(e) => e.span,
    }
}

/// One step on the way from a body to an expression.
#[derive(Clone, Debug)]
pub enum Step {
    /// A block, and the index of the statement the path goes through.
    Block { span: burdin_diag::Span, stmt: usize, stmt_span: burdin_diag::Span },
    /// An expression, and the role of the child the path goes into next (or `Plain` for the last one).
    Expr { span: burdin_diag::Span, role: Role, kind: &'static str },
}

/// The chain of nodes from `body` down to the innermost expression with exactly the span `lo..hi`, outermost first,
/// ending with that expression; `None` when no expression has that span.
pub fn path_to(body: &Block, lo: u32, hi: u32) -> Option<Vec<Step>> {
    let mut steps = Vec::new();
    if block_path(body, lo, hi, &mut steps) { Some(steps) } else { None }
}

fn block_path(b: &Block, lo: u32, hi: u32, steps: &mut Vec<Step>) -> bool {
    for (index, s) in b.stmts.iter().enumerate() {
        let span = stmt_span(s);
        if !(span.lo <= lo && hi <= span.hi) {
            continue;
        }
        steps.push(Step::Block { span: b.span, stmt: index, stmt_span: span });
        for child in stmt_children(s) {
            if child_path(child, Role::Plain, lo, hi, steps) {
                return true;
            }
        }
        steps.pop();
    }
    false
}

fn child_path(child: Child<'_>, _outer: Role, lo: u32, hi: u32, steps: &mut Vec<Step>) -> bool {
    match child {
        Child::Expr(role, e) => {
            if !(e.span.lo <= lo && hi <= e.span.hi) {
                return false;
            }
            let at = steps.len();
            steps.push(Step::Expr { span: e.span, role: Role::Plain, kind: kind_name(e) });
            for c in children(e) {
                let inner_role = match &c {
                    Child::Expr(r, _) | Child::Block(r, _) => *r,
                };
                if let Step::Expr { role, .. } = &mut steps[at] {
                    *role = inner_role;
                }
                if child_path(c, inner_role, lo, hi, steps) {
                    return true;
                }
            }
            if let Step::Expr { role, .. } = &mut steps[at] {
                *role = Role::Plain;
            }
            if e.span.lo == lo && e.span.hi == hi {
                return true;
            }
            let _ = role;
            steps.truncate(at);
            false
        }
        Child::Block(role, b) => {
            if !(b.span.lo <= lo && hi <= b.span.hi) {
                return false;
            }
            let _ = role;
            block_path(b, lo, hi, steps)
        }
    }
}

/// The innermost expression with exactly the span `lo..hi` under `body`.
pub fn expr_at(body: &Block, lo: u32, hi: u32) -> Option<&Expr> {
    fn in_block(b: &Block, lo: u32, hi: u32) -> Option<&Expr> {
        for s in &b.stmts {
            let span = stmt_span(s);
            if span.lo <= lo && hi <= span.hi {
                for c in stmt_children(s) {
                    if let Some(found) = in_child(c, lo, hi) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }
    fn in_child(c: Child<'_>, lo: u32, hi: u32) -> Option<&Expr> {
        match c {
            Child::Expr(_, e) => {
                if !(e.span.lo <= lo && hi <= e.span.hi) {
                    return None;
                }
                for inner in children(e) {
                    if let Some(found) = in_child(inner, lo, hi) {
                        return Some(found);
                    }
                }
                (e.span.lo == lo && e.span.hi == hi).then_some(e)
            }
            Child::Block(_, b) => in_block(b, lo, hi),
        }
    }
    in_block(body, lo, hi)
}

/// The innermost expression whose span covers `lo..hi`, when the selection is inside one expression of `body`.
pub fn expr_covering(body: &Block, lo: u32, hi: u32) -> Option<&Expr> {
    struct Found<'a> {
        best: Option<&'a Expr>,
    }
    fn child<'a>(c: Child<'a>, lo: u32, hi: u32, found: &mut Found<'a>) {
        match c {
            Child::Expr(_, e) => {
                if e.span.lo <= lo && hi <= e.span.hi {
                    let size = e.span.hi - e.span.lo;
                    if found.best.is_none_or(|b| size < b.span.hi - b.span.lo) {
                        found.best = Some(e);
                    }
                    for inner in children(e) {
                        child(inner, lo, hi, found);
                    }
                }
            }
            Child::Block(_, b) => {
                for s in &b.stmts {
                    for c in stmt_children(s) {
                        child(c, lo, hi, found);
                    }
                }
            }
        }
    }
    let mut found = Found { best: None };
    for s in &body.stmts {
        for c in stmt_children(s) {
            child(c, lo, hi, &mut found);
        }
    }
    found.best
}

/// Calls `f` for every expression under `b`, outermost first, with the role it has in its parent. `f` returns
/// whether to walk into the expression's children.
pub fn each_expr<'a>(b: &'a Block, f: &mut dyn FnMut(&'a Expr, Role) -> bool) {
    walk(b, f, false);
}

/// Like [`each_expr`], but the `{...}` holes of an interpolated string are not walked into: what they hold is
/// evaluated by the format call, and a tool that edits expressions leaves strings alone.
pub fn each_expr_skipping_holes<'a>(b: &'a Block, f: &mut dyn FnMut(&'a Expr, Role) -> bool) {
    walk(b, f, true);
}

fn walk<'a>(b: &'a Block, f: &mut dyn FnMut(&'a Expr, Role) -> bool, skip_holes: bool) {
    fn child<'a>(c: Child<'a>, f: &mut dyn FnMut(&'a Expr, Role) -> bool, skip_holes: bool) {
        match c {
            Child::Expr(role, e) => {
                if skip_holes && matches!(role, Role::Hole) {
                    return;
                }
                if f(e, role) {
                    for inner in children(e) {
                        child(inner, f, skip_holes);
                    }
                }
            }
            Child::Block(_, b) => walk(b, f, skip_holes),
        }
    }
    for s in &b.stmts {
        for c in stmt_children(s) {
            child(c, f, skip_holes);
        }
    }
}

/// A short name for an expression's kind, for diagnostics of the engine.
pub fn kind_name(e: &Expr) -> &'static str {
    use ExprKind as K;
    match &e.kind {
        K::Call(..) => "call",
        K::Closure { .. } => "closure",
        K::While { .. } => "while",
        K::For { .. } => "for",
        K::Loop(_) => "loop",
        K::Match(..) => "match",
        K::If { .. } | K::IfLet { .. } => "if",
        _ => "expression",
    }
}

/// Whether an expression binds tighter than any operator, so it can replace an operand without parentheses.
pub fn is_atomic(e: &Expr) -> bool {
    use ExprKind as K;
    matches!(
        e.kind,
        K::Int(_)
            | K::Float(_)
            | K::Char(_)
            | K::Str(_)
            | K::Interp(_)
            | K::Bool(_)
            | K::Unit
            | K::Name { .. }
            | K::SelfValue
            | K::Tuple(_)
            | K::Array(_)
            | K::ArrayRepeat(..)
            | K::Block(_)
            | K::Field(..)
            | K::TupleField(..)
            | K::TypeArgs(..)
            | K::Call(..)
            | K::Index(..)
            | K::Try(_)
            | K::StructLit(..)
    )
}

/// Whether evaluating an expression has no effect and costs nothing worth naming: a literal, or a name and the
/// fields of one. Such an expression may be evaluated more often, or in another order, without changing anything.
pub fn is_pure(e: &Expr) -> bool {
    use ExprKind as K;
    match &e.kind {
        K::Int(_) | K::Float(_) | K::Char(_) | K::Str(_) | K::Bool(_) | K::Unit | K::Name { .. } | K::SelfValue => true,
        K::Field(x, _) | K::TupleField(x, ..) => is_pure(x),
        K::Unary(UnOp::Neg, x) => matches!(x.kind, K::Int(_) | K::Float(_)),
        _ => false,
    }
}

/// Whether an expression is a place: a name and the fields of one.
pub fn is_place(e: &Expr) -> bool {
    use ExprKind as K;
    match &e.kind {
        K::Name { .. } | K::SelfValue => true,
        K::Field(x, _) | K::TupleField(x, ..) => is_place(x),
        K::Index(x, i) => is_place(x) && is_pure(i),
        _ => false,
    }
}

/// The name at the root of a place (`a` in `a.b[i]`).
pub fn place_root(e: &Expr) -> Option<&str> {
    use ExprKind as K;
    match &e.kind {
        K::Name { name, .. } => Some(&name.name),
        K::Field(x, _) | K::TupleField(x, ..) | K::Index(x, _) => place_root(x),
        K::Unary(UnOp::Deref, x) => place_root(x),
        _ => None,
    }
}

/// `text` parenthesized when it replaces an expression in an operand position and is not atomic.
pub fn wrap(text: &str, atomic: bool, role: Role) -> String {
    if atomic || !matches!(role, Role::Operand | Role::Callee) { text.to_string() } else { format!("({text})") }
}

/// The byte offset of the start of the line holding `at`.
pub fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

/// The byte offset just after the line break that ends the line holding `at` (or the end of the text).
pub fn line_end(text: &str, at: usize) -> usize {
    text[at..].find('\n').map_or(text.len(), |i| at + i + 1)
}

/// The white space before `at` on its line, when nothing else is: the indentation of a statement that starts
/// its line. `None` when other code comes first.
pub fn indent_of(text: &str, at: usize) -> Option<String> {
    let start = line_start(text, at);
    let before = &text[start..at];
    before.chars().all(|c| c == ' ' || c == '\t').then(|| before.to_string())
}

/// Whether only white space follows `at` on its line.
pub fn ends_line(text: &str, at: usize) -> bool {
    let end = line_end(text, at);
    text[at..end].trim().is_empty()
}

/// `text` with `from` removed from the start of every line after the first and `to` put in its place.
pub fn reindent(text: &str, from: &str, to: &str) -> String {
    let mut out = String::new();
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
            if line.trim().is_empty() {
                continue;
            }
            out.push_str(to);
            out.push_str(line.strip_prefix(from).unwrap_or(line.trim_start()));
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Whether `name` is a plain identifier that is not a keyword.
pub fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let starts = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_');
    starts && chars.all(|c| c.is_alphanumeric() || c == '_') && burdin_syntax::token::TokenKind::keyword(name).is_none()
}

/// The identifier tokens of a stretch of source, as `(offset, text)`. Strings and comments are skipped.
pub fn identifiers(text: &str) -> Vec<(usize, String)> {
    let mut diags = burdin_diag::Diagnostics::default();
    let lexed = burdin_syntax::lexer::lex(burdin_diag::FileId(0), text, &mut diags);
    lexed
        .tokens
        .iter()
        .filter_map(|t| match &t.kind {
            burdin_syntax::token::TokenKind::Ident(name) => Some((t.span.lo as usize, name.clone())),
            _ => None,
        })
        .collect()
}

/// The span of a top-level item.
pub fn item_span(item: &Item) -> burdin_diag::Span {
    burdin_syntax::walk::item_span(item)
}

/// The name of a top-level item that has one.
pub fn item_name(item: &Item) -> Option<&Ident> {
    match item {
        Item::Fn(d) => Some(&d.name),
        Item::Const(c) => Some(&c.name),
        Item::Struct(s) => Some(&s.name),
        Item::Enum(e) => Some(&e.name),
        Item::ErrorEnum(e) => Some(&e.name),
        Item::TypeAlias(t) => Some(&t.name),
        Item::Interface(i) => Some(&i.name),
        Item::Derive(d) => Some(&d.name),
        Item::ExternFn(x) => Some(&x.decl.name),
        Item::Extension(_) | Item::Impl(_) | Item::Extern(_) => None,
    }
}

/// Calls `f` for every block under `b`, `b` itself first, then the blocks nested in its statements.
pub fn each_block<'a>(b: &'a Block, f: &mut dyn FnMut(&'a Block)) {
    f(b);
    for s in &b.stmts {
        for c in stmt_children(s) {
            block_of(c, f);
        }
    }
}

fn block_of<'a>(c: Child<'a>, f: &mut dyn FnMut(&'a Block)) {
    match c {
        Child::Block(_, b) => each_block(b, f),
        Child::Expr(_, e) => {
            for inner in children(e) {
                block_of(inner, f);
            }
        }
    }
}

/// The expression whose direct child has exactly the span `lo..hi`, when there is one.
pub fn parent_of(body: &Block, lo: u32, hi: u32) -> Option<&Expr> {
    struct Found<'a> {
        at: Option<&'a Expr>,
    }
    fn child<'a>(c: Child<'a>, lo: u32, hi: u32, found: &mut Found<'a>) {
        match c {
            Child::Expr(_, e) => {
                if e.span.lo == lo && e.span.hi == hi {
                    return;
                }
                for inner in children(e) {
                    if let Child::Expr(_, x) = inner
                        && x.span.lo == lo
                        && x.span.hi == hi
                    {
                        found.at = Some(e);
                        return;
                    }
                    child(inner, lo, hi, found);
                    if found.at.is_some() {
                        return;
                    }
                }
            }
            Child::Block(_, b) => {
                for s in &b.stmts {
                    for c in stmt_children(s) {
                        child(c, lo, hi, found);
                        if found.at.is_some() {
                            return;
                        }
                    }
                }
            }
        }
    }
    let mut found = Found { at: None };
    for s in &body.stmts {
        for c in stmt_children(s) {
            child(c, lo, hi, &mut found);
            if found.at.is_some() {
                return found.at;
            }
        }
    }
    None
}

/// What a stretch of statements or expressions mentions that a refactoring must not move: `return`, `break`,
/// `continue`, and `self`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ControlFlow {
    pub returns: bool,
    pub breaks: bool,
    pub continues: bool,
    pub self_value: bool,
    pub comptime: bool,
}

impl ControlFlow {
    pub fn any(&self) -> bool {
        self.returns || self.breaks || self.continues || self.self_value || self.comptime
    }

    /// The first reason a stretch of code cannot be moved into a function, if there is one.
    pub fn reason(&self) -> Option<&'static str> {
        if self.returns {
            Some("it holds a `return`, which would leave the new function instead of the one it is in")
        } else if self.breaks {
            Some("it holds a `break`, which would leave inside the new function instead of the loop it is in")
        } else if self.continues {
            Some("it holds a `continue`, which would not reach the loop it is in")
        } else if self.self_value {
            Some("it uses `self`, which an extracted free function cannot take")
        } else if self.comptime {
            Some("it holds a `comptime` expression, which would be evaluated in another function")
        } else {
            None
        }
    }
}

/// What a block's expressions mention, for [`ControlFlow`].
pub fn control_flow(b: &Block) -> ControlFlow {
    let mut out = ControlFlow::default();
    each_expr(b, &mut |e, _| {
        match &e.kind {
            ExprKind::Return(_) => out.returns = true,
            ExprKind::Break => out.breaks = true,
            ExprKind::Continue => out.continues = true,
            ExprKind::SelfValue => out.self_value = true,
            ExprKind::Comptime(_) | ExprKind::ComptimeAssert(..) => out.comptime = true,
            _ => {}
        }
        true
    });
    out
}

/// What one expression mentions, for [`ControlFlow`].
pub fn control_flow_of(e: &Expr) -> ControlFlow {
    let block = Block { stmts: vec![Stmt::Expr(e.clone())], span: e.span };
    control_flow(&block)
}
