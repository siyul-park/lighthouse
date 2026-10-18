//! The sandbox through the binary: a command check cannot read the secrets of
//! the home it runs for, cannot write there, and reaches the network only
//! through the hosts the project lists; and turning the sandbox off, or
//! changing what it allows, goes through trust. The "internet" is a server on
//! the loopback of the test.

use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    thread,
};

use predicates::prelude::*;
use tempfile::TempDir;

use crate::{
    checks::{decision, lighthouse, project, trust, write},
    fix::{UPPERCASE, text_project},
};

const LOCAL_FILE: &str = ".lighthouse/local.toml";

/// What a check finds out about its surroundings: whether it reads the secret
/// and an ordinary file of the home, whether it writes there, and what it
/// gets from the server through the proxy and past it.
const SURVEY: &str = r#"secret=$(cat "$HOME/.ssh/id_fake" 2>/dev/null); plain=$(cat "$HOME/plain.txt" 2>/dev/null); touch "$HOME/pwned" 2>/dev/null; wrote=$(ls "$HOME/pwned" 2>/dev/null); via=$(curl -s --max-time 5 "http://127.0.0.1:$PORT/"); direct=$(curl -s --noproxy '*' --max-time 3 "http://127.0.0.1:$PORT/"); echo "$1:1: secret=[$secret] plain=[$plain] wrote=[$wrote] via=[$via] direct=[$direct]"; exit 1"#;

/// A server on the loopback that answers every request `ok`; its port.
fn serve() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut connection) = connection else {
                continue;
            };
            thread::spawn(move || {
                let mut request = [0_u8; 1024];
                let _ = connection.read(&mut request);
                let _ = connection.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                );
            });
        }
    });
    port
}

fn local(dir: &TempDir, text: &str) {
    write(dir.path(), LOCAL_FILE, text);
}

/// A trusted project whose check runs `SURVEY` against the server on `port`,
/// which the project lets the programs reach; and the home they run for.
fn surveyed(port: u16) -> (TempDir, TempDir) {
    let script = SURVEY.replace("$PORT", &port.to_string());
    let dir = project(&decision(&format!(
        "  check:\n    type: command\n    argv: [sh, -c, {script:?}, sh, \"{{file}}\"]\n"
    )));
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [\"core\", \"local\"]\n[rules]\n\"local/probe\" = \"error\"\n[sandbox]\nnetwork = [\"127.0.0.1:{port}\"]\n"
        )),
    );
    let home = tempfile::tempdir().unwrap();
    write(home.path(), ".ssh/id_fake", "FAKE-KEY\n");
    write(home.path(), "plain.txt", "visible\n");
    trust(dir.path());
    (dir, home)
}

#[test]
fn a_command_check_reads_but_the_secrets_writes_nowhere_and_reaches_only_the_listed_hosts() {
    let (dir, home) = surveyed(serve());

    let out = lighthouse(dir.path())
        .env("HOME", home.path())
        .arg("check")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("secret=[] plain=[visible] wrote=[] via=[ok] direct=[]"),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!home.path().join("pwned").exists());
}

#[test]
fn turning_the_sandbox_off_needs_a_trusted_project_and_every_run_says_so() {
    let dir = text_project(UPPERCASE);
    local(&dir, "[sandbox]\nenabled = false\n");

    lighthouse(dir.path())
        .arg("check")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("needs a trusted project"));

    trust(dir.path());
    lighthouse(dir.path())
        .arg("check")
        .assert()
        .stderr(predicate::str::contains(
            "sandbox off: programs run with your full access",
        ));
}

#[test]
fn what_the_sandbox_allows_is_part_of_the_trust_a_personal_file_cannot_change_alone() {
    let dir = text_project(UPPERCASE);
    trust(dir.path());
    let trusted = |dir: &TempDir| {
        let out = lighthouse(dir.path())
            .args(["check", "--fix", "--dry-run"])
            .output()
            .unwrap();
        let stderr = String::from_utf8(out.stderr).unwrap();
        !stderr.contains("not trusted")
    };
    assert!(trusted(&dir));

    local(&dir, "[sandbox]\nnetwork = [\"api.test\"]\n");

    assert!(!trusted(&dir), "a host is new");
    trust(dir.path());
    assert!(trusted(&dir));
    fs::remove_file(dir.path().join(LOCAL_FILE)).unwrap();
    assert!(!trusted(&dir), "the hosts are gone");
}

#[test]
fn without_a_home_a_sandboxed_program_is_not_run_for_the_secrets_cannot_be_located() {
    let (dir, _home) = surveyed(serve());

    let out = lighthouse(dir.path())
        .env_remove("HOME")
        .arg("check")
        .output()
        .unwrap();

    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        said.contains("cannot locate the user's secrets: HOME is not set"),
        "{said}"
    );
    assert!(!said.contains("plain=[visible]"), "{said}");
}
