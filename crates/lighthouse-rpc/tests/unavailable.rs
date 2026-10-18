//! An agent when the sandbox cannot be had, in a test binary of its own: the
//! simulation is process-wide. The edit is refused with the reason, and the
//! program is never run without its sandbox.

use lighthouse_plugin::{EditRequest, ModelClient};
use lighthouse_process::{Policy, Simulation, simulate_unavailable};
use lighthouse_rpc::CommandModel;
use lighthouse_spec::{Config, Projects};

#[test]
fn an_agent_is_refused_when_the_sandbox_is_unavailable_and_never_runs_without_it() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::parse_inline(
        "plugins = []\n[models.providers.p]\ncommand = [\"sh\", \"-c\", \"touch ran\"]\n",
    )
    .unwrap();
    let provider = config
        .models(&Projects::default())
        .unwrap()
        .provider("p")
        .unwrap();
    let agent = CommandModel::new(dir.path(), "p/m", "m", &provider, &Policy::default()).unwrap();
    let _gone: Simulation = simulate_unavailable("no backend here");

    let error = agent
        .edit(&EditRequest {
            prompt: "fix it",
            dir: dir.path(),
        })
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("sandbox unavailable: no backend here"),
        "{error}"
    );
    assert!(!dir.path().join("ran").exists(), "the agent ran");
}
