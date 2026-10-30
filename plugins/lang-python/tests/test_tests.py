import ast
import unittest

from lang_python.tests import body_statements, is_test_file, manual_assertions, role_of, test_case
from tests.support import model_of

SOURCE = """\
import unittest
import pytest

ROWS = [(1, 2), (3, 4)]


def check(value):
    assert value


def build():
    return 1


class Plain:
    pass


class TestOuter:
    def test_one(self):
        assert 1

    class TestInner:
        def test_two(self):
            assert 1


class Case(unittest.TestCase):
    def test_three(self):
        self.assertTrue(1)


@pytest.mark.parametrize("a", [1, 2])
def test_param(a):
    assert a


def test_rows():
    for a, b in ROWS:
        assert a < b


def test_plain():
    assert 1
"""


def decls(source: str = SOURCE, rel: str = "test_x.py"):
    model = model_of({rel: source})
    found = model.decls(rel)
    return {d.name: d for d in found.list}, model.file(rel).tree


class TestFileTests(unittest.TestCase):
    def test_is_test_file(self):
        for rel in (
            "test_a.py",
            "pkg/a_test.py",
            "conftest.py",
            "tests/a.py",
            "pkg/test/helpers.py",
            "pkg/tests/sub/a.py",
        ):
            self.assertTrue(is_test_file(rel), rel)
        for rel in ("a.py", "pkg/testing.py", "pkg/contest.py", "pkg/tests.py"):
            self.assertFalse(is_test_file(rel), rel)


class TestCaseTests(unittest.TestCase):
    def test_body_statements(self):
        node = ast.parse('def f():\n    """Docs."""\n    x = 1\n    return x\n').body[0]
        self.assertEqual([type(s).__name__ for s in body_statements(node)], ["Assign", "Return"])
        bare = ast.parse("def f():\n    pass\n").body[0]
        self.assertEqual(len(body_statements(bare)), 1)

    def test_manual_assertions(self):
        source = (
            "def test_a(x):\n"
            "    if x != 1:\n        raise AssertionError('x')\n"
            "    if not x:\n        fail('no')\n"
            "    if x > 1:\n        print(x)\n"
            "    if x == 2:\n        raise ValueError\n    else:\n        pass\n"
        )
        self.assertEqual(manual_assertions(ast.parse(source).body[0]), 2)

    def test_role_of(self):
        found, _ = decls()
        self.assertEqual(role_of(found["check"], True), "test-helper")
        self.assertEqual(role_of(found["build"], True), "fixture")
        self.assertEqual(role_of(found["ROWS"], True), "fixture")
        self.assertEqual(role_of(found["Plain"], True), "fixture")
        self.assertIsNone(role_of(found["Case"], True))
        self.assertIsNone(role_of(found["TestOuter"], True))
        self.assertIsNone(role_of(found["test_one"], True))
        self.assertIsNone(role_of(found["check"], False))

    def test_test_case(self):
        found, tree = decls()
        outer = test_case(found["test_one"], tree, {})
        self.assertEqual((outer["style"], outer["nesting"]), ("scenario", 1))
        self.assertEqual(test_case(found["test_two"], tree, {})["nesting"], 2)
        self.assertEqual(test_case(found["test_three"], tree, {})["nesting"], 1)
        self.assertEqual(test_case(found["test_plain"], tree, {})["nesting"], 0)
        self.assertEqual(test_case(found["test_param"], tree, {})["style"], "table")
        self.assertEqual(test_case(found["test_rows"], tree, {"m::f#function": None})["targets"], ["m::f#function"])
        self.assertEqual(test_case(found["test_rows"], tree, {})["style"], "table")


if __name__ == "__main__":
    unittest.main()
