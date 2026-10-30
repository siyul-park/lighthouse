//! A `command` model provider run as a model: it classifies or edits.

use std::collections::BTreeMap;

use lighthouse_plugin::{DecisionRef, EditRequest, ModelClient, Question, State, SystemOneRequest};
use lighthouse_process::Policy;
use lighthouse_rpc::CommandModel;
use lighthouse_spec::Config;

fn provider(argv: &str) -> lighthouse_spec::ModelProvider {
    provider_of("protocol = \"typesafe\"\n", argv)
}

/// A command provider with the given lines of configuration before its `command`.
fn provider_of(lines: &str, argv: &str) -> lighthouse_spec::ModelProvider {
    let config = Config::parse_inline(&format!(
        "plugins = []\n[models.providers.p]\n{lines}command = {argv}\n"
    ))
    .unwrap();
    config
        .models(&lighthouse_spec::Projects::default())
        .unwrap()
        .provider("p")
        .unwrap()
}

fn request() -> SystemOneRequest {
    let decision = DecisionRef {
        id: "d/x".to_owned(),
        requirement: "r".to_owned(),
        prompt: "r".to_owned(),
        valid: Vec::new(),
        invalid: Vec::new(),
    };
    SystemOneRequest {
        state: State {
            language: "go".to_owned(),
            file: "a.go".to_owned(),
            text: String::new(),
            facts: serde_json::Value::Null,
            neighbors: Vec::new(),
        },
        model: String::new(),
        questions: BTreeMap::from([("d/x".to_owned(), Question::new(&decision))]),
        options: serde_json::Map::new(),
    }
}

