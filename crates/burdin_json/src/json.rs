//! A small JSON value with a parser and a printer: what JSON-RPC needs and nothing more (`spec/lsp.md`, LS1). Objects
//! keep their keys in insertion order, so a server's output is deterministic and the tests can compare it as text.
//! Both protocol servers share it: the language server and the Debug Adapter Protocol server (`spec/debug.md`,
//! DBG11).

use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// Every number. LSP's integers (positions, ids, versions) stay below 2^53, where `f64` is exact.
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// An empty object, to be filled with [`Json::with`].
    pub fn obj() -> Json {
        Json::Obj(Vec::new())
    }

    /// The object with `key` set to `value` (replacing an earlier value of the key).
    pub fn with(mut self, key: &str, value: impl Into<Json>) -> Json {
        self.set(key, value);
        self
    }

    pub fn set(&mut self, key: &str, value: impl Into<Json>) {
        if let Json::Obj(fields) = self {
            let value = value.into();
            match fields.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = value,
                None => fields.push((key.to_string(), value)),
            }
        }
    }

    /// The value of a key of an object; `Null` when absent or when `self` is not an object.
    pub fn get(&self, key: &str) -> &Json {
        static NULL: Json = Json::Null;
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map_or(&NULL, |(_, v)| v),
            _ => &NULL,
        }
    }

    /// Drops a key of an object, if it has one. The DAP server uses it to take the fields it interprets out of a
    /// request before forwarding it (`spec/debug.md`, DBG11).
    pub fn remove(&mut self, key: &str) {
        if let Json::Obj(fields) = self {
            fields.retain(|(k, _)| k != key);
        }
    }

    /// A value down a path of keys.
    pub fn at(&self, path: &[&str]) -> &Json {
        path.iter().fold(self, |v, key| v.get(key))
    }

    /// Sets `value` down a path of keys, creating the objects on the way. The DAP server uses it to rewrite a
    /// request's arguments in place (`spec/debug.md`, DBG11).
    pub fn set_at(&mut self, path: &[&str], value: impl Into<Json>) {
        let value = value.into();
        match path {
            [] => *self = value,
            [key] => self.set(key, value),
            [key, rest @ ..] => {
                if !matches!(self.get(key), Json::Obj(_)) {
                    self.set(key, Json::obj());
                }
                if let Json::Obj(fields) = self
                    && let Some(slot) = fields.iter_mut().find(|(k, _)| k == key)
                {
                    slot.1.set_at(rest, value);
                }
            }
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        self.as_f64().filter(|n| *n >= 0.0 && n.fract() == 0.0 && *n <= u32::MAX as f64).map(|n| n as u32)
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_f64().filter(|n| n.fract() == 0.0).map(|n| n as i64)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> &[Json] {
        match self {
            Json::Arr(items) => items,
            _ => &[],
        }
    }

    /// The fields of an object, in order (empty for anything else).
    pub fn as_object(&self) -> &[(String, Json)] {
        match self {
            Json::Obj(fields) => fields,
            _ => &[],
        }
    }

    /// The compact text of the value.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                if n.is_finite() && n.fract() == 0.0 && n.abs() < 9.0e15 {
                    let _ = write!(out, "{}", *n as i64);
                } else if n.is_finite() {
                    let _ = write!(out, "{n}");
                } else {
                    out.push_str("null");
                }
            }
            Json::Str(s) => write_string(s, out),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Obj(fields) => {
                out.push('{');
                for (i, (key, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(key, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }

    /// Indented text, two spaces per level, for golden files a person reads.
    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        self.pretty(&mut out, 0);
        out
    }

    fn pretty(&self, out: &mut String, depth: usize) {
        let indent = |out: &mut String, depth: usize| out.push_str(&"  ".repeat(depth));
        match self {
            Json::Arr(items) if !items.is_empty() => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    indent(out, depth + 1);
                    item.pretty(out, depth + 1);
                    out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                }
                indent(out, depth);
                out.push(']');
            }
            Json::Obj(fields) if !fields.is_empty() => {
                out.push_str("{\n");
                for (i, (key, value)) in fields.iter().enumerate() {
                    indent(out, depth + 1);
                    write_string(key, out);
                    out.push_str(": ");
                    value.pretty(out, depth + 1);
                    out.push_str(if i + 1 < fields.len() { ",\n" } else { "\n" });
                }
                indent(out, depth);
                out.push('}');
            }
            other => other.write(out),
        }
    }

    /// Parses a JSON text. The error says what was expected and at which byte.
    pub fn parse(text: &str) -> Result<Json, String> {
        let mut p = Parser { s: text.as_bytes(), at: 0, depth: 0 };
        p.ws();
        let value = p.value()?;
        p.ws();
        if p.at != p.s.len() {
            return Err(format!("unexpected text after the JSON value at byte {}", p.at));
        }
        Ok(value)
    }
}

impl From<bool> for Json {
    fn from(b: bool) -> Json {
        Json::Bool(b)
    }
}

impl From<&str> for Json {
    fn from(s: &str) -> Json {
        Json::Str(s.to_string())
    }
}

impl From<String> for Json {
    fn from(s: String) -> Json {
        Json::Str(s)
    }
}

impl From<&String> for Json {
    fn from(s: &String) -> Json {
        Json::Str(s.clone())
    }
}

impl From<u32> for Json {
    fn from(n: u32) -> Json {
        Json::Num(n as f64)
    }
}

impl From<i32> for Json {
    fn from(n: i32) -> Json {
        Json::Num(n as f64)
    }
}

