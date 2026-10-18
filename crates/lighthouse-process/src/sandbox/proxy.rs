//! The HTTP proxy that is a sandboxed program's only way to the network. It
//! tunnels a `CONNECT` and forwards a plain `GET http://host/` to the hosts on
//! its list and refuses everything else. TLS is never opened: the list is of
//! the host a request names. It is bounded: a few clients at once, a head of
//! limited size and time, a host of plain characters, a connection that is
//! given time to be made.

use std::{
    collections::BTreeSet,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream, ToSocketAddrs},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use super::relay::{Server, relay};

/// The most a request head may take.
const MAX_HEAD: usize = 16 * 1024;
/// How long a client may take to send its request head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

/// How many clients are served at once; the others are turned away.
const MAX_CLIENTS: usize = 64;
/// How many refused hosts are named; the rest are only counted.
const MAX_BLOCKED: usize = 100;
/// How long a client turned away is waited for to finish its request.
const BUSY_TIMEOUT: Duration = Duration::from_secs(1);
/// How long a connection to a host may take to be made.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The ports a host named without one is let through on.
const WEB_PORTS: [u16; 2] = [80, 443];

/// A proxy on a loopback port, serving until it is dropped.
pub struct Proxy {
    port: u16,
    blocked: Arc<Mutex<Blocked>>,
    _server: Server,
}

/// What the proxy refused.
#[derive(Default)]
struct Blocked {
    hosts: BTreeSet<String>,
    /// Refusals past [`MAX_BLOCKED`] hosts.
    more: usize,
}

/// One of the places for clients of the proxy; given back when dropped.
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(active: &Arc<AtomicUsize>) -> Option<Self> {
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CLIENTS {
            active.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Self(Arc::clone(active)))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Proxy {
    /// Starts a proxy that lets through the hosts of `allow`, as [`allows`]
    /// reads them.
    pub fn start(allow: &[String]) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let blocked = Arc::new(Mutex::new(Blocked::default()));
        let (list, refused) = (allow.to_vec(), Arc::clone(&blocked));
        let active = Arc::new(AtomicUsize::new(0));
        let server = Server::spawn(
            move || {
                let (client, _) = listener.accept()?;
                client.set_nonblocking(false)?;
                Ok(client)
            },
            // A client that fails sees its connection close; nothing else can
            // be said to it.
            move |mut client: TcpStream| {
                let Some(_slot) = Slot::take(&active) else {
                    // Read the request before answering it: a connection
                    // closed with unread bytes is reset and the answer lost.
                    let _ = client.set_read_timeout(Some(BUSY_TIMEOUT));
                    let _ = read_head(&mut client);
                    return drop(reply(&mut client, "503 Service Unavailable", "busy"));
                };
                drop(serve(client, &list, &refused));
            },
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

    /// The hosts it refused so far, sorted, and how many refusals more than it
    /// names.
    pub fn blocked(&self) -> Vec<String> {
        let Ok(blocked) = self.blocked.lock() else {
            return Vec::new();
        };
        let more = (blocked.more > 0).then(|| format!("and {} more", blocked.more));
        blocked.hosts.iter().cloned().chain(more).collect()
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
/// one it covers the web ports, 80 and 443. Names compare without regard to
/// case.
pub fn allows(list: &[String], host: &str, port: u16) -> bool {
    list.iter().any(|entry| {
        let (name, only) = match entry.rsplit_once(':') {
            Some((name, digits)) => match digits.parse::<u16>() {
                Ok(only) => (name, Some(only)),
                Err(_) => (entry.as_str(), None),
            },
            None => (entry.as_str(), None),
        };
        only.map_or(WEB_PORTS.contains(&port), |only| only == port) && names(name, host)
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
fn serve(mut client: TcpStream, allow: &[String], blocked: &Mutex<Blocked>) -> io::Result<()> {
    client.set_read_timeout(Some(HEAD_TIMEOUT))?;
    let (head, rest) = read_head(&mut client)?;
    client.set_read_timeout(None)?;
    let Some(route) = route(&head) else {
        return reply(&mut client, "400 Bad Request", "not a proxy request");
    };
    if !allows(allow, &route.host, route.port) {
        if let Ok(mut blocked) = blocked.lock() {
            if blocked.hosts.len() < MAX_BLOCKED || blocked.hosts.contains(&route.host) {
                blocked.hosts.insert(route.host.clone());
            } else {
                blocked.more += 1;
            }
        }
        let why = format!(
            "lighthouse sandbox: {} is not in the network list",
            route.host
        );
        return reply(&mut client, "403 Forbidden", &why);
    }
    let Some(mut upstream) = connect(&route.host, route.port) else {
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

/// A connection to `host`, given [`CONNECT_TIMEOUT`] to be made to the first
/// address that answers.
fn connect(host: &str, port: u16) -> Option<TcpStream> {
    (host, port)
        .to_socket_addrs()
        .ok()?
        .find_map(|address| TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).ok())
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
    let mut named = false;
    for header in lines.filter(|h| !h.is_empty()) {
        let (name, value) = header.split_once(':')?;
        match name.to_ascii_lowercase().as_str() {
            "connection" | "proxy-connection" => continue,
            // The host a request is for is the one it names to the server.
            "host" => {
                let (said, at) = authority(value.trim(), 80)?;
                if !said.eq_ignore_ascii_case(&host) || at != port {
                    return None;
                }
                named = true;
            }
            _ => {}
        }
        forwarded.push_str(header);
        forwarded.push_str("\r\n");
    }
    if !named {
        forwarded.push_str(&format!("Host: {}\r\n", &rest[..end]));
    }
    forwarded.push_str("Connection: close\r\n\r\n");
    Some(Route {
        host,
        port,
        head: Some(forwarded),
    })
}

/// `host[:port]`, the port `default` when there is none. A host is letters,
/// digits, dots and hyphens, or an IPv6 address in brackets: nothing a log or
/// a message could be bent by.
fn authority(text: &str, default: u16) -> Option<(String, u16)> {
    let (host, port) = match text.strip_prefix('[') {
        Some(rest) => {
            let (address, after) = rest.split_once(']')?;
            let plain = |b: u8| b.is_ascii_hexdigit() || b == b':' || b == b'.';
            if address.is_empty() || !address.bytes().all(plain) {
                return None;
            }
            let port = match after {
                "" => None,
                _ => Some(after.strip_prefix(':')?),
            };
            (address, port)
        }
        None => {
            let (host, port) = match text.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (text, None),
            };
            let plain = |b: u8| b.is_ascii_alphanumeric() || b == b'.' || b == b'-';
            if host.is_empty() || !host.bytes().all(plain) {
                return None;
            }
            (host, port)
        }
    };
    let port = match port {
        Some(digits) => digits.parse().ok().filter(|port| *port != 0)?,
        None => default,
    };
    Some((host.to_owned(), port))
}

fn reply(client: &mut TcpStream, status: &str, why: &str) -> io::Result<()> {
    let body = format!("{why}\n");
    write!(
        client,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
