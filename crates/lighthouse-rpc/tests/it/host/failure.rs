//! What the host does when a plugin that is a script crashes, hangs, floods or
//! answers wrongly.

use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use lighthouse_engine::EXIT_INCOMPLETE;
use lighthouse_plugin::{Registry, Workspace};
use lighthouse_rpc::register;

use super::{
    config,
    fake::{Fake, T, engine_with, index, only_gap, ws},
};

#[test]
fn a_crash_is_incomplete_and_stderr_becomes_notices() {
    let fake = Fake::new("crash");
    let provider = fake.provider(T);
    let ws = Workspace::new(fake.path());
    let indexed = index(provider.as_ref(), &ws, &["a.fake", "b.fake"]);
    let gap = only_gap(&indexed);
    assert_eq!(gap.path, None);
    assert!(gap.reason.contains("plugin `fake`"), "{}", gap.reason);
    assert!(
        gap.reason.contains("2 file(s) not analyzed"),
        "{}",
        gap.reason
    );
    assert!(gap.reason.contains("exit status: 7"), "{}", gap.reason);
    assert!(indexed.fragments.is_empty());
    assert_eq!(
        indexed.notices,
        [
            "plugin `fake` stderr: boom",
            "plugin `fake` stderr: second line"
        ]
    );
    let again = index(provider.as_ref(), &ws, &["a.fake"]);
    assert!(only_gap(&again).reason.contains("exit status: 7"));
}

#[test]
fn a_timeout_kills_the_plugin_and_later_calls_fail_fast() {
    let fake = Fake::new("sleep");
    let provider = fake.provider(Duration::from_secs(3));
    let ws = Workspace::new(fake.path());
    let started = Instant::now();
    let indexed = index(provider.as_ref(), &ws, &["a.fake"]);
    assert!(only_gap(&indexed).reason.contains("timed out after 3s"));
    assert!(started.elapsed() < Duration::from_secs(10));
    let started = Instant::now();
    let again = index(provider.as_ref(), &ws, &["a.fake"]);
    assert!(only_gap(&again).reason.contains("timed out after 3s"));
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[test]
fn malformed_answers_are_incomplete() {
    for (mode, expect) in [
        ("garbage", "malformed"),
        ("badshape", "malformed `lighthouse/index` result"),
        ("badid", "malformed symbol id `nope`"),
    ] {
        let fake = Fake::new(mode);
        let provider = fake.provider(T);
        let indexed = index(provider.as_ref(), &Workspace::new(fake.path()), &["a.fake"]);
        let gap = only_gap(&indexed);
        assert!(gap.reason.contains(expect), "{mode}: {}", gap.reason);
        assert!(indexed.fragments.is_empty());
    }
}

#[test]
fn an_error_response_is_incomplete_but_the_plugin_stays_usable() {
    let fake = Fake::new("reject");
    let provider = fake.provider(T);
    let ws = Workspace::new(fake.path());
    let first = index(provider.as_ref(), &ws, &["a.fake"]);
    assert!(
        only_gap(&first)
            .reason
            .contains("rejected `lighthouse/index` (-32000): no thanks")
    );
    let second = index(provider.as_ref(), &ws, &["a.fake"]);
    assert!(
        only_gap(&second)
            .reason
            .contains("rejected `lighthouse/index`"),
        "still answering"
    );
}

#[test]
fn fragments_for_unrequested_files_are_ignored_with_a_notice() {
    let fake = Fake::new("unrequested");
    let provider = fake.provider(T);
    let indexed = index(provider.as_ref(), &Workspace::new(fake.path()), &["a.fake"]);
    assert!(indexed.fragments.is_empty());
    assert!(
        indexed
            .notices
            .iter()
            .any(|n| n.contains("zzz.fake") && n.contains("not requested"))
    );
    assert!(
        indexed
            .incomplete
            .iter()
            .any(|i| i.reason.contains("no usable fragment"))
    );
}

#[test]
fn the_engine_turns_a_plugin_crash_into_exit_code_3() {
    let fake = Fake::new("crash");
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("a.fake"), "x").unwrap();
    let outcome = engine_with(&fake, project.path()).check(&[], &[]).unwrap();
    assert_eq!(outcome.incomplete.len(), 1);
    assert!(
        outcome.incomplete[0]
            .reason
            .contains("language `fake` failed")
            || outcome.incomplete[0].reason.contains("exit status: 7")
    );
    assert!(outcome.notices.iter().any(|n| n.contains("stderr: boom")));
    assert_eq!(outcome.exit_code(false, false), EXIT_INCOMPLETE);
    assert_eq!(outcome.exit_code(false, true), 0);
}

