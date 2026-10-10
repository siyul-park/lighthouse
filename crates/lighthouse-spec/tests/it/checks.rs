//! The providers a `check:` block can name, and what the types around them say.

use lighthouse_spec::{
    Batch, BuiltinCheck, Catalog, Check, CheckKind, CheckOutput, CommandCheck, Decision,
    DecisionStatus, ExitCodes, ModelCheck, NamedRule,
};
use lighthouse_test_support::catalog::*;
use serde_json::json;

fn check(value: serde_json::Value) -> Result<Check, serde_json::Error> {
    serde_json::from_value(value)
}

fn bundled(id: &str) -> &'static Decision {
    Catalog::bundled().decision(id).unwrap()
}

#[test]
fn check_is_automated_unless_a_model_answers_it() {
    let cel = check(json!({"type":"cel","select":"file","where":"true","message":"m"})).unwrap();
    assert!(cel.is_automated());
    let model = Check::of(CheckKind::Model(ModelCheck::default()));
    assert!(!model.is_automated());
}

#[test]
fn check_kind_deterministic_is_false_only_for_a_model() {
    let named = CheckKind::Builtin(BuiltinCheck::Named(NamedRule { id: "p/a".into() }));
    assert!(named.deterministic());
    assert!(!CheckKind::Model(ModelCheck::default()).deterministic());
}

#[test]
fn check_kind_label_names_the_operation_of_a_standard_builtin() {
    let order = bundled("design/declaration-groups").check.as_ref().unwrap();
    assert_eq!(order.kind.label(), "order");
    let proximity = bundled("design/contiguity").check.as_ref().unwrap();
    assert_eq!(proximity.kind.label(), "proximity");
    let cel = bundled("core/max-lines").check.as_ref().unwrap();
    assert_eq!(cel.kind.label(), "cel");
}

#[test]
fn builtin_check_named_is_the_id_of_a_registered_rule_only() {
    let named = BuiltinCheck::Named(NamedRule {
        id: "core/no-unused-allow".into(),
    });
    assert_eq!(named.named(), Some("core/no-unused-allow"));
    let order = bundled("design/declaration-groups").check.as_ref().unwrap();
    let CheckKind::Builtin(op) = &order.kind else {
        panic!("builtin expected");
    };
    assert_eq!(op.named(), None);
}

#[test]
fn exit_codes_default_to_zero_clean_and_one_findings() {
    let codes = ExitCodes::default();
    assert_eq!(codes.clean, [0]);
    assert_eq!(codes.findings, [1]);
}

#[test]
fn command_check_reads_argv_batch_and_exit_codes_and_refuses_strays() {
    let ok = check(json!({
        "type": "command", "argv": ["lint", "{files}"], "batch": "all",
        "exitCodes": {"clean": [0], "findings": [2]}, "timeout": "5s"
    }))
    .unwrap();
    let CheckKind::Command(CommandCheck {
        batch, exit_codes, ..
    }) = &ok.kind
    else {
        panic!("command expected");
    };
    assert_eq!(*batch, Batch::All);
    assert_eq!(exit_codes.findings, [2]);
    assert!(check(json!({"type":"command","argv":["x"],"where":"true"})).is_err());
    assert!(check(json!({"type":"command","argv":["x"],"scope":"file"})).is_err());
}

#[test]
fn command_check_select_belongs_to_sarif_output_and_names_sarif_levels() {
    let load = |fields: &str| {
        let spec = format!(
            "  severity: error\n  check:\n    type: command\n    argv: [lint]\n{fields}  examples:\n    - name: bad\n      language: go\n      kind: invalid\n      files: [{{ path: a.go, body: x }}]\n      expect: [{{ line: 1 }}]\n    - name: good\n      language: go\n      kind: valid\n      files: [{{ path: a.go, body: y }}]\n"
        );
        Catalog::from_files(decision_with(&spec)).map_err(|e| e.to_string())
    };
    let select = "    select: { ruleIds: [\"govet:*\"], levels: [error, note] }\n";
    assert!(load(&format!("    output: sarif\n{select}")).is_ok());
    assert!(load(select).unwrap_err().contains("needs `output: sarif`"));
    let level = "    output: sarif\n    select: { levels: [fatal] }\n";
    assert!(load(level).unwrap_err().contains("SARIF levels"));
    let stray = "    output: sarif\n    select: { rules: [x] }\n";
    assert!(load(stray).is_err());
}

