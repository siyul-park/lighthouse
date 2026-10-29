//! The reference of the edit constructors, generated into the docs from the
//! same table that documents the schema: what each takes and means.

use std::fmt::Write;

use super::decision::NOTICE;

/// The constructors an `edits` expression builds its list from.
pub const OPERATIONS: &[Operation] = &[
    Operation {
        name: "remove",
        summary: "Removes a declaration with its extent, or a range of text such as a \
                  comment. A range that fills whole lines takes the lines, and one \
                  blank line when that would leave two. One spec of a parenthesized \
                  `const` group whose values depend on their place (`iota`, or specs \
                  that repeat the one before) keeps its place as the blank identifier \
                  `_`, so that the specs after it keep their values.",
        params: &[Param {
            name: "target",
            ty: "node, symbol id or span",
            required: true,
            description: "A node or its id, or `{start: {line, col}, end: {line, col}}` \
                          (1-based, columns in bytes, end exclusive) with an optional \
                          `file`, else the finding's file.",
        }],
        example: "edits: \"[remove(finding.span)]\"",
    },
    Operation {
        name: "add",
        summary: "Inserts text at an anchor. At `before(node)` and `after(node)` it goes \
                  at the start of the line of the node's extent, or of the line after \
                  it, so the text is whole lines; `start(file)` and `end(file)` are the \
                  two ends of the finding's file.",
        params: &[
            Param {
                name: "anchor",
                ty: "`before(node)`, `after(node)`, `start(file)` or `end(file)`",
                required: true,
                description: "Where the text goes.",
            },
            Param {
                name: "text",
                ty: "string",
                required: true,
                description: "The text, computed with CEL's strings.",
            },
        ],
        example: "edits: \"[add(before(node), '// ' + node.name + ' is documented.\\n')]\"",
    },
    Operation {
        name: "replace",
        summary: "Replaces a range of text, or the extent of a declaration.",
        params: &[
            Param {
                name: "target",
                ty: "node, symbol id or span",
                required: true,
                description: "What is replaced.",
            },
            Param {
                name: "text",
                ty: "string",
                required: true,
                description: "The new text.",
            },
        ],
        example: "edits: \"[replace(finding.span, '// ' + finding.decision)]\"",
    },
    Operation {
        name: "move",
        summary: "Relocates a declaration, with its leading doc comments and attributes, \
                  next to another one. Both must live in the same container (the same \
                  file level, the same impl or class body); the blank-line separation \
                  of the moved block is kept.",
        params: &[
            Param {
                name: "node",
                ty: "node or symbol id",
                required: true,
                description: "The declaration to move.",
            },
            Param {
                name: "anchor",
                ty: "`before(node)` or `after(node)`",
                required: true,
                description: "The declaration it goes next to.",
            },
        ],
        example: "edits: \"[move(node, after(finding.evidence.caller))]\"",
    },
    Operation {
        name: "rename",
        summary: "Renames a symbol at its declaration and at every reference site. \
                  Declined unless the provider reports reference sites and every \
                  reference edge to the symbol is semantic.",
        params: &[
            Param {
                name: "symbol",
                ty: "node or symbol id",
                required: true,
                description: "The symbol to rename.",
            },
            Param {
                name: "name",
                ty: "string",
                required: true,
                description: "The new name.",
            },
        ],
        example: "edits: \"[rename(node, node.name + 'Value')]\"",
    },
    Operation {
        name: "reorder",
        summary: "Puts sibling declarations in the order of the list, in the places \
                  they hold. Nothing changes when they already are in it. Each \
                  container (the file level, an `impl` or class body) is permuted on \
                  its own.",
        params: &[Param {
            name: "nodes",
            ty: "list of nodes or symbol ids",
            required: true,
            description: "The declarations, in the order they must have.",
        }],
        example: "edits: \"[reorder(file.symbols.filter(s, s.kind == 'function').sortBy(s, s.name))]\"",
    },
];

/// How an order is written: in the decisions, as CEL.
const ORDERS: &str = "## Orders\n\n\
     An order is CEL in the decision: its check compares a node with its siblings \
     (`node.prev`, `node.next`, `file.symbols`) by a rank, and its fix puts the nodes \
     in the order of that rank with `reorder` or `move`. Decisions that share an order \
     share the rank. A pack lists CEL `variables` that its decisions use by name, as \
     they use their own: a variable with `params` is a function of them, and one \
     without is a value that is computed once per file when it reads nothing of the \
     selected node. `design/pack.yaml` shares the declaration group this way:\n\n\
     ```yaml\n\
     # design/pack.yaml\n\
     variables:\n  \
     - name: declaration_group\n    \
       params: [n]\n    \
       expression: >-\n      \
         ... the index in options.groups of the first candidate group of n that it names\n\n\
     # design/file-order/declaration-groups.yaml\n\
     check:\n  \
     type: cel\n  \
     select: symbol\n  \
     variables:\n    \
     - name: placed   # once per file\n      \
       expression: file.symbols.filter(s, declaration_group(s) >= 0)\n    \
     - name: displaced\n      \
       expression: placed.map(s, declaration_group(s)).misplaced() ...\n  \
     where: size(displacement) > 0\n\
     fix:\n  \
     type: cel\n  \
     edits: \"[reorder(placed.sortBy(s, declaration_group(s)))]\"\n\
     ```\n\n\
     `design/vertical-distance` reads the same `declaration_group(func)`. A function sees the \
     `options` and the `file` of its caller. `list.misplaced()` gives the \
     indexes of the fewest elements of a list of ints that, moved, put it in order.\n\n";

