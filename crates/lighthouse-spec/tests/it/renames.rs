//! Decisions renamed: the old ids still answer, and the ids a decision was
//! renamed from are its own.

use lighthouse_spec::Catalog;
use lighthouse_test_support::catalog::*;

#[test]
fn a_renamed_decision_still_answers_to_its_old_ids() {
    let catalog = Catalog::bundled();

    let now = catalog.decision("core/max-lines").unwrap();
    assert_eq!(now.was_names().collect::<Vec<_>>(), ["core/max-file-lines"]);
    assert_eq!(
        catalog.decision("core/max-file-lines").unwrap().id(),
        "core/max-lines"
    );
    assert_eq!(catalog.aliases()["core/max-file-lines"], "core/max-lines");
    assert_eq!(
        catalog.identities()["core/max-file-lines"],
        now.uid().unwrap()
    );
    assert!(catalog.decision("core/nonesuch").is_none());
}

#[test]
fn config_rename_rules_moves_the_settings_of_old_ids_to_the_new_ones() {
    let mut config = lighthouse_spec::Config::parse_inline(
        "plugins = [\"core\"]\n[rules]\n\"core/max-file-lines\" = { level = \"warn\", options = { max = 5 } }\n[[overrides]]\nfiles = [\"a/**\"]\nrules = { \"core/max-file-lines\" = \"off\" }\n",
    )
    .unwrap();

    let renamed = config.rename_rules(&Catalog::bundled().aliases());

    assert_eq!(
        renamed,
        [lighthouse_spec::Renamed {
            old: "core/max-file-lines".to_owned(),
            new: "core/max-lines".to_owned(),
            both: false,
        }]
    );
    let ids: Vec<_> = config.configured().map(|(id, _)| id).collect();
    assert_eq!(ids, ["core/max-lines", "core/max-lines"]);
}

#[test]
fn config_that_sets_the_old_and_the_new_id_keeps_the_new_one() {
    let mut config = lighthouse_spec::Config::parse_inline(
        "plugins = [\"core\"]\n[rules]\n\"core/max-file-lines\" = \"off\"\n\"core/max-lines\" = \"error\"\n",
    )
    .unwrap();

    let renamed = config.rename_rules(&Catalog::bundled().aliases());

    assert!(renamed[0].both);
    let kept: Vec<_> = config
        .configured()
        .map(|(id, c)| (id.to_owned(), c.level))
        .collect();
    assert_eq!(
        kept,
        [(
            "core/max-lines".to_owned(),
            Some(lighthouse_model::Severity::Error)
        )]
    );
}

#[test]
fn projects_rename_the_rules_of_extended_layers_too() {
    let project = "apiVersion: lighthouse/v1alpha1\nkind: Project\nmetadata:\n  name: team/base\nspec:\n  rules:\n    core/max-file-lines: warn\n";
    let mut files = lighthouse_test_support::catalog::Files::new();
    files.insert("team.yaml".to_owned(), project.to_owned());
    let local = Catalog::from_local(files).unwrap();
    let mut projects = Catalog::overlay(Catalog::bundled(), &local)
        .unwrap()
        .projects()
        .unwrap();

    let renamed = projects.rename_rules(&Catalog::bundled().aliases());

    assert_eq!(renamed.len(), 1);
    assert_eq!(renamed[0].new, "core/max-lines");
    let ids: Vec<_> = projects
        .get("team/base")
        .unwrap()
        .configured()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, ["core/max-lines"]);
}

mod renamed_from {
    use super::*;

    fn named(id: &str, was: &str) -> String {
        let annotation = if was.is_empty() {
            String::new()
        } else {
            format!("  annotations:\n    lighthouse/was-names: {was}\n")
        };
        decision(id, "s", SPEC).replace("\nspec:\n", &format!("\n{annotation}spec:\n"))
    }

    fn two(a: String, b: String) -> Files {
        files(&[
            ("p/pack.yaml", pack("p", &[("s", &["a", "b"])])),
            ("p/s/a.yaml", a),
            ("p/s/b.yaml", b),
        ])
    }

    #[test]
    fn a_name_a_decision_was_renamed_from_belongs_to_it_alone() {
        let own = Catalog::from_files(with("p/s/a.yaml", &named("p/a", "p/a"))).unwrap_err();
        assert!(own.to_string().contains("is its own id"), "{own}");

        let live = Catalog::from_files(two(named("p/a", "p/b"), named("p/b", ""))).unwrap_err();
        assert!(
            live.to_string()
                .contains("is the id of a decision that exists"),
            "{live}"
        );

        let both =
            Catalog::from_files(two(named("p/a", "p/old"), named("p/b", "p/old"))).unwrap_err();
        assert!(both.to_string().contains("lists too"), "{both}");

        assert!(
            Catalog::from_files(two(named("p/a", "p/old, p/older"), named("p/b", "p/x"))).is_ok()
        );
    }

    #[test]
    fn an_id_that_is_live_is_never_an_alias() {
        // A layer that renames `p/a` away and then a new decision takes the
        // name: the overlay is invalid, but nothing resolves through it.
        let catalog = Catalog::from_files(two(named("p/a", "p/old"), named("p/b", ""))).unwrap();

        assert_eq!(catalog.aliases()["p/old"], "p/a");
        assert!(catalog.decision("p/b").is_some());
        assert_eq!(catalog.decision("p/old").unwrap().id(), "p/a");
    }
}
