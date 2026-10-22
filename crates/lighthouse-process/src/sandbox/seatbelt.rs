//! The Seatbelt profile (SBPL) of macOS: a base policy cut to what the
//! programs Lighthouse runs need, with the reads, writes and network of a
//! sandbox runtime. What it was taken from is credited in `docs/NOTICE`.

use std::path::Path;

use super::Plan;

/// The services a program may look up: the user database and groups, logging
/// and notifications, the per-user directories, and certificate validation.
/// Not the keychain (`com.apple.SecurityServer`), Launch Services, the
/// pasteboard, preferences or the network daemons: a program has no need of
/// them, and each is a way out of the sandbox or to a secret. (The base policy
/// this was cut from allows far more.)
const SERVICES: [&str; 6] = [
    "com.apple.system.opendirectoryd.libinfo",
    "com.apple.system.opendirectoryd.membership",
    "com.apple.logd",
    "com.apple.system.notification_center",
    "com.apple.bsd.dirhelper",
    "com.apple.trustd.agent",
];

/// The devices every program may write.
const DEVICES: [&str; 3] = ["/dev/null", "/dev/stdout", "/dev/stderr"];

/// The profile that lets a program read everything but what `plan` denies,
/// write only what it lists, and reach only the proxy's loopback port.
pub fn profile(plan: &Plan) -> String {
    let services = SERVICES
        .iter()
        .map(|name| format!("(global-name \"{name}\")"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut rules = vec![
        "(version 1)".to_owned(),
        "(deny default)".to_owned(),
        "(allow process-exec)".to_owned(),
        "(allow process-fork)".to_owned(),
        "(allow signal (target same-sandbox))".to_owned(),
        "(allow sysctl-read)".to_owned(),
        format!("(allow mach-lookup {services})"),
        "(allow ipc-posix-shm)".to_owned(),
        "(allow file-ioctl)".to_owned(),
        "(allow file-read*)".to_owned(),
    ];
    rules.extend(
        plan.deny
            .iter()
            .map(|denied| format!("(deny file-read* (subpath {}))", literal(&denied.path))),
    );
    let devices = DEVICES
        .iter()
        .map(|device| format!("(literal \"{device}\")"));
    let writes = plan
        .writes
        .iter()
        .map(|path| format!("(subpath {})", literal(path)));
    let places: Vec<String> = devices.chain(writes).collect();
    rules.push(format!("(allow file-write* {})", places.join(" ")));
    if let Some(port) = plan.port {
        rules.push(format!(
            "(allow network-outbound (remote ip \"localhost:{port}\"))"
        ));
    }
    rules.join("\n") + "\n"
}

/// `path` as an SBPL string literal.
fn literal(path: &Path) -> String {
    let text = path.to_string_lossy();
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}
