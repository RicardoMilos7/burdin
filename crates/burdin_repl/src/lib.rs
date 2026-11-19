//! `burdin repl`: the interactive read-eval-print loop (`spec/repl.md`).
//!
//! Everything a session is comes from one design: the session is a program — one module file whose `fn main`
//! body grows with every statement input and whose items accumulate below it — and every input is checked by
//! the real front end through the query-based compiler library and evaluated by the comptime interpreter. So
//! every diagnostic is the one `burdin build` reports, moves and borrows persist across lines exactly as in a
//! function body, and a printed value is byte for byte what the compiled program prints, because the printing
//! runs the same standard-library `Formatter` code.
//!
//! The crate has no LLVM dependency. The two operations that need code generation — `:ir` and `:compile` —
//! are closures the driver hands in ([`Native`]), so a session works everywhere the compiler front end works.

mod complete;
mod editor;
mod highlight;
mod terminal;
mod text;

use burdin_comptime::{Bindings, SessionFault};
use burdin_diag::{Code, Diagnostics, RenderOptions, SourceMap, Span};
use burdin_query::{Analysis, Db, MoveEvent, StmtKind};
use std::io::{IsTerminal, Write};
use text::{Input, SESSION_FILE, SESSION_FN, Session};

/// `:compile`'s operation: builds the session text and runs it; `Ok` is the program's exit status.
pub type CompileHook<'a> = Box<dyn FnMut(&str) -> Result<i32, ()> + 'a>;

/// `:ir`'s operation: the IR of one function of the session text, or why there is none.
pub type IrHook<'a> = Box<dyn FnMut(&str, &str) -> Result<String, String> + 'a>;

/// The two operations the driver provides because they need the LLVM back end (`spec/repl.md`, RP10). The
/// closures borrow the driver's build options, so the type carries their lifetime.
pub struct Native<'a> {
    /// `:compile`: builds the session text as a one-file program and runs it; `Ok` carries the program's exit
    /// status, `Err` means the build failed and was already reported.
    pub compile: CompileHook<'a>,
    /// `:ir <fn>`: the LLVM IR of one function of the session text, or why there is none.
    pub ir: IrHook<'a>,
}

/// How `burdin repl` runs. The driver fills this in from its command line.
pub struct Config<'a> {
    /// Compile-time code sees `release` as true.
    pub release: bool,
    /// The step budget of each input's evaluation; `None` is the default.
    pub steps: Option<u64>,
    /// The most threads the front end may use; 0 is the machine's parallelism.
    pub jobs: usize,
    /// How diagnostics render (format, color, width, and limit).
    pub render: RenderOptions,
    /// The operations that need the back end.
    pub native: Native<'a>,
}

/// The prompt of a fresh input, and of a line that continues one.
const PROMPT: &str = "burdin> ";
const MORE: &str = "......> ";

/// What one complete input came to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Ok,
    Failed,
    Quit,
}

/// Runs the REPL over standard input until `:quit` or the end of input. Returns the process exit status: 0 when
/// every input was accepted, 1 when any input, meta-command, or evaluation failed. Standard input not being a
/// terminal disables every interactive feature (prompts included, `spec/repl.md` RP11), so a scripted session
/// is deterministic.
pub fn run(config: &mut Config) -> i32 {
    let interactive = std::io::stdin().is_terminal();
    let mut repl = Repl::new(config);
    let mut editor = if interactive {
        println!("Burdin REPL — items, lets, and expressions; :help lists the meta-commands, :quit leaves");
        let history = burdin_query::package::home_dir().map(|home| home.join("repl_history"));
        editor::Editor::new(history)
    } else {
        None
    };
    let mut failed = false;
    let mut pending = String::new();
    loop {
        let prompt = if pending.is_empty() { PROMPT } else { MORE };
        let line = match &mut editor {
            Some(editor) => match editor.read(prompt, &mut |before, word| repl.completions(before, word)) {
                editor::Read::Line(line) => Some(line),
                editor::Read::Interrupted => {
                    pending.clear();
                    continue;
                }
                editor::Read::Eof => None,
            },
            None => {
                if interactive {
                    let mut out = std::io::stdout();
                    let _ = write!(out, "{prompt}");
                    let _ = out.flush();
                }
                // One line, with the lock taken for the read only: the editor may own standard input next.
                let mut line = String::new();
                match std::io::stdin().read_line(&mut line) {
                    Ok(0) | Err(_) => None,
                    Ok(_) => Some(line.trim_end_matches(['\n', '\r']).to_string()),
                }
            }
        };
        let Some(line) = line else { break };
        let mut buffered = std::mem::take(&mut pending);
        if !buffered.is_empty() {
            buffered.push('\n');
        }
        buffered.push_str(&line);
        match text::classify(&buffered) {
            Input::Incomplete => {
                pending = buffered;
                continue;
            }
            _ => pending.clear(),
        }
        match repl.input(&buffered) {
            Outcome::Ok => {}
            Outcome::Failed => failed = true,
            Outcome::Quit => break,
        }
    }
    // A still-pending input at the end of input is reported, not lost: it is submitted as statements, and the
    // front end says what is wrong with it.
    if !pending.is_empty() && repl.accept_body(&pending) == Outcome::Failed {
        failed = true;
    }
    if let Some(editor) = &mut editor {
        editor.save_history();
    }
    i32::from(failed)
}

