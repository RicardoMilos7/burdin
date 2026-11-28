//! Memoized body checks: the cache the query library (`burdin_query`) keeps between revisions of a program
//! (`spec/query.md`).
//!
//! Checking one body reads the declarations of the whole program (its signatures, types, impls) and writes a few
//! append-only registries (generic instance requests, `any` vtables and type identities, allocator types, embedded
//! files). So a body's result is a function of four things, and the cache key is a fingerprint of exactly these:
//!
//! 1. the *item*: which function or constant, and what its type parameters stand for;
//! 2. the *text* of the declaration (the body itself);
//! 3. the *declarations*: the text of every file with the function bodies removed ([`declaration_fingerprint`]),
//!    so an edit outside a body invalidates every entry and an edit inside one invalidates only its own;
//! 4. the *registry chain*: a fingerprint of the registries as the item finds them. It starts from the content of
//!    the registries at the beginning of a pass and absorbs what each item appends, so an item whose predecessors
//!    registered something different finds a different chain and is checked again.
//!
//! A hit replays what the live check did to shared state (diagnostics, closures, instance requests, and the
//! registry entries) so the rest of the pass cannot tell the difference. Entries hold spans, and spans are byte
//! offsets: [`CheckCache::apply_edits`] moves them when a file changes, and drops the entries an edit touches.

use crate::consteval::ConstExpr;
use crate::generics::Request;
use crate::globals::Globals;
use burdin_diag::{Diagnostic, Diagnostics, FileId, SourceMap, Span};
use burdin_tir::{self as tir, AnyVTable, FnSig, Ty, visit};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A 128-bit fingerprint: two differently salted hashes of the same bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Fp(u64, u64);

/// Feeds a value to both halves of a [`Fp`].
pub(crate) struct Fingerprinter {
    a: DefaultHasher,
    b: DefaultHasher,
}

impl Fingerprinter {
    pub(crate) fn new() -> Fingerprinter {
        let a = DefaultHasher::new();
        let mut b = DefaultHasher::new();
        b.write_u64(0x9e37_79b9_7f4a_7c15);
        Fingerprinter { a, b }
    }

    pub(crate) fn finish_fp(&self) -> Fp {
        Fp(self.a.finish(), self.b.finish())
    }

    pub(crate) fn fp(&mut self, fp: Fp) {
        self.write_u64(fp.0);
        self.write_u64(fp.1);
    }
}

impl Hasher for Fingerprinter {
    fn write(&mut self, bytes: &[u8]) {
        self.a.write(bytes);
        self.b.write(bytes);
    }

    fn finish(&self) -> u64 {
        self.a.finish()
    }
}

/// The fingerprint of a string.
pub(crate) fn fp_of_str(text: &str) -> Fp {
    let mut h = Fingerprinter::new();
    text.hash(&mut h);
    h.finish_fp()
}

/// What the checker has been asked to do and what it did, for the revision that just ran. A test (or a tool that
/// wants to show progress) reads it through [`CheckCache::stats`].
#[derive(Clone, Debug, Default)]
pub struct CheckStats {
    /// The name of every body checked from scratch, in order. Generic code is checked once per pass that needs it,
    /// so a name can repeat; [`CheckStats::distinct_checked`] is the set.
    pub checked: Vec<String>,
    /// How many bodies were taken from the cache.
    pub reused: usize,
    /// How many passes over the bodies the run took (a pass that allocates a generic instance asks for another).
    pub passes: usize,
}

impl CheckStats {
    /// The names of the bodies checked from scratch, each once, in the order they were first checked.
    pub fn distinct_checked(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.checked.iter().filter(|name| seen.insert(name.as_str())).cloned().collect()
    }
}

/// A byte-range replacement in one file: bytes `lo..hi` of the old text became `new_len` bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edit {
    pub file: FileId,
    pub lo: u32,
    pub hi: u32,
    pub new_len: u32,
    /// Whether the replacement changed the number of lines. A diagnostic or trace frame that records a line number
    /// is stale when it did.
    pub lines_changed: bool,
    /// The (zero-based) line of the new text where the replacement ends. Columns on that line moved.
    pub end_line: u32,
}

impl Edit {
    fn delta(&self) -> i64 {
        i64::from(self.new_len) - i64::from(self.hi - self.lo)
    }
}

/// The values a checked body produces: a function or a constant.
#[derive(Clone)]
pub(crate) enum Value {
    Function(tir::Function),
    Const(tir::Const),
}

