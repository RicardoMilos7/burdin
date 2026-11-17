//! The primitive operations: operators, casts, built-in functions and methods, and the structural questions
//! (equality, ordering, hashing, layout) that compiled code answers with generated glue. Each one follows the code
//! generator's behavior so a comptime result is the result the compiled program would compute (CT1).

use crate::interp::{Fault, Interp, Mode, R, impure, range_bounds};
use crate::value::Value;
use burdin_diag::Span;
use burdin_tir::{
    self as tir, Alloc, BinOp, Builtin, Expr, ExprKind as K, FloatOp, FloatTy, IntTy, Method, Program, Ty, UnOp,
};
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::rc::Rc;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn panic(msg: impl Into<String>, span: Span) -> crate::interp::Unwind {
    Fault::Panic(msg.into(), span).into()
}

fn internal(msg: impl Into<String>, span: Span) -> crate::interp::Unwind {
    Fault::Internal(msg.into(), span).into()
}

/// `v` reduced to the range of `ty`, two's complement.
pub(crate) fn wrap(v: i128, ty: IntTy) -> i128 {
    let bits = ty.bits();
    let mask: u128 = (1u128 << bits) - 1;
    let m = (v as u128) & mask;
    if ty.signed() && m >= 1u128 << (bits - 1) { (m as i128) - (1i128 << bits) } else { m as i128 }
}

/// The referent of a shared reference, or the value itself.
pub(crate) fn shallow(v: &Value) -> &Value {
    match v {
        Value::Borrow(inner) => shallow(inner),
        other => other,
    }
}

fn round_float(v: f64, ty: &Ty) -> f64 {
    match ty {
        Ty::Float(FloatTy::F32) => f64::from(v as f32),
        _ => v,
    }
}

/// One floating-point operation in `f64` precision (`spec/stdlib.md`, "Float functions"). `to_bits` and
/// `from_bits` are handled with the integer values they produce and consume.
fn float_op64(op: FloatOp, x: f64, args: &[Value]) -> f64 {
    match op {
        FloatOp::Abs => x.abs(),
        FloatOp::Sqrt => x.sqrt(),
        FloatOp::Cbrt => x.cbrt(),
        FloatOp::Powi => x.powi(args[0].int() as i32),
        FloatOp::Powf => x.powf(args[0].float()),
        FloatOp::Exp => x.exp(),
        FloatOp::Exp2 => x.exp2(),
        FloatOp::Ln => x.ln(),
        FloatOp::Log2 => x.log2(),
        FloatOp::Log10 => x.log10(),
        FloatOp::Floor => x.floor(),
        FloatOp::Ceil => x.ceil(),
        FloatOp::Round => x.round(),
        FloatOp::RoundTiesEven => x.round_ties_even(),
        FloatOp::Trunc => x.trunc(),
        FloatOp::Sin => x.sin(),
        FloatOp::Cos => x.cos(),
        FloatOp::Tan => x.tan(),
        FloatOp::Asin => x.asin(),
        FloatOp::Acos => x.acos(),
        FloatOp::Atan => x.atan(),
        FloatOp::Atan2 => x.atan2(args[0].float()),
        FloatOp::Sinh => x.sinh(),
        FloatOp::Cosh => x.cosh(),
        FloatOp::Tanh => x.tanh(),
        FloatOp::Hypot => x.hypot(args[0].float()),
        FloatOp::MulAdd => x.mul_add(args[0].float(), args[1].float()),
        FloatOp::Copysign => x.copysign(args[0].float()),
        FloatOp::ToBits => unreachable!("`to_bits` returns an integer"),
    }
}

/// The `f32` form of [`float_op64`]: every step is in single precision, so computing in `f64` first cannot
/// round twice and disagree with compiled code.
fn float_op32(op: FloatOp, x: f32, args: &[Value]) -> f32 {
    let arg = |i: usize| args[i].float() as f32;
    match op {
        FloatOp::Abs => x.abs(),
        FloatOp::Sqrt => x.sqrt(),
        FloatOp::Cbrt => x.cbrt(),
        FloatOp::Powi => x.powi(args[0].int() as i32),
        FloatOp::Powf => x.powf(arg(0)),
        FloatOp::Exp => x.exp(),
        FloatOp::Exp2 => x.exp2(),
        FloatOp::Ln => x.ln(),
        FloatOp::Log2 => x.log2(),
        FloatOp::Log10 => x.log10(),
        FloatOp::Floor => x.floor(),
        FloatOp::Ceil => x.ceil(),
        FloatOp::Round => x.round(),
        FloatOp::RoundTiesEven => x.round_ties_even(),
        FloatOp::Trunc => x.trunc(),
        FloatOp::Sin => x.sin(),
        FloatOp::Cos => x.cos(),
        FloatOp::Tan => x.tan(),
        FloatOp::Asin => x.asin(),
        FloatOp::Acos => x.acos(),
        FloatOp::Atan => x.atan(),
        FloatOp::Atan2 => x.atan2(arg(0)),
        FloatOp::Sinh => x.sinh(),
        FloatOp::Cosh => x.cosh(),
        FloatOp::Tanh => x.tanh(),
        FloatOp::Hypot => x.hypot(arg(0)),
        FloatOp::MulAdd => x.mul_add(arg(0), arg(1)),
        FloatOp::Copysign => x.copysign(arg(0)),
        FloatOp::ToBits => unreachable!("`to_bits` returns an integer"),
    }
}

fn utf8(c: char) -> Vec<u8> {
    let mut buf = [0u8; 4];
    c.encode_utf8(&mut buf).as_bytes().to_vec()
}

/// Structural equality: the derived `==` (and the comparison `List.contains` and `Map` keys use).
pub(crate) fn value_eq(a: &Value, b: &Value) -> bool {
    let (a, b) = (shallow(a), shallow(b));
    match (a, b) {
        (Value::Unit, Value::Unit) => true,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Str(_) | Value::String(_), Value::Str(_) | Value::String(_)) => a.bytes() == b.bytes(),
        (Value::Tuple(x), Value::Tuple(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| value_eq(p, q)),
        (Value::Variant(i, x), Value::Variant(j, y)) => {
            i == j && x.len() == y.len() && x.iter().zip(y).all(|(p, q)| value_eq(p, q))
        }
        (Value::Array(_) | Value::List(_) | Value::Slice(..), Value::Array(_) | Value::List(_) | Value::Slice(..)) => {
            let (x, y) = (a.elems(), b.elems());
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| value_eq(p, q))
        }
        (Value::Map(x), Value::Map(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.iter().any(|(k2, v2)| value_eq(k, k2) && value_eq(v, v2)))
        }
        (Value::Opt(x), Value::Opt(y)) => match (x, y) {
            (None, None) => true,
            (Some(p), Some(q)) => value_eq(p, q),
            _ => false,
        },
        (Value::Res(x), Value::Res(y)) => match (x, y) {
            (Ok(p), Ok(q)) | (Err(p), Err(q)) => value_eq(p, q),
            _ => false,
        },
        (Value::Heap(x), Value::Heap(y)) => value_eq(x, y),
        (Value::Int(_) | Value::Bool(_) | Value::Char(_), Value::Int(_) | Value::Bool(_) | Value::Char(_)) => {
            a.int() == b.int()
        }
        _ => false,
    }
}

/// Structural ordering: the derived `Ord`, and `str` byte order.
pub(crate) fn value_cmp(a: &Value, b: &Value) -> Ordering {
    let (a, b) = (shallow(a), shallow(b));
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Str(_) | Value::String(_), Value::Str(_) | Value::String(_)) => a.bytes().cmp(b.bytes()),
        (Value::Tuple(x), Value::Tuple(y)) => seq_cmp(x, y),
        (Value::Variant(i, x), Value::Variant(j, y)) => i.cmp(j).then_with(|| seq_cmp(x, y)),
        (Value::Array(_) | Value::List(_) | Value::Slice(..), Value::Array(_) | Value::List(_) | Value::Slice(..)) => {
            seq_cmp(a.elems(), b.elems())
        }
        (Value::Opt(x), Value::Opt(y)) => match (x, y) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (Some(p), Some(q)) => value_cmp(p, q),
        },
        (Value::Int(_) | Value::Bool(_) | Value::Char(_), Value::Int(_) | Value::Bool(_) | Value::Char(_)) => {
            a.int().cmp(&b.int())
        }
        _ => Ordering::Equal,
    }
}

