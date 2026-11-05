//! The index step: the providers are asked for the units whose key moved, or
//! whose imports' `api` did, and for nothing else.
//!
//! Per language it pulls `units` and the key of each from the graph, finds the
//! stale ones (no stored `fragment`, another key, or an import whose `api` is
//! not what was recorded), sends them to the provider in one request and sets
//! what comes back as `fragment` inputs. A unit whose `api` moved makes its
//! importers stale for the next round, and rounds go on until none does. No
//! stale unit means no request, so no provider process.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::PoisonError,
};

use lighthouse_graph::{Durability, Graph};
use lighthouse_model::ucm::{File, Fragment, ORDERED};
use lighthouse_plugin::{Fragments, Indexed, Unit};
use lighthouse_protocol::hash::Hasher;
use serde::{Deserialize, Serialize};

use super::{regions::Views, *};
use crate::{
    Error as EngineError,
    graph::{
        API, CLAIMED, EVERY, FRAGMENT, PROVIDER, Provider, REGIONS as REGIONS_UNIT, Read, Stored,
        UNIT, UNITS,
    },
};

/// What one provider is asked in a run: its units, and the regions of the host
/// files that selected it.
#[derive(Default)]
pub(super) struct Call<'a> {
    pub units: Vec<UnitCall<'a>>,
    pub regions: Vec<EmbeddedFile>,
}

/// A unit of files and the inputs it holds.
pub(super) struct UnitCall<'a> {
    pub id: String,
    pub files: Vec<&'a Input>,
}

/// A unit to send: what the provider is given of it.
struct Pending<'a> {
    id: String,
    key: String,
    imports: BTreeMap<String, String>,
    sources: Vec<Source<'a>>,
}

/// What a unit's answer holds, as stored: the fragments, the source trees and
/// what the provider said of the unit.
#[derive(Serialize, Deserialize)]
struct Payload {
    fragments: Vec<Fragment>,
    sources: Vec<lighthouse_protocol::Source>,
    notices: Vec<String>,
    incomplete: Vec<Incomplete>,
}

/// What a lane's units give the run.
#[derive(Default)]
struct Taken {
    fragments: Vec<Fragment>,
    sources: Vec<lighthouse_protocol::Source>,
    notices: Vec<String>,
    incomplete: Vec<Incomplete>,
}

/// One language of the step: its call and what is known of it.
struct Lane<'a> {
    language: usize,
    id: String,
    call: Call<'a>,
    /// The key of the unit of regions, none without regions.
    regions: Option<String>,
}

impl Lane<'_> {
    /// The ids of the units of the call, the unit of regions last.
    fn ids(&self) -> impl Iterator<Item = &str> {
        self.call
            .units
            .iter()
            .map(|unit| unit.id.as_str())
            .chain(self.regions.iter().map(|_| REGIONS_UNIT))
    }

    fn at(&self, id: &str) -> (String, String) {
        (self.id.clone(), id.to_owned())
    }
}

