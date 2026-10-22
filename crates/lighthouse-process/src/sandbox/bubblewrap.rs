//! The bubblewrap command line of Linux: the namespaces Docker uses, with the
//! host's files read-only, the secrets and the places where programs leave
//! their sockets (`/run` and `/tmp`) covered, and what the run needs below them
//! bound again after the cover.

use super::Plan;

/// `program` run in the namespaces of `plan`. The network namespace is empty;
/// with a bridge, the program starts under it, which forwards a local port to
/// the proxy's socket.
pub fn argv(plan: &Plan, program: &[String]) -> Vec<String> {
    let mut args = words(&[
        "bwrap",
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
    ]);
    for place in &plan.mask {
        args.extend(["--tmpfs".to_owned(), place.to_string_lossy().into_owned()]);
    }
    for path in &plan.reads {
        let path = path.to_string_lossy().into_owned();
        args.extend(["--ro-bind".to_owned(), path.clone(), path]);
    }
    for denied in &plan.deny {
        let path = denied.path.to_string_lossy().into_owned();
        if denied.dir {
            args.extend(["--tmpfs".to_owned(), path]);
        } else {
            args.extend(["--ro-bind".to_owned(), "/dev/null".to_owned(), path]);
        }
    }
    for path in &plan.writes {
        let path = path.to_string_lossy().into_owned();
        args.extend(["--bind".to_owned(), path.clone(), path]);
    }
    args.extend(words(&[
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--",
    ]));
    if let (Some(bridge), Some(port)) = (&plan.bridge, plan.port) {
        args.push(bridge.exe.to_string_lossy().into_owned());
        args.extend(words(&["internal", "bridge"]));
        args.push(bridge.socket.to_string_lossy().into_owned());
        args.push(port.to_string());
        args.push("--".to_owned());
    }
    args.extend(program.iter().cloned());
    args
}

fn words(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}
