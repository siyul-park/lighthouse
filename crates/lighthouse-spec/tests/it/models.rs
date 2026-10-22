//! What a project says about models: references, providers, bindings and the
//! evaluation policy; and the `model` check as the `cel` check it is.

use lighthouse_model::{Safety, Severity};
use lighthouse_process::Policy;
use lighthouse_spec::{
    CommandProtocol, Config, Evaluation, Fix, FixKind, FixPolicy, FixScope, ModelCheck,
    ModelProvider, Models, Output, Point, Projects, Routing, Select, Subject, Task,
};

const PROJECT: &str = r#"
plugins = ["core", "model-http"]

[models]
classification = "openrouter/qwen/qwen3-coder:free"
budget = 50

[models.providers.openrouter]
protocol = "openai"
url = "https://openrouter.ai/api/v1"
headers = { Authorization = "Bearer ${OPENROUTER_API_KEY}" }

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
        panic!("a url provider is a model-http provider");
    };
    assert_eq!(plugin, "model-http");
    assert_eq!(options["protocol"], "openai");
    assert_eq!(options["url"], "https://openrouter.ai/api/v1");
    assert_eq!(
        options["headers"]["Authorization"],
        "Bearer ${OPENROUTER_API_KEY}"
    );
    assert!(matches!(
        binding.provider("script"),
        Some(ModelProvider::Command { .. })
    ));
    let ModelProvider::Rpc { options, .. } = binding.provider("openai").unwrap() else {
        panic!("openai is built in");
    };
    assert_eq!(options["url"], "https://api.openai.com/v1");
    assert_eq!(
        options["headers"]["Authorization"], "Bearer ${OPENAI_API_KEY}",
        "a protocol's own service has its key by default"
    );
    assert!(binding.provider("ollama").is_none());
}

#[test]
fn a_timeout_reaches_model_http_as_a_duration_and_is_absent_without_one() {
    let ModelProvider::Rpc { options, .. } =
        provider("protocol = \"openai\"\nurl = \"https://x/v1\"\ntimeout = \"10m\"\n")
    else {
        panic!("a url provider is a model-http provider");
    };
    assert_eq!(options["timeout"], "10m");
    let ModelProvider::Rpc { options, .. } = binding().provider("openrouter").unwrap() else {
        panic!("a url provider is a model-http provider");
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
    assert!(
        !binding()
            .provider("local")
            .unwrap()
            .remote(&lighthouse_process::Policy::default())
    );
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
fn body_travels_in_the_options_and_unknown_fields_are_rejected() {
    let ModelProvider::Rpc { options, .. } = provider(
        "protocol = \"openai\"\n\
         body = { provider = { require_parameters = true }, max_tokens = 1024 }\n",
    ) else {
        panic!("a protocol provider is a model-http provider");
    };
    assert_eq!(options["body"]["provider"]["require_parameters"], true);
    assert_eq!(options["body"]["max_tokens"], 1024);
    let ModelProvider::Rpc { options, .. } = binding().provider("openrouter").unwrap() else {
        panic!("a url provider is a model-http provider");
    };
    assert!(!options.contains_key("body"));

    for removed in [
        "logprobs = true",
        "spec = \"openai\"",
        "type = \"rpc\"",
        "tasks = []",
        "stdin = \"prompt\"",
        "keyEnv = \"K\"",
        "baseUrl = \"http://x\"",
        "extraBody = { a = 1 }",
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
    assert_eq!(options["url"], "https://api.typesafe.ai");
    assert_eq!(
        options["headers"]["Authorization"],
        "Bearer ${TYPESAFE_API_KEY}"
    );
    let ModelProvider::Rpc { options, .. } =
        provider("protocol = \"openai\"\nurl = \"http://localhost:11434/v1\"\n")
    else {
        panic!("a url provider is a model-http provider");
    };
    assert!(
        !options.contains_key("headers"),
        "a local service needs no key"
    );
    let ModelProvider::Rpc { options, .. } =
        provider("protocol = \"openai\"\nheaders = { Authorization = \"Token ${MINE}\" }\n")
    else {
        panic!("a protocol provider is a model-http provider");
    };
    assert_eq!(options["headers"]["Authorization"], "Token ${MINE}");
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
        program,
        cost,
        pointer,
        ..
    } = provider(
        "protocol = \"typesafe\"\ncommand = [\"run\", \"{model}\"]\npointer = \"/result\"\nenv = { K = \"${K}\" }\n",
    )
    else {
        panic!("a command provider is a command");
    };
    assert_eq!(program.command, ["run", "{model}"]);
    assert_eq!(program.env["K"], "${K}");
    assert_eq!((cost, pointer.as_deref()), (1.0, Some("/result")));
}

#[test]
fn a_provider_with_two_reaches_or_none_is_invalid_naming_the_fields() {
    let both = invalid("protocol = \"openai\"\nurl = \"http://x\"\ncommand = [\"run\"]\n");
    assert!(both.contains("`url` and `command`"), "{both}");
    let none = invalid("protocol = \"anthropic\"\n");
    assert!(none.contains("none of the protocols"), "{none}");
    let nothing = invalid("cost = 1\n");
    assert!(
        nothing.contains("`url`, `command` or `plugin`"),
        "{nothing}"
    );
    let several = invalid("protocol = [\"openai\", \"typesafe\"]\n");
    assert!(several.contains("add a `url`"), "{several}");
    let unprotocolled = invalid("url = \"http://x\"\n");
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
            refused.contains("`typesafe` (classification) or plain text (agent)"),
            "{refused}"
        );
    }
    let pointer = invalid("command = [\"run\"]\npointer = \"/result\"\n");
    assert!(pointer.contains("has no `pointer`"), "{pointer}");
    let empty = invalid("protocol = \"typesafe\"\ncommand = []\n");
    assert!(empty.contains("`command` needs a program"), "{empty}");
}

