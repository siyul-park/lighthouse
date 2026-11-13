//! Go: one package of one file, and a test file of owner tests. Names are a
//! function of the declaration's kind, visibility and index only, so two
//! arrangements that differ by the renaming of equal declarations differ only
//! in the digits of their names, and no order decision may read them.

use super::{Arrangement, View};
use crate::tester::witness::{Kind, Relation, Visibility};

/// The source file of the arrangement.
pub(super) fn source(view: &View, arrangement: &Arrangement, package: &str) -> String {
    let decls: Vec<String> = arrangement
        .source
        .iter()
        .map(|&index| decl_text(view, index))
        .collect();
    format!("package {package}\n\n{}\n", decls.join("\n\n"))
}

/// The test file: one owner test per tested declaration, in test order.
pub(super) fn tests(view: &View, arrangement: &Arrangement, package: &str) -> String {
    let tests: Vec<String> = arrangement
        .tests
        .iter()
        .map(|&target| test_text(view, target))
        .collect();
    format!(
        "package {package}\n\nimport \"testing\"\n\n{}\n",
        tests.join("\n\n")
    )
}

/// The name of a declaration: its case is its visibility, a constructor or
/// option builder is named after its type, an `init` and a hook by what they
/// are.
fn name_of(view: &View, index: usize) -> String {
    let decl = view.decl(index);
    let public = decl.visibility == Visibility::Public;
    let (upper, lower) = match decl.kind {
        Kind::Type => ('T', 't'),
        Kind::Func => ('F', 'f'),
        Kind::Method => ('M', 'm'),
        Kind::Const => ('C', 'c'),
        Kind::Var => ('V', 'v'),
        Kind::Init => return "init".to_owned(),
        Kind::Hook => return view.alphabet.hook.unwrap_or("String").to_owned(),
        Kind::Constructor => return built_name(view, index, "New", "new"),
        Kind::Option => return built_name(view, index, "With", "with"),
    };
    format!("{}{index}", if public { upper } else { lower })
}

/// A name that is a prefix and the name of the owner type with a capital.
fn built_name(view: &View, index: usize, public: &str, private: &str) -> String {
    let prefix = if view.decl(index).visibility == Visibility::Public {
        public
    } else {
        private
    };
    let owner = name_of(view, view.owner(index));
    let mut letters = owner.chars();
    let first: String = letters
        .next()
        .into_iter()
        .flat_map(char::to_uppercase)
        .collect();
    format!("{prefix}{first}{}", letters.as_str())
}

/// One declaration.
fn decl_text(view: &View, index: usize) -> String {
    let name = name_of(view, index);
    match view.decl(index).kind {
        Kind::Type => type_text(view, index, &name),
        Kind::Const => format!("const {name} = 1"),
        Kind::Var => format!("var {name} = 0"),
        Kind::Method => {
            let receiver = name_of(view, view.owner(index));
            func(
                &format!("func (r *{receiver}) {name}()"),
                &body(view, index),
            )
        }
        Kind::Hook => hook_text(view, index, &name),
        Kind::Constructor => {
            let built = name_of(view, view.owner(index));
            let mut lines = body(view, index);
            lines.push(format!("return &{built}{{}}"));
            func(&format!("func {name}() *{built}"), &lines)
        }
        Kind::Option => {
            let built = name_of(view, view.owner(index));
            let mut lines = body(view, index);
            lines.push(format!("return func(*{built}) {{}}"));
            func(&format!("func {name}() func(*{built})"), &lines)
        }
        Kind::Func | Kind::Init => func(&format!("func {name}()"), &body(view, index)),
    }
}

/// A struct of the types it names, or an empty one.
fn type_text(view: &View, index: usize, name: &str) -> String {
    let fields = view.targets(index, Relation::Uses);
    if fields.is_empty() {
        return format!("type {name} struct{{}}");
    }
    let fields: Vec<String> = fields
        .iter()
        .map(|&f| format!("\th{f} *{}", name_of(view, f)))
        .collect();
    format!("type {name} struct {{\n{}\n}}", fields.join("\n"))
}

/// The hook method: `Clone` clones, the others convert to a string.
fn hook_text(view: &View, index: usize, name: &str) -> String {
    let receiver = name_of(view, view.owner(index));
    if name == "Clone" {
        return func(
            &format!("func (r *{receiver}) Clone() *{receiver}"),
            &["return r".to_owned()],
        );
    }
    func(
        &format!("func (r *{receiver}) {name}() string"),
        &["return \"\"".to_owned()],
    )
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
                (Relation::Uses, _) => Some(format!("var _ *{target}")),
                (Relation::Calls, Kind::Func) => Some(format!("{target}()")),
                (Relation::Calls, Kind::Method) => Some(format!("r.{target}()")),
                (Relation::Calls, _) => Some(format!("_ = {target}")),
            }
        })
        .collect()
}

/// The test that names `target` as its owner.
fn test_text(view: &View, target: usize) -> String {
    let name = name_of(view, target);
    if view.decl(target).kind == Kind::Func {
        return func(
            &format!("func Test{name}(t *testing.T)"),
            &[format!("{name}()")],
        );
    }
    let owner = name_of(view, view.owner(target));
    func(
        &format!("func Test{owner}_{name}(t *testing.T)"),
        &[format!("var x {owner}"), format!("x.{name}()")],
    )
}

/// A function with this signature and these statements.
fn func(signature: &str, body: &[String]) -> String {
    if body.is_empty() {
        return format!("{signature} {{}}");
    }
    let lines: Vec<String> = body.iter().map(|line| format!("\t{line}")).collect();
    format!("{signature} {{\n{}\n}}", lines.join("\n"))
}
