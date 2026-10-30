//! The dense pack pages of the generated docs: an index per section, short
//! decisions as table rows, and a full entry only for decisions that need one.

use std::fmt::Write;

use lighthouse_model::squash;
use lighthouse_spec::{Pack, Section};

use super::decision::{Density, NOTICE, decision_markdown, is_row};

/// The page of `pack`, whose catalog lies in `catalog_dir` of the repository.
pub fn pack_markdown(pack: &Pack, catalog_dir: &str) -> String {
    let mut out = format!(
        "{NOTICE}\n\n# {}\n\n{}\n\n",
        pack.title,
        squash(&pack.intro)
    );
    for section in &pack.sections {
        section_markdown(&mut out, pack, section, catalog_dir);
    }
    out.truncate(out.trim_end().len());
    out.push('\n');
    out
}

/// One table per section: every decision with its check and fix, and the
/// requirement of the ones that need no entry of their own.
fn section_markdown(out: &mut String, pack: &Pack, section: &Section, catalog_dir: &str) {
    let _ = write!(
        out,
        "## {}\n\n{}\n\n",
        section.title,
        squash(&section.intro)
    );
    let _ = writeln!(out, "| id | title | check | fix | requirement |");
    let _ = writeln!(out, "| --- | --- | --- | --- | --- |");
    for decision in &section.decisions {
        out.push_str(&decision_markdown(decision, Density::Row));
    }
    out.push('\n');
    // A page lies in `docs/decisions/`, two directories below the root.
    let sources = format!("../../{catalog_dir}/{}/{}", pack.id, section.id);
    for decision in section.decisions.iter().filter(|d| !is_row(d)) {
        let density = Density::Entry {
            level: 3,
            sources: Some(&sources),
        };
        out.push_str(&decision_markdown(decision, density));
    }
}
