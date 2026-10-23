//! What the CEL expressions of the checks need besides the shared environment
//! (see `lighthouse_spec::environment`, with the libraries the plugins
//! register): the check that the functions they call exist, the variables of a
//! check, and what the code model says as a field of a node, built only for the
//! expressions that read it; see [`Needs`].

use cel::Context;

mod functions;
mod needs;
mod variables;
pub(crate) use functions::{check_calls, libraries_called};
pub(crate) use needs::Needs;
pub(crate) use variables::{check_scope, declare as declare_variables, reached};

/// A context of the shared CEL environment.
pub(crate) fn context() -> Result<Context<'static, 'static>, String> {
    lighthouse_spec::context()
}
