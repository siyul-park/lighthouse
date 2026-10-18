//! A fake agent under the sandbox: a shell script with a home directory that
//! holds fake secrets, run through `run` the way the callers do.

use std::{fs, path::Path, time::Duration};

use lighthouse_process::{Invocation, Profile, Ran, Sandbox, run, secrets};
use tempfile::TempDir;

use crate::support::server;

/// The world of a fake agent: a home with secrets, the directory it may
/// change, and one it may not.
struct World {
    home: TempDir,
    work: TempDir,
    outside: TempDir,
}

impl World {
    fn new() -> Self {
        let world = Self {
            home: tempfile::tempdir().unwrap(),
            work: tempfile::tempdir().unwrap(),
            outside: tempfile::tempdir().unwrap(),
        };
        for secret in [".ssh/id", ".aws/credentials"] {
            let file = world.home.path().join(secret);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "fake-secret").unwrap();
        }
        world
    }

    /// Reads and writes as the work directory allows, reaching `network`.
    fn profile(&self, network: &[String]) -> Profile {
        Profile {
            writes: vec![self.work.path().to_owned()],
            network: network.to_vec(),
            deny: secrets(Some(self.home.path()), self.work.path()),
        }
    }

    /// Runs `script` as the fake agent in the work directory.
    fn run(&self, script: &str, profile: &Profile) -> Ran {
        let path = std::env::var("PATH").unwrap();
        let env = [
            ("PATH".to_owned(), path),
            ("HOME".to_owned(), self.home.path().display().to_string()),
            ("WORK".to_owned(), self.work.path().display().to_string()),
            (
                "OUTSIDE".to_owned(),
                self.outside.path().display().to_string(),
            ),
        ];
        let argv = ["bash".to_owned(), "-c".to_owned(), script.to_owned()];
        run(&Invocation {
            argv: &argv,
            dir: self.work.path(),
            stdin: b"",
            env: Some(&env),
            limit: Duration::from_secs(30),
            grace: Duration::from_millis(300),
            max_output: 1 << 16,
            sandbox: Sandbox::On(profile),
        })
        .unwrap()
    }
}

fn lines(ran: &Ran) -> Vec<String> {
    String::from_utf8_lossy(&ran.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn an_agent_reads_no_secret_and_writes_only_where_it_may() {
    let world = World::new();
    let script = r#"
        cat "$HOME/.ssh/id" 2>/dev/null && echo read-ssh
        cat "$HOME/.aws/credentials" 2>/dev/null && echo read-aws
        echo x > "$OUTSIDE/file" 2>/dev/null && echo wrote-outside
        echo x > "$WORK/file" && echo wrote-copy
        echo x > "$TMPDIR/file" && echo wrote-tmp
        echo x > /dev/null && echo wrote-null
    "#;

    let ran = world.run(script, &world.profile(&[]));

    assert_eq!(
        lines(&ran),
        ["wrote-copy", "wrote-tmp", "wrote-null"],
        "{ran:?}"
    );
    assert!(world.work.path().join("file").is_file());
    assert!(!world.outside.path().join("file").exists());
}

#[test]
fn a_program_reads_the_toolchain_and_the_project_but_not_its_secrets() {
    let world = World::new();
    let root = tempfile::tempdir().unwrap();
    for (name, text) in [
        (".env", "KEY=1"),
        (".env.local", "KEY=2"),
        (".envrc", "layout"),
        ("README", "hi"),
    ] {
        fs::write(root.path().join(name), text).unwrap();
    }
    fs::create_dir_all(root.path().join(".lighthouse")).unwrap();
    fs::write(root.path().join(".lighthouse/local.toml"), "[approval]").unwrap();
    let profile = Profile {
        deny: secrets(Some(world.home.path()), root.path()),
        ..Profile::default()
    };
    let script = format!(
        r#"
        cd {root}
        for file in .env .env.local .lighthouse/local.toml; do
            cat "$file" >/dev/null 2>&1 && echo "read $file"
        done
        cat README .envrc >/dev/null && echo read-others
        ls /usr/bin >/dev/null && echo read-toolchain
    "#,
        root = root.path().display()
    );

    let ran = world.run(&script, &profile);

    assert_eq!(lines(&ran), ["read-others", "read-toolchain"], "{ran:?}");
}

#[test]
fn the_temporary_directory_of_a_run_is_its_own_and_is_gone_afterwards() {
    let world = World::new();
    let profile = world.profile(&[]);

    let first = world.run(r#"echo "$TMPDIR""#, &profile);
    let second = world.run(r#"echo "$TMPDIR""#, &profile);

    let (first, second) = (lines(&first)[0].clone(), lines(&second)[0].clone());
    assert_ne!(first, second);
    assert!(!Path::new(&first).exists(), "{first} is removed");
}

#[test]
fn without_a_network_list_nothing_is_reached_not_even_a_local_server() {
    let world = World::new();
    let port = server();
    let script = format!(
        r#"
        (exec 3<>/dev/tcp/127.0.0.1/{port}) 2>/dev/null && echo reached-local
        (exec 3<>/dev/tcp/1.1.1.1/443) 2>/dev/null && echo reached-internet
        echo done
    "#
    );

    let ran = world.run(&script, &world.profile(&[]));

    assert_eq!(lines(&ran), ["done"], "{ran:?}");
}

/// The script of an agent that speaks to a proxy by hand: a request in
/// absolute form for the server, a CONNECT to a name that is not listed, and
/// a connection that goes around the proxy.
#[cfg(target_os = "macos")]
fn through_proxy(port: u16) -> String {
    format!(
        r#"
        proxy=${{HTTP_PROXY#http://}}
        ask() {{
            exec 3<>/dev/tcp/${{proxy%:*}}/${{proxy#*:}}
            printf "$1" >&3
            head -n 1 <&3 | tr -d '\r'
            exec 3<&-
        }}
        ask 'GET http://127.0.0.1:{port}/ HTTP/1.1\r\nHost: a\r\n\r\n'
        ask 'CONNECT localhost:{port} HTTP/1.1\r\n\r\n'
        (exec 3<>/dev/tcp/127.0.0.1/{port}) 2>/dev/null && echo went-around
        echo done
    "#
    )
}

#[cfg(target_os = "macos")]
#[test]
fn an_agent_reaches_a_listed_host_through_the_proxy_and_nothing_else() {
    let world = World::new();
    let port = server();
    let listed = [format!("127.0.0.1:{port}")];

    let ran = world.run(&through_proxy(port), &world.profile(&listed));

    assert_eq!(
        lines(&ran),
        ["HTTP/1.1 200 OK", "HTTP/1.1 403 Forbidden", "done"],
        "{ran:?}"
    );
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(
        said.contains("refused network access to localhost"),
        "{said}"
    );
}
