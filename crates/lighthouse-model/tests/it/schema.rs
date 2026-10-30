use lighthouse_model::{API_VERSION, Schema, header, schema, schema_file};

use crate::resource::Toy;

#[test]
fn schema_describes_the_envelope_and_names_its_file() {
    let schema = Schema::new::<Toy>().schema;
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
fn the_schema_of_a_spec_is_the_schema_of_its_document() {
    let described = schema::<Toy>();

    assert_eq!(described, Schema::new::<Toy>().schema);
    assert_eq!(described["title"], "ToyKind");
    assert!(
        described["$id"]
            .as_str()
            .unwrap()
            .ends_with("/toy-kind.schema.json")
    );
}
