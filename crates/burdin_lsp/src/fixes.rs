//! The fixes that write code the compiler's suggestions do not: the arms of a `match` (E0006 and the `_` arm), and a
//! derive that a bound or a derive requirement needs (E0141, E0130). `spec/lsp.md`, LS18.

use crate::index::{FileIndex, KEYWORDS, Kind, Tk};
use crate::nav::Resolver;
use crate::server::Server;
use burdin_diag::Diagnostic;
use burdin_syntax::ast::{self, ExprKind, Item, PatternKind};
use burdin_syntax::docs;
use burdin_syntax::walk::{self, Visit};

/// A text edit: the byte range it replaces and the new text.
pub type Edit = (u32, u32, String);

/// A fix with the file it edits, which is not always the file the diagnostic is in.
pub struct Fix {
    pub title: String,
    pub path: String,
    pub edits: Vec<Edit>,
}

// ---- match arms -------------------------------------------------------------------------------------------------

/// One arm of a `match` as the source writes it.
struct ArmSyntax {
    /// The arm from its pattern to the end of its body.
    lo: u32,
    /// Where the body starts.
    body_lo: u32,
    body_hi: u32,
    /// The body is a block (or an `if`, a `match`, ...), which needs no comma after it.
    body_is_block: bool,
    /// `_` or `else`, without a guard.
    catch_all: bool,
    /// The arm is written `else`, not `_`.
    is_else: bool,
}

/// A `match` expression as the source writes it.
struct MatchSyntax {
    lo: u32,
    hi: u32,
    /// The end of the scrutinee, for a `match` with no arms.
    scrutinee_hi: u32,
    arms: Vec<ArmSyntax>,
}

#[derive(Default)]
struct Matches {
    found: Vec<MatchSyntax>,
}

impl Visit for Matches {
    fn expr(&mut self, e: &ast::Expr) {
        if let ExprKind::Match(scrutinee, arms) = &e.kind {
            let arms = arms
                .iter()
                .map(|a| {
                    let wild = a.pattern.as_ref().is_none_or(|p| matches!(p.kind, PatternKind::Wild));
                    ArmSyntax {
                        lo: a.span.lo,
                        body_lo: a.body.span.lo,
                        body_hi: a.body.span.hi,
                        body_is_block: a.body.is_block_like(),
                        catch_all: wild && a.guard.is_none(),
                        is_else: a.pattern.is_none(),
                    }
                })
                .collect();
            self.found.push(MatchSyntax { lo: e.span.lo, hi: e.span.hi, scrutinee_hi: scrutinee.span.hi, arms });
        }
        walk::walk_expr(self, e);
    }
}

/// Every `match` of a file, outer ones before the ones inside them.
fn matches_of(server: &mut Server, i: usize, path: &str) -> Vec<MatchSyntax> {
    let Some(tree) = server.packages[i].db.syntax_tree(path) else { return Vec::new() };
    let mut matches = Matches::default();
    walk::for_each_fn(tree.ast(), &|_| false, &mut |_, decl| matches.block(&decl.body));
    matches.found
}

