//! A `cel` fix: one expression that returns the edits, evaluated for the
//! subject of a finding with the variables of its check.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
};

use cel::{Context, Program, common::value::CowVal, context::VariableResolver};
use lighthouse_plugin::{Error as PluginError, FixRequest};
use lighthouse_spec::{CelCheck, Decision, Select};
use serde_json::{Map, Value, json};

use super::edits::{self, Lowered, Place};
use crate::{
    Error,
    builder::{Builder, span},
    eval::{Fact, cel_fact, compile, compile_shared, to_json},
    glob::slashed,
    library::{self, Needs},
    manifest::rule_options,
};

/// The edits and the variables of a check, compiled.
pub(crate) struct CelFix {
    edits: Program,
    variables: Vec<(String, Arc<Program>)>,
    select: Option<Select>,
    needs: Needs,
    decision: Decision,
    library: Context<'static, 'static>,
    /// Per variable, whether it reads nothing of the finding: its value is the
    /// same for every finding of one file.
    shared: Vec<bool>,
    /// What the findings of the file last asked about are judged with.
    cached: Mutex<Option<Arc<FileValues>>>,
}

/// The file as a value and the shared variables of one file text, found once
/// for all the findings fixed in it.
struct FileValues {
    path: PathBuf,
    text: String,
    options: Map<String, Value>,
    file: Option<Fact>,
    variables: Vec<(String, Fact)>,
}

/// The values are read in place by every evaluation, never copied.
impl VariableResolver for FileValues {
    fn resolve<'b>(&'b self, variable: &str) -> Option<CowVal<'b, 'b>> {
        let found = if variable == "file" {
            self.file.as_ref()
        } else {
            self.variables
                .iter()
                .find(|(name, _)| name == variable)
                .map(|(_, fact)| fact)
        };
        found.map(|fact| CowVal::Borrowed(fact.as_ref()))
    }
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
        let mut library = library::context().map_err(|e| Error::invalid(id, e))?;
        let declared = library::declare_variables(id, &mut library, &variables)?;
        let (values, bodies) = (declared.values, declared.bodies);
        let edits = compile(id, "fix edits", edits)?;
        let variables: Vec<(String, Arc<Program>)> = values
            .into_iter()
            .map(|v| {
                Ok((
                    v.name.clone(),
                    compile_shared(id, &format!("variable `{}`", v.name), &v.expression)?,
                ))
            })
            .collect::<Result<_, Error>>()?;
        let level = crate::cel_rule::file_level(&variables);
        let shared = |name: &str| {
            variables
                .iter()
                .zip(&level)
                .any(|((variable, _), file_level)| *file_level && variable == name)
        };
        library::check_scope(id, &declared.functions, &bodies, &shared)?;
        let mut per_finding = vec!["finding".to_owned()];
        let shared_by_finding: Vec<bool> = variables
            .iter()
            .zip(&level)
            .map(|((name, program), file_level)| {
                let names = lighthouse_spec::program_identifiers(program);
                let same = *file_level && !per_finding.iter().any(|n| names.contains(n.as_str()));
                if !same {
                    per_finding.push(name.clone());
                }
                same
            })
            .collect();
        let read = [&edits]
            .into_iter()
            .chain(variables.iter().map(|(_, program)| program.as_ref()))
            .chain(bodies.iter().map(Arc::as_ref))
            .collect::<Vec<&Program>>();
        library::check_calls(id, &library, &declared.functions, read.iter().copied())?;
        Ok(Self {
            needs: Needs::reading(read.iter().copied()),
            edits,
            variables,
            select,
            decision: decision.clone(),
            library,
            shared: shared_by_finding,
            cached: Mutex::new(None),
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
            language.unwrap_or_default(),
        );
        let needs_file = self.needs.ident("file") || self.select == Some(Select::File);
        let values = self.values(id, request, &options, &builder, needs_file)?;
        let mut scope = self.library.new_inner_scope();
        scope.set_variable_resolver(&*values);
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
            Some(Select::File) => values.file.as_ref().map(|f| f.clone_as_boxed()),
            Some(Select::Function) => symbol.and_then(|s| builder.function_fact(s)),
            Some(Select::Test) => symbol.and_then(|s| builder.test_fact(s)),
            _ => symbol.map(|s| builder.symbol_fact(s)),
        };
        let subject = subject.unwrap_or_else(|| cel_fact(&Value::Null));
        if let Some(select) = self.select {
            scope.add_variable_as_val(select.variable(), subject.clone_as_boxed());
        }
        scope.add_variable_as_val("node", subject);
        for ((name, program), shared) in self.variables.iter().zip(&self.shared) {
            if *shared {
                continue;
            }
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

    /// The file and the shared variables of the file of `request`: the ones the
    /// last finding was fixed with when it is the same text, else found now.
    fn values(
        &self,
        id: &str,
        request: &FixRequest,
        options: &Map<String, Value>,
        builder: &Builder,
        needs_file: bool,
    ) -> Result<Arc<FileValues>, PluginError> {
        let finding = request.finding;
        let mut cached = self.cached.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(found) = cached
            .as_ref()
            .filter(|c| c.path == finding.file && c.text == request.text && c.options == *options)
        {
            return Ok(Arc::clone(found));
        }
        let file = request
            .project
            .file(&finding.file)
            .filter(|_| needs_file)
            .map(|f| cel_fact(&builder.file(f, request.text)));
        let mut scope = self.library.new_inner_scope();
        scope.add_variable_as_val("options", cel_fact(&Value::Object(options.clone())));
        if let Some(file) = &file {
            scope.add_variable_as_val("file", file.clone_as_boxed());
        }
        let mut variables = Vec::new();
        for ((name, program), shared) in self.variables.iter().zip(&self.shared) {
            if !*shared {
                continue;
            }
            let bound = program
                .execute(&scope)
                .map_err(|e| PluginError::Failed(format!("{id}: fix: variable `{name}`: {e}")))?;
            if let Ok(fact) = Fact::try_from(bound.clone()) {
                variables.push((name.clone(), fact));
            }
            scope.add_variable_from_value(name, bound);
        }
        let built = Arc::new(FileValues {
            path: finding.file.clone(),
            text: request.text.to_owned(),
            options: options.clone(),
            file,
            variables,
        });
        *cached = Some(Arc::clone(&built));
        Ok(built)
    }
}