#[test]
fn a_provider_serves_the_tasks_its_protocol_covers() {
    let serves = |text: &str| {
        let provider = provider(text);
        (
            provider.serves(Task::Classification),
            provider.serves(Task::Agent),
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
        "chat completions classify by their logprobs and run no agent"
    );
    assert_eq!(
        serves("protocol = \"typesafe\"\n"),
        (Some(true), Some(false))
    );
    assert_eq!(
        serves("protocol = [\"typesafe\", \"openai\"]\nurl = \"http://x\"\n"),
        (Some(true), Some(false))
    );
    assert_eq!(
        serves("plugin = \"model-local\"\n"),
        (None, None),
        "only the handshake of a plugin says what it classifies and whether it runs an agent"
    );
}

#[test]
fn agent_is_one_binding_of_the_project_and_unsafe_fixes_are_previewed_by_default() {
    let config = Config::parse_inline(
        "plugins = []\n[models]\nagent = [\"a/one\", \"b/two\"]\nclassification = \"a/one\"\n",
    )
    .unwrap();
    let binding = config.models(&Projects::default()).unwrap();
    assert_eq!(binding.agent, ["a/one", "b/two"]);
    let bound: Vec<_> = binding.bindings().collect();
    assert_eq!(
        bound,
        [
            (Task::Classification, "a/one"),
            (Task::Agent, "a/one"),
            (Task::Agent, "b/two")
        ]
    );
    assert!(!config.fix().unsafe_fixes);
    assert!(config.fix().verify.is_empty());
    assert!(config.fix().decisions.is_empty(), "no decision is listed");
    assert_eq!(config.fix().scope, FixScope::Files);
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
fn the_decisions_a_model_may_fix_and_the_programs_that_verify_them_are_read_and_checked() {
    let config = Config::parse_inline(
        "plugins = []\n[fix]\nunsafe = true\ndecisions = [\"design/exported-doc\", \"testing/*\"]\nscope = \"symbol\"\nverify = [[\"cargo\", \"test\", \"-q\"], { command = [\"make\", \"lint\"], timeout = \"3m\", output = \"lines\" }]\n",
    )
    .unwrap();
    assert!(config.fix().unsafe_fixes);
    assert_eq!(config.fix().decisions, ["design/exported-doc", "testing/*"]);
    assert_eq!(config.fix().scope, FixScope::Symbol);
    let verify = &config.fix().verify;
    assert_eq!(verify[0].command, ["cargo", "test", "-q"]);
    assert_eq!(verify[0].output(Point::Verify), Output::Status);
    assert_eq!(verify[0].timeout(Point::Verify).as_secs(), 600);
    assert_eq!(verify[1].output(Point::Verify), Output::Lines);
    assert_eq!(verify[1].timeout(Point::Verify).as_secs(), 180);

    for (text, wanted) in [
        ("[approval]\nfix = \"allow\"\n", "approval"),
        ("[fix]\nunsafe = \"allow\"\n", "allow"),
        ("[fix]\ntimeout = \"20m\"\n", "timeout"),
        ("[fix]\nscope = \"extent\"\n", "extent"),
        (
            "[fix]\nverify = [[]]\n",
            "`fix.verify[0]`: `command` needs a program",
        ),
        (
            "[fix]\nverify = [[\" \"]]\n",
            "`fix.verify[0]`: `command` needs a program",
        ),
        (
            "[fix]\nverify = [[\"make\", \"{file}\"]]\n",
            "`{file}` is not a placeholder of a `[fix] verify` program",
        ),
        (
            "[fix]\nverify = [{ command = [\"make\"], output = \"sarif\" }]\n",
            "`output`",
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
    let extra = invalid("plugin = \"model-local\"\nbody = { a = 1 }\n");
    assert!(extra.contains("`body`"), "{extra}");
    let headers = invalid("command = [\"run\"]\nheaders = { A = \"b\" }\n");
    assert!(
        headers.contains("`command` provider has no `headers`"),
        "{headers}"
    );
    let env = invalid("protocol = \"openai\"\nenv = { A = \"b\" }\n");
    assert!(env.contains("`url` provider has no `env`"), "{env}");
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
fn a_provider_is_remote_when_it_reaches_a_host_outside_the_machine() {
    let none = Policy::default();
    let binding = binding();
    assert!(
        binding.provider("openrouter").unwrap().remote(&none),
        "a standard API is a service outside the machine"
    );
    assert!(
        !binding.provider("script").unwrap().remote(&none),
        "a program that reaches no host stays on the machine"
    );
    assert!(
        !provider("protocol = \"openai\"\nurl = \"http://localhost:11434/v1\"\n").remote(&none),
        "a loopback url stays on the machine"
    );
    assert!(
        provider("command = [\"agent\"]\nsandbox = { network = [\"api.test\"] }\n").remote(&none),
        "a program whose sandbox lists a host leaves it"
    );
    assert!(
        !provider("command = [\"agent\"]\nenv = { KEY = \"${KEY}\" }\n").remote(&none),
        "a key does not make a program remote"
    );
    let project = Policy {
        network: vec!["api.project.test".to_owned()],
        ..Policy::default()
    };
    assert!(
        provider("command = [\"agent\"]\n").remote(&project),
        "a program under a project that lists a host may reach it"
    );
}

#[test]
fn only_a_loopback_address_or_localhost_is_this_machine() {
    let none = Policy::default();
    let at =
        |url: &str| provider(&format!("protocol = \"openai\"\nurl = \"{url}\"\n")).remote(&none);
    for local in [
        "http://localhost:11434/v1",
        "http://LOCALHOST/v1",
        "http://127.0.0.1:8080",
        "http://127.1.2.3/v1",
        "http://[::1]:8000/v1",
        "http://user:pw@127.0.0.1/v1",
    ] {
        assert!(!at(local), "{local} is on the machine");
    }
    for far in [
        "http://127.attacker.example/v1",
        "http://localhost.attacker.example/v1",
        "http://127.0.0.1.attacker.example/v1",
        "http://128.0.0.1/v1",
        "http://[2001:db8::1]/v1",
        "http://localhost@attacker.example/v1",
    ] {
        assert!(at(far), "{far} is remote");
    }
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
        safety: Safety::Unsafe,
        requires: Vec::new(),
        priority: 0,
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
        FixScope::References
    );
    assert_eq!(
        FixScope::of_fix(Some(&model(None)), FixScope::File),
        FixScope::File
    );
    let cel = |scope| Fix {
        safety: Safety::Unsafe,
        requires: Vec::new(),
        priority: 0,
        kind: FixKind::Cel {
            edits: "[]".to_owned(),
            scope,
        },
    };
    assert_eq!(
        FixScope::of_fix(Some(&cel(None)), FixScope::Files),
        FixScope::References
    );
    assert_eq!(
        FixScope::of_fix(Some(&cel(Some(FixScope::Files))), FixScope::Files),
        FixScope::Files
    );
    assert_eq!(
        FixScope::of_fix(Some(&model(Some(FixScope::Symbol))), FixScope::Files),
        FixScope::Symbol
    );
}

#[test]
fn fix_policy_is_the_fix_table_with_its_defaults() {
    let config = Config::parse_inline("plugins = []\n").unwrap();

    let policy: &FixPolicy = config.fix();

    assert!(policy.decisions.is_empty() && policy.verify.is_empty());
    assert_eq!(policy.scope, FixScope::Files);
    assert!(!policy.unsafe_fixes);
}

#[test]
fn agent_routes_list_the_agents_rules_name() {
    let models = Config::parse_inline(
        "[rules.\"d/x\"]\nlevel = \"warn\"\nagent = [\"a/one\", \"b/two\"]\n[rules.\"d/y\"]\nlevel = \"warn\"\n",
    )
    .unwrap()
    .models(&Projects::default())
    .unwrap();

    let routes: Vec<_> = models.agent_routes().collect();

    assert_eq!(
        routes,
        [("d/x", &["a/one".to_owned(), "b/two".to_owned()][..])]
    );
}

#[test]
fn a_command_provider_lists_the_hosts_it_may_reach_and_no_other_reach_has_the_sandbox() {
    let ModelProvider::Command { program, .. } = provider(
        "command = [\"agent\"]\nsandbox = { network = [\"api.test\", \"*.cdn.test\"], write = [\"~/.cache/agent\"] }\n",
    ) else {
        panic!("a command provider");
    };
    let sandbox = program.sandbox.unwrap();
    assert_eq!(sandbox.network, ["api.test", "*.cdn.test"]);
    assert_eq!(sandbox.write, ["~/.cache/agent"]);

    for (reach, wanted) in [
        (
            "protocol = \"openai\"\nurl = \"http://127.0.0.1:9\"\n",
            "a `url` provider has no `sandbox`",
        ),
        ("plugin = \"x\"\n", "a `plugin` provider has no `sandbox`"),
    ] {
        let error = invalid(&format!("{reach}sandbox = {{ network = [\"a.test\"] }}\n"));
        assert!(error.contains(wanted), "{error}");
    }
}

#[test]
fn a_host_or_place_a_provider_lists_that_cannot_mean_what_it_says_makes_it_invalid() {
    let invalid = |network: &str| {
        Config::parse_inline(&format!(
            "plugins = []\n[models.providers.p]\ncommand = [\"agent\"]\nsandbox = {{ network = {network} }}\n"
        ))
        .unwrap()
        .models(&lighthouse_spec::Projects::default())
        .unwrap()
        .invalid
        .get("p")
        .cloned()
    };

    for bad in ["\"*\"", "\"*.com\"", "\"a.test:99999\"", "\"\""] {
        let error = invalid(&format!("[{bad}]")).expect(bad);
        assert!(
            error.contains("`sandbox.network`") && error.contains(bad.trim_matches('"')),
            "{error}"
        );
    }
    assert_eq!(invalid("[\"api.test\", \"*.cdn.test:8443\"]"), None);
}

#[test]
fn a_variable_in_a_header_or_an_environment_must_be_a_name() {
    let headers =
        invalid("protocol = \"openai\"\nurl = \"http://x\"\nheaders = { A = \"${NAME\" }\n");
    assert!(headers.contains("`headers.A`"), "{headers}");
    let env = invalid("command = [\"run\"]\nenv = { A = \"${1BAD}\" }\n");
    assert!(env.contains("`env.A`"), "{env}");
    let name =
        invalid("protocol = \"openai\"\nurl = \"http://x\"\nheaders = { \"A b\" = \"c\" }\n");
    assert!(name.contains("not a header"), "{name}");
}
