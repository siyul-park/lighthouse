//! The provider of files that expose a syntax tree, with a parser that reads
//! `key=value` lines.

use std::collections::BTreeMap;

use lighthouse_protocol::{
    Context, EmbeddedFile, EmbeddedRegion, FileRef, IndexParams, IndexResult, Overlay, ProjectRef,
    TextOffsets, UnitRef,
    language::{HostError, HostLanguage, HostParser, HostProvider, HostTree, Provider},
};
use serde_json::json;

struct Lines;

impl HostParser for Lines {
    fn language(&self) -> HostLanguage {
        LANGUAGE
    }

    fn parse(&self, text: &str) -> Result<HostTree, HostError> {
        if text.contains('!') {
            return Err(HostError::Syntax("a bang".to_owned()));
        }
        if text.is_empty() {
            return Err(HostError::Tree("nothing to select".to_owned()));
        }
        Ok(HostTree {
            ast: json!({"text": text}),
            mapping: BTreeMap::new(),
            comments: Vec::new(),
        })
    }
}

const LANGUAGE: HostLanguage = HostLanguage {
    name: "lang-lines",
    version: "1",
    id: "lines",
    title: "Lines",
    aliases: &["lines"],
    globs: &["**/*.lines"],
};

fn params(overlays: &[(&str, &str)]) -> IndexParams {
    let inputs = [("a.lines", true), ("b.lines", false)];
    IndexParams {
        project: ProjectRef {
            root: "/nonexistent-lighthouse-project".to_owned(),
        },
        language: "lines".to_owned(),
        units: inputs
            .iter()
            .map(|(path, source_ast)| UnitRef {
                id: (*path).to_owned(),
                files: vec![FileRef {
                    path: (*path).to_owned(),
                    hash: String::new(),
                    source_ast: *source_ast,
                }],
                regions: Vec::new(),
                key: String::new(),
                imports: BTreeMap::new(),
            })
            .collect(),
        files: Vec::new(),
        context: Context {
            overlays: Some(
                overlays
                    .iter()
                    .map(|(path, text)| Overlay {
                        path: (*path).to_owned(),
                        text: (*text).to_owned(),
                    })
                    .collect(),
            ),
            ..Context::default()
        },
    }
}

/// The trees of every unit of the result, and the fragments, in order.
fn flat(
    result: &IndexResult,
) -> (
    Vec<&lighthouse_protocol::Source>,
    Vec<&lighthouse_protocol::Fragment>,
) {
    let sources = result.units.iter().flat_map(|unit| &unit.sources).collect();
    let fragments = result
        .units
        .iter()
        .flat_map(|unit| &unit.fragments)
        .collect();
    (sources, fragments)
}

/// The gaps of every unit of the result.
fn gaps(result: &IndexResult) -> Vec<&lighthouse_protocol::Incomplete> {
    result
        .units
        .iter()
        .flat_map(|unit| &unit.incomplete)
        .collect()
}

#[test]
fn the_provider_announces_its_language_and_the_source_ast_capability() {
    let handler = HostProvider::new(Lines);
    assert_eq!(handler.name(), "lang-lines");
    let manifests = handler.manifest();
    let manifest = &manifests[0];
    assert_eq!(manifest.id, "lines");
    assert_eq!(manifest.globs, ["**/*.lines"]);
    assert_eq!(manifest.capabilities, ["source-ast", "overlays"]);
}

#[test]
fn a_file_asked_for_its_tree_has_one_and_a_fragment() {
    let mut handler = HostProvider::new(Lines);
    let result = handler.index(params(&[("a.lines", "x=1")])).unwrap();
    let (sources, fragments) = flat(&result);
    assert_eq!(sources.len(), 1, "a file not asked for has no tree");
    assert_eq!(sources[0].path, "a.lines");
    assert_eq!(sources[0].ast, json!({"text": "x=1"}));
    assert_eq!(fragments.len(), 2, "every host file is a file subject");
    assert_eq!(fragments[0].file.path, "a.lines");
    assert_eq!(fragments[1].file.path, "b.lines");
    assert!(gaps(&result).is_empty());
}

