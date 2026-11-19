//! The public API for tools built on the compiler: the formatter, the linter, the documentation generator, the
//! language server, and the REPL (`spec/query.md`, QL7). Everything here is plain data (strings, byte offsets, and
//! paths), so a tool depends on this crate's types and never on the typed tree or the checker's internals.
//!
//! Positions are byte offsets into the text of a file, as [`Db::file_text`] returns it; [`Db::line_col`] converts one
//! to a 1-based line and column.

use crate::db::{Analysis, Db, Parsed};
use burdin_diag::{Diagnostic, FileId, Span};
use burdin_syntax::ast;
use burdin_syntax::lexer;
use burdin_syntax::token::{Token, TokenKind};
use burdin_tir::visit::{self, Visit};
use burdin_tir::{ConstId, Expr, ExprKind, FuncId, Function, LocalId, LocalKind, Pat, Program, Stmt, Ty};
use std::rc::Rc;

/// A stretch of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// The path of the file, as [`Db::file_paths`] lists it.
    pub path: String,
    pub lo: u32,
    pub hi: u32,
}

/// What a piece of source text between two tokens is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriviaKind {
    Whitespace,
    /// `// ...`, to the end of the line (the line break itself is white space).
    LineComment,
    /// `/// ...`: a documentation comment.
    DocComment,
    /// `/* ... */`.
    BlockComment,
}

/// Source text that is not a token: white space and comments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trivia {
    pub kind: TriviaKind,
    pub lo: u32,
    pub hi: u32,
}

/// The syntax of one file, lossless: the tree the compiler checks, the tokens, and the white space and comments
/// between them. Tokens and trivia are in source order and together cover every byte of the text, so a formatter can
/// put the comments back where they were.
pub struct SyntaxTree {
    pub path: String,
    parsed: Rc<Parsed>,
    /// The tokens, without the final end-of-input token.
    pub tokens: Vec<Token>,
    pub trivia: Vec<Trivia>,
}

impl SyntaxTree {
    pub fn text(&self) -> &str {
        &self.parsed.text
    }

    /// The tree the compiler checks. Spans in it are byte offsets into [`SyntaxTree::text`].
    pub fn ast(&self) -> &ast::File {
        &self.parsed.ast
    }

    /// The syntax errors of the file. Empty when it parses.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.parsed.diags
    }

    /// The text of a span of this file.
    pub fn slice(&self, lo: u32, hi: u32) -> &str {
        self.text().get(lo as usize..hi as usize).unwrap_or("")
    }

    /// The text rebuilt from the tokens and the trivia, in source order. It is [`SyntaxTree::text`] again, which is
    /// what lossless means; text the lexer rejected is kept with the white space.
    pub fn rebuild(&self) -> String {
        let mut spans: Vec<(u32, u32)> = self.tokens.iter().map(|t| (t.span.lo, t.span.hi)).collect();
        spans.extend(self.trivia.iter().map(|t| (t.lo, t.hi)));
        spans.sort_unstable();
        spans.iter().map(|&(lo, hi)| self.slice(lo, hi)).collect()
    }
}

/// What a name refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefinitionKind {
    Local,
    Parameter,
    Function,
    Constant,
}

/// What the name at a position resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    /// The name as written at the position.
    pub name: String,
    pub kind: DefinitionKind,
    /// Where the name was written.
    pub at: Location,
    /// Where it is declared: the binding of a local, the declaration of a function, a constant's name. `None` for a
    /// declaration the compiler generated.
    pub definition: Option<Location>,
    /// The type of the expression, or the signature of the function.
    pub ty: String,
}

/// An expression and its type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Typed {
    pub ty: String,
    pub at: Location,
}

/// How a parameter is passed (`spec/memory-rules.md`, MV1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Convention {
    /// A read-only borrow: no marker.
    Read,
    /// `mut`: borrowed exclusively, and written.
    Mut,
    /// `own`: ownership moves to the callee.
    Own,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamInfo {
    /// The parameter's name; `self` for a receiver.
    pub name: String,
    pub ty: String,
    pub convention: Convention,
    /// A default value is written in the declaration (`spec/arguments.md`, AG1).
    pub has_default: bool,
}

/// The signature of a function, method, or constant-like item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemSignature {
    /// The qualified name the compiler uses (`math.add`, `Rect.new`). An instantiation of a generic function shows
    /// its arguments (`max<i32>`); the declaration itself shows the type parameters (`max<T>`).
    pub name: String,
    pub params: Vec<ParamInfo>,
    pub ret: String,
    /// The signature as it is written: `fn add(a: i32, b: i32): i32`.
    pub display: String,
    /// Where the item is declared.
    pub location: Option<Location>,
}

