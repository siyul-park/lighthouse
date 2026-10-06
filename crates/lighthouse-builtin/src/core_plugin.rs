use lighthouse_config::{RuleConfig, Rules};
use lighthouse_model::{
    Capability, Diagnostic, File, Fingerprint, Fragment, Options, Position, Severity, Span,
};
use lighthouse_plugin::{
    Analyzer, Conventions, Ctx, Error, LanguageProvider, Manifest, Plugin, Preset, Rule, RuleMeta,
    Scope, Workspace,
};
use serde::Deserialize;
use serde_json::{Value, json};

const LINE_COUNT: &str = "core/line-count";
const MAX_FILE_LINES: &str = "core/max-file-lines";
const DEFAULT_MAX: usize = 1000;

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
        vec![Box::new(MaxFileLines::new())]
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

    fn index(&self, _: &Workspace, file: &File, _: &str) -> Result<Fragment, Error> {
        Ok(Fragment {
            files: vec![file.clone()],
            ..Fragment::default()
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
#[serde(deny_unknown_fields, default)]
struct MaxOptions {
    max: usize,
}

impl Default for MaxOptions {
    fn default() -> Self {
        Self { max: DEFAULT_MAX }
    }
}

struct MaxFileLines {
    meta: RuleMeta,
}

impl MaxFileLines {
    fn new() -> Self {
        Self {
            meta: RuleMeta {
                id: MAX_FILE_LINES.to_owned(),
                severity: Severity::Warn,
                scope: Scope::File,
                description: "file exceeds the maximum number of lines".to_owned(),
                docs: "Large files tend to mix responsibilities. Split by cohesion, not by size alone.\n\
                       Option `max` (default 1000) sets the line limit."
                    .to_owned(),
                analyzers: vec![LINE_COUNT.to_owned()],
                capabilities: Vec::new(),
                citation: None,
            },
        }
    }
}

impl Rule for MaxFileLines {
    fn meta(&self) -> &RuleMeta {
        &self.meta
    }

    fn validate(&self, options: &Options) -> Result<(), Error> {
        lighthouse_plugin::options::<MaxOptions>(MAX_FILE_LINES, options).map(drop)
    }

    fn check(&self, ctx: &Ctx, options: &Options) -> Result<Vec<Diagnostic>, Error> {
        let MaxOptions { max } = lighthouse_plugin::options(MAX_FILE_LINES, options)?;
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
            Severity::Warn,
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
    }
}
