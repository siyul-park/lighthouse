use std::{
    io::{Read, Write},
    net::TcpStream,
};

use lighthouse_process::{Proxy, allows};

use crate::support::server;

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|entry| (*entry).to_owned()).collect()
}

/// What the proxy answers to `request`, read to the end.
fn ask(proxy: &Proxy, request: &str) -> String {
    let mut connection = TcpStream::connect(("127.0.0.1", proxy.port())).unwrap();
    connection.write_all(request.as_bytes()).unwrap();
    let mut answer = String::new();
    let _ = connection.read_to_string(&mut answer);
    answer
}

#[test]
fn a_name_matches_itself_in_any_case_and_a_suffix_matches_what_is_below_it() {
    let list = names(&["api.example.com", "*.cdn.test", "127.0.0.1:8080"]);

    assert!(allows(&list, "API.example.com", 443));
    assert!(allows(&list, "a.cdn.test", 443));
    assert!(allows(&list, "a.b.cdn.test", 443));
    assert!(!allows(&list, "cdn.test", 443), "a suffix is what is below");
    assert!(!allows(&list, "evilcdn.test", 443));
    assert!(!allows(&list, "example.com", 443));
    assert!(allows(&list, "127.0.0.1", 8080));
    assert!(
        !allows(&list, "127.0.0.1", 8081),
        "a listed port is the only one"
    );
}

#[test]
fn a_connect_to_a_listed_host_is_tunnelled_and_an_unlisted_one_is_refused() {
    let port = server();
    let proxy = Proxy::start(&names(&[&format!("127.0.0.1:{port}")])).unwrap();

    let through = ask(
        &proxy,
        &format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n\r\nGET / HTTP/1.1\r\n\r\n"),
    );
    let refused = ask(
        &proxy,
        &format!("CONNECT localhost:{port} HTTP/1.1\r\n\r\n"),
    );

    assert!(
        through.starts_with("HTTP/1.1 200 Connection Established"),
        "{through}"
    );
    assert!(
        through.ends_with("ok"),
        "the tunnel carries the answer: {through}"
    );
    assert!(refused.starts_with("HTTP/1.1 403 Forbidden"), "{refused}");
    assert!(
        refused.contains("localhost"),
        "the host is named: {refused}"
    );
    assert_eq!(proxy.blocked(), ["localhost"]);
}

#[test]
fn a_connect_to_a_host_below_a_listed_suffix_passes_the_list() {
    let proxy = Proxy::start(&names(&["*.example.test"])).unwrap();

    let refused = ask(&proxy, "CONNECT other.test:443 HTTP/1.1\r\n\r\n");
    let listed = ask(&proxy, "CONNECT api.example.test:1 HTTP/1.1\r\n\r\n");

    assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");
    assert!(
        listed.starts_with("HTTP/1.1 502"),
        "a listed host is let through to a connection that fails: {listed}"
    );
}

#[test]
fn a_plain_request_in_absolute_form_is_forwarded_by_host() {
    let port = server();
    let proxy = Proxy::start(&names(&[&format!("127.0.0.1:{port}")])).unwrap();

    let forwarded = ask(
        &proxy,
        &format!(
            "GET http://127.0.0.1:{port}/x HTTP/1.1\r\nHost: a\r\nProxy-Connection: keep-alive\r\n\r\n"
        ),
    );
    let refused = ask(
        &proxy,
        &format!("GET http://localhost:{port}/ HTTP/1.1\r\n\r\n"),
    );

    assert!(forwarded.starts_with("HTTP/1.1 200 OK"), "{forwarded}");
    assert!(forwarded.ends_with("ok"), "{forwarded}");
    assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");
}

#[test]
fn a_request_that_is_not_for_a_proxy_is_a_bad_request() {
    let proxy = Proxy::start(&names(&["127.0.0.1"])).unwrap();

    let answer = ask(&proxy, "GET /path HTTP/1.1\r\n\r\n");

    assert!(answer.starts_with("HTTP/1.1 400"), "{answer}");
}
