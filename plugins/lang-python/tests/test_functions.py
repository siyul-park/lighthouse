import ast
import unittest

from lang_python.functions import Functions, Types, parameters, returns
from lang_python.project import resolver_of
from lang_python.visibility import Reach
from tests.support import model_of

SOURCE = '''\
from dataclasses import dataclass
from typing import Optional, Self


class Store:
    """Holds things."""

    def __init__(self, name: str):
        self.name = name

    def fetch(self, key: str, *rest: int, flag: bool = False, **extra) -> Optional["Store"]:
        return self.lookup(key, *rest, flag=flag, **extra)

    def lookup(self, key: str, *rest: int, flag: bool = False, **extra) -> "Store":
        return self

    @classmethod
    def make(cls) -> Self:
        return cls("x")

    @staticmethod
    def helper(a, b):
        return a + b


@dataclass
class Row:
    ident: int
    label: Optional[str] = None
    count: int = 0
    note: str = "n"


def load(path: str) -> Store:
    return Store(path)


def forward(path: str) -> Store:
    return load(path)


def fact(n: int) -> int:
    return 1 if n < 2 else n * fact(n - 1)


def nothing() -> None:
    print("x")


def untyped(a, b):
    yield a
'''


class World:
    def __init__(self):
        self.model = model_of({"m.py": SOURCE})
        self.parsed = self.model.file("m.py")
        self.decls = {d.id.split("::", 1)[1]: d for d in self.model.decls("m.py").list}
        self.resolver = resolver_of(self.model, self.parsed)
        self.scope = self.module_scope = self.parsed.scopes().module(self.parsed.tree)
        self.types = Types(Reach(self.model))
        self.functions = Functions(self.model, self.parsed, self.types)

    def node(self, key: str):
        return self.decls[key].body

    def scope_of(self, key: str):
        """The scope of a def, entered through the class around it."""
        decl = self.decls[key]
        scope = self.module_scope
        if decl.owner is not None:
            scope = self.parsed.scopes().enter(decl.owner.node, scope)
        return self.parsed.scopes().enter(decl.body, scope)


class ParametersTests(unittest.TestCase):
    def test_parameters(self):
        found = parameters(World().node("Store::fetch#method"))
        self.assertEqual([p.arg for p in found], ["self", "key", "rest", "flag", "extra"])
        self.assertEqual(
            [p.arg for p in parameters(ast.parse("lambda a, /, b, *, c: 0", mode="eval").body)], list("abc")
        )

    def test_returns(self):
        world = World()
        self.assertEqual(returns(world.node("load#function")), 1)
        self.assertEqual(returns(world.node("nothing#function")), 0)
        self.assertEqual(returns(world.node("untyped#function")), 1)
        self.assertEqual(returns(ast.parse("def f():\n    return None\n").body[0]), 0)
        self.assertEqual(returns(ast.parse("lambda: 1", mode="eval").body), 1)


class TypesTests(unittest.TestCase):
    def test_types_field(self):
        world = World()
        field = lambda name: world.types.field(world.decls[f"Row::{name}#field"], world.resolver, world.scope)  # noqa: E731
        self.assertEqual(field("ident")[0]["text"], "int")
        self.assertFalse(field("ident")[1])
        self.assertTrue(field("label")[1])
        self.assertTrue(field("count")[1])
        self.assertIsNone(world.types.field(world.decls["Store::name#field"], world.resolver, world.scope))

    def test_types_signature(self):
        world = World()
        signature = world.types.signature(world.decls["load#function"], world.resolver, world.scope)
        self.assertEqual([p["text"] for p in signature["params"]], ["str"])
        self.assertEqual(signature["results"][0]["text"], "Store")
        self.assertEqual(signature["results"][0]["symbol"], "m::Store")
        self.assertTrue(signature["results"][0]["exported"])
        method = world.types.signature(world.decls["Store::fetch#method"], world.resolver, world.scope)
        self.assertEqual(len(method["params"]), 4)
        self.assertIsNone(world.types.signature(world.decls["nothing#function"], world.resolver, world.scope))
        untyped = world.types.signature(world.decls["untyped#function"], world.resolver, world.scope)
        self.assertEqual([p["text"] for p in untyped["params"]], ["Any", "Any"])

    def test_types_constructs(self):
        world = World()
        built = {
            key: world.types.constructs(world.decls[key], world.resolver, world.scope)
            for key in (
                "Store::__init__#method",
                "load#function",
                "Store::make#method",
                "Store::lookup#method",
                "nothing#function",
                "Store::helper#method",
                "fact#function",
            )  # fmt: skip
        }
        self.assertEqual(
            built,
            {
                "Store::__init__#method": True,
                "load#function": True,
                "Store::make#method": True,
                "Store::lookup#method": False,
                "nothing#function": False,
                "Store::helper#method": False,
                "fact#function": False,
            },
        )


class FunctionsTests(unittest.TestCase):
    def test_functions_summary(self):
        world = World()
        summary = world.functions.summary(world.decls["fact#function"], world.scope)
        self.assertEqual(
            (summary["params"], summary["returns"], summary["statements"], summary["top_level"]), (1, 1, 1, 1)
        )
        self.assertEqual([f["kind"] for f in summary["flow"]], ["if", "recursion"])
        self.assertEqual(summary["signature"]["params"][0]["text"], "int")

    def test_functions_summary_of_a_method_leaves_the_receiver_out(self):
        world = World()
        summary = world.functions.summary(world.decls["Store::lookup#method"], world.scope)
        self.assertEqual(summary["params"], 4)
        static = world.functions.summary(world.decls["Store::helper#method"], world.scope)
        self.assertEqual(static["params"], 2)

    def test_functions_summary_says_what_a_body_forwards_to(self):
        world = World()
        self.assertEqual(
            world.functions.summary(world.decls["forward#function"], world.scope)["forwards_to"], "m::load#function"
        )
        self.assertEqual(
            world.functions.summary(world.decls["Store::fetch#method"], world.scope_of("Store::fetch#method"))[
                "forwards_to"
            ],
            "m::Store::lookup#method",
        )
        self.assertEqual(
            world.functions.summary(world.decls["load#function"], world.scope)["forwards_to"], "m::Store#type"
        )
        self.assertNotIn("forwards_to", world.functions.summary(world.decls["fact#function"], world.scope))
        self.assertTrue(world.functions.summary(world.decls["load#function"], world.scope)["constructs"])


if __name__ == "__main__":
    unittest.main()
