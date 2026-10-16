//! What a project says about models: references, providers, bindings and the
//! evaluation policy; and the `model` check as the `cel` check it is.

use lighthouse_model::Severity;
use lighthouse_spec::{
    Config, Evaluation, ModelCheck, ModelProvider, Models, Projects, Routing, Select, Subject,
};

const PROJECT: &str = r#"
plugins = ["core", "model-http"]

[models]
classification = "openrouter/qwen/qwen3-coder:free"
budget = 50

[models.providers.openrouter]
spec = "openai"
baseUrl = "https://openrouter.ai/api/v1"
keyEnv = "OPENROUTER_API_KEY"

[models.providers.script]
type = "command"
argv = ["./scripts/classify", "{model}"]
stdin = "request"

[rules."design/prefer-method"]
level = "warn"
model = "script/sonnet"
evaluation = { precision = 0.95, recall = 0.8 }
"#;

fn binding() -> Models {
    Config::parse_inline(PROJECT)
        .unwrap()
        .models(&Projects::default())
        .unwrap()
}

#[test]
fn the_project_binds_models_by_reference_and_limits_what_they_cost() {
    let binding = binding();
    assert_eq!(binding.classification, ["openrouter/qwen/qwen3-coder:free"]);
    assert_eq!(
        binding.route("design/prefer-method").unwrap(),
        ["script/sonnet"]
    );
    assert_eq!(binding.routing, Routing::Ordered);
    assert_eq!(binding.budget, 50);
    let ids: Vec<_> = binding.ids().collect();
    assert_eq!(ids, ["openrouter/qwen/qwen3-coder:free", "script/sonnet"]);
}

const ROUTE: &str = r#"
[models]
classification = ["a/one", "b/two"]
routing = "round-robin"

[rules."design/naming"]
level = "warn"
model = ["c/three", "a/one"]
"#;

#[test]
fn a_binding_is_one_model_or_a_route_of_several_in_the_order_written() {
    let binding = Config::parse_inline(ROUTE)
        .unwrap()
        .models(&Projects::default())
        .unwrap();

    assert_eq!(binding.classification, ["a/one", "b/two"]);
    assert_eq!(
        binding.route("design/naming").unwrap(),
        ["c/three", "a/one"]
    );
    assert_eq!(binding.routing, Routing::RoundRobin);
    let ids: Vec<_> = binding.ids().collect();
    assert_eq!(ids, ["a/one", "b/two", "c/three", "a/one"]);
}

#[test]
fn an_unknown_routing_is_rejected() {
    let error = Config::parse_inline("[models]\nrouting = \"random\"\n").unwrap_err();

    assert!(error.to_string().contains("random"), "{error}");
}

#[test]
fn a_provider_is_declared_by_spec_or_by_command_and_openai_is_built_in() {
    let binding = binding();
    let ModelProvider::Rpc {
        plugin, options, ..
    } = binding.provider("openrouter").unwrap()
    else {
        panic!("a spec provider is a model-http provider");
    };
    assert_eq!(plugin, "model-http");
    assert_eq!(options["baseUrl"], "https://openrouter.ai/api/v1");
    assert!(matches!(
        binding.provider("script"),
        Some(ModelProvider::Command { .. })
    ));
    let ModelProvider::Rpc { options, .. } = binding.provider("openai").unwrap() else {
        panic!("openai is built in");
    };
    assert_eq!(options["keyEnv"], "OPENAI_API_KEY");
    assert!(binding.provider("ollama").is_none());
}

#[test]
fn local_is_built_in_and_costs_nothing_and_cannot_be_declared() {
    let ModelProvider::Rpc {
        plugin,
        options,
        cost,
    } = binding().provider("local").unwrap()
    else {
        panic!("local is a plugin's provider");
    };
    assert_eq!(plugin, "model-local");
    assert!(options.is_empty());
    assert_eq!(cost, Some(0.0));
    assert!(!binding().provider("local").unwrap().remote());
    assert!(lighthouse_spec::RESERVED_PROVIDERS.contains(&"local"));
}

#[test]
fn a_predefined_provider_needs_no_declaration_and_routes_list_the_rules_that_bind_models() {
    assert!(ModelProvider::predefined("openai").is_some());
    assert!(ModelProvider::predefined("nope").is_none());

    let binding = binding();
    let routes: Vec<_> = binding.routes().collect();

    assert_eq!(
        routes,
        [("design/prefer-method", &["script/sonnet".to_owned()][..])]
    );
}

