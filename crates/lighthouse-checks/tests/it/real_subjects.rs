//! The query a decision translates to agrees with the decision on real
//! subjects: the source files of its own examples, read by the language
//! plugins the way `decision test` reads them, and this whole repository, under
//! the repository's own configuration and under tightened options. A probe
//! decision of the same scope reports every subject of the decision's kind
//! with the facts its query reads, and the query must hold of exactly the
//! subjects the shipped decision reports.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use lighthouse_engine::{Engine, Run};
use lighthouse_logic::{Const, Op, Operand, Root, Tier, Ucq};
use lighthouse_model::Diagnostic;
use lighthouse_plugin::Registry;
use lighthouse_spec::{Catalog, CheckKind, Config, Decision, Example};
use serde_json::{Map, Value, json};

/// The decisions whose queries real subjects must exercise.
const REQUIRED: [&str; 4] = [
    "core/max-lines",
    "core/max-len",
    "core/no-unused-allow",
    "testing/public-boundary",
];

/// What stands in the message for a field the facts do not have.
const MISSING: &str = "@missing@";

/// The facts of a value a query reads: its fields, the members of a list, and
/// whether only the size of a container is read.
#[derive(Default)]
struct Node {
    fields: BTreeMap<String, Node>,
    members: Option<Box<Node>>,
    read: bool,
    sized: bool,
}

impl Node {
    fn at(&mut self, fields: &[String]) -> &mut Node {
        fields
            .iter()
            .fold(self, |node, f| node.fields.entry(f.clone()).or_default())
    }

    /// The expression that hands out what is read of `base`; `depth` names the
    /// variable of a list's members.
    fn render(&self, base: &str, depth: usize) -> String {
        if !self.fields.is_empty() {
            // A field the facts lack is told apart from a null one.
            let fields: Vec<String> = self
                .fields
                .iter()
                .map(|(name, node)| {
                    let read = node.render(&format!("{base}.{name}"), depth);
                    format!("'{name}': (has({base}.{name}) ? {read} : '{MISSING}')")
                })
                .collect();
            // A space before each brace: `}}` would close the message template.
            return format!("({base} == null ? null : {{{} }})", fields.join(", "));
        }
        if let Some(members) = &self.members {
            let member = format!("m{depth}");
            return format!(
                "({base} == null ? null : {base}.map({member}, {}))",
                members.render(&member, depth + 1)
            );
        }
        if self.sized && !self.read {
            return format!("size({base})");
        }
        if self.read || base.contains('.') {
            return base.to_owned();
        }
        "0".to_owned()
    }

    /// Puts back what `render` reduced to a size: a list of that many items.
    fn expand(&self, value: &mut Value) {
        if !self.fields.is_empty() {
            for (name, node) in &self.fields {
                if value.get(name).and_then(Value::as_str) == Some(MISSING) {
                    value.as_object_mut().map(|o| o.remove(name));
                } else if let Some(child) = value.get_mut(name) {
                    node.expand(child);
                }
            }
        } else if let Some(members) = &self.members {
            for item in value.as_array_mut().into_iter().flatten() {
                members.expand(item);
            }
        } else if self.sized
            && !self.read
            && let Some(n) = value.as_u64()
        {
            *value = Value::Array(vec![json!(0); usize::try_from(n).unwrap_or(0)]);
        }
    }
}

/// Where a path of the query lands in the subject's and the file's facts.
fn locate<'n>(
    path: &lighthouse_logic::Path,
    lists: &BTreeMap<usize, &lighthouse_logic::Path>,
    subject: &'n mut Node,
    file: &'n mut Node,
) -> &'n mut Node {
    match path.root {
        Root::Subject => subject.at(&path.fields),
        Root::File => file.at(&path.fields),
        Root::Binder(id) => {
            let list = locate(lists[&id], lists, subject, file);
            list.members
                .get_or_insert_with(Default::default)
                .at(&path.fields)
        }
    }
}

/// The facts the query reads of the subject and of its file.
fn reads(ucq: &Ucq) -> (Node, Node) {
    let (mut subject, mut file) = (Node::default(), Node::default());
    let lists: BTreeMap<usize, &lighthouse_logic::Path> = ucq
        .cqs
        .iter()
        .flat_map(|cq| &cq.atoms)
        .map(|a| (a.binder, &a.list))
        .collect();
    for cq in &ucq.cqs {
        for atom in &cq.atoms {
            let list = locate(&atom.list, &lists, &mut subject, &mut file);
            list.members.get_or_insert_with(Default::default);
        }
        for (paths, _) in [(&cq.empties, ()), (&cq.filled, ())] {
            for path in paths {
                locate(path, &lists, &mut subject, &mut file).sized = true;
            }
        }
        for cmp in &cq.cmps {
            for side in [&cmp.left, &cmp.right] {
                if let Operand::Path(path) = side {
                    locate(path, &lists, &mut subject, &mut file).read = true;
                }
            }
        }
    }
    (subject, file)
}

