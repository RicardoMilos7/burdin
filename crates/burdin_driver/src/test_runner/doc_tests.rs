//! Doc tests: the Burdin code blocks of `///` and `//!` comments, each compiled as its own program against the
//! documented package and run like a `test` block (`spec/test-runner.md`, "Doc tests"; issues #650, #651, #652).
//!
//! Extraction walks the same doc comments `burdin doc` reads (`burdin_syntax::docs`) and parses them with the same
//! Markdown (`burdin_doc::markdown`), and keeps every Burdin block: untagged, or tagged `burdin` or `bd`, with the
//! words after the language as attributes. The lines a hidden `# ` marker starts are compiled and never shown.
//!
//! Building one doc test writes a one-file package: a `burdin.toml` that depends on the documented package (its
//! public API only, because a dependency sees no more) and a `src/main.bd` that holds the example, wrapped in
//! `fn main() { ... }` when it declares no `main`, with its declarations hoisted above the wrapper and its lines
//! padded so the example lands on the line of its doc comment. A failure is reported there: the compile's
//! diagnostics are rewritten from the generated file to the original one before they are shown.

use super::Outcome;
use crate::package::{self, Workspace};
use crate::{CompileOptions, EntryMode, FrontEnd, SourceOverrides, emit_program, front_end, render_options};
use burdin_diag::{Diagnostic, Diagnostics, FileId, Severity, SourceMap, Span};
use burdin_doc::markdown::{self, Block, BlockLine};
use burdin_syntax::docs::{self, DocComment};
use burdin_syntax::{ast, parse as parse_ast};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The attributes of a doc block's info string (`spec/test-runner.md`, "Doc-test attributes").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct DocAttrs {
    pub(super) ignore: bool,
    pub(super) no_run: bool,
    pub(super) should_panic: bool,
    pub(super) compile_fail: bool,
    pub(super) compile_fail_code: Option<String>,
}

/// One doc test: a Burdin code block of one doc comment, with everything building and reporting it need.
#[derive(Clone, Debug)]
pub(super) struct DocExample {
    /// The package whose doc comment holds the block.
    pub(super) package: String,
    /// The dotted module path of the file, as a test block's is; empty for the entry file.
    pub(super) module: String,
    /// The documented item (`Point.new`), or the file's stem for module documentation.
    pub(super) owner: String,
    /// The file as diagnostics spell it.
    pub(super) file: String,
    /// The source line of the block's opening fence.
    pub(super) line: u32,
    pub(super) attrs: DocAttrs,
    /// How the generated package reaches the documented package.
    pub(super) dep: Dep,
    /// The example's lines: as the compiler reads them, and where each one is in the original file.
    pub(super) lines: Vec<DocLine>,
    /// The byte range of the fence line's text in the original file, for a diagnostic that belongs to no line of
    /// the example (the generated `fn main` wrapper).
    pub(super) fence: (u32, u32),
    /// The original file's text, shared by every doc test of that file: it is the source map a failure renders
    /// with, and the copy a single-file target makes of itself.
    pub(super) text: Arc<String>,
}

/// What the generated package depends on.
#[derive(Clone, Debug)]
pub(super) enum Dep {
    /// A package directory with an importable root module (`src/<name>.bd` or `src/lib.bd`).
    Package(PathBuf),
    /// A single-file target: the file is copied into a package of the build directory.
    File,
    /// The package has no importable root module, or a name a manifest cannot hold, so an example cannot name it.
    None,
}

/// One line of a doc-test block: as the compiler reads it (the `# ` marker removed, the indentation kept), and its
/// byte range in the original file: the text after `///` (or `//!`) and one optional space, to the end of the line.
#[derive(Clone, Debug)]
pub(super) struct DocLine {
    pub(super) line: BlockLine,
    pub(super) lo: u32,
    pub(super) hi: u32,
}

/// What building one doc test produced.
pub(super) enum Built {
    /// A compiled example waiting for its process.
    Exe(PathBuf),
    /// The test is decided without a process: `no_run`, `compile_fail`, or a compile failure.
    Done(Outcome),
}

// --- extraction -------------------------------------------------------------------------------------------------

/// The doc tests as test cases: the identity fields a `test` block gets, with the owner and line of the doc
/// comment in the name and the original file and line as the location.
pub(super) fn cases(examples: &[DocExample], qualify: bool) -> Vec<super::TestCase> {
    examples
        .iter()
        .map(|example| {
            let name = format!("{} (line {})", example.owner, example.line);
            let local = if example.module.is_empty() { name.clone() } else { format!("{}.{}", example.module, name) };
            let (display, path) = if qualify {
                (format!("{}::{}", example.package, name), format!("{}::{}", example.package, local))
            } else {
                (name.clone(), local.clone())
            };
            super::TestCase {
                package: example.package.clone(),
                module: example.module.clone(),
                name,
                local,
                display,
                path,
                file: example.file.clone(),
                line: example.line,
                run: super::Run::Doc(example.clone()),
                attrs: super::Attrs::of_doc(&example.attrs),
            }
        })
        .collect()
}

