//! A minimal `burdin bindgen` for build scripts (`spec/build-scripts.md`, BS7).
//!
//! It reads one C header and writes a Burdin module of raw declarations: functions as `extern "C"`, structs as
//! `repr(C)` structs, typedefs as type aliases, enum values and object-like `#define`s as constants. It is
//! deliberately small: there is no libclang, no `#include`, and no preprocessor beyond `#define NAME value`. The
//! full tool is stage 3, so a construct it cannot represent is skipped with a comment instead of guessed at.

use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: burdin bindgen <header> --out <file>";

pub(crate) fn run(args: &[String]) -> ExitCode {
    let mut header: Option<String> = None;
    let mut out: Option<String> = None;
    let mut iterator = args.iter();
    while let Some(raw) = iterator.next() {
        let (option, inline) = match raw.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            _ => (raw.as_str(), None),
        };
        match option {
            "--out" | "-o" => {
                let value = match inline {
                    Some(value) => value.to_string(),
                    None => match iterator.next() {
                        Some(value) => value.clone(),
                        None => return usage_error("`--out` needs a path"),
                    },
                };
                out = Some(value);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ if option.starts_with('-') => return usage_error(&format!("unknown option `{option}`")),
            _ if header.is_none() => header = Some(option.to_string()),
            _ => return usage_error(&format!("unexpected argument `{option}`")),
        }
    }
    let Some(header) = header else {
        return usage_error("missing the header path");
    };
    let Some(out) = out else {
        return usage_error("missing `--out <file>`");
    };
    let source = match std::fs::read_to_string(&header) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("error: cannot read {header}: {e}");
            return ExitCode::from(2);
        }
    };
    let generated = generate(&header, &source);
    if let Some(parent) = Path::new(&out).parent().filter(|parent| !parent.as_os_str().is_empty())
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("error: cannot create {}: {e}", parent.display());
        return ExitCode::from(2);
    }
    if let Err(e) = std::fs::write(&out, generated) {
        eprintln!("error: cannot write {out}: {e}");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("error: {message}\n{USAGE}");
    ExitCode::from(2)
}

/// The words Burdin reserves. A C declaration that would need one as a name is skipped.
const KEYWORDS: &[&str] = &[
    "any",
    "as",
    "break",
    "comptime",
    "const",
    "continue",
    "derive",
    "else",
    "enum",
    "error",
    "extern",
    "false",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "interface",
    "let",
    "loop",
    "match",
    "mut",
    "orelse",
    "own",
    "priv",
    "return",
    "self",
    "Self",
    "struct",
    "true",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
];

fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !KEYWORDS.contains(&name)
}

/// Generates the Burdin module for one header.
pub(crate) fn generate(header: &str, source: &str) -> String {
    let (code, macros) = preprocess(source);
    let mut parser = Parser::new(tokenize(&code));
    let mut output = Output::default();
    for (name, ty, value) in macros {
        if is_name(&name) {
            output.constant(&name, &ty, &value);
        } else {
            output.skipped(format!("`#define {name}` because the name is reserved"));
        }
    }
    parser.items(&mut output);
    output.finish(header)
}

// --- Preprocessor -----------------------------------------------------------------------------------------------

/// Removes `#` lines from the code and keeps the object-like integer, float, and string `#define`s. Every other
/// preprocessor directive is ignored: includes are stage 3.
fn preprocess(source: &str) -> (String, Vec<(String, String, String)>) {
    let mut code = String::new();
    let mut macros = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('#') {
            if let Some(rest) = rest.trim_start().strip_prefix("define") {
                let rest = rest.trim_start();
                let mut parts = rest.splitn(2, char::is_whitespace);
                let name = parts.next().unwrap_or("").trim();
                let value = strip_comment(parts.next().unwrap_or("").trim());
                // `NAME(...)` is a function-like macro: not part of the minimal tool.
                if !name.is_empty()
                    && !name.contains('(')
                    && !value.is_empty()
                    && let Some((ty, literal)) = macro_value(value)
                {
                    macros.push((name.to_string(), ty, literal));
                }
            }
            continue;
        }
        code.push_str(line);
        code.push('\n');
    }
    (code, macros)
}

/// The part of a line before a `//` or `/*` comment.
fn strip_comment(value: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let c = bytes[index] as char;
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if c == '/' && index + 1 < bytes.len() && (bytes[index + 1] == b'/' || bytes[index + 1] == b'*') {
            return value[..index].trim();
        }
        index += 1;
    }
    value
}

