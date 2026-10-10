pub struct Ctx;

pub struct Builder {
    pub ctx: Option<Ctx>,
    name: String,
}

pub fn first(items: &[u32]) -> u32 {
    items.first().copied().unwrap()
}

pub fn named(b: &mut Builder) -> String {
    b.name.parse::<String>().expect("name")
}

pub fn never() -> u32 {
    unreachable!()
}

pub fn later() {
    todo!("later")
}

pub fn missing() {
    unimplemented!()
}

pub fn fails(code: u32) {
    if code > 3 {
        panic!("code {code}");
    }
}

pub fn inner() -> u32 {
    fn helper() -> u32 {
        Some(1).unwrap()
    }
    let f = || Some(2).unwrap();
    f() + helper()
}

pub fn flipped(r: Result<u32, String>) -> String {
    r.unwrap_err()
}

pub fn declared() -> u32 {
    struct Local;
    impl Local {
        fn value(&self) -> u32 {
            Some(3).unwrap()
        }
    }
    Local.value()
}

pub fn quiet() -> Option<u32> {
    Some(1).map(|v| v + 1)
}
