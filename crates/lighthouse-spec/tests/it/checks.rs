//! The providers a `check:` block can name, and what the types around them say.

use lighthouse_process::Policy;
use lighthouse_spec::{
    Batching, Catalog, Check, CheckKind, CommandCheck, Decision, DecisionStatus, ModelCheck,
    Output, Point, Program, ProgramSpec, Sandbox, Stdin,
};
use lighthouse_test_support::catalog::*;
use serde_json::json;

fn check(value: serde_json::Value) -> Result<Check, serde_json::Error> {
    serde_json::from_value(value)
}

fn bundled(id: &str) -> &'static Decision {
    lighthouse_test_support::bundled().decision(id).unwrap()
}

#[test]
fn check_is_deterministic_unless_a_model_answers_it() {
    let cel = check(json!({"type":"cel","select":"file","where":"true","message":"m"})).unwrap();
    assert!(cel.kind.deterministic());
    let model = Check::new(CheckKind::Model(ModelCheck::default()));
    assert!(!model.kind.deterministic());
}

#[test]
fn check_kind_name_is_the_type_as_written() {
    let cel = bundled("core/file-size").check.as_ref().unwrap();
    assert_eq!(cel.kind.name(), "cel");
}

#[test]
fn there_is_no_builtin_check() {
    for value in [
        json!({"type":"builtin","id":"p/a"}),
        json!({"type":"builtin","op":"cycle","edge":"imports","level":"module"}),
    ] {
        let error = check(value).unwrap_err().to_string();
        assert!(error.contains("expected cel, command or model"), "{error}");
    }
}

#[test]
fn check_kind_deterministic_is_false_only_for_a_model() {
    let cel = check(json!({"type":"cel","select":"file","where":"true","message":"m"})).unwrap();
    assert!(cel.kind.deterministic());
    assert!(!CheckKind::Model(ModelCheck::default()).deterministic());
}

fn command(value: serde_json::Value) -> CommandCheck {
    match check(value).unwrap().kind {
        CheckKind::Command(command) => command,
        other => panic!("command expected, got {}", other.name()),
    }
}

#[test]
fn a_program_defaults_follow_its_output_and_its_binding_point() {
    let lines = Program::of(vec!["lint".into()]);
    assert_eq!(lines.output(Point::Check), Output::Lines);
    assert_eq!(lines.success(Point::Check), [0, 1]);
    assert_eq!(lines.output(Point::Fix), Output::Files);
    assert_eq!(lines.success(Point::Fix), [0]);
    assert_eq!(lines.output(Point::Verify), Output::Status);
    assert_eq!(lines.success(Point::Verify), [0]);
    assert_eq!(lines.timeout(Point::Check).as_secs(), 120);
    assert_eq!(lines.timeout(Point::Verify).as_secs(), 600);
    assert_eq!(lines.timeout(Point::Agent).as_secs(), 600);
}

#[test]
fn batching_follows_the_placeholders_of_the_command() {
    let batching =
        |args: &[&str]| Program::of(args.iter().map(|a| (*a).to_owned()).collect()).batching();
    assert_eq!(batching(&["lint", "{files}"]), Batching::Files);
    assert_eq!(batching(&["lint", "{file}"]), Batching::File);
    assert_eq!(batching(&["lint"]), Batching::Once);
}

#[test]
fn command_check_reads_the_program_fields_and_refuses_strays() {
    let ok = command(json!({
        "type": "command", "command": ["lint", "{files}"], "success": [0, 2],
        "timeout": "5s", "output": "status", "files": ["**/*.go"],
        "env": {"TOKEN": "${TOKEN}"}, "sandbox": {"network": ["api.example.com"]}
    }));
    assert_eq!(ok.program.success, Some(vec![0, 2]));
    assert_eq!(ok.program.output, Some(Output::Status));
    assert_eq!(ok.files, ["**/*.go"]);
    assert_eq!(ok.program.env["TOKEN"], "${TOKEN}");
    assert!(check(json!({"type":"command","command":["x"],"where":"true"})).is_err());
    assert!(check(json!({"type":"command","command":["x"],"scope":"file"})).is_err());
    for gone in ["argv", "batch", "exitCodes", "select", "columns"] {
        let stray = check(json!({"type": "command", "command": ["x"], gone: []}));
        assert!(stray.is_err(), "`{gone}` is gone");
    }
}

