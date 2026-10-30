//! The judgments of one file: what each frame of a check reads, and the
//! diagnostics it reports.

use super::*;

impl Evaluation<'_> {
    pub(super) fn check_file(&self, ctx: &Context) -> Result<Vec<Diagnostic>, PluginError> {
        let Some((file, text)) = ctx.file else {
            return Ok(Vec::new());
        };
        if self.rule.select != Select::File && self.rule.needs.ident("file") {
            let _ = self
                .shared
                .file
                .set(cel_fact(&self.builder.file(file, text)));
        }
        let symbols = matches!(
            self.rule.select,
            Select::Symbol | Select::Function | Select::Test | Select::Event
        );
        if !symbols && self.focus.is_some_and(|l| l != file.analysis_language()) {
            return Ok(Vec::new());
        }
        match self.rule.select {
            Select::File if ctx.applies.admits_file(file) => {
                let value = cel_fact(&self.builder.file(file, text));
                let _ = self.shared.file.set(value.clone_as_boxed());
                let anchor = Anchor {
                    file: file.path.clone(),
                    span: top_of_file(),
                    symbol: None,
                };
                Ok(self
                    .judge(value, anchor, &file.path.to_string_lossy())?
                    .into_iter()
                    .collect())
            }
            Select::Symbol | Select::Function | Select::Test | Select::Event => {
                self.check_symbols(ctx, file)
            }
            Select::Comment => self.check_comments(ctx, file, text),
            Select::Annotation => self.check_annotations(file),
            Select::File | Select::Edge | Select::Module | Select::Decision => Ok(Vec::new()),
        }
    }

    /// The symbols of `file` that are subjects of the decision's scope.
    fn check_symbols(&self, ctx: &Context, file: &File) -> Result<Vec<Diagnostic>, PluginError> {
        let project = ctx.project;
        let mut found = Vec::new();
        for symbol in project
            .symbols_in(&file.path)
            .filter(|s| ctx.applies.admits_symbol(project, s))
            .filter(|s| {
                self.focus
                    .is_none_or(|language| project.language_of(s) == Some(language))
            })
        {
            if self.rule.select == Select::Event {
                found.extend(self.judge_events(symbol)?);
                continue;
            }
            let value = match self.rule.select {
                Select::Function => self.builder.function_fact(symbol),
                Select::Test => self.builder.test_fact(symbol),
                _ => Some(self.builder.symbol_fact(symbol)),
            };
            if let Some(value) = value {
                found.extend(self.judge_symbol(symbol, value)?);
            }
        }
        Ok(found)
    }

    /// One finding per event of the function, at the event.
    fn judge_events(&self, function: &Symbol) -> Result<Vec<Diagnostic>, PluginError> {
        let mut found = Vec::new();
        for event in self.builder.event_facts(function) {
            let place = Anchor {
                file: function.file.clone(),
                span: event.span,
                symbol: Some(function.id.clone()),
            };
            found.extend(self.judge(event.fact, place, &event.key)?);
        }
        Ok(found)
    }

    /// The comments of `file` that are subjects of the decision's scope.
    fn check_comments(
        &self,
        ctx: &Context,
        file: &File,
        text: &str,
    ) -> Result<Vec<Diagnostic>, PluginError> {
        let project = ctx.project;
        let mut found = Vec::new();
        for comment in project.comments_in(&file.path) {
            if !ctx.applies.admits_comment(project.file(&comment.file)) {
                continue;
            }
            let value = cel_fact(&self.builder.comment(comment, text));
            let place = Anchor {
                file: file.path.clone(),
                span: comment.span,
                symbol: None,
            };
            found.extend(self.judge(value, place, &comment.text)?);
        }
        Ok(found)
    }

    /// The directives of `file`, one subject per rule each names, as the
    /// engine judged them.
    fn check_annotations(&self, file: &File) -> Result<Vec<Diagnostic>, PluginError> {
        let Some(annotations) = self.builder.annotations(&file.path) else {
            return Err(PluginError::Failed(format!(
                "{}: the directives are judged after the other rules, and this rule ran before",
                self.meta.id
            )));
        };
        let mut found = Vec::new();
        for annotation in annotations {
            let span = serde_json::from_value(annotation["place"].clone()).map_err(|e| {
                PluginError::Failed(format!("{}: an annotation has no place: {e}", self.meta.id))
            })?;
            let place = Anchor {
                file: file.path.clone(),
                span,
                symbol: None,
            };
            let key = annotation["snippet"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            found.extend(self.judge(cel_fact(&annotation), place, &key)?);
        }
        Ok(found)
    }

    pub(super) fn check_project(&self, ctx: &Context) -> Result<Vec<Diagnostic>, PluginError> {
        let project = ctx.project;
        let declared = modules::index(project);
        let mut found = Vec::new();
        match self.rule.select {
            Select::Module => {
                for module in &project.modules {
                    let Some(declared) = declared.get(module.path.as_str()) else {
                        continue;
                    };
                    if !admits_module(ctx, declared, module) {
                        continue;
                    }
                    let value = cel_fact(&self.builder.module(module, declared));
                    let place = Anchor {
                        file: declared.anchor.file.clone(),
                        span: top_of_file(),
                        symbol: None,
                    };
                    found.extend(self.judge(value, place, &module.path)?);
                }
            }
            Select::Edge => {
                for (at, edge) in project.edges.iter().enumerate() {
                    let Some(place) = edge_anchor(ctx, &declared, at, edge) else {
                        continue;
                    };
                    let value = cel_fact(&facts::edge(project, edge));
                    let key = format!("{:?}->{:?}", edge.from, edge.to);
                    found.extend(self.judge(value, place, &key)?);
                }
            }
            Select::Decision => found.extend(self.check_decisions(project)?),
            _ => {}
        }
        Ok(found)
    }

    /// The decision documents of the project that are subjects.
    fn check_decisions(&self, project: &Project) -> Result<Vec<Diagnostic>, PluginError> {
        let mut found = Vec::new();
        for document in project.documents().iter().filter(|d| d.kind == "Decision") {
            let value = cel_fact(&facts::document(document));
            let place = Anchor {
                file: document.file.clone(),
                span: Span {
                    start: document.at,
                    end: document.at,
                },
                symbol: None,
            };
            // A decision is the same one under another name.
            let key = document.uid.as_deref().unwrap_or(&document.name);
            found.extend(self.judge(value, place, key)?);
        }
        Ok(found)
    }

    fn judge_symbol(
        &self,
        symbol: &Symbol,
        value: Fact,
    ) -> Result<Option<Diagnostic>, PluginError> {
        let place = Anchor {
            file: symbol.file.clone(),
            span: symbol.span,
            symbol: Some(symbol.id.clone()),
        };
        self.judge(value, place, symbol.id.as_str())
    }

    /// Evaluates the rule for one selected value; a finding is reported at
    /// `place` unless the check says otherwise. A variable of the file that
    /// could not be found is the failure, not the expression that read it.
    fn judge(
        &self,
        value: Fact,
        place: Anchor,
        key: &str,
    ) -> Result<Option<Diagnostic>, PluginError> {
        self.judge_value(value, place, key).map_err(|error| {
            self.shared.failed.get().map_or(error, |message| {
                PluginError::Incomplete(format!("{}: execution error: {message}", self.meta.id))
            })
        })
    }

    fn judge_value(
        &self,
        value: Fact,
        place: Anchor,
        key: &str,
    ) -> Result<Option<Diagnostic>, PluginError> {
        let rule = self.rule;
        let mut frame = Frame::new(&self.meta.id, &rule.library);
        frame.set_fact(rule.select.variable(), value);
        frame.share(&self.shared);
        for (at, (name, program)) in rule.variables.iter().enumerate() {
            if rule.used[at] {
                self.variable(&mut frame, at, name, program)?;
            }
        }
        let found = frame.truth(&rule.condition)?;
        let abstain = rule.abstain.as_ref().filter(|_| self.reviewing);
        match (abstain, found) {
            (None, false) => return Ok(None),
            (Some(abstain), false) if !abstain.asks(&frame)? => return Ok(None),
            _ => {}
        }
        // The message and evidence of a subject only suspected may read what
        // the subject does not have: they are then left empty.
        let message = frame.text(&rule.message);
        let evidence = frame.evidence(&rule.evidence);
        let (message, evidence) = if found {
            (message?, evidence?)
        } else {
            (message.unwrap_or_default(), evidence.unwrap_or_default())
        };
        // Like the message, the place of a subject only suspected may name what
        // the subject does not have: it is then the default.
        let place = match self.place(&frame, place.clone()) {
            Err(error) if found => return Err(error),
            placed => placed.unwrap_or(place),
        };
        let fingerprint = match &rule.identity {
            Some(identity) => {
                let subject = frame.string(&identity.subject, "identity.subject")?;
                let snippet = match &identity.snippet {
                    Some(program) => frame.string(program, "identity.snippet")?,
                    None => String::new(),
                };
                self.meta.fingerprint(&subject, &snippet)
            }
            None => self.meta.fingerprint(&place.file.to_string_lossy(), key),
        };
        let mut diagnostic = Diagnostic::new(
            &self.meta.id,
            self.meta.severity,
            message,
            place.file,
            place.span,
            fingerprint,
        );
        diagnostic.symbol = place.symbol.map(|id| id.as_str().to_owned());
        diagnostic.evidence = evidence;
        if let Some(abstain) = abstain {
            let fact = abstain.fact_of(&frame)?;
            self.candidates.borrow_mut().push(Candidate {
                diagnostic: diagnostic.clone(),
                fact,
                decided: found,
            });
            if !found {
                return Ok(None);
            }
        }
        Ok(Some(diagnostic))
    }

    /// Gives variable number `at` its value in the frame: the file's one
    /// value, found once, when it reads nothing of the subject.
    fn variable(
        &self,
        frame: &mut Frame,
        at: usize,
        name: &str,
        program: &Program,
    ) -> Result<(), PluginError> {
        let value = |frame: &Frame| {
            Fact::try_from(frame.run(program)?).map_err(|_| frame.fail("the value cannot be held"))
        };
        // A file-level variable is not held by the frame: whatever reads it
        // finds it through the resolver, when it first does.
        if self.shared.names[at].is_none() {
            frame.set_fact(name, value(frame)?);
        }
        Ok(())
    }

    fn place(&self, frame: &Frame, default: Anchor) -> Result<Anchor, PluginError> {
        let Some(at) = &self.rule.at else {
            return Ok(default);
        };
        if let Some(program) = &at.symbol {
            let text = frame.string(program, "at.symbol")?;
            let symbol = SymbolId::parse(&text)
                .and_then(|id| self.builder.project().symbol(&id))
                .ok_or_else(|| {
                    frame.fail(format!(
                        "`at.symbol` is `{text}`, not a symbol of the project"
                    ))
                })?;
            return Ok(Anchor {
                file: symbol.file.clone(),
                span: symbol.span,
                symbol: Some(symbol.id.clone()),
            });
        }
        let line = |program: &Option<Program>, what: &str| -> Result<Option<u32>, PluginError> {
            program
                .as_ref()
                .map(|p| Ok(u32::try_from(frame.int(p, what)?.max(1)).unwrap_or(u32::MAX)))
                .transpose()
        };
        let start = line(&at.line, "at.line")?;
        let end = line(&at.end_line, "at.endLine")?;
        let position = |line: u32| Position { line, col: 1 };
        let span = match (start, end) {
            (None, None) => default.span,
            (Some(s), None) => Span {
                start: position(s),
                end: position(s),
            },
            (None, Some(e)) => Span {
                start: default.span.start,
                end: position(e),
            },
            (Some(s), Some(e)) => Span {
                start: position(s),
                end: position(e),
            },
        };
        Ok(Anchor { span, ..default })
    }
}
