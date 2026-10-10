use lighthouse_plugin::Workspace;

#[test]
fn workspace_constructor_prefixes() {
    let mut ws = Workspace::new("/p");
    assert!(ws.constructor_prefixes("go").is_empty(), "unknown language");
    ws.constructors
        .insert("go".to_owned(), vec!["New".to_owned()]);
    assert_eq!(ws.constructor_prefixes("go"), ["New"]);
    assert!(ws.constructor_prefixes("rust").is_empty());
}
