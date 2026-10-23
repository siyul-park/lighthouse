//! The CEL library plugin kind: functions a decision's expressions may call,
//! beyond the `cel` crate's own. A library describes itself in a manifest (its
//! id, version and the functions it provides) and registers them into a CEL
//! environment; the registry's union of the manifests is the one function
//! catalog. Libraries run in this process only: they are called once per node.

use std::fmt;

use cel::Env;

use crate::Error;

/// Where a function comes from, in the order a decision's author prefers them
/// (`rules/cel-everywhere.md` section 2.4). A library never holds step 1: the
/// `cel` crate already provides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Precedence {
    /// The CEL spec or an official extension (cel-go `ext`, Kubernetes CEL
    /// libraries) defines it: official name, overloads and meaning.
    Official,
    /// Nothing official has it: designed after the nearest official precedent.
    Designed,
}

impl Precedence {
    /// The step of the precedence rule: 2 for an official extension, 3 for a
    /// design of ours.
    pub fn step(self) -> u8 {
        match self {
            Self::Official => 2,
            Self::Designed => 3,
        }
    }
}

/// One overload of a function: the types of its arguments (not counting the
/// receiver of a member function) and of its result, as CEL spells them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Overload {
    pub params: Vec<String>,
    pub result: String,
}

impl Overload {
    /// An overload of `params` giving `result`.
    pub fn new(params: &[&str], result: &str) -> Self {
        Self {
            params: params.iter().map(|p| (*p).to_owned()).collect(),
            result: result.to_owned(),
        }
    }
}

/// A function a library provides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    /// The name a call writes, with its namespace for a global function
    /// (`regex.replace`, `glob.matches`), without it for a member (`indexOf`).
    pub name: String,
    /// The type of the value a member function is called on (`list`); `None`
    /// for a global or namespaced function.
    pub receiver: Option<String>,
    pub overloads: Vec<Overload>,
    pub doc: String,
    pub example: String,
    pub precedence: Precedence,
}

impl Function {
    /// The namespace of the name (`regex` of `regex.replace`), if it has one.
    pub fn namespace(&self) -> Option<&str> {
        self.name.split_once('.').map(|(namespace, _)| namespace)
    }

    /// Whether an overload takes `args` arguments (not counting the value a
    /// member function is called on).
    pub fn accepts(&self, args: usize) -> bool {
        self.overloads.iter().any(|o| o.params.len() == args)
    }
}

impl fmt::Display for Function {
    /// The overloads as called, receiver first: `list.indexOf(dyn) -> int`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = self
            .receiver
            .as_deref()
            .map_or(String::new(), |r| format!("{r}."));
        let overloads: Vec<String> = self
            .overloads
            .iter()
            .map(|o| {
                format!(
                    "{prefix}{}({}) -> {}",
                    self.name,
                    o.params.join(", "),
                    o.result
                )
            })
            .collect();
        f.write_str(&overloads.join("; "))
    }
}

/// What a library declares about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CelLibraryManifest {
    /// Fully qualified `plugin/name`.
    pub id: String,
    pub version: String,
    pub functions: Vec<Function>,
}

/// Registers the functions of its manifest into an environment (cel-go's
/// `cel.Library`). `register` must install exactly what the manifest lists.
pub trait CelLibrary: Send + Sync {
    fn manifest(&self) -> &CelLibraryManifest;
    fn register(&self, env: &mut Env) -> Result<(), Error>;
}

/// A function of the catalog with the library that provides it.
#[derive(Debug, Clone, Copy)]
pub struct CatalogFunction<'r> {
    pub library: &'r str,
    pub version: &'r str,
    pub function: &'r Function,
}