#[test]
fn a_file_that_cannot_be_read_or_parsed_is_a_gap() {
    let mut handler = HostProvider::new(Lines);
    let unreadable = handler.index(params(&[])).unwrap();
    assert!(flat(&unreadable).0.is_empty());
    assert!(
        gaps(&unreadable)[0]
            .reason
            .starts_with("cannot read Lines host document"),
        "{:?}",
        gaps(&unreadable)
    );
    let invalid = handler.index(params(&[("a.lines", "x!")])).unwrap();
    assert_eq!(gaps(&invalid)[0].reason, "invalid Lines: a bang");
    let empty = handler.index(params(&[("a.lines", "")])).unwrap();
    assert_eq!(
        gaps(&empty)[0].reason,
        "cannot expose Lines syntax tree: nothing to select"
    );
}

fn region(pointer: &str, text: &str) -> EmbeddedRegion {
    EmbeddedRegion {
        id: pointer.to_owned(),
        injection_id: "fence".to_owned(),
        node_pointer: pointer.to_owned(),
        source_pointer: format!("{pointer}/text"),
        provider: "lines".to_owned(),
        syntax: None,
        context: json!({}),
        text: text.to_owned(),
        source_range: [0, text.len() as u32],
        text_map: None,
    }
}

#[test]
fn regions_that_are_documents_are_read_and_the_others_are_a_notice() {
    let mut handler = HostProvider::new(Lines);
    let mut request = params(&[]);
    request.units = vec![UnitRef {
        id: "#regions".to_owned(),
        files: Vec::new(),
        regions: vec![EmbeddedFile {
            path: "doc.md".to_owned(),
            hash: String::new(),
            text: String::new(),
            test: false,
            facts: BTreeMap::new(),
            regions: vec![region("/children/0", "x=1"), region("/children/1", "x!")],
        }],
        key: String::new(),
        imports: BTreeMap::new(),
    }];
    let result = handler.index(request).unwrap();
    assert!(gaps(&result).is_empty(), "{:?}", gaps(&result));
    let (_, fragments) = flat(&result);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].file.path, "doc.md");
    assert!(fragments[0].symbols.is_empty());
    assert_eq!(
        result.units[0].notices,
        ["doc.md: the lines at /children/1 is not a valid Lines document: a bang"]
    );
}

#[test]
fn text_offsets_repeat_the_source_range_for_each_byte_of_a_character() {
    let mut offsets = TextOffsets::with_capacity(4);
    offsets.push('x', 1, 2);
    offsets.push('雪', 2, 8);
    assert_eq!(offsets.len(), 4);
    let map = offsets.text_map("x雪").unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(map[1].source_range, [2, 8]);
    offsets.pop();
    assert_eq!(offsets.len(), 3);
    assert!(offsets.text_map("x雪").is_none());
}

#[test]
fn serve_answers_the_handshake_and_an_index_request_until_exit() {
    use std::io::Cursor;

    use lighthouse_protocol::{
        EXIT, INDEX, INITIALIZE, Message, SHUTDOWN, language::serve, read_message, write_message,
    };
    use serde_json::Value;

    let messages = [
        Message::request(
            1,
            INITIALIZE,
            json!({
                "protocolVersion": "0.7",
                "clientInfo": { "name": "t", "version": "0" },
                "workspaceFolders": [{ "uri": "file:///p/", "name": "p" }],
            }),
        )
        .unwrap(),
        Message::request(
            2,
            INDEX,
            json!({
                "project": { "root": "/p" },
                "language": "lines",
                "units": [{ "id": "a.lines", "key": "", "files": [{ "path": "a.lines", "hash": "", "source_ast": true }] }],
                "context": { "overlays": [{ "path": "a.lines", "text": "x=1" }] },
            }),
        )
        .unwrap(),
        Message::request(3, SHUTDOWN, ()).unwrap(),
        Message::notification(EXIT),
    ];
    let mut input = Vec::new();
    for message in &messages {
        write_message(&mut input, message).unwrap();
    }
    let mut output = Vec::new();

    serve(
        Cursor::new(input),
        &mut output,
        &mut HostProvider::new(Lines),
    )
    .unwrap();

    let mut reader = Cursor::new(output);
    let replies: Vec<Message> = std::iter::from_fn(|| read_message(&mut reader).unwrap()).collect();
    assert_eq!(replies.len(), 3);
    let handshake = replies[0].clone().into_result::<Value>().unwrap().unwrap();
    assert_eq!(handshake["serverInfo"]["name"], "lang-lines");
    let indexed = replies[1].clone().into_result::<Value>().unwrap().unwrap();
    assert_eq!(
        indexed["units"][0]["sources"][0]["ast"],
        json!({"text": "x=1"})
    );
}
