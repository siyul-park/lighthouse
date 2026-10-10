use std::{fs, path::Path, sync::Arc, time::Duration};

use lighthouse_model::Incomplete;
use lighthouse_plugin::{
    Error, Indexed, LanguageProvider, Plugin, PluginManifest, ProviderManifest, Source, Workspace,
};
use lighthouse_protocol as wire;
use serde_json::Value;

use crate::{client::Client, convert, manifest::Found};

/// A language plugin running as a separate process.
pub struct RpcPlugin {
    manifest: PluginManifest,
    languages: Vec<wire::ProviderManifest>,
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
        let declared = &found.manifest.provides.languages;
        let mut reported: Vec<&str> = result.languages.iter().map(|l| l.id.as_str()).collect();
        reported.sort_unstable();
        let mut promised: Vec<&str> = declared.iter().map(String::as_str).collect();
        promised.sort_unstable();
        if !promised.is_empty() && promised != reported {
            return Err(failed(format!(
                "its manifest provides languages [{}], the process reports [{}]",
                promised.join(", "),
                reported.join(", ")
            )));
        }
        Ok(Self {
            manifest: PluginManifest {
                id: result.id,
                version: result.version,
            },
            languages: result.languages,
            client: Arc::new(client),
        })
    }
}

impl Plugin for RpcPlugin {
    /// The identity the process reported during the handshake.
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// One provider per language the process declared, all sharing its connection.
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        self.languages
            .iter()
            .map(|language| {
                Box::new(Provider {
                    plugin: self.manifest.id.clone(),
                    language: language.clone(),
                    manifest: convert::provider_manifest(language),
                    client: Arc::clone(&self.client),
                }) as Box<dyn LanguageProvider>
            })
            .collect()
    }
}

struct Provider {
    plugin: String,
    language: wire::ProviderManifest,
    manifest: ProviderManifest,
    client: Arc<Client>,
}

impl LanguageProvider for Provider {
    fn manifest(&self) -> &ProviderManifest {
        &self.manifest
    }

    /// Never fails as a whole: a crash, timeout or malformed answer becomes an
    /// incomplete entry next to the plugin's captured stderr.
    fn index(&self, ws: &Workspace, files: &[Source]) -> Result<Indexed, Error> {
        let (request, cache_notice) = self.request(ws, files);
        let response = self
            .client
            .call::<_, wire::IndexResult>(wire::INDEX, request)
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
        indexed.notices.extend(cache_notice);
        indexed.notices.extend(self.client.drain_stderr());
        Ok(indexed)
    }
}

impl Provider {
    /// The request, and a notice when the cache directory could not be made.
    fn request(&self, ws: &Workspace, files: &[Source]) -> (wire::IndexParams, Option<String>) {
        let (cache, notice) = match self.cache_of(ws) {
            Ok(cache) => (cache, None),
            Err(reason) => (None, Some(reason)),
        };
        let params = wire::IndexParams {
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
                overlays: overlays_of(ws, files),
                cache,
            },
        };
        (params, notice)
    }
}

impl Provider {
    /// The directory of this plugin's cache, created on demand and kept out of
    /// version control; `Ok(None)` when the run has no cache, and the reason
    /// when the directory cannot be made.
    fn cache_of(&self, ws: &Workspace) -> Result<Option<wire::CacheRef>, String> {
        let Some(base) = ws.cache_dir.as_ref() else {
            return Ok(None);
        };
        let dir = base.join(&self.plugin);
        let made = fs::create_dir_all(&dir).and_then(|()| {
            let ignore = base.join(".gitignore");
            if ignore.exists() {
                Ok(())
            } else {
                fs::write(&ignore, "*\n")
            }
        });
        made.map_err(|e| {
            format!(
                "cache directory {} cannot be made, `{}` runs without a cache: {e}",
                dir.display(),
                self.plugin
            )
        })?;
        Ok(Some(wire::CacheRef {
            dir: dir.to_string_lossy().into_owned(),
        }))
    }
}

/// The texts that stand in for requested files, for the `index` context; none
/// when nothing is overlaid.
fn overlays_of(ws: &Workspace, files: &[Source]) -> Option<Vec<wire::Overlay>> {
    let found: Vec<wire::Overlay> = files
        .iter()
        .filter_map(|s| {
            let text = ws.overlays.get(&s.file.path)?;
            Some(wire::Overlay {
                path: s.file.path.to_string_lossy().replace('\\', "/"),
                text: text.clone(),
            })
        })
        .collect();
    (!found.is_empty()).then_some(found)
}
