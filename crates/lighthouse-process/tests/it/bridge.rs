//! The way into a namespace without a network: what `lighthouse internal
//! bridge` does, on a host with Unix sockets.

use std::{io::Write, net::TcpListener, os::unix::net::UnixListener, thread};

use lighthouse_process::bridge;

#[test]
fn bridge_forwards_a_local_port_to_the_socket_and_returns_the_exit_code_of_the_program() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("proxy.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    thread::spawn(move || {
        let (mut connection, _) = listener.accept().unwrap();
        connection.write_all(b"from-the-socket\n").unwrap();
    });
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let program = [
        "bash".to_owned(),
        "-c".to_owned(),
        r#"exec 3<>/dev/tcp/127.0.0.1/$0; read -r line <&3; [ "$line" = from-the-socket ] && exit 7; exit 1"#.to_owned(),
        port.to_string(),
    ];

    let code = bridge(&socket, port, &program).unwrap();

    assert_eq!(code, 7);
}

#[test]
fn bridge_without_a_program_is_an_error() {
    let dir = tempfile::tempdir().unwrap();

    let error = bridge(&dir.path().join("none.sock"), 1, &[]).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}
