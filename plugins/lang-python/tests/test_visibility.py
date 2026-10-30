import unittest

from lang_python.visibility import Reach
from tests.support import model_of

FILES = {
    "pkg/__init__.py": "from ._impl.engine import Engine, _tuned\n__all__ = ['Engine']\n",
    "pkg/api.py": (
        "def shown(): ...\n\n\ndef _hidden(): ...\n\n\nclass _Local:\n    def method(self): ...\n\n\n"
        "class Open:\n    def act(self): ...\n\n    def _quiet(self): ...\n\n    def __repr__(self): ...\n"
    ),
    "pkg/listed.py": "__all__ = ['only']\n\n\ndef only(): ...\n\n\ndef other(): ...\n",
    "pkg/_impl/engine.py": "class Engine: ...\n\n\ndef _tuned(): ...\n\n\ndef plain(): ...\n",
    "tests/test_api.py": "def test_shown(): ...\n",
    "docs/.lighthouse-snippets/guide.md.1/snippet.py": "def demo(): ...\n",
}


def visibilities(rel: str) -> dict[str, str]:
    model = model_of(FILES)
    reach = Reach(model)
    return {d.id.split("::", 1)[1]: reach.visibility(d, rel) for d in model.decls(rel).list}


class ReachTests(unittest.TestCase):
    def test_reach_visibility(self):
        self.assertEqual(
            visibilities("pkg/api.py"),
            {
                "shown#function": "public",
                "_hidden#function": "private",
                "_Local#type": "private",
                "_Local::method#method": "internal",
                "Open#type": "public",
                "Open::act#method": "public",
                "Open::_quiet#method": "private",
                "Open::__repr__#method": "public",
            },
        )

    def test_reach_visibility_follows_a_static_all(self):
        self.assertEqual(visibilities("pkg/listed.py"), {"only#function": "public", "other#function": "private"})

    def test_reach_visibility_of_an_internal_module_and_its_reexports(self):
        found = visibilities("pkg/_impl/engine.py")
        self.assertEqual(found["Engine#type"], "internal")
        self.assertEqual(found["plain#function"], "internal")
        self.assertEqual(found["_tuned#function"], "private")

    def test_reach_visibility_in_tests_and_fences_is_private(self):
        self.assertEqual(visibilities("tests/test_api.py"), {"test_shown#test": "private"})
        self.assertEqual(visibilities("docs/.lighthouse-snippets/guide.md.1/snippet.py"), {"demo#function": "private"})


if __name__ == "__main__":
    unittest.main()
