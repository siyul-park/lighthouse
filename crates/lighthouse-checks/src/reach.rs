//! How far the facts and functions of a check reach, so that the host can tell
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
    ("signature", Reach::Local),
    ("type_ref", Reach::Local),
    ("file_first_test", Reach::Local),
    ("data_only", Reach::Neighbors),
    ("helper_user", Reach::Neighbors),
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
    ("documented_by_interface", Reach::Global),
    ("hidden_target", Reach::Global),
    ("touched_by_tests", Reach::Global),
    ("module_tested", Reach::Global),
    ("local_callers", Reach::Global),
    ("effective", Reach::Global),
    ("homonyms", Reach::Global),
];

/// The functions of the library, by name.
pub const FUNCTIONS: &[(&str, Reach)] = &[
    ("metrics", Reach::Local),
    ("callers", Reach::Neighbors),
    ("callees", Reach::Neighbors),
    ("owner", Reach::Neighbors),
    ("edges", Reach::Neighbors),
    ("tests", Reach::Global),
    ("annotations", Reach::Local),
    // The order keys of this crate rank a declaration by its own file and
    // owner.
    ("rank", Reach::Local),
    ("limit", Reach::Local),
    ("counted", Reach::Local),
    ("exposed", Reach::Local),
    ("globMatch", Reach::Local),
    ("layerOf", Reach::Local),
    ("lines", Reach::Local),
    ("trim", Reach::Local),
    ("join", Reach::Local),
    ("trimPrefixes", Reach::Local),
    ("trimSuffixes", Reach::Local),
    ("trimLeft", Reach::Local),
    ("trimRight", Reach::Local),
    ("leadingRun", Reach::Local),
    ("words", Reach::Local),
    ("drop", Reach::Local),
];

/// The fields that say where in its file a value is. The neighbors digest
/// leaves them out, so a check that reads them of a neighbor reaches the
/// project.
const POSITIONS: [&str; 4] = ["line", "col", "pos", "end_line"];

/// The reach of the expressions: the greatest reach of the facts they
/// mention and the functions they call.
pub(crate) fn of(needs: &Needs) -> Reach {
    let facts = FACTS.iter().filter(|(word, _)| needs.mentions(word));
    let functions = FUNCTIONS.iter().filter(|(name, _)| needs.calls(name));
    let reach = facts
        .chain(functions)
        .map(|(_, reach)| *reach)
        .max()
        .unwrap_or(Reach::Local);
    if reach >= Reach::Neighbors && POSITIONS.iter().any(|word| needs.mentions(word)) {
        Reach::Global
    } else {
        reach
    }
}