/// What a live check appended to the registries, so a hit can append it again.
#[derive(Clone, Default)]
struct Delta {
    requests: Vec<Request>,
    ifaces: Vec<((usize, Vec<Ty>), String)>,
    uses: Vec<(usize, Span)>,
    type_ids: Vec<Ty>,
    vtables: Vec<AnyVTable>,
    allocator: Vec<Ty>,
    embedded: Vec<String>,
    fn_bounds: Vec<FnSig>,
    sym_exprs: Vec<ConstExpr>,
}

/// The lengths of the registries and the shared closure list before an item is checked.
#[derive(Clone, Copy)]
pub(crate) struct Mark {
    requests: usize,
    ifaces: usize,
    uses: usize,
    type_ids: usize,
    vtables: usize,
    allocator: usize,
    embedded: usize,
    fn_bounds: usize,
    sym_exprs: usize,
    diags: usize,
    closures: usize,
}

impl Mark {
    pub(crate) fn take(globals: &Globals, diags: &Diagnostics, closures: usize) -> Mark {
        let any = globals.any.borrow();
        Mark {
            requests: globals.requests.borrow().len(),
            ifaces: any.ifaces.len(),
            uses: any.uses.len(),
            type_ids: any.type_ids.len(),
            vtables: any.vtables.len(),
            allocator: globals.allocator_requests.borrow().len(),
            embedded: globals.embedded.borrow().len(),
            fn_bounds: globals.fn_bounds.borrow().len(),
            sym_exprs: globals.sym_exprs.borrow().len(),
            diags: diags.len(),
            closures,
        }
    }
}

impl Delta {
    fn capture(globals: &Globals, mark: &Mark) -> Delta {
        let any = globals.any.borrow();
        Delta {
            requests: globals.requests.borrow()[mark.requests..].to_vec(),
            ifaces: (mark.ifaces..any.ifaces.len()).map(|i| (any.ifaces[i].clone(), any.names[i].clone())).collect(),
            uses: any.uses[mark.uses..].to_vec(),
            type_ids: any.type_ids[mark.type_ids..].to_vec(),
            vtables: any.vtables[mark.vtables..].to_vec(),
            allocator: globals.allocator_requests.borrow()[mark.allocator..].to_vec(),
            embedded: globals.embedded.borrow()[mark.embedded..].to_vec(),
            fn_bounds: globals.fn_bounds.borrow()[mark.fn_bounds..].to_vec(),
            sym_exprs: globals.sym_exprs.borrow()[mark.sym_exprs..].to_vec(),
        }
    }

    /// Appends what the live check appended. The registries are in the state the live check found them in (that is
    /// what the chain fingerprint guarantees), so every entry lands at the index it had.
    fn apply(&self, globals: &Globals) {
        globals.requests.borrow_mut().extend(self.requests.iter().cloned());
        {
            let mut any = globals.any.borrow_mut();
            for (key, name) in &self.ifaces {
                let id = any.ifaces.len();
                any.iface_index.insert(key.clone(), id);
                any.ifaces.push(key.clone());
                any.names.push(name.clone());
                any.methods.push(None);
                any.supers.push(None);
            }
            for entry in &self.uses {
                if !any.uses.contains(entry) {
                    any.uses.push(*entry);
                }
            }
            for ty in &self.type_ids {
                let id = any.type_ids.len();
                any.type_id_index.insert(ty.clone(), id);
                any.type_ids.push(ty.clone());
            }
            for vtable in &self.vtables {
                let id = any.vtables.len();
                any.vtable_index.insert((vtable.concrete.clone(), vtable.iface), id);
                any.vtables.push(vtable.clone());
            }
        }
        globals.allocator_requests.borrow_mut().extend(self.allocator.iter().cloned());
        globals.embedded.borrow_mut().extend(self.embedded.iter().cloned());
        globals.fn_bounds.borrow_mut().extend(self.fn_bounds.iter().cloned());
        globals.sym_exprs.borrow_mut().extend(self.sym_exprs.iter().cloned());
    }

    /// What this delta adds to the chain: everything a later item can observe, and nothing a span or a request
    /// (outputs no item reads) could change.
    fn absorb_into(&self, h: &mut Fingerprinter) {
        self.ifaces.hash(h);
        self.type_ids.hash(h);
        for v in &self.vtables {
            v.concrete.hash(h);
            v.type_id.hash(h);
            v.iface.hash(h);
            v.entries.iter().for_each(|f| f.0.hash(h));
        }
        self.allocator.hash(h);
        self.embedded.hash(h);
        self.fn_bounds.hash(h);
        format!("{:?}", self.sym_exprs).hash(h);
    }
}

