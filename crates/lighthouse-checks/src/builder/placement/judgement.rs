//! The judgement of one function: the candidates of each level and the level
//! that is wrong.

use std::{collections::BTreeMap, path::Path};

use lighthouse_model::ucm::{Model, Symbol, SymbolId};
use serde_json::{Value, json};

use super::{MIN_GAIN, MIN_USES, uses::Uses};

pub(super) struct Judgement {
    pub(super) settled: bool,
    pub(super) level: &'static str,
    pub(super) current_distance: f64,
    pub(super) candidates: Vec<Candidate>,
    pub(super) owner: Option<Owner>,
    pub(super) file: Option<FileTarget>,
    /// The module the function belongs in; where it is when it stays.
    pub(super) target: String,
    /// The single-use helpers that move along with it.
    pub(super) unit: Vec<SymbolId>,
}

pub(super) struct Candidate {
    pub(super) module: String,
    pub(super) behavior: usize,
    pub(super) distance: f64,
    pub(super) reason: Option<String>,
}

pub(super) struct Owner {
    pub(super) id: SymbolId,
    pub(super) via: &'static str,
}

/// The file of the module the function belongs in.
pub(super) struct FileTarget {
    pub(super) file: String,
    pub(super) uses: usize,
}

impl Judgement {
    pub(super) fn new(home: &str) -> Self {
        Self {
            settled: false,
            level: "",
            current_distance: 1.0,
            candidates: Vec::new(),
            owner: None,
            file: None,
            target: home.to_owned(),
            unit: Vec::new(),
        }
    }

    /// The module and file the function would go to.
    pub(super) fn destination(&self, symbol: &Symbol) -> (String, std::path::PathBuf) {
        let file = self
            .file
            .as_ref()
            .map_or_else(|| symbol.file.clone(), |f| f.file.clone().into());
        (self.target.clone(), file)
    }

    /// The coarsest level at which the function is in the wrong place: its
    /// module, or the type it is a method of in all but name. The fact names as
    /// `level` only the owner, which a decision on by default reports, because
    /// a level stands for the order findings of the function. A wrong module
    /// (`move`) and file (`file.misfiled`) are data of the fact for the
    /// decisions that a model answers.
    pub(super) fn coarsest(&mut self) -> &'static str {
        let moved = self
            .candidates
            .first()
            .filter(|c| c.reason.is_none() && c.behavior >= MIN_USES)
            .filter(|c| self.current_distance - c.distance >= MIN_GAIN)
            .map(|c| c.module.clone());
        if self.settled {
            return "";
        }
        if let Some(module) = moved {
            self.target = module;
            return "module";
        }
        if self.owner.as_ref().is_some_and(|o| o.via == "callers") {
            return "owner";
        }
        ""
    }

    /// Whether the file it would go to is another one than where it is.
    fn misfiled(&self, file: &Path) -> bool {
        !self.settled
            && self.level.is_empty()
            && self
                .file
                .as_ref()
                .is_some_and(|f| Path::new(&f.file) != file)
    }

    pub(super) fn value(&self, symbol: &Symbol) -> Value {
        json!({
            "level": if self.level == "module" { "" } else { self.level },
            "move": self.level == "module",
            "file": self.file.as_ref().map_or_else(|| json!({}), |f| json!({
                "misfiled": self.misfiled(&symbol.file),
                "uses": f.uses,
            })),
        })
    }
}

/// The placement of a symbol that is no free function to place: every key, no
/// level.
pub(super) fn absent() -> Value {
    json!({ "level": "", "move": false, "file": {} })
}

/// The file of `target` whose symbols the function uses most, and how many of
/// its uses lie in that file; the file of the owner type when there is one.
/// Never a new file.
pub(super) fn pick_file(
    project: &dyn Model,
    uses: &Uses,
    (target, home, file): (&str, &str, &Path),
    judgement: &Judgement,
) -> Option<FileTarget> {
    if let Some(owner) = &judgement.owner {
        let ty = project.symbol(&owner.id)?;
        return Some(FileTarget {
            uses: 0,
            file: ty.file.to_string_lossy().into_owned(),
        });
    }
    let mut files: BTreeMap<&Path, usize> = BTreeMap::new();
    for used in uses.by_module.get(target).into_iter().flatten() {
        *files.entry(used.file.as_path()).or_default() += 1;
    }
    let (best, count) = files
        .iter()
        .max_by_key(|(path, n)| (**n, std::cmp::Reverse(**path)))?;
    let here = if target == home {
        files.get(file).copied().unwrap_or(0)
    } else {
        0
    };
    let moves = target != home || (*count >= MIN_USES && *count > here);
    moves.then(|| FileTarget {
        file: best.to_string_lossy().into_owned(),
        uses: *count,
    })
}