/// The ownership facts the move and borrow checks computed for one function (`spec/memory-rules.md`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ownership {
    pub function: String,
    /// Every place the function moves out of: a local and the field path from it.
    pub moves: Vec<MoveEvent>,
    /// The locals the function owns, which are dropped at the end of their scope unless moved out first.
    pub owned: Vec<OwnedLocal>,
    /// The parameters a second-class return value derives from (RT1, RT3), by index; empty when the result is
    /// static.
    pub return_origins: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveEvent {
    pub local: String,
    /// Field indices from the local to the moved place; empty is the whole local. [`burdin_tir::MOVED_DEREF`] steps
    /// walk through a `Heap<T>`.
    pub path: Vec<usize>,
    /// The checker proved the local is already moved out on every path that reaches its scope's end, so it needs no
    /// drop and no drop flag (`spec/memory-rules.md`, MV9).
    pub always_moved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedLocal {
    pub name: String,
    pub ty: String,
    /// A parameter taken with `own`, rather than a `let` or a pattern binding.
    pub is_parameter: bool,
}

/// One statement of a function body, as a tool sees it. The REPL reads these to decide which statement an input
/// appended and whether its value is printed (`spec/repl.md`, RP5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StmtInfo {
    pub at: Location,
    pub kind: StmtKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StmtKind {
    /// `let pattern = ...`: the names the pattern binds, with their types, in pattern order.
    Let(Vec<(String, String)>),
    /// A bare expression statement: its type, and whether that type is `()` or `!`.
    Expr { ty: String, unit: bool, never: bool },
    /// Anything else the tree does not name (currently nothing: a body is lets and expressions).
    Other,
}

/// What one expression does to the locals it mentions, as a tool sees it (`spec/repl.md`, `:ownership`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExprFacts {
    /// The type of the whole expression.
    pub ty: String,
    /// Every local it mentions, in source order, with how the mention uses the value. A local mentioned more
    /// than once appears once, with every use merged.
    pub locals: Vec<LocalUse>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalUse {
    pub name: String,
    pub ty: String,
    /// `&local` somewhere in the expression.
    pub borrowed: bool,
    /// `&mut local` somewhere in the expression.
    pub borrowed_mut: bool,
}

/// A field or a method of one type, for completion after `.`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    /// A field of the type itself (`false` is a method).
    pub is_field: bool,
    /// The field's type, or `fn add(a: i32, b: i32): i32` for a method.
    pub detail: String,
}

impl Db {
    /// The syntax of a file with its comments and white space. Parses the file if its text changed.
    pub fn syntax_tree(&mut self, path: &str) -> Option<SyntaxTree> {
        let parsed = self.syntax(path)?;
        let id = self.file_id(path)?;
        let mut diags = burdin_diag::Diagnostics::default();
        let lexed = lexer::lex(id, &parsed.text, &mut diags);
        let mut tokens = lexed.tokens;
        if matches!(tokens.last(), Some(t) if t.kind == TokenKind::Eof) {
            tokens.pop();
        }
        let trivia = trivia_between(&parsed.text, &tokens);
        Some(SyntaxTree { path: path.to_string(), parsed, tokens, trivia })
    }

    /// The 1-based line and column of a byte offset of a file.
    pub fn line_col(&self, path: &str, offset: u32) -> Option<(u32, u32)> {
        let id = self.file_id(path)?;
        Some(self.sources().get(id).line_col(offset))
    }
}

/// The white space and comments in the gaps between tokens.
fn trivia_between(text: &str, tokens: &[Token]) -> Vec<Trivia> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let mut gaps: Vec<(usize, usize)> = Vec::new();
    for t in tokens {
        gaps.push((at, t.span.lo as usize));
        at = t.span.hi as usize;
    }
    gaps.push((at, text.len()));
    for (lo, hi) in gaps {
        let (lo, hi) = (lo.min(text.len()), hi.min(text.len()));
        let gap = &text[lo..hi];
        let mut i = 0usize;
        while i < gap.len() {
            let rest = &gap[i..];
            let (kind, len) = if rest.starts_with("//") {
                let len = rest.find('\n').unwrap_or(rest.len());
                let kind = if rest.starts_with("///") && !rest.starts_with("////") {
                    TriviaKind::DocComment
                } else {
                    TriviaKind::LineComment
                };
                (kind, len)
            } else if let Some(after) = rest.strip_prefix("/*") {
                let len = after.find("*/").map_or(rest.len(), |e| e + 4);
                (TriviaKind::BlockComment, len)
            } else {
                let len = rest.find(|c: char| !matches!(c, ' ' | '\t' | '\r' | '\n')).unwrap_or(rest.len());
                (TriviaKind::Whitespace, len)
            };
            // A byte the lexer rejected is neither a token nor a comment; it stays with the white space, and taking
            // at least one byte guarantees progress.
            let len = len.max(1);
            out.push(Trivia { kind, lo: (lo + i) as u32, hi: (lo + i + len) as u32 });
            i += len;
        }
    }
    out
}

