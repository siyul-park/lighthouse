//! `content(path)`: what a file holds, hashed once.

use std::{fs::File, io::Read, path::Path};

use lighthouse_graph::{Error, Graph};
use lighthouse_protocol::hash;
use serde::{Deserialize, Serialize};

use super::{CONTENT, Hash, OVERLAY, ROOT, STAT};

const CHUNK: usize = 256 << 10;

/// The files the walk reads the text of through the graph.
const IGNORE_FILES: [&str; 2] = [".gitignore", ".gitattributes"];

/// The identity of a file's bytes: BLAKE3 for the graph, SHA-256 (hex) for the
/// hash the providers are given, which the host's own checks compare with the
/// SHA-256 of text in memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    pub blake3: Hash,
    pub sha256: String,
    pub readable: Readable,
    /// The text of an ignore file, which the walk parses, or of an overlay.
    pub text: Option<String>,
}

impl Content {
    /// The content of text held in memory, as an overlay.
    pub fn of_text(text: &str) -> Self {
        Self {
            blake3: blake3::hash(text.as_bytes()).into(),
            sha256: hash::sha256(text),
            readable: Readable::Text,
            text: Some(text.to_owned()),
        }
    }

    pub(super) fn failed(readable: Readable, why: &str) -> Self {
        Self {
            blake3: blake3::hash(why.as_bytes()).into(),
            sha256: String::new(),
            readable,
            text: None,
        }
    }
}

/// Whether the bytes are text a language can take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Readable {
    Text,
    NotUtf8,
    Unreadable(String),
    Missing,
}

/// The text of a file, read when this runs and not kept: what the graph
/// remembers of it is its hash, which reading `content` records, so that what
/// reads the text is run again when the bytes change. `None` for a file that
/// is no text.
pub(super) fn text(graph: &Graph, path: &String) -> Result<Option<String>, Error> {
    let content = graph.query(&CONTENT, path)?;
    if content.readable != Readable::Text {
        return Ok(None);
    }
    if let Some(text) = &content.text {
        return Ok(Some(text.clone()));
    }
    let root = graph.input(&ROOT, &())?.unwrap_or_default();
    Ok(std::fs::read_to_string(Path::new(root.as_str()).join(path)).ok())
}

pub(super) fn content(graph: &Graph, path: &String) -> Result<Content, Error> {
    if let Some(overlaid) = overlay(graph, path)? {
        return Ok(overlaid);
    }
    graph.input(&STAT, path)?;
    let root = graph.input(&ROOT, &())?.unwrap_or_default();
    let file = Path::new(root.as_str()).join(path);
    let mut content = read(&file);
    if content.readable == Readable::Text && IGNORE_FILES.iter().any(|name| file.ends_with(name)) {
        content.text = std::fs::read_to_string(&file).ok();
    }
    Ok(content)
}

/// The overlay of a path, if one is set: an input that is never set for most
/// paths reads as none, and the read is recorded all the same.
fn overlay(graph: &Graph, path: &String) -> Result<Option<Content>, Error> {
    Ok(graph
        .input(&OVERLAY, path)?
        .and_then(|content| content.as_ref().clone()))
}

/// Reads the file once, in chunks, for both hashes and for whether it is
/// UTF-8.
fn read(path: &Path) -> Content {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Content::failed(Readable::Missing, "missing");
        }
        Err(e) => return Content::failed(Readable::Unreadable(e.to_string()), &e.to_string()),
    };
    let (mut blake, mut sha) = (blake3::Hasher::new(), hash::Hasher::new());
    let (mut buf, mut carry, mut utf8) = (vec![0u8; CHUNK + 4], 0usize, true);
    loop {
        let n = match file.read(&mut buf[carry..CHUNK + carry]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Content::failed(Readable::Unreadable(e.to_string()), &e.to_string()),
        };
        blake.update(&buf[carry..carry + n]);
        sha.update(&buf[carry..carry + n]);
        if utf8 {
            let end = carry + n;
            match std::str::from_utf8(&buf[..end]) {
                Ok(_) => carry = 0,
                Err(e) if e.error_len().is_none() => {
                    let from = e.valid_up_to();
                    buf.copy_within(from..end, 0);
                    carry = end - from;
                }
                Err(_) => utf8 = false,
            }
        }
    }
    Content {
        blake3: blake.finalize().into(),
        sha256: sha.hex(),
        readable: if utf8 && carry == 0 {
            Readable::Text
        } else {
            Readable::NotUtf8
        },
        text: None,
    }
}
