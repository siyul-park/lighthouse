use std::io::Cursor;

use lighthouse_protocol::{FrameError, Id, Message, read_message, write_message};
use serde_json::json;

fn roundtrip(message: &Message) -> Message {
    let mut bytes = Vec::new();
    write_message(&mut bytes, message).unwrap();
    read_message(&mut Cursor::new(bytes)).unwrap().unwrap()
}

#[test]
fn messages_roundtrip_through_content_length_framing() {
    let request = Message::request(7, "index", json!({ "files": [] })).unwrap();
    assert_eq!(roundtrip(&request), request);
    let response = Message::response(Id::Number(7), json!({ "ok": true })).unwrap();
    assert_eq!(roundtrip(&response), response);
    let note = Message::notification("exit");
    assert_eq!(roundtrip(&note), note);
}

#[test]
fn frames_carry_a_byte_length_and_survive_multibyte_text() {
    let message = Message::response(Id::Text("é".to_owned()), json!("日本語")).unwrap();
    let mut bytes = Vec::new();
    write_message(&mut bytes, &message).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    let (header, body) = text.split_once("\r\n\r\n").unwrap();
    assert_eq!(header, format!("Content-Length: {}", body.len()));
    assert_eq!(roundtrip(&message), message);
}

#[test]
fn consecutive_messages_are_read_in_order_then_none() {
    let mut bytes = Vec::new();
    for n in 1..=2 {
        write_message(&mut bytes, &Message::request(n, "a", ()).unwrap()).unwrap();
    }
    let mut reader = Cursor::new(bytes);
    assert_eq!(
        read_message(&mut reader).unwrap().unwrap().id,
        Some(Id::Number(1))
    );
    assert_eq!(
        read_message(&mut reader).unwrap().unwrap().id,
        Some(Id::Number(2))
    );
    assert!(read_message(&mut reader).unwrap().is_none());
}

#[test]
fn malformed_input_is_an_error_not_a_panic() {
    let read = |text: &str| read_message(&mut Cursor::new(text.as_bytes().to_vec()));
    assert!(matches!(
        read("Content-Type: x\r\n\r\n{}"),
        Err(FrameError::Header(_))
    ));
    assert!(matches!(
        read("Content-Length: nope\r\n\r\n"),
        Err(FrameError::Header(_))
    ));
    assert!(matches!(
        read("Content-Length: 3\r\n\r\n{]"),
        Err(FrameError::Io(_))
    ));
    assert!(matches!(
        read("Content-Length: 2\r\n\r\n{]"),
        Err(FrameError::Json(_))
    ));
    assert!(matches!(
        read("Content-Length: 999999999999\r\n\r\n"),
        Err(FrameError::TooLarge(_))
    ));
    assert!(matches!(
        read("Content-Length: 5\r\n"),
        Err(FrameError::Header(_))
    ));
}

#[test]
fn error_responses_come_back_on_the_err_side() {
    let failure = Message::failure(Some(Id::Number(1)), -32601, "nope");
    let result = roundtrip(&failure)
        .into_result::<serde_json::Value>()
        .unwrap();
    assert_eq!(result.unwrap_err().code, -32601);
}

#[test]
fn message_into_params_reads_absent_params_as_null() {
    let request = Message::request(1, "m", json!({ "n": 2 })).unwrap();
    let params: serde_json::Value = request.into_params().unwrap();
    assert_eq!(params, json!({ "n": 2 }));

    let absent: Option<u8> = Message::notification("exit").into_params().unwrap();
    assert_eq!(absent, None);
    assert!(Message::notification("exit").into_params::<u8>().is_err());
}
