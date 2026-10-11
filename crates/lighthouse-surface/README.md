# lighthouse-surface

The commands an agent also uses, defined once for both frontends. The clap tree
(`cli`), the argument structs that are both clap flags and MCP tool arguments
(`args`), the operations behind them and the JSON they print (`kinds`) live
here; `lighthouse-cli` parses and prints, `lighthouse-mcp` wraps the same
operations as tools. Operations return a `Reply` (stdout, JSON value, notes for
stderr, exit code) and never print.
