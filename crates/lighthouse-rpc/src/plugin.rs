use std::{path::Path, sync::Arc, time::Duration};

use lighthouse_model::{Capability, Incomplete};
use lighthouse_plugin::{
    Conventions, Error, Indexed, LanguageProvider, Manifest, Plugin, Source, Workspace,
};
use lighthouse_protocol as wire;
use serde_json::Value;

use crate::{client::Client, convert, manifest::Found};

/// A language plugin running as a separate process.
pub struct RpcPlugin {
    manifest: Manifest,
    languages: Vec<wire::Language>,
    client: Arc<Client>,
}

impl RpcPlugin {
    /// Starts the process and performs the `initialize` handshake.
    pub fn connect(found: &Found, root: &Path, timeout: Duration) -> Result<Self, crate::Error> {
        let id = &found.manifest.id;
        let failed = |reason: String| crate::Error::Plugin {
            id: id.clone(),
            reason,
        };
        let client = Client::spawn(&found.manifest, &found.dir, root, timeout).map_err(failed)?;
        let unavailable = |client: &Client, reason: String| crate::Error::Unavailable {
            id: id.clone(),
            reason,
            notices: client.drain_stderr(),
        };
        let params = wire::InitializeParams {
            root: root.to_string_lossy().into_owned(),
            protocol_version: wire::VERSION.to_owned(),
            client_info: wire::ClientInfo {
                name: "lighthouse".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
        };
        let result: wire::InitializeResult = client
            .call(wire::INITIALIZE, params)
            .map_err(|reason| unavailable(&client, reason))?;
        if result.protocol_version != wire::VERSION {
            return Err(failed(format!(
                "speaks protocol {}, lighthouse speaks {}",
                result.protocol_version,
                wire::VERSION
            )));
        }
        if result.id != *id {
            return Err(failed(format!(
                "identifies itself as `{}`, its manifest says `{id}`",
                result.id
            )));
        }
        Ok(Self {
            manifest: Manifest {
                id: result.id,
                version: result.version,
            },
            languages: result.languages,
            client: Arc::new(client),
        })
    }
}

impl Plugin for RpcPlugin {
    fn manifest(&self) -> Manifest {
        self.manifest.clone()
    }

    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        self.languages
            .iter()
            .map(|language| {
                Box::new(Provider {
                    plugin: self.manifest.id.clone(),
                    language: language.clone(),
                    capabilities: language
                        .capabilities
                        .iter()
                        .filter_map(|c| convert::capability(c))
                        .collect(),
                    client: Arc::clone(&self.client),
                }) as Box<dyn LanguageProvider>
            })
            .collect()
    }
}

struct Provider {
    plugin: String,
    language: wire::Language,
    capabilities: Vec<Capability>,
    client: Arc<Client>,
}

impl Provider {
    fn request(&self, ws: &Workspace, files: &[Source]) -> wire::IndexParams {
        wire::IndexParams {
            project: wire::ProjectRef {
                root: ws.root.to_string_lossy().into_owned(),
            },
            language: self.language.id.clone(),
            files: files
                .iter()
                .map(|s| wire::FileRef {
                    path: s.file.path.to_string_lossy().replace('\\', "/"),
                    hash: s.file.hash.clone(),
                })
                .collect(),
            context: wire::Context {
                options: ws
                    .languages
                    .iter()
                    .map(|(id, options)| (id.clone(), Value::Object(options.clone())))
                    .collect(),
                overlays: None,
            },
        }
    }
}

impl LanguageProvider for Provider {
    fn id(&self) -> &str {
        &self.language.id
    }

    fn globs(&self) -> &[String] {
        &self.language.globs
    }

    fn conventions(&self) -> Conventions {
        Conventions {
            test_globs: self.language.conventions.test_globs.clone(),
        }
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    fn fallback(&self) -> bool {
        self.language.fallback
    }

    fn priority(&self) -> i32 {
        self.language.priority
    }

    /// Never fails as a whole: a crash, timeout or malformed answer becomes an
    /// incomplete entry next to the plugin's captured stderr.
    fn index(&self, ws: &Workspace, files: &[Source]) -> Result<Indexed, Error> {
        let response = self
            .client
            .call::<_, wire::IndexResult>(wire::INDEX, self.request(ws, files))
            .and_then(|result| {
                convert::indexed(result, files)
                    .map_err(|e| format!("plugin `{}` sent a malformed result: {e}", self.plugin))
            });
        let mut indexed = response.unwrap_or_else(|reason| Indexed {
            incomplete: vec![Incomplete {
                path: None,
                reason: format!("{reason}; {} file(s) not analyzed", files.len()),
            }],
            ..Indexed::default()
        });
        indexed.notices.extend(self.client.drain_stderr());
        Ok(indexed)
    }
}
