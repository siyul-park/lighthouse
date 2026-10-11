use std::fs;

use lighthouse_ml::{
    labels::{Baseline, Collect, Label, Source},
    metrics::Truth,
};
use lighthouse_model::{AgentKind, Attribution, Judgment, Severity};
use lighthouse_spec::Catalog;
use lighthouse_store::{NewJudgment, Observed, Run, Stamp, Store, Unchecked};
use serde_json::json;

const RULE: &str = "core/max-lines";

fn observed(fingerprint: &str) -> Observed {
    Observed {
        fingerprint: fingerprint.to_owned(),
        rule_id: RULE.to_owned(),
        decision_uid: None,
        severity: Severity::Warn,
        authored_severity: Severity::Warn,
        path: "a.rs".to_owned(),
        locator: json!({}),
        symbol: None,
        message: "m".to_owned(),
        evidence: json!({}),
        facts: json!({}),
        options: json!({}),
        meaning_version: None,
        check_revision: None,
        decision_hash: None,
    }
}

fn judge(
    store: &mut Store,
    fingerprint: &str,
    judgment: Judgment,
    who: Attribution,
    meaning: &str,
) {
    let review = NewJudgment {
        fingerprint: fingerprint.to_owned(),
        judgment,
        reason: None,
        suppress: None,
        attribution: who,
        commit: None,
        expect_seen: None,
        lighthouse_version: "test".to_owned(),
    };
    let meaning = meaning.to_owned();
    store
        .resolve(&review, move |_| Stamp {
            meaning_version: Some(meaning.clone()),
            ..Stamp::default()
        })
        .unwrap();
}

fn who(kind: AgentKind, id: &str) -> Attribution {
    Attribution::new(kind, Some(id.to_owned()))
}

fn model(id: &str) -> Attribution {
    Attribution::model(id)
}

fn collect(
    root: &std::path::Path,
    baseline: Option<&Baseline>,
    exclude: Option<&str>,
) -> Vec<Label> {
    let catalog = Catalog::bundled().clone();
    let decision = catalog.decision(RULE).unwrap().clone();
    Collect {
        root,
        decision: &decision,
        baseline,
        exclude_model: exclude,
        examples: Vec::new(),
    }
    .labels()
    .unwrap()
}

#[test]
fn labels_weigh_by_who_gave_them_and_skip_other_meanings_and_the_model_itself() {
    let dir = tempfile::tempdir().unwrap();
    let decision = Catalog::bundled().decision(RULE).unwrap().clone();
    let meaning = decision.meaning_version();
    let mut store = Store::open(dir.path()).unwrap();
    let names = ["p", "a", "m", "self", "old", "na", "zz", "b1"];
    store
        .record(&Run {
            observed: names.iter().map(|n| observed(n)).collect(),
            reported: vec![String::new()],
            rules: vec![RULE.to_owned()],
            configured: vec![RULE.to_owned()],
            unchecked: Unchecked::Nothing,
            commit: None,
            dirty: false,
            catalog_version: None,
            lighthouse_version: "test".to_owned(),
        })
        .unwrap();
    judge(
        &mut store,
        "p",
        Judgment::Fail,
        who(AgentKind::Person, "kim"),
        &meaning,
    );
    judge(
        &mut store,
        "a",
        Judgment::Pass,
        who(AgentKind::SoftwareAgent, "claude"),
        &meaning,
    );
    judge(
        &mut store,
        "m",
        Judgment::Fail,
        model("openai/gpt@v1"),
        &meaning,
    );
    judge(
        &mut store,
        "self",
        Judgment::Fail,
        model("local/x@v2"),
        &meaning,
    );
    judge(
        &mut store,
        "old",
        Judgment::Fail,
        who(AgentKind::Person, "kim"),
        "another-meaning",
    );
    judge(
        &mut store,
        "na",
        Judgment::NotApplicable,
        who(AgentKind::Person, "kim"),
        &meaning,
    );
    // One subject judged by a person, then a coding agent, then a model: the
    // person's label stands, whoever spoke last.
    judge(
        &mut store,
        "zz",
        Judgment::Fail,
        who(AgentKind::Person, "kim"),
        &meaning,
    );
    judge(
        &mut store,
        "zz",
        Judgment::Pass,
        who(AgentKind::SoftwareAgent, "claude"),
        &meaning,
    );
    judge(
        &mut store,
        "zz",
        Judgment::Pass,
        model("openai/gpt@v1"),
        &meaning,
    );
    // A reviewed table outranks a coding agent.
    judge(
        &mut store,
        "b1",
        Judgment::Pass,
        who(AgentKind::SoftwareAgent, "claude"),
        &meaning,
    );
    drop(store);

    let table = dir.path().join("t.tsv");
    fs::write(
        &table,
        "# comment\nfingerprint\trepo\tlocation\tverdict\treason\nb1\tcel\tx\tTP\t\nb2\tcel\ty\tFP\t\nb3\tcel\tz\tunclear\t\n-\tcel\tw\tTP\t\nb4\tother\tv\tTP\t\n",
    )
    .unwrap();
    let baseline = Baseline {
        file: &table,
        repo: Some("cel"),
    };
    let labels = collect(dir.path(), Some(&baseline), Some("local/x"));
    let by = |key: &str| labels.iter().find(|l| l.key == key);

    let p = by("p").unwrap();
    assert_eq!(
        (p.truth, p.weight, p.source),
        (Truth::Fail, 1.0, Source::Person)
    );
    let a = by("a").unwrap();
    assert_eq!(
        (a.truth, a.weight, a.source),
        (Truth::Pass, 0.7, Source::Agent)
    );
    let m = by("m").unwrap();
    assert_eq!((m.weight, m.source), (0.5, Source::Model));
    assert_eq!(m.attribution["hadRole"], "model");
    assert!(
        by("self").is_none(),
        "a model never trains on its own labels"
    );
    assert!(by("old").is_none(), "another meaning version is no label");
    let na = by("na").unwrap();
    assert_eq!(
        na.truth,
        Truth::NotApplicable,
        "the selector is graded by it"
    );
    let all = by("zz").unwrap();
    assert_eq!((all.truth, all.source), (Truth::Fail, Source::Person));
    let b1 = by("b1").unwrap();
    assert_eq!((b1.truth, b1.source), (Truth::Fail, Source::Tsv));
    assert_eq!(by("b2").unwrap().truth, Truth::Pass);
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
