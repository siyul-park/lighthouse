//! The RPC host against fake plugins written as shell scripts: handshake,
//! failure handling, and discovery.
use std::{
    fs,
    path::{Path, PathBuf},
};

use lighthouse_config::Config;
use lighthouse_plugin::{Plugin, Registry};
use lighthouse_rpc::{Error, discover, register, search_dirs};

#[cfg(unix)]
mod process {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::Path,
        time::{Duration, Instant},
    };

    use super::manifest;
    use lighthouse_config::Config;
    use lighthouse_engine::{EXIT_INCOMPLETE, Engine};
    use lighthouse_model::{File, Incomplete};
    use lighthouse_plugin::{Indexed, LanguageProvider, Plugin, Registry, Source, Workspace};
    use lighthouse_rpc::{Error, RpcPlugin, register};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    const SCRIPT: &str = r#"#!/bin/bash
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
    initialize() { send '{"jsonrpc":"2.0","id":1,"result":{"id":"'"${1:-fake}"'","version":"1.2","protocolVersion":"'"${2:-0.1}"'","languages":'"$languages"'}}'; }
    fragment='{"file":{"path":"a.fake","generated":true},"modules":[{"path":"m"}],"symbols":[],"edges":[],"functions":[],"tests":[]}'
    recv
    case "$mode" in
      crash_at_init) echo "dying early" >&2; exit 3;;
      version) initialize fake 9.9;;
      identity) initialize other 0.1;;
      *) initialize;;
    esac
    [ "$mode" = deaf ] && sleep 300
    while recv; do
      case "$body" in
        *'"method":"shutdown"'*) echo shutdown >> "$log"; send '{"jsonrpc":"2.0","id":3,"result":null}';;
        *'"method":"exit"'*) echo exit >> "$log"; exit 0;;
        *'"method":"index"'*)
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

    struct Fake {
        dir: TempDir,
    }

    impl Fake {
        fn new(mode: &str) -> Self {
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

        fn path(&self) -> &Path {
            self.dir.path()
        }

        fn connect(&self, timeout: Duration) -> Result<RpcPlugin, Error> {
            let found = lighthouse_rpc::load(self.path()).unwrap();
            RpcPlugin::connect(&found, self.path(), timeout)
        }

        fn provider(&self, timeout: Duration) -> Box<dyn LanguageProvider> {
            self.connect(timeout).unwrap().languages().remove(0)
        }

        fn log(&self) -> String {
            fs::read_to_string(self.path().join("log")).unwrap_or_default()
        }
    }

    fn file(path: &str) -> File {
        File {
            path: path.into(),
            lang: "fake".to_owned(),
            hash: "abc".to_owned(),
            generated: false,
            test: false,
        }
    }

    fn index(provider: &dyn LanguageProvider, ws: &Workspace, paths: &[&str]) -> Indexed {
        let files: Vec<File> = paths.iter().map(|p| file(p)).collect();
        let sources: Vec<Source> = files.iter().map(|file| Source { file, text: "" }).collect();
        provider.index(ws, &sources).unwrap()
    }

    const T: Duration = Duration::from_secs(20);

    fn only_gap(indexed: &Indexed) -> &Incomplete {
        assert_eq!(indexed.incomplete.len(), 1, "{:?}", indexed.incomplete);
        &indexed.incomplete[0]
    }

    #[test]
    fn a_manifest_that_provides_the_languages_the_process_reports_is_accepted() {
        let fake = Fake::new("ok");
        let manifest = fake.path().join("lighthouse-plugin.toml");
        let text = fs::read_to_string(&manifest).unwrap();
        fs::write(
            &manifest,
            format!("{text}[spec.provides]\nlanguages = [\"fake\"]\n"),
        )
        .unwrap();

        assert!(fake.connect(T).is_ok());
    }

    #[test]
    fn a_manifest_that_provides_other_languages_than_the_process_reports_is_refused() {
        let fake = Fake::new("ok");
        let manifest = fake.path().join("lighthouse-plugin.toml");
        let text = fs::read_to_string(&manifest).unwrap();
        fs::write(
            &manifest,
            format!("{text}[spec.provides]\nlanguages = [\"other\"]\n"),
        )
        .unwrap();

        let error = fake.connect(T).err().unwrap();

        assert!(
            error
                .to_string()
                .contains("provides languages [other], the process reports [fake]"),
            "{error}"
        );
    }

    #[test]
    fn handshake_declares_languages_and_index_converts_the_result() {
        let fake = Fake::new("ok");
        let plugin = fake.connect(T).unwrap();
        let manifest = plugin.manifest();
        assert_eq!(
            (manifest.id.as_str(), manifest.version.as_str()),
            ("fake", "1.2")
        );
        let provider = plugin.languages().remove(0);
        assert_eq!(provider.manifest().id, "fake");
        assert_eq!(provider.manifest().globs, ["**/*.fake"]);
        assert_eq!(
            provider.manifest().capabilities,
            [lighthouse_model::Capability::SemanticEdges],
            "unknown capabilities are ignored"
        );
        let mut ws = Workspace::new(fake.path());
        ws.languages.insert(
            "fake".to_owned(),
            serde_json::from_value(json!({ "tags": ["x"] })).unwrap(),
        );
        let indexed = index(provider.as_ref(), &ws, &["a.fake", "b.fake"]);
        assert_eq!(indexed.fragments.len(), 1);
        let kept = &indexed.fragments[0].files[0];
        assert_eq!(kept.path, Path::new("a.fake"));
        assert!(kept.generated);
        assert_eq!(kept.hash, "abc", "the engine's file data is kept");
        assert_eq!(indexed.fragments[0].modules[0].path, "m");
        assert_eq!(indexed.notices, ["from plugin"]);
        assert!(indexed.incomplete.iter().any(|i| i.reason == "half"));
        assert!(
            indexed
                .incomplete
                .iter()
                .any(|i| i.path.as_deref() == Some(Path::new("b.fake"))
                    && i.reason.contains("no usable fragment"))
        );

        let request: Value =
            serde_json::from_str(&fs::read_to_string(fake.path().join("log.request")).unwrap())
                .unwrap();
        let params = &request["params"];
        assert_eq!(params["language"], "fake");
        assert_eq!(
            params["project"]["root"],
            fake.path().to_string_lossy().as_ref()
        );
        assert_eq!(
            params["files"][0],
            json!({ "path": "a.fake", "hash": "abc" })
        );
        assert_eq!(params["context"]["options"]["fake"]["tags"][0], "x");
        assert!(params["context"].get("overlays").is_none());
    }

    #[test]
    fn dropping_the_plugin_sends_shutdown_then_exit() {
        let fake = Fake::new("ok");
        drop(fake.provider(T));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !fake.log().contains("exit") && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fake.log(), "shutdown\nexit\n");
    }

    #[test]
    fn handshake_refuses_other_protocol_versions_and_identities() {
        let message = |mode: &str| match Fake::new(mode).connect(T) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("{mode} connected"),
        };
        assert!(message("version").contains("speaks protocol 9.9, lighthouse speaks 0.1"));
        assert!(message("identity").contains("identifies itself as `other`"));
        let early = message("crash_at_init");
        assert!(early.contains("plugin `fake`"), "{early}");
    }

    #[test]
    fn a_crash_is_incomplete_and_stderr_becomes_notices() {
        let fake = Fake::new("crash");
        let provider = fake.provider(T);
        let ws = Workspace::new(fake.path());
        let indexed = index(provider.as_ref(), &ws, &["a.fake", "b.fake"]);
        let gap = only_gap(&indexed);
        assert_eq!(gap.path, None);
        assert!(gap.reason.contains("plugin `fake`"), "{}", gap.reason);
        assert!(
            gap.reason.contains("2 file(s) not analyzed"),
            "{}",
            gap.reason
        );
        assert!(gap.reason.contains("exit status: 7"), "{}", gap.reason);
        assert!(indexed.fragments.is_empty());
        assert_eq!(
            indexed.notices,
            [
                "plugin `fake` stderr: boom",
                "plugin `fake` stderr: second line"
            ]
        );
        let again = index(provider.as_ref(), &ws, &["a.fake"]);
        assert!(only_gap(&again).reason.contains("exit status: 7"));
    }

    #[test]
    fn a_timeout_kills_the_plugin_and_later_calls_fail_fast() {
        let fake = Fake::new("sleep");
        let provider = fake.provider(Duration::from_secs(3));
        let ws = Workspace::new(fake.path());
        let started = Instant::now();
        let indexed = index(provider.as_ref(), &ws, &["a.fake"]);
        assert!(only_gap(&indexed).reason.contains("timed out after 3s"));
        assert!(started.elapsed() < Duration::from_secs(10));
        let started = Instant::now();
        let again = index(provider.as_ref(), &ws, &["a.fake"]);
        assert!(only_gap(&again).reason.contains("timed out after 3s"));
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn malformed_answers_are_incomplete() {
        for (mode, expect) in [
            ("garbage", "malformed"),
            ("badshape", "malformed `index` result"),
            ("badid", "malformed symbol id `nope`"),
        ] {
            let fake = Fake::new(mode);
            let provider = fake.provider(T);
            let indexed = index(provider.as_ref(), &Workspace::new(fake.path()), &["a.fake"]);
            let gap = only_gap(&indexed);
            assert!(gap.reason.contains(expect), "{mode}: {}", gap.reason);
            assert!(indexed.fragments.is_empty());
        }
    }

    #[test]
    fn an_error_response_is_incomplete_but_the_plugin_stays_usable() {
        let fake = Fake::new("reject");
        let provider = fake.provider(T);
        let ws = Workspace::new(fake.path());
        let first = index(provider.as_ref(), &ws, &["a.fake"]);
        assert!(
            only_gap(&first)
                .reason
                .contains("rejected `index` (-32000): no thanks")
        );
        let second = index(provider.as_ref(), &ws, &["a.fake"]);
        assert!(
            only_gap(&second).reason.contains("rejected `index`"),
            "still answering"
        );
    }

    #[test]
    fn fragments_for_unrequested_files_are_ignored_with_a_notice() {
        let fake = Fake::new("unrequested");
        let provider = fake.provider(T);
        let indexed = index(provider.as_ref(), &Workspace::new(fake.path()), &["a.fake"]);
        assert!(indexed.fragments.is_empty());
        assert!(
            indexed
                .notices
                .iter()
                .any(|n| n.contains("zzz.fake") && n.contains("not requested"))
        );
        assert!(
            indexed
                .incomplete
                .iter()
                .any(|i| i.reason.contains("no usable fragment"))
        );
    }

    fn engine_with(fake: &Fake, root: &Path) -> Engine {
        let mut registry = Registry::default();
        registry.register(&fake.connect(T).unwrap()).unwrap();
        Engine::new(
            registry,
            Config::parse_inline("plugins = [\"fake\"]").unwrap(),
            root,
        )
        .unwrap()
    }

    #[test]
    fn the_engine_turns_a_plugin_crash_into_exit_code_3() {
        let fake = Fake::new("crash");
        let project = tempfile::tempdir().unwrap();
        fs::write(project.path().join("a.fake"), "x").unwrap();
        let outcome = engine_with(&fake, project.path()).check(&[], &[]).unwrap();
        assert_eq!(outcome.incomplete.len(), 1);
        assert!(
            outcome.incomplete[0]
                .reason
                .contains("language `fake` failed")
                || outcome.incomplete[0].reason.contains("exit status: 7")
        );
        assert!(outcome.notices.iter().any(|n| n.contains("stderr: boom")));
        assert_eq!(outcome.exit_code(false, false), EXIT_INCOMPLETE);
        assert_eq!(outcome.exit_code(false, true), 0);
    }

    fn ws(fake: &Fake) -> Workspace {
        Workspace::new(fake.path())
    }

    #[test]
    fn fragments_must_be_unique_and_describe_only_their_own_file() {
        let notices_and_gaps = |mode: &str| {
            let fake = Fake::new(mode);
            let provider = fake.provider(T);
            let indexed = index(provider.as_ref(), &ws(&fake), &["a.fake"]);
            (indexed.fragments.len(), indexed.notices, indexed.incomplete)
        };
        let (fragments, notices, gaps) = notices_and_gaps("dupe");
        assert_eq!(fragments, 1);
        assert!(notices.iter().any(|n| n.contains("second fragment")));
        assert_eq!(gaps.len(), 1, "duplicate incomplete entries are merged");
        for mode in ["foreign", "stray"] {
            let (fragments, notices, gaps) = notices_and_gaps(mode);
            assert_eq!(fragments, 0, "{mode}");
            assert!(
                notices.iter().any(|n| n.contains("fragment dropped")),
                "{mode}"
            );
            assert!(
                gaps.iter().any(|g| g.reason.contains("no usable fragment")),
                "{mode}"
            );
        }
    }

    #[test]
    fn a_plugin_that_never_reads_cannot_block_past_the_timeout_or_the_drop() {
        let fake = Fake::new("deaf");
        let timeout = Duration::from_secs(5);
        let provider = fake.provider(timeout);
        let paths: Vec<String> = (0..4000).map(|n| format!("dir/{n:0>90}.fake")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let started = Instant::now();
        let indexed = index(provider.as_ref(), &ws(&fake), &refs);
        let reason = only_gap(&indexed).reason.clone();
        assert!(
            reason.contains("timed out after 5s") && reason.contains("`index`"),
            "{reason}"
        );
        assert!(started.elapsed() < timeout * 3);
        let started = Instant::now();
        drop(provider);
        assert!(started.elapsed() < timeout * 3);
    }

    #[test]
    fn stderr_and_notifications_are_bounded() {
        let fake = Fake::new("noisy");
        let provider = fake.provider(T);
        let indexed = index(provider.as_ref(), &ws(&fake), &["a.fake"]);
        assert!(indexed.notices.len() <= 51, "{}", indexed.notices.len());
        assert!(indexed.notices.iter().all(|n| n.len() < 2300));
        assert!(
            indexed
                .notices
                .iter()
                .any(|n| n.contains("earlier line(s) dropped"))
        );
        assert!(indexed.notices.iter().any(|n| n.ends_with(" ...")));
    }

    #[test]
    fn a_timeout_kills_the_whole_process_group() {
        let fake = Fake::new("grandchild");
        // The fake answers initialize at once and stalls only on index, but the
        // one timeout covers both: keep it far above a loaded machine's
        // process start-up so that only the stalled index can exceed it.
        let provider = fake.provider(Duration::from_secs(6));
        let indexed = index(provider.as_ref(), &ws(&fake), &["a.fake"]);
        assert!(only_gap(&indexed).reason.contains("timed out"));
        let pid = fs::read_to_string(fake.path().join("log.pid")).unwrap();
        let alive = || {
            std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        while alive() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(), "the grandchild survived");
    }

    #[test]
    fn a_plugin_that_dies_during_initialize_is_incomplete_not_a_config_error() {
        let fake = Fake::new("crash_at_init");
        let toml = format!(
            "[{{ id = \"fake\", path = {:?} }}]",
            fake.path().to_str().unwrap()
        );
        let mut registry = Registry::default();
        let registered =
            register(&mut registry, &config(&toml), Path::new("."), &[]).expect("not an error");
        assert_eq!(registered.incomplete.len(), 1);
        assert!(registered.incomplete[0].reason.contains("unavailable"));
        assert!(registry.has_plugin("fake"));
        assert_eq!(registry.languages().count(), 0);
    }

    #[test]
    fn an_explicit_path_disambiguates_and_relative_commands_resolve_next_to_the_manifest() {
        let search = tempfile::tempdir().unwrap();
        manifest(&search.path().join("a"), "fake", "/does/not/exist");
        manifest(&search.path().join("b"), "fake", "/does/not/exist");
        let fake = Fake::new("ok");
        let project = tempfile::tempdir().unwrap();
        let toml = format!(
            "[{{ id = \"fake\", path = {:?}, timeout = \"30s\" }}]",
            fake.path().to_str().unwrap()
        );
        let mut registry = Registry::default();
        register(
            &mut registry,
            &config(&toml),
            project.path(),
            &[search.path().to_owned()],
        )
        .unwrap();
        assert!(registry.has_plugin("fake"));
        assert_eq!(registry.languages().count(), 1);
    }

    fn config(plugins: &str) -> Config {
        Config::parse_inline(&format!("plugins = {plugins}")).unwrap()
    }
}

// Discovery.

/// A `Plugin` document in TOML; `runtime` adds lines to the `[spec.runtime]` table.
fn plugin_document(id: &str, command: &str, runtime: &str) -> String {
    format!(
        "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Plugin\"\n[metadata]\nname = \"{id}\"\n[spec]\nversion = \"1\"\n[spec.runtime]\ncommand = \"{command}\"\n{runtime}"
    )
}

fn manifest(dir: &Path, id: &str, command: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("lighthouse-plugin.toml"),
        plugin_document(id, command, ""),
    )
    .unwrap();
}

