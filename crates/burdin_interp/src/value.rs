//! Compile-time values and the places that hold them.
//!
//! A value is owned: moving it moves the Rust value, and a clone is a deep copy. A read-only view (`str`, a slice,
//! `&T`) is a copy of what it views; the memory checks guarantee that the viewed storage does not change while the
//! view is alive, so a copy reads the same bytes. A `&mut T` and a `mut` parameter are [`Place`]s instead, because
//! writes through them must reach the original storage.

use std::rc::Rc;

#[derive(Clone, Debug)]
pub enum Value {
    Unit,
    Bool(bool),
    /// Every integer type, kept in range for its type.
    Int(i128),
    /// Both float types; an `f32` value is always rounded to `f32` precision.
    Float(f64),
    Char(char),
    /// A `str` view: its bytes.
    Str(Rc<[u8]>),
    String(Vec<u8>),
    /// A tuple or struct: the fields in declaration order. A range is the tuple of its two endpoints.
    Tuple(Vec<Value>),
    /// An enum value: the variant index and its fields.
    Variant(usize, Vec<Value>),
    Array(Vec<Value>),
    /// A `[T]` view: the viewed elements, of which `lo..hi` are visible. Iterating it advances `lo`.
    Slice(Rc<Vec<Value>>, usize, usize),
    List(Vec<Value>),
    /// A `Map`, in insertion order (`spec/const-eval.md`, CE6).
    Map(Vec<(Value, Value)>),
    Heap(Box<Value>),
    /// `&mut T`, or a shared reference that had to keep its place.
    Ref(Place),
    /// A shared reference: a copy of the referent.
    Borrow(Box<Value>),
    /// A closure: its body's index in `Program::closures` and its capture record, in capture order. A borrow
    /// capture holds a [`Value::Ref`] to the captured place.
    Closure(usize, Vec<Value>),
    /// `any I`: the vtable index and the boxed value.
    Any(usize, Box<Value>),
    /// `str.chars()`: the bytes and the next byte offset.
    Chars(Rc<[u8]>, usize),
    /// The owning iterators of a `List`, an array, or a `Map` (its pairs as tuples), and the borrowing iterator of
    /// a `Map`: the remaining items in order.
    Items(std::collections::VecDeque<Value>),
    /// A slot whose value was moved out, or that was never initialized.
    Moved,
}

/// Where a value lives: a local or a temporary of a frame, then a path into it.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub frame: usize,
    pub root: Root,
    pub path: Vec<Step>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Root {
    Local(usize),
    Temp(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// A field of a tuple, struct, or enum variant.
    Field(usize),
    /// An element of an array, a `List`, or a slice.
    Index(usize),
    /// The pointee of a `Heap`, or the value in an `any` box.
    Deref,
    /// Entry `n` of a closure's capture record.
    Env(usize),
}

impl Value {
    /// `Option.Some(v)`: variant 1 of std's `Option`.
    pub fn some(v: Value) -> Value {
        Value::Variant(1, vec![v])
    }

    /// `Option.None`: variant 0.
    pub fn none() -> Value {
        Value::Variant(0, Vec::new())
    }

    /// An `Option` from a Rust `Option`.
    pub fn opt(v: Option<Value>) -> Value {
        v.map_or_else(Value::none, Value::some)
    }

    /// `Result.Ok(v)`: variant 0 of std's `Result`.
    pub fn ok(v: Value) -> Value {
        Value::Variant(0, vec![v])
    }

    /// `Result.Err(v)`: variant 1.
    pub fn err(v: Value) -> Value {
        Value::Variant(1, vec![v])
    }

    pub fn str(s: &str) -> Value {
        Value::Str(Rc::from(s.as_bytes()))
    }

    pub fn int(&self) -> i128 {
        match self {
            Value::Int(v) => *v,
            Value::Bool(b) => i128::from(*b),
            Value::Char(c) => i128::from(*c as u32),
            Value::Borrow(v) => v.int(),
            other => panic!("expected an integer, found {other:?}"),
        }
    }

    pub fn bool(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Borrow(v) => v.bool(),
            other => panic!("expected a bool, found {other:?}"),
        }
    }

    pub fn float(&self) -> f64 {
        match self {
            Value::Float(f) => *f,
            Value::Borrow(v) => v.float(),
            other => panic!("expected a float, found {other:?}"),
        }
    }

    pub fn char(&self) -> char {
        match self {
            Value::Char(c) => *c,
            Value::Borrow(v) => v.char(),
            other => panic!("expected a char, found {other:?}"),
        }
    }

    /// The bytes of a `str` or `String` (or a reference to one).
    pub fn bytes(&self) -> &[u8] {
        match self {
            Value::Str(b) => b,
            Value::String(b) => b,
            Value::Borrow(v) => v.bytes(),
            other => panic!("expected a string, found {other:?}"),
        }
    }

    /// The elements of an array, `List`, or slice (or a reference to one).
    pub fn elems(&self) -> &[Value] {
        match self {
            Value::Array(v) | Value::List(v) => v,
            Value::Slice(v, lo, hi) => &v[*lo..*hi],
            Value::Borrow(v) => v.elems(),
            other => panic!("expected a sequence, found {other:?}"),
        }
    }

    /// The referent of a shared reference, or the value itself.
    pub fn deref(self) -> Value {
        match self {
            Value::Borrow(v) => v.deref(),
            other => other,
        }
    }

    pub fn is_moved(&self) -> bool {
        matches!(self, Value::Moved)
    }
}
