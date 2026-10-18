//! A sandbox that cannot be had, in a test binary of its own: the simulation
//! is process-wide.

use std::{path::Path, time::Duration};

use lighthouse_process::{
    Invocation, Profile, Sandbox, Simulation, run, sandbox_available, simulate_unavailable,
};

#[test]
fn a_backend_that_cannot_be_used_fails_the_run_and_never_runs_it_unsandboxed() {
    let _gone: Simulation = simulate_unavailable("no backend here");
    let profile = Profile::default();
    let argv = ["echo".to_owned(), "ran".to_owned()];

    let error = run(&Invocation {
        argv: &argv,
        dir: Path::new("."),
        stdin: b"",
        env: None,
        limit: Duration::from_secs(5),
        grace: Duration::from_millis(300),
        max_output: 1024,
        sandbox: Sandbox::On(&profile),
    })
    .unwrap_err();

    assert!(
        error.starts_with("sandbox unavailable: no backend here; "),
        "{error}"
    );
    assert!(error.ends_with("or set [sandbox] enabled = false in a trusted project"));
    assert_eq!(sandbox_available(), Err("no backend here".to_owned()));
}
