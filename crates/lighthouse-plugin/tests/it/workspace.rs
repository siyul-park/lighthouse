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

#[test]
fn workspace_read() {
    let mut ws = Workspace::new("/no/such/root");
    let path = std::path::Path::new("a.txt");
    let error = ws.read(path).unwrap_err();
    assert!(error.to_string().contains("a.txt"), "{error}");
    ws.overlays.insert(path.to_owned(), "overlaid".to_owned());
    assert_eq!(ws.read(path).unwrap(), "overlaid");
}
