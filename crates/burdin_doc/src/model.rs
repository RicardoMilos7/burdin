//! The documented API of a package: what `burdin doc` renders as HTML, prints as JSON, and counts for coverage.
//!
//! The model is built from syntax trees and source text alone (`extract.rs`). It holds only what a reader of the
//! documentation sees: names, the declaration as written, the doc comment, where the declaration is, and the members
//! that Burdin spreads over several mechanisms (inherent methods, interface impls, operators, extension methods,
//! derives), gathered on the type's own entry (`spec/doc.md`, DC14).

use burdin_diag::{FileId, Span};

/// A doc comment with the information the renderers need.
#[derive(Clone, Debug)]
pub struct Doc {
    /// The Markdown text.
    pub text: String,
    /// The source line of each line of `text`, for diagnostics about a link inside the comment.
    pub lines: Vec<Span>,
    /// Every intra-doc link in the text, in order, with what it resolved to (`links.rs`).
    pub links: Vec<Link>,
}

/// One intra-doc link: `[Type]`, `[module.function]`, `` [`Type.method`] ``, `[text](Type)`, or `[text][Type]`.
#[derive(Clone, Debug)]
pub struct Link {
    /// The text shown for the link.
    pub text: String,
    /// The path as written, without backticks or `()`.
    pub target: String,
    /// The line of the doc text (0-based) the link is on.
    pub line: usize,
    pub resolution: Resolution,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// A module of the package: its path.
    Module(Vec<String>),
    /// An item of the package: its module's path and its name, and the member (field, variant, method) when the
    /// link names one.
    Item { module: Vec<String>, name: String, member: Option<String> },
    /// A name the package does not document: the standard library, a primitive type, or a dependency. It is shown as
    /// code and has no URL.
    External,
    /// A `priv` item or member, which the site leaves out unless `--private` is given.
    Hidden,
    /// Nothing by that name.
    Broken,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Function,
    Struct,
    Enum,
    ErrorEnum,
    Interface,
    TypeAlias,
    Constant,
    Derive,
    ExternFunction,
}

impl Kind {
    /// The word a reader sees.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Function => "function",
            Kind::Struct => "struct",
            Kind::Enum => "enum",
            Kind::ErrorEnum => "error enum",
            Kind::Interface => "interface",
            Kind::TypeAlias => "type alias",
            Kind::Constant => "constant",
            Kind::Derive => "derive",
            Kind::ExternFunction => "extern function",
        }
    }

    /// The prefix of the item's page name and of its anchors: `struct.Point.html`.
    pub fn slug(self) -> &'static str {
        match self {
            Kind::Function => "fn",
            Kind::Struct => "struct",
            Kind::Enum => "enum",
            Kind::ErrorEnum => "error",
            Kind::Interface => "interface",
            Kind::TypeAlias => "type",
            Kind::Constant => "const",
            Kind::Derive => "derive",
            Kind::ExternFunction => "extern",
        }
    }

    /// The heading of the list of this kind on a module page.
    pub fn plural(self) -> &'static str {
        match self {
            Kind::Function => "Functions",
            Kind::Struct => "Structs",
            Kind::Enum => "Enums",
            Kind::ErrorEnum => "Error enums",
            Kind::Interface => "Interfaces",
            Kind::TypeAlias => "Type aliases",
            Kind::Constant => "Constants",
            Kind::Derive => "Derives",
            Kind::ExternFunction => "Extern functions",
        }
    }

    /// The order of the lists on a module page: types first, then interfaces, then functions.
    pub fn order(self) -> u8 {
        match self {
            Kind::Struct => 0,
            Kind::Enum => 1,
            Kind::ErrorEnum => 2,
            Kind::Interface => 3,
            Kind::TypeAlias => 4,
            Kind::Derive => 5,
            Kind::Function => 6,
            Kind::ExternFunction => 7,
            Kind::Constant => 8,
        }
    }

    /// Whether the item is a type, so its page has fields or variants and the member lists of DC14.
    pub fn is_type(self) -> bool {
        matches!(self, Kind::Struct | Kind::Enum | Kind::ErrorEnum)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    Field,
    Variant,
    /// A method of an `impl Type { }` block.
    Method,
    /// An interface method with no body: an implementer must write it.
    Required,
    /// An interface method with a body: an implementer may leave it out.
    Provided,
    /// `fn Type.name(self)` declared outside the type.
    Extension,
    /// A method of an operator interface, shown the way it is written at a call: `a + b`.
    Operator,
}

