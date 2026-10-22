//! The edit constructors of a `cel` fix: `remove`, `add`, `replace`, `move`,
//! `rename` and `reorder`, with the anchors `before`, `after`, `start` and
//! `end` that `add` and `move` take. A constructor builds a value that
//! describes an edit and never touches a file; the fix run lowers the list
//! an `edits` expression returns. They are overloads of the shared
//! environment, so `replace` sits beside the strings extension's.

use std::collections::HashMap;

use cel::{
    DeclarationError, Env, ExecutionError, Value,
    common::{
        functions::Function,
        types::{DYN_TYPE, LIST_TYPE, STRING_TYPE, Type},
        value::{CowVal, Val},
    },
};

/// The kind of edit or anchor a constructor builds, under this key.
pub const KIND: &str = "edit";

/// Registers the constructors on `env`.
pub(crate) fn install(env: &mut Env) -> Result<(), DeclarationError> {
    let constructors: [(&str, Function, Vec<Type>); 10] = [
        ("remove", remove, vec![DYN_TYPE]),
        ("add", add, vec![DYN_TYPE, STRING_TYPE]),
        ("replace", replace, vec![DYN_TYPE, STRING_TYPE]),
        ("move", relocate, vec![DYN_TYPE, DYN_TYPE]),
        ("rename", rename, vec![DYN_TYPE, STRING_TYPE]),
        ("reorder", reorder, vec![LIST_TYPE]),
        ("before", before, vec![DYN_TYPE]),
        ("after", after, vec![DYN_TYPE]),
        ("start", start, vec![DYN_TYPE]),
        ("end", end, vec![DYN_TYPE]),
    ];
    for (name, op, args) in constructors {
        env.add_overload(name, &format!("{name}_edit"), args, op)?;
    }
    Ok(())
}

/// The value `{ edit: kind, names[i]: args[i] }`.
fn built<'b, 'v>(
    kind: &str,
    names: &[&str],
    args: Vec<CowVal<'b, 'v>>,
) -> Result<CowVal<'b, 'v>, ExecutionError> {
    let mut fields = HashMap::from([(KIND.to_owned(), Value::from(kind))]);
    for (name, arg) in names.iter().zip(&args) {
        fields.insert((*name).to_owned(), Value::try_from(&**arg as &dyn Val)?);
    }
    let value = Box::<dyn Val>::try_from(Value::Map(fields.into()))?;
    Ok(CowVal::from(value))
}

macro_rules! constructors {
    ($($function:ident: $kind:literal ($($field:literal),*);)*) => {$(
        fn $function<'b, 'v>(args: Vec<CowVal<'b, 'v>>) -> Result<CowVal<'b, 'v>, ExecutionError> {
            built($kind, &[$($field),*], args)
        }
    )*};
}

constructors! {
    remove: "remove" ("target");
    add: "add" ("anchor", "text");
    replace: "replace" ("target", "text");
    relocate: "move" ("node", "anchor");
    rename: "rename" ("symbol", "name");
    reorder: "reorder" ("nodes");
    before: "before" ("node");
    after: "after" ("node");
    start: "start" ("file");
    end: "end" ("file");
}
