//! Models as the engine offers them: a model of a plugin, named by a
//! provider of the project, answering for the model the reference names.

use lighthouse_engine::{Engine, ModelOptions};
use lighthouse_plugin::{
    Abstained, Answer, Capabilities, CreateFineTuningJobRequest, DecisionRef, Error, Explored,
    FineTuningJob, Model, ModelClient, Plugin, PluginManifest, Registry, State, SystemOneRequest,
    SystemOneResponse,
};
use lighthouse_spec::{Catalog, Config};

/// A model that echoes the model it was asked for, taken from the options.
struct Echo(Model);

impl ModelClient for Echo {
    fn manifest(&self) -> &Model {
        &self.0
    }

    fn classify(&self, params: &SystemOneRequest) -> Result<SystemOneResponse, Error> {
        Ok(SystemOneResponse {
            model: params.model.clone(),
            answers: params
                .questions
                .keys()
                .map(|id| {
                    let answer = Answer {
                        kind: "noul".to_owned(),
                        noul: 0.25,
                    };
                    (id.clone(), answer)
                })
                .collect(),
            usage: serde_json::Value::Null,
        })
    }

    fn train(&self, _: &CreateFineTuningJobRequest) -> Result<FineTuningJob, Error> {
        Err(Error::Failed("no".to_owned()))
    }
}

fn echo() -> Echo {
    Echo(Model {
        id: "echo".to_owned(),
        object: "model".to_owned(),
        created: 0,
        owned_by: "echo".to_owned(),
        capabilities: Capabilities {
            classification: Some(lighthouse_plugin::Supports { supported: true }),
            ..Capabilities::default()
        },
        version: "1".to_owned(),
        deterministic: true,
        abstains: false,
        cost: 0.0,
    })
}

struct Models(PluginManifest);

impl Plugin for Models {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }

    fn models(&self) -> Vec<Box<dyn ModelClient>> {
        vec![Box::new(echo())]
    }
}

fn registry() -> Registry {
    let mut registry = Registry::default();
    registry
        .register(&Models(PluginManifest {
            id: "echo".to_owned(),
            version: "1".to_owned(),
        }))
        .unwrap();
    registry
}

fn decision() -> DecisionRef {
    DecisionRef {
        id: "d/x".to_owned(),
        requirement: "r".to_owned(),
        prompt: "r".to_owned(),
        valid: Vec::new(),
        invalid: Vec::new(),
    }
}

#[test]
fn a_registry_holds_the_models_of_its_plugins_by_id() {
    let registry = registry();
    assert!(registry.model("echo").is_some());
    assert!(registry.model("other").is_none());
}

#[test]
fn a_reference_names_a_provider_and_the_model_it_is_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let config =
        Config::parse_inline("plugins = [\"echo\"]\n[models.providers.work]\nplugin = \"echo\"\n")
            .unwrap();
    let engine = Engine::new(registry(), config, Catalog::bundled(), dir.path())
        .unwrap()
        .with_models(ModelOptions::default());

    let model = engine.model("work/some/model:free").unwrap();
    let answer = model
        .classify(&SystemOneRequest {
            state: State {
                language: "go".to_owned(),
                file: "a.go".to_owned(),
                text: String::new(),
                facts: serde_json::Value::Null,
                neighbors: Vec::new(),
            },
            model: String::new(),
            questions: std::collections::BTreeMap::from([(
                "d/x".to_owned(),
                lighthouse_plugin::Question::new(&decision()),
            )]),
            options: serde_json::Map::new(),
        })
        .unwrap();

    // The provider names the model the request is for.
    assert_eq!(answer.model, "some/model:free");
    assert_eq!(answer.answers["d/x"].noul, 0.25);
    assert!(engine.model("nowhere/m").is_err());
    assert!(engine.model("no-slash").is_err());
}

#[test]
fn a_subject_nothing_decided_says_why() {
    assert_eq!(Abstained::Unbound.to_string(), "no model is bound");
    assert_eq!(
        Abstained::OverBudget.to_string(),
        "the model budget was spent"
    );
    assert!(Abstained::Failed("x".to_owned()).to_string().contains('x'));
    let explored = Abstained::Explored(Explored {
        model: "local/m@1".to_owned(),
        confidence: 0.951,
    });
    assert_eq!(
        explored.to_string(),
        "explored: local/m@1 said fail at 0.95"
    );
}

#[test]
fn a_probability_fails_a_subject_only_at_or_above_the_fail_threshold() {
    use lighthouse_plugin::{FAIL_AT, fails};

    assert_eq!(fails(0.95), Some(0.95));
    assert_eq!(fails(FAIL_AT), Some(FAIL_AT));
    assert_eq!(fails(0.5), None);
    // Even a model that is sure the subject is fine leaves it for review: the
    // band has no lower bound in M1.
    assert_eq!(fails(0.02), None);
}

#[test]
fn engine_with_commands_builds_the_command_models_of_a_trusted_project() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::parse_inline(
        "plugins = []\n[models.providers.prog]\nprotocol = \"typesafe\"\ncommand = [\"run\"]\n",
    )
    .unwrap();
    let engine = |trusted: bool| {
        let engine = Engine::new(
            Registry::default(),
            config.clone(),
            Catalog::bundled(),
            dir.path(),
        )
        .unwrap()
        .with_trust(trusted);
        engine.with_commands(std::sync::Arc::new(|_, _, _| {
            Some(Box::new(echo()) as Box<dyn ModelClient>)
        }))
    };

    let trusted = engine(true);
    assert_eq!(trusted.model("prog/m").unwrap().manifest().id, "echo");
    // Without trust the project's programs are not run.
    let untrusted = engine(false);
    let refused = untrusted.model("prog/m").unwrap();
    assert_eq!(refused.manifest().id, "prog/m");
}

#[test]
fn a_project_cannot_declare_the_reserved_provider_local() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::parse_inline(
        "plugins = [\"echo\"]\n[models]\nclassification = \"local/echo\"\n\
         [models.providers.local]\nplugin = \"echo\"\n",
    )
    .unwrap();
    let error = Engine::new(registry(), config, Catalog::bundled(), dir.path())
        .err()
        .expect("local is reserved");
    assert!(error.to_string().contains("reserved"), "{error}");
}

#[test]
fn a_trained_model_is_another_model_to_the_answer_cache_by_its_digest() {
    let dir = tempfile::tempdir().unwrap();
    let config =
        Config::parse_inline("plugins = [\"echo\"]\n[models.providers.work]\nplugin = \"echo\"\n")
            .unwrap();
    let engine = Engine::new(registry(), config, Catalog::bundled(), dir.path())
        .unwrap()
        .with_models(ModelOptions::default());
    let manifest = |digest: &str| {
        engine
            .model(&format!("work/learned@sha256:{digest}"))
            .unwrap()
            .manifest()
            .clone()
    };

    // The answer cache key is made of the manifest's id and version.
    let (first, second) = (manifest("aa"), manifest("bb"));
    assert_ne!(first.id, second.id);
    assert_ne!(first.version, second.version);
    assert_eq!(first.version, manifest("aa").version);
}
