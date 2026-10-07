// A plain comment above the attributes belongs to the item.
/// Holds the limit.
#[derive(Debug)]
pub struct Limit {
    /// How many.
    #[allow(dead_code)]
    pub count: u8,
}

/// Kinds of work.
#[derive(Clone, Copy)]
pub enum Kind {
    /// The quick one.
    Fast,
    Slow,
}

#[cfg(feature = "extra")]
/// Only with the feature.
pub fn extra() -> u8 {
    1
}

impl Limit {
    /// Makes one.
    #[must_use]
    pub fn new() -> Self {
        Self { count: 0 }
    }

    pub fn count(&self) -> u8 {
        self.count
    }
}

pub const MAX: u8 = 9; // trailing note

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks the default.
    #[test]
    fn new_starts_at_zero() {
        assert_eq!(Limit::new().count(), 0);
    }
}
