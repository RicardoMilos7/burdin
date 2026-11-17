//! `select { ... }` (spec/channels.md, CH6-CH8): wait on several channel operations and run exactly one arm.
//!
//! ```text
//! select {
//!     v = rx.recv() => body       // receive: `v` is what `rx.recv()` returns, an `Option<T>`
//!     tx.send(value) => body      // send: the arm may also bind what `tx.send(value)` returns
//!     v = rx2.recv() if open => body   // a guard: the arm is skipped when the condition is false
//!     default => body             // runs, without waiting, when no other arm can
//! }
//! ```
//!
//! The parser rewrites the form into ordinary code over the `__`-prefixed members of `std/src/channel.bd`, so name
//! resolution, type checking, the move checker, and code generation need no `select` of their own. The rewriting for
//! two channel arms and no `default` is
//!
//! ```text
//! {
//!     let c0 = rx.clone()                 // every channel expression is evaluated once, up front
//!     let c1 = tx.clone()                 // (a guard is evaluated here too: `let g0 = open`)
//!     let mut s0 = c0.__empty()           // the slot a received value lands in
//!     let mut s1 = c1.__wrap(value)       // the slot the value to send waits in; the value moves here at once
//!     let mut sel = __select_new(2)       // what waits (only without `default`)
//!     sel.add_recv(c0)                    // with a guard: `if g0 { sel.add_recv(c0) }`
//!     sel.add_send(c1)
//!     let mut i: usize = 2                // the arm that ran; 2 (the number of arms) means none yet
//!     while i == 2 {
//!         let seen = sel.epoch()          // read before the cases are looked at, so no change is missed
//!         if c0.__try_recv_slot(mut s0) { i = 0 }       // the first arm that can run, in source order
//!         else if c1.__try_send_slot(mut s1) { i = 1 }  // (a guard: `g0 && c0.__try_recv_slot(mut s0)`)
//!         else { sel.wait(seen) }                       // with `default`: `i = <the default arm>` instead
//!     }
//!     match i {
//!         0 => { let v = c0.__take(mut s0); body0 }
//!         else => { let _ = c1.__take(mut s1); body1 }
//!     }
//! }
//! ```
//!
//! The arm bodies are outside the loop, so `break`, `continue`, `return`, and `?` in a body mean what they mean
//! anywhere else in the function.

use super::{PResult, Parser, Restriction};
use crate::ast::*;
use crate::token::TokenKind;
use burdin_diag::Span;

/// One parsed arm, before it is rewritten.
struct Arm {
    /// The pattern before `=`, when written.
    binding: Option<Pattern>,
    /// What the arm waits for.
    kind: Kind,
    /// The channel expression of a `recv` or `send` arm.
    channel: Option<Expr>,
    /// The value of a `send` arm.
    value: Option<Expr>,
    /// The condition after `if`: the arm takes part only when it is true.
    guard: Option<Expr>,
    body: Expr,
    span: Span,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// `channel.recv()`.
    Recv,
    /// `channel.send(value)`.
    Send,
    Default,
}

