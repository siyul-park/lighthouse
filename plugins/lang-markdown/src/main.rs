//! The `lang-markdown` plugin: the syntax tree of Markdown files, with the map
//! from each node to its bytes and from each code block's text to the bytes
//! it was read from, for the host to select source regions from.

use lighthouse_protocol::{HostError, HostLanguage, HostParser, HostTree, serve_host};

mod body;
mod fence;
mod lines;
mod tree;

struct Markdown;

impl HostParser for Markdown {
    fn parse(&self, text: &str) -> Result<HostTree, HostError> {
        tree::parse(text)
    }
}

fn main() {
    serve_host(
        HostLanguage {
            plugin: "lang-markdown",
            version: env!("CARGO_PKG_VERSION"),
            id: "markdown",
            name: "Markdown",
            globs: &["**/*.md", "**/*.markdown"],
        },
        Markdown,
        std::io::BufReader::new(std::io::stdin().lock()),
        std::io::stdout().lock(),
    );
}

#[cfg(test)]
mod tests;
