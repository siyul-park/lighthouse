//! The contract every program runs under: one environment, one command line
//! and one reading of the exit code, whatever the program is run for.

use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use lighthouse_process::{
    Environment, Home, Job, Policy, Profile, context, expand, expand_value, referenced, variables,
};

fn line(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).to_owned()).collect()
}

/// The variables of a user, looked up by name: what `Environment::new_in`
/// reads instead of the variables of the test process. `PATH` is the
/// process's, which the shell of the program needs.
fn users<const N: usize>(vars: [(&str, &str); N]) -> impl Fn(&str) -> Option<String> {
    let mut vars: BTreeMap<String, String> = vars
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
    vars.insert("PATH".to_owned(), std::env::var("PATH").unwrap());
    move |name| vars.get(name).cloned()
}

fn unsandboxed() -> Policy {
    Policy {
        enabled: false,
        ..Policy::default()
    }
}

/// Runs `script` under `environment` without a sandbox and returns what it
/// ran as.
fn ran(script: &str, environment: &Environment, limit: Duration) -> lighthouse_process::Ran {
    let command = line(&["sh", "-c", script]);
    let policy = unsandboxed();
    let profile = Profile::new(Path::new("."));
    Job {
        command: &command,
        dir: Path::new("."),
        stdin: b"",
        environment,
        timeout: limit,
        policy: &policy,
        profile: &profile,
    }
    .run()
    .unwrap()
}

#[test]
fn the_exit_code_is_read_against_the_codes_that_mean_the_program_ran() {
    let table: [(&str, &[i32], Result<i32, &str>); 6] = [
        ("exit 0", &[0], Ok(0)),
        ("exit 1", &[0], Err("exited 1")),
        ("exit 1", &[0, 1], Ok(1)),
        ("exit 2", &[0, 1], Err("exited 2")),
        ("exit 2", &[0, 2], Ok(2)),
        ("kill -9 $$", &[0, 1], Err("signal")),
    ];
    let environment = Environment::new(&BTreeMap::new(), Vec::new(), Home::User).unwrap();

    for (script, success, want) in table {
        let got = ran(script, &environment, Duration::from_secs(5)).exit(success);

        match want {
            Ok(code) => assert_eq!(got, Ok(code), "{script} in {success:?}"),
            Err(why) => assert!(
                got.as_ref().is_err_and(|e| e.contains(why)),
                "{script} in {success:?}: {got:?}"
            ),
        }
    }
}

#[test]
fn a_program_that_outlives_its_timeout_is_an_error() {
    let environment = Environment::new(&BTreeMap::new(), Vec::new(), Home::User).unwrap();
    let command = line(&["sleep", "5"]);
    let policy = unsandboxed();
    let profile = Profile::new(Path::new("."));

    let error = Job {
        command: &command,
        dir: Path::new("."),
        stdin: b"",
        environment: &environment,
        timeout: Duration::from_millis(300),
        policy: &policy,
        profile: &profile,
    }
    .run()
    .unwrap_err();

    assert!(error.contains("timed out"), "{error}");
}

#[test]
fn the_environment_is_the_allowlist_the_declared_variables_and_lighthouses_own() {
    let users = users([("LIGHTHOUSE_PROGRAM_TEST_LEAK", "leaked")]);
    let declared = BTreeMap::from([("KEPT".to_owned(), "yes".to_owned())]);
    let environment = Environment::new_in(
        &declared,
        context("local/probe", "{}"),
        Home::Own(Path::new("/tmp/home-of-the-run")),
        users,
    )
    .unwrap();

    assert!(
        environment
            .vars()
            .iter()
            .any(|(name, value)| name == "KEPT" && value == "yes"),
        "the declared variable is among the variables of the run"
    );
    let printed = ran("env | sort", &environment, Duration::from_secs(5));
    let text = String::from_utf8_lossy(&printed.stdout).into_owned();
    let names: Vec<&str> = text.lines().filter_map(|l| l.split('=').next()).collect();

    for wanted in [
        "PATH",
        "KEPT",
        "HOME",
        "XDG_CONFIG_HOME",
        "LIGHTHOUSE_DECISION",
        "LIGHTHOUSE_OPTIONS",
        "LIGHTHOUSE_API_VERSION",
    ] {
        assert!(names.contains(&wanted), "{wanted} in {names:?}");
    }
    assert!(!text.contains("leaked"), "the environment was not cleared");
    assert!(text.contains("HOME=/tmp/home-of-the-run"), "{text}");
    for name in &names {
        let known = [
            "PATH", "USER", "LOGNAME", "LANG", "KEPT", "PWD", "SHLVL", "_", "OLDPWD",
        ];
        assert!(
            known.contains(name)
                || name.starts_with("LIGHTHOUSE_")
                || name.starts_with("XDG_")
                || *name == "HOME",
            "`{name}` is not in the allowlist"
        );
    }
}

