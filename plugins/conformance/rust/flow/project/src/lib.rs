pub fn branches(n: i32) -> &'static str {
    if n < 0 {
        "neg"
    } else if n == 0 {
        "zero"
    } else {
        "pos"
    }
}

pub fn guarded(n: Option<i32>) -> i32 {
    match n {
        Some(x) if x > 3 => x,
        Some(x) => x + 1,
        None => 0,
    }
}

pub fn dispatcher(k: u8) -> u32 {
    match k {
        0 => 1,
        1 => 2,
        _ => 3,
    }
}

pub fn explicit_returns(k: u8) -> u32 {
    match k {
        0 => return 1,
        _ => return 2,
    }
}

pub fn side_effects(k: u8) {
    match k {
        0 => touch(),
        _ => touch(),
    }
}

fn touch() {}

pub fn statements_in_arms(k: u8) -> u32 {
    match k {
        0 => {
            touch();
            1
        }
        _ => 2,
    }
}

pub fn let_else(v: Option<u8>) -> u8 {
    let Some(x) = v else {
        return 0;
    };
    x
}

pub fn if_let(v: Option<u8>) -> u8 {
    if let Some(x) = v { x } else { 0 }
}

pub fn loops(rows: &[Vec<u8>]) -> u32 {
    let mut total = 0;
    'outer: for row in rows {
        for v in row {
            if *v == 0 {
                continue 'outer;
            }
            total += 1;
        }
    }
    while total > 100 {
        total -= 1;
    }
    loop {
        break;
    }
    total as u32
}

pub fn closures(v: Vec<u8>) -> usize {
    v.iter()
        .filter(|x| {
            if **x > 1 {
                true
            } else {
                false
            }
        })
        .count()
}

pub fn question(s: &str) -> Result<u32, std::num::ParseIntError> {
    let n = s.parse::<u32>()?;
    Ok(n)
}

pub fn logic(a: bool, b: bool, c: bool) -> bool {
    a && b && c || a
}

pub fn factorial(n: u64) -> u64 {
    if n == 0 { 1 } else { n * factorial(n - 1) }
}

pub struct Walker;

impl Walker {
    pub fn walk(&self, n: u32) {
        if n > 0 {
            self.walk(n - 1);
        }
    }
}

pub fn outer() -> u32 {
    fn inner() -> u32 {
        1
    }
    inner()
}

pub async fn asyncs() -> u32 {
    let f = async {
        if true { 1 } else { 2 }
    };
    f.await
}
