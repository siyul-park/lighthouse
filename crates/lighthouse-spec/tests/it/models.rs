//! What a project says about models: references, providers, bindings and the
//! evaluation policy; and the `model` check as the `cel` check it is.

use lighthouse_model::{Safety, Severity};
use lighthouse_spec::{
    Approval, CommandProtocol, Config, Evaluation, Fix, FixKind, FixScope, ModelCheck,
    ModelProvider, Models, Projects, Routing, Select, Subject, Task,
};

const PROJECT: &str = r#"
plugins = ["core", "model-http"]

[models]
classification = "openrouter/qwen/qwen3-coder:free"
budget = 50

[models.providers.openrouter]
protocol = "openai"
baseUrl = "https://openrouter.ai/api/v1"
keyEnv = "OPENROUTER_API_KEY"

[models.providers.script]
protocol = "typesafe"
command = ["./scripts/classify", "{model}"]

[rules."design/prefer-method"]
level = "warn"
classification = "script/sonnet"
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
    let ids: Vec<_> = binding.bindings().map(|(_, id)| id).collect();
    assert_eq!(ids, ["openrouter/qwen/qwen3-coder:free", "script/sonnet"]);
}

const ROUTE: &str = r#"
[models]
classification = ["a/one", "b/two"]
routing = "round-robin"

[rules."design/naming"]
level = "warn"
classification = ["c/three", "a/one"]
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
    let ids: Vec<_> = binding.bindings().map(|(_, id)| id).collect();
    assert_eq!(ids, ["a/one", "b/two", "c/three", "a/one"]);
}

#[test]
fn an_unknown_routing_is_rejected() {
    let error = Config::parse_inline("[models]\nrouting = \"random\"\n").unwrap_err();

    assert!(error.to_string().contains("random"), "{error}");
}

#[test]
fn a_provider_is_reached_by_url_command_or_plugin_and_openai_is_built_in() {
    let binding = binding();
    let ModelProvider::Rpc {
        plugin, options, ..
    } = binding.provider("openrouter").unwrap()
    else {
        panic!("a baseUrl provider is a model-http provider");
    };
    assert_eq!(plugin, "model-http");
    assert_eq!(options["protocol"], "openai");
    assert_eq!(options["baseUrl"], "https://openrouter.ai/api/v1");
    assert!(matches!(
        binding.provider("script"),
        Some(ModelProvider::Command { .. })
    ));
    let ModelProvider::Rpc { options, .. } = binding.provider("openai").unwrap() else {
        panic!("openai is built in");
    };
    assert_eq!(options["baseUrl"], "https://api.openai.com/v1");
    assert_eq!(options["keyEnv"], "OPENAI_API_KEY");
    assert!(binding.provider("ollama").is_none());
}

