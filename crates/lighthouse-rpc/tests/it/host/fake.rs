//! A plugin that is a shell script: the fixture the host tests connect to.

use std::{fs, os::unix::fs::PermissionsExt, path::Path, time::Duration};

use lighthouse_engine::Engine;
use lighthouse_model::{Incomplete, ucm::File};
use lighthouse_plugin::{Indexed, LanguageProvider, Plugin, Registry, Source, Workspace};
use lighthouse_rpc::{Error, RpcPlugin};
use lighthouse_spec::Config;
use tempfile::TempDir;

pub(super) const SCRIPT: &str = r#"#!/bin/bash
    mode="$1"
    log="$2"
    send() { printf 'Content-Length: %d\r\n\r\n%s' "${#1}" "$1"; }
    recv() {
      n=0
      while IFS= read -r line; do
        line="${line%$'\r'}"
        [ -z "$line" ] && break
        case "$line" in Content-Length:*) n="${line#*: }";; esac
      done
      [ "$n" -gt 0 ] || exit 0
      body=$(dd bs=1 count="$n" 2>/dev/null)
    }
    languages='[{"id":"fake","globs":["**/*.fake"],"capabilities":["semantic-edges","future-thing"]}]'
    initialize() { send '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"'"${2:-0.3}"'","serverInfo":{"name":"'"${1:-fake}"'","version":"1.2"},"capabilities":{"languages":'"$languages"'}}}'; }
    fragment='{"file":{"path":"a.fake","generated":true},"modules":[{"path":"m"}],"symbols":[],"edges":[],"functions":[],"tests":[]}'
    recv
    case "$mode" in
      crash_at_init) echo "dying early" >&2; exit 3;;
      version) initialize fake 9.9;;
      identity) initialize other;;
      *) initialize;;
    esac
    [ "$mode" = deaf ] && sleep 300
    while recv; do
      case "$body" in
        *'"method":"shutdown"'*) echo shutdown >> "$log"; send '{"jsonrpc":"2.0","id":3,"result":null}';;
        *'"method":"exit"'*) echo exit >> "$log"; exit 0;;
        *'"method":"lighthouse/index"'*)
          printf '%s' "$body" > "$log.request"
          id=$(printf '%s' "$body" | sed 's/.*"id":\([0-9]*\).*/\1/')
          case "$mode" in
            ok) send '{"jsonrpc":"2.0","id":'"$id"',"result":{"fragments":['"$fragment"'],"notices":["from plugin"],"incomplete":[{"path":"a.fake","reason":"half"}]}}';;
            noisy)
              for i in $(seq 1 500); do echo "line $i" >&2; done
              head -c 100000 /dev/zero | tr '\0' 'x' >&2; echo >&2
              for i in $(seq 1 2000); do send '{"jsonrpc":"2.0","method":"note","params":{}}'; done
              send '{"jsonrpc":"2.0","id":'"$id"',"result":{"fragments":[]}}';;
            dupe) send '{"jsonrpc":"2.0","id":'"$id"',"result":{"fragments":['"$fragment"','"$fragment"'],"incomplete":[{"reason":"x"},{"reason":"x"}]}}';;
        foreign) send '{"jsonrpc":"2.0","id":'"$id"',"result":{"fragments":[{"file":{"path":"a.fake"},"symbols":[{"id":"m::x#function","kind":"function","visibility":"public","file":"b.fake","span":{"start":{"line":1,"col":1},"end":{"line":1,"col":2}},"name":"x"}]}]}}';;
        stray) send '{"jsonrpc":"2.0","id":'"$id"',"result":{"fragments":[{"file":{"path":"a.fake"},"modules":[{"path":"m"}],"edges":[{"kind":"calls","from":{"symbol":"m::y#function"},"to":"m::z","resolution":"semantic"}]}]}}';;
        deaf) sleep 300;;
            grandchild) sleep 300 & echo $! > "$log.pid"; sleep 300;;
            crash) echo "boom" >&2; echo "second line" >&2; exit 7;;
            sleep) sleep 30;;
            garbage) printf 'Content-Length: 4\r\n\r\nnope';;
            badshape) send '{"jsonrpc":"2.0","id":'"$id"',"result":{"fragments":"x"}}';;
            badid) send '{"jsonrpc":"2.0","id":'"$id"',"result":{"fragments":[{"file":{"path":"a.fake"},"symbols":[{"id":"nope","kind":"function","visibility":"public","file":"a.fake","span":{"start":{"line":1,"col":1},"end":{"line":1,"col":2}},"name":"x"}]}]}}';;
            reject) send '{"jsonrpc":"2.0","id":'"$id"',"error":{"code":-32000,"message":"no thanks"}}';;
            unrequested) send '{"jsonrpc":"2.0","id":'"$id"',"result":{"fragments":[{"file":{"path":"zzz.fake"}}]}}';;
            *) send '{"jsonrpc":"2.0","id":'"$id"',"result":{"fragments":[]}}';;
          esac;;
      esac
    done
    "#;

pub(super) struct Fake {
    dir: TempDir,
}

impl Fake {
    pub(super) fn new(mode: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("plugin.sh");
        fs::write(&script, SCRIPT).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let log = dir.path().join("log");
        fs::write(
            dir.path().join("lighthouse-plugin.toml"),
            super::plugin_document(
                "fake",
                "./plugin.sh",
                &format!("args = [\"{mode}\", \"{}\"]\n", log.display()),
            ),
        )
        .unwrap();
        Self { dir }
    }

    pub(super) fn path(&self) -> &Path {
        self.dir.path()
    }

    pub(super) fn connect(&self, timeout: Duration) -> Result<RpcPlugin, Error> {
        let found = lighthouse_rpc::load(self.path()).unwrap();
        RpcPlugin::connect(&found, self.path(), timeout)
    }

    pub(super) fn provider(&self, timeout: Duration) -> Box<dyn LanguageProvider> {
        self.connect(timeout).unwrap().languages().remove(0)
    }

    pub(super) fn log(&self) -> String {
        fs::read_to_string(self.path().join("log")).unwrap_or_default()
    }
}

pub(super) fn file(path: &str) -> File {
    File {
        path: path.into(),
        lang: "fake".to_owned(),
        hash: "abc".to_owned(),
        generated: false,
        test: false,
    }
}

pub(super) fn index(provider: &dyn LanguageProvider, ws: &Workspace, paths: &[&str]) -> Indexed {
    let files: Vec<File> = paths.iter().map(|p| file(p)).collect();
    let sources: Vec<Source> = files.iter().map(|file| Source { file, text: "" }).collect();
    provider.index(ws, &sources).unwrap()
}

pub(super) const T: Duration = Duration::from_secs(20);

pub(super) fn only_gap(indexed: &Indexed) -> &Incomplete {
    assert_eq!(indexed.incomplete.len(), 1, "{:?}", indexed.incomplete);
    &indexed.incomplete[0]
}

pub(super) fn engine_with(fake: &Fake, root: &Path) -> Engine {
    let mut registry = Registry::default();
    registry.register(&fake.connect(T).unwrap()).unwrap();
    Engine::new(
        registry,
        Config::parse_inline("plugins = [\"fake\"]").unwrap(),
        lighthouse_test_support::bundled(),
        root,
    )
    .unwrap()
}

pub(super) fn ws(fake: &Fake) -> Workspace {
    Workspace::new(fake.path())
}
