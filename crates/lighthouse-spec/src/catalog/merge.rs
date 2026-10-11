use std::collections::{BTreeMap, BTreeSet};

use lighthouse_resource::Resource;

use crate::{
    CheckKind, Content, Decision, Error, Example, ExampleFile, Pack, PackSpec, Section,
    SectionSpec,
    decision::{PACK_LABEL, SECTION_LABEL},
    validate,
};

use super::{LOCAL_PACK, LOCAL_SECTION, load::Files};

/// Packs with their decisions, in the order the packs list them. Every
/// decision is listed once, in the pack and section its labels name.
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
                labels_match(&decision, &id, &name)?;
                members.push(resolve_examples(files, &dir, decision)?);
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
            sections,
        });
    }
    if let Some((id, (dir, decision))) = waiting.into_iter().next() {
        let place = if decision.section().is_empty() {
            "no section".to_owned()
        } else {
            format!(
                "section `{}` of pack `{}`",
                decision.section(),
                decision.pack()
            )
        };
        return Err(Error::layout(
            &dir,
            format!("`{id}` names {place} but its pack does not list it"),
        ));
    }
    Ok(built)
}

/// A decision of the local layer, with the pack and section labels a project
/// does not have to write.
pub(super) fn local_decision(file: &str, mut decision: Decision) -> Result<Decision, Error> {
    if !decision.id().starts_with("local/") {
        return Err(Error::layout(
            file,
            format!(
                "id is `{}`, a local decision is `local/<name>`",
                decision.id()
            ),
        ));
    }
    for (key, value) in [(PACK_LABEL, LOCAL_PACK), (SECTION_LABEL, LOCAL_SECTION)] {
        let labels = &mut decision.metadata_mut().labels;
        match labels.get(key) {
            Some(found) if found != value => {
                return Err(Error::layout(
                    file,
                    format!("label `{key}` is `{found}`, a local decision's is `{value}`"),
                ));
            }
            _ => {
                labels.insert(key.to_owned(), value.to_owned());
            }
        }
    }
    if let Some(CheckKind::Builtin(builtin)) = decision.check.as_ref().map(|c| &c.kind)
        && builtin.named().is_some()
    {
        return Err(Error::layout(
            file,
            "a local decision may not name a bundled rule with `builtin: {id}`; use a standard operation, `cel` or `command`",
        ));
    }
    Ok(decision)
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

/// A decision's labels name the pack and section that list it.
fn labels_match(decision: &Decision, pack: &str, section: &str) -> Result<(), Error> {
    let id = decision.id();
    let labels = &decision.metadata().labels;
    for (key, want) in [(PACK_LABEL, pack), (SECTION_LABEL, section)] {
        match labels.get(key) {
            Some(found) if found == want => {}
            Some(found) => {
                return Err(Error::invalid(
                    id,
                    format!(
                        "label `{key}` is `{found}` but the {} lists it",
                        if key == PACK_LABEL { "pack" } else { "section" }
                    ),
                ));
            }
            None => {
                return Err(Error::invalid(
                    id,
                    format!("needs the label `{key}: {want}`"),
                ));
            }
        }
    }
    Ok(())
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
        let Content::File(source) = &file.content else {
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
            source.clone()
        } else {
            format!("{dir}/{source}")
        };
        let text = files
            .get(&key)
            .ok_or_else(|| Error::invalid(id, format!("example source `{source}` does not exist")))?
            .clone();
        resolved.push(file.with_loaded(text));
    }
    Ok(resolved)
}
