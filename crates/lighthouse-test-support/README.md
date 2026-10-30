# lighthouse-test-support

The shared test fixtures and the bundled composition for tests. A dev dependency; never: production use.

Builds the bundled CEL libraries, composition, catalog and plugin registries the way the composition root does, builds the language plugins of this repository from source, and writes the documents tests put in a project.

**Depends on:** `lang-cel`, `lighthouse-checks`, `lighthouse-model`, `lighthouse-plugin`, `lighthouse-spec`.

## Entry points

- `libraries`, `composition`, `cel`, `bundled`, `registry`, `full`, `full_registry`: the bundled composition, shared by the tests of a process.
- `catalog`: decision, pack and catalog-file builders.
- `project`, `listing`, `installed`: project documents and installed plugin directories.
- `lang_go`, `lang_typescript`, `lang_python`, `lang_rust`, `fake_model`, `model_local`, `model_http`: the built plugins.

## Tests

None of its own: it is the fixture the `tests/it` of the other crates use.
