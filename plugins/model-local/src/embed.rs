//! The one place text becomes vectors: EmbeddingGemma-300M (fp32, CPU, ONNX)
//! through ONNX Runtime, with the document prompt of its model card. Vectors
//! are unit length and rounded to 1e-5, so that the jitter of the runtime
//! moves neither a digest nor a score.
//!
//! Each text is run alone, padded to a length bucket of its own: ONNX Runtime
//! rounds a row differently by the texts it is batched with (about 5e-7), so
//! only a vector that is a function of its text alone is the same however the
//! texts are grouped by the caller.

use std::{
    collections::{BTreeMap, btree_map::Entry},
    path::{Path, PathBuf},
};

use hf_hub::{Repo, RepoType, api::sync::ApiBuilder};
use ort::{session::Session, value::Tensor};
use tokenizers::{Tokenizer, TruncationParams};

/// The embedder, by its model name.
pub const DEFAULT: &str = "EmbeddingGemma300M";
/// What the runtime and the padding make of the model: part of the embedder's
/// name, so a model made with another is told apart and not served mismatched
/// vectors.
const VERSION: &str = "ort1";
const REPO: &str = "onnx-community/embeddinggemma-300m-ONNX";
/// The commit of the repository the weights are taken from.
const REVISION: &str = "5090578d9565bb06545b4552f76e6bc2c93e4a66";
const FILES: [&str; 3] = ["tokenizer.json", "onnx/model.onnx", "onnx/model.onnx_data"];
/// The document prompt of the model card.
const PREFIX: &str = "title: none | text: ";
const DIM: usize = 768;
const MAX_LENGTH: usize = 512;
/// The lengths a text is padded to: the first that holds it.
const BUCKETS: [usize; 4] = [64, 128, 256, MAX_LENGTH];
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
    /// Loads the embedder `name` (`EmbeddingGemma300M`, with or without the
    /// version it was made with); the first use downloads its weights.
    pub fn open(name: &str) -> Result<Self, String> {
        let model = name.split_once(':').map_or(name, |(model, _)| model);
        if model != DEFAULT {
            return Err(format!("no embedder `{model}`: only {DEFAULT} is built in"));
        }
        let dir = cache_dir(|key| std::env::var(key).ok());
        eprintln!(
            "model-local: loading {DEFAULT}; the first use downloads its weights to {}",
            dir.display()
        );
        let [tokenizer, model, _] = FILES.map(|file| fetch(&dir, file));
        let mut gemma = Gemma::load(&tokenizer?, &model?)?;
        let run = move |texts: &[String]| gemma.embed(texts);
        Ok(Self::new(
            format!("{DEFAULT}:{VERSION}"),
            DIM,
            Box::new(run),
        ))
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

/// The model and its tokenizer.
struct Gemma {
    session: Session,
    tokenizer: Tokenizer,
}

impl Gemma {
    fn load(tokenizer: &Path, model: &Path) -> Result<Self, String> {
        let mut tokenizer =
            Tokenizer::from_file(tokenizer).map_err(|e| format!("{}: {e}", tokenizer.display()))?;
        tokenizer
            .with_padding(None)
            .with_truncation(Some(TruncationParams {
                max_length: MAX_LENGTH,
                ..TruncationParams::default()
            }))
            .map_err(|e| e.to_string())?;
        let session = Session::builder()
            .and_then(|mut builder| builder.commit_from_file(model))
            .map_err(|e| format!("{}: {e}", model.display()))?;
        Ok(Self { session, tokenizer })
    }

    /// The vector of each text, in the order of the texts.
    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        texts.iter().map(|text| self.embed_one(text)).collect()
    }

    /// The vector of one text, padded to the bucket of its length.
    fn embed_one(&mut self, text: &str) -> Result<Vec<f32>, String> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| e.to_string())?;
        let tokens = encoding.get_ids();
        let len = bucket_of(tokens.len());
        let mut ids = vec![0_i64; len];
        for (id, token) in ids.iter_mut().zip(tokens) {
            *id = i64::from(*token);
        }
        let mut mask = vec![0_i64; len];
        mask[..tokens.len()].fill(1);
        let tensor = |data| Tensor::from_array(([1, len], data)).map_err(|e| e.to_string());
        let outputs = self
            .session
            .run(ort::inputs![
                "input_ids" => tensor(ids)?,
                "attention_mask" => tensor(mask)?
            ])
            .map_err(|e| e.to_string())?;
        let (_, data) = outputs["sentence_embedding"]
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;
        Ok(data.to_vec())
    }
}

