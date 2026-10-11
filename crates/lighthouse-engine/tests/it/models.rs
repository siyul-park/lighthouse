//! Models as the engine offers them: a model of a plugin, named by a
//! provider of the project, answering with the options the provider sets.

use lighthouse_engine::{Engine, ModelOptions};
use lighthouse_plugin::{
    Answer, Classifier, ClassifyParams, ClassifyResult, DecisionRef, Error, ModelManifest,
    ModelRef, Open, Plugin, PluginManifest, Registry, ResultKind, TrainParams, TrainResponse,
};
use lighthouse_spec::{Catalog, Config};

/// A model that echoes the model it was asked for, taken from the options.
struct Echo(ModelManifest);

impl Classifier for Echo {
    fn manifest(&self) -> &ModelManifest {
        &self.0
    }

    fn classify(&self, params: &ClassifyParams) -> Result<ClassifyResult, Error> {
        let asked = params.options["model"].as_str().unwrap_or_default();
        Ok(ClassifyResult {
            results: params
                .subjects
                .iter()
                .map(|s| Answer {
                    key: s.key.clone(),
                    kind: ResultKind::Pass,
                    confidence: 1.0,
                    reason: asked.to_owned(),
                    span: None,
                })
                .collect(),
            model: ModelRef {
                id: self.0.id.clone(),
                version: "1".to_owned(),
            },
        })
    }

    fn train(&self, _: &TrainParams) -> Result<TrainResponse, Error> {
        Err(Error::Failed("no".to_owned()))
    }
}

struct Models(PluginManifest);

impl Plugin for Models {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }

    fn models(&self) -> Vec<Box<dyn Classifier>> {
        vec![Box::new(Echo(ModelManifest {
            id: "echo".to_owned(),
            tasks: vec!["classification".to_owned()],
            version: "1".to_owned(),
            trains: false,
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
        examples: Vec::new(),
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
        .classify(&ClassifyParams {
            decision: decision(),
            subjects: Vec::new(),
            options: serde_json::Map::new(),
        })
        .unwrap();

    assert_eq!(answer.model.id, "work/some/model:free");
    assert!(engine.model("nowhere/m").is_err());
    assert!(engine.model("no-slash").is_err());
}

#[test]
fn a_subject_nothing_decided_says_why() {
    assert_eq!(Open::Unbound.to_string(), "no model is bound");
    assert_eq!(Open::OverBudget.to_string(), "the model budget was spent");
    assert!(Open::Failed("x".to_owned()).to_string().contains('x'));
}