/// The innermost expression that contains a position, and what it stands for.
struct Found {
    span: Span,
    ty: Ty,
    what: What,
}

enum What {
    Local { name: String, declared: Span, is_param: bool },
    Func(FuncId),
    Const(ConstId),
    Other,
}

struct Finder<'a> {
    func: &'a Function,
    file: FileId,
    offset: u32,
    best: Option<Found>,
}

impl Visit for Finder<'_> {
    fn expr(&mut self, e: &Expr) {
        let s = e.span;
        let inside = s.file == self.file && s.lo <= self.offset && self.offset < s.hi;
        if inside && self.best.as_ref().is_none_or(|b| s.hi - s.lo <= b.span.hi - b.span.lo) {
            let what = match &e.kind {
                ExprKind::Local(LocalId(i)) => match self.func.locals.get(*i) {
                    Some(local) => What::Local {
                        name: local.name.clone(),
                        declared: local.span,
                        is_param: matches!(local.kind, LocalKind::BorrowedParam | LocalKind::OwnParam),
                    },
                    None => What::Other,
                },
                ExprKind::Call(id, _) => What::Func(*id),
                ExprKind::Const(id) => What::Const(*id),
                _ => What::Other,
            };
            self.best = Some(Found { span: s, ty: e.ty.clone(), what });
        }
        visit::walk_expr(self, e);
    }
}

impl Analysis {
    /// The path of every file of the program, in module order.
    pub fn file_paths(&self) -> &[String] {
        &self.files
    }

    fn file_id(&self, path: &str) -> Option<FileId> {
        self.files.iter().position(|p| p == path).map(|i| FileId(i as u32))
    }

    fn location(&self, span: Span) -> Location {
        Location { path: self.files.get(span.file.0 as usize).cloned().unwrap_or_default(), lo: span.lo, hi: span.hi }
    }

    /// The diagnostics that point into one file.
    pub fn diagnostics_of(&self, path: &str) -> Vec<&Diagnostic> {
        let Some(file) = self.file_id(path) else { return Vec::new() };
        self.diagnostics.iter().filter(|d| d.span.file == file).collect()
    }

    /// The innermost expression that contains a byte offset of a file, with its type.
    pub fn type_at(&self, path: &str, offset: u32) -> Option<Typed> {
        let program = self.program.as_ref()?;
        let (found, _) = self.found_at(program, path, offset)?;
        Some(Typed { ty: type_text(&found.ty), at: self.location(found.span) })
    }

    /// What the name at a byte offset of a file resolves to: a local or parameter, a function, or a constant.
    pub fn resolve_at(&self, path: &str, offset: u32) -> Option<Resolution> {
        let program = self.program.as_ref()?;
        let (found, _) = self.found_at(program, path, offset)?;
        let at = self.location(found.span);
        let (name, kind, definition, ty) = match &found.what {
            What::Local { name, declared, is_param } => (
                name.clone(),
                if *is_param { DefinitionKind::Parameter } else { DefinitionKind::Local },
                (declared.lo < declared.hi).then(|| self.location(*declared)),
                type_text(&found.ty),
            ),
            What::Func(id) => {
                let function = program.functions.get(id.0)?;
                let name = source_text(function.name.split('<').next().unwrap_or(&function.name));
                let definition = (function.span.lo < function.span.hi).then(|| self.location(function.span));
                let ty = signature_display(&function.name, program.signatures.get(id.0)?);
                (name, DefinitionKind::Function, definition, ty)
            }
            What::Const(id) => {
                let constant = program.consts.get(id.0)?;
                let definition = (constant.span.lo < constant.span.hi).then(|| self.location(constant.span));
                (source_text(&constant.name), DefinitionKind::Constant, definition, type_text(&found.ty))
            }
            What::Other => return None,
        };
        Some(Resolution { name, kind, at, definition, ty })
    }