/// Collects the doc tests of the selected packages: every Burdin code block of every `///` and `//!` comment of
/// their source files, in module order and then in source order within a file. Warnings about a block's unknown
/// attributes, and a note about a package an example cannot import, go to standard error as they are found.
pub(super) fn collect(workspace: &Workspace, selected: &[usize]) -> Vec<DocExample> {
    let mut out = Vec::new();
    for &index in selected {
        let package = &workspace.packages[index];
        // Other packages reach this one as `use <package>` through its root module; without one an example has no
        // way to name anything of it. A single-file target is a package of one module, but only when its stem is a
        // name a manifest can hold.
        let (dep, why) = match (&package.dir, package.root_module.is_some()) {
            (Some(dir), true) => (Dep::Package(dir.clone()), None),
            (Some(_), false) => (
                Dep::None,
                Some(format!(
                    "package `{}` has no importable root module (`src/{0}.bd` or `src/lib.bd`); its doc tests cannot import it",
                    package.name
                )),
            ),
            (None, _) if package::is_ident(&package.name) => (Dep::File, None),
            (None, _) => (
                Dep::None,
                Some(format!(
                    "the file's stem `{}` is not an identifier, so it cannot be a dependency; its doc tests cannot import it",
                    package.name
                )),
            ),
        };
        let mut of_package = Vec::new();
        // The entry file of the package (`src/main.bd`, or the root module a library package starts from) has the
        // empty module path in a test's full name, exactly as a `test` block's does. The rule is the loader's
        // (`package.rs`): `src/main.bd`, then the root module, then the first module.
        let entry = if index == workspace.entry_package {
            workspace.entry_module
        } else {
            package
                .modules
                .iter()
                .position(|m| m.path.len() == 1 && m.path[0] == "main")
                .or(package.root_module)
                .unwrap_or(0)
        };
        for (module_index, module) in package.modules.iter().enumerate() {
            let Ok(text) = std::fs::read_to_string(&module.file) else { continue };
            let stem =
                module.file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "module".into());
            let path = if module_index == entry { "" } else { &module.path.join(".") };
            of_package.extend(file_examples(
                &package.name,
                path,
                &dep,
                &package::source_path_text(&module.file),
                &stem,
                text,
            ));
        }
        if let (Some(why), true) = (&why, !of_package.is_empty()) {
            eprintln!("note: {why}");
        }
        out.extend(of_package);
    }
    out
}

/// The doc tests of one source file. Pure, so the unit tests can drive it with a string.
fn file_examples(package: &str, module: &str, dep: &Dep, file: &str, stem: &str, text: String) -> Vec<DocExample> {
    let line_starts = line_starts_of(&text);
    let file_id = FileId(0);
    let mut diags = Diagnostics::default();
    let ast = parse_ast(file_id, &text, &mut diags);
    let text = Arc::new(text);
    // What every doc test of this file shares.
    let cx = FileContext { package, module, dep, file, text: &text, line_starts: &line_starts };
    let mut out = Vec::new();
    if let Some(comment) = docs::module_docs(file_id, &text) {
        out.extend(cx.comment_examples(stem, &comment));
    }
    for item in &ast.items {
        for (owner, offset) in doc_targets(&text, item) {
            if let Some(comment) = docs::doc_before(file_id, &text, offset) {
                out.extend(cx.comment_examples(&owner, &comment));
            }
        }
    }
    out
}

/// What every doc test of one file shares: the identity fields a test case needs and the text the block's lines
/// are mapped into.
struct FileContext<'a> {
    package: &'a str,
    module: &'a str,
    dep: &'a Dep,
    file: &'a str,
    text: &'a Arc<String>,
    line_starts: &'a [u32],
}

