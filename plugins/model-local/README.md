# model-local

The model that is trained from the labels of a project: it embeds the labelled subjects, fits the simplest learner the data supports, and answers `lighthouse/classify` with the stored model its reference names.

**Layer.** Out-of-process plugin; depends on `lighthouse-protocol` and `lighthouse-model`. Built by `make plugins`; its manifest provides the model `model-local`, whose one model is `learned`, referenced as `learned@sha256:<hex>`.

## Entry points
- `serve`: the handshake and handler for `lighthouse/classify` and `lighthouse/train`.
- `train`: a training file of the host's dataset store in, an artifact in the host's model store out.
- `rows`: reads a JSONL training file into `Line`s (text, label, weight, group, source) and the examples of its question.
- `embed`: EmbeddingGemma-300M through `fastembed`, loaded only when a learner needs vectors.
- `fit`: scores every candidate by out-of-fold Brier (`folds`) and keeps the simplest within one standard error of the best.
- `candidates`: the learners, simplest first: `rate`, `knn` (k = 5), `logistic`; `newton` is the logistic fit and Platt scaling; `band` is the card's band and AUROC.
- `artifact`: the model as one JSON document stored at `<dir>/sha256/<hex>.json`.

## Invariants
- The same training file gives the same artifact, byte for byte; the digest is that of the RFC 8785 canonical form, with numbers kept to six significant digits.
- Folds are by group (one file), so a subject is never scored by a model fitted on its own group.
- Below 10 labels in a class only the rate is fitted, and nothing is embedded.
- A model answers only the decision it was trained for; a missing or altered artifact is an error naming its path and digest.

## Tests
Unit tests in `src/` and process tests in `plugin_tests.rs` with a fake embedder; `cargo test -p model-local`. The test that downloads the embedder's weights is ignored. Models in [docs/models.md](../../docs/models.md).
