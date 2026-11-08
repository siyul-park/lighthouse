//! The model of the project as queries over what the providers said.
//!
//! The host says what it knows of each file (`host`) and which files make the
//! project (`paths`). From the provider's answers for the units the files are
//! in, `part` makes the fragments of a file as the host stamps them, `slice`
//! qualifies them, `declared` is the modules a file declares and `declarations` what
//! the table takes of a slice, `symbols` is the table of all declarations,
//! `resolved` the slice with its targets resolved against it, `places` and
//! `edges_in` the parts of it that a read of the whole project takes of a file,
//! `edges` the reverse maps of all edges, `sites` where they refer from and
//! `incoming` what the rest of the project says of the symbols of one file. `project` puts the files
//! together; it is held in memory only.
//!
//! Each kind is a function of the kinds it reads: an edit that changes
//! nothing a node reads, such as a body in `symbols`, leaves it equal and what
//! read it is not run again.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use lighthouse_graph::{Durability, Error, Graph, Input, Query};
use serde::{Deserialize, Serialize};

use super::{
    Declarations, Edge, Edges, File, Fragment, Implemented, Incoming, Markers, Methods, Node,
    ORDERED, Place, Project, REGIONS, Resolved, Settled, Sites, Slice, Symbol, SymbolId, Table,
    TestCase, ambiguity, collisions, declared, merge::duplicate_notice, methods,
};

/// The id of the unit that holds the regions selected in the project's files.
pub const REGIONS_UNIT: &str = "#regions";

/// What the host says of a file of the project. `Volatile`.
pub static HOST: Input<String, Host> = Input::new("host");
/// The languages that evaluate their declarations in the order they are
/// written, by language id. `Static`.
pub static ORDERED_BY: Input<String, bool> = Input::new("ordered_by");
/// The fragments of the unit `(language, id)` as the provider gave them, and
/// its source trees as JSON. Persisted and read only by what builds the model
/// of a file, which validates without loading.
pub static PAYLOAD: Input<(String, String), Payload> = Input::new("payload").persisted();
/// The units of a language that have a model in the run, by language id,
/// sorted. `Volatile`.
pub static MEMBERS: Input<String, Vec<String>> = Input::new("members");
/// The paths of the files of the project, sorted. `Volatile`.
pub static PATHS: Input<(), Vec<String>> = Input::new("paths");
/// The patterns of the markers of generated files. `Stable`.
pub static MARKERS: Input<(), Vec<String>> = Input::new("markers");

/// What the provider of a unit `(language, id)` said of the method sets of its
/// types and interfaces.
pub static METHODS: Query<(String, String), Methods> = Query::new("methods", methods::methods);
/// Which type of a language satisfies which interface of it, and which methods
/// implement one, by unit.
pub static IMPLEMENTS: Query<String, Implemented> = Query::new("implements", methods::implements);
/// The part of `implements` that concerns the unit `(language, id)`: an edit
/// elsewhere that leaves it as it was leaves the files of the unit green.
pub static SETTLED: Query<(String, String), Settled> = Query::new("settled", methods::settled);
pub(crate) static PART: Query<String, Vec<Fragment>> = Query::new("part", part);
pub(crate) static DECLARED: Query<String, Vec<(String, String)>> = Query::new("declared", declare);
/// The module paths declared in several languages, from every file.
pub static COLLISIONS: Query<(), BTreeSet<String>> = Query::new("collisions", collide);
/// The fragments of a file, qualified.
pub static SLICE: Query<String, Slice> = Query::new("slice", slice);
/// What a file declares, with no position or doc: equal after an edit inside a
/// body.
pub static DECLARATIONS: Query<String, Declarations> = Query::new("declarations", declarations);
/// The ids that several files declare, which the first of them owns: the only
/// ids for which what a file owns is not what it declares.
pub static DUPLICATED: Query<(), BTreeSet<SymbolId>> = Query::new("duplicated", duplicated);
/// The table of all declarations.
pub static SYMBOLS: Query<(), Table> = Query::new("symbols", symbols);
/// A slice with its targets resolved against the table.
pub static RESOLVED: Query<String, Resolved> = Query::new("resolved", resolved);
/// Where the symbols of a file are, with what the file says of each: what a
/// read of the position of a declaration takes of the file, so that an edit of
/// a comment or a body that moves nothing leaves it equal.
pub static PLACES: Query<String, Vec<Place>> = Query::new("places", places);
/// The edges of a file in the order they stand, each with the file the fragment
/// about it is about when it starts at a module: what a read of the edges of the
/// project takes of the file.
pub static EDGES_IN: Query<String, Vec<(Edge, Option<PathBuf>)>> = Query::new("edges_in", edges_in);
/// The reverse maps of all edges.
pub static EDGES: Query<(), Edges> = Query::new("edges", edges);
/// Where the edges refer from.
pub static SITES: Query<(), Sites> = Query::new("sites", sites);
/// What the rest of the project says of the symbols of one file.
pub static INCOMING: Query<String, Incoming> = Query::new("incoming", incoming);
/// The tests of every module, by module path, one per symbol; held in memory
/// only.
pub static CASES: Query<(), BTreeMap<String, Vec<TestCase>>> =
    Query::new("cases", cases).transient();
