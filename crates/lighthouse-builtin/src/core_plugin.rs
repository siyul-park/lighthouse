use std::sync::LazyLock;

use lighthouse_model::{
    Capability, Diagnostic, Fingerprint, Fragment, Position, Span,
    annotation::{ANNOTATION_REASON, UNUSED_ALLOW},
};
use lighthouse_plugin::{
    Analyzer, AnalyzerManifest, Ctx, Error, Fixer, Indexed, LanguageProvider, Plugin,
    PluginManifest, PresetManifest, ProviderManifest, Rule, RuleManifest, Scope, Source, Workspace,
};
use lighthouse_spec::PatternRule;
use serde::Deserialize;
use serde_json::{Value, json};

const LINE_COUNT: &str = "core/line-count";
const MAX_FILE_LINES: &str = "core/max-file-lines";

pub struct Core;

impl Plugin for Core {
    fn manifest(&self) -> &PluginManifest {
        static MANIFEST: LazyLock<PluginManifest> = LazyLock::new(|| PluginManifest {
            id: "core".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        });
        &MANIFEST
    }

    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Text::new())]
    }

    fn analyzers(&self) -> Vec<Box<dyn Analyzer>> {
        vec![Box::new(LineCount)]
    }

    fn rules(&self) -> Vec<Box<dyn Rule>> {
        vec![
            max_file_lines(),
            annotation_rule(ANNOTATION_REASON),
            annotation_rule(UNUSED_ALLOW),
        ]
    }

    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        lighthouse_declarative::Declarative::bundled_fixers("core")
    }

    fn presets(&self) -> Vec<PresetManifest> {
        let rules = self.rules();
        PresetManifest::standard("core", rules.iter().map(|rule| rule.manifest()))
    }
}

/// Fallback provider: every file is plain text.
struct Text {
    manifest: ProviderManifest,
}

impl Text {
    fn new() -> Self {
        Self {
            manifest: ProviderManifest {
                fallback: true,
                capabilities: vec![Capability::Overlays],
                ..ProviderManifest::new("text", vec!["**".to_owned()])
            },
        }
    }
}

impl LanguageProvider for Text {
    fn manifest(&self) -> &ProviderManifest {
        &self.manifest
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
    fn manifest(&self) -> &AnalyzerManifest {
        static MANIFEST: LazyLock<AnalyzerManifest> = LazyLock::new(|| AnalyzerManifest {
            id: LINE_COUNT.to_owned(),
            requires: Vec::new(),
            scope: Scope::File,
        });
        &MANIFEST
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

#[derive(Deserialize)]
struct Unconfigured {}

/// A rule about allow annotations. The engine reads the annotations of the
/// whole project and reports these findings itself, because whether an
/// annotation is used depends on every other rule's findings; the rule exists
/// so that configuration, presets and the catalog treat it like any other.
fn annotation_rule(id: &'static str) -> Box<dyn Rule> {
    Box::new(PatternRule::new(
        id,
        &[],
        |_: &RuleManifest, _: &Ctx, _: Unconfigured| Ok(Vec::new()),
    ))
}

fn max_file_lines() -> Box<dyn Rule> {
    Box::new(PatternRule::new(
        MAX_FILE_LINES,
        &[LINE_COUNT],
        |meta: &RuleManifest, ctx: &Ctx, Limit { max }| {
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
