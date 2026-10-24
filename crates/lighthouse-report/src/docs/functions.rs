//! The reference of the CEL functions the registered libraries provide,
//! generated from the function catalog: what a decision's expressions may
//! call beyond the `cel` crate's standard library and extensions.

use std::{collections::BTreeMap, fmt::Write};

use super::decision::NOTICE;

/// One function of the catalog, as the page shows it.
pub struct FunctionDoc {
    pub library: String,
    pub version: String,
    /// The overloads as called: `regex.replace(string, string, string) -> string`.
    pub signatures: Vec<String>,
    pub doc: String,
    pub example: String,
    /// The step of the precedence rule that admitted it: 2 or 3.
    pub step: u8,
    pub name: String,
}

/// The page of the library functions, by library.
pub fn cel_functions_markdown(functions: &[FunctionDoc]) -> String {
    let mut out = format!(
        "{NOTICE}\n\n# CEL functions\n\n\
         Every check, fix, variable and event mapping is written in one CEL \
         environment: the standard library of CEL and the extensions of the `cel` crate \
         (strings, lists, math, encoders, optionals) exactly as shipped, the edit \
         constructors of a fix ([fix operations](fix-operations.md)), and the libraries \
         below, which plugins register. A function a decision calls that none provides, \
         or calls with arguments no overload takes, is refused when the decision loads. \
         The step is the rule of preference for a function a decision needs: 1, the \
         `cel` crate provides it; 2, a CEL specification or an official extension \
         defines it (implemented as defined, one library each); 3, nothing official, \
         designed after the nearest precedent. A library's id and version are part of \
         the revision of every check that calls it.\n\n"
    );
    let mut by_library: BTreeMap<(&str, &str), Vec<&FunctionDoc>> = BTreeMap::new();
    for function in functions {
        by_library
            .entry((&function.library, &function.version))
            .or_default()
            .push(function);
    }
    for ((library, version), functions) in by_library {
        let _ = write!(out, "## `{library}` {version}\n\n");
        for function in functions {
            let _ = write!(
                out,
                "### `{}` (step {})\n\n{}\n\n",
                function.name, function.step, function.doc
            );
            for signature in &function.signatures {
                let _ = writeln!(out, "- `{signature}`");
            }
            let _ = write!(out, "\n```text\n{}\n```\n\n", function.example);
        }
    }
    out.pop();
    out
}
