//! A shape in words: what a finding of a contradiction says the code looks
//! like, as a reader names it, without the rendered files.

use super::{Decl, Kind, Relation, Shape, Visibility};

impl Shape {
    /// The declarations by letter, with what each is and how it relates to
    /// the others: `a public type A, a public method B owned by A`.
    pub fn describe(&self) -> String {
        let letter = |at: usize| char::from(b'A' + u8::try_from(at % 26).unwrap_or(0));
        let parts: Vec<String> = self
            .decls
            .iter()
            .enumerate()
            .map(|(at, decl)| {
                let relations: Vec<String> = self
                    .edges
                    .iter()
                    .filter(|edge| edge.from == at)
                    .map(|edge| format!("{} {}", relation(edge.relation), letter(edge.to)))
                    .collect();
                let tail = if relations.is_empty() {
                    String::new()
                } else {
                    format!(" {}", relations.join(" and "))
                };
                format!("{} {}{tail}", noun(decl), letter(at))
            })
            .collect();
        parts.join(", ")
    }
}

/// What a declaration is: `a public type`, `a private function`.
fn noun(decl: &Decl) -> String {
    let visibility = match decl.visibility {
        Visibility::Public => "public",
        Visibility::Private => "private",
    };
    let kind = match decl.kind {
        Kind::Type => "type",
        Kind::Func => "function",
        Kind::Method => "method",
        Kind::Constructor => "constructor",
        Kind::Option => "option",
        Kind::Hook => "hook",
        Kind::Init => "init function",
        Kind::Const => "constant",
        Kind::Var => "variable",
    };
    let tested = if decl.tested { " with a test" } else { "" };
    format!("a {visibility} {kind}{tested}")
}

fn relation(relation: Relation) -> &'static str {
    match relation {
        Relation::Owner => "owned by",
        Relation::Uses => "using",
        Relation::Calls => "calling",
    }
}
