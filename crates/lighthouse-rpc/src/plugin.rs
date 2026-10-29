use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use lighthouse_model::Incomplete;

use globset::{Glob, GlobSetBuilder};
use lighthouse_plugin::{
    Error, Indexed, LanguageProvider, ModelClient, Plugin, PluginManifest, ProviderManifest,
    Source, Workspace,
};
use lighthouse_process::Policy;
use lighthouse_protocol as wire;
use lighthouse_protocol::hash;
use serde_json::Value;

use crate::{agent::Agent, client::Client, convert, manifest::Found, model::Remote};

/// Characters of the binary digest that name a plugin's cache directory.
const DIGEST_CHARS: usize = 16;

/// A plugin running as a separate process: the languages and models it
/// declared in the handshake, sharing one connection.
pub struct RpcPlugin {
    manifest: PluginManifest,
    languages: Vec<wire::ProviderManifest>,
    models: Vec<lighthouse_plugin::Model>,
    client: Arc<Client>,
    /// Digest of the binary the process runs, which keys its cache directory:
    /// a rebuilt plugin never reads what an older build kept.
    digest: String,
    /// How its models that edit are run.
    agent: Agent,
}

impl RpcPlugin {
    /// Starts the process and performs the `initialize` handshake.
    pub fn connect(found: &Found, root: &Path, timeout: Duration) -> Result<Self, crate::Error> {
        let id = found.id();
        let failed = |reason: String| crate::Error::Plugin {
            id: id.to_owned(),
            reason,
        };
        let client = Client::spawn(found, root, timeout).map_err(failed)?;
        let unavailable = |client: &Client, reason: String| crate::Error::Unavailable {
            id: id.to_owned(),
            reason,
            notices: client.drain_stderr(),
        };
        let params = wire::InitializeParams {
            protocol_version: wire::VERSION.to_owned(),
            client_info: wire::Implementation {
                name: "lighthouse".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            workspace_folders: vec![folder(root).map_err(failed)?],
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
        if result.server_info.name != id {
            return Err(failed(format!(
                "identifies itself as `{}`, its manifest says `{id}`",
                result.server_info.name
            )));
        }
        let provides = &found.resource.spec.provides;
        let languages = result.capabilities.languages;
        let reported: Vec<&str> = languages.iter().map(|l| l.id.as_str()).collect();
        declared("languages", &provides.languages, &reported).map_err(failed)?;
        let models = result.capabilities.models.data;
        let offered: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        declared("models", &provides.models, &offered).map_err(failed)?;
        let digest = if languages.is_empty() {
            String::new()
        } else {
            let binary = fs::read(found.command_path());
            let version = &found.resource.spec.version;
            hash::sha256(binary.as_deref().unwrap_or(version.as_bytes()))
        };
        Ok(Self {
            manifest: PluginManifest {
                id: result.server_info.name,
                version: result.server_info.version,
            },
            languages,
            models,
            client: Arc::new(client),
            digest,
            agent: Agent::new(
                id,
                std::iter::once(found.command_path().to_string_lossy().into_owned())
                    .chain(found.resource.spec.runtime.args.iter().cloned())
                    .collect(),
                &Policy::default(),
                root,
                timeout,
            ),
        })
    }

    /// The plugin with its agents run under the `[sandbox]` of the project at
    /// `root`: the hosts they may reach and the places they may write besides
    /// the directory they are given. Until it is said, an agent runs
    /// sandboxed with no network and no place of its own.
    #[must_use]
    pub fn confined(self, sandbox: &Policy, root: &Path) -> Self {
        Self {
            agent: self.agent.confined(sandbox, root),
            ..self
        }
    }
}

impl Plugin for RpcPlugin {
    /// One model per entry the process declared, sharing its connection.
    fn models(&self) -> Vec<Box<dyn ModelClient>> {
        self.models
            .iter()
            .map(|manifest| {
                Box::new(Remote {
                    manifest: manifest.clone(),
                    client: Arc::clone(&self.client),
                    agent: self.agent.clone(),
                }) as Box<dyn ModelClient>
            })
            .collect()
    }

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
                    language: language.id.clone(),
                    source_ast: language
                        .capabilities
                        .iter()
                        .any(|capability| capability == "source-ast"),
                    manifest: convert::provider_manifest(language),
                    client: Arc::clone(&self.client),
                    digest: self.digest.clone(),
                }) as Box<dyn LanguageProvider>
            })
            .collect()
    }
}

struct Provider {
    language: String,
    source_ast: bool,
    manifest: ProviderManifest,
    client: Arc<Client>,
    digest: String,
}

impl LanguageProvider for Provider {
    fn manifest(&self) -> &ProviderManifest {
        &self.manifest
    }

    /// Never fails as a whole: a crash, timeout or malformed answer becomes an
    /// incomplete entry next to the plugin's captured stderr.
    fn index(&self, ws: &Workspace, files: &[Source]) -> Result<Indexed, Error> {
        let (request, cache_notice) = self.request(ws, files);
        Ok(self.run_index(request, files, cache_notice))
    }
}

