//! The layers of the configuration: the project file, then the user's file and
//! the repository-local file over it. Meaning is the project's alone; the
//! environment keys are the later layer's, key by key.

use lighthouse_resource::Format;
use lighthouse_spec::{
    Approval, BUNDLED, Config, EnvironmentSpec, Layer, Layers, Models, Projects, Routing,
};
use serde_json::json;

const PROJECT: &str = r#"
plugins = ["core"]

[models]
classification = "team/small"
edit = "team/agent"
budget = 50

[models.providers.team]
command = ["team-agent", "{model}"]
env = { TOKEN = "secret-token" }

[models.providers.shared]
protocol = "openai"
baseUrl = "https://example.test/v1"

[rules."design/complexity"]
level = "warn"
edit = "team/agent"
"#;

const USER: &str = r#"
[models]
edit = "mine/agent"
routing = "round-robin"

[models.providers.team]
command = ["my-agent"]

[approval]
fix = "allow"
"#;

const LOCAL: &str = r#"
[models]
edit = "local/agent"

[rules."design/complexity"]
edit = "local/agent"
classification = ["a/one", "b/two"]
"#;

const USER_FILE: &str = "/home/me/.config/lighthouse/config.toml";
const LOCAL_FILE: &str = "/repo/.lighthouse/local.toml";

fn document(spec: &str) -> String {
    let body: toml::Table = toml::from_str(spec).unwrap();
    let envelope = json!({
        "apiVersion": "lighthouse/v1alpha1",
        "kind": "Project",
        "metadata": { "name": "demo" },
        "spec": body,
    });
    toml::to_string(&envelope).unwrap()
}

fn layers(files: &[(&str, &str)]) -> (Config, Layers) {
    let files = files
        .iter()
        .map(|(origin, text)| Layer::parse(origin, text).unwrap())
        .collect();
    Layers::build(Format::Toml, "lighthouse.toml", &document(PROJECT), files).unwrap()
}

fn models(files: &[(&str, &str)]) -> Models {
    layers(files).0.models(&Projects::default()).unwrap()
}

fn refused(origin: &str, text: &str) -> String {
    Layer::parse(origin, text).unwrap_err().to_string()
}

#[test]
fn a_meaning_key_in_a_personal_file_is_refused_naming_the_key_and_the_file() {
    for key in ["plugins", "extends", "languages", "overrides", "generated"] {
        let message = refused(LOCAL_FILE, &format!("{key} = []\n"));
        assert!(
            message.contains(&format!("`{key}`")) && message.contains(LOCAL_FILE),
            "{message}"
        );
    }
    let message = refused(LOCAL_FILE, "[fix]\ndecisions = [\"design/*\"]\n");
    assert!(
        message.contains("`fix`") && message.contains(LOCAL_FILE),
        "{message}"
    );
}

#[test]
fn a_meaning_key_of_a_rule_in_a_personal_file_is_refused_naming_the_key_and_the_file() {
    for key in ["level", "options", "generated", "evaluation"] {
        let text = format!("[rules.\"design/complexity\"]\n{key} = \"warn\"\n");
        let message = refused(USER_FILE, &text);
        assert!(
            message.contains(&format!("`rules.\"design/complexity\".{key}`"))
                && message.contains(USER_FILE),
            "{message}"
        );
    }
}

#[test]
fn a_key_that_is_nobodys_is_refused_with_the_file() {
    let message = refused(USER_FILE, "[models]\nedt = \"a/b\"\n");
    assert!(
        message.contains("edt") && message.contains(USER_FILE),
        "{message}"
    );
}

#[test]
fn the_later_layer_wins_key_by_key() {
    let bound = models(&[(USER_FILE, USER), (LOCAL_FILE, LOCAL)]);

    assert_eq!(bound.edit, ["local/agent"], "local over user over project");
    assert_eq!(bound.classification, ["team/small"], "nobody else set it");
    assert_eq!(bound.budget, 50, "the project's, kept");
    assert_eq!(bound.routing, Routing::RoundRobin, "the user's, kept");
}

