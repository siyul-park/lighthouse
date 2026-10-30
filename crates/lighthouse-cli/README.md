# lighthouse-cli

The argv and MCP surfaces, the terminal and hook adapters. Layer L5. Never: logic beyond parsing and printing.

The `lighthouse` binary and the frontends over the session: one table of operations (`surface/`), the terminal (`terminal/`) and the MCP server (`mcp/`).

**Depends on:** `lighthouse-model`, `lighthouse-process`, `lighthouse-protocol`, `lighthouse-report`, `lighthouse-resource`, `lighthouse-session`.

## Entry points

- `surface::run`, `Action`: the command tree and the operations behind it, shared by both frontends.
- `surface::Context`, `Reply`: who is calling, and what an operation returns (text or JSON, notes, exit code).
- `surface::tools`: the table of MCP tools, each naming the CLI command it is the twin of, the type its arguments and the type its result are, and deriving both JSON Schemas (`kinds` lists the result schemas that `schema/` holds).
- `mcp::serve`: `lighthouse mcp`, tools and resources over stdio (rmcp).
- `terminal/` (bin only): clap parsing, printing, `init`, `docs`, `trust`, and the agent hooks (`hook claude-code <event>`).

## Invariants

- Every MCP tool is the twin of a CLI command: same name path, same flags (camelCase in MCP), same JSON as `--format json`. `surface::tools` defines each tool once, with the command it is the twin of.
- Noun-verb commands and one meaning per flag. Operations return a `Reply` and never print; only `terminal/` writes to stdout and stderr.
- Frontends import session, report and L0 vocabulary only; session re-exports the rest.
- Hooks never fix. A judgment recorded over MCP is attributed to a software agent.
- [docs/cli.md](../../docs/cli.md) and [docs/mcp.md](../../docs/mcp.md) are generated from these definitions; `lighthouse docs check` fails when they are stale.

## Tests

`crates/lighthouse-cli/tests/it` (CLI, MCP contract and snapshots); `cargo test -p lighthouse-cli`.
