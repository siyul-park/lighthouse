use std::fs;

use lighthouse_ml::{
    labels::{Judged, Label, Labels, Source, Table},
    metrics::Outcome,
};
use lighthouse_model::{AgentKind, Attribution, Judgment};

const RULE: &str = "core/max-lines";

fn judged(fingerprint: &str, judgment: Judgment, by: Attribution, meaning: &str) -> Judged {
    Judged {
        fingerprint: fingerprint.to_owned(),
        meaning_version: Some(meaning.to_owned()),
        judgment,
        by,
    }
}

fn who(kind: AgentKind, id: &str) -> Attribution {
    Attribution::new(kind, Some(id.to_owned()))
}

fn model(id: &str) -> Attribution {
    Attribution::model(id)
}

fn collect(judgments: Vec<Judged>, table: Option<&Table>, exclude: Option<&str>) -> Vec<Label> {
    let catalog = lighthouse_test_support::bundled().clone();
    let decision = catalog.decision(RULE).unwrap().clone();
    Labels {
        judgments,
        decision: &decision,
        table,
        exclude_model: exclude,
        examples: Vec::new(),
    }
    .collect()
    .unwrap()
}

#[test]
fn labels_weigh_by_who_gave_them_and_skip_other_meanings_and_the_model_itself() {
    let dir = tempfile::tempdir().unwrap();
    let decision = lighthouse_test_support::bundled()
        .decision(RULE)
        .unwrap()
        .clone();
    let meaning = decision.meaning_version();
    let judge =
        |fingerprint, judgment, by, meaning: &str| judged(fingerprint, judgment, by, meaning);
    let judgments = vec![
        judge("p", Judgment::Fail, who(AgentKind::Person, "kim"), &meaning),
        judge(
            "a",
            Judgment::Pass,
            who(AgentKind::SoftwareAgent, "claude"),
            &meaning,
        ),
        judge("m", Judgment::Fail, model("openai/gpt@v1"), &meaning),
        judge("self", Judgment::Fail, model("local/x@v2"), &meaning),
        judge(
            "old",
            Judgment::Fail,
            who(AgentKind::Person, "kim"),
            "another-meaning",
        ),
        judge(
            "na",
            Judgment::NotApplicable,
            who(AgentKind::Person, "kim"),
            &meaning,
        ),
        // One subject judged by a person, then a coding agent, then a model:
        // the person's label stands, whoever spoke last.
        judge(
            "zz",
            Judgment::Fail,
            who(AgentKind::Person, "kim"),
            &meaning,
        ),
        judge(
            "zz",
            Judgment::Pass,
            who(AgentKind::SoftwareAgent, "claude"),
            &meaning,
        ),
        judge("zz", Judgment::Pass, model("openai/gpt@v1"), &meaning),
        // A reviewed table outranks a coding agent.
        judge(
            "b1",
            Judgment::Pass,
            who(AgentKind::SoftwareAgent, "claude"),
            &meaning,
        ),
    ];

    let table = dir.path().join("t.tsv");
    fs::write(
        &table,
        "# comment\nfingerprint\trepo\tlocation\tverdict\treason\nb1\tcel\tx\tTP\t\nb2\tcel\ty\tFP\t\nb3\tcel\tz\tunclear\t\n-\tcel\tw\tTP\t\nb4\tother\tv\tTP\t\n",
    )
    .unwrap();
    let table = Table {
        file: &table,
        repo: Some("cel"),
    };
    let labels = collect(judgments, Some(&table), Some("local/x"));
    let by = |key: &str| labels.iter().find(|l| l.key == key);

    let p = by("p").unwrap();
    assert_eq!(
        (p.judgment, p.source.weight(), p.source),
        (Judgment::Fail, 1.0, Source::Person)
    );
    let a = by("a").unwrap();
    assert_eq!(
        (a.judgment, a.source.weight(), a.source),
        (Judgment::Pass, 0.7, Source::Agent)
    );
    let m = by("m").unwrap();
    assert_eq!((m.source.weight(), m.source), (0.5, Source::Model));
    assert_eq!(m.attribution["hadRole"], "model");
    assert!(
        by("self").is_none(),
        "a model never trains on its own labels"
    );
    assert!(by("old").is_none(), "another meaning version is no label");
    let na = by("na").unwrap();
    assert_eq!(
        na.judgment,
        Judgment::NotApplicable,
        "the selector is graded by it"
    );
    let all = by("zz").unwrap();
    assert_eq!((all.judgment, all.source), (Judgment::Fail, Source::Person));
    let b1 = by("b1").unwrap();
    assert_eq!((b1.judgment, b1.source), (Judgment::Fail, Source::Tsv));
    assert_eq!(by("b2").unwrap().judgment, Judgment::Pass);
    assert!(by("b3").is_none() && by("b4").is_none());
    assert_eq!(labels.len(), 7);
}

#[test]
fn weight_trusts_people_and_tables_over_agents_and_models() {
    assert_eq!(Source::Person.weight(), 1.0);
    assert_eq!(Source::Tsv.weight(), 1.0);
    assert!(Source::Agent.weight() > Source::Model.weight());
}

#[test]
fn name_is_what_a_dataset_line_calls_the_source() {
    let names: Vec<_> = [
        Source::Person,
        Source::Tsv,
        Source::Agent,
        Source::Model,
        Source::Example,
    ]
    .map(Source::name)
    .into();
    assert_eq!(names, ["person", "tsv", "agent", "model", "example"]);
}

#[test]
fn label_example_is_a_subject_of_the_decision_with_the_outcome_the_check_gave() {
    let label = Label::example(
        RULE,
        "example:core/max-lines:fail:1".to_owned(),
        Judgment::Fail,
        Outcome::Fail,
        None,
    );
    assert_eq!(
        (label.judgment, label.source, label.observed),
        (Judgment::Fail, Source::Example, Some(Outcome::Fail))
    );
    assert_eq!(label.attribution["id"], format!("example:{RULE}"));
}