#[test]
fn a_user_value_wins_over_the_project_and_loses_to_the_local_one() {
    assert_eq!(models(&[(USER_FILE, USER)]).edit, ["mine/agent"]);
    assert_eq!(models(&[(LOCAL_FILE, LOCAL)]).edit, ["local/agent"]);
    assert_eq!(
        models(&[(LOCAL_FILE, LOCAL), (USER_FILE, USER)]).edit,
        ["mine/agent"],
        "the order of the layers is what decides, not the kind of file"
    );
}

#[test]
fn a_provider_is_replaced_whole_by_the_later_layer_and_the_others_stay() {
    let bound = models(&[(USER_FILE, USER)]);

    let team = bound.provider("team").unwrap();
    let lighthouse_spec::ModelProvider::Command { argv, env, .. } = team else {
        panic!("a command provider");
    };
    assert_eq!(argv, ["my-agent"]);
    assert!(env.is_empty(), "the project's env went with its provider");
    assert!(bound.provider("shared").is_some());
}

#[test]
fn the_approval_is_the_later_layers() {
    assert_eq!(layers(&[]).0.fix().approval, Approval::Ask);
    assert_eq!(
        layers(&[(USER_FILE, USER)]).0.fix().approval,
        Approval::Allow
    );
}

#[test]
fn the_sandbox_is_on_without_hosts_and_each_key_is_the_later_layers() {
    let plain = layers(&[]).0;
    assert!(plain.sandbox().enabled);
    assert!(plain.sandbox().network.is_empty());

    let hosts = layers(&[(
        USER_FILE,
        "[sandbox]\nnetwork = [\"a.test\", \"*.b.test:443\"]\n",
    )])
    .0;
    assert!(hosts.sandbox().enabled);
    assert_eq!(hosts.sandbox().network, ["a.test", "*.b.test:443"]);

    let (off, stack) = layers(&[
        (USER_FILE, "[sandbox]\nnetwork = [\"a.test\"]\n"),
        (LOCAL_FILE, "[sandbox]\nenabled = false\n"),
    ]);
    assert!(!off.sandbox().enabled);
    assert_eq!(off.sandbox().network, ["a.test"], "keys are not joined");
    let keys: Vec<_> = stack
        .entries()
        .into_iter()
        .filter(|entry| entry.key.starts_with("sandbox."))
        .map(|entry| (entry.key, entry.value, entry.origin))
        .collect();
    assert_eq!(
        keys,
        [
            (
                "sandbox.enabled".to_owned(),
                json!(false),
                LOCAL_FILE.to_owned()
            ),
            (
                "sandbox.network".to_owned(),
                json!(["a.test"]),
                USER_FILE.to_owned()
            ),
        ]
    );
    assert!(
        layers(&[])
            .1
            .entries()
            .iter()
            .any(|entry| entry.key == "sandbox.enabled"
                && entry.value == json!(true)
                && entry.origin == BUNDLED),
        "the default is shown"
    );
}

#[test]
fn a_decision_binds_its_own_agent_over_the_projects_and_over_the_models_one() {
    let bound = models(&[(LOCAL_FILE, LOCAL)]);

    assert_eq!(bound.edit_for("design/complexity"), ["local/agent"]);
    assert_eq!(
        bound.edit_for("design/other"),
        ["local/agent"],
        "the models edit"
    );
    assert_eq!(
        bound.route("design/complexity").unwrap(),
        ["a/one", "b/two"]
    );
    let project_only = models(&[]);
    assert_eq!(project_only.edit_for("design/complexity"), ["team/agent"]);
    assert_eq!(project_only.edit_for("design/other"), ["team/agent"]);
}

#[test]
fn a_personal_binding_of_a_decision_the_project_does_not_list_enables_nothing() {
    let local = "[rules.\"design/other\"]\nedit = \"local/agent\"\n";
    let (config, _) = layers(&[(LOCAL_FILE, local)]);
    let projects = Projects::default();

    let bound = config.models(&projects).unwrap();
    let active = config
        .resolve(std::path::Path::new(""), "", &projects)
        .unwrap();

    assert_eq!(bound.edit_for("design/other"), ["local/agent"]);
    assert!(
        active
            .get("design/other")
            .is_none_or(|rule| rule.level.is_none())
    );
}

