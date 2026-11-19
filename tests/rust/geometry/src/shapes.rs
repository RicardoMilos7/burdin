/// A rectangle, public through the re-export in the crate root.
pub struct Rect {
    width: i32,
    height: i32,
}

impl Rect {
    pub fn new(width: i32, height: i32) -> Rect {
        Rect { width, height }
    }

    pub fn area(&self) -> i64 {
        self.width as i64 * self.height as i64
    }

    pub fn grow(&mut self, by: i32) {
        self.width += by;
        self.height += by;
    }
}
