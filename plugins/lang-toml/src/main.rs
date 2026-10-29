//! The `lang-toml` plugin: the syntax tree of TOML files, with the map from
//! each value to its bytes, for the host to select source regions from.

use lighthouse_protocol::{HostError, HostLanguage, HostParser, HostTree, serve_host};

mod strings;
mod tree;

struct Toml;

impl HostParser for Toml {
    fn parse(&self, text: &str) -> Result<HostTree, HostError> {
        tree::parse(text)
    }
}

fn main() {
    serve_host(
        HostLanguage {
            plugin: "lang-toml",
            version: env!("CARGO_PKG_VERSION"),
            id: "toml",
            name: "TOML",
            aliases: &["toml"],
            globs: &["**/*.toml"],
        },
        Toml,
        std::io::BufReader::new(std::io::stdin().lock()),
        std::io::stdout().lock(),
    );
}

#[cfg(test)]
mod tests;
