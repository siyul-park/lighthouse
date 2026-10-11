use std::collections::HashMap;

use serde::Serialize;

mod math {
    pub const SCALE: u32 = 2;

    pub fn square(x: u32) -> u32 {
        x * x
    }
}

use math::square as sq;

#[derive(Serialize)]
pub struct Counter {
    count: u32,
    label: String,
}

impl Counter {
    pub fn new() -> Self {
        Self {
            count: 0,
            label: String::new(),
        }
    }

    pub fn bump(&mut self) -> u32 {
        self.count += 1;
        self.double()
    }

    fn double(&self) -> u32 {
        sq(self.count) * math::SCALE
    }

    fn load(&self, k: u32) -> u32 {
        self.read(k)
    }

    fn read(&self, k: u32) -> u32 {
        k
    }

    pub fn get(&self, k: u32) -> u32 {
        self.load(k)
    }
}

fn helper(x: u32) -> u32 {
    x
}

fn wrapper(x: u32) -> u32 {
    helper(x)
}

pub fn apply(items: Vec<u32>) -> Vec<u32> {
    items.into_iter().map(helper).collect()
}

pub fn unknown(x: &Mystery) {
    x.double();
}

pub fn literal() -> Counter {
    let c = Counter {
        count: 1,
        label: String::new(),
    };
    c
}

pub fn map() -> HashMap<u32, u32> {
    HashMap::new()
}

pub fn typed(c: &mut Counter) -> u32 {
    c.bump()
}

pub fn chain() -> u32 {
    let mut c = Counter::new();
    c.bump();
    c.count
}
