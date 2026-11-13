//! Shapes of declarations and their arrangements: the small files an oracle
//! search renders to see whether the order decisions of a configuration can
//! all hold. A shape is a set of declarations with visibility and the edges
//! between them; an arrangement is an order of its declarations in the file.
//! Shapes are enumerated by increasing size and counted once up to renaming.

use std::collections::BTreeSet;

use lighthouse_spec::Decision;
use serde_json::{Map, Value};

mod build;
mod scratch;
mod text;

pub use scratch::{Scratch, Unit};

/// The largest bound the enumeration offers. It grows with the factorial of
/// the size, and the oracle's claim is only ever "no witness up to the bound".
pub const MAX_BOUND: usize = 3;

/// The prefix of the decisions that read test files: the alphabet then holds
/// tests that name a declaration.
const TESTING_PREFIX: &str = "testing/";

/// The hook names Go can render with a signature `go vet` accepts.
const GO_HOOKS: [&str; 4] = ["Clone", "String", "GoString", "Error"];

/// A set of declarations and the edges between them. Declarations are in a
/// canonical order and edges are sorted, so equal shapes are equal values.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Shape {
    pub decls: Vec<Decl>,
    pub edges: Vec<Edge>,
}

/// One declaration of a shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Decl {
    pub kind: Kind,
    pub visibility: Visibility,
    /// A test in the test file names it as its target. The test counts toward
    /// the size of the shape.
    pub tested: bool,
}

/// What one declaration is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Type,
    Func,
    /// A method of a type; the shape holds an [`Relation::Owner`] edge to it.
    Method,
    /// The constructor of a type (Go `func NewT() *T`, Rust `fn new() -> Self`),
    /// owned by it.
    Constructor,
    /// Go: the option builder of a type (`func WithT() func(*T)`), owned by it.
    Option,
    /// A hook of a type: Go an exported method that implements clone,
    /// conversion or an interface (`String`), Rust an impl of a trait. Owned
    /// by the type.
    Hook,
    /// Go: an `init` function.
    Init,
    Const,
    Var,
}

/// Whether a declaration is visible outside its package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Visibility {
    Public,
    Private,
}

/// A directed relation between two declarations of a shape, by index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub relation: Relation,
}

/// How one declaration relates to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Relation {
    /// A method belongs to a type.
    Owner,
    /// A type names another as a field, a function, method, constant or
    /// variable names one as its type.
    Uses,
    /// A function or method calls a function, a method of its own type, or
    /// reads a constant or variable.
    Calls,
}

/// An order of the declarations of a shape in its source file, and of the
/// tests that name them in its test file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Arrangement {
    /// Declaration indices, in source order.
    pub source: Vec<usize>,
    /// Indices of the tested declarations, in test order.
    pub tests: Vec<usize>,
}

/// A language the search renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Go,
    Rust,
}

/// Which kinds of declaration a search draws on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alphabet {
    /// Test functions that name a target, for decisions that read test files.
    pub tests: bool,
    /// The name a hook of a type carries, when hooks are in the alphabet. One
    /// representative hook per language suffices because the hooks of a type
    /// share one rank; if an option ever orders hooks among themselves, each
    /// needs its own kind.
    pub hook: Option<&'static str>,
}

/// Why no verdict can be given for a decision in a language: the caller
/// reports `incomplete` with this reason, never silence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incomplete {
    /// No renderer exists for the language.
    NoRenderer(String),
    /// The decision orders a group of declarations the language's alphabet
    /// cannot produce, so no bound over the alphabet is a claim about it.
    Alphabet {
        decision: String,
        language: String,
        group: String,
    },
    /// The options of the decision do not resolve.
    Options { decision: String, reason: String },
    /// The bound is above [`MAX_BOUND`].
    Bound(usize),
}

impl Language {
    /// The renderer of the language with this id.
    pub fn of(id: &str) -> Result<Self, Incomplete> {
        match id {
            "go" => Ok(Self::Go),
            "rust" => Ok(Self::Rust),
            _ => Err(Incomplete::NoRenderer(id.to_owned())),
        }
    }

    /// The id of the language.
    pub fn id(self) -> &'static str {
        match self {
            Self::Go => "go",
            Self::Rust => "rust",
        }
    }

    /// The groups of `design/declaration-groups` the alphabet can produce.
    pub fn groups(self) -> &'static [&'static str] {
        match self {
            Self::Go => &[
                "public-type",
                "private-type",
                "type",
                "public-const",
                "private-const",
                "const",
                "var",
                "init",
                "method",
                "public-function",
                "private-function",
                "private",
            ],
            Self::Rust => &[
                "public-type",
                "private-type",
                "type",
                "public-const",
                "private-const",
                "const",
                "var",
                "method",
                "public-function",
                "private-function",
                "private",
            ],
        }
    }
}