/// The white space at the start of the line `offset` is on.
fn line_indent(text: &str, offset: u32) -> &str {
    let start = text[..offset as usize].rfind('\n').map_or(0, |n| n + 1);
    let line = &text[start..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// A case with nothing to tell apart: every part of it is a wildcard (`_`, `(_, _)`).
fn is_open(case: &str) -> bool {
    case.chars().all(|c| matches!(c, '_' | '(' | ')' | ',' | ' '))
}

/// "add the missing match arms" for an E0006 at `d`: one `case => todo()` arm for each case the arms leave out,
/// after the last arm (LS18).
pub fn missing_arms(server: &mut Server, i: usize, path: &str, text: &str, d: &Diagnostic) -> Vec<(String, Vec<Edit>)> {
    let Some(m) = matches_of(server, i, path).into_iter().find(|m| m.lo == d.span.lo) else { return Vec::new() };
    let Some(cases) = server.packages[i].current().and_then(|a| a.match_cases(path, m.lo, m.hi)) else {
        return Vec::new();
    };
    if cases.missing.is_empty() || cases.before.len() != m.arms.len() {
        return Vec::new();
    }
    let (at, prefix, indent) = match m.arms.last() {
        Some(last) => {
            // The comma after the last arm stays where it is; without one, an expression body gets it.
            let (at, comma) = if text[last.body_hi as usize..].starts_with(',') {
                (last.body_hi + 1, "")
            } else if last.body_is_block {
                (last.body_hi, "")
            } else {
                (last.body_hi, ",")
            };
            let indent = if text[..last.lo as usize]
                .rfind('\n')
                .is_some_and(|n| text[n + 1..last.lo as usize].trim().is_empty())
            {
                line_indent(text, last.lo).to_string()
            } else {
                format!("{}    ", line_indent(text, m.lo))
            };
            (at, comma, indent)
        }
        None => {
            let open =
                text[m.scrutinee_hi as usize..].find('{').map_or(m.scrutinee_hi, |n| m.scrutinee_hi + n as u32 + 1);
            (open, "", format!("{}    ", line_indent(text, m.lo)))
        }
    };
    let mut new_text = prefix.to_string();
    for case in &cases.missing {
        new_text.push_str(&format!("\n{indent}{case} => todo(),"));
    }
    if m.arms.is_empty() {
        new_text.push_str(&format!("\n{}", line_indent(text, m.lo)));
    }
    // An empty `match x {}` has its closing brace right after the opening one.
    let end = if m.arms.is_empty() { text[at as usize..].find('}').map_or(at, |n| at + n as u32) } else { at };
    vec![("add the missing match arms".to_string(), vec![(at, end, new_text)])]
}

/// "expand `_` into the remaining cases" for a `_` (or `else`) arm the range touches: one arm for each case the
/// wildcard stands for, each with a copy of the arm's body (LS18).
pub fn expand_wildcard(
    server: &mut Server,
    i: usize,
    path: &str,
    text: &str,
    lo: u32,
    hi: u32,
) -> Vec<(String, Vec<Edit>)> {
    let mut out = Vec::new();
    for m in matches_of(server, i, path) {
        for (k, arm) in m.arms.iter().enumerate() {
            // The cursor is on the pattern or the `=>`, not inside the body (which can be a match of its own).
            if !arm.catch_all || lo > arm.body_lo || hi < arm.lo {
                continue;
            }
            let Some(cases) = server.packages[i].current().and_then(|a| a.match_cases(path, m.lo, m.hi)) else {
                continue;
            };
            let Some(covered) = cases.before.get(k).filter(|c| cases.before.len() == m.arms.len() && !c.is_empty())
            else {
                continue;
            };
            if covered.iter().any(|c| is_open(c)) {
                continue;
            }
            let body = &text[arm.body_lo as usize..arm.body_hi as usize];
            let separator = format!("{}\n{}", if arm.body_is_block { "" } else { "," }, line_indent(text, arm.lo));
            let arms: Vec<String> = covered.iter().map(|c| format!("{c} => {body}")).collect();
            let name = if arm.is_else { "else" } else { "_" };
            out.push((
                format!("expand `{name}` into the remaining cases"),
                vec![(arm.lo, arm.body_hi, arms.join(&separator))],
            ));
        }
    }
    out
}

// ---- derives ----------------------------------------------------------------------------------------------------

/// The interfaces a declaration can derive (`spec/derives.md`).
const DERIVABLE: [&str; 9] = ["Copy", "Clone", "Eq", "Ord", "Hash", "Default", "Debug", "Serialize", "Deserialize"];

/// The interfaces a derive needs beside it, written out with it: `Ord` with `Eq`, `Copy` with `Clone`.
fn with_prerequisites(interface: &str) -> Vec<&'static str> {
    match interface {
        "Ord" => vec!["Eq", "Ord"],
        "Copy" => vec!["Clone", "Copy"],
        other => DERIVABLE.iter().copied().filter(|d| *d == other).collect(),
    }
}

/// "derive `Clone` for `Point`": for an E0141 whose message says `` `Point` does not implement `Clone` ``, and for an
/// E0130 that names the field's type (`` ... but this one is `Point` ``). The type is one the entry package declares;
/// the derive goes into its `: A, B` list, which is made when it has none (LS18).
pub fn missing_derive(server: &mut Server, i: usize, d: &Diagnostic) -> Vec<Fix> {
    let parts: Vec<&str> = d.message.split('`').collect();
    let (ty, interface) = match d.code.0 {
        "E0141" if parts.get(2).is_some_and(|p| p.trim() == "does not implement") && parts.len() > 3 => {
            (parts[1], parts[3])
        }
        "E0130" if parts.len() > 5 && parts[2].starts_with(" requires every field") => (parts[5], parts[1]),
        _ => return Vec::new(),
    };
    let needed = with_prerequisites(interface);
    if needed.is_empty() || ty.contains(['(', '[', '&']) {
        return Vec::new();
    }
    let name = ty.split('<').next().unwrap_or(ty).rsplit('.').next().unwrap_or(ty).to_string();
    let Some(path) = declared_type(server, i, &name) else { return Vec::new() };
    let path = &path;
    let Some(tree) = server.packages[i].db.syntax_tree(path) else { return Vec::new() };
    let text = tree.text();
    let found = tree.ast().items.iter().find_map(|item| match item {
        Item::Struct(s) if s.name.name == name => Some((s.name.span, &s.generics, &s.derives)),
        Item::Enum(e) if e.name.name == name => Some((e.name.span, &e.generics, &e.derives)),
        _ => None,
    });
    let Some((name_span, generics, derives)) = found else { return Vec::new() };
    let written: Vec<&str> = derives.iter().map(|t| text[t.span.lo as usize..t.span.hi as usize].trim()).collect();
    let add: Vec<&str> = needed.into_iter().filter(|n| !written.contains(n)).collect();
    if add.is_empty() {
        return Vec::new();
    }
    let edit = match derives.last() {
        Some(last) => (last.span.hi, last.span.hi, format!(", {}", add.join(", "))),
        None => {
            // After the name and its type parameters.
            let mut at = name_span.hi;
            if !generics.is_empty() {
                at = generics_end(text, name_span.hi);
            }
            (at, at, format!(": {}", add.join(", ")))
        }
    };
    let title = format!("derive `{}` for `{name}`", add.join(", "));
    vec![Fix { title, path: path.clone(), edits: vec![edit] }]
}

/// The file of the one struct or enum called `name` that the entry package declares.
fn declared_type(server: &mut Server, i: usize, name: &str) -> Option<String> {
    let mut resolver = Resolver::new(&mut server.packages[i]);
    let declared: Vec<String> = resolver
        .all_items()
        .iter()
        .filter(|(path, item)| {
            item.name == name
                && item.parent.is_none()
                && matches!(item.kind, Kind::Struct | Kind::Enum)
                && resolver
                    .p
                    .package_of(path)
                    .is_some_and(|p| resolver.p.packages[p].is_entry && !resolver.p.packages[p].embedded)
        })
        .map(|(path, _)| path.clone())
        .collect();
    match declared.as_slice() {
        [path] => Some(path.clone()),
        _ => None,
    }
}

/// The offset just after the `>` that closes the type parameter list starting at `from` (the end of the name).
fn generics_end(text: &str, from: u32) -> u32 {
    let mut depth = 0;
    for (n, c) in text[from as usize..].char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return from + n as u32 + 1;
                }
            }
            _ => {}
        }
    }
    from
}