#[test]
fn fragments_must_be_unique_and_describe_only_their_own_file() {
    let notices_and_gaps = |mode: &str| {
        let fake = Fake::new(mode);
        let provider = fake.provider(T);
        let indexed = index(provider.as_ref(), &ws(&fake), &["a.fake"]);
        (indexed.fragments.len(), indexed.notices, indexed.incomplete)
    };
    let (fragments, notices, gaps) = notices_and_gaps("dupe");
    assert_eq!(fragments, 1);
    assert!(notices.iter().any(|n| n.contains("second fragment")));
    assert_eq!(gaps.len(), 1, "duplicate incomplete entries are merged");
    for mode in ["foreign", "stray"] {
        let (fragments, notices, gaps) = notices_and_gaps(mode);
        assert_eq!(fragments, 0, "{mode}");
        assert!(
            notices.iter().any(|n| n.contains("fragment dropped")),
            "{mode}"
        );
        assert!(
            gaps.iter().any(|g| g.reason.contains("no usable fragment")),
            "{mode}"
        );
    }
}

#[test]
fn a_plugin_that_never_reads_cannot_block_past_the_timeout_or_the_drop() {
    let fake = Fake::new("deaf");
    let timeout = Duration::from_secs(5);
    let provider = fake.provider(timeout);
    let paths: Vec<String> = (0..4000).map(|n| format!("dir/{n:0>90}.fake")).collect();
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    let started = Instant::now();
    let indexed = index(provider.as_ref(), &ws(&fake), &refs);
    let reason = only_gap(&indexed).reason.clone();
    assert!(
        reason.contains("timed out after 5s") && reason.contains("`lighthouse/index`"),
        "{reason}"
    );
    assert!(started.elapsed() < timeout * 3);
    let started = Instant::now();
    drop(provider);
    assert!(started.elapsed() < timeout * 3);
}

#[test]
fn stderr_and_notifications_are_bounded() {
    let fake = Fake::new("noisy");
    let provider = fake.provider(T);
    let indexed = index(provider.as_ref(), &ws(&fake), &["a.fake"]);
    assert!(indexed.notices.len() <= 51, "{}", indexed.notices.len());
    assert!(indexed.notices.iter().all(|n| n.len() < 2300));
    assert!(
        indexed
            .notices
            .iter()
            .any(|n| n.contains("earlier line(s) dropped"))
    );
    assert!(indexed.notices.iter().any(|n| n.ends_with(" ...")));
}

#[test]
fn a_timeout_kills_the_whole_process_group() {
    let fake = Fake::new("grandchild");
    // The fake answers initialize at once and stalls only on index, but the
    // one timeout covers both: keep it far above a loaded machine's
    // process start-up so that only the stalled index can exceed it.
    let provider = fake.provider(Duration::from_secs(6));
    let indexed = index(provider.as_ref(), &ws(&fake), &["a.fake"]);
    assert!(only_gap(&indexed).reason.contains("timed out"));
    let pid = fs::read_to_string(fake.path().join("log.pid")).unwrap();
    let alive = || {
        std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    while alive() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!alive(), "the grandchild survived");
}

#[test]
fn a_plugin_that_dies_during_initialize_is_incomplete_not_a_config_error() {
    let fake = Fake::new("crash_at_init");
    let toml = format!(
        "[{{ id = \"fake\", path = {:?} }}]",
        fake.path().to_str().unwrap()
    );
    let mut registry = Registry::default();
    let registered = register(
        &mut registry,
        &config(&toml),
        Path::new("."),
        Path::new("."),
        &[],
    )
    .expect("not an error");
    assert_eq!(registered.incomplete.len(), 1);
    assert!(registered.incomplete[0].reason.contains("unavailable"));
    assert!(registry.has_plugin("fake"));
    assert_eq!(registry.languages().len(), 0);
}
