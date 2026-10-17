//! The `command` fix kind: an external program on a scratch copy of the
//! finding's file, under the process contract. It runs without a shell,
//! bounded in time and output; what it did is read back as a text edit and
//! verified like any other fix.

use std::{
    fs,
    path::{Component, Path},
};

use lighthouse_model::{EditOp, LineIndex, Span};
use lighthouse_plugin::{Error as PluginError, FixRequest};
use lighthouse_process::{GRACE, Invocation, Output, Stdin};
use lighthouse_spec::CommandSpec;
use serde_json::json;

use super::ops::Evaluated;
use crate::command::contract;

pub(crate) fn run(
    id: &str,
    spec: &CommandSpec,
    request: &FixRequest,
) -> Result<Evaluated, PluginError> {
    let fail = |message: String| PluginError::Failed(format!("{id}: fix command: {message}"));
    let finding = request.finding;
    if finding.file.is_absolute()
        || finding
            .file
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Ok(Err(format!(
            "`{}` is not a relative, normalized path",
            finding.file.display()
        )));
    }
    let scratch = tempfile::tempdir().map_err(|e| fail(e.to_string()))?;
    let target = scratch.path().join(&finding.file);
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir).map_err(|e| fail(e.to_string()))?;
    }
    fs::write(&target, request.text).map_err(|e| fail(e.to_string()))?;
    let argv = match contract::expand(&spec.argv, &request.workspace.root, |arg| match arg {
        "{file}" => Some(vec![target.to_string_lossy().into_owned()]),
        "{line}" => Some(vec![finding.span.start.line.to_string()]),
        "{symbol}" => Some(vec![finding.symbol.clone().unwrap_or_default()]),
        "{rule}" => Some(vec![finding.rule_id.clone()]),
        _ => None,
    }) {
        Ok(argv) => argv,
        Err(reason) => return Ok(Err(reason)),
    };
    let stdin = match spec.stdin {
        Stdin::None => Vec::new(),
        Stdin::File => request.text.as_bytes().to_vec(),
    };
    let decision = json!({
        "decision": finding.rule_id,
        "message": finding.message,
        "file": finding.file,
        "line": finding.span.start.line,
        "symbol": finding.symbol,
        "fingerprint": finding.fingerprint.as_str(),
        "evidence": finding.evidence,
    });
    let environment = contract::environment(&spec.env, &decision, request.options);
    let output = lighthouse_process::run(&Invocation {
        argv: &argv,
        dir: scratch.path(),
        stdin: &stdin,
        env: Some(&environment),
        limit: spec.timeout.get(),
        grace: GRACE,
        max_output: lighthouse_process::MAX_OUTPUT,
    })
    .map_err(|reason| fail(format!("`{}`: {reason}", argv.join(" "))))?;
    interpret(id, spec, request, &argv, &target, output)
}

/// What a command's exit status and output mean: declined, an error, or the
/// new text of the file as a replacement.
fn interpret(
    id: &str,
    spec: &CommandSpec,
    request: &FixRequest,
    argv: &[String],
    target: &Path,
    output: lighthouse_process::Ran,
) -> Result<Evaluated, PluginError> {
    let fail = |message: String| PluginError::Failed(format!("{id}: fix command: {message}"));
    let finding = request.finding;
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    match output.code {
        Some(0) => {}
        Some(1) => {
            let why = if stderr.is_empty() {
                "no reason given"
            } else {
                &stderr
            };
            return Ok(Err(format!("`{}` declined: {why}", argv.join(" "))));
        }
        code => {
            let how = code.map_or("was ended by a signal".to_owned(), |c| {
                format!("exited {c}")
            });
            return Err(fail(format!("`{}` {how}: {stderr}", argv.join(" "))));
        }
    }
    let changed = match spec.output {
        Output::InPlace => {
            if fs::symlink_metadata(target).is_ok_and(|m| !m.is_file()) {
                return Ok(Err(
                    "the command left something other than a file".to_owned()
                ));
            }
            fs::read_to_string(target).map_err(|e| fail(format!("cannot read the result: {e}")))?
        }
        Output::Text => String::from_utf8(output.stdout)
            .map_err(|_| fail("stdout is not valid UTF-8".to_owned()))?,
    };
    if spec.output == Output::Text && changed.is_empty() {
        return Ok(Err(
            "the command printed no text, so it changed nothing".to_owned()
        ));
    }
    Ok(replacement(&finding.file, request.text, &changed))
}

/// The smallest whole-line replacement that turns `old` into `new`; declined
/// when they are the same.
fn replacement(file: &Path, old: &str, new: &str) -> Evaluated {
    if old == new {
        return Err("the command changed nothing".to_owned());
    }
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = old_lines[prefix..]
        .iter()
        .rev()
        .zip(new_lines[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let offset = |lines: &[&str]| lines.iter().map(|l| l.len()).sum::<usize>();
    let start = offset(&old_lines[..prefix]);
    let end = old.len() - offset(&old_lines[old_lines.len() - suffix..]);
    let text = new
        [offset(&new_lines[..prefix])..new.len() - offset(&new_lines[new_lines.len() - suffix..])]
        .to_owned();
    let index = LineIndex::new(old);
    Ok(vec![EditOp::Replace {
        file: file.to_owned(),
        span: Span {
            start: index.position(start),
            end: index.position(end),
        },
        text,
    }])
}