fn seq_cmp(x: &[Value], y: &[Value]) -> Ordering {
    for (p, q) in x.iter().zip(y) {
        match value_cmp(p, q) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    x.len().cmp(&y.len())
}

fn mix(acc: u64, x: u64) -> u64 {
    (acc ^ x).wrapping_mul(FNV_PRIME)
}

/// `x.hash()`: FNV-1a over the value, exactly as the generated `hash_at` glue computes it.
pub(crate) fn hash(p: &Program, v: &Value, ty: &Ty, acc: u64) -> u64 {
    let v = shallow(v);
    match ty {
        Ty::Int(i) => {
            let n = v.int();
            let word = if i.signed() { (n as i64) as u64 } else { n as u64 };
            mix(acc, word)
        }
        Ty::Bool | Ty::Char => mix(acc, v.int() as u64),
        Ty::Float(f) => {
            let x = v.float();
            let bits = if x == 0.0 {
                0
            } else if *f == FloatTy::F32 {
                u64::from((x as f32).to_bits())
            } else {
                x.to_bits()
            };
            mix(acc, bits)
        }
        Ty::Str | Ty::String => v.bytes().iter().fold(acc, |h, b| mix(h, u64::from(*b))),
        Ty::Tuple(ts) => match v {
            Value::Tuple(fs) => fs.iter().zip(ts).fold(acc, |h, (f, t)| hash(p, f, t, h)),
            _ => acc,
        },
        Ty::Struct(a) => match v {
            Value::Tuple(fs) => fs.iter().zip(&p.structs[a.id].fields).fold(acc, |h, (f, d)| hash(p, f, &d.ty, h)),
            _ => acc,
        },
        Ty::Range(inner, _) => {
            let (lo, hi) = range_bounds(v);
            let h = hash(p, &Value::Int(lo), inner, acc);
            hash(p, &Value::Int(hi), inner, h)
        }
        Ty::Option(inner) => match v {
            Value::Opt(Some(x)) => hash(p, x, inner, mix(acc, 1)),
            _ => mix(acc, 0),
        },
        Ty::Result(ok, err) => match v {
            Value::Res(Ok(x)) => hash(p, x, ok, mix(acc, 1)),
            Value::Res(Err(x)) => hash(p, x, err, mix(acc, 0)),
            _ => acc,
        },
        Ty::Enum(a) => match v {
            Value::Variant(i, fs) => {
                let h = mix(acc, *i as u64);
                fs.iter().zip(&p.enums[a.id].variants[*i].fields).fold(h, |h, (f, d)| hash(p, f, &d.ty, h))
            }
            _ => acc,
        },
        Ty::Array(t, _) => v.elems().iter().fold(acc, |h, x| hash(p, x, t, h)),
        _ => acc,
    }
}

/// `Type.size()` and `Type.align()`: the C layout the code generator gives the type on a 64-bit target.
pub(crate) fn layout(p: &Program, ty: &Ty) -> (u64, u64) {
    storage(p, ty).unwrap_or((0, 1))
}

/// A container's layout with its allocator value appended, when it stores one.
fn container(p: &Program, base: (u64, u64), alloc: &Alloc) -> (u64, u64) {
    match alloc.allocator_ty() {
        Some(t) => product(&[base, member(p, t)]),
        None => base,
    }
}

/// The size and alignment of a type's runtime value, or `None` for a type without one.
fn storage(p: &Program, ty: &Ty) -> Option<(u64, u64)> {
    Some(match ty {
        Ty::Unit | Ty::Never | Ty::Error | Ty::Param(_) | Ty::Const(_) => return None,
        Ty::Bool => (1, 1),
        Ty::Char => (4, 4),
        Ty::Int(i) => {
            let n = u64::from(i.bits() / 8);
            (n, n)
        }
        Ty::Float(FloatTy::F32) => (4, 4),
        Ty::Float(FloatTy::F64) => (8, 8),
        Ty::Str | Ty::Chars | Ty::Slice(_) | Ty::Any(_) => (16, 8),
        Ty::Fn { .. } | Ty::String => (24, 8),
        // A container with a stored allocator carries its value after its buffer words (`spec/allocators.md`).
        Ty::List(_, a) => container(p, (24, 8), a),
        Ty::Map(_, _, a) => container(p, (24, 8), a),
        Ty::Heap(_, a) => container(p, (8, 8), a),
        Ty::CString | Ty::CPtr(_) | Ty::Ref { .. } | Ty::Shared(_) => (8, 8),
        Ty::Array(elem, n) => {
            let (size, align) = storage(p, elem)?;
            (size * n, align)
        }
        Ty::Option(inner) => match storage(p, inner) {
            Some(l) => product(&[(1, 1), l]),
            None => (1, 1),
        },
        Ty::Result(ok, err) => product(&[(1, 1), member(p, ok), member(p, err)]),
        Ty::Tuple(ts) => product(&ts.iter().map(|t| member(p, t)).collect::<Vec<_>>()),
        Ty::Struct(a) => product(&p.structs[a.id].fields.iter().map(|f| member(p, &f.ty)).collect::<Vec<_>>()),
        Ty::Range(inner, _) => product(&[member(p, inner), member(p, inner)]),
        Ty::RangeIter(inner, _) => product(&[member(p, inner), member(p, inner), (1, 1)]),
        Ty::ListIntoIter(_, a) | Ty::MapIntoIter(_, _, a) => container(p, (32, 8), a),
        Ty::MapIter(..) | Ty::MapIterMut(..) => (32, 8),
        Ty::IterMut(_) => (16, 8),
        // A pending map lookup: the map's address, the key, the key's hash, the probe slot, and the found flag.
        Ty::MapEntry(key, _, _) => product(&[(8, 8), member(p, key), (8, 8), (8, 8), (1, 1)]),
        Ty::ArrayIntoIter(inner, n) => product(&[member(p, &Ty::Array(inner.clone(), *n)), (8, 8)]),
        Ty::Enum(a) => {
            let mut size = 0;
            let mut align = 1;
            for v in &p.enums[a.id].variants {
                if v.fields.is_empty() {
                    continue;
                }
                let (s, al) = product(&v.fields.iter().map(|f| member(p, &f.ty)).collect::<Vec<_>>());
                size = size.max(s);
                align = align.max(al);
            }
            if size == 0 {
                (4, 4)
            } else {
                let unit = align.min(8);
                let count = size.div_ceil(unit);
                product(&[(4, 4), (count * unit, unit)])
            }
        }
    })
}

/// A member of an aggregate: a type without a value is an empty struct.
fn member(p: &Program, ty: &Ty) -> (u64, u64) {
    storage(p, ty).unwrap_or((0, 1))
}

/// The C layout of a struct with these members.
fn product(members: &[(u64, u64)]) -> (u64, u64) {
    let mut offset: u64 = 0;
    let mut align: u64 = 1;
    for &(size, al) in members {
        offset = offset.div_ceil(al) * al + size;
        align = align.max(al);
    }
    (offset.div_ceil(align) * align, align)
}

/// The value every byte of which is zero, as `const_zero` builds it: the error payload of a failed `parse`.
pub(crate) fn zero_value(p: &Program, ty: &Ty) -> Value {
    match ty {
        Ty::Bool => Value::Bool(false),
        Ty::Char => Value::Char('\0'),
        Ty::Int(_) => Value::Int(0),
        Ty::Float(_) => Value::Float(0.0),
        Ty::Str => Value::str(""),
        Ty::String => Value::String(Vec::new()),
        Ty::List(_, _) => Value::List(Vec::new()),
        Ty::Map(..) => Value::Map(Vec::new()),
        Ty::Tuple(ts) => Value::Tuple(ts.iter().map(|t| zero_value(p, t)).collect()),
        Ty::Struct(a) => Value::Tuple(p.structs[a.id].fields.iter().map(|f| zero_value(p, &f.ty)).collect()),
        Ty::Enum(a) => match p.enums[a.id].variants.first() {
            Some(v) => Value::Variant(0, v.fields.iter().map(|f| zero_value(p, &f.ty)).collect()),
            None => Value::Unit,
        },
        Ty::Array(t, n) => Value::Array(vec![zero_value(p, t); *n as usize]),
        Ty::Option(_) => Value::Opt(None),
        Ty::Result(_, e) => Value::Res(Err(Box::new(zero_value(p, e)))),
        Ty::Range(..) => Value::Tuple(vec![Value::Int(0), Value::Int(0)]),
        _ => Value::Unit,
    }
}

/// `__float_str(v, prec, single)`: `%.{prec}f` when `prec >= 0`, otherwise the shortest `%.{p}e` text that reads
/// back as the same value (as an `f32` when `single`), like the runtime's `f64_to_string`.
pub(crate) fn float_str(v: f64, prec: i128, single: bool) -> String {
    if !v.is_finite() {
        let sign = if v.is_sign_negative() { "-" } else { "" };
        return format!("{sign}{}", if v.is_nan() { "nan" } else { "inf" });
    }
    if prec >= 0 {
        return format!("{:.*}", prec as usize, v);
    }
    let mut p = 0;
    loop {
        let text = c_exp(v, p);
        let parsed: f64 = text.parse().unwrap_or(f64::NAN);
        let same = if single { (parsed as f32) == (v as f32) } else { parsed == v };
        if same || p >= 17 {
            return text;
        }
        p += 1;
    }
}

/// C's `%.{p}e`: Rust's `{:.p$e}` with the exponent written as a sign and at least two digits.
fn c_exp(v: f64, p: usize) -> String {
    let text = format!("{v:.p$e}");
    let (mantissa, exp) = text.split_once('e').unwrap_or((&text, "0"));
    let (sign, digits) = match exp.strip_prefix('-') {
        Some(d) => ('-', d),
        None => ('+', exp),
    };
    format!("{mantissa}e{sign}{digits:0>2}")
}

/// `i32.parse(s)` and the other integer types: the runtime's decimal parser, which reads into 64 bits.
fn parse_int(s: &[u8], ty: IntTy) -> Option<i128> {
    if s.is_empty() {
        return None;
    }
    let neg = ty.signed() && s[0] == b'-';
    let digits = if neg { &s[1..] } else { s };
    if digits.is_empty() {
        return None;
    }
    let limit: i128 = if !ty.signed() {
        i128::from(u64::MAX)
    } else if neg {
        1i128 << 63
    } else {
        i128::from(i64::MAX)
    };
    let mut n: i128 = 0;
    for &b in digits {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n * 10 + i128::from(b - b'0');
        if n > limit {
            return None;
        }
    }
    let n = if neg { -n } else { n };
    let wide = if ty.signed() { wrap(n, IntTy::I64) } else { wrap(n, IntTy::U64) };
    Some(wrap(wide, ty))
}

/// The character that starts at byte `at`, and its width.
fn decode(bytes: &[u8], at: usize) -> Option<(char, usize)> {
    let rest = bytes.get(at..)?;
    let first = *rest.first()?;
    let width = match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    };
    let end = width.min(rest.len());
    let c = std::str::from_utf8(&rest[..end]).ok().and_then(|s| s.chars().next()).unwrap_or('\u{fffd}');
    Some((c, width.max(1)))
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

impl<'p> Interp<'p, '_> {
    /// Runs `f` on the value of `e` without copying it when `e` is a place (a read through a view of it counts
    /// as the place itself).
    fn peek<T>(&mut self, e: &'p Expr, f: impl FnOnce(&Value) -> T) -> R<T> {
        let mut e = e;
        while let K::StringAsStr(inner) | K::Ref { place: inner, mutable: false } = &e.kind {
            e = inner;
        }
        if Self::is_place_expr(e) {
            let p = self.eval_place(e)?;
            let v = self.place_mut(&p, e.span)?;
            return Ok(f(shallow(v)));
        }
        let v = self.eval(e, Mode::Use)?;
        let out = f(shallow(&v));
        self.drop_value(v, &e.ty, e.span)?;
        Ok(out)
    }

    pub(crate) fn is_place_expr(e: &Expr) -> bool {
        matches!(e.kind, K::Local(_) | K::Field(..) | K::Index(..) | K::Deref(_) | K::DerefRef(_))
    }

    pub(crate) fn builtin(&mut self, b: Builtin, args: &'p [Expr], ty: &Ty, span: Span) -> R<Value> {
        Ok(match b {
            Builtin::Print | Builtin::Println => return Err(impure("printing", span)),
            Builtin::Eprint | Builtin::Eprintln => return Err(impure("printing to standard error", span)),
            Builtin::StdoutWrite | Builtin::StdoutWriteLine | Builtin::StdoutWriteBytes => {
                return Err(impure("writing to standard output", span));
            }
            Builtin::StdoutFlush => return Err(impure("flushing standard output", span)),
            Builtin::StderrWrite | Builtin::StderrWriteLine | Builtin::StderrWriteBytes => {
                return Err(impure("writing to standard error", span));
            }
            Builtin::IsTerminal => return Err(impure("asking whether a stream is a terminal", span)),
            Builtin::StdinReadLine
            | Builtin::StdinReadToString
            | Builtin::StdinAvail
            | Builtin::StdinNextByte
            | Builtin::StdinError => {
                return Err(impure("reading standard input", span));
            }
            Builtin::ReadToString => return Err(impure("reading a file", span)),
            Builtin::WriteFile => return Err(impure("writing a file", span)),
            Builtin::FileOpen => return Err(impure("opening a file", span)),
            Builtin::FileRead => return Err(impure("reading a file", span)),
            Builtin::FileWrite => return Err(impure("writing a file", span)),
            Builtin::FileSeek => return Err(impure("seeking in a file", span)),
            Builtin::FileSync => return Err(impure("syncing a file", span)),
            Builtin::FileSetLen => return Err(impure("resizing a file", span)),
            Builtin::FileClose => return Err(impure("closing a file", span)),
            Builtin::FileError => return Err(impure("reading a file error", span)),
            Builtin::ReadDir => return Err(impure("reading a directory", span)),
            Builtin::CreateDir => return Err(impure("creating a directory", span)),
            Builtin::RemoveDir => return Err(impure("removing a directory", span)),
            Builtin::RemoveFile => return Err(impure("removing a file", span)),
            Builtin::RenamePath => return Err(impure("renaming a path", span)),
            Builtin::Metadata => return Err(impure("reading file metadata", span)),
            Builtin::Canonicalize => return Err(impure("resolving a path", span)),
            Builtin::Clock => return Err(impure("reading the clock", span)),
            Builtin::Entropy => return Err(impure("reading system entropy", span)),
            Builtin::Sleep => return Err(impure("sleeping", span)),
            Builtin::EnvArgs => return Err(impure("reading the process arguments", span)),
            Builtin::EnvList => return Err(impure("reading the environment", span)),
            Builtin::EnvGet => return Err(impure("reading an environment variable", span)),
            Builtin::EnvSet | Builtin::EnvSetBytes => {
                return Err(impure("setting an environment variable", span));
            }
            Builtin::EnvRemove => return Err(impure("removing an environment variable", span)),
            Builtin::CurrentDir => return Err(impure("reading the working directory", span)),
            Builtin::SetCurrentDir | Builtin::SetCurrentDirBytes => {
                return Err(impure("changing the working directory", span));
            }
            Builtin::ProcessExit => return Err(impure("ending the process", span)),
            Builtin::RunCommand => return Err(impure("running a command", span)),
            Builtin::ProcessSpawn => return Err(impure("starting a process", span)),
            Builtin::ProcessWait => return Err(impure("waiting for a process", span)),
            Builtin::ProcessClose => return Err(impure("closing a process", span)),
            Builtin::PipeReadToString | Builtin::PipeReadLine | Builtin::PipeReadBoth => {
                return Err(impure("reading a pipe", span));
            }
            Builtin::PipeWrite => return Err(impure("writing a pipe", span)),
            Builtin::PipeClose => return Err(impure("closing a pipe", span)),
            Builtin::CPtrNull => return Err(impure("making a C pointer", span)),
            Builtin::CStringFrom => return Err(impure("allocating a C string", span)),
            Builtin::CStrToString => return Err(impure("reading a C string", span)),
            Builtin::Panic => {
                let msg = self.peek(&args[0], |v| String::from_utf8_lossy(v.bytes()).into_owned())?;
                return Err(panic(msg, span));
            }
            Builtin::Unreachable => return Err(panic("unreachable", span)),
            Builtin::Assert => {
                if !self.eval(&args[0], Mode::Use)?.bool() {
                    return Err(panic("assertion failed", span));
                }
                Value::Unit
            }
            Builtin::AssertEq => {
                let a = self.eval(&args[0], Mode::Use)?;
                let b = self.eval(&args[1], Mode::Use)?;
                if !value_eq(&a, &b) {
                    return Err(panic("assertion failed: the two values differ", span));
                }
                Value::Unit
            }
            Builtin::ComptimeAssert => {
                if !self.eval(&args[0], Mode::Use)?.bool() {
                    let msg = match &args[1].kind {
                        K::Str(s) => s.clone(),
                        _ => String::new(),
                    };
                    return Err(Fault::Assert(msg, span).into());
                }
                Value::Unit
            }
            Builtin::TargetOs => Value::str(tir::target_os()),
            Builtin::TargetArch => Value::str(tir::target_arch()),
            // The portable kind of an OS error code (`spec/errors.md`, ERR11). It is pure, so it evaluates at
            // compile time like the target queries above.
            Builtin::IoErrorKind => {
                let code = self.eval(&args[0], Mode::Use)?.int() as i32;
                let Ty::Enum(a) = ty else { unreachable!("`__io_kind` returns IoErrorKind") };
                let variants = &self.p.enums[a.id].variants;
                let name = tir::ioerror::kind_name(tir::ioerror::host_platform(), code);
                let index = variants
                    .iter()
                    .position(|v| v.name == name)
                    .or_else(|| variants.iter().position(|v| v.name == "Other"))
                    .unwrap_or(0);
                Value::Variant(index, Vec::new())
            }
            Builtin::StringNew => Value::String(Vec::new()),
            Builtin::StringFrom => {
                let bytes = self.peek(&args[0], |v| v.bytes().to_vec())?;
                self.charge(bytes.len() as u64, span)?;
                Value::String(bytes)
            }
            Builtin::FloatStr => {
                let v = self.eval(&args[0], Mode::Use)?.float();
                let prec = self.eval(&args[1], Mode::Use)?.int();
                let single = self.eval(&args[2], Mode::Use)?.bool();
                let text = float_str(v, prec, single);
                self.charge(text.len() as u64, span)?;
                Value::String(text.into_bytes())
            }
            Builtin::FloatFromBits(ft) => {
                let bits = self.eval(&args[0], Mode::Use)?.int();
                let v = match ft {
                    FloatTy::F32 => f64::from(f32::from_bits(bits as u32)),
                    FloatTy::F64 => f64::from_bits(bits as u64),
                };
                Value::Float(v)
            }
            Builtin::HeapNew => Value::Heap(Box::new(self.eval(&args[0], Mode::Move)?)),
            Builtin::SharedNew => Value::Heap(Box::new(self.eval(&args[0], Mode::Move)?)),
            // Allocation is a run-time operation: the interpreter models containers abstractly and has no
            // allocator (`spec/allocators.md`, AL10).
            Builtin::RawAlloc
            | Builtin::RawRealloc
            | Builtin::RawDealloc
            | Builtin::ListNewIn
            | Builtin::ListWithCapacityIn
            | Builtin::MapNewIn
            | Builtin::HeapNewIn => {
                return Err(impure("allocating memory with an explicit allocator", span));
            }
            Builtin::ListNew | Builtin::ListWithCapacity => Value::List(Vec::new()),
            Builtin::MapNew => Value::Map(Vec::new()),
            // Capacity is not modeled at compile time; the argument is still evaluated.
            Builtin::MapWithCapacity => {
                let _ = self.eval(&args[0], Mode::Use)?;
                Value::Map(Vec::new())
            }
            Builtin::IntParse(int_ty) => {
                let parsed = self.peek(&args[0], |v| parse_int(v.bytes(), int_ty))?;
                match parsed {
                    Some(n) => Value::Res(Ok(Box::new(Value::Int(n)))),
                    None => {
                        let err = match ty {
                            Ty::Result(_, e) => zero_value(self.p, e),
                            _ => Value::Unit,
                        };
                        Value::Res(Err(Box::new(err)))
                    }
                }
            }
            Builtin::Swap => {
                let a = self.eval_place(&args[0])?;
                let b = self.eval_place(&args[1])?;
                let va = std::mem::replace(self.place_mut(&a, span)?, Value::Moved);
                let vb = std::mem::replace(self.place_mut(&b, span)?, va);
                *self.place_mut(&a, span)? = vb;
                Value::Unit
            }
            Builtin::Replace => {
                let place = self.eval_place(&args[0])?;
                let v = self.eval(&args[1], Mode::Move)?;
                std::mem::replace(self.place_mut(&place, span)?, v)
            }
        })
    }

    pub(crate) fn method(&mut self, m: Method, args: &'p [Expr], ty: &Ty, span: Span) -> R<Value> {
        let recv = &args[0];
        Ok(match m {
            Method::AsPtr => return Err(impure("taking the address of a C string", span)),
            Method::IsNull => return Err(impure("inspecting a C pointer", span)),
            Method::ToString => {
                let recv_ty = recv.ty.clone();
                let text: Vec<u8> = self.peek(recv, |v| match (&recv_ty, v) {
                    (Ty::Bool, v) => {
                        if v.bool() {
                            b"true".to_vec()
                        } else {
                            b"false".to_vec()
                        }
                    }
                    (Ty::Char, v) => utf8(v.char()),
                    (Ty::Int(_), v) => v.int().to_string().into_bytes(),
                    (_, v) => v.bytes().to_vec(),
                })?;
                self.charge(text.len() as u64, span)?;
                Value::String(text)
            }
            Method::WrappingAdd | Method::WrappingSub | Method::WrappingMul => {
                let i = int_of(&recv.ty, span)?;
                let x = self.eval(recv, Mode::Use)?.int();
                let y = self.eval(&args[1], Mode::Use)?.int();
                Value::Int(wrap(exact(m, x, y), i))
            }
            Method::CheckedAdd | Method::CheckedSub | Method::CheckedMul => {
                let i = int_of(&recv.ty, span)?;
                let x = self.eval(recv, Mode::Use)?.int();
                let y = self.eval(&args[1], Mode::Use)?.int();
                let r = exact_checked(m, x, y);
                match r {
                    Some(r) if r >= i.min() && r <= i.max() => Value::Opt(Some(Box::new(Value::Int(r)))),
                    _ => Value::Opt(None),
                }
            }
            Method::Min | Method::Max => {
                let x = self.eval(recv, Mode::Use)?.int();
                let y = self.eval(&args[1], Mode::Use)?.int();
                Value::Int(if m == Method::Min { x.min(y) } else { x.max(y) })
            }
            Method::Abs => {
                let i = int_of(&recv.ty, span)?;
                let x = self.eval(recv, Mode::Use)?.int();
                Value::Int(self.negate_if(x < 0, x, i, span)?)
            }
            Method::PushStr => {
                let place = self.eval_place(recv)?;
                let bytes = self.peek(&args[1], |v| v.bytes().to_vec())?;
                self.charge(bytes.len() as u64, span)?;
                match self.place_mut(&place, span)? {
                    Value::String(s) => s.extend_from_slice(&bytes),
                    other => return Err(internal(format!("`push_str` on {other:?}"), span)),
                }
                Value::Unit
            }
            Method::PushChar => {
                let place = self.eval_place(recv)?;
                let c = self.eval(&args[1], Mode::Use)?.char();
                self.charge(1, span)?;
                match self.place_mut(&place, span)? {
                    Value::String(s) => s.extend_from_slice(&utf8(c)),
                    other => return Err(internal(format!("`push` on {other:?}"), span)),
                }
                Value::Unit
            }
            Method::Push => {
                let place = self.eval_place(recv)?;
                let v = self.eval(&args[1], Mode::Move)?;
                self.charge(1, span)?;
                list_mut(self.place_mut(&place, span)?, span)?.push(v);
                Value::Unit
            }
            Method::ListPop => {
                let place = self.eval_place(recv)?;
                Value::Opt(list_mut(self.place_mut(&place, span)?, span)?.pop().map(Box::new))
            }
            Method::ListLast => {
                let last = self.peek(recv, |v| v.elems().last().cloned())?;
                Value::Opt(last.map(|v| Box::new(Value::Borrow(Box::new(v)))))
            }
            Method::ListFirst => {
                let first = self.peek(recv, |v| v.elems().first().cloned())?;
                Value::Opt(first.map(|v| Box::new(Value::Borrow(Box::new(v)))))
            }
            Method::ListGet => {
                let index = self.eval(&args[1], Mode::Use)?.int();
                let found = self.peek(recv, |v| usize::try_from(index).ok().and_then(|i| v.elems().get(i).cloned()))?;
                Value::Opt(found.map(|v| Box::new(Value::Borrow(Box::new(v)))))
            }
            Method::ListGetMut | Method::ListFirstMut | Method::ListLastMut => {
                return Err(impure("a mutable view of a list element", span));
            }
            // The interpreter models a list by its elements: there is no capacity to change.
            Method::ListCapacity => Value::Int(self.peek(recv, |v| v.elems().len())? as i128),
            Method::ListIsEmpty => Value::Bool(self.peek(recv, |v| v.elems().is_empty())?),
            Method::ListReserve | Method::ListReserveExact => {
                let _ = self.eval(&args[1], Mode::Use)?.int();
                Value::Unit
            }
            Method::ListShrinkToFit => Value::Unit,
            Method::ListSwap => {
                let a = self.eval(&args[1], Mode::Use)?.int();
                let b = self.eval(&args[2], Mode::Use)?.int();
                let place = self.eval_place(recv)?;
                let len = self.peek(recv, |v| v.elems().len())?;
                let (i, j) = (usize::try_from(a).unwrap_or(usize::MAX), usize::try_from(b).unwrap_or(usize::MAX));
                if i >= len || j >= len {
                    return Err(bounds(a.max(b), len, span));
                }
                self.charge(1, span)?;
                let list = list_mut(self.place_mut(&place, span)?, span)?;
                list.swap(i, j);
                Value::Unit
            }
            Method::ListSwapRemove => {
                let index = self.eval(&args[1], Mode::Use)?.int();
                let place = self.eval_place(recv)?;
                let len = self.peek(recv, |v| v.elems().len())?;
                let i = usize::try_from(index).unwrap_or(usize::MAX);
                if i >= len {
                    return Err(bounds(index, len, span));
                }
                self.charge(1, span)?;
                let list = list_mut(self.place_mut(&place, span)?, span)?;
                list.swap_remove(i)
            }
            Method::ListTruncate => {
                let index = self.eval(&args[1], Mode::Use)?.int();
                let elem = match &recv.ty {
                    Ty::List(t, _) => (**t).clone(),
                    _ => Ty::Unit,
                };
                let place = self.eval_place(recv)?;
                let removed = {
                    let list = list_mut(self.place_mut(&place, span)?, span)?;
                    let i = usize::try_from(index).unwrap_or(usize::MAX);
                    if i >= list.len() { Vec::new() } else { list.split_off(i) }
                };
                for v in removed {
                    self.charge(1, span)?;
                    self.drop_value(v, &elem, span)?;
                }
                Value::Unit
            }
            Method::ListSplitOff => {
                let index = self.eval(&args[1], Mode::Use)?.int();
                let place = self.eval_place(recv)?;
                let len = self.peek(recv, |v| v.elems().len())?;
                let i = usize::try_from(index).unwrap_or(usize::MAX);
                if i > len {
                    return Err(bounds(index, len, span));
                }
                self.charge((len - i) as u64, span)?;
                let list = list_mut(self.place_mut(&place, span)?, span)?;
                Value::List(list.split_off(i))
            }
            Method::ListAppend => {
                let place = self.eval_place(recv)?;
                let other = self.eval(&args[1], Mode::Move)?.deref();
                let items = match other {
                    Value::List(items) => items,
                    Value::Moved => Vec::new(),
                    other => return Err(internal(format!("`append` of {other:?}"), span)),
                };
                self.charge(items.len() as u64, span)?;
                let list = list_mut(self.place_mut(&place, span)?, span)?;
                list.extend(items);
                Value::Unit
            }
            Method::ListDrain => {
                let place = self.eval_place(recv)?;
                let range = self.eval(&args[1], Mode::Use)?;
                let (lo, hi) = range_bounds(&range);
                let inclusive = matches!(args[1].ty, Ty::Range(_, true));
                let len = self.peek(recv, |v| v.elems().len())?;
                let lo = usize::try_from(lo).unwrap_or(usize::MAX);
                let hi = usize::try_from(hi).unwrap_or(usize::MAX);
                let end = if inclusive {
                    if hi >= len {
                        return Err(bounds(hi as i128, len, span));
                    }
                    hi + 1
                } else {
                    hi
                };
                if end > len || lo > end {
                    return Err(bounds(end as i128, len, span));
                }
                self.charge((end - lo) as u64, span)?;
                let list = list_mut(self.place_mut(&place, span)?, span)?;
                let removed: Vec<Value> = list.splice(lo..end, std::iter::empty()).collect();
                Value::Items(VecDeque::from(removed))
            }
            Method::ListExtendFromSlice => {
                let place = self.eval_place(recv)?;
                let items = self.peek(&args[1], |v| v.elems().to_vec())?;
                self.charge(items.len() as u64, span)?;
                let list = list_mut(self.place_mut(&place, span)?, span)?;
                list.extend(items);
                Value::Unit
            }
            Method::ListInsert => {
                let place = self.eval_place(recv)?;
                let index = self.eval(&args[1], Mode::Use)?.int();
                let v = self.eval(&args[2], Mode::Move)?;
                self.charge(1, span)?;
                let list = list_mut(self.place_mut(&place, span)?, span)?;
                let len = list.len();
                let i = usize::try_from(index).unwrap_or(usize::MAX);
                if i > len {
                    return Err(bounds(index, len, span));
                }
                list.insert(i, v);
                Value::Unit
            }
            Method::ListRemove => {
                let place = self.eval_place(recv)?;
                let index = self.eval(&args[1], Mode::Use)?.int();
                let list = list_mut(self.place_mut(&place, span)?, span)?;
                let len = list.len();
                let i = usize::try_from(index).unwrap_or(usize::MAX);
                if i >= len {
                    return Err(bounds(index, len, span));
                }
                list.remove(i)
            }
            Method::ListContains => {
                let needle = self.eval(&args[1], Mode::Use)?;
                let n = self.peek(recv, |v| v.elems().len())?;
                self.charge(n as u64, span)?;
                Value::Bool(self.peek(recv, |v| v.elems().iter().any(|x| value_eq(x, &needle)))?)
            }
            Method::ListSort => {
                let place = self.eval_place(recv)?;
                let list = list_mut(self.place_mut(&place, span)?, span)?;
                list.sort_by(value_cmp);
                let n = list.len() as u64;
                self.charge(n, span)?;
                Value::Unit
            }
            Method::Clear => {
                let place = self.eval_place(recv)?;
                let old = match self.place_mut(&place, span)? {
                    Value::List(xs) => Value::List(std::mem::take(xs)),
                    Value::Map(xs) => Value::Map(std::mem::take(xs)),
                    other => return Err(internal(format!("`clear` on {other:?}"), span)),
                };
                let recv_ty = recv.ty.clone();
                self.drop_value(old, &recv_ty, span)?;
                Value::Unit
            }
            Method::MapInsert => {
                let place = self.eval_place(recv)?;
                let k = self.eval(&args[1], Mode::Move)?;
                let v = self.eval(&args[2], Mode::Move)?;
                self.charge(1, span)?;
                let (kt, vt) = match &recv.ty {
                    Ty::Map(k, v, _) => ((**k).clone(), (**v).clone()),
                    _ => (Ty::Unit, Ty::Unit),
                };
                let map = map_mut(self.place_mut(&place, span)?, span)?;
                match map.iter_mut().find(|(key, _)| value_eq(key, &k)) {
                    Some((_, slot)) => {
                        let old = std::mem::replace(slot, v);
                        self.drop_value(old, &vt, span)?;
                        self.drop_value(k, &kt, span)?;
                    }
                    None => map.push((k, v)),
                }
                Value::Unit
            }
            Method::MapGet => {
                let k = self.eval(&args[1], Mode::Use)?;
                let found = self.peek(recv, |v| match v {
                    Value::Map(pairs) => pairs.iter().find(|(key, _)| value_eq(key, &k)).map(|(_, v)| v.clone()),
                    _ => None,
                })?;
                Value::Opt(found.map(|v| Box::new(Value::Borrow(Box::new(v)))))
            }
            Method::MapGetMut | Method::MapGetOrInsert | Method::MapGetOr => {
                return Err(impure("a mutable view of a map value", span));
            }
            Method::IterMut => return Err(impure("a mutable iterator", span)),
            Method::MapRetain => return Err(impure("a callback over a map value", span)),
            // Capacity is not modeled at compile time: the map is a list of pairs, so reserving and shrinking
            // change nothing observable.
            Method::MapReserve | Method::MapShrinkToFit => Value::Unit,
            Method::MapEntry | Method::MapEntryOrInsert | Method::MapEntryOrInsertWith => {
                return Err(impure("an entry into a map value", span));
            }
            Method::MapContainsKey => {
                let k = self.eval(&args[1], Mode::Use)?;
                Value::Bool(self.peek(recv, |v| match v {
                    Value::Map(pairs) => pairs.iter().any(|(key, _)| value_eq(key, &k)),
                    _ => false,
                })?)
            }
            Method::MapRemove => {
                let place = self.eval_place(recv)?;
                let k = self.eval(&args[1], Mode::Use)?;
                let map = map_mut(self.place_mut(&place, span)?, span)?;
                match map.iter().position(|(key, _)| value_eq(key, &k)) {
                    Some(i) => {
                        let (key, v) = map.remove(i);
                        if let Ty::Map(kt, _, _) = &recv.ty {
                            let kt = (**kt).clone();
                            self.drop_value(key, &kt, span)?;
                        }
                        Value::Opt(Some(Box::new(v)))
                    }
                    None => Value::Opt(None),
                }
            }
            Method::MapRemoveEntry => {
                let place = self.eval_place(recv)?;
                let k = self.eval(&args[1], Mode::Use)?;
                let map = map_mut(self.place_mut(&place, span)?, span)?;
                match map.iter().position(|(key, _)| value_eq(key, &k)) {
                    Some(i) => {
                        let (key, v) = map.remove(i);
                        Value::Opt(Some(Box::new(Value::Tuple(vec![key, v]))))
                    }
                    None => Value::Opt(None),
                }
            }
            Method::Unwrap | Method::Expect => {
                let v = self.eval(recv, Mode::Move)?.deref();
                let message = if m == Method::Expect {
                    Some(self.peek(&args[1], |v| String::from_utf8_lossy(v.bytes()).into_owned())?)
                } else {
                    None
                };
                match v {
                    Value::Opt(Some(x)) | Value::Res(Ok(x)) => *x,
                    Value::Opt(None) => {
                        return Err(panic(message.unwrap_or_else(|| "called `unwrap` on a `None` value".into()), span));
                    }
                    Value::Res(Err(_)) => {
                        return Err(panic(message.unwrap_or_else(|| "called `unwrap` on a `Err` value".into()), span));
                    }
                    other => return Err(internal(format!("`unwrap` on {other:?}"), span)),
                }
            }
            Method::IsSome | Method::IsNone | Method::IsOk | Method::IsErr => {
                let yes = self.peek(recv, |v| matches!(v, Value::Opt(Some(_)) | Value::Res(Ok(_))))?;
                Value::Bool(if matches!(m, Method::IsSome | Method::IsOk) { yes } else { !yes })
            }
            Method::FindStr => {
                let needle = self.peek(&args[1], |v| v.bytes().to_vec())?;
                let found = self.peek(recv, |v| find_bytes(v.bytes(), &needle))?;
                Value::Opt(found.map(|i| Box::new(Value::Int(i as i128))))
            }
            Method::Find => {
                let c = self.eval(&args[1], Mode::Use)?.char();
                let found = self.peek(recv, |v| find_bytes(v.bytes(), &utf8(c)))?;
                Value::Opt(found.map(|i| Box::new(Value::Int(i as i128))))
            }
            Method::StartsWith => {
                let prefix = self.peek(&args[1], |v| v.bytes().to_vec())?;
                Value::Bool(self.peek(recv, |v| v.bytes().starts_with(&prefix))?)
            }
            Method::Chars => Value::Chars(self.peek(recv, |v| Rc::from(v.bytes()))?, 0),
            Method::AsStr => Value::Str(self.peek(recv, |v| Rc::from(v.bytes()))?),
            Method::Len => {
                let n = self.peek(recv, |v| match v {
                    Value::Str(b) => b.len(),
                    Value::String(b) => b.len(),
                    Value::Map(pairs) => pairs.len(),
                    other => other.elems().len(),
                })?;
                Value::Int(n as i128)
            }
            Method::Clone => self.peek(recv, Value::clone)?,
            Method::Hash => {
                let recv_ty = recv.ty.clone();
                let p = self.p;
                Value::Int(i128::from(self.peek(recv, |v| hash(p, v, &recv_ty, FNV_OFFSET))?))
            }
            Method::Iter | Method::IntoIter => self.iter_value(recv, m == Method::IntoIter, span)?,
            Method::IterNext => self.iter_next(recv, ty, span)?,
            Method::IterNextBack => self.iter_next_back(recv, ty, span)?,
            Method::SharedGet => {
                let inner = self.peek(recv, |v| match v {
                    Value::Heap(b) => (**b).clone(),
                    other => other.clone(),
                })?;
                Value::Borrow(Box::new(inner))
            }
            Method::SharedStrongCount => Value::Int(1),
            Method::Float(op) => self.float_method(op, args, span)?,
        })
    }

    /// A floating-point method the compiler provides (`spec/stdlib.md`, "Float functions"), computed in the
    /// receiver's precision so an `f32` result is the one compiled code produces.
    fn float_method(&mut self, op: FloatOp, args: &'p [Expr], span: Span) -> R<Value> {
        let ft = match &args[0].ty {
            Ty::Float(f) => *f,
            other => return Err(internal(format!("a floating-point method on `{other}`"), span)),
        };
        let x = self.eval(&args[0], Mode::Use)?.float();
        if op == FloatOp::ToBits {
            return Ok(Value::Int(match ft {
                FloatTy::F32 => i128::from((x as f32).to_bits()),
                FloatTy::F64 => i128::from(x.to_bits()),
            }));
        }
        let mut rest = Vec::with_capacity(args.len() - 1);
        for a in &args[1..] {
            rest.push(self.eval(a, Mode::Use)?);
        }
        Ok(Value::Float(match ft {
            FloatTy::F32 => f64::from(float_op32(op, x as f32, &rest)),
            FloatTy::F64 => float_op64(op, x, &rest),
        }))
    }

    fn iter_value(&mut self, recv: &'p Expr, into: bool, span: Span) -> R<Value> {
        let v = self.eval(recv, if into { Mode::Move } else { Mode::Use })?.deref();
        Ok(match (&recv.ty, v) {
            (Ty::Slice(_) | Ty::Chars, v) => v,
            (Ty::Array(..) | Ty::List(_, _), Value::Array(xs) | Value::List(xs)) => {
                if into {
                    Value::Items(VecDeque::from(xs))
                } else {
                    let n = xs.len();
                    Value::Slice(Rc::new(xs), 0, n)
                }
            }
            (Ty::Map(..), Value::Map(pairs)) => Value::Items(
                pairs
                    .into_iter()
                    .map(|(k, v)| {
                        if into {
                            Value::Tuple(vec![k, v])
                        } else {
                            Value::Tuple(vec![Value::Borrow(Box::new(k)), Value::Borrow(Box::new(v))])
                        }
                    })
                    .collect(),
            ),
            (Ty::Range(..), v) => {
                let (lo, hi) = range_bounds(&v);
                Value::RangeIter(lo, hi, false)
            }
            (ty, v) => return Err(internal(format!("an iterator over `{ty}` from {v:?}"), span)),
        })
    }

    fn iter_next(&mut self, recv: &'p Expr, ty: &Ty, span: Span) -> R<Value> {
        let item_is_view = matches!(ty, Ty::Option(t) if matches!(**t, Ty::Ref { .. }));
        let inclusive = matches!(recv.ty, Ty::RangeIter(_, true));
        let place = self.eval_place(recv)?;
        let it = self.place_mut(&place, span)?;
        let it = match it {
            Value::Borrow(_) => return Err(internal("`next` on a shared iterator", span)),
            other => other,
        };
        let next = match it {
            Value::Slice(xs, lo, hi) => {
                if *lo >= *hi {
                    None
                } else {
                    let v = xs[*lo].clone();
                    *lo += 1;
                    Some(if item_is_view { Value::Borrow(Box::new(v)) } else { v })
                }
            }
            Value::Chars(bytes, at) => match decode(bytes, *at) {
                Some((c, w)) => {
                    *at += w;
                    Some(Value::Char(c))
                }
                None => None,
            },
            Value::Items(items) => items.pop_front(),
            Value::RangeIter(next, hi, done) => {
                if *done || (inclusive && *next > *hi) || (!inclusive && *next >= *hi) {
                    None
                } else {
                    let v = *next;
                    if inclusive && v == *hi {
                        *done = true;
                    } else {
                        *next += 1;
                    }
                    Some(Value::Int(v))
                }
            }
            other => return Err(internal(format!("`next` on {other:?}"), span)),
        };
        Ok(Value::Opt(next.map(Box::new)))
    }

    /// `it.next_back()` on a slice, a range iterator, or `Chars` (spec/iterators.md, IT14).
    fn iter_next_back(&mut self, recv: &'p Expr, ty: &Ty, span: Span) -> R<Value> {
        let item_is_view = matches!(ty, Ty::Option(t) if matches!(**t, Ty::Ref { .. }));
        let inclusive = matches!(recv.ty, Ty::RangeIter(_, true));
        let place = self.eval_place(recv)?;
        let it = self.place_mut(&place, span)?;
        let it = match it {
            Value::Borrow(_) => return Err(internal("`next_back` on a shared iterator", span)),
            other => other,
        };
        let next = match it {
            Value::Slice(xs, lo, hi) => {
                if *lo >= *hi {
                    None
                } else {
                    *hi -= 1;
                    let v = xs[*hi].clone();
                    Some(if item_is_view { Value::Borrow(Box::new(v)) } else { v })
                }
            }
            Value::Chars(bytes, at) => {
                let mut start = bytes.len();
                if start <= *at {
                    None
                } else {
                    start -= 1;
                    while start > *at && (bytes[start] & 0xc0) == 0x80 {
                        start -= 1;
                    }
                    let c = decode(bytes, start).map(|(c, _)| c);
                    let kept: Rc<[u8]> = Rc::from(&bytes[..start]);
                    *bytes = kept;
                    c.map(Value::Char)
                }
            }
            Value::RangeIter(lo, hi, done) => {
                if *done || (inclusive && *lo > *hi) || (!inclusive && *lo >= *hi) {
                    None
                } else if inclusive {
                    let v = *hi;
                    if *lo == *hi {
                        *done = true;
                    } else {
                        *hi -= 1;
                    }
                    Some(Value::Int(v))
                } else {
                    *hi -= 1;
                    Some(Value::Int(*hi))
                }
            }
            other => return Err(internal(format!("`next_back` on {other:?}"), span)),
        };
        Ok(Value::Opt(next.map(Box::new)))
    }

    pub(crate) fn binary(&mut self, op: BinOp, l: &'p Expr, r: &'p Expr, span: Span) -> R<Value> {
        if matches!(op, BinOp::And | BinOp::Or) {
            let a = self.eval(l, Mode::Use)?.bool();
            if (op == BinOp::And) != a {
                return Ok(Value::Bool(a));
            }
            return Ok(Value::Bool(self.eval(r, Mode::Use)?.bool()));
        }
        let ty = if l.ty == Ty::Never { r.ty.clone() } else { l.ty.clone() };
        let a = self.eval(l, Mode::Use)?.deref();
        let b = self.eval(r, Mode::Use)?.deref();
        let cmp = |o: Ordering| match op {
            BinOp::Eq => o == Ordering::Equal,
            BinOp::Ne => o != Ordering::Equal,
            BinOp::Lt => o == Ordering::Less,
            BinOp::Gt => o == Ordering::Greater,
            BinOp::Le => o != Ordering::Greater,
            _ => o != Ordering::Less,
        };
        if let Ty::Float(_) = ty {
            let (x, y) = (a.float(), b.float());
            return Ok(match op {
                BinOp::Add => Value::Float(round_float(x + y, &ty)),
                BinOp::Sub => Value::Float(round_float(x - y, &ty)),
                BinOp::Mul => Value::Float(round_float(x * y, &ty)),
                BinOp::Div => Value::Float(round_float(x / y, &ty)),
                BinOp::Rem => Value::Float(round_float(x % y, &ty)),
                BinOp::Eq => Value::Bool(x == y),
                BinOp::Ne => Value::Bool(x != y),
                BinOp::Lt => Value::Bool(x < y),
                BinOp::Gt => Value::Bool(x > y),
                BinOp::Le => Value::Bool(x <= y),
                BinOp::Ge => Value::Bool(x >= y),
                _ => return Err(internal("a bit operator on floats", span)),
            });
        }
        match op {
            BinOp::Eq => return Ok(Value::Bool(value_eq(&a, &b))),
            BinOp::Ne => return Ok(Value::Bool(!value_eq(&a, &b))),
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => return Ok(Value::Bool(cmp(value_cmp(&a, &b)))),
            _ => {}
        }
        if let (Ty::Bool, Value::Bool(x), Value::Bool(y)) = (&ty, &a, &b) {
            return Ok(Value::Bool(match op {
                BinOp::BitAnd => *x & *y,
                BinOp::BitOr => *x | *y,
                BinOp::BitXor => *x ^ *y,
                _ => return Err(internal("arithmetic on bools", span)),
            }));
        }
        let i = int_of(&ty, span)?;
        let (x, y) = (a.int(), b.int());
        Ok(Value::Int(match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                let m = match op {
                    BinOp::Add => Method::WrappingAdd,
                    BinOp::Sub => Method::WrappingSub,
                    _ => Method::WrappingMul,
                };
                match exact_checked(m, x, y) {
                    Some(v) if v >= i.min() && v <= i.max() => v,
                    _ if self.release => wrap(exact(m, x, y), i),
                    _ => {
                        let verb = match op {
                            BinOp::Add => "add",
                            BinOp::Sub => "subtract",
                            _ => "multiply",
                        };
                        return Err(panic(format!("attempt to {verb} with overflow"), span));
                    }
                }
            }
            BinOp::Div | BinOp::Rem => {
                if y == 0 {
                    return Err(panic("division by zero", span));
                }
                if i.signed() && x == i.min() && y == -1 {
                    if !self.release {
                        let what = if op == BinOp::Div { "divide" } else { "calculate the remainder" };
                        return Err(panic(format!("attempt to {what} with overflow"), span));
                    }
                    if op == BinOp::Div { x } else { 0 }
                } else if op == BinOp::Div {
                    x / y
                } else {
                    x % y
                }
            }
            BinOp::Shl | BinOp::Shr => {
                if y < 0 || y >= i128::from(i.bits()) {
                    return Err(panic("attempt to shift with overflow", span));
                }
                if op == BinOp::Shl { wrap(x << y, i) } else { x >> y }
            }
            BinOp::BitAnd => wrap(x & y, i),
            BinOp::BitOr => wrap(x | y, i),
            BinOp::BitXor => wrap(x ^ y, i),
            _ => return Err(internal(format!("operator {op:?} on `{ty}`"), span)),
        }))
    }

    /// `-x` when `negate`, with the overflow rule of the profile.
    fn negate_if(&self, negate: bool, x: i128, i: IntTy, span: Span) -> R<i128> {
        if !negate {
            return Ok(x);
        }
        let r = -x;
        if r < i.min() || r > i.max() {
            if self.release {
                return Ok(wrap(r, i));
            }
            return Err(panic("attempt to negate with overflow", span));
        }
        Ok(r)
    }

    pub(crate) fn unary(&mut self, op: UnOp, v: Value, ty: &Ty, span: Span) -> R<Value> {
        let v = v.deref();
        Ok(match (op, ty) {
            (UnOp::Not, Ty::Bool) => Value::Bool(!v.bool()),
            (UnOp::Not, Ty::Int(i)) => Value::Int(wrap(!v.int(), *i)),
            (UnOp::Neg, Ty::Float(_)) => Value::Float(-v.float()),
            (UnOp::Neg, Ty::Int(i)) => Value::Int(self.negate_if(true, v.int(), *i, span)?),
            _ => return Err(internal(format!("operator {op:?} on `{ty}`"), span)),
        })
    }

    pub(crate) fn cast(&mut self, v: Value, from: &Ty, to: &Ty, span: Span) -> R<Value> {
        let v = v.deref();
        if from == to {
            return Ok(v);
        }
        Ok(match (from, to) {
            (Ty::Int(_) | Ty::Bool | Ty::Char, Ty::Int(i)) => Value::Int(wrap(v.int(), *i)),
            (Ty::Int(_) | Ty::Bool | Ty::Char, Ty::Char) => {
                let n = wrap(v.int(), IntTy::U32);
                Value::Char(char::from_u32(n as u32).unwrap_or('\u{fffd}'))
            }
            (Ty::Int(_), Ty::Float(_)) => Value::Float(round_float(v.int() as f64, to)),
            (Ty::Float(_), Ty::Int(i)) => {
                let x = v.float();
                Value::Int(if x.is_nan() { 0 } else { (x as i128).clamp(i.min(), i.max()) })
            }
            (Ty::Float(_), Ty::Float(_)) => Value::Float(round_float(v.float(), to)),
            (Ty::CPtr(_), _) | (_, Ty::CPtr(_)) => return Err(impure("converting a C pointer", span)),
            _ => return Err(internal(format!("a cast from `{from}` to `{to}`"), span)),
        })
    }

    /// `base[lo..hi]` and `base[lo..=hi]`, with the bounds checks of compiled code.
    pub(crate) fn slice(&mut self, base: Value, lo: i128, hi: i128, inclusive: bool, span: Span) -> R<Value> {
        let len = match &base {
            Value::Str(b) => b.len(),
            Value::String(b) => b.len(),
            other => other.elems().len(),
        };
        let as_index = |x: i128| usize::try_from(x).unwrap_or(usize::MAX);
        let (lo_u, mut hi_u) = (as_index(lo), as_index(hi));
        if inclusive {
            if hi_u >= len {
                return Err(bounds(hi, len, span));
            }
            if lo_u > hi_u {
                return Err(bounds(lo, hi_u, span));
            }
            hi_u += 1;
        } else if hi_u > len {
            return Err(bounds(hi, len, span));
        }
        if lo_u > hi_u {
            return Err(bounds(lo, hi_u, span));
        }
        Ok(match base {
            Value::Str(b) => Value::Str(Rc::from(&b[lo_u..hi_u])),
            Value::String(b) => Value::Str(Rc::from(&b[lo_u..hi_u])),
            Value::Slice(xs, start, _) => Value::Slice(xs, start + lo_u, start + hi_u),
            Value::Array(xs) | Value::List(xs) => Value::Slice(Rc::new(xs), lo_u, hi_u),
            other => return Err(internal(format!("slicing {other:?}"), span)),
        })
    }
}

