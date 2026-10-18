//! The Seatbelt profile (SBPL) of macOS. The base is the one of OpenAI Codex,
//! `seatbelt_base_policy.sbpl` (Apache-2.0), itself taken from Chromium's
//! sandbox, cut to what the programs Lighthouse runs need; the reads, writes
//! and network are Anthropic sandbox-runtime's.

use std::path::Path;

use super::Plan;

/// The devices every program may write.
const DEVICES: [&str; 4] = ["/dev/null", "/dev/tty", "/dev/stdout", "/dev/stderr"];

/// The profile that lets a program read everything but what `plan` denies,
/// write only what it lists, and reach only the proxy's loopback port.
pub fn profile(plan: &Plan) -> String {
    let mut rules = vec![
        "(version 1)".to_owned(),
        "(deny default)".to_owned(),
        "(allow process-exec)".to_owned(),
        "(allow process-fork)".to_owned(),
        "(allow signal (target same-sandbox))".to_owned(),
        "(allow sysctl-read)".to_owned(),
        "(allow mach-lookup)".to_owned(),
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
