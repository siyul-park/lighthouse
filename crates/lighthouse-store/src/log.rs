//! The decision log: `.lighthouse/decisions.jsonl`, one review event per line,
//! append-only and meant to be committed. Concurrent branches that both
//! appended merge by concatenation (`merge=union`), and the same event read
//! twice is one event.

use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use serde_json::Value;

use crate::{Error, ReviewEvent, digest};

/// File name of the log inside the store directory.
pub(crate) const FILE: &str = "decisions.jsonl";
/// Length of an event id: the hash of the event without it.
const ID_BYTES: usize = 16;
/// Ids of entries imported from a store that predates the log.
pub(crate) const LEGACY_PREFIX: &str = "legacy-";

/// Gives the event the id its content determines.
pub(crate) fn seal(event: &mut ReviewEvent) -> Result<(), Error> {
    event.id = id_of(event)?;
    Ok(())
}

/// The line of an event: canonical JSON with keys in sorted order, so that
/// the same event is always the same bytes.
pub(crate) fn line(event: &ReviewEvent) -> Result<String, Error> {
    Ok(digest::canonical(&serde_json::to_value(event)?))
}

/// Every event of the log, in file order; none if there is no log. A line
/// that is not an event, or whose id does not match its content, is an error
/// naming the line: a decision that cannot be read must not be skipped.
pub(crate) fn read(path: &Path) -> Result<Vec<ReviewEvent>, Error> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(io(path, source)),
    };
    let mut events = Vec::new();
    for (index, text) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let fail = |reason: String| Error::Log {
            path: path.display().to_string(),
            line: index + 1,
            reason,
        };
        let event: ReviewEvent = serde_json::from_str(text).map_err(|e| fail(e.to_string()))?;
        if !event.id.starts_with(LEGACY_PREFIX) && id_of(&event)? != event.id {
            return Err(fail("the id does not match the entry".to_owned()));
        }
        event
            .verdict
            .validate(event.reason)
            .map_err(|e| fail(e.to_string()))?;
        events.push(event);
    }
    Ok(events)
}

/// Appends one event as a line, in a single write, and flushes it to disk
/// before returning.
pub(crate) fn append(path: &Path, event: &ReviewEvent) -> Result<(), Error> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|source| io(dir, source))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)
        .map_err(|source| io(path, source))?;
    let mut text = String::new();
    if ends_without_newline(&mut file).map_err(|source| io(path, source))? {
        text.push('\n');
    }
    text.push_str(&line(event)?);
    text.push('\n');
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|source| io(path, source))
}

fn id_of(event: &ReviewEvent) -> Result<String, Error> {
    let mut body = serde_json::to_value(event)?;
    if let Value::Object(map) = &mut body {
        map.remove("id");
    }
    Ok(digest::hash(&digest::canonical(&body), ID_BYTES))
}

fn ends_without_newline(file: &mut fs::File) -> std::io::Result<bool> {
    if file.metadata()?.len() == 0 {
        return Ok(false);
    }
    let mut last = [0u8; 1];
    file.seek(SeekFrom::End(-1))?;
    file.read_exact(&mut last)?;
    Ok(last[0] != b'\n')
}

fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.display().to_string(),
        source,
    }
}
