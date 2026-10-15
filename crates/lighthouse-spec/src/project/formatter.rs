use std::collections::BTreeMap;

use lighthouse_process::{Formatter, FormatterOutput, FormatterStdin};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
