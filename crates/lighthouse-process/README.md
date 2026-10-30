# lighthouse-process

Running a process: group, timeout, caps and sandbox policy. Layer L0. Never: what it runs.

Runs an external program under a time limit and an output cap, inside an OS sandbox; every caller of the tool that starts a program (a check, a fix, a verification command, a model provider) goes through it and says how it is sandboxed.

**Depends on:** no workspace crate.

## Entry points

- `run`, `run_lines`, `Invocation`, `Ran`: one program, one result, with the environment and the exit-code meaning shared.
- `Sandbox`, `Policy`: the OS-level isolation of a spawned process (Seatbelt, namespaces).
- `copy_project`, `clone_file`: the scratch copies fixes and examples run in.

## Tests

`crates/lighthouse-process/tests/it`; `cargo test -p lighthouse-process`.
