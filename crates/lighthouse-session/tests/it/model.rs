//! The models of a project: what they are bound to, grading a check against
//! labels, training, attaching and detaching a model, and making one again.
//! The model is the `fake-model` plugin: it fails what says `maybe_bad`,
//! passes what says `maybe_good` and is unsure of the rest.

use std::{fs, path::Path};

use lighthouse_engine::ModelOptions;
use lighthouse_session::{
    AgentKind, Attribution, CheckRequest,
    model::{Binding, EvalRequest, Labelling, RebuildRequest, Repo, TrainRequest},
};
use tempfile::TempDir;

use crate::fixture::{canonical, grant, in_dir, isolate, session, write};

const DECISION: &str = "local/naming-ban";

const BAN: &str = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/naming-ban
  uid: 7d1c2a40-8b3e-4f6a-a2d5-9e0f1b2c3d4e
spec:
  title: Ban
  context: A file named like a ban word is a violation.
  scope: { subject: file }
  requirement: A file MUST NOT be named like a ban word.
  severity: warn
  check:
    type: cel
    select: file
    where: file.path == "bad_one.txt"
    abstain: file.path.startsWith("maybe")
    message: "{{ file.path }} is banned"
  examples:
    - name: bad
      language: text
      kind: invalid
      files: [{ path: bad_one.txt, body: "bad_one\n" }]
      expect: [{ line: 1 }]
    - name: good
      language: text
      kind: valid
      files: [{ path: fine.txt, body: "fine\n" }]
"#;

/// The plugin as the project lists it, and the provider that speaks to it.
fn plugin() -> String {
    format!(
        "{{ id = \"fake-model\", path = {:?} }}",
        lighthouse_test_support::fake_model().to_str().unwrap()
    )
}

const PROVIDER: &str = "[models.providers.fake]\nplugin = \"fake-model\"\noptions = { tag = 1 }\n";

/// A project with the decision, five files, and `models` for its spec.
fn project(models: &str) -> TempDir {
    isolate();
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [\"core\", \"local\", {}]\n[rules]\n\"local/naming-ban\" = \"warn\"\n{models}",
            plugin()
        )),
    );
    write(dir.path(), ".lighthouse/decisions/ban.yaml", BAN);
    for (name, text) in [
        ("bad_one.txt", "bad_one\n"),
        ("maybe_bad.txt", "maybe_bad\n"),
        ("maybe_good.txt", "maybe_good\n"),
        ("maybe_unsure.txt", "maybe_unsure\n"),
        ("fine.txt", "fine\n"),
    ] {
        write(dir.path(), name, text);
    }
    // The plugin is a program the project brings: the user trusts it.
    grant(&session(dir.path()));
    dir
}

/// The model bound to every decision nothing else binds.
fn bound() -> TempDir {
    project(&format!(
        "[models]\nclassification = \"fake/m1\"\n{PROVIDER}"
    ))
}

/// A model provider declared and bound to nothing.
fn declared() -> TempDir {
    project(&format!("[models]\n{PROVIDER}"))
}

/// A table that calls what the model fails right and the unsure file wrong.
fn labelled(dir: &Path) {
    let session = session(dir);
    let run = lighthouse_session::check(
        &session,
        &CheckRequest {
            paths: vec![session.root.clone()],
            models: ModelOptions {
                off: true,
                all: true,
                ..ModelOptions::default()
            },
            detached: true,
            ..CheckRequest::default()
        },
    )
    .unwrap();
    let by_file = |name: &str| {
        run.findings()
            .iter()
            .find(|d| d.file == Path::new(name))
            .unwrap_or_else(|| panic!("no finding for {name}"))
            .fingerprint
            .as_str()
            .to_owned()
    };
    write(
        dir,
        "labels.tsv",
        &format!(
            "fingerprint\trepo\tlocation\tverdict\treason\n{}\tr\tx\tTP\t\n{}\tr\ty\tTP\t\n{}\tr\tz\tFP\t\n",
            by_file("bad_one.txt"),
            by_file("maybe_bad.txt"),
            by_file("maybe_unsure.txt")
        ),
    );
}

fn request(dir: &Path, model: Option<&str>) -> EvalRequest {
    EvalRequest {
        decision: DECISION.to_owned(),
        labelling: Labelling {
            repos: Vec::new(),
            table: Some(dir.join("labels.tsv")),
            config: None,
        },
        candidate: None,
        model: model.map(str::to_owned),
        audit: None,
    }
}

fn kim() -> Attribution {
    Attribution::new(AgentKind::Person, Some("kim".to_owned()))
}

