//! The probes the tests read facts with: a fact of a file, and the fields of
//! the nodes of a list.

use lighthouse_checks::probe;
use lighthouse_model::ucm::{Applicability, File, Model, Project};
use lighthouse_plugin::{Annotations, Context, NoModels, NoQueries, Notices, Text, Workspace};
use serde_json::json;

use crate::support::World;

fn world() -> (World, Project) {
    let mut w = World::default();
    w.func("m", "a", "m/a.ucm");
    let project = w.project();
    (w, project)
}

#[test]
fn file_fact() {
    let (_, project) = world();
    let file: File = project
        .file(std::path::Path::new("m/a.ucm"))
        .unwrap()
        .clone();
    let ws = Workspace::new(".");
    let facts = Default::default();
    let notices = Notices::default();
    let given = Text::given("");
    let ctx = Context {
        workspace: &ws,
        model: &project,
        file: Some((&file, &given)),
        facts: &facts,
        trusted: false,
        annotations: &Annotations::default(),
        notices: &notices,
        applies: Applicability::default(),
        models: &NoModels,
        batch: &Default::default(),
        documents: &[],
        queries: &NoQueries,
    };

    let pair = probe::file_fact(
        &lighthouse_test_support::cel(),
        (&ctx, &project),
        &file,
        "",
        "pair",
    )
    .unwrap();

    assert_eq!(pair, json!(null));
}

#[test]
fn node_fields() {
    let (_, project) = world();
    let symbol = project.symbols.iter().find(|s| s.name == "a").unwrap();

    let fields = probe::node_fields(&project, symbol);

    assert!(fields.contains(&"name".to_owned()));
    assert!(fields.contains(&"file".to_owned()));
}