    /// The signature of the function or method whose declaration contains a byte offset of a file.
    pub fn signature_of(&self, path: &str, offset: u32) -> Option<ItemSignature> {
        let program = self.program.as_ref()?;
        let file = self.file_id(path)?;
        // A generic function has one symbolic declaration and one function per instantiation, all sharing the
        // declaration's span; the declaration is the one that describes the item as written.
        let mut best: Option<usize> = None;
        for (index, function) in program.functions.iter().enumerate() {
            let s = function.span;
            if s.file != file || !(s.lo <= offset && offset < s.hi) {
                continue;
            }
            let better = match best {
                None => true,
                Some(b) => {
                    let old = &program.functions[b];
                    (function.symbolic && !old.symbolic)
                        || (function.symbolic == old.symbolic && !function.name.contains('<'))
                }
            };
            if better {
                best = Some(index);
            }
        }
        let index = best?;
        let function = &program.functions[index];
        let sig = program.signatures.get(index)?;
        Some(ItemSignature {
            name: function.name.clone(),
            params: param_infos(sig),
            ret: type_text(&sig.ret),
            display: signature_display(&function.name, sig),
            location: (function.span.lo < function.span.hi).then(|| self.location(function.span)),
        })
    }

    /// The ownership facts of the innermost function or closure whose body contains a byte offset of a file.
    pub fn ownership_of(&self, path: &str, offset: u32) -> Option<Ownership> {
        let program = self.program.as_ref()?;
        let file = self.file_id(path)?;
        let mut best: Option<&Function> = None;
        for function in program.functions.iter().filter(|f| !f.symbolic).chain(program.closures.iter().map(|c| &c.func))
        {
            let s = function.span;
            if s.file != file || !(s.lo <= offset && offset < s.hi) {
                continue;
            }
            if best.is_none_or(|b| s.hi - s.lo < b.span.hi - b.span.lo) {
                best = Some(function);
            }
        }
        let function = best?;
        Some(Ownership {
            function: function.name.clone(),
            moves: function
                .moved
                .iter()
                .filter_map(|place| {
                    let local = function.locals.get(place.local.0)?;
                    Some(MoveEvent {
                        local: local.name.clone(),
                        path: place.path.clone(),
                        always_moved: place.always_dead,
                    })
                })
                .collect(),
            owned: function
                .locals
                .iter()
                .filter(|local| matches!(local.kind, LocalKind::Owned | LocalKind::OwnParam | LocalKind::Scrutinee))
                .map(|local| OwnedLocal {
                    name: local.name.clone(),
                    ty: type_text(&local.ty),
                    is_parameter: matches!(local.kind, LocalKind::OwnParam),
                })
                .collect(),
            return_origins: function.ret_origins.clone(),
        })
    }

