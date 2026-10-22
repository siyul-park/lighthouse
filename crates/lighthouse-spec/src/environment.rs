//! The one CEL environment of the tool: the standard library of CEL with the
//! extensions `cel` ships (strings, lists, math, base64 encoders) and the
//! optional syntax, exactly as that crate provides them, and the edit
//! constructors of a fix. The checks, the fixes
//! and the event mappings of the providers all compile and run in it.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

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

/// `source` compiled in the environment, once: the same program is shared by
/// whoever asks for it again. The variables of a pack are named by many
/// decisions, and each is read when the catalog is validated, built into a
/// rule and built into a fixer.
pub fn compile_shared(source: &str) -> Result<Arc<Program>, String> {
    type Compiled = HashMap<String, Result<Arc<Program>, String>>;
    static COMPILED: OnceLock<Mutex<Compiled>> = OnceLock::new();
    let compiled = COMPILED.get_or_init(Mutex::default);
    if let Some(found) = compiled
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(source)
    {
        return found.clone();
    }
    let made = environment()?
        .compile(source)
        .map(Arc::new)
        .map_err(|e| e.to_string());
    compiled
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(source.to_owned(), made.clone());
    made
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
    crate::ext::bindings::install(&mut env)
        .map_err(|e| format!("the CEL bindings extension: {e}"))?;
    crate::ext::lists::install(&mut env).map_err(|e| format!("the CEL lists additions: {e}"))?;
    crate::ext::regex::install(&mut env).map_err(|e| format!("the CEL regex extension: {e}"))?;
    crate::edit::install(&mut env).map_err(|e| format!("the CEL edit constructors: {e}"))?;
    Ok(Arc::new(env))
}