/// A `#define` value: a C integer (with an optional suffix), a float, or a string. The result is the Burdin type
/// and the literal text.
fn macro_value(value: &str) -> Option<(String, String)> {
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        // Only the escapes Burdin strings share with C are accepted; a C escape that has no Burdin spelling would
        // make the generated module unparsable.
        let inner = &value[1..value.len() - 1];
        if inner.contains("\\0") || inner.contains("\\x") || inner.contains("\\u") {
            return None;
        }
        return Some(("str".to_string(), value.to_string()));
    }
    if !value.starts_with(|c: char| c.is_ascii_digit() || c == '.' || c == '-' || c == '+') {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    if (lower.contains('.') || lower.contains('e')) && !lower.starts_with("0x") {
        let number = value.trim_end_matches(['f', 'F', 'l', 'L']);
        return Some(("f64".to_string(), number.to_string()));
    }
    let number = value.trim_end_matches(['u', 'U', 'l', 'L']);
    let suffix = &value[number.len()..];
    let unsigned = suffix.chars().any(|c| c == 'u' || c == 'U');
    let longs = suffix.chars().filter(|c| *c == 'l' || *c == 'L').count();
    let (digits, radix) = if let Some(hex) = number.strip_prefix("0x").or_else(|| number.strip_prefix("0X")) {
        (hex, 16)
    } else if let Some(octal) = number.strip_prefix("0o").or_else(|| number.strip_prefix("0O")) {
        (octal, 8)
    } else if number.len() > 1 && number.starts_with('0') && number.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
        // C octal is a leading zero, not `0o`.
        (&number[1..], 8)
    } else {
        (number, 10)
    };
    let value = i128::from_str_radix(digits, radix).ok()?;
    // A width suffix is honored: C's `long` is 32-bit on Windows and 64-bit elsewhere, and a value that does not fit
    // still widens (a hex constant may exceed the width the suffix names).
    let wide = longs >= 2 || (longs == 1 && !cfg!(windows));
    let ty = if unsigned {
        if wide || u32::try_from(value).is_err() {
            if u64::try_from(value).is_ok() {
                "u64"
            } else {
                return None;
            }
        } else {
            "u32"
        }
    } else if i32::try_from(value).is_ok() && !wide {
        "i32"
    } else if i64::try_from(value).is_ok() {
        "i64"
    } else {
        return None;
    };
    Some((ty.to_string(), value.to_string()))
}

// --- Tokens -----------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Ident(String),
    Number(String),
    Str(String),
    Char(String),
    Punct(char),
    Ellipsis,
}

fn tokenize(source: &str) -> Vec<Token> {
    let bytes: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let c = bytes[index];
        if c.is_whitespace() {
            index += 1;
            continue;
        }
        if c == '/' && index + 1 < bytes.len() && bytes[index + 1] == '/' {
            while index < bytes.len() && bytes[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if c == '/' && index + 1 < bytes.len() && bytes[index + 1] == '*' {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == '*' && bytes[index + 1] == '/') {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = index;
            while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || bytes[index] == '_') {
                index += 1;
            }
            tokens.push(Token::Ident(bytes[start..index].iter().collect()));
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && index + 1 < bytes.len() && bytes[index + 1].is_ascii_digit()) {
            let start = index;
            while index < bytes.len() {
                let c = bytes[index];
                if c.is_ascii_alphanumeric()
                    || c == '.'
                    || c == '_'
                    || ((c == '+' || c == '-') && index > start && matches!(bytes[index - 1], 'e' | 'E' | 'p' | 'P'))
                {
                    index += 1;
                } else {
                    break;
                }
            }
            tokens.push(Token::Number(bytes[start..index].iter().collect()));
            continue;
        }
        if c == '"' {
            let start = index;
            index += 1;
            while index < bytes.len() {
                if bytes[index] == '\\' {
                    index += 2;
                } else if bytes[index] == '"' {
                    index += 1;
                    break;
                } else {
                    index += 1;
                }
            }
            tokens.push(Token::Str(bytes[start..index.min(bytes.len())].iter().collect()));
            continue;
        }
        if c == '\'' {
            let start = index;
            index += 1;
            while index < bytes.len() {
                if bytes[index] == '\\' {
                    index += 2;
                } else if bytes[index] == '\'' {
                    index += 1;
                    break;
                } else {
                    index += 1;
                }
            }
            tokens.push(Token::Char(bytes[start..index.min(bytes.len())].iter().collect()));
            continue;
        }
        if c == '.' && index + 2 < bytes.len() && bytes[index + 1] == '.' && bytes[index + 2] == '.' {
            tokens.push(Token::Ellipsis);
            index += 3;
            continue;
        }
        tokens.push(Token::Punct(c));
        index += 1;
    }
    tokens
}

// --- Output -----------------------------------------------------------------------------------------------------

#[derive(Default)]
struct Output {
    items: Vec<String>,
    functions: Vec<String>,
    seen: Vec<String>,
    skipped: Vec<String>,
}

impl Output {
    fn skipped(&mut self, what: impl Into<String>) {
        self.skipped.push(what.into());
    }

    fn type_alias(&mut self, name: &str, ty: &str) {
        if !is_name(name) || !self.unique(name) || name == ty {
            return;
        }
        self.items.push(format!("type {name} = {ty}"));
    }

    fn constant(&mut self, name: &str, ty: &str, value: &str) {
        if !is_name(name) || !self.unique(name) {
            return;
        }
        self.items.push(format!("const {name}: {ty} = {value}"));
    }

