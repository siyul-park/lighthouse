//! What an agent did, from the JSON lines its program prints: the mapping the
//! provider declares (`events`, in CEL) turns each line into tool events, so
//! that nothing here knows which harness printed it. The lines are read as the
//! program prints them, whatever the size of the stream, and only the events
//! and a few kilobytes of what the agent said are kept.

use std::{
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use cel::{Context, ExecutionError, FunctionContext, Program, Value};
use lighthouse_process::Lines;
use lighthouse_protocol::{Tool, ToolEvent};
use lighthouse_spec::{EventsSpec, compile, context};
use serde_json::json;

use crate::shell;

/// How much of what an agent said is kept, from the end: its reply to the
/// brief is the last of it.
pub(crate) const NOTES_BYTES: usize = 2048;
/// How many events of one run are kept; the rest are counted.
pub(crate) const MAX_EVENTS: usize = 10_000;
/// How many lines are still read once [`MAX_EVENTS`] are kept, to count what
/// they hold and to hear the end of what the agent said; the rest are skipped.
const LINES_PAST_CAP: usize = 1_000;

/// Rewrites what a line printed before it is read, such as hiding secrets.
pub(crate) type Scrubber = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// The mapping of a provider, compiled.
pub(crate) struct Events {
    select: Program,
    each: Option<Program>,
    tool: Program,
    path: Option<Program>,
    pattern: Option<Program>,
    command: Option<Program>,
    said: Option<Program>,
    /// The context every line is judged in: the functions and the table of
    /// tool names.
    base: Context<'static, 'static>,
}

/// What a run of an agent printed, as the report keeps it.
pub(crate) struct Mapped {
    pub(crate) events: Vec<ToolEvent>,
    /// The end of what it said.
    pub(crate) said: String,
    /// Why `events` is not all it did, when it is not.
    pub(crate) incomplete: Option<String>,
}

impl Events {
    /// Compiles `spec`, or says which expression does not.
    pub(crate) fn new(spec: &EventsSpec) -> Result<Self, String> {
        let compile =
            |key: &str, source: &str| compile(source).map_err(|e| format!("`events.{key}`: {e}"));
        let optional = |key: &str, source: &Option<String>| {
            source.as_deref().map(|s| compile(key, s)).transpose()
        };
        let mut base = context().map_err(|e| format!("`events`: {e}"))?;
        base.add_function("commands", commands)
            .map_err(|e| format!("`events`: {e}"))?;
        let tools = cel::to_value(&spec.tools).map_err(|e| format!("`events.tools`: {e}"))?;
        base.add_variable_from_value("tools", tools);
        Ok(Self {
            select: compile("select", &spec.select)?,
            each: optional("each", &spec.each)?,
            tool: compile("tool", &spec.tool)?,
            path: optional("path", &spec.path)?,
            pattern: optional("pattern", &spec.pattern)?,
            command: optional("command", &spec.command)?,
            said: optional("said", &spec.said)?,
            base,
        })
    }

    /// What one line says: the calls it holds, each as the events of its
    /// paths, and what the agent said. No more than `limit` events are built, and
    /// none when it is 0: the line is read for what was said only.
    fn map(&self, line: &Value, limit: usize) -> (Vec<Vec<ToolEvent>>, String) {
        let mut context = self.base.new_inner_scope();
        context.add_variable_from_value("line", line.clone());
        let said = text(self.said.as_ref(), &context).unwrap_or_default();
        if limit == 0 || !matches!(self.select.execute(&context), Ok(Value::Bool(true))) {
            return (Vec::new(), said);
        }
        let items: Vec<Option<Value>> = match &self.each {
            None => vec![None],
            Some(each) => match each.execute(&context) {
                Ok(Value::List(items)) => items.iter().cloned().map(Some).collect(),
                _ => Vec::new(),
            },
        };
        let mut calls = Vec::new();
        let mut built = 0;
        for item in items {
            if built >= limit {
                break;
            }
            let mut inner = context.new_inner_scope();
            if let Some(item) = item {
                inner.add_variable_from_value("item", item);
            }
            let events = self.call(&inner, limit - built);
            if !events.is_empty() {
                built += events.len();
                calls.push(events);
            }
        }
        (calls, said)
    }

    /// The events one call is: one for each path it names, up to `limit`.
    fn call(&self, context: &Context<'_, '_>, limit: usize) -> Vec<ToolEvent> {
        let tool = match self.tool.execute(context) {
            Ok(Value::String(word)) => match Tool::parse(&word) {
                Some(tool) => tool,
                None => return Vec::new(),
            },
            _ => return Vec::new(),
        };
        let pattern = text(self.pattern.as_ref(), context);
        let command = text(self.command.as_ref(), context);
        paths(self.path.as_ref(), context)
            .into_iter()
            .take(limit)
            .map(|path| ToolEvent {
                tool,
                path,
                outside: false,
                pattern: pattern.clone(),
                command: command.clone(),
            })
            .collect()
    }
}

/// One run of an agent being read.
#[derive(Clone)]
pub(crate) struct Reading {
    events: Arc<Events>,
    roots: Arc<Roots>,
    scrub: Scrubber,
    found: Arc<Mutex<Found>>,
}

#[derive(Default)]
struct Found {
    /// The calls read, each as the events of the paths it names.
    calls: Vec<Vec<ToolEvent>>,
    /// How many events `calls` hold.
    kept: usize,
    /// Events past [`MAX_EVENTS`] that were seen: at least that many.
    dropped: usize,
    /// Lines read since [`MAX_EVENTS`] were kept.
    past: usize,
    /// Lines skipped for being too many past that.
    unread: usize,
    said: String,
}

impl Reading {
    /// A reading of the lines of one run in `root` through `events`:
    /// [`Reading::lines`] takes them as they are printed and
    /// [`Reading::finish`] gives what they say.
    pub(crate) fn new(events: &Arc<Events>, root: &Path, scrub: Scrubber) -> Self {
        Self {
            events: Arc::clone(events),
            roots: Arc::new(Roots::of(root)),
            scrub,
            found: Arc::default(),
        }
    }

    /// The receiver of the lines of the run, which reads each as it comes.
    pub(crate) fn lines(&self) -> Lines {
        let reading = self.clone();
        Arc::new(move |line| reading.feed(line))
    }

    /// What the lines said, once the run is over; `cut` when the program
    /// printed more than was read.
    pub(crate) fn finish(&self, cut: bool) -> Mapped {
        let found = std::mem::take(&mut *self.found.lock().unwrap_or_else(PoisonError::into_inner));
        let mut notes = Vec::new();
        if found.dropped > 0 {
            notes.push(format!(
                "at least {} event(s) past the first {MAX_EVENTS} were not kept",
                found.dropped
            ));
        }
        if found.unread > 0 {
            notes.push(format!(
                "{} line(s) past the event limit were read for what the agent said only",
                found.unread
            ));
        }
        if cut {
            notes.push("a line of its output was too long to read".to_owned());
        }
        let events = found
            .calls
            .into_iter()
            .flat_map(|call| {
                let placed = call.into_iter().map(|event| self.roots.place(event));
                self.roots.ground(placed.collect())
            })
            .collect();
        Mapped {
            events,
            said: tail(found.said.trim(), NOTES_BYTES).to_owned(),
            incomplete: (!notes.is_empty()).then(|| notes.join("; ")),
        }
    }

    /// Reads one line: a line that is not JSON says nothing. What it says is
    /// scrubbed once more after it is read, for a secret that the JSON
    /// escaped.
    fn feed(&self, line: &[u8]) {
        let limit = self.admit();
        let scrubbed = (self.scrub)(&String::from_utf8_lossy(line));
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&scrubbed) else {
            return;
        };
        let Ok(value) = cel::to_value(&json) else {
            return;
        };
        // Past the lines still read for events, a line is read for what the
        // agent said only: its final reply is never lost to the cap.
        let (calls, said) = self.events.map(&value, limit);
        let said = (self.scrub)(&said);
        let mut found = self.found.lock().unwrap_or_else(PoisonError::into_inner);
        if !said.is_empty() {
            if !found.said.is_empty() {
                found.said.push('\n');
            }
            found.said.push_str(&said);
            let over = found.said.len().saturating_sub(2 * NOTES_BYTES);
            if over > 0 {
                let from = found.said.len() - tail(&found.said, NOTES_BYTES).len();
                found.said.drain(..from);
            }
        }
        for call in calls {
            if found.kept + call.len() <= MAX_EVENTS {
                found.kept += call.len();
                let hidden = call.into_iter().map(|event| self.hide(event));
                found.calls.push(hidden.collect());
            } else {
                found.dropped += call.len();
            }
        }
    }

    /// How many events a new line may build: one more than is left, to see
    /// that there are more; none when the lines past the event limit are too
    /// many, and the line is then read for what was said only.
    fn admit(&self) -> usize {
        let mut found = self.found.lock().unwrap_or_else(PoisonError::into_inner);
        if found.kept >= MAX_EVENTS {
            if found.past >= LINES_PAST_CAP {
                found.unread += 1;
                return 0;
            }
            found.past += 1;
        }
        MAX_EVENTS - found.kept + 1
    }

    /// `event` with the secrets hidden in what it carries.
    fn hide(&self, event: ToolEvent) -> ToolEvent {
        let hide = |text: Option<String>| text.map(|t| (self.scrub)(&t));
        ToolEvent {
            path: hide(event.path),
            pattern: hide(event.pattern),
            command: hide(event.command),
            ..event
        }
    }
}

