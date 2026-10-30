import unittest

from tests.support import fragment, index

SOURCE = """\
import ast
import logging
import logging.handlers
from logging import Handler
from typing import Protocol


class Base:
    def run(self): ...


class Child(Base):
    def run(self): ...

    def fresh(self): ...


class Visitor(ast.NodeVisitor):
    def visit(self, node): ...

    def mine(self): ...


class Emitter(Handler):
    def emit(self, record): ...


class Custom(logging.Handler):
    def flush(self): ...

    def other(self): ...


class Rotating(logging.handlers.RotatingFileHandler):
    def _open(self): ...


class Plain:
    def __repr__(self): return "p"

    def __call__(self): ...

    def helper(self): ...


class Reads(Protocol):
    def read(self) -> bytes: ...


class Reader:
    def read(self) -> bytes: ...

    def close(self): ...


class Writer:
    def write(self): ...


def __dir__():
    return []


def __getattr__(name):
    raise AttributeError(name)


def helper():
    return 1
"""


def _key(symbol: str, name: str) -> str:
    """`name@owner`, the owner being the name itself for a function of the module."""
    parts = symbol.split("::")
    return name + "@" + (parts[-2] if len(parts) > 2 else name)


def implementations() -> dict[str, bool]:
    result = index({"m.py": SOURCE})
    names = {s["id"]: s["name"] for s in fragment(result, "m.py")["symbols"]}
    return {
        _key(f["symbol"], names[f["symbol"]]): bool(f.get("implementation"))
        for f in fragment(result, "m.py")["functions"]
        if f["symbol"] in names
    }


class ImplementationTest(unittest.TestCase):
    def test_overrides_and_protocols_are_implementations(self):
        found = implementations()
        expected = {
            "run@Base": False,
            "run@Child": True,
            "fresh@Child": False,
            "visit@Visitor": True,
            "mine@Visitor": False,
            "emit@Emitter": True,
            "_open@Rotating": True,
            "flush@Custom": True,
            "other@Custom": False,
            "__repr__@Plain": True,
            "__call__@Plain": True,
            "helper@Plain": False,
            "read@Reader": True,
            "close@Reader": False,
            "write@Writer": False,
        }
        for key, want in expected.items():
            self.assertEqual(found.get(key), want, key)

    def test_module_hooks_are_implementations(self):
        found = implementations()
        self.assertTrue(found["__dir__@__dir__"])
        self.assertTrue(found["__getattr__@__getattr__"])
        self.assertFalse(found["helper@helper"])


if __name__ == "__main__":
    unittest.main()