    /// The innermost expression of the program that contains a position, and the function it is in.
    fn found_at<'a>(&self, program: &'a Program, path: &str, offset: u32) -> Option<(Found, &'a Function)> {
        let file = self.file_id(path)?;
        let mut best: Option<(Found, &Function)> = None;
        let bodies = program.functions.iter().filter(|f| !f.symbolic).chain(program.closures.iter().map(|c| &c.func));
        for function in bodies {
            // A function's span is its declaration, so one in another file cannot hold the position; a closure's is
            // its expression.
            if function.span.file != file {
                continue;
            }
            let mut finder = Finder { func: function, file, offset, best: None };
            finder.block(&function.body);
            if let Some(found) = finder.best
                && best.as_ref().is_none_or(|(b, _)| found.span.hi - found.span.lo <= b.span.hi - b.span.lo)
            {
                best = Some((found, function));
            }
        }
        best
    }

    /// The statements of the body of the function `function` declared in `path`, in order. The REPL reads these
    /// to see which statements one input appended (`spec/repl.md`, RP5).
    pub fn statements_of(&self, path: &str, function: &str) -> Option<Vec<StmtInfo>> {
        let program = self.program.as_ref()?;
        let file = self.file_id(path)?;
        for f in program.functions.iter().filter(|f| !f.symbolic) {
            if f.span.file != file || source_text(&f.name) != function {
                continue;
            }
            let mut out = Vec::new();
            for stmt in &f.body.stmts {
                match stmt {
                    Stmt::Let(pat, init, _) => {
                        let mut ids = Vec::new();
                        pattern_ids(pat, &mut ids);
                        out.push(StmtInfo {
                            at: self.location(init.span),
                            kind: StmtKind::Let(
                                ids.into_iter()
                                    .map(|id| {
                                        let local = &f.locals[id];
                                        (local.name.clone(), type_text(&local.ty))
                                    })
                                    .collect(),
                            ),
                        });
                    }
                    Stmt::Expr(e) => {
                        let ty = type_text(&e.ty);
                        out.push(StmtInfo {
                            at: self.location(e.span),
                            kind: StmtKind::Expr { unit: e.ty == Ty::Unit, never: e.ty == Ty::Never, ty },
                        });
                    }
                }
            }
            // The body's tail expression counts as one more statement: it is what an input's trailing
            // expression is, and the caller's indexes cover it.
            if let Some(t) = &f.body.tail {
                let ty = type_text(&t.ty);
                out.push(StmtInfo {
                    at: self.location(t.span),
                    kind: StmtKind::Expr { unit: t.ty == Ty::Unit, never: t.ty == Ty::Never, ty },
                });
            }
            return Some(out);
        }
        None
    }

    /// The expression with exactly the span `lo..hi` of `path`, with its type and the locals it mentions
    /// (`:ownership`, `spec/repl.md` RP9). `None` when no expression has exactly that span (the caller then knows
    /// the span it asked about is not one whole expression).
    pub fn expr_facts(&self, path: &str, lo: u32, hi: u32) -> Option<ExprFacts> {
        let program = self.program.as_ref()?;
        let file = self.file_id(path)?;
        for f in program.functions.iter().filter(|f| !f.symbolic) {
            if f.span.file != file {
                continue;
            }
            let mut finder = ExactFinder { func: f, file, lo, hi, found: None };
            finder.block(&f.body);
            if let Some(found) = finder.found {
                return Some(ExprFacts { ty: type_text(&found.ty), locals: found.uses });
            }
        }
        None
    }

    /// The fields and the methods of the type named `ty` (as [`type_text`] prints it: `Rect`, `List<i32>`), for
    /// completion after `.`. Methods come from the functions whose qualified name starts with the type's name.
    /// Empty for a type the program does not declare.
    pub fn members_of_type(&self, ty: &str) -> Vec<Member> {
        let Some(program) = self.program.as_ref() else { return Vec::new() };
        let name = ty.split('<').next().unwrap_or(ty);
        let name = name.split('.').next_back().unwrap_or(name).trim();
        let mut out = Vec::new();
        for def in &program.structs {
            if def.name == name {
                out.extend(def.fields.iter().filter(|f| !f.is_priv).map(|f| Member {
                    name: f.name.clone(),
                    is_field: true,
                    detail: type_text(&f.ty),
                }));
            }
        }
        for def in &program.enums {
            if def.name == name {
                out.extend(def.variants.iter().map(|v| Member {
                    name: v.name.clone(),
                    is_field: false,
                    detail: "a variant".to_string(),
                }));
            }
        }
        for (id, f) in program.functions.iter().enumerate() {
            let qualified = source_text(&f.name);
            if let Some(method) = qualified.strip_prefix(&format!("{name}."))
                && program.signatures.get(id).is_some_and(|sig| sig.has_self)
            {
                out.push(Member {
                    name: method.to_string(),
                    is_field: false,
                    detail: signature_display(&f.name, &program.signatures[id]),
                });
            }
        }
        out
    }
}

/// The local ids a pattern binds, in pattern order. A `Pat` carries no names; the names live on the function's
/// locals.
fn pattern_ids(pat: &Pat, out: &mut Vec<usize>) {
    match pat {
        Pat::Binding(id, _) => out.push(id.0),
        Pat::At(id, _, inner) => {
            out.push(id.0);
            pattern_ids(inner, out);
        }
        Pat::Some(inner) | Pat::Ok(inner) | Pat::Err(inner) => pattern_ids(inner, out),
        Pat::Product(subs) | Pat::Variant(_, subs) => subs.iter().for_each(|s| pattern_ids(s, out)),
        Pat::Or(alts) => alts.iter().for_each(|a| pattern_ids(a, out)),
        Pat::Wild | Pat::Bool(_) | Pat::Int(_) | Pat::Char(_) | Pat::Str(_) | Pat::Range(..) | Pat::None => {}
    }
}

/// The expression with exactly one span, and the locals it mentions.
struct ExactFound {
    ty: Ty,
    uses: Vec<LocalUse>,
}

/// Finds the expression with exactly the span `lo..hi` of one file, and collects the locals it mentions.
struct ExactFinder<'a> {
    func: &'a Function,
    file: FileId,
    lo: u32,
    hi: u32,
    found: Option<ExactFound>,
}

