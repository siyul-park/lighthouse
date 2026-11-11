pub struct Pair(i32, i32);

fn helper(n: i32) -> i32 {
    n
}

/// One step of every kind.
pub fn kinds(xs: &[i32]) -> Result<i32, String> {
    let unset;
    let mut total = helper(xs[0]) + 2 * 10;
    total += xs[1] << 3;
    total = -total;
    if xs.is_empty() {
        return Err(format!("empty {}", 1));
    }
    if total > 10 {
        total = helper(1);
    }
    match total {
        1 => total = helper(1),
        _ => {}
    }
    for x in xs {
        total ^= x;
    }
    {
        unset = total;
    }
    let pair = Pair(unset, helper(unset));
    let _ = (pair.0, vec![1, 2], [3][0..1].len(), true);
    let f = |a: i32| helper(a);
    unsafe {
        let _ = f(1);
    }
    loop {
        break;
    }
    helper(total).checked_add(1).ok_or(String::new())
}

pub fn jump(xs: &[i32]) {
    for _ in xs {
        if xs.len() > 1 {
            continue;
        }
    }
}

pub fn repeated(n: usize) -> Vec<i32> {
    let zeros = [0; 8];
    zeros[..n].to_vec()
}

pub fn labelled(n: i32) -> String {
    format!("{n}")
}

pub trait First {
    fn run(&self) -> i32;
}

pub trait Second {
    fn run(&self) -> i32;
}

pub struct A;
pub struct B;

impl First for A {
    fn run(&self) -> i32 {
        1
    }
}

impl Second for B {
    fn run(&self) -> i32 {
        2
    }
}

/// Calls through a multi-segment macro path and a method.
pub fn logged(n: i32) -> i32 {
    tracing::info!("n = {}", n);
    helper(n).abs()
}
