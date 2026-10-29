//! The `lang-yaml` plugin: the syntax tree of YAML files, with the map from
//! each value to its bytes, for the host to select source regions from.

use lighthouse_protocol::{HostError, HostLanguage, HostParser, HostTree, serve_host};

mod scalar;
mod tree;

struct Yaml;

impl HostParser for Yaml {
    fn parse(&self, text: &str) -> Result<HostTree, HostError> {
        tree::parse(text)
    }
}

fn main() {
    serve_host(
        HostLanguage {
            plugin: "lang-yaml",
            version: env!("CARGO_PKG_VERSION"),
            id: "yaml",
            name: "YAML",
            aliases: &["yaml", "yml"],
            globs: &["**/*.yaml", "**/*.yml"],
        },
        Yaml,
        std::io::BufReader::new(std::io::stdin().lock()),
        std::io::stdout().lock(),
    );
}

#[cfg(test)]
mod tests;