/// One running session. `'n` is the lifetime of the native operations the driver handed in.
struct Repl<'a, 'n> {
    config: &'a mut Config<'n>,
    db: Db,
    session: Session,
    bindings: Bindings,
    /// How many statements of the session function have been evaluated.
    done: usize,
}

impl<'a, 'n> Repl<'a, 'n> {
    fn new(config: &'a mut Config<'n>) -> Repl<'a, 'n> {
        let options = burdin_query::Options {
            mode: burdin_query::EntryMode::Test,
            release: config.release,
            comptime_steps: config.steps,
            no_cache: false,
            jobs: config.jobs,
        };
        let mut repl = Repl {
            config,
            db: Db::from_files(&[(SESSION_FILE, "")], options).expect("the session package loads"),
            session: Session::default(),
            bindings: Bindings::default(),
            done: 0,
        };
        // The empty session is analyzed once up front, so the standard library is checked before the first
        // input and every input costs the same.
        repl.analyze();
        repl
    }

    fn comptime_options(&self) -> burdin_comptime::Options {
        burdin_comptime::Options {
            release: self.config.release,
            steps: self.config.steps.unwrap_or(burdin_comptime::DEFAULT_STEPS),
        }
    }

    /// Analyzes the session as the chunks now stand: the query library's, so every diagnostic is the one
    /// `burdin build` reports.
    fn analyze(&mut self) -> Analysis {
        let text = text::text(&self.session);
        self.db.set_file(SESSION_FILE, &text);
        match self.db.analyze() {
            Ok(analysis) => analysis,
            Err(cancelled) => unreachable!("nothing cancels a session's analysis: {cancelled}"),
        }
    }

    /// Renders diagnostics to standard error, the way the driver does.
    fn report(&self, diags: &Diagnostics) {
        let rendered = diags.render_with(self.db.sources(), &self.config.render);
        let mut err = std::io::stderr();
        let _ = err.write_all(rendered.text.as_bytes());
    }

    fn report_warnings(&self, analysis: &Analysis) {
        if analysis.diagnostics.has_warnings() {
            self.report(&analysis.diagnostics);
        }
    }

    /// Handles one complete input.
    fn input(&mut self, input: &str) -> Outcome {
        if input.trim_start().starts_with(':') {
            return self.meta(input);
        }
        match text::classify(input) {
            Input::Nothing => Outcome::Ok,
            Input::Item => self.accept_item(input),
            Input::Body | Input::Bad => self.accept_body(input),
            // The loop keeps reading lines until an input is complete; a submitted one never is.
            Input::Incomplete => unreachable!("an incomplete input was submitted: {input}"),
        }
    }

    /// An item input: appended below `main`, checked against the session, rolled back on any error.
    fn accept_item(&mut self, item: &str) -> Outcome {
        self.session.push_item(item);
        let analysis = self.analyze();
        if analysis.diagnostics.has_errors() {
            self.report(&analysis.diagnostics);
            self.session.pop();
            return Outcome::Failed;
        }
        self.report_warnings(&analysis);
        Outcome::Ok
    }

