//! Structural search and replace (`spec/refactor.md`, RF1-RF6): a pattern written as Burdin code, matched against the
//! syntax tree of a package, with `$name` placeholders that stand for any expression. A match is a whole expression
//! node, so the engine never edits inside a string, a comment, or a token: only whole expressions are replaced (RF4).
//!
//! Names in a pattern match by what they resolve to, not by their spelling: a pattern `geometry.rectangle_area`
//! matches `area` where an import brought that function into scope, and a name imported under an alias matches the
//! name it was declared with (RF3). A placeholder may carry a type constraint, written after the pattern in a `where`
//! clause: `$a.unwrap_or($b) where $a: Option<_>`; `_` matches any part of a type.

use crate::tree::{self, Role};
use crate::{Engine, FileChange, Refusal, apply, refuse};
use burdin_query::{Analysis, Location};
use burdin_syntax::ast::{self, Expr, ExprKind, Stmt};
use std::collections::{BTreeMap, HashMap};

/// A placeholder in a pattern: `$name`, with its type constraint when one is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placeholder {
    /// The name after `$`.
    pub name: String,
    /// The type as written after `name:`, `_` marking any part.
    pub ty: Option<String>,
}

/// A parsed pattern: the expression tree, the placeholders, and the text the tree's spans point into.
pub struct Pattern {
    /// The synthetic source the tree's spans point into.
    text: String,
    expr: Expr,
    places: Vec<Placeholder>,
}

/// The prefix a placeholder's identifier has in the parsed pattern.
const PREFIX: &str = "__burdin_ssr_";

/// What one placeholder bound in one match: its name, the matched text, and where the matched expression is.
#[derive(Clone, Debug)]
pub struct Binding {
    pub name: String,
    pub text: String,
    pub lo: u32,
    pub hi: u32,
}

/// Splits a pattern into its expression and its `where` constraints. `pattern` is:
///
/// ```text
/// $a.unwrap_or($b) where $a: Option<_>
/// ```
pub fn parse(pattern: &str) -> Result<Pattern, Refusal> {
    let (body, constraints) = match split_where(pattern) {
        Some((b, c)) => (b.trim().to_string(), c.trim().to_string()),
        None => (pattern.trim().to_string(), String::new()),
    };
    if body.is_empty() {
        return refuse("the pattern is empty");
    }
    if body.contains("//") || body.contains("/*") || body.contains('{') {
        return refuse("a pattern is one expression, without comments or a block");
    }
    // Each `$name` becomes an identifier the parser accepts. A repeated placeholder keeps one identifier, so the
    // parsed tree shows where a match must bind the same expression twice.
    let mut names: Vec<String> = Vec::new();
    let mut source = String::new();
    let mut rest = body.as_str();
    while let Some(at) = rest.find('$') {
        source.push_str(&rest[..at]);
        rest = &rest[at + 1..];
        let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len());
        let name = &rest[..end];
        if name.is_empty() {
            return refuse("`$` must be followed by a placeholder name, as in `$a`");
        }
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
        source.push_str(PREFIX);
        source.push_str(name);
        rest = &rest[end..];
    }
    source.push_str(rest);
    let mut places: Vec<Placeholder> = names.into_iter().map(|name| Placeholder { name, ty: None }).collect();
    for part in constraints.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((name, ty)) = part.strip_prefix('$').and_then(|p| p.split_once(':')) else {
            return refuse(format!("`{part}` is not a constraint on a placeholder; write `$a: Type`"));
        };
        let (name, ty) = (name.trim(), ty.trim());
        if ty.is_empty() {
            return refuse(format!("the constraint on `${name}` names no type"));
        }
        match places.iter_mut().find(|p| p.name == name) {
            Some(p) => p.ty = Some(ty.to_string()),
            None => return refuse(format!("`${name}` is constrained but does not appear in the pattern")),
        }
    }
    let (text, expr) = parse_expr(&source)?;
    Ok(Pattern { text, expr, places })
}

/// A pattern's text and its `where` clause, split at the last top-level ` where `.
fn split_where(pattern: &str) -> Option<(&str, &str)> {
    let mut found: Option<usize> = None;
    let mut at = 0;
    while let Some(i) = pattern[at..].find(" where ") {
        let index = at + i;
        let before = pattern[..index].chars().next_back().unwrap_or(' ');
        if !(before.is_alphanumeric() || before == '_' || before == '$') {
            found = Some(index);
        }
        at = index + 7;
    }
    found.map(|index| (&pattern[..index], &pattern[index + 7..]))
}