#[test]
fn command_new_runs_a_program_that_prints_the_answer() {
    let dir = tempfile::tempdir().unwrap();
    let provider = provider(r#"["sh", "-c", "cat >/dev/null; printf '{\"noul\": 0.75}'"]"#);

    let command = CommandModel::new(
        &lighthouse_test_support::cel(),
        dir.path(),
        "p/m",
        "m",
        &provider,
        &Policy::default(),
    )
    .unwrap();
    let answer = command.classify(&request()).unwrap();

    assert_eq!(command.manifest().id, "p/m");
    assert_eq!(answer.answers["d/x"].noul, 0.75);
}

#[test]
fn command_new_treats_an_omitted_answer_as_abstention() {
    let dir = tempfile::tempdir().unwrap();
    let provider = provider(r#"[ "sh", "-c", "cat >/dev/null; printf '{\\\"answers\\\":{}}'" ]"#);

    let answer = CommandModel::new(
        &lighthouse_test_support::cel(),
        dir.path(),
        "p/m",
        "m",
        &provider,
        &Policy::default(),
    )
    .unwrap()
    .classify(&request())
    .unwrap();

    assert!(answer.answers.is_empty());
}

#[test]
fn command_new_rejects_an_answer_with_an_unknown_type() {
    let dir = tempfile::tempdir().unwrap();
    let provider = provider(
        r#"[ "sh", "-c", "cat >/dev/null; printf '{\\\"answers\\\":{\\\"d/x\\\":{\\\"type\\\":\\\"invalid\\\",\\\"noul\\\":0.9}}}'" ]"#,
    );

    let error = CommandModel::new(
        &lighthouse_test_support::cel(),
        dir.path(),
        "p/m",
        "m",
        &provider,
        &Policy::default(),
    )
    .unwrap()
    .classify(&request())
    .unwrap_err();

    assert!(error.to_string().contains("unknown type"), "{error}");
}

#[test]
fn command_new_fails_a_program_that_exits_with_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let provider = provider(r#"["sh", "-c", "exit 3"]"#);

    let command = CommandModel::new(
        &lighthouse_test_support::cel(),
        dir.path(),
        "p/m",
        "m",
        &provider,
        &Policy::default(),
    )
    .unwrap();

    assert!(command.classify(&request()).is_err());
}

#[test]
fn command_new_is_none_for_a_provider_that_is_not_a_command() {
    let dir = tempfile::tempdir().unwrap();
    let config =
        Config::parse_inline("plugins = []\n[models.providers.p]\nplugin = \"x\"\n").unwrap();
    let provider = config
        .models(&lighthouse_spec::Projects::default())
        .unwrap()
        .provider("p")
        .unwrap();

    assert!(
        CommandModel::new(
            &lighthouse_test_support::cel(),
            dir.path(),
            "p/m",
            "m",
            &provider,
            &Policy::default()
        )
        .is_none()
    );
}

#[test]
fn command_new_gives_the_program_the_login_variables_and_no_other_of_the_environment() {
    let dir = tempfile::tempdir().unwrap();
    let provider = provider(r#"["sh", "-c", "env | cut -d= -f1 | tr '\\n' ' ' >&2; exit 1"]"#);

    let command = CommandModel::new(
        &lighthouse_test_support::cel(),
        dir.path(),
        "p/m",
        "m",
        &provider,
        &Policy::default(),
    )
    .unwrap();
    let failed = command.classify(&request()).unwrap_err().to_string();

    for name in ["PATH", "HOME", "USER", "LOGNAME"] {
        assert_eq!(
            failed.contains(&format!("{name} ")),
            std::env::var(name).is_ok(),
            "{name}: {failed}"
        );
    }
    assert!(!failed.contains("CARGO"), "{failed}");
}

#[test]
fn a_command_without_a_protocol_serves_edit_and_neither_classifies_nor_chats() {
    let dir = tempfile::tempdir().unwrap();
    let provider = provider_of("", r#"["true"]"#);

    let command = CommandModel::new(
        &lighthouse_test_support::cel(),
        dir.path(),
        "p/m",
        "m",
        &provider,
        &Policy::default(),
    )
    .unwrap();

    assert!(command.manifest().capabilities.classification.is_none());
    assert!(
        command
            .classify(&request())
            .unwrap_err()
            .to_string()
            .contains("does not classify")
    );
}

#[test]
fn a_command_speaking_openai_is_no_provider_at_all() {
    let error = Config::parse_inline(
        "plugins = []\n[models.providers.p]\nprotocol = \"openai\"\ncommand = [\"cat\"]\n",
    )
    .unwrap()
    .models(&lighthouse_spec::Projects::default())
    .unwrap()
    .invalid["p"]
        .clone();

    assert!(
        error.contains("`typesafe` (classification) or plain text (agent)"),
        "{error}"
    );
}

#[test]
fn a_plain_text_command_edits_in_the_directory_it_is_given_and_keeps_what_it_said() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let provider = provider_of("", r#"["sh", "-c", "cat > prompt.txt; echo fixed 1"]"#);
    let command = CommandModel::new(
        &lighthouse_test_support::cel(),
        home.path(),
        "p/m",
        "m",
        &provider,
        &Policy::default(),
    )
    .unwrap();

    let said = command
        .edit(&EditRequest {
            prompt: "make it better",
            dir: work.path(),
            model: "",
        })
        .unwrap();

    assert_eq!(said.said, "fixed 1");
    let prompt = std::fs::read_to_string(work.path().join("prompt.txt")).unwrap();
    assert_eq!(prompt, "make it better");
    assert!(!home.path().join("prompt.txt").exists());
}

#[test]
fn what_an_agent_said_is_kept_to_two_kilobytes_and_a_failure_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let chatty = provider_of(
        "",
        r#"["sh", "-c", "head -c 5000 /dev/zero | tr '\\0' 'é'"]"#,
    );
    let failing = provider_of("", r#"["sh", "-c", "echo no >&2; exit 2"]"#);
    let asked = EditRequest {
        prompt: "p",
        dir: dir.path(),
        model: "",
    };

    let said = CommandModel::new(
        &lighthouse_test_support::cel(),
        dir.path(),
        "p/m",
        "m",
        &chatty,
        &Policy::default(),
    )
    .unwrap()
    .edit(&asked)
    .unwrap();
    let error = CommandModel::new(
        &lighthouse_test_support::cel(),
        dir.path(),
        "p/m",
        "m",
        &failing,
        &Policy::default(),
    )
    .unwrap()
    .edit(&asked)
    .unwrap_err();

    assert!(said.said.len() <= 2048, "{}", said.said.len());
    assert!(error.to_string().contains("exited 2"), "{error}");
}

#[test]
fn a_command_with_a_protocol_does_not_edit() {
    let dir = tempfile::tempdir().unwrap();
    let provider = provider(r#"["sh", "-c", "true"]"#);
    let command = CommandModel::new(
        &lighthouse_test_support::cel(),
        dir.path(),
        "p/m",
        "m",
        &provider,
        &Policy::default(),
    )
    .unwrap();

    let error = command
        .edit(&EditRequest {
            prompt: "p",
            dir: dir.path(),
            model: "",
        })
        .unwrap_err();

    assert!(error.to_string().contains("does not edit"), "{error}");
}

#[test]
fn an_agent_runs_with_a_home_of_its_own_and_writes_only_in_its_directory() {
    let work = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let script = format!(
        "echo $HOME > home.txt; echo $XDG_CONFIG_HOME > xdg.txt; \
         touch {}/escaped 2> denied.txt; true",
        outside.path().display()
    );
    let provider = provider_of("", &serde_json::json!(["sh", "-c", script]).to_string());
    let command = CommandModel::new(
        &lighthouse_test_support::cel(),
        work.path(),
        "p/m",
        "m",
        &provider,
        &Policy::default(),
    )
    .unwrap();

    command
        .edit(&EditRequest {
            prompt: "go",
            dir: work.path(),
            model: "",
        })
        .unwrap();

    let home = std::fs::read_to_string(work.path().join("home.txt")).unwrap();
    let xdg = std::fs::read_to_string(work.path().join("xdg.txt")).unwrap();
    let real = std::env::var("HOME").unwrap();
    assert_ne!(home.trim(), real, "the agent saw the real home");
    assert!(xdg.trim().starts_with(home.trim()), "{xdg} under {home}");
    assert!(!std::path::Path::new(home.trim()).exists(), "home removed");
    assert!(!outside.path().join("escaped").exists(), "wrote outside");
}

/// An agent that writes its key variable and its arguments into its directory
/// and says the key on stdout, configured with `env = { KEY = "${NAME}" }`.
/// A user with the one variable `name`, read by name instead of from the
/// environment of the test process.
fn user(name: &str, value: &str) -> impl Fn(&str) -> Option<String> + Send + Sync + 'static {
    let (name, value) = (name.to_owned(), value.to_owned());
    move |asked| (asked == name).then(|| value.clone())
}

fn keyed(name: &str) -> lighthouse_spec::ModelProvider {
    let script =
        "printf %s \"$KEY\" > key.txt; printf '%s' \"$*\" > argv.txt; echo \"the key is $KEY\"";
    provider_of(
        &format!("env = {{ KEY = \"${{{name}}}\" }}\n"),
        &serde_json::json!(["sh", "-c", script, "sh", "--model", "{model}"]).to_string(),
    )
}

#[test]
fn a_key_reaches_the_program_by_its_environment_and_nowhere_else() {
    let work = tempfile::tempdir().unwrap();
    let provider = keyed("LIGHTHOUSE_RPC_TEST_KEY");
    let command = CommandModel::new_in(
        &lighthouse_test_support::cel(),
        work.path(),
        "p/m",
        "m",
        &provider,
        &Policy::default(),
        user("LIGHTHOUSE_RPC_TEST_KEY", "sk-agent-secret"),
    )
    .unwrap();

    let said = command
        .edit(&EditRequest {
            prompt: "go",
            dir: work.path(),
            model: "",
        })
        .unwrap();

    let read = |name: &str| std::fs::read_to_string(work.path().join(name)).unwrap();
    assert_eq!(
        read("key.txt"),
        "sk-agent-secret",
        "the program has its key"
    );
    assert_eq!(read("argv.txt"), "--model m", "the command line has no key");
    assert_eq!(
        said.said, "the key is ${LIGHTHOUSE_RPC_TEST_KEY}",
        "the notes have its name"
    );
    assert!(!format!("{:?}", command.manifest()).contains("sk-agent-secret"));
}

#[test]
fn the_version_of_a_model_holds_the_name_of_a_key_and_a_hash_of_its_value() {
    let work = tempfile::tempdir().unwrap();
    let version = |value: &str| {
        let provider = keyed("LIGHTHOUSE_RPC_TEST_ROTATED");
        CommandModel::new_in(
            &lighthouse_test_support::cel(),
            work.path(),
            "p/m",
            "m",
            &provider,
            &Policy::default(),
            user("LIGHTHOUSE_RPC_TEST_ROTATED", value),
        )
        .unwrap()
        .manifest()
        .version
        .clone()
    };

    let first = version("sk-one");
    let again = version("sk-one");
    let rotated = version("sk-two");

    assert_eq!(first, again);
    assert_ne!(
        first, rotated,
        "another key is another version, so a cache cannot cross"
    );
}

#[test]
fn a_key_that_is_not_set_fails_the_run_naming_the_variable() {
    let work = tempfile::tempdir().unwrap();
    let provider = keyed("LIGHTHOUSE_RPC_TEST_NEVER_SET");
    let command = CommandModel::new(
        &lighthouse_test_support::cel(),
        work.path(),
        "p/m",
        "m",
        &provider,
        &Policy::default(),
    )
    .unwrap();

    let error = command
        .edit(&EditRequest {
            prompt: "go",
            dir: work.path(),
            model: "",
        })
        .unwrap_err();

    assert!(
        error.to_string().contains("LIGHTHOUSE_RPC_TEST_NEVER_SET"),
        "{error}"
    );
}
