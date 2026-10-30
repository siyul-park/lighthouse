import ast
import unittest

from lang_python.comments import Tokens
from lang_python.extent import Lines, before, doc_of, extent_of, position, span, span_for, span_of, start_of
from tests.support import model_of

SOURCE = (
    "# about f\n"
    "@decorate\n"
    "def f():\n"
    '    """Docs of f."""\n'
    "    return 1  # done\n"
    "\n"
    "\n"
    "LIMIT = 3\n"
    '"""The limit."""\n'
    "\n"
    "\n"
    "def g(): ...\n"
)


def declared(name: str, source: str = SOURCE):
    decls = model_of({"m.py": source}).decls("m.py")
    return next(d for d in decls.list if d.name == name), Tokens(source, Lines(source)), Lines(source)


class PositionTests(unittest.TestCase):
    def test_position(self):
        self.assertEqual(position(4, 7), {"line": 4, "col": 7})

    def test_span(self):
        self.assertEqual(span(1, 0, 2, 5), {"start": {"line": 1, "col": 1}, "end": {"line": 2, "col": 6}})

    def test_span_of(self):
        node = ast.parse("x = 1\nyy = 2\n").body[1]
        self.assertEqual(span_of(node), {"start": {"line": 2, "col": 1}, "end": {"line": 2, "col": 7}})

    def test_before(self):
        self.assertTrue(before({"line": 1, "col": 9}, {"line": 2, "col": 1}))
        self.assertTrue(before({"line": 2, "col": 1}, {"line": 2, "col": 2}))
        self.assertFalse(before({"line": 2, "col": 2}, {"line": 2, "col": 2}))


class LinesTests(unittest.TestCase):
    def test_lines_byte_col(self):
        lines = Lines("é = 1\nx\n")
        self.assertEqual(lines.byte_col(1, 1), 2)
        self.assertEqual(lines.byte_col(2, 1), 1)
        self.assertEqual(lines.byte_col(9, 4), 4)

    def test_lines_line_text(self):
        lines = Lines("a\nb")
        self.assertEqual([lines.line_text(n) for n in (0, 1, 2, 3)], ["", "a", "b", ""])


class DeclarationTests(unittest.TestCase):
    def test_start_of(self):
        decl, _, lines = declared("f")
        self.assertEqual(start_of(decl.node, lines), (2, 0))
        plain, _, lines = declared("LIMIT")
        self.assertEqual(start_of(plain.node, lines), (8, 0))

    def test_span_for(self):
        decl, _, lines = declared("f")
        self.assertEqual(span_for(decl, lines), span(2, 0, 5, 12))
        plain, _, lines = declared("LIMIT")
        self.assertEqual(span_for(plain, lines), span(8, 0, 8, 9))

    def test_extent_of(self):
        decl, tokens, lines = declared("f")
        # From the comment above the decorator to the comment that trails the last statement.
        self.assertEqual(extent_of(decl, tokens, lines), span(1, 0, 5, 20))
        limit, tokens, lines = declared("LIMIT")
        # The docstring after a variable belongs to its extent.
        self.assertEqual(extent_of(limit, tokens, lines), span(8, 0, 9, 16))
        self.assertEqual(extent_of(declared("g")[0], tokens, lines), span(12, 0, 12, 12))

    def test_extent_of_an_instance_field_is_none(self):
        source = "class A:\n    def __init__(self):\n        self.x = 1\n"
        decls = model_of({"m.py": source}).decls("m.py")
        field = next(d for d in decls.list if d.name == "x")
        self.assertIsNone(extent_of(field, Tokens(source, Lines(source)), Lines(source)))

    def test_doc_of(self):
        self.assertEqual(doc_of(declared("f")[0]), "Docs of f.")
        self.assertEqual(doc_of(declared("LIMIT")[0]), "The limit.")
        self.assertIsNone(doc_of(declared("g")[0]))


if __name__ == "__main__":
    unittest.main()
