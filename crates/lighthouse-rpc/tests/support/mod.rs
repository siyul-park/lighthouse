//! A bare protocol client: drives a plugin process without the host adapter,
//! so tests see exactly what the plugin put on the wire.

use std::{
    io::BufReader,
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use lighthouse_protocol::{self as wire, Message, read_message, write_message};
use serde::{Serialize, de::DeserializeOwned};

pub struct Plugin {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: i64,
}

impl Plugin {
    /// Starts the binary listed in the manifest of the plugin directory.
    pub fn start(dir: &Path, root: &Path) -> Self {
        let manifest = lighthouse_rpc::load(dir).unwrap().manifest;
        let mut child = Command::new(dir.join(&manifest.command))
            .args(&manifest.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        Self {
            stdin: child.stdin.take().unwrap(),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child,
            next: 1,
        }
    }

    pub fn raw(&mut self, text: &str) {
        use std::io::Write;
        self.stdin.write_all(text.as_bytes()).unwrap();
        self.stdin.flush().unwrap();
    }

    pub fn read(&mut self) -> Option<Message> {
        read_message(&mut self.stdout).unwrap()
    }

    /// Sends a request and returns the whole response.
    pub fn request(&mut self, method: &str, params: impl Serialize) -> Message {
        let id = self.next;
        self.next += 1;
        write_message(
            &mut self.stdin,
            &Message::request(id, method, params).unwrap(),
        )
        .unwrap();
        self.read().expect("a response")
    }

    pub fn call<R: DeserializeOwned>(&mut self, method: &str, params: impl Serialize) -> R {
        self.request(method, params)
            .into_result()
            .unwrap()
            .unwrap_or_else(|e| panic!("{method} failed: {}", e.message))
    }

    pub fn initialize(&mut self, root: &Path) -> wire::InitializeResult {
        self.call(
            wire::INITIALIZE,
            wire::InitializeParams {
                root: root.to_string_lossy().into_owned(),
                protocol_version: wire::VERSION.to_owned(),
                client_info: wire::ClientInfo {
                    name: "conformance".to_owned(),
                    version: "0".to_owned(),
                },
            },
        )
    }

    /// Shuts the plugin down the way hosts do and returns its exit code.
    pub fn finish(mut self) -> Option<i32> {
        let shutdown = self.request(wire::SHUTDOWN, ());
        assert!(shutdown.error.is_none());
        write_message(&mut self.stdin, &Message::notification(wire::EXIT)).unwrap();
        self.child.wait().unwrap().code()
    }
}
