//! The `lang-toml` plugin: the syntax tree of TOML files, with the map from
//! each value to its bytes, for the host to select source regions from.

use std::io::{self, BufReader};

use lighthouse_protocol::language::{self, HostProvider};

mod comments;
mod provider;
mod regions;

fn main() {
    let mut provider = HostProvider::new(provider::Toml);
    let input = BufReader::new(io::stdin().lock());
    if let Err(error) = language::serve(input, io::stdout().lock(), &mut provider) {
        eprintln!("lang-toml: {error}");
    }
}

#[cfg(test)]
mod tests;