/// The directory an agent ran in, as the paths it may report name it.
pub(crate) struct Roots {
    /// The directory, as it was given.
    dir: PathBuf,
    /// The forms of it that a path may name it by: as given, and with its
    /// links resolved.
    forms: Vec<PathBuf>,
}

impl Roots {
    /// The roots of `root`.
    pub(crate) fn of(root: &Path) -> Self {
        let given = clean(root);
        let resolved = root.canonicalize().ok().and_then(|path| clean(&path));
        let mut forms: Vec<PathBuf> = given.into_iter().chain(resolved).collect();
        forms.dedup();
        Self {
            dir: root.to_owned(),
            forms,
        }
    }

    /// The events of one call with the paths that name nothing dropped, after
    /// the run: the path of a `read` counts when it names an existing file
    /// under the directory, and that of a `search` an existing file or
    /// directory, whatever the harness made of its command line. A call whose
    /// paths all name nothing is still a call, with no path.
    pub(crate) fn ground(&self, call: Vec<ToolEvent>) -> Vec<ToolEvent> {
        let Some(first) = call.first().cloned() else {
            return call;
        };
        let kept: Vec<ToolEvent> = call
            .into_iter()
            .filter(|event| self.names_something(event))
            .collect();
        if kept.is_empty() {
            vec![ToolEvent {
                path: None,
                ..first
            }]
        } else {
            kept
        }
    }

