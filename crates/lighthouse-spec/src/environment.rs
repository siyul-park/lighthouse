//! The one CEL environment of the tool: the standard library of CEL with the
//! extensions `cel` ships (strings, lists, math, base64 encoders) and the
//! optional syntax, exactly as that crate provides them, and the edit
//! constructors of a fix. The checks, the fixes
//! and the event mappings of the providers all compile and run in it.

use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

use cel::{Context, DeclarationError, Env, Program, extensions};

/// The namespaces of the functions the `cel` crate ships (`lists.range`,
/// `math.greatest`, ...). The libraries a plugin registers add their own.
const NAMESPACES: [&str; 7] = [
    "lists", "math", "strings", "base64", "optional", "sets", "encoders",
];

static INSTALLED: OnceLock<Installed> = OnceLock::new();

/// An extension of the `cel` crate, as it installs itself.
type Extension = fn(&mut Env) -> Result<(), DeclarationError>;

/// The environment the composition root installed, with the namespaces its
/// libraries add.
struct Installed {
    env: Arc<Env>,
    namespaces: BTreeSet<String>,
}

/// The environment without any library: the `cel` crate's standard library
/// and extensions as shipped, and the edit constructors of a fix. The
/// composition root registers the libraries of its plugins into it and
/// [`install`]s the result; spec never knows a library.
pub fn base() -> Result<Env, String> {
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
    crate::edit::install(&mut env).map_err(|e| format!("the CEL edit constructors: {e}"))?;
    Ok(env)
}

/// Installs the one environment of the process, once, before any expression
/// is compiled: `namespaces` are those of the functions its libraries added.
/// A second call is an error, so that two compositions never disagree.
pub fn install(env: Env, namespaces: impl IntoIterator<Item = String>) -> Result<(), String> {
    INSTALLED
        .set(Installed {
            env: Arc::new(env),
            namespaces: namespaces.into_iter().collect(),
        })
        .map_err(|_| "the CEL environment is installed already".to_owned())
}

/// The environment the composition root installed.
pub fn environment() -> Result<Arc<Env>, String> {
    INSTALLED.get().map(|i| i.env.clone()).ok_or_else(|| {
        "the CEL environment is not installed: the composition root installs it before any expression is compiled"
            .to_owned()
    })
}

/// Whether `name` is the namespace of functions (`regex` of `regex.extract`),
/// as the `cel` crate or an installed library has them.
pub(crate) fn is_namespace(name: &str) -> bool {
    NAMESPACES.contains(&name) || INSTALLED.get().is_some_and(|i| i.namespaces.contains(name))
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