fn config(plugins: &str) -> Config {
    Config::parse_inline(&format!("plugins = {plugins}")).unwrap()
}

fn paths(found: &[lighthouse_rpc::Found]) -> Vec<(String, PathBuf)> {
    found
        .iter()
        .map(|f| (f.manifest.id.clone(), f.dir.clone()))
        .collect()
}
#[test]
fn discovery_reads_manifests_under_each_search_directory_in_order() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    manifest(&a.path().join("two"), "two", "x");
    manifest(&a.path().join("one"), "one", "x");
    fs::create_dir_all(a.path().join("no-manifest")).unwrap();
    manifest(&b.path().join("three"), "three", "x");
    let search = [
        a.path().to_owned(),
        b.path().to_owned(),
        a.path().join("missing"),
    ];
    let found = discover(&search).unwrap().found;
    assert_eq!(
        paths(&found),
        [
            ("one".to_owned(), a.path().join("one")),
            ("two".to_owned(), a.path().join("two")),
            ("three".to_owned(), b.path().join("three")),
        ]
    );
}

#[test]
fn a_broken_manifest_only_matters_to_the_plugin_it_names() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("bad")).unwrap();
    fs::write(dir.path().join("bad/lighthouse-plugin.toml"), "id = 1").unwrap();
    let search = [dir.path().to_owned()];
    let discovered = discover(&search).unwrap();
    assert!(discovered.found.is_empty());
    assert!(
        discovered.broken[0]
            .to_string()
            .contains("bad/lighthouse-plugin.toml")
    );

    let mut registry = Registry::default();
    let registered = register(
        &mut registry,
        &config("[\"other\"]"),
        Path::new("."),
        &search,
    )
    .expect("an unlisted broken manifest is ignored");
    assert!(registered.notices[0].contains("ignored plugin manifest"));
    let err = register(&mut registry, &config("[\"bad\"]"), Path::new("."), &search).unwrap_err();
    assert!(matches!(err, Error::Manifest { .. }), "{err}");

    fs::write(
        dir.path().join("bad/lighthouse-plugin.toml"),
        plugin_document("x", "c", "extra = 1\n"),
    )
    .unwrap();
    assert_eq!(discover(&search).unwrap().broken.len(), 1);
}

