//! Session signals: what the hooks record of a session (a person's messages
//! classified, the findings an edit introduced or resolved), the measures of
//! the loop that turns signals into proposed decisions, and the commands that
//! list and settle them. The signals live in the local store and are never
//! committed; only decisions an agent records are.

mod classify;
mod propose;
mod read;
mod scrub;
mod speak;
mod strip;

use std::{collections::BTreeMap, path::Path};

use lighthouse_ml::metrics::Rate;
use lighthouse_model::{AgentKind, Attribution, Diagnostic, Role, SignalClass, SignalStatus};
use lighthouse_spec::{DecisionStatus, Surface};
pub use lighthouse_store::Signal;
use lighthouse_store::{NewSignal, SignalFilter, Store};
use schemars::JsonSchema;
use serde::Serialize;

pub use classify::{EXCERPT_CHARS, Hit, Reading, classify, read as read_prompt, rules, truncated};
pub use propose::{
    DUPLICATE_THRESHOLD, Duplicate, Support, candidates, record_proposal, settle_derived, support,
};
pub use read::{DERIVED_PREFIX, Said, features, said, signal_id};
pub use scrub::{REDACTED, scrub};
pub use speak::{digest, prompt_context, stop_context};
pub use strip::{Stripped, stripped};

use crate::{Result, Session};

impl Session {
    /// How long signals are kept, in seconds.
    fn retention(&self) -> u64 {
        self.config.signals().retention().as_secs()
    }

    /// Whether the agent hears about signals.
    pub fn surfaces(&self) -> bool {
        self.config.signals().surface() == Surface::On
    }
}

/// What an edit did to one finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Introduced,
    Resolved,
}

/// How many signals fall in each bucket.
#[derive(Debug, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub total: usize,
    /// By class: `directive`, `correction`, `preference`, `none`, `abstain`
    /// and `finding`.
    pub by_class: BTreeMap<String, usize>,
    /// By the classifier rule that fired, or the decision of a finding.
    pub by_rule: BTreeMap<String, usize>,
    /// By status: `open`, `recorded`, `linked` and `dismissed`.
    pub by_status: BTreeMap<String, usize>,
    /// Prompts no rule decided, which a bound model could.
    pub abstained: usize,
}

impl Change {
    fn name(self) -> &'static str {
        match self {
            Self::Introduced => "introduced",
            Self::Resolved => "resolved",
        }
    }
}

/// The measures of the loop, counted from signal statuses and proposals, never
/// from what the agent says of itself.
#[derive(Debug, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Measures {
    /// The share of signals that state a decision that were recorded or
    /// linked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture: Option<Rate>,
    /// The share of proposals the dedupe check named a possible duplicate for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplicate: Option<Rate>,
    /// The share of proposals a person has accepted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ratification: Option<Rate>,
}

/// The `DecisionSignals` kind.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignalReport {
    /// The open signals that state a decision, or every signal with `all`.
    pub signals: Vec<Signal>,
    /// How many `limit` left out.
    pub omitted: usize,
    pub counts: Counts,
    pub measures: Measures,
}

/// Records what a person's prompt says, whatever it says: a class a rule
/// proves, `none`, or an abstention. Returns the signal.
pub fn capture_prompt(session: &Session, hook_session: &str, prompt: &str) -> Result<Signal> {
    let said = said(prompt);
    let now = lighthouse_model::now();
    let new = NewSignal {
        id: signal_id(&[hook_session, &now, "prompt", said.rule, &said.excerpt]),
        session: hook_session.to_owned(),
        event: "UserPromptSubmit".to_owned(),
        class: said.class,
        excerpt: said.excerpt,
        features: said.features,
        rule: said.rule.to_owned(),
        by: Attribution::new(AgentKind::Person, None),
    };
    let mut store = Store::open(&session.root)?;
    store.add_signal(&new, session.retention())?;
    Ok(store.signal(&new.id)?)
}

/// Records the findings a session introduced and resolved as signals of class
/// `finding`, attributed to `by`, the agent that made the edits. A finding
/// seen again, by the next hook, is recorded once.
pub fn capture_findings(
    session: &Session,
    hook_session: &str,
    event: &str,
    introduced: &[Diagnostic],
    resolved: &[String],
    by: &Attribution,
) -> Result<()> {
    if introduced.is_empty() && resolved.is_empty() {
        return Ok(());
    }
    let mut store = Store::open(&session.root)?;
    let mut news = Vec::new();
    for d in introduced {
        news.push(finding(
            hook_session,
            event,
            Change::Introduced,
            d.fingerprint.as_str(),
            &d.rule_id,
            &format!("{}: {}", d.rule_id, d.message),
            by,
        ));
    }
    for fingerprint in resolved {
        let known = store.finding(fingerprint).ok();
        let (rule, text) = known.map_or_else(
            || (String::new(), format!("finding {}", short(fingerprint))),
            |f| (f.decision.clone(), format!("{}: {}", f.decision, f.message)),
        );
        news.push(finding(
            hook_session,
            event,
            Change::Resolved,
            fingerprint,
            &rule,
            &text,
            by,
        ));
    }
    for new in &news {
        store.add_signal(new, session.retention())?;
    }
    Ok(())
}