#[test]
fn check_output_is_lines_unless_the_check_says_sarif() {
    let command = |value| match check(value).unwrap().kind {
        CheckKind::Command(command) => command,
        other => panic!("command expected, got {}", other.label()),
    };
    assert_eq!(
        command(json!({"type": "command", "argv": ["x"]})).output,
        CheckOutput::Lines
    );
    let sarif = command(json!({"type": "command", "argv": ["x"], "output": "sarif"}));
    assert_eq!(sarif.output, CheckOutput::Sarif);
}

#[test]
fn model_check() {
    let ok = check(json!({"type":"model","select":"symbol.kind == 'type'","prompt":"Is it so?"}));
    assert!(ok.is_ok());
    assert!(check(json!({"type":"model"})).is_ok());
    assert!(check(json!({"type":"model","shots":"examples"})).is_err());
}

#[test]
fn decision_spec_enforced_needs_an_accepted_status_and_a_check() {
    let decision = bundled("core/max-lines");
    assert!(decision.enforced());
    let proposed = decision.clone().map_spec(|mut s| {
        s.status = DecisionStatus::Proposed;
        s
    });
    assert!(!proposed.enforced());
    let doc = bundled("core/allow-annotation");
    assert!(!doc.enforced());
}

#[test]
fn status_enforced_is_true_only_for_accepted() {
    assert!(DecisionStatus::Accepted.enforced());
    for status in [
        DecisionStatus::Proposed,
        DecisionStatus::Rejected,
        DecisionStatus::Deprecated,
        DecisionStatus::Superseded,
    ] {
        assert!(!status.enforced(), "{status}");
    }
}

#[test]
fn status_is_default_only_for_accepted() {
    assert!(DecisionStatus::Accepted.is_default());
    assert!(!DecisionStatus::Rejected.is_default());
}

#[test]
fn a_supersession_names_decisions_that_exist_and_are_marked_superseded() {
    let two = |old_status: &str, supersedes: &str| {
        let old = format!("{SPEC}  status: {old_status}\n");
        let new = format!("{SPEC}  supersedes: [{supersedes}]\n");
        let mut files = base();
        files.insert("p/s/old.yaml".into(), decision("p/old", "s", &old));
        files.insert("p/s/new.yaml".into(), decision("p/new", "s", &new));
        files.insert(
            "p/pack.yaml".into(),
            pack("p", &[("s", &["a", "old", "new"])]),
        );
        Catalog::from_files(files)
    };
    assert!(two("superseded", "p/old").is_ok());
    let not_marked = two("accepted", "p/old").unwrap_err().to_string();
    assert!(not_marked.contains("superseded"), "{not_marked}");
    let missing = two("superseded", "p/nope").unwrap_err().to_string();
    assert!(missing.contains("no such decision"), "{missing}");
}

#[test]
fn a_builtin_names_one_operation_and_only_its_own_fields() {
    let order = json!({"type":"builtin","op":"order","clauses":[{"within":"file","by":["p/k"],"message":"m"}]});
    assert!(check(order).is_ok());
    assert!(
        check(json!({"type":"builtin","op":"cycle","edge":"imports","level":"module"})).is_ok()
    );
    let strays = [
        json!({"type":"builtin","op":"cycle","edge":"imports","level":"module","maxDistance":1}),
        json!({"type":"builtin","op":"cycle","edge":"imports","level":"module","prompt":"p"}),
        json!({"type":"builtin","clauses":[]}),
        json!({"type":"builtin","op":"order","id":"p/a","clauses":[]}),
        json!({"type":"builtin","op":"judge"}),
    ];
    for stray in strays {
        assert!(check(stray.clone()).is_err(), "{stray}");
    }
}