#[test]
fn every_agent_a_decision_binds_is_a_binding_to_check() {
    let bound = models(&[(LOCAL_FILE, LOCAL)]);

    let edits: Vec<_> = bound
        .bindings()
        .filter(|(task, _)| *task == lighthouse_spec::Task::Edit)
        .map(|(_, reference)| reference)
        .collect();

    assert!(bound.binds_edit());
    assert_eq!(edits, ["local/agent", "local/agent"]);
}

#[test]
fn without_personal_files_the_configuration_is_the_projects() {
    let (config, stack) = layers(&[]);
    let plain = Config::parse_as(Format::Toml, "lighthouse.toml", &document(PROJECT)).unwrap();
    let projects = Projects::default();

    assert_eq!(
        config.models(&projects).unwrap(),
        plain.models(&projects).unwrap()
    );
    assert_eq!(config.fix().approval, plain.fix().approval);
    assert!(
        stack
            .entries()
            .iter()
            .all(|entry| entry.origin == BUNDLED || entry.origin == "lighthouse.toml")
    );
}

#[test]
fn each_entry_names_the_layer_its_value_comes_from() {
    let (_, stack) = layers(&[(USER_FILE, USER), (LOCAL_FILE, LOCAL)]);
    let entries = stack.entries();
    let origin = |key: &str| {
        entries
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| (entry.origin.as_str(), entry.value.clone()))
            .unwrap_or_else(|| panic!("{key} in {entries:?}"))
    };

    assert_eq!(origin("models.explore"), (BUNDLED, json!(0.05)));
    assert_eq!(origin("models.budget"), ("lighthouse.toml", json!(50)));
    assert_eq!(origin("models.classification").0, "lighthouse.toml");
    assert_eq!(origin("models.edit"), (LOCAL_FILE, json!("local/agent")));
    assert_eq!(origin("models.routing"), (USER_FILE, json!("round-robin")));
    assert_eq!(origin("approval.fix"), (USER_FILE, json!("allow")));
    assert_eq!(origin("models.providers.shared").0, "lighthouse.toml");
    assert_eq!(origin("models.providers.team").0, USER_FILE);
    assert_eq!(
        origin("rules.\"design/complexity\".edit"),
        (LOCAL_FILE, json!("local/agent"))
    );
    assert_eq!(
        origin("rules.\"design/complexity\".classification"),
        (LOCAL_FILE, json!(["a/one", "b/two"]))
    );
}

#[test]
fn the_values_a_provider_passes_to_its_program_are_not_shown() {
    let (_, stack) = layers(&[]);
    let team = stack
        .entries()
        .into_iter()
        .find(|entry| entry.key == "models.providers.team")
        .unwrap();

    assert_eq!(team.value["env"], json!({"TOKEN": "***"}));
}

#[test]
fn the_defaults_are_the_bundled_layer() {
    let (_, stack) = Layers::build(
        Format::Toml,
        "lighthouse.toml",
        &document("plugins = [\"core\"]"),
        Vec::new(),
    )
    .unwrap();

    let bundled: Vec<_> = stack
        .entries()
        .into_iter()
        .filter(|entry| entry.origin == BUNDLED)
        .map(|entry| entry.key)
        .collect();

    assert_eq!(
        bundled,
        [
            "approval.fix",
            "models.budget",
            "models.explore",
            "models.routing",
            "sandbox.enabled",
            "sandbox.network"
        ]
    );
}

#[test]
fn the_environment_file_is_the_spec_without_an_envelope() {
    let spec = EnvironmentSpec::parse(USER_FILE, USER).unwrap();

    assert_eq!(spec.approval.fix, Some(Approval::Allow));
    assert!(spec.models.providers.contains_key("team"));
}

