//! Adding a project to the `extends` of the configuration file, the way a
//! person would write it: the rest of the file keeps its text.

use std::{fs, path::PathBuf};

use lighthouse_model::write_atomic;
use toml_edit::{Array, DocumentMut, Item, TableLike, Value};

use crate::{Result, config_file};

/// The shareable project that says where the CEL of decisions, packs,
/// projects and recipes is. A project that writes decisions extends it.
pub const AUTHORING: &str = "core/authoring";

/// What asking a project to extend another came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Extended {
    /// The file now extends it.
    Added(PathBuf),
    /// The file already extended it.
    Present,
    /// The file is not TOML, or has no `spec` to put it in: the caller says
    /// what to write.
    Manual(PathBuf),
}

/// Adds `name` to the `extends` of the configuration file in `root`, when it
/// is a TOML file that does not have it yet.
pub fn ensure_extends(root: &std::path::Path, name: &str) -> Result<Extended> {
    let Some(path) = config_file(root) else {
        return Ok(Extended::Manual(root.join(lighthouse_spec::FILE_NAME)));
    };
    if path.extension().is_none_or(|ext| ext != "toml") {
        return Ok(Extended::Manual(path));
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut document: DocumentMut = text
        .parse()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let Some(spec) = document.get_mut("spec").and_then(Item::as_table_like_mut) else {
        return Ok(Extended::Manual(path));
    };
    match extends_of(spec) {
        Some(list) if list.iter().any(|item| item.as_str() == Some(name)) => {
            return Ok(Extended::Present);
        }
        Some(list) => list.push(name),
        None => return Ok(Extended::Manual(path)),
    }
    write_atomic(&path, &document.to_string())?;
    Ok(Extended::Added(path))
}

/// The `extends` list of `spec`, made when it has none; `None` when `extends`
/// is something else.
fn extends_of(spec: &mut dyn TableLike) -> Option<&mut Array> {
    if spec.get("extends").is_none() {
        spec.insert("extends", Item::Value(Value::Array(Array::new())));
    }
    spec.get_mut("extends")?.as_array_mut()
}
