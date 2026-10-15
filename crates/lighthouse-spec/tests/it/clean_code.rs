//! Every Clean Code item has exactly one outcome: a bundled decision derived
//! from it, a reason it does not apply, or the fact (or phase) that unlocks it.

use lighthouse_spec::Catalog;
use std::collections::BTreeSet;

/// The chapter 17 heuristics (Martin, Clean Code, 2008) and the rules of
/// chapters 3 to 13, in the book's order.
const ITEMS: &[&str] = &[
    "C1 Inappropriate Information",
    "C2 Obsolete Comment",
    "C3 Redundant Comment",
    "C4 Poorly Written Comment",
    "C5 Commented-Out Code",
    "E1 Build Requires More Than One Step",
    "E2 Tests Require More Than One Step",
    "F1 Too Many Arguments",
    "F2 Output Arguments",
    "F3 Flag Arguments",
    "F4 Dead Function",
    "G1 Multiple Languages in One Source File",
    "G2 Obvious Behavior Is Unimplemented",
    "G3 Incorrect Behavior at the Boundaries",
    "G4 Overridden Safeties",
    "G5 Duplication",
    "G6 Code at Wrong Level of Abstraction",
    "G7 Base Classes Depending on Their Derivatives",
    "G8 Too Much Information",
    "G9 Dead Code",
    "G10 Vertical Separation",
    "G11 Inconsistency",
    "G12 Clutter",
    "G13 Artificial Coupling",
    "G14 Feature Envy",
    "G15 Selector Arguments",
    "G16 Obscured Intent",
    "G17 Misplaced Responsibility",
    "G18 Inappropriate Static",
    "G19 Use Explanatory Variables",
    "G20 Function Names Should Say What They Do",
    "G21 Understand the Algorithm",
    "G22 Make Logical Dependencies Physical",
    "G23 Prefer Polymorphism to If/Else or Switch/Case",
    "G24 Follow Standard Conventions",
    "G25 Replace Magic Numbers with Named Constants",
    "G26 Be Precise",
    "G27 Structure over Convention",
    "G28 Encapsulate Conditionals",
    "G29 Avoid Negative Conditionals",
    "G30 Functions Should Do One Thing",
    "G31 Hidden Temporal Couplings",
    "G32 Don't Be Arbitrary",
    "G33 Encapsulate Boundary Conditions",
    "G34 Functions Should Descend Only One Level of Abstraction",
    "G35 Keep Configurable Data at High Levels",
    "G36 Avoid Transitive Navigation",
    "J1 Avoid Long Import Lists by Using Wildcards",
    "J2 Don't Inherit Constants",
    "J3 Constants versus Enums",
    "N1 Choose Descriptive Names",
    "N2 Choose Names at the Appropriate Level of Abstraction",
    "N3 Use Standard Nomenclature Where Possible",
    "N4 Unambiguous Names",
    "N5 Use Long Names for Long Scopes",
    "N6 Avoid Encodings",
    "N7 Names Should Describe Side-Effects",
    "T1 Insufficient Tests",
    "T2 Use a Coverage Tool!",
    "T3 Don't Skip Trivial Tests",
    "T4 An Ignored Test Is a Question about an Ambiguity",
    "T5 Test Boundary Conditions",
    "T6 Exhaustively Test Near Bugs",
    "T7 Patterns of Failure Are Revealing",
    "T8 Test Coverage Patterns Can Be Revealing",
    "T9 Tests Should Be Fast",
    "ch. 3 Small functions",
    "ch. 3 Do one thing",
    "ch. 3 One level of abstraction",
    "ch. 3 Switch statements",
    "ch. 3 Command-query separation",
    "ch. 3 No side effects",
    "ch. 3 Prefer exceptions to error codes",
    "ch. 3 Extract try/catch blocks",
    "ch. 3 Error handling is one thing",
    "ch. 3 DRY",
    "ch. 4 Good comments",
    "ch. 4 TODO comments",
    "ch. 4 Redundant and noise comments",
    "ch. 4 Misleading comments",
    "ch. 4 Mandated comments",
    "ch. 4 Journal comments",
    "ch. 4 Position markers",
    "ch. 4 Closing brace comments",
    "ch. 4 Commented-out code",
    "ch. 4 HTML comments",
    "ch. 5 Vertical size and ordering",
    "ch. 5 Vertical openness and density",
    "ch. 5 Vertical distance",
    "ch. 5 Horizontal size",
    "ch. 5 Horizontal openness and indentation",
    "ch. 5 Dummy scopes",
    "ch. 6 Data/object anti-symmetry",
    "ch. 6 Law of Demeter",
    "ch. 6 Hybrids",
    "ch. 6 DTOs, active records",
    "ch. 7 Don't return null",
    "ch. 7 Don't pass null",
    "ch. 7 Provide context with exceptions",
    "ch. 7 Define exceptions by the caller's needs",
    "ch. 8 Use third-party code through boundaries",
    "ch. 8 Learning tests",
    "ch. 9 F.I.R.S.T.",
    "ch. 9 Single concept per test",
    "ch. 9 One assert per test",
    "ch. 9 Clean tests",
    "ch. 10 Classes should be small",
    "ch. 10 Single responsibility and cohesion",
    "ch. 10 Organize for change",
    "ch. 10 Isolate from change",
    "ch. 10 Class organization",
    "ch. 11 Separate construction from use",
    "ch. 11 Dependency injection",
    "ch. 11 Globals, singletons",
    "ch. 12 Beck's four rules",
    "ch. 13 Concurrency: SRP, limit shared data",
    "ch. 13 Keep synchronized sections small",
    "ch. 13 Shared objects and shutdown",
];