/// The fingerprint of the registries as a pass finds them: the chain starts here.
fn registry_fingerprint(globals: &Globals) -> Fp {
    let mut h = Fingerprinter::new();
    let any = globals.any.borrow();
    any.ifaces.hash(&mut h);
    any.type_ids.hash(&mut h);
    for v in &any.vtables {
        v.concrete.hash(&mut h);
        v.type_id.hash(&mut h);
        v.iface.hash(&mut h);
        v.entries.iter().for_each(|f| f.0.hash(&mut h));
    }
    globals.allocator_requests.borrow().hash(&mut h);
    globals.embedded.borrow().hash(&mut h);
    globals.fn_bounds.borrow().hash(&mut h);
    format!("{:?}", globals.sym_exprs.borrow()).hash(&mut h);
    h.finish_fp()
}

/// One memoized body.
struct Entry {
    /// Where the declaration was when the entry was made, kept current by [`CheckCache::apply_edits`]. A hit
    /// needs the declaration to be where the entry says it is.
    span: Span,
    value: Value,
    /// The closures the body created, and the length of the shared list they were appended to.
    closures: Vec<tir::Closure>,
    closure_base: usize,
    diags: Vec<Diagnostic>,
    delta: Delta,
    /// The chain after this item.
    chain_out: Fp,
    /// The body recorded a source line or column (a trace frame or an assertion's location).
    located: bool,
    /// The files any span of the entry points into (almost always one file, so a tiny `Vec` avoids a `HashSet` per entry).
    files: Vec<FileId>,
    /// The last revision that used the entry, for [`CheckCache::evict`].
    used: u64,
}

/// The cache of checked bodies a [`crate::check_cached`] run reads and fills.
pub struct CheckCache {
    enabled: bool,
    entries: HashMap<Fp, Entry>,
    /// The declaration fingerprint the entries were made under.
    declarations: Option<Fp>,
    generation: u64,
    stats: CheckStats,
    cancel: Option<Arc<AtomicBool>>,
}

impl CheckCache {
    /// A cache that remembers bodies between runs.
    pub fn new() -> CheckCache {
        CheckCache {
            enabled: true,
            entries: HashMap::new(),
            declarations: None,
            generation: 0,
            stats: CheckStats::default(),
            cancel: None,
        }
    }

    /// A cache that stores nothing: every run checks every body, as [`crate::check`] does.
    pub fn disabled() -> CheckCache {
        let mut cache = CheckCache::new();
        cache.enabled = false;
        cache
    }

    /// Turns storing and reuse on or off for the runs that follow. The query library turns it off for a program it
    /// does not return (one with syntax errors), so that such a run leaves the answers of the last checked program in
    /// place for the next edit to reuse.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// The flag a run polls between bodies; setting it makes [`crate::check_cached`] return `None`.
    pub fn set_cancel(&mut self, cancel: Option<Arc<AtomicBool>>) {
        self.cancel = cancel;
    }

    /// What the last run checked and reused.
    pub fn stats(&self) -> &CheckStats {
        &self.stats
    }

    /// How many bodies are remembered.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Forgets every body.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.declarations = None;
    }

    /// Keeps the `keep` most recently used bodies and forgets the rest. A run uses every body it needs, so after a
    /// run the entries it did not touch are the ones no revision of the program asks for any more.
    pub fn evict(&mut self, keep: usize) {
        if self.entries.len() <= keep {
            return;
        }
        let mut used: Vec<u64> = self.entries.values().map(|e| e.used).collect();
        used.sort_unstable_by(|a, b| b.cmp(a));
        let cutoff = used[keep - 1];
        let mut kept = 0usize;
        self.entries.retain(|_, e| {
            if e.used > cutoff {
                kept += 1;
                return true;
            }
            if e.used == cutoff && kept < keep {
                kept += 1;
                return true;
            }
            false
        });
    }

    /// Moves every span an entry holds across the edits, and drops the entries an edit touches. Call it before a
    /// run that follows a change to a file's text, with one edit per changed file; `sources` is the new text.
    pub fn apply_edits(&mut self, edits: &[Edit], sources: &SourceMap) {
        if edits.is_empty() || self.entries.is_empty() {
            return;
        }
        let touched: HashSet<FileId> = edits.iter().map(|e| e.file).collect();
        self.entries.retain(|_, entry| {
            if !entry.files.iter().any(|f| touched.contains(f)) {
                return true;
            }
            // A body that recorded a line or column must not move across a change that renumbers lines or shifts
            // columns on its own line.
            if entry.located {
                let line = |span: Span| sources.get(span.file).line_col(span.lo).0.saturating_sub(1);
                for edit in edits.iter().filter(|e| entry.files.contains(&e.file)) {
                    let after = entry.span.file == edit.file && entry.span.lo >= edit.hi;
                    if edit.lines_changed || (after && line(shifted(entry.span, edit)) <= edit.end_line) {
                        return false;
                    }
                }
            }
            let mut keep = true;
            let mut files = Vec::new();
            let mut shift = |span: &mut Span| {
                for edit in edits {
                    if span.file != edit.file {
                        continue;
                    }
                    if span.hi <= edit.lo {
                        // before the edit: unchanged
                    } else if span.lo >= edit.hi {
                        let delta = edit.delta();
                        span.lo = (i64::from(span.lo) + delta) as u32;
                        span.hi = (i64::from(span.hi) + delta) as u32;
                    } else {
                        keep = false;
                    }
                }
                if !files.contains(&span.file) {
                    files.push(span.file);
                }
            };
            entry.map_spans(&mut shift);
            if keep {
                entry.files = files;
            }
            keep
        });
    }
}

