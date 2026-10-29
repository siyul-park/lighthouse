//! What the project and the machine allow, said once for a rule, a command, a
//! plugin and a model.

use lighthouse_engine::{Availability, Needs};
use lighthouse_model::Unavailable;
use lighthouse_spec::{Config, Models, Task};

use crate::support::*;

/// The models a project binds: an agent that edits, a classifier of the
/// plugin that serves `local`, and `extra` besides.
fn models(extra: &str) -> Models {
    let config = Config::parse_inline(&format!(
        "[models]\nagent = \"agent/m\"\nclassification = \"local/m\"\n{extra}[models.providers.agent]\ncommand = [\"sh\", \"agent.sh\"]\n"
    ))
    .unwrap();
    config
        .models(lighthouse_test_support::bundled().projects())
        .unwrap()
}

fn edit(decision: &str) -> Needs {
    Needs::Model {
        decision: decision.to_owned(),
        task: Task::Agent,
    }
}

#[test]
fn what_needs_nothing_runs_and_what_nothing_serves_does_not() {
    let models = models("");
    let available = Availability::new(false, false, &models);

    assert_eq!(available.of(&Needs::Nothing), Ok(()));
    assert_eq!(available.of(&Needs::Unserved), Err(Unavailable::NoProvider));
}

#[test]
fn a_program_runs_in_a_trusted_project_only() {
    let models = models("");

    assert_eq!(
        Availability::new(false, false, &models).of(&Needs::Program),
        Err(Unavailable::UntrustedProject)
    );
    assert_eq!(
        Availability::new(true, false, &models).of(&Needs::Program),
        Ok(())
    );
}

#[test]
fn an_agent_that_edits_is_a_program_and_needs_a_binding() {
    let bound = models("");
    let unbound = Config::parse_inline("")
        .unwrap()
        .models(lighthouse_test_support::bundled().projects())
        .unwrap();

    assert_eq!(
        Availability::new(false, false, &bound).of(&edit("a/b")),
        Err(Unavailable::UntrustedProject)
    );
    assert_eq!(
        Availability::new(true, false, &bound).of(&edit("a/b")),
        Ok(())
    );
    assert_eq!(
        Availability::new(true, false, &unbound).of(&edit("a/b")),
        Err(Unavailable::NoAgentBinding)
    );
}

#[test]
fn a_model_that_is_no_program_needs_no_trust() {
    let models = models("");
    let classify = Needs::Model {
        decision: "a/b".to_owned(),
        task: Task::Classification,
    };

    assert_eq!(
        Availability::new(false, false, &models).of(&classify),
        Ok(())
    );
}

#[test]
fn a_budget_of_none_stops_the_models_that_cost() {
    let free = models("");
    let none = models("budget = 0\n");
    let classify = Needs::Model {
        decision: "a/b".to_owned(),
        task: Task::Classification,
    };

    assert_eq!(
        Availability::new(true, false, &free).of(&edit("a/b")),
        Ok(())
    );
    assert_eq!(
        Availability::new(true, false, &none).of(&edit("a/b")),
        Err(Unavailable::NoBudget)
    );
    assert_eq!(
        Availability::new(true, false, &none).of(&classify),
        Ok(()),
        "a model that costs nothing needs no budget"
    );
}

#[test]
fn engine_availability_follows_the_trust_the_engine_was_given() {
    let dir = project("TODO\n");

    let untrusted = engine(&dir, "");
    let trusted = engine(&dir, "").with_trust(true);

    assert_eq!(
        untrusted.availability().of(&Needs::Program),
        Err(Unavailable::UntrustedProject)
    );
    assert_eq!(trusted.availability().of(&Needs::Program), Ok(()));
}
