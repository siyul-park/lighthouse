# lang-cel

The bundled CEL plugin, a library crate with a manifest (`lighthouse-plugin.toml`) and no `runtime`: it is compiled into the binary and the composition root registers its libraries (`lighthouse-session`). Its functions are domain-free: node fields stay in the fact builder. Never depends on spec or on checks.

The bundled CEL plugin: the CEL functions Lighthouse implements beyond the `cel` crate, and the language provider that indexes the CEL expressions embedded in YAML and TOML files (the decisions of a catalog).

**Depends on:** `lighthouse-model`, `lighthouse-plugin`.

## Entry points

- `libraries/`: one library per module (`bind`, `glob`, `lists`, `order`, `regex`); library ids are `lang-cel/<name>` and each manifest lists the functions it registers.
- `language/`: the `cel` language provider; it indexes selected regions (never files) into symbols, call and reference edges, and function summaries at the coordinates of the host file.
- `language/analysis/`: `metrics` (size and shape), `names` (the functions an expression can call, as symbols), `spans` (where a call or field lies) and the edge walk.

## Invariants

- A library registers exactly the functions its manifest lists; the registry unions the manifests into the function catalog that load-time checks and the generated docs read.
- The parser's offsets are byte offsets with the last byte of a token as the stop, so the end of a site is `stop + 1`; the tests of `analysis` pin the sites of an identifier, a call and a field.
- A decision's document is `Kind:name`; a decision resolves the shared variables of the pack named by the part of its name before the last `/`.
- Capabilities: semantic edges, extent, reference sites, complete references, overlays.

## Tests

`cargo test -p lang-cel`. The libraries against their upstream documentation: `plugins/lang-cel/tests/it`.
