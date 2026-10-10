use std::collections::HashMap;

pub struct Request {
    pub id: u32,
    pub name: String,
    pub parent: Option<u32>,
    pub tags: Vec<String>,
    pub index: HashMap<String, u32>,
    #[serde(default)]
    pub depth: u32,
}

pub struct Pair {
    pub a: u32,
    pub b: u32,
}

pub fn build(r: &Request, n: u32) -> u32 {
    r.id + n
}

pub fn split() -> Result<(Pair, u32), String> {
    Ok((Pair { a: 0, b: 0 }, 0))
}

pub struct Server;

impl Server {
    pub fn new() -> Self {
        Server
    }
}

impl Default for Server {
    fn default() -> Self {
        Server
    }
}