impl Visit for ExactFinder<'_> {
    fn expr(&mut self, e: &Expr) {
        let span = e.span;
        if span.file == self.file && span.lo == self.lo && span.hi == self.hi {
            // The smallest expression with this span is the one the caller means (a parenthesized expression
            // and its operand can share one).
            let mut uses = LocalsUsed { func: self.func, out: Vec::new() };
            uses.expr(e);
            self.found = Some(ExactFound { ty: e.ty.clone(), uses: merge_uses(uses.out) });
            return;
        }
        visit::walk_expr(self, e);
    }
}

/// The locals one expression mentions, in source order, with the borrows it takes.
struct LocalsUsed<'a> {
    func: &'a Function,
    out: Vec<LocalUse>,
}

impl Visit for LocalsUsed<'_> {
    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Local(LocalId(i)) => {
                let (name, ty) = match self.func.locals.get(*i) {
                    Some(local) => (local.name.clone(), type_text(&local.ty)),
                    None => (String::new(), String::new()),
                };
                if !name.is_empty() {
                    self.out.push(LocalUse { name, ty, borrowed: false, borrowed_mut: false });
                }
            }
            ExprKind::Ref { place, mutable } => {
                if let ExprKind::Local(LocalId(i)) = &place.kind
                    && let Some(local) = self.func.locals.get(*i)
                    && !local.name.is_empty()
                {
                    self.out.push(LocalUse {
                        name: local.name.clone(),
                        ty: type_text(&local.ty),
                        borrowed: !*mutable,
                        borrowed_mut: *mutable,
                    });
                    return;
                }
            }
            _ => {}
        }
        visit::walk_expr(self, e);
    }
}

/// Merges the mentions of one local: one entry, with every borrow flag any mention took.
fn merge_uses(uses: Vec<LocalUse>) -> Vec<LocalUse> {
    let mut out: Vec<LocalUse> = Vec::new();
    for u in uses {
        if let Some(prev) = out.iter_mut().find(|p| p.name == u.name) {
            prev.borrowed |= u.borrowed;
            prev.borrowed_mut |= u.borrowed_mut;
        } else {
            out.push(u);
        }
    }
    out
}

/// The text of a type as source writes it. The checker prints a struct or enum type with its module path
/// (`geometry.shapes.Rect`); a tool shows the name the source uses (`Rect`).
pub(crate) fn type_text(ty: &Ty) -> String {
    source_text(&ty.to_string())
}

/// Every dotted identifier in `text`, replaced by its last segment.
pub(crate) fn source_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if let Some(at) = word.rfind('.') {
            out.push_str(&word[at + 1..]);
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' || c == '.' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// The parameters of a signature, as they are written at the call site.
pub(crate) fn param_infos(sig: &burdin_tir::Signature) -> Vec<ParamInfo> {
    sig.params
        .iter()
        .enumerate()
        .map(|(index, param)| ParamInfo {
            name: match (&param.name, index == 0 && sig.has_self) {
                (Some(name), _) => name.clone(),
                (None, true) => "self".to_string(),
                (None, false) => String::new(),
            },
            ty: type_text(&param.ty),
            convention: if param.own {
                Convention::Own
            } else if param.mut_param || (index == 0 && sig.has_self && param.exclusive) {
                Convention::Mut
            } else {
                Convention::Read
            },
            has_default: param.default.is_some(),
        })
        .collect()
}

/// `fn add(a: i32, b: i32): i32`, from a name and a checked signature.
pub(crate) fn signature_display(name: &str, sig: &burdin_tir::Signature) -> String {
    let mut out = format!("fn {}(", source_text(name));
    for (index, param) in sig.params.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        match (&param.name, index == 0 && sig.has_self) {
            (Some(name), _) => {
                if param.mut_param {
                    out.push_str("mut ");
                }
                if param.own {
                    out.push_str("own ");
                }
                out.push_str(name);
                out.push_str(": ");
                out.push_str(&type_text(&param.ty));
            }
            (None, true) => {
                if param.own {
                    out.push_str("own ");
                } else if param.exclusive {
                    out.push_str("mut ");
                }
                out.push_str("self");
            }
            (None, false) => out.push_str(&type_text(&param.ty)),
        }
    }
    out.push(')');
    if sig.ret != Ty::Unit {
        out.push_str(": ");
        out.push_str(&type_text(&sig.ret));
    }
    out
}
