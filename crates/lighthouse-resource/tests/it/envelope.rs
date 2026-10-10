use std::path::Path;

use lighthouse_resource::{
    API_VERSION, Descriptor, Error, Format, Metadata, Resource, Spec, documents, header, kind_of,
    resource, schema, schema_file, to_document, to_yaml,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Toy {
    per_language: Vec<String>,
}

impl Spec for Toy {
    const KIND: &'static str = "ToyKind";
}

fn toy() -> Resource<Toy> {
    Resource::new(
        Metadata::named("a/b"),
        Toy {
            per_language: vec!["go".to_owned()],
        },
    )
}

const YAML: &str = "apiVersion: lighthouse/v1alpha1\nkind: ToyKind\nmetadata:\n  name: a/b\nspec:\n  perLanguage: [go]\n";
const TOML: &str = "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"ToyKind\"\n[metadata]\nname = \"a/b\"\n[spec]\nperLanguage = [\"go\"]\n";
const JSON: &str = r#"{"apiVersion":"lighthouse/v1alpha1","kind":"ToyKind","metadata":{"name":"a/b"},"spec":{"perLanguage":["go"]}}"#;

#[test]
fn one_resource_reads_from_yaml_toml_and_json() {
    for (format, text) in [
        (Format::Yaml, YAML),
        (Format::Toml, TOML),
        (Format::Json, JSON),
    ] {
        let docs = documents(format, "toy", text).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(kind_of(&docs[0]), Some("ToyKind"));
        assert_eq!(resource::<Toy>("toy", &docs[0]).unwrap(), toy());
    }
}

#[test]
fn yaml_may_hold_several_documents_and_skips_empty_ones() {
    let text = format!("# only a comment\n---\n{YAML}---\n---\n{YAML}");
    assert_eq!(documents(Format::Yaml, "toy", &text).unwrap().len(), 2);
}

#[test]
fn a_document_without_api_version_is_refused() {
    let docs = documents(Format::Yaml, "old.yaml", "id: core/x\n").unwrap();
    let err = resource::<Toy>("old.yaml", &docs[0]).unwrap_err();
    assert!(matches!(err, Error::Unversioned { .. }));
    assert!(err.to_string().contains("no `apiVersion`"));
}

#[test]
fn another_kind_or_version_is_a_mismatch_and_unknown_fields_are_refused() {
    let other =
        json!({"apiVersion": API_VERSION, "kind": "Other", "metadata": {"name": "x"}, "spec": {}});
    assert!(matches!(
        resource::<Toy>("p", &other),
        Err(Error::Mismatch { .. })
    ));
    let typo = json!({"apiVersion": API_VERSION, "kind": "ToyKind", "metadata": {"name": "x"}, "spec": {"perLanguage": [], "extra": 1}});
    assert!(matches!(
        resource::<Toy>("p", &typo),
        Err(Error::Invalid { .. })
    ));
}

#[test]
fn written_yaml_reads_back() {
    let text = to_yaml(&serde_json::to_value(toy()).unwrap());
    assert_eq!(text, YAML);
}

#[test]
fn schema_describes_the_envelope_and_names_its_file() {
    let schema = Descriptor::of::<Toy>().schema;
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(schema["properties"]["kind"]["const"], "ToyKind");
    assert_eq!(schema["properties"]["apiVersion"]["const"], API_VERSION);
    assert_eq!(
        schema_file("DecisionOverride"),
        "decision-override.schema.json"
    );
    assert_eq!(
        header("Decision", "../schema/"),
        "# yaml-language-server: $schema=../schema/decision.schema.json\n"
    );
}

#[test]
fn durations_need_a_unit() {
    use lighthouse_resource::parse_duration;
    use std::time::Duration;
    assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
    assert_eq!(parse_duration("2m"), Some(Duration::from_secs(120)));
    assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
    assert_eq!(parse_duration("1h"), Some(Duration::from_secs(3600)));
    assert_eq!(parse_duration("30"), None);
    assert_eq!(parse_duration("s"), None);
    assert_eq!(parse_duration("3d"), None);
}

#[test]
fn the_format_follows_the_extension() {
    assert_eq!(Format::of_path(Path::new("a/b.yaml")), Some(Format::Yaml));
    assert_eq!(Format::of_path(Path::new("b.yml")), Some(Format::Yaml));
    assert_eq!(
        Format::of_path(Path::new("lighthouse.toml")),
        Some(Format::Toml)
    );
    assert_eq!(Format::of_path(Path::new("x.json")), Some(Format::Json));
    assert_eq!(Format::of_path(Path::new("x.md")), None);
    assert_eq!(Format::of_path(Path::new("Makefile")), None);
}

#[test]
fn a_document_is_yaml_under_the_comment_that_names_its_schema() {
    let text = to_document(&toy(), "../schema");

    assert_eq!(
        text,
        format!("# yaml-language-server: $schema=../schema/toy-kind.schema.json\n{YAML}")
    );
}

#[test]
fn the_schema_of_a_spec_is_the_schema_of_its_document() {
    let described = schema::<Toy>();

    assert_eq!(described, Descriptor::of::<Toy>().schema);
    assert_eq!(described["title"], "ToyKind");
    assert!(
        described["$id"]
            .as_str()
            .unwrap()
            .ends_with("/toy-kind.schema.json")
    );
}

#[test]
fn new_uid_is_a_fresh_uuid_v4_each_time() {
    let (first, second) = (
        lighthouse_resource::new_uid(),
        lighthouse_resource::new_uid(),
    );

    assert_ne!(first, second);
    assert!(lighthouse_resource::is_uid(&first), "{first}");
}

#[test]
fn is_uid_accepts_only_the_canonical_lowercase_v4_form() {
    let uid = "5d6b1c1e-2b0e-4a43-9a3e-0f1b6f5d2a11";

    assert!(lighthouse_resource::is_uid(uid));
    assert!(!lighthouse_resource::is_uid(&uid.to_uppercase()));
    assert!(!lighthouse_resource::is_uid(&uid.replace('-', "")));
    assert!(!lighthouse_resource::is_uid(
        "6ba7b810-9dad-11d1-80b4-00c04fd430c8"
    ));
    assert!(!lighthouse_resource::is_uid("core/max-lines"));
}
