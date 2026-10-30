use std::path::Path;

use lighthouse_model::{Document, Format, Position};

fn at(line: u32, col: u32) -> Position {
    Position { line, col }
}

const HEAD: &str = "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n";

#[test]
fn list_gives_the_place_each_yaml_document_names_itself() {
    let text = format!(
        "{HEAD}  name: a/one\n  uid: u1\n  labels:\n    lighthouse/pack: core\n---\n# another\n{HEAD}  name: a/two\nspec: {{}}\n---\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: other\n"
    );

    let (found, errors) = Document::list(Format::Yaml, Path::new("d.yaml"), &text);

    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(found[0].file, Path::new("d.yaml"));
    let names: Vec<_> = found.iter().map(|d| (d.name.as_str(), d.at)).collect();
    assert_eq!(names, [("a/one", at(4, 9)), ("a/two", at(13, 9))]);
    assert_eq!(found[0].uid.as_deref(), Some("u1"));
    assert_eq!(found[0].labels["lighthouse/pack"], "core");
    assert_eq!(found[1].kind, "Decision");
}

#[test]
fn list_reads_toml_and_json_and_reports_the_part_that_does_not_parse() {
    let toml =
        "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Pack\"\n[metadata]\nname = \"a/p\"\n";
    let (found, _) = Document::list(Format::Toml, Path::new("p.toml"), toml);
    assert_eq!(found[0].at, at(4, 9));

    let json = "{\n \"apiVersion\": \"lighthouse/v1alpha1\",\n \"kind\": \"Pack\",\n \"metadata\": {\n  \"name\": \"a/q\"\n }\n}";
    let (found, _) = Document::list(Format::Json, Path::new("q.json"), json);
    assert_eq!(found[0].at, at(5, 12));

    let broken = format!("{HEAD}  name: a/ok\n---\nkey: [unclosed\n");
    let (found, errors) = Document::list(Format::Yaml, Path::new("b.yaml"), &broken);
    assert_eq!(found.len(), 1);
    assert_eq!(errors.len(), 1);
}
