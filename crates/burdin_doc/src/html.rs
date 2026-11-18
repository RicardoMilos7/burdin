//! The static HTML site (`spec/doc.md`, DC13-DC18): an overview page, one page per module, one per item, a sidebar on
//! every page, cross-links from signatures and doc comments, and a client-side search index.
//!
//! The pages are built as strings from the model and need no template files. The stylesheet and the search script are
//! compiled into the `burdin` binary (`assets/`) and copied into the output, so the site is self-contained: it makes
//! no request to any other server and works when opened from disk.

use crate::markdown::{self, LinkRender, escape};
use crate::model::*;
use std::fmt::Write;

const CSS: &str = include_str!("assets/doc.css");
const SEARCH_JS: &str = include_str!("assets/search.js");

/// The path of the stylesheet and the script inside the site.
pub const CSS_PATH: &str = "static/doc.css";
pub const SEARCH_PATH: &str = "static/search.js";
pub const INDEX_PATH: &str = "search-index.js";

/// A generated file: its path inside the output directory and its text.
pub struct Page {
    pub path: String,
    pub text: String,
}

fn module_dir(path: &[String]) -> String {
    if path.is_empty() { String::new() } else { format!("{}/", path.join("/")) }
}

/// The path of a module's page, from the site root.
pub fn module_page(path: &[String]) -> String {
    format!("{}index.html", module_dir(path))
}

/// An item's page as it is named inside its module's directory.
fn item_file(kind: Kind, name: &str) -> String {
    format!("{}.{}.html", kind.slug(), name)
}

/// The path of an item's page, from the site root.
pub fn item_page(module: &[String], kind: Kind, name: &str) -> String {
    format!("{}{}", module_dir(module), item_file(kind, name))
}

/// `../` once per directory a page is below the root.
fn to_root(page_path: &str) -> String {
    "../".repeat(page_path.matches('/').count())
}

fn anchor(kind: MemberKind, name: &str) -> String {
    format!("{}.{}", kind.slug(), name)
}

/// Generates every file of the site.
pub fn render(api: &Api) -> Vec<Page> {
    let mut pages = Vec::new();
    let site = Site { api };
    pages.push(Page { path: "index.html".to_string(), text: site.overview() });
    for m in &api.modules {
        if !m.path.is_empty() {
            pages.push(Page { path: module_page(&m.path), text: site.module(m) });
        }
        for item in &m.items {
            pages.push(Page { path: item_page(&m.path, item.kind, &item.name), text: site.item(m, item) });
        }
    }
    pages.push(Page { path: CSS_PATH.to_string(), text: CSS.to_string() });
    pages.push(Page { path: SEARCH_PATH.to_string(), text: SEARCH_JS.to_string() });
    pages.push(Page { path: INDEX_PATH.to_string(), text: search_index(api) });
    pages
}

struct Site<'a> {
    api: &'a Api,
}

/// Renders the intra-doc links of one doc comment, in order.
struct Links<'a> {
    api: &'a Api,
    root: &'a str,
    links: &'a [Link],
    next: usize,
}

impl LinkRender for Links<'_> {
    fn link(&mut self, text_html: String) -> String {
        let Some(link) = self.links.get(self.next) else { return text_html };
        self.next += 1;
        match &link.resolution {
            Resolution::Broken => format!("<span class=\"broken-link\">{text_html}</span>"),
            Resolution::Hidden => format!("<span class=\"hidden-link\">{text_html}</span>"),
            Resolution::External => format!("<span class=\"external-link\">{text_html}</span>"),
            res => match target_url(self.api, res) {
                Some(url) => format!("<a href=\"{}{}\">{}</a>", self.root, escape(&url), text_html),
                None => text_html,
            },
        }
    }
}

/// The page (and anchor) a resolved link leads to, from the site root.
fn target_url(api: &Api, res: &Resolution) -> Option<String> {
    match res {
        Resolution::Module(path) => api.module(path).map(|_| module_page(path)),
        Resolution::Item { module, name, member } => {
            let item = api.item(module, name)?;
            let page = item_page(module, item.kind, name);
            match member {
                None => Some(page),
                Some(m) => {
                    let found = item
                        .members
                        .iter()
                        .chain(&item.methods)
                        .chain(&item.extensions)
                        .chain(&item.operators)
                        .chain(item.implementations.iter().flat_map(|i| i.members.iter()))
                        .find(|x| &x.name == m)?;
                    Some(format!("{page}#{}", anchor(found.kind, &found.name)))
                }
            }
        }
        _ => None,
    }
}

