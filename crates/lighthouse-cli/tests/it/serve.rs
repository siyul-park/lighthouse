//! `lighthouse serve`: a command run there is the command run here, and a
//! command is run here whenever no server of this build takes it.

use std::{
    fs,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Child, Command, Output as Process, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use lighthouse_cli::serve::{
    self, Hello, Output, Refusal, Run, Runner, Socket, permits, socket_in,
};
use lighthouse_protocol::{ErrorObject, Message, read_message, write_message};
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::agent::{BIG, edit, pre_payload, stop, text_project};
use crate::checks::home;

/// A directory for sockets, short enough for the path of one.
fn runtime() -> TempDir {
    tempfile::Builder::new()
        .prefix("lh")
        .tempdir_in("/tmp")
        .unwrap()
}

/// The program, in the project at `dir`, with sockets in `runtime`.
fn lighthouse(dir: &Path, runtime: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lighthouse"));
    command
        .current_dir(dir)
        .envs(home(dir))
        .env("XDG_RUNTIME_DIR", runtime)
        .env_remove("LIGHTHOUSE_NO_SERVE")
        .env_remove("LIGHTHOUSE_TRUST");
    command
}

/// The socket of the project at `dir` when sockets are in `runtime`.
fn socket(dir: &Path, runtime: &Path) -> PathBuf {
    socket_in(&runtime.join("lighthouse"), dir)
}

/// A `lighthouse serve` that ends with the test.
struct Serving {
    child: Child,
}

impl Serving {
    fn start(dir: &Path, runtime: &Path, idle: &str) -> Self {
        let child = lighthouse(dir, runtime)
            .args(["serve", "--idle", idle])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let served = Self { child };
        let until = Instant::now() + Duration::from_secs(20);
        while !answers(dir, runtime) {
            assert!(Instant::now() < until, "the server did not come up");
            thread::sleep(Duration::from_millis(20));
        }
        served
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn answers(dir: &Path, runtime: &Path) -> bool {
    lighthouse(dir, runtime)
        .args(["serve", "status"])
        .output()
        .unwrap()
        .status
        .success()
}

/// What a run printed and how it ended.
fn seen(out: &Process) -> (String, i32) {
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap(),
    )
}

fn run_hook(
    dir: &Path,
    runtime: &Path,
    event: &str,
    payload: &Value,
    local: bool,
) -> (String, i32) {
    let mut command = lighthouse(dir, runtime);
    command.args(["hook", "claude-code", event, "--allow-incomplete"]);
    if local {
        command.arg("--no-serve");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(
        &mut child.stdin.take().unwrap(),
        payload.to_string().as_bytes(),
    )
    .unwrap();
    seen(&child.wait_with_output().unwrap())
}

#[test]
fn a_served_run_is_a_local_run() {
    let (local, served) = (text_project(), text_project());
    let runtime = runtime();
    let server = Serving::start(served.path(), runtime.path(), "3s");

    for format in ["text", "json", "sarif"] {
        let on = |dir: &Path, extra: &[&str]| {
            let mut command = lighthouse(dir, runtime.path());
            command.args(["check", "--format", format, "--limit", "10"]);
            seen(&command.args(extra).output().unwrap())
        };
        assert_eq!(
            on(served.path(), &[]),
            on(local.path(), &["--no-serve"]),
            "{format}"
        );
    }

    let sequence = |dir: &Path, local: bool| {
        let prompt = json!({ "session_id": "s1", "cwd": dir, "hook_event_name": "UserPromptSubmit", "prompt": "keep it short" });
        let start = json!({ "session_id": "s1", "cwd": dir, "hook_event_name": "SessionStart", "source": "startup" });
        let mut seen = vec![
            run_hook(dir, runtime.path(), "session-start", &start, local),
            run_hook(dir, runtime.path(), "user-prompt-submit", &prompt, local),
            run_hook(
                dir,
                runtime.path(),
                "pre-tool-use",
                &pre_payload(dir, "small.txt"),
                local,
            ),
        ];
        fs::write(dir.join("small.txt"), BIG).unwrap();
        seen.push(run_hook(
            dir,
            runtime.path(),
            "post-tool-use",
            &edit(dir, "small.txt"),
            local,
        ));
        seen.push(run_hook(
            dir,
            runtime.path(),
            "stop",
            &stop(dir, false),
            local,
        ));
        seen
    };
    let (via_server, alone) = (sequence(served.path(), false), sequence(local.path(), true));
    assert_eq!(via_server, alone);
    assert!(via_server[3].0.contains("core/max-lines"), "{via_server:?}");

    // The commands above reached the server: it was asked after its idle time
    // would have ended it.
    assert!(answers(served.path(), runtime.path()));

    // A served run loads no session; one that is not served does.
    let loads = |extra: &[(&str, &str)]| {
        let mut command = lighthouse(served.path(), runtime.path());
        command
            .args(["check", "--timings"])
            .envs(extra.iter().copied());
        let out = command.output().unwrap();
        String::from_utf8_lossy(&out.stderr).contains("session bundled catalog")
    };
    // The first run of a server loads the project.
    let _ = loads(&[]);
    assert!(!loads(&[]), "the same environment is served");
    assert!(
        loads(&[("LIGHTHOUSE_TRUST", "1")]),
        "a variable only the client has is run here"
    );
    assert!(
        loads(&[("PATH", "/usr/bin:/bin")]),
        "a PATH of its own is run here"
    );
    drop(server);
}

/// Ends the server at `path` when dropped.
struct Stop<'a>(&'a Path);

impl Drop for Stop<'_> {
    fn drop(&mut self) {
        if let Ok(stream) = std::os::unix::net::UnixStream::connect(self.0) {
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let request = Message::request(1, "shutdown", ()).unwrap();
            write_message(&mut writer, &request).unwrap();
            let _ = read_message(&mut reader);
        }
    }
}

/// A server that answers every command with a fixed output, and keeps what it
/// was asked.
struct Canned(Mutex<Vec<Run>>);

impl Runner for Canned {
    fn run(&self, run: &Run) -> Result<Output, Refusal> {
        self.0.lock().unwrap().push(run.clone());
        if run.argv.iter().any(|arg| arg == "--strict") {
            return Err(Refusal("not run here".to_owned()));
        }
        Ok(Output {
            stdout: "from the server\n".to_owned(),
            stderr: "a note\n".to_owned(),
            exit: 7,
        })
    }
}

#[test]
fn a_command_is_forwarded_with_its_environment_and_its_exit_code_is_the_servers() {
    let dir = text_project();
    let runtime = runtime();
    let path = socket(dir.path(), runtime.path());
    let canned = Canned(Mutex::new(Vec::new()));
    let hello = build_of_the_program();
    thread::scope(|scope| {
        let _stop = Stop(&path);
        scope.spawn(|| serve::serve(&path, Duration::from_secs(30), (&hello, &canned)).unwrap());
        let until = Instant::now() + Duration::from_secs(5);
        while !path.exists() {
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(10));
        }
        let out = lighthouse(dir.path(), runtime.path())
            .args(["check", "--format", "json"])
            .env("LIGHTHOUSE_ATTRIBUTED_TO", "me")
            .output()
            .unwrap();
        assert_eq!(seen(&out), ("from the server\n".to_owned(), 7));
        assert_eq!(String::from_utf8_lossy(&out.stderr), "a note\n");

        let local = lighthouse(dir.path(), runtime.path())
            .args(["check", "--no-serve"])
            .output()
            .unwrap();
        assert_ne!(seen(&local).1, 7);

        let strict = lighthouse(dir.path(), runtime.path())
            .args(["check", "--strict"])
            .output()
            .unwrap();
        assert_ne!(
            seen(&strict).0,
            "from the server\n",
            "a command the server does not run is left to the client"
        );
    });
    let asked = canned.0.lock().unwrap();
    assert_eq!(asked[0].env["LIGHTHOUSE_ATTRIBUTED_TO"], "me");
    assert!(asked[0].env.keys().all(|name| Run::decides(name)));
    assert!(asked[0].argv.contains(&"check".to_owned()));
}

