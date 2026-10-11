//! What the process environment decides: colour, the XDG directories, and the
//! generated reference docs.

use std::{fs, path::Path};

use crate::checks::{lighthouse, project, write};

/// The ANSI escape that starts a style.
const ESCAPE: &str = "\u{1b}[";

fn help(dir: &Path, envs: &[(&str, &str)], flags: &[&str]) -> String {
    let mut command = lighthouse(dir);
    command.env_remove("NO_COLOR").env_remove("CLICOLOR_FORCE");
    for (name, value) in envs {
        command.env(name, value);
    }
    let out = command.args(flags).arg("--help").output().unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn no_color_wins_over_a_forced_color_and_the_flag_wins_over_both() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        help(dir.path(), &[("CLICOLOR_FORCE", "1")], &[]).contains(ESCAPE),
        "CLICOLOR_FORCE colours the help"
    );
    assert!(
        !help(
            dir.path(),
            &[("CLICOLOR_FORCE", "1"), ("NO_COLOR", "1")],
            &[]
        )
        .contains(ESCAPE),
        "NO_COLOR removes it"
    );
    assert!(!help(dir.path(), &[], &[]).contains(ESCAPE));
    assert!(help(dir.path(), &[("NO_COLOR", "1")], &["--color", "always"]).contains(ESCAPE));
    assert!(
        !help(
            dir.path(),
            &[("CLICOLOR_FORCE", "1")],
            &["--color", "never"]
        )
        .contains(ESCAPE)
    );
    assert!(help(dir.path(), &[("LIGHTHOUSE_COLOR", "always")], &[]).contains(ESCAPE));
}

#[test]
fn trust_is_kept_below_xdg_config_home_and_nothing_below_home() {
    let dir = project(&crate::checks::decision(
        "  check:\n    type: command\n    argv: [sh, tools/lint.sh]\n",
    ));
    let (config, home) = (dir.path().join("config"), dir.path().join("home"));
    fs::create_dir_all(&home).unwrap();
    let run = |args: &[&str], config: Option<&Path>| {
        let mut command = lighthouse(dir.path());
        command.env("HOME", &home).env_remove("XDG_CONFIG_HOME");
        if let Some(config) = config {
            command.env("XDG_CONFIG_HOME", config);
        }
        command.args(args).assert().success();
    };
    run(&["trust", "--yes"], Some(&config));
    assert!(config.join("lighthouse/trust.toml").is_file());
    assert!(!home.join(".lighthouse").exists());

    run(&["trust", "--yes"], None);
    assert!(home.join(".config/lighthouse/trust.toml").is_file());
    assert!(!home.join(".lighthouse").exists());
}

#[test]
fn a_relative_xdg_directory_is_ignored() {
    let dir = project(&crate::checks::decision(
        "  check:\n    type: command\n    argv: [sh, tools/lint.sh]\n",
    ));
    let home = dir.path().join("home");
    fs::create_dir_all(&home).unwrap();
    lighthouse(dir.path())
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", "relative")
        .args(["trust", "--yes"])
        .assert()
        .success();
    assert!(home.join(".config/lighthouse/trust.toml").is_file());
    assert!(!dir.path().join("relative").exists());
}

#[test]
fn docs_check_covers_the_command_and_tool_references() {
    let dir = tempfile::tempdir().unwrap();
    lighthouse(dir.path())
        .args(["docs", "generate"])
        .assert()
        .success();
    for page in ["docs/cli.md", "docs/mcp.md"] {
        assert!(dir.path().join(page).is_file(), "{page}");
    }
    lighthouse(dir.path())
        .args(["docs", "check"])
        .assert()
        .success();
    let cli = fs::read_to_string(dir.path().join("docs/cli.md")).unwrap();
    assert!(cli.contains("lighthouse decision show"), "{cli}");
    let mcp = fs::read_to_string(dir.path().join("docs/mcp.md")).unwrap();
    assert!(mcp.contains("review_resolve"), "{mcp}");

    for page in ["docs/cli.md", "docs/mcp.md"] {
        let path = dir.path().join(page);
        let text = fs::read_to_string(&path).unwrap();
        write(dir.path(), page, "stale\n");
        lighthouse(dir.path())
            .args(["docs", "check"])
            .assert()
            .code(1);
        fs::write(&path, text).unwrap();
        lighthouse(dir.path())
            .args(["docs", "check"])
            .assert()
            .success();
    }
}
