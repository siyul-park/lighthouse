//! What a signature and a field say about types: the types a function takes
//! and gives, and whether a field may be left out.

use lighthouse_protocol as wire;
use quote::ToTokens;
use syn::{Attribute, Field, FnArg, ReturnType, Signature, Type};

use crate::{
    names::{Index, ModId, TyCx, generics_of},
    tree::Vis,
};

/// The collections and wrappers whose empty value means "not given".
const OPTIONAL_TYPES: [&str; 5] = ["Option", "Vec", "HashMap", "BTreeMap", "IndexMap"];

/// The types of the parameters (receiver excluded) and of the result, as
/// written, with the project type each names.
pub fn signature_of(idx: &Index, cx: &TyCx, sig: &Signature) -> wire::Signature {
    let params = sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(typed) => Some(type_ref(idx, cx, &typed.ty)),
            FnArg::Receiver(_) => None,
        })
        .collect();
    let results = match &sig.output {
        ReturnType::Type(_, ty) => vec![type_ref(idx, cx, ty)],
        ReturnType::Default => Vec::new(),
    };
    wire::Signature { params, results }
}

/// The type of a field of a type declared in module `m`.
pub fn field_type(idx: &Index, m: ModId, ty: &Type) -> wire::TypeRef {
    let cx = TyCx {
        module: m,
        self_ty: None,
        generics: generics_of(&syn::Generics::default(), None),
    };
    type_ref(idx, &cx, ty)
}

fn type_ref(idx: &Index, cx: &TyCx, ty: &Type) -> wire::TypeRef {
    let sym = idx.type_sym(ty, cx);
    wire::TypeRef {
        text: spelled(ty),
        exported: sym.as_ref().map(|s| s.vis == Vis::Pub),
        symbol: sym.map(|s| s.id.split('#').next().unwrap_or(&s.id).to_owned()),
    }
}

/// The tokens of a type without the spaces the token printer puts between
/// them: `Option<Ctx>`, `&mut Builder`.
fn spelled(ty: &Type) -> String {
    let printed = ty.to_token_stream().to_string();
    let chars: Vec<char> = printed.chars().collect();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::with_capacity(printed.len());
    for (at, &c) in chars.iter().enumerate() {
        if c == ' ' {
            let before = at.checked_sub(1).map(|i| chars[i]).is_some_and(word);
            let after = chars.get(at + 1).copied().is_some_and(word);
            if !(before && after) {
                continue;
            }
        }
        out.push(c);
    }
    out
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