fn int_of(ty: &Ty, span: Span) -> R<IntTy> {
    match ty {
        Ty::Int(i) => Ok(*i),
        other => Err(internal(format!("integer arithmetic on `{other}`"), span)),
    }
}

fn exact(m: Method, x: i128, y: i128) -> i128 {
    match m {
        Method::WrappingAdd | Method::CheckedAdd => x.wrapping_add(y),
        Method::WrappingSub | Method::CheckedSub => x.wrapping_sub(y),
        _ => (x as u128).wrapping_mul(y as u128) as i128,
    }
}

fn exact_checked(m: Method, x: i128, y: i128) -> Option<i128> {
    match m {
        Method::WrappingAdd | Method::CheckedAdd => x.checked_add(y),
        Method::WrappingSub | Method::CheckedSub => x.checked_sub(y),
        _ => x.checked_mul(y),
    }
}

pub(crate) fn bounds(index: i128, len: usize, span: Span) -> crate::interp::Unwind {
    // The runtime prints the index as an unsigned 64-bit number.
    let index = index as u64;
    Fault::Panic(format!("index out of bounds: the index is {index} but the length is {len}"), span).into()
}

fn list_mut(v: &mut Value, span: Span) -> R<&mut Vec<Value>> {
    match v {
        Value::List(xs) => Ok(xs),
        other => Err(internal(format!("a list method on {other:?}"), span)),
    }
}