// ---- missing struct fields ---------------------------------------------------------------------------------------

/// A struct literal as the source writes it: `Point { x: 1 }` or `.{ x: 1 }`.
struct LiteralSyntax {
    lo: u32,
    hi: u32,
    /// The start of the last field and the end of the last field's value; `None` for a literal with no fields.
    last: Option<(u32, u32)>,
    has_base: bool,
}

#[derive(Default)]
struct Literals {
    found: Vec<LiteralSyntax>,
}

impl Literals {
    fn add(&mut self, e: &ast::Expr, fields: &[ast::FieldInit], base: &Option<Box<ast::Expr>>) {
        let last = fields.last().map(|f| (f.name.span.lo, f.value.as_ref().map_or(f.name.span.hi, |v| v.span.hi)));
        self.found.push(LiteralSyntax { lo: e.span.lo, hi: e.span.hi, last, has_base: base.is_some() });
    }
}

impl Visit for Literals {
    fn expr(&mut self, e: &ast::Expr) {
        match &e.kind {
            ExprKind::StructLit(_, _, fields, base) | ExprKind::TargetStruct(fields, base) => self.add(e, fields, base),
            _ => {}
        }
        walk::walk_expr(self, e);
    }
}

/// "add the missing fields" for an E0003 whose message says `` `Point` is missing the field(s) `y` ``: each missing
/// field is written `name: todo()` after the last one, in declaration order (LS18).
pub fn missing_fields(
    server: &mut Server,
    i: usize,
    path: &str,
    text: &str,
    d: &Diagnostic,
) -> Vec<(String, Vec<Edit>)> {
    let parts: Vec<&str> = d.message.split('`').collect();
    if !parts.get(2).is_some_and(|p| p.starts_with(" is missing the field(s)")) {
        return Vec::new();
    }
    let missing: Vec<String> = parts.iter().skip(3).step_by(2).map(|f| format!("{f}: todo()")).collect();
    let Some(tree) = server.packages[i].db.syntax_tree(path) else { return Vec::new() };
    let mut literals = Literals::default();
    walk::for_each_fn(tree.ast(), &|_| false, &mut |_, decl| literals.block(&decl.body));
    let Some(lit) = literals.found.iter().find(|l| l.lo == d.span.lo && l.hi == d.span.hi && !l.has_base) else {
        return Vec::new();
    };
    let edit = match lit.last {
        None => {
            let Some(open) = text[lit.lo as usize..lit.hi as usize].find('{') else { return Vec::new() };
            (lit.lo + open as u32 + 1, lit.hi - 1, format!(" {} ", missing.join(", ")))
        }
        Some((first, end)) => {
            let rest = &text[end as usize..lit.hi as usize];
            let comma = rest.trim_start().starts_with(',');
            let after_comma = end + rest.find(',').map_or(0, |n| n as u32 + 1);
            if text[lit.lo as usize..lit.hi as usize].contains('\n') {
                let indent = line_indent(text, first);
                let list = missing.join(&format!(",\n{indent}"));
                if comma {
                    (after_comma, after_comma, format!("\n{indent}{list},"))
                } else {
                    (end, end, format!(",\n{indent}{list}"))
                }
            } else if comma {
                (after_comma, after_comma, format!(" {},", missing.join(", ")))
            } else {
                (end, end, format!(", {}", missing.join(", ")))
            }
        }
    };
    vec![("add the missing fields".to_string(), vec![edit])]
}