impl Engine {
    /// Sets what the units of each language are made of and groups the inputs
    /// by the unit they are in, languages side by side. A host file's whole
    /// unit is a host unit. Returns the host units and the others.
    pub(super) fn plan_units<'a>(
        &self,
        inputs: &'a [Input],
        hosts: Option<&globset::GlobSet>,
        run: &mut Run,
    ) -> (BTreeMap<usize, Call<'a>>, BTreeMap<usize, Call<'a>>) {
        let (mut host, mut plain) = (BTreeMap::new(), BTreeMap::new());
        let mut guard = self.graph.lock().unwrap_or_else(PoisonError::into_inner);
        let opened = guard.get_or_insert_with(|| self.open_graph());
        let graph = &mut opened.graph;
        for &language in &self.providers {
            match self.group(graph, language, inputs) {
                Ok(units) => {
                    for unit in units {
                        let is_host = hosts.is_some_and(|hosts| {
                            unit.files.iter().any(|i| hosts.is_match(&i.file.path))
                        });
                        let calls = if is_host { &mut host } else { &mut plain };
                        calls
                            .entry(language)
                            .or_insert_with(Call::default)
                            .units
                            .push(unit);
                    }
                }
                Err(e) => run.incomplete.push(Incomplete {
                    path: None,
                    provider: Some(self.provider(language).manifest().id.clone()),
                    reason: format!("the units of the language cannot be listed: {e}"),
                }),
            }
        }
        (host, plain)
    }

    fn group<'a>(
        &self,
        graph: &mut Graph,
        language: usize,
        inputs: &'a [Input],
    ) -> Result<Vec<UnitCall<'a>>, EngineError> {
        let manifest = self.provider(language).manifest();
        let by_path: BTreeMap<String, &Input> = inputs
            .iter()
            .filter(|input| input.language == language)
            .map(|input| (crate::graph::key(&input.file.path), input))
            .collect();
        let id = manifest.id.clone();
        graph.set_input(
            &PROVIDER,
            &id,
            self.provider_of(language),
            Durability::Static,
        )?;
        let claimed: Vec<String> = by_path.keys().cloned().collect();
        graph.set_input(&CLAIMED, &id, claimed, Durability::Volatile)?;
        let units = graph.query(&UNITS, &id)?;
        Ok(units
            .iter()
            .map(|unit| UnitCall {
                id: unit.id.clone(),
                files: unit
                    .files
                    .iter()
                    .filter_map(|p| by_path.get(p).copied())
                    .collect(),
            })
            .collect())
    }

    /// What decides a provider's answers besides the files of a unit.
    fn provider_of(&self, language: usize) -> Provider {
        let provider = self.provider(language);
        let manifest = provider.manifest();
        let mut hasher = Hasher::new();
        let mut part = |text: &str| {
            hasher.update((text.len() as u64).to_le_bytes());
            hasher.update(text);
        };
        part(&crate::cache::build_identity());
        part(&manifest.id);
        part(&provider.identity());
        let options = self.ws.languages.get(&manifest.id);
        part(&serde_json::to_string(&options).unwrap_or_default());
        part(&self.ws.source_globs.join("\n"));
        for name in &manifest.units.env {
            part(&std::env::var(name).unwrap_or_default());
        }
        Provider {
            identity: hasher.hex(),
            by: manifest.units.by.into(),
            manifest: manifest.units.manifest.clone().unwrap_or_default(),
            settings: manifest.units.settings.clone(),
        }
    }

    /// Has each provider index the units it has no answer for, the providers
    /// side by side, and takes the answers of the others from the graph. A
    /// call that fails leaves its files and regions incomplete.
    pub(super) fn index_calls(
        &self,
        ws: &Workspace,
        calls: BTreeMap<usize, Call<'_>>,
        views: &Views,
        parts: &mut Vec<Fragment>,
        mut asts: Option<&mut Vec<lighthouse_protocol::Source>>,
        run: &mut Run,
    ) {
        let mut guard = self.graph.lock().unwrap_or_else(PoisonError::into_inner);
        let opened = guard.get_or_insert_with(|| self.open_graph());
        let graph = &mut opened.graph;
        let mut lanes: Vec<Lane> = calls
            .into_iter()
            .map(|(language, call)| Lane {
                language,
                id: self.provider(language).manifest().id.clone(),
                regions: self.regions_key(language, &call.regions),
                call,
            })
            .collect();
        if let Err(e) = self.settle(ws, graph, &mut lanes, run) {
            run.incomplete.push(Incomplete {
                path: None,
                provider: None,
                reason: format!("the index cannot be kept: {e}"),
            });
        }
        let started = std::time::Instant::now();
        for lane in &lanes {
            let taken = self.fetch(graph, lane, views);
            parts.extend(taken.fragments);
            run.notices.extend(taken.notices);
            run.incomplete
                .extend(taken.incomplete.into_iter().map(|gap| Incomplete {
                    provider: Some(lane.id.clone()),
                    ..gap
                }));
            match asts.as_deref_mut() {
                Some(asts) => asts.extend(taken.sources),
                None if !taken.sources.is_empty() => run.incomplete.push(Incomplete {
                    path: None,
                    provider: Some(lane.id.clone()),
                    reason:
                        "provider unexpectedly returned source ASTs for files that host no regions"
                            .to_owned(),
                }),
                None => {}
            }
        }
        run.timings.fetch += started.elapsed();
    }

    /// Writes what the step set: the answers of the units, under their keys.
    pub(super) fn commit_index(&self, run: &mut Run) {
        let mut guard = self.graph.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(opened) = guard.as_mut()
            && let Err(e) = opened.graph.commit()
        {
            run.notices.insert(format!("the graph was not saved: {e}"));
        }
    }

    /// The key of the unit of regions: what the provider is given of it.
    fn regions_key(&self, language: usize, regions: &[EmbeddedFile]) -> Option<String> {
        if regions.is_empty() {
            return None;
        }
        let mut hasher = Hasher::new();
        let mut part = |bytes: &[u8]| {
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        };
        part(self.provider_of(language).identity.as_bytes());
        for file in regions {
            part(file.file.path.to_string_lossy().as_bytes());
            part(file.file.hash.as_bytes());
            part(&[u8::from(file.file.test)]);
            part(file.text.as_bytes());
            part(&serde_json::to_vec(&file.regions).unwrap_or_default());
        }
        Some(hasher.hex())
    }

    /// Rounds of asking the providers, while a unit is stale.
    fn settle(
        &self,
        ws: &Workspace,
        graph: &mut Graph,
        lanes: &mut [Lane<'_>],
        run: &mut Run,
    ) -> Result<(), EngineError> {
        let mut settled: BTreeSet<(String, String)> = BTreeSet::new();
        let claimed: Vec<std::sync::Arc<Vec<String>>> = lanes
            .iter()
            .map(|lane| Ok(graph.input(&CLAIMED, &lane.id)?.unwrap_or_default()))
            .collect::<Result<_, EngineError>>()?;
        let rounds = 1 + lanes.iter().map(|lane| lane.ids().count()).sum::<usize>();
        for _ in 0..rounds {
            let mut asked: Vec<(usize, Vec<Pending<'_>>)> = Vec::new();
            for (at, lane) in lanes.iter().enumerate() {
                let stale = self.stale(graph, lane, &settled)?;
                if !stale.is_empty() {
                    asked.push((at, stale));
                }
            }
            if asked.is_empty() {
                return Ok(());
            }
            let answers: Vec<(
                Result<Indexed, lighthouse_plugin::Error>,
                std::time::Duration,
            )> = in_pool(|| {
                asked
                    .par_iter()
                    .map(|(at, pending)| {
                        let held: Vec<Unit<'_>> = pending
                            .iter()
                            .map(|p| Unit {
                                id: &p.id,
                                key: &p.key,
                                files: &p.sources,
                                imports: &p.imports,
                            })
                            .collect();
                        let started = std::time::Instant::now();
                        let indexed =
                            self.provider(lanes[*at].language)
                                .index(ws, &held, &claimed[*at]);
                        (indexed, started.elapsed())
                    })
                    .collect()
            });
            for ((at, pending), (indexed, spent)) in asked.iter().zip(answers) {
                let lane = &lanes[*at];
                self.keep(graph, lane, pending, (indexed, spent), &mut settled, run)?;
            }
        }
        Ok(())
    }

    /// The units of the lane that need the provider: those it has no clean
    /// answer for under the key they have now, or whose imports are not what
    /// that answer saw. A unit that was asked and gave nothing clean is not
    /// asked again.
    fn stale<'a>(
        &self,
        graph: &Graph,
        lane: &'a Lane<'_>,
        settled: &BTreeSet<(String, String)>,
    ) -> Result<Vec<Pending<'a>>, EngineError> {
        let mut pending = Vec::new();
        for id in lane.ids() {
            let unit = lane.at(id);
            if settled.contains(&unit) {
                continue;
            }
            let key = self.key_of(graph, lane, id)?;
            let stored = graph.input(&FRAGMENT, &unit)?;
            let mut imports = BTreeMap::new();
            let mut stale = match &stored {
                Some(stored) => {
                    stored.key != key
                        || !stored.clean
                        || stored.reads.iter().any(|read| read.moved(stored.at))
                }
                None => true,
            };
            for (import, seen) in stored.iter().flat_map(|stored| &stored.imports) {
                let now = graph.query(&API, &(lane.id.clone(), import.clone()))?;
                stale |= now.as_str() != seen;
                imports.insert(import.clone(), now.as_ref().clone());
            }
            if stale {
                pending.push(Pending {
                    id: id.to_owned(),
                    key,
                    imports,
                    sources: self.sources_of(lane, id),
                });
            }
        }
        Ok(pending)
    }

    /// The key of a unit of the lane, as hex.
    fn key_of(&self, graph: &Graph, lane: &Lane<'_>, id: &str) -> Result<String, EngineError> {
        if id == REGIONS_UNIT {
            return Ok(lane.regions.clone().unwrap_or_default());
        }
        let key = graph.query(&UNIT, &lane.at(id))?;
        Ok(key.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// What the provider is given of a unit: its files, or the host files of
    /// its regions.
    fn sources_of<'a>(&self, lane: &'a Lane<'_>, id: &str) -> Vec<Source<'a>> {
        if id == REGIONS_UNIT {
            return lane
                .call
                .regions
                .iter()
                .map(|file| Source {
                    file: &file.file,
                    text: &file.text,
                    regions: &file.regions,
                })
                .collect();
        }
        lane.call
            .units
            .iter()
            .find(|unit| unit.id == id)
            .into_iter()
            .flat_map(|unit| &unit.files)
            .map(|input| Source {
                file: &input.file,
                text: &input.text,
                regions: &[],
            })
            .collect()
    }

    /// Records what a provider answered: the notices and gaps in the run, and
    /// each unit as the `fragment` input of the graph.
    fn keep(
        &self,
        graph: &mut Graph,
        lane: &Lane<'_>,
        pending: &[Pending<'_>],
        (indexed, spent): (
            Result<Indexed, lighthouse_plugin::Error>,
            std::time::Duration,
        ),
        settled: &mut BTreeSet<(String, String)>,
        run: &mut Run,
    ) -> Result<(), EngineError> {
        let id = &lane.id;
        run.timings.index.push((id.clone(), spent));
        run.timings.sent.push((id.clone(), pending.len()));
        let mut indexed = match indexed {
            Ok(indexed) => indexed,
            Err(e) => {
                let files: usize = pending.iter().map(|p| p.sources.len()).sum();
                if files > 0 {
                    run.incomplete.push(Incomplete {
                        path: None,
                        provider: Some(id.clone()),
                        reason: format!(
                            "language `{id}` had an execution error, {files} file(s) not analyzed: {e}"
                        ),
                    });
                }
                settled.extend(pending.iter().map(|p| lane.at(&p.id)));
                return Ok(());
            }
        };
        if !indexed.timings.is_empty() {
            run.timings
                .index_phases
                .push((id.clone(), std::mem::take(&mut indexed.timings)));
        }
        let clean = indexed.notices.is_empty() && indexed.incomplete.is_empty();
        run.notices.extend(indexed.notices);
        run.incomplete
            .extend(indexed.incomplete.into_iter().map(|gap| Incomplete {
                provider: Some(id.clone()),
                ..gap
            }));
        let answered: BTreeSet<&str> = indexed.units.iter().map(|u| u.id.as_str()).collect();
        settled.extend(
            pending
                .iter()
                .filter(|p| !clean || !answered.contains(p.id.as_str()))
                .map(|p| lane.at(&p.id)),
        );
        let apis: BTreeMap<&str, &str> = indexed
            .units
            .iter()
            .map(|unit| (unit.id.as_str(), unit.api.as_str()))
            .collect();
        let mut stored = Vec::new();
        for unit in &indexed.units {
            let Some(asked) = pending.iter().find(|p| p.id == unit.id) else {
                continue;
            };
            let imports = unit
                .imports
                .iter()
                .map(|import| (import.clone(), api_now(graph, lane, &apis, import)))
                .collect();
            stored.push((lane.at(&unit.id), stored_of(unit, asked, clean, imports)?));
        }
        for (unit, value) in stored {
            graph.set_input(&FRAGMENT, &unit, value, Durability::Volatile)?;
        }
        Ok(())
    }

    /// The fragments and trees of the lane's units under their current keys,
    /// from the graph, stamped as the providers' answers are.
    fn fetch(&self, graph: &Graph, lane: &Lane<'_>, views: &Views) -> Taken {
        let current = self.files_of(lane);
        let ids: Vec<&str> = lane.ids().collect();
        let held: Vec<Option<std::sync::Arc<Stored>>> = ids
            .iter()
            .map(|id| {
                let key = self.key_of(graph, lane, id).ok()?;
                let stored = graph.input(&FRAGMENT, &lane.at(id)).ok()??;
                (stored.key == key).then_some(stored)
            })
            .collect();
        let decoded: Vec<Payload> = in_pool(|| {
            held.par_iter()
                .flatten()
                .filter_map(|stored| serde_json::from_slice(&stored.fragment).ok())
                .collect()
        });
        let mut taken = Taken::default();
        for payload in decoded {
            for mut fragment in payload.fragments {
                for file in &mut fragment.files {
                    if let Some(now) = current.get(file.path.as_path()) {
                        let facts = std::mem::take(&mut file.facts);
                        // What the host says of a file is read again; `generated` also stays
                        // where the provider said it of a file the host did not (see `stored_of`).
                        let added = file.generated;
                        *file = (*now).clone();
                        file.generated |= added;
                        file.facts = facts;
                    }
                }
                taken.fragments.push(fragment);
            }
            taken.sources.extend(payload.sources);
            taken.notices.extend(payload.notices);
            taken.incomplete.extend(payload.incomplete);
        }
        let provider = self.provider(lane.language);
        views.stamp(&mut taken.fragments, &lane.id);
        if provider.manifest().ordered_declarations {
            for file in taken.fragments.iter_mut().flat_map(|f| &mut f.files) {
                file.facts.insert(ORDERED.to_owned(), String::new());
            }
        }
        taken
    }

    /// The files of the lane's call as the run has them now, by path.
    fn files_of<'a>(&self, lane: &'a Lane<'_>) -> BTreeMap<&'a Path, &'a File> {
        let plain = lane
            .call
            .units
            .iter()
            .flat_map(|unit| &unit.files)
            .map(|input| (input.file.path.as_path(), &input.file));
        let regions = lane
            .call
            .regions
            .iter()
            .map(|file| (file.file.path.as_path(), &file.file));
        plain.chain(regions).collect()
    }
}

/// The `api` of an import once the answers of the round are in: the one a
/// returned unit gave, else the graph's. Of every unit, it is the hash of all.
fn api_now(graph: &Graph, lane: &Lane<'_>, round: &BTreeMap<&str, &str>, import: &str) -> String {
    let unit = |id: &str| match round.get(id) {
        Some(api) => (*api).to_owned(),
        None => graph
            .query(&API, &(lane.id.clone(), id.to_owned()))
            .map(|api| api.as_ref().clone())
            .unwrap_or_default(),
    };
    if import != EVERY {
        return unit(import);
    }
    let mut hasher = blake3::Hasher::new();
    let units = graph.query(&UNITS, &lane.id);
    for listed in units.iter().flat_map(|units| units.iter()) {
        for part in [listed.id.clone(), unit(&listed.id)] {
            hasher.update(&(part.len() as u64).to_le_bytes());
            hasher.update(part.as_bytes());
        }
    }
    hasher.finalize().to_hex().to_string()
}

/// A unit as stored: the answer with the key it was asked under.
fn stored_of(
    unit: &Fragments,
    asked: &Pending<'_>,
    clean: bool,
    imports: Vec<(String, String)>,
) -> Result<Stored, EngineError> {
    // What the host says of a file is read again from the run that fetches the
    // unit; a fragment keeps of `generated` only what the provider added.
    let mut fragments = unit.fragments.clone();
    for file in fragments
        .iter_mut()
        .flat_map(|fragment| &mut fragment.files)
    {
        let host = asked.sources.iter().find(|s| s.file.path == file.path);
        file.generated &= host.is_none_or(|s| !s.file.generated);
    }
    let payload = Payload {
        fragments,
        sources: unit.sources.clone(),
        notices: unit.notices.clone(),
        incomplete: unit.incomplete.clone(),
    };
    Ok(Stored {
        key: asked.key.clone(),
        clean,
        imports,
        body: unit.body.clone(),
        api: unit.api.clone(),
        reads: unit.reads.iter().map(|path| Read::of(path)).collect(),
        at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| {
                i64::try_from(since.as_nanos()).unwrap_or(i64::MAX)
            }),
        fragment: serde_json::to_vec(&payload)
            .map_err(|e| lighthouse_graph::Error::Encode(e.to_string()))?,
    })
}
