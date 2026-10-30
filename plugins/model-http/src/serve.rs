//! The plugin as the protocol sees it: the handshake, and each method handed
//! to the protocol that serves it.

use lighthouse_protocol::{
    Capabilities, ErrorObject, Handler, InitializeParams, InitializeResult, Model,
    SystemOneRequest, SystemOneResponse,
};

use crate::{
    chat::{self, PROMPT_VERSION},
    endpoint::Endpoint,
    http::Client,
    typesafe,
};

/// The plugin's id, and the id of the one model it offers.
pub const ID: &str = "model-http";

pub struct Plugin {
    client: Box<dyn Client>,
}

impl Plugin {
    pub fn new(client: Box<dyn Client>) -> Self {
        Self { client }
    }
}

impl Handler for Plugin {
    fn initialize(&mut self, _: InitializeParams) -> Result<InitializeResult, ErrorObject> {
        let model = Model {
            abstains: true,
            cost: 1.0,
            ..Model::new(ID, PROMPT_VERSION, Capabilities::classifier(false))
        };
        Ok(InitializeResult::new(
            ID,
            env!("CARGO_PKG_VERSION"),
            Vec::new(),
            vec![model],
        ))
    }

    fn classify(&mut self, params: SystemOneRequest) -> Result<SystemOneResponse, ErrorObject> {
        if params.model.is_empty() {
            return Err("the request names no model".into());
        }
        let endpoint = Endpoint::of(&params.options)?;
        // The method's own protocol first, then Chat Completions by adaptation.
        let client = self.client.as_ref();
        if endpoint.speaks("typesafe") {
            typesafe::systemone(client, &endpoint, &params)
        } else if endpoint.speaks("openai") {
            chat::systemone(client, &endpoint, &params)
        } else {
            Err(format!(
                "no protocol of {:?} serves classification (served: [\"typesafe\", \"openai\"])",
                endpoint.protocols
            )
            .into())
        }
    }
}

#[cfg(test)]
mod tests;
