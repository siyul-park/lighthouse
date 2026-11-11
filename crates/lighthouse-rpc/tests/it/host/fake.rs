//! A plugin that is a shell script: the fixture the host tests connect to.

use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::Path, time::Duration};

use lighthouse_engine::Engine;
use lighthouse_model::{
    Incomplete,
    ucm::{File, Fragment},
};
use lighthouse_plugin::{Indexed, LanguageProvider, Plugin, Registry, Source, Unit, Workspace};
use lighthouse_rpc::{Error, RpcPlugin, StartOptions};
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
    initialize() { send '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"'"${2:-0.9}"'","serverInfo":{"name":"'"${1:-fake}"'","version":"1.2"},"capabilities":{"languages":'"$languages"'}}}'; }
    fragment='{"file":{"path":"a.fake"},"modules":[{"path":"m"}],"symbols":[],"edges":[],"functions":[],"tests":[]}'
    unit() { send '{"jsonrpc":"2.0","id":'"$id"',"result":{"units":[{"id":"u","body":"b","api":"a",'"$1"'}]}}'; }
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
          id=$(printf '%s' "$body" | sed 's/^{"jsonrpc":"2.0","id":\([0-9]*\),.*/\1/')
          case "$mode" in
            ok) unit '"fragments":['"$fragment"'],"notices":["from plugin"],"incomplete":[{"path":"a.fake","reason":"half"}]';;
            noisy)
              for i in $(seq 1 500); do echo "line $i" >&2; done
              head -c 100000 /dev/zero | tr '\0' 'x' >&2; echo >&2
              for i in $(seq 1 2000); do send '{"jsonrpc":"2.0","method":"note","params":{}}'; done
              unit '"fragments":[]';;
            dupe) unit '"fragments":['"$fragment"','"$fragment"'],"incomplete":[{"reason":"x"},{"reason":"x"}]';;
        foreign) unit '"fragments":[{"file":{"path":"a.fake"},"symbols":[{"id":"m::x#function","kind":"function","visibility":"public","file":"b.fake","span":{"start":{"line":1,"col":1},"end":{"line":1,"col":2}},"name":"x"}]}]';;
        stray) unit '"fragments":[{"file":{"path":"a.fake"},"modules":[{"path":"m"}],"edges":[{"kind":"calls","from":{"symbol":"m::y#function"},"to":"m::z","resolution":"semantic"}]}]';;
        deaf) sleep 300;;
            grandchild) sleep 300 & echo $! > "$log.pid"; sleep 300;;
            crash) echo "boom" >&2; echo "second line" >&2; exit 7;;
            sleep) sleep 30;;
            garbage) printf 'Content-Length: 4\r\n\r\nnope';;
            badshape) send '{"jsonrpc":"2.0","id":'"$id"',"result":{"units":"x"}}';;
            badid) unit '"fragments":[{"file":{"path":"a.fake"},"symbols":[{"id":"nope","kind":"function","visibility":"public","file":"a.fake","span":{"start":{"line":1,"col":1},"end":{"line":1,"col":2}},"name":"x"}]}]';;
            reject) send '{"jsonrpc":"2.0","id":'"$id"',"error":{"code":-32000,"message":"no thanks"}}';;
            unrequested) unit '"fragments":[{"file":{"path":"zzz.fake"}}]';;
            *) unit '"fragments":[]';;
          esac;;
      esac
    done
    "#;

pub(super) struct Fake {
    logs: TempDir,
    dir: TempDir,
}

impl Fake {
    pub(super) fn new(mode: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("plugin.sh");
        fs::write(&script, SCRIPT).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let logs = tempfile::tempdir().unwrap();
        let log = logs.path().join("log");
        fs::write(
            dir.path().join("lighthouse-plugin.toml"),
            super::plugin_document(
                "fake",
                "./plugin.sh",
                &format!("args = [\"{mode}\", \"{}\"]\n", log.display()),
            ),
        )
        .unwrap();
        Self { dir, logs }
    }

    pub(super) fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Where the script writes its log and the requests it gets: beside the
    /// plugin, so that running it does not change the plugin's directory.
    pub(super) fn logs(&self) -> &Path {
        self.logs.path()
    }

    pub(super) fn connect(&self, timeout: Duration) -> Result<RpcPlugin, Error> {
        let found = lighthouse_rpc::load(self.path()).unwrap();
        RpcPlugin::connect(&found, self.path(), timeout)
    }

    /// Like [`connect`](Self::connect), keeping the answer in `cache`.
    pub(super) fn connect_kept(&self, cache: &Path) -> Result<RpcPlugin, Error> {
        let found = lighthouse_rpc::load(self.path()).unwrap();
        let options = StartOptions {
            cache: Some(cache),
            ..StartOptions::default()
        };
        RpcPlugin::connect_with(&found, self.path(), T, &options)
    }

    pub(super) fn provider(&self, timeout: Duration) -> Box<dyn LanguageProvider> {
        self.connect(timeout).unwrap().languages().remove(0)
    }

    pub(super) fn log(&self) -> String {
        fs::read_to_string(self.logs().join("log")).unwrap_or_default()
    }
}

pub(super) fn file(path: &str) -> File {
    File {
        path: path.into(),
        lang: "fake".to_owned(),
        generated: false,
        test: false,
        facts: Default::default(),
    }
}

/// What a provider answered for one request, its units together.
pub(super) struct Answer {
    pub(super) fragments: Vec<Fragment>,
    pub(super) notices: Vec<String>,
    pub(super) incomplete: Vec<Incomplete>,
}

impl From<Indexed> for Answer {
    fn from(indexed: Indexed) -> Self {
        let mut answer = Self {
            fragments: Vec::new(),
            notices: indexed.notices,
            incomplete: indexed.incomplete,
        };
        for unit in indexed.units {
            answer.fragments.extend(unit.fragments);
            answer.notices.extend(unit.notices);
            answer.incomplete.extend(unit.incomplete);
        }
        answer
    }
}

/// The files at `paths` as one unit of `provider`.
pub(super) fn index(provider: &dyn LanguageProvider, ws: &Workspace, paths: &[&str]) -> Answer {
    let files: Vec<File> = paths.iter().map(|p| file(p)).collect();
    let sources: Vec<Source> = files
        .iter()
        .map(|file| Source {
            file,
            text: "",
            regions: &[],
        })
        .collect();
    let imports = BTreeMap::new();
    let unit = Unit {
        id: "u",
        key: "",
        files: &sources,
        imports: &imports,
    };
    let every: Vec<String> = paths.iter().map(|p| (*p).to_owned()).collect();
    provider.index(ws, &[unit], &every).unwrap().into()
}

pub(super) const T: Duration = Duration::from_secs(20);

pub(super) fn only_gap(indexed: &Answer) -> &Incomplete {
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