impl Parser<'_> {
    /// Whether the current token starts a `select` expression: the word `select` followed by `{` on the same line.
    /// `select` is not reserved, so it stays an ordinary name everywhere else.
    pub(super) fn at_select(&self, r: Restriction) -> bool {
        r == Restriction::None
            && matches!(self.kind(), TokenKind::Ident(name) if name == "select")
            && matches!(self.nth_kind(1), TokenKind::LBrace)
            && !self.toks[(self.pos + 1).min(self.toks.len() - 1)].nl_before
    }

    pub(super) fn select_expr(&mut self) -> PResult<Expr> {
        let start = self.span();
        self.bump();
        let id = self.select_count;
        self.select_count += 1;
        self.open(&TokenKind::LBrace, true)?;
        let mut arms = Vec::new();
        let mut failed = false;
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(self.unexpected("`}`"));
            }
            match self.select_arm() {
                Ok(arm) => arms.push(arm),
                Err(_) => {
                    failed = true;
                    // Skip to the next arm: the next line inside the braces.
                    self.recover_select_arm();
                }
            }
        }
        let close = self.close(&TokenKind::RBrace)?;
        let span = start.to(close.span);
        // The whole `select` has been read, so an error found now is reported and the parse goes on after the closing
        // brace: the expression stands for `()`, and the statements after it are checked as usual.
        let unit = Expr { kind: ExprKind::Unit, span };
        if failed {
            return Ok(unit);
        }
        if arms.is_empty() {
            self.error_at(span, "a `select` needs at least one arm".to_string());
            return Ok(unit);
        }
        if arms.iter().filter(|a| a.kind == Kind::Default).count() > 1 {
            self.error_at(span, "a `select` has at most one `default` arm".to_string());
            return Ok(unit);
        }
        Ok(lower(id, arms, span))
    }

    /// Skips to the end of a malformed arm: a line break or `}` at the select's own depth.
    fn recover_select_arm(&mut self) {
        let mut depth = 0i32;
        let start = self.pos;
        while !self.at_eof() {
            let t = self.tok();
            if depth == 0 && (matches!(t.kind, TokenKind::RBrace) || (self.pos > start && t.nl_before)) {
                break;
            }
            match t.kind {
                TokenKind::LBrace | TokenKind::LParen | TokenKind::LBracket => depth += 1,
                TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket => depth = (depth - 1).max(0),
                _ => {}
            }
            self.bump();
        }
    }

    fn select_arm(&mut self) -> PResult<Arm> {
        let arm_start = self.span();
        let is_default = matches!(self.kind(), TokenKind::Ident(name) if name == "default")
            && self.nth_kind(1) == &TokenKind::FatArrow;
        let mut binding = None;
        let mut guard = None;
        let (kind, channel, value) = if is_default {
            self.bump();
            (Kind::Default, None, None)
        } else {
            // `name = op`, `mut name = op`, `_ = op`, or just `op`.
            let has_binding = matches!(self.kind(), TokenKind::Underscore | TokenKind::Mut)
                || (matches!(self.kind(), TokenKind::Ident(_)) && self.nth_kind(1) == &TokenKind::Eq);
            if has_binding {
                let pattern = self.at_pattern()?;
                self.expect(&TokenKind::Eq)?;
                binding = Some(pattern);
            }
            let head = self.expr()?;
            let op = self.select_op(head)?;
            if self.eat(&TokenKind::If) {
                guard = Some(self.expr()?);
            }
            op
        };
        self.expect(&TokenKind::FatArrow)?;
        let body = self.expr()?;
        let span = arm_start.to(body.span);
        let block_body = body.is_block_like();
        if !(self.eat(&TokenKind::Comma) || self.at(&TokenKind::RBrace) || self.tok().nl_before || block_body) {
            return Err(self.unexpected("`,`, a line break, or `}` after the select arm"));
        }
        Ok(Arm { binding, kind, channel, value, guard, body, span })
    }

    /// Checks that `head` is `channel.recv()` or `channel.send(value)` and splits it.
    fn select_op(&mut self, head: Expr) -> PResult<(Kind, Option<Expr>, Option<Expr>)> {
        let span = head.span;
        if let ExprKind::Call(callee, mut args) = head.kind
            && let ExprKind::Field(channel, method) = callee.kind
        {
            if method.name == "recv" && args.is_empty() {
                return Ok((Kind::Recv, Some(*channel), None));
            }
            if method.name == "send" && args.len() == 1 {
                let arg = args.remove(0);
                if arg.name.is_none() && !arg.mut_marker {
                    return Ok((Kind::Send, Some(*channel), Some(arg.expr)));
                }
            }
        }
        Err(self.error_at(
            span,
            "a `select` arm waits for `channel.recv()`, `channel.send(value)`, or is `default`".to_string(),
        ))
    }
}

// ----- the rewriting ---------------------------------------------------------------------------------------------

fn name(text: &str, span: Span) -> Expr {
    Expr { kind: ExprKind::Name { name: Ident { name: text.to_string(), span }, args: Vec::new() }, span }
}

fn int(value: usize, span: Span) -> Expr {
    Expr { kind: ExprKind::Int(value as u128), span }
}

fn call(callee: Expr, args: Vec<Arg>, span: Span) -> Expr {
    Expr { kind: ExprKind::Call(Box::new(callee), args), span }
}

