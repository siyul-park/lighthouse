//! The text each backend is given, per kind of caller.

use std::{fs, path::PathBuf};

use lighthouse_process::{
    Bridge, DENIED, Denied, Plan, Policy, Profile, Sandbox, bwrap_argv, is_project_secret,
    seatbelt_profile, secrets,
};

fn path(text: &str) -> PathBuf {
    PathBuf::from(text)
}

fn dir(text: &str) -> Denied {
    Denied {
        path: path(text),
        dir: true,
    }
}

fn file(text: &str) -> Denied {
    Denied {
        path: path(text),
        dir: false,
    }
}

/// What a command check gets: the project read-only, the user cache to write,
/// and the hosts of `[sandbox] network`.
fn command_check() -> Plan {
    Plan {
        writes: vec![path("/tmp/run-1"), path("/home/u/.cache")],
        deny: vec![dir("/home/u/.ssh"), file("/proj/.env")],
        port: Some(40000),
        bridge: None,
    }
}

/// What a command fix and a formatter get: their scratch directory, no network.
fn scratch_only() -> Plan {
    Plan {
        writes: vec![path("/tmp/run-2"), path("/tmp/scratch")],
        deny: vec![dir("/home/u/.ssh")],
        port: None,
        bridge: None,
    }
}

/// What an edit agent gets: the scratch copy and an isolated home to write,
/// and the hosts of its provider, reached through a bridge on Linux.
fn edit_agent() -> Plan {
    Plan {
        writes: vec![path("/tmp/run-3"), path("/tmp/copy"), path("/tmp/home")],
        deny: vec![dir("/home/u/.ssh"), file("/home/u/.netrc")],
        port: Some(40001),
        bridge: Some(Bridge {
            exe: path("/usr/bin/lighthouse"),
            socket: path("/tmp/run-3/proxy.sock"),
        }),
    }
}

#[test]
fn seatbelt_reads_all_but_the_secrets_and_writes_the_list() {
    let profile = seatbelt_profile(&command_check());

    assert_eq!(
        profile,
        r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow sysctl-read)
(allow mach-lookup)
(allow ipc-posix-shm)
(allow file-ioctl)
(allow file-read*)
(deny file-read* (subpath "/home/u/.ssh"))
(deny file-read* (subpath "/proj/.env"))
(allow file-write* (literal "/dev/null") (literal "/dev/tty") (literal "/dev/stdout") (literal "/dev/stderr") (subpath "/tmp/run-1") (subpath "/home/u/.cache"))
(allow network-outbound (remote ip "localhost:40000"))
"#
    );
}

#[test]
fn seatbelt_without_hosts_has_no_network_rule() {
    let profile = seatbelt_profile(&scratch_only());

    assert!(!profile.contains("network"), "{profile}");
    assert!(profile.contains(r#"(subpath "/tmp/scratch")"#));
}

#[test]
fn seatbelt_escapes_the_paths_it_quotes() {
    let plan = Plan {
        writes: vec![path(r#"/tmp/a"b\c"#)],
        ..Plan::default()
    };

    let profile = seatbelt_profile(&plan);

    assert!(profile.contains(r#"(subpath "/tmp/a\"b\\c")"#), "{profile}");
}

#[test]
fn bubblewrap_binds_the_host_read_only_and_covers_the_secrets() {
    let argv = bwrap_argv(&scratch_only(), &["fmt".to_owned(), "-x".to_owned()]);

    assert_eq!(
        argv.join(" "),
        "bwrap --ro-bind / / --dev /dev --proc /proc --tmpfs /home/u/.ssh \
         --bind /tmp/run-2 /tmp/run-2 --bind /tmp/scratch /tmp/scratch \
         --unshare-all --die-with-parent --new-session -- fmt -x"
    );
}

#[test]
fn bubblewrap_masks_a_file_with_the_null_device() {
    let plan = Plan {
        deny: vec![file("/proj/.env")],
        ..Plan::default()
    };

    let argv = bwrap_argv(&plan, &["true".to_owned()]);

    assert!(
        argv.join(" ").contains("--ro-bind /dev/null /proj/.env"),
        "{argv:?}"
    );
}

#[test]
fn bubblewrap_starts_the_program_under_the_bridge_when_it_reaches_hosts() {
    let argv = bwrap_argv(&edit_agent(), &["agent".to_owned(), "run".to_owned()]);

    assert_eq!(
        argv.join(" "),
        "bwrap --ro-bind / / --dev /dev --proc /proc --tmpfs /home/u/.ssh \
         --ro-bind /dev/null /home/u/.netrc \
         --bind /tmp/run-3 /tmp/run-3 --bind /tmp/copy /tmp/copy --bind /tmp/home /tmp/home \
         --unshare-all --die-with-parent --new-session -- \
         /usr/bin/lighthouse internal bridge /tmp/run-3/proxy.sock 40001 -- agent run"
    );
}

#[test]
fn the_secrets_are_the_fixed_list_below_home_and_the_files_of_the_project() {
    let root = tempfile::tempdir().unwrap();
    for name in [".env", ".env.local", ".envrc", "env", "main.rs"] {
        fs::write(root.path().join(name), "").unwrap();
    }

    let found = secrets(Some(&path("/home/u")), root.path());

    for denied in DENIED {
        assert!(found.contains(&path("/home/u").join(denied)), "{denied}");
    }
    let project = |name: &str| root.path().join(name);
    assert!(found.contains(&project(".env")));
    assert!(found.contains(&project(".env.local")));
    assert!(found.contains(&project(".lighthouse/local.toml")));
    assert!(!found.contains(&project(".envrc")));
    assert!(!found.contains(&project("main.rs")));
}

#[test]
fn a_project_secret_is_an_environment_file_at_the_root_or_the_local_settings() {
    for secret in [".env", ".env.local", ".lighthouse/local.toml"] {
        assert!(is_project_secret(&path(secret)), "{secret}");
    }
    for plain in [
        ".envrc",
        "src/.env",
        ".lighthouse/decisions/a.yaml",
        "main.rs",
    ] {
        assert!(!is_project_secret(&path(plain)), "{plain}");
    }
}

#[test]
fn a_profile_starts_with_nothing_to_write_and_nowhere_to_go() {
    let root = tempfile::tempdir().unwrap();

    let profile = Profile::new(root.path());

    assert!(profile.writes.is_empty());
    assert!(profile.network.is_empty());
    assert!(
        profile
            .deny
            .contains(&root.path().join(".lighthouse/local.toml"))
    );
}

#[test]
fn a_profile_that_caches_writes_the_cache_directory_of_the_user() {
    let root = tempfile::tempdir().unwrap();

    let profile = Profile::new(root.path()).caching();

    let home = std::env::var_os("HOME").is_some();
    assert_eq!(profile.writes.len(), usize::from(home), "{profile:?}");
}

#[test]
fn a_policy_that_is_off_runs_the_program_without_a_sandbox() {
    let profile = Profile::default();
    let on = Policy::default();
    let off = Policy {
        enabled: false,
        ..Policy::default()
    };

    assert!(matches!(on.sandbox(&profile), Sandbox::On(_)));
    assert!(matches!(off.sandbox(&profile), Sandbox::Off));
}