/// A translated decision and the probe that hands out its subjects.
struct Probed<'a> {
    decision: &'a Decision,
    ucq: Ucq,
    probe: String,
    subject: Node,
    file: Node,
    /// Whether the decision reports elsewhere than at its subject.
    at: bool,
    /// The options a tightened run sets: those the query compares with a
    /// number, lower, and the flags it reads, set.
    tight: Map<String, Value>,
    /// Whether one of them is a number: a threshold the repository can cross.
    threshold: bool,
}

/// What was compared of a decision: its subjects and those it reports.
#[derive(Default)]
struct Tally {
    subjects: usize,
    violating: usize,
    runs: usize,
}

type Key = (String, u32, Option<String>);

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The formal decisions that translate, with the options a tightened run sets
/// for each.
fn probed(bundled: &Catalog) -> Vec<Probed<'_>> {
    let cel = lighthouse_test_support::cel();
    let mut found = Vec::new();
    for decision in bundled.decisions() {
        let Some(CheckKind::Cel(check)) = decision.check.as_ref().map(|c| &c.kind) else {
            continue;
        };
        let Some(select) = check.selects(decision.scope.subject) else {
            continue;
        };
        if lighthouse_logic::tier(&cel, decision).tier != Tier::F {
            continue;
        }
        let ucq = lighthouse_logic::translate(&cel, check, select.variable())
            .unwrap_or_else(|e| panic!("{}: a formal decision translates: {e}", decision.id()));
        // A finding reported elsewhere than at its subject is told by file,
        // which identifies the subject only when that is a file.
        assert!(
            check.at.is_none() || select.name() == "file",
            "{}: reports at `at` but selects {}, which needs an identity key",
            decision.id(),
            select.name()
        );
        let (subject, file) = reads(&ucq);
        let defaults = decision
            .resolve_options(&Map::new(), None)
            .unwrap_or_default();
        let mut tight = Map::new();
        let mut threshold = false;
        for cq in &ucq.cqs {
            for cmp in &cq.cmps {
                // A threshold goes lower, a flag the code does not set is set.
                for (side, other) in [(&cmp.left, &cmp.right), (&cmp.right, &cmp.left)] {
                    let Operand::Option(fields) = side else {
                        continue;
                    };
                    let Some(name) = fields.first() else { continue };
                    match (defaults.get(name), other) {
                        (Some(Value::Number(n)), _) => {
                            threshold = true;
                            let n = n.as_i64().unwrap_or(0);
                            tight.insert(
                                name.clone(),
                                json!(if n >= 8 { n / 4 } else { (n - 1).max(0) }),
                            );
                        }
                        (Some(Value::Bool(_)), Operand::Const(Const::Bool(b))) => {
                            tight.insert(name.clone(), json!(*b == (cmp.op == Op::Eq)));
                        }
                        _ => {}
                    }
                }
            }
        }
        found.push(Probed {
            decision,
            ucq,
            probe: format!("local/probe-{}", decision.id().replace('/', "-")),
            subject,
            file,
            at: check.at.is_some(),
            tight,
            threshold,
        });
    }
    found
}

