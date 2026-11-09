use lighthouse_graph::{Graph, Options};
use lighthouse_model::ucm::{File, Fragment};

use super::*;
use crate::graph::set_units;

fn fragment(path: &str) -> Fragment {
    Fragment {
        files: vec![File {
            path: path.into(),
            lang: "rust".to_owned(),
            generated: false,
            test: false,
            facts: Default::default(),
        }],
        ..Fragment::default()
    }
}

#[test]
fn a_file_is_found_whichever_way_the_model_lists_the_paths() {
    let mut graph = Graph::in_memory(Options::default()).unwrap();
    let paths = ["a.rs", "a/b.rs", "a/c.rs", "b.rs"];
    set_units(&mut graph, &paths.map(fragment)).unwrap();
    let stated = Stated::of(&graph).unwrap();

    for path in paths {
        assert!(stated.file(Path::new(path)).is_some(), "{path}");
    }
}