    /// A statement input: appended to `main`'s body, checked, then evaluated with the bindings carried over.
    /// A bare expression of a non-unit type is not a statement in Burdin, so the front end answers with exactly
    /// one type error at it: that answer is the print wrap's trigger (`spec/repl.md`, RP5) — the expression
    /// moves into `__repl_v` and a `println` prints it with `Debug`, the value printed byte for byte as the
    /// compiled program prints it. A failed input is rolled back so the session stays at its last accepted
    /// state.
    fn accept_body(&mut self, input: &str) -> Outcome {
        let chunk = self.session.len();
        self.session.push_body(input);
        let analysis = self.analyze();
        if analysis.diagnostics.has_errors() {
            if let Some(expr) = printable_tail(&analysis, chunk, &self.session) {
                return self.wrap(expr, true);
            }
            self.report(&analysis.diagnostics);
            self.session.pop();
            return Outcome::Failed;
        }
        self.report_warnings(&analysis);
        self.evaluate(&analysis)
    }

    /// Rewrites the last body chunk around its trailing expression and re-analyzes. With `print` the value is
    /// printed; when the type turns out not to implement `Debug` (the wrap's one possible failure), the wrap
    /// is redone without the `println`, the input is kept and evaluated, and a note says nothing was printed.
    fn wrap(&mut self, expr: (u32, u32), print: bool) -> Outcome {
        let raw = self.session.last_body().expect("the chunk was just pushed").to_string();
        if self.session.wrap_last(expr, print).is_err() {
            eprintln!("error: the input's value could not be printed; the input was discarded");
            return Outcome::Failed;
        }
        let wrapped = self.analyze();
        if !wrapped.diagnostics.has_errors() {
            self.report_warnings(&wrapped);
            return self.evaluate(&wrapped);
        }
        // The wrap with the `println` can fail one way: the value's type does not implement `Debug` (E0193 at
        // the synthesized placeholder, which would point at the wrap rather than the input). The wrap without
        // it is just the `let`, which the raw analysis's only error says is well-typed otherwise.
        if print && only_placeholder_errors(&wrapped) {
            return self.wrap_without_print(expr, &raw);
        }
        self.report(&wrapped.diagnostics);
        self.session.pop();
        println!("note: the input's value could not be printed; the input was discarded");
        Outcome::Failed
    }

    /// The trailing expression's type does not implement `Debug`: bind it without printing, evaluate the input,
    /// and say so.
    fn wrap_without_print(&mut self, expr: (u32, u32), raw: &str) -> Outcome {
        // The chunk holds the printed wrap; put the original text back and wrap it without the `println`.
        self.session.pop();
        self.session.push_body(raw);
        if self.session.wrap_last(expr, false).is_err() {
            eprintln!("error: the input's value could not be printed; the input was discarded");
            return Outcome::Failed;
        }
        let bound = self.analyze();
        if bound.diagnostics.has_errors() {
            self.report(&bound.diagnostics);
            self.session.pop();
            return Outcome::Failed;
        }
        let ty = bound
            .statements_of(SESSION_FILE, SESSION_FN)
            .unwrap_or_default()
            .last()
            .and_then(|s| match &s.kind {
                StmtKind::Let(names) => names.first().map(|(_, ty)| ty.clone()),
                _ => None,
            })
            .unwrap_or_default();
        println!("note: the value's type `{ty}` does not implement Debug; nothing was printed");
        self.evaluate(&bound)
    }

    /// Evaluates the statements the last input appended, carrying the bindings. A fault rolls the input back.
    fn evaluate(&mut self, analysis: &Analysis) -> Outcome {
        let Some(program) = analysis.program.as_ref() else { return Outcome::Failed };
        let count = analysis.statements_of(SESSION_FILE, SESSION_FN).unwrap_or_default().len();
        let file = self.db.file_id(SESSION_FILE).expect("the session file is loaded");
        let Some(main) = burdin_comptime::session_function(program, SESSION_FN, file) else { return Outcome::Failed };
        let snapshot = self.bindings.clone();
        let opts = self.comptime_options();
        let result = burdin_comptime::eval_stmts(program, main, self.done, &mut self.bindings, opts);
        match result {
            Ok(()) => {
                self.done = count;
                Outcome::Ok
            }
            Err(fault) => {
                self.report_fault(&fault);
                self.bindings = snapshot;
                self.session.pop();
                Outcome::Failed
            }
        }
    }