impl Default for CheckCache {
    fn default() -> Self {
        CheckCache::new()
    }
}

/// `span` after `edit`, when it lies after the edit.
fn shifted(span: Span, edit: &Edit) -> Span {
    let delta = edit.delta();
    Span::new(span.file, (i64::from(span.lo) + delta) as u32, (i64::from(span.hi) + delta) as u32)
}

impl Entry {
    /// Visits every span the entry holds.
    fn map_spans(&mut self, f: &mut dyn FnMut(&mut Span)) {
        f(&mut self.span);
        match &mut self.value {
            Value::Function(func) => map_function_spans(func, f),
            Value::Const(c) => {
                f(&mut c.span);
                c.locals.iter_mut().for_each(|l| f(&mut l.span));
                map_expr_spans(&mut c.value, f);
            }
        }
        for closure in &mut self.closures {
            f(&mut closure.span);
            map_function_spans(&mut closure.func, f);
        }
        for d in &mut self.diags {
            f(&mut d.span);
            for n in &mut d.notes {
                if let Some(span) = &mut n.span {
                    f(span);
                }
            }
            for e in d.suggestions.iter_mut().flat_map(|s| s.edits.iter_mut()) {
                f(&mut e.span);
            }
        }
        for (_, span) in &mut self.delta.uses {
            f(span);
        }
    }
}

fn map_function_spans(func: &mut tir::Function, f: &mut dyn FnMut(&mut Span)) {
    f(&mut func.span);
    func.locals.iter_mut().for_each(|l| f(&mut l.span));
    let mut walker = SpanWalker { f };
    visit::VisitMut::block(&mut walker, &mut func.body);
}

fn map_expr_spans(expr: &mut tir::Expr, f: &mut dyn FnMut(&mut Span)) {
    let mut walker = SpanWalker { f };
    visit::VisitMut::expr(&mut walker, expr);
}

struct SpanWalker<'a> {
    f: &'a mut dyn FnMut(&mut Span),
}

impl visit::VisitMut for SpanWalker<'_> {
    fn expr(&mut self, e: &mut tir::Expr) {
        (self.f)(&mut e.span);
        visit::walk_expr_mut(self, e);
    }
    fn block(&mut self, b: &mut tir::Block) {
        // The closing brace is where the scope's values drop (`spec/check.md`, CK10).
        (self.f)(&mut b.close);
        visit::walk_block_mut(self, b);
    }
}