impl From<i64> for Json {
    fn from(n: i64) -> Json {
        Json::Num(n as f64)
    }
}

impl From<u64> for Json {
    fn from(n: u64) -> Json {
        Json::Num(n as f64)
    }
}

impl From<usize> for Json {
    fn from(n: usize) -> Json {
        Json::Num(n as f64)
    }
}

impl From<f64> for Json {
    fn from(n: f64) -> Json {
        Json::Num(n)
    }
}

impl From<Vec<Json>> for Json {
    fn from(items: Vec<Json>) -> Json {
        Json::Arr(items)
    }
}

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(value: Option<T>) -> Json {
        value.map_or(Json::Null, Into::into)
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
    depth: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.at < self.s.len() && matches!(self.s[self.at], b' ' | b'\t' | b'\n' | b'\r') {
            self.at += 1;
        }
    }

    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("expected {what} at byte {}", self.at))
    }

    fn eat(&mut self, word: &str) -> bool {
        if self.s[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.depth += 1;
        if self.depth > 512 {
            return Err("the JSON value is nested too deeply".to_string());
        }
        let value = match self.s.get(self.at) {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') if self.eat("true") => Ok(Json::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(Json::Bool(false)),
            Some(b'n') if self.eat("null") => Ok(Json::Null),
            Some(c) if *c == b'-' || c.is_ascii_digit() => self.number(),
            _ => self.err("a JSON value"),
        };
        self.depth -= 1;
        value
    }

    fn object(&mut self) -> Result<Json, String> {
        self.at += 1;
        let mut fields = Vec::new();
        self.ws();
        if self.eat("}") {
            return Ok(Json::Obj(fields));
        }
        loop {
            self.ws();
            if self.s.get(self.at) != Some(&b'"') {
                return self.err("a string key");
            }
            let key = self.string()?;
            self.ws();
            if !self.eat(":") {
                return self.err("`:`");
            }
            self.ws();
            let value = self.value()?;
            match fields.iter_mut().find(|(k, _): &&mut (String, Json)| *k == key) {
                Some(slot) => slot.1 = value,
                None => fields.push((key, value)),
            }
            self.ws();
            if self.eat(",") {
                continue;
            }
            if self.eat("}") {
                return Ok(Json::Obj(fields));
            }
            return self.err("`,` or `}`");
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.at += 1;
        let mut items = Vec::new();
        self.ws();
        if self.eat("]") {
            return Ok(Json::Arr(items));
        }
        loop {
            self.ws();
            items.push(self.value()?);
            self.ws();
            if self.eat(",") {
                continue;
            }
            if self.eat("]") {
                return Ok(Json::Arr(items));
            }
            return self.err("`,` or `]`");
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits =
            self.s.get(self.at..self.at + 4).ok_or_else(|| format!("expected 4 hex digits at byte {}", self.at))?;
        let text = std::str::from_utf8(digits).map_err(|_| format!("expected 4 hex digits at byte {}", self.at))?;
        let n = u32::from_str_radix(text, 16).map_err(|_| format!("expected 4 hex digits at byte {}", self.at))?;
        self.at += 4;
        Ok(n)
    }

    fn string(&mut self) -> Result<String, String> {
        self.at += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(&c) = self.s.get(self.at) else { return self.err("the end of the string") };
            self.at += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let Some(&e) = self.s.get(self.at) else { return self.err("an escape") };
                    self.at += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xd800..0xdc00).contains(&hi) && self.eat("\\u") {
                                let lo = self.hex4()?;
                                if (0xdc00..0xe000).contains(&lo) {
                                    0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00)
                                } else {
                                    0xfffd
                                }
                            } else {
                                hi
                            };
                            char::from_u32(code).unwrap_or('\u{fffd}')
                        }
                        _ => return self.err("a valid escape"),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                c => out.push(c),
            }
        }
        String::from_utf8(out).map_err(|_| format!("invalid UTF-8 in a string before byte {}", self.at))
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.at;
        while self.at < self.s.len() && matches!(self.s[self.at], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
            self.at += 1;
        }
        let text = std::str::from_utf8(&self.s[start..self.at]).unwrap_or("");
        text.parse::<f64>().map(Json::Num).map_err(|_| format!("invalid number at byte {start}"))
    }
}

#[cfg(test)]
mod tests {
    use super::Json;

    #[test]
    fn values_round_trip() {
        let text = r#"{"a":[1,2.5,-3,true,false,null],"b":"x\"\\\n\u0001é😀","c":{}}"#;
        let value = Json::parse(text).unwrap();
        assert_eq!(value.get("a").as_array().len(), 6);
        assert_eq!(value.get("b").as_str(), Some("x\"\\\n\u{1}é😀"));
        assert_eq!(Json::parse(&value.to_text()).unwrap(), value);
    }

    #[test]
    fn surrogate_pairs_decode() {
        assert_eq!(Json::parse(r#""😀""#).unwrap().as_str(), Some("😀"));
    }

    #[test]
    fn errors_name_the_position() {
        assert!(Json::parse("{\"a\" 1}").unwrap_err().contains("byte 5"));
        assert!(Json::parse("[1,]").is_err());
        assert!(Json::parse("1 2").is_err());
    }

    #[test]
    fn integers_print_without_a_fraction() {
        assert_eq!(Json::from(42u32).to_text(), "42");
        assert_eq!(Json::from(-1i64).to_text(), "-1");
        assert_eq!(Json::from(0.5).to_text(), "0.5");
    }
}
