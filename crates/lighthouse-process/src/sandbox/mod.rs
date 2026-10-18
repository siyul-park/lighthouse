//! The OS sandbox every program run through [`crate::run`] lives in. The
//! policy is the shape of Anthropic's sandbox-runtime: a program reads
//! everything but a list of secrets, writes only where its [`Profile`] says,
//! and reaches only the hosts of the profile, through a proxy of ours. Seatbelt
//! (`sandbox-exec`) enforces it on macOS and bubblewrap on Linux; where the
//! backend cannot be used, a sandboxed run fails and never runs unsandboxed.

mod backend;
mod bridge;
mod bubblewrap;
mod entries;
mod plan;
mod probe;
mod proxy;
mod relay;
mod seatbelt;

use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use tempfile::TempDir;

pub(crate) use backend::{ended, wrap};
use bridge::Uplink;
pub use bridge::bridge;
pub use bubblewrap::argv as bwrap_argv;
pub use entries::{check_host, check_write};
pub use plan::{Bridge, Denied, Plan};
pub use probe::{Simulation, simulate_unavailable};
pub use proxy::{Proxy, allows};
pub use seatbelt::profile as seatbelt_profile;

/// The paths below the home directory that a sandboxed program may not read.
pub const DENIED: [&str; 21] = [
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".config/gcloud",
    ".kube",
    ".docker/config.json",
    ".config/gh",
    ".netrc",
    ".git-credentials",
    ".npmrc",
    ".pypirc",
    "Library/Keychains",
    ".config/lighthouse",
    ".cargo/credentials.toml",
    ".cargo/credentials",
    ".local/share/keyrings",
    ".password-store",
    ".config/op",
    ".m2/settings.xml",
    ".gem/credentials",
];

/// How a program runs: under the OS sandbox with a profile, or, only in a
/// trusted project that says `[sandbox] enabled = false`, with the user's full
/// access.
#[derive(Debug, Clone, Copy)]
pub enum Sandbox<'a> {
    On(&'a Profile),
    Off,
}

/// What a sandboxed program may do besides reading: write where it says (and
/// in its own temporary directory and the null and terminal devices, always),
/// reach the hosts it lists, and not read what it denies on top of [`DENIED`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    pub writes: Vec<PathBuf>,
    /// Hosts, as `name`, `*.suffix` or either with `:port`.
    pub network: Vec<String>,
    pub deny: Vec<PathBuf>,
}

/// The `[sandbox]` table of a project, as it resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// Whether programs run sandboxed.
    pub enabled: bool,
    /// The hosts that checks and verification commands may reach.
    pub network: Vec<String>,
    /// The places besides their own that they may write: paths below the
    /// project, or below the home directory with a `~/` in front.
    pub write: Vec<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            enabled: true,
            network: Vec::new(),
            write: Vec::new(),
        }
    }
}

impl Policy {
    /// The places of `write` as paths, for a program run for the project at
    /// `root`. One that would be a secret is left out whatever it says.
    pub fn writes(&self, root: &Path) -> Vec<PathBuf> {
        let home = env::var_os("HOME").map(PathBuf::from);
        let secret = secrets(home.as_deref(), root);
        self.write
            .iter()
            .filter_map(|entry| match entry.strip_prefix("~/") {
                Some(rest) => home.as_ref().map(|home| home.join(rest)),
                None => Some(root.join(entry)),
            })
            .filter(|path| !secret.iter().any(|secret| path.starts_with(secret)))
            .collect()
    }

    /// `profile` when the project sandboxes its programs, else none.
    pub fn sandbox<'a>(&self, profile: &'a Profile) -> Sandbox<'a> {
        if self.enabled {
            Sandbox::On(profile)
        } else {
            Sandbox::Off
        }
    }
}

impl Profile {
    /// A profile that reads everything but the secrets of the user and of the
    /// project at `root`, writes nothing and reaches nothing.
    pub fn new(root: &Path) -> Self {
        let home = env::var_os("HOME").map(PathBuf::from);
        Self {
            deny: secrets(home.as_deref(), root),
            ..Self::default()
        }
    }

    /// The same profile, writing the user's cache directory too, where
    /// toolchains keep what they build.
    pub fn caching(mut self) -> Self {
        self.writes.extend(user_cache());
        self
    }
}

/// What a sandboxed run holds while it lives: its temporary directory, the
/// proxy that carries its network, and the plan of the backend.
pub(crate) struct Started {
    _dir: TempDir,
    tmp: PathBuf,
    pub(crate) plan: Plan,
    proxy: Option<Proxy>,
    _uplink: Option<Uplink>,
}

