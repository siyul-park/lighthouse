//! The `lang-markdown` plugin: the syntax tree of Markdown files, with the map
//! from each node to its bytes and from each code block's text to the bytes
//! it was read from, for the host to select source regions from.

use std::io::{self, BufReader};

use lighthouse_protocol::language::{self, HostProvider};

mod provider;
mod regions;

fn main() {
    let mut provider = HostProvider::new(provider::Markdown);
    let input = BufReader::new(io::stdin().lock());
    if let Err(error) = language::serve(input, io::stdout().lock(), &mut provider) {
        eprintln!("lang-markdown: {error}");
    }
}

#[cfg(test)]
mod tests;