fn load_check(fields: &str) -> Result<Catalog, String> {
    let spec = format!(
        "  severity: error\n  check:\n    type: command\n    command: [lint]\n{fields}  examples:\n    - name: bad\n      language: go\n      kind: invalid\n      files: [{{ path: a.go, body: x }}]\n      expect: [{{ line: 1 }}]\n    - name: good\n      language: go\n      kind: valid\n      files: [{{ path: a.go, body: y }}]\n"
    );
    Catalog::from_files(decision_with(&spec), &lighthouse_test_support::cel())
        .map_err(|e| e.to_string())
}

#[test]
fn sarif_options_belong_to_sarif_output_and_name_sarif_levels() {
    let sarif = "    sarif: { rules: [\"govet:*\"], levels: [error, note] }\n";
    assert!(load_check(&format!("    output: sarif\n{sarif}")).is_ok());
    assert!(
        load_check(sarif)
            .unwrap_err()
            .contains("needs `output: sarif`")
    );
    let level = "    output: sarif\n    sarif: { levels: [fatal] }\n";
    assert!(load_check(level).unwrap_err().contains("SARIF levels"));
    let stray = "    output: sarif\n    sarif: { select: [x] }\n";
    assert!(load_check(stray).is_err());
    for unit in ["utf16CodeUnits", "unicodeCodePoints", "bytes"] {
        let columns = format!("    output: sarif\n    sarif: {{ columns: {unit} }}\n");
        assert!(load_check(&columns).is_ok(), "{unit}");
    }
    assert!(load_check("    output: sarif\n    sarif: { columns: chars }\n").is_err());
}

#[test]
fn a_check_prints_lines_sarif_or_a_status_and_nothing_else() {
    for output in ["lines", "sarif", "status"] {
        assert!(
            load_check(&format!("    output: {output}\n")).is_ok(),
            "{output}"
        );
    }
    for output in ["files", "stdout"] {
        let refused = load_check(&format!("    output: {output}\n")).unwrap_err();
        assert!(refused.contains("`output`"), "{refused}");
    }
}

#[test]
fn a_placeholder_the_binding_point_does_not_accept_is_refused_at_load() {
    let refused = |args: &str| {
        load_check("").map(drop).unwrap();
        let spec = format!(
            "  severity: error\n  check:\n    type: command\n    command: {args}\n  examples:\n    - name: bad\n      language: go\n      kind: invalid\n      files: [{{ path: a.go, body: x }}]\n      expect: [{{ line: 1 }}]\n    - name: good\n      language: go\n      kind: valid\n      files: [{{ path: a.go, body: y }}]\n"
        );
        Catalog::from_files(decision_with(&spec), &lighthouse_test_support::cel())
            .map_err(|e| e.to_string())
    };
    for fine in ["file", "files", "decision"] {
        assert!(
            refused(&format!("[lint, \"{{{fine}}}\"]")).is_ok(),
            "{fine}"
        );
    }
    for (bad, hint) in [
        ("model", "not a placeholder"),
        ("line", "not a placeholder"),
        ("rule", "it is gone"),
        ("path", "it is gone"),
    ] {
        let why = refused(&format!("[lint, \"{{{bad}}}\"]")).unwrap_err();
        assert!(why.contains(hint) && why.contains(bad), "{bad}: {why}");
    }
    let mixed = refused("[lint, \"--file={file}\"]").unwrap_err();
    assert!(mixed.contains("fills a whole argument"), "{mixed}");
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
    let decision = bundled("core/file-size");
    assert!(decision.enforced());
    let proposed = decision.clone().map_spec(|mut s| {
        s.status = DecisionStatus::Proposed;
        s
    });
    assert!(!proposed.enforced());
    let doc = bundled("testing/validation");
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
        files.insert("p/s/old.yaml".into(), decision("p/old", &old));
        files.insert("p/s/new.yaml".into(), decision("p/new", &new));
        files.insert(
            "p/pack.yaml".into(),
            pack("p", &[("s", &["a", "old", "new"])]),
        );
        Catalog::from_files(files, &lighthouse_test_support::cel())
    };
    assert!(two("superseded", "p/old").is_ok());
    let not_marked = two("accepted", "p/old").unwrap_err().to_string();
    assert!(not_marked.contains("superseded"), "{not_marked}");
    let missing = two("superseded", "p/nope").unwrap_err().to_string();
    assert!(missing.contains("no such decision"), "{missing}");
}