impl Provider {
    fn run_index(
        &self,
        request: wire::IndexParams,
        files: &[Source],
        cache_notice: Option<String>,
    ) -> Indexed {
        let required_asts: BTreeSet<_> = request
            .inputs
            .iter()
            .filter_map(|input| match input {
                wire::IndexInput::File {
                    file,
                    source_ast: true,
                } => Some(PathBuf::from(&file.path)),
                _ => None,
            })
            .collect();
        let response = self
            .client
            .call::<_, wire::IndexResult>(wire::INDEX, request)
            .and_then(|result| {
                convert::indexed(result, files, &required_asts).map_err(|e| {
                    format!(
                        "plugin `{}` sent a malformed result: {e}",
                        self.client.plugin()
                    )
                })
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
        indexed
    }
}

impl Provider {
    /// The request, and a notice when the cache directory could not be made.
    /// A request that carries overlays has no cache: the texts it sends are
    /// not what is on disk, and results from them must not be kept as if they
    /// were.
    fn request(&self, ws: &Workspace, files: &[Source]) -> (wire::IndexParams, Option<String>) {
        let overlays = overlays_of(ws, files);
        let source_globs = self.source_ast.then(|| {
            let mut builder = GlobSetBuilder::new();
            for pattern in &ws.source_globs {
                builder.add(Glob::new(pattern).expect("source globs are validated by Config"));
            }
            builder
                .build()
                .expect("source globs are validated by Config")
        });
        let (cache, notice) = match overlays {
            Some(_) => (None, None),
            None => match self.cache_of(ws) {
                Ok(cache) => (cache, None),
                Err(reason) => (None, Some(reason)),
            },
        };
        let inputs: Vec<wire::IndexInput> = files
            .iter()
            .map(|source| {
                if source.regions.is_empty() {
                    let path = source.file.path.to_string_lossy().replace('\\', "/");
                    let source_ast = source_globs
                        .as_ref()
                        .is_some_and(|globs| globs.is_match(&source.file.path));
                    wire::IndexInput::File {
                        file: wire::FileRef {
                            path,
                            hash: source.file.hash.clone(),
                        },
                        source_ast,
                    }
                } else {
                    wire::IndexInput::Regions {
                        regions: embedded_wire(source),
                    }
                }
            })
            .collect();
        let params = wire::IndexParams {
            project: wire::ProjectRef {
                root: ws.root.to_string_lossy().into_owned(),
            },
            language: self.language.clone(),
            inputs,
            context: wire::Context {
                options: ws
                    .languages
                    .iter()
                    .map(|(id, options)| (id.clone(), Value::Object(options.clone())))
                    .collect(),
                overlays,
                cache,
            },
        };
        (params, notice)
    }

    /// The directory of this plugin build's cache, created on demand and kept
    /// out of version control; `Ok(None)` when the run has no cache, and the
    /// reason when the directory cannot be made.
    fn cache_of(&self, ws: &Workspace) -> Result<Option<wire::CacheRef>, String> {
        let Some(base) = ws.cache_dir.as_ref() else {
            return Ok(None);
        };
        let plugin = self.client.plugin();
        let dir = base
            .join(plugin)
            .join(&self.digest[..DIGEST_CHARS.min(self.digest.len())]);
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
                "cache directory {} cannot be made, `{plugin}` runs without a cache: {e}",
                dir.display()
            )
        })?;
        Ok(Some(wire::CacheRef {
            dir: dir.to_string_lossy().into_owned(),
        }))
    }
}

/// The workspace folder of a project root, which may be relative.
pub(crate) fn folder(root: &Path) -> Result<wire::WorkspaceFolder, String> {
    let root = std::path::absolute(root).map_err(|e| format!("cannot absolutize the root: {e}"))?;
    wire::WorkspaceFolder::at(&root)
}

/// Checks what a manifest promised against what the process reported; a
/// manifest that promises nothing is not checked.
fn declared(kind: &str, promised: &[String], reported: &[&str]) -> Result<(), String> {
    let promised: BTreeSet<&str> = promised.iter().map(String::as_str).collect();
    let reported: BTreeSet<&str> = reported.iter().copied().collect();
    if promised.is_empty() || promised == reported {
        return Ok(());
    }
    let list = |ids: &BTreeSet<&str>| ids.iter().copied().collect::<Vec<_>>().join(", ");
    Err(format!(
        "its manifest provides {kind} [{}], the process reports [{}]",
        list(&promised),
        list(&reported)
    ))
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

/// The generic embedded source request for an out-of-process analysis-language provider.
fn embedded_wire(source: &Source<'_>) -> wire::EmbeddedFile {
    wire::EmbeddedFile {
        path: source.file.path.to_string_lossy().replace('\\', "/"),
        hash: source.file.hash.clone(),
        text: source.text.to_owned(),
        generated: source.file.generated,
        test: source.file.test,
        facts: source.file.facts.clone(),
        sources: source.regions.to_vec(),
    }
}
