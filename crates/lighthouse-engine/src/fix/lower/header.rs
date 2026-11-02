//! The import header of a file: the statements that bind the names the rest
//! of it uses. A reorder or a move never puts a declaration above one, since
//! the declaration (a constant that reads an imported name) would run before
//! the name is bound. Imports are found by the lines that open them at the
//! start of a line; a language this does not know has no header.

/// The offset just past the last import statement that starts a line of
/// `text`, 0 when there is none.
pub(super) fn end(text: &str, language: &str) -> usize {
    let opens: fn(&str) -> bool = match language {
        "python" => |line| {
            line.starts_with("import ") || (line.starts_with("from ") && line.contains(" import "))
        },
        "typescript" => |line| line.starts_with("import ") || line.starts_with("import{"),
        "rust" => |line| {
            ["use ", "pub use ", "pub(crate) use ", "extern crate "]
                .iter()
                .any(|open| line.starts_with(open))
        },
        "go" => |line| line.starts_with("import ") || line.starts_with("import("),
        _ => return 0,
    };
    let mut last = 0;
    let mut offset = 0;
    let mut open = 0isize;
    let mut continued = false;
    for line in text.split_inclusive('\n') {
        offset += line.len();
        let body = line.trim_end();
        if open <= 0 && !continued {
            if !opens(body) {
                continue;
            }
            open = 0;
        }
        open += depth(body);
        continued = body.ends_with('\\');
        if open <= 0 && !continued {
            last = offset;
        }
    }
    last
}

/// How many brackets `line` opens, net of those it closes.
fn depth(line: &str) -> isize {
    line.bytes().fold(0, |depth, byte| match byte {
        b'(' | b'[' | b'{' => depth + 1,
        b')' | b']' | b'}' => depth - 1,
        _ => depth,
    })
}
