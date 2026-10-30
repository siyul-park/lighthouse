//! Trust is the user's: recorded for exactly what the project would run.

use crate::fixture::{grant, project, read, session, withdraw, write};

#[test]
fn trust() {
    let dir = project("error");
    let before = session(dir.path());
    assert!(!before.trusted());

    let file = grant(&before);
    assert!(file.ends_with("trust.toml"), "{file:?}");
    assert!(session(dir.path()).trusted());

    // The decision's fix is a command: changing it asks again.
    let short = read(dir.path(), ".lighthouse/decisions/short.yaml");
    write(
        dir.path(),
        ".lighthouse/decisions/short.yaml",
        &short.replace("head -n 2", "head -n 1"),
    );
    assert!(!session(dir.path()).trusted());
}

#[test]
fn revoke() {
    let dir = project("error");
    let before = session(dir.path());
    assert!(!withdraw(&before.root));

    grant(&before);
    assert!(session(dir.path()).trusted());
    assert!(withdraw(&before.root));
    assert!(!session(dir.path()).trusted());
}

#[test]
fn basis_trusted() {
    let dir = project("error");
    let before = session(dir.path());
    assert!(!before.basis.trusted(&before.root));
    assert!(
        before
            .basis
            .commands
            .iter()
            .any(|c| c.contains("fixer of local/short")),
        "{:?}",
        before.basis.commands
    );
    grant(&before);
    assert!(before.basis.trusted(&before.root));
}