const NO_INHERITANCE: &str = "no inheritance in Go or Rust; layers covers the neutral form";
const JAVA_ONLY: &str = "a Java idiom";
const PROCESS: &str = "a process practice, not a property of the code";
const NO_EXCEPTIONS: &str = "Go and Rust return errors; there is no try/catch block to extract";
const NO_HTML_DOCS: &str = "neither language renders HTML in comments";
const NO_MANDATE: &str =
    "documenting every item is the opposite of the rule; missing_docs_in_private_items asks for it";
const NO_DUMMY_SCOPES: &str = "neither language has an empty loop body to stand in for a statement";
const NO_NULL: &str = "Rust has no null to pass";

/// Items that do not apply to Go and Rust projects, with the reason.
const NOT_APPLICABLE: &[(&str, &str)] = &[
    (
        "G7 Base Classes Depending on Their Derivatives",
        NO_INHERITANCE,
    ),
    ("G21 Understand the Algorithm", PROCESS),
    ("J1 Avoid Long Import Lists by Using Wildcards", JAVA_ONLY),
    ("J2 Don't Inherit Constants", JAVA_ONLY),
    ("J3 Constants versus Enums", JAVA_ONLY),
    ("T6 Exhaustively Test Near Bugs", PROCESS),
    ("T7 Patterns of Failure Are Revealing", PROCESS),
    ("T8 Test Coverage Patterns Can Be Revealing", PROCESS),
    ("ch. 3 Extract try/catch blocks", NO_EXCEPTIONS),
    ("ch. 3 Error handling is one thing", NO_EXCEPTIONS),
    ("ch. 4 HTML comments", NO_HTML_DOCS),
    ("ch. 4 Mandated comments", NO_MANDATE),
    ("ch. 5 Dummy scopes", NO_DUMMY_SCOPES),
    ("ch. 7 Don't pass null", NO_NULL),
];

/// The facts and phases that unlock an item no bundled decision covers yet.
/// The fact table is design.md; R5c and R5d are the phases that add new kinds.
const UNLOCKS: &[&str] = &[
    "func.gap",
    "symbol.func",
    "flow",
    "events",
    "external-calls",
    "param-attributes",
    "type-origin",
    "attributes",
    "effects",
    "locals",
    "clones",
    "coverage",
    "R5c",
    "R5d",
];