#[test]
fn the_same_id_in_two_places_is_an_error_and_nothing_is_started() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    manifest(&a.path().join("lang"), "lang-x", "/does/not/exist");
    manifest(&b.path().join("lang"), "lang-x", "/does/not/exist");
    let mut registry = Registry::default();
    let err = register(
        &mut registry,
        &config("[\"lang-x\"]"),
        Path::new("."),
        &[a.path().to_owned(), b.path().to_owned()],
    )
    .unwrap_err();
    let Error::Conflict { id, places } = &err else {
        panic!("{err}");
    };
    assert_eq!(id, "lang-x");
    assert_eq!(places.len(), 2);
    assert!(err.to_string().contains("provided more than once"));
}

#[test]
fn an_id_that_is_also_built_in_is_an_error() {
    let a = tempfile::tempdir().unwrap();
    manifest(&a.path().join("core"), "core", "/does/not/exist");
    let mut registry = lighthouse_builtin_stub();
    let err = register(
        &mut registry,
        &config("[\"core\"]"),
        Path::new("."),
        &[a.path().to_owned()],
    )
    .unwrap_err();
    let Error::Conflict { places, .. } = err else {
        panic!("not a conflict");
    };
    assert_eq!(places[0], "built in");
}

/// A registry that already holds an in-process plugin called `core`.
fn lighthouse_builtin_stub() -> Registry {
    struct Core(lighthouse_plugin::PluginManifest);
    impl Plugin for Core {
        fn manifest(&self) -> &lighthouse_plugin::PluginManifest {
            &self.0
        }
    }
    let mut registry = Registry::default();
    registry
        .register(&Core(lighthouse_plugin::PluginManifest {
            id: "core".to_owned(),
            version: "0".to_owned(),
        }))
        .unwrap();
    registry
}