/// Parses one expression by wrapping it in a function body. Returns the text its spans point into and the expression.
fn parse_expr(source: &str) -> Result<(String, Expr), Refusal> {
    let text = format!("fn __burdin_ssr_pattern() {{\n    {source}\n}}\n");
    let Some((text, expr)) = parse_expression_text(&text) else {
        return refuse(format!("`{source}` is not one expression"));
    };
    Ok((text, expr))
}

/// The last expression of `text`, read as a function's body: `None` when the text is not exactly one expression.
fn parse_expression_text(text: &str) -> Option<(String, Expr)> {
    let mut diags = burdin_diag::Diagnostics::default();
    let file = burdin_syntax::parse(burdin_diag::FileId(0), text, &mut diags);
    if diags.has_errors() {
        return None;
    }
    let [ast::Item::Fn(decl)] = file.items.as_slice() else { return None };
    match decl.body.stmts.as_slice() {
        [Stmt::Expr(e)] => Some((text.to_string(), e.clone())),
        _ => None,
    }
}

impl Pattern {
    /// The placeholders of this pattern, in the order they first appear.
    pub fn placeholders(&self) -> &[Placeholder] {
        &self.places
    }

    /// The text of the pattern as parsed (its placeholders renamed).
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether `e` of `text` matches the pattern, filling `bound` with the placeholders' text.
    fn matches(&self, cx: &MatchCx<'_>, pattern: &Expr, e: &Expr, bound: &mut Vec<Binding>) -> bool {
        if let ExprKind::Name { name, args } = &pattern.kind
            && args.is_empty()
            && let Some(place) = name.name.strip_prefix(PREFIX)
        {
            let value = cx.text(e);
            if let Some(old) = bound.iter().find(|b| b.name == place) {
                return old.text == value;
            }
            bound.push(Binding { name: place.to_string(), text: value, lo: e.span.lo, hi: e.span.hi });
            return true;
        }
        // A dotted path and a plain name denote the same item when each resolves to what the other names.
        if let (Some(p), Some(t)) = (dotted(pattern), dotted(e))
            && p != t
            && cx.name_matches(&p, &t, e)
        {
            return true;
        }
        match (&pattern.kind, &e.kind) {
            (ExprKind::Int(a), ExprKind::Int(b)) => a == b,
            (ExprKind::Float(a), ExprKind::Float(b)) => a == b,
            (ExprKind::Char(a), ExprKind::Char(b)) => a == b,
            (ExprKind::Str(a), ExprKind::Str(b)) => a == b,
            (ExprKind::Bool(a), ExprKind::Bool(b)) => a == b,
            (ExprKind::Unit, ExprKind::Unit) | (ExprKind::SelfValue, ExprKind::SelfValue) => true,
            (ExprKind::Break { label: la, value: va }, ExprKind::Break { label: lb, value: vb }) => {
                la.as_ref().map(|l| &l.name) == lb.as_ref().map(|l| &l.name)
                    && match (va, vb) {
                        (Some(a), Some(b)) => self.matches(cx, a, b, bound),
                        (None, None) => true,
                        _ => false,
                    }
            }
            (ExprKind::Continue(a), ExprKind::Continue(b)) => {
                a.as_ref().map(|l| &l.name) == b.as_ref().map(|l| &l.name)
            }
            (ExprKind::Labeled { label: a, body: x }, ExprKind::Labeled { label: b, body: y }) => {
                a.name == b.name && self.matches(cx, x, y, bound)
            }
            (ExprKind::Name { name: a, args: at }, ExprKind::Name { name: b, args: bt }) => {
                at.is_empty() && bt.is_empty() && cx.name_matches(&a.name, &b.name, e)
            }
            (ExprKind::Tuple(a), ExprKind::Tuple(b)) | (ExprKind::Array(a), ExprKind::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| self.matches(cx, x, y, bound))
            }
            (ExprKind::Block(a), ExprKind::Block(b)) => self.blocks(cx, a, b, bound),
            (ExprKind::Binary(a, x, y), ExprKind::Binary(b, p, q)) => {
                a == b && self.matches(cx, x, p, bound) && self.matches(cx, y, q, bound)
            }
            (ExprKind::Unary(a, x), ExprKind::Unary(b, y)) => a == b && self.matches(cx, x, y, bound),
            (ExprKind::Assign(..), ExprKind::Assign(..))
            | (ExprKind::OrElse(..), ExprKind::OrElse(..))
            | (ExprKind::Index(..), ExprKind::Index(..))
            | (ExprKind::ArrayRepeat(..), ExprKind::ArrayRepeat(..))
            | (ExprKind::DynField(..), ExprKind::DynField(..)) => {
                let (a, b) = two(&pattern.kind);
                let (x, y) = two(&e.kind);
                self.matches(cx, a, x, bound) && self.matches(cx, b, y, bound)
            }
            (ExprKind::Call(a, args_a), ExprKind::Call(b, args_b)) => {
                self.matches(cx, a, b, bound)
                    && args_a.len() == args_b.len()
                    && args_a.iter().zip(args_b).all(|(x, y)| {
                        x.name.as_ref().map(|n| &n.name) == y.name.as_ref().map(|n| &n.name)
                            && self.matches(cx, &x.expr, &y.expr, bound)
                    })
            }
            (ExprKind::Field(a, fa), ExprKind::Field(b, fb)) => fa.name == fb.name && self.matches(cx, a, b, bound),
            (ExprKind::TupleField(a, ia, _), ExprKind::TupleField(b, ib, _)) => {
                ia == ib && self.matches(cx, a, b, bound)
            }
            (ExprKind::Try(a), ExprKind::Try(b)) => self.matches(cx, a, b, bound),
            (ExprKind::Return(Some(a)), ExprKind::Return(Some(b))) => self.matches(cx, a, b, bound),
            (
                ExprKind::If { cond: c1, then_block: t1, else_expr: e1 },
                ExprKind::If { cond: c2, then_block: t2, else_expr: e2 },
            ) => {
                self.matches(cx, c1, c2, bound)
                    && self.blocks(cx, t1, t2, bound)
                    && match (e1, e2) {
                        (None, None) => true,
                        (Some(x), Some(y)) => self.matches(cx, x, y, bound),
                        _ => false,
                    }
            }
            (
                ExprKind::IfLet { scrutinee: s1, then_block: t1, else_expr: e1, .. },
                ExprKind::IfLet { scrutinee: s2, then_block: t2, else_expr: e2, .. },
            ) => {
                self.matches(cx, s1, s2, bound)
                    && self.blocks(cx, t1, t2, bound)
                    && match (e1, e2) {
                        (None, None) => true,
                        (Some(x), Some(y)) => self.matches(cx, x, y, bound),
                        _ => false,
                    }
            }
            (ExprKind::While { cond: c1, body: b1 }, ExprKind::While { cond: c2, body: b2 }) => {
                self.matches(cx, c1, c2, bound) && self.blocks(cx, b1, b2, bound)
            }
            (ExprKind::Loop(b1), ExprKind::Loop(b2)) => self.blocks(cx, b1, b2, bound),
            (ExprKind::For { iter: i1, body: b1, .. }, ExprKind::For { iter: i2, body: b2, .. }) => {
                self.matches(cx, i1, i2, bound) && self.blocks(cx, b1, b2, bound)
            }
            (ExprKind::Range { lo: a, hi: b, inclusive: i }, ExprKind::Range { lo: c, hi: d, inclusive: j }) => {
                i == j && self.matches(cx, a, c, bound) && self.matches(cx, b, d, bound)
            }
            (ExprKind::Closure { body: a, .. }, ExprKind::Closure { body: b, .. }) => self.matches(cx, a, b, bound),
            (ExprKind::Match(s1, arms1), ExprKind::Match(s2, arms2)) => {
                self.matches(cx, s1, s2, bound)
                    && arms1.len() == arms2.len()
                    && arms1.iter().zip(arms2).all(|(x, y)| {
                        let guard = match (&x.guard, &y.guard) {
                            (None, None) => true,
                            (Some(a), Some(b)) => self.matches(cx, a, b, bound),
                            _ => false,
                        };
                        guard && self.matches(cx, &x.body, &y.body, bound)
                    })
            }
            (ExprKind::StructLit(path_a, f1), ExprKind::StructLit(path_b, f2)) => {
                path_a.len() == path_b.len()
                    && path_a.iter().zip(path_b).all(|(x, y)| x.name == y.name)
                    && f1.len() == f2.len()
                    && f1.iter().zip(f2).all(|(x, y)| {
                        x.name.name == y.name.name
                            && match (&x.value, &y.value) {
                                (None, None) => true,
                                (Some(a), Some(b)) => self.matches(cx, a, b, bound),
                                _ => false,
                            }
                    })
            }
            (ExprKind::Comptime(a), ExprKind::Comptime(b)) => self.matches(cx, a, b, bound),
            (ExprKind::Interp(_), ExprKind::Interp(_)) => false,
            _ => false,
        }
    }

    fn blocks(&self, cx: &MatchCx<'_>, a: &ast::Block, b: &ast::Block, bound: &mut Vec<Binding>) -> bool {
        a.stmts.len() == b.stmts.len() && a.stmts.iter().zip(&b.stmts).all(|(x, y)| self.stmt(cx, x, y, bound))
    }

    fn stmt(&self, cx: &MatchCx<'_>, a: &Stmt, b: &Stmt, bound: &mut Vec<Binding>) -> bool {
        match (a, b) {
            (Stmt::Expr(x), Stmt::Expr(y)) => self.matches(cx, x, y, bound),
            (Stmt::Let { init: x, else_block: e1, .. }, Stmt::Let { init: y, else_block: e2, .. }) => {
                self.matches(cx, x, y, bound)
                    && match (e1, e2) {
                        (None, None) => true,
                        (Some(p), Some(q)) => self.blocks(cx, p, q, bound),
                        _ => false,
                    }
            }
            _ => false,
        }
    }
}

