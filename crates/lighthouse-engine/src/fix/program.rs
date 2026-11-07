//! One run of the program of a fix by a command: what it is given, the text
//! it prints and the edits that make. It is the part of the fix by a command
//! that needs neither the run nor the graph, so a node of the graph (see
//! [`super::node`]) and the fix run both call it.

use std::{collections::BTreeMap, path::Path};

use lighthouse_model::Diagnostic;
use lighthouse_plugin::Workspace;
use lighthouse_process::{Environment, Home, Job, Ran, context, expand};
use lighthouse_protocol::hash;
use lighthouse_spec::{Output, Point, Program, Stdin, file_fact};
use serde_json::json;

use super::{
    edits::replace_changed,
    model::Class,
    rewrite::{Change, Changes, Standing},
};

/// How many characters of what a program said a reason carries.
const SAID_SHOWN: usize = 400;

/// What a file with no provider facts has.
pub(super) static NO_FACTS: BTreeMap<String, String> = BTreeMap::new();

/// Why a finding gets no fix by a command, and how that is counted.
pub(super) type Failure = (Class, String);

/// What a run of the program is given.
pub(super) struct Launch<'a> {
    /// The program of the language of the finding's file.
    pub(super) program: &'a Program,
    pub(super) finding: &'a Diagnostic,
    /// The text of the finding's file as it stands.
    pub(super) text: &'a str,
    /// The options the finding's rule ran with.
    pub(super) options: &'a serde_json::Map<String, serde_json::Value>,
    /// What the provider says of the finding's file, for `{file.<fact>}`.
    pub(super) facts: &'a BTreeMap<String, String>,
}

/// Whether the program reads the file on stdin and prints its new text.
pub(super) fn is_filter(program: &Program) -> bool {
    program.stdin == Stdin::File && program.output(Point::Fix) == Output::Stdout
}

/// The change of a filter: the program is given the file on stdin, in the
/// project, and prints its new text.
pub(super) fn filter(launch: &Launch<'_>, workspace: &Workspace) -> Result<Changes, Failure> {
    let ran = run(launch, workspace, &workspace.root, None)?;
    filtered(launch, ran)
}

/// The text edits that turn each file of `after` from what stands into the
/// program's version, and the digest of each as it stands.
pub(super) fn edits_of(
    after: &BTreeMap<std::path::PathBuf, String>,
    world: &dyn Standing,
) -> Result<
    (
        Vec<super::edits::TextEdit>,
        BTreeMap<std::path::PathBuf, String>,
    ),
    Failure,
> {
    let mut edits = Vec::new();
    let mut bases = BTreeMap::new();
    for (path, new) in after {
        let old = world.text(path).ok_or_else(|| {
            (
                Class::Conflict,
                format!("`{}` cannot be read", path.display()),
            )
        })?;
        bases.insert(path.clone(), hash::sha256(&old));
        edits.extend(replace_changed(path, &old, new));
    }
    if edits.is_empty() {
        return Err((Class::Unchanged, "the command changed nothing".to_owned()));
    }
    Ok((edits, bases))
}

/// One run of the program in `dir`, where it may write only `writable`.
pub(super) fn run(
    launch: &Launch<'_>,
    workspace: &Workspace,
    dir: &Path,
    writable: Option<&Path>,
) -> Result<Ran, Failure> {
    let fail = |reason: String| (Class::Unanswered, reason);
    let (program, finding) = (launch.program, launch.finding);
    let shown = program.shown();
    if let Some(fact) = program
        .command
        .iter()
        .filter_map(|arg| file_fact(arg))
        .find(|fact| !launch.facts.contains_key(*fact))
    {
        return Err(fail(format!(
            "`{}` has no `{fact}` fact: the provider of its language states none",
            finding.file.display()
        )));
    }
    let command = expand(&program.command, dir, |arg| match arg {
        "{file}" => Some(vec![finding.file.to_string_lossy().replace('\\', "/")]),
        "{line}" => Some(vec![finding.span.start.line.to_string()]),
        "{symbol}" => Some(vec![finding.symbol.clone().unwrap_or_default()]),
        "{decision}" => Some(vec![finding.rule_id.clone()]),
        _ => file_fact(arg).and_then(|fact| launch.facts.get(fact).map(|v| vec![v.clone()])),
    })
    .map_err(fail)?;
    let decision = json!({
        "decision": finding.rule_id,
        "message": finding.message,
        "file": finding.file,
        "line": finding.span.start.line,
        "symbol": finding.symbol,
        "fingerprint": finding.fingerprint.as_str(),
        "evidence": finding.evidence,
    });
    let environment = Environment::new(
        &program.env,
        context(
            &decision.to_string(),
            &serde_json::Value::Object(launch.options.clone()).to_string(),
        ),
        Home::User,
    )
    .map_err(fail)?;
    let policy = program.policy(&workspace.sandbox);
    let mut profile = policy.profile(&workspace.root, dir);
    profile.writes.extend(writable.map(Path::to_owned));
    let stdin = match program.stdin {
        Stdin::None => &[][..],
        Stdin::File => launch.text.as_bytes(),
    };
    let job = Job {
        command: &command,
        dir,
        stdin,
        environment: &environment,
        timeout: program.timeout(Point::Fix),
        policy: &policy,
        profile: &profile,
    };
    // A filter's stdout is the new text of a file, not something to read.
    let ran = if program.output(Point::Fix) == Output::Stdout {
        job.run_keeping_stdout()
    } else {
        job.run()
    }
    .map_err(|reason| fail(format!("`{shown}`: {reason}")))?;
    ran.exit(&program.success(Point::Fix))
        .map_err(|reason| fail(format!("`{shown}` {reason}")))?;
    Ok(ran)
}

/// The change of a program whose stdout is the new text of the finding's
/// file.
pub(super) fn filtered(launch: &Launch<'_>, ran: Ran) -> Result<Changes, Failure> {
    let new = std::str::from_utf8(&ran.stdout)
        .map_err(|_| (Class::Unanswered, "stdout is not valid UTF-8".to_owned()))?;
    if new == launch.text {
        return Err(unchanged(&ran));
    }
    if new.is_empty() {
        return Err((
            Class::Unchanged,
            "the command printed no text, so it changed nothing".to_owned(),
        ));
    }
    Ok(Changes::from([(
        launch.finding.file.clone(),
        Change::Text(new.to_owned()),
    )]))
}

/// A run that changed nothing, with what the program said as the reason.
pub(super) fn unchanged(ran: &Ran) -> Failure {
    let said = String::from_utf8_lossy(&ran.stderr);
    let said: String = said.trim().chars().take(SAID_SHOWN).collect();
    let reason = if said.is_empty() {
        "the command changed no file".to_owned()
    } else {
        format!("the command changed no file: {said}")
    };
    (Class::Unchanged, reason)
}
