use lighthouse_model::ModelRef;

#[test]
fn a_reference_splits_at_its_first_slash() {
    let model = ModelRef::parse("openai/gpt-5/mini").unwrap();

    assert_eq!((model.provider, model.model), ("openai", "gpt-5/mini"));
}

#[test]
fn a_reference_needs_a_provider_and_a_model() {
    for text in ["gpt", "/gpt", "openai/", "/"] {
        assert_eq!(ModelRef::parse(text), None, "{text}");
    }
}