// ---- generated functions and methods -----------------------------------------------------------------------------

/// The argument list of the call whose callee name ends at `name_hi`: the byte range of each argument and where the
/// list ends. `None` when no `(` follows the name.
fn call_arguments(ix: &FileIndex, name_hi: u32) -> Option<(Vec<(u32, u32)>, u32)> {
    let name = ix.tokens.iter().position(|t| t.hi == name_hi)?;
    if ix.tokens.get(name + 1)?.kind != Tk::LParen {
        return None;
    }
    let mut depth = 0;
    let mut args: Vec<(u32, u32)> = Vec::new();
    let mut start: Option<u32> = None;
    let mut last = 0;
    for (k, t) in ix.tokens.iter().enumerate().skip(name + 1) {
        match t.kind {
            Tk::LParen | Tk::LBracket | Tk::LBrace => depth += 1,
            Tk::RParen | Tk::RBracket | Tk::RBrace => depth -= 1,
            Tk::Comma if depth == 1 => {
                args.extend(start.take().map(|lo| (lo, last)));
                continue;
            }
            _ => {}
        }
        if depth == 0 {
            args.extend(start.map(|lo| (lo, last)));
            return Some((args, t.hi));
        }
        if k > name + 1 {
            start.get_or_insert(t.lo);
            last = t.hi;
        }
    }
    None
}