    fn names_something(&self, event: &ToolEvent) -> bool {
        let Some(path) = &event.path else {
            return true;
        };
        // Never through a link: one is not a file of the directory.
        let found = fs::symlink_metadata(self.dir.join(path));
        match event.tool {
            Tool::Read => found.is_ok_and(|meta| meta.is_file()),
            Tool::Search => found.is_ok(),
            _ => true,
        }
    }

    /// `event` with its path made relative to the root, folded lexically
    /// (`./`, `//`, `a/..`). A path outside the root, or one that crosses a
    /// link of the directory (which may lead anywhere), is dropped and the
    /// event marked `outside`, so that no path of the machine is kept. A
    /// pattern that is a path is placed the same way.
    pub(crate) fn place(&self, event: ToolEvent) -> ToolEvent {
        let event = match event.pattern.as_deref().filter(|p| names_a_path(p)) {
            Some(pattern) => match self.inside(pattern) {
                Some(pattern) => ToolEvent {
                    pattern: Some(pattern),
                    ..event
                },
                None => ToolEvent {
                    pattern: None,
                    outside: true,
                    ..event
                },
            },
            None => event,
        };
        let Some(path) = &event.path else {
            return event;
        };
        match self.inside(path) {
            Some(path) => ToolEvent {
                path: Some(path),
                ..event
            },
            None => ToolEvent {
                path: None,
                outside: true,
                ..event
            },
        }
    }