fn map_mut(v: &mut Value, span: Span) -> R<&mut Vec<(Value, Value)>> {
    match v {
        Value::Map(xs) => Ok(xs),
        other => Err(internal(format!("a map method on {other:?}"), span)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_follows_twos_complement() {
        assert_eq!(wrap(128, IntTy::I8), -128);
        assert_eq!(wrap(-1, IntTy::U8), 255);
        assert_eq!(wrap(256, IntTy::U8), 0);
        assert_eq!(wrap(i128::from(u64::MAX) + 1, IntTy::U64), 0);
    }

    #[test]
    fn float_text_matches_c() {
        assert_eq!(float_str(1.5, 2, false), "1.50");
        assert_eq!(float_str(0.1, -1, false), "1e-01");
        assert_eq!(float_str(123.0, -1, false), "1.23e+02");
        assert_eq!(float_str(0.1f32 as f64, -1, true), "1e-01");
        assert_eq!(float_str(f64::INFINITY, -1, false), "inf");
    }

    #[test]
    fn float_operations_follow_ieee_754() {
        // The exactly rounded operations, at the bit patterns the conformance tests pin.
        assert_eq!(float_op64(FloatOp::Sqrt, 2.0, &[]).to_bits(), 0x3ff6a09e667f3bcd);
        assert_eq!(float_op64(FloatOp::Abs, -0.0, &[]).to_bits(), 0);
        assert_eq!(float_op64(FloatOp::Floor, -1.5, &[]), -2.0);
        assert_eq!(float_op64(FloatOp::Ceil, 1.5, &[]), 2.0);
        assert_eq!(float_op64(FloatOp::Round, 2.5, &[]), 3.0);
        assert_eq!(float_op64(FloatOp::RoundTiesEven, 2.5, &[]), 2.0);
        assert_eq!(float_op64(FloatOp::Trunc, -1.9, &[]), -1.0);
        assert_eq!(float_op64(FloatOp::Powi, 2.0, &[Value::Int(10)]), 1024.0);
        let neg = Value::Float(-1.0);
        assert_eq!(float_op64(FloatOp::Copysign, 0.1, &[neg]).to_bits(), 0xbfb999999999999a);
        // A fused multiply-add rounds once: 0.1 * 10 - 1 is not zero, while the two-step form is.
        let args = [Value::Float(10.0), Value::Float(-1.0)];
        assert_eq!(float_op64(FloatOp::MulAdd, 0.1, &args).to_bits(), 0x3c90000000000000);
        // The same functions in f32 precision; computing in f64 first would round twice.
        assert_eq!(float_op32(FloatOp::Sqrt, 2.0, &[]).to_bits(), 0x3fb504f3);
        assert_eq!(float_op32(FloatOp::MulAdd, 0.1, &args).to_bits(), 0x32800000);
        assert_eq!(float_op32(FloatOp::Abs, -0.0, &[]).to_bits(), 0);
    }

    #[test]
    fn parsing_reads_64_bits_then_truncates() {
        assert_eq!(parse_int(b"42", IntTy::I32), Some(42));
        assert_eq!(parse_int(b"-7", IntTy::I64), Some(-7));
        assert_eq!(parse_int(b"-7", IntTy::U32), None);
        assert_eq!(parse_int(b"", IntTy::I32), None);
        assert_eq!(parse_int(b"300", IntTy::U8), Some(44));
        assert_eq!(parse_int(b"18446744073709551616", IntTy::U64), None);
    }

    #[test]
    fn layouts_follow_c() {
        assert_eq!(product(&[(1, 1), (8, 8)]), (16, 8));
        assert_eq!(product(&[(4, 4), (1, 1)]), (8, 4));
        assert_eq!(product(&[]), (0, 1));
    }

    #[test]
    fn strings_hash_byte_by_byte() {
        let p = Program {
            functions: vec![],
            closures: vec![],
            consts: vec![],
            structs: vec![],
            enums: vec![],
            signatures: vec![],
            externs: vec![],
            main: None,
            tests: vec![],
            any_ifaces: vec![],
            any_vtables: vec![],
            any_type_ids: vec![],
            build_inputs: vec![],
            allocator_fns: vec![],
            global_alloc_ty: None,
        };
        let h = hash(&p, &Value::str("a"), &Ty::Str, FNV_OFFSET);
        assert_eq!(h, (FNV_OFFSET ^ 0x61).wrapping_mul(FNV_PRIME));
    }
}
