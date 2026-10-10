//! What a signature and a field say about types: the project types a
//! function takes and returns, and whether a field may be left out.

use syn::{Attribute, Field, FnArg, ReturnType, Signature, Type};

use crate::{
    names::{Index, TyCx},
    util::unwrap_result,
};

/// The collections and wrappers whose empty value means "not given".
const OPTIONAL_TYPES: [&str; 5] = ["Option", "Vec", "HashMap", "BTreeMap", "IndexMap"];

/// The project struct types the parameters name (receiver excluded) and the
/// results name (the `Ok` type of a `Result`, each element of a tuple), as
/// kind-less symbol ids in order of first appearance.
pub fn project_types(idx: &Index, cx: &TyCx, sig: &Signature) -> (Vec<String>, Vec<String>) {
    let params = sig.inputs.iter().filter_map(|arg| match arg {
        FnArg::Typed(typed) => Some(&*typed.ty),
        FnArg::Receiver(_) => None,
    });
    let results = match &sig.output {
        ReturnType::Type(_, ty) => elements(unwrap_result(ty)),
        ReturnType::Default => Vec::new(),
    };
    (named(idx, cx, params), named(idx, cx, results))
}

fn elements(ty: &Type) -> Vec<&Type> {
    match ty {
        Type::Tuple(tuple) => tuple.elems.iter().collect(),
        other => vec![other],
    }
}

fn named<'a>(idx: &Index, cx: &TyCx, types: impl IntoIterator<Item = &'a Type>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for ty in types {
        let Some(sym) = idx.type_sym(ty, cx) else {
            continue;
        };
        let id = sym.id.split('#').next().unwrap_or(&sym.id).to_owned();
        if !found.contains(&id) {
            found.push(id);
        }
    }
    found
}

/// Whether a caller may leave the field out: it is an `Option`, a `Vec` or a
/// map, or `#[serde(default)]` fills it in.
pub fn optional_field(field: &Field) -> bool {
    let by_type = match &field.ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|s| OPTIONAL_TYPES.contains(&s.ident.to_string().as_str())),
        _ => false,
    };
    by_type || field.attrs.iter().any(serde_default)
}

fn serde_default(attr: &Attribute) -> bool {
    let mut found = false;
    if attr.path().is_ident("serde") {
        let _ = attr.parse_nested_meta(|meta| {
            found |= meta.path.is_ident("default");
            if meta.input.peek(syn::Token![=]) {
                meta.value()?.parse::<syn::Expr>()?;
            }
            Ok(())
        });
    }
    found
}