/// The open signals of a session that state a decision, no stop has asked
/// about yet. Empty when the project has no store.
pub fn unasked(root: &Path, hook_session: &str) -> Result<Vec<Signal>> {
    let Some(store) = Store::open_existing(root)? else {
        return Ok(Vec::new());
    };
    Ok(store.signals(&SignalFilter {
        session: Some(hook_session.to_owned()),
        status: Some("open".to_owned()),
        unasked: true,
        said: true,
    })?)
}

/// Marks signals as asked about.
pub fn mark_asked(root: &Path, signals: &[Signal]) -> Result<()> {
    let ids: Vec<String> = signals.iter().map(|s| s.id.clone()).collect();
    Ok(Store::open(root)?.mark_asked(&ids)?)
}

/// The signals, their counts and the measures of the loop.
pub fn report(session: &Session, all: bool, limit: Option<usize>) -> Result<SignalReport> {
    let Some(store) = Store::open_existing(&session.root)? else {
        return Ok(empty());
    };
    let every = store.signals(&SignalFilter::default())?;
    let proposals = store.proposals()?;
    let mut counts = Counts {
        total: every.len(),
        ..Counts::default()
    };
    for s in &every {
        *counts.by_class.entry(s.class.to_string()).or_default() += 1;
        *counts
            .by_status
            .entry(s.status.name().to_owned())
            .or_default() += 1;
        if s.class == SignalClass::Abstain {
            counts.abstained += 1;
        } else if !s.rule.is_empty() {
            *counts.by_rule.entry(s.rule.clone()).or_default() += 1;
        }
    }
    let said: Vec<&Signal> = every
        .iter()
        .filter(|s| SignalClass::SAID.contains(&s.class))
        .collect();
    let settled = said
        .iter()
        .filter(|s| {
            matches!(
                s.status,
                SignalStatus::Recorded { .. } | SignalStatus::Linked { .. }
            )
        })
        .count();
    let accepted = proposals
        .iter()
        .filter(|p| {
            session
                .catalog
                .decisions()
                .any(|d| d.is_key(&p.decision_uid) && d.status == DecisionStatus::Accepted)
        })
        .count();
    let measures = Measures {
        capture: Rate::new(settled, said.len()),
        duplicate: Rate::new(
            proposals
                .iter()
                .filter(|p| !p.duplicates.is_empty())
                .count(),
            proposals.len(),
        ),
        ratification: Rate::new(accepted, proposals.len()),
    };
    let mut shown: Vec<Signal> = every
        .into_iter()
        .filter(|s| all || (s.status == SignalStatus::Open && SignalClass::SAID.contains(&s.class)))
        .collect();
    let omitted = limit.map_or(0, |n| shown.len().saturating_sub(n));
    if let Some(n) = limit {
        shown.truncate(n);
    }
    Ok(SignalReport {
        signals: shown,
        omitted,
        counts,
        measures,
    })
}

/// Dismisses the signal `id` (a unique prefix) with `reason`, attributed to
/// `by`. A dismissal is the label that teaches the classifier.
pub fn dismiss(session: &Session, id: &str, reason: &str, by: &Attribution) -> Result<Signal> {
    if reason.trim().is_empty() {
        return Err("a dismissal needs a reason: say why the signal is not a decision".into());
    }
    let mut store = Store::open_existing(&session.root)?
        .ok_or_else(|| format!("no recorded signal matches `{id}`"))?;
    let signal = store.signal(id)?;
    let status = SignalStatus::Dismissed {
        reason: reason.trim().to_owned(),
    };
    store.settle_signal(&signal.id, &status, &spelled(by))?;
    Ok(store.signal(&signal.id)?)
}

/// `person:<id>` or `agent:<id>`, the way `--attributed-to` spells it.
pub(crate) fn spelled(by: &Attribution) -> String {
    let kind = match (by.kind, by.role) {
        (AgentKind::Person, _) => "person",
        (_, Some(Role::Model)) => "model",
        _ => "agent",
    };
    by.id
        .as_ref()
        .map_or_else(|| kind.to_owned(), |id| format!("{kind}:{id}"))
}

fn finding(
    hook_session: &str,
    event: &str,
    change: Change,
    fingerprint: &str,
    rule: &str,
    text: &str,
    by: &Attribution,
) -> NewSignal {
    NewSignal {
        id: signal_id(&[hook_session, change.name(), fingerprint]),
        session: hook_session.to_owned(),
        event: event.to_owned(),
        class: SignalClass::Finding,
        excerpt: truncated(&scrub(&format!("{} {text}", change.name()))),
        features: serde_json::json!({ "change": change.name(), "fingerprint": short(fingerprint) }),
        rule: rule.to_owned(),
        by: by.clone(),
    }
}

fn short(fingerprint: &str) -> &str {
    fingerprint.get(..7).unwrap_or(fingerprint)
}

fn empty() -> SignalReport {
    SignalReport {
        signals: Vec::new(),
        omitted: 0,
        counts: Counts::default(),
        measures: Measures::default(),
    }
}
