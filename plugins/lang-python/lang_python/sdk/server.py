"""The JSON-RPC loop of the plugin protocol: LSP framing on a byte stream, requests answered in order."""

import json
import sys
from typing import Any, BinaryIO, Protocol

PROTOCOL_VERSION = "0.4"

PARSE_ERROR = -32700
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
INTERNAL_ERROR = -32603
REQUEST_FAILED = -32803
NOT_INITIALIZED = -32002


class Provider(Protocol):
    """What a language provider is: its identity, the languages it declares and how it indexes."""

    name: str
    version: str

    def manifest(self) -> list[dict[str, Any]]: ...

    def index(self, params: dict[str, Any]) -> dict[str, Any]: ...


class _Failure(Exception):
    """A JSON-RPC error answer."""

    def __init__(self, code: int, message: str):
        super().__init__(message)
        self.code = code
        self.message = message


class _FramingError(Exception):
    """Input that is not a framed JSON message."""


def _read_headers(stream: BinaryIO) -> dict[str, str] | None:
    """The header lines of the next message by lower-cased name, or None at the end of the input."""
    headers: dict[str, str] = {}
    seen = False
    while True:
        line = stream.readline()
        if line == b"":
            if seen:
                raise _FramingError("input ended inside a header")
            return None
        seen = True
        text = line.decode("utf-8", errors="replace").strip()
        if text == "":
            return headers
        name, _, value = text.partition(":")
        headers[name.strip().lower()] = value.strip()


def _content_length(headers: dict[str, str]) -> int:
    """The body length the headers state."""
    raw = headers.get("content-length")
    if raw is None:
        raise _FramingError("missing Content-Length")
    try:
        length = int(raw)
    except ValueError as error:
        raise _FramingError("bad Content-Length") from error
    if length < 0:
        raise _FramingError("missing Content-Length")
    return length


def read_message(stream: BinaryIO) -> dict[str, Any] | None:
    """The next message, or None at the end of the input."""
    headers = _read_headers(stream)
    if headers is None:
        return None
    length = _content_length(headers)
    body = stream.read(length)
    if len(body) != length:
        raise _FramingError("input ended inside a body")
    try:
        message = json.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as error:
        raise _FramingError("invalid JSON: %s" % error) from error
    if not isinstance(message, dict):
        raise _FramingError("a message must be an object")
    return message


def write_message(stream: BinaryIO, message: dict[str, Any]) -> None:
    """Writes a message after its `Content-Length` header, counted in UTF-8 bytes."""
    body = json.dumps(message, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    stream.write(b"Content-Length: %d\r\n\r\n" % len(body))
    stream.write(body)
    stream.flush()


def _require(params: Any, names: tuple[str, ...]) -> dict[str, Any]:
    """Refuses params that lack a required member, as the Rust server does."""
    if not isinstance(params, dict):
        raise _Failure(INVALID_PARAMS, "params must be an object")
    for name in names:
        if name not in params:
            raise _Failure(INVALID_PARAMS, "missing field `%s`" % name)
    return params


class _Session:
    """What the loop knows of the conversation."""

    def __init__(self, provider: Provider):
        self.provider = provider
        self.initialized = False
        self.shutdown = False

    def _route(self, method: str, params: Any) -> Any:
        if method == "initialize":
            body = _require(params, ("protocolVersion", "clientInfo", "workspaceFolders"))
            if body["protocolVersion"] != PROTOCOL_VERSION:
                raise _Failure(
                    REQUEST_FAILED,
                    "host speaks protocol %s, this plugin speaks %s" % (body["protocolVersion"], PROTOCOL_VERSION),
                )
            self.initialized = True
            return {
                "protocolVersion": PROTOCOL_VERSION,
                "serverInfo": {"name": self.provider.name, "version": self.provider.version},
                "capabilities": {"languages": self.provider.manifest()},
            }
        if not self.initialized:
            raise _Failure(NOT_INITIALIZED, "`%s` before `initialize`" % method)
        if method == "lighthouse/index":
            return self.provider.index(_require(params, ("project", "language", "inputs")))
        if method == "shutdown":
            self.shutdown = True
            return None
        raise _Failure(METHOD_NOT_FOUND, "unknown method %s" % method)

    def answer(self, message: dict[str, Any]) -> dict[str, Any] | None:
        request_id = message.get("id")
        method = message.get("method")
        if not isinstance(method, str):
            return None
        try:
            result = self._route(method, message.get("params"))
        except _Failure as failure:
            outcome: dict[str, Any] = {"error": {"code": failure.code, "message": failure.message}}
        except Exception as error:  # a bad request never ends the process
            outcome = {"error": {"code": INTERNAL_ERROR, "message": "panic: %s: %s" % (type(error).__name__, error)}}
        else:
            outcome = {"result": result}
        if request_id is None:
            return None
        return {"jsonrpc": "2.0", "id": request_id, **outcome}


def serve(provider: Provider, source: BinaryIO | None = None, sink: BinaryIO | None = None) -> int:
    """Serves a provider on stdin and stdout until `exit` or the end of the input; 0 after `shutdown`, 1 otherwise."""
    source = source if source is not None else sys.stdin.buffer
    sink = sink if sink is not None else sys.stdout.buffer
    session = _Session(provider)
    while True:
        try:
            message = read_message(source)
        except _FramingError as error:
            write_message(sink, {"jsonrpc": "2.0", "id": None, "error": {"code": PARSE_ERROR, "message": str(error)}})
            return 1
        if message is None or message.get("method") == "exit":
            return 0 if session.shutdown else 1
        reply = session.answer(message)
        if reply is not None:
            write_message(sink, reply)
