//! `model-local`: the model that learns from the labels of a project. It
//! embeds subjects with EmbeddingGemma-300M, fits a rate, nearest neighbours
//! or a logistic regression to the labels (whichever the data supports),
//! and keeps the result as a file named by its digest, so that a reference
//! to `learned@sha256:<hex>` is always the same model.

mod artifact;
mod band;
mod candidates;
mod embed;
mod fit;
#[cfg(test)]
mod fixture;
mod folds;
mod newton;
#[cfg(test)]
mod plugin_tests;
mod rows;
mod serve;
mod train;

use std::{
    io::{stdin, stdout},
    process::ExitCode,
};

use lighthouse_protocol::serve as protocol_serve;

fn main() -> ExitCode {
    let mut handler = serve::Plugin::new(embed::Embedder::open);
    match protocol_serve(stdin().lock(), stdout().lock(), &mut handler) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("model-local: {e}");
            ExitCode::FAILURE
        }
    }
}
