//! The `command` fix kind: an external program on a scratch copy of the
//! finding's file, under the process contract. It runs without a shell,
//! bounded in time and output; what it did is read back as a text edit and
//! verified like any other fix.

use std::{
    env, fs,
    path::{Component, Path},
    time::Duration,
};

use lighthouse_model::{EditOp, LineIndex, Span};
use lighthouse_plugin::{Error as PluginError, FixRequest};
use lighthouse_process::Spec;
use lighthouse_spec::{CommandOutput, CommandSpec, CommandStdin, timeout_seconds};
use serde_json::json;

use super::ops::Evaluated;

/// How long a command that was told to stop may take before it is killed.
const GRACE: Duration = Duration::from_secs(2);
/// The version of the contract between Lighthouse and its commands.
const API_VERSION: &str = "1";
/// What a command inherits from its caller, besides what it declares.
const INHERITED: [&str; 4] = ["PATH", "HOME", "LANG", "TMPDIR"];

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
    let argv = match arguments(spec, request, &target) {
        Ok(argv) => argv,
        Err(reason) => return Ok(Err(reason)),
    };
    let stdin = match spec.stdin {
        CommandStdin::None => Vec::new(),
        CommandStdin::File => request.text.as_bytes().to_vec(),
    };
    let environment = environment(spec, request);
    let output = lighthouse_process::run(&Spec {
        argv: &argv,
        dir: scratch.path(),
        stdin: &stdin,
        env: Some(&environment),
        limit: Duration::from_secs(timeout_seconds(&spec.timeout).unwrap_or(30)),
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
    output: lighthouse_process::Output,
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
        CommandOutput::InPlace => {
            if fs::symlink_metadata(target).is_ok_and(|m| !m.is_file()) {
                return Ok(Err(
                    "the command left something other than a file".to_owned()
                ));
            }
            fs::read_to_string(target).map_err(|e| fail(format!("cannot read the result: {e}")))?
        }
        CommandOutput::Text => String::from_utf8(output.stdout)
            .map_err(|_| fail("stdout is not valid UTF-8".to_owned()))?,
    };
    if spec.output == CommandOutput::Text && changed.is_empty() {
        return Ok(Err(
            "the command printed no text, so it changed nothing".to_owned()
        ));
    }
    Ok(replacement(&finding.file, request.text, &changed))
}

/// The environment a command gets and nothing else: the inherited allowlist,
/// the declared variables and the context Lighthouse passes.
fn environment(spec: &CommandSpec, request: &FixRequest) -> Vec<(String, String)> {
    let finding = request.finding;
    let mut vars: Vec<(String, String)> = INHERITED
        .iter()
        .filter_map(|k| env::var(k).ok().map(|v| ((*k).to_owned(), v)))
        .collect();
    vars.extend(spec.env.iter().map(|(k, v)| (k.clone(), v.clone())));
    let decision = json!({
        "rule": finding.rule_id,
        "message": finding.message,
        "file": finding.file,
        "line": finding.span.start.line,
        "symbol": finding.symbol,
        "fingerprint": finding.fingerprint.as_str(),
        "evidence": finding.evidence,
    });
    vars.push(("LIGHTHOUSE_DECISION".to_owned(), decision.to_string()));
    vars.push((
        "LIGHTHOUSE_OPTIONS".to_owned(),
        serde_json::Value::Object(request.options.clone()).to_string(),
    ));
    vars.push(("LIGHTHOUSE_API_VERSION".to_owned(), API_VERSION.to_owned()));
    vars
}

/// The command line: each placeholder argument (`{file}`, `{line}`, `{symbol}`,
/// `{rule}`) is replaced as a whole argument, so no value can grow into an
/// option; a value that starts with `-` is refused.
fn arguments(
    spec: &CommandSpec,
    request: &FixRequest,
    target: &Path,
) -> Result<Vec<String>, String> {
    let finding = request.finding;
    let mut argv = Vec::new();
    for (at, arg) in spec.argv.iter().enumerate() {
        // A program named by a relative path is the project's own, found from
        // the project root, though the command runs in the scratch directory;
        // trust is judged on that same file.
        if at == 0 && arg.contains('/') && !Path::new(arg).is_absolute() {
            argv.push(request.ws.root.join(arg).to_string_lossy().into_owned());
            continue;
        }
        let value = match arg.as_str() {
            "{file}" => target.to_string_lossy().into_owned(),
            "{line}" => finding.span.start.line.to_string(),
            "{symbol}" => finding.symbol.clone().unwrap_or_default(),
            "{rule}" => finding.rule_id.clone(),
            other => other.to_owned(),
        };
        if value != *arg && value.starts_with('-') {
            return Err(format!(
                "the value of `{arg}` starts with `-`, so it is refused"
            ));
        }
        argv.push(value);
    }
    Ok(argv)
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
