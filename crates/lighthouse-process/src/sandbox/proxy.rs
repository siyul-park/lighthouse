//! The HTTP proxy that is a sandboxed program's only way to the network. It
//! tunnels a `CONNECT` and forwards a plain `GET http://host/` to the hosts on
//! its list and refuses everything else. TLS is never opened: the list is of
//! the host a request names.

use std::{
    collections::BTreeSet,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    time::Duration,
};

use super::relay::{Server, relay};

/// The most a request head may take.
const MAX_HEAD: usize = 16 * 1024;
/// How long a client may take to send its request head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

/// A proxy on a loopback port, serving until it is dropped.
pub struct Proxy {
    port: u16,
    blocked: Arc<Mutex<BTreeSet<String>>>,
    _server: Server,
}

impl Proxy {
    /// Starts a proxy that lets through the hosts of `allow`, as [`allows`]
    /// reads them.
    pub fn start(allow: &[String]) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let blocked = Arc::new(Mutex::new(BTreeSet::new()));
        let (list, refused) = (allow.to_vec(), Arc::clone(&blocked));
        let server = Server::spawn(
            move || {
                let (client, _) = listener.accept()?;
                client.set_nonblocking(false)?;
                Ok(client)
            },
            // A client that fails sees its connection close; nothing else can
            // be said to it.
            move |client: TcpStream| drop(serve(client, &list, &refused)),
        );
        Ok(Self {
            port,
            blocked,
            _server: server,
        })
    }

    /// The loopback port it listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The hosts it refused so far, sorted.
    pub fn blocked(&self) -> Vec<String> {
        self.blocked
            .lock()
            .map(|hosts| hosts.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// Where a request goes, and what is sent there first.
struct Route {
    host: String,
    port: u16,
    /// For a `CONNECT` nothing: the tunnel starts with the client's bytes. For
    /// a plain request, the request in the form a server reads.
    head: Option<String>,
}

/// Whether `list` lets `host` on `port` through. An entry is a name, or
/// `*.suffix` for every host below it; either may end in `:port`, and without
/// one it covers every port. Names compare without regard to case.
pub fn allows(list: &[String], host: &str, port: u16) -> bool {
    list.iter().any(|entry| {
        let (name, only) = match entry.rsplit_once(':') {
            Some((name, digits)) => match digits.parse::<u16>() {
                Ok(only) => (name, Some(only)),
                Err(_) => (entry.as_str(), None),
            },
            None => (entry.as_str(), None),
        };
        only.is_none_or(|only| only == port) && names(name, host)
    })
}

fn names(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(suffix) => {
            let (host, suffix) = (host.to_ascii_lowercase(), suffix.to_ascii_lowercase());
            host.strip_suffix(&suffix)
                .is_some_and(|below| below.len() > 1 && below.ends_with('.'))
        }
        None => pattern.eq_ignore_ascii_case(host),
    }
}

/// Serves one client: reads its request, and tunnels or forwards it when the
/// host is on the list.
fn serve(
    mut client: TcpStream,
    allow: &[String],
    blocked: &Mutex<BTreeSet<String>>,
) -> io::Result<()> {
    client.set_read_timeout(Some(HEAD_TIMEOUT))?;
    let (head, rest) = read_head(&mut client)?;
    client.set_read_timeout(None)?;
    let Some(route) = route(&head) else {
        return reply(&mut client, "400 Bad Request", "not a proxy request");
    };
    if !allows(allow, &route.host, route.port) {
        if let Ok(mut hosts) = blocked.lock() {
            hosts.insert(route.host.clone());
        }
        let why = format!(
            "lighthouse sandbox: {} is not in the network list",
            route.host
        );
        return reply(&mut client, "403 Forbidden", &why);
    }
    let Ok(mut upstream) = TcpStream::connect((route.host.as_str(), route.port)) else {
        return reply(&mut client, "502 Bad Gateway", "the host cannot be reached");
    };
    match &route.head {
        None => client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")?,
        Some(head) => upstream.write_all(head.as_bytes())?,
    }
    upstream.write_all(&rest)?;
    relay(client, upstream);
    Ok(())
}

/// The request head, up to and including the blank line, and the bytes that
/// followed it.
fn read_head(client: &mut TcpStream) -> io::Result<(String, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = bytes.split_off(end + 4);
            return Ok((String::from_utf8_lossy(&bytes).into_owned(), rest));
        }
        if bytes.len() > MAX_HEAD {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let n = client.read(&mut chunk)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
}

/// Where the request `head` is for; none when it is not for a proxy.
fn route(head: &str) -> Option<Route> {
    let mut lines = head.split("\r\n");
    let mut line = lines.next()?.split(' ');
    let (method, target, version) = (line.next()?, line.next()?, line.next()?);
    if method == "CONNECT" {
        let (host, port) = authority(target, 443)?;
        return Some(Route {
            host,
            port,
            head: None,
        });
    }
    let rest = target.strip_prefix("http://")?;
    let end = rest.find('/').unwrap_or(rest.len());
    let (host, port) = authority(&rest[..end], 80)?;
    let path = if end == rest.len() { "/" } else { &rest[end..] };
    let mut forwarded = format!("{method} {path} {version}\r\n");
    for header in lines.filter(|h| !h.is_empty()) {
        let name = header
            .split(':')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if name != "connection" && name != "proxy-connection" {
            forwarded.push_str(header);
            forwarded.push_str("\r\n");
        }
    }
    forwarded.push_str("Connection: close\r\n\r\n");
    Some(Route {
        host,
        port,
        head: Some(forwarded),
    })
}

/// `host[:port]`, the port `default` when there is none.
fn authority(text: &str, default: u16) -> Option<(String, u16)> {
    match text.rsplit_once(':') {
        Some((host, port)) if !text.ends_with(']') => Some((host.to_owned(), port.parse().ok()?)),
        _ => Some((text.to_owned(), default)),
    }
    .filter(|(host, _)| !host.is_empty())
}

fn reply(client: &mut TcpStream, status: &str, why: &str) -> io::Result<()> {
    let body = format!("{why}\n");
    write!(
        client,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
