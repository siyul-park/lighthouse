# lang-cel

The bundled CEL plugin: the CEL functions Lighthouse implements beyond the `cel` crate, and the language provider that indexes the CEL expressions embedded in YAML and TOML files (the decisions of a catalog).

## Role

**Layer.** A library crate with a manifest (`lighthouse-plugin.toml`) and no `runtime`: it is compiled into the binary and the composition root registers its libraries (`lighthouse-session`). It depends on `lighthouse-model` and `lighthouse-plugin` and never on spec or on checks. Its functions are domain-free: node fields stay in the fact builder. The provider implements `plugin::LanguageProvider`, whose `manifest` and `index` are the two methods of the out-of-process `Provider`, called in process.

## Modules

- `libraries/`: one library per module (`bind`, `glob`, `lists`, `order`, `regex`); library ids are `lang-cel/<name>` and each manifest lists the functions it registers.
- `language/`: the `cel` provider (`source.rs`); it indexes selected regions (never files) at the coordinates of the host file.
- `language/analysis/`: `names` (the functions an expression can call, as symbols), `metrics` (size and shape, the `functions` fact) and `spans` (where a call or field lies, the sites of `edges`).

## Facts and capabilities

| Fact | Source |
| --- | --- |
| `symbols` | the functions an expression can call |
| `edges` | `calls` and `references` with sites |
| `functions` | size and shape of an expression |

Capabilities: `semantic-edges`, `extent`, `reference-sites`, `complete-references`, `overlays`.

## Project model

- A library registers exactly the functions its manifest lists; the registry unions the manifests into the function catalog that load-time checks and the generated docs read.
- The parser's offsets are byte offsets with the last byte of a token as the stop, so the end of a site is `stop + 1`; the tests of `analysis` pin the sites of an identifier, a call and a field.
- A decision's document is `Kind:name`; a decision resolves the shared variables of the pack named by the part of its name before the last `/`.

## Conformance

Not part of the golden suite: the provider runs in process on regions of decision files, not on files of a project. `plugins/lang-cel/tests/it` holds its manifest test and the libraries against their upstream documentation.

## Build

Compiled into the host binary (`cargo build -p lighthouse-cli`); nothing to install.

## Tests

`cargo test -p lang-cel`.
