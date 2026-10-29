//! What of a prompt is the person's own words: code blocks, quotes (a paste
//! arrives as one) and logs are not, so they are cut out before anything is matched.

use std::sync::LazyLock;

use regex::Regex;

static INLINE_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`[^`\n]*`").expect("inline code"));
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://\S+").expect("url"));
/// A line of a log, a stack trace or a compiler's output.
static LOG_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*(?:\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}|\[\d{2}:\d{2}|at\s+\S+\s*\(|File\s+\x22|Traceback\b|thread\s+'|panic(?:ked)?\b|(?:error|warning)(?:\[\w+\])?:|\s*-->\s|\d+\s*[|│]|\s*\^+\s*$|\s*(?:ERROR|WARN|INFO|DEBUG|TRACE|FATAL)\b[: \]])",
    )
    .expect("log line")
});

/// A prompt with what is not the person's words cut out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stripped {
    pub text: String,
    /// Whether code was cut: a fence, an indented block or inline code.
    pub had_code: bool,
    /// Whether pasted content, a quote or a log was cut.
    pub had_pasted: bool,
}

/// The prompt without code, quoted lines, log lines and URLs.
/// What stays is what the person wrote.
pub fn stripped(prompt: &str) -> Stripped {
    let mut pasted = false;
    let mut text = String::with_capacity(prompt.len());
    let mut fenced: Option<&str> = None;
    let mut code = false;
    for line in prompt.lines().filter(|line| !line.trim().is_empty()) {
        let trimmed = line.trim_start();
        if let Some(fence) = fenced {
            code = true;
            if trimmed.starts_with(fence) {
                fenced = None;
            }
            continue;
        }
        if let Some(fence) = ["```", "~~~"].into_iter().find(|f| trimmed.starts_with(f)) {
            fenced = Some(fence);
            code = true;
            continue;
        }
        let quoted = trimmed.starts_with('>') || LOG_LINE.is_match(line);
        if quoted || line.starts_with("    ") {
            pasted |= quoted;
            code |= !quoted;
            continue;
        }
        text.push_str(line);
        text.push('\n');
    }
    let had_inline = INLINE_CODE.is_match(&text);
    let text = INLINE_CODE.replace_all(&text, " ");
    let text = URL.replace_all(&text, " ");
    Stripped {
        text: text.trim().to_owned(),
        had_code: code || had_inline,
        had_pasted: pasted,
    }
}
