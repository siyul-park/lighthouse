use std::{fs, path::Path};

use lighthouse_protocol::{Http, ROUTES};

/// The `(method, HTTP twin)` rows of the method table in docs/plugin-protocol.md.
fn documented() -> Vec<(String, Option<String>)> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/plugin-protocol.md");
    let text = fs::read_to_string(path).unwrap();
    let table = text.split("## Methods").nth(1).unwrap();
    table
        .lines()
        .take_while(|line| line.starts_with('|') || line.is_empty())
        .filter(|line| line.starts_with("| `"))
        .map(|line| {
            let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
            let twin = cells[2].trim_matches('`');
            (
                cells[0].trim_matches('`').to_owned(),
                (twin != "none").then(|| twin.to_owned()),
            )
        })
        .collect()
}

fn twin(http: &Option<Http>) -> Option<String> {
    http.map(|http| format!("{} {}", http.verb, http.path))
}

#[test]
fn the_method_table_of_the_docs_is_the_route_table() {
    let routes: Vec<_> = ROUTES
        .iter()
        .map(|route| (route.method.to_owned(), twin(&route.http)))
        .collect();
    assert_eq!(documented(), routes);
}

#[test]
fn no_method_is_routed_twice() {
    let mut methods: Vec<_> = ROUTES.iter().map(|route| route.method).collect();
    methods.sort_unstable();
    methods.dedup();
    assert_eq!(methods.len(), ROUTES.len());
}
