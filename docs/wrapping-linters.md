# Wrapping a linter

A linter that writes SARIF 2.1.0 is wrapped, not rewritten: a `command` check names the program with
`output: sarif`, and Lighthouse turns each result into a finding. No bundled decision wraps a tool, because the
tools are installed by the user; the decision lives in the project (`.lighthouse/decisions`), and the project
must trust commands (`lighthouse trust`; see [architecture](architecture.md#rules)).

```yaml
spec:
  scope: { subject: project }
  severity: error
  check:
    type: command
    argv: [golangci-lint, run, --output.sarif.path=stdout, --show-stats=false, ./...]
    batch: all
    output: sarif
    columns: bytes                    # golangci-lint counts bytes and omits columnKind
    env: { HOME: /home/dev }          # the caches; a command gets no HOME of its own
    exitCodes: { clean: [0], findings: [1] }
    select: { ruleIds: [errcheck, staticcheck], levels: [error, warning] }
```

## Reading

- **Exit codes** are the declared ones. The log is read after a clean code as well as a findings code, for tools
  that always exit 0 (a `sh -c 'cargo clippy --message-format=json | clippy-sarif'` pipeline exits with its last
  command).
- **Incomplete, never clean.** No output at all, output that is not a SARIF log, a log of another version than
  2.1.0, and a findings code over a log with no result each leave the analysis incomplete (exit 3), with the start
  of stderr in the message. Only a log that was read and holds no result is clean.
- **`select`** is a field of the command check, not the CEL `select`. It keeps the results whose `ruleId` matches
  one of `ruleIds` and whose `level` is one of `levels` (a result without a level is a `warning`); both default to
  everything. Globs are those of module paths: `*` is a run of characters inside one segment, `/` and `::`
  separate segments, `**` spans segments. `select` and `columns` are refused without `output: sarif`.
- **Location.** The first location is used. `uri` is resolved against `uriBaseId` and `originalUriBaseIds`
  (following up to four bases), or read as a `file:` URI (`file:///a`, `file://localhost/a`, `file:/a`), or taken
  relative to the project root, which is what a tool that writes plain relative paths means. A `#` or `?` suffix
  is cut, `%XX` escapes are decoded, and `.` and `..` segments are folded. A result without a location is about
  the project.
- **Dropped results.** A result in a file the project does not have is dropped, and the run says so in a notice
  with the count and the first path.
- **Columns** are one-based and converted to byte columns. The unit is, in order: `columns`
  (`utf16CodeUnits | unicodeCodePoints | bytes`), the run's `columnKind`, then UTF-16 code units. SARIF names the
  two kinds in §3.14.27 and requires `columnKind` from a producer that processes text and reports results, but
  gives no default for a log that omits it, so the last step is Lighthouse's choice, and golangci-lint, which
  omits it, needs `columns: bytes`. If a file cannot be read to convert, its results start at column 1 and a
  notice names the first such file.
- **Message** is `message.text`, else the rule's `messageStrings[message.id]` with its `{n}` arguments filled,
  else the rule id. `ruleIndex` is the position in `tool.driver.rules`; a negative value means none.
- **Severity** is the decision's; the tool's `level`, `ruleId`, name and `helpUri` are evidence.
- **Suppressions.** A result with a `suppression` whose `status` is `accepted` (or absent: §3.35.3 gives no
  default) is the tool's own (`//nolint`, an attribute). It is reported as a suppression of its kind (`inSource` or
  `external`) with the tool's justification: hidden like a directive, visible in SARIF output. Tools that leave
  such results out of the log, as golangci-lint does, show nothing.
- **Identity** is the rule, the innermost project symbol that contains the start (else the file) and the message
  with each run of digits replaced by `#`. Names stay, so two findings about different callees in one place are
  different findings. The tool's `partialFingerprints` are not used: most hash the line text, which moves more
  often than a symbol does.

## Verified against the tools' documentation

- golangci-lint v2 writes SARIF with `--output.sarif.path=stdout` (or a file path), v1 with `--out-format sarif`
  (the migration guide: "Previously 'sarif'" is `--output.sarif.path`,
  <https://github.com/golangci/golangci-lint/blob/main/docs/content/docs/product/migration-guide.md>). It exits
  `1` when it found issues (`--issues-exit-code`, default 1; a timeout is 4;
  <https://github.com/golangci/golangci-lint/blob/main/pkg/commands/run.go>). Run with v2.12.2, a result has
  `ruleId` (the linter), `level`, `message.text`, a relative `uri` without `uriBaseId` and `startLine` /
  `startColumn` in bytes, with no `columnKind`; stdout holds only the log. It needs `HOME` or
  `GOLANGCI_LINT_CACHE` and Go's caches, which the cleared environment of a command lacks.
- Clippy has no SARIF output of its own: `cargo clippy --message-format=json | clippy-sarif`
  (<https://github.com/psastras/sarif-rs>).
- SARIF 2.1.0
  (<https://docs.oasis-open.org/sarif/sarif/v2.1.0/errata01/os/sarif-v2.1.0-errata01-os-complete.html>):
  `columnKind` §3.14.27, `originalUriBaseIds` §3.14.14, `uriBaseId` resolution §3.4.4, `suppression` §3.35.
