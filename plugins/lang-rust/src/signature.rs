//! What a signature and a field say about types: the project types a
//! function takes, and whether a field may be left out.

use syn::{Attribute, Field, FnArg, Signature, Type};

use crate::names::{Index, TyCx};

/// The collections and wrappers whose empty value means "not given".
const OPTIONAL_TYPES: [&str; 5] = ["Option", "Vec", "HashMap", "BTreeMap", "IndexMap"];

/// The project types the parameters name (receiver excluded) as kind-less
/// symbol ids, one per parameter in order: a type taken twice appears twice.
pub fn project_types(idx: &Index, cx: &TyCx, sig: &Signature) -> Vec<String> {
    sig.inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(typed) => idx.type_sym(&typed.ty, cx),
            FnArg::Receiver(_) => None,
        })
        .map(|sym| sym.id.split('#').next().unwrap_or(&sym.id).to_owned())
        .collect()
}

/// Whether a caller may leave the field out: it is an `Option`, a `Vec` or a
/// map, `#[serde(default)]` fills it in, or its struct has a container-level
/// `#[serde(default)]` (`container_default`).
pub fn optional_field(field: &Field, container_default: bool) -> bool {
    let by_type = match &field.ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|s| OPTIONAL_TYPES.contains(&s.ident.to_string().as_str())),
        _ => false,
    };
    by_type || container_default || serde_default(&field.attrs)
}

/// Whether one of the attributes is `#[serde(default)]` or
/// `#[serde(default = "...")]`.
pub fn serde_default(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("serde") {
            return false;
        }
        let mut found = false;
        let parsed = attr.parse_nested_meta(|meta| {
            found |= meta.path.is_ident("default");
            if meta.input.peek(syn::Token![=]) {
                meta.value()?.parse::<syn::Expr>()?;
            }
            Ok(())
        });
        // A serde attribute we cannot read is no evidence that a default
        // fills the field: it counts as required rather than optional.
        parsed.is_ok() && found
    })
}
