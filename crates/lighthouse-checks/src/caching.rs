//! How far the facts of a check reach, so that the host can tell
//! what a stored result of a rule depends on. A rule's reach is the greatest
//! of what its expressions mention: a fact or function not listed here is
//! local, and a test fails when the builder or the library has one that is not
//! declared (declare it `Local` to say so).
//!
//! - `Local`: the subject's own symbol, its file's text and the summary of its
//!   function, its module and the kind and name of its owner.
//! - `Neighbors`: the symbols one edge, one owner or one member away, as the
//!   neighbors digest of the cache describes them: identity, kind, visibility,
//!   owner, file, flags and how many callers, callees, references and members
//!   they have, but not where in their file they lie.
//! - `Global`: what is read across the project: indexes over every symbol,
//!   walks through the call graph, test naming and the documents.

use lighthouse_model::Reach;

use crate::library::Needs;

/// The facts of a check that are not local, by the word an expression
/// mentions them with: the derived fields of a value, and the counts of its
/// relations that every symbol has.
pub const FACTS: &[(&str, Reach)] = &[
    ("callers", Reach::Neighbors),
    ("callees", Reach::Neighbors),
    ("references", Reach::Neighbors),
    ("members", Reach::Neighbors),
    ("fan_in", Reach::Neighbors),
    ("fan_out", Reach::Neighbors),
    ("constructor_named", Reach::Local),
    ("built", Reach::Local),
    ("role", Reach::Local),
    ("symbols", Reach::Local),
    // What the directives of the file suppressed: judged after every rule.
    ("annotations", Reach::Global),
    ("cycle", Reach::Global),
    ("comments", Reach::Local),
    ("params", Reach::Local),
    ("results", Reach::Local),
    ("type", Reach::Local),
    ("metrics", Reach::Local),
    ("span", Reach::Local),
    ("words", Reach::Local),
    ("prev", Reach::Local),
    ("next", Reach::Local),
    ("owner", Reach::Neighbors),
    ("uses", Reach::Neighbors),
    ("tests", Reach::Global),
    // A function with the single-use helpers it calls, transitively; how far a
    // symbol reaches the others through what the symbols use.
    ("unit", Reach::Global),
    ("reach", Reach::Global),
    // The declarations of the subject's own file between it and the previous
    // method of its owner; and the function fact a symbol carries as `func`.
    ("gap", Reach::Local),
    ("func", Reach::Local),
    ("line_widths", Reach::Local),
    // The declarations a provider groups into blocks.
    ("blocks", Reach::Local),
    ("inline_bodies", Reach::Local),
    // The source file a test file is named after.
    ("pair", Reach::Global),
    ("data_only", Reach::Neighbors),
    ("receiver_affinity", Reach::Neighbors),
    ("owner_home", Reach::Neighbors),
    ("envy", Reach::Neighbors),
    ("private_reach", Reach::Neighbors),
    // Produced for a file only along with `private_reach`, which decides.
    ("module", Reach::Local),
    ("module_known", Reach::Local),
    ("module_test_of", Reach::Local),
    // The call graph walked past one edge, the test naming convention applied
    // across modules, or an index over every symbol of the project.
    ("forwards_only", Reach::Global),
    ("forward_target", Reach::Global),
    ("touched_by_tests", Reach::Global),
    ("module_tested", Reach::Global),
    ("local_callers", Reach::Global),
    ("param_structs", Reach::Global),
    ("module_imports", Reach::Global),
    // The exported types of other modules that carry the subject's name.
    ("homonyms", Reach::Global),
    // Whether the subject's file holds tests of both case styles at its level.
    ("style_mix", Reach::Local),
    // The types of a module, with the cohesion of their methods; the types
    // that implement an interface, tests included.
    ("types", Reach::Global),
    ("implementers", Reach::Global),
    // How a module is tied to its parent module, by what the symbols of both
    // use of each other.
    ("parent", Reach::Global),
    // How far a module reaches the others through what its symbols use and
    // import.
    ("stability", Reach::Global),
];

/// The fields that say where in its file a value is. The neighbors digest
/// leaves them out, so a check that mentions them is keyed on the digest that
/// has them.
const POSITIONS: [&str; 4] = ["line", "col", "pos", "end_line"];

/// The reach of the expressions, and whether they read the place of
/// neighbors: the greatest reach of the facts they mention and the functions
/// they call.
pub(crate) fn reach(needs: &Needs) -> (Reach, bool) {
    let reach = FACTS
        .iter()
        .filter(|(word, _)| needs.field(word))
        .map(|(_, reach)| *reach)
        .max()
        .unwrap_or(Reach::Local);
    let positions = reach == Reach::Neighbors && POSITIONS.iter().any(|word| needs.field(word));
    (reach, positions)
}
