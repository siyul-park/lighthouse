//! What every library of the plugin is built from: its manifest, the function
//! that registers its functions, and the entries of a manifest.

use cel::{Env, ExecutionError, common::value::CowVal};
use lighthouse_plugin::{CelLibrary, CelLibraryManifest, Error, Function, Overload, Precedence};

use crate::ID;

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
