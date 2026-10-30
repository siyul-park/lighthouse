# lang-python

The Python language provider: parses each file with the `ast` of the interpreter that runs it, reads scopes from `symtable`, and speaks the plugin protocol on stdin and stdout.

**Layer.** Out-of-process Python plugin (Python 3.11 or newer, standard library only); depends on no package and no host crate. Built by `make plugins` into `target/plugins/lang-python`. It always runs in isolated mode (`python3 -I`): no user site, no `PYTHON*` environment, no script directory on `sys.path`; only its own package is added, by absolute path. Code of the project is parsed, never imported or executed.

## Entry points

- `sdk/`: the JSON-RPC loop and its framing (`server.py`, `serve(provider)`), and the wire types (`wire.py`, generated from the protocol schema by `scripts/wire.py` and checked in; `make lint` fails when it is stale). Nothing in it is about a language.
- `main.py`, `provider.py`: the entry, and the `Provider` with `manifest()` and `index(params)`; one request is one `Indexing`.
- `index.py`: one parsed file to one `Fragment`.
- `project/`: the files and import roots (`files.py`), parsed files (`model.py`), `symtable` scopes (`scopes.py`) and names resolved to declarations (`resolve.py`).
- Roles, named after the `Fragment` field they fill: `modules.py`, `symbols.py`, `edges.py`, `functions.py` (with `flow.py`), `tests.py` (pytest and unittest facts), `comments.py`, `regions.py`; and `extent.py`, `visibility.py`, `generated.py`.

## Facts and capabilities

| Fact | Source |
| --- | --- |
| `modules` | one per file; a package's `__init__.py` is its directory; `test_of` for test files |
| `symbols` | functions, classes (`interface` for `Protocol`), methods, class and dataclass fields, `Enum` members, assignments, tests; `extent`, `doc` (PEP 257), `role` in test files |
| `edges` | `calls`, `references`, `imports`, `implements` with sites; `semantic` where scopes, imports and annotations settle the name, else `syntactic` |
| `functions` | flow, nesting, statements, tokens, parameters, returns, signature, `forwards_to`, `implementation`, `constructs`, `manual_assertions` |
| `tests` | `test_*` functions, `Test*` classes, `unittest.TestCase` methods; `parametrize` and loops over literal rows are tables |
| `comments`, `generated` | every comment; `# @generated` and `DO NOT EDIT` above the first statement |

Capabilities: `semantic-edges` (partial: annotations, imports and local inference only), `extent`, `reference-sites`, `overlays`.

## Project model

- Import roots: the project root, `src` where the layout says so, the package directories `pyproject.toml` declares statically (`[tool.setuptools]`, `[tool.hatch]`), and the directory above the topmost package of the importing file. Regular packages and modules win over PEP 420 namespace packages.
- Public means: no leading `_` (dunder names are public), or listed in the module's static `__all__` when it has one, or imported and listed in the `__all__` of a package `__init__.py` above it. A public name of a module whose path has an underscore component is `internal`; everything in a test file is `private`.
- A use on a receiver of unknown type (an untyped parameter, `getattr`, a dynamic attribute) is a `syntactic` edge to every project member that bears the name, when at most eight do. Third-party imports are `syntactic` (no `site-packages` lookup); the standard library is `semantic`.
- Syntax newer than the running interpreter, or any syntax error, is an `incomplete` entry (`line:col: message`); the file keeps an empty fragment. `.pyi` files, `testdata` and hidden directories get empty fragments.
- Only the options of the protocol exist: `[languages.python]` has no keys, and any key is an `incomplete` entry. The interpreter is the `python3` on `PATH`.

## Fences of host files

A `python`, `py`, `py3`, `python3` or `rusthon` fence of a Markdown file is a module of its own, analysed as `<dir>/.lighthouse-snippets/<file>.<n>/snippet.py`, that consumes the project from outside through the same import roots. A fence that is not valid Python is a notice. Names the prose defines are `syntactic` edges to the project declarations that bear them, counted in one notice. Spans and comments are mapped to the host through `text_map`.

## Conformance

Goldens: `crates/lighthouse-rpc/tests/conformance/python`. Shared case names: `comments`, `excluded`, `extents`, `flow`, `generated`, `members`, `projects`, `roles`, `signatures`, `testcases`, `visibility`, `syntax`, `unicode`, `references`. Python's own: `annotations`, `asyncs`, `imports`, `models`, `modulebody`, `namespaces`, `scopes`.

Not applicable, with the reason: `aliases` (a Python alias is an assignment, a `var`), `embedded` (no embedding of types), `errors` and `events` (no error-handling or event facts for Python), `generics` (type parameters are not symbols), `layout` and `tags` (Go file layout and build tags), `nomod`, `xtest`, `wider` (Go modules and packages).

## Tests

`python3 -I -m unittest discover -s tests -t .` from this directory (also under 3.13). `ruff format --check .` and `ruff check .` (rules `E`, `F`) lint the source. Protocol in [docs/plugin-protocol.md](../../docs/plugin-protocol.md).