#[test]
fn a_model_binding_inside_an_override_is_a_load_error_naming_the_override_and_the_key() {
    for task in ["edit", "classification"] {
        let text = document(&format!(
            "plugins = [\"core\"]\n[[overrides]]\nfiles = [\"src/**\"]\n[overrides.rules.\"design/complexity\"]\nlevel = \"warn\"\n{task} = \"a/one\"\n"
        ));

        let message = Config::parse_as(Format::Toml, "lighthouse.toml", &text)
            .unwrap_err()
            .to_string();

        assert!(
            message.contains(&format!(
                "`overrides[0].rules.\"design/complexity\".{task}`"
            )),
            "{message}"
        );
    }
}

#[test]
fn an_override_without_a_binding_is_what_it_was() {
    let text = document(
        "plugins = [\"core\"]\n[[overrides]]\nfiles = [\"src/**\"]\n[overrides.rules.\"design/complexity\"]\nlevel = \"info\"\n",
    );

    assert!(Config::parse_as(Format::Toml, "lighthouse.toml", &text).is_ok());
}

const PROVIDERS: &str = r#"
[models.providers.http]
protocol = "openai"
baseUrl = "https://user:pass@api.example.test:8443/v1/chat?key=sk-secret#frag"
extraBody = { api_key = "sk-body", n = 2 }

[models.providers.rpc]
plugin = "fake-model"
options = { token = "sk-option" }

[models.providers.run]
command = ["agent", "--model", "{model}", "--flag=visible"]
env = { TOKEN = "sk-env" }
"#;

fn shown(key: &str) -> serde_json::Value {
    let (_, stack) = layers(&[(USER_FILE, PROVIDERS)]);
    stack
        .entries()
        .into_iter()
        .find(|entry| entry.key == key)
        .unwrap()
        .value
}

#[test]
fn what_may_carry_a_secret_is_not_shown() {
    let http = shown("models.providers.http");
    assert_eq!(http["baseUrl"], "https://api.example.test:8443/v1/chat");
    assert_eq!(http["extraBody"], json!({"api_key": "***", "n": "***"}));

    let rpc = shown("models.providers.rpc");
    assert_eq!(rpc["options"], json!({"token": "***"}));

    let run = shown("models.providers.run");
    assert_eq!(run["env"], json!({"TOKEN": "***"}));
    assert_eq!(
        run["command"],
        json!(["agent", "--model", "{model}", "--flag=visible"])
    );
    let all = [http, rpc, run]
        .iter()
        .map(ToString::to_string)
        .collect::<String>();
    for secret in ["sk-secret", "sk-body", "sk-option", "sk-env", "user:pass"] {
        assert!(!all.contains(secret), "{secret} in {all}");
    }
}

#[test]
fn an_error_in_a_personal_file_names_the_line_and_the_key_and_not_the_value() {
    let cases = [
        (
            "[models]\nbudget = \"sk-secret-1\"\n",
            ":2",
            "budget",
            "invalid type",
        ),
        (
            "[models]\nrouting = \"sk-secret-2\"\n",
            ":2",
            "routing",
            "unknown variant",
        ),
        (
            "[models]\nbudget = 1\nbudget = 2 # sk-secret-3\n",
            ":3",
            "",
            "duplicate",
        ),
        ("[models]\nedit = \"sk-secret-4\n", ":2", "edit", ""),
    ];
    for (text, line, key, what) in cases {
        let message = refused(USER_FILE, text);

        assert!(message.contains(&format!("{USER_FILE}{line}")), "{message}");
        assert!(message.contains(key), "{message}");
        assert!(message.contains(what), "{message}");
        assert!(!message.contains("sk-secret"), "{message}");
    }
}

#[test]
fn a_value_that_cannot_work_is_refused_with_the_file_the_line_and_the_key() {
    let explore = refused(USER_FILE, "[models]\nexplore = 7\n");
    assert!(
        explore.contains(&format!("{USER_FILE}:2")) && explore.contains("`models.explore`"),
        "{explore}"
    );

    let provider = refused(
        USER_FILE,
        "# mine\n[models.providers.both]\ncommand = [\"x\"]\nbaseUrl = \"https://u:pw-secret@h\"\n",
    );
    assert!(
        provider.contains(&format!("{USER_FILE}:2"))
            && provider.contains("`models.providers.both`")
            && !provider.contains("pw-secret"),
        "{provider}"
    );
}
