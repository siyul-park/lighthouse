import ast
import unittest

from lang_python.symbols import (
    FileDecls,
    Origin,
    bound_names,
    dotted,
    executable,
    has_decorator,
    is_docstring,
    is_dunder,
    walk_flat,
)

SOURCE = """\
class Shape:
    sides = 4

    def area(self):
        return 0

    @property
    def name(self):
        return "shape"

    @property
    def side(self):
        return 1

    @side.setter
    def side(self, value):
        pass
"""


def file_decls(source: str = SOURCE) -> FileDecls:
    return FileDecls("m.py", "m", ast.parse(source), source, False)


def statement(source: str) -> ast.stmt:
    return ast.parse(source).body[0]


class FileDeclsTests(unittest.TestCase):
    def test_file_decls_add(self):
        decls = file_decls("")
        node = ast.parse("def f(): ...\n").body[0]
        first = decls.add("function", "f", None, Origin(node, node, node))
        again = decls.add("function", "f", None, Origin(node, node, node))
        self.assertIs(first, again)
        self.assertEqual(first.id, "m::f#function")
        self.assertEqual(len(first.nodes), 2)
        self.assertEqual(list(decls.top), ["f"])
        owned = decls.add("method", "g", first, Origin(node, node))
        self.assertEqual(owned.id, "m::f::g#method")

    def test_file_decls_add_escapes_what_the_id_format_reserves(self):
        decls = file_decls("")
        node = ast.parse("x = 1\n").body[0]
        self.assertEqual(decls.add("var", "a#b::c", None, Origin(node, node)).id, "m::a%23b:%3Ac#var")

    def test_file_decls_member(self):
        decls = file_decls()
        shape = decls.top["Shape"]
        self.assertEqual(decls.member(shape, "area").kind, "method")
        self.assertEqual(decls.member(shape, "sides").kind, "field")
        self.assertIsNone(decls.member(shape, "missing"))

    def test_file_decls_of(self):
        decls = file_decls()
        shape = decls.top["Shape"]
        self.assertIs(decls.of(shape.node), shape)
        side = decls.member(shape, "side")
        self.assertEqual(len(side.nodes), 2)
        self.assertTrue(all(decls.of(n) is side for n in side.nodes))
        self.assertIsNone(decls.of(ast.parse("x\n").body[0]))


class ExpressionTests(unittest.TestCase):
    def test_dotted(self):
        expr = ast.parse("a.b.c", mode="eval").body
        self.assertEqual(dotted(expr), "a.b.c")
        self.assertIsNone(dotted(ast.parse("f().b", mode="eval").body))

    def test_is_docstring(self):
        self.assertTrue(is_docstring(statement('"""Docs."""')))
        self.assertFalse(is_docstring(statement("1")))
        self.assertFalse(is_docstring(statement("x = 'a'")))

    def test_has_decorator(self):
        node = statement("@a.b.cached(1)\n@plain\ndef f(): ...\n")
        self.assertTrue(has_decorator(node, "cached"))
        self.assertTrue(has_decorator(node, "plain"))
        self.assertFalse(has_decorator(node, "b"))
        self.assertFalse(has_decorator(statement("def g(): ...\n"), "plain"))

    def test_walk_flat(self):
        body = ast.parse(
            "a = 1\nif a:\n    b = 2\nelse:\n    c = 3\ntry:\n    d = 4\nexcept E:\n    e = 5\nfinally:\n    f = 6\n"
            "with w:\n    g = 7\nmatch a:\n    case 1:\n        h = 8\ndef inner():\n    hidden = 9\n"
        ).body
        names = [n.id for s in walk_flat(body) if isinstance(s, ast.Assign) for n in s.targets]
        self.assertEqual(names, list("abcdefgh"))

    def test_bound_names(self):
        target = ast.parse("a, (b, *c) = x", mode="exec").body[0].targets[0]
        self.assertEqual([n.id for n in bound_names(target)], ["a", "b", "c"])
        self.assertEqual(bound_names(ast.parse("o.x = 1").body[0].targets[0]), [])

    def test_is_dunder(self):
        self.assertTrue(is_dunder("__init__"))
        self.assertFalse(is_dunder("__private"))
        self.assertFalse(is_dunder("____"))
        self.assertFalse(is_dunder("_x_"))


class ExecutableTests(unittest.TestCase):
    def test_executable(self):
        sources = [
            "run()",
            "x += 1",
            "raise E",
            "assert x",
            "del x",
            "if f():\n    pass",
            "for a in g():\n    pass",
            "with open(p):\n    pass",
            "if a:\n    run()",
            "try:\n    run()\nexcept E:\n    pass",
            "match f():\n    case 1:\n        pass",
        ]
        for source in sources:
            self.assertTrue(executable(statement(source)), source)

    def test_executable_leaves_declarations_alone(self):
        sources = [
            '"""Docs."""',
            "...",
            "x = 1",
            "x: int = 2",
            "if a:\n    y = 1",
            "for a in b:\n    pass",
            "try:\n    z = 1\nexcept E:\n    pass",
            "def f(): ...",
            "import os",
        ]
        for source in sources:
            self.assertFalse(executable(statement(source)), source)


if __name__ == "__main__":
    unittest.main()