    fn struct_item(&mut self, name: &str, fields: &[String]) {
        if !is_name(name) || !self.unique(name) {
            return;
        }
        let mut item = format!("struct {name} repr(C) {{");
        if fields.is_empty() {
            item.push_str(" }");
        } else {
            item.push('\n');
            for field in fields {
                item.push_str("    ");
                item.push_str(field);
                item.push('\n');
            }
            item.push('}');
        }
        self.items.push(item);
    }

    fn function(&mut self, name: &str, text: String) {
        if !is_name(name) || !self.unique(name) {
            return;
        }
        self.functions.push(text);
    }

    fn unique(&mut self, name: &str) -> bool {
        if self.seen.iter().any(|seen| seen == name) {
            return false;
        }
        self.seen.push(name.to_string());
        true
    }

    fn finish(self, header: &str) -> String {
        let mut text = format!(
            "// Generated by `burdin bindgen` from `{header}`; do not edit.\n// A skipped declaration is stage 3 work.\n"
        );
        for skipped in &self.skipped {
            text.push_str(&format!("// bindgen skipped {skipped}\n"));
        }
        if !self.skipped.is_empty() {
            text.push('\n');
        }
        for item in &self.items {
            text.push_str(item);
            text.push('\n');
        }
        if !self.functions.is_empty() {
            text.push_str("extern \"C\" {\n");
            for function in &self.functions {
                text.push_str("    ");
                text.push_str(function);
                text.push('\n');
            }
            text.push_str("}\n");
        }
        text
    }
}