#[test]
fn explicit_paths_must_hold_the_listed_id_and_a_runnable_command() {
    let mismatch = tempfile::tempdir().unwrap();
    manifest(mismatch.path(), "other", "x");
    let toml = format!(
        "[{{ id = \"fake\", path = {:?} }}]",
        mismatch.path().to_str().unwrap()
    );
    let mut registry = Registry::default();
    let err = register(&mut registry, &config(&toml), Path::new("."), &[]).unwrap_err();
    assert!(matches!(err, Error::IdMismatch { .. }), "{err}");

    let missing = tempfile::tempdir().unwrap();
    manifest(missing.path(), "fake", "./nothing-here");
    let toml = format!(
        "[{{ id = \"fake\", path = {:?} }}]",
        missing.path().to_str().unwrap()
    );
    let err = register(&mut registry, &config(&toml), Path::new("."), &[]).unwrap_err();
    assert!(err.to_string().contains("cannot start"), "{err}");

    let err = register(
        &mut registry,
        &config("[{ id = \"fake\", path = \"no/such/dir\" }]"),
        Path::new("."),
        &[],
    )
    .unwrap_err();
    assert!(matches!(err, Error::Io { .. }), "{err}");
}

#[test]
fn only_listed_plugins_start_and_unknown_ids_are_left_to_the_engine() {
    let search = tempfile::tempdir().unwrap();
    manifest(
        &search.path().join("unlisted"),
        "unlisted",
        "/does/not/exist",
    );
    let mut registry = Registry::default();
    register(
        &mut registry,
        &config("[\"nowhere\"]"),
        Path::new("."),
        &[search.path().to_owned()],
    )
    .unwrap();
    assert!(!registry.has_plugin("nowhere") && !registry.has_plugin("unlisted"));
}