/// The tests of one module: a projection of `cases`, so that an edit that
/// leaves them as they were leaves what read them green; held in memory only.
pub static MODULE_CASES: Query<String, Vec<TestCase>> =
    Query::new("module_cases", module_cases).transient();
/// What putting the files together drops or leaves undecided, for the user
/// to see: the ids declared in several files and the targets that stayed
/// ambiguous.
pub static NOTICES: Query<(), Vec<String>> = Query::new("notices", notices);
/// The files put together; held in memory only.
pub static PROJECT: Query<(), Project> = Query::new("project", project).transient();

/// The fragments of a unit, its source trees as JSON and the method sets of
/// its types, which the host settles with those of the other units.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payload {
    pub fragments: Vec<Fragment>,
    pub sources: Vec<u8>,
    pub methods: Methods,
}

/// How the host stamps the fragments a provider gave for a file.
struct Stamping<'a> {
    graph: &'a Graph,
    host: &'a Host,
    path: &'a Path,
}

impl Stamping<'_> {
    /// The fragments of the unit whose first file is this one, as stored, with
    /// what settling the method sets of the unit decided for them.
    fn of(&self, language: &str, unit: &str) -> Result<Vec<Fragment>, Error> {
        let at = (language.to_owned(), unit.to_owned());
        let payload = self.graph.input(&PAYLOAD, &at)?.unwrap_or_default();
        let mut parts: Vec<Fragment> = payload
            .fragments
            .iter()
            .filter(|fragment| fragment.files.first().is_some_and(|f| f.path == self.path))
            .cloned()
            .collect();
        methods::settle(self.graph, &at, &payload, &mut parts)?;
        Ok(parts)
    }

    /// What the host says of a file is read again; `generated` also stays where
    /// the provider said it of a file the host did not.
    fn restamp(&self, fragment: &mut Fragment) -> Result<(), Error> {
        for file in &mut fragment.files {
            let Some(now) = self.graph.input(&HOST, &key(&file.path))? else {
                continue;
            };
            let facts = std::mem::take(&mut file.facts);
            let added = file.generated;
            file.clone_from(&now.file);
            file.generated |= added;
            file.facts = facts;
        }
        Ok(())
    }

    /// The fact of a language that evaluates its declarations in order.
    fn ordered(&self, fragment: &mut Fragment, language: &str) -> Result<(), Error> {
        let ordered = self
            .graph
            .input(&ORDERED_BY, &language.to_owned())?
            .is_some_and(|ordered| *ordered);
        if ordered {
            for file in &mut fragment.files {
                file.facts.insert(ORDERED.to_owned(), String::new());
            }
        }
        Ok(())
    }

    /// What the provider of the host unit left out of its own files: the
    /// language of the regions, and the regions themselves.
    fn host(&self, fragments: &mut [Fragment]) {
        let native = fragments.iter().any(has_content);
        for file in fragments.iter_mut().flat_map(|f| &mut f.files) {
            if file.path != self.path {
                continue;
            }
            if let Some(language) = self.host.analysis_language().filter(|_| !native) {
                file.facts
                    .insert("analysisLanguage".to_owned(), language.to_owned());
            }
            if let Some(regions) = self.host.regions() {
                file.facts.insert(REGIONS.to_owned(), regions);
            }
        }
    }

    /// States on what the provider of `language` returned for the regions of
    /// the file what the host knows: the language of the modules they
    /// declare, whether they are example code, and that example code has no
    /// tests to own.
    fn region(&self, fragment: &mut Fragment, language: &str, example: bool) {
        for module in &mut fragment.modules {
            module.language = Some(language.to_owned());
            module.example = example;
        }
        if example {
            fragment.tests.clear();
        }
        for file in &mut fragment.files {
            if let Some(regions) = self.host.regions() {
                file.facts.insert(REGIONS.to_owned(), regions);
            }
        }
    }
}

