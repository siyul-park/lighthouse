use std::fs;

use lighthouse_model::{AgentKind, Attribution, Judgment, Severity};
use lighthouse_plugin::{LabelSource, ResultKind};
use lighthouse_session::labels::{Baseline, collect};
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
    Attribution {
        kind,
        id: Some(id.to_owned()),
    }
}

#[test]
fn labels_weigh_by_who_gave_them_and_skip_other_meanings_and_the_model_itself() {
    let dir = tempfile::tempdir().unwrap();
    let decision = Catalog::bundled().decision(RULE).unwrap().clone();
    let meaning = decision.meaning_version();
    let mut store = Store::open(dir.path()).unwrap();
    let names = ["p", "a", "m", "self", "old", "na"];
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
        who(AgentKind::SoftwareAgent, "openai/gpt@v1"),
        &meaning,
    );
    judge(
        &mut store,
        "self",
        Judgment::Fail,
        who(AgentKind::SoftwareAgent, "local/x@v2"),
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
    let labels = collect(dir.path(), &decision, Some(&baseline), Some("local/x")).unwrap();
    let by = |fp: &str| labels.iter().find(|l| l.fingerprint == fp);

    let p = by("p").unwrap();
    assert_eq!(
        (p.kind, p.weight, p.source),
        (ResultKind::Fail, 1.0, LabelSource::Person)
    );
    assert_eq!(by("a").unwrap().weight, 0.7);
    let m = by("m").unwrap();
    assert_eq!((m.weight, m.source), (0.5, LabelSource::Model));
    assert!(
        by("self").is_none(),
        "a model never trains on its own labels"
    );
    assert!(by("old").is_none(), "another meaning version is no label");
    assert!(by("na").is_none());
    assert_eq!(by("b1").unwrap().source, LabelSource::Baseline);
    assert_eq!(by("b2").unwrap().kind, ResultKind::Pass);
    assert!(by("b3").is_none() && by("b4").is_none());
    assert_eq!(labels.len(), 5);
}
