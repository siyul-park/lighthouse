# lang-markdown

The Markdown host-file provider: parses Markdown files with `comrak` (pinned, GitHub Flavored Markdown: tables, strikethrough, autolinks, task lists, footnotes, and `---` front matter) and returns each document as a JSON tree with the map from every node to its bytes, for the host to select source regions from (the fenced code of a document, for example). It has no code model of its own: a Markdown file is a file for file decisions, and its subjects (sections, links) come with MD2.

**Layer.** Out-of-process plugin; depends on `lighthouse-protocol` only, whose `serve_host` is the rest of the provider. Built by `make plugins` into `target/plugins/lang-markdown`.

## Entry points
- `main.rs`: the language (`markdown`, globs `**/*.md`, `**/*.markdown`) and `serve_host`.
- `tree.rs`: the document as `{"kind": "document", "children": [...]}`, a range for every node the parser locates.
- `body.rs`: where a code block's text lies: line by line through the indentation lists and blockquotes remove.
- `fence.rs`: the info string of a fence and the provider key it names.
- `lines.rs`: the lines of the text and the byte offsets of the parser's line and column positions.

## The tree
Every node is an object with `kind` (comrak's `xml_node_name`: `document`, `block_quote`, `list`, `item`, `code_block`, `html_block`, `paragraph`, `heading`, `thematic_break`, `table`, `table_row`, `table_cell`, `text`, `softbreak`, `linebreak`, `code`, `html_inline`, `emph`, `strong`, `strikethrough`, `link`, `image`, `footnote_definition`, `footnote_reference`, `taskitem`, `frontmatter`) and, when it has any, `children` in document order. Blocks nest, so a selector reaches a block at any depth with the descendant segment: `$..?(@.kind == 'code_block')`.

| Kind | Fields |
|---|---|
| `list`, `item` | `ordered`; `start` and `delimiter` (`period`, `paren`) when ordered, else `bullet`; `tight` |
| `heading` | `level`, `setext` |
| `link`, `image` | `url`, `title` |
| `text`, `code`, `html_inline`, `html_block`, `frontmatter` | `text` |
| `table` | `alignments` (`none`, `left`, `center`, `right`) |
| `table_row` | `header` |
| `footnote_definition`, `footnote_reference` | `name` |
| `taskitem` | `checked` |
| `code_block` | `fenced`, `fence` (the marker as written, absent when indented), `closed`, `info` (raw), `lang`, `attributes`, `key`, `text` |

A fence's info string is read as rustdoc and mdBook read it: words separated by spaces and commas. `lang` is the first word as written and `attributes` the rest (`rust,ignore` → `rust`, `["ignore"]`; `rust no_run, edition2021` → `["no_run", "edition2021"]`). `key` is the provider key of the language, in lower case; the labels with a provider of ours:

| Label (any case) | `key` |
|---|---|
| `cel` | `cel` |
| `go`, `golang` | `go` |
| `rust`, `rs` | `rust` |
| `toml` | `toml` |
| `yaml`, `yml` | `yaml` |

Any other label is its own lower case, and a fence with none (or an indented block) has the empty key. Nothing is analyzed because of a key; a source rule selects on it.

## The map
- Each node has a `range`: the bytes from its first to its last, from the parser's line and column positions (columns count bytes).
- A `code_block` has `text_pointer` to its `text` scalar, whose entry has the `range` of the body (from the first byte kept of its first line to the end of its last line) and a `text_map` from the bytes of the text to the bytes of the document. The parser strips the indentation of lists and blockquotes, a fence's offset and an indented block's four columns; each line of the text is matched to its line of the document by its end, so the map holds each line from where the stripping stopped, line breaks included (`\r\n` stays). A fence the document ends without a line break has a zero-width segment at the end of the text for the break the parser adds.
- A body that cannot be matched line by line (a tab the parser expanded) has the whole lines as its `range` and no `text_map`: a destination provider then anchors to the whole body, never to a guessed place.
- An open fence runs to the end of its last line.

## Invariants
- A tree deeper than 256 nested blocks makes the file an `incomplete` entry rather than overflowing.
- Capability `source-ast` only. Selected regions (a Markdown fence whose language is Markdown) are read as documents; one that is not is a notice.

## Tests
`cargo test -p lang-markdown`; the shared provider is tested in `crates/lighthouse-protocol/tests/it/host.rs`. Protocol in [docs/plugin-protocol.md](../../docs/plugin-protocol.md).
