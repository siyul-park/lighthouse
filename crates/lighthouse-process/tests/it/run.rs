use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use lighthouse_process::{Invocation, Sandbox, run, run_lines};

fn spec<'a>(argv: &'a [String], limit: Duration) -> Invocation<'a> {
    Invocation {
        argv,
        dir: Path::new("."),
        stdin: b"",
        env: None,
        limit,
        grace: Duration::from_millis(300),
        max_output: 1024,
        sandbox: Sandbox::Off,
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).to_owned()).collect()
}

#[test]
fn output_and_exit_code_are_returned_and_capped() {
    let line = argv(&["sh", "-c", "yes | head -c 100000; echo oops >&2; exit 3"]);

    let out = run(&spec(&line, Duration::from_secs(5))).unwrap();

    assert!(!out.success);
    assert_eq!(out.code, Some(3));
    assert_eq!(out.stdout.len(), 1024);
    assert_eq!(out.stderr, b"oops\n");
    assert!(out.truncated, "what was dropped is said");
}

#[test]
fn output_within_the_cap_is_not_truncated() {
    let line = argv(&["sh", "-c", "echo fine"]);

    let out = run(&spec(&line, Duration::from_secs(5))).unwrap();

    assert!(!out.truncated);
}

#[test]
fn run_lines_hands_over_every_line_of_an_output_larger_than_the_cap() {
    // 3000 lines of 1 KiB each, three times what `max_output` keeps.
    let line = argv(&[
        "sh",
        "-c",
        "yes \"$(head -c 1023 /dev/zero | tr '\\0' x)\" | head -n 3000",
    ]);
    let seen = Arc::new(Mutex::new((0usize, 0usize)));
    let sink = Arc::clone(&seen);

    let out = run_lines(
        &spec(&line, Duration::from_secs(20)),
        Arc::new(move |line: &[u8]| {
            let mut seen = sink.lock().unwrap();
            seen.0 += 1;
            seen.1 += line.len();
        }),
    )
    .unwrap();

    assert_eq!(*seen.lock().unwrap(), (3000, 3000 * 1023));
    assert!(out.stdout.is_empty(), "the lines are not kept");
    assert!(!out.truncated);
}

#[test]
fn run_lines_ends_a_last_line_that_has_no_line_break() {
    let line = argv(&["sh", "-c", "printf 'a\\nb'"]);
    let seen = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let sink = Arc::clone(&seen);

    run_lines(
        &spec(&line, Duration::from_secs(5)),
        Arc::new(move |line: &[u8]| sink.lock().unwrap().push(line.to_vec())),
    )
    .unwrap();

    assert_eq!(*seen.lock().unwrap(), [b"a".to_vec(), b"b".to_vec()]);
}

#[test]
fn run_lines_drops_a_line_longer_than_the_limit_and_says_so() {
    let line = argv(&[
        "sh",
        "-c",
        "head -c 9000000 /dev/zero | tr '\\0' x; echo; echo ok",
    ]);
    let seen = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let sink = Arc::clone(&seen);

    let out = run_lines(
        &spec(&line, Duration::from_secs(20)),
        Arc::new(move |line: &[u8]| sink.lock().unwrap().push(line.to_vec())),
    )
    .unwrap();

    assert_eq!(*seen.lock().unwrap(), [b"ok".to_vec()]);
    assert!(out.truncated);
}

#[test]
fn a_daemon_the_program_leaves_behind_does_not_hold_the_run_up() {
    let line = argv(&["sh", "-c", "sleep 30 & echo started"]);
    let started = Instant::now();

    let out = run(&spec(&line, Duration::from_secs(10))).unwrap();

    assert_eq!(out.stdout, b"started\n");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn an_environment_is_cleared_to_what_is_given() {
    let line = argv(&["/usr/bin/env"]);
    let env = [("ONLY".to_owned(), "this".to_owned())];
    let mut s = spec(&line, Duration::from_secs(5));
    s.env = Some(&env);

    let out = run(&s).unwrap();

    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ONLY=this");
}

#[cfg(unix)]
#[test]
fn a_program_that_ignores_sigterm_is_killed_after_the_grace_period() {
    let line = argv(&["sh", "-c", "trap '' TERM; sleep 30"]);
    let started = Instant::now();

    let error = run(&spec(&line, Duration::from_millis(200))).unwrap_err();

    assert!(error.contains("timed out"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[cfg(unix)]
#[test]
fn a_program_that_obeys_sigterm_stops_before_the_grace_runs_out() {
    let line = argv(&["sleep", "30"]);
    let started = Instant::now();

    assert!(run(&spec(&line, Duration::from_millis(100))).is_err());

    assert!(started.elapsed() < Duration::from_millis(280));
}

#[test]
fn a_program_that_cannot_start_is_an_error() {
    let line = argv(&["lighthouse-no-such-program"]);

    assert!(
        run(&spec(&line, Duration::from_secs(1)))
            .unwrap_err()
            .contains("cannot start")
    );
}

#[cfg(unix)]
#[test]
fn reap_kills_a_running_program_and_collects_it() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();

    lighthouse_process::reap(&mut child);

    assert!(child.try_wait().unwrap().is_some());
}

#[cfg(unix)]
#[test]
fn a_grandchild_that_holds_the_pipes_open_does_not_hang_the_run() {
    let line = argv(&["sh", "-c", "sleep 30 & echo done"]);
    let started = Instant::now();

    let out = run(&spec(&line, Duration::from_secs(10))).unwrap();

    assert!(out.success);
    assert_eq!(out.stdout, b"done\n");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[cfg(unix)]
#[test]
fn a_timeout_kills_the_whole_group_even_when_the_leader_obeys_sigterm() {
    let line = argv(&["sh", "-c", "(trap '' TERM; sleep 30) & wait"]);
    let started = Instant::now();

    let error = run(&spec(&line, Duration::from_millis(100))).unwrap_err();

    assert!(error.contains("timed out"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[cfg(unix)]
fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[cfg(unix)]
#[test]
fn a_timeout_kills_the_group_when_the_leader_exits_within_the_grace_period() {
    let dir = tempfile::tempdir().unwrap();
    let pid = dir.path().join("pid");
    let script = format!(
        "trap 'exit 0' TERM; (trap '' TERM; exec sh -c 'echo $$ > {}; exec sleep 30') & wait",
        pid.display()
    );
    let line = argv(&["sh", "-c", &script]);

    let error = run(&spec(&line, Duration::from_millis(500))).unwrap_err();

    assert!(error.contains("timed out"), "{error}");
    let pid = std::fs::read_to_string(&pid).unwrap().trim().to_owned();
    let gone = (0..40).any(|_| {
        std::thread::sleep(Duration::from_millis(50));
        !alive(&pid)
    });
    assert!(gone, "the grandchild {pid} outlived the run");
}
