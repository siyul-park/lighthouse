macro_rules! double {
    ($x:expr) => {
        $x * 2
    };
}

fn helper() -> u32 {
    2
}

fn other() -> u32 {
    3
}

fn hidden() -> u32 {
    4
}

pub fn std_macros() -> u32 {
    assert_eq!(helper(), 2);
    let repeated = vec![other(); 2];
    println!("{}", other());
    repeated.len() as u32
}

pub fn user_macro() -> u32 {
    double!(helper())
}

pub fn unreadable_macro() {
    custom! { a => b; hidden }
}

#[derive(Debug, Clone)]
pub struct Derived;

#[inline]
#[allow(dead_code)]
pub fn attributed() {}

#[tokio::main]
async fn entry() {
    helper();
}
