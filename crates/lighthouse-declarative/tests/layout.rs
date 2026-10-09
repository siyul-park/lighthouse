//! The vocabulary the ordering checks and order keys share.

use lighthouse_declarative::layout::{
    declarations, exposed, has_word_prefix, is_declaration, owner_key,
};
use lighthouse_model::{
    File, Fragment, Position, Project, Span, Symbol, SymbolId, SymbolKind, Visibility,
};
use lighthouse_plugin::{Ctx, NoKeys, Workspace};

fn at(line: u32) -> Span {
    let p = Position { line, col: 1 };
    Span { start: p, end: p }
}

fn symbol(name: &str, kind: SymbolKind, owner: Option<&Symbol>, line: u32) -> Symbol {
    let owners: Vec<&str> = owner.iter().map(|o| o.name.as_str()).collect();
    Symbol {
        id: SymbolId::new("m", &owners, name, kind),
        kind,
        visibility: Visibility::Public,
        owner: owner.map(|o| o.id.clone()),
        file: "m/a.go".into(),
        span: at(line),
        extent: None,
        doc: None,
        name: name.to_owned(),
        role: None,
    }
}

fn file() -> File {
    File {
        path: "m/a.go".into(),
        lang: "go".to_owned(),
        hash: String::new(),
        generated: false,
        test: false,
    }
}

#[test]
fn declarations_lists_each_module_of_a_file_in_source_order() {
    let ty = symbol("Store", SymbolKind::Type, None, 3);
    let method = symbol("Get", SymbolKind::Method, Some(&ty), 9);
    let func = symbol("Open", SymbolKind::Function, None, 5);
    let field = symbol("id", SymbolKind::Field, Some(&ty), 4);
    let file = file();
    let project = Project::merge([Fragment {
        files: vec![file.clone()],
        symbols: vec![method.clone(), func.clone(), field, ty.clone()],
        ..Fragment::default()
    }]);
    let ws = Workspace::new(".");
    let facts = Default::default();
    let ctx = Ctx {
        ws: &ws,
        project: &project,
        file: Some((&file, "")),
        facts: &facts,
        keys: &NoKeys,
        trusted: false,
        memo: &lighthouse_plugin::Memo::default(),
    };

    let lists = declarations(&ctx);

    let names: Vec<Vec<&str>> = lists
        .iter()
        .map(|l| l.iter().map(|s| s.name.as_str()).collect())
        .collect();
    assert_eq!(names, [["Store", "Open", "Get"]]);
}

#[test]
fn exposed_is_everything_that_is_not_private() {
    let mut s = symbol("a", SymbolKind::Function, None, 1);
    assert!(exposed(&s));
    s.visibility = Visibility::Internal;
    assert!(exposed(&s));
    s.visibility = Visibility::Private;
    assert!(!exposed(&s));
}

#[test]
fn owner_key_names_the_owner_or_the_written_path_of_a_method() {
    let ty = symbol("Store", SymbolKind::Type, None, 1);
    let method = symbol("Get", SymbolKind::Method, Some(&ty), 2);
    assert_eq!(owner_key(&method).as_deref(), Some(ty.id.as_str()));
    let mut elsewhere = symbol("Get", SymbolKind::Method, None, 2);
    elsewhere.id = SymbolId::parse("m::Other::Get#method").unwrap();
    assert_eq!(owner_key(&elsewhere).as_deref(), Some("m::Other"));
    assert_eq!(owner_key(&ty), None);
}

#[test]
fn has_word_prefix_wants_a_whole_word() {
    assert!(has_word_prefix("New", "New"));
    assert!(has_word_prefix("NewStore", "New"));
    assert!(has_word_prefix("new_in", "new"));
    assert!(!has_word_prefix("Newton", "New"));
    assert!(!has_word_prefix("newline", "new"));
}

#[test]
fn is_declaration_leaves_out_fields_and_members_of_interfaces() {
    let ty = symbol("Store", SymbolKind::Type, None, 1);
    let iface = symbol("Reader", SymbolKind::Interface, None, 2);
    let field = symbol("id", SymbolKind::Field, Some(&ty), 3);
    let method = symbol("Get", SymbolKind::Method, Some(&ty), 4);
    let declared = symbol("Read", SymbolKind::Method, Some(&iface), 5);
    let project = Project::merge([Fragment {
        files: vec![file()],
        symbols: vec![
            ty.clone(),
            iface.clone(),
            field.clone(),
            method.clone(),
            declared.clone(),
        ],
        ..Fragment::default()
    }]);
    assert!(is_declaration(&project, &ty));
    assert!(is_declaration(&project, &method));
    assert!(!is_declaration(&project, &field));
    assert!(!is_declaration(&project, &declared));
}