fn arg(expr: Expr) -> Arg {
    Arg { name: None, mut_marker: false, mut_span: None, expr }
}

fn mut_arg(expr: Expr) -> Arg {
    let span = expr.span;
    Arg { name: None, mut_marker: true, mut_span: Some(span), expr }
}

/// `recv.method(args)`.
fn method(recv: Expr, method: &str, args: Vec<Arg>, span: Span) -> Expr {
    let field = Expr { kind: ExprKind::Field(Box::new(recv), Ident { name: method.to_string(), span }), span };
    call(field, args, span)
}

fn let_stmt(pattern: Pattern, init: Expr, span: Span) -> Stmt {
    Stmt::Let { pattern, ty: None, init, else_block: None, span }
}

fn binding(text: &str, mutable: bool, span: Span) -> Pattern {
    Pattern { kind: PatternKind::Binding { name: Ident { name: text.to_string(), span }, mutable }, span }
}

fn block(stmts: Vec<Stmt>, span: Span) -> Block {
    Block { stmts, span }
}

fn block_expr(b: Block) -> Expr {
    Expr { span: b.span, kind: ExprKind::Block(b) }
}

fn lower(id: usize, arms: Vec<Arm>, span: Span) -> Expr {
    let temp = |suffix: &str| format!("__sel{id}_{suffix}");
    let has_default = arms.iter().any(|a| a.kind == Kind::Default);
    let n = arms.len();
    let sel = format!("__sel{id}");
    let index = temp("i");
    let seen = temp("seen");

    // Only the arms that wait on a channel have a handle and a slot.
    let channel_arms: Vec<usize> = (0..n).filter(|&i| arms[i].kind != Kind::Default).collect();
    let mut stmts: Vec<Stmt> = Vec::new();
    let mut arms = arms;
    // The arms whose guard is written: they are tried, and registered with the select, only when it is true.
    let mut guarded = vec![false; n];

    // Every channel expression is evaluated once, and a value to send moves into its slot now.
    for &i in &channel_arms {
        let a_span = arms[i].span;
        let channel = arms[i].channel.take().expect("a channel arm has its channel");
        let value = arms[i].value.take();
        let c = temp(&format!("c{i}"));
        let s = temp(&format!("s{i}"));
        if let Some(guard) = arms[i].guard.take() {
            stmts.push(let_stmt(binding(&temp(&format!("g{i}")), false, a_span), guard, a_span));
            guarded[i] = true;
        }
        stmts.push(let_stmt(binding(&c, false, a_span), method(channel, "clone", vec![], a_span), a_span));
        let slot = match value {
            Some(v) => method(name(&c, a_span), "__wrap", vec![arg(v)], a_span),
            None => method(name(&c, a_span), "__empty", vec![], a_span),
        };
        stmts.push(let_stmt(binding(&s, true, a_span), slot, a_span));
    }

    // Only a `select` that can wait needs something to wait with.
    if !has_default {
        stmts.push(let_stmt(
            binding(&sel, true, span),
            call(name("__select_new", span), vec![arg(int(channel_arms.len(), span))], span),
            span,
        ));
        for &i in &channel_arms {
            let a_span = arms[i].span;
            let add = if arms[i].kind == Kind::Send { "add_send" } else { "add_recv" };
            let c = name(&temp(&format!("c{i}")), a_span);
            let add_call = Stmt::Expr(method(name(&sel, a_span), add, vec![arg(c)], a_span));
            if guarded[i] {
                let cond = name(&temp(&format!("g{i}")), a_span);
                let then_block = block(vec![add_call], a_span);
                stmts.push(Stmt::Expr(Expr {
                    kind: ExprKind::If { cond: Box::new(cond), then_block, else_expr: None },
                    span: a_span,
                }));
            } else {
                stmts.push(add_call);
            }
        }
    }

    // `let mut i: usize = n`, and the loop that runs until an arm is chosen.
    let usize_ty = Type {
        kind: TypeKind::Path(vec![PathSegment { name: Ident { name: "usize".to_string(), span }, args: Vec::new() }]),
        span,
    };
    stmts.push(Stmt::Let {
        pattern: binding(&index, true, span),
        ty: Some(usize_ty),
        init: int(n, span),
        else_block: None,
        span,
    });

    // The chain of attempts, first arm first; the last `else` waits, or runs the `default` arm.
    let assign_index = |value: usize, s: Span| {
        Stmt::Expr(Expr { kind: ExprKind::Assign(Box::new(name(&index, s)), Box::new(int(value, s))), span: s })
    };
    let mut tail: Expr = if has_default {
        let default_at = arms.iter().position(|a| a.kind == Kind::Default).unwrap_or(0);
        block_expr(block(vec![assign_index(default_at, span)], span))
    } else {
        block_expr(block(vec![Stmt::Expr(method(name(&sel, span), "wait", vec![arg(name(&seen, span))], span))], span))
    };
    for &i in channel_arms.iter().rev() {
        let a_span = arms[i].span;
        let c = name(&temp(&format!("c{i}")), a_span);
        let s = name(&temp(&format!("s{i}")), a_span);
        let attempt = if arms[i].kind == Kind::Send { "__try_send_slot" } else { "__try_recv_slot" };
        let mut cond = method(c, attempt, vec![mut_arg(s)], a_span);
        if guarded[i] {
            let g = name(&temp(&format!("g{i}")), a_span);
            cond = Expr { kind: ExprKind::Binary(BinOp::And, Box::new(g), Box::new(cond)), span: a_span };
        }
        tail = Expr {
            kind: ExprKind::If {
                cond: Box::new(cond),
                then_block: block(vec![assign_index(i, a_span)], a_span),
                else_expr: Some(Box::new(tail)),
            },
            span: a_span,
        };
    }
    let mut loop_body = Vec::new();
    if !has_default {
        loop_body.push(let_stmt(binding(&seen, false, span), method(name(&sel, span), "epoch", vec![], span), span));
    }
    loop_body.push(Stmt::Expr(tail));
    let cond = Expr { kind: ExprKind::Binary(BinOp::Eq, Box::new(name(&index, span)), Box::new(int(n, span))), span };
    stmts.push(Stmt::Expr(Expr { kind: ExprKind::While { cond: Box::new(cond), body: block(loop_body, span) }, span }));

    // The arm that ran. The last arm is the `else` of the match, so the match covers every integer.
    let mut match_arms = Vec::new();
    for (i, a) in arms.into_iter().enumerate() {
        let a_span = a.span;
        let mut body_stmts = Vec::new();
        if a.kind != Kind::Default {
            let c = name(&temp(&format!("c{i}")), a_span);
            let s = name(&temp(&format!("s{i}")), a_span);
            let taken = method(c, "__take", vec![mut_arg(s)], a_span);
            let pattern = a.binding.unwrap_or(Pattern { kind: PatternKind::Wild, span: a_span });
            body_stmts.push(let_stmt(pattern, taken, a_span));
        }
        body_stmts.push(Stmt::Expr(a.body));
        let body = block_expr(block(body_stmts, a_span));
        let pattern = if i + 1 == n {
            None
        } else {
            Some(Pattern { kind: PatternKind::Lit(LitPat::Int { value: i as u128, negative: false }), span: a_span })
        };
        match_arms.push(MatchArm { pattern, guard: None, body, span: a_span });
    }
    stmts.push(Stmt::Expr(Expr { kind: ExprKind::Match(Box::new(name(&index, span)), match_arms), span }));
    block_expr(block(stmts, span))
}

