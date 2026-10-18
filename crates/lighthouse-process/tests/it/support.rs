//! A local server standing for the allowed host: tests need no internet.

use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

/// Answers every connection with the HTTP response `ok`, after reading its
/// request head, and returns the port it listens on.
pub fn server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for connection in listener.incoming().flatten() {
            thread::spawn(move || reply(connection));
        }
    });
    port
}

fn reply(mut connection: std::net::TcpStream) {
    let mut seen = Vec::new();
    let mut chunk = [0u8; 1024];
    while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
        match connection.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => seen.extend_from_slice(&chunk[..n]),
        }
    }
    let _ = connection
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
}