#[test]
fn a_socket_nobody_listens_on_is_not_a_server() {
    let (stale, alone) = (text_project(), text_project());
    let runtime = runtime();
    drop(Socket::claim(&socket(stale.path(), runtime.path())).unwrap());

    let check = |dir: &Path, flag: &[&str]| {
        seen(
            &lighthouse(dir, runtime.path())
                .arg("check")
                .args(flag)
                .output()
                .unwrap(),
        )
    };

    assert_eq!(
        check(stale.path(), &[]),
        check(alone.path(), &["--no-serve"])
    );
}

#[test]
fn a_server_of_another_build_is_told_to_shut_down_and_the_command_runs_here() {
    let dir = text_project();
    let runtime = runtime();
    let path = socket(dir.path(), runtime.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(&path).unwrap();
    let other = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let mut methods = Vec::new();
        while let Ok(Some(request)) = read_message(&mut reader) {
            let id = request.id.clone().unwrap();
            let method = request.method.clone().unwrap();
            let answer = match method.as_str() {
                "initialize" => Message::response(
                    id,
                    Hello {
                        version: "another build".to_owned(),
                        protocol: "1".to_owned(),
                    },
                )
                .unwrap(),
                _ => Message::response(id, ()).unwrap(),
            };
            write_message(&mut writer, &answer).unwrap();
            methods.push(method);
            if methods.last().unwrap() == "shutdown" {
                break;
            }
        }
        methods
    });

    let out = lighthouse(dir.path(), runtime.path())
        .args(["check", "--format", "json"])
        .output()
        .unwrap();

    let local = lighthouse(dir.path(), runtime.path())
        .args(["check", "--format", "json", "--no-serve"])
        .output()
        .unwrap();
    assert_eq!(seen(&out), seen(&local));
    assert_eq!(other.join().unwrap(), ["initialize", "shutdown"]);
    let _ = ErrorObject {
        code: 0,
        message: String::new(),
        data: None,
    };
}

