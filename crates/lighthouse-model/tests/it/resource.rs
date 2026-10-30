use lighthouse_model::{API_VERSION, Error, Format, Metadata, Resource, Spec, documents, resource};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Toy {
    pub(crate) per_language: Vec<String>,
}

impl Spec for Toy {
    const KIND: &'static str = "ToyKind";
}

pub(crate) fn toy() -> Resource<Toy> {
    Resource::new(
        Metadata::named("a/b"),
        Toy {
            per_language: vec!["go".to_owned()],
        },
    )
}

pub(crate) const YAML: &str = "apiVersion: lighthouse/v1alpha1\nkind: ToyKind\nmetadata:\n  name: a/b\nspec:\n  perLanguage: [go]\n";
pub(crate) const TOML: &str = "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"ToyKind\"\n[metadata]\nname = \"a/b\"\n[spec]\nperLanguage = [\"go\"]\n";
pub(crate) const JSON: &str = r#"{"apiVersion":"lighthouse/v1alpha1","kind":"ToyKind","metadata":{"name":"a/b"},"spec":{"perLanguage":["go"]}}"#;

#[test]
fn a_document_without_api_version_is_refused() {
    let docs = documents(Format::Yaml, "old.yaml", "id: core/x\n").unwrap();
    let err = resource::<Toy>("old.yaml", &docs[0]).unwrap_err();
    assert!(matches!(err, Error::Mismatch { .. }));
    assert!(err.to_string().contains("found (none)"), "{err}");
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
fn new_uid_is_a_fresh_uuid_v4_each_time() {
    let (first, second) = (lighthouse_model::new_uid(), lighthouse_model::new_uid());

    assert_ne!(first, second);
    assert!(lighthouse_model::is_uid(&first), "{first}");
}

#[test]
fn is_uid_accepts_only_the_canonical_lowercase_v4_form() {
    let uid = "5d6b1c1e-2b0e-4a43-9a3e-0f1b6f5d2a11";

    assert!(lighthouse_model::is_uid(uid));
    assert!(!lighthouse_model::is_uid(&uid.to_uppercase()));
    assert!(!lighthouse_model::is_uid(&uid.replace('-', "")));
    assert!(!lighthouse_model::is_uid(
        "6ba7b810-9dad-11d1-80b4-00c04fd430c8"
    ));
    assert!(!lighthouse_model::is_uid("core/max-lines"));
}