/// Where the receiver of the method call named by token `name` starts: the chain of names, calls, and indexes before
/// the dot.
fn receiver_start(ix: &FileIndex, name: usize) -> Option<u32> {
    if name < 2 || ix.tokens[name - 1].kind != Tk::Dot {
        return None;
    }
    let mut k = name - 2;
    loop {
        if matches!(ix.tokens[k].kind, Tk::RParen | Tk::RBracket) {
            let mut depth = 0;
            loop {
                match ix.tokens[k].kind {
                    Tk::RParen | Tk::RBracket => depth += 1,
                    Tk::LParen | Tk::LBracket => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    break;
                }
                k = k.checked_sub(1)?;
            }
            if k > 0 && ix.tokens[k - 1].ident().is_some() {
                k -= 1;
            }
        } else if !matches!(ix.tokens[k].kind, Tk::Ident(_) | Tk::SelfValue | Tk::SelfType | Tk::Num | Tk::Str) {
            return None;
        }
        if k >= 2 && ix.tokens[k - 1].kind == Tk::Dot {
            k -= 2;
        } else {
            return Some(ix.tokens[k].lo);
        }
    }
}

/// The text split at the commas that are not inside brackets.
fn split_top(text: &str) -> Vec<&str> {
    let (mut depth, mut start, mut out) = (0i32, 0, Vec::new());
    for (n, c) in text.char_indices() {
        match c {
            '(' | '[' | '<' | '{' => depth += 1,
            ')' | ']' | '>' | '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&text[start..n]);
                start = n + 1;
            }
            _ => {}
        }
    }
    out.push(&text[start..]);
    out
}

fn is_plain_name(word: &str) -> bool {
    !word.is_empty()
        && !word.starts_with(|c: char| c.is_ascii_digit())
        && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !KEYWORDS.contains(&word)
}

/// The parameters a call's arguments ask for (`r: Rect`, `mut total: i32`), typed by the checker. The call is
/// analyzed as the tuple of its arguments (`area(r, 2)` as `(r, 2, ())`), which has a type even when the callee is
/// unknown. `None` when an argument has no type; the document's text is put back before the answer.
fn call_parameters(
    server: &mut Server,
    i: usize,
    path: &str,
    text: &str,
    ix: &FileIndex,
    call_lo: u32,
    name_hi: u32,
) -> Option<Vec<String>> {
    let (args, end) = call_arguments(ix, name_hi)?;
    if args.is_empty() {
        return Some(Vec::new());
    }
    // `mut x` and `name: x` are the call's own syntax; the probe keeps the value only.
    let mut values = Vec::new();
    let mut written = Vec::new();
    for &(lo, hi) in &args {
        let arg = &text[lo as usize..hi as usize];
        let (marker, rest) = match arg.strip_prefix("mut ") {
            Some(rest) => (true, rest.trim_start()),
            None => (false, arg),
        };
        let (label, value) = match rest.split_once(':') {
            Some((label, value)) if is_plain_name(label.trim()) => (Some(label.trim()), value.trim()),
            _ => (None, rest),
        };
        values.push(value.to_string());
        written.push((marker, label.map(str::to_string), value.to_string()));
    }
    let before = text[..call_lo as usize].trim_end_matches([' ', '\t']);
    let statement = before.is_empty() || before.ends_with(['\n', '{', ';']);
    let head = if statement { "let __probe = " } else { "" };
    let probe = format!("{}{head}({}, ()){}", &text[..call_lo as usize], values.join(", "), &text[end as usize..]);
    let tuple = crate::edit::expected_type_at(server, i, path, text, &probe, call_lo + head.len() as u32).ok()??;
    let inner = tuple.strip_prefix('(')?.strip_suffix(')')?;
    let mut types = split_top(inner);
    types.pop();
    if types.len() != written.len() || types.iter().any(|t| t.contains("{error}")) {
        return None;
    }
    let mut used: Vec<String> = Vec::new();
    let mut params = Vec::new();
    for (k, ((marker, label, value), ty)) in written.into_iter().zip(types).enumerate() {
        let mut name =
            label.unwrap_or_else(|| if is_plain_name(&value) { value.clone() } else { format!("arg{}", k + 1) });
        while used.contains(&name) {
            name.push('_');
        }
        used.push(name.clone());
        params.push(format!("{}{name}: {}", if marker { "mut " } else { "" }, ty.trim()));
    }
    Some(params)
}

