//! A fake agent under the sandbox: a shell script with a home directory that
//! holds fake secrets, run through `run` the way the callers do.

use std::{
    fs,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    time::Duration,
};

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
        self.run_in(self.work.path(), script, profile)
    }

    /// Runs `script` as the fake agent in `dir`.
    fn run_in(&self, dir: &Path, script: &str, profile: &Profile) -> Ran {
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
            dir,
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
fn a_program_killed_by_a_signal_has_no_exit_code_under_any_backend() {
    let world = World::new();

    let ran = world.run("kill -9 $$", &world.profile(&[]));

    assert!(!ran.success);
    assert_eq!(ran.code, None, "{ran:?}");
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

/// A Unix socket of the host that something listens on, removed when dropped.
struct Socket {
    path: PathBuf,
    _listener: UnixListener,
}

impl Socket {
    fn bind(place: &Path) -> Option<Self> {
        let path = place.join(format!("lighthouse-test-{}.sock", std::process::id()));
        let _ = fs::remove_file(&path);
        let listener = UnixListener::bind(&path).ok()?;
        Some(Self {
            path,
            _listener: listener,
        })
    }

    /// The script that succeeds when the socket can be connected to.
    fn connect(&self) -> String {
        format!(
            r#"perl -MSocket -e 'socket(S, PF_UNIX, SOCK_STREAM, 0); connect(S, sockaddr_un($ARGV[0])) or exit 1' {:?} && echo connected"#,
            self.path
        )
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[test]
fn a_program_cannot_connect_to_a_socket_the_host_listens_on_under_run() {
    let world = World::new();
    let place = if cfg!(target_os = "linux") {
        PathBuf::from("/run")
    } else {
        world.outside.path().to_owned()
    };
    let Some(socket) = Socket::bind(&place) else {
        return;
    };
    assert!(UnixStream::connect(&socket.path).is_ok(), "the control");

    let ran = world.run(&socket.connect(), &world.profile(&[]));

    assert_eq!(lines(&ran), Vec::<String>::new(), "{ran:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_program_that_needs_nothing_in_tmp_cannot_connect_to_the_sockets_there() {
    let world = World::new();
    let socket = Socket::bind(world.outside.path()).unwrap();
    assert!(UnixStream::connect(&socket.path).is_ok(), "the control");
    let profile = Profile::default();

    let ran = world.run_in(Path::new("/"), &socket.connect(), &profile);

    assert_eq!(lines(&ran), Vec::<String>::new(), "{ran:?}");
}

#[cfg(target_os = "macos")]
#[test]
fn a_program_leads_its_group_in_a_session_of_its_own_and_has_no_terminal() {
    let world = World::new();
    let script = r#"exec perl -e 'print getpgrp(0) == $$ ? "leader" : "member", "\n"; print open(T, ">/dev/tty") ? "terminal" : "no terminal", "\n"'"#;

    let ran = world.run(script, &world.profile(&[]));

    assert_eq!(lines(&ran), ["leader", "no terminal"], "{ran:?}");
}