impl Alphabet {
    /// The alphabet for the decisions of a cell, each with the options it is
    /// configured with: tests are in it when a `testing/*` decision is among
    /// them, and a hook when the effective `hooks` of a decision name one the
    /// language can render. A decision that orders a group, or a hook, the
    /// alphabet cannot produce makes the whole cell incomplete.
    pub fn of(
        language: Language,
        cell: &[(&Decision, &Map<String, Value>)],
    ) -> Result<Self, Incomplete> {
        let mut alphabet = Self {
            tests: false,
            // The hook of a Rust type is an impl of a trait, whatever the options say.
            hook: (language == Language::Rust).then_some("Drop"),
        };
        for (decision, configured) in cell {
            let id = decision.id();
            let options = decision
                .resolve_options(configured, Some(language.id()))
                .map_err(|error| Incomplete::Options {
                    decision: id.to_owned(),
                    reason: error.to_string(),
                })?;
            let missing = |group: &str| Incomplete::Alphabet {
                decision: id.to_owned(),
                language: language.id().to_owned(),
                group: group.to_owned(),
            };
            alphabet.tests |= id.starts_with(TESTING_PREFIX);
            if let Some(group) = names(&options, "groups")
                .into_iter()
                .find(|group| !language.groups().contains(group))
            {
                return Err(missing(group));
            }
            let hooks = names(&options, "hooks");
            if !hooks.is_empty() {
                // Only Go renders a named hook; a Rust trait impl stands for
                // the hooks of a language whose `hooks` is empty.
                let renderable = hooks
                    .iter()
                    .find_map(|hook| GO_HOOKS.iter().find(|go| *go == hook));
                alphabet.hook = match (language, renderable) {
                    (Language::Go, Some(hook)) => Some(hook),
                    _ => return Err(missing("hooks")),
                };
            }
        }
        Ok(alphabet)
    }

    /// The declaration labels of the language, in order.
    fn labels(self, language: Language) -> Vec<Decl> {
        use Kind::{Const, Constructor, Func, Hook, Init, Method, Option, Type, Var};
        use Visibility::{Private, Public};
        let mut kinds = vec![
            (Type, Public),
            (Type, Private),
            (Func, Public),
            (Func, Private),
            (Method, Public),
            (Method, Private),
            (Constructor, Public),
            (Constructor, Private),
            (Const, Public),
            (Const, Private),
            (Var, Public),
            (Var, Private),
        ];
        if language == Language::Go {
            kinds.extend([(Option, Public), (Option, Private), (Init, Private)]);
        }
        if self.hook.is_some() {
            kinds.push((Hook, Public));
        }
        let mut labels = Vec::new();
        for (kind, visibility) in kinds {
            for tested in [false, true] {
                let decl = Decl {
                    kind,
                    visibility,
                    tested,
                };
                if !tested || (self.tests && decl.testable()) {
                    labels.push(decl);
                }
            }
        }
        labels
    }
}

impl Kind {
    /// A declaration that belongs to a type and is linked to it.
    pub fn owned(self) -> bool {
        matches!(
            self,
            Self::Method | Self::Constructor | Self::Option | Self::Hook
        )
    }

    /// A declaration with a body that calls other declarations.
    pub fn acts(self) -> bool {
        matches!(
            self,
            Self::Func | Self::Method | Self::Constructor | Self::Init
        )
    }
}

impl Decl {
    /// The tests only name what they can call from outside the package.
    fn testable(self) -> bool {
        self.visibility == Visibility::Public && matches!(self.kind, Kind::Func | Kind::Method)
    }

    fn size(self) -> usize {
        1 + usize::from(self.tested)
    }
}

impl Shape {
    /// Declarations and tests together.
    pub fn size(&self) -> usize {
        self.decls.iter().map(|decl| decl.size()).sum()
    }

