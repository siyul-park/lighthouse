pub struct A;
pub struct B;
pub struct C;
pub struct D;
pub struct E;

impl A {
    fn run(&self) {}
    fn tick(&self) {}
    fn once(&self) {}
}
impl B {
    fn run(&self) {}
    fn tick(&self) {}
}
impl C {
    fn run(&self) {}
}
impl D {
    fn run(&self) {}
}
impl E {
    fn run(&self) {}
}

/// Five private `run` methods: too common to guess.
pub fn common(x: &external::Thing) {
    x.run();
}

/// Two private `tick` methods: both are possible targets.
pub fn few(x: &external::Thing) {
    x.tick();
}

/// One private `once` method.
pub fn single(x: &external::Thing) {
    x.once();
}

/// The receiver is known, so the call is exact and nothing is guessed.
pub fn exact(a: &A) {
    a.tick();
}
