use std::collections::{BTreeMap, BTreeSet};

use lighthouse_resource::Resource;

use crate::{
    Decision, Error, Example, ExampleFile, Pack, PackSpec, Section, SectionSpec, validate,
};

use super::{load::Files, variables};

/// Packs with their decisions, in the order the packs list them. Every
/// decision is listed once, in the section of the pack its id names.
pub(super) fn assemble(
    files: &Files,
    packs: Vec<(String, Resource<PackSpec>)>,
    decisions: Vec<(String, Decision)>,
) -> Result<Vec<Pack>, Error> {
    let mut waiting: BTreeMap<String, (String, Decision)> = BTreeMap::new();
    for (dir, decision) in decisions {
        let id = decision.id().to_owned();
        if waiting.insert(id.clone(), (dir, decision)).is_some() {
            return Err(Error::invalid(&id, "defined twice"));
        }
    }
    let mut built = Vec::new();
    let mut seen_packs = BTreeSet::new();
    for (path, pack) in packs {
        let id = pack.metadata.name;
        if !seen_packs.insert(id.clone()) {
            return Err(Error::layout(
                &path,
                format!("pack `{id}` is defined twice"),
            ));
        }
        variables::validate(&id, &pack.spec.variables)?;
        let shared = pack.spec.variables;
        let mut sections = Vec::new();
        let mut names = BTreeSet::new();
        for SectionSpec {
            name,
            title,
            intro,
            decisions: listed,
        } in pack.spec.sections
        {
            if !names.insert(name.clone()) {
                return Err(Error::layout(
                    &path,
                    format!("section `{name}` is listed twice"),
                ));
            }
            let mut members = Vec::new();
            for short in &listed {
                let full = format!("{id}/{short}");
                let (dir, decision) = waiting.remove(&full).ok_or_else(|| {
                    Error::layout(
                        &path,
                        format!("`{short}` is listed in section `{name}` but is not defined, or is listed twice"),
                    )
                })?;
                members.push(variables::import(
                    resolve_examples(files, &dir, decision)?,
                    &shared,
                )?);
            }
            sections.push(Section {
                id: name,
                title,
                intro,
                decisions: members,
            });
        }
        built.push(Pack {
            id,
            title: pack.spec.title,
            intro: pack.spec.intro,
            variables: shared,
            sections,
        });
    }
    if let Some((id, (dir, decision))) = waiting.into_iter().next() {
        return Err(Error::layout(
            &dir,
            format!(
                "`{id}` is not listed in a section of its pack `{}`",
                decision.pack()
            ),
        ));
    }
    Ok(built)
}

/// A decision of the local layer is `local/<name>` and names no bundled rule.
pub(super) fn check_local(file: &str, decision: &Decision) -> Result<(), Error> {
    if !decision.id().starts_with("local/") {
        return Err(Error::layout(
            file,
            format!(
                "id is `{}`, a local decision is `local/<name>`",
                decision.id()
            ),
        ));
    }
    Ok(())
}

pub(super) fn merge_sections(into: &mut Pack, from: &Pack) {
    for section in &from.sections {
        match into.sections.iter_mut().find(|s| s.id == section.id) {
            None => into.sections.push(section.clone()),
            Some(existing) => existing.decisions.extend(section.decisions.iter().cloned()),
        }
    }
}

pub(super) fn resolve_examples(
    files: &Files,
    dir: &str,
    decision: Decision,
) -> Result<Decision, Error> {
    let id = decision.id().to_owned();
    let mut resolved = Ok(());
    let decision = decision.map_spec(|mut spec| {
        match resolve_all(files, dir, &id, std::mem::take(&mut spec.examples)) {
            Ok(examples) => spec.examples = examples,
            Err(e) => resolved = Err(e),
        }
        spec
    });
    resolved.map(|()| decision)
}

fn resolve_all(
    files: &Files,
    dir: &str,
    id: &str,
    examples: Vec<Example>,
) -> Result<Vec<Example>, Error> {
    examples
        .into_iter()
        .map(|mut example| {
            example.files = resolve_files(files, dir, id, std::mem::take(&mut example.files))?;
            example.fixed = resolve_files(files, dir, id, std::mem::take(&mut example.fixed))?;
            Ok(example)
        })
        .collect()
}

/// Loads the text of the files that name a `source` in the catalog layer.
fn resolve_files(
    files: &Files,
    dir: &str,
    id: &str,
    list: Vec<ExampleFile>,
) -> Result<Vec<ExampleFile>, Error> {
    let mut resolved = Vec::new();
    for file in list {
        let Some(source) = file.source() else {
            resolved.push(file);
            continue;
        };
        if !validate::relative(source) {
            return Err(Error::invalid(
                id,
                format!("example source `{source}` escapes its directory"),
            ));
        }
        let key = if dir.is_empty() {
            source.to_owned()
        } else {
            format!("{dir}/{source}")
        };
        let text = files
            .get(&key)
            .ok_or_else(|| Error::invalid(id, format!("example source `{source}` does not exist")))?
            .clone();
        resolved.push(file.with_text(text));
    }
    Ok(resolved)
}
