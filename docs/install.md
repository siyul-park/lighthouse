# Install

Lighthouse is the `lighthouse` executable and its plugins. A release archive holds both: the plugins sit in `plugins/` next to the executable, where discovery finds them, so unpacking the archive is the whole install. Until the official release every build is the prerelease `v0.0.0-<sha12>` of a commit (`lighthouse --version` prints `0.0.0-<sha12>`); pin the one you use. Releases are built for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` and `aarch64-apple-darwin`.

## GitHub Action

```yaml
- uses: actions/setup-node@v5
  with: { node-version: 24 }
- uses: siyul-park/lighthouse@v0.0.0-0123456789ab
  with: { version: 0.0.0-0123456789ab }
- run: lighthouse check
  env:
    LIGHTHOUSE_TRUST: "1"
```

`version` is required and is the tag without its `v`. The action downloads the archive of the runner and `SHA256SUMS` from the release, checks the sum, unpacks to `$RUNNER_TOOL_CACHE/lighthouse/<version>/<target>` and adds that directory to `PATH`. A later run of the same version finds it in the tool cache. It never fetches `latest`.

## Install script

```sh
curl -fsSLO https://github.com/siyul-park/lighthouse/releases/download/v0.0.0-0123456789ab/install.sh
sh install.sh --version v0.0.0-0123456789ab
```

`--version` is required; `--prefix` (default `~/.local`) says where it goes. The script checks the sum of the archive, unpacks it to `<prefix>/lib/lighthouse/<version>/` and links `<prefix>/bin/lighthouse` to the executable there. The plugins stay next to the real executable, and discovery follows the link. Put `<prefix>/bin` on your `PATH`.

`LIGHTHOUSE_DOWNLOAD_URL` replaces `https://github.com/siyul-park/lighthouse/releases/download`; it is for testing the script against a local mirror.

Verify the provenance of an archive with `gh attestation verify <archive> --repo siyul-park/lighthouse`.

## From source

```sh
make plugins cli
```

This builds `target/plugins/` and `target/release/lighthouse`. Running the executable from `target/release/` does not find `target/plugins/`; name the plugins by `path` in `lighthouse.toml`, as this repository does, or copy `target/plugins` to `target/release/plugins`.

## Tools Lighthouse runs

The archive does not bundle the tools a plugin runs. Install the ones of the plugins your project lists:

| Plugin | Needs |
| --- | --- |
| `lang-go` | `go`, on `PATH` |
| `lang-typescript` | node 24 or later |
| `lang-python` | python 3.11 or later |
| fix sandbox, Linux only | bubblewrap (`bwrap`) |

Lighthouse reports a tool it cannot find. CI images provide `go` and python 3.12; node 24 needs `actions/setup-node`.

## CI trust

Lighthouse starts the programs a project names, its plugins among them, so it asks you to trust a project first (`lighthouse trust`). A CI job has no terminal and checks a revision it already runs: set `LIGHTHOUSE_TRUST=1` in the job to trust every project. Do not set it on a machine that checks code you have not read.