/// The decision as a probe: the same scope, options and languages, a check
/// that selects every subject of its kind and reports the facts its query
/// reads (and the options) as the message.
fn probe_text(p: &Probed, name: &str) -> String {
    let Some(CheckKind::Cel(check)) = p.decision.check.as_ref().map(|c| &c.kind) else {
        unreachable!("a probed decision has a cel check");
    };
    let select = check
        .selects(p.decision.scope.subject)
        .expect("a selected subject");
    let variable = select.variable();
    let mut message = format!("'s': {}", p.subject.render(variable, 0));
    if select.scope() == lighthouse_model::RunScope::File {
        message.push_str(&format!(", 'f': {}", p.file.render("file", 0)));
    }
    if p.decision.options.is_some() {
        message.push_str(", 'o': options");
    }
    let mut document = serde_json::to_value(p.decision).expect("a decision is JSON");
    document["metadata"] = json!({ "name": format!("local/{name}") });
    let spec = document["spec"].as_object_mut().expect("a spec");
    for key in ["fix", "supersedes", "provenance", "consequences"] {
        spec.remove(key);
    }
    let mut probe = spec["check"].clone();
    let probe_check = probe.as_object_mut().expect("a check is an object");
    for key in [
        "at",
        "variables",
        "evidence",
        "identity",
        "abstain",
        "prompt",
    ] {
        probe_check.remove(key);
    }
    probe_check.insert("where".to_owned(), json!("true"));
    probe_check.insert(
        "message".to_owned(),
        json!(format!("{{{{ {{ {message} }} }}}}")),
    );
    spec.insert("check".to_owned(), probe);
    for example in spec["examples"].as_array_mut().into_iter().flatten() {
        if let Some(example) = example.as_object_mut() {
            example.remove("fixed");
            example.remove("conflicts");
        }
    }
    document.to_string()
}

fn probe_catalog(probes: &[Probed]) -> Catalog {
    let files = probes
        .iter()
        .map(|p| {
            let name = p.probe.trim_start_matches("local/");
            (format!("{name}.yaml"), probe_text(p, name))
        })
        .collect();
    Catalog::from_local(files, &lighthouse_test_support::cel())
        .unwrap_or_else(|e| panic!("the probes: {e}"))
}

/// Where a finding is, and what it is about: a decision that reports
/// elsewhere than at its subject (`at`) is told by file alone, which the
/// subject of such a decision (a file) has once.
fn place(at: bool, d: &Diagnostic) -> Key {
    if at {
        return (d.file.to_string_lossy().into_owned(), 0, None);
    }
    (
        d.file.to_string_lossy().into_owned(),
        d.span.start.line,
        d.symbol.clone(),
    )
}

fn count(found: impl IntoIterator<Item = Key>) -> BTreeMap<Key, usize> {
    let mut by_place = BTreeMap::new();
    for key in found {
        *by_place.entry(key).or_insert(0) += 1;
    }
    by_place
}

/// Holds the query to the decision on one run, where `p` ran together with
/// its probe.
fn compare(p: &Probed, run: &Run, tally: &mut Tally, what: &str) {
    let id = p.decision.id();
    let expected = count(
        run.diagnostics
            .iter()
            .filter(|d| d.rule_id == id)
            .map(|d| place(p.at, d)),
    );
    let mut subjects = 0;
    let found = count(
        run.diagnostics
            .iter()
            .filter(|d| d.rule_id == p.probe)
            .filter_map(|d| {
                subjects += 1;
                let mut both: Value = serde_json::from_str(&d.message)
                    .unwrap_or_else(|e| panic!("{id}: the probe says {}: {e}", d.message));
                p.subject.expand(&mut both["s"]);
                if let Some(file) = both.get_mut("f") {
                    p.file.expand(file);
                }
                let file = both.get("f").unwrap_or(&Value::Null);
                let options = both.get("o").cloned().unwrap_or_else(|| json!({}));
                p.ucq
                    .holds(&both["s"], file, &options)
                    .then(|| place(p.at, d))
            }),
    );
    tally.subjects += subjects;
    tally.violating += expected.values().sum::<usize>();
    tally.runs += 1;
    assert_eq!(
        found, expected,
        "{id}: the query and the decision differ on {what}\n{}",
        p.ucq
    );
}

/// Level `warn` for the rules, over the plugins listed.
fn rules_toml(plugins: Vec<toml::Value>, rules: &[(String, &Map<String, Value>)]) -> Config {
    let mut spec = toml::Table::new();
    spec.insert("plugins".to_owned(), toml::Value::Array(plugins));
    let mut table = toml::Table::new();
    for (id, options) in rules {
        let mut rule = toml::Table::new();
        rule.insert("level".to_owned(), "warn".into());
        if !options.is_empty() {
            let converted = toml::Value::try_from(options).expect("options are TOML");
            rule.insert("options".to_owned(), converted);
        }
        table.insert(id.clone(), rule.into());
    }
    spec.insert("rules".to_owned(), table.into());
    Config::parse(&lighthouse_test_support::project(
        &toml::to_string(&spec).unwrap(),
    ))
    .unwrap_or_else(|e| panic!("{e}"))
}

