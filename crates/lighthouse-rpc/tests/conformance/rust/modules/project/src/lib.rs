pub mod shapes;
mod util;

mod nested {
    pub mod deep {
        pub fn f() -> u32 {
            3
        }
    }
}

#[path = "other/special.rs"]
mod special;

pub use util::helper as aid;

use nested::deep::{self, f};
use shapes::{Circle, Square as Sq};

pub fn run() -> u32 {
    let _ = (Circle, Sq);
    aid() + shapes::square::side() + deep::f() + f() + special::special()
}

#[cfg_attr(unix, path = "other/platform.rs")]
mod platform;

pub fn platform_name() -> &'static str {
    platform::name()
}
