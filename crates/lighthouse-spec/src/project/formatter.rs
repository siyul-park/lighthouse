use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a formatter is given on stdin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FormatterStdin {
    #[default]
    None,
    /// The text of the file.
    File,
}

/// Where a formatter leaves the formatted text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FormatterOutput {
    /// In the scratch copy of the file, which is appended to `argv` (or fills
    /// an `{file}` argument).
    #[default]
    InPlace,
    /// On stdout, which carries only the text.
    Text,
}

/// The command that formats a file of a language, by the command contract of
/// fixes: exit `0` succeeds, stdout carries only the result, stderr is for
/// people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Formatter {
    pub argv: Vec<String>,
    #[serde(default)]
    pub stdin: FormatterStdin,
    #[serde(default)]
    pub output: FormatterOutput,
    /// Extra environment variables; the rest is cleared to an allowlist.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

/// How a formatter is written: `[gofmt, -w]` is the short form (scratch copy,
/// path appended).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum FormatterSpec {
    Argv(Vec<String>),
    Detailed(Formatter),
}

impl From<FormatterSpec> for Formatter {
    fn from(spec: FormatterSpec) -> Self {
        match spec {
            FormatterSpec::Argv(argv) => Self {
                argv,
                stdin: FormatterStdin::None,
                output: FormatterOutput::InPlace,
                env: BTreeMap::new(),
            },
            FormatterSpec::Detailed(formatter) => formatter,
        }
    }
}