    /// `path` below the root and not through a link, as `locate` gives it.
    fn inside(&self, path: &str) -> Option<String> {
        let located = self.locate(path)?;
        (!self.crosses_link(path)).then_some(located)
    }

    /// Whether `path`, followed component by component from the root as the
    /// system would (`..` after a link leaves the link's target, not the
    /// link's directory), passes through a link.
    fn crosses_link(&self, path: &str) -> bool {
        let raw = Path::new(path);
        let below = if raw.is_absolute() {
            match self
                .forms
                .iter()
                .find_map(|root| raw.strip_prefix(root).ok())
            {
                Some(below) => below,
                None => return false,
            }
        } else {
            raw
        };
        let mut at = self.dir.clone();
        for component in below.components() {
            match component {
                Component::Normal(name) => {
                    at.push(name);
                    if fs::symlink_metadata(&at).is_ok_and(|meta| meta.is_symlink()) {
                        return true;
                    }
                }
                Component::ParentDir => {
                    at.pop();
                }
                _ => {}
            }
        }
        false
    }

    /// `path` below the root (`.` for the root itself); `None` when it is not.
    fn locate(&self, path: &str) -> Option<String> {
        let cleaned = clean(Path::new(path))?;
        let inside = if cleaned.is_absolute() {
            self.forms
                .iter()
                .find_map(|root| cleaned.strip_prefix(root).ok())?
        } else {
            &cleaned
        };
        Some(if inside.as_os_str().is_empty() {
            ".".to_owned()
        } else {
            inside
                .components()
                .map(Component::as_os_str)
                .map(OsStr::to_string_lossy)
                .collect::<Vec<_>>()
                .join("/")
        })
    }
}

/// `text` cut to its last `max` bytes, at a character boundary.
pub(crate) fn tail(text: &str, max: usize) -> &str {
    let from = (text.len().saturating_sub(max)..=text.len())
        .find(|&at| text.is_char_boundary(at))
        .unwrap_or(text.len());
    &text[from..]
}

/// Whether a search pattern is a path of the machine: absolute, in one word.
fn names_a_path(pattern: &str) -> bool {
    pattern.starts_with('/') && !pattern.contains(char::is_whitespace)
}

/// The text an expression gives, when it gives a string that is not empty.
fn text(program: Option<&Program>, context: &Context<'_, '_>) -> Option<String> {
    match program?.execute(context) {
        Ok(Value::String(text)) if !text.is_empty() => Some(text.to_string()),
        _ => None,
    }
}

/// The paths of a call: a string is one, a list is each of its strings, and
/// a call with none is one path-less entry.
fn paths(program: Option<&Program>, context: &Context<'_, '_>) -> Vec<Option<String>> {
    let named = |value: &Value| match value {
        Value::String(path) if !path.is_empty() => Some(path.to_string()),
        _ => None,
    };
    let found: Vec<Option<String>> = match program.map(|p| p.execute(context)) {
        Some(Ok(Value::List(items))) => items
            .iter()
            .filter_map(|item| named(item).map(Some))
            .collect(),
        Some(Ok(value)) => named(&value).map(Some).into_iter().collect(),
        _ => Vec::new(),
    };
    if found.is_empty() { vec![None] } else { found }
}

/// `commands(line)`: the commands of a shell command line, each as a map of
/// `word` (the program), `args`, `operands` (the arguments that do not start
/// with `-`), `rest` (the operands after the first) and `text`.
fn commands(ftx: &FunctionContext, line: Arc<String>) -> Result<Value, ExecutionError> {
    let found: Vec<serde_json::Value> = shell::segments(&line)
        .into_iter()
        .map(|segment| {
            let operands = segment.operands();
            json!({
                "word": segment.word,
                "args": segment.args,
                "operands": operands,
                "rest": operands.iter().skip(1).collect::<Vec<_>>(),
                "text": segment.text,
            })
        })
        .collect();
    cel::to_value(found).map_err(|e| ftx.error(e.to_string()))
}

