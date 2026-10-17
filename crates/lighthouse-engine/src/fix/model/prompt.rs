//! The request that asks a model to fix one finding. The same inputs always
//! write the same request, so that an answer can be kept under its digest.

use std::fmt::Write;

use lighthouse_model::{Diagnostic, Position};
use lighthouse_plugin::{CreateChatCompletionRequest, FixDecision};
use lighthouse_spec::Example;
use serde_json::{Value, json};

use super::extent::Extent;

/// How many examples of the decision the prompt shows at most.
pub(super) const MAX_EXAMPLES: usize = 3;

/// Tokens the answer may use beyond the size of the code, for its wrapper.
const ANSWER_MARGIN: usize = 256;

/// Bytes per token, to estimate how long the code is.
const BYTES_PER_TOKEN: usize = 3;

const SYSTEM: &str = "You fix one finding of a design rule in one piece of code. \
Reply with JSON {\"replacement\": \"<new text of the extent>\"} and nothing else. \
Keep behaviour. Change only what the requirement needs. Keep the code's style.";

/// What the model is told about a finding.
pub(super) struct Ask<'a> {
    pub(super) decision: &'a FixDecision,
    /// What the decision adds for its fix, such as how it is usually done.
    pub(super) instruction: Option<&'a str>,
    pub(super) finding: &'a Diagnostic,
    pub(super) language: &'a str,
    pub(super) extent: &'a Extent,
    /// Examples of the decision that show a fix, as [`examples_for`] chose them.
    pub(super) examples: &'a [Example],
}

impl Ask<'_> {
    /// The chat completion that asks for the fix. A model that takes a response
    /// format must answer the schema of `{replacement}`; a plain-text one is
    /// told the shape in the instructions only.
    pub(super) fn request(
        &self,
        structured: bool,
    ) -> Result<CreateChatCompletionRequest, serde_json::Error> {
        let mut body = json!({
            "model": "",
            "temperature": 0,
            "max_completion_tokens": self.answer_tokens(),
            "messages": [
                {"role": "system", "content": SYSTEM},
                {"role": "user", "content": self.user()},
            ],
        });
        if structured {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "replacement",
                    "strict": true,
                    "schema": {
                        "type": "object",
                        "properties": {"replacement": {"type": "string"}},
                        "required": ["replacement"],
                        "additionalProperties": false,
                    },
                },
            });
        }
        serde_json::from_value(body)
    }

    /// One and a half times the code in tokens, and a margin.
    fn answer_tokens(&self) -> usize {
        let code = self.extent.text.len().div_ceil(BYTES_PER_TOKEN);
        (code * 3).div_ceil(2) + ANSWER_MARGIN
    }

    fn user(&self) -> String {
        let (decision, finding, extent) = (self.decision, self.finding, self.extent);
        let mut text = format!(
            "Decision: {}\nRequirement: {}\n",
            decision.id, decision.requirement
        );
        if !decision.context.is_empty() {
            let _ = writeln!(text, "Context: {}", decision.context);
        }
        if let Some(instruction) = self.instruction {
            let _ = writeln!(text, "Instruction: {instruction}");
        }
        let _ = write!(text, "\nFinding: {}\n", finding.message);
        if finding.evidence != Value::Null {
            let _ = writeln!(text, "Evidence: {}", finding.evidence);
        }
        let _ = writeln!(
            text,
            "Where, as line:column in the code below: {}-{}",
            place(extent, finding.span.start),
            place(extent, finding.span.end)
        );
        let _ = write!(
            text,
            "\nFile: {} ({})\n\nCode to fix:\n",
            finding.file.display(),
            self.language
        );
        fenced(&mut text, self.language, &extent.text);
        for (n, example) in self.examples.iter().enumerate() {
            write_example(&mut text, n + 1, example);
        }
        text
    }
}

/// Appends the fixes `example` shows, as numbered before and after pairs.
pub(super) fn write_example(text: &mut String, number: usize, example: &Example) {
    for fixed in &example.fixed {
        let before = example
            .files
            .iter()
            .find(|file| file.path == fixed.path)
            .map_or("", |file| file.text());
        let _ = write!(text, "\nExample {number} ({}), before:\n", fixed.path);
        fenced(text, &example.language, before);
        let _ = write!(text, "\nExample {number} ({}), after:\n", fixed.path);
        fenced(text, &example.language, fixed.text());
    }
}

/// The examples the prompt of a finding in `language` shows, at most `limit`:
/// those that show a fix, those in that language first, each group in the
/// order of the decision.
pub(super) fn examples_for(examples: &[Example], language: &str, limit: usize) -> Vec<Example> {
    let (same, other): (Vec<&Example>, Vec<&Example>) = examples
        .iter()
        .partition(|example| example.language == language);
    same.into_iter().chain(other).take(limit).cloned().collect()
}

/// `at` as line:column of the extent, or `outside` when it is not in it.
fn place(extent: &Extent, at: Position) -> String {
    extent.relative(at).map_or_else(
        || "outside".to_owned(),
        |at| format!("{}:{}", at.line, at.col),
    )
}

/// Appends `code` in a fenced block: the fence is longer than any run of
/// backticks in the code, so the block ends where it should.
fn fenced(text: &mut String, language: &str, code: &str) {
    let longest = code
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let fence = "`".repeat(longest.max(2) + 1);
    let _ = write!(text, "{fence}{language}\n{code}");
    if !code.ends_with('\n') {
        text.push('\n');
    }
    let _ = writeln!(text, "{fence}");
}