/// The dotted path an expression spells, when it is a name and the fields of one (`geometry.rectangle_area`).
fn dotted(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Name { name, args } if args.is_empty() => Some(name.name.clone()),
        ExprKind::Field(base, field) => Some(format!("{}.{}", dotted(base)?, field.name)),
        _ => None,
    }
}

/// The two expression children of a binary node.
fn two(kind: &ExprKind) -> (&Expr, &Expr) {
    match kind {
        ExprKind::Assign(a, b)
        | ExprKind::OrElse(a, b)
        | ExprKind::Index(a, b)
        | ExprKind::ArrayRepeat(a, b)
        | ExprKind::DynField(a, b) => (a, b),
        _ => unreachable!("not a binary node"),
    }
}

/// What a match needs from the package: the text, the type of each expression, and the names' resolutions.
struct MatchCx<'a> {
    analysis: &'a Analysis,
    path: &'a str,
    text: &'a str,
    /// The definitions functions and constants are found by, from `Analysis::functions`.
    qualified: HashMap<(String, u32), String>,
}

impl MatchCx<'_> {
    fn text(&self, e: &Expr) -> String {
        self.text.get(e.span.lo as usize..e.span.hi as usize).unwrap_or("").to_string()
    }

    /// Whether the name `pattern` written in the pattern matches the name `written` at `at` in the code (RF3).
    fn name_matches(&self, pattern: &str, written: &str, at: &Expr) -> bool {
        if pattern == written {
            return true;
        }
        let Some(resolved) = self.analysis.resolve_at(self.path, at.span.lo) else { return false };
        let last = pattern.rsplit('.').next().unwrap_or(pattern);
        if resolved.name != last {
            return false;
        }
        if !pattern.contains('.') {
            return true;
        }
        // A dotted pattern names a module path: `geometry.rectangle_area` matches the function declared in
        // `geometry`. The package's own name is not part of the qualified name.
        match resolved.definition {
            Some(definition) => match self.qualified.get(&(definition.path.clone(), definition.lo)) {
                Some(full) => full == pattern || full.ends_with(&format!(".{pattern}")),
                None => false,
            },
            None => false,
        }
    }
}

