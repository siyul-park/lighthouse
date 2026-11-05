//! The model of a run: what the host says of the files is set as inputs of the
//! graph, and the project is pulled from it.

use std::collections::BTreeMap;

use lighthouse_graph::{Durability, Graph};
use lighthouse_spec::key;

use super::{index::Call, regions::Views, *};
use crate::graph::{HOST, Host, MARKERS, PATHS, PROJECT};

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
    /// The project the providers' answers make, from the graph; what goes
    /// wrong is a gap, and the run goes on with no project.
    pub(super) fn assemble(
        &self,
        inputs: &[Input],
        (units, views): (&Units, &Views),
        run: &mut Run,
    ) -> Project {
        let mut guard = self.lock_graph();
        let graph = self.graph_in(&mut guard, &mut run.notices);
        let built = self
            .host_inputs(graph, inputs, (units, views))
            .and_then(|()| Ok(in_pool(|| graph.query(&PROJECT, &()))?));
        match built {
            Ok(project) => (*project).clone(),
            Err(e) => {
                run.incomplete.push(Incomplete {
                    path: None,
                    provider: None,
                    reason: format!("the model cannot be built: {e}"),
                });
                Project::default()
            }
        }
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
            graph.set_input(&HOST, &path, host, Durability::Volatile)?;
            paths.push(path);
        }
        paths.sort();
        graph.set_input(&PATHS, &(), paths, Durability::Volatile)?;
        Ok(())
    }
}
