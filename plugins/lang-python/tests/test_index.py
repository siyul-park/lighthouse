import unittest

from lang_python.functions import Types
from lang_python.index import Unit
from lang_python.visibility import Reach
from tests.support import model_of

SOURCE = '''\
"""Module docs."""

from .gone import missing


class Shape:
    """A shape."""

    def area(self) -> int:
        return 0


def probe(thing):
    return thing.area()


def test_probe():
    assert probe(Shape()) == 0
'''


def unit(files: dict[str, str] | None = None, rel: str = "pkg/shape.py") -> Unit:
    model = model_of(files or {"pkg/__init__.py": "", rel: SOURCE})
    return Unit(model, Reach(model), Types(Reach(model)), model.file(rel))


class UnitTests(unittest.TestCase):
    def test_unit_edge(self):
        found = unit()
        source = {"symbol": "m::a#function"}
        found.edge("calls", source, "m::b#function", None, "semantic")
        found.edge("calls", source, "m::b#function", None, "semantic")
        found.edge("calls", source, "m::b#function", None, "syntactic")
        found.edge("imports", {"module": "m"}, "os", None, "semantic")
        self.assertEqual(
            [(e["kind"], e["to"], e["resolution"]) for e in found.edges],
            [("calls", "m::b#function", "semantic"), ("imports", "os", "semantic")],
        )
        self.assertEqual(found.targets, {"m::a#function": {"m::b#function": None}})

    def test_unit_index(self):
        indexed = unit().index()
        fragment = indexed.fragment
        self.assertEqual(fragment["file"], {"path": "pkg/shape.py"})
        self.assertEqual(fragment["modules"], [{"path": "pkg/shape"}])
        names = {s["name"]: s for s in fragment["symbols"]}
        self.assertEqual(names["Shape"]["doc"], "A shape.")
        self.assertEqual(names["area"]["owner"], names["Shape"]["id"])
        self.assertEqual(names["test_probe"]["kind"], "function")  # not a test file
        self.assertEqual(fragment["tests"], [])
        self.assertEqual(len(fragment["functions"]), 3)
        self.assertEqual((indexed.unresolved, indexed.guessed), (1, True))

    def test_unit_index_of_a_test_file_names_the_module_it_tests(self):
        files = {
            "pkg/__init__.py": "",
            "pkg/shape.py": "def f(): ...\n",
            "tests/test_shape.py": "def test_f():\n    assert 1\n",
        }
        fragment = unit(files, "tests/test_shape.py").index().fragment
        self.assertEqual(fragment["modules"], [{"path": "tests/test_shape", "test_of": "pkg/shape"}])
        self.assertEqual(fragment["symbols"][0]["visibility"], "private")

    def test_unit_index_marks_generated_files_and_attaches_comments(self):
        files = {"g.py": "# @generated\n# about f\ndef f(): ...\n"}
        fragment = unit(files, "g.py").index().fragment
        self.assertTrue(fragment["file"]["generated"])
        self.assertEqual(fragment["comments"][0]["attached_to"], "g::f#function")


if __name__ == "__main__":
    unittest.main()
