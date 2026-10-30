import io
import json
import unittest
from typing import Any

from lang_python.provider import Provider
from lang_python.server import PROTOCOL_VERSION, read_message, serve, write_message


def frame(message: dict[str, Any]) -> bytes:
    body = json.dumps(message).encode()
    return b"Content-Length: %d\r\n\r\n%s" % (len(body), body)


def run(*messages: bytes) -> tuple[int, list[dict[str, Any]]]:
    source = io.BytesIO(b"".join(messages))
    sink = io.BytesIO()
    code = serve(source, sink, Provider("lang-python", "0"))
    replies = []
    out = io.BytesIO(sink.getvalue())
    while (message := read_message(out)) is not None:
        replies.append(message)
    return code, replies


INITIALIZE = {
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
        "protocolVersion": PROTOCOL_VERSION,
        "clientInfo": {"name": "t", "version": "0"},
        "workspaceFolders": [],
    },
}


class ServerTests(unittest.TestCase):
    def test_lifecycle_ends_with_zero_after_shutdown_and_exit(self):
        code, replies = run(
            frame(INITIALIZE),
            frame({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}),
            frame({"jsonrpc": "2.0", "method": "exit"}),
        )
        self.assertEqual(code, 0)
        self.assertEqual(replies[0]["result"]["serverInfo"]["name"], "lang-python")
        self.assertIsNone(replies[1]["result"])

    def test_input_that_closes_before_shutdown_is_an_unclean_stop(self):
        code, _ = run(frame(INITIALIZE))
        self.assertEqual(code, 1)

    def test_a_request_before_initialize_is_refused(self):
        _, replies = run(frame({"jsonrpc": "2.0", "id": 1, "method": "lighthouse/index", "params": {}}))
        self.assertEqual(replies[0]["error"]["code"], -32002)

    def test_unknown_method_and_invalid_params(self):
        _, replies = run(
            frame(INITIALIZE),
            frame({"jsonrpc": "2.0", "id": 2, "method": "nope"}),
            frame({"jsonrpc": "2.0", "id": 3, "method": "lighthouse/index", "params": {}}),
        )
        self.assertEqual(replies[1]["error"]["code"], -32601)
        self.assertEqual(replies[2]["error"]["code"], -32602)

    def test_another_protocol_version_fails_the_request(self):
        stale = json.loads(json.dumps(INITIALIZE))
        stale["params"]["protocolVersion"] = "9.9"
        _, replies = run(frame(stale))
        self.assertEqual(replies[0]["error"]["code"], -32803)

    def test_garbage_is_a_parse_error_that_ends_the_loop(self):
        code, replies = run(b"Content-Length: 2\r\n\r\n{]", frame(INITIALIZE))
        self.assertEqual(code, 1)
        self.assertEqual(len(replies), 1)
        self.assertEqual(replies[0]["error"]["code"], -32700)

    def test_headers_are_case_insensitive_and_extra_headers_ignored(self):
        body = json.dumps(INITIALIZE).encode()
        raw = b"content-LENGTH:   %d\r\nContent-Type: x\r\n\r\n%s" % (len(body), body)
        _, replies = run(raw)
        self.assertNotIn("error", replies[0])

    def test_a_failing_handler_is_an_internal_error_and_the_loop_goes_on(self):
        index = {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "lighthouse/index",
            "params": {"project": {"root": "/nonexistent"}, "language": "python", "inputs": [{"kind": "x"}]},
        }
        _, replies = run(frame(INITIALIZE), frame(index), frame({"jsonrpc": "2.0", "id": 3, "method": "shutdown"}))
        self.assertEqual(replies[1]["error"]["code"], -32603)
        self.assertIsNone(replies[2]["result"])

    def test_write_message_frames_utf8_by_bytes(self):
        sink = io.BytesIO()
        write_message(sink, {"text": "héllo"})
        head, _, body = sink.getvalue().partition(b"\r\n\r\n")
        self.assertEqual(int(head.split(b":")[1]), len(body))


if __name__ == "__main__":
    unittest.main()
