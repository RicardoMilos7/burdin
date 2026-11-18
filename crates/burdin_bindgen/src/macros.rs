//! Object-like macros as constants (`spec/bindgen.md`, BG9).
//!
//! libclang hands over a macro as its tokens. This module evaluates the ones that are constant expressions: integer,
//! character, floating-point, and string literals, the C operators on them, casts to the integer and floating-point
//! types, parentheses, and names of earlier macros and enumerators. Anything else (a function call, `sizeof`, a type
//! name that is not a plain integer type) is not a constant to Burdin, and the macro is left out.
//!
//! Integer arithmetic follows C's rules for the width and signedness of the result: a literal takes the first of
//! `int`, `unsigned int`, `long`, ... that holds it, an operation takes the wider of its operands (unsigned wins a
//! tie), and the value wraps to that width. `long` is 32 bits where the target says so (`Config::long_bits`).

use crate::clang::{Token, TokenKind};

/// The target facts evaluation needs.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Width of C `long` in bits: 32 on Windows, 64 elsewhere.
    pub long_bits: u8,
}

/// An integer type: a width in bits and a signedness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntTy {
    pub bits: u8,
    pub unsigned: bool,
}

impl IntTy {
    pub const INT: IntTy = IntTy { bits: 32, unsigned: false };

    /// The Burdin spelling of the type.
    pub fn name(self) -> &'static str {
        match (self.bits, self.unsigned) {
            (8, false) => "i8",
            (8, true) => "u8",
            (16, false) => "i16",
            (16, true) => "u16",
            (32, false) => "i32",
            (32, true) => "u32",
            (_, false) => "i64",
            (_, true) => "u64",
        }
    }

    /// `value` reduced to this type's range, the way C wraps it.
    pub fn wrap(self, value: i128) -> i128 {
        let modulus: i128 = 1 << self.bits;
        let mut value = value.rem_euclid(modulus);
        if !self.unsigned && value >= modulus / 2 {
            value -= modulus;
        }
        value
    }

    fn fits(self, value: i128) -> bool {
        self.wrap(value) == value
    }

    /// C's integer promotion: anything narrower than `int` becomes `int`.
    fn promoted(self) -> IntTy {
        if self.bits < 32 { IntTy::INT } else { self }
    }
}

/// The value of a constant expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i128, IntTy),
    Float(f64, bool),
    Str(String),
}

/// What a name inside a macro refers to.
pub enum Lookup {
    /// A constant that is already known, such as an enumerator.
    Value(Value),
    /// Another object-like macro; its tokens (without its name) are evaluated in turn.
    Tokens(Vec<Token>),
    Unknown,
}

/// Evaluates the body of a macro. `lookup` resolves identifiers.
pub fn evaluate(tokens: &[Token], config: Config, lookup: &mut dyn FnMut(&str) -> Lookup) -> Option<Value> {
    let mut parser = Parser { tokens, pos: 0, config, lookup, depth: 0 };
    let value = parser.ternary()?;
    if parser.pos == tokens.len() { Some(value) } else { None }
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    config: Config,
    lookup: &'a mut dyn FnMut(&str) -> Lookup,
    depth: u32,
}