/// `: T` when the call starting at `call_lo` is the initializer of `let x: T = ...`, which is the type the new
/// function must return for the program to check; nothing otherwise.
fn expected_return(text: &str, ix: &FileIndex, call_lo: u32) -> String {
    let Some(eq) = ix.tokens.iter().rposition(|t| t.hi <= call_lo) else { return String::new() };
    if &text[ix.tokens[eq].lo as usize..ix.tokens[eq].hi as usize] != "=" {
        return String::new();
    }
    let Some(let_at) = ix.tokens[..eq].iter().rposition(|t| t.kind == Tk::Keyword("let")) else {
        return String::new();
    };
    let mut colon = let_at + 2;
    if ix.tokens.get(let_at + 1).is_some_and(|t| t.kind == Tk::Keyword("mut")) {
        colon += 1;
    }
    match ix.tokens.get(colon) {
        Some(t) if t.kind == Tk::Colon && colon < eq && eq - let_at < 40 => {
            format!(": {}", text[t.hi as usize..ix.tokens[eq].lo as usize].trim())
        }
        _ => String::new(),
    }
}

/// "create function `area`" for an E0002 `` unknown function `area` ``: the function goes after the one that calls
/// it, with the arguments' types as parameters and `todo()` as the body (LS18).
pub fn missing_function(
    server: &mut Server,
    i: usize,
    path: &str,
    text: &str,
    d: &Diagnostic,
) -> Vec<(String, Vec<Edit>)> {
    let Some(name) = d.message.strip_prefix("unknown function `").and_then(|m| m.strip_suffix('`')) else {
        return Vec::new();
    };
    let Some(ix) = server.packages[i].index(path) else { return Vec::new() };
    if text[..d.span.lo as usize].ends_with('.') {
        return Vec::new();
    }
    // A function another module declares needs its `use` (the compiler's suggestion), not a second function.
    let declared_elsewhere = Resolver::new(&mut server.packages[i]).all_items().iter().any(|(file, item)| {
        item.kind == Kind::Function && item.parent.is_none() && item.name == name && !file.starts_with("<std>/")
    });
    if declared_elsewhere {
        return Vec::new();
    }
    let Some(around) = ix
        .items
        .iter()
        .filter(|it| it.parent.is_none() && it.span.0 <= d.span.lo && d.span.lo <= it.span.1)
        .min_by_key(|it| it.span.1 - it.span.0)
    else {
        return Vec::new();
    };
    let Some(params) = call_parameters(server, i, path, text, &ix, d.span.lo, d.span.hi) else { return Vec::new() };
    let ret = expected_return(text, &ix, d.span.lo);
    let stub = format!("\n\nfn {name}({}){ret} {{\n    todo()\n}}", params.join(", "));
    vec![(format!("create function `{name}`"), vec![(around.span.1, around.span.1, stub)])]
}

