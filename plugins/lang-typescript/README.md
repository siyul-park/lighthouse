# lang-typescript

The TypeScript and JavaScript language provider: loads one TypeScript 6.0.3 program per project through the compiler API, reads symbols and edges from its checker, and speaks the plugin protocol on stdin and stdout.

## Role

**Layer.** Out-of-process Node plugin (Node 24 or newer); depends on `typescript` and `vscode-jsonrpc` at exact versions, never on a host crate. It implements `Provider` and is served by `serve` (`src/sdk/server.ts`). The `typescript` of a project's own `node_modules` is never loaded.

## Modules

- `src/sdk/`: the part no language shapes: the JSON-RPC loop and `Provider` (`server.ts`) and the wire types (`wire.ts`, generated from the protocol schema by `npm run wire`, part of every build, not checked in).
- `main.ts`, `provider.ts`: the entry, and the `Provider` with `manifest()` and `index(params)`.
- `project.ts`, `project/` (`paths.ts`, `selfref.ts`): one program per owning `tsconfig.json` or `jsconfig.json` (project references followed), a default program for files in none, the overlay-aware compiler host.
- `index.ts`, `index/` (`unit.ts`, `diagnostics.ts`): groups files and fences by program; the fragment of one file.
- Roles, named after the `Fragment` field they fill: `model.ts` and `symbols.ts`, `edges.ts` (with `edges/forward.ts`), `functions.ts` (with `functions/signature.ts`), `flow.ts`, `tests.ts` (with `tests/assertions.ts`), `comments.ts`, `regions.ts`; and `extent.ts` (with `extent/lines.ts`), `visibility.ts`. `commonjs.ts` is the export syntax they share.

## Facts and capabilities

| Fact | Source |
| --- | --- |
| `modules` | one per file (its path without extension) |
| `symbols` | functions, methods, classes, interfaces, type aliases, enums, fields, variants, variables, tests, and the `<module>` function for statements a module runs on load; `extent`, `doc` (JSDoc), `optional`, `type_ref` of fields |
| `edges` | `calls`, `references`, `imports`, `implements` from the checker with sites; `syntactic` only for an unresolved relative import, a call on an `any` receiver, or a name the prose of a fence defines |
| `functions` | flow, nesting, statements, tokens, signature, `forwards_to`, `implementation`, `constructs`, `manual_assertions` |
| `tests` | `describe`, `it`, `test` and `suite` calls of the test frameworks (`node:test`, Jest, Vitest, Mocha, ...), with table (`each`) or scenario style |
| `comments`, `generated` | every comment; `@generated` and `DO NOT EDIT` above the first statement |

Capabilities: `semantic-edges`, `extent`, `reference-sites`, `overlays`.

## Project model

- One module per file; overloads and merged declarations share one symbol whose primary node is the implementation.
- Exported names are `public`, or `internal` when the nearest `package.json` is `private`; the rest and everything in test files is `private`; `protected` is `internal`.
- Nothing is executed: no script, no build, no network; analysis needs the sources and the `typescript` this plugin ships with.
- Declaration files, `node_modules`, `testdata` and hidden directories get empty fragments. A file with a syntax error is an `incomplete` entry and still has a fragment; type errors are not gaps.
- A change to the wire goes through protocol `VERSION` and the other providers in the same commit.

### Fences of host files

A fence of a Markdown file is a module of its own that consumes the package around the file from outside: a bare import of the nearest `package.json` name resolves to the package entry. It is analyzed as `<dir>/.lighthouse-snippets/<file>.<n>/snippet.<ext>` with the extension from the first word of the info string (`ts`, `tsx`, `js`, `node`, ...; default `.ts`). A fence that is not a module is analyzed with an unmapped `export {};` tail. A fence that is not valid code is a notice; fences that do not type-check are counted in one notice and their edges stay as far as they resolve.

## Conformance

Goldens: `crates/lighthouse-rpc/tests/conformance/typescript`. Shared cases (the same name in every language plugin): `comments`, `errors`, `excluded`, `extents`, `flow`, `generated`, `signatures`, `tests`, `visibility`. Own cases: `commonjs`, `declarations`, `generics`, `heritage`, `imports`, `interop`, `jsdoc`, `jsx`, `members`, `merging`, `namespaces`, `nomodule`, `overloads`, `projects`, `references`, `scripts`, `traced`, `unicode`, `unresolved`.

Not applicable: `events` (the provider reports no body events).

## Build

`make plugins` builds `target/plugins/lang-typescript` (`npm ci --omit=dev --ignore-scripts`).

## Tests

`npm test` (wire types, compiler, then `node --test` on `test/`). Protocol in [docs/plugin-protocol.md](../../docs/plugin-protocol.md).
