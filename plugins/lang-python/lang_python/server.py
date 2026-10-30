"""The JSON-RPC loop of the plugin protocol: LSP framing on a byte stream, requests answered in order."""

import json
from typing import Any, BinaryIO, Protocol

PROTOCOL_VERSION = "0.4"

PARSE_ERROR = -32700
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
INTERNAL_ERROR = -32603
REQUEST_FAILED = -32803
NOT_INITIALIZED = -32002


class Handler(Protocol):
    """The methods of a language provider."""

    def initialize(self, params: dict[str, Any]) -> dict[str, Any]: ...

    def index(self, params: dict[str, Any]) -> dict[str, Any]: ...


class Failure(Exception):
    """A JSON-RPC error answer."""

    def __init__(self, code: int, message: str):
        super().__init__(message)
        self.code = code
        self.message = message


class FramingError(Exception):
    """Input that is not a framed JSON message."""


def read_message(stream: BinaryIO) -> dict[str, Any] | None:
    """The next message, or None at the end of the input."""
    length = None
    seen = False
    while True:
        line = stream.readline()
        if line == b"":
            if seen:
                raise FramingError("input ended inside a header")
            return None
        seen = True
        text = line.decode("utf-8", errors="replace").strip()
        if text == "":
            break
        name, _, value = text.partition(":")
        if name.strip().lower() == "content-length":
            try:
                length = int(value.strip())
            except ValueError as error:
                raise FramingError("bad Content-Length") from error
    if length is None or length < 0:
        raise FramingError("missing Content-Length")
    body = stream.read(length)
    if len(body) != length:
        raise FramingError("input ended inside a body")
    try:
        message = json.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as error:
        raise FramingError("invalid JSON: %s" % error) from error
    if not isinstance(message, dict):
        raise FramingError("a message must be an object")
    return message


def write_message(stream: BinaryIO, message: dict[str, Any]) -> None:
    body = json.dumps(message, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    stream.write(b"Content-Length: %d\r\n\r\n" % len(body))
    stream.write(body)
    stream.flush()


def require(params: Any, names: tuple[str, ...]) -> dict[str, Any]:
    """Refuses params that lack a required member, as the Rust server does."""
    if not isinstance(params, dict):
        raise Failure(INVALID_PARAMS, "params must be an object")
    for name in names:
        if name not in params:
            raise Failure(INVALID_PARAMS, "missing field `%s`" % name)
    return params


class Session:
    """What the loop knows of the conversation."""

    def __init__(self, handler: Handler):
        self.handler = handler
        self.initialized = False
        self.shutdown = False

    def route(self, method: str, params: Any) -> Any:
        if method == "initialize":
            body = require(params, ("protocolVersion", "clientInfo", "workspaceFolders"))
            if body["protocolVersion"] != PROTOCOL_VERSION:
                raise Failure(
                    REQUEST_FAILED,
                    "host speaks protocol %s, this plugin speaks %s" % (body["protocolVersion"], PROTOCOL_VERSION),
                )
            result = self.handler.initialize(body)
            self.initialized = True
            return result
        if not self.initialized:
            raise Failure(NOT_INITIALIZED, "`%s` before `initialize`" % method)
        if method == "lighthouse/index":
            return self.handler.index(require(params, ("project", "language", "inputs")))
        if method == "shutdown":
            self.shutdown = True
            return None
        raise Failure(METHOD_NOT_FOUND, "unknown method %s" % method)

    def answer(self, message: dict[str, Any]) -> dict[str, Any] | None:
        request_id = message.get("id")
        method = message.get("method")
        if not isinstance(method, str):
            return None
        try:
            result = self.route(method, message.get("params"))
        except Failure as failure:
            outcome: dict[str, Any] = {"error": {"code": failure.code, "message": failure.message}}
        except Exception as error:  # a bad request never ends the process
            outcome = {"error": {"code": INTERNAL_ERROR, "message": "panic: %s: %s" % (type(error).__name__, error)}}
        else:
            outcome = {"result": result}
        if request_id is None:
            return None
        return {"jsonrpc": "2.0", "id": request_id, **outcome}


def serve(source: BinaryIO, sink: BinaryIO, handler: Handler) -> int:
    """Serves until `exit` or the end of the input; 0 after `shutdown`, 1 otherwise."""
    session = Session(handler)
    while True:
        try:
            message = read_message(source)
        except FramingError as error:
            write_message(sink, {"jsonrpc": "2.0", "id": None, "error": {"code": PARSE_ERROR, "message": str(error)}})
            return 1
        if message is None or message.get("method") == "exit":
            return 0 if session.shutdown else 1
        reply = session.answer(message)
        if reply is not None:
            write_message(sink, reply)