/// The plugins that start from `dir`, as a configuration lists them.
fn started(plugins: &[(&str, PathBuf)]) -> Vec<toml::Value> {
    plugins
        .iter()
        .map(|(id, dir)| {
            let mut entry = toml::Table::new();
            entry.insert("id".to_owned(), (*id).into());
            entry.insert("path".to_owned(), dir.to_string_lossy().as_ref().into());
            toml::Value::Table(entry)
        })
        .collect()
}

/// The plugins a registry holds, as a configuration lists them.
fn listed(registry: &Registry) -> Vec<toml::Value> {
    registry
        .manifests()
        .iter()
        .map(|m| toml::Value::String(m.id.clone()))
        .collect()
}

/// The plugins of the languages this machine can build.
fn plugins() -> Vec<(&'static str, PathBuf)> {
    let optional = [
        ("lang-go", lighthouse_test_support::lang_go()),
        (
            "lang-typescript",
            lighthouse_test_support::lang_typescript(),
        ),
        ("lang-python", lighthouse_test_support::lang_python()),
    ];
    let mut all = vec![
        ("lang-rust", lighthouse_test_support::lang_rust()),
        ("lang-yaml", lighthouse_test_support::lang_yaml()),
        ("lang-toml", lighthouse_test_support::lang_toml()),
        ("lang-markdown", lighthouse_test_support::lang_markdown()),
    ];
    all.extend(
        optional
            .into_iter()
            .filter_map(|(id, dir)| Some((id, dir?))),
    );
    all
}

fn languages_of(registry: &Registry) -> Vec<String> {
    registry
        .languages()
        .iter()
        .map(|l| l.provider.manifest().id.clone())
        .collect()
}

fn write(dir: &Path, example: &Example) {
    for file in &example.files {
        let path = dir.join(&file.path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, file.text()).unwrap();
    }
}

fn report(what: &str, tallies: &BTreeMap<String, Tally>, excluded: &[String]) {
    eprintln!("{what}: decision, runs, subjects compared, subjects violating");
    for (id, t) in tallies {
        eprintln!(
            "  {id}: {} runs, {} subjects, {} violating",
            t.runs, t.subjects, t.violating
        );
    }
    eprintln!("{what}, excluded: {excluded:#?}");
}

/// What an example that writes decisions of its own is planted into.
struct Planting<'a, F: Fn() -> Registry> {
    bundled: &'a Catalog,
    bare: &'a F,
    comp: &'a lighthouse_checks::Composition,
}

/// An engine over an example that writes decisions of its own, which the
/// search for contradictions between decisions reads: the probe of the
/// decision is planted with them, in a registry that has no probes.
fn planted_engine<F: Fn() -> Registry>(
    planting: &Planting<'_, F>,
    p: &Probed,
    example: &Example,
    mut files: lighthouse_spec::Files,
    dir: &Path,
) -> Engine {
    let id = p.decision.id();
    let name = p.probe.trim_start_matches("local/");
    files.insert(format!("{name}.yaml"), probe_text(p, name));
    let (registry, catalog) =
        lighthouse_checks::planted(planting.comp, (planting.bare)(), planting.bundled, files)
            .unwrap_or_else(|e| panic!("{id} {}: {e}", example.name));
    let none = Map::new();
    let mut ruled: Vec<(String, &Map<String, Value>)> = catalog
        .decisions()
        .filter(|d| d.id().starts_with("local/") && d.id() != p.probe)
        .map(|d| (d.id().to_owned(), &none))
        .collect();
    ruled.push((id.to_owned(), &example.options));
    ruled.push((p.probe.clone(), &example.options));
    let config = rules_toml(listed(&registry), &ruled);
    let cel = planting.comp.cel().clone();
    let orders =
        std::sync::Arc::new(move |d: &Decision| lighthouse_logic::tier(&cel, d).tier == Tier::O);
    let conflicts = lighthouse_engine::Conflicts::new(std::sync::Arc::new(catalog.clone()), orders);
    Engine::new(registry, config, &catalog, dir)
        .unwrap_or_else(|e| panic!("{id} {}: {e}", example.name))
        .with_conflicts(conflicts)
}