/// "create method `length` on `Point`" for an E0002 `` no method `length` on type `Point` ``, when the entry package
/// declares `Point`: the method goes into the type's own `Point { }` block, which is made when there is none (LS18).
pub fn missing_method(server: &mut Server, i: usize, path: &str, text: &str, d: &Diagnostic) -> Vec<Fix> {
    let parts: Vec<&str> = d.message.split('`').collect();
    if !(parts.len() > 3 && parts[0] == "no method " && parts[2] == " on type ") {
        return Vec::new();
    }
    let name = parts[1];
    let ty = crate::nav::base_type(parts[3]).rsplit('.').next().unwrap_or_default().to_string();
    let Some(ix) = server.packages[i].index(path) else { return Vec::new() };
    let Some(name_token) = ix.tokens.iter().position(|t| t.lo == d.span.lo) else { return Vec::new() };
    let Some(call_lo) = receiver_start(&ix, name_token) else { return Vec::new() };
    let Some(type_path) = declared_type(server, i, &ty) else { return Vec::new() };
    let Some(params) = call_parameters(server, i, path, text, &ix, call_lo, d.span.hi) else { return Vec::new() };
    let ret = expected_return(text, &ix, call_lo);
    let params: String = params.iter().map(|p| format!(", {p}")).collect();
    let method = format!("fn {name}(self{params}){ret} {{\n        todo()\n    }}\n");
    let Some(tree) = server.packages[i].db.syntax_tree(&type_path) else { return Vec::new() };
    let type_text = tree.text();
    let block = tree.ast().items.iter().find_map(|item| match item {
        Item::Impl(b) if b.interface.is_none() && docs::type_name(&b.ty) == Some(ty.as_str()) => Some(b),
        _ => None,
    });
    let edit = match block {
        Some(b) => {
            let close = b.span.hi - 1;
            let line_start = type_text[..close as usize].rfind('\n').map_or(0, |n| n + 1) as u32;
            let blank = if b.methods.is_empty() { "" } else { "\n" };
            if type_text[line_start as usize..close as usize].trim().is_empty() {
                (line_start, line_start, format!("{blank}    {method}"))
            } else {
                (close, close, format!("\n{blank}    {method}"))
            }
        }
        None => {
            let Some((name_span, end, generics)) = tree.ast().items.iter().find_map(|item| match item {
                Item::Struct(s) if s.name.name == ty => Some((s.name.span, s.span.hi, !s.generics.is_empty())),
                Item::Enum(e) if e.name.name == ty => Some((e.name.span, e.span.hi, !e.generics.is_empty())),
                _ => None,
            }) else {
                return Vec::new();
            };
            let header_end = if generics { generics_end(type_text, name_span.hi) } else { name_span.hi };
            let header = &type_text[name_span.lo as usize..header_end as usize];
            (end, end, format!("\n\n{header} {{\n    {method}}}"))
        }
    };
    vec![Fix { title: format!("create method `{name}` on `{ty}`"), path: type_path, edits: vec![edit] }]
}

// ---- implementing an interface ------------------------------------------------------------------------------------

/// Whether a declaration's `: A, B` list names `interface`.
fn derives_name(derives: &[ast::Type], interface: &str) -> bool {
    derives.iter().any(|t| docs::type_name(t) == Some(interface))
}

