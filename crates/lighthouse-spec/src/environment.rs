//! The one CEL environment of the tool: the standard library of CEL with the
//! extensions `cel` ships (strings, lists, math, base64 encoders) and the
//! optional syntax, exactly as that crate provides them. The checks, the fixes
//! and the event mappings of the providers all compile and run in it.

use std::sync::{Arc, OnceLock};

use cel::{Context, DeclarationError, Env, Program, extensions};

/// An extension of the `cel` crate, as it installs itself.
type Extension = fn(&mut Env) -> Result<(), DeclarationError>;

/// The environment, built once.
pub fn environment() -> Result<Arc<Env>, String> {
    static ENVIRONMENT: OnceLock<Result<Arc<Env>, String>> = OnceLock::new();
    ENVIRONMENT.get_or_init(build).clone()
}

/// A context of the environment, to add variables and functions to.
pub fn context() -> Result<Context<'static, 'static>, String> {
    Ok(Context::with_env(environment()?))
}

/// `source` compiled in the environment.
pub fn compile(source: &str) -> Result<Program, String> {
    environment()?.compile(source).map_err(|e| e.to_string())
}

fn build() -> Result<Arc<Env>, String> {
    let mut env = Env::stdlib();
    let installed: [(&str, Extension); 4] = [
        ("strings", extensions::strings),
        ("lists", extensions::lists),
        ("math", extensions::math),
        ("encoders", extensions::encoders),
    ];
    for (name, install) in installed {
        env.add_extension(install)
            .map_err(|e| format!("the CEL `{name}` extension: {e}"))?;
    }
    crate::ext::lists::install(&mut env).map_err(|e| format!("the CEL lists additions: {e}"))?;
    crate::ext::regex::install(&mut env).map_err(|e| format!("the CEL regex extension: {e}"))?;
    Ok(Arc::new(env))
}
