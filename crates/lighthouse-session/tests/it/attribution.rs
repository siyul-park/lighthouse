//! `--attributed-to person|agent[:<id>]`. Every case passes the spec, so the
//! environment variable never decides here.

use lighthouse_session::{AgentKind, Attribution, attribute_to};

fn who(spec: &str, default: &Attribution) -> Attribution {
    attribute_to(Some(spec), default).unwrap()
}

fn dana() -> Attribution {
    Attribution::new(AgentKind::Person, Some("dana".to_owned()))
}

#[test]
fn a_kind_and_an_id_name_who() {
    let agent = who("agent:claude", &dana());
    assert_eq!(agent.kind, AgentKind::SoftwareAgent);
    assert_eq!(agent.id.as_deref(), Some("claude"));
    let person = who("person:ana", &dana());
    assert_eq!(person.kind, AgentKind::Person);
    assert_eq!(person.id.as_deref(), Some("ana"));
}

#[test]
fn a_kind_alone_keeps_the_id_of_a_default_of_that_kind() {
    assert_eq!(who("person", &dana()).id.as_deref(), Some("dana"));
    assert_eq!(who("person:", &dana()).id.as_deref(), Some("dana"));
    assert_eq!(who("agent", &dana()).id, None);
}

#[test]
fn another_kind_is_refused_and_says_what_is_expected() {
    let error = attribute_to(Some("robot:r2"), &dana())
        .unwrap_err()
        .to_string();
    assert!(error.contains("is not person or agent"), "{error}");
}
