//! Proposing the fixes of an agent that edits. The findings given to a model
//! are grouped by file, and the agent is run once for each file; what it
//! leaves is one edit of the whole file, which every finding of the file
//! shares. The checks then say which of the findings it fixed.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use lighthouse_model::{Diagnostic, LineIndex, Position, Safety, Span, ucm::EditOp};

use super::{
    FixBinding, FixedBy,
    lower::{Lowerer, Sources},
    model::{MAX_FINDINGS, ModelFixer, World},
    propose::{Aim, Proposal, Proposer},
};
use crate::{Error, Overlays, Run, engine::Scopes};

/// A finding given to a model, and the fix that is asked of it.
type Asked<'o> = (&'o Diagnostic, FixBinding);

impl Proposer<'_> {
    /// The proposals for the findings `found`, each file's findings sharing
    /// the one proposal of the agent's run over it; the findings that get
    /// none are declined with the reason.
    pub(super) fn propose_edits<'o>(
        &self,
        outcome: &Run,
        texts: &Overlays,
        (lowerer, sources, scope): (&Lowerer, &Sources, &Scopes),
        found: Vec<Asked<'o>>,
        decline: &mut dyn FnMut(&Diagnostic, String),
    ) -> Result<Vec<Proposal>, Error> {
        let Some(models) = self.models.as_ref().filter(|m| m.edits()) else {
            return Ok(Vec::new());
        };
        let world = World::new(&self.engine.ws.root, outcome, texts, scope);
        let mut proposals = Vec::new();
        for (file, mut group) in by_file(found) {
            let rest = group.split_off(group.len().min(MAX_FINDINGS));
            for (finding, _) in &rest {
                decline(
                    finding,
                    format!(
                        "left for the next run: an agent is given {MAX_FINDINGS} findings of a file at most"
                    ),
                );
            }
            match self.edit_file(models, &world, (lowerer, sources), &file, &group) {
                Ok(edited) => proposals.extend(edited),
                Err(reason) => {
                    for (finding, _) in &group {
                        decline(finding, reason.clone());
                    }
                }
            }
        }
        Ok(proposals)
    }

    /// The proposals for `group`, the findings of `file`, from one run of the
    /// agent.
    fn edit_file(
        &self,
        models: &ModelFixer<'_>,
        world: &World<'_>,
        (lowerer, sources): (&Lowerer, &Sources),
        file: &Path,
        group: &[Asked<'_>],
    ) -> Result<Vec<Proposal>, String> {
        let language = world
            .outcome
            .project
            .file(file)
            .map_or("", |f| f.lang.as_str());
        if let Some(gap) = self.overlay_gap(language) {
            return Err(gap);
        }
        let findings: Vec<_> = group
            .iter()
            .map(|(finding, binding)| (*finding, &binding.decision))
            .collect();
        let edit = models.edit(&models.batch(file, language, &findings), world)?;
        let text = sources.text(file)?;
        let whole = Span {
            start: Position { line: 1, col: 1 },
            end: LineIndex::new(&text).position(text.len()),
        };
        let edits = lowerer
            .lower(&EditOp::Replace {
                file: file.to_owned(),
                span: whole,
                text: edit.text,
            })
            .map_err(|why| format!("replace: {why}"))?;
        if edits.is_empty() {
            return Err("the fix changes nothing".to_owned());
        }
        Ok(group
            .iter()
            .map(|(finding, binding)| Proposal {
                fingerprint: finding.fingerprint.clone(),
                rule_id: finding.rule_id.clone(),
                file: finding.file.clone(),
                line: finding.span.start.line,
                fixer: FixedBy::Model,
                description: format!("model edit by `{}`", edit.model),
                safety: Safety::Suggested.max(binding.cap),
                edits: edits.clone(),
                files: vec![file.to_owned()],
                aim: Aim::File,
            })
            .collect())
    }
}

/// The findings by their file, in the order of their lines.
fn by_file(found: Vec<Asked<'_>>) -> BTreeMap<PathBuf, Vec<Asked<'_>>> {
    let mut files: BTreeMap<PathBuf, Vec<Asked<'_>>> = BTreeMap::new();
    for asked in found {
        files.entry(asked.0.file.clone()).or_default().push(asked);
    }
    for group in files.values_mut() {
        group.sort_by_key(|(finding, _)| (finding.span.start.line, finding.span.start.col));
    }
    files
}