#[cfg(test)]
mod tests {
    use crate::ast::*;
    use crate::parser::parse_file;
    use burdin_diag::{Diagnostics, FileId};

    fn parse(src: &str) -> (File, Diagnostics) {
        let mut diags = Diagnostics::default();
        let lexed = crate::lexer::lex(FileId(0), src, &mut diags);
        let file = parse_file(FileId(0), lexed, &mut diags);
        (file, diags)
    }

    /// The statements of `main`'s body.
    fn main_stmts(file: &File) -> &[Stmt] {
        for item in &file.items {
            if let Item::Fn(f) = item
                && f.name.name == "main"
            {
                return &f.body.stmts;
            }
        }
        panic!("no main");
    }

    /// The top-level statements of the block a `select` was rewritten into.
    fn rewritten(src: &str) -> Vec<Stmt> {
        let (file, diags) = parse(&format!("fn main() {{\n{src}\n}}"));
        assert_eq!(diags.len(), 0, "unexpected diagnostics for {src}");
        match main_stmts(&file) {
            [Stmt::Expr(Expr { kind: ExprKind::Block(b), .. })] => b.stmts.clone(),
            other => panic!("expected one block statement, got {other:?}"),
        }
    }

    fn let_names(stmts: &[Stmt]) -> Vec<String> {
        stmts
            .iter()
            .filter_map(|s| match s {
                Stmt::Let { pattern: Pattern { kind: PatternKind::Binding { name, .. }, .. }, .. } => {
                    Some(name.name.clone())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn two_receive_arms_become_handles_slots_a_select_object_a_loop_and_a_match() {
        let stmts = rewritten("select {\n v = a.recv() => 1\n w = b.recv() => 2\n}");
        let names = let_names(&stmts);
        assert_eq!(
            names,
            ["__sel0_c0", "__sel0_s0", "__sel0_c1", "__sel0_s1", "__sel0", "__sel0_i"],
            "every channel is cloned once, each arm has a slot, and the select object and the index follow"
        );
        // The select object is added to once per channel arm, then come the loop and the match.
        assert!(matches!(stmts[stmts.len() - 2], Stmt::Expr(Expr { kind: ExprKind::While { .. }, .. })));
        match &stmts[stmts.len() - 1] {
            Stmt::Expr(Expr { kind: ExprKind::Match(_, arms), .. }) => {
                assert_eq!(arms.len(), 2);
                assert!(arms[0].pattern.is_some(), "the first arm matches its index");
                assert!(arms[1].pattern.is_none(), "the last arm is the `else`, so the match is exhaustive");
            }
            other => panic!("expected the match last, got {other:?}"),
        }
    }

    #[test]
    fn default_makes_the_select_non_blocking() {
        let stmts = rewritten("select {\n v = a.recv() => 1\n default => 2\n}");
        assert!(
            !let_names(&stmts).contains(&"__sel0".to_string()),
            "with a default there is no select object to wait with"
        );
    }

    #[test]
    fn a_guard_gets_a_variable_of_its_own() {
        let stmts = rewritten("select {\n v = a.recv() if open => 1\n w = b.recv() => 2\n}");
        let names = let_names(&stmts);
        assert!(names.contains(&"__sel0_g0".to_string()));
        assert!(!names.contains(&"__sel0_g1".to_string()), "an arm without a guard has no guard variable");
    }

    #[test]
    fn nested_selects_use_different_names() {
        let (file, diags) = parse("fn main() {\n select {\n v = a.recv() => select {\n w = b.recv() => 1\n }\n }\n}");
        assert_eq!(diags.len(), 0);
        let text = format!("{:?}", main_stmts(&file));
        assert!(text.contains("__sel0_c0"), "the outer select is numbered first");
        assert!(text.contains("__sel1_c0"), "the inner select has its own names");
    }

    #[test]
    fn select_is_an_ordinary_name_elsewhere() {
        let (_, diags) = parse("fn main() {\n let select = 1\n let x = select + 1\n if select == 1 { }\n}");
        assert_eq!(diags.len(), 0, "`select` is only special before a brace on its own line");
    }

    #[test]
    fn malformed_selects_are_syntax_errors_and_parsing_continues() {
        for src in [
            "select {\n}",
            "select {\n default => 1\n default => 2\n}",
            "select {\n v = a.len() => 1\n}",
            "select {\n a.send() => 1\n}",
            "select {\n a.send(1, 2) => 1\n}",
            "select {\n v = a.recv(1) => 1\n}",
            "select {\n a.send(mut x) => 1\n}",
        ] {
            let (file, diags) = parse(&format!("fn main() {{\n{src}\nlet after = 1\n}}"));
            assert_eq!(diags.len(), 1, "exactly one error for {src}");
            assert_eq!(main_stmts(&file).len(), 2, "the statement after the select is still parsed for {src}");
        }
    }
}