#[test]
fn the_query_holds_of_the_subjects_the_decision_reports_on_the_source_of_its_examples() {
    let bundled = lighthouse_test_support::bundled();
    let probes = probed(bundled);
    let catalog = Catalog::overlay(bundled, &probe_catalog(&probes)).unwrap();
    let comp = lighthouse_test_support::composition();
    let plugins = plugins();
    let base = || {
        lighthouse_checks::registry_of(&comp, &catalog, lighthouse_test_support::libraries())
            .unwrap_or_else(|e| panic!("{e}"))
    };
    let startup = rules_toml(started(&plugins), &[]);
    let pool = lighthouse_test_support::pooled(base, &startup);
    let known = languages_of(&pool());
    let bare = std::cell::LazyCell::new(|| {
        lighthouse_test_support::pooled(
            || {
                lighthouse_checks::registry_of(&comp, bundled, lighthouse_test_support::libraries())
                    .unwrap_or_else(|e| panic!("{e}"))
            },
            &startup,
        )
    });
    let mut tallies = BTreeMap::<String, Tally>::new();
    let mut skipped = Vec::new();
    for p in &probes {
        let id = p.decision.id();
        for example in &p.decision.examples {
            if example.language != "text" && !known.contains(&example.language) {
                skipped.push(format!(
                    "{id} {}: no {} plugin",
                    example.name, example.language
                ));
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            write(dir.path(), example);
            let files = lighthouse_engine::local_decisions(example);
            let reads = pool()
                .rule(&p.probe)
                .is_some_and(|r| !r.reads_documents().is_empty());
            let engine = if reads && !files.is_empty() {
                planted_engine(
                    &Planting {
                        bundled,
                        bare: &*bare,
                        comp: &comp,
                    },
                    p,
                    example,
                    files,
                    dir.path(),
                )
            } else {
                let registry = pool();
                let config = rules_toml(
                    listed(&registry),
                    &[
                        (id.to_owned(), &example.options),
                        (p.probe.clone(), &example.options),
                    ],
                );
                Engine::new(registry, config, &catalog, dir.path())
                    .unwrap_or_else(|e| panic!("{id} {}: {e}", example.name))
            };
            let run = engine
                .check(&[], &[id.to_owned(), p.probe.clone()])
                .unwrap_or_else(|e| panic!("{id} {}: {e}", example.name));
            assert!(
                run.incomplete.is_empty(),
                "{id} {}: {:?}",
                example.name,
                run.incomplete
            );
            compare(
                p,
                &run,
                tallies.entry(id.to_owned()).or_default(),
                &format!(
                    "the files of its example {} ({})",
                    example.name, example.language
                ),
            );
        }
    }
    report("examples", &tallies, &skipped);
    for id in REQUIRED {
        assert!(
            probes.iter().any(|p| p.decision.id() == id),
            "{id} is not formal"
        );
    }
    for p in &probes {
        let id = p.decision.id();
        let t = tallies
            .get(id)
            .unwrap_or_else(|| panic!("{id}: no example ran"));
        assert!(t.subjects > 0, "{id}: the examples have no subject");
        let trivial = p.ucq.cqs.is_empty() || p.ucq.cqs.iter().any(|cq| *cq == Default::default());
        assert!(
            trivial || t.violating > 0,
            "{id}: the examples show no violating subject"
        );
    }
}

/// The repository's `lighthouse.toml` for a run that holds each probe to its
/// decision: the probe takes the level, the options and the path overrides the
/// decision has, so the two are asked the same thing. With `tight`, both are
/// enabled with the options set lower than the code keeps. The decisions off
/// for the whole repository are left out, and each path an override turns a
/// decision off for is listed.
fn repository_config(
    plugins: &[(&str, PathBuf)],
    in_process: &[String],
    probes: &[Probed],
    projects: &lighthouse_spec::Projects,
    tight: bool,
    excluded: &mut Vec<String>,
) -> (Config, Vec<usize>) {
    let text = fs::read_to_string(workspace().join("lighthouse.toml")).unwrap();
    let mut doc: toml::Table = text.parse().unwrap();
    let mut listed: Vec<toml::Value> = in_process
        .iter()
        .map(|id| toml::Value::String(id.clone()))
        .collect();
    listed.extend(started(plugins));
    doc["spec"]
        .as_table_mut()
        .unwrap()
        .insert("plugins".to_owned(), toml::Value::Array(listed));
    let base = Config::parse(&doc.to_string())
        .unwrap_or_else(|e| panic!("{e}"))
        .resolve(Path::new("none/none.txt"), "text", projects)
        .unwrap_or_else(|e| panic!("{e}"));
    let spec = doc["spec"].as_table_mut().unwrap();
    let mut run = Vec::new();
    let mut rules = spec
        .remove("rules")
        .and_then(|r| r.try_into::<toml::Table>().ok())
        .unwrap_or_default();
    for (at, p) in probes.iter().enumerate() {
        let id = p.decision.id();
        let configured = base.get(id);
        let mut options = configured.map(|c| c.options.clone()).unwrap_or_default();
        if tight {
            options.extend(p.tight.clone());
            set(&mut rules, id, "warn", &options);
            set(&mut rules, &p.probe, "warn", &options);
        } else if let Some(level) = configured.and_then(|c| c.level) {
            let level = serde_json::to_value(level).unwrap();
            set(&mut rules, &p.probe, level.as_str().unwrap(), &options);
        } else {
            excluded.push(format!("{id}: off for the repository in lighthouse.toml"));
            continue;
        }
        run.push(at);
    }
    spec.insert("rules".to_owned(), toml::Value::Table(rules));
    for over in spec
        .get_mut("overrides")
        .and_then(|o| o.as_array_mut())
        .into_iter()
        .flatten()
    {
        let Some(table) = over.as_table_mut() else {
            continue;
        };
        let files = table.get("files").cloned();
        let Some(rules) = table.get_mut("rules").and_then(|r| r.as_table_mut()) else {
            continue;
        };
        for &at in &run {
            let id = probes[at].decision.id();
            let Some(value) = rules.get(id).cloned() else {
                continue;
            };
            if value.as_str() == Some("off") {
                excluded.push(format!("{id}: off for {files:?}"));
            }
            rules.insert(probes[at].probe.clone(), value);
        }
    }
    let config = Config::parse(&doc.to_string()).unwrap_or_else(|e| panic!("{e}"));
    (config, run)
}

fn set(rules: &mut toml::Table, id: &str, level: &str, options: &Map<String, Value>) {
    let mut rule = toml::Table::new();
    rule.insert("level".to_owned(), level.into());
    if !options.is_empty() {
        rule.insert(
            "options".to_owned(),
            toml::Value::try_from(options).expect("options are TOML"),
        );
    }
    rules.insert(id.to_owned(), rule.into());
}

/// One pass over the whole repository: the decisions and their probes, asked
/// under one configuration.
fn pass(
    name: &str,
    tight: bool,
    full: &Catalog,
    probes: &[Probed],
    tallies: &mut BTreeMap<String, Tally>,
) {
    let catalog = Catalog::overlay(full, &probe_catalog(probes)).unwrap();
    let plugins = plugins();
    let comp = lighthouse_test_support::composition();
    let mut registry =
        lighthouse_checks::registry_of(&comp, &catalog, lighthouse_test_support::libraries())
            .unwrap_or_else(|e| panic!("{e}"));
    let in_process: Vec<String> = registry.manifests().iter().map(|m| m.id.clone()).collect();
    let mut excluded = Vec::new();
    let (config, run) = repository_config(
        &plugins,
        &in_process,
        probes,
        catalog.projects(),
        tight,
        &mut excluded,
    );
    let root = workspace().canonicalize().unwrap();
    lighthouse_rpc::register(&mut registry, &config, &root, &root, &[]).unwrap();
    let rules: Vec<String> = run
        .iter()
        .flat_map(|&at| {
            [
                probes[at].decision.id().to_owned(),
                probes[at].probe.clone(),
            ]
        })
        .collect();
    let outcome = Engine::new(registry, config, &catalog, &root)
        .unwrap_or_else(|e| panic!("{name}: {e}"))
        .check(&[], &rules)
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(
        outcome.incomplete.is_empty(),
        "{name}: {:?}",
        outcome.incomplete
    );
    for &at in &run {
        let p = &probes[at];
        compare(
            p,
            &outcome,
            tallies.entry(p.decision.id().to_owned()).or_default(),
            &format!("the subjects of this repository ({name})"),
        );
    }
    excluded.sort();
    excluded.dedup();
    report(name, tallies, &excluded);
}

#[test]
#[ignore = "indexes this whole repository twice: `make decisions` runs it in release"]
fn the_query_holds_of_the_subjects_the_decision_reports_on_this_repository() {
    let full = lighthouse_test_support::full();
    let probes = probed(full);
    let (mut own, mut tight) = (BTreeMap::new(), BTreeMap::new());
    pass("repository's configuration", false, full, &probes, &mut own);
    pass("tightened options", true, full, &probes, &mut tight);
    for p in &probes {
        let id = p.decision.id();
        if p.threshold {
            let t = tight.get(id).unwrap_or_else(|| panic!("{id}: not asked"));
            assert!(
                t.violating > 0,
                "{id}: no subject violates it with {:?}; the query was not exercised",
                p.tight
            );
        }
    }
}