#[test]
fn logprobs_and_extra_body_travel_in_the_options_and_unknown_fields_are_rejected() {
    let config = Config::parse_inline(
        "[models.providers.lp]\nspec = \"openai\"\nlogprobs = true\n\
         extraBody = { provider = { require_parameters = true }, max_tokens = 1024 }\n",
    )
    .unwrap();
    let ModelProvider::Rpc { options, .. } = config
        .models(&Projects::default())
        .unwrap()
        .provider("lp")
        .unwrap()
    else {
        panic!("a spec provider is a model-http provider");
    };
    assert_eq!(options["logprobs"], true);
    assert_eq!(options["extraBody"]["provider"]["require_parameters"], true);
    assert_eq!(options["extraBody"]["max_tokens"], 1024);
    let plain = binding().provider("openrouter").unwrap();
    let ModelProvider::Rpc { options, .. } = plain else {
        panic!("a spec provider is a model-http provider");
    };
    assert!(!options.contains_key("logprobs") && !options.contains_key("extraBody"));

    let unknown = "[models.providers.x]\nspec = \"openai\"\nlogprob = true\n";
    assert!(Config::parse_inline(unknown).is_err());
}

#[test]
fn a_provider_naming_an_unknown_spec_is_invalid() {
    let bad = Config::parse_inline("[models.providers.x]\nspec = \"nope\"\n").unwrap();
    assert!(
        bad.models(&Projects::default())
            .unwrap()
            .invalid
            .contains_key("x")
    );
}

#[test]
fn the_evaluation_policy_of_a_rule_defaults_to_nine_tenths_and_tolerating_misses() {
    let binding = binding();
    let policy = binding.policy("design/prefer-method");
    assert_eq!((policy.precision, policy.recall), (0.95, Some(0.8)));
    assert_eq!(binding.policy("other/rule"), Evaluation::default());
    assert_eq!(Evaluation::default().precision, 0.9);
}

#[test]
fn a_candidate_rule_is_enabled_beside_the_projects() {
    let config = Config::parse_inline("plugins = [\"core\"]\n")
        .unwrap()
        .with_rule("local/candidate", Severity::Warn);
    assert!(config.lists("local"));
    let enabled: Vec<_> = config.configured().map(|(id, _)| id).collect();
    assert_eq!(enabled, ["local/candidate"]);
}

#[test]
fn a_model_check_is_a_cel_check_that_never_fails_and_reviews_what_it_selects() {
    let check = ModelCheck {
        select: Some("symbol.name.startsWith(\"x\")".to_owned()),
        prompt: Some("Is it x?".to_owned()),
        ..ModelCheck::default()
    };
    let cel = check.as_cel(Subject::Symbol).unwrap();
    assert_eq!(cel.condition, "false");
    assert_eq!(cel.review.as_deref(), Some("symbol.name.startsWith(\"x\")"));
    assert_eq!(cel.select, Some(Select::Symbol));
    assert!(ModelCheck::default().as_cel(Subject::Project).is_none());
}

#[test]
fn model_provider_remote() {
    let binding = binding();
    assert!(
        binding.provider("openrouter").unwrap().remote(),
        "a standard API is a service outside the machine"
    );
    assert!(
        !binding.provider("script").unwrap().remote(),
        "a program that is given no key stays on the machine"
    );
}

#[test]
fn exploring_sends_a_twentieth_of_the_fails_on_unless_the_project_says() {
    assert_eq!(binding().explore, 0.05);
    let said = Config::parse_inline("plugins = []\n[models]\nexplore = 0.25\n").unwrap();
    assert_eq!(said.models(&Projects::default()).unwrap().explore, 0.25);
    for share in ["0.0", "1.0"] {
        let text = format!("plugins = []\n[models]\nexplore = {share}\n");
        assert!(Config::parse_inline(&text).is_ok(), "{share}");
    }
}

#[test]
fn a_share_to_explore_outside_zero_and_one_is_a_configuration_error() {
    for share in ["1.5", "-0.1"] {
        let text = format!("plugins = []\n[models]\nexplore = {share}\n");
        let error = Config::parse_inline(&text).unwrap_err().to_string();
        assert!(error.contains("`models.explore` is"), "{error}");
        assert!(error.contains("not a share from 0 to 1"), "{error}");
    }
}
