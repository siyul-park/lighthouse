//! A `command` model provider run as a model: it classifies, chats or edits.

use std::collections::BTreeMap;

use lighthouse_plugin::{
    ChatRequest, CreateChatCompletionRequest, DecisionRef, EditRequest, ModelClient, Question,
    State, SystemOneRequest,
};
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

    let command = CommandModel::new(dir.path(), "p/m", "m", &provider).unwrap();
    let answer = command.classify(&request()).unwrap();

    assert_eq!(command.manifest().id, "p/m");
    assert_eq!(answer.answers["d/x"].noul, 0.75);
}

#[test]
fn command_new_fails_a_program_that_exits_with_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let provider = provider(r#"["sh", "-c", "exit 3"]"#);

    let command = CommandModel::new(dir.path(), "p/m", "m", &provider).unwrap();

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

    assert!(CommandModel::new(dir.path(), "p/m", "m", &provider).is_none());
}

#[test]
fn command_new_gives_the_program_the_login_variables_and_no_other_of_the_environment() {
    let dir = tempfile::tempdir().unwrap();
    let provider = provider(r#"["sh", "-c", "env | cut -d= -f1 | tr '\\n' ' ' >&2; exit 1"]"#);

    let command = CommandModel::new(dir.path(), "p/m", "m", &provider).unwrap();
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

fn conversation() -> ChatRequest {
    let body: CreateChatCompletionRequest = serde_json::from_value(serde_json::json!({
        "model": "ignored",
        "messages": [
            {"role": "system", "content": "You fix code."},
            {"role": "user", "content": [{"type": "text", "text": "Fix this."}]},
        ],
    }))
    .unwrap();
    ChatRequest {
        body,
        options: serde_json::Map::new(),
    }
}

#[test]
fn a_command_without_a_protocol_gets_the_conversation_as_text_and_answers_in_text() {
    let dir = tempfile::tempdir().unwrap();
    let provider = provider_of("", r#"["sh", "-c", "cat > seen; printf 'the answer\\n'"]"#);

    let command = CommandModel::new(dir.path(), "p/m", "m", &provider).unwrap();
    let reply = command.chat(&conversation()).unwrap();

    assert_eq!(
        command.manifest().capabilities.chat.map(|c| c.supported),
        Some(true)
    );
    assert!(command.manifest().capabilities.classification.is_none());
    assert_eq!(
        reply.choices[0].message.content.as_deref(),
        Some("the answer")
    );
    assert_eq!(reply.model, "m");
    let seen = std::fs::read_to_string(dir.path().join("seen")).unwrap();
    assert_eq!(seen, "You fix code.\n\nFix this.");
}

#[test]
fn a_text_command_may_wrap_its_answer_in_json_a_pointer_leads_to() {
    let dir = tempfile::tempdir().unwrap();
    let provider = provider_of(
        "pointer = \"/result\"\n",
        r#"["sh", "-c", "cat >/dev/null; printf '{\"result\": \"inside\"}'"]"#,
    );

    let command = CommandModel::new(dir.path(), "p/m", "m", &provider).unwrap();
    let reply = command.chat(&conversation()).unwrap();

    assert_eq!(reply.choices[0].message.content.as_deref(), Some("inside"));
}

#[test]
fn a_text_command_that_prints_nothing_has_given_no_answer() {
    let dir = tempfile::tempdir().unwrap();
    let provider = provider_of("", r#"["sh", "-c", "cat >/dev/null"]"#);

    let command = CommandModel::new(dir.path(), "p/m", "m", &provider).unwrap();

    assert!(
        command
            .chat(&conversation())
            .unwrap_err()
            .to_string()
            .contains("no answer")
    );
}

#[test]
fn an_openai_command_reads_the_request_and_prints_the_completion() {
    let dir = tempfile::tempdir().unwrap();
    let printed = r#"{\"id\":\"c\",\"object\":\"chat.completion\",\"created\":1,\"model\":\"m\",\"choices\":[{\"index\":0,\"message\":{\"role\":\"assistant\",\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}"#;
    let provider = provider_of(
        "protocol = \"openai\"\n",
        &format!(r#"["sh", "-c", "cat > seen; printf '{printed}'"]"#),
    );

    let command = CommandModel::new(dir.path(), "p/m", "m", &provider).unwrap();
    let reply = command.chat(&conversation()).unwrap();

    assert_eq!(reply.choices[0].message.content.as_deref(), Some("hi"));
    let seen: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("seen")).unwrap()).unwrap();
    assert_eq!(seen["model"], "m");
    assert_eq!(seen["messages"][0]["content"], "You fix code.");
    assert!(seen.get("options").is_none());
}

#[test]
fn an_openai_command_whose_output_is_no_completion_fails() {
    let dir = tempfile::tempdir().unwrap();
    let provider = provider_of(
        "protocol = \"openai\"\n",
        r#"["sh", "-c", "cat >/dev/null; printf '{\"nope\": 1}'"]"#,
    );

    let command = CommandModel::new(dir.path(), "p/m", "m", &provider).unwrap();

    assert!(
        command
            .chat(&conversation())
            .unwrap_err()
            .to_string()
            .contains("not a chat completion")
    );
}

#[test]
fn a_model_serves_only_the_task_of_its_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let typesafe = CommandModel::new(dir.path(), "p/m", "m", &provider(r#"["true"]"#)).unwrap();
    let text = CommandModel::new(dir.path(), "p/m", "m", &provider_of("", r#"["true"]"#)).unwrap();

    assert!(
        typesafe
            .chat(&conversation())
            .unwrap_err()
            .to_string()
            .contains("does not chat")
    );
    assert!(
        text.classify(&request())
            .unwrap_err()
            .to_string()
            .contains("does not classify")
    );
}

#[test]
fn a_plain_text_command_edits_in_the_directory_it_is_given_and_keeps_what_it_said() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let provider = provider_of("", r#"["sh", "-c", "cat > prompt.txt; echo fixed 1"]"#);
    let command = CommandModel::new(home.path(), "p/m", "m", &provider).unwrap();

    let said = command
        .edit(&EditRequest {
            prompt: "make it better",
            dir: work.path(),
        })
        .unwrap();

    assert_eq!(said, "fixed 1");
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
    };

    let said = CommandModel::new(dir.path(), "p/m", "m", &chatty)
        .unwrap()
        .edit(&asked)
        .unwrap();
    let error = CommandModel::new(dir.path(), "p/m", "m", &failing)
        .unwrap()
        .edit(&asked)
        .unwrap_err();

    assert!(said.len() <= 2048, "{}", said.len());
    assert!(error.to_string().contains("exited with Some(2)"), "{error}");
}

#[test]
fn a_command_with_a_protocol_does_not_edit() {
    let dir = tempfile::tempdir().unwrap();
    let provider = provider(r#"["sh", "-c", "true"]"#);
    let command = CommandModel::new(dir.path(), "p/m", "m", &provider).unwrap();

    let error = command
        .edit(&EditRequest {
            prompt: "p",
            dir: dir.path(),
        })
        .unwrap_err();

    assert!(error.to_string().contains("does not edit"), "{error}");
}
