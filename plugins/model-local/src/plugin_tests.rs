//! The plugin through the protocol's methods, with an embedder that needs no
//! weights: text with `bad` in it points one way, any other the other.

use std::{fs, path::Path};

use lighthouse_protocol::{
    ClientInfo, CreateFineTuningJobRequest, Criteria, Criterion, FineTuneMethod,
    FineTuningJobStatus, Handler, InitializeParams, Instructions, MODELS_DIR, Question, State,
    SystemOneRequest, VERSION, artifact_path, hash::sha256,
};
use serde_json::{Value, json};

use crate::{
    embed::Embedder,
    fixture::{PERSON, file},
    serve::Plugin,
};

fn fake(name: &str) -> Result<Embedder, String> {
    let run = |texts: &[String]| {
        Ok(texts
            .iter()
            .map(|t| {
                let wobble = (t.len() % 7) as f32 * 0.01;
                if t.contains("bad") {
                    vec![1.0, wobble]
                } else {
                    vec![wobble, 1.0]
                }
            })
            .collect())
    };
    Ok(Embedder::new(name, 2, Box::new(run)))
}

fn offline(name: &str) -> Result<Embedder, String> {
    Err(format!("{name} needs weights"))
}

/// `fails` bad and `passes` good subjects, each in a file of its own.
fn dataset(fails: usize, passes: usize) -> String {
    let texts: Vec<(String, bool, String)> = (0..fails)
        .map(|i| (format!("bad {i}"), true, format!("f{i}.go")))
        .chain((0..passes).map(|i| (format!("good {i}"), false, format!("p{i}.go"))))
        .collect();
    let rows: Vec<_> = texts
        .iter()
        .map(|(text, fail, group)| (text.as_str(), *fail, group.as_str(), PERSON))
        .collect();
    file(&rows)
}

/// A plugin started on a project at `root` that holds `dataset`; its digest.
fn project(root: &Path, open: crate::embed::Open, dataset: &str) -> (Plugin, String) {
    let mut plugin = Plugin::new(open);
    plugin
        .initialize(InitializeParams {
            root: root.display().to_string(),
            protocol_version: VERSION.to_owned(),
            client_info: ClientInfo {
                name: "test".to_owned(),
                version: "0".to_owned(),
            },
        })
        .unwrap();
    let hex = sha256(dataset);
    let store = root.join(".lighthouse/datasets/sha256");
    fs::create_dir_all(&store).unwrap();
    fs::write(store.join(format!("{hex}.jsonl")), dataset).unwrap();
    (plugin, hex)
}