/// What the host says of a file: the record it keeps, the unit the file is
/// in, and what the injections made of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub file: File,
    /// The id of its unit, in the language of the file.
    pub unit: String,
    /// Its unit is a host unit: one that holds files regions are taken from.
    pub staged: bool,
    /// The languages its regions were routed to, in the order of the
    /// providers, each with whether the regions are example code.
    pub views: Vec<(String, bool)>,
}

impl Host {
    /// Sets the inputs of the model of the given fragments, one unit per file,
    /// with nothing else known of the files.
    pub fn set_units(graph: &mut Graph, fragments: &[Fragment]) -> Result<(), Error> {
        let mut paths = Vec::new();
        let mut members: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for fragment in fragments {
            let Some(file) = fragment.files.first() else {
                continue;
            };
            let (language, path) = (file.lang.clone(), key(&file.path));
            let unit = (language, path.clone());
            let payload = Payload {
                fragments: vec![fragment.clone()],
                sources: Vec::new(),
                methods: Methods::default(),
            };
            let host = Self {
                file: file.clone(),
                unit: path.clone(),
                staged: false,
                views: Vec::new(),
            };
            graph.set_input(&PAYLOAD, &unit, payload, Durability::Volatile)?;
            graph.set_input(&HOST, &path, host, Durability::Volatile)?;
            members.entry(unit.0).or_default().push(path.clone());
            paths.push(path);
        }
        for (language, units) in members {
            graph.set_input(&MEMBERS, &language, units, Durability::Volatile)?;
        }
        paths.sort();
        graph.set_input(&PATHS, &(), paths, Durability::Volatile)?;
        graph.set_input(&MARKERS, &(), Vec::new(), Durability::Stable)?;
        Ok(())
    }

    /// The fact that lists the regions of the file, see `File::regions`.
    fn regions(&self) -> Option<String> {
        let mut views: Vec<&(String, bool)> = self.views.iter().collect();
        views.sort();
        let listed: Vec<String> = views
            .iter()
            .map(|(language, example)| {
                format!("{language}:{}", if *example { "example" } else { "code" })
            })
            .collect();
        (!listed.is_empty()).then(|| listed.join(","))
    }

    /// The language a file with no code model of its own is analyzed as: the
    /// one language of the code it holds. Example code is not the file's
    /// language, however much of it there is.
    fn analysis_language(&self) -> Option<&str> {
        let mut code = self
            .views
            .iter()
            .filter(|(_, example)| !example)
            .map(|(language, _)| language.as_str());
        let language = code.next()?;
        code.next().is_none().then_some(language)
    }
}

/// The key of a project-relative path in the kinds that take one.
pub fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Makes the queries of the model known to `graph` before anything pulls them,
/// so that a node stored as having read one is validated by name.
///
/// # Errors
///
/// A kind is registered with another type.
pub fn register(graph: &Graph) -> Result<(), Error> {
    METHODS.register(graph)?;
    IMPLEMENTS.register(graph)?;
    SETTLED.register(graph)?;
    PART.register(graph)?;
    DECLARED.register(graph)?;
    COLLISIONS.register(graph)?;
    SLICE.register(graph)?;
    DECLARATIONS.register(graph)?;
    DUPLICATED.register(graph)?;
    SYMBOLS.register(graph)?;
    RESOLVED.register(graph)?;
    PLACES.register(graph)?;
    EDGES_IN.register(graph)?;
    EDGES.register(graph)?;
    SITES.register(graph)?;
    INCOMING.register(graph)?;
    CASES.register(graph)?;
    MODULE_CASES.register(graph)?;
    NOTICES.register(graph)?;
    PROJECT.register(graph)
}