    /// Reports a session fault: the code ordinary compile-time evaluation uses, the fault's own detail, and the
    /// note that the input was discarded.
    fn report_fault(&mut self, fault: &SessionFault) {
        let at = Span::new(
            self.db.file_id(SESSION_FILE).unwrap_or(burdin_diag::FileId(0)),
            fault.span().lo,
            fault.span().hi,
        );
        let mut diags = Diagnostics::default();
        diags
            .error(fault.code(), at, fault.detail())
            .note_without_span("the input was discarded and the session is unchanged");
        self.report(&diags);
    }

    // ----- meta-commands -----------------------------------------------------------------------------

    /// Handles one meta-command (`:help`, `:quit`, ...).
    fn meta(&mut self, input: &str) -> Outcome {
        let trimmed = input.trim();
        let (word, rest) = trimmed.split_once(char::is_whitespace).unwrap_or((trimmed, ""));
        let rest = rest.trim();
        match word {
            ":help" => {
                println!("{HELP}");
                Outcome::Ok
            }
            ":quit" => Outcome::Quit,
            ":reset" => self.reset(),
            ":type" => self.type_of(rest),
            ":ownership" => self.ownership(rest),
            ":explain" => self.explain(rest),
            ":ir" => self.show_ir(rest),
            ":load" => self.load(rest),
            ":compile" => self.compile(),
            _ => {
                eprintln!("error: unknown meta-command `{word}`; :help lists them");
                Outcome::Failed
            }
        }
    }

    /// `:reset`: the drops of the live bindings run (their side effects included), and the session starts over.
    fn reset(&mut self) -> Outcome {
        let analysis = self.analyze();
        let mut failed = false;
        if let Some(program) = &analysis.program
            && let Some(file) = self.db.file_id(SESSION_FILE)
            && let Some(main) = burdin_comptime::session_function(program, SESSION_FN, file)
        {
            let opts = self.comptime_options();
            let result = burdin_comptime::drop_bindings(program, main, &mut self.bindings, opts);
            if let Err(fault) = result {
                self.report_fault(&fault);
                failed = true;
            }
        }
        self.session = Session::default();
        self.bindings = Bindings::default();
        self.done = 0;
        if failed { Outcome::Failed } else { Outcome::Ok }
    }

    /// `:type <expr>`: the type of an expression, from an analysis of the session with the expression appended
    /// and then discarded. Nothing is evaluated and nothing moves.
    fn type_of(&mut self, expr: &str) -> Outcome {
        if expr.is_empty() {
            eprintln!("error: `:type` needs an expression, as in `:type 1 + 2`");
            return Outcome::Failed;
        }
        let probe = self.probe(expr);
        if let Some(ty) = &probe.ty {
            println!("{ty}");
        }
        probe.outcome()
    }

    /// `:ownership <expr>`: what the expression moves, borrows, and reads, without evaluating it.
    fn ownership(&mut self, expr: &str) -> Outcome {
        if expr.is_empty() {
            eprintln!("error: `:ownership` needs an expression, as in `:ownership s.field`");
            return Outcome::Failed;
        }
        let probe = self.probe(expr);
        if let Some(answer) = probe.ownership() {
            println!("type: {}", answer.ty);
            println!("reads: {}", list_or_nothing(&answer.reads));
            println!("moves: {}", list_or_nothing(&answer.moves));
            println!("borrows: {}", list_or_nothing(&answer.borrows));
        }
        probe.outcome()
    }

    /// `:explain <code>`: the error reference entry, the same text `burdin explain` prints.
    fn explain(&mut self, code: &str) -> Outcome {
        let code = code.trim();
        if code.is_empty() {
            print!("{}", burdin_diag::code_list());
            return Outcome::Ok;
        }
        match burdin_diag::explain_section(code) {
            Some(section) => {
                println!("{section}");
                if let Some(url) = burdin_diag::code_url(&code.to_ascii_uppercase()) {
                    println!("\nOnline: {url}");
                }
                Outcome::Ok
            }
            None => {
                eprintln!("error: unknown error code `{code}`");
                Outcome::Failed
            }
        }
    }