/// Renders a doc comment's Markdown with its links resolved.
fn doc_html(api: &Api, root: &str, doc: &Option<Doc>) -> String {
    let Some(doc) = doc else { return String::new() };
    let blocks = markdown::parse(&doc.text);
    let mut links = Links { api, root, links: &doc.links, next: 0 };
    format!("<div class=\"docs\">\n{}</div>\n", markdown::render_blocks(&blocks, &mut links))
}

/// The first sentence of a doc comment, without links, for lists.
fn summary_html(doc: &Option<Doc>) -> String {
    let Some(doc) = doc else { return String::new() };
    escape(&markdown::summary_text(&markdown::parse(&doc.text)))
}

fn summary_plain(doc: &Option<Doc>) -> String {
    doc.as_ref().map(|d| markdown::summary_text(&markdown::parse(&d.text))).unwrap_or_default()
}

impl Site<'_> {
    fn layout(&self, page_path: &str, title: &str, current: Option<&Module>, body: &str) -> String {
        let root = to_root(page_path);
        let pkg = escape(&self.api.package);
        let mut out = String::new();
        let _ = write!(
            out,
            "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
             <title>{} - {pkg}</title>\n<link rel=\"stylesheet\" href=\"{root}{CSS_PATH}\">\n</head>\n<body>\n",
            escape(title)
        );
        let _ = write!(
            out,
            "<nav class=\"sidebar\">\n<a class=\"package\" href=\"{root}index.html\">{pkg}</a>\n\
             <input id=\"search\" type=\"search\" placeholder=\"Search\" autocomplete=\"off\" aria-label=\"Search the documentation\">\n"
        );
        out.push_str("<h2>Modules</h2>\n<ul class=\"modules\">\n");
        for m in &self.api.modules {
            let name = m.display(&self.api.package);
            let class = if current.is_some_and(|c| c.path == m.path) { " class=\"current\"" } else { "" };
            let _ = writeln!(out, "<li{class}><a href=\"{root}{}\">{}</a></li>", module_page(&m.path), escape(&name));
        }
        out.push_str("</ul>\n");
        if let Some(m) = current {
            out.push_str("<h2>In this module</h2>\n<ul class=\"items\">\n");
            let mut items: Vec<&Item> = m.items.iter().collect();
            items.sort_by(|a, b| (a.kind.order(), &a.name).cmp(&(b.kind.order(), &b.name)));
            for i in items {
                let _ = writeln!(
                    out,
                    "<li><a href=\"{root}{}\">{}</a></li>",
                    item_page(&m.path, i.kind, &i.name),
                    escape(&i.name)
                );
            }
            out.push_str("</ul>\n");
        }
        out.push_str("</nav>\n<main>\n<div id=\"search-results\" hidden></div>\n<div id=\"content\">\n");
        out.push_str(body);
        let _ = write!(
            out,
            "</div>\n</main>\n<script>window.BURDIN_DOC_ROOT = \"{root}\";</script>\n\
             <script src=\"{root}{INDEX_PATH}\"></script>\n<script src=\"{root}{SEARCH_PATH}\"></script>\n</body>\n</html>\n"
        );
        out
    }

    fn overview(&self) -> String {
        let api = self.api;
        let mut body = String::new();
        // A one-file target has one module with no path: its page is the overview.
        if let Some(m) = api.module(&[]) {
            return self.module_body_page("index.html", m);
        }
        let _ = writeln!(body, "<h1>Package <span class=\"name\">{}</span></h1>", escape(&api.package));
        if let Some(root) = api.modules.iter().find(|m| m.is_root) {
            body.push_str(&doc_html(api, "", &root.docs));
        }
        body.push_str("<h2 id=\"modules\">Modules</h2>\n<table class=\"items\">\n");
        for m in &api.modules {
            let _ = writeln!(
                body,
                "<tr><td><a href=\"{}\">{}</a></td><td>{}</td></tr>",
                module_page(&m.path),
                escape(&m.display(&api.package)),
                summary_html(&m.docs)
            );
        }
        body.push_str("</table>\n");
        self.layout("index.html", &api.package, None, &body)
    }

    fn module(&self, m: &Module) -> String {
        self.module_body_page(&module_page(&m.path), m)
    }

    fn module_body_page(&self, page_path: &str, m: &Module) -> String {
        let api = self.api;
        let root = to_root(page_path);
        let name = m.display(&api.package);
        let mut body = String::new();
        let _ = writeln!(
            body,
            "<h1><span class=\"kind\">{}</span> <span class=\"name\">{}</span></h1>",
            if m.path.is_empty() { "package" } else { "module" },
            escape(&name)
        );
        let _ = writeln!(body, "<p class=\"source\">{}</p>", escape(&m.file_path));
        body.push_str(&doc_html(api, &root, &m.docs));
        let mut kinds: Vec<Kind> = m.items.iter().map(|i| i.kind).collect();
        kinds.sort_by_key(|k| k.order());
        kinds.dedup();
        for kind in kinds {
            let _ = writeln!(body, "<h2 id=\"{}\">{}</h2>\n<table class=\"items\">", kind.slug(), kind.plural());
            let mut items: Vec<&Item> = m.items.iter().filter(|i| i.kind == kind).collect();
            items.sort_by(|a, b| a.name.cmp(&b.name));
            for i in items {
                let badge = if i.private { " <span class=\"badge\">private</span>" } else { "" };
                let _ = writeln!(
                    body,
                    "<tr><td><a href=\"{}\">{}</a>{badge}</td><td>{}</td></tr>",
                    item_file(i.kind, &i.name),
                    escape(&i.name),
                    summary_html(&i.docs)
                );
            }
            body.push_str("</table>\n");
        }
        if !m.extensions.is_empty() {
            body.push_str("<h2 id=\"extensions\">Extension methods</h2>\n");
            for e in &m.extensions {
                body.push_str(&self.member_html(&root, e));
            }
        }
        if !m.reexports.is_empty() {
            body.push_str("<h2 id=\"reexports\">Re-exports</h2>\n<table class=\"items\">\n");
            for r in &m.reexports {
                let shown = match target_url(api, &r.target) {
                    Some(url) => format!("<a href=\"{root}{}\">{}</a>", escape(&url), escape(&r.path)),
                    None => format!("<span class=\"external-link\">{}</span>", escape(&r.path)),
                };
                let _ = writeln!(
                    body,
                    "<tr><td><code>pub use {shown}</code></td><td>as <code>{}</code></td></tr>",
                    escape(&r.name)
                );
            }
            body.push_str("</table>\n");
        }
        self.layout(page_path, &name, Some(m), &body)
    }

    /// A signature with the names of documented items linked.
    fn sig_html(&self, root: &str, module: Option<&Module>, signature: &str, skip: Option<&str>) -> String {
        let mut out = String::new();
        let chars: Vec<char> = signature.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c.is_ascii_alphabetic() || c == '_' {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                let after_dot = start > 0 && chars[start - 1] == '.';
                match (!after_dot && skip != Some(word.as_str())).then(|| self.find_type(module, &word)).flatten() {
                    Some((m, item)) => {
                        let _ = write!(
                            out,
                            "<a href=\"{root}{}\">{}</a>",
                            escape(&item_page(&m.path, item.kind, &item.name)),
                            escape(&word)
                        );
                    }
                    None => out.push_str(&escape(&word)),
                }
            } else {
                out.push_str(&escape(&c.to_string()));
                i += 1;
            }
        }
        out
    }

    /// The documented item a bare name in a signature stands for: one in the same module, else the only one with
    /// that name in the package. Only types and interfaces are linked, which keeps `a`, `x`, and `new` plain.
    fn find_type<'a>(&'a self, module: Option<&Module>, name: &str) -> Option<(&'a Module, &'a Item)> {
        let linkable = |i: &Item| {
            matches!(i.kind, Kind::Struct | Kind::Enum | Kind::ErrorEnum | Kind::Interface | Kind::TypeAlias)
        };
        if let Some(cur) = module.and_then(|c| self.api.module(&c.path))
            && let Some(i) = cur.items.iter().find(|i| i.name == name && linkable(i))
        {
            return Some((cur, i));
        }
        let mut found = self
            .api
            .modules
            .iter()
            .flat_map(|m| m.items.iter().filter(|i| i.name == name && linkable(i)).map(move |i| (m, i)));
        let first = found.next()?;
        found.next().is_none().then_some(first)
    }

    fn member_html(&self, root: &str, m: &Member) -> String {
        let mut out = String::new();
        let id = anchor(m.kind, &m.name);
        let _ = writeln!(out, "<div class=\"member\" id=\"{}\">", escape(&id));
        let mut head = String::new();
        if m.kind == MemberKind::Operator {
            let _ = write!(head, "<code class=\"operator\">{}</code> ", escape(&m.display));
        }
        let kind_badge = match m.kind {
            MemberKind::Required => " <span class=\"badge\">required</span>",
            MemberKind::Provided => " <span class=\"badge\">provided</span>",
            MemberKind::Extension => " <span class=\"badge\">extension</span>",
            _ => "",
        };
        let private = if m.private { " <span class=\"badge\">private</span>" } else { "" };
        let instance = m
            .instance
            .as_ref()
            .map_or(String::new(), |i| format!(" <span class=\"badge\">for <code>{}</code></span>", escape(i)));
        let _ = writeln!(
            out,
            "<div class=\"signature\">{head}<code>{}</code>{kind_badge}{private}{instance}</div>",
            self.sig_html(root, None, &m.signature, None)
        );
        if let Some(o) = &m.origin {
            let note = match &o.import {
                Some(import) => format!(
                    "declared in module <code>{}</code>; import it with <code>{}</code>",
                    escape(&o.module),
                    escape(import)
                ),
                None => format!("declared in <code>{}</code>", escape(&o.module)),
            };
            let _ = writeln!(out, "<p class=\"origin\">{note}</p>");
        }
        out.push_str(&doc_html(self.api, root, &m.docs));
        out.push_str("</div>\n");
        out
    }

    fn item(&self, m: &Module, item: &Item) -> String {
        let api = self.api;
        let page_path = item_page(&m.path, item.kind, &item.name);
        let root = to_root(&page_path);
        let mut body = String::new();
        let private = if item.private { " <span class=\"badge\">private</span>" } else { "" };
        let _ = writeln!(
            body,
            "<h1><span class=\"kind\">{}</span> <span class=\"name\">{}</span>{private}</h1>",
            item.kind.label(),
            escape(&item.name)
        );
        let module_link =
            format!("<a href=\"{root}{}\">{}</a>", module_page(&m.path), escape(&m.display(&api.package)));
        let _ =
            writeln!(body, "<p class=\"source\">in {module_link} &middot; {}:{}</p>", escape(&m.file_path), item.line);
        let _ = writeln!(
            body,
            "<pre class=\"signature\"><code>{}</code></pre>",
            self.sig_html(&root, Some(m), &item.signature, Some(&item.name))
        );
        body.push_str(&doc_html(api, &root, &item.docs));
        if !item.reexported_by.is_empty() {
            let list: Vec<String> = item.reexported_by.iter().map(|n| format!("<code>{}</code>", escape(n))).collect();
            let _ = writeln!(body, "<p class=\"origin\">Re-exported by {}.</p>", list.join(", "));
        }
        let section = |body: &mut String, id: &str, title: &str, members: &[Member]| {
            if members.is_empty() {
                return;
            }
            let _ = writeln!(body, "<h2 id=\"{id}\">{title}</h2>");
            for mem in members {
                body.push_str(&self.member_html(&root, mem));
            }
        };
        let fields: Vec<Member> = item.members.iter().filter(|x| x.kind == MemberKind::Field).cloned().collect();
        let variants: Vec<Member> = item.members.iter().filter(|x| x.kind == MemberKind::Variant).cloned().collect();
        section(&mut body, "fields", "Fields", &fields);
        section(&mut body, "variants", "Variants", &variants);
        if item.kind == Kind::Interface {
            let required: Vec<Member> =
                item.methods.iter().filter(|x| x.kind == MemberKind::Required).cloned().collect();
            let provided: Vec<Member> =
                item.methods.iter().filter(|x| x.kind == MemberKind::Provided).cloned().collect();
            section(&mut body, "required", "Required methods", &required);
            section(&mut body, "provided", "Provided methods", &provided);
        } else {
            section(&mut body, "methods", "Methods", &item.methods);
        }
        section(&mut body, "operators", "Operators", &item.operators);
        section(&mut body, "extensions", "Extension methods", &item.extensions);
        if !item.implementations.is_empty() {
            body.push_str("<h2 id=\"implements\">Implements</h2>\n");
            for imp in &item.implementations {
                let name = match imp.target.as_ref().and_then(|t| target_url(api, t)) {
                    Some(url) => format!("<a href=\"{root}{}\">{}</a>", escape(&url), escape(&imp.interface)),
                    None => escape(&imp.interface),
                };
                let derived = if imp.derived { " <span class=\"badge\">derived</span>" } else { "" };
                let instance = imp
                    .instance
                    .as_ref()
                    .map_or(String::new(), |i| format!(" <span class=\"badge\">for <code>{}</code></span>", escape(i)));
                let wh = if imp.where_clause.is_empty() {
                    String::new()
                } else {
                    format!(" <code>{}</code>", escape(&imp.where_clause))
                };
                let _ = writeln!(
                    body,
                    "<div class=\"impl\" id=\"impl.{}\"><div class=\"signature\"><code>{name}</code>{derived}{instance}{wh}</div>",
                    escape(&imp.interface)
                );
                for mem in &imp.members {
                    body.push_str(&self.member_html(&root, mem));
                }
                body.push_str("</div>\n");
            }
        }
        self.layout(&page_path, &item.name, Some(m), &body)
    }
}

