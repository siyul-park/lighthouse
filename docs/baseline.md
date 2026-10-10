# Baseline

The numbers that search, learned checks and the caches are judged against: what a strict run finds on four
reference repositories, how long it takes, and how precise each reviewed decision is. They come from the
code at R3b (`474266b`) and R3c, on an Apple M4 Pro, 24 GiB. `scripts/baseline.sh` reproduces the first two
parts; the data lives in `baseline/`.

## Reference repositories

All four run with the strict preset (`baseline/strict-root.toml` for the Cargo workspace,
`baseline/strict-go.toml` for Go modules): `core/recommended`, `design/strict`, `testing/recommended`.

| repo | language | commit | source files | source lines | findings | exit |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| strict-root | Rust, Go | `81b286d` (Lighthouse, frozen copy, not committed) | 244 rs, 51 go | 50,897 rs, 3,627 go | 767 | 0 |
| cel | Go | `a492a70` + 22 uncommitted files | 473 | 267,726 | 3,976 | 3 |
| qlbridge | Go | `dbc8560` + 1 uncommitted file | 182 | 60,506 | 3,307 | 1 |
| minivm | Go | `de276b3` + 1 uncommitted file | 274 | 188,892 | 1,508 | 1 |

- Files and lines are Go and Rust sources outside `.git`, `target`, `vendor` and `node_modules`; the run may read
  more (cel's vendored modules). Symbol counts are not reported by any command yet; see Known limits.
- cel exits 3 because 40 incomplete notices remain (for example a vendored `.binpb` that is not UTF-8); the
  findings stand.
- The working trees of cel, qlbridge and minivm are the author's checkouts, so their numbers move with them. The
  strict-root copy is fixed: it is `81b286d`, the Lighthouse tree the R2 gates started from. Findings per decision
  are in `baseline/<repo>.json`.

## Timings

Median of 3 runs, in milliseconds, release build, Go build cache warm, `--no-store`. `index` sums the language
providers (Go type-checks through `go list`, so it is the largest phase on Go repositories); `rules` is wall clock.

| repo | setup | read | index | merge | analyzers | rules | identity | report | total |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| strict-root | 10 | 16 | 503 | 25 | 6 | 264 | 6 | 1 | 734 |
| cel | 8 | 37 | 619 | 60 | 13 | 608 | 24 | 3 | 1421 |
| qlbridge | 8 | 18 | 384 | 30 | 6 | 338 | 18 | 2 | 832 |
| minivm | 8 | 18 | 1143 | 117 | 7 | 361 | 10 | 1 | 1699 |

Phases overlap slightly (rules run while later providers finish), so they do not sum to the total.

## Precision per decision

A reviewer read each sampled finding against the decision's requirement and called it true (TP) or false (FP).
The interval is Wilson 95%. Rows that are not warn or error rules, or that sample little, are indications only.
Sample files are `baseline/precision/<decision>.tsv` (fingerprint, repo, location, verdict, reason).

| decision | review | TP / sample | precision | Wilson 95% | samples |
| --- | --- | ---: | ---: | --- | --- |
| design/complexity | R3a | 23 / 23 | 100% | 86-100 | retained |
| design/cognitive-complexity | R3a | 20 / 20 | 100% | 84-100 | retained |
| design/max-statements | R3a | 19 / 20 | 95% | 76-99 | retained |
| design/max-lines-per-function | R3a | 20 / 20 | 100% | 84-100 | retained |
| design/max-depth | R3a | 20 / 20 | 100% | 84-100 | retained |
| design/max-params | R3a | 18 / 19 | 95% | 75-99 | retained |
| design/max-results | R3a | 20 / 20 | 100% | 84-100 | retained |
| design/error-identity | R3b | 34 / 35 | 97% | 85.5-99.5 | retained (all findings) |
| design/no-private-types | R3b | 8 / 8 | 100% | 67.6-100 | retained (all findings) |
| design/no-stored-context | R3b | 1 / 2 | 50% | 9.5-90.5 | retained (one debatable) |
| design/no-panic (Go) | R3b | 20 / 20 | 100% | 83.9-100 | not retained; 20 of 104 |
| design/no-panic (Rust) | R3b | 21 / 21 | 100% | 84.5-100 | not retained; 21 of 41 |
| design/feature-envy | R2 | 12 / 15 | 80% | 54.8-93.0 | retained by name |
| design/misplaced-symbol | R2 | 13 / 15 | 87% | 62.1-96.3 | retained by name |
| design/owner-file | R2 | 15 / 15 | 100% | 79.6-100 | 4 of 15 named |
| design/unique-type-names | R2 | 5 / 5 | 100% | 56.6-100 | retained by name |
| design/tiny-modules | R2 | 5 / 5 | 100% | 56.6-100 | retained by name |
| testing/no-hidden-target | R2 | 0 / 5 | 0% | 0-43.4 | retained by name |

- `no-panic` is info: the review checked that each finding is a real panic site (all were), and that about one in
  twenty of the Go sites is worth changing; the rest are invariants and recovered hot paths. Rust: none of the 21.
- `no-hidden-target` is wrong on a pattern: a helper builds an environment with the target passed as an argument,
  which the code model cannot tell from a helper that asserts. It finds 7 in cel and nothing elsewhere.
- `design/max-statements` and `design/max-params` count one false positive each: a benchmark fixture that emits a
  byte program, and a method that implements an interface outside the project.
- R2 samples were five per decision and kept by name; where a finding still exists in the R3b run the TSV carries
  its fingerprint, else `-`. R3a fingerprints are those of the reviewed run, and R3b's are from the final run.

## Known limits

- One reviewer judged every sample, an agent reading the finding and its source (R2 judged some from the message
  alone); there is no second opinion or agreement measure.
- Samples, not exhaustive: random for R3a (seed 7) and the limit rules, complete for the small R3b rules,
  a handful per decision for R2. The intervals say how little five or twenty findings settle.
- Precision here is the share of findings that are right, not recall: what a rule misses is not measured.
- Fingerprints are stable only while the identity of a finding is; R3b changed the identity of event findings, so
  older samples may no longer match a run.
- The numbers are of one machine, a warm Go build cache and the strict preset; the preset the projects run with
  differs per project.
- Symbols per repository are missing: no command reports the size of the code model. Add one when the caches
  need it.