/// The places a doc comment can sit on one item: the item itself and the members its declaration holds (fields,
/// variants, methods). The owner is the name a report shows.
fn doc_targets(text: &str, item: &ast::Item) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    let name_of = |name: &ast::Ident| name.name.clone();
    let push = |out: &mut Vec<(String, u32)>, name: String, span: Span| out.push((name, span.lo));
    match item {
        ast::Item::Fn(f) => push(&mut out, name_of(&f.name), f.span),
        ast::Item::Extension(x) => {
            let recv = docs::type_name(&x.recv).unwrap_or("_");
            push(&mut out, format!("{recv}.{}", x.func.name.name), x.func.span)
        }
        ast::Item::Const(c) => push(&mut out, name_of(&c.name), c.span),
        ast::Item::Struct(s) => {
            push(&mut out, name_of(&s.name), s.span);
            for field in &s.fields {
                push(&mut out, format!("{}.{}", s.name.name, field.name.name), field.name.span);
            }
        }
        ast::Item::Enum(e) => {
            push(&mut out, name_of(&e.name), e.span);
            for variant in &e.variants {
                push(&mut out, format!("{}.{}", e.name.name, variant.name.name), variant.name.span);
            }
        }
        // The parser lowers an `error enum` before anything else sees it; the arm is here so a future change to
        // that order cannot silently drop its doc tests.
        ast::Item::ErrorEnum(e) => {
            push(&mut out, name_of(&e.name), e.span);
            for variant in &e.variants {
                push(&mut out, format!("{}.{}", e.name.name, variant.name.name), variant.name.span);
            }
        }
        ast::Item::TypeAlias(a) => push(&mut out, name_of(&a.name), a.span),
        ast::Item::Impl(b) => {
            // The parser adds the impl blocks a `derives (...)` clause and an `error enum` generate; their methods
            // carry no doc comment of their own.
            if docs::is_generated(text, b.span) && b.span == b.ty.span {
                return out;
            }
            let recv = docs::type_name(&b.ty).unwrap_or("_");
            for method in &b.methods {
                push(&mut out, format!("{recv}.{}", method.name.name), method.span);
            }
        }
        ast::Item::Interface(i) => {
            push(&mut out, name_of(&i.name), i.span);
            for method in &i.methods {
                push(&mut out, format!("{}.{}", i.name.name, method.decl.name.name), method.decl.span);
            }
        }
        ast::Item::Extern(block) => {
            for decl in &block.decls {
                push(&mut out, name_of(&decl.name), decl.span);
            }
        }
        ast::Item::ExternFn(x) => push(&mut out, name_of(&x.decl.name), x.decl.span),
        ast::Item::Derive(d) => {
            push(&mut out, name_of(&d.name), d.span);
            for method in &d.methods {
                push(&mut out, format!("{}.{}", d.name.name, method.name.name), method.span);
            }
        }
    }
    out
}

/// The Burdin code blocks of one doc comment, as doc tests. The Markdown is the one `burdin doc` renders with, so
/// the two agree about where a block starts and what its info string says.
impl FileContext<'_> {
    fn comment_examples(&self, owner: &str, comment: &DocComment) -> Vec<DocExample> {
        let blocks = markdown::parse(&comment.text);
        let mut code = Vec::new();
        code_blocks(&blocks, &mut code);
        let mut out = Vec::new();
        for block in code {
            let Block::Code { info, text: block_text, line } = block else {
                unreachable!("only code blocks were kept")
            };
            if !markdown::is_burdin_info(info) {
                continue;
            }
            let lines = markdown::block_lines(block_text);
            if lines.is_empty() {
                // An empty block has nothing to test.
                continue;
            }
            let (attrs, warnings) = parse_attrs(info);
            let fence_span = comment.lines[*line];
            for warning in warnings {
                eprintln!("warning: {}:{}: {warning}", self.file, line_of(self.line_starts, fence_span.lo));
            }
            let mut doc_lines = Vec::with_capacity(lines.len());
            for (j, content) in lines.iter().enumerate() {
                let (lo, hi) = text_range_of(comment.lines[*line + 1 + j], self.text);
                doc_lines.push(DocLine { line: content.clone(), lo, hi });
            }
            out.push(DocExample {
                package: self.package.to_string(),
                module: self.module.to_string(),
                owner: owner.to_string(),
                file: self.file.to_string(),
                line: line_of(self.line_starts, fence_span.lo),
                attrs,
                dep: self.dep.clone(),
                lines: doc_lines,
                fence: text_range_of(fence_span, self.text),
                text: self.text.clone(),
            });
        }
        out
    }
}

/// The fenced code blocks of a doc text, in document order, including the ones nested in a quote or a list.
fn code_blocks<'a>(blocks: &'a [Block], out: &mut Vec<&'a Block>) {
    for block in blocks {
        match block {
            Block::Code { .. } => out.push(block),
            Block::Quote(inner) => code_blocks(inner, out),
            Block::List { items, .. } => items.iter().for_each(|item| code_blocks(item, out)),
            _ => {}
        }
    }
}

/// Parses the attributes of a Burdin block's info string: the words after the language, comma or space separated.
/// The language is the first word before the first comma, so an untagged block writes its attributes after a
/// comma (`",no_run"`). Returns the attributes and a warning for every word that is not one.
fn parse_attrs(info: &str) -> (DocAttrs, Vec<String>) {
    let mut attrs = DocAttrs::default();
    let mut warnings = Vec::new();
    let mut words: Vec<&str> = Vec::new();
    for (index, segment) in info.split(',').enumerate() {
        let mut segment: Vec<&str> = segment.split_whitespace().collect();
        if index == 0 && !segment.is_empty() {
            // The first word of the first segment is the language, not an attribute.
            segment.remove(0);
        }
        words.extend(segment);
    }
    let mut words = words.into_iter().peekable();
    while let Some(word) = words.next() {
        match word {
            "ignore" => attrs.ignore = true,
            "no_run" => attrs.no_run = true,
            "should_panic" => attrs.should_panic = true,
            "compile_fail" => {
                attrs.compile_fail = true;
                if let Some(code) = words.peek().copied().filter(|code| code.starts_with('E')) {
                    if is_error_code(code) {
                        attrs.compile_fail_code = Some(code.to_string());
                    } else {
                        warnings
                            .push(format!("`compile_fail` needs an error code like E0123, got `{code}`; ignoring it"));
                    }
                    words.next();
                }
            }
            other => warnings.push(format!("unknown code-block attribute `{other}`")),
        }
    }
    (attrs, warnings)
}

