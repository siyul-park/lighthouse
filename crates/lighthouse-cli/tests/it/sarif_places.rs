//! Where a SARIF result lands, which unit its columns count in, and what the
//! reader says when it leaves something out: through the binary, with a fake
//! tool.

use serde_json::{Value, json};

use super::checks::{lighthouse, project, trust, write};
use super::command_sarif::{SCRIPT, check, location, result, wrapped, wrapping};

fn log_of(results: Vec<Value>, column_kind: Option<&str>) -> Value {
    let mut run = json!({
        "tool": { "driver": { "name": "fake" } },
        "originalUriBaseIds": {
            "%SRC%": { "uri": "file://@ROOT@/src/" },
            "%HOP%": { "uri": "a/", "uriBaseId": "%SRC%" },
        },
        "results": results,
    });
    if let Some(kind) = column_kind {
        run["columnKind"] = json!(kind);
    }
    json!({ "version": "2.1.0", "runs": [run] })
}

/// The file each result, named by its message, was placed in.
fn placed(results: Vec<Value>, extra: &str, files: &[(&str, &str)]) -> Vec<(String, Value)> {
    let dir = wrapped(extra, &log_of(results, None));
    for (name, text) in files {
        write(dir.path(), name, text);
    }
    check(dir.path())
        .into_iter()
        .map(|d| (d["message"].as_str().unwrap().to_owned(), d))
        .collect()
}

#[test]
fn a_uri_lands_in_its_file_however_it_is_spelled() {
    let at = |uri: &str, base: Option<&str>| location(uri, base, 1, 1);
    let spellings = [
        ("plain", "src/a.txt", None, "src/a.txt"),
        ("escaped", "src/sp%20ace.txt", None, "src/sp ace.txt"),
        ("file-triple", "file://@ROOT@/src/a.txt", None, "src/a.txt"),
        ("file-single", "file:@ROOT@/src/a.txt", None, "src/a.txt"),
        ("fragment", "src/a.txt#L3", None, "src/a.txt"),
        ("query", "src/a.txt?x=1", None, "src/a.txt"),
        ("dotdot", "src/x/../a.txt", None, "src/a.txt"),
        ("dot", "./src/./a.txt", None, "src/a.txt"),
        ("base", "a.txt", Some("%SRC%"), "src/a.txt"),
        ("hop", "a.txt", Some("%HOP%"), "src/a/a.txt"),
    ];
    let results = spellings
        .iter()
        .map(|(name, uri, base, _)| result("r", "error", name, at(uri, *base)))
        .collect();
    let found = placed(
        results,
        "",
        &[("src/sp ace.txt", "s\n"), ("src/a/a.txt", "x\n")],
    );
    for (name, _, _, file) in spellings {
        let hit = found
            .iter()
            .find(|(m, _)| m == name)
            .unwrap_or_else(|| panic!("{name}"));
        assert_eq!(hit.1["file"], file, "{name}");
    }
}

#[test]
fn a_uri_that_climbs_out_of_the_project_is_dropped_with_a_notice() {
    let results = vec![
        result("r", "error", "up", location("../outside.txt", None, 1, 1)),
        result(
            "r",
            "error",
            "far",
            location("file:///not/here.go", None, 1, 1),
        ),
        result("r", "error", "kept", location("src/a.txt", None, 1, 1)),
    ];
    let dir = wrapped("", &log_of(results, None));
    let out = lighthouse(dir.path())
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"kept\"") && !stdout.contains("\"up\"") && !stdout.contains("\"far\"")
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("fake: 2 result(s) in files the project does not have were dropped"),
        "{stderr}"
    );
}