#[test]
fn a_variable_in_a_value_is_the_users_and_an_unset_one_names_itself() {
    let users = users([("LIGHTHOUSE_PROGRAM_TEST_KEY", "sk-test-value")]);
    let declared = BTreeMap::from([(
        "TOKEN".to_owned(),
        "Bearer ${LIGHTHOUSE_PROGRAM_TEST_KEY}".to_owned(),
    )]);

    let environment = Environment::new_in(&declared, Vec::new(), Home::User, &users).unwrap();

    // The program holds the value; what it prints of it is scrubbed.
    let printed = ran(
        "test \"$TOKEN\" = 'Bearer sk-test-value' && printf ok",
        &environment,
        Duration::from_secs(5),
    );
    assert_eq!(printed.stdout, b"ok");
    assert_eq!(
        environment.scrub("it said sk-test-value twice: sk-test-value"),
        "it said ${LIGHTHOUSE_PROGRAM_TEST_KEY} twice: ${LIGHTHOUSE_PROGRAM_TEST_KEY}"
    );
    assert_eq!(
        referenced(&declared).into_iter().collect::<Vec<_>>(),
        ["LIGHTHOUSE_PROGRAM_TEST_KEY"]
    );

    let unset = BTreeMap::from([(
        "TOKEN".to_owned(),
        "${LIGHTHOUSE_PROGRAM_TEST_UNSET}".to_owned(),
    )]);
    let error = Environment::new_in(&unset, Vec::new(), Home::User, &users).unwrap_err();
    assert!(error.contains("LIGHTHOUSE_PROGRAM_TEST_UNSET"), "{error}");
    assert!(!error.contains("sk-test-value"), "{error}");
}

#[test]
fn a_value_a_program_echoes_on_stderr_is_scrubbed_from_what_the_error_shows() {
    let users = users([("LIGHTHOUSE_PROGRAM_TEST_SECRET", "sk-echoed")]);
    let declared = BTreeMap::from([(
        "TOKEN".to_owned(),
        "${LIGHTHOUSE_PROGRAM_TEST_SECRET}".to_owned(),
    )]);
    let environment = Environment::new_in(&declared, Vec::new(), Home::User, &users).unwrap();

    let failed = ran(
        "echo \"bad key $TOKEN\" >&2; exit 4",
        &environment,
        Duration::from_secs(5),
    );
    let error = failed.exit(&[0]).unwrap_err();

    assert!(error.contains("exited 4"), "{error}");
    assert!(!error.contains("sk-echoed"), "{error}");
    assert!(
        error.contains("${LIGHTHOUSE_PROGRAM_TEST_SECRET}"),
        "{error}"
    );
}