#[test]
fn search_directories_cover_the_project_home_and_the_executable() {
    let dirs = search_dirs(Path::new("/work/project"));
    assert_eq!(dirs[0], Path::new("/work/project/.lighthouse/plugins"));
    assert!(
        dirs.iter()
            .any(|d| d.ends_with(".lighthouse/plugins") && d != &dirs[0])
    );
    assert!(dirs.last().unwrap().ends_with("plugins"));
}

#[test]
fn plugin_manifest_reads_command_and_defaults_args_and_rejects_unknown_keys() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join(lighthouse_rpc::FILE_NAME),
        plugin_document("x", "./run", "args = [\"-v\"]\n"),
    )
    .unwrap();
    let found = lighthouse_rpc::load(dir.path()).unwrap();
    let manifest: &lighthouse_rpc::PluginManifest = &found.manifest;
    assert_eq!(
        (manifest.id.as_str(), manifest.command.as_str()),
        ("x", "./run")
    );
    assert_eq!(manifest.args, ["-v"]);

    fs::write(
        dir.path().join(lighthouse_rpc::FILE_NAME),
        plugin_document("x", "c", ""),
    )
    .unwrap();
    assert!(
        lighthouse_rpc::load(dir.path())
            .unwrap()
            .manifest
            .args
            .is_empty()
    );
    fs::write(
        dir.path().join(lighthouse_rpc::FILE_NAME),
        plugin_document("x", "c", "extra = 1\n"),
    )
    .unwrap();
    assert!(matches!(
        lighthouse_rpc::load(dir.path()),
        Err(Error::Manifest { .. })
    ));
}