#[test]
fn a_command_cannot_be_a_placeholder() {
    let catalog = |argv: &str| {
        let spec = format!(
            "  severity: error\n  check:\n    type: command\n    argv: {argv}\n  examples:\n    - name: bad\n      language: go\n      kind: invalid\n      files: [{{ path: a.go, body: x }}]\n      expect: [{{ line: 1 }}]\n    - name: good\n      language: go\n      kind: valid\n      files: [{{ path: a.go, body: y }}]\n"
        );
        Catalog::from_files(decision_with(&spec))
    };
    assert!(catalog("[lint, \"{file}\"]").is_ok());
    let refused = catalog("[\"{file}\"]").unwrap_err().to_string();
    assert!(refused.contains("cannot be a placeholder"), "{refused}");
}

#[test]
fn a_standard_operation_needs_the_scope_it_judges() {
    let order = "  severity: error\n  check:\n    type: builtin\n    op: order\n    clauses:\n      - { within: file, by: [p/k], message: m }\n";
    let cycle = "  severity: error\n  check:\n    type: builtin\n    op: cycle\n    edge: imports\n    level: module\n";
    let examples = "  examples:\n    - name: bad\n      language: go\n      kind: invalid\n      files: [{ path: a.go, body: x }]\n      expect: [{ line: 1 }]\n    - name: good\n      language: go\n      kind: valid\n      files: [{ path: a.go, body: y }]\n";
    let load = |scope: &str, check: &str| {
        let text = decision(
            "p/a",
            "s",
            &format!(
                "  title: A\n  context: i\n  scope: {{ subject: {scope} }}\n  requirement: A MUST b.\n{check}{examples}"
            ),
        );
        Catalog::from_files(with("p/s/a.yaml", &text))
    };
    assert!(load("file", order).is_ok());
    assert!(load("project", cycle).is_ok());
    assert!(
        load("project", order)
            .unwrap_err()
            .to_string()
            .contains("one file at a time")
    );
    assert!(
        load("file", cycle)
            .unwrap_err()
            .to_string()
            .contains("whole project")
    );
}

#[test]
fn supersession_that_goes_round_in_a_circle_is_an_error_of_any_length() {
    let chain = |links: &[(&str, &str)]| {
        let mut files = base();
        let ids: Vec<&str> = links.iter().map(|(from, _)| *from).collect();
        for (from, to) in links {
            let name = from.rsplit('/').next().unwrap();
            let spec = format!("{SPEC}  status: superseded\n  supersedes: [{to}]\n");
            files.insert(format!("p/s/{name}.yaml"), decision(from, "s", &spec));
        }
        let names: Vec<&str> = ids.iter().map(|i| i.rsplit('/').next().unwrap()).collect();
        let mut all = vec!["a"];
        all.extend(names);
        files.insert("p/pack.yaml".into(), pack("p", &[("s", &all)]));
        Catalog::from_files(files)
    };
    let three = chain(&[("p/b", "p/c"), ("p/c", "p/d"), ("p/d", "p/b")]);
    assert!(three.unwrap_err().to_string().contains("circle"));
}

#[test]
fn parse_template_splits_text_from_holes_and_refuses_an_open_hole() {
    use lighthouse_spec::{TemplatePart, parse_template};
    let parts = parse_template("a {{ x.y }} b").unwrap();
    assert_eq!(
        parts,
        [
            TemplatePart::Text("a ".into()),
            TemplatePart::Hole("x.y".into()),
            TemplatePart::Text(" b".into()),
        ]
    );
    assert!(parse_template("a {{ x").is_err());
    assert_eq!(parse_template("").unwrap(), []);
}