/// `E` and four digits, the shape of an error code in the registry.
fn is_error_code(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.len() == 5 && bytes[0] == b'E' && bytes[1..].iter().all(u8::is_ascii_digit)
}

/// The byte range of one doc-comment line's text in the original file: after the `///` (or `//!`) marker and the
/// one optional space, to the end of the line.
fn text_range_of(span: Span, text: &str) -> (u32, u32) {
    let marker = span.lo as usize + 3;
    let space = usize::from(text.as_bytes().get(marker) == Some(&b' '));
    (((marker + space) as u32).min(span.hi), span.hi)
}

/// The 1-based line of a byte offset, from the file's line-start table.
fn line_of(line_starts: &[u32], offset: u32) -> u32 {
    line_starts.partition_point(|start| *start <= offset) as u32
}

fn line_starts_of(text: &str) -> Vec<u32> {
    let mut starts = vec![0u32];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i as u32 + 1);
        }
    }
    starts
}

// --- generation -------------------------------------------------------------------------------------------------

/// The program of one doc test and how its lines map back to the doc comment.
#[derive(Debug)]
pub(super) struct Program {
    /// The generated `src/main.bd`.
    pub(super) text: String,
    /// For each generated line: the example line it holds (an index into `DocExample.lines`), or `None` for the
    /// padding, the `fn main` wrapper, and the closing brace.
    line_of: Vec<Option<usize>>,
    line_starts: Vec<u32>,
}

impl Program {
    /// The 0-based generated line of a byte offset, and the offset within that line.
    fn locate(&self, offset: u32) -> (usize, u32) {
        let line = self.line_starts.partition_point(|start| *start <= offset) - 1;
        (line, offset - self.line_starts[line])
    }
}

/// Builds the program of one doc test. The example is the whole program when it declares a `fn main`; otherwise
/// its leading declarations (the longest prefix that parses as module items) are hoisted above a `fn main` wrapper
/// that holds the remaining statements. Padding lines put the example on the line of its doc comment, so a panic
/// it raises reports the line a reader sees.
pub(super) fn generate(example: &DocExample) -> Program {
    let content: Vec<&str> = example.lines.iter().map(|line| line.line.compiled()).collect();
    let mut text = String::new();
    let mut line_of: Vec<Option<usize>> = Vec::new();
    let push = |text: &mut String, line_of: &mut Vec<Option<usize>>, content: Option<usize>, line: &str| {
        text.push_str(line);
        text.push('\n');
        line_of.push(content);
    };
    // The padding is a comment line, so the generated file stays valid above the example.
    let pad = |text: &mut String, line_of: &mut Vec<Option<usize>>, count: u32| {
        for _ in 0..count {
            text.push_str("// doc test\n");
            line_of.push(None);
        }
    };
    if content.iter().any(|line| declares_main(line)) {
        // The example is a program: its first line sits `fence` lines down, on its own source line.
        pad(&mut text, &mut line_of, example.line);
        for (j, line) in content.iter().enumerate() {
            push(&mut text, &mut line_of, Some(j), line);
        }
    } else {
        let hoisted = longest_item_prefix(&content);
        // The first statement sits on its own source line: the padding, the wrapper, then the statements.
        pad(&mut text, &mut line_of, (example.line + hoisted as u32).saturating_sub(1));
        push(&mut text, &mut line_of, None, "fn main() {");
        for (j, line) in content.iter().enumerate().skip(hoisted) {
            push(&mut text, &mut line_of, Some(j), line);
        }
        push(&mut text, &mut line_of, None, "}");
        for (j, line) in content.iter().enumerate().take(hoisted) {
            push(&mut text, &mut line_of, Some(j), line);
        }
    }
    let line_starts = line_starts_of(&text);
    Program { text, line_of, line_starts }
}

/// Whether a line of an example declares the program's entry point.
fn declares_main(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix("fn main") else { return false };
    rest.is_empty() || rest.starts_with(['(', ' ', '\t', ':'])
}

/// The longest prefix of an example's lines that parses as module items: those are hoisted above the wrapper. Zero
/// when the example is statements only.
fn longest_item_prefix(content: &[&str]) -> usize {
    for k in (1..=content.len()).rev() {
        if parses_clean(&content[..k].join("\n")) {
            return k;
        }
    }
    0
}

fn parses_clean(text: &str) -> bool {
    let mut diags = Diagnostics::default();
    parse_ast(FileId(0), text, &mut diags);
    !diags.has_errors()
}

// --- building ---------------------------------------------------------------------------------------------------

