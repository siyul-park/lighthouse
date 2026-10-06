//! Macro invocations are never expanded. The arguments of macros that take
//! plain expressions (`assert_eq!`, `println!`, `vec!`, ...) are read as
//! expressions so the calls inside them are seen; anything that does not parse
//! that way is invisible to the analysis.

use syn::{Expr, Macro, Token, parse::Parser, punctuated::Punctuated};

/// The expressions a macro invocation's arguments parse as; `None` when they
/// are not expressions.
pub fn arguments(mac: &Macro) -> Option<Vec<Expr>> {
    let tokens = mac.tokens.clone();
    if let Ok(list) = Punctuated::<Expr, Token![,]>::parse_terminated.parse2(tokens.clone()) {
        return Some(list.into_iter().collect());
    }
    let repeat = |input: syn::parse::ParseStream| -> syn::Result<Vec<Expr>> {
        let element: Expr = input.parse()?;
        input.parse::<Token![;]>()?;
        let count: Expr = input.parse()?;
        Ok(vec![element, count])
    };
    if let Ok(list) = repeat.parse2(tokens.clone()) {
        return Some(list);
    }
    if is_named(mac, "matches") {
        let first = |input: syn::parse::ParseStream| -> syn::Result<Expr> {
            let value: Expr = input.parse()?;
            let _rest: proc_macro2::TokenStream = input.parse()?;
            Ok(value)
        };
        return first.parse2(tokens).ok().map(|value| vec![value]);
    }
    None
}

pub fn is_named(mac: &Macro, name: &str) -> bool {
    mac.path.segments.last().is_some_and(|s| s.ident == name)
}
