//! The CEL environment of the tool: the standard library of CEL with the
//! extensions `cel` ships (strings, lists, math, base64 encoders) and the
//! optional syntax, exactly as that crate provides them, and the edit
//! constructors of a fix, plus the libraries the plugins register. The
//! composition root builds one [`Cel`] and hands it to everything that
//! compiles or runs an expression: the catalog, the checks, the fixes, the
//! variables and the event mappings of the providers.

use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

use cel::{Context, DeclarationError, Env, Program, extensions};

use crate::{Catalog, Error};

/// The namespaces of the functions the `cel` crate ships (`lists.range`,
/// `math.greatest`, ...). The libraries a plugin registers add their own.
const NAMESPACES: [&str; 7] = [
    "lists", "math", "strings", "base64", "optional", "sets", "encoders",
];

/// An extension of the `cel` crate, as it installs itself.
type Extension = fn(&mut Env) -> Result<(), DeclarationError>;

type Compiled = HashMap<String, Result<Arc<Program>, String>>;

/// One CEL environment, with what is derived from it: the namespaces of its
/// functions, the programs compiled in it and the bundled catalog validated
/// against it. Clones share all of it.
#[derive(Clone)]
pub struct Cel(Arc<Inner>);

struct Inner {
    env: Arc<Env>,
    namespaces: BTreeSet<String>,
    compiled: Mutex<Compiled>,
    bundled: OnceLock<Result<Catalog, String>>,
}

impl Cel {
    /// The environment `env`, whose libraries added the functions of
    /// `namespaces`.
    pub fn new(env: Env, namespaces: impl IntoIterator<Item = String>) -> Self {
        Self(Arc::new(Inner {
            env: Arc::new(env),
            namespaces: namespaces.into_iter().collect(),
            compiled: Mutex::default(),
            bundled: OnceLock::new(),
        }))
    }

    /// The environment of the `cel` crate alone, with no library.
    pub fn standard() -> Result<Self, String> {
        Ok(Self::new(base()?, []))
    }

    /// The environment itself.
    pub fn environment(&self) -> Arc<Env> {
        self.0.env.clone()
    }

    /// A context of the environment, to add variables and functions to.
    pub fn context(&self) -> Context<'static, 'static> {
        Context::with_env(self.environment())
    }

    /// `source` compiled in the environment.
    pub fn compile(&self, source: &str) -> Result<Program, String> {
        self.0.env.compile(source).map_err(|e| e.to_string())
    }

    /// `source` compiled in the environment, once: the same program is shared
    /// by whoever asks for it again. The variables of a pack are named by many
    /// decisions, and each is read when the catalog is validated, built into a
    /// rule and built into a fixer.
    pub fn compile_shared(&self, source: &str) -> Result<Arc<Program>, String> {
        if let Some(found) = self.compiled().get(source) {
            return found.clone();
        }
        let made = self
            .0
            .env
            .compile(source)
            .map(Arc::new)
            .map_err(|e| e.to_string());
        self.compiled().insert(source.to_owned(), made.clone());
        made
    }

    /// The catalog compiled into the binary, validated against this
    /// environment once.
    pub fn bundled(&self) -> Result<&Catalog, Error> {
        self.0
            .bundled
            .get_or_init(|| {
                Catalog::from_files(crate::catalog::load::embedded(), self)
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(|why| Error::invalid("the bundled catalog", why.as_str()))
    }

    /// Whether `name` is the namespace of functions (`regex` of
    /// `regex.extract`), as the `cel` crate or a library has them.
    pub fn is_namespace(&self, name: &str) -> bool {
        NAMESPACES.contains(&name) || self.0.namespaces.contains(name)
    }

    fn compiled(&self) -> std::sync::MutexGuard<'_, Compiled> {
        self.0
            .compiled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// The environment without any library: the `cel` crate's standard library
/// and extensions as shipped, and the edit constructors of a fix. The
/// composition root registers the libraries of its plugins into it and builds
/// the [`Cel`] with [`Cel::new`]; spec never knows a library.
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