/// Builds the doc tests the plan selected and did not ignore, in parallel (at most `jobs` at once), each into its
/// own package under `root`. `doc` holds the plan positions of those doc tests and `tests` the plan itself; every
/// build comes back as its plan position with what it produced.
pub(super) fn build(
    root: &Path,
    doc: &[usize],
    tests: &[super::Planned],
    options: &CompileOptions,
    jobs: Option<usize>,
    stream: Option<&super::report::Stream>,
) -> Result<Vec<(usize, Built)>, ExitCode> {
    let jobs = jobs.unwrap_or_else(default_jobs).min(doc.len()).max(1);
    // With more than one example building at once, a compile's own threads would multiply the workers; one build
    // alone still uses every thread the machine has.
    let per_compile = if doc.len() > 1 { 1 } else { options.jobs };
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results = std::sync::Mutex::new(Vec::new());
    let failure = std::sync::Mutex::new(None::<ExitCode>);
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                loop {
                    let slot = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(&plan_index) = doc.get(slot) else { break };
                    if failure.lock().unwrap().is_some() {
                        break;
                    }
                    let super::Run::Doc(example) = &tests[plan_index].case.run else {
                        unreachable!("a doc index points at a doc test")
                    };
                    match build_one(root, plan_index, example, options, per_compile) {
                        Ok(built) => {
                            // A decided test never starts a process, so its two events are emitted here; one that
                            // will run reports its start when the runner starts the process.
                            if let (Some(stream), Built::Done(outcome)) = (stream, &built) {
                                stream.test_started(&tests[plan_index]);
                                stream.test_finished(&tests[plan_index], outcome);
                            }
                            results.lock().unwrap().push((plan_index, built));
                        }
                        Err(code) => {
                            *failure.lock().unwrap() = Some(code);
                            break;
                        }
                    }
                }
            });
        }
    });
    match failure.into_inner().unwrap() {
        Some(code) => Err(code),
        None => Ok(results.into_inner().unwrap()),
    }
}

fn default_jobs() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

/// Writes one doc test's package and compiles it.
fn build_one(
    root: &Path,
    slot: usize,
    example: &DocExample,
    options: &CompileOptions,
    per_compile: usize,
) -> Result<Built, ExitCode> {
    let started = Instant::now();
    let dir = root.join("doc").join(format!("doctest-{slot}"));
    let program = generate(example);
    let name = entry_name(&example.package);
    write_package(&dir, name, example, &program.text)?;
    let doc_options = CompileOptions {
        release: options.release,
        debug: options.debug,
        strip: options.strip,
        lto: options.lto,
        pgo_generate: options.pgo_generate,
        pgo_use: options.pgo_use,
        emit_ir: None,
        link_libs: options.link_libs,
        mode: EntryMode::Build,
        diagnostics: options.diagnostics,
        comptime_steps: options.comptime_steps,
        emit_deps: None,
        report_clean: false,
        run_build_scripts: true,
        jobs: per_compile,
    };
    let FrontEnd { files, sources, program: checked, diags, .. } =
        front_end(&dir, &doc_options, &SourceOverrides::new())?;
    // The compiler lists canonical paths (`\\?\` on Windows, `/private/var` on macOS), so compare canonical forms.
    let entry_file = dir.join("src").join("main.bd");
    let entry_file = entry_file.canonicalize().unwrap_or(entry_file);
    let generated = files
        .iter()
        .position(|(path, _)| path == &entry_file || path.canonicalize().is_ok_and(|canonical| canonical == entry_file))
        .map(|index| FileId(index as u32))
        .expect("the generated package's entry file is among the compiled files");
    if diags.has_errors() {
        return Ok(failed_build(example, &program, diags, &sources, generated, &doc_options, started.elapsed()));
    }
    if example.attrs.compile_fail {
        let note = match &example.attrs.compile_fail_code {
            Some(expected) => format!("expected the example to fail with {expected}, but it compiled"),
            None => "expected the example not to compile, but it did".to_string(),
        };
        return Ok(Built::Done(Outcome {
            passed: false,
            stdout: Vec::new(),
            stderr: Vec::new(),
            note: Some(note),
            duration: started.elapsed(),
        }));
    }
    if example.attrs.no_run {
        return Ok(Built::Done(Outcome {
            passed: true,
            stdout: Vec::new(),
            stderr: Vec::new(),
            note: None,
            duration: started.elapsed(),
        }));
    }
    let checked = checked.expect("no errors means checking produced a program");
    let exe = dir.join("program").with_extension(std::env::consts::EXE_EXTENSION);
    emit_program(&checked, &sources, name, &exe, &doc_options)?;
    Ok(Built::Exe(exe))
}

