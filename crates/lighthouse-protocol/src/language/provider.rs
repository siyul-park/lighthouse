//! The contract of a language provider and the loop that serves it.

use std::io::{BufRead, Write};

use super::server::{self, Handler, ServeError};
use crate::{
    ErrorObject, IndexParams, IndexResult, InitializeParams, InitializeResult, LanguageManifest,
};

/// What a language provider is: its identity, the languages it declares and how
/// it indexes the files of one request. An `Err` from `index` becomes a
/// failed-request response; the provider stays usable.
///
/// A provider that panics is answered with an internal error and the loop
/// carries on, but its state may then be inconsistent (a panic can leave a
/// half-updated cache or index). A host should treat the batch as incomplete
/// and may restart the plugin rather than trust later answers.
pub trait Provider {
    /// The plugin id, which is the directory the plugin is installed in.
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    /// The languages the provider answers `initialize` with.
    fn manifest(&self) -> Vec<LanguageManifest>;
    /// Indexes the files of one language.
    fn index(&mut self, params: IndexParams) -> Result<IndexResult, ErrorObject>;
}

/// The protocol methods of a plugin that indexes languages and offers no model.
struct Languages<'a, P>(&'a mut P);

impl<P: Provider> Handler for Languages<'_, P> {
    fn initialize(&mut self, _: InitializeParams) -> Result<InitializeResult, ErrorObject> {
        Ok(InitializeResult::new(
            self.0.name(),
            self.0.version(),
            self.0.manifest(),
            Vec::new(),
        ))
    }

    fn index(&mut self, params: IndexParams) -> Result<IndexResult, ErrorObject> {
        self.0.index(params)
    }
}

/// Answers requests from `reader` on `writer` until the host exits; see the
/// protocol loop of [`crate::serve`] for how a session ends. The loop knows
/// framing and method names only, nothing about the code model.
pub fn serve(
    reader: impl BufRead,
    writer: impl Write,
    provider: &mut impl Provider,
) -> Result<(), ServeError> {
    server::serve(reader, writer, &mut Languages(provider))
}