    /// `:ir <fn>`: the LLVM IR of one function of the session, printed as the back end emits it.
    fn show_ir(&mut self, name: &str) -> Outcome {
        if name.is_empty() {
            eprintln!("error: `:ir` needs a function name, as in `:ir main`");
            return Outcome::Failed;
        }
        let text = text::text(&self.session);
        match (self.config.native.ir)(&text, name) {
            Ok(ir) => {
                print!("{ir}");
                Outcome::Ok
            }
            Err(message) => {
                eprintln!("error: {message}");
                Outcome::Failed
            }
        }
    }

    /// `:load <file>`: a file's module content, appended as one atomic item input.
    fn load(&mut self, path: &str) -> Outcome {
        if path.is_empty() {
            eprintln!("error: `:load` needs a file path, as in `:load helpers.bd`");
            return Outcome::Failed;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            eprintln!("error: cannot read {path}");
            return Outcome::Failed;
        };
        // The file must be a module on its own, so its own errors report with its own path and lines.
        let mut sources = SourceMap::default();
        let file = sources.add(path.to_string(), content.clone());
        let mut diags = Diagnostics::default();
        burdin_syntax::parse(file, &content, &mut diags);
        if diags.has_errors() {
            let rendered = diags.render_with(&sources, &self.config.render);
            let mut err = std::io::stderr();
            let _ = err.write_all(rendered.text.as_bytes());
            return Outcome::Failed;
        }
        self.accept_item(&content)
    }

    /// `:compile`: the driver builds the session and runs it natively (RP10).
    fn compile(&mut self) -> Outcome {
        let text = text::text(&self.session);
        match (self.config.native.compile)(&text) {
            Ok(_) => Outcome::Ok,
            Err(()) => Outcome::Failed,
        }
    }

    /// The completion candidates for the word `word` at the end of `before` (`spec/repl.md`, RP13), from the
    /// session as it stands.
    fn completions(&mut self, before: &str, word: &str) -> Vec<String> {
        let analysis = self.analyze();
        complete::candidates(&mut self.db, &analysis, before, word)
    }

    /// Analyzes the session with one extra statement appended, for `:type` and `:ownership`: nothing of the
    /// probe reaches the session, and the next analysis sets the file's text again.
    fn probe(&mut self, expr: &str) -> Probe {
        // The moves of the session as it stands, so the probe can tell which ones the expression adds.
        let before = self.analyze().ownership_of(SESSION_FILE, 0).map(|o| o.moves).unwrap_or_default();
        let chunk = self.session.len();
        self.session.push_body(expr);
        let analysis = self.analyze();
        let mut failed = false;
        if analysis.diagnostics.has_errors() {
            // A bare non-unit expression gets exactly the type error a statement gets (`expected (), found T`);
            // the probe asked for that expression's type, so that answer is not a failure.
            if printable_tail(&analysis, chunk, &self.session).is_none() {
                self.report(&analysis.diagnostics);
                failed = true;
            }
        } else {
            self.report_warnings(&analysis);
        }
        let stmts = analysis.statements_of(SESSION_FILE, SESSION_FN).unwrap_or_default();
        let tail = stmts.last().cloned();
        let ty = tail.as_ref().map(|t| match &t.kind {
            StmtKind::Expr { ty, .. } => ty.clone(),
            _ => String::new(),
        });
        let facts = tail.as_ref().and_then(|t| analysis.expr_facts(SESSION_FILE, t.at.lo, t.at.hi));
        let moves = tail
            .as_ref()
            .and_then(|t| analysis.ownership_of(SESSION_FILE, t.at.lo).map(|o| o.moves))
            .map(|after: Vec<MoveEvent>| after.iter().filter(|m| !before.contains(m)).cloned().collect())
            .unwrap_or_default();
        self.session.pop();
        Probe { failed, ty, facts, moves }
    }
}

/// What one probe analysis found. `failed` is whether the expression did not check against the session.
struct Probe {
    failed: bool,
    /// The type of the probed expression.
    ty: Option<String>,
    facts: Option<burdin_query::ExprFacts>,
    /// The places the expression moves that the session did not already move.
    moves: Vec<MoveEvent>,
}