const MAX_DEPTH: u32 = 24;

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_punct(&self, text: &str) -> bool {
        self.peek().is_some_and(|token| token.kind == TokenKind::Punctuation && token.text == text)
    }

    fn eat_punct(&mut self, text: &str) -> bool {
        if self.peek_punct(text) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn ternary(&mut self) -> Option<Value> {
        let condition = self.binary(1)?;
        if !self.eat_punct("?") {
            return Some(condition);
        }
        let then = self.ternary()?;
        if !self.eat_punct(":") {
            return None;
        }
        let otherwise = self.ternary()?;
        if truth(&condition)? { Some(then) } else { Some(otherwise) }
    }

    fn precedence(op: &str) -> Option<u8> {
        Some(match op {
            "||" => 1,
            "&&" => 2,
            "|" => 3,
            "^" => 4,
            "&" => 5,
            "==" | "!=" => 6,
            "<" | ">" | "<=" | ">=" => 7,
            "<<" | ">>" => 8,
            "+" | "-" => 9,
            "*" | "/" | "%" => 10,
            _ => return None,
        })
    }

    fn binary(&mut self, min: u8) -> Option<Value> {
        let mut left = self.unary()?;
        while let Some(token) = self.peek() {
            if token.kind != TokenKind::Punctuation {
                break;
            }
            let op = token.text.clone();
            let Some(level) = Self::precedence(&op) else { break };
            if level < min {
                break;
            }
            self.pos += 1;
            let right = self.binary(level + 1)?;
            left = apply(&op, left, right)?;
        }
        Some(left)
    }

    fn unary(&mut self) -> Option<Value> {
        if self.eat_punct("-") {
            return match self.unary()? {
                Value::Int(value, ty) => {
                    let ty = ty.promoted();
                    Some(Value::Int(ty.wrap(-value), ty))
                }
                Value::Float(value, single) => Some(Value::Float(-value, single)),
                Value::Str(_) => None,
            };
        }
        if self.eat_punct("+") {
            return match self.unary()? {
                Value::Int(value, ty) => Some(Value::Int(value, ty.promoted())),
                other @ Value::Float(..) => Some(other),
                Value::Str(_) => None,
            };
        }
        if self.eat_punct("~") {
            return match self.unary()? {
                Value::Int(value, ty) => {
                    let ty = ty.promoted();
                    Some(Value::Int(ty.wrap(!value), ty))
                }
                _ => None,
            };
        }
        if self.eat_punct("!") {
            let value = self.unary()?;
            return Some(Value::Int(i128::from(!truth(&value)?), IntTy::INT));
        }
        if self.peek_punct("(")
            && let Some((target, after)) = self.cast_ahead()
        {
            self.pos = after;
            let operand = self.unary()?;
            return convert(operand, target);
        }
        self.primary()
    }

    /// When the `(` at the current position opens a cast to a plain numeric type, the target and the index of the
    /// token after the closing `)`.
    fn cast_ahead(&self) -> Option<(CastTarget, usize)> {
        let mut index = self.pos + 1;
        let mut words: Vec<&str> = Vec::new();
        while let Some(token) = self.tokens.get(index) {
            if token.kind == TokenKind::Punctuation && token.text == ")" {
                break;
            }
            if !matches!(token.kind, TokenKind::Keyword | TokenKind::Identifier) {
                return None;
            }
            words.push(token.text.as_str());
            index += 1;
        }
        self.tokens.get(index)?;
        let target = cast_target(&words, self.config)?;
        // `(int) - 1` is a cast, but `(size_t)` alone, or `(size_t) * 2` as a product, would be a parenthesized name.
        let next = self.tokens.get(index + 1)?;
        let starts_operand = match next.kind {
            TokenKind::Punctuation => matches!(next.text.as_str(), "(" | "-" | "+" | "~" | "!"),
            TokenKind::Comment => false,
            _ => true,
        };
        if starts_operand { Some((target, index + 1)) } else { None }
    }

    fn primary(&mut self) -> Option<Value> {
        let token = self.peek()?.clone();
        match token.kind {
            TokenKind::Literal => {
                self.pos += 1;
                if token.text.starts_with('"') {
                    // Adjacent string literals are one string.
                    let mut text = decode_string(&token.text)?;
                    while let Some(next) = self.peek()
                        && next.kind == TokenKind::Literal
                        && next.text.starts_with('"')
                    {
                        text.push_str(&decode_string(&next.text)?);
                        self.pos += 1;
                    }
                    return Some(Value::Str(text));
                }
                literal(&token.text, self.config)
            }
            TokenKind::Identifier => {
                self.pos += 1;
                match (self.lookup)(&token.text) {
                    Lookup::Value(value) => Some(value),
                    Lookup::Tokens(tokens) => {
                        if self.depth >= MAX_DEPTH {
                            return None;
                        }
                        let mut inner = Parser {
                            tokens: &tokens,
                            pos: 0,
                            config: self.config,
                            lookup: &mut *self.lookup,
                            depth: self.depth + 1,
                        };
                        let value = inner.ternary()?;
                        if inner.pos == tokens.len() { Some(value) } else { None }
                    }
                    Lookup::Unknown => None,
                }
            }
            TokenKind::Punctuation if token.text == "(" => {
                self.pos += 1;
                let value = self.ternary()?;
                if self.eat_punct(")") { Some(value) } else { None }
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
enum CastTarget {
    Int(IntTy),
    Float(bool),
}

/// The numeric type a run of type words names, or `None` for anything else.
fn cast_target(words: &[&str], config: Config) -> Option<CastTarget> {
    let named = match words {
        ["size_t"] | ["uintptr_t"] => Some(CastTarget::Int(IntTy { bits: 64, unsigned: true })),
        ["ssize_t"] | ["ptrdiff_t"] | ["intptr_t"] => Some(CastTarget::Int(IntTy { bits: 64, unsigned: false })),
        ["int8_t"] => Some(CastTarget::Int(IntTy { bits: 8, unsigned: false })),
        ["uint8_t"] => Some(CastTarget::Int(IntTy { bits: 8, unsigned: true })),
        ["int16_t"] => Some(CastTarget::Int(IntTy { bits: 16, unsigned: false })),
        ["uint16_t"] => Some(CastTarget::Int(IntTy { bits: 16, unsigned: true })),
        ["int32_t"] => Some(CastTarget::Int(IntTy::INT)),
        ["uint32_t"] => Some(CastTarget::Int(IntTy { bits: 32, unsigned: true })),
        ["int64_t"] => Some(CastTarget::Int(IntTy { bits: 64, unsigned: false })),
        ["uint64_t"] => Some(CastTarget::Int(IntTy { bits: 64, unsigned: true })),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let mut unsigned = false;
    let mut signed = false;
    let mut longs = 0;
    let mut base: Option<&str> = None;
    for word in words {
        match *word {
            "const" | "volatile" => {}
            "unsigned" => unsigned = true,
            "signed" => signed = true,
            "long" => longs += 1,
            "int" | "short" | "char" | "float" | "double" if base.is_none() => base = Some(word),
            _ => return None,
        }
    }
    match base {
        Some("float") if !unsigned && !signed && longs == 0 => Some(CastTarget::Float(true)),
        Some("double") if !unsigned && !signed && longs == 0 => Some(CastTarget::Float(false)),
        Some("float") | Some("double") => None,
        Some("char") if longs == 0 => Some(CastTarget::Int(IntTy { bits: 8, unsigned })),
        Some("short") if longs == 0 => Some(CastTarget::Int(IntTy { bits: 16, unsigned })),
        Some("int") | None if longs == 0 => {
            // A bare `signed` or `unsigned` is `int`; with no word at all it is not a type.
            if base.is_none() && !unsigned && !signed {
                return None;
            }
            Some(CastTarget::Int(IntTy { bits: 32, unsigned }))
        }
        Some("int") | None if longs == 1 => Some(CastTarget::Int(IntTy { bits: config.long_bits, unsigned })),
        Some("int") | None if longs == 2 => Some(CastTarget::Int(IntTy { bits: 64, unsigned })),
        _ => None,
    }
}

fn convert(value: Value, target: CastTarget) -> Option<Value> {
    match (value, target) {
        (Value::Int(value, _), CastTarget::Int(ty)) => Some(Value::Int(ty.wrap(value), ty)),
        (Value::Float(value, _), CastTarget::Int(ty)) => {
            if value.is_finite() {
                Some(Value::Int(ty.wrap(value.trunc() as i128), ty))
            } else {
                None
            }
        }
        (Value::Int(value, _), CastTarget::Float(single)) => Some(Value::Float(value as f64, single)),
        (Value::Float(value, _), CastTarget::Float(single)) => Some(Value::Float(value, single)),
        (Value::Str(_), _) => None,
    }
}

fn truth(value: &Value) -> Option<bool> {
    match value {
        Value::Int(value, _) => Some(*value != 0),
        Value::Float(value, _) => Some(*value != 0.0),
        Value::Str(_) => None,
    }
}

fn common(a: IntTy, b: IntTy) -> IntTy {
    let (a, b) = (a.promoted(), b.promoted());
    if a.bits != b.bits {
        return if a.bits > b.bits { a } else { b };
    }
    IntTy { bits: a.bits, unsigned: a.unsigned || b.unsigned }
}

fn apply(op: &str, left: Value, right: Value) -> Option<Value> {
    match (left, right) {
        (Value::Int(a, at), Value::Int(b, bt)) => {
            if matches!(op, "<<" | ">>") {
                let ty = at.promoted();
                if b < 0 || b >= ty.bits as i128 {
                    return None;
                }
                let result = if op == "<<" { a << b } else { a >> b };
                return Some(Value::Int(ty.wrap(result), ty));
            }
            let ty = common(at, bt);
            let (a, b) = (ty.wrap(a), ty.wrap(b));
            let flag = |condition: bool| Some(Value::Int(i128::from(condition), IntTy::INT));
            let arithmetic = |result: i128| Some(Value::Int(ty.wrap(result), ty));
            match op {
                "+" => arithmetic(a + b),
                "-" => arithmetic(a - b),
                "*" => arithmetic(a * b),
                "/" => {
                    if b == 0 {
                        None
                    } else {
                        arithmetic(a / b)
                    }
                }
                "%" => {
                    if b == 0 {
                        None
                    } else {
                        arithmetic(a % b)
                    }
                }
                "&" => arithmetic(a & b),
                "|" => arithmetic(a | b),
                "^" => arithmetic(a ^ b),
                "==" => flag(a == b),
                "!=" => flag(a != b),
                "<" => flag(a < b),
                ">" => flag(a > b),
                "<=" => flag(a <= b),
                ">=" => flag(a >= b),
                "&&" => flag(a != 0 && b != 0),
                "||" => flag(a != 0 || b != 0),
                _ => None,
            }
        }
        (Value::Str(_), _) | (_, Value::Str(_)) => None,
        (left, right) => {
            let (a, a_single) = as_float(&left)?;
            let (b, b_single) = as_float(&right)?;
            let single = a_single && b_single;
            let flag = |condition: bool| Some(Value::Int(i128::from(condition), IntTy::INT));
            match op {
                "+" => Some(Value::Float(a + b, single)),
                "-" => Some(Value::Float(a - b, single)),
                "*" => Some(Value::Float(a * b, single)),
                "/" => Some(Value::Float(a / b, single)),
                "==" => flag(a == b),
                "!=" => flag(a != b),
                "<" => flag(a < b),
                ">" => flag(a > b),
                "<=" => flag(a <= b),
                ">=" => flag(a >= b),
                _ => None,
            }
        }
    }
}

fn as_float(value: &Value) -> Option<(f64, bool)> {
    match value {
        Value::Int(value, _) => Some((*value as f64, false)),
        Value::Float(value, single) => Some((*value, *single)),
        Value::Str(_) => None,
    }
}

/// One numeric or character literal token.
fn literal(text: &str, config: Config) -> Option<Value> {
    if let Some(inner) = text.strip_prefix('\'') {
        let inner = inner.strip_suffix('\'')?;
        let mut chars = decode_escapes(inner)?;
        if chars.len() != 1 {
            return None;
        }
        return Some(Value::Int(i128::from(chars.remove(0) as i8), IntTy::INT));
    }
    let lower = text.to_ascii_lowercase();
    let is_hex = lower.starts_with("0x");
    let is_float = !is_hex && (lower.contains('.') || lower.contains('e'));
    if is_float {
        let single = lower.ends_with('f');
        let number = lower.trim_end_matches(['f', 'l']);
        let value: f64 = number.parse().ok()?;
        return value.is_finite().then_some(Value::Float(value, single));
    }
    let number = lower.trim_end_matches(['u', 'l']);
    let suffix = &lower[number.len()..];
    let unsigned_suffix = suffix.contains('u');
    let longs = suffix.matches('l').count();
    if longs > 2 || suffix.matches('u').count() > 1 {
        return None;
    }
    let (digits, radix) = if let Some(hex) = number.strip_prefix("0x") {
        (hex, 16)
    } else if let Some(binary) = number.strip_prefix("0b") {
        (binary, 2)
    } else if number.len() > 1 && number.starts_with('0') {
        (&number[1..], 8)
    } else {
        (number, 10)
    };
    let value = i128::from_str_radix(digits, radix).ok()?;
    let long = config.long_bits;
    let ladder = [
        (IntTy::INT, 0),
        (IntTy { bits: 32, unsigned: true }, 0),
        (IntTy { bits: long, unsigned: false }, 1),
        (IntTy { bits: long, unsigned: true }, 1),
        (IntTy { bits: 64, unsigned: false }, 2),
        (IntTy { bits: 64, unsigned: true }, 2),
    ];
    for (ty, rank) in ladder {
        if rank < longs {
            continue;
        }
        if unsigned_suffix && !ty.unsigned {
            continue;
        }
        // A decimal literal without `u` is never unsigned; octal, hex, and binary ones may be.
        if radix == 10 && !unsigned_suffix && ty.unsigned {
            continue;
        }
        if ty.fits(value) {
            return Some(Value::Int(value, ty));
        }
    }
    None
}

/// The bytes of a C string literal's escape sequences, read as characters.
fn decode_escapes(body: &str) -> Option<Vec<u8>> {
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let c = bytes[index];
        index += 1;
        if c != b'\\' {
            out.push(c);
            continue;
        }
        let e = *bytes.get(index)?;
        index += 1;
        match e {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'a' => out.push(7),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'v' => out.push(11),
            b'\\' | b'\'' | b'"' | b'?' => out.push(e),
            b'x' => {
                let start = index;
                while index < bytes.len() && bytes[index].is_ascii_hexdigit() {
                    index += 1;
                }
                if start == index {
                    return None;
                }
                let value = u32::from_str_radix(std::str::from_utf8(&bytes[start..index]).ok()?, 16).ok()?;
                out.push(u8::try_from(value).ok()?);
            }
            b'0'..=b'7' => {
                let mut value = u32::from(e - b'0');
                let mut digits = 1;
                while digits < 3 && index < bytes.len() && (b'0'..=b'7').contains(&bytes[index]) {
                    value = value * 8 + u32::from(bytes[index] - b'0');
                    index += 1;
                    digits += 1;
                }
                out.push(u8::try_from(value).ok()?);
            }
            _ => return None,
        }
    }
    Some(out)
}

/// The text of a plain C string literal, or `None` when it is not valid UTF-8 or has a prefix.
fn decode_string(literal: &str) -> Option<String> {
    let body = literal.strip_prefix('"')?.strip_suffix('"')?;
    String::from_utf8(decode_escapes(body)?).ok()
}

/// A string as a Burdin string literal: the escapes Burdin has, and doubled braces. `None` for a control
/// character Burdin cannot spell.
pub fn burdin_string(text: &str) -> Option<String> {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            '{' => out.push_str("{{"),
            '}' => out.push_str("}}"),
            c if c.is_control() => return None,
            c => out.push(c),
        }
    }
    out.push('"');
    Some(out)
}

/// A float as a Burdin literal, always with a `.` or an exponent so it is not an integer.
pub fn burdin_float(value: f64) -> String {
    let text = format!("{value:?}");
    if text.contains(['.', 'e', 'E']) { text } else { format!("{text}.0") }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LP64: Config = Config { long_bits: 64 };
    const LLP64: Config = Config { long_bits: 32 };

    /// A tiny tokenizer for the tests: libclang does this for the real tool.
    fn lex(text: &str) -> Vec<Token> {
        let chars: Vec<char> = text.chars().collect();
        let mut tokens = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c.is_whitespace() {
                i += 1;
            } else if c.is_ascii_alphabetic() || c == '_' {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                let keyword = matches!(
                    word.as_str(),
                    "unsigned" | "signed" | "int" | "long" | "short" | "char" | "float" | "double" | "const"
                );
                let kind = if keyword { TokenKind::Keyword } else { TokenKind::Identifier };
                tokens.push(Token { kind, text: word });
            } else if c.is_ascii_digit() || c == '.' {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric()
                        || chars[i] == '.'
                        || ((chars[i] == '-' || chars[i] == '+') && matches!(chars[i - 1], 'e' | 'E')))
                {
                    i += 1;
                }
                tokens.push(Token { kind: TokenKind::Literal, text: chars[start..i].iter().collect() });
            } else if c == '"' || c == '\'' {
                let start = i;
                i += 1;
                while i < chars.len() && chars[i] != c {
                    i += if chars[i] == '\\' { 2 } else { 1 };
                }
                i += 1;
                tokens.push(Token { kind: TokenKind::Literal, text: chars[start..i].iter().collect() });
            } else {
                let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
                let op = if ["<<", ">>", "<=", ">=", "==", "!=", "&&", "||"].contains(&two.as_str()) {
                    two
                } else {
                    c.to_string()
                };
                i += op.len();
                tokens.push(Token { kind: TokenKind::Punctuation, text: op });
            }
        }
        tokens
    }

    fn eval_with(text: &str, config: Config) -> Option<Value> {
        let mut lookup = |name: &str| match name {
            "BASE" => Lookup::Value(Value::Int(100, IntTy::INT)),
            "CHAIN" => Lookup::Tokens(lex("BASE + 1")),
            "LOOP" => Lookup::Tokens(lex("LOOP")),
            _ => Lookup::Unknown,
        };
        evaluate(&lex(text), config, &mut lookup)
    }

    fn eval(text: &str) -> Option<Value> {
        eval_with(text, LP64)
    }

    fn int(value: i128, bits: u8, unsigned: bool) -> Option<Value> {
        Some(Value::Int(value, IntTy { bits, unsigned }))
    }

    #[test]
    fn integer_literals_take_the_first_type_that_holds_them() {
        assert_eq!(eval("42"), int(42, 32, false));
        assert_eq!(eval("0x7fffffff"), int(0x7fff_ffff, 32, false));
        assert_eq!(eval("0xffffffff"), int(0xffff_ffff, 32, true));
        assert_eq!(eval("4294967296"), int(4_294_967_296, 64, false));
        assert_eq!(eval("10u"), int(10, 32, true));
        assert_eq!(eval("10UL"), int(10, 64, true));
        assert_eq!(eval("010"), int(8, 32, false));
        assert_eq!(eval("0b101"), int(5, 32, false));
        assert_eq!(eval("18446744073709551615"), None, "an unsuffixed decimal never becomes unsigned");
        assert_eq!(eval("18446744073709551615u"), int(u64::MAX as i128, 64, true));
    }

    #[test]
    fn long_is_as_wide_as_the_target_says() {
        assert_eq!(eval_with("5L", LP64), int(5, 64, false));
        assert_eq!(eval_with("5L", LLP64), int(5, 32, false));
        assert_eq!(eval_with("5LL", LLP64), int(5, 64, false));
        assert_eq!(eval_with("3000000000L", LLP64), int(3_000_000_000, 64, false));
    }

    #[test]
    fn operators_follow_c_precedence_and_width() {
        assert_eq!(eval("1 + 2 * 3"), int(7, 32, false));
        assert_eq!(eval("(1 << 4) | 3"), int(19, 32, false));
        assert_eq!(eval("1 << 31"), int(-2_147_483_648, 32, false));
        assert_eq!(eval("1u << 31"), int(2_147_483_648, 32, true));
        assert_eq!(eval("-1"), int(-1, 32, false));
        assert_eq!(eval("~0u"), int(0xffff_ffff, 32, true));
        assert_eq!(eval("0xffffffffu + 1"), int(0, 32, true));
        assert_eq!(eval("1 ? 5 : 6"), int(5, 32, false));
        assert_eq!(eval("2 > 1 && 3 != 3"), int(0, 32, false));
        assert_eq!(eval("7 / 0"), None);
        assert_eq!(eval("1 << 40"), None);
    }

    #[test]
    fn names_resolve_to_earlier_constants_and_macros() {
        assert_eq!(eval("BASE * 2"), int(200, 32, false));
        assert_eq!(eval("CHAIN"), int(101, 32, false));
        assert_eq!(eval("LOOP"), None, "a self-referential macro is not a constant");
        assert_eq!(eval("MISSING"), None);
    }

    #[test]
    fn casts_pick_the_type() {
        assert_eq!(eval("(unsigned)-1"), int(0xffff_ffff, 32, true));
        assert_eq!(eval("(uint8_t)300"), int(44, 8, true));
        assert_eq!(eval("(long)1"), int(1, 64, false));
        assert_eq!(eval("(size_t)1"), int(1, 64, true));
        assert_eq!(eval("(double)1"), Some(Value::Float(1.0, false)));
        assert_eq!(eval("(BASE) + 1"), int(101, 32, false), "a parenthesized name is not a cast");
    }

    #[test]
    fn floats_and_characters() {
        assert_eq!(eval("3.5"), Some(Value::Float(3.5, false)));
        assert_eq!(eval("2.5f"), Some(Value::Float(2.5, true)));
        assert_eq!(eval("1e3"), Some(Value::Float(1000.0, false)));
        assert_eq!(eval("1.5 * 2"), Some(Value::Float(3.0, false)));
        assert_eq!(eval("'A'"), int(65, 32, false));
        assert_eq!(eval("'\\n'"), int(10, 32, false));
        assert_eq!(eval("'\\x41'"), int(65, 32, false));
    }

    #[test]
    fn strings_concatenate_and_decode() {
        assert_eq!(eval("\"ab\" \"cd\""), Some(Value::Str("abcd".to_string())));
        assert_eq!(eval("\"a\\tb\""), Some(Value::Str("a\tb".to_string())));
        assert_eq!(eval("\"a\" + 1"), None);
        assert_eq!(eval("\"\\xff\""), None, "not valid UTF-8");
    }

    #[test]
    fn what_is_not_a_constant_is_left_out() {
        assert_eq!(eval("sizeof(int)"), None);
        assert_eq!(eval("f(1)"), None);
        assert_eq!(eval("(1"), None);
        assert_eq!(eval("1 2"), None);
        assert_eq!(eval(""), None);
    }

    #[test]
    fn burdin_literals_escape_braces_and_quotes() {
        assert_eq!(burdin_string("a\"b{c}\n").as_deref(), Some("\"a\\\"b{{c}}\\n\""));
        assert_eq!(burdin_string("a\u{1}"), None);
        assert_eq!(burdin_float(3.0), "3.0");
        assert_eq!(burdin_float(0.25), "0.25");
    }
}
