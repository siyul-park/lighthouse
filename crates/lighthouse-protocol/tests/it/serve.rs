use std::io::Cursor;

use lighthouse_protocol::{
    EXIT, ErrorObject, Handler, INDEX, INITIALIZE, IndexParams, IndexResult, InitializeParams,
    InitializeResult, Message, SHUTDOWN, ServeError, read_message, serve, write_message,
};
use serde_json::{Value, json};

struct Stub {
    panic: bool,
}

impl Handler for Stub {
    fn initialize(&mut self, params: InitializeParams) -> Result<InitializeResult, ErrorObject> {
        Ok(InitializeResult::new(
            params.workspace_folders[0].name.clone(),
            "1",
            Vec::new(),
            Vec::new(),
        ))
    }

    fn index(&mut self, _: IndexParams) -> Result<IndexResult, ErrorObject> {
        if self.panic {
            panic!("boom");
        }
        Err("no index".into())
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
        "protocolVersion": "0.3",
        "clientInfo": { "name": "t", "version": "0" },
        "workspaceFolders": [{ "uri": "file:///p/", "name": "p" }],
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
    assert_eq!(first["serverInfo"]["name"], "p");
    let codes: Vec<i64> = replies[1..3]
        .iter()
        .map(|r| r.error.as_ref().unwrap().code)
        .collect();
    assert_eq!(codes, [-32803, -32601]);
    assert!(replies[3].error.is_none());
}

#[test]
fn serve_survives_a_panicking_handler_and_reports_bad_input() {
    let mut output = Vec::new();
    let messages = [
        Message::request(1, INITIALIZE, initialize_params()).unwrap(),
        Message::request(2, INDEX, index_params()).unwrap(),
        Message::request(3, INITIALIZE, json!("bad")).unwrap(),
        Message::notification(EXIT),
    ];

    let ended = serve(input(&messages), &mut output, &mut Stub { panic: true });

    assert!(matches!(ended, Err(ServeError::ExitWithoutShutdown)));
    let replies = replies(output);
    let panicked = replies[1].error.as_ref().unwrap();
    assert!(panicked.message.contains("boom"), "{panicked:?}");
    assert_eq!(panicked.code, -32603);
    assert_eq!(replies[2].error.as_ref().unwrap().code, -32602);

    let closed = serve(
        Cursor::new(b"Content-Length: x\r\n\r\n".to_vec()),
        Vec::new(),
        &mut Stub { panic: false },
    );
    assert!(matches!(closed, Err(ServeError::Frame(_))));
    let closed = serve(input(&[]), Vec::new(), &mut Stub { panic: false });
    assert!(matches!(closed, Err(ServeError::Closed)));
}

#[test]
fn an_error_keeps_the_body_beside_the_message() {
    let body = json!({ "error": { "message": "no" } });
    let error = ErrorObject::with_data("the service answered 400", body.clone());
    assert_eq!(error.message, "the service answered 400");
    assert_eq!(error.code, -32803);
    assert_eq!(error.data, Some(body));
    assert_eq!(ErrorObject::from("plain".to_owned()).data, None);
}

#[test]
fn a_host_of_another_protocol_version_is_refused_before_the_handler_sees_it() {
    let mut output = Vec::new();
    let mut params = initialize_params();
    params["protocolVersion"] = json!("0.1");
    let messages = [
        Message::request(1, INITIALIZE, params).unwrap(),
        Message::request(2, INDEX, index_params()).unwrap(),
        Message::notification(EXIT),
    ];

    serve(input(&messages), &mut output, &mut Stub { panic: false }).unwrap_err();

    let replies = replies(output);
    let refused = replies[0].error.clone().unwrap();
    assert_eq!(refused.code, -32803);
    assert!(
        refused.message.contains("speaks protocol 0.1"),
        "{refused:?}"
    );
    // The refused handshake leaves the plugin uninitialized.
    assert_eq!(replies[1].error.as_ref().unwrap().code, -32002);
}

#[test]
fn a_request_before_the_handshake_is_refused() {
    let mut output = Vec::new();
    let messages = [
        Message::request(1, INDEX, index_params()).unwrap(),
        Message::request(2, "lighthouse/nope", ()).unwrap(),
        Message::request(3, INITIALIZE, initialize_params()).unwrap(),
        Message::request(4, INDEX, index_params()).unwrap(),
        Message::request(5, SHUTDOWN, ()).unwrap(),
        Message::notification(EXIT),
    ];

    serve(input(&messages), &mut output, &mut Stub { panic: false }).unwrap();

    let codes: Vec<Option<i64>> = replies(output)
        .iter()
        .map(|reply| reply.error.as_ref().map(|e| e.code))
        .collect();
    assert_eq!(
        codes,
        [Some(-32002), Some(-32002), None, Some(-32803), None]
    );
}
