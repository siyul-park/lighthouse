use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use lighthouse_model::{GeneratedSpec, Incomplete};

use globset::{Glob, GlobSetBuilder};
use lighthouse_plugin::{
    Error, Indexed, LanguageManifest, LanguageProvider, ModelClient, Plugin, PluginManifest,
    Source, Unit, Workspace,
};
use lighthouse_process::Policy;
use lighthouse_protocol as wire;
use lighthouse_protocol::hash;
use serde_json::Value;

use crate::{agent::Agent, client::Client, convert, kept::Kept, manifest::Found, model::Remote};

/// How plugins are started beyond what the configuration says.
#[derive(Debug, Clone, Copy, Default)]
pub struct StartOptions<'a> {
    /// The cache directory the answer to `initialize` is kept in; with it a
    /// plugin is started when a request first needs it.
    pub cache: Option<&'a Path>,
    /// Variables added to the environment of every plugin process.
    pub env: &'a [(String, String)],
}

/// A plugin running as a separate process: the languages and models it
/// declared in the handshake, sharing one connection.
pub struct RpcPlugin {
    manifest: PluginManifest,
    ignores: Vec<String>,
    generated: GeneratedSpec,
    languages: Vec<wire::LanguageManifest>,
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
        Self::connect_with(found, root, timeout, &StartOptions::default())
    }

    /// Like [`connect`](Self::connect), as `options` say: what the plugin
    /// answered to `initialize` is kept in the cache directory, by a key of
    /// the plugin build, and while the kept answer is there for this build no
    /// process starts here: it starts when the first request is made, and has
    /// to answer as it did. The process gets the environment `options` add.
    pub fn connect_with(
        found: &Found,
        root: &Path,
        timeout: Duration,
        options: &StartOptions<'_>,
    ) -> Result<Self, crate::Error> {
        let id = found.id();
        let failed = unavailable(id);
        let params = wire::InitializeParams {
            protocol_version: wire::VERSION.to_owned(),
            client_info: wire::Implementation {
                name: "lighthouse".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            workspace_folders: vec![folder(root).map_err(failed)?],
        };
        let kept = options.cache.and_then(|base| Kept::of(base, found));
        if let Some(result) = kept.as_ref().and_then(Kept::read) {
            let client = Client::lazy(found, root, timeout, options.env, params, result.clone());
            return Self::from_result(found, root, timeout, client, result);
        }
        let client = Client::spawn(found, root, timeout, options.env).map_err(failed)?;
        let result: wire::InitializeResult =
            client
                .call(wire::INITIALIZE, params)
                .map_err(|reason| crate::Error::Unavailable {
                    id: id.to_owned(),
                    reason,
                    notices: client.drain_stderr(),
                })?;
        let plugin = Self::from_result(found, root, timeout, client, result.clone())?;
        if let Some(kept) = &kept {
            kept.write(&result);
        }
        Ok(plugin)
    }

    /// The plugin that `client` speaks to, if `result` is what its manifest
    /// promised.
    fn from_result(
        found: &Found,
        root: &Path,
        timeout: Duration,
        client: Client,
        result: wire::InitializeResult,
    ) -> Result<Self, crate::Error> {
        let id = found.id();
        let failed = unavailable(id);
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
            let binary = found.command_path().and_then(|path| fs::read(path).ok());
            let version = &found.resource.spec.version;
            hash::sha256(binary.as_deref().unwrap_or(version.as_bytes()))
        };
        Ok(Self {
            ignores: found.resource.spec.ignores.clone(),
            generated: found.resource.spec.generated.clone(),
            manifest: PluginManifest {
                id: result.server_info.name,
                version: result.server_info.version,
            },
            languages,
            models,
            client: Arc::new(client),
            digest,
            agent: Agent::new(id, found.argv(), &Policy::default(), root, timeout),
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

    fn ignores(&self) -> Vec<String> {
        self.ignores.clone()
    }

    fn generated(&self) -> GeneratedSpec {
        self.generated.clone()
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
                    manifest: convert::language_manifest(language),
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
    manifest: LanguageManifest,
    client: Arc<Client>,
    digest: String,
}

impl LanguageProvider for Provider {
    fn manifest(&self) -> &LanguageManifest {
        &self.manifest
    }

    fn identity(&self) -> String {
        self.digest.clone()
    }

    /// Never fails as a whole: a crash, timeout or malformed answer becomes an
    /// incomplete entry next to the plugin's captured stderr.
    fn index(&self, ws: &Workspace, units: &[Unit], files: &[String]) -> Result<Indexed, Error> {
        Ok(self.run_index(self.request(ws, units, files), units))
    }
}

impl Provider {
    fn run_index(&self, request: wire::IndexParams, units: &[Unit]) -> Indexed {
        let required_asts: BTreeSet<_> = request
            .units
            .iter()
            .flat_map(|unit| &unit.files)
            .filter(|file| file.source_ast)
            .map(|file| PathBuf::from(&file.path))
            .collect();
        let response = self
            .client
            .call::<_, wire::IndexResult>(wire::INDEX, request)
            .and_then(|result| {
                convert::indexed(result, units, &required_asts).map_err(|e| {
                    format!(
                        "plugin `{}` sent a malformed result: {e}",
                        self.client.plugin()
                    )
                })
            });
        let files: usize = units.iter().map(|unit| unit.files.len()).sum();
        let mut indexed = response.unwrap_or_else(|reason| Indexed {
            incomplete: vec![Incomplete {
                path: None,
                provider: None,
                reason: format!("{reason}; {files} file(s) not analyzed"),
            }],
            ..Indexed::default()
        });
        indexed.notices.extend(self.client.drain_stderr());
        indexed
    }

    /// The request for the units, with every overlay of the run: a unit may
    /// read a file of another.
    fn request(&self, ws: &Workspace, units: &[Unit], files: &[String]) -> wire::IndexParams {
        let source_globs = self.source_ast.then(|| {
            let mut builder = GlobSetBuilder::new();
            for pattern in &ws.source_globs {
                builder.add(Glob::new(pattern).expect("source globs are validated by Config"));
            }
            builder
                .build()
                .expect("source globs are validated by Config")
        });
        let overlays: Vec<wire::Overlay> = ws
            .overlays
            .iter()
            .map(|(path, text)| wire::Overlay {
                path: path.to_string_lossy().replace('\\', "/"),
                text: text.clone(),
            })
            .collect();
        wire::IndexParams {
            project: wire::ProjectRef {
                root: ws.root.to_string_lossy().into_owned(),
            },
            language: self.language.clone(),
            units: units
                .iter()
                .map(|unit| unit_ref(unit, source_globs.as_ref()))
                .collect(),
            files: files.to_vec(),
            context: wire::Context {
                options: ws
                    .languages
                    .iter()
                    .map(|(id, options)| (id.clone(), Value::Object(options.clone())))
                    .collect(),
                overlays: (!overlays.is_empty()).then_some(overlays),
            },
        }
    }
}

/// The workspace folder of a project root, which may be relative.
pub(crate) fn folder(root: &Path) -> Result<wire::WorkspaceFolder, String> {
    let root = std::path::absolute(root).map_err(|e| format!("cannot absolutize the root: {e}"))?;
    wire::WorkspaceFolder::at(&root)
}

/// The unit as the wire has it: its files, or the host files whose regions it
/// holds.
fn unit_ref(unit: &Unit, source_globs: Option<&globset::GlobSet>) -> wire::UnitRef {
    let (regions, files): (Vec<&Source>, Vec<&Source>) = unit
        .files
        .iter()
        .partition(|source| !source.regions.is_empty());
    wire::UnitRef {
        id: unit.id.to_owned(),
        files: files
            .into_iter()
            .map(|source| wire::FileRef {
                path: source.file.path.to_string_lossy().replace('\\', "/"),
                hash: hash::sha256(source.text),
                source_ast: source_globs.is_some_and(|globs| globs.is_match(&source.file.path)),
            })
            .collect(),
        regions: regions
            .into_iter()
            .map(|source| embedded_wire(source))
            .collect(),
        key: unit.key.to_owned(),
        imports: unit.imports.clone(),
    }
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

/// The generic embedded source request for an out-of-process analysis-language provider.
fn embedded_wire(source: &Source<'_>) -> wire::EmbeddedFile {
    wire::EmbeddedFile {
        path: source.file.path.to_string_lossy().replace('\\', "/"),
        hash: hash::sha256(source.text),
        text: source.text.to_owned(),
        test: source.file.test,
        facts: source.file.facts.clone(),
        regions: source.regions.to_vec(),
    }
}

/// The error of the plugin `id` that does not start or answer as it promised.
fn unavailable(id: &str) -> impl Fn(String) -> crate::Error + Copy + '_ {
    move |reason| crate::Error::Plugin {
        id: id.to_owned(),
        reason,
    }
}
