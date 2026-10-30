//! What the agent is told when the project surfaces signals: the context of a
//! message that reads like a decision, the review task at a stop, and the
//! digest of the project's decisions at the start of a session.

use std::{collections::BTreeMap, path::Path};

use lighthouse_model::SignalClass;
use lighthouse_spec::{Decision, DecisionStatus};
use lighthouse_store::{Filter, Signal, StatusFilter, Store};

use super::{
    propose::{CANDIDATE_FLOOR, candidates, documents, first_line, support},
    read::DERIVED_PREFIX,
};
use crate::{Result, Session, active_decisions, git::git, scope::changed};

/// Decisions the digest lists, as ECC's session start limits its instincts.
const DIGEST_LIMIT: usize = 6;
/// Existing decisions a message is offered to link to.
const CANDIDATES: usize = 3;

/// The context for a message `signal` classified as a decision, with the
/// existing decisions that may already say it. `None` for any other class.
pub fn prompt_context(session: &Session, signal: &Signal) -> Option<String> {
    if !SignalClass::SAID.contains(&signal.class) {
        return None;
    }
    let found = candidates(session, &signal.excerpt, None, CANDIDATE_FLOOR, CANDIDATES).ok()?;
    let mut out = format!(
        "Lighthouse: this reads like a design decision ({}, signal {}).\n",
        signal.class, signal.id
    );
    if !found.is_empty() {
        out.push_str("Existing decisions that may already say it:\n");
        for d in &found {
            out.push_str(&format!("- {} ({}): {}\n", d.id, d.status, d.requirement));
        }
    }
    out.push_str(&format!(
        "Link or update one with `decision_update`, or record it with `decision_create` (status proposed). \
         Either way set provenance.wasDerivedFrom to [\"{DERIVED_PREFIX}{}\"].",
        signal.id
    ));
    Some(out)
}

/// The review task at a stop: the open signals of the session.
pub fn stop_context(signals: &[Signal]) -> String {
    let mut out = format!(
        "Lighthouse: {} signal(s) of this session are open:\n",
        signals.len()
    );
    for s in signals {
        out.push_str(&format!("- {} [{}] {}\n", s.id, s.class, s.excerpt));
    }
    out.push_str(
        "Record, link or dismiss each: `decision_create` or `decision_update` with \
         provenance.wasDerivedFrom [\"signal:<id>\"], or `decision_signal_dismiss` with a reason.",
    );
    out
}

/// The digest of a session start: up to six accepted decisions in force,
/// ranked, and the proposals that wait for a person. `None` when there is
/// nothing to say.
///
/// The rank is BM25 (SQLite FTS5) of the decisions' text for what the session
/// is about: the words of the branch, the languages of the changed files and
/// the decisions with open findings. With nothing to ask, the first six by id.
pub fn digest(session: &Session) -> Result<Option<String>> {
    let root = &session.root;
    let active = active_decisions(session)?;
    let accepted: Vec<&Decision> = session
        .catalog
        .decisions()
        .filter(|d| d.status == DecisionStatus::Accepted && active.contains(d.id()))
        .collect();
    let proposals: Vec<&Decision> = session
        .catalog
        .decisions()
        .filter(|d| d.status == DecisionStatus::Proposed)
        .collect();
    if accepted.is_empty() && proposals.is_empty() {
        return Ok(None);
    }
    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let mut about = vec![branch.trim().to_owned()];
    about.extend(
        languages_of(&changed(root).unwrap_or_default())
            .into_iter()
            .map(str::to_owned),
    );
    about.extend(
        open_findings(root)?
            .into_keys()
            .map(|id| id.replace(['/', '-'], " ")),
    );
    let ranked = Store::open(root)?.rank(&documents(&accepted), &about.join(" "), DIGEST_LIMIT)?;
    let mut shown: Vec<&Decision> = ranked
        .iter()
        .filter_map(|r| accepted.iter().copied().find(|d| d.id() == r.id))
        .collect();
    for d in &accepted {
        if shown.len() == DIGEST_LIMIT {
            break;
        }
        if !shown.iter().any(|s| s.id() == d.id()) {
            shown.push(d);
        }
    }
    let mut out = format!(
        "Lighthouse decisions of this project ({} of {} accepted, ranked by BM25 (FTS5) for the branch, \
         the languages of the changed files and the decisions with open findings):\n",
        shown.len(),
        accepted.len()
    );
    for d in shown {
        out.push_str(&format!("- {}: {}\n", d.id(), first_line(&d.requirement)));
    }
    if !proposals.is_empty() {
        out.push_str("Proposals that wait for a person to accept them:\n");
    }
    for d in proposals {
        let line = support(session, d)?.map_or_else(|| "no support".to_owned(), |s| s.line());
        out.push_str(&format!(
            "- {} [{line}]: {}\n",
            d.id(),
            first_line(&d.requirement)
        ));
    }
    Ok(Some(out.trim_end().to_owned()))
}

/// The open findings the store remembers, by decision.
fn open_findings(root: &Path) -> Result<BTreeMap<String, usize>> {
    let mut counts = BTreeMap::new();
    if let Some(store) = Store::open_existing(root)? {
        for f in store.list(&Filter {
            decision: None,
            status: StatusFilter::Open,
        })? {
            *counts.entry(f.observed.decision).or_default() += 1;
        }
    }
    Ok(counts)
}

/// The language ids of the files, by extension.
fn languages_of(files: &[std::path::PathBuf]) -> Vec<&'static str> {
    let mut found = Vec::new();
    for file in files {
        let language = match file.extension().and_then(|e| e.to_str()) {
            Some("rs") => "rust",
            Some("go") => "go",
            Some("md") => "markdown",
            Some("yaml" | "yml") => "yaml",
            Some("toml") => "toml",
            Some("cel") => "cel",
            _ => continue,
        };
        if !found.contains(&language) {
            found.push(language);
        }
    }
    found
}