/// Whether a type `pattern` covers the type `actual`: equal, or with `_` standing for any part of it (a wildcard
/// matches one whole type argument, or the rest of the type when it is last).
pub fn ty_covers(pattern: &str, actual: &str) -> bool {
    covers(&type_tokens(pattern), &type_tokens(actual))
}

/// The tokens of a type: a run of letters, digits, and `_`, or one other character.
fn type_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
            if !c.is_whitespace() {
                out.push(c.to_string());
            }
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

fn covers(pattern: &[String], actual: &[String]) -> bool {
    match pattern.split_first() {
        None => actual.is_empty(),
        Some((first, rest)) if first == "_" => {
            (0..=actual.len()).any(|take| balanced(&actual[..take]) && covers(rest, &actual[take..]))
        }
        Some((first, rest)) => actual.first() == Some(first) && covers(rest, &actual[1..]),
    }
}

/// Whether the brackets of a token list are balanced, so a wildcard may take all of it.
fn balanced(tokens: &[String]) -> bool {
    let mut depth = 0i32;
    for t in tokens {
        match t.as_str() {
            "<" | "(" | "[" => depth += 1,
            ">" | ")" | "]" => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

/// One place the pattern matched.
#[derive(Clone, Debug)]
pub struct Hit {
    pub path: String,
    pub at: Location,
    /// The matched text.
    pub source: String,
    /// The replacement text, when a replacement is given.
    pub replacement: Option<String>,
    /// How the match sits in its parent, which decides whether its replacement needs parentheses.
    pub role: Role,
    /// The placeholder bindings, in pattern order.
    pub bindings: Vec<Binding>,
}

/// Every match of `pattern` in the entry package, in source order. `replacement` is the text that takes the place of
/// each match; `None` only lists the matches. A match that fails a placeholder's type constraint is not a match.
pub fn search(engine: &mut Engine<'_>, pattern: &Pattern, replacement: Option<&str>) -> Result<Vec<Hit>, Refusal> {
    let files = engine.entry_files();
    let replacement = match replacement {
        Some(text) if !text.trim().is_empty() => {
            let text = text.trim();
            if parse_expr(&placeholder_idents(text)).is_err() {
                return refuse(format!("`{text}` is not one expression, so it cannot be a replacement"));
            }
            Some(text.to_string())
        }
        _ => None,
    };
    let qualified: HashMap<(String, u32), String> = engine
        .analysis
        .functions()
        .into_iter()
        .map(|f| ((f.location.path.clone(), f.location.lo), f.qualified))
        .collect();
    let mut hits: Vec<Hit> = Vec::new();
    for path in files {
        let parsed = engine.ast(&path)?;
        let text = parsed.text.clone();
        let ast_file = &parsed.ast;
        let cx = MatchCx { analysis: &engine.analysis, path: &path, text: &text, qualified: qualified.clone() };
        for_each_body(ast_file, &text, &mut |body| {
            tree::each_expr_skipping_holes(body, &mut |e, role| {
                let mut bound: Vec<Binding> = Vec::new();
                if !pattern.matches(&cx, &pattern.expr, e, &mut bound) {
                    return true;
                }
                // The placeholders' type constraints, checked against what the checker typed the match with.
                for place in pattern.places.iter().filter(|p| p.ty.is_some()) {
                    let Some(binding) = bound.iter().find(|b| b.name == place.name) else { continue };
                    let ty = place.ty.as_deref().unwrap_or("_");
                    let known = cx.analysis.expr_facts(&path, binding.lo, binding.hi).map(|f| f.ty);
                    if !known.is_some_and(|t| ty_covers(ty, &t)) {
                        return true;
                    }
                }
                hits.push(Hit {
                    path: path.clone(),
                    at: Location { path: path.clone(), lo: e.span.lo, hi: e.span.hi },
                    source: cx.text(e),
                    replacement: None,
                    role,
                    bindings: bound,
                });
                true
            });
        });
    }
    // A match inside another match is not listed on its own: replacing the outer one covers it (RF4).
    hits.sort_by_key(|h| (h.path.clone(), h.at.lo, std::cmp::Reverse(h.at.hi)));
    let mut kept: Vec<Hit> = Vec::new();
    for hit in hits {
        let same = kept.iter().any(|k| k.path == hit.path && (k.at.lo, k.at.hi) == (hit.at.lo, hit.at.hi));
        let nested = kept.iter().any(|k| k.path == hit.path && k.at.lo <= hit.at.lo && hit.at.hi <= k.at.hi);
        if same || nested {
            continue;
        }
        kept.push(hit);
    }
    for hit in &mut kept {
        // A replacement that is not atomic takes parentheses where the match stood in an operand position.
        hit.replacement = replacement.as_deref().map(|r| {
            let text = substitute(r, &hit.bindings);
            tree::wrap(&text, is_atomic_text(&text), hit.role)
        });
    }
    Ok(kept)
}

/// `replacement` with each `$name` replaced by the matched text, parenthesized when it stands in an operand position
/// and is not atomic (RF5).
fn substitute(replacement: &str, bindings: &[Binding]) -> String {
    let mut out = String::new();
    let mut rest = replacement;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        rest = &rest[at + 1..];
        let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len());
        let name = &rest[..end];
        match bindings.iter().find(|b| b.name == name) {
            Some(binding) => {
                let operand = binding.text.starts_with(['-', '!', '*', '&'])
                    || substitute_before(&out)
                    || substitute_after(&rest[end..]);
                if operand && !is_atomic_text(&binding.text) {
                    out.push('(');
                    out.push_str(&binding.text);
                    out.push(')');
                } else {
                    out.push_str(&binding.text);
                }
            }
            None => {
                out.push('$');
                out.push_str(name);
            }
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Whether the text before the placeholder in a replacement is an operator or a `.`, so the placeholder is an
/// operand.
fn substitute_before(out: &str) -> bool {
    let trimmed = out.trim_end();
    if trimmed.ends_with(" as") {
        return true;
    }
    matches!(trimmed.chars().next_back(), Some(c) if !(c.is_alphanumeric() || c == '_' || matches!(c, '(' | ',' | '[' | '{' | ':' | '!' | '@')))
}

/// Whether the text after the placeholder in a replacement is an operator or a `.`.
fn substitute_after(rest: &str) -> bool {
    let trimmed = rest.trim_start();
    if trimmed.starts_with("as ") {
        return true;
    }
    matches!(trimmed.chars().next(), Some(c) if !(c.is_alphanumeric() || c == '_' || matches!(c, ')' | ',' | ']' | '}' | ';' | ':')))
}

/// `text` with each `$name` written as the identifier the parser accepts, so a replacement can be checked as one
/// expression before any match exists.
fn placeholder_idents(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        rest = &rest[at + 1..];
        let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len());
        out.push_str(PREFIX);
        out.push_str(&rest[..end]);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Whether `text` spells an atomic expression: a name, a literal, a call, or a field access.
pub fn is_atomic_text(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    match parse_expression_text(&format!("fn f() {{ {text} }}\n")) {
        Some((_, e)) => tree::is_atomic(&e),
        None => false,
    }
}

/// Calls `f` for the body of every function, method, extension method, and test of a file. Code the compiler
/// generated (derives, lowered error enums) has no text of its own and is skipped.
fn for_each_body<'a>(file: &'a ast::File, text: &str, f: &mut dyn FnMut(&'a ast::Block)) {
    let generated = |span: burdin_diag::Span| {
        let slice = text.get(span.lo as usize..span.hi as usize).unwrap_or("");
        !slice.is_empty() && !slice.contains(char::is_whitespace) && !slice.contains('{')
    };
    burdin_syntax::walk::for_each_fn(file, &generated, &mut |_, decl| {
        if !generated(decl.span) {
            f(&decl.body);
        }
    });
}

/// What a replacement did.
pub struct ReplaceReport {
    pub hits: Vec<Hit>,
    pub changes: Vec<FileChange>,
    /// The matches skipped because applying them alone would not check, each with the reason.
    pub skipped: Vec<(Hit, String)>,
}

/// Replaces every match in the package, checking the result before it is returned (RF2, RF6). When the result as a
/// whole does not check, the matches are applied one at a time and only those that keep the package checking are
/// kept; the rest are reported in [`ReplaceReport::skipped`].
pub fn replace(engine: &mut Engine<'_>, pattern: &Pattern, replacement: &str) -> Result<ReplaceReport, Refusal> {
    let hits = search(engine, pattern, Some(replacement))?;
    let mut report = ReplaceReport { hits: hits.clone(), changes: Vec::new(), skipped: Vec::new() };
    if hits.is_empty() {
        return Ok(report);
    }
    let all: BTreeMap<String, Vec<(usize, usize, String)>> = by_file(&hits);
    match verify(engine, &all) {
        Ok(changes) => report.changes = changes,
        Err(_) => {
            let mut accepted: BTreeMap<String, Vec<(usize, usize, String)>> = BTreeMap::new();
            for hit in &hits {
                let one = by_file(std::slice::from_ref(hit));
                match verify(engine, &one) {
                    Ok(_) => accepted.entry(hit.path.clone()).or_default().extend(one.into_values().flatten()),
                    Err(r) => report.skipped.push((hit.clone(), r.0)),
                }
            }
            report.changes = verify(engine, &accepted)?;
        }
    }
    Ok(report)
}

/// The edits of `hits`, by file.
fn by_file(hits: &[Hit]) -> BTreeMap<String, Vec<(usize, usize, String)>> {
    let mut out: BTreeMap<String, Vec<(usize, usize, String)>> = BTreeMap::new();
    for hit in hits {
        out.entry(hit.path.clone()).or_default().push((
            hit.at.lo as usize,
            hit.at.hi as usize,
            hit.replacement.clone().unwrap_or_default(),
        ));
    }
    out
}

/// Applies edits by file and checks the result.
fn verify(engine: &mut Engine<'_>, edits: &BTreeMap<String, Vec<(usize, usize, String)>>) -> crate::Outcome {
    let mut edited = BTreeMap::new();
    for (path, list) in edits {
        edited.insert(path.clone(), apply(&engine.text(path), list.clone()));
    }
    engine.verify(edited, &HashMap::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_parses_its_placeholders_and_constraints() {
        let p = parse("$a.unwrap_or($b) where $a: Option<_>").expect("pattern");
        let names: Vec<&str> = p.placeholders().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(p.places[0].ty.as_deref(), Some("Option<_>"));
        assert!(parse("$a where $b: i32").is_err());
        assert!(parse("1 +").is_err());
        assert!(parse("$").is_err());
    }

    #[test]
    fn a_repeated_placeholder_must_bind_the_same_text() {
        let mut db = crate::testing::db(&[("main.bd", "fn main() {\n    println(1)\n}\n")]);
        let analysis = db.analyze().expect("analysis");
        let pattern = parse("$a - $a").expect("pattern");
        let source = "fn f() { x - x }";
        let cx = MatchCx { analysis: &analysis, path: "main.bd", text: source, qualified: HashMap::new() };
        let (_, same) = parse_expression_text(source).expect("expr");
        let mut bound: Vec<Binding> = Vec::new();
        assert!(pattern.matches(&cx, &pattern.expr, &same, &mut bound));
        assert_eq!(bound[0].text, "x");
        let other_text = "fn f() { x - y }";
        let other_cx = MatchCx { analysis: &analysis, path: "main.bd", text: other_text, qualified: HashMap::new() };
        let (_, other) = parse_expression_text(other_text).expect("expr");
        let mut bound: Vec<Binding> = Vec::new();
        assert!(!pattern.matches(&other_cx, &pattern.expr, &other, &mut bound));
    }

    #[test]
    fn a_type_wildcard_covers_the_parts_it_marks() {
        assert!(ty_covers("Option<_>", "Option<i32>"));
        assert!(ty_covers("Option<i32>", "Option<i32>"));
        assert!(!ty_covers("Option<i32>", "Option<u8>"));
        assert!(ty_covers("List<_>", "List<Str>"));
        assert!(ty_covers("_", "i32"));
        assert!(ty_covers("Result<_, Err>", "Result<i32, Err>"));
        assert!(!ty_covers("Result<_, _>", "Option<i32>"));
    }

    #[test]
    fn substitution_parenthesizes_only_where_it_must() {
        let bindings = vec![
            Binding { name: "a".to_string(), text: "x + 1".to_string(), lo: 0, hi: 0 },
            Binding { name: "b".to_string(), text: "y".to_string(), lo: 0, hi: 0 },
        ];
        assert_eq!(substitute("$a * 2", &bindings), "(x + 1) * 2");
        assert_eq!(substitute("f($a)", &bindings), "f(x + 1)");
        assert_eq!(substitute("$b.foo()", &bindings), "y.foo()");
        assert_eq!(substitute("$a", &bindings), "x + 1");
        assert_eq!(substitute("2 * $a", &bindings), "2 * (x + 1)");
    }

    #[test]
    fn a_non_atomic_replacement_takes_parentheses_in_an_operand_position() {
        let source = "fn main() {\n    let x = 1\n    let b = (x + 1) * 4\n    println(\"{b}\")\n}\n";
        let mut db = crate::testing::db(&[("main.bd", source)]);
        let mut engine = crate::Engine::new(&mut db).expect("engine");
        let pattern = parse("$a + 1").expect("pattern");
        let hits = search(&mut engine, &pattern, Some("2 * 3")).expect("matches");
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].replacement.as_deref(), Some("(2 * 3)"));
        assert!(hits[0].at.lo > 0);
    }
}
