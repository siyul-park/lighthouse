use super::*;

use lighthouse_plugin::{
    Capabilities, CreateFineTuningJobRequest, Error, FineTuningJob, Plugin, PluginManifest,
    Registry, Supports, SystemOneRequest, SystemOneResponse,
};
use lighthouse_spec::Config;

#[test]
fn failure_limit_is_shared_by_every_binding_of_a_model() {
    let failures = Failures::default();
    assert_eq!(failures.skipped("provider/model"), None);
    assert!(!failures.record("provider/model", "first"));
    assert!(!failures.record("provider/model", "second"));
    assert_eq!(failures.skipped("provider/model"), None);
    assert!(failures.record("provider/model", "third"));
    assert_eq!(failures.skipped("provider/model").as_deref(), Some("third"));
    assert!(!failures.record("provider/model", "fourth"));
    assert_eq!(
        failures.skipped("provider/model").as_deref(),
        Some("fourth")
    );
    assert_eq!(failures.skipped("another/model"), None);
}

struct Echo(Model);

impl ModelClient for Echo {
    fn manifest(&self) -> &Model {
        &self.0
    }

    fn classify(&self, _: &SystemOneRequest) -> Result<SystemOneResponse, Error> {
        Ok(SystemOneResponse {
            model: "echo".to_owned(),
            answers: BTreeMap::new(),
            usage: serde_json::Value::Null,
        })
    }

    fn train(&self, _: &CreateFineTuningJobRequest) -> Result<FineTuningJob, Error> {
        Err(Error::Failed("no".to_owned()))
    }
}

struct Models {
    manifest: PluginManifest,
    model: Model,
}

impl Plugin for Models {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn models(&self) -> Vec<Box<dyn ModelClient>> {
        vec![Box::new(Echo(self.model.clone()))]
    }
}

#[test]
fn run_resolves_only_models_that_declare_classification() {
    for supported in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = Registry::default();
        registry
            .register(&Models {
                manifest: PluginManifest {
                    id: "echo".to_owned(),
                    version: "1".to_owned(),
                },
                model: Model {
                    id: "echo".to_owned(),
                    object: "model".to_owned(),
                    created: 0,
                    owned_by: "echo".to_owned(),
                    capabilities: Capabilities {
                        classification: Some(Supports { supported }),
                        ..Capabilities::default()
                    },
                    version: "1".to_owned(),
                    deterministic: true,
                    abstains: false,
                    cost: 0.0,
                },
            })
            .unwrap();
        let config = Config::parse_inline(
            "plugins = [\"echo\"]\n[models.providers.work]\nplugin = \"echo\"\n",
        )
        .unwrap();
        let engine = super::super::Engine::new(
            registry,
            config,
            lighthouse_test_support::bundled(),
            dir.path(),
        )
        .unwrap()
        .with_models(super::super::ModelOptions::default());
        let scopes = Scopes::default();
        let models = RunModels::new(&engine, &scopes, None);

        assert_eq!(models.model("work/echo").is_some(), supported);
        if !supported {
            assert!(
                models
                    .notes
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|note| { note.contains("does not list `classification`") })
            );
        }
    }
}
