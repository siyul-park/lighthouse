use std::{
    any::{Any, TypeId, type_name},
    fmt::Debug,
    marker::PhantomData,
    sync::Arc,
};

use serde::{Serialize, de::DeserializeOwned};

use crate::{Error, Graph, node::Hash};

/// What a key is: encoded to identify a node, decoded to run its query.
pub trait Key: Serialize + DeserializeOwned + Debug + 'static {}
impl<T: Serialize + DeserializeOwned + Debug + 'static> Key for T {}

/// What a value is: encoded to be hashed and stored.
pub trait Value: Serialize + DeserializeOwned + Send + Sync + 'static {}
impl<T: Serialize + DeserializeOwned + Send + Sync + 'static> Value for T {}

/// A kind of node, with its types erased so the graph can run and decode it
/// from a name read off the disk.
pub(crate) trait Kind: Send + Sync {
    fn name(&self) -> &'static str;
    fn is_input(&self) -> bool;
    fn persisted(&self) -> bool;
    fn execute(&self, graph: &Graph, key: &[u8]) -> Result<Output, Error>;
    fn decode(&self, blob: &[u8]) -> Option<Arc<dyn Any + Send + Sync>>;
    fn describe(&self, key: &[u8]) -> String;
    /// What the name stands for: input or query, and the key and value types.
    fn signature(&self) -> Signature;
}

/// What a kind name stands for. A name is global to a graph: it identifies
/// the nodes of the kind in the store, so two kinds of one name must agree.
pub(crate) struct Signature {
    input: bool,
    key: TypeId,
    value: TypeId,
    text: String,
}

impl Signature {
    fn of<K: Key, V: Value>(input: bool, name: &str) -> Self {
        Self {
            input,
            key: TypeId::of::<K>(),
            value: TypeId::of::<V>(),
            text: format!(
                "{} {name}<{}, {}>",
                if input { "input" } else { "query" },
                type_name::<K>(),
                type_name::<V>()
            ),
        }
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        (self.input, self.key, self.value) == (other.input, other.key, other.value)
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

/// A value as computed: itself, and the bytes it hashes and is stored as.
pub(crate) struct Output {
    pub value: Arc<dyn Any + Send + Sync>,
    pub blob: Arc<[u8]>,
}

impl Output {
    pub(crate) fn new<V: Value>(value: V) -> Result<Self, Error> {
        let blob = encode(&value)?.into();
        Ok(Self {
            value: Arc::new(value),
            blob,
        })
    }
}

/// A value set from outside, such as the content of a file. Declare it as a
/// `static`.
pub struct Input<K, V> {
    name: &'static str,
    persisted: bool,
    types: PhantomData<fn(K) -> V>,
}

impl<K, V> Input<K, V> {
    /// An input kind named `name`, which identifies its nodes in the store.
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            persisted: false,
            types: PhantomData,
        }
    }

    /// Keeps the last value set in the store: it is readable after a reopen
    /// until it is set again. An input that is not persisted reads as unset
    /// in a process that did not set it.
    pub const fn persisted(self) -> Self {
        Self {
            persisted: true,
            ..self
        }
    }
}

impl<K: Key, V: Value> Kind for Input<K, V> {
    fn name(&self) -> &'static str {
        self.name
    }

    fn is_input(&self) -> bool {
        true
    }

    fn persisted(&self) -> bool {
        self.persisted
    }

    fn execute(&self, _: &Graph, key: &[u8]) -> Result<Output, Error> {
        Err(Error::Encode(format!(
            "{} is an input: it is set, not run",
            describe::<K>(self.name, key)
        )))
    }

    fn decode(&self, blob: &[u8]) -> Option<Arc<dyn Any + Send + Sync>> {
        decode_value::<V>(blob)
    }

    fn describe(&self, key: &[u8]) -> String {
        describe::<K>(self.name, key)
    }

    fn signature(&self) -> Signature {
        Signature::of::<K, V>(true, self.name)
    }
}

/// A value derived by a pure function of its key and of what it reads
/// through the [`Graph`]. Declare it as a `static`. A cycle among queries is
/// reported as [`Error::Cycle`], so a query passes the errors of what it
/// reads up with `?`.
pub struct Query<K, V> {
    name: &'static str,
    run: fn(&Graph, &K) -> Result<V, Error>,
}

impl<K, V> Query<K, V> {
    /// A query kind named `name` that computes a value with `run`.
    pub const fn new(name: &'static str, run: fn(&Graph, &K) -> Result<V, Error>) -> Self {
        Self { name, run }
    }
}

impl<K: Key, V: Value> Kind for Query<K, V> {
    fn name(&self) -> &'static str {
        self.name
    }

    fn is_input(&self) -> bool {
        false
    }

    fn persisted(&self) -> bool {
        false
    }

    fn execute(&self, graph: &Graph, key: &[u8]) -> Result<Output, Error> {
        let key: K = decode(key)?;
        Output::new((self.run)(graph, &key)?)
    }

    fn decode(&self, blob: &[u8]) -> Option<Arc<dyn Any + Send + Sync>> {
        decode_value::<V>(blob)
    }

    fn describe(&self, key: &[u8]) -> String {
        describe::<K>(self.name, key)
    }

    fn signature(&self) -> Signature {
        Signature::of::<K, V>(false, self.name)
    }
}

pub(crate) fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, Error> {
    postcard::to_stdvec(value).map_err(|err| Error::Encode(err.to_string()))
}

pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    postcard::from_bytes(bytes).map_err(|err| Error::Encode(err.to_string()))
}

pub(crate) fn hash(blob: &[u8]) -> Hash {
    blake3::hash(blob).into()
}

fn decode_value<V: Value>(blob: &[u8]) -> Option<Arc<dyn Any + Send + Sync>> {
    let value: V = postcard::from_bytes(blob).ok()?;
    Some(Arc::new(value))
}

fn describe<K: Key>(name: &str, key: &[u8]) -> String {
    match postcard::from_bytes::<K>(key) {
        Ok(key) => format!("{name}({key:?})"),
        Err(_) => format!("{name}(?)"),
    }
}
