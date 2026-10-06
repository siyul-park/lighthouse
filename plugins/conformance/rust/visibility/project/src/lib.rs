pub struct Open {
    pub a: u8,
    pub(crate) b: u8,
    c: u8,
}

pub(crate) struct Shared;

struct Hidden;

pub enum Kind {
    A,
    B,
}

pub trait Tr {
    fn m(&self);
}

pub const LIMIT: u32 = 1;
pub(crate) const SOFT: u32 = 2;
static COUNT: u32 = 0;

pub fn open() {}
pub(crate) fn crate_wide() {}
fn private() {}

mod inner {
    pub fn exposed_internal() {}
    pub(in crate::inner) fn scoped() {}
    pub fn reexported() {}
    pub(super) fn up() {}
}

pub use inner::reexported;

pub mod outer {
    pub fn deep() {}
    pub(super) fn parent() {}
    pub(self) fn own() {}
}

impl Open {
    pub fn get(&self) -> u8 {
        self.a
    }
    pub(crate) fn soft(&self) -> u8 {
        self.b
    }
    fn hide(&self) -> u8 {
        self.c
    }
}

impl Tr for Open {
    fn m(&self) {}
}

#[cfg(test)]
mod tests {
    pub fn helper() {}

    #[test]
    fn t() {
        helper();
    }
}
