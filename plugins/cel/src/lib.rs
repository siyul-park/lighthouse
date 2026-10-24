//! The bundled `cel` plugin: the CEL functions Lighthouse implements beyond
//! the `cel` crate, one library per module. The registry unions their
//! manifests into the function catalog, and the composition root registers
//! them into the shared environment. The functions are domain-free: node
//! fields stay in the fact builder.

use cel::{Env, ExecutionError, common::value::CowVal};
use lighthouse_plugin::{
    CelLibrary, CelLibraryManifest, Error, Function, Overload, Plugin, PluginManifest, Precedence,
};

pub mod glob;

mod bind;
mod language;
mod lists;
mod order;
mod regex;

/// The plugin id, and the prefix of its libraries' ids.
pub const ID: &str = "cel";

/// The plugin that provides the libraries.
pub struct Cel {
    manifest: PluginManifest,
}

impl Cel {
    /// The plugin, with the version of this crate.
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: ID.to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
        }
    }
}

impl Default for Cel {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for Cel {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn languages(&self) -> Vec<Box<dyn lighthouse_plugin::LanguageProvider>> {
        vec![Box::new(language::Language::new())]
    }

    fn cel_libraries(&self) -> Vec<Box<dyn CelLibrary>> {
        [
            regex::library(),
            lists::library(),
            glob::library(),
            bind::library(),
            order::library(),
        ]
        .into_iter()
        .map(|library| Box::new(library) as Box<dyn CelLibrary>)
        .collect()
    }
}

/// A library: its manifest, and what registers the functions it lists.
pub(crate) struct Library {
    manifest: CelLibraryManifest,
    install: fn(&mut Env) -> Result<(), cel::DeclarationError>,
}

impl Library {
    pub(crate) fn new(
        name: &str,
        functions: Vec<Function>,
        install: fn(&mut Env) -> Result<(), cel::DeclarationError>,
    ) -> Self {
        Self {
            manifest: CelLibraryManifest {
                id: format!("{ID}/{name}"),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                functions,
            },
            install,
        }
    }
}

impl CelLibrary for Library {
    fn manifest(&self) -> &CelLibraryManifest {
        &self.manifest
    }

    fn register(&self, env: &mut Env) -> Result<(), Error> {
        (self.install)(env)
            .map_err(|e| Error::Failed(format!("CEL library `{}`: {e}", self.manifest.id)))
    }
}

/// A function entry of a manifest.
pub(crate) fn function(
    name: &str,
    receiver: Option<&str>,
    overloads: Vec<Overload>,
    doc: &str,
    example: &str,
    precedence: Precedence,
) -> Function {
    Function {
        name: name.to_owned(),
        receiver: receiver.map(str::to_owned),
        overloads,
        doc: doc.to_owned(),
        example: example.to_owned(),
        precedence,
    }
}

/// `args` as the `N` arguments of an overload.
pub(crate) fn take_args<'b, 'v, const N: usize>(
    args: Vec<CowVal<'b, 'v>>,
) -> Result<[CowVal<'b, 'v>; N], ExecutionError> {
    args.try_into()
        .map_err(|args: Vec<_>| ExecutionError::invalid_argument_count(N, args.len()))
}
