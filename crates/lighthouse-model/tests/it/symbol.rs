//! The symbol every declaration has, before a provider sets what it knows.

use std::path::PathBuf;

use lighthouse_model::{
    Position, Span,
    ucm::{Place, Symbol, SymbolId, SymbolKind, Visibility},
};

#[test]
fn a_new_symbol_has_no_owner_role_or_type_and_a_new_place_no_extent_or_doc() {
    let at = Position { line: 2, col: 3 };
    let span = Span { start: at, end: at };
    let id = SymbolId::new("m", &[], "f", SymbolKind::Function);
    let symbol = Symbol::new(
        id.clone(),
        SymbolKind::Function,
        Visibility::Public,
        PathBuf::from("src/m.rs"),
        "f",
    );
    assert_eq!(symbol.id, id);
    assert_eq!(symbol.name, "f");
    let place = Place::new(id, span);
    assert_eq!(place.span, span);
    assert!(symbol.owner.is_none() && place.extent.is_none() && place.doc.is_none());
    assert_eq!(place.whole(), span);
    assert!(!symbol.implementation);
    assert!(symbol.role.is_none() && symbol.type_ref.is_none() && !symbol.optional);
}