#[test]
fn a_command_cannot_be_a_placeholder() {
    let catalog = |argv: &str| {
        let spec = format!(
            "  severity: error\n  check:\n    type: command\n    command: {argv}\n  examples:\n    - name: bad\n      language: go\n      kind: invalid\n      files: [{{ path: a.go, body: x }}]\n      expect: [{{ line: 1 }}]\n    - name: good\n      language: go\n      kind: valid\n      files: [{{ path: a.go, body: y }}]\n"
        );
        Catalog::from_files(decision_with(&spec), &lighthouse_test_support::cel())
    };
    assert!(catalog("[lint, \"{file}\"]").is_ok());
    let refused = catalog("[\"{file}\"]").unwrap_err().to_string();
    assert!(refused.contains("cannot be a placeholder"), "{refused}");
}

#[test]
fn supersession_that_goes_round_in_a_circle_is_an_error_of_any_length() {
    let chain = |links: &[(&str, &str)]| {
        let mut files = base();
        let ids: Vec<&str> = links.iter().map(|(from, _)| *from).collect();
        for (from, to) in links {
            let name = from.rsplit('/').next().unwrap();
            let spec = format!("{SPEC}  status: superseded\n  supersedes: [{to}]\n");
            files.insert(format!("p/s/{name}.yaml"), decision(from, &spec));
        }
        let names: Vec<&str> = ids.iter().map(|i| i.rsplit('/').next().unwrap()).collect();
        let mut all = vec!["a"];
        all.extend(names);
        files.insert("p/pack.yaml".into(), pack("p", &[("s", &all)]));
        Catalog::from_files(files, &lighthouse_test_support::cel())
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

#[test]
fn a_program_shows_its_command_line_and_names_what_is_wrong_with_it() {
    let program = |args: &[&str]| Program::of(args.iter().map(|a| (*a).to_owned()).collect());

    assert_eq!(
        program(&["gofmt", "-l", "{files}"]).shown(),
        "gofmt -l {files}"
    );
    assert_eq!(program(&["gofmt", "{files}"]).problem(Point::Check), None);
    assert!(
        program(&["lint", "{line}"])
            .problem(Point::Check)
            .unwrap()
            .contains("{line}")
    );
    assert!(program(&["{file}"]).problem(Point::Fix).is_some());

    let filter = Program {
        stdin: Stdin::File,
        output: Some(Output::Stdout),
        ..program(&["gofmt"])
    };
    assert_eq!(filter.problem(Point::Fix), None);
    let unread = Program {
        stdin: Stdin::None,
        ..filter
    };
    assert!(unread.problem(Point::Fix).is_some());
}

#[test]
fn a_binding_point_accepts_its_placeholders_and_an_output_finds_or_changes() {
    assert_eq!(Point::Check.accepts(), ["file", "files", "decision"]);
    assert_eq!(Point::Fix.accepts(), ["file", "line", "symbol", "decision"]);
    assert!(Point::Verify.accepts().is_empty());
    assert_eq!(Point::Agent.accepts(), ["model"]);
    for finds in [Output::Lines, Output::Sarif, Output::Status] {
        assert!(finds.finds(), "{finds:?}");
    }
    for changes in [Output::Files, Output::Stdout] {
        assert!(!changes.finds(), "{changes:?}");
    }
}

#[test]
fn a_program_adds_its_own_hosts_to_the_sandbox_of_the_project() {
    let project = Policy {
        enabled: true,
        network: vec!["api.project.test".to_owned()],
        write: Vec::new(),
    };
    let own = Program {
        sandbox: Some(Sandbox {
            network: vec!["api.own.test".to_owned()],
            write: Vec::new(),
        }),
        ..Program::of(vec!["lint".into()])
    };

    assert_eq!(
        own.policy(&project).network,
        ["api.project.test", "api.own.test"]
    );
    assert_eq!(
        Program::of(vec!["lint".into()]).policy(&project).network,
        ["api.project.test"]
    );
}

#[test]
fn a_program_is_a_list_or_a_table() {
    let list: ProgramSpec = serde_json::from_value(json!(["cargo", "test"])).unwrap();
    assert_eq!(
        list,
        ProgramSpec::Command(vec!["cargo".into(), "test".into()])
    );

    let table: ProgramSpec =
        serde_json::from_value(json!({"command": ["cargo", "test"], "stdin": "file"})).unwrap();
    let ProgramSpec::Program(table) = table else {
        panic!("a table is a program");
    };
    assert_eq!(table.stdin, Stdin::File);

    let wrong = serde_json::from_value::<ProgramSpec>(json!({"argv": ["cargo"]}));
    assert!(wrong.is_err());
}

#[test]
fn a_decision_of_the_catalog_as_it_ships_is_bundled_and_an_edited_one_is_not() {
    let shipped = bundled("go/formatted");
    assert!(shipped.bundled(lighthouse_test_support::bundled()));

    let mut value = serde_json::to_value(shipped).unwrap();
    value["spec"]["severity"] = json!("info");
    let edited: Decision = serde_json::from_value(value).unwrap();
    assert!(!edited.bundled(lighthouse_test_support::bundled()));
}

#[test]
fn file_fact() {
    assert_eq!(
        lighthouse_spec::file_fact("{file.edition}"),
        Some("edition")
    );
    assert_eq!(lighthouse_spec::file_fact("{file}"), None);
    assert_eq!(lighthouse_spec::file_fact("--edition={file.edition}"), None);
    assert_eq!(lighthouse_spec::file_fact("{file.}"), None);
    assert_eq!(lighthouse_spec::file_fact("{file.Edition}"), None);
    assert_eq!(lighthouse_spec::file_fact("{file.a-b}"), None);
}

#[test]
fn a_fact_of_a_file_is_a_placeholder_of_a_check_beside_its_files_and_of_a_fix() {
    let program = |args: &[&str]| Program::of(args.iter().map(|a| (*a).to_owned()).collect());
    let fact = "{file.edition}";

    assert_eq!(
        program(&["fmt", "--edition", fact, "{files}"]).problem(Point::Check),
        None
    );
    assert_eq!(
        program(&["fmt", fact, "{file}"]).problem(Point::Check),
        None
    );
    let alone = program(&["fmt", fact]).problem(Point::Check).unwrap();
    assert!(alone.contains("needs `{file}` or `{files}`"), "{alone}");
    let mixed = program(&["fmt", "--edition={file.edition}", "{files}"])
        .problem(Point::Check)
        .unwrap();
    assert!(mixed.contains("fills a whole argument"), "{mixed}");
    assert_eq!(program(&["fmt", fact]).problem(Point::Fix), None);
    for point in [Point::Verify, Point::Provider, Point::Agent] {
        let why = program(&["fmt", fact]).problem(point).unwrap();
        assert!(why.contains("is not a placeholder"), "{point}: {why}");
    }
}
