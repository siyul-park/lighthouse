//! Test support: builds the bundled language plugins from source.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

static LANG_GO: OnceLock<Option<PathBuf>> = OnceLock::new();

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Directory of the `lang-go` plugin (manifest and binary, the layout
/// `make plugins` produces), built once per process. `None` after printing a
/// skip message when no Go toolchain runs in `plugins/lang-go`, unless `CI` or
/// `LIGHTHOUSE_REQUIRE_GO` is set, where a missing toolchain panics; a failing
/// build panics.
pub fn lang_go() -> Option<PathBuf> {
    LANG_GO.get_or_init(build_lang_go).clone()
}

fn build_lang_go() -> Option<PathBuf> {
    let source = workspace().join("plugins/lang-go");
    let version = Command::new("go")
        .arg("version")
        .current_dir(&source)
        .output();
    if !version.is_ok_and(|o| o.status.success()) {
        assert!(
            env::var_os("CI").is_none() && env::var_os("LIGHTHOUSE_REQUIRE_GO").is_none(),
            "no Go toolchain runs in plugins/lang-go (see .go-version) and Go is required here"
        );
        eprintln!("skipping: no Go toolchain runs in plugins/lang-go (see .go-version)");
        return None;
    }
    let out = workspace().join("target/plugins-test/lang-go");
    fs::create_dir_all(&out).expect("create plugin dir");
    let tmp = out.join(format!("lang-go.{}.tmp", std::process::id()));
    let status = Command::new("go")
        .args(["build", "-o"])
        .arg(&tmp)
        .arg("./cmd/lang-go")
        .current_dir(&source)
        .status()
        .expect("run go build");
    assert!(status.success(), "go build of plugins/lang-go failed");
    fs::rename(&tmp, out.join("lang-go")).expect("install plugin binary");
    install_manifest(&source, &out, &goroot(&source));
    Some(out)
}

fn goroot(dir: &Path) -> String {
    let output = Command::new("go")
        .args(["env", "GOROOT"])
        .current_dir(dir)
        .output()
        .expect("run go env");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// Installs the manifest. On unix the plugin starts through a launcher that
/// puts the toolchain that built it first on `PATH`, so tests do not depend on
/// version manager shims resolving a toolchain from the directory of a
/// throwaway project.
fn install_manifest(source: &Path, out: &Path, goroot: &str) {
    let text = fs::read_to_string(source.join("lighthouse-plugin.toml")).unwrap();
    let tmp = out.join(format!("manifest.{}.tmp", std::process::id()));
    if cfg!(unix) {
        let launcher = out.join(format!("run.{}.tmp", std::process::id()));
        let script = format!(
            "#!/bin/sh\nPATH=\"{goroot}/bin:$PATH\" exec \"$(dirname \"$0\")/lang-go\" \"$@\"\n"
        );
        fs::write(&launcher, script).unwrap();
        #[cfg(unix)]
        fs::set_permissions(
            &launcher,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fs::rename(&launcher, out.join("run")).unwrap();
        fs::write(
            &tmp,
            text.replace("command = \"./lang-go\"", "command = \"./run\""),
        )
        .unwrap();
    } else {
        fs::write(&tmp, text).unwrap();
    }
    fs::rename(&tmp, out.join("lighthouse-plugin.toml")).expect("install plugin manifest");
}
