//! A sandbox that cannot be had, in a test binary of its own: the simulation
//! is process-wide.

use lighthouse_engine::{Availability, Needs};
use lighthouse_model::Unavailable;
use lighthouse_process::{Simulation, simulate_unavailable};
use lighthouse_spec::{Catalog, Config};

#[test]
fn a_program_does_not_run_where_the_sandbox_it_is_asked_for_does_not_work() {
    let _gone: Simulation = simulate_unavailable("no backend here");
    let models = Config::parse_inline("")
        .unwrap()
        .models(Catalog::bundled().projects())
        .unwrap();

    assert_eq!(
        Availability::new(true, true, &models).of(&Needs::Program),
        Err(Unavailable::SandboxUnavailable)
    );
    assert_eq!(
        Availability::new(true, false, &models).of(&Needs::Program),
        Ok(()),
        "a project that turned the sandbox off does not need it"
    );
}

/// The CEL environment is installed before any test reads a decision.
#[ctor::ctor(unsafe)]
fn compose() {
    lighthouse_test_support::cel();
}