fn train_request(root: &Path, hex: &str) -> CreateFineTuningJobRequest {
    CreateFineTuningJobRequest {
        model: "learned".to_owned(),
        training_file: format!("sha256:{hex}"),
        validation_file: None,
        method: FineTuneMethod {
            kind: "supervised".to_owned(),
            supervised: Default::default(),
        },
        seed: 0,
        suffix: None,
        metadata: [
            ("decision", "d/x".to_owned()),
            (
                "datasets",
                root.join(".lighthouse/datasets").display().to_string(),
            ),
            ("dir", root.join(MODELS_DIR).display().to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect(),
        options: serde_json::Map::new(),
    }
}

fn ask(model: &str, key: &str, text: &str) -> SystemOneRequest {
    let side = || Criterion {
        meaning: "m".to_owned(),
        examples: Vec::new(),
    };
    SystemOneRequest {
        state: State {
            language: "go".to_owned(),
            file: "a.go".to_owned(),
            text: text.to_owned(),
            facts: Value::Null,
            neighbors: Vec::new(),
        },
        model: model.to_owned(),
        questions: [(
            key.to_owned(),
            Question {
                kind: "noul".to_owned(),
                instructions: Instructions {
                    requirement: "r".to_owned(),
                    prompt: "r".to_owned(),
                },
                criteria: Criteria {
                    fail: side(),
                    pass: side(),
                },
            },
        )]
        .into(),
        options: serde_json::Map::new(),
    }
}

/// The reference a training gave.
fn trained(plugin: &mut Plugin, root: &Path, hex: &str) -> String {
    let job = plugin.train(train_request(root, hex)).unwrap();
    assert_eq!(
        job.status,
        FineTuningJobStatus::Succeeded,
        "{:?}",
        job.error
    );
    job.fine_tuned_model.unwrap()
}

fn noul(plugin: &mut Plugin, model: &str, key: &str, text: &str) -> Result<f64, String> {
    let response = plugin
        .classify(ask(model, key, text))
        .map_err(|e| e.message)?;
    Ok(response.answers[key].noul)
}

#[test]
fn the_process_lists_the_models_its_manifest_provides() {
    let root = tempfile::tempdir().unwrap();
    let manifest = include_str!("../lighthouse-plugin.toml");
    let result = Plugin::new(fake)
        .initialize(InitializeParams {
            root: root.path().display().to_string(),
            protocol_version: VERSION.to_owned(),
            client_info: ClientInfo {
                name: "test".to_owned(),
                version: "0".to_owned(),
            },
        })
        .unwrap();

    let ids: Vec<String> = result
        .models
        .data
        .iter()
        .map(|m| format!("\"{}\"", m.id))
        .collect();
    assert!(
        manifest.contains(&format!("models = [{}]", ids.join(", "))),
        "{ids:?}"
    );
}

#[test]
fn a_trained_model_answers_its_decision_by_what_it_learned() {
    let root = tempfile::tempdir().unwrap();
    let (mut plugin, hex) = project(root.path(), fake, &dataset(12, 12));
    let model = trained(&mut plugin, root.path(), &hex);

    assert!(model.starts_with("learned@sha256:"), "{model}");
    let bad = noul(&mut plugin, &model, "d/x", "bad one").unwrap();
    let good = noul(&mut plugin, &model, "d/x", "good one").unwrap();
    assert!(bad > 0.8 && good < 0.2, "{bad} {good}");
    assert_eq!(bad, (bad * 1e4).round() / 1e4);
}

#[test]
fn training_twice_gives_the_same_digest_and_the_same_file() {
    let (one, two) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let data = dataset(12, 12);
    let reference = |root: &Path| {
        let (mut plugin, hex) = project(root, fake, &data);
        trained(&mut plugin, root, &hex)
    };

    let model = reference(one.path());
    assert_eq!(model, reference(two.path()));
    let hex = model.strip_prefix("learned@sha256:").unwrap();
    let path = |root: &Path| artifact_path(&root.join(MODELS_DIR), hex);
    assert_eq!(
        fs::read(path(one.path())).unwrap(),
        fs::read(path(two.path())).unwrap()
    );
}

#[test]
fn a_model_answers_only_the_decision_it_was_trained_for() {
    let root = tempfile::tempdir().unwrap();
    let (mut plugin, hex) = project(root.path(), fake, &dataset(12, 12));
    let model = trained(&mut plugin, root.path(), &hex);

    let error = noul(&mut plugin, &model, "d/other", "bad").unwrap_err();
    assert!(error.contains("trained for `d/x`"), "{error}");
}

#[test]
fn a_model_that_is_not_trained_says_to_train_first() {
    let root = tempfile::tempdir().unwrap();
    let (mut plugin, _) = project(root.path(), fake, &dataset(12, 12));

    let error = noul(&mut plugin, "learned", "d/x", "bad").unwrap_err();
    assert!(error.contains("train first"), "{error}");
}

#[test]
fn an_artifact_that_is_missing_or_altered_names_its_path_and_digest() {
    let root = tempfile::tempdir().unwrap();
    let (mut plugin, hex) = project(root.path(), fake, &dataset(12, 12));
    let model = trained(&mut plugin, root.path(), &hex);
    let digest = model.strip_prefix("learned@sha256:").unwrap().to_owned();
    let path = artifact_path(&root.path().join(MODELS_DIR), &digest);

    let (mut fresh, _) = project(root.path(), fake, &dataset(12, 12));
    fs::write(
        &path,
        fs::read_to_string(&path).unwrap().replace("rate", "rote"),
    )
    .unwrap();
    let altered = noul(&mut fresh, &model, "d/x", "bad").unwrap_err();
    assert!(
        altered.contains(&digest) && altered.contains(&path.display().to_string()),
        "{altered}"
    );

    fs::remove_file(&path).unwrap();
    let (mut fresh, _) = project(root.path(), fake, &dataset(12, 12));
    let missing = noul(&mut fresh, &model, "d/x", "bad").unwrap_err();
    assert!(
        missing.contains(&digest) && missing.contains(&path.display().to_string()),
        "{missing}"
    );
}

#[test]
fn few_labels_train_the_rate_without_weights() {
    let root = tempfile::tempdir().unwrap();
    let (mut plugin, hex) = project(root.path(), offline, &dataset(3, 9));
    let model = trained(&mut plugin, root.path(), &hex);

    let p = noul(&mut plugin, &model, "d/x", "anything").unwrap();
    assert_eq!(p, noul(&mut plugin, &model, "d/x", "bad").unwrap());
    assert!((0.2..0.3).contains(&p), "{p}");
}

#[test]
fn a_training_file_that_cannot_be_read_fails_the_job_with_the_reason() {
    let root = tempfile::tempdir().unwrap();
    let (mut plugin, _) = project(root.path(), fake, &dataset(12, 12));

    let job = plugin
        .train(train_request(root.path(), &"ab".repeat(32)))
        .unwrap();
    assert_eq!(job.status, FineTuningJobStatus::Failed);
    assert_eq!(job.fine_tuned_model, None);
    assert!(job.error.unwrap().message.contains("the training file"));
    assert_eq!(json!(job.result_files), json!([]));
}
