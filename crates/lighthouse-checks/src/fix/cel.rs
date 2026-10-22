//! A `cel` fix: one expression that returns the edits, evaluated for the
//! subject of a finding with the variables of its check.

use cel::{Context, Program};
use lighthouse_plugin::{Error as PluginError, FixRequest};
use lighthouse_spec::{CelCheck, Decision, Select};
use serde_json::{Value, json};

use super::edits::{self, Lowered, Place};
use crate::{
    Error,
    builder::{Builder, span},
    eval::{cel_fact, compile, to_json},
    glob::slashed,
    library::{self, Needs},
    manifest::rule_options,
};

/// The edits and the variables of a check, compiled.
pub(crate) struct CelFix {
    edits: Program,
    variables: Vec<(String, Program)>,
    select: Option<Select>,
    needs: Needs,
    decision: Decision,
    library: Context<'static, 'static>,
}

impl CelFix {
    pub(crate) fn new(decision: &Decision, edits: &str) -> Result<Self, Error> {
        let id = decision.id();
        let check = decision.check.as_ref().and_then(|check| match &check.kind {
            lighthouse_spec::CheckKind::Cel(cel) => Some(cel.clone()),
            lighthouse_spec::CheckKind::Model(model) => model.as_cel(decision.scope.subject),
            _ => None,
        });
        let (variables, select) = check.as_ref().map_or((Vec::new(), None), |cel: &CelCheck| {
            (cel.variables.clone(), cel.selects(decision.scope.subject))
        });
        let mut sources = vec![edits];
        sources.extend(variables.iter().map(|v| v.expression.as_str()));
        Ok(Self {
            edits: compile(id, "fix edits", edits)?,
            variables: variables
                .iter()
                .map(|v| {
                    Ok((
                        v.name.clone(),
                        compile(id, &format!("variable `{}`", v.name), &v.expression)?,
                    ))
                })
                .collect::<Result<_, Error>>()?,
            select,
            needs: Needs::of(sources, []),
            decision: decision.clone(),
            library: library::context().map_err(|e| Error::invalid(id, e))?,
        })
    }

    /// The edits for the finding of `request`; why there are none when the
    /// expression returns an empty list.
    pub(crate) fn lower(&self, id: &str, request: &FixRequest) -> Result<Lowered, PluginError> {
        let fail = |message: String| PluginError::Failed(format!("{id}: fix: {message}"));
        let finding = request.finding;
        let project = request.project;
        let file = project.file(&finding.file);
        let language = file.map(|f| f.lang.as_str());
        let options = rule_options(&self.decision, request.options, language)?;
        let builder = Builder::standalone(
            project,
            request.workspace,
            &self.needs,
            &options,
            &finding.rule_id,
            language.unwrap_or_default(),
        );
        let mut scope = self.library.new_inner_scope();
        let mut set = |name: &str, value: Value| scope.add_variable_as_val(name, cel_fact(&value));
        set(
            "finding",
            json!({
                "decision": finding.rule_id,
                "message": finding.message,
                "file": slashed(&finding.file),
                "span": span(&finding.file, finding.span),
                "symbol": finding.symbol.clone().unwrap_or_default(),
                "evidence": if finding.evidence.is_null() { json!({}) } else { finding.evidence.clone() },
                "fingerprint": finding.fingerprint.as_str(),
                "facts": request.facts,
            }),
        );
        set("options", Value::Object(options.clone()));
        let file_value = file.map(|f| builder.file(f, request.text));
        if let Some(value) = &file_value {
            set("file", value.clone());
        }
        let symbol = finding
            .symbol
            .as_deref()
            .and_then(lighthouse_model::ucm::SymbolId::parse)
            .and_then(|id| project.symbol(&id));
        let comment = || {
            let comment = project
                .comments_in(&finding.file)
                .iter()
                .find(|c| c.span == finding.span)?;
            Some(cel_fact(&builder.comment(comment, request.text)))
        };
        let subject = match self.select {
            Some(Select::Comment) => comment(),
            Some(Select::File) => file_value.as_ref().map(cel_fact),
            Some(Select::Function) => symbol.and_then(|s| builder.function_fact(s)),
            Some(Select::Test) => symbol.and_then(|s| builder.test_fact(s)),
            _ => symbol.map(|s| builder.symbol_fact(s)),
        };
        let subject = subject.unwrap_or_else(|| cel_fact(&Value::Null));
        if let Some(select) = self.select {
            scope.add_variable_as_val(select.variable(), subject.clone_as_boxed());
        }
        scope.add_variable_as_val("node", subject);
        for (name, program) in &self.variables {
            let bound = program
                .execute(&scope)
                .map_err(|e| fail(format!("variable `{name}`: {e}")))?;
            scope.add_variable_from_value(name, bound);
        }
        let returned = self
            .edits
            .execute(&scope)
            .map_err(|e| fail(e.to_string()))?;
        let Value::Array(list) = to_json(&returned) else {
            return Err(fail("`edits` is not a list".to_owned()));
        };
        let place = Place {
            project,
            file: &finding.file,
            text: request.text,
        };
        Ok(edits::lower(&list, &place))
    }
}