/// The model trained on the labels, as `<provider>/<model>@<digest>`.
fn trained(dir: &Path) -> String {
    let labelling = request(dir, None).labelling;
    let done = in_dir(dir, || {
        lighthouse_session::model::train(&TrainRequest {
            decision: DECISION.to_owned(),
            model: Some("fake/m1".to_owned()),
            from_model: None,
            labelling,
        })
    })
    .unwrap();
    format!("fake/m1@{}", done.result.unwrap().result_files[0])
}

fn attached(dir: &Path) -> String {
    let model = trained(dir);
    let done = in_dir(dir, || {
        lighthouse_session::model::attach(&request(dir, Some(&model)), kim())
    })
    .unwrap();
    assert!(done.reasons.is_empty(), "{:?}", done.reasons);
    model
}

#[test]
fn bound_models() {
    let dir = bound();
    let session = session(dir.path());
    let all = lighthouse_session::model::bound_models(&session, &[]).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].model, "fake/m1");
    assert_eq!(all[0].binding, Binding::Classification);
    assert_eq!(all[0].decision, None);
    assert!(!all[0].remote, "a plugin on this machine");

    // A model bound to a decision is not listed for the others.
    let none = lighthouse_session::model::bound_models(&session, &["local/naming-ban".to_owned()])
        .unwrap();
    assert!(none.is_empty(), "{none:?}");
}

#[test]
fn remote_bound() {
    let local = bound();
    assert!(!lighthouse_session::model::remote_bound(&session(
        local.path()
    )));

    let remote = project(
        "[models]\nclassification = \"web/gpt\"\n[models.providers.web]\nprotocol = \"openai\"\nurl = \"https://api.example.com/v1\"\n",
    );
    let web = session(remote.path());
    assert!(
        lighthouse_session::model::remote_bound(&web),
        "{:?}",
        lighthouse_session::model::bound_models(&web, &[])
    );
    assert!(lighthouse_session::model::bound_models(&web, &[]).unwrap()[0].remote);
}

#[test]
fn repo_parse() {
    let repo = Repo::parse("lh=/work/lighthouse").unwrap();
    assert_eq!(repo.name.as_deref(), Some("lh"));
    assert_eq!(repo.dir, Path::new("/work/lighthouse"));
    let refused = Repo::parse("/work/lighthouse").err().unwrap();
    assert!(refused.to_string().contains("is not name=dir"), "{refused}");
}

#[test]
fn repo() {
    // A repository other than the working directory's is where the labels are
    // read, and the rows of the table that name it are its labels.
    let dir = bound();
    labelled(dir.path());
    let elsewhere = tempfile::tempdir().unwrap();
    let labelled_in = |name: &str| {
        let mut asked = request(dir.path(), None);
        asked.labelling.repos =
            vec![Repo::parse(&format!("{name}={}", canonical(dir.path()).display())).unwrap()];
        in_dir(elsewhere.path(), || lighthouse_session::model::eval(&asked))
            .unwrap()
            .labelled
    };
    assert_eq!(
        labelled_in("r"),
        4,
        "three rows of the table and an example"
    );
    assert_eq!(labelled_in("other"), 1, "no row names this repository");
}

#[test]
fn labelling() {
    let dir = bound();
    labelled(dir.path());
    let mut asked = request(dir.path(), None);
    // Without a table the decision has only its examples to be graded on.
    asked.labelling.table = None;
    let report = in_dir(dir.path(), || lighthouse_session::model::eval(&asked)).unwrap();
    assert_eq!(report.labelled, 1, "the invalid example's subject");
    // A repository that is not there is an error that names it.
    asked.labelling.repos = vec![Repo::parse("gone=/no/such/dir").unwrap()];
    let missing = in_dir(dir.path(), || lighthouse_session::model::eval(&asked));
    assert!(missing.err().unwrap().to_string().contains("/no/such/dir"));
}

#[test]
fn eval() {
    let dir = bound();
    labelled(dir.path());
    let report = in_dir(dir.path(), || {
        lighthouse_session::model::eval(&request(dir.path(), None))
    })
    .unwrap();
    assert_eq!(report.decision, DECISION);
    // Three reviewed rows and the one subject the invalid example shows.
    assert_eq!(report.labelled, 4);
    let precision = report.current.metrics.precision.unwrap();
    assert_eq!((precision.hits, precision.of), (3, 3));
    assert_eq!(report.current.usage.model_decided, 1);
    assert_eq!(report.meets_policy, Some(true));

    let both = request(dir.path(), Some("fake/m1"));
    let mut exclusive = both;
    exclusive.candidate = Some(dir.path().join(".lighthouse/decisions/ban.yaml"));
    let refused = in_dir(dir.path(), || lighthouse_session::model::eval(&exclusive));
    assert!(refused.err().unwrap().to_string().contains("exclusive"));
}

