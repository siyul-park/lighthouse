mod inner {
    pub struct A;

    pub fn f() {}

    pub(crate) fn hidden() {}

    mod deeper {
        pub fn g() {}
    }

    pub mod open {
        pub fn h() {}
    }
}

mod named {
    pub fn n() {}
    pub fn unexported() {}
}

pub use inner::*;
pub use named::n;
