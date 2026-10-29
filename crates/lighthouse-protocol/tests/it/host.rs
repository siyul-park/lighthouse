//! The provider of files that expose a syntax tree, with a parser that reads
//! `key=value` lines.

use std::collections::BTreeMap;

use lighthouse_protocol::{
    Context, FileRef, Handler, HostError, HostLanguage, HostParser, HostTree, IndexInput,
    IndexParams, InitializeParams, Overlay, ProjectRef, TextOffsets, host_handler,
};
use serde_json::json;

struct Lines;

impl HostParser for Lines {
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
    plugin: "lang-lines",
    version: "1",
    id: "lines",
    name: "Lines",
    globs: &["**/*.lines"],
};

fn params(overlays: &[(&str, &str)]) -> IndexParams {
    let inputs = [("a.lines", true), ("b.lines", false)];
    IndexParams {
        project: ProjectRef {
            root: "/nonexistent-lighthouse-project".to_owned(),
        },
        language: "lines".to_owned(),
        inputs: inputs
            .iter()
            .map(|(path, source_ast)| IndexInput::File {
                file: FileRef {
                    path: (*path).to_owned(),
                    hash: String::new(),
                },
                source_ast: *source_ast,
            })
            .collect(),
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

#[test]
fn the_provider_announces_its_language_and_the_source_ast_capability() {
    let mut handler = host_handler(LANGUAGE, Lines);
    let answer = handler
        .initialize(InitializeParams {
            protocol_version: String::new(),
            client_info: lighthouse_protocol::Implementation {
                name: "host".to_owned(),
                version: "1".to_owned(),
            },
            workspace_folders: Vec::new(),
        })
        .unwrap();
    assert_eq!(answer.server_info.name, "lang-lines");
    let manifest = &answer.capabilities.languages[0];
    assert_eq!(manifest.id, "lines");
    assert_eq!(manifest.globs, ["**/*.lines"]);
    assert_eq!(manifest.capabilities, ["source-ast", "overlays"]);
}

#[test]
fn a_file_asked_for_its_tree_has_one_and_a_fragment() {
    let mut handler = host_handler(LANGUAGE, Lines);
    let result = handler.index(params(&[("a.lines", "x=1")])).unwrap();
    assert_eq!(result.sources.len(), 1, "a file not asked for has no tree");
    assert_eq!(result.sources[0].path, "a.lines");
    assert_eq!(result.sources[0].ast, json!({"text": "x=1"}));
    assert_eq!(
        result.fragments.len(),
        2,
        "every host file is a file subject"
    );
    assert_eq!(result.fragments[0].file.path, "a.lines");
    assert_eq!(result.fragments[1].file.path, "b.lines");
    assert!(result.incomplete.is_empty());
}

#[test]
fn a_file_that_cannot_be_read_or_parsed_is_a_gap() {
    let mut handler = host_handler(LANGUAGE, Lines);
    let unreadable = handler.index(params(&[])).unwrap();
    assert!(unreadable.sources.is_empty());
    assert!(
        unreadable.incomplete[0]
            .reason
            .starts_with("cannot read Lines host document"),
        "{:?}",
        unreadable.incomplete
    );
    let invalid = handler.index(params(&[("a.lines", "x!")])).unwrap();
    assert_eq!(invalid.incomplete[0].reason, "invalid Lines: a bang");
    let empty = handler.index(params(&[("a.lines", "")])).unwrap();
    assert_eq!(
        empty.incomplete[0].reason,
        "cannot expose Lines syntax tree: nothing to select"
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
fn serve_host_answers_the_handshake_and_an_index_request_until_exit() {
    use std::io::Cursor;

    use lighthouse_protocol::{
        EXIT, INDEX, INITIALIZE, Message, SHUTDOWN, read_message, serve_host, write_message,
    };
    use serde_json::Value;

    let messages = [
        Message::request(
            1,
            INITIALIZE,
            json!({
                "protocolVersion": "0.4",
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
                "inputs": [{ "kind": "file", "file": { "path": "a.lines", "hash": "" }, "source_ast": true }],
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

    serve_host(LANGUAGE, Lines, Cursor::new(input), &mut output);

    let mut reader = Cursor::new(output);
    let replies: Vec<Message> = std::iter::from_fn(|| read_message(&mut reader).unwrap()).collect();
    assert_eq!(replies.len(), 3);
    let handshake = replies[0].clone().into_result::<Value>().unwrap().unwrap();
    assert_eq!(handshake["serverInfo"]["name"], "lang-lines");
    let indexed = replies[1].clone().into_result::<Value>().unwrap().unwrap();
    assert_eq!(indexed["sources"][0]["ast"], json!({"text": "x=1"}));
}