/// The search index: a script that sets one global, so the site works from `file://` (a `fetch` of a JSON file would
/// be blocked there).
fn search_index(api: &Api) -> String {
    let mut entries: Vec<String> = Vec::new();
    let push = |entries: &mut Vec<String>, name: &str, path: &str, kind: &str, url: &str, desc: &str| {
        entries.push(format!(
            "{{\"n\":{},\"p\":{},\"k\":{},\"u\":{},\"d\":{}}}",
            js(name),
            js(path),
            js(kind),
            js(url),
            js(desc)
        ));
    };
    for m in &api.modules {
        let mname = m.display(&api.package);
        if !m.path.is_empty() {
            let last = m.path.last().map_or("", String::as_str);
            push(&mut entries, last, &mname, "module", &module_page(&m.path), &summary_plain(&m.docs));
        }
        let prefix = if m.path.is_empty() { String::new() } else { format!("{mname}.") };
        for e in &m.extensions {
            let url = format!("{}#{}", module_page(&m.path), anchor(e.kind, &e.name));
            push(
                &mut entries,
                &e.name,
                &format!("{prefix}{}", e.name),
                "extension method",
                &url,
                &summary_plain(&e.docs),
            );
        }
        for item in &m.items {
            let url = item_page(&m.path, item.kind, &item.name);
            let path = format!("{prefix}{}", item.name);
            push(&mut entries, &item.name, &path, item.kind.label(), &url, &summary_plain(&item.docs));
            for mem in item
                .members
                .iter()
                .chain(&item.methods)
                .chain(&item.operators)
                .chain(&item.extensions)
                .chain(item.implementations.iter().flat_map(|i| i.members.iter()))
            {
                let murl = format!("{url}#{}", anchor(mem.kind, &mem.name));
                push(
                    &mut entries,
                    &mem.name,
                    &format!("{path}.{}", mem.name),
                    mem.kind.label(),
                    &murl,
                    &summary_plain(&mem.docs),
                );
            }
        }
    }
    format!("window.BURDIN_DOC_INDEX = [\n{}\n];\n", entries.join(",\n"))
}

/// A JavaScript (and JSON) string literal. `<` is escaped so the text cannot close a `<script>` element.
fn js(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003c"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
