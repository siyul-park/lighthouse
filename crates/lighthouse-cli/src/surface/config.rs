//! `config show`.

use std::fmt::Write;

use lighthouse_session::environment::{self, Setting};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::surface::{
    Result,
    args::ConfigShowArgs,
    reply::{Context, Format, Reply},
};

/// The `ConfigShow` kind.
#[derive(Debug, Serialize, JsonSchema)]
pub(super) struct ConfigShow {
    /// The environment keys that have a value, sorted by key.
    settings: Vec<Setting>,
}

/// `config show`.
pub fn show(ctx: &Context, args: &ConfigShowArgs, format: Format) -> Result<Reply> {
    let session = ctx.session()?;
    let shown = ConfigShow {
        settings: environment::show(&session, args.origin),
    };
    Reply::data(format, &shown, || {
        let mut out = String::new();
        for setting in &shown.settings {
            if let Some(origin) = &setting.origin {
                let _ = write!(out, "{origin}\t");
            }
            let _ = writeln!(out, "{} = {}", setting.key, toml_value(&setting.value));
        }
        out
    })
}

/// `value` as TOML writes it inline.
fn toml_value(value: &Value) -> String {
    toml::Value::try_from(value).map_or_else(|_| value.to_string(), |value| value.to_string())
}
