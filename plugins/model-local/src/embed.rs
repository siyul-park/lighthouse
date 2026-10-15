//! The one place text becomes vectors: EmbeddingGemma-300M (fp32, CPU, ONNX)
//! through fastembed, with the document prompt of its model card. Vectors
//! are unit length and rounded to 1e-5, so that the jitter of the runtime
//! moves neither a digest nor a score.

use std::{
    collections::{BTreeMap, btree_map::Entry},
    path::PathBuf,
    str::FromStr,
};

use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

/// The embedder, by the name of its fastembed variant.
pub const DEFAULT: &str = "EmbeddingGemma300M";
/// The document prompt of the model card.
const PREFIX: &str = "title: none | text: ";
const MAX_LENGTH: usize = 512;
const BATCH: usize = 32;
/// Components are rounded to a multiple of 1/this.
const SCALE: f64 = 1e5;

type Run = Box<dyn FnMut(&[String]) -> Result<Vec<Vec<f32>>, String>>;

/// An embedder that is loaded.
pub struct Embedder {
    name: String,
    dim: usize,
    run: Run,
}

impl Embedder {
    /// Loads the fastembed model `name`; the first use downloads its weights.
    pub fn open(name: &str) -> Result<Self, String> {
        let model = EmbeddingModel::from_str(name)?;
        let dim = TextEmbedding::get_model_info(&model)
            .map_err(|e| e.to_string())?
            .dim;
        let dir = cache_dir(|key| std::env::var(key).ok());
        eprintln!(
            "model-local: loading {model:?}; the first use downloads its weights to {}",
            dir.display()
        );
        let options = TextInitOptions::new(model.clone())
            .with_max_length(MAX_LENGTH)
            .with_cache_dir(dir)
            .with_show_download_progress(false);
        let mut session = TextEmbedding::try_new(options).map_err(|e| e.to_string())?;
        let run =
            move |texts: &[String]| session.embed(texts, Some(BATCH)).map_err(|e| e.to_string());
        Ok(Self::new(format!("{model:?}"), dim, Box::new(run)))
    }

    /// An embedder that gives `dim` numbers for each text by `run`.
    pub fn new(name: impl Into<String>, dim: usize, run: Run) -> Self {
        Self {
            name: name.into(),
            dim,
            run,
        }
    }

    /// `<name>@<dimension>`: what a card and an artifact call the embedder.
    pub fn id(&self) -> String {
        format!("{}@{}", self.name, self.dim)
    }

    /// The unit vector of each text.
    pub fn vectors(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let prompted: Vec<String> = texts.iter().map(|t| format!("{PREFIX}{t}")).collect();
        let raw = (self.run)(&prompted)?;
        if raw.len() != texts.len() || raw.iter().any(|v| v.len() != self.dim) {
            return Err(format!(
                "{} did not give {} numbers a text",
                self.name, self.dim
            ));
        }
        Ok(raw.into_iter().map(unit).collect())
    }
}

/// How an embedder is opened by its name.
pub type Open = fn(&str) -> Result<Embedder, String>;

/// The embedders opened so far: one is loaded once however often it is asked.
pub struct Embedders {
    open: Open,
    loaded: BTreeMap<String, Embedder>,
}

impl Embedders {
    pub fn new(open: Open) -> Self {
        Self {
            open,
            loaded: BTreeMap::new(),
        }
    }

    pub fn get(&mut self, name: &str) -> Result<&mut Embedder, String> {
        match self.loaded.entry(name.to_owned()) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),
            Entry::Vacant(entry) => Ok(entry.insert((self.open)(name)?)),
        }
    }
}

/// Where fastembed keeps its weights: `FASTEMBED_CACHE_DIR`, else
/// `$XDG_CACHE_HOME/lighthouse/fastembed`, else `~/.cache/lighthouse/fastembed`.
pub fn cache_dir(var: impl Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(dir) = var("FASTEMBED_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    let base = var("XDG_CACHE_HOME").map_or_else(
        || PathBuf::from(var("HOME").unwrap_or_default()).join(".cache"),
        PathBuf::from,
    );
    base.join("lighthouse").join("fastembed")
}

/// `vector` at unit length, each component rounded to 1e-5.
fn unit(vector: Vec<f32>) -> Vec<f32> {
    let norm = vector
        .iter()
        .map(|x| f64::from(*x).powi(2))
        .sum::<f64>()
        .sqrt();
    let scale = if norm > 0.0 { 1.0 / norm } else { 1.0 };
    vector
        .iter()
        .map(|x| ((f64::from(*x) * scale * SCALE).round() / SCALE) as f32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn weights_are_cached_where_the_environment_says() {
        let at = |pairs: &[(&str, &str)]| cache_dir(env(pairs));
        assert_eq!(at(&[("FASTEMBED_CACHE_DIR", "/w")]), PathBuf::from("/w"));
        assert_eq!(
            at(&[("XDG_CACHE_HOME", "/x"), ("HOME", "/h")]),
            PathBuf::from("/x/lighthouse/fastembed")
        );
        assert_eq!(
            at(&[("HOME", "/h")]),
            PathBuf::from("/h/.cache/lighthouse/fastembed")
        );
    }

    #[test]
    fn vectors_are_unit_length_rounded_and_prompted() {
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let spy = seen.clone();
        let mut embedder = Embedder::new(
            "Fake",
            2,
            Box::new(move |texts| {
                spy.borrow_mut().extend_from_slice(texts);
                Ok(texts.iter().map(|_| vec![3.0, 4.0]).collect())
            }),
        );
        let vectors = embedder.vectors(&["a".to_owned()]).unwrap();

        assert_eq!(vectors, [[0.6, 0.8]]);
        assert_eq!(seen.borrow()[0], "title: none | text: a");
        assert_eq!(embedder.id(), "Fake@2");
    }

    #[test]
    fn a_vector_of_another_size_is_an_error() {
        let mut embedder = Embedder::new("Fake", 3, Box::new(|t| Ok(vec![vec![1.0]; t.len()])));
        assert!(
            embedder
                .vectors(&["a".to_owned()])
                .unwrap_err()
                .contains("3 numbers")
        );
    }

    /// Downloads the weights on first use: run by hand with `--ignored`.
    #[test]
    #[ignore = "downloads the embedder weights"]
    fn the_real_embedder_gives_768_unit_numbers_and_the_same_text_the_same_vector() {
        let mut embedder = Embedder::open(DEFAULT).unwrap();
        let texts = ["func a() {}".to_owned(), "func a() {}".to_owned()];
        let vectors = embedder.vectors(&texts).unwrap();

        assert!(embedder.id().ends_with("@768"));
        assert_eq!(vectors[0].len(), 768);
        let dot = |a: &[f32], b: &[f32]| -> f64 {
            a.iter()
                .zip(b)
                .map(|(x, y)| f64::from(*x) * f64::from(*y))
                .sum()
        };
        assert!((dot(&vectors[0], &vectors[0]) - 1.0).abs() < 1e-3);
        assert!((dot(&vectors[0], &vectors[1]) - 1.0).abs() < 1e-3);
        assert_eq!(vectors[0], vectors[1]);
    }
}
