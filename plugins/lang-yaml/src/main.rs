//! The `lang-yaml` plugin: the syntax tree of YAML files, with the map from
//! each value to its bytes, for the host to select source regions from.

use std::io::{self, BufReader};

use lighthouse_protocol::language::{self, HostProvider};

mod provider;
mod regions;

fn main() {
    let mut provider = HostProvider::new(provider::Yaml);
    let input = BufReader::new(io::stdin().lock());
    if let Err(error) = language::serve(input, io::stdout().lock(), &mut provider) {
        eprintln!("lang-yaml: {error}");
    }
}

#[cfg(test)]
mod tests;
