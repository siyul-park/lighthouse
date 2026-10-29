//! The bundled `lang-cel` plugin: the CEL functions Lighthouse implements
//! beyond the `cel` crate, one library per module, and the language provider
//! for the CEL embedded in other files. The registry unions the manifests of
//! the libraries into the function catalog, and the composition root registers
//! them into the shared environment. The functions are domain-free: node
//! fields stay in the fact builder.

mod language;
mod libraries;
mod library;
mod plugin;

pub use plugin::{Cel, ID};

pub(crate) use library::{Library, function, take_args};