/// The compile of a doc test failed: turn the diagnostics into its outcome. A `compile_fail` example passes when
/// the failure is the one it asks for; any other example fails with the rewritten diagnostics as its output.
fn failed_build(
    example: &DocExample,
    program: &Program,
    diags: Diagnostics,
    sources: &SourceMap,
    generated: FileId,
    options: &CompileOptions,
    duration: Duration,
) -> Built {
    let (mapped, sources) = map_diagnostics(diags, sources, generated, example, program);
    let mut codes: Vec<&str> = Vec::new();
    for diagnostic in mapped.iter().filter(|d| d.severity == Severity::Error) {
        if !codes.contains(&diagnostic.code.0) {
            codes.push(diagnostic.code.0);
        }
    }
    let stderr = mapped.render_with(&sources, &render_options(options.diagnostics, std::io::stderr().is_terminal()));
    let note = if example.attrs.compile_fail {
        match &example.attrs.compile_fail_code {
            Some(expected) if codes.contains(&expected.as_str()) => None,
            Some(expected) => Some(match codes.first() {
                Some(found) => format!("expected the example to fail with {expected}, but it failed with {found}"),
                None => format!("expected the example to fail with {expected}, but it failed without an error code"),
            }),
            None => None,
        }
    } else {
        Some("did not compile".to_string())
    };
    Built::Done(Outcome {
        passed: example.attrs.compile_fail && note.is_none(),
        stdout: Vec::new(),
        stderr: stderr.text.into_bytes(),
        note,
        duration,
    })
}

/// Writes the package of one doc test: `burdin.toml` with the documented package as its dependency, and the example
/// as `src/main.bd`. A single-file target is copied into a package of the build directory first.
fn write_package(dir: &Path, name: &str, example: &DocExample, program: &str) -> Result<(), ExitCode> {
    let src = dir.join("src");
    let write = |path: &Path, text: &str| -> Result<(), ExitCode> {
        if let Err(e) = std::fs::write(path, text) {
            eprintln!("error: cannot write {}: {e}", path.display());
            return Err(ExitCode::from(2));
        }
        Ok(())
    };
    let create = |dir: &Path| -> Result<(), ExitCode> {
        std::fs::create_dir_all(dir).map_err(|e| {
            eprintln!("error: cannot create {}: {e}", dir.display());
            ExitCode::from(2)
        })
    };
    let manifest = |dep: Option<(&str, &Path)>| -> String {
        let mut text = format!("[package]\nname = \"{name}\"\n");
        if let Some((dep, path)) = dep {
            // Forward slashes only: a Windows path's backslashes would read as escapes in the manifest's strings.
            text.push_str(&format!(
                "\n[dependencies]\n{dep} = {{ path = \"{}\" }}\n",
                path.display().to_string().replace('\\', "/")
            ));
        }
        text
    };
    match &example.dep {
        Dep::Package(dep) => {
            create(&src)?;
            write(&dir.join("burdin.toml"), &manifest(Some((&example.package, dep))))?;
            write(&src.join("main.bd"), program)?;
        }
        Dep::File => {
            // Every doc test copies the file into a package of its own: the builds run in parallel, so two of them
            // must never write or read the same copy.
            let dep_dir = dir.with_file_name(format!("{}-dep", dir.file_name().unwrap_or_default().to_string_lossy()));
            create(&dep_dir.join("src"))?;
            write(&dep_dir.join("burdin.toml"), &format!("[package]\nname = \"{}\"\n", example.package))?;
            write(&dep_dir.join("src").join(format!("{}.bd", example.package)), example.text.as_str())?;
            create(&src)?;
            write(&dir.join("burdin.toml"), &manifest(Some((&example.package, &dep_dir))))?;
            write(&src.join("main.bd"), program)?;
        }
        Dep::None => {
            create(&src)?;
            write(&dir.join("burdin.toml"), &manifest(None))?;
            write(&src.join("main.bd"), program)?;
        }
    }
    Ok(())
}

/// The generated package's name: `doctest`, unless the documented package already has that name.
fn entry_name(dep: &str) -> &'static str {
    if dep == "doctest" { "doctest_run" } else { "doctest" }
}

// --- mapping a failure back to the doc comment ------------------------------------------------------------------

/// Rewrites the diagnostics of a doc test's compile so they point at the doc comment: the generated file is
/// replaced by the original one, and a span inside the example is moved to the line of the comment it came from.
/// A span on a generated line (the wrapper, the padding) becomes the example's fence line.
fn map_diagnostics(
    diags: Diagnostics,
    sources: &SourceMap,
    generated: FileId,
    example: &DocExample,
    program: &Program,
) -> (Diagnostics, SourceMap) {
    let mut mapped = Diagnostics::default();
    for diagnostic in diags.into_vec() {
        mapped.push(map_diagnostic(diagnostic, generated, example, program));
    }
    let mut remapped = SourceMap::default();
    for (id, file) in sources.iter() {
        if id == generated {
            remapped.add(example.file.clone(), example.text.as_ref().clone());
        } else {
            remapped.add(file.path.clone(), file.text.clone());
        }
    }
    (mapped, remapped)
}