/// The fragments of a file, as the host stamps them: the ones of its unit,
/// then the ones of the regions routed to other languages, each stamped with
/// what the host knows of the file.
fn part(graph: &Graph, path: &String) -> Result<Vec<Fragment>, Error> {
    let Some(host) = graph.input(&HOST, path)? else {
        return Ok(Vec::new());
    };
    let stamping = Stamping {
        graph,
        host: &host,
        path: Path::new(path),
    };
    let language = &host.file.lang;
    let mut parts = stamping.of(language, &host.unit)?;
    for part in &mut parts {
        stamping.restamp(part)?;
        stamping.ordered(part, language)?;
    }
    if host.staged {
        stamping.host(&mut parts);
    }
    for (view, example) in &host.views {
        for mut part in stamping.of(view, REGIONS_UNIT)? {
            stamping.restamp(&mut part)?;
            stamping.region(&mut part, view, *example);
            stamping.ordered(&mut part, view)?;
            parts.push(part);
        }
    }
    let markers = graph.input(&MARKERS, &())?.unwrap_or_default();
    let markers = Markers::shared(&markers).map_err(|e| Error::Encode(e.to_string()))?;
    for part in &mut parts {
        markers.mark(part);
    }
    Ok(parts)
}

/// The module paths a file declares, each with the language it declares it in.
fn declare(graph: &Graph, path: &String) -> Result<Vec<(String, String)>, Error> {
    let parts = graph.query(&PART, path)?;
    Ok(parts.iter().flat_map(declared).collect())
}

/// The module paths declared in several languages, from every file.
fn collide(graph: &Graph, _: &()) -> Result<BTreeSet<String>, Error> {
    let found = each(graph, |graph, path| graph.query(&DECLARED, path))?;
    Ok(collisions(
        found.iter().flat_map(|found| found.iter().cloned()),
    ))
}

fn slice(graph: &Graph, path: &String) -> Result<Slice, Error> {
    let parts = graph.query(&PART, path)?;
    let collisions = graph.query(&COLLISIONS, &())?;
    Ok(Slice::new(
        PathBuf::from(path),
        parts.as_ref().clone(),
        &collisions,
    ))
}

fn declarations(graph: &Graph, path: &String) -> Result<Declarations, Error> {
    let slice = graph.query(&SLICE, path)?;
    Ok(Declarations::new(&slice))
}

fn duplicated(graph: &Graph, _: &()) -> Result<BTreeSet<SymbolId>, Error> {
    let declared = each(graph, |graph, path| graph.query(&DECLARATIONS, path))?;
    let mut seen = BTreeSet::new();
    let mut twice = BTreeSet::new();
    for symbol in declared.iter().flat_map(|declared| &declared.symbols) {
        if !seen.insert(&symbol.id) {
            twice.insert(symbol.id.clone());
        }
    }
    Ok(twice)
}

/// The table of the symbols of every file.
fn symbols(graph: &Graph, _: &()) -> Result<Table, Error> {
    let declared = each(graph, |graph, path| graph.query(&DECLARATIONS, path))?;
    Ok(Table::new(declared.iter().map(Arc::as_ref)))
}

fn resolved(graph: &Graph, path: &String) -> Result<Resolved, Error> {
    let slice = graph.query(&SLICE, path)?;
    let table = graph.query(&SYMBOLS, &())?;
    Ok(Resolved::new(&slice, &table))
}

fn places(graph: &Graph, path: &String) -> Result<Vec<Place>, Error> {
    let resolved = graph.query(&RESOLVED, path)?;
    Ok(resolved.slice.fragment.places.clone())
}

fn edges_in(graph: &Graph, path: &String) -> Result<Vec<(Edge, Option<PathBuf>)>, Error> {
    let resolved = graph.query(&RESOLVED, path)?;
    let mut files = resolved
        .slice
        .runs
        .iter()
        .flat_map(|(len, file)| std::iter::repeat_n(file.as_ref(), *len));
    Ok(resolved
        .slice
        .fragment
        .edges
        .iter()
        .map(|edge| {
            let file = files.next().flatten();
            let origin = file.filter(|_| matches!(edge.from, Node::Module(_)));
            (edge.clone(), origin.cloned())
        })
        .collect())
}