#[test]
fn a_manifest_is_a_document_in_toml_yaml_or_json() {
    let toml = plugin_document("x", "./run", "args = [\"-v\"]\n");
    let yaml = "apiVersion: lighthouse/v1alpha1\nkind: Plugin\nmetadata: {name: x}\nspec:\n  version: '1'\n  runtime: {command: ./run, args: ['-v']}\n  provides: {languages: [go], orderKeys: [x/group]}\n";
    let json = r#"{"apiVersion":"lighthouse/v1alpha1","kind":"Plugin","metadata":{"name":"x"},"spec":{"version":"1","runtime":{"command":"./run","args":["-v"]}}}"#;
    for (file, text) in [
        ("lighthouse-plugin.toml", toml.as_str()),
        ("lighthouse-plugin.yaml", yaml),
        ("lighthouse-plugin.json", json),
    ] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(file), text).unwrap();

        let found = lighthouse_rpc::load(dir.path()).unwrap();

        assert_eq!(found.manifest.id, "x", "{file}");
        assert_eq!(found.manifest.command, "./run", "{file}");
        assert_eq!(found.manifest.args, ["-v"], "{file}");
    }
}

#[test]
fn a_manifest_from_before_the_resource_model_migrates() {
    let old: serde_json::Value =
        serde_json::from_str(r#"{"id":"x","version":"1","command":"./run","args":["-v"]}"#)
            .unwrap();

    let document = lighthouse_rpc::migrate(&old).unwrap();

    let parsed =
        lighthouse_rpc::parse(lighthouse_config::Format::Json, "x", &document.to_string()).unwrap();
    assert_eq!(
        (parsed.id.as_str(), parsed.command.as_str()),
        ("x", "./run")
    );
    assert_eq!(parsed.args, ["-v"]);
    let legacy = lighthouse_rpc::parse(lighthouse_config::Format::Json, "x", &old.to_string());
    assert!(legacy.unwrap_err().contains("lighthouse spec migrate"));
}

#[test]
fn is_legacy_recognizes_an_old_manifest() {
    assert!(lighthouse_rpc::is_legacy(
        &serde_json::json!({ "id": "x", "version": "1", "command": "./run" })
    ));
    assert!(!lighthouse_rpc::is_legacy(
        &serde_json::json!({ "id": "x" })
    ));
}

#[test]
fn the_plugin_kind_has_a_schema_and_provides_nothing_unless_it_says_so() {
    let kinds: Vec<&str> = lighthouse_rpc::descriptors()
        .iter()
        .map(|d| d.kind)
        .collect();
    assert_eq!(kinds, ["Plugin"]);

    let bare = lighthouse_rpc::parse(
        lighthouse_config::Format::Toml,
        "x",
        &plugin_document("x", "./run", ""),
    )
    .unwrap();
    assert_eq!(bare.provides, lighthouse_rpc::Provides::default());

    let yaml = "apiVersion: lighthouse/v1alpha1\nkind: Plugin\nmetadata: {name: x}\nspec:\n  version: '1'\n  runtime: {command: ./run}\n  provides: {languages: [go], decisions: [decisions], orderKeys: [x/group], embedders: [x/e], fixOps: [x/op]}\n";
    let rich = lighthouse_rpc::parse(lighthouse_config::Format::Yaml, "x", yaml).unwrap();
    assert_eq!(rich.provides.languages, ["go"]);
    assert_eq!(rich.provides.order_keys, ["x/group"]);
    assert_eq!(rich.provides.fix_ops, ["x/op"]);
}

#[test]
fn a_manifest_is_found_in_a_directory_by_any_of_its_names() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(lighthouse_rpc::file_in(dir.path()), None);

    fs::write(dir.path().join("lighthouse-plugin.json"), "{}").unwrap();
    assert_eq!(
        lighthouse_rpc::file_in(dir.path()),
        Some(dir.path().join("lighthouse-plugin.json"))
    );
    fs::write(dir.path().join(lighthouse_rpc::FILE_NAME), "").unwrap();
    assert_eq!(
        lighthouse_rpc::file_in(dir.path()),
        Some(dir.path().join(lighthouse_rpc::FILE_NAME))
    );
}

#[test]
fn a_parsed_plugin_document_becomes_a_manifest() {
    let document = lighthouse_config::Resource::new(
        lighthouse_config::Metadata::named("x"),
        lighthouse_rpc::PluginSpec {
            version: "2".to_owned(),
            runtime: lighthouse_rpc::Runtime {
                command: "run".to_owned(),
                args: vec!["-v".to_owned()],
            },
            provides: lighthouse_rpc::Provides::default(),
        },
    );

    let manifest = lighthouse_rpc::PluginManifest::from_resource(document);

    assert_eq!(
        (
            manifest.id.as_str(),
            manifest.version.as_str(),
            manifest.command.as_str()
        ),
        ("x", "2", "run")
    );
    assert_eq!(manifest.args, ["-v"]);
}