impl Probe {
    fn outcome(&self) -> Outcome {
        if self.failed { Outcome::Failed } else { Outcome::Ok }
    }

    /// The answer of `:ownership`: the type, what the expression reads, moves, and borrows.
    fn ownership(&self) -> Option<OwnershipAnswer> {
        let facts = self.facts.as_ref()?;
        let moved: Vec<String> = self
            .moves
            .iter()
            .map(|m| {
                if m.path.is_empty() {
                    m.local.clone()
                } else {
                    let steps = m.path.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(".");
                    format!("{}:{}", m.local, steps)
                }
            })
            .collect();
        let borrows: Vec<String> = facts
            .locals
            .iter()
            .filter(|l| l.borrowed || l.borrowed_mut)
            .map(|l| if l.borrowed_mut { format!("mut {}", l.name) } else { l.name.clone() })
            .collect();
        let reads: Vec<String> = facts
            .locals
            .iter()
            .map(|l| l.name.clone())
            .filter(|name| !self.moves.iter().any(|m| &m.local == name))
            .collect();
        Some(OwnershipAnswer { ty: self.ty.clone()?, reads, moves: moved, borrows })
    }
}

/// The `:ownership` answer, as it prints.
struct OwnershipAnswer {
    ty: String,
    reads: Vec<String>,
    moves: Vec<String>,
    borrows: Vec<String>,
}

/// The trailing statement of the chunk `chunk`, when the front end's answer to the input is exactly the one
/// type error a bare non-unit expression statement gets (`expected (), found T`, E0003 at the expression
/// itself): the expression's byte range in the chunk's own text. That answer is the print wrap's trigger
/// (`spec/repl.md`, RP5); any other error is a real one the caller reports.
fn printable_tail(analysis: &Analysis, chunk: usize, session: &Session) -> Option<(u32, u32)> {
    let stmts = analysis.statements_of(SESSION_FILE, SESSION_FN)?;
    let place = text::body_place(session, chunk)?;
    let tail = stmts.last()?;
    if !matches!(tail.kind, StmtKind::Expr { .. }) {
        return None;
    }
    let only_statement_error = analysis
        .diagnostics
        .iter()
        .all(|d| d.code == Code::TYPE_MISMATCH && d.span.lo == tail.at.lo && d.span.hi == tail.at.hi);
    if !only_statement_error {
        return None;
    }
    let (lo, hi) = (tail.at.lo.saturating_sub(place.lo), tail.at.hi.saturating_sub(place.lo));
    (lo < hi).then_some((lo, hi))
}

/// Whether every error of the wrapped analysis is E0193 at the synthesized `println`'s placeholder: the value's
/// type does not implement `Debug`, and the wrap without the `println` is the answer.
fn only_placeholder_errors(analysis: &Analysis) -> bool {
    let Some(tail) = analysis.statements_of(SESSION_FILE, SESSION_FN).unwrap_or_default().last().cloned() else {
        return false;
    };
    analysis.diagnostics.has_errors()
        && analysis
            .diagnostics
            .iter()
            .all(|d| d.code == Code::NOT_FORMATTABLE && tail.at.lo <= d.span.lo && d.span.hi <= tail.at.hi)
}

/// The text of `names`, or `nothing` when the list is empty.
fn list_or_nothing(names: &[String]) -> String {
    if names.is_empty() { "nothing".to_string() } else { names.join(", ") }
}

/// The help text `:help` prints.
const HELP: &str = "\
meta-commands:
  :help                this list
  :quit                leave the REPL
  :reset               run the drops of every binding and start the session over
  :type <expr>         the type of an expression, without evaluating it
  :ownership <expr>    what an expression moves, borrows, and reads
  :explain <code>      the error reference entry for a code, as `burdin explain` prints it
  :load <file>         append a file's items to the session
  :ir <fn>             the LLVM IR of one function of the session
  :compile             build the session and run it natively

an input is an item (fn, struct, enum, use, ...), statements (let, assignment, ...), or an
expression; an expression's value prints with Debug, and a failed input leaves the session unchanged";