#[test]
fn train() {
    let dir = declared();
    labelled(dir.path());
    let labelling = request(dir.path(), None).labelling;
    let done = in_dir(dir.path(), || {
        lighthouse_session::model::train(&TrainRequest {
            decision: DECISION.to_owned(),
            model: Some("fake/m1".to_owned()),
            from_model: None,
            labelling,
        })
    })
    .unwrap();
    assert_eq!(done.model, "fake/m1");
    assert!(
        done.training_file.starts_with("sha256:"),
        "{}",
        done.training_file
    );
    assert_eq!(
        format!("{:?}", done.result.unwrap().status).to_lowercase(),
        "succeeded"
    );
    // The dataset the model learned from is kept, named by its digest.
    let sets = fs::read_dir(dir.path().join(".lighthouse/datasets/sha256")).unwrap();
    assert_eq!(sets.count(), 1);
}

#[test]
fn attach() {
    let dir = declared();
    labelled(dir.path());
    let model = trained(dir.path());

    let done = in_dir(dir.path(), || {
        lighthouse_session::model::attach(&request(dir.path(), Some(&model)), kim())
    })
    .unwrap();
    assert!(done.reasons.is_empty(), "{:?}", done.reasons);
    assert_eq!(done.model, model);
    assert!(done.revision.is_some());
    assert_eq!(done.evaluation["labelled"], 4);

    // From then on the project binds it to the decision.
    let session = session(dir.path());
    let listed = lighthouse_session::model::bound_models(&session, &[]).unwrap();
    assert!(
        listed
            .iter()
            .any(|m| m.binding == Binding::Attached && m.model == model)
    );
    let again = in_dir(dir.path(), || {
        lighthouse_session::model::attach(&request(dir.path(), Some(&model)), kim())
    })
    .unwrap();
    assert!(again.revision.is_none());
    assert!(again.reasons.iter().any(|r| r.contains("already attached")));

    let nameless = in_dir(dir.path(), || {
        lighthouse_session::model::attach(&request(dir.path(), None), kim())
    });
    assert!(nameless.err().unwrap().to_string().contains("--model"));
}

#[test]
fn detach() {
    let dir = declared();
    labelled(dir.path());
    let nothing = in_dir(dir.path(), || {
        lighthouse_session::model::detach(None, DECISION, kim())
    });
    assert!(
        nothing
            .err()
            .unwrap()
            .to_string()
            .contains("no model is attached")
    );

    let model = attached(dir.path());
    let done = in_dir(dir.path(), || {
        lighthouse_session::model::detach(None, DECISION, kim())
    })
    .unwrap();
    assert_eq!(
        (done.decision.as_str(), done.model.as_str()),
        (DECISION, model.as_str())
    );
    assert!(!done.revision.is_empty());
    let session = session(dir.path());
    assert!(
        lighthouse_session::model::bound_models(&session, &[])
            .unwrap()
            .iter()
            .all(|m| m.binding != Binding::Attached)
    );
    let unknown = in_dir(dir.path(), || {
        lighthouse_session::model::detach(None, "local/none", kim())
    });
    assert!(
        unknown
            .err()
            .unwrap()
            .to_string()
            .contains("unknown decision")
    );
}

#[test]
fn rebuild() {
    let dir = declared();
    labelled(dir.path());
    let model = attached(dir.path());
    let artifact = {
        let hex = model.split_once("@sha256:").unwrap().1;
        dir.path()
            .join(format!(".lighthouse/models/sha256/{hex}.json"))
    };
    let rebuild = || -> Vec<(String, String)> {
        in_dir(dir.path(), || {
            lighthouse_session::model::rebuild(&RebuildRequest {
                decisions: Vec::new(),
                labelling: request(dir.path(), None).labelling,
            })
        })
        .unwrap()
        .into_iter()
        .map(|row| {
            (
                row.model,
                serde_json::to_value(row.status)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
        })
        .collect()
    };

    assert_eq!(rebuild(), [(model.clone(), "present".to_owned())]);

    // A model file that is gone is made again, byte for byte.
    let before = fs::read_to_string(&artifact).unwrap();
    fs::remove_file(&artifact).unwrap();
    assert_eq!(rebuild(), [(model, "rebuilt".to_owned())]);
    assert_eq!(fs::read_to_string(&artifact).unwrap(), before);
}