/// One parameter of an edit constructor.
pub struct Param {
    pub name: &'static str,
    pub ty: &'static str,
    pub required: bool,
    pub description: &'static str,
}

/// One edit constructor.
pub struct Operation {
    pub name: &'static str,
    pub summary: &'static str,
    pub params: &'static [Param],
    pub example: &'static str,
}

/// The page of the fix operations.
pub fn fix_operations_markdown() -> String {
    let mut out = format!(
        "{NOTICE}\n\n# Fix operations\n\n\
         A decision's `fix:` block says how the findings of its rule are fixed. Its \
         `type` is one of `cel`, `command`, `model` and `rpc`; it carries a `safety` \
         cap, and may list the provider capabilities it `requires`:\n\n\
         ```yaml\nfix:\n  safety: safe        # safe | unsafe\n  type: cel           # cel | command | model | rpc\n  edits: \"[...]\"\n```\n\n\
         The `cel` type is one CEL expression, `edits`, that returns a list of edits \
         (an empty list proposes none), as a Kubernetes MutatingAdmissionPolicy returns \
         patches. CEL never mutates: the constructors below build values that state \
         edits over code-model nodes; the orchestrator lowers them to text edits with \
         the spans the language provider reports, formats, re-checks and rolls back \
         what gets worse. The expression sees `finding` (`decision`, `message`, `file`, \
         `span`, `symbol`, `evidence`, `fingerprint`, `facts`), `node` (the finding's \
         subject, with the fields of the code model), `file` (its file, with \
         `symbols` and `comments`), `options` (the options the rule ran with) and the \
         `variables` of the decision's check, so a fix never redefines what its check \
         computed. `requires` follows from the constructors it calls: `move`, \
         `reorder`, `before` and `after` need `extent`, `rename` needs \
         `complete-references`; what `remove` and `replace` need depends on their \
         target, so the fix says it.\n\n"
    );
    for op in OPERATIONS {
        let _ = write!(out, "## `{}`\n\n{}\n\n", op.name, op.summary);
        let _ = writeln!(out, "| param | type | meaning |");
        let _ = writeln!(out, "| --- | --- | --- |");
        for p in op.params {
            let required = if p.required { ", required" } else { "" };
            let _ = writeln!(
                out,
                "| `{}` | {}{required} | {} |",
                p.name, p.ty, p.description
            );
        }
        let _ = write!(out, "\n```yaml\n{}\n```\n\n", op.example);
    }
    out.push_str(ORDERS);
    out.push_str(
        "\n## Commands\n\n\
         The `command` type is a program like the others ([programs](../programs.md)): its \
         shape, environment, exit codes and trust are the same everywhere, and it runs without \
         a shell. Structured data goes through `rpc`, not through commands.\n\n\
         ```yaml\nfix:\n  safety: unsafe\n  type: command\n  command: [\"gofmt\", \"-s\", \"-w\", \"{file}\"]\n  output: files   # files | stdout\n  stdin: none     # none | file\n  env: {}         # extra variables\n  timeout: 30s\n  scope: file\n```\n\n\
         - **Exit codes:** a code in `success` (default `[0]`) means it ran; an empty result \
         changes nothing and is `unchanged`, with what the program said as the reason. Any other \
         code, a signal or a timeout is an error: nothing is applied and the tail of stderr is \
         the reason.\n\
         - **Output:** `files` (default) diffs a scratch copy of the project, a repository with \
         one commit of the base, against the original; `stdout` takes the new text of the file \
         from stdout, which carries only that (`stdin: file`, no copy). stderr is for people \
         and is never parsed.\n\
         - **stdin:** `none` (default) or `file`, the content of the target file.\n\
         - **Command line:** `{file}` (in the scratch copy), `{line}`, `{symbol}` and \
         `{decision}` fill a whole argument each (`--out={file}` is refused, and so is a value \
         that starts with `-`). The program runs with the scratch directory as its working \
         directory.\n\
         - **Environment:** cleared to `PATH`, `USER`, `LOGNAME`, `LANG`, `HOME` and `TMPDIR`, \
         plus the declared `env`, plus `LIGHTHOUSE_DECISION` (the finding as JSON: decision, \
         file, line, symbol, fingerprint, evidence), `LIGHTHOUSE_OPTIONS` (the rule's options as \
         JSON) and `LIGHTHOUSE_API_VERSION`.\n\
         - **Limits:** the timeout sends SIGTERM to the process group, then SIGKILL after a \
         grace period; stdout and stderr are capped.\n\
         - **Trust:** a command of the project runs only in a project the user trusts \
         (`lighthouse trust`); the repository cannot grant trust itself. The programs of the \
         bundled decisions need none.\n\n\
         The `rpc` type, a language plugin's own fix method, is reserved: it loads, and the \
         findings of its rule are declined as not yet supported.\n",
    );
    out
}
