//! How the check and the fix of a decision run on this machine, for this
//! project.

use lighthouse_report::{Aim, CheckBy, FixBy};
use lighthouse_session::Fixing;

use crate::fixture::{project, session, trusted};

#[test]
fn fixing_of() {
    let dir = project("error");
    let untrusted = Fixing::of(&session(dir.path())).unwrap();
    let session = session(dir.path());
    let decision = session.catalog.decision("local/short").unwrap();
    let described = untrusted
        .describe(decision, Aim::Decision("local/short"))
        .unwrap();
    // The state is read from the project: its fix is a command, and the user
    // has not trusted the project to run it.
    assert!(!described.fix.unwrap().runs);

    let allowed = Fixing::of(&trusted(dir.path())).unwrap();
    let described = allowed
        .describe(decision, Aim::Decision("local/short"))
        .unwrap();
    assert!(described.fix.unwrap().runs);
}

#[test]
fn fixing_all() {
    let dir = project("error");
    let session = trusted(dir.path());
    let fixing = Fixing::of(&session).unwrap();
    let all = fixing.all(
        &session.catalog,
        ["local/short", "core/max-lines", "local/none"],
    );
    // A decision that is not in the catalog has no descriptor.
    assert!(all.keys().eq(["core/max-lines", "local/short"]));
    let short = &all["local/short"];
    assert_eq!(short.check.by, CheckBy::Cel);
    assert_eq!(short.fix.as_ref().unwrap().by, FixBy::Command);
}

#[test]
fn fixing_describe() {
    let dir = project("error");
    let session = trusted(dir.path());
    let fixing = Fixing::of(&session).unwrap();
    let decision = session.catalog.decision("local/short").unwrap();

    let whole = fixing
        .describe(decision, Aim::Decision("local/short"))
        .unwrap();
    assert_eq!(whole.check.by, CheckBy::Cel);
    assert!(whole.check.runs);
    let fix = whole.fix.unwrap();
    assert!(fix.runs && fix.why.is_none());
    assert!(!whole.brief, "a safe fix that runs needs no brief");

    // The application is aimed at what was asked for.
    let one = fixing
        .describe(decision, Aim::Findings(&["abcdef0123"]))
        .unwrap();
    assert_ne!(one.fix.unwrap().apply, fix.apply);

    // An `unsafe` fix asks for a brief.
    let warned = trusted(project("warn").path());
    let fixing = Fixing::of(&warned).unwrap();
    let decision = warned.catalog.decision("local/short").unwrap();
    assert!(
        fixing
            .describe(decision, Aim::Decision("local/short"))
            .unwrap()
            .brief
    );
}

#[test]
fn fixing() {
    let dir = project("error");
    let session = session(dir.path());
    let fixing = Fixing::of(&session).unwrap();
    // The fix that cannot run says why.
    let decision = session.catalog.decision("local/short").unwrap();
    let fix = fixing
        .describe(decision, Aim::Decision("local/short"))
        .unwrap()
        .fix
        .unwrap();
    assert!(!fix.runs);
    assert!(fix.why.unwrap().to_string().contains("trust"));
}
