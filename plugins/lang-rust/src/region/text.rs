//! The text of a fence as the compiler reads it, and where each byte of it lies
//! in the host file. rustdoc's reading is followed: a `# ` line is code that
//! the page hides, and `##` stands for a shown `#`; a fence with statements
//! and no item around them is the body of a `fn main`.

use lighthouse_protocol::EmbeddedSource;

const WRAP_HEAD: &str = "fn main() {\n";
const WRAP_TAIL: &str = "}\n";

/// The text of a fence, one entry per byte telling the host bytes it came from
/// and `None` for the bytes the host does not show: hidden lines and the
/// lines that wrap the fence.
pub(super) struct Text {
    pub(super) text: String,
    host: Vec<Option<[usize; 2]>>,
}

impl Text {
    /// The fence as it is written, with its hidden lines unhidden.
    pub(super) fn of(source: &EmbeddedSource) -> Self {
        let bytes = host_bytes(source);
        let mut text = Self {
            text: String::new(),
            host: Vec::new(),
        };
        let mut at = 0;
        for line in source.text.split_inclusive('\n') {
            let host = &bytes[at..at + line.len()];
            at += line.len();
            text.line(line, host);
        }
        text
    }

    /// The same text as the body of `fn main`; its lines are the host's none.
    pub(super) fn wrapped(&self) -> Self {
        let mut text = String::from(WRAP_HEAD);
        let mut host: Vec<Option<[usize; 2]>> = vec![None; WRAP_HEAD.len()];
        text.push_str(&self.text);
        host.extend(&self.host);
        if !self.text.ends_with('\n') {
            text.push('\n');
            host.push(None);
        }
        text.push_str(WRAP_TAIL);
        host.extend(vec![None; WRAP_TAIL.len()]);
        Self { text, host }
    }

    /// The host bytes of the bytes `start..end` of the text: from where the
    /// first starts to where the last the host shows ends. `None` when the
    /// host shows none of them, as for a hidden line or the wrapper.
    pub(super) fn host(&self, start: usize, end: usize) -> Option<[usize; 2]> {
        let from = self.host.get(start).copied().flatten()?;
        let last = (start..end.max(start + 1))
            .rev()
            .find_map(|at| self.host.get(at).copied().flatten())?;
        Some([from[0], last[1].max(from[0])])
    }

    fn line(&mut self, line: &str, host: &[[usize; 2]]) {
        let body = line.trim_end_matches(['\r', '\n']);
        let indent = body.len() - body.trim_start().len();
        let rest = &body[indent..];
        if rest == "#" || rest.starts_with("# ") {
            let hidden = indent + if rest == "#" { 1 } else { 2 };
            self.push(&line[..indent], &host[..indent], false);
            self.push(&line[hidden..], &host[hidden..], false);
        } else if rest.starts_with("##") {
            self.push(&line[..indent], &host[..indent], true);
            self.push(&line[indent + 1..], &host[indent + 1..], true);
        } else {
            self.push(line, host, true);
        }
    }

    fn push(&mut self, text: &str, host: &[[usize; 2]], shown: bool) {
        self.text.push_str(text);
        self.host.extend(host.iter().map(|at| shown.then_some(*at)));
    }
}

/// The host bytes each byte of the fence's text comes from: the segment's own
/// bytes when it is copied as is, the whole segment when it was changed on the
/// way (a tab, a line ending), the whole fence without a map.
fn host_bytes(source: &EmbeddedSource) -> Vec<[usize; 2]> {
    let [from, to] = source.source_range.map(|at| at as usize);
    let mut bytes = vec![[from, to]; source.text.len()];
    let linear = to - from == source.text.len();
    let Some(segments) = &source.text_map else {
        if linear {
            for (at, entry) in bytes.iter_mut().enumerate() {
                *entry = [from + at, from + at + 1];
            }
        }
        return bytes;
    };
    for segment in segments {
        let [text_from, text_to] = segment.text_range.map(|at| at as usize);
        let [host_from, host_to] = segment.source_range.map(|at| at as usize);
        let linear = text_to - text_from == host_to - host_from;
        for (offset, entry) in bytes[text_from..text_to].iter_mut().enumerate() {
            *entry = if linear {
                [host_from + offset, host_from + offset + 1]
            } else {
                [host_from, host_to]
            };
        }
    }
    bytes
}