fn edges(graph: &Graph, _: &()) -> Result<Edges, Error> {
    let table = graph.query(&SYMBOLS, &())?;
    let resolved = each(graph, |graph, path| graph.query(&RESOLVED, path))?;
    Ok(Edges::new(&table, resolved.iter().map(Arc::as_ref)))
}

fn sites(graph: &Graph, _: &()) -> Result<Sites, Error> {
    let table = graph.query(&SYMBOLS, &())?;
    let resolved = each(graph, |graph, path| graph.query(&RESOLVED, path))?;
    Ok(Sites::new(&table, resolved.iter().map(Arc::as_ref)))
}

fn incoming(graph: &Graph, path: &String) -> Result<Incoming, Error> {
    let resolved = graph.query(&RESOLVED, path)?;
    let table = graph.query(&SYMBOLS, &())?;
    let edges = graph.query(&EDGES, &())?;
    Ok(Incoming::new(&resolved, &table, &edges.reverse))
}

fn project(graph: &Graph, _: &()) -> Result<Project, Error> {
    let resolved = each(graph, |graph, path| graph.query(&RESOLVED, path))?;
    let edges = graph.query(&EDGES, &())?;
    let sites = graph.query(&SITES, &())?;
    Ok(Project::assemble(
        resolved.iter().map(Arc::as_ref),
        (&edges, &sites),
    ))
}

fn notices(graph: &Graph, _: &()) -> Result<Vec<String>, Error> {
    let declared = each(graph, |graph, path| graph.query(&DECLARATIONS, path))?;
    let mut symbols: Vec<&Symbol> = declared.iter().flat_map(|d| &d.symbols).collect();
    symbols.sort_by(|a, b| a.id.cmp(&b.id));
    let mut notices: Vec<String> = duplicate_notice(&symbols).into_iter().collect();
    let ambiguous: usize = each(graph, |graph, path| graph.query(&RESOLVED, path))?
        .iter()
        .map(|resolved| resolved.ambiguous)
        .sum();
    if ambiguous > 0 {
        notices.push(ambiguity(ambiguous));
    }
    Ok(notices)
}

fn cases(graph: &Graph, _: &()) -> Result<BTreeMap<String, Vec<TestCase>>, Error> {
    let resolved = each(graph, |graph, path| graph.query(&RESOLVED, path))?;
    let mut all: Vec<&TestCase> = resolved
        .iter()
        .flat_map(|resolved| &resolved.slice.fragment.tests)
        .collect();
    all.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    all.dedup_by(|a, b| a.symbol == b.symbol);
    let mut by_module: BTreeMap<String, Vec<TestCase>> = BTreeMap::new();
    for case in all {
        by_module
            .entry(case.symbol.module().to_owned())
            .or_default()
            .push(case.clone());
    }
    Ok(by_module)
}

fn module_cases(graph: &Graph, module: &String) -> Result<Vec<TestCase>, Error> {
    let cases = graph.query(&CASES, &())?;
    Ok(cases.get(module).cloned().unwrap_or_default())
}

/// A query of every path of the project, fanned out.
fn each<T: Send + 'static>(
    graph: &Graph,
    run: fn(&Graph, &String) -> Result<T, Error>,
) -> Result<Vec<T>, Error> {
    let paths = paths(graph)?.as_ref().clone();
    graph.par_map(paths, run).into_iter().collect()
}

/// The paths of the project.
fn paths(graph: &Graph) -> Result<Arc<Vec<String>>, Error> {
    Ok(graph.input(&PATHS, &())?.unwrap_or_default())
}

/// Whether a fragment says anything of its file besides that it exists.
fn has_content(fragment: &Fragment) -> bool {
    !fragment.modules.is_empty()
        || !fragment.symbols.is_empty()
        || !fragment.places.is_empty()
        || !fragment.edges.is_empty()
        || !fragment.functions.is_empty()
        || !fragment.tests.is_empty()
        || !fragment.comments.is_empty()
}