#[test]
fn a_timeout_reaches_model_http_as_a_duration_and_is_absent_without_one() {
    let ModelProvider::Rpc { options, .. } =
        provider("protocol = \"openai\"\nbaseUrl = \"https://x/v1\"\ntimeout = \"10m\"\n")
    else {
        panic!("a baseUrl provider is a model-http provider");
    };
    assert_eq!(options["timeout"], "10m");
    let ModelProvider::Rpc { options, .. } = binding().provider("openrouter").unwrap() else {
        panic!("a baseUrl provider is a model-http provider");
    };
    assert!(!options.contains_key("timeout"));
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

fn provider(text: &str) -> ModelProvider {
    Config::parse_inline(&format!("[models.providers.p]\n{text}"))
        .unwrap()
        .models(&Projects::default())
        .unwrap()
        .provider("p")
        .unwrap()
}

/// Why `[models.providers.p]` with `text` cannot be a provider.
fn invalid(text: &str) -> String {
    let models = Config::parse_inline(&format!("[models.providers.p]\n{text}"))
        .unwrap()
        .models(&Projects::default())
        .unwrap();
    models.invalid["p"].clone()
}

#[test]
fn extra_body_travels_in_the_options_and_unknown_fields_are_rejected() {
    let ModelProvider::Rpc { options, .. } = provider(
        "protocol = \"openai\"\n\
         extraBody = { provider = { require_parameters = true }, max_tokens = 1024 }\n",
    ) else {
        panic!("a protocol provider is a model-http provider");
    };
    assert_eq!(options["extraBody"]["provider"]["require_parameters"], true);
    assert_eq!(options["extraBody"]["max_tokens"], 1024);
    let ModelProvider::Rpc { options, .. } = binding().provider("openrouter").unwrap() else {
        panic!("a baseUrl provider is a model-http provider");
    };
    assert!(!options.contains_key("extraBody"));

    for removed in [
        "logprobs = true",
        "spec = \"openai\"",
        "type = \"rpc\"",
        "tasks = []",
        "stdin = \"prompt\"",
    ] {
        let text = format!("[models.providers.x]\nprotocol = \"openai\"\n{removed}\n");
        assert!(Config::parse_inline(&text).is_err(), "{removed}");
    }
}

#[test]
fn a_protocol_alone_is_http_at_its_own_service_and_a_key_is_only_defaulted_there() {
    let ModelProvider::Rpc { options, .. } = provider("protocol = \"typesafe\"\n") else {
        panic!("a protocol provider is a model-http provider");
    };
    assert_eq!(options["baseUrl"], "https://api.typesafe.ai");
    assert_eq!(options["keyEnv"], "TYPESAFE_API_KEY");
    let ModelProvider::Rpc { options, .. } =
        provider("protocol = \"openai\"\nbaseUrl = \"http://localhost:11434/v1\"\n")
    else {
        panic!("a baseUrl provider is a model-http provider");
    };
    assert!(
        !options.contains_key("keyEnv"),
        "a local service needs no key"
    );
}

#[test]
fn a_plugin_provider_takes_options_and_a_command_provider_its_own_fields() {
    let ModelProvider::Rpc {
        plugin, options, ..
    } = provider("plugin = \"model-local\"\noptions = { tag = 1 }\ncost = 0\n")
    else {
        panic!("a plugin provider is an rpc provider");
    };
    assert_eq!(
        (plugin.as_str(), options["tag"].as_i64()),
        ("model-local", Some(1))
    );
    let ModelProvider::Command {
        argv,
        cost,
        pointer,
        key_env,
        ..
    } = provider(
        "protocol = \"typesafe\"\ncommand = [\"run\", \"{model}\"]\npointer = \"/result\"\nkeyEnv = \"K\"\n",
    )
    else {
        panic!("a command provider is a command");
    };
    assert_eq!(argv, ["run", "{model}"]);
    assert_eq!(
        (cost, pointer.as_deref(), key_env.as_deref()),
        (1.0, Some("/result"), Some("K"))
    );
}

#[test]
fn a_provider_with_two_reaches_or_none_is_invalid_naming_the_fields() {
    let both = invalid("protocol = \"openai\"\nbaseUrl = \"http://x\"\ncommand = [\"run\"]\n");
    assert!(both.contains("`baseUrl` and `command`"), "{both}");
    let none = invalid("protocol = \"anthropic\"\n");
    assert!(none.contains("none of the protocols"), "{none}");
    let nothing = invalid("cost = 1\n");
    assert!(
        nothing.contains("`baseUrl`, `command` or `plugin`"),
        "{nothing}"
    );
    let several = invalid("protocol = [\"openai\", \"typesafe\"]\n");
    assert!(several.contains("add a `baseUrl`"), "{several}");
    let unprotocolled = invalid("baseUrl = \"http://x\"\n");
    assert!(
        unprotocolled.contains("needs a `protocol`"),
        "{unprotocolled}"
    );
}

#[test]
fn a_command_speaks_typesafe_or_plain_text_and_nothing_else() {
    let protocol = |text: &str| match provider(text) {
        ModelProvider::Command { protocol, .. } => protocol,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        protocol("command = [\"run\"]\n"),
        CommandProtocol::PlainText
    );
    assert_eq!(
        protocol("protocol = \"typesafe\"\ncommand = [\"run\"]\n"),
        CommandProtocol::TypeSafe
    );
    for text in [
        "protocol = \"anthropic\"\ncommand = [\"run\"]\n",
        "protocol = \"openai\"\ncommand = [\"run\"]\n",
        "protocol = [\"openai\", \"typesafe\"]\ncommand = [\"run\"]\n",
    ] {
        let refused = invalid(text);
        assert!(
            refused.contains("`typesafe` (classification) or plain text (edit)"),
            "{refused}"
        );
    }
    let pointer = invalid("command = [\"run\"]\npointer = \"/result\"\n");
    assert!(pointer.contains("has no `pointer`"), "{pointer}");
    let empty = invalid("protocol = \"typesafe\"\ncommand = []\n");
    assert!(empty.contains("`command` is empty"), "{empty}");
}

#[test]
fn a_provider_serves_the_tasks_its_protocol_covers() {
    let serves = |text: &str| {
        let provider = provider(text);
        (
            provider.serves(Task::Classification),
            provider.serves(Task::Edit),
        )
    };
    let command = |protocol: &str| format!("{protocol}command = [\"run\"]\n");
    assert_eq!(
        serves(&command("protocol = \"typesafe\"\n")),
        (Some(true), Some(false))
    );
    assert_eq!(serves(&command("")), (Some(false), Some(true)));
    assert_eq!(
        serves("protocol = \"openai\"\n"),
        (Some(true), Some(false)),
        "chat completions classify by their logprobs and edit nothing"
    );
    assert_eq!(
        serves("protocol = \"typesafe\"\n"),
        (Some(true), Some(false))
    );
    assert_eq!(
        serves("protocol = [\"typesafe\", \"openai\"]\nbaseUrl = \"http://x\"\n"),
        (Some(true), Some(false))
    );
    assert_eq!(
        serves("plugin = \"model-local\"\n"),
        (None, Some(false)),
        "only the handshake of a plugin says what it classifies, and it never edits"
    );
}

#[test]
fn edit_is_one_binding_of_the_project_and_fixes_are_asked_for_and_previewed_by_default() {
    let config = Config::parse_inline(
        "plugins = []\n[models]\nedit = [\"a/one\", \"b/two\"]\nclassification = \"a/one\"\n",
    )
    .unwrap();
    let binding = config.models(&Projects::default()).unwrap();
    assert_eq!(binding.edit, ["a/one", "b/two"]);
    let bound: Vec<_> = binding.bindings().collect();
    assert_eq!(
        bound,
        [
            (Task::Classification, "a/one"),
            (Task::Edit, "a/one"),
            (Task::Edit, "b/two")
        ]
    );
    assert_eq!(config.approval(), Approval::Ask);
    assert!(config.verify().is_empty());
    assert!(config.fix_decisions().is_empty(), "no decision is listed");
    assert_eq!(config.fix_scope(), FixScope::Files);
}

#[test]
fn chat_is_no_binding_and_a_rule_binds_classification_not_model() {
    let chat = Config::parse_inline("plugins = []\n[models]\nchat = \"a/one\"\n")
        .unwrap_err()
        .to_string();
    assert!(chat.contains("chat"), "{chat}");

    let bound = Config::parse_inline(
        "plugins = []\n[rules]\n\"d/x\" = { level = \"warn\", classification = \"a/one\" }\n",
    )
    .unwrap();
    let models = bound.models(&Projects::default()).unwrap();
    assert_eq!(models.route("d/x"), Some(&["a/one".to_owned()][..]));

    let old = Config::parse_inline(
        "plugins = []\n[rules]\n\"d/x\" = { level = \"warn\", model = \"a/one\" }\n",
    )
    .unwrap_err()
    .to_string();
    assert!(old.contains("model"), "{old}");
}

#[test]
fn approval_the_decisions_a_model_may_fix_and_the_commands_that_verify_them_are_read_and_checked() {
    let config = Config::parse_inline(
        "plugins = []\n[approval]\nfix = \"allow\"\n[fix]\ndecisions = [\"design/exported-doc\", \"testing/*\"]\nscope = \"symbol\"\nverify = [[\"cargo\", \"test\", \"-q\"]]\n",
    )
    .unwrap();
    assert_eq!(config.approval(), Approval::Allow);
    assert_eq!(config.fix_decisions(), ["design/exported-doc", "testing/*"]);
    assert_eq!(config.fix_scope(), FixScope::Symbol);
    assert_eq!(config.verify(), [["cargo", "test", "-q"]]);
    assert_eq!(config.verify_timeout(), std::time::Duration::from_secs(600));
    let slow = Config::parse_inline("plugins = []\n[fix]\ntimeout = \"20m\"\n").unwrap();
    assert_eq!(slow.verify_timeout(), std::time::Duration::from_secs(1200));

    for (text, wanted) in [
        ("[approval]\nfix = \"always\"\n", "always"),
        ("[approval]\nfix = \"verified\"\n", "verified"),
        ("[approval]\nfix = \"preview\"\n", "preview"),
        ("[fix]\nscope = \"extent\"\n", "extent"),
        ("[approval]\nminor = \"gate\"\n", "minor"),
        ("[fix]\nverify = [[]]\n", "`fix.verify[0]` needs a program"),
        (
            "[fix]\nverify = [[\" \"]]\n",
            "`fix.verify[0]` needs a program",
        ),
        ("[fix]\ncommands = []\n", "commands"),
    ] {
        let error = Config::parse_inline(&format!("plugins = []\n{text}"))
            .unwrap_err()
            .to_string();
        assert!(error.contains(wanted), "{text}: {error}");
    }
}

#[test]
fn a_field_of_another_reach_is_refused() {
    let timeout = invalid("plugin = \"model-local\"\ntimeout = \"5s\"\n");
    assert!(
        timeout.contains("`plugin` provider has no `timeout`"),
        "{timeout}"
    );
    let options = invalid("protocol = \"typesafe\"\ncommand = [\"run\"]\noptions = { a = 1 }\n");
    assert!(
        options.contains("`command` provider has no `options`"),
        "{options}"
    );
    let protocol = invalid("plugin = \"model-local\"\nprotocol = \"openai\"\n");
    assert!(
        protocol.contains("`plugin` provider has no `protocol`"),
        "{protocol}"
    );
    let extra = invalid("plugin = \"model-local\"\nextraBody = { a = 1 }\n");
    assert!(extra.contains("`extraBody`"), "{extra}");
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
    assert_eq!(
        cel.abstain.as_deref(),
        Some("symbol.name.startsWith(\"x\")")
    );
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

#[test]
fn fix_scope_of_fix_is_the_projects_cap_unless_the_decision_declares_a_narrower_scope() {
    let model = |scope| Fix {
        safety: Safety::Suggested,
        requires: Vec::new(),
        kind: FixKind::Model {
            prompt: None,
            scope,
        },
    };

    assert_eq!(FixScope::of_fix(None, FixScope::Files), FixScope::Files);
    assert_eq!(FixScope::of_fix(None, FixScope::Symbol), FixScope::Symbol);
    assert_eq!(
        FixScope::of_fix(Some(&model(Some(FixScope::Files))), FixScope::Files),
        FixScope::Files
    );
    assert_eq!(
        FixScope::of_fix(Some(&model(Some(FixScope::Files))), FixScope::Symbol),
        FixScope::Symbol
    );
    assert_eq!(
        FixScope::of_fix(Some(&model(None)), FixScope::Files),
        FixScope::Files
    );
    assert_eq!(
        FixScope::of_fix(Some(&model(Some(FixScope::Symbol))), FixScope::Files),
        FixScope::Symbol
    );
}
