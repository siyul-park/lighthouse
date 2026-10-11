//! The help of the loop commands: short summaries, and examples where the
//! design asks for them.

use lighthouse_surface::cli::{command, command_at, help};

/// The longest summary `--help` lists next to a command.
const SHORT_HELP_LIMIT: usize = 80;

fn walk(
    command: &clap::Command,
    path: &mut Vec<String>,
    visit: &mut dyn FnMut(&[String], &clap::Command),
) {
    for sub in command.get_subcommands().filter(|c| c.get_name() != "help") {
        path.push(sub.get_name().to_owned());
        visit(path, sub);
        walk(sub, path, visit);
        path.pop();
    }
}

#[test]
fn every_summary_is_at_most_eighty_characters() {
    walk(&command(), &mut Vec::new(), &mut |path, command| {
        let about = command
            .get_about()
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(!about.is_empty(), "{path:?} has no summary");
        assert!(
            about.chars().count() <= SHORT_HELP_LIMIT,
            "{path:?}: {} characters: {about}",
            about.chars().count()
        );
        assert!(!about.contains('\n'), "{path:?}");
    });
}

#[test]
fn the_commands_the_design_names_carry_examples() {
    for path in [
        &["check"][..],
        &["review", "resolve"],
        &["decision", "eval"],
        &["model", "train"],
    ] {
        let (_, long) = help(path);
        assert!(
            long.contains("Examples") && long.contains("lighthouse "),
            "{path:?} has no examples"
        );
        assert!(command_at(path).get_after_long_help().is_some());
    }
}
