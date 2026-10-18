//! Trust for the commands a fix runs: given at a terminal, bound to the exact
//! project files, never granted by the repository itself.

use std::fs;

use predicates::prelude::*;

use crate::fix::{UPPERCASE, lighthouse, read, text_project, trust, write};

#[test]
fn a_command_runs_only_after_the_user_trusts_the_project() {
    let dir = text_project(UPPERCASE);

    lighthouse(dir.path())
        .args(["check", "--fix"])
        .assert()
        .stderr(predicate::str::contains("untrusted project"));
    assert_eq!(read(&dir, "a.txt"), "hello\n");

    trust(dir.path());
    lighthouse(dir.path())
        .args(["check", "--fix"])
        .assert()
        .stderr(predicate::str::contains("fixed local/shout [safe] a.txt"));
    assert_eq!(read(&dir, "a.txt"), "HELLO\n");
}

#[test]
fn a_change_to_the_configuration_or_the_rules_withdraws_trust() {
    let dir = text_project(UPPERCASE);
    trust(dir.path());
    write(dir.path(), "a.txt", "hello\n");

    let config = read(&dir, "lighthouse.toml");
    write(
        dir.path(),
        "lighthouse.toml",
        &format!("{config}# edited\n"),
    );
    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("untrusted project"));

    write(dir.path(), "lighthouse.toml", &config);
    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("would fix local/shout"));

    let rule = read(&dir, ".lighthouse/decisions/shout.yaml");
    write(
        dir.path(),
        ".lighthouse/decisions/shout.yaml",
        &rule.replace("tr a-z A-Z", "tr a-z B-Z"),
    );
    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("untrusted project"));
}

#[test]
fn trust_can_be_revoked_and_ci_can_override_it() {
    let dir = text_project(UPPERCASE);
    trust(dir.path());
    lighthouse(dir.path())
        .args(["trust", "--revoke"])
        .assert()
        .success();

    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("untrusted project"));
    lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("would fix local/shout"));
}

#[test]
fn a_fix_table_in_lighthouse_toml_cannot_grant_trust() {
    let dir = text_project(UPPERCASE);
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\", \"local\"]\n[fix]\ncommands = \"allow\"\n[rules]\n\"local/shout\" = \"error\"\n",
        ),
    );

    lighthouse(dir.path())
        .args(["check", "--fix"])
        .assert()
        .code(2);
    assert_eq!(read(&dir, "a.txt"), "hello\n");
}

#[test]
fn trust_lists_what_it_trusts_and_needs_a_terminal_or_yes() {
    let dir = text_project(UPPERCASE);
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\", \"local\"]\n[rules]\n\"local/shout\" = \"error\"\n[languages.go]\nformatter = [\"gofmt\"]\n",
        ),
    );

    lighthouse(dir.path())
        .arg("trust")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("formatter for go: gofmt"))
        .stdout(predicate::str::contains(
            "fixer of local/shout: sh -c tr a-z A-Z",
        ))
        .stderr(predicate::str::contains("--yes"));
    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("untrusted project"));

    lighthouse(dir.path())
        .args(["trust", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("trusting"));
}

#[test]
fn a_program_inside_the_project_is_trusted_by_its_content() {
    let dir = text_project(UPPERCASE);
    write(
        dir.path(),
        "tools/shout.sh",
        "#!/bin/sh\ntr a-z A-Z < \"$1\"\n",
    );
    let rule = read(&dir, ".lighthouse/decisions/shout.yaml").replace(
        "argv: [sh, \"-c\", \"tr a-z A-Z < \\\"$1\\\"\", sh, \"{file}\"]",
        "argv: [\"./tools/shout.sh\", \"{file}\"]",
    );
    write(dir.path(), ".lighthouse/decisions/shout.yaml", &rule);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.path().join("tools/shout.sh");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    trust(dir.path());
    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("would fix local/shout"));

    write(
        dir.path(),
        "tools/shout.sh",
        "#!/bin/sh\ncat \"$1\"; echo pwned\n",
    );

    lighthouse(dir.path())
        .args(["check", "--fix", "--dry-run"])
        .assert()
        .stderr(predicate::str::contains("untrusted project"));
}
