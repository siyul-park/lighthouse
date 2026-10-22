//! A shell command line read as the commands it runs, for the mapping of an
//! agent's events: a harness that reads and searches through the shell says
//! what it did in one command line, with pipes and several files.

/// How deep a command inside `sh -c '...'` is looked into.
const DEPTH: usize = 3;
/// Shells whose `-c` argument is a script.
const SHELLS: [&str; 5] = ["sh", "bash", "zsh", "dash", "ksh"];
/// Words that only start another command.
const WRAPPERS: [&str; 5] = ["env", "command", "time", "nohup", "exec"];

/// One command of a command line: what a pipe, `&&`, `||`, `;` or a line
/// break separates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Segment {
    /// The program, without the directory it was named in.
    pub(crate) word: String,
    /// Its arguments, unquoted, without redirections.
    pub(crate) args: Vec<String>,
    /// The command as written.
    pub(crate) text: String,
}

impl Segment {
    /// The arguments that do not start with `-`: what the command works on,
    /// and for some commands first what it looks for.
    pub(crate) fn operands(&self) -> Vec<&str> {
        self.args
            .iter()
            .map(String::as_str)
            .filter(|arg| !arg.starts_with('-'))
            .collect()
    }
}

/// A command line being cut into commands and words.
#[derive(Default)]
struct Lexer {
    /// The commands closed so far: their words, and their text.
    commands: Vec<(Vec<String>, String)>,
    /// The words of the command being read.
    words: Vec<String>,
    word: String,
    /// A word has begun (an empty `''` is one).
    started: bool,
    quote: Option<char>,
    /// Where the command being read begins.
    from: usize,
}

impl Lexer {
    fn push(&mut self, c: char) {
        self.word.push(c);
        self.started = true;
    }

    fn end_word(&mut self) {
        if self.started {
            self.words.push(std::mem::take(&mut self.word));
            self.started = false;
        }
    }

    /// Closes the command that runs up to `at`; the next begins at `after`.
    fn end_command(&mut self, line: &str, at: usize, after: usize) {
        self.end_word();
        let words = without_redirections(std::mem::take(&mut self.words));
        self.commands
            .push((words, line[self.from..at].trim().to_owned()));
        self.from = after;
    }

    /// A backslash and what follows it: inside double quotes it escapes only
    /// `"`, `\`, `$` and the backtick.
    fn escape(&mut self, next: Option<char>) {
        let Some(next) = next else { return };
        if self.quote == Some('"') && !matches!(next, '"' | '\\' | '$' | '`') {
            self.word.push('\\');
        }
        self.push(next);
    }
}

/// The commands of `line`, in order. A script given to a shell (`sh -c`,
/// `bash -lc`) is read in its turn, and a leading `VAR=value` or a wrapper
/// such as `env` is not the command.
pub(crate) fn segments(line: &str) -> Vec<Segment> {
    read(line, DEPTH)
}

fn read(line: &str, depth: usize) -> Vec<Segment> {
    split(line)
        .into_iter()
        .flat_map(|(words, text)| {
            let words = command_words(words);
            let inner = depth
                .checked_sub(1)
                .and_then(|left| script(&words).map(|script| read(&script, left)));
            match (inner, words.split_first()) {
                (Some(inner), _) => inner,
                (None, Some((word, args))) => vec![Segment {
                    word: word.rsplit('/').next().unwrap_or(word).to_owned(),
                    args: args.to_vec(),
                    text,
                }],
                (None, None) => Vec::new(),
            }
        })
        .collect()
}

/// The words of a command without what only starts it.
fn command_words(words: Vec<String>) -> Vec<String> {
    let mut words = words.into_iter().peekable();
    while let Some(first) = words.peek() {
        let assignment = first.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        if assignment || WRAPPERS.contains(&first.as_str()) {
            words.next();
        } else {
            break;
        }
    }
    words.collect()
}

