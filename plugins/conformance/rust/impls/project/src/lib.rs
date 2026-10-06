use std::fmt;

/// A bounded stack.
pub struct Stack<T> {
    items: Vec<T>,
    pub limit: usize,
}

impl<T> Stack<T> {
    pub const EMPTY: usize = 0;

    /// An empty stack.
    pub fn new(limit: usize) -> Self {
        Self {
            items: Vec::new(),
            limit,
        }
    }

    pub fn push(&mut self, item: T) {
        self.check();
        self.items.push(item);
    }

    fn check(&self) {
        let _ = self.limit;
    }
}

impl<T: fmt::Debug> fmt::Display for Stack<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.items)
    }
}

/// Things with a name.
pub trait Named {
    /// The name.
    fn name(&self) -> String;

    fn shout(&self) -> String {
        self.name().to_uppercase()
    }
}

pub trait Aged {
    fn name(&self) -> u32;
}

pub struct Dog;

impl Named for Dog {
    fn name(&self) -> String {
        "dog".to_owned()
    }
}

impl Aged for Dog {
    fn name(&self) -> u32 {
        3
    }
}

impl Dog {
    pub fn bark(&self) -> String {
        self.shout()
    }
}

pub struct Meters(pub f64);

impl From<f64> for Meters {
    fn from(v: f64) -> Self {
        Meters(v)
    }
}

impl From<u32> for Meters {
    fn from(v: u32) -> Self {
        Meters(f64::from(v))
    }
}

pub trait Summary {
    fn summary(&self) -> String;
}

impl<T: Named> Summary for T {
    fn summary(&self) -> String {
        self.name()
    }
}

impl Named for String {
    fn name(&self) -> String {
        self.clone()
    }
}

mod other {
    impl super::Dog {
        pub fn wag(&self) {}
    }
}

pub struct Marker;

impl Summary for Marker {
    fn summary(&self) -> String {
        String::new()
    }
}

pub fn build() -> Stack<u8> {
    let mut stack = Stack::new(3);
    stack.push(1);
    stack
}
