use lighthouse_config::{RuleConfig, Rules};
use lighthouse_model::{Capability, Diagnostic, Fingerprint, Fragment, Options, Position, Span};
use lighthouse_plugin::{
    Analyzer, Conventions, Ctx, Error, Indexed, LanguageProvider, Manifest, Plugin, Preset, Rule,
    RuleMeta, Scope, Source, Workspace,
};
use lighthouse_spec::PatternRule;
use serde::Deserialize;
use serde_json::{Value, json};

const LINE_COUNT: &str = "core/line-count";
const MAX_FILE_LINES: &str = "core/max-file-lines";

pub struct Core;

impl Plugin for Core {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "core".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Text::new())]
    }

    fn analyzers(&self) -> Vec<Box<dyn Analyzer>> {
        vec![Box::new(LineCount)]
    }

    fn rules(&self) -> Vec<Box<dyn Rule>> {
        vec![max_file_lines()]
    }

    fn presets(&self) -> Vec<Preset> {
        let rules = self.rules();
        let rules = rules.iter().map(|r| r.meta()).map(|meta| {
            let config = RuleConfig {
                level: Some(meta.severity),
                options: Options::new(),
            };
            (meta.id.clone(), config)
        });
        vec![Preset {
            id: "core/recommended".to_owned(),
            rules: Rules::from_iter(rules),
        }]
    }
}

/// Fallback provider: every file is plain text.
struct Text {
    globs: Vec<String>,
}

impl Text {
    fn new() -> Self {
        Self {
            globs: vec!["**".to_owned()],
        }
    }
}

impl LanguageProvider for Text {
    fn id(&self) -> &str {
        "text"
    }

    fn globs(&self) -> &[String] {
        &self.globs
    }

    fn conventions(&self) -> Conventions {
        Conventions::default()
    }

    fn capabilities(&self) -> &[Capability] {
        &[]
    }

    fn fallback(&self) -> bool {
        true
    }

    fn index(&self, _: &Workspace, files: &[Source]) -> Result<Indexed, Error> {
        let fragments = files
            .iter()
            .map(|source| Fragment {
                files: vec![source.file.clone()],
                ..Fragment::default()
            })
            .collect();
        Ok(Indexed {
            fragments,
            ..Indexed::default()
        })
    }
}

struct LineCount;

impl Analyzer for LineCount {
    fn id(&self) -> &str {
        LINE_COUNT
    }

    fn requires(&self) -> &[String] {
        &[]
    }

    fn scope(&self) -> Scope {
        Scope::File
    }

    fn run(&self, ctx: &Ctx) -> Result<Value, Error> {
        let (_, text) = ctx
            .file
            .ok_or_else(|| Error::Failed("no file".to_owned()))?;
        Ok(json!(text.lines().count()))
    }
}

#[derive(Deserialize)]
struct Limit {
    max: usize,
}

fn max_file_lines() -> Box<dyn Rule> {
    Box::new(PatternRule::new(
        MAX_FILE_LINES,
        &[LINE_COUNT],
        |meta: &RuleMeta, ctx: &Ctx, Limit { max }| {
            let (file, _) = ctx
                .file
                .ok_or_else(|| Error::Failed("no file".to_owned()))?;
            let lines: usize = ctx.fact(LINE_COUNT)?;
            if lines <= max {
                return Ok(Vec::new());
            }
            let at = |line: usize| Position {
                line: u32::try_from(line).unwrap_or(u32::MAX),
                col: 1,
            };
            let mut diagnostic = Diagnostic::new(
                MAX_FILE_LINES,
                meta.severity,
                format!("file has {lines} lines, limit is {max}"),
                &file.path,
                Span {
                    start: at(max + 1),
                    end: at(lines),
                },
                Fingerprint::of(MAX_FILE_LINES, &file.path.to_string_lossy(), ""),
            );
            diagnostic.evidence = json!({ "lines": lines, "max": max });
            Ok(vec![diagnostic])
        },
    ))
}