/// Items waiting on a fact or phase.
const PENDING: &[(&str, &str)] = &[
    ("C1 Inappropriate Information", "R5c"),
    ("C5 Commented-Out Code", "R5c"),
    ("F2 Output Arguments", "param-attributes"),
    ("F3 Flag Arguments", "param-attributes"),
    ("F4 Dead Function", "R5c"),
    ("G1 Multiple Languages in One Source File", "events"),
    ("G3 Incorrect Behavior at the Boundaries", "coverage"),
    ("G9 Dead Code", "R5c"),
    ("G15 Selector Arguments", "param-attributes"),
    ("G19 Use Explanatory Variables", "flow"),
    (
        "G23 Prefer Polymorphism to If/Else or Switch/Case",
        "events",
    ),
    ("G25 Replace Magic Numbers with Named Constants", "events"),
    ("G27 Structure over Convention", "events"),
    ("G28 Encapsulate Conditionals", "flow"),
    ("G29 Avoid Negative Conditionals", "flow"),
    ("G31 Hidden Temporal Couplings", "events"),
    ("G33 Encapsulate Boundary Conditions", "locals"),
    (
        "G35 Keep Configurable Data at High Levels",
        "external-calls",
    ),
    ("G36 Avoid Transitive Navigation", "events"),
    ("N5 Use Long Names for Long Scopes", "locals"),
    ("N6 Avoid Encodings", "R5c"),
    ("N7 Names Should Describe Side-Effects", "effects"),
    ("T2 Use a Coverage Tool!", "coverage"),
    (
        "T4 An Ignored Test Is a Question about an Ambiguity",
        "events",
    ),
    ("ch. 3 Switch statements", "events"),
    ("ch. 3 Command-query separation", "effects"),
    ("ch. 3 No side effects", "effects"),
    ("ch. 3 Prefer exceptions to error codes", "events"),
    ("ch. 4 TODO comments", "R5c"),
    ("ch. 4 Journal comments", "R5c"),
    ("ch. 4 Closing brace comments", "R5c"),
    ("ch. 4 Commented-out code", "R5c"),
    ("ch. 5 Vertical openness and density", "R5c"),
    ("ch. 5 Horizontal size", "R5c"),
    ("ch. 5 Horizontal openness and indentation", "R5c"),
    ("ch. 6 Law of Demeter", "events"),
    ("ch. 6 Hybrids", "R5c"),
    ("ch. 7 Don't return null", "events"),
    (
        "ch. 8 Use third-party code through boundaries",
        "external-calls",
    ),
    ("ch. 8 Learning tests", "external-calls"),
    ("ch. 13 Keep synchronized sections small", "events"),
];

const PROVENANCE_PREFIX: &str = "Martin, Clean Code (2008), ";

/// The items the bundled decisions name in their provenance.
fn covered() -> BTreeSet<&'static str> {
    Catalog::bundled()
        .decisions()
        .flat_map(|d| &d.spec().provenance.was_derived_from)
        .filter_map(|source| source.strip_prefix(PROVENANCE_PREFIX))
        .collect()
}

fn names<'a>(entries: &[(&'a str, &str)]) -> BTreeSet<&'a str> {
    entries.iter().map(|(item, _)| *item).collect()
}

#[test]
fn every_item_has_exactly_one_outcome() {
    let covered = covered();
    let not_applicable = names(NOT_APPLICABLE);
    let pending = names(PENDING);
    for item in ITEMS {
        let outcomes = [
            covered.contains(item),
            not_applicable.contains(item),
            pending.contains(item),
        ];
        let count = outcomes.iter().filter(|held| **held).count();
        assert_eq!(
            count, 1,
            "{item}: covered, not applicable, pending = {outcomes:?}"
        );
    }
}

#[test]
fn the_book_has_no_item_the_list_does_not_name() {
    let items: BTreeSet<_> = ITEMS.iter().copied().collect();
    assert_eq!(items.len(), ITEMS.len(), "an item listed twice");
    let named: Vec<_> = covered()
        .into_iter()
        .chain(names(NOT_APPLICABLE))
        .chain(names(PENDING))
        .filter(|item| !items.contains(item))
        .collect();
    assert!(
        named.is_empty(),
        "outcomes for items the list lacks: {named:?}"
    );
}

#[test]
fn a_pending_item_names_the_fact_or_phase_that_unlocks_it() {
    for (item, unlock) in PENDING {
        assert!(UNLOCKS.contains(unlock), "{item} waits on unknown {unlock}");
    }
}

#[test]
fn a_not_applicable_item_says_why() {
    for (item, reason) in NOT_APPLICABLE {
        assert!(!reason.is_empty(), "{item} has no reason");
    }
}