/// The fingerprint of everything in the program but the function bodies: the package layout, the file paths, the
/// options that change what a body produces, and the text of every file with each body removed. A body edit leaves
/// it alone; any other edit changes it.
pub(crate) fn declaration_fingerprint(
    packages: &[crate::Package<'_>],
    entry: (usize, usize),
    mode: crate::EntryMode,
    traces: bool,
    sources: &SourceMap,
) -> Fp {
    let mut h = Fingerprinter::new();
    (entry, mode, traces).hash(&mut h);
    for package in packages {
        (&package.name, package.is_root, package.edition, &package.deps, package.root_module).hash(&mut h);
        for module in &package.modules {
            let file = sources.get(module.file);
            (&module.path, &file.path).hash(&mut h);
            let text = file.text.as_str();
            let mut bodies: Vec<Span> = module.ast.bodies.clone();
            bodies.sort_by_key(|s| s.lo);
            let mut at = 0usize;
            for body in bodies {
                let (lo, hi) = (body.lo as usize, body.hi as usize);
                // A body nested in another (not in the grammar today) is already removed with its parent.
                if lo < at || hi > text.len() {
                    continue;
                }
                text[at..lo].hash(&mut h);
                // The body is gone, but a declaration's text must not run into the next one.
                0xB0D1u16.hash(&mut h);
                at = hi;
            }
            text[at..].hash(&mut h);
        }
    }
    h.finish_fp()
}

/// The state of one pass over the bodies: the chain fingerprint the next item finds the registries in.
pub(crate) struct Pass {
    chain: Fp,
}

impl CheckCache {
    /// Starts a run: forgets the entries a different set of declarations made, and resets the counters. A run with
    /// the cache switched off stores and reads nothing, so it keeps the entries of the last run: they are valid again
    /// when the declarations of that run come back (a partial program's declarations differ only while it is partial).
    pub(crate) fn begin(&mut self, declarations: Fp) {
        self.generation += 1;
        self.stats = CheckStats::default();
        if !self.enabled {
            return;
        }
        if self.declarations != Some(declarations) {
            self.entries.clear();
            self.declarations = Some(declarations);
        }
    }

    /// Starts a pass over the bodies. `instances` fingerprints the generic instances allocated so far: bodies read
    /// the methods and signatures of instances, so a body checked before an instance existed is not the same body
    /// checked after it.
    pub(crate) fn begin_pass(&mut self, globals: &Globals, instances: Fp) -> Pass {
        self.stats.passes += 1;
        let mut h = Fingerprinter::new();
        h.fp(registry_fingerprint(globals));
        h.fp(instances);
        Pass { chain: h.finish_fp() }
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(|flag| flag.load(Ordering::Relaxed))
    }
}

/// Which body is being checked.
pub(crate) struct ItemKey {
    /// The item and what its type parameters stand for.
    pub(crate) identity: Fp,
    /// The text of its declaration.
    pub(crate) text: Fp,
    /// Where the declaration is.
    pub(crate) span: Span,
    /// The name [`CheckStats`] reports.
    pub(crate) name: String,
    /// A symbolic body's closures are discarded, so they neither occupy the shared list nor depend on it.
    pub(crate) symbolic: bool,
}

type ClosureSink = std::rc::Rc<std::cell::RefCell<Vec<tir::Closure>>>;

impl CheckCache {
    /// Checks one item through the cache. `live` runs the real check when there is no usable entry.
    pub(crate) fn run(
        &mut self,
        pass: &mut Pass,
        globals: &Globals,
        key: &ItemKey,
        diags: &mut Diagnostics,
        sink: &ClosureSink,
        live: impl FnOnce(&mut Diagnostics) -> Value,
    ) -> Value {
        let sink_len = if key.symbolic { 0 } else { sink.borrow().len() };
        let mut h = Fingerprinter::new();
        h.fp(key.identity);
        h.fp(key.text);
        h.fp(self.declarations.unwrap_or(Fp(0, 0)));
        h.fp(pass.chain);
        let entry_key = h.finish_fp();
        if self.enabled
            && let Some(entry) = self.entries.get_mut(&entry_key)
            && entry.span == key.span
            && (entry.closures.is_empty() || entry.closure_base == sink_len)
        {
            entry.used = self.generation;
            for d in &entry.diags {
                diags.push(d.clone());
            }
            if !key.symbolic {
                sink.borrow_mut().extend(entry.closures.iter().cloned());
            }
            entry.delta.apply(globals);
            pass.chain = entry.chain_out;
            self.stats.reused += 1;
            return entry.value.clone();
        }
        globals.impure.set(false);
        globals.located.set(false);
        let mark = Mark::take(globals, diags, sink_len);
        let value = live(diags);
        self.stats.checked.push(key.name.clone());
        let delta = Delta::capture(globals, &mark);
        let mut h = Fingerprinter::new();
        h.fp(pass.chain);
        delta.absorb_into(&mut h);
        let chain_out = h.finish_fp();
        // A body that read a file (`embed`) depends on something the cache cannot see change.
        if self.enabled && !globals.impure.get() {
            let closures = if key.symbolic { Vec::new() } else { sink.borrow()[mark.closures..].to_vec() };
            let mut entry = Entry {
                span: key.span,
                value: value.clone(),
                closures,
                closure_base: sink_len,
                diags: diags.iter().skip(mark.diags).cloned().collect(),
                delta,
                chain_out,
                located: globals.located.get(),
                files: Vec::new(),
                used: self.generation,
            };
            let mut files = Vec::new();
            entry.map_spans(&mut |span: &mut Span| {
                if !files.contains(&span.file) {
                    files.push(span.file);
                }
            });
            entry.files = files;
            self.entries.insert(entry_key, entry);
        }
        pass.chain = chain_out;
        value
    }
}
