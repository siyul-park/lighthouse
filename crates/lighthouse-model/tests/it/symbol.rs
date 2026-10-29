//! The symbol every declaration has, before a provider sets what it knows.

use std::path::PathBuf;

use lighthouse_model::{
    Position, Span,
    ucm::{Symbol, SymbolId, SymbolKind, Visibility},
};

#[test]
fn a_new_symbol_has_no_owner_extent_doc_role_or_type() {
    let at = Position { line: 2, col: 3 };
    let span = Span { start: at, end: at };
    let id = SymbolId::new("m", &[], "f", SymbolKind::Function);
    let symbol = Symbol::new(
        id.clone(),
        SymbolKind::Function,
        Visibility::Public,
        PathBuf::from("src/m.rs"),
        span,
        "f",
    );
    assert_eq!(symbol.id, id);
    assert_eq!(symbol.name, "f");
    assert_eq!(symbol.span, span);
    assert!(symbol.owner.is_none() && symbol.extent.is_none() && symbol.doc.is_none());
    assert!(symbol.role.is_none() && symbol.type_ref.is_none() && !symbol.optional);
}