/// "implement `Shape` for `Rect`" for the type the range touches: one action for each interface of the entry package
/// that the type does not implement and that has methods without a default body, writing the methods with `todo()`
/// bodies after the type (LS18). An interface with type parameters, associated types, or requirements is left out: its
/// impl needs more than the signatures.
pub fn implement_interfaces(server: &mut Server, i: usize, path: &str, lo: u32, hi: u32) -> Vec<Fix> {
    let mut r = Resolver::new(&mut server.packages[i]);
    let in_entry = |r: &Resolver, file: &str| {
        r.p.package_of(file).is_some_and(|p| r.p.packages[p].is_entry && !r.p.packages[p].embedded)
    };
    let Some(file) = r.index(path) else { return Vec::new() };
    let Some(ty) = file.items.iter().find(|it| {
        it.parent.is_none()
            && matches!(it.kind, Kind::Struct | Kind::Enum)
            && it.name_span.0 <= hi
            && lo <= it.name_span.1
    }) else {
        return Vec::new();
    };
    let Some(tree) = r.p.db.syntax_tree(path) else { return Vec::new() };
    if !in_entry(&r, path) {
        return Vec::new();
    }
    let text = tree.text();
    let (generics, derives) = tree
        .ast()
        .items
        .iter()
        .find_map(|item| match item {
            Item::Struct(s) if s.name.name == ty.name => Some((!s.generics.is_empty(), s.derives.clone())),
            Item::Enum(e) if e.name.name == ty.name => Some((!e.generics.is_empty(), e.derives.clone())),
            _ => None,
        })
        .unwrap_or_default();
    let header_end = if generics { generics_end(text, ty.name_span.1) } else { ty.name_span.1 };
    let header = text[ty.name_span.0 as usize..header_end as usize].to_string();
    let all = r.all_items();
    let mut out = Vec::new();
    let interfaces: Vec<&(String, crate::index::Item)> =
        all.iter().filter(|(p, it)| it.kind == Kind::Interface && it.parent.is_none() && in_entry(&r, p)).collect();
    for (iface_path, iface) in interfaces {
        let implemented = all.iter().any(|(_, it)| {
            it.kind == Kind::Impl
                && it.owner.as_deref() == Some(ty.name.as_str())
                && it.interface.as_deref() == Some(iface.name.as_str())
        });
        let Some(iface_ix) = r.index(iface_path) else { continue };
        let index = iface_ix.items.iter().position(|it| it.kind == Kind::Interface && it.span == iface.span);
        // What the interface declares is in its syntax: a method with no body must be written, and an interface with
        // type parameters or associated types needs more than the signatures.
        let Some(iface_tree) = r.p.db.syntax_tree(iface_path) else { continue };
        let Some(decl) = iface_tree.ast().items.iter().find_map(|item| match item {
            Item::Interface(d) if d.name.name == iface.name => Some(d),
            _ => None,
        }) else {
            continue;
        };
        let without_body: Vec<&str> =
            decl.methods.iter().filter(|m| !m.has_body).map(|m| m.decl.name.name.as_str()).collect();
        let required: Vec<&crate::index::Item> = iface_ix
            .items
            .iter()
            .filter(|m| m.kind == Kind::InterfaceMethod && m.parent == index && without_body.contains(&m.name.as_str()))
            .collect();
        let plain = decl.generics.is_empty() && decl.associated.is_empty();
        if implemented
            || !plain
            || required.is_empty()
            || !iface.requires.is_empty()
            || derives_name(&derives, &iface.name)
        {
            continue;
        }
        let methods: Vec<String> =
            required.iter().map(|m| format!("    {} {{\n        todo()\n    }}", m.detail)).collect();
        let block = format!("\n\n{header}: {} {{\n{}\n}}", iface.name, methods.join("\n\n"));
        let mut edits = vec![(ty.span.1, ty.span.1, block)];
        let imported = file.uses.iter().any(|u| {
            u.items.iter().any(|(n, _)| *n == iface.name) || u.path.last().is_some_and(|(n, _)| *n == iface.name)
        });
        if iface_path != path && !imported {
            // Another file's interface needs a `use`; the root module has no path to write.
            let module = r.p.module(iface_path).map(|m| m.module.join(".")).unwrap_or_default();
            if module.is_empty() {
                continue;
            }
            let (at, new_text) = crate::edit::use_at(text, &file, &format!("{module}.{}", iface.name));
            edits.push((at, at, new_text));
        }
        out.push(Fix { title: format!("implement `{}` for `{}`", iface.name, ty.name), path: path.to_string(), edits });
    }
    out
}
