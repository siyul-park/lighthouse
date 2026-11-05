//! `content(path)`: what a file holds, hashed once.

use std::{fs::File, io::Read, path::Path};

use lighthouse_graph::{Db, Error};
use lighthouse_protocol::hash;
use serde::{Deserialize, Serialize};

use super::{Hash, OVERLAY, ROOT, STAT};

const CHUNK: usize = 256 << 10;

/// Whether the bytes are text a language can take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Readable {
    Text,
    NotUtf8,
    Unreadable(String),
    Missing,
}

/// The identity of a file's bytes: BLAKE3 for the graph, SHA-256 (hex) for the
/// hash the providers are given, which the host's own checks compare with the
/// SHA-256 of text in memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Content {
    pub blake3: Hash,
    pub sha256: String,
    pub readable: Readable,
}

impl Content {
    /// The content of text held in memory, as an overlay.
    pub(crate) fn of_text(text: &str) -> Self {
        Self {
            blake3: blake3::hash(text.as_bytes()).into(),
            sha256: hash::sha256(text),
            readable: Readable::Text,
        }
    }

    fn failed(readable: Readable, why: &str) -> Self {
        Self {
            blake3: blake3::hash(why.as_bytes()).into(),
            sha256: String::new(),
            readable,
        }
    }
}

/// The overlay of a path, if one is set: an input that is never set for most
/// paths reads as none.
fn overlay(db: &Db, path: &String) -> Result<Option<Content>, Error> {
    match db.input(&OVERLAY, path) {
        Ok(content) => Ok(content.as_ref().clone()),
        Err(Error::MissingInput(_)) => Ok(None),
        Err(other) => Err(other),
    }
}

pub(super) fn content(db: &Db, path: &String) -> Result<Content, Error> {
    if let Some(overlaid) = overlay(db, path)? {
        return Ok(overlaid);
    }
    let (_stat, _salt) = *db.input(&STAT, path)?;
    let root = db.input(&ROOT, &())?;
    Ok(read(&Path::new(root.as_str()).join(path)))
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
    }
}
