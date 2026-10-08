# Learning

This page holds the methods behind the learned part of a check and behind repository mining.
The model they serve is defined in [rule-pipeline.md](rule-pipeline.md). Everything here is a
policy for plugins (Embedder, Classifier, Judge), not part of the core. Methods can change
without changing the decision model.

## Contract

A learned part must:
- be trained only from recorded evidence of its decision, by an explicit `lighthouse learn train <decision>`;
- beat the baselines on held-out data;
- output calibrated probabilities and abstain between two thresholds; an abstained case goes to the judge, or is reported as SARIF `review` when there is no judge;
- be reproducible from a recorded tuple:
  - meaning version and check revision;
  - feature version;
  - embedder id and model version;
  - model version.

Models are local derived artifacts, rebuildable from `decisions.jsonl`, and are not committed.
Attaching one is a `minor` revision with an evaluation report.

## Features

| Group | Examples |
| --- | --- |
| Structural | code-model facts of the subject and delta: edge direction, depth, kind, ownership, graph distance, metrics |
| Semantic (compact) | similarity to the nearest `fail`/`pass`, prototype similarity, cluster distance |
| Historical | the decision's precision, judgment counts of similar subjects |

- **No leakage.** Inputs are only what is known before judging. A judge's reason is never an input.
- **Out-of-fold similarity.** Neighbour features exclude the subject and its near-duplicates. Splits are by project, cluster and time.
- **Raw embeddings** are an ablation, not the default input.

## Models and calibration

- **Baselines.** A smoothed rate per cluster (Beta prior from the decision's precision) and a kNN model. A gradient-boosted tree model (LightGBM-class) is used only when it beats both.
- **Label weights** follow judgment strength (see [rule-pipeline.md](rule-pipeline.md#evidence)). A model trained mostly on judge or mined labels is capped at `warn`/`info`, and its agreement with human judgments is reported separately.
- **Calibration.** Isotonic or Platt calibration on a holdout set. Thresholds are recomputed on every retrain to meet the decision's precision target.
- **Exploration.** A fixed fraction of confident calls on both sides is re-judged.

## Embeddings

- Provided by the `Embedder` plugin kind: `embed(texts) → vectors`, with model-specific prompts in its manifest.
- Local by default; a remote provider is opt-in per project.
- Vectors are cached by content hash and are never written to `decisions.jsonl`.
- Without an embedder, similarity falls back to structural signals.
- A current fit is a small code embedding model run locally. Example: EmbeddingGemma 2, 270M parameters, 8k tokens, Matryoshka 768 → 256 dimensions, Apache-2.0.

## Repository mining

Mining produces weak evidence and `proposed` decisions.

1. **Collect.** On `init` over a bounded window (the last N commits), then after each commit:
   - **Change direction.** Only refactor-like commits are labelled: symbols moved or renamed, edges removed, tests unchanged, behaviour-neutral. Before is `fail`, after is `pass`. A later revert flips the label.
   - **Prevalence.** Within a cluster of similar code in the current tree, the dominant shape is `pass` and outliers are candidates. Needs no history.
2. **Group.**
   - **Structure first.** The key is the code-model delta signature, with identifiers abstracted: edge kind added or removed, symbol moved across an owner or module, visibility changed, test added, order changed. Embeddings alone would cluster by topic, not by decision.
   - **Embedding second.** Within one signature, embeddings refine the clusters.
   - **Support and consistency.** A cluster needs at least *k* members, at least two commits or authors, and no reverse edits. Contradictions produce a conflict report.
   - **Match before create.** A cluster matching an existing decision becomes a `minor` revision proposal instead.
3. **Propose.** A `proposed` decision with:
   - a requirement drafted from the delta signature;
   - examples from members (before → `invalid`, after → `valid`/`fixed`);
   - a judge prompt;
   - a learned part when one beats the baselines;
   - a deterministic draft when the model's trees allow it.

   A rejection suppresses the cluster until its evidence changes.
