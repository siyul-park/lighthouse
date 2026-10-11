//! Models as the engine offers them: a model of a plugin, named by a
//! provider of the project, answering for the model the reference names.

use lighthouse_engine::{Engine, ModelOptions};
use lighthouse_plugin::{
    Answer, Capabilities, Classifier, CreateFineTuningJobRequest, DecisionRef, Error,
    FineTuningJob, Model, Open, Plugin, PluginManifest, Registry, State, SystemOneRequest,
    SystemOneResponse,
};
use lighthouse_spec::{Catalog, Config};

/// A model that echoes the model it was asked for, taken from the options.
struct Echo(Model);

impl Classifier for Echo {
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

struct Models(PluginManifest);

impl Plugin for Models {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }

    fn models(&self) -> Vec<Box<dyn Classifier>> {
        vec![Box::new(Echo(Model {
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
        }))]
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
    let config = Config::parse_inline(
        "plugins = [\"echo\"]\n[models.providers.work]\ntype = \"rpc\"\nplugin = \"echo\"\n",
    )
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
                lighthouse_plugin::Question::of(&decision()),
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
    assert_eq!(Open::Unbound.to_string(), "no model is bound");
    assert_eq!(Open::OverBudget.to_string(), "the model budget was spent");
    assert!(Open::Failed("x".to_owned()).to_string().contains('x'));
}

#[test]
fn a_probability_fails_a_subject_only_at_or_above_the_upper_bound() {
    use lighthouse_plugin::{UPPER, decide};

    assert_eq!(decide(0.95, UPPER), Some(0.95));
    assert_eq!(decide(UPPER, UPPER), Some(UPPER));
    assert_eq!(decide(0.5, UPPER), None);
    // Even a model that is sure the subject is fine leaves it for review: the
    // band has no lower bound in M1.
    assert_eq!(decide(0.02, UPPER), None);
}
