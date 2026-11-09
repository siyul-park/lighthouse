//! The model of a run: what the host says of the files is set as inputs of the
//! graph, and the project is pulled from it.

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_graph::{Durability, Graph, revision};
use lighthouse_spec::key;

use super::{index::Call, regions::Views, *};
use lighthouse_model::ucm::{
    HOST, Host, MARKERS, MEMBERS, NOTICES, PATHS, PROJECT, REGIONS_UNIT, register,
};

/// The unit each file of a run is in, and whether the unit is a host unit:
/// one with files that injections take regions from.
pub(super) struct Units(BTreeMap<PathBuf, (String, bool)>);

impl Units {
    pub(super) fn of(
        hosts: &BTreeMap<usize, Call<'_>>,
        others: &BTreeMap<usize, Call<'_>>,
    ) -> Self {
        let mut units = BTreeMap::new();
        for (calls, staged) in [(hosts, true), (others, false)] {
            for unit in calls.values().flat_map(|call| &call.units) {
                for input in &unit.files {
                    units.insert(input.file.path.clone(), (unit.id.clone(), staged));
                }
            }
        }
        Self(units)
    }
}

impl Engine {
    /// What the providers' answers make of the project, as the graph holds
    /// it: the files as the model states them, and what merging them noticed.
    /// What goes wrong is a gap, and the run goes on with no project.
    pub(super) fn stated(
        &self,
        inputs: &[Input],
        (units, views): (&Units, &Views),
        run: &mut Run,
    ) -> Stated {
        let mut guard = self.lock_graph();
        let graph = self.graph_in(&mut guard, &mut run.notices);
        let built = self
            .host_inputs(graph, inputs, (units, views))
            .and_then(|()| {
                let graph: &Graph = graph;
                in_pool(|| {
                    let stated = Stated::of(graph)?;
                    let notices = graph.query(&NOTICES, &())?;
                    Ok::<_, Error>((stated, notices))
                })
            });
        match built {
            Ok((stated, notices)) => {
                run.notices.extend(notices.iter().cloned());
                stated
            }
            Err(e) => {
                run.incomplete.push(Incomplete {
                    path: None,
                    provider: None,
                    reason: format!("the model cannot be built: {e}"),
                });
                Stated::default()
            }
        }
    }

    /// The project the last run read, merged from the graph: what a fix and
    /// the analyses of the whole project work from, which no check does.
    pub fn project(&self) -> Result<Project, Error> {
        let mut guard = self.lock_graph();
        let Some(graph) = guard.as_mut() else {
            return Ok(Project::default());
        };
        let graph: &Graph = graph;
        let project = in_pool(|| graph.query(&PROJECT, &()))?;
        Ok((*project).clone())
    }

    /// The run with the project it read.
    pub(super) fn outcome(&self, mut run: Run) -> Result<Outcome, Error> {
        let project = self.project()?;
        run.revision = self.lock_graph().as_ref().map(revision).unwrap_or_default();
        Ok(Outcome { run, project })
    }

    /// Says what the host knows of each file, and which files there are.
    fn host_inputs(
        &self,
        graph: &mut Graph,
        inputs: &[Input],
        (units, views): (&Units, &Views),
    ) -> Result<(), Error> {
        graph.set_input(
            &MARKERS,
            &(),
            self.generated.markers.patterns().to_vec(),
            Durability::Stable,
        )?;
        let mut paths = Vec::with_capacity(inputs.len());
        let mut members: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for input in inputs {
            let Some((unit, staged)) = units.0.get(&input.file.path) else {
                continue;
            };
            let host = Host {
                file: input.file.clone(),
                unit: unit.clone(),
                staged: *staged,
                views: views.of(&input.file.path),
            };
            let path = key(&input.file.path);
            members
                .entry(input.file.lang.clone())
                .or_default()
                .insert(unit.clone());
            for (view, _) in &host.views {
                members
                    .entry(view.clone())
                    .or_default()
                    .insert(REGIONS_UNIT.to_owned());
            }
            graph.set_input(&HOST, &path, host, Durability::Volatile)?;
            paths.push(path);
        }
        for (language, units) in members {
            let units = units.into_iter().collect();
            graph.set_input(&MEMBERS, &language, units, Durability::Volatile)?;
        }
        paths.sort();
        graph.set_input(&PATHS, &(), paths, Durability::Volatile)?;
        register(graph)?;
        Ok(())
    }
}