    /// Every distinct order of the declarations and of the tests: orders that
    /// render the same text under the renaming of equal declarations are one.
    pub fn arrangements(&self) -> Vec<Arrangement> {
        let tested: Vec<usize> = (0..self.decls.len())
            .filter(|&i| self.decls[i].tested)
            .collect();
        let tests = permutations(tested.len());
        let mut seen = BTreeSet::new();
        let mut all = Vec::new();
        for source in permutations(self.decls.len()) {
            for order in &tests {
                let arrangement = Arrangement {
                    source: source.clone(),
                    tests: order.iter().map(|&t| tested[t]).collect(),
                };
                if seen.insert(self.layout(&arrangement)) {
                    all.push(arrangement);
                }
            }
        }
        all
    }

    /// The shape read in the order of `arrangement`, with positions for
    /// indices: what two arrangements must share to render the same text.
    fn layout(&self, arrangement: &Arrangement) -> (Vec<Decl>, Vec<Edge>, Vec<usize>) {
        let mut position = vec![0; self.decls.len()];
        for (at, &decl) in arrangement.source.iter().enumerate() {
            position[decl] = at;
        }
        let decls = arrangement.source.iter().map(|&i| self.decls[i]).collect();
        let mut edges: Vec<Edge> = self
            .edges
            .iter()
            .map(|edge| Edge {
                from: position[edge.from],
                to: position[edge.to],
                relation: edge.relation,
            })
            .collect();
        edges.sort();
        let tests = arrangement.tests.iter().map(|&i| position[i]).collect();
        (decls, edges, tests)
    }
}

/// Every shape of up to `bound` declarations (tests included) once up to
/// renaming, smallest first: fewest declarations, then fewest edges, then in
/// the order of the values.
pub fn enumerate(
    language: Language,
    alphabet: Alphabet,
    bound: usize,
) -> Result<Vec<Shape>, Incomplete> {
    if bound > MAX_BOUND {
        return Err(Incomplete::Bound(bound));
    }
    let mut multisets = Vec::new();
    collect_multisets(
        &alphabet.labels(language),
        0,
        bound,
        &mut Vec::new(),
        &mut multisets,
    );
    let shapes: BTreeSet<Shape> = multisets
        .iter()
        .flat_map(|decls| shapes_of(decls))
        .collect();
    let mut shapes: Vec<Shape> = shapes.into_iter().collect();
    shapes.sort_by_key(|shape| (shape.size(), shape.edges.len()));
    Ok(shapes)
}

/// Every non-empty multiset of labels that fits `budget`, each as a sorted
/// list.
fn collect_multisets(
    labels: &[Decl],
    from: usize,
    budget: usize,
    current: &mut Vec<Decl>,
    out: &mut Vec<Vec<Decl>>,
) {
    for (i, &label) in labels.iter().enumerate().skip(from) {
        if label.size() > budget {
            continue;
        }
        current.push(label);
        out.push(current.clone());
        collect_multisets(labels, i, budget - label.size(), current, out);
        current.pop();
    }
}

/// The distinct shapes over these declarations, one per edge set up to the
/// renaming of equal declarations.
fn shapes_of(decls: &[Decl]) -> BTreeSet<Shape> {
    let renamings = renamings(decls);
    build::edge_sets(decls)
        .iter()
        .map(|edges| Shape {
            decls: decls.to_vec(),
            edges: canonical(&renamings, edges),
        })
        .collect()
}

/// The permutations of the declarations that map each to one of the same
/// label.
fn renamings(decls: &[Decl]) -> Vec<Vec<usize>> {
    permutations(decls.len())
        .into_iter()
        .filter(|permutation| (0..decls.len()).all(|i| decls[permutation[i]] == decls[i]))
        .collect()
}

/// The least sorted edge list over the renamings.
fn canonical(renamings: &[Vec<usize>], edges: &[Edge]) -> Vec<Edge> {
    renamings
        .iter()
        .map(|renaming| {
            let mut renamed: Vec<Edge> = edges
                .iter()
                .map(|edge| Edge {
                    from: renaming[edge.from],
                    to: renaming[edge.to],
                    relation: edge.relation,
                })
                .collect();
            renamed.sort();
            renamed
        })
        .min()
        .unwrap_or_default()
}

/// The strings of the array option `key`, none when it is not one.
fn names<'a>(options: &'a Map<String, Value>, key: &str) -> Vec<&'a str> {
    options
        .get(key)
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// Every order of `0..n`.
fn permutations(n: usize) -> Vec<Vec<usize>> {
    if n == 0 {
        return vec![Vec::new()];
    }
    permutations(n - 1)
        .into_iter()
        .flat_map(|rest| {
            (0..=rest.len()).map(move |at| {
                let mut order = rest.clone();
                order.insert(at, n - 1);
                order
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
