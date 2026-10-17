//! `--color auto|always|never`. The flag is read before the command line is
//! parsed, because the colour of the help and of usage errors depends on it.
//! `auto` leaves the decision to clap, which honours `NO_COLOR`
//! (no-color.org), `CLICOLOR_FORCE` and whether the stream is a terminal.

use std::ffi::OsString;

use clap::{ColorChoice, ValueEnum};

/// The environment variable of the flag.
const ENV: &str = "LIGHTHOUSE_COLOR";

/// The colour choice from the arguments (the flag wins) or the environment;
/// `auto` when neither names a valid one.
pub fn choice(args: &[OsString]) -> ColorChoice {
    let named = flag(args).or_else(|| std::env::var(ENV).ok());
    named
        .and_then(|name| ColorChoice::from_str(&name, true).ok())
        .unwrap_or(ColorChoice::Auto)
}

/// The value of `--color`, in either spelling, before any `--`.
fn flag(args: &[OsString]) -> Option<String> {
    let mut args = args.iter().filter_map(|arg| arg.to_str());
    while let Some(arg) = args.next() {
        if arg == "--" {
            return None;
        }
        if let Some(value) = arg.strip_prefix("--color=") {
            return Some(value.to_owned());
        }
        if arg == "--color" {
            return args.next().map(str::to_owned);
        }
    }
    None
}