#[test]
fn a_file_the_reader_cannot_read_starts_its_columns_at_one_and_says_so() {
    let dir = wrapped(
        "",
        &log_of(
            vec![result(
                "r",
                "error",
                "gone",
                location("src/gone.txt", None, 2, 9),
            )],
            None,
        ),
    );
    write(dir.path(), "src/gone.txt", "ab\ncdefghij\n");
    write(
        dir.path(),
        "tools/lint.sh",
        "rm src/gone.txt\nsed \"s#@ROOT@#$PWD#g\" tools/log.json\nexit 1\n",
    );
    trust(dir.path());
    let out = lighthouse(dir.path())
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let found: Vec<Value> = stdout
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    assert_eq!(found.len(), 1, "{stdout}");
    assert_eq!(found[0]["span"]["start"]["col"], 1);
    assert_eq!(found[0]["span"]["start"]["line"], 2);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("could not be read to convert columns"),
        "{stderr}"
    );
}

#[test]
fn columns_count_in_the_unit_the_decision_the_log_or_the_default_names() {
    // `é😀x` is 2 + 4 + 1 bytes; `x` is column 7 in bytes, 4 in UTF-16 code
    // units and 3 in code points.
    let cases = [
        (
            "bytes, as golangci-lint counts",
            None,
            "    columns: bytes\n",
            7,
        ),
        ("the log says code points", Some("unicodeCodePoints"), "", 3),
        ("the log says UTF-16", Some("utf16CodeUnits"), "", 4),
        ("nothing says: UTF-16", None, "", 4),
        (
            "the decision wins over the log",
            Some("utf16CodeUnits"),
            "    columns: unicodeCodePoints\n",
            3,
        ),
    ];
    for (name, kind, extra, column) in cases {
        let at = location("src/wide.txt", None, 1, column);
        let dir = wrapped(extra, &log_of(vec![result("r", "error", name, at)], kind));
        let found = check(dir.path());
        assert_eq!(found.len(), 1, "{name}");
        assert_eq!(found[0]["span"]["start"]["col"], 7, "{name}");
    }
}

#[test]
fn a_negative_rule_index_means_no_rule_and_the_rule_id_names_it() {
    let mut by_id = result("errcheck", "error", "", location("src/a.txt", None, 1, 1));
    by_id["ruleIndex"] = json!(-1);
    by_id["message"] = json!({ "text": "indexed -1" });
    let mut nameless = result("x", "error", "", location("src/a.txt", None, 1, 1));
    nameless["ruleIndex"] = json!(-1);
    nameless.as_object_mut().unwrap().remove("ruleId");
    nameless["message"] = json!({ "id": "default" });
    let found = placed(vec![by_id, nameless], "", &[]);
    assert_eq!(found.len(), 2, "{found:?}");
    let first = found.iter().find(|(m, _)| m == "indexed -1").unwrap();
    assert_eq!(first.1["evidence"]["ruleId"], "errcheck");
    let second = found.iter().find(|(m, _)| m == "unknown").unwrap();
    assert_eq!(second.1["evidence"]["ruleId"], "unknown");
}

#[test]
fn findings_of_one_rule_about_different_callees_in_one_place_keep_apart() {
    let one = "Error return value of `os.Remove` is not checked";
    let results = vec![
        result("errcheck", "error", one, location("src/a.txt", None, 1, 1)),
        result(
            "errcheck",
            "error",
            &one.replace("Remove", "Mkdir"),
            location("src/a.txt", None, 1, 1),
        ),
    ];
    let found = placed(results, "", &[]);
    assert_eq!(found.len(), 2);
    assert_ne!(found[0].1["fingerprint"], found[1].1["fingerprint"]);
}

#[test]
fn no_output_at_all_is_incomplete_even_when_the_exit_code_is_clean() {
    for code in [0, 1] {
        let dir = project(&wrapping(""));
        write(dir.path(), "tools/lint.sh", &format!("exit {code}\n"));
        trust(dir.path());
        let out = lighthouse(dir.path()).arg("check").output().unwrap();
        assert_eq!(out.status.code(), Some(3), "exit {code}");
        assert!(String::from_utf8_lossy(&out.stdout).contains("printed no SARIF log"));
    }
    let clean = log_of(Vec::new(), None);
    let dir = wrapped("", &clean);
    write(
        dir.path(),
        "tools/lint.sh",
        SCRIPT.replace("exit 1", "exit 0").as_str(),
    );
    trust(dir.path());
    lighthouse(dir.path()).arg("check").assert().success();
}
