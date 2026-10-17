//! A model of the project, as the engine asks it: a model of a plugin process
//! with the options that pick it, or a program the host builds a client for.
//! Both are a [`ModelClient`] named by the reference to the model.

use std::sync::Arc;

use lighthouse_plugin::{
    Capabilities, ChatRequest, CreateChatCompletionResponse, CreateFineTuningJobRequest, Error,
    FineTuningJob, Model, ModelClient, Registry, Supports, SystemOneRequest, SystemOneResponse,
};
use lighthouse_protocol::hash;
use lighthouse_spec::ModelProvider;
use serde_json::{Map, Value};

/// Builds the client of a `command` provider: the whole reference, the
/// model part of it and the provider. The host supplies it, so that the
/// engine does not know how a program is run as a model.
pub type CommandModels =
    Arc<dyn Fn(&str, &str, &ModelProvider) -> Option<Box<dyn ModelClient>> + Send + Sync>;

/// What building a client needs from the engine.
pub(super) struct Clients<'a> {
    pub(super) registry: &'a Registry,
    /// The project is trusted to run the commands it names.
    pub(super) trusted: bool,
    pub(super) commands: Option<&'a CommandModels>,
}

impl<'a> Clients<'a> {
    /// The model `model` of `provider`, named `name` (the whole reference);
    /// one that cannot be used is a model that fails with the reason, so its
    /// subjects stay for review.
    pub(super) fn build(
        &self,
        name: &str,
        model: &str,
        provider: &ModelProvider,
    ) -> Box<dyn ModelClient + 'a> {
        match provider {
            ModelProvider::Rpc {
                plugin,
                options,
                cost,
            } => match self.registry.model(plugin) {
                Some(inner) => {
                    let options = options.clone();
                    Box::new(Plugin {
                        manifest: manifest(name, inner.manifest(), model, &options, *cost),
                        name: model.to_owned(),
                        model: inner,
                        options,
                    })
                }
                None => refused(name, format!("plugin `{plugin}` offers no model")),
            },
            ModelProvider::Command { .. } if !self.trusted => refused(
                name,
                "this project is not trusted to run commands (run `lighthouse trust`)".to_owned(),
            ),
            ModelProvider::Command { .. } => self
                .commands
                .and_then(|build| build(name, model, provider))
                .unwrap_or_else(|| refused(name, "the host cannot run command models".to_owned())),
        }
    }
}

/// A model that cannot run, kept so that its failure is reported.
struct Refused {
    manifest: Model,
    reason: String,
}

/// A model of a plugin process, named and configured by the project.
struct Plugin<'a> {
    manifest: Model,
    /// The part of the reference after the provider.
    name: String,
    model: &'a dyn ModelClient,
    options: Map<String, Value>,
}

impl ModelClient for Refused {
    fn manifest(&self) -> &Model {
        &self.manifest
    }

    fn classify(&self, _: &SystemOneRequest) -> Result<SystemOneResponse, Error> {
        Err(self.failure())
    }

    fn chat(&self, _: &ChatRequest) -> Result<CreateChatCompletionResponse, Error> {
        Err(self.failure())
    }
}

impl Refused {
    fn failure(&self) -> Error {
        Error::Failed(format!("model `{}`: {}", self.manifest.id, self.reason))
    }
}

impl ModelClient for Plugin<'_> {
    fn manifest(&self) -> &Model {
        &self.manifest
    }

    fn classify(&self, params: &SystemOneRequest) -> Result<SystemOneResponse, Error> {
        let mut asked = params.clone();
        asked.model.clone_from(&self.name);
        asked.options.clone_from(&self.options);
        self.model.classify(&asked)
    }

    fn chat(&self, request: &ChatRequest) -> Result<CreateChatCompletionResponse, Error> {
        let mut asked = request.clone();
        asked.body.model.clone_from(&self.name);
        asked.options.clone_from(&self.options);
        self.model.chat(&asked)
    }

    fn train(&self, request: &CreateFineTuningJobRequest) -> Result<FineTuningJob, Error> {
        let mut asked = request.clone();
        asked.model.clone_from(&self.name);
        asked.options.clone_from(&self.options);
        self.model.train(&asked)
    }
}

fn manifest(
    name: &str,
    inner: &Model,
    model: &str,
    options: &Map<String, Value>,
    cost: Option<f64>,
) -> Model {
    let version = format!(
        "{}+{}",
        inner.version,
        hash::short(&format!("{model}{}", Value::Object(options.clone())), 4)
    );
    Model {
        id: name.to_owned(),
        version,
        cost: cost.unwrap_or(inner.cost),
        ..inner.clone()
    }
}

fn refused<'a>(name: &str, reason: String) -> Box<dyn ModelClient + 'a> {
    Box::new(Refused {
        manifest: Model {
            id: name.to_owned(),
            object: "model".to_owned(),
            created: 0,
            owned_by: name.to_owned(),
            capabilities: Capabilities {
                classification: Some(Supports { supported: true }),
                chat: Some(Supports { supported: true }),
                ..Capabilities::default()
            },
            version: String::new(),
            deterministic: false,
            abstains: true,
            cost: 0.0,
        },
        reason,
    })
}
