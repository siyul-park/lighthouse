# lang-typescript

The TypeScript and JavaScript language provider: loads one TypeScript 6.0.3 program per project through the compiler API, reads symbols and edges from its checker, and speaks the plugin protocol on stdin and stdout.

**Layer.** Out-of-process Node plugin (Node 24 or newer); depends on `typescript` and `vscode-jsonrpc` at exact versions, never on a host crate. Built by `make plugins` into `target/plugins/lang-typescript` (`npm ci --omit=dev --ignore-scripts`). The `typescript` of a project's own `node_modules` is never loaded.

## Entry points

- `server.ts`: the JSON-RPC loop (`initialize`, `lighthouse/index`, `shutdown`, `exit`); `provider.ts`: the manifest and one `index` request.
- `projects.ts`, `selfref.ts`: one program per owning `tsconfig.json` or `jsconfig.json` (project references followed), a default program for files in none, the overlay-aware compiler host.
- `index.ts`: groups files and fences by program; `unit.ts`: the fragment of one file.
- `symbols.ts`, `model.ts`, `visibility.ts`: declarations, merged declarations and exports, visibility.
- `uses.ts`: edges; `flow.ts`, `forward.ts`, `types.ts`: summaries; `tests.ts`: test cases; `comments.ts`, `extent.ts`.
- `regions.ts`: the TypeScript and JavaScript fences of host files (see below).
- `wire.ts`: generated from the protocol schema by `npm run wire` (part of every build); not checked in.

## Invariants

- One module per file (its path without extension); overloads and merged declarations share one symbol whose primary node is the implementation.
- Edges come from the checker and are `semantic`. A bare-name `syntactic` edge is emitted only for an unresolved relative import, a call on an `any` receiver, or a name the prose defines in a fence, and only when some file of the project declares that name.
- Exported names are `public`, or `internal` when the nearest `package.json` is `private`; the rest and everything in test files is `private`; `protected` is `internal`.
- Nothing is executed: no script, no build, no network; analysis needs the sources and the `typescript` this plugin ships with.
- Declaration files, `node_modules`, `testdata` and hidden directories get empty fragments. A file with a syntax error is an `incomplete` entry and still has a fragment; type errors are not gaps.
- A change to the wire goes through protocol `VERSION` and the other providers in the same commit.

## Fences of host files

A fence of a Markdown file is a module of its own that consumes the package around the file from outside: a bare import of the nearest `package.json` name resolves to the package entry. It is analyzed as `<dir>/.lighthouse-snippets/<file>.<n>/snippet.<ext>` with the extension from the first word of the info string (`ts`, `tsx`, `js`, `node`, ...; default `.ts`). A fence that is not a module is analyzed with an unmapped `export {};` tail. A fence that is not valid code is a notice; fences that do not type-check are counted in one notice and their edges stay as far as they resolve.

## Tests

`npm test` (compiler, then `node --test` on `test/`); goldens: `crates/lighthouse-rpc/tests/conformance/typescript`. Protocol in [docs/plugin-protocol.md](../../docs/plugin-protocol.md).