/// Where the weights are kept: `FASTEMBED_CACHE_DIR`, else
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

/// `file` of the pinned revision, in the cache `dir` (the layout of the
/// Hugging Face hub), downloaded when it is not there.
fn fetch(dir: &Path, file: &str) -> Result<PathBuf, String> {
    let repo = Repo::with_revision(REPO.to_owned(), RepoType::Model, REVISION.to_owned());
    let snapshot = dir
        .join(repo.folder_name())
        .join("snapshots")
        .join(REVISION);
    if snapshot.join(file).exists() {
        return Ok(snapshot.join(file));
    }
    let api = ApiBuilder::new()
        .with_cache_dir(dir.to_path_buf())
        .with_progress(false)
        .build()
        .map_err(|e| format!("hugging face: {e}"))?;
    api.repo(repo)
        .get(file)
        .map_err(|e| format!("{REPO}/{file}: {e}"))
}

/// The first bucket that holds `tokens`.
fn bucket_of(tokens: usize) -> usize {
    BUCKETS
        .into_iter()
        .find(|bucket| *bucket >= tokens)
        .unwrap_or(MAX_LENGTH)
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

    #[test]
    fn an_embedder_that_is_not_built_in_is_an_error() {
        assert!(Embedder::open("Other").is_err());
    }

    #[test]
    fn a_text_is_padded_to_the_first_bucket_that_holds_it() {
        assert_eq!(bucket_of(0), 64);
        assert_eq!(bucket_of(64), 64);
        assert_eq!(bucket_of(65), 128);
        assert_eq!(bucket_of(300), 512);
        assert_eq!(bucket_of(MAX_LENGTH + 1), MAX_LENGTH);
    }

    fn dot(a: &[f32], b: &[f32]) -> f64 {
        a.iter()
            .zip(b)
            .map(|(x, y)| f64::from(*x) * f64::from(*y))
            .sum()
    }

    fn cosine(a: &[f32], b: &[f32]) -> f64 {
        dot(a, b) / (dot(a, a) * dot(b, b)).sqrt()
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
        assert!((dot(&vectors[0], &vectors[0]) - 1.0).abs() < 1e-3);
        assert!((dot(&vectors[0], &vectors[1]) - 1.0).abs() < 1e-3);
        assert_eq!(vectors[0], vectors[1]);
    }

    #[test]
    #[ignore = "downloads the embedder weights"]
    fn a_text_has_the_same_vector_alone_and_among_texts_of_other_lengths() {
        let mut embedder = Embedder::open(DEFAULT).unwrap();
        let word = "return fmt.Errorf(\"open %s: %w\", path, err) ";
        let others: Vec<String> = (0..20).map(|i| word.repeat(1 + i * 7)).collect();
        let text = "func a() { return nil }".to_owned();
        let alone = embedder.vectors(std::slice::from_ref(&text)).unwrap();

        let mut among = others[..10].to_vec();
        among.push(text.clone());
        among.extend_from_slice(&others[10..]);
        let together = embedder.vectors(&among).unwrap();

        assert_eq!(together[10], alone[0]);
    }

    #[test]
    #[ignore = "downloads the embedder weights"]
    fn the_vectors_are_those_fastembed_made() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/fastembed-vectors.json")).unwrap();
        let texts: Vec<String> = serde_json::from_value(fixture["texts"].clone()).unwrap();
        let made: Vec<Vec<f32>> = serde_json::from_value(fixture["vectors"].clone()).unwrap();

        let vectors = Embedder::open(DEFAULT).unwrap().vectors(&texts).unwrap();

        for (text, (vector, made)) in texts.iter().zip(vectors.iter().zip(&made)) {
            let cosine = cosine(vector, made);
            assert!(cosine >= 0.999_999, "{text:?}: {cosine}");
        }
    }
}
