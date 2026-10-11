//! Both directions between the clap tree and the MCP tools, and what each
//! tool's schemas are generated from.

use std::collections::BTreeSet;

use clap::Command;
use lighthouse_surface::{
    cli::{command, command_at},
    kinds::kinds,
    tools::{camel_case, tools},
};
use serde_json::Value;

/// Flags a tool does not take: they choose how the CLI prints or what it
/// touches, while a tool always returns the JSON output.
const CLI_ONLY: [&str; 10] = [
    "help",
    "version",
    "format",
    "timings",
    "fix",
    "fixer",
    "color",
    "config",
    "unsafe-fixes",
    "dry-run",
];

/// Housekeeping commands: CLI only, as the design lists them.
const CLI_ONLY_COMMANDS: [&str; 1] = ["review_prune"];

/// The leaf commands of the tree, as paths.
fn leaves(command: &Command, path: &mut Vec<String>, found: &mut Vec<Vec<String>>) {
    let mut any = false;
    for sub in command.get_subcommands().filter(|c| c.get_name() != "help") {
        any = true;
        path.push(sub.get_name().to_owned());
        leaves(sub, path, found);
        path.pop();
    }
    if !any {
        found.push(path.clone());
    }
}

fn leaf_paths() -> Vec<Vec<String>> {
    let mut found = Vec::new();
    leaves(&command(), &mut Vec::new(), &mut found);
    found
}

/// The names a command's arguments have as tool properties: long flags and
/// positionals, in camelCase.
fn argument_names(path: &[&str]) -> BTreeSet<String> {
    command_at(path)
        .get_arguments()
        .filter(|arg| arg.get_long().is_none_or(|long| !CLI_ONLY.contains(&long)))
        .filter(|arg| !CLI_ONLY.contains(&arg.get_id().as_str()))
        .map(|arg| match arg.get_long() {
            Some(long) => camel_case(long),
            None => camel_case(&arg.get_id().as_str().replace('_', "-")),
        })
        .collect()
}

fn properties(schema: &Value) -> BTreeSet<String> {
    schema["properties"]
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn every_command_of_the_loop_has_a_tool_and_every_tool_a_command() {
    let names: BTreeSet<String> = tools().iter().map(|t| t.name.to_owned()).collect();
    for path in leaf_paths() {
        let name = path.join("_");
        assert!(
            names.contains(&name) || CLI_ONLY_COMMANDS.contains(&name.as_str()),
            "`{name}` has no tool"
        );
    }
    let leaves: BTreeSet<String> = leaf_paths().iter().map(|path| path.join("_")).collect();
    for tool in tools() {
        let own = tool.path.join("_");
        assert!(
            leaves.contains(&own),
            "tool `{}` has no command `{own}`",
            tool.name
        );
        assert!(
            tool.name == own || tool.name == "fix",
            "`{}` is not the command path joined by `_`",
            tool.name
        );
    }
}

#[test]
fn input_properties_are_the_long_flags_in_camel_case() {
    for tool in tools() {
        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object", "{}", tool.name);
        assert_eq!(schema["additionalProperties"], false, "{}", tool.name);
        let names = argument_names(tool.path);
        let props = properties(&schema);
        if tool.name == "fix" {
            // `fix` is `check --fix` as its own tool: it takes the fix options.
            assert!(props.contains("fingerprints") && props.contains("dryRun"));
            continue;
        }
        let extra: Vec<_> = props.difference(&names).collect();
        let missing: Vec<_> = names.difference(&props).collect();
        assert!(
            extra.is_empty() && missing.is_empty(),
            "{}: tool-only {extra:?}, flag-only {missing:?}",
            tool.name
        );
    }
}

#[test]
fn enums_and_defaults_are_those_of_the_flags() {
    for tool in tools().iter().filter(|t| t.name != "fix") {
        let schema = tool.input_schema();
        for arg in command_at(tool.path).get_arguments() {
            let Some(long) = arg.get_long() else { continue };
            let Some(prop) = schema["properties"].get(camel_case(long)) else {
                continue;
            };
            let values: Vec<String> = arg
                .get_possible_values()
                .iter()
                .map(|value| value.get_name().to_owned())
                .collect();
            if !values.is_empty() {
                let listed = prop
                    .pointer("/enum")
                    .or_else(|| prop.pointer("/oneOf"))
                    .or_else(|| prop.pointer("/anyOf"))
                    .unwrap_or_else(|| panic!("{} --{long} lists no values: {prop}", tool.name));
                let text = listed.to_string();
                for value in &values {
                    assert!(
                        text.contains(&format!("\"{value}\"")),
                        "{} --{long}: `{value}` missing in {text}",
                        tool.name
                    );
                }
            }
            let defaults = arg.get_default_values();
            if let Some(default) = defaults.first() {
                let schema_default = prop
                    .get("default")
                    .unwrap_or_else(|| panic!("{} --{long} has no default: {prop}", tool.name));
                let schema_default = schema_default
                    .as_str()
                    .map_or_else(|| schema_default.to_string(), str::to_owned);
                assert_eq!(
                    default.to_string_lossy(),
                    schema_default,
                    "{} --{long}",
                    tool.name
                );
            }
        }
    }
}

#[test]
fn every_tool_names_an_output_kind_whose_schema_exists() {
    let kinds = kinds();
    for tool in tools() {
        let schema = kinds
            .get(tool.kind)
            .unwrap_or_else(|| panic!("`{}` has no kind `{}`", tool.name, tool.kind));
        assert_eq!(schema["title"], tool.kind);
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        jsonschema::meta::validate(schema).unwrap_or_else(|e| panic!("{}: {e}", tool.kind));
    }
}

#[test]
fn a_tools_title_and_description_are_its_commands_help() {
    for tool in tools() {
        let (title, description) = tool.help();
        assert!(!title.is_empty(), "{}", tool.name);
        assert!(description.starts_with(title.trim_end_matches('.')) || tool.name == "fix");
    }
}
