//! What the ordering rules share: the declarations of a file in source order
//! and the vocabulary that tells a declaration's owner and visibility.

use lighthouse_model::{Project, Symbol, SymbolKind, SymbolRole, Visibility};
use lighthouse_plugin::Ctx;

/// Declarations of the focused file in source order, one list per module the
/// file declares (an inline module is a module of its own). A declaration is a
/// type, interface, constant, variable, function or method that is not a member
/// of an interface and not nested in a function body. Test code is left out,
/// except the fixtures of a test file, which are ordered like any other
/// declarations.
pub(crate) fn declarations<'a>(ctx: &Ctx<'a>) -> Vec<Vec<&'a Symbol>> {
    let Some((file, _)) = ctx.file else {
        return Vec::new();
    };
    let mut all: Vec<&Symbol> = ctx
        .project
        .symbols_in(&file.path)
        .filter(|s| is_declaration(ctx.project, s))
        .filter(|s| {
            if file.test {
                s.role == Some(SymbolRole::Fixture)
            } else {
                !ctx.project.in_test(&s.id)
            }
        })
        .collect();
    all.sort_by(|a, b| (a.span.start, &a.id).cmp(&(b.span.start, &b.id)));
    let mut modules: Vec<(&str, Vec<&Symbol>)> = Vec::new();
    for symbol in all {
        let module = symbol.id.module();
        match modules.iter_mut().find(|(m, _)| *m == module) {
            Some((_, list)) => list.push(symbol),
            None => modules.push((module, vec![symbol])),
        }
    }
    modules.into_iter().map(|(_, list)| list).collect()
}

/// Visible beyond its own scope: public, or public inside the project.
pub(crate) fn exposed(symbol: &Symbol) -> bool {
    symbol.visibility != Visibility::Private
}

/// What a member belongs to: its owner, or for a member of a type declared
/// elsewhere, the written owner path of its id.
pub(crate) fn owner_key(symbol: &Symbol) -> Option<String> {
    if let Some(owner) = &symbol.owner {
        return Some(owner.as_str().to_owned());
    }
    (symbol.kind == SymbolKind::Method)
        .then(|| {
            symbol
                .id
                .as_str()
                .rsplit_once("::")
                .map(|(head, _)| head.to_owned())
        })
        .flatten()
}

/// Whether `name` is `prefix` or `prefix` followed by a word of its own: an
/// upper-case letter or an underscore (`New`, `NewStore`, `new`, `new_in`; not
/// `Newton` or `newline`).
pub(crate) fn has_word_prefix(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|rest| {
        rest.chars()
            .next()
            .is_none_or(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit())
    })
}

pub(crate) fn is_declaration(project: &Project, symbol: &Symbol) -> bool {
    use SymbolKind::{Const, Function, Interface, Method, Type, Var};
    if !matches!(
        symbol.kind,
        Type | Interface | Const | Var | Function | Method
    ) {
        return false;
    }
    match symbol.owner.as_ref().and_then(|o| project.symbol(o)) {
        None => true,
        Some(owner) => owner.kind == Type && matches!(symbol.kind, Method | Const | Var),
    }
}