// --- Parser -----------------------------------------------------------------------------------------------------

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    structs: Vec<String>,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Parser {
        Parser { tokens, pos: 0, structs: Vec::new() }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn take(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    fn eat_punct(&mut self, c: char) -> bool {
        if self.peek() == Some(&Token::Punct(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn peek_punct(&self, c: char) -> bool {
        self.peek() == Some(&Token::Punct(c))
    }

    fn peek_ident(&self) -> Option<&str> {
        match self.peek() {
            Some(Token::Ident(name)) => Some(name),
            _ => None,
        }
    }

    fn eat_word(&mut self, word: &str) -> bool {
        if self.peek_ident() == Some(word) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn take_ident(&mut self) -> Option<String> {
        match self.peek() {
            Some(Token::Ident(name)) => {
                let name = name.clone();
                self.pos += 1;
                Some(name)
            }
            _ => None,
        }
    }

    /// Every top-level declaration. The loop always consumes a token or leaves through a `;`, so a construct the
    /// parser does not know cannot stall the tool.
    fn items(&mut self, output: &mut Output) {
        while self.peek().is_some() {
            if self.eat_punct(';') || self.eat_punct('}') {
                continue;
            }
            while self.eat_word("static")
                || self.eat_word("inline")
                || self.eat_word("register")
                || self.eat_word("auto")
                || self.eat_word("_Noreturn")
            {}
            self.skip_attributes();
            if self.peek_ident() == Some("typedef") {
                self.pos += 1;
                self.typedef(output);
            } else if self.peek_ident() == Some("struct") && self.definition_ahead() {
                self.pos += 1;
                self.struct_definition(output);
            } else if self.peek_ident() == Some("enum") && self.definition_ahead() {
                self.pos += 1;
                self.enum_definition(output, false);
            } else if self.peek_ident() == Some("union") {
                self.pos += 1;
                self.skip_declaration();
                output.skipped("a `union` declaration (a union stays bytes behind `CPtr<u8>` in stage 1)");
            } else if self.eat_word("extern") {
                // `extern "C" { ... }` opens a block of declarations; its contents are parsed in place.
                if matches!(self.peek(), Some(Token::Str(_))) {
                    self.pos += 1;
                }
                if self.eat_punct('{') {
                    continue;
                }
                self.skip_declaration();
            } else {
                self.external(output);
            }
        }
    }

    /// Whether a `struct`/`enum` at the current position opens a definition (`struct Name {` or `struct {`).
    fn definition_ahead(&self) -> bool {
        match self.tokens.get(self.pos + 1) {
            Some(Token::Punct('{')) => true,
            Some(Token::Ident(_)) => self.tokens.get(self.pos + 2) == Some(&Token::Punct('{')),
            _ => false,
        }
    }

    fn typedef(&mut self, output: &mut Output) {
        if self.peek_ident() == Some("struct") && self.definition_ahead() {
            self.pos += 1;
            let tag = self.take_ident();
            self.typedef_struct(output, tag);
            return;
        }
        if self.peek_ident() == Some("enum") {
            self.pos += 1;
            self.enum_definition(output, true);
            return;
        }
        if self.peek_ident() == Some("struct") {
            // `typedef struct Tag Alias;`: the alias is the same type, so it needs no second declaration when the
            // names match.
            self.pos += 1;
            let tag = self.take_ident();
            let alias = self.take_ident();
            if let (Some(alias), Some(tag)) = (alias, tag)
                && alias != tag
            {
                output.type_alias(&alias, &tag);
            }
            self.skip_declaration();
            return;
        }
        let Some(base) = self.base_type() else {
            self.skip_declaration();
            return;
        };
        // `typedef void (*Callback)(int);` is a function pointer type: stage 3.
        if self.peek_punct('(') && self.peek_is_function_pointer() {
            self.skip_declaration();
            output.skipped("a function pointer typedef (stage 3)");
            return;
        }
        loop {
            let Some(declarator) = self.declarator(&base, false) else {
                self.skip_declaration();
                return;
            };
            if let Some(name) = &declarator.name {
                let ty = special_alias(name).map(str::to_string).unwrap_or_else(|| declarator.ty.clone());
                output.type_alias(name, &ty);
            }
            if self.eat_punct(',') {
                continue;
            }
            self.skip_declaration();
            return;
        }
    }

    /// `typedef struct [Tag] { ... } Alias;`: the struct takes the tag when there is one, and the alias becomes a
    /// second name for it, so a later `struct Tag *` and `Alias` both resolve.
    fn typedef_struct(&mut self, output: &mut Output, tag: Option<String>) {
        let fields = self.aggregate_fields();
        self.skip_attributes();
        let alias = self.take_ident();
        self.skip_declaration();
        match fields {
            Some(fields) => {
                let name = tag.clone().or(alias.clone());
                if let Some(name) = &name {
                    self.structs.push(name.clone());
                    output.struct_item(name, &fields);
                }
                if let (Some(tag), Some(alias)) = (&tag, &alias)
                    && tag != alias
                {
                    self.structs.push(alias.clone());
                    output.type_alias(alias, tag);
                }
            }
            None => output.skipped("an anonymous aggregate"),
        }
    }

    /// A `struct` definition; the `struct` word is already consumed and a `{` follows the optional tag. A trailing
    /// identifier here declares a variable in C, not another name for the type, so only the tag names the struct.
    fn struct_definition(&mut self, output: &mut Output) {
        let name = self.take_ident();
        let fields = self.aggregate_fields();
        self.skip_attributes();
        self.skip_declaration();
        match fields {
            Some(fields) => {
                if let Some(name) = &name {
                    self.structs.push(name.clone());
                    output.struct_item(name, &fields);
                }
            }
            None => output.skipped("an anonymous aggregate"),
        }
    }

    /// The fields of one `struct { ... }`, or `None` when a field cannot be represented faithfully.
    fn aggregate_fields(&mut self) -> Option<Vec<String>> {
        if !self.eat_punct('{') {
            return None;
        }
        let mut fields = Vec::new();
        let mut representable = true;
        while !self.eat_punct('}') {
            self.peek()?;
            if self.eat_punct(';') {
                continue;
            }
            self.skip_attributes();
            if self.eat_word("struct") || self.eat_word("union") {
                // A nested anonymous aggregate: its layout is C's, not a field we can spell.
                self.skip_to_semicolon();
                representable = false;
                continue;
            }
            let Some(base) = self.base_type() else {
                self.skip_to_semicolon();
                representable = false;
                continue;
            };
            if self.peek_punct(';') {
                self.pos += 1;
                continue;
            }
            loop {
                self.skip_attributes();
                let Some(declarator) = self.declarator(&base, false) else {
                    self.skip_to_semicolon();
                    representable = false;
                    break;
                };
                self.skip_attributes();
                if self.eat_punct(':') {
                    // A bitfield changes the layout of the fields around it.
                    self.skip_until(&[',', ';']);
                    representable = false;
                } else if let Some(name) = &declarator.name {
                    if is_name(name) {
                        fields.push(format!("{name}: {}", declarator.ty));
                    } else {
                        representable = false;
                    }
                }
                if self.eat_punct(',') {
                    continue;
                }
                break;
            }
            self.eat_punct(';');
        }
        representable.then_some(fields)
    }

    /// An `enum` definition; the `enum` word is already consumed. With `typedef`, a trailing alias names the type.
    fn enum_definition(&mut self, output: &mut Output, typedef: bool) {
        let tag = self.take_ident();
        if !self.peek_punct('{') {
            // `enum Tag` used as a type in a declaration, or a forward declaration.
            self.skip_declaration();
            return;
        }
        self.pos += 1;
        let mut values: Vec<(String, i64)> = Vec::new();
        let mut next = 0i64;
        let mut representable = true;
        while !self.eat_punct('}') {
            if self.peek().is_none() {
                representable = false;
                break;
            }
            let Some(name) = self.take_ident() else {
                self.skip_until(&[',', '}']);
                self.eat_punct(',');
                continue;
            };
            self.skip_attributes();
            if self.eat_punct('=') {
                match self.integer(&values) {
                    Some(value) => next = value,
                    None => {
                        representable = false;
                        self.skip_until(&[',', '}']);
                    }
                }
            }
            if representable {
                values.push((name, next));
            }
            next = next.wrapping_add(1);
            if !self.eat_punct(',') && !self.peek_punct('}') {
                self.skip_until(&[',', '}']);
                if self.eat_punct(',') {
                    continue;
                }
            }
        }
        self.skip_attributes();
        let alias = if typedef { self.take_ident() } else { None };
        self.skip_declaration();
        if !representable {
            output.skipped("an enum with a value the minimal bindgen cannot read");
            return;
        }
        let ty = if values.iter().all(|(_, value)| i32::try_from(*value).is_ok()) { "i32" } else { "i64" };
        if let Some(name) = &tag {
            output.type_alias(name, ty);
        }
        if let Some(alias) = &alias
            && Some(alias) != tag.as_ref()
        {
            output.type_alias(alias, ty);
        }
        for (name, value) in values {
            output.constant(&name, ty, &value.to_string());
        }
    }

    /// A non-typedef top-level declaration: a function prototype, a function definition, or a variable.
    fn external(&mut self, output: &mut Output) {
        let Some(mut base) = self.base_type() else {
            self.skip_declaration();
            return;
        };
        self.skip_attributes();
        while self.eat_punct('*') {
            base = wrap_pointer(&base);
            self.skip_attributes();
            while self.eat_word("const") || self.eat_word("volatile") {}
        }
        if self.peek_punct('(') && self.peek_is_function_pointer() {
            self.skip_declaration();
            output.skipped("a function returning a function pointer");
            return;
        }
        let Some(name) = self.take_ident() else {
            self.skip_declaration();
            return;
        };
        if !self.peek_punct('(') {
            // A global variable: raw declarations only cover functions, structs, aliases, and constants.
            self.skip_declaration();
            return;
        }
        self.pos += 1; // `(`
        let params = self.parameters();
        self.skip_attributes();
        if self.peek_punct('{') {
            self.skip_balanced('{', '}');
        } else {
            self.skip_declaration();
        }
        let Some((params, variadic)) = params else {
            output.skipped(format!("`{name}` because a parameter could not be read"));
            return;
        };
        if name == "main" {
            output.skipped("`main` (Burdin reserves the symbol)");
            return;
        }
        if self.structs.iter().any(|s| s == &base) || params.iter().any(|(_, ty)| self.structs.iter().any(|s| s == ty))
        {
            output.skipped(format!("`{name}` because a struct crosses the C ABI by value (stage 3)"));
            return;
        }
        let mut text = format!("fn {name}(");
        for (index, (param, ty)) in params.iter().enumerate() {
            if index > 0 {
                text.push_str(", ");
            }
            text.push_str(param);
            text.push_str(": ");
            text.push_str(ty);
        }
        if variadic {
            if !params.is_empty() {
                text.push_str(", ");
            }
            text.push_str("...");
        }
        text.push(')');
        if base != "()" {
            text.push_str(": ");
            text.push_str(&base);
        }
        output.function(&name, text);
    }

    /// The parameters of a function, with `(` already consumed. `None` when one cannot be represented.
    fn parameters(&mut self) -> Option<(Vec<(String, String)>, bool)> {
        let mut params: Vec<(String, String)> = Vec::new();
        let mut variadic = false;
        if self.eat_punct(')') {
            return Some((params, variadic));
        }
        loop {
            if self.peek() == Some(&Token::Ellipsis) {
                self.pos += 1;
                variadic = true;
            } else if self.peek_ident() == Some("void") && self.tokens.get(self.pos + 1) == Some(&Token::Punct(')')) {
                self.pos += 1;
            } else {
                self.skip_attributes();
                let base = self.base_type()?;
                self.skip_attributes();
                if self.peek_punct('(') && self.peek_is_function_pointer() {
                    // `int callback(int)`: a function pointer parameter is a raw pointer.
                    self.skip_balanced('(', ')');
                    if self.peek_punct('(') {
                        self.skip_balanced('(', ')');
                    }
                    let name = format!("arg{}", params.len());
                    params.push((name, "CPtr<CVoid>".to_string()));
                } else {
                    let mut ty = base;
                    while self.eat_punct('*') {
                        ty = wrap_pointer(&ty);
                        self.skip_attributes();
                        while self.eat_word("const") || self.eat_word("volatile") {}
                    }
                    // An array parameter decays to a pointer.
                    while self.eat_punct('[') {
                        self.skip_balanced('[', ']');
                        ty = wrap_pointer(&ty);
                    }
                    let name = match self.peek_ident() {
                        Some(word) if !is_parameter_stop(word) => self.take_ident(),
                        _ => None,
                    };
                    let name = match name {
                        Some(name) if is_name(&name) => name,
                        _ => format!("arg{}", params.len()),
                    };
                    self.skip_attributes();
                    params.push((name, ty));
                }
            }
            if self.eat_punct(',') {
                continue;
            }
            if !self.eat_punct(')') {
                return None;
            }
            return Some((params, variadic));
        }
    }

    /// A base type as a Burdin type. `None` when it cannot be read.
    fn base_type(&mut self) -> Option<String> {
        if self.peek_ident() == Some("struct")
            || self.peek_ident() == Some("enum")
            || self.peek_ident() == Some("union")
        {
            self.pos += 1;
            let tag = self.take_ident()?;
            return Some(tag);
        }
        let mut words: Vec<String> = Vec::new();
        while let Some(word) = self.peek_ident() {
            if is_type_word(word) {
                words.push(word.to_string());
                self.pos += 1;
            } else {
                break;
            }
        }
        if words.is_empty() {
            let name = self.take_ident()?;
            let mapped = map_named_type(&name);
            return is_name(&mapped).then_some(mapped);
        }
        map_type_words(&words)
    }

    /// One declarator after a base type: pointer stars, a name, and array suffixes.
    fn declarator(&mut self, base: &str, parameter: bool) -> Option<Declarator> {
        let mut ty = base.to_string();
        while self.eat_punct('*') {
            ty = wrap_pointer(&ty);
            self.skip_attributes();
            while self.eat_word("const") || self.eat_word("volatile") {}
        }
        let name = match self.peek_ident() {
            Some(word) if !is_parameter_stop(word) => self.take_ident(),
            _ => None,
        };
        while self.eat_punct('[') {
            let mut dimension = String::new();
            while let Some(token) = self.peek() {
                if token == &Token::Punct(']') {
                    self.pos += 1;
                    break;
                }
                dimension.push_str(&token_text(token));
                self.pos += 1;
            }
            if parameter || dimension.is_empty() {
                ty = wrap_pointer(&ty);
            } else {
                ty = format!("[{ty}; {dimension}]");
            }
        }
        Some(Declarator { name, ty })
    }

    /// An integer literal or an earlier enum constant, for `= value`.
    fn integer(&mut self, seen: &[(String, i64)]) -> Option<i64> {
        match self.take()? {
            Token::Number(text) => {
                let text = text.trim_end_matches(['u', 'U', 'l', 'L']);
                if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
                    i64::from_str_radix(hex, 16).ok()
                } else {
                    text.parse().ok()
                }
            }
            Token::Ident(name) => seen.iter().find(|(constant, _)| constant == &name).map(|(_, value)| *value),
            _ => None,
        }
    }

    fn peek_is_function_pointer(&self) -> bool {
        // Current token is `(`; a function pointer declarator has `*` next.
        self.tokens.get(self.pos + 1) == Some(&Token::Punct('*'))
    }

    fn skip_attributes(&mut self) {
        while matches!(self.peek_ident(), Some(word) if is_attribute(word)) {
            self.pos += 1;
            if self.peek_punct('(') {
                self.skip_balanced('(', ')');
            }
        }
    }

    fn skip_balanced(&mut self, open: char, close: char) {
        if !self.eat_punct(open) {
            return;
        }
        let mut depth = 1;
        while depth > 0 {
            match self.take() {
                Some(Token::Punct(c)) if c == open => depth += 1,
                Some(Token::Punct(c)) if c == close => depth -= 1,
                Some(_) => {}
                None => break,
            }
        }
    }

    fn skip_until(&mut self, stops: &[char]) {
        let mut depth = 0usize;
        while let Some(token) = self.peek() {
            match token {
                Token::Punct('(') | Token::Punct('[') | Token::Punct('{') => depth += 1,
                Token::Punct(')') | Token::Punct(']') | Token::Punct('}') => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                Token::Punct(c) if depth == 0 && stops.contains(c) => return,
                _ => {}
            }
            self.pos += 1;
        }
    }

    /// Skips the rest of a declaration, including a function body. Always consumes something.
    fn skip_declaration(&mut self) {
        let mut depth = 0usize;
        while let Some(token) = self.peek() {
            match token {
                Token::Punct('(') | Token::Punct('[') => depth += 1,
                Token::Punct(')') | Token::Punct(']') => depth = depth.saturating_sub(1),
                Token::Punct('{') => {
                    self.skip_balanced('{', '}');
                    if depth == 0 && self.eat_punct(';') {
                        return;
                    }
                    continue;
                }
                Token::Punct(';') if depth == 0 => {
                    self.pos += 1;
                    return;
                }
                _ => {}
            }
            self.pos += 1;
        }
    }

    fn skip_to_semicolon(&mut self) {
        self.skip_declaration();
    }
}

struct Declarator {
    name: Option<String>,
    ty: String,
}

fn token_text(token: &Token) -> String {
    match token {
        Token::Ident(name) => name.clone(),
        Token::Number(text) | Token::Str(text) | Token::Char(text) => text.clone(),
        Token::Punct(c) => c.to_string(),
        Token::Ellipsis => "...".to_string(),
    }
}

fn is_parameter_stop(word: &str) -> bool {
    is_type_word(word)
        || matches!(
            word,
            "struct"
                | "enum"
                | "union"
                | "__attribute__"
                | "__attribute"
                | "__declspec"
                | "__pragma"
                | "alignas"
                | "_Alignas"
        )
}

/// A C attribute that carries no type or name the generated declarations need.
fn is_attribute(word: &str) -> bool {
    matches!(word, "__attribute__" | "__attribute" | "__declspec" | "__pragma" | "alignas" | "_Alignas")
}

fn is_type_word(word: &str) -> bool {
    matches!(
        word,
        "const"
            | "volatile"
            | "restrict"
            | "__restrict"
            | "signed"
            | "unsigned"
            | "short"
            | "long"
            | "int"
            | "char"
            | "float"
            | "double"
            | "void"
            | "_Bool"
            | "bool"
    )
}

fn map_type_words(words: &[String]) -> Option<String> {
    let unsigned = words.iter().any(|w| w == "unsigned");
    let long = words.iter().filter(|w| *w == "long").count();
    let word = |name: &str| words.iter().any(|w| w == name);
    if word("void") {
        return Some("()".to_string());
    }
    if word("double") {
        return Some("f64".to_string());
    }
    if word("float") {
        return Some("f32".to_string());
    }
    if word("_Bool") || word("bool") {
        return Some("u8".to_string());
    }
    if word("char") {
        return Some(
            if unsigned {
                "u8"
            } else if word("signed") {
                "i8"
            } else {
                "CChar"
            }
            .to_string(),
        );
    }
    if word("short") {
        return Some(if unsigned { "u16" } else { "i16" }.to_string());
    }
    if long > 0 {
        // C `long` is 32-bit on Windows and 64-bit elsewhere; `long long` is 64-bit everywhere.
        let wide = long > 1 || !cfg!(windows);
        return Some(
            match (unsigned, wide) {
                (false, true) => "i64",
                (false, false) => "i32",
                (true, true) => "u64",
                (true, false) => "u32",
            }
            .to_string(),
        );
    }
    if words.iter().any(|w| w == "int") || unsigned || word("signed") {
        return Some(if unsigned { "u32" } else { "i32" }.to_string());
    }
    None
}

fn map_named_type(name: &str) -> String {
    let mapped = match name {
        "size_t" => "usize",
        "uintptr_t" => "usize",
        "ssize_t" | "ptrdiff_t" | "intptr_t" => "isize",
        "int8_t" => "i8",
        "uint8_t" => "u8",
        "int16_t" => "i16",
        "uint16_t" => "u16",
        "int32_t" => "i32",
        "uint32_t" => "u32",
        "int64_t" => "i64",
        "uint64_t" => "u64",
        other => other,
    };
    mapped.to_string()
}

/// The portable alias for a typedef whose C spelling depends on the platform.
fn special_alias(name: &str) -> Option<&'static str> {
    Some(match name {
        "size_t" => "usize",
        "uintptr_t" => "usize",
        "ssize_t" | "ptrdiff_t" | "intptr_t" => "isize",
        _ => return None,
    })
}

fn wrap_pointer(ty: &str) -> String {
    if ty == "()" { "CPtr<CVoid>".to_string() } else { format!("CPtr<{ty}>") }
}

#[cfg(test)]
mod tests {
    use super::generate;

    fn output(header: &str) -> String {
        generate("test.h", header)
    }

    #[test]
    fn macros_become_constants_with_an_inferred_type() {
        let text = output("#define ANSWER 42\n#define BIG 0x100000000\n#define RATIO 1.5\n#define NAME \"ok\"\n");
        assert!(text.contains("const ANSWER: i32 = 42"), "{text}");
        assert!(text.contains("const BIG: i64 = 4294967296"), "{text}");
        assert!(text.contains("const RATIO: f64 = 1.5"), "{text}");
        assert!(text.contains("const NAME: str = \"ok\""), "{text}");
    }

    #[test]
    fn c_suffixes_and_octal_literals_are_read() {
        let text = output("#define MASK 0xFFULL\n#define MODE 0755\n#define SMALL 7u\n");
        assert!(text.contains("const MASK: u64 = 255"), "{text}");
        assert!(text.contains("const MODE: i32 = 493"), "{text}");
        assert!(text.contains("const SMALL: u32 = 7"), "{text}");
    }

    #[test]
    fn typedefs_follow_the_platform_and_the_portable_names() {
        let text = output(
            "typedef unsigned long size_t;\ntypedef unsigned char byte;\ntypedef char *string;\ntypedef long code;\ntypedef unsigned long long wide;\n",
        );
        assert!(text.contains("type size_t = usize"), "{text}");
        assert!(text.contains("type byte = u8"), "{text}");
        assert!(text.contains("type string = CPtr<CChar>"), "{text}");
        let long_ty = if cfg!(windows) { "i32" } else { "i64" };
        assert!(text.contains(&format!("type code = {long_ty}")), "{text}");
        assert!(text.contains("type wide = u64"), "{text}");
    }

    #[test]
    fn structs_are_repr_c_with_c_field_types() {
        let text = output("struct Point {\n    int x;\n    int y;\n    const char *name;\n    int values[4];\n};\n");
        assert!(text.contains("struct Point repr(C) {"), "{text}");
        assert!(text.contains("    x: i32"), "{text}");
        assert!(text.contains("    name: CPtr<CChar>"), "{text}");
        assert!(text.contains("    values: [i32; 4]"), "{text}");
    }

    #[test]
    fn enums_become_a_type_and_constants() {
        let text = output("enum Mode {\n    MODE_OFF,\n    MODE_ON = 5,\n    MODE_NEXT\n};\n");
        assert!(text.contains("type Mode = i32"), "{text}");
        assert!(text.contains("const MODE_OFF: i32 = 0"), "{text}");
        assert!(text.contains("const MODE_ON: i32 = 5"), "{text}");
        assert!(text.contains("const MODE_NEXT: i32 = 6"), "{text}");
    }

    #[test]
    fn an_enum_value_can_name_an_earlier_constant() {
        let text = output("typedef enum { A = 3, B = A } E;\n");
        assert!(text.contains("type E = i32"), "{text}");
        assert!(text.contains("const B: i32 = 3"), "{text}");
    }

    #[test]
    fn unix_and_windows_spell_long_differently() {
        let text = output("typedef long plain;\ntypedef long long wide;\n");
        let plain = if cfg!(windows) { "i32" } else { "i64" };
        assert!(text.contains(&format!("type plain = {plain}")), "{text}");
        assert!(text.contains("type wide = i64"), "{text}");
    }

    #[test]
    fn functions_become_a_single_extern_block() {
        let text = output(
            "typedef unsigned long size_t;\nsize_t strlen(const char *s);\nint add(int a, int b);\nvoid clear(void);\nint printf(const char *fmt, ...);\n",
        );
        assert!(text.contains("extern \"C\" {"), "{text}");
        assert!(text.contains("fn strlen(s: CPtr<CChar>): usize"), "{text}");
        assert!(text.contains("fn add(a: i32, b: i32): i32"), "{text}");
        assert!(text.contains("fn clear()"), "{text}");
        assert!(text.contains("fn printf(fmt: CPtr<CChar>, ...): i32"), "{text}");
        assert_eq!(text.matches("extern \"C\" {").count(), 1, "{text}");
    }

    #[test]
    fn unnamed_parameters_get_positional_names() {
        let text = output("int atoi(const char *);\n");
        assert!(text.contains("fn atoi(arg0: CPtr<CChar>): i32"), "{text}");
    }

    #[test]
    fn a_tagged_typedef_keeps_both_names() {
        let text = output("typedef struct Point { int x; } PointAlias;\ntypedef enum Color { RED } ColorAlias;\n");
        assert!(text.contains("struct Point repr(C) {"), "{text}");
        assert!(text.contains("type PointAlias = Point"), "{text}");
        assert!(text.contains("type Color = i32"), "{text}");
        assert!(text.contains("type ColorAlias = i32"), "{text}");
        assert!(text.contains("const RED: i32 = 0"), "{text}");
    }

    #[test]
    fn a_trailing_name_after_a_struct_definition_is_a_variable_not_a_type() {
        let text = output("struct Tag { int x; } variable;\n");
        assert!(text.contains("struct Tag repr(C) {"), "{text}");
        assert!(!text.contains("type variable"), "{text}");
    }

    #[test]
    fn a_pointed_at_struct_can_cross_the_abi() {
        let text = output("struct Node { int value; };\nstruct Node *make_node(void);\n");
        assert!(text.contains("fn make_node(): CPtr<Node>"), "{text}");
    }

    #[test]
    fn unsupported_constructs_are_skipped_with_a_comment() {
        let text = output(
            "typedef int (*Callback)(int);\nunion Value { int i; float f; };\nint apply(int (*cb)(int), int x);\n",
        );
        assert!(text.contains("bindgen skipped a function pointer typedef"), "{text}");
        assert!(text.contains("bindgen skipped a `union` declaration"), "{text}");
        assert!(text.contains("fn apply(arg0: CPtr<CVoid>, x: i32): i32"), "{text}");
    }

    #[test]
    fn reserved_words_are_not_emitted_as_names() {
        let text = output("int match(int x);\n");
        assert!(!text.contains("fn match"), "{text}");
    }

    #[test]
    fn duplicate_declarations_are_emitted_once() {
        let text = output("int f(int x);\nint f(int x);\n");
        assert_eq!(text.matches("fn f(").count(), 1, "{text}");
    }

    #[test]
    fn a_function_definition_is_declared_and_its_body_skipped() {
        let text = output("int twice(int x) { return x * 2; }\nint after(void);\n");
        assert!(text.contains("fn twice(x: i32): i32"), "{text}");
        assert!(text.contains("fn after()"), "{text}");
    }

    #[test]
    fn includes_are_ignored_without_losing_later_declarations() {
        let text = output("#include <stdio.h>\n#define X 1\nint f(void);\n");
        assert!(text.contains("const X: i32 = 1"), "{text}");
        assert!(text.contains("fn f()"), "{text}");
    }
}
