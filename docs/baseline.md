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
- The working trees of cel, qlbridge and minivm are the author's checkouts, so their numbers move with them;
  each `baseline/<repo>.json` lists the dirty files and the SHA-256 of `git diff HEAD` that were measured. The
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

## Incremental

What the result cache (host) and the provider unit caches (F2) save. `scripts/fastgate.sh <repo>...` measures
it on a temporary copy of each repository and fails unless every warm run prints the same JSON and exit code
as a `--no-cache` run of the same tree, after each of seven edits (a body-only edit, a rename of a called
function, a new caller in another file, a new exported type with a homonym, a rule option, the check of a
decision, a deleted file). Apple M4 Pro, 24 GiB, release build, Go build cache warm, `--no-store`, median of 3,
total ms. The `lighthouse` row is this repository at the commit measured (Rust and Go, strict preset), not the
frozen `strict-root` copy.

| repo | cold `--no-cache` | cold, empty cache | warm, no change | warm, one-function body edit | edit as share of cold |
| --- | ---: | ---: | ---: | ---: | ---: |
| lighthouse | 971 | 1108 (+14%) | 587 | 904 | 93% |
| cel | 2619 | 3151 (+20%) | 2031 | 2356 | 90% |
| qlbridge | 964 | 1058 (+10%) | 342 | 800 | 83% |
| minivm | 1979 | 2374 (+20%) | 1019 | 1877 | 95% |

**The target is not met.** The design asked for a one-file edit under 40% of cold on each Go repository; it
is at 83–95%. Where the time goes in the edit run (phases, ms; cold `--no-cache` in brackets):

| repo | index | merge | hashing | rules | identity |
| --- | ---: | ---: | ---: | ---: | ---: |
| lighthouse | 533 (677) | 38 (37) | 38 (0) | 207 (320) | 9 (9) |
| cel | 1465 (1489) | 148 (145) | 126 (0) | 423 (801) | 47 (40) |
| qlbridge | 473 (483) | 31 (31) | 28 (0) | 166 (324) | 22 (20) |
| minivm | 1385 (1439) | 116 (115) | 103 (0) | 186 (354) | 12 (11) |

- **Index** is most of the time and barely moves on Go after an edit: the provider still runs `go list` over
  the module graph and loads (type-checks) the changed package and its dependents; only the untouched packages
  come from the cache. On a warm run with nothing changed it falls to 169 ms on qlbridge and 692 ms on minivm,
  so the cache works, but a one-function edit invalidates its package, which is what is reloaded.
- **cel** gains nothing from the provider cache even without a change (index 1476 ms): 40 files of a nested
  module that needs a newer Go than the toolchain are incomplete, and an incomplete unit is never written to
  the cache, so that module is analyzed every run. The 40 incomplete files are the same with and without
  the cache.
- **Rules** drop by about 40–55% after an edit: local and neighbor rules are answered from the cache, but
  rules that read the whole project (declared `global`) run again for every file whenever anything changes.
  Lowering some of them to `neighbors` is the next step.
- **Hashing** the merged project for the keys costs 3–5% of a cold run (28 ms qlbridge, 38 ms this repository,
  103 ms minivm, 126 ms cel). **Merge** is unchanged and is not cached.
- **Writing** the cache on a cold run costs +10–20%: the Go provider writes one record per package and the host
  one row per file or project state; a warm run pays none of it.

## Precision per decision

A reviewer read each sampled finding against the decision's requirement and called it true (TP) or false (FP).
The interval is Wilson 95%. Rows that are not warn or error rules, or that sample little, are indications only.
Sample files are `baseline/precision/<pack>/<decision>.tsv` (fingerprint, repo, location, verdict, reason).

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
| design/no-panic (Go) | R3b | 20 / 20 | 100% | 83.9-100 (not comparable) | not retained; 20 of 104 |
| design/no-panic (Rust) | R3b | 21 / 21 | 100% | 84.5-100 (not comparable) | not retained; 21 of 41 |
| design/feature-envy | R4 | 11 / 27 | 41% | 25-59 | all findings, after the refinements |
| design/misplaced-symbol | R4 | 9 / 36 | 25% | 14-41 | all findings, after the refinements |
| design/no-panic | R4 | 23 / 24 | 96% | 80-99 | 24 of the 107 the R3b sample missed (seed 11) |
| testing/standard-assertions | R4 | 13 / 13 | 100% | 77-100 | all findings, after the refinements |
| testing/owner | R4 | 17 / 22 | 77% | 57-90 | 22 of 651 (seed 5); the other 5 are unclear |

Retired in R4 (a deterministic check below 20% is not a signal; the wording lives on in a model decision,
whose `select` is the old condition, widened for recall; the TSVs keep the evidence):

| retired | became | TP / sample | Wilson 95% |
| --- | --- | ---: | --- |
| design/private-helper-callers | `select` of design/justified-helpers | 1 / 24 | 1-20 |
| design/unique-type-names | `select` of design/terminology | 1 / 27 | 1-18 |
| design/tiny-modules | `select` of design/meaningful-separation | 0 / 20 | 0-16 |
| design/owner-file | model check, `select` of its own | 0 / 22 | 0-15 |
| testing/no-hidden-target (before the fix) | kept; creation helpers no longer followed | 0 / 7 | 0-35 |

- "Not comparable" marks an interval whose samples are not retained: a later review cannot be set against it
  sample by sample, and it counts "real panic site" or "violation of the definition" as the reviewer then meant it.
  Each TSV starts with `# retained: yes|partial|no`; `no-panic.tsv` holds the R4 sample only.
- `no-panic` is info: the review checked that each finding is a real panic site (all were), and that about one in
  twenty of the Go sites is worth changing; the rest are invariants and recovered hot paths. Rust: none of the 21.
- `no-hidden-target` was wrong on one pattern, a helper that builds an environment the target is passed to; it
  no longer follows helpers that return a value other than an error or a bool, and finds nothing on the four
  repositories.
- R4 refinements and what they did to the counts (strict, cel / qlbridge / minivm / this repository): feature-envy
  17 / 5 / 21 / 15 to 11 / 3 / 7 / 6; misplaced-symbol 15 / 5 / 8 / 12 to 14 / 5 / 8 / 9; no-panic 39 to 20 here
  (the dev-only support crate is test code); standard-assertions 995 / 17 / 3 to 0 / 13 / 0 (cel imports no
  assertion library); exported-doc 971 to 415 in qlbridge and 206 to 154 in minivm (interface implementations
  are exempt); testing/owner 537 to 516 in qlbridge and 139 to 134 in cel (the implementation flag knows every
  interface of the project and of the import closure).
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
