# lang-markdown

The Markdown host-file provider: parses Markdown files with `comrak` (pinned, GitHub Flavored Markdown: tables, strikethrough, autolinks, task lists, footnotes, and `---` front matter) and returns each document as a JSON tree with the map from every node to its bytes, for the host to select source regions from (the fenced code of a document, for example). It has no code model of its own: a Markdown file is a file for file decisions, and its subjects (sections, links) come with MD2.

## Role

**Layer.** Out-of-process plugin; depends on `lighthouse-protocol` only. It implements `HostParser`, and `HostProvider::new(parser)` is the `Provider` that `language::serve` serves. Built by `make plugins` into `target/plugins/lang-markdown`.

**Pack.** The plugin also provides the decision pack `markdown` (`decisions/markdown/`, named by `spec.catalog` in the manifest). The host reads it without starting the process, so listing `lang-markdown` in `plugins` makes `markdown/recommended` available to `extends`; `make plugins` copies `decisions/` next to the binary.

## Modules

- `main.rs`: the entry, `language::serve(HostProvider::new(Markdown))`.
- `provider.rs`: the parser: the document as `{"kind": "document", "children": [...]}`, a range for every node the parser locates; and the language (`markdown`, globs `**/*.md`, `**/*.markdown`).
- `regions.rs`: where a code block's text lies, line by line through the indentation lists and blockquotes remove; `regions/fence.rs`: the info string of a fence and the provider key it names; `regions/lines.rs`: the lines of the text and the byte offsets of the parser's line and column positions.
- No `symbols` and no `comments`: Markdown declares nothing and keeps no comments.

## Facts and capabilities

The fragment of a host file is the file alone (so file decisions see it) and its comments where the parser keeps them; the tree is the `Source`.

| Fact | Source |
| --- | --- |
| `source` | the document tree and its map, for files asked with `source_ast` |
| `comments` | none |

Capabilities: `source-ast`, `overlays`.

### The tree

Every node is an object with `kind` (comrak's `xml_node_name`: `document`, `block_quote`, `list`, `item`, `code_block`, `html_block`, `paragraph`, `heading`, `thematic_break`, `table`, `table_row`, `table_cell`, `text`, `softbreak`, `linebreak`, `code`, `html_inline`, `emph`, `strong`, `strikethrough`, `link`, `image`, `footnote_definition`, `footnote_reference`, `taskitem`, `frontmatter`) and, when it has any, `children` in document order. Blocks nest, so a selector reaches a block at any depth with the descendant segment: `$..?(@.kind == 'code_block')`.

| Kind | Fields |
| --- | --- |
| `list`, `item` | `ordered`; `start` and `delimiter` (`period`, `paren`) when ordered, else `bullet`; `tight` |
| `heading` | `level`, `setext` |
| `link`, `image` | `url`, `title` |
| `text`, `code`, `html_inline`, `html_block`, `frontmatter` | `text` |
| `table` | `alignments` (`none`, `left`, `center`, `right`) |
| `table_row` | `header` |
| `footnote_definition`, `footnote_reference` | `name` |
| `taskitem` | `checked` |
| `code_block` | `fenced`, `fence` (the marker as written, absent when indented), `closed`, `info` (raw), `lang`, `attributes`, `text` |

A fence's info string is read as rustdoc and mdBook read it: words separated by spaces and commas. `lang` is the first word as written and `attributes` the rest (`rust,ignore` → `rust`, `["ignore"]`; `rust no_run, edition2021` → `["no_run", "edition2021"]`). A fence with no language (or an indented block) has an empty `lang`. The provider knows no other language: which one a name stands for is what the languages declare as their aliases, and a source rule reads `lang` with `language.from`.

### The map

- Each node has a `range`: the bytes from its first to its last, from the parser's line and column positions (columns count bytes).
- A `code_block` has `text_pointer` to its `text` scalar, whose entry has the `range` of the body (from the first byte kept of its first line to the end of its last line) and a `text_map` from the bytes of the text to the bytes of the document. The parser strips the indentation of lists and blockquotes, a fence's offset and an indented block's four columns; each line of the text is matched to its line of the document by its end, so the map holds each line from where the stripping stopped, line breaks included (`\r\n` stays). A fence the document ends without a line break has a zero-width segment at the end of the text for the break the parser adds.
- A body that cannot be matched line by line (a tab the parser expanded) has the whole lines as its `range` and no `text_map`: a destination provider then anchors to the whole body, never to a guessed place.
- An open fence runs to the end of its last line.

## Project model

- A tree deeper than 256 nested blocks makes the file an `incomplete` entry rather than overflowing.
- Selected regions (a fence or scalar whose language is Markdown) are read as documents; one that is not is a notice, one the tree cannot expose is an `incomplete` entry.

## Conformance

There are no golden cases: the shared provider is tested in `crates/lighthouse-protocol/tests/it/host.rs` and the tree and its map in this plugin's tests.

## Build

`make plugins` builds `target/plugins/lang-markdown` (`cargo build --release -p lang-markdown`).

## Tests

`cargo test -p lang-markdown`. Protocol in [docs/plugin-protocol.md](../../docs/plugin-protocol.md).
