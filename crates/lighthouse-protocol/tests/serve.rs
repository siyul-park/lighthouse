use std::io::Cursor;

use lighthouse_protocol::{
    EXIT, Handler, INDEX, INITIALIZE, IndexParams, IndexResult, InitializeParams, InitializeResult,
    Message, SHUTDOWN, ServeError, read_message, serve, write_message,
};
use serde_json::{Value, json};

struct Stub {
    panic: bool,
}

impl Handler for Stub {
    fn initialize(&mut self, params: InitializeParams) -> Result<InitializeResult, String> {
        Ok(InitializeResult {
            id: params.root,
            version: "1".to_owned(),
            protocol_version: params.protocol_version,
            languages: Vec::new(),
        })
    }

    fn index(&mut self, _: IndexParams) -> Result<IndexResult, String> {
        if self.panic {
            panic!("boom");
        }
        Err("no index".to_owned())
    }
}

fn input(messages: &[Message]) -> Cursor<Vec<u8>> {
    let mut bytes = Vec::new();
    for message in messages {
        write_message(&mut bytes, message).unwrap();
    }
    Cursor::new(bytes)
}

fn replies(output: Vec<u8>) -> Vec<Message> {
    let mut reader = Cursor::new(output);
    std::iter::from_fn(|| read_message(&mut reader).unwrap()).collect()
}

fn initialize_params() -> Value {
    json!({
        "root": "/p",
        "protocolVersion": "0.1",
        "clientInfo": { "name": "t", "version": "0" },
    })
}

fn index_params() -> Value {
    json!({ "project": { "root": "/p" }, "language": "x", "files": [] })
}

#[test]
fn serve_answers_each_request_until_shutdown_then_exit() {
    let mut output = Vec::new();
    let messages = [
        Message::request(1, INITIALIZE, initialize_params()).unwrap(),
        Message::request(2, INDEX, index_params()).unwrap(),
        Message::request(3, "nope", ()).unwrap(),
        Message::request(4, SHUTDOWN, ()).unwrap(),
        Message::notification(EXIT),
    ];

    serve(input(&messages), &mut output, &mut Stub { panic: false }).unwrap();

    let replies = replies(output);
    assert_eq!(replies.len(), 4);
    let first = replies[0].clone().into_result::<Value>().unwrap().unwrap();
    assert_eq!(first["id"], "/p");
    let codes: Vec<i64> = replies[1..3]
        .iter()
        .map(|r| r.error.as_ref().unwrap().code)
        .collect();
    assert_eq!(codes, [-32603, -32601]);
    assert!(replies[3].error.is_none());
}

#[test]
fn serve_survives_a_panicking_handler_and_reports_bad_input() {
    let mut output = Vec::new();
    let messages = [
        Message::request(1, INDEX, index_params()).unwrap(),
        Message::request(2, INITIALIZE, json!("bad")).unwrap(),
        Message::notification(EXIT),
    ];

    let ended = serve(input(&messages), &mut output, &mut Stub { panic: true });

    assert!(matches!(ended, Err(ServeError::ExitWithoutShutdown)));
    let replies = replies(output);
    assert!(replies[0].error.as_ref().unwrap().message.contains("boom"));
    assert_eq!(replies[1].error.as_ref().unwrap().code, -32602);

    let closed = serve(
        Cursor::new(b"Content-Length: x\r\n\r\n".to_vec()),
        Vec::new(),
        &mut Stub { panic: false },
    );
    assert!(matches!(closed, Err(ServeError::Frame(_))));
    let closed = serve(input(&[]), Vec::new(), &mut Stub { panic: false });
    assert!(matches!(closed, Err(ServeError::Closed)));
}