impl Started {
    /// Gets everything a run under `profile` needs, or says why it cannot be.
    pub(crate) fn new(profile: &Profile, cwd: &Path) -> Result<Self, String> {
        if env::var_os("HOME").is_none_or(|home| home.is_empty()) {
            return Err("cannot locate the user's secrets: HOME is not set".to_owned());
        }
        probe::available().map_err(|reason| {
            format!(
                "sandbox unavailable: {reason}; {}, or set [sandbox] enabled = false in a trusted project",
                backend::HINT
            )
        })?;
        let dir = tempfile::tempdir().map_err(|e| format!("no temporary directory: {e}"))?;
        let tmp = dir
            .path()
            .canonicalize()
            .map_err(|e| format!("no temporary directory: {e}"))?;
        let proxy = (!profile.network.is_empty())
            .then(|| Proxy::start(&profile.network))
            .transpose()
            .map_err(|e| format!("no network proxy: {e}"))?;
        let plan = Plan::of(profile, &tmp, cwd, proxy.as_ref().map(Proxy::port))?;
        let uplink = plan
            .bridge
            .as_ref()
            .zip(proxy.as_ref())
            .map(|(bridge, proxy)| Uplink::start(&bridge.socket, proxy.port()))
            .transpose()
            .map_err(|e| format!("no network bridge: {e}"))?;
        Ok(Self {
            _dir: dir,
            tmp,
            plan,
            proxy,
            _uplink: uplink,
        })
    }

    /// The variables the program is given on top of its own: where it may put
    /// temporary files, and the proxy that is its only way out.
    pub(crate) fn env(&self) -> Vec<(String, String)> {
        let mut env = vec![("TMPDIR".to_owned(), self.tmp.to_string_lossy().into_owned())];
        if let Some(proxy) = &self.proxy {
            let url = format!("http://127.0.0.1:{}", proxy.port());
            for name in ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"] {
                env.push((name.to_owned(), url.clone()));
                env.push((name.to_lowercase(), url.clone()));
            }
            env.push(("NO_PROXY".to_owned(), String::new()));
            env.push(("no_proxy".to_owned(), String::new()));
        }
        env
    }

    /// What the proxy refused, to tell the user why a request failed.
    pub(crate) fn blocked(&self) -> Vec<String> {
        self.proxy.as_ref().map(Proxy::blocked).unwrap_or_default()
    }
}

/// The secrets a sandboxed program may not read: [`DENIED`] below `home`, and
/// the environment files and personal settings of the project at `root`.
pub fn secrets(home: Option<&Path>, root: &Path) -> Vec<PathBuf> {
    let personal = home
        .into_iter()
        .flat_map(|home| DENIED.iter().map(|path| home.join(path)));
    let files = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| is_project_secret(Path::new(&entry.file_name())))
        .map(|entry| entry.path());
    personal
        .chain(files)
        .chain([root.join(".lighthouse/local.toml")])
        .collect()
}

/// Whether the project-relative `path` is a secret of the project: an
/// environment file at its root, or the personal settings of its user.
pub fn is_project_secret(path: &Path) -> bool {
    path == Path::new(".lighthouse/local.toml")
        || path
            .to_str()
            .is_some_and(|name| name == ".env" || name.starts_with(".env."))
}

/// Whether `program` names something to run from `dir`: a file at a path, or a
/// file of that name in a directory of `env`'s `PATH` (ours without `env`). The
/// sandbox starts the program for the caller, so it cannot say that it was
/// missing; this does.
pub(crate) fn runnable(program: &str, dir: &Path, env: Option<&[(String, String)]>) -> bool {
    if program.contains('/') {
        return dir.join(program).is_file();
    }
    let path = match env {
        Some(env) => env
            .iter()
            .find(|(name, _)| name == "PATH")
            .map(|(_, value)| OsString::from(value)),
        None => std::env::var_os("PATH"),
    };
    let path = path.unwrap_or_else(|| OsString::from("/usr/bin:/bin"));
    std::env::split_paths(&path).any(|place| place.join(program).is_file())
}

/// The directory the OS keeps a user's caches in.
fn user_cache() -> Option<PathBuf> {
    let absolute = |name: &str| {
        env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let home = absolute("HOME")?;
    if cfg!(target_os = "macos") {
        Some(home.join("Library/Caches"))
    } else {
        absolute("XDG_CACHE_HOME").or_else(|| Some(home.join(".cache")))
    }
}