#[test]
fn only_a_peer_of_the_servers_own_user_is_permitted() {
    assert!(permits(501, 501));
    assert!(!permits(0, 501));
    assert!(!permits(502, 501));
}

#[test]
fn a_server_ends_after_its_idle_time_and_a_second_one_does_not_start() {
    let dir = text_project();
    let runtime = runtime();
    let mut first = Serving::start(dir.path(), runtime.path(), "1500ms");

    let second = lighthouse(dir.path(), runtime.path())
        .arg("serve")
        .output()
        .unwrap();
    assert!(second.status.success());
    assert!(String::from_utf8_lossy(&second.stdout).contains("already serving"));

    let until = Instant::now() + Duration::from_secs(15);
    let ended = loop {
        if let Some(status) = first.child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < until, "the idle server did not end");
        thread::sleep(Duration::from_millis(50));
    };
    assert!(ended.success());
    assert!(!socket(dir.path(), runtime.path()).exists());
    assert!(!answers(dir.path(), runtime.path()));
}

/// What the program says it is: asked of a server of its own.
fn build_of_the_program() -> Hello {
    let (dir, runtime) = (text_project(), runtime());
    let server = Serving::start(dir.path(), runtime.path(), "30s");
    let status = lighthouse(dir.path(), runtime.path())
        .args(["serve", "status"])
        .output()
        .unwrap();
    drop(server);
    let said = String::from_utf8(status.stdout).unwrap();
    let version = said
        .split_once('(')
        .unwrap()
        .1
        .trim_end()
        .trim_end_matches(')');
    Hello {
        version: version.to_owned(),
        protocol: serve::identity().protocol,
    }
}

/// A command asked for in `env`.
fn asked_in(env: &[(&str, &str)]) -> Run {
    Run {
        argv: vec!["lighthouse".to_owned(), "check".to_owned()],
        cwd: PathBuf::from("/"),
        env: env
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        stdin: None,
    }
}

#[test]
fn run_differs_names_a_variable_the_server_does_not_have_as_the_client_does() {
    let server = asked_in(&[("PATH", "/bin"), ("HOME", "/home/me")]).env;

    let same = asked_in(&[("PATH", "/bin"), ("HOME", "/home/me")]);
    let trust_only_here = asked_in(&[
        ("PATH", "/bin"),
        ("HOME", "/home/me"),
        ("LIGHTHOUSE_TRUST", "1"),
    ]);
    let other_path = asked_in(&[("PATH", "/usr/bin"), ("HOME", "/home/me")]);
    let unset_here = asked_in(&[("PATH", "/bin")]);

    assert_eq!(same.differs(&server), None);
    assert_eq!(
        trust_only_here.differs(&server),
        Some("LIGHTHOUSE_TRUST".to_owned())
    );
    assert_eq!(other_path.differs(&server), Some("PATH".to_owned()));
    assert_eq!(unset_here.differs(&server), Some("HOME".to_owned()));
}

#[test]
fn run_decides_names_what_a_program_inherits_and_what_lighthouse_reads() {
    for name in [
        "PATH",
        "HOME",
        "TMPDIR",
        "LANG",
        "USER",
        "LOGNAME",
        "NO_COLOR",
        "XDG_CONFIG_HOME",
        "LIGHTHOUSE_TRUST",
    ] {
        assert!(Run::decides(name), "{name}");
    }
    assert!(!Run::decides("TERM"));
}
