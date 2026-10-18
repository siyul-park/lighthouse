//! The entries a project writes into `[sandbox]`: the hosts programs may reach
//! and the places they may write. They are read once, when the configuration
//! loads, so that a mistake is an error that names the entry and not a hole.

use std::path::{Component, Path};

use super::{DENIED, is_project_secret};

/// Why `entry`, a host of `network`, is not one: a name, or `*.suffix` for
/// every host below a suffix of at least two labels, either with a `:port`.
pub fn check_host(entry: &str) -> Result<(), String> {
    let (name, port) = match entry.rsplit_once(':') {
        Some((name, port)) => (name, Some(port)),
        None => (entry, None),
    };
    if let Some(port) = port
        && !port.parse::<u16>().is_ok_and(|port| port > 0)
    {
        return Err(format!("`{port}` is not a port from 1 to 65535"));
    }
    let (suffix, wildcard) = match name.strip_prefix("*.") {
        Some(suffix) => (suffix, true),
        None => (name, false),
    };
    if name.is_empty() {
        return Err("a host is needed".to_owned());
    }
    if name == "*" || suffix.is_empty() {
        return Err("it would let every host through".to_owned());
    }
    let plain = |b: u8| b.is_ascii_alphanumeric() || b == b'.' || b == b'-';
    let labels: Vec<&str> = suffix.split('.').collect();
    if !suffix.bytes().all(plain) || labels.iter().any(|label| label.is_empty()) {
        return Err("a host is letters, digits, hyphens and dots".to_owned());
    }
    if wildcard && labels.len() < 2 {
        return Err("a suffix has at least two labels, such as `*.example.com`".to_owned());
    }
    Ok(())
}

/// Why `entry`, a place of `write`, is not one: a path below the project, or
/// below the home directory when it starts with `~/`; never the project or the
/// home directory itself, nor a secret.
pub fn check_write(entry: &str) -> Result<(), String> {
    let (home, path) = match entry.strip_prefix("~/") {
        Some(rest) => (true, rest),
        None => (false, entry),
    };
    let path = Path::new(path);
    if path.is_absolute() || (!home && entry.starts_with('~')) {
        return Err("a place is relative to the project, or starts with `~/`".to_owned());
    }
    if !path.components().all(|c| matches!(c, Component::Normal(_))) || path.as_os_str().is_empty()
    {
        return Err("a place is a path below the project or the home directory".to_owned());
    }
    let secret = if home {
        DENIED.iter().any(|denied| path.starts_with(denied))
    } else {
        is_project_secret(path) || path.starts_with(".lighthouse/local.toml")
    };
    if secret {
        return Err("a secret cannot be written".to_owned());
    }
    Ok(())
}
