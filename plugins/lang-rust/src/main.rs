//! Lighthouse language plugin for Rust: reads sources with `syn`, resolves
//! names through its own module tree and speaks the plugin protocol on stdin
//! and stdout.

mod body;
mod cargo;
mod comments;
mod events;
mod extract;
mod macros;
mod names;
mod provider;
mod signature;
mod testcase;
mod tree;
mod util;

use std::{
    io::{self, BufReader},
    process::ExitCode,
};

const ID: &str = "lang-rust";
const VERSION: &str = "0.1.0";

/// Deeply nested source recurses deeply in the parser and the walks; the work
/// runs on a thread with room for it.
const STACK_BYTES: usize = 256 * 1024 * 1024;

fn main() -> ExitCode {
    let worker = std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(serve);
    match worker.map(std::thread::JoinHandle::join) {
        Ok(Ok(code)) => code,
        Ok(Err(_)) => {
            eprintln!("{ID}: the analysis thread panicked");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("{ID}: cannot start the analysis thread: {error}");
            ExitCode::FAILURE
        }
    }
}

fn serve() -> ExitCode {
    let mut provider = provider::Provider::new(ID, VERSION);
    let input = BufReader::new(io::stdin().lock());
    match lighthouse_protocol::serve(input, io::stdout().lock(), &mut provider) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{ID}: {error}");
            ExitCode::FAILURE
        }
    }
}
