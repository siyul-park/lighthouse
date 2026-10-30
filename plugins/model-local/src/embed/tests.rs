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
        serde_json::from_str(include_str!("../../testdata/fastembed-vectors.json")).unwrap();
    let texts: Vec<String> = serde_json::from_value(fixture["texts"].clone()).unwrap();
    let made: Vec<Vec<f32>> = serde_json::from_value(fixture["vectors"].clone()).unwrap();

    let vectors = Embedder::open(DEFAULT).unwrap().vectors(&texts).unwrap();

    for (text, (vector, made)) in texts.iter().zip(vectors.iter().zip(&made)) {
        let cosine = cosine(vector, made);
        assert!(cosine >= 0.999_999, "{text:?}: {cosine}");
    }
}
