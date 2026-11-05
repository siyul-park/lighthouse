use std::{any::Any, collections::BTreeSet, sync::Arc};

use serde::{Deserialize, Serialize};

use lighthouse_graph::Graph;

use crate::{Error, Rule, Workspace};

/// What a run may ask of the query graph of the engine that runs it: the value
/// of the kind `kind` at `key`, which the caller knows the type of.
pub trait Queries: Sync {
    /// The value of `kind` at `key`.
    fn pull(&self, kind: &str, key: &str) -> Result<Arc<dyn Any + Send + Sync>, Error>;
}

/// What the facts of rules are made of, supplied by the plugin that owns them:
/// the inputs they read, and the queries over the graph of the engine that
/// answer them.
pub trait Pulling: Send + Sync {
    /// Sets the inputs the facts of `rules` read.
    fn inputs(
        &self,
        graph: &mut Graph,
        rules: &[&dyn Rule],
        ws: &Workspace,
    ) -> Result<(), lighthouse_graph::Error>;

    /// The queries over `graph` a rule may pull from.
    fn queries<'g>(&self, graph: &'g Graph) -> Box<dyn Queries + 'g>;
}

/// The queries of a run that has no graph: every pull fails.
pub struct NoQueries;

impl Queries for NoQueries {
    fn pull(&self, kind: &str, _: &str) -> Result<Arc<dyn Any + Send + Sync>, Error> {
        Err(Error::Failed(format!("`{kind}` is asked of no graph")))
    }
}

/// The fields of the model's values that the expressions of rules read: what
/// a fact is built with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reads {
    pub fields: BTreeSet<String>,
    /// The fields read from something other than the subject itself.
    pub related: BTreeSet<String>,
    /// Some expression computes the name of the field it reads.
    pub dynamic: bool,
    pub related_dynamic: bool,
}

impl Reads {
    /// Adds what `other` reads.
    pub fn merge(&mut self, other: &Self) {
        self.fields.extend(other.fields.iter().cloned());
        self.related.extend(other.related.iter().cloned());
        self.dynamic |= other.dynamic;
        self.related_dynamic |= other.related_dynamic;
    }
}