/// `path` folded lexically: no `.`, no empty component, `a/..` gone. A
/// relative path that climbs above where it starts has no such form.
fn clean(path: &Path) -> Option<PathBuf> {
    let mut folded = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !folded.pop() && !path.is_absolute() {
                    return None;
                }
            }
            other => folded.push(other.as_os_str()),
        }
    }
    Some(folded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> Roots {
        Roots {
            dir: PathBuf::from("/work/scratch"),
            forms: vec![PathBuf::from("/work/scratch")],
        }
    }

    #[test]
    fn locate_makes_a_path_below_the_root_relative() {
        assert_eq!(
            roots().locate("/work/scratch/pkg/a.go").as_deref(),
            Some("pkg/a.go")
        );
        assert_eq!(roots().locate("/work/scratch").as_deref(), Some("."));
    }

    #[test]
    fn locate_folds_dots_double_slashes_and_parents() {
        let roots = roots();

        assert_eq!(roots.locate("./pkg//a.go").as_deref(), Some("pkg/a.go"));
        assert_eq!(roots.locate("pkg/../b.go").as_deref(), Some("b.go"));
        assert_eq!(
            roots.locate("/work/scratch//pkg/./x/../a.go").as_deref(),
            Some("pkg/a.go")
        );
        assert_eq!(roots.locate("pkg/..").as_deref(), Some("."));
    }

    #[test]
    fn locate_refuses_what_climbs_out_of_the_root() {
        let roots = roots();

        assert_eq!(roots.locate("../a.go"), None);
        assert_eq!(roots.locate("pkg/../../a.go"), None);
        assert_eq!(roots.locate("/work/scratch/../other/a.go"), None);
        assert_eq!(roots.locate("/etc/passwd"), None);
    }

    #[test]
    fn locate_does_not_mistake_a_sibling_with_the_same_prefix_for_the_root() {
        assert_eq!(roots().locate("/work/scratch2/a.go"), None);
    }

    #[test]
    fn locate_knows_every_root_the_directory_has() {
        let roots = Roots {
            dir: PathBuf::from("/var/folders/x/scratch"),
            forms: vec![
                PathBuf::from("/var/folders/x/scratch"),
                PathBuf::from("/private/var/folders/x/scratch"),
            ],
        };

        assert_eq!(
            roots
                .locate("/private/var/folders/x/scratch/a.go")
                .as_deref(),
            Some("a.go")
        );
        assert_eq!(
            roots.locate("/var/folders/x/scratch/a.go").as_deref(),
            Some("a.go")
        );
    }

    #[test]
    fn place_drops_a_path_outside_and_marks_the_event() {
        let event = |path: &str| ToolEvent {
            tool: Tool::Read,
            path: Some(path.to_owned()),
            outside: false,
            pattern: None,
            command: None,
        };

        let outside = roots().place(event("/Users/someone/.ssh/id_rsa"));
        let inside = roots().place(event("/work/scratch/a.go"));

        assert_eq!((outside.path, outside.outside), (None, true));
        assert_eq!(
            (inside.path.as_deref(), inside.outside),
            (Some("a.go"), false)
        );
    }

    #[test]
    fn tail_cuts_at_a_character_boundary() {
        assert_eq!(tail("héllo", 3), "llo");
        assert_eq!(tail("héllo", 4), "llo");
        assert_eq!(tail("héllo", 5), "éllo");
        assert_eq!(tail("ab", 9), "ab");
    }

    #[test]
    fn place_marks_a_pattern_that_is_a_path_outside_and_keeps_the_other_patterns() {
        let search = |pattern: &str| ToolEvent {
            tool: Tool::Search,
            path: None,
            outside: false,
            pattern: Some(pattern.to_owned()),
            command: None,
        };

        let outside = roots().place(search("/Users/someone/.ssh/id_rsa"));
        let inside = roots().place(search("/work/scratch/pkg/a.go"));
        let plain = roots().place(search("func /Parse/"));
        let regex = roots().place(search("/Parse.*/"));

        assert_eq!((outside.pattern, outside.outside), (None, true));
        assert_eq!(
            (inside.pattern.as_deref(), inside.outside),
            (Some("pkg/a.go"), false)
        );
        assert_eq!(plain.pattern.as_deref(), Some("func /Parse/"));
        assert_eq!(regex.pattern, None, "an absolute word is taken for a path");
    }

    #[cfg(unix)]
    mod links {
        use std::os::unix::fs::symlink;

        use super::*;

        fn read(path: &str) -> ToolEvent {
            ToolEvent {
                tool: Tool::Read,
                path: Some(path.to_owned()),
                outside: false,
                pattern: None,
                command: None,
            }
        }

        /// A directory with `real/a.go`, `link` to `real`, `file-link` to
        /// `real/a.go`.
        fn scratch() -> (tempfile::TempDir, Roots) {
            let dir = tempfile::tempdir().unwrap();
            fs::create_dir(dir.path().join("real")).unwrap();
            fs::write(dir.path().join("real/a.go"), "x").unwrap();
            symlink("real", dir.path().join("link")).unwrap();
            symlink("real/a.go", dir.path().join("file-link")).unwrap();
            let roots = Roots::of(dir.path());
            (dir, roots)
        }

        #[test]
        fn a_path_that_crosses_a_link_is_outside_and_has_no_name() {
            let (_dir, roots) = scratch();

            for path in [
                "link/a.go",
                "file-link",
                "link/../real/a.go",
                "real/../link",
            ] {
                let placed = roots.place(read(path));
                assert_eq!((placed.path, placed.outside), (None, true), "{path}");
            }
            let plain = roots.place(read("real/a.go"));
            assert_eq!(plain.path.as_deref(), Some("real/a.go"));
        }

        #[test]
        fn ground_does_not_follow_a_link_to_a_file() {
            let (_dir, roots) = scratch();
            let linked = ToolEvent {
                path: Some("file-link".to_owned()),
                ..read("")
            };

            let kept = roots.ground(vec![linked]);

            assert_eq!(kept.len(), 1);
            assert_eq!(kept[0].path, None);
        }
    }

    fn call(tool: Tool, paths: &[&str]) -> Vec<ToolEvent> {
        paths
            .iter()
            .map(|path| ToolEvent {
                tool,
                path: Some((*path).to_owned()),
                outside: false,
                pattern: None,
                command: None,
            })
            .collect()
    }

    fn kept(events: &[ToolEvent]) -> Vec<Option<&str>> {
        events.iter().map(|e| e.path.as_deref()).collect()
    }

    #[test]
    fn ground_keeps_a_file_without_a_dot_and_drops_the_operand_that_is_no_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Makefile"), "all:\n").unwrap();
        std::fs::write(dir.path().join("x.go"), "package x\n").unwrap();
        let roots = Roots::of(dir.path());

        let make = roots.ground(call(Tool::Read, &["Makefile"]));
        let sed = roots.ground(call(Tool::Read, &["s/a/b/", "x.go"]));

        assert_eq!(kept(&make), [Some("Makefile")]);
        assert_eq!(kept(&sed), [Some("x.go")]);
    }

    #[test]
    fn ground_lets_a_search_name_a_directory_and_a_read_only_a_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("pkg")).unwrap();
        let roots = Roots::of(dir.path());

        let search = roots.ground(call(Tool::Search, &["pkg", "."]));
        let read = roots.ground(call(Tool::Read, &["pkg"]));

        assert_eq!(kept(&search), [Some("pkg"), Some(".")]);
        assert_eq!(kept(&read), [None], "a call whose paths name nothing stays");
    }

    #[test]
    fn ground_leaves_alone_what_is_not_a_read_or_a_search() {
        let roots = Roots::of(tempfile::tempdir().unwrap().path());

        let edit = roots.ground(call(Tool::Edit, &["new.go"]));
        let list = roots.ground(call(Tool::List, &["*.go"]));

        assert_eq!(kept(&edit), [Some("new.go")]);
        assert_eq!(kept(&list), [Some("*.go")]);
    }
}