#[test]
fn a_value_a_program_echoes_on_stdout_is_scrubbed_too_unless_stdout_is_data() {
    let users = users([("LIGHTHOUSE_PROGRAM_TEST_STDOUT", "sk-on-stdout")]);
    let declared = BTreeMap::from([(
        "TOKEN".to_owned(),
        "${LIGHTHOUSE_PROGRAM_TEST_STDOUT}".to_owned(),
    )]);
    let environment = Environment::new_in(&declared, Vec::new(), Home::User, &users).unwrap();
    let command = line(&["sh", "-c", "echo \"a/b.go:1: key $TOKEN\""]);
    let policy = unsandboxed();
    let profile = Profile::new(Path::new("."));
    let job = Job {
        command: &command,
        dir: Path::new("."),
        stdin: b"",
        environment: &environment,
        timeout: Duration::from_secs(5),
        policy: &policy,
        profile: &profile,
    };

    let read = String::from_utf8(job.run().unwrap().stdout).unwrap();
    let data = String::from_utf8(job.run_keeping_stdout().unwrap().stdout).unwrap();

    assert_eq!(read, "a/b.go:1: key ${LIGHTHOUSE_PROGRAM_TEST_STDOUT}\n");
    assert!(
        data.contains("sk-on-stdout"),
        "stdout as data is as printed"
    );
}

#[test]
fn only_a_name_in_braces_is_a_variable() {
    assert_eq!(variables("Bearer ${KEY} and $HOME"), Ok(vec!["KEY"]));
    assert_eq!(variables("plain"), Ok(Vec::new()));
    for bad in ["${", "${}", "${1A}", "${A-b}", "${A:-b}"] {
        assert!(variables(bad).is_err(), "{bad}");
    }
    assert!(expand_value("${LIGHTHOUSE_PROGRAM_TEST_NEVER_SET}").is_err());
}

#[test]
fn placeholders_fill_whole_arguments_and_a_value_never_grows_into_an_option() {
    let fill = |arg: &str| match arg {
        "{file}" | "{files}" => Some(vec!["a.go".to_owned(), "b.go".to_owned()]),
        "{line}" => Some(vec!["7".to_owned()]),
        _ => None,
    };

    let got = expand(
        &line(&["tool", "--flag={file}", "{files}", "{line}"]),
        Path::new("/proj"),
        fill,
    )
    .unwrap();

    assert_eq!(got, ["tool", "--flag={file}", "a.go", "b.go", "7"]);
    let dashed = expand(&line(&["tool", "{file}"]), Path::new("/proj"), |_| {
        Some(vec!["-rf".to_owned()])
    })
    .unwrap_err();
    assert!(dashed.contains("starts with `-`"), "{dashed}");
    let own = expand(&line(&["scripts/lint", "x"]), Path::new("/proj"), |_| None).unwrap();
    assert_eq!(
        own,
        ["/proj/scripts/lint", "x"],
        "a relative program is the project's"
    );
}

#[test]
fn a_programs_own_hosts_and_places_are_added_to_the_projects() {
    let project = Policy {
        enabled: true,
        network: vec!["api.project.test".to_owned()],
        write: vec!["~/.cache/project".to_owned()],
    };

    let own = project.with(
        &["api.own.test".to_owned(), "api.project.test".to_owned()],
        &["~/.cache/own".to_owned()],
    );

    assert_eq!(own.network, ["api.project.test", "api.own.test"]);
    assert_eq!(own.write, ["~/.cache/project", "~/.cache/own"]);
    assert!(own.enabled);
    assert_eq!(
        project.network,
        ["api.project.test"],
        "the project's is as it was"
    );
    let profile = own.profile(Path::new("/proj"), Path::new("/tmp/run"));
    assert_eq!(profile.network, ["api.project.test", "api.own.test"]);
}

#[test]
fn run_lines_hands_the_lines_over_and_scrubs_only_stderr() {
    let environment = Environment::new(&BTreeMap::new(), Vec::new(), Home::User).unwrap();
    let command = line(&["sh", "-c", "echo one; echo two; echo err >&2"]);
    let policy = unsandboxed();
    let profile = Profile::new(Path::new("."));
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&seen);

    let ran = Job {
        command: &command,
        dir: Path::new("."),
        stdin: b"",
        environment: &environment,
        timeout: Duration::from_secs(5),
        policy: &policy,
        profile: &profile,
    }
    .run_lines(Arc::new(move |line: &[u8]| {
        sink.lock()
            .unwrap()
            .push(String::from_utf8_lossy(line).into_owned());
    }))
    .unwrap();

    assert_eq!(*seen.lock().unwrap(), ["one", "two"]);
    assert!(ran.stdout.is_empty());
    assert_eq!(ran.stderr, b"err\n");
}