impl MemberKind {
    pub fn slug(self) -> &'static str {
        match self {
            MemberKind::Field => "field",
            MemberKind::Variant => "variant",
            MemberKind::Method | MemberKind::Required | MemberKind::Provided => "method",
            MemberKind::Extension => "extension",
            MemberKind::Operator => "operator",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MemberKind::Field => "field",
            MemberKind::Variant => "variant",
            MemberKind::Method => "method",
            MemberKind::Required => "required method",
            MemberKind::Provided => "provided method",
            MemberKind::Extension => "extension method",
            MemberKind::Operator => "operator",
        }
    }
}

/// Where an extension method is declared, and what a module needs to call it (`spec/extensions.md`, EX2).
#[derive(Clone, Debug)]
pub struct Origin {
    /// The module that declares it, as a reader writes it: `text`, `geometry.shapes`.
    pub module: String,
    /// The import that brings it into scope (`use text.shout`), or `None` in the module that declares it.
    pub import: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Member {
    pub kind: MemberKind,
    /// The name in the source: a field, variant, or method name.
    pub name: String,
    /// What a reader sees as the entry's heading: the name, or for an operator the way it is written (`a + b`).
    pub display: String,
    /// The declaration as written: `x: i32`, `fn area(self): f64`, `Circle(f64)`.
    pub signature: String,
    pub docs: Option<Doc>,
    pub private: bool,
    pub span: Span,
    pub line: u32,
    /// Set on an extension method.
    pub origin: Option<Origin>,
    /// The specific instance an `impl` block is for (`Stack<i32>`), when it is not for every instance.
    pub instance: Option<String>,
}

/// An interface the type implements, by an `impl Type: Interface` block or a `derives` clause.
#[derive(Clone, Debug)]
pub struct Implementation {
    /// The interface as written: `Display`, `Iterator<i32>`.
    pub interface: String,
    /// The interface's declaration in the package, when it is one.
    pub target: Option<Resolution>,
    pub derived: bool,
    pub instance: Option<String>,
    /// The `where` clause of the block, as written.
    pub where_clause: String,
    pub members: Vec<Member>,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub kind: Kind,
    pub name: String,
    pub private: bool,
    /// The declaration as written, without its body.
    pub signature: String,
    pub docs: Option<Doc>,
    /// Where the name is declared.
    pub span: Span,
    pub line: u32,
    /// The fields of a struct and the variants of an enum.
    pub members: Vec<Member>,
    /// Inherent methods, or the methods of an interface.
    pub methods: Vec<Member>,
    pub implementations: Vec<Implementation>,
    pub operators: Vec<Member>,
    pub extensions: Vec<Member>,
    /// The modules that re-export the item with `pub use`.
    pub reexported_by: Vec<String>,
}

/// A `pub use` of a module: what it makes reachable under this module's name.
#[derive(Clone, Debug)]
pub struct Reexport {
    /// The name it is reachable by.
    pub name: String,
    /// What was imported, as written: `inner.answer`.
    pub path: String,
    pub target: Resolution,
    pub span: Span,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct Module {
    /// The path inside the package: `["geometry", "shapes"]`; empty for a one-file target.
    pub path: Vec<String>,
    pub file: FileId,
    /// The file's path relative to the package, with `/` separators.
    pub file_path: String,
    pub docs: Option<Doc>,
    pub items: Vec<Item>,
    /// Every extension method the module declares, whatever its receiver (`spec/doc.md`, DC14).
    pub extensions: Vec<Member>,
    pub reexports: Vec<Reexport>,
    /// Whether other packages reach the module as the package itself (`use name`).
    pub is_root: bool,
}

impl Module {
    /// The name a reader writes: the path joined by dots, or the package's name for a one-file target and for the
    /// root module, which other packages reach as the package itself.
    pub fn display(&self, package: &str) -> String {
        if self.path.is_empty() || self.is_root { package.to_string() } else { self.path.join(".") }
    }
}

/// Everything `burdin doc` documents.
#[derive(Clone, Debug)]
pub struct Api {
    pub package: String,
    /// Whether `priv` items and members are included.
    pub private: bool,
    /// In path order.
    pub modules: Vec<Module>,
}

impl Api {
    pub fn module(&self, path: &[String]) -> Option<&Module> {
        self.modules.iter().find(|m| m.path == path)
    }

    pub fn item(&self, module: &[String], name: &str) -> Option<&Item> {
        self.module(module)?.items.iter().find(|i| i.name == name)
    }
}