fn map_diagnostic(
    mut diagnostic: Diagnostic,
    generated: FileId,
    example: &DocExample,
    program: &Program,
) -> Diagnostic {
    if diagnostic.span.file == generated {
        diagnostic.span = map_span(program, example, diagnostic.span);
    }
    for note in &mut diagnostic.notes {
        if let Some(span) = note.span
            && span.file == generated
        {
            note.span = Some(map_span(program, example, span));
        }
    }
    for label in &mut diagnostic.labels {
        if label.span.file == generated {
            label.span = map_span(program, example, label.span);
        }
    }
    for suggestion in &mut diagnostic.suggestions {
        if suggestion.span.file == generated {
            suggestion.span = map_span(program, example, suggestion.span);
        }
    }
    diagnostic.children = std::mem::take(&mut diagnostic.children)
        .into_iter()
        .map(|child| map_diagnostic(child, generated, example, program))
        .collect();
    diagnostic
}

/// Where a span of the generated file is in the original one.
fn map_span(program: &Program, example: &DocExample, span: Span) -> Span {
    let lo = map_byte(program, example, span.lo);
    let hi = if span.hi > span.lo { map_byte(program, example, span.hi - 1) + 1 } else { lo };
    Span::new(span.file, lo, hi.max(lo))
}

/// Where a byte of the generated file is in the original one. A byte on a generated line (the wrapper, the
/// padding) points at the start of the example's fence line.
fn map_byte(program: &Program, example: &DocExample, offset: u32) -> u32 {
    let (line, within) = program.locate(offset);
    let Some(j) = program.line_of[line] else { return example.fence.0 };
    let doc_line = &example.lines[j];
    // The hidden-line marker was removed after the line's indentation, so a column past the indentation moves by
    // the marker's length.
    let indent = doc_line.line.text.len() - doc_line.line.text.trim_start().len();
    let shift = if (within as usize) >= indent { doc_line.line.hidden.len() } else { 0 };
    (doc_line.lo + within + shift as u32).min(doc_line.hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, lo: u32, hi: u32) -> DocLine {
        DocLine { line: BlockLine { text: text.to_string(), hidden: "" }, lo, hi }
    }

    fn example(lines: Vec<DocLine>, fence: (u32, u32), fence_line: u32) -> DocExample {
        DocExample {
            package: "app".to_string(),
            module: String::new(),
            owner: "owner".to_string(),
            file: "src/lib.bd".to_string(),
            line: fence_line,
            attrs: DocAttrs::default(),
            dep: Dep::None,
            lines,
            fence,
            text: Arc::new(String::new()),
        }
    }

    #[test]
    fn attributes_after_the_language_word() {
        let (attrs, warnings) = parse_attrs("");
        assert_eq!(attrs, DocAttrs::default());
        assert!(warnings.is_empty());
        let (attrs, warnings) = parse_attrs("burdin");
        assert_eq!(attrs, DocAttrs::default());
        assert!(warnings.is_empty());
        let (attrs, warnings) = parse_attrs("burdin,no_run");
        assert!(attrs.no_run && warnings.is_empty());
        let (attrs, warnings) = parse_attrs("burdin no_run should_panic");
        assert!(attrs.no_run && attrs.should_panic && warnings.is_empty());
        let (attrs, warnings) = parse_attrs("bd,ignore");
        assert!(attrs.ignore && warnings.is_empty());
        // An untagged block's attributes still need their comma, because the first word is the language.
        let (attrs, _) = parse_attrs(",no_run");
        assert!(attrs.no_run);
        // `compile_fail` takes the error code that follows it.
        let (attrs, warnings) = parse_attrs("burdin,compile_fail E0123");
        assert!(attrs.compile_fail);
        assert_eq!(attrs.compile_fail_code, Some("E0123".to_string()));
        assert!(warnings.is_empty());
        let (attrs, warnings) = parse_attrs("burdin,compile_fail,no_run");
        assert!(attrs.compile_fail && attrs.no_run && attrs.compile_fail_code.is_none());
        assert!(warnings.is_empty());
        // A word that is not an attribute warns; a malformed code is consumed with its own warning.
        let (attrs, warnings) = parse_attrs("burdin,bogus");
        assert_eq!(attrs, DocAttrs::default());
        assert_eq!(warnings, ["unknown code-block attribute `bogus`".to_string()]);
        let (attrs, warnings) = parse_attrs("burdin,compile_fail E12");
        assert!(attrs.compile_fail && attrs.compile_fail_code.is_none());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("E12"));
    }

    #[test]
    fn a_line_that_declares_main() {
        assert!(declares_main("fn main() {}"));
        assert!(declares_main("fn main(): i32 { 3 }"));
        assert!(declares_main("  fn main() {}"));
        assert!(!declares_main("let s = \"fn main()\""));
        assert!(!declares_main("fn main_() {}"));
        assert!(!declares_main("fn mainx() {}"));
    }

    #[test]
    fn statements_wrap_in_a_padded_main() {
        // The example's fence is on line 3, so its first statement is on line 4: the padding, the wrapper on line
        // 3, and the statements on their own lines.
        let case = example(vec![line("let x = 1", 20, 29), line("assert_eq(x, 1)", 30, 44)], (12, 17), 3);
        let program = generate(&case);
        assert_eq!(program.text, "// doc test\n// doc test\nfn main() {\nlet x = 1\nassert_eq(x, 1)\n}\n");
        assert_eq!(program.line_of, [None, None, None, Some(0), Some(1), None]);
        // A diagnostic on each statement maps back to the byte range of its doc line.
        assert_eq!(map_byte(&program, &case, program.line_starts[3]), 20);
        assert_eq!(map_byte(&program, &case, program.line_starts[4]), 30);
    }

    #[test]
    fn declarations_are_hoisted_above_the_wrapper() {
        let case = example(vec![line("fn helper(): i32 { 3 }", 10, 30), line("let x = helper()", 31, 47)], (5, 9), 1);
        let program = generate(&case);
        assert_eq!(program.text, "// doc test\nfn main() {\nlet x = helper()\n}\nfn helper(): i32 { 3 }\n");
        assert_eq!(program.line_of, [None, None, Some(1), None, Some(0)]);
    }

    #[test]
    fn a_block_that_declares_main_is_the_whole_program() {
        let case = example(vec![line("fn main() {", 4, 14), line("    run()", 15, 23), line("}", 24, 25)], (0, 3), 1);
        let program = generate(&case);
        assert_eq!(program.text, "// doc test\nfn main() {\n    run()\n}\n");
        assert_eq!(program.line_of, [None, Some(0), Some(1), Some(2)]);
    }

    #[test]
    fn generated_lines_map_to_the_fence() {
        let case = example(vec![line("let x = 1", 20, 29)], (12, 17), 3);
        let program = generate(&case);
        // The `fn main() {` wrapper and the closing brace point at the fence line, the statement at its doc line.
        assert_eq!(map_byte(&program, &case, program.line_starts[2]), 12);
        assert_eq!(map_byte(&program, &case, program.line_starts[4]), 12);
        let span = map_span(&program, &case, Span::new(FileId(0), program.line_starts[3], program.line_starts[3] + 3));
        assert_eq!((span.lo, span.hi), (20, 23));
    }

    #[test]
    fn hidden_lines_keep_their_marker_for_the_mapping() {
        // The compiled line lost two bytes; a diagnostic in it moves right by them.
        let mut hidden = line("use app.{Point}", 40, 55);
        hidden.line = BlockLine { text: "use app.{Point}".to_string(), hidden: "# " };
        let case = example(vec![hidden], (30, 34), 2);
        let program = generate(&case);
        // The line is a `use`, so it is hoisted below the wrapper: the fifth generated line.
        assert_eq!(
            program.text,
            "// doc test
// doc test
fn main() {
}
use app.{Point}
"
        );
        let at = map_byte(&program, &case, program.line_starts[4] + 4);
        assert_eq!(at, 40 + 4 + 2);
    }

    #[test]
    fn extraction_walks_items_and_module_docs() {
        let text = "//! The module.\n//!\n//! ```burdin\n//! let setup = 1\n//! ```\n\n\
                    /// Adds one.\n///\n/// ```\n/// # use app.{add}\n/// assert_eq(add(1), 2)\n/// ```\nfn add(a: i32): i32 { a + 1 }\n\n\
                    /// ```ignore\n/// never compiled\n/// ```\n/// ```text\n/// plain\n/// ```\nfn quiet() {}\n";
        let cases = file_examples("app", "", &Dep::None, "src/lib.bd", "lib", text.to_string());
        let names: Vec<(&str, u32, usize, &str, bool)> = cases
            .iter()
            .map(|case| {
                (
                    case.owner.as_str(),
                    case.line,
                    case.lines.len(),
                    case.lines[0].line.compiled(),
                    case.lines[0].line.is_hidden(),
                )
            })
            .collect();
        assert_eq!(names, [("lib", 3, 1, "let setup = 1", false), ("add", 9, 2, "use app.{add}", true),]);
        // The hidden line keeps its marker for the mapping and its compiled form for the program.
        assert_eq!(cases[1].lines[0].line.hidden, "# ");
        assert_eq!(cases[1].lines[1].line.compiled(), "assert_eq(add(1), 2)");
    }

    #[test]
    fn the_longest_item_prefix_is_hoisted() {
        assert_eq!(longest_item_prefix(&["let x = 1"]), 0);
        assert_eq!(longest_item_prefix(&["struct P { x: i32 }"]), 1);
        assert_eq!(longest_item_prefix(&["fn h(): i32 { 3 }", "let x = h()"]), 1);
        assert_eq!(longest_item_prefix(&["use app.{P}", "struct P { x: i32 }", "let p = P { x: 1 }"]), 2);
    }

    #[test]
    fn the_generated_package_names_avoid_the_dependency() {
        assert_eq!(entry_name("app"), "doctest");
        assert_eq!(entry_name("doctest"), "doctest_run");
    }
}
