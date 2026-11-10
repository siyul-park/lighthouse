//! A project kept loaded between requests: it answers as a cold load does,
//! and sees what changed on disk between them.

use lighthouse_engine::ModelOptions;
use lighthouse_report::Format;
use lighthouse_session::{CheckRequest, Session, Warm};

use crate::fixture::{LONG, project, session, write};

/// The request every test makes: the whole project, remembered nowhere.
fn request(session: &Session) -> CheckRequest {
    CheckRequest {
        paths: vec![session.root.clone()],
        store: false,
        cache: true,
        ..CheckRequest::default()
    }
}

#[test]
fn warm_new() {
    let warm = Warm::new();

    assert!(format!("{warm:?}").contains("loaded: false"));
}

#[test]
fn warm_session() {
    let dir = project("error");
    let mut warm = Warm::new();
    let at = (dir.path(), None);

    let first = warm.session(at, true).unwrap();
    assert!(first.catalog.decision("local/short").is_some());
    assert!(
        warm.session(at, true)
            .unwrap()
            .catalog
            .decision("local/short")
            .is_some()
    );

    let edited = project("error");
    write(
        edited.path(),
        ".lighthouse/decisions/short.yaml",
        "not a decision",
    );
    assert!(warm.session((edited.path(), None), true).is_err());

    let empty = tempfile::tempdir().unwrap();
    let none = warm.session((empty.path(), None), true).err().unwrap();
    assert!(
        none.to_string().contains("no lighthouse.toml found"),
        "{none}"
    );
    assert!(warm.session((empty.path(), None), false).is_ok());
}

#[test]
fn warm_check() {
    let dir = project("error");
    let mut warm = Warm::new();
    let at = (dir.path(), None);
    let cold = |dir: &std::path::Path| {
        let session = session(dir);
        lighthouse_session::check(&session, &request(&session))
            .unwrap()
            .render(Format::Text, None)
    };
    let served = |warm: &mut Warm| {
        warm.check(at, |session| Ok(request(session)))
            .unwrap()
            .render(Format::Text, None)
    };

    for _ in 0..2 {
        assert_eq!(served(&mut warm), cold(dir.path()));
    }
    write(dir.path(), "a.txt", "a\n");
    let clean = served(&mut warm);
    assert_eq!(clean, cold(dir.path()));
    assert!(!clean.contains("local/short"), "{clean}");

    write(dir.path(), "a.txt", LONG);
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project("[rules]\n\"local/short\" = \"warn\"\n"),
    );
    let warned = served(&mut warm);
    assert_eq!(warned, cold(dir.path()));
    assert!(warned.contains("warn local/short"), "{warned}");

    let off = warm
        .check(at, |session| {
            Ok(CheckRequest {
                models: ModelOptions {
                    off: true,
                    ..ModelOptions::default()
                },
                ..request(session)
            })
        })
        .unwrap();
    assert_eq!(off.render(Format::Text, None), warned);
    let uncached = warm
        .check(at, |session| {
            Ok(CheckRequest {
                cache: false,
                ..request(session)
            })
        })
        .unwrap();
    assert_eq!(uncached.render(Format::Text, None), warned);
}

#[test]
fn overlays_stand_in_for_files_for_one_request_only() {
    let dir = project("error");
    let mut warm = Warm::new();
    let at = (dir.path(), None);
    let plain = |warm: &mut Warm| {
        warm.check(at, |session| Ok(request(session)))
            .unwrap()
            .render(Format::Text, None)
    };
    let before = plain(&mut warm);
    assert!(before.contains("a.txt"), "{before}");

    let overlaid = |warm: &mut Warm, texts: &[(&str, &str)]| {
        let overlays = texts
            .iter()
            .map(|(path, text)| (std::path::PathBuf::from(path), (*text).to_owned()))
            .collect();
        warm.check(at, |session| {
            Ok(CheckRequest {
                overlays,
                ..request(session)
            })
        })
        .unwrap()
    };
    assert!(
        overlaid(&mut warm, &[("a.txt", "a\n")])
            .run
            .diagnostics
            .is_empty()
    );
    assert_eq!(
        plain(&mut warm),
        before,
        "an overlay is gone after its request"
    );
    let created = overlaid(&mut warm, &[("b.txt", LONG)]);
    assert_eq!(created.run.diagnostics.len(), 2);
    assert_eq!(plain(&mut warm), before);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        LONG
    );
    assert!(!dir.path().join("b.txt").exists());
}

#[test]
fn an_overlaid_request_that_remembers_or_narrows_to_changes_is_refused() {
    let dir = project("error");
    let mut warm = Warm::new();
    let at = (dir.path(), None);
    let overlays = || [(std::path::PathBuf::from("a.txt"), "a\n".to_owned())].into();
    let remembered = warm.check(at, |session| {
        Ok(CheckRequest {
            overlays: overlays(),
            store: true,
            ..request(session)
        })
    });
    assert!(
        remembered
            .err()
            .unwrap()
            .to_string()
            .contains("never remembered")
    );
    let narrowed = warm.check(at, |session| {
        Ok(CheckRequest {
            overlays: overlays(),
            changed: true,
            ..request(session)
        })
    });
    assert!(narrowed.err().unwrap().to_string().contains("alternatives"));
}
