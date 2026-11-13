//! Rust: one module of the crate, its tests a `#[cfg(test)]` module at the
//! end of it.

use super::{Arrangement, View};
use crate::tester::witness::{Kind, Relation, Visibility};

/// The module of the arrangement.
pub(super) fn source(view: &View, arrangement: &Arrangement) -> String {
    let mut items: Vec<String> = arrangement.source.iter().map(|&i| item(view, i)).collect();
    if !arrangement.tests.is_empty() {
        items.push(tests(view, arrangement));
    }
    format!("{}\n", items.join("\n\n"))
}

fn item(view: &View, index: usize) -> String {
    let name = name_of(view, index);
    let decl = view.decl(index);
    let public = if decl.visibility == Visibility::Public {
        "pub "
    } else {
        ""
    };
    match decl.kind {
        Kind::Type => type_text(view, index, &name, public),
        Kind::Func => func(&format!("{public}fn {name}()"), &body(view, index), ""),
        Kind::Method => {
            let owner = name_of(view, view.owner(index));
            let signature = format!("{public}fn {name}(&self)");
            let method = func(&signature, &body(view, index), "    ");
            format!("impl {owner} {{\n{method}\n}}")
        }
        Kind::Const => format!("{public}const {name}: i32 = 1;"),
        Kind::Var => format!("{public}static {name}: i32 = 0;"),
        Kind::Constructor => constructor(view, index, public),
        Kind::Hook => {
            let owner = name_of(view, view.owner(index));
            format!("impl Drop for {owner} {{\n    fn drop(&mut self) {{}}\n}}")
        }
        Kind::Option | Kind::Init => unreachable!("Go declarations are not in a Rust alphabet"),
    }
}

/// `fn new() -> Self` in an impl block of its type.
fn constructor(view: &View, index: usize, public: &str) -> String {
    let owner = view.owner(index);
    let fields: Vec<String> = view
        .targets(owner, Relation::Uses)
        .iter()
        .map(|&f| format!("h{f}: None"))
        .collect();
    let built = if fields.is_empty() {
        "Self".to_owned()
    } else {
        format!("Self {{ {} }}", fields.join(", "))
    };
    let mut lines = body(view, index);
    lines.push(built);
    let method = func(&format!("{public}fn new() -> Self"), &lines, "    ");
    format!("impl {} {{\n{method}\n}}", name_of(view, owner))
}

fn type_text(view: &View, index: usize, name: &str, public: &str) -> String {
    let fields = view.targets(index, Relation::Uses);
    if fields.is_empty() {
        return format!("{public}struct {name};");
    }
    let fields: Vec<String> = fields
        .iter()
        .map(|&f| format!("    h{f}: Option<Box<{}>>,", name_of(view, f)))
        .collect();
    format!("{public}struct {name} {{\n{}\n}}", fields.join("\n"))
}

/// What a function or method does: the statements for its edges.
fn body(view: &View, index: usize) -> Vec<String> {
    view.shape
        .edges
        .iter()
        .filter(|edge| edge.from == index)
        .filter_map(|edge| {
            let target = name_of(view, edge.to);
            match (edge.relation, view.decl(edge.to).kind) {
                (Relation::Owner, _) => None,
                (Relation::Uses, _) => Some(format!("let _: Option<&{target}> = None;")),
                (Relation::Calls, Kind::Func) => Some(format!("{target}();")),
                (Relation::Calls, Kind::Method) => Some(format!("self.{target}();")),
                (Relation::Calls, Kind::Const) => Some(format!("let _ = {target};")),
                (Relation::Calls, _) => Some(format!("let _ = &{target};")),
            }
        })
        .collect()
}

/// The `#[cfg(test)]` module of the tests, each named after its target.
fn tests(view: &View, arrangement: &Arrangement) -> String {
    let tests: Vec<String> = arrangement
        .tests
        .iter()
        .map(|&target| test_text(view, target))
        .collect();
    format!("#[cfg(test)]\nmod tests {{\n{}\n}}", tests.join("\n\n"))
}

fn test_text(view: &View, target: usize) -> String {
    let name = name_of(view, target);
    if view.decl(target).kind == Kind::Func {
        let signature = format!("#[test]\n    fn {name}()");
        return func(&signature, &[format!("super::{name}();")], "    ");
    }
    let owner = name_of(view, view.owner(target)).to_lowercase();
    func(&format!("#[test]\n    fn {owner}_{name}()"), &[], "    ")
}

/// A function with this signature and these statements, indented.
fn func(signature: &str, body: &[String], indent: &str) -> String {
    let head = format!("{indent}{signature}");
    if body.is_empty() {
        return format!("{head} {{}}");
    }
    let lines: Vec<String> = body
        .iter()
        .map(|line| format!("{indent}    {line}"))
        .collect();
    format!("{head} {{\n{}\n{indent}}}", lines.join("\n"))
}

/// The name of a declaration: a constructor is `new`, a hook is the trait.
fn name_of(view: &View, index: usize) -> String {
    let prefix = match view.decl(index).kind {
        Kind::Type => 'T',
        Kind::Method => 'm',
        Kind::Const => 'C',
        Kind::Var => 'V',
        Kind::Constructor => return "new".to_owned(),
        Kind::Hook => return view.alphabet.hook.unwrap_or("Drop").to_owned(),
        Kind::Func | Kind::Option | Kind::Init => 'f',
    };
    format!("{prefix}{index}")
}
