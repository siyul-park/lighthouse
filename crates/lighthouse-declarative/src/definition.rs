use cel::Program;

use crate::Error;

use lighthouse_spec::{CelCheck, Select};

/// A CEL check with every expression compiled.
#[derive(Debug)]
pub(crate) struct Compiled {
    pub select: Select,
    pub condition: Program,
    pub message: Vec<Piece>,
    pub evidence: Vec<(String, Program)>,
}

/// A piece of a message template.
#[derive(Debug)]
pub(crate) enum Piece {
    Text(String),
    Hole(Program),
}

/// Compiles every expression of `check`, naming `rule` in errors.
pub(crate) fn compile(check: &CelCheck, rule: &str) -> Result<Compiled, Error> {
    let program = |what: &str, source: &str| {
        Program::compile(source).map_err(|e| Error::invalid(rule, format!("{what}: {e}")))
    };
    let mut evidence = Vec::new();
    for (name, source) in &check.evidence {
        evidence.push((
            name.clone(),
            program(&format!("evidence `{name}`"), source)?,
        ));
    }
    Ok(Compiled {
        select: check.select,
        condition: program("where", &check.condition)?,
        message: template(rule, &check.message)?,
        evidence,
    })
}

fn template(rule: &str, text: &str) -> Result<Vec<Piece>, Error> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let (before, after) = rest.split_at(open);
        if !before.is_empty() {
            pieces.push(Piece::Text(before.to_owned()));
        }
        let Some(close) = after.find("}}") else {
            return Err(Error::invalid(rule, "message: `{{` is never closed"));
        };
        let source = after[2..close].trim();
        let program = Program::compile(source)
            .map_err(|e| Error::invalid(rule, format!("message `{source}`: {e}")))?;
        pieces.push(Piece::Hole(program));
        rest = &after[close + 2..];
    }
    if !rest.is_empty() {
        pieces.push(Piece::Text(rest.to_owned()));
    }
    Ok(pieces)
}