/// The script a shell command runs: the argument after the option cluster
/// that holds `c`.
fn script(words: &[String]) -> Option<String> {
    let (shell, args) = words.split_first()?;
    let name = shell.rsplit('/').next().unwrap_or(shell);
    if !SHELLS.contains(&name) {
        return None;
    }
    let at = args
        .iter()
        .position(|arg| arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c'))?;
    args.get(at + 1).cloned()
}

/// The commands of `line` as their words and their text, cut at what is not
/// quoted: `|`, `||`, `&&`, `&`, `;` and a line break. Redirections are not
/// words.
fn split(line: &str) -> Vec<(Vec<String>, String)> {
    let mut lexer = Lexer::default();
    let mut chars = line.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match (lexer.quote, c) {
            (Some(q), c) if c == q => lexer.quote = None,
            (Some('"') | None, '\\') => lexer.escape(chars.next().map(|(_, next)| next)),
            (Some(_), c) => lexer.word.push(c),
            (None, '\'' | '"') => {
                lexer.quote = Some(c);
                lexer.started = true;
            }
            (None, c) if c.is_whitespace() && c != '\n' => lexer.end_word(),
            (None, '|' | '&' | ';' | '\n') => {
                lexer.end_word();
                // `2>&1` is a redirection, not a separator.
                if c == '&' && lexer.words.last().is_some_and(|w| w.ends_with('>')) {
                    continue;
                }
                // `||` and `&&` are one separator.
                if c != ';' && c != '\n' {
                    chars.next_if(|&(_, next)| next == c);
                }
                let after = chars.peek().map_or(line.len(), |&(next, _)| next);
                lexer.end_command(line, at, after);
            }
            (None, c) => lexer.push(c),
        }
    }
    lexer.end_command(line, line.len(), line.len());
    lexer
        .commands
        .into_iter()
        .filter(|(words, _)| !words.is_empty())
        .collect()
}

/// `words` without the redirections: a word that starts (after digits) with
/// `<` or `>`, and the word after it when it is the operator alone.
fn without_redirections(words: Vec<String>) -> Vec<String> {
    let mut kept = Vec::new();
    let mut skip_next = false;
    for word in words {
        if skip_next {
            skip_next = false;
            continue;
        }
        let rest = word.trim_start_matches(|c: char| c.is_ascii_digit() || c == '&');
        if rest.starts_with('<') || rest.starts_with('>') {
            skip_next = rest.chars().all(|c| c == '<' || c == '>');
            continue;
        }
        kept.push(word);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(line: &str) -> Vec<(String, Vec<String>)> {
        segments(line)
            .into_iter()
            .map(|s| (s.word, s.args))
            .collect()
    }

    fn words(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| (*p).to_owned()).collect()
    }

    #[test]
    fn segments_cut_a_pipeline_at_each_command() {
        assert_eq!(
            commands("cat a.go | rg Parse"),
            [
                ("cat".to_owned(), words(&["a.go"])),
                ("rg".to_owned(), words(&["Parse"])),
            ]
        );
    }

    #[test]
    fn segments_cut_at_and_or_semicolon_and_line_break() {
        let line = "cd pkg && ls || echo no; pwd\nid";
        let found: Vec<String> = segments(line).into_iter().map(|s| s.word).collect();

        assert_eq!(found, ["cd", "ls", "echo", "pwd", "id"]);
    }

    #[test]
    fn segments_keep_what_is_quoted_in_one_word() {
        assert_eq!(
            commands(r#"rg -n 'a | b' "dir with space" x\ y"#),
            [(
                "rg".to_owned(),
                words(&["-n", "a | b", "dir with space", "x y"])
            )]
        );
    }

    #[test]
    fn segments_read_the_script_a_shell_is_given() {
        assert_eq!(
            commands(r#"/bin/zsh -lc "sed -n '1,120p' pkg/a.go""#),
            [("sed".to_owned(), words(&["-n", "1,120p", "pkg/a.go"]))]
        );
        assert_eq!(
            commands("bash -c 'cat a.go | head -3'")
                .into_iter()
                .map(|c| c.0)
                .collect::<Vec<_>>(),
            ["cat", "head"]
        );
    }

    #[test]
    fn segments_skip_assignments_wrappers_and_the_directory_of_a_program() {
        assert_eq!(
            commands("FOO=1 env /usr/bin/cat a.go"),
            [("cat".to_owned(), words(&["a.go"]))]
        );
    }

    #[test]
    fn segments_leave_out_redirections() {
        assert_eq!(
            commands("go test ./... > out.txt 2>&1"),
            [("go".to_owned(), words(&["test", "./..."]))]
        );
        assert_eq!(
            commands("cat a.go 2> /dev/null"),
            [("cat".to_owned(), words(&["a.go"]))]
        );
    }

    #[test]
    fn segments_say_the_command_as_it_was_written() {
        let found = segments("cd x &&  go test -run cat ./...  ");

        assert_eq!(found[1].text, "go test -run cat ./...");
    }

    #[test]
    fn segments_stop_looking_into_scripts_at_a_depth() {
        let line = "sh -c \"sh -c \\\"sh -c \\\\\\\"sh -c 'cat a'\\\\\\\"\\\"\"";

        assert_eq!(commands(line).len(), 1);
    }

    #[test]
    fn operands_are_the_arguments_that_are_not_options() {
        let found = &segments("rg -n Parse pkg other")[0];

        assert_eq!(found.operands(), ["Parse", "pkg", "other"]);
    }
}
