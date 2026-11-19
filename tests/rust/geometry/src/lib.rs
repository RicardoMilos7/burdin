//! A small Rust crate that `burdin bindgen --rust` reads in the tests (spec/rust-interop.md).
//!
//! It has one of everything the tool maps (constants, `repr(C)` structs, an enum, opaque handles, free functions,
//! methods, a re-export, a nested module, and a `#[no_mangle]` function) and a few items it must skip with a note.

mod shapes;

pub use shapes::Rect;

pub const MAX_SHAPES: u32 = 64;
pub const SCALE: f64 = 2.5;
pub const NAME: &str = "geometry";
pub const ENABLED: bool = true;
pub const OFFSET: i64 = -7;

/// A point. It is laid out as C lays out the same struct, so Burdin reads it directly.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

/// A struct with padding and a nested struct.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub a: Vec2,
    pub b: Vec2,
    pub tag: u8,
    pub weight: f32,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Point = 0,
    Line = 5,
    Poly = 6,
}

/// An opaque handle: its fields are private and it is not `repr(C)`.
pub struct Counter {
    n: u32,
}

impl Counter {
    pub fn new(start: u32) -> Counter {
        Counter { n: start }
    }

    pub fn inc(&mut self) -> u32 {
        self.n += 1;
        self.n
    }

    pub fn get(&self) -> u32 {
        self.n
    }

    pub fn add_all(&mut self, values: &[u32]) {
        for value in values {
            self.n += value;
        }
    }

    pub fn into_total(self) -> u32 {
        self.n
    }

    pub fn peek(&self) -> &u32 {
        &self.n
    }
}

/// A handle with a `Vec` and a `String` inside.
pub struct Canvas {
    name: String,
    shapes: Vec<Segment>,
}

impl Canvas {
    pub fn new(name: &str) -> Canvas {
        Canvas { name: name.to_string(), shapes: Vec::new() }
    }

    pub fn push(&mut self, segment: Segment) {
        self.shapes.push(segment);
    }

    pub fn len(&self) -> usize {
        self.shapes.len()
    }

    pub fn first(&self) -> Segment {
        self.shapes[0]
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn total_weight(&self) -> f32 {
        self.shapes.iter().map(|s| s.weight).sum()
    }
}

pub fn length(v: &Vec2) -> f64 {
    (v.x * v.x + v.y * v.y).sqrt()
}

pub fn add(a: Vec2, b: Vec2) -> Vec2 {
    Vec2 { x: a.x + b.x, y: a.y + b.y }
}

pub fn scale(v: &mut Vec2, by: f64) {
    v.x *= by;
    v.y *= by;
}

pub fn is_even(n: i64) -> bool {
    n % 2 == 0
}

pub fn not(flag: bool) -> bool {
    !flag
}

pub fn greeting(name: &str) -> String {
    format!("hello, {name}")
}

pub fn describe(kind: Kind) -> u32 {
    match kind {
        Kind::Point => 100,
        Kind::Line => 200,
        Kind::Poly => 300,
    }
}

pub fn kind_of(sides: u32) -> Kind {
    match sides {
        0 | 1 => Kind::Point,
        2 => Kind::Line,
        _ => Kind::Poly,
    }
}

pub fn sum(values: &[i32]) -> i64 {
    values.iter().map(|&v| v as i64).sum()
}

pub fn double_all(values: &mut [i32]) {
    for value in values {
        *value *= 2;
    }
}

pub fn make_rect(width: i32, height: i32) -> Rect {
    Rect::new(width, height)
}

pub mod util {
    pub fn clamp(value: i32, low: i32, high: i32) -> i32 {
        value.max(low).min(high)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn geometry_native_add(a: i32, b: i32) -> i32 {
    a + b
}

/// A `#[no_mangle]` function that takes a C string.
///
/// # Safety
/// `text` must point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn geometry_native_len(text: *const core::ffi::c_char) -> usize {
    unsafe { core::ffi::CStr::from_ptr(text) }.to_bytes().len()
}

/// A Rust function with a raw pointer, wrapped like any other.
///
/// # Safety
/// `buffer` must be valid for `len` writes.
pub unsafe fn fill(buffer: *mut u8, len: usize, value: u8) {
    for i in 0..len {
        unsafe { *buffer.add(i) = value };
    }
}

// Everything below is skipped, each with its own note.

pub fn first<T: Clone>(items: &[T]) -> T {
    items[0].clone()
}

pub trait Shape {
    fn area(&self) -> f64;
}

pub fn parse(text: &str) -> Result<i32, String> {
    text.parse::<i32>().map_err(|e| e.to_string())
}

pub struct Wrapper<T>(pub T);

pub async fn later() {}

pub fn pair() -> (i32, i32) {
    (1, 2)
}
